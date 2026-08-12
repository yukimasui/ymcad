//! コンポーネント（ブロックの再定義）。
//!
//! 名前を付けた**定義**を 1 つ作り、それを**インスタンス**として何度でも配置する。
//! 定義を編集すると全インスタンスが追従する。
//!
//! # なぜ「ブロック」ではなく「コンポーネント」か
//!
//! 定義 + インスタンス（インスタンシング）という核は古くない。むしろ現代のツールが
//! 揃って同じ形に収束している（Figma の Component / ゲームエンジンの Prefab /
//! SVG の `<use>` / USD の Reference / React の Component）。**ここは残す。**
//!
//! 古いのは 1990〜2000 年代の実装の選択で、いちばん痛いのが
//! **ダイナミックブロックの「アクション」= GUI によるマクロ記録**。
//! 作るのが難しく、デバッグがもっと難しく、読めない。
//! これを**型付きパラメータ + テキストの式**に置き換えるのが今回の主眼
//! （詳細は `docs/DECISIONS.md` の ADR-0028）。
//!
//! # 定義は「ふつうのエンティティ + 疎な式の束縛」
//!
//! [`Definition::entities`] は**ふつうの [`Entity`]** で、式は入っていない。
//! 段階 2 で足す「束縛」が式を持ち、**パラメトリックにしたい座標だけ**を疎に上書きする。
//!
//! `Geometry` を式化した並行型階層（`ParamLine` / `ParamCircle` …）にしなかったのは、
//! **既存の作図・編集ツールが定義の中で使えなくなる**から。
//! LINE / CIRCLE / ARC / TRIM / EXTEND / FILLET / CHAMFER / レイヤ機能を
//! 全部作り直すことになる。
//!
//! 束縛が無ければクラシックなブロックに degrade するので、
//! パラメータ無しの段階とパラメータありの段階が自然に繋がる。
//!
//! # 妥当性はコマンド境界で保証する
//!
//! **`Document` に入っている `Definition` は常に妥当。**
//! 入れ子の循環検出（そして将来は式の型検査とパラメータ間の循環検出）は
//! すべて [`Command`](crate::Command) の `execute` で行う。
//!
//! そのおかげで [`resolve`] が infallible になり、`bbox()` が `Result` にならない。
//! [`Xline::new`](crate::geom::Xline::new) が零ベクトルを弾くのと同じ流儀で、
//! **境界で検証し、内側は不変条件を信じる**。
//!
//! # 倍率は一様のみ
//!
//! AutoCAD の `INSERT` は X/Y 別倍率を持つが、あれは**円を壊す失敗機能**。
//! 一様倍率なら既存の 5 変種が変換で閉じ、`Ellipse` / `EllipticArc` が要らない。

use std::collections::BTreeMap;

use crate::entity::{Entity, Geometry};
use crate::error::{CadError, Result};
use crate::geom::tolerance::is_zero_len;
use crate::geom::{Aabb, Line, Point2};

/// 入れ子の深さの上限。
///
/// 循環はコマンドが弾くので通常ここには到達しない。**万一到達したら打ち切る**ための
/// 最後の砦で、無限再帰でスタックを溢れさせないためにある。
pub const MAX_NESTING_DEPTH: usize = 16;

/// 定義の識別子。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DefinitionId(u32);

impl DefinitionId {
    /// 内部表現。
    #[must_use]
    pub const fn index(self) -> u32 {
        self.0
    }
}

/// インスタンスの配置。
///
/// **倍率は一様（`f64` 1 つ）。** モジュールドキュメントの理由を参照。
///
/// # なぜ反転フラグが必要か
///
/// 2 次元の相似変換は「平行移動 ∘ 回転 ∘ **反射(任意)** ∘ 一様倍率」に分解される。
/// **反射は (基点・回転・正の倍率) の 3 つでは表現できない**（行列式の符号が違う）。
/// 反転フラグを持たないと `MIRROR` がインスタンスに対して何もできなくなる。
///
/// AutoCAD は負の倍率でこれを表しているが、負の倍率は「倍率」と「反転」の 2 つの
/// 意味を 1 つの数に詰め込むので、`SCALE` の入力検証（0 と負値を拒否）と衝突する。
/// **別のフラグに分けるほうが素直。**
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// 定義の基点がここへ来る。
    pub origin: Point2,
    /// 回転角 [rad]。反時計回り。
    pub rotation: f64,
    /// 一様倍率。0 と負値は [`Placement::new`] が拒否する。
    pub scale: f64,
    /// 鏡像反転しているか。`MIRROR` のたびに反転する。
    pub flipped: bool,
}

impl Placement {
    /// 配置を作る。
    ///
    /// # Errors
    ///
    /// 倍率が 0 / 負 / 非有限の場合、回転角が非有限の場合
    /// [`CadError::DegenerateGeometry`]。
    ///
    /// 倍率 0 は図形を点に潰し、負値は鏡像になってしまう
    /// （反転したいなら `MIRROR` を使う）。`SCALE` コマンドと同じ約束。
    pub fn new(origin: Point2, rotation: f64, scale: f64, flipped: bool) -> Result<Self> {
        if !origin.x.is_finite() || !origin.y.is_finite() {
            return Err(CadError::DegenerateGeometry(
                "配置の基点が有限ではありません",
            ));
        }
        if !rotation.is_finite() {
            return Err(CadError::DegenerateGeometry(
                "配置の回転角が有限ではありません",
            ));
        }
        if !scale.is_finite() || scale <= 0.0 || is_zero_len(scale) {
            return Err(CadError::DegenerateGeometry(
                "配置の倍率は 0 より大きい有限の値でなければなりません",
            ));
        }
        Ok(Self {
            origin,
            rotation,
            scale,
            flipped,
        })
    }

    /// 恒等に近い配置（無回転・等倍・非反転）。
    #[must_use]
    pub fn at(origin: Point2) -> Self {
        Self {
            origin,
            rotation: 0.0,
            scale: 1.0,
            flipped: false,
        }
    }
}

/// パラメータの値。
///
/// 段階 2 の式で使う。段階 1 では `overrides` が常に空なので出番が無いが、
/// **永続化の形を先に決めておく**ために置いてある（後から足すと形式変更になる）。
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// 数値。
    Number(f64),
    /// 真偽。
    Bool(bool),
    /// 選択肢（`ParamType::Choice` の候補のいずれか）。
    Choice(String),
}

/// 配置されたコンポーネント。
#[derive(Clone, Debug, PartialEq)]
pub struct Instance {
    /// 参照する定義。
    pub definition: DefinitionId,
    /// 配置。
    pub placement: Placement,
    /// パラメータの個別上書き。
    ///
    /// **無い項目は定義の既定値を継承する。** これが Figma のインスタンスと同じ挙動で、
    /// AutoCAD の「定義を編集（全部変わる）か EXPLODE（リンクが切れる）」の
    /// 二択を解消する部分。項目を消せば既定値に戻る（リセット）。
    pub overrides: BTreeMap<String, Value>,
}

impl Instance {
    /// 上書き無しのインスタンス。
    #[must_use]
    pub fn new(definition: DefinitionId, placement: Placement) -> Self {
        Self {
            definition,
            placement,
            overrides: BTreeMap::new(),
        }
    }

    /// 配置を移した複製。
    #[must_use]
    pub fn with_placement(&self, placement: Placement) -> Self {
        Self {
            definition: self.definition,
            placement,
            overrides: self.overrides.clone(),
        }
    }
}

/// コンポーネントの定義。
///
/// 段階 2 でパラメータ宣言と式の束縛が加わる。中身が**ふつうの [`Entity`]** で
/// あることは変わらない（モジュールドキュメントの理由を参照）。
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Definition {
    /// 定義名。図面内で一意。
    pub name: String,
    /// 基点。挿入時にここが指定点へ来る。
    pub origin: Point2,
    /// 中身。**ふつうの [`Entity`]** で、式は入っていない。
    pub entities: Vec<Entity>,
}

impl Definition {
    /// 名前・基点・中身から作る。
    #[must_use]
    pub fn new(name: impl Into<String>, origin: Point2, entities: Vec<Entity>) -> Self {
        Self {
            name: name.into(),
            origin,
            entities,
        }
    }
}

/// 定義の一覧。
///
/// [`LayerTable`](crate::LayerTable) / [`GroupTable`](crate::GroupTable) と同じ作り。
/// 変更系は `pub(crate)` に絞り、[`EditCtx`](crate::command::EditCtx) 経由でしか触れない。
#[derive(Clone, Debug, Default)]
pub struct DefinitionTable {
    defs: Vec<Option<Definition>>,
    by_name: BTreeMap<String, DefinitionId>,
}

impl DefinitionTable {
    /// 空の表。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    // ---- 参照系 -----------------------------------------------------------

    /// ID から定義を引く。
    #[must_use]
    pub fn get(&self, id: DefinitionId) -> Option<&Definition> {
        self.defs.get(id.index() as usize)?.as_ref()
    }

    /// 名前から ID を引く。
    #[must_use]
    pub fn by_name(&self, name: &str) -> Option<DefinitionId> {
        self.by_name.get(name).copied()
    }

    /// ID 昇順で走査する。
    pub fn iter(&self) -> impl Iterator<Item = (DefinitionId, &Definition)> + '_ {
        self.defs.iter().enumerate().filter_map(|(i, d)| {
            let def = d.as_ref()?;
            let index = u32::try_from(i).ok()?;
            Some((DefinitionId(index), def))
        })
    }

    /// 定義数。
    #[must_use]
    pub fn len(&self) -> usize {
        self.defs.iter().filter(|d| d.is_some()).count()
    }

    /// 定義が 1 つも無いか。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// まだ使われていない定義名を作る（`コンポーネント1`, `コンポーネント2`, …）。
    #[must_use]
    pub fn next_default_name(&self) -> String {
        let mut n = 1u32;
        loop {
            let name = format!("コンポーネント{n}");
            if !self.by_name.contains_key(&name) {
                return name;
            }
            n += 1;
        }
    }

    // ---- 変更系（`pub(crate)` — EditCtx からのみ到達可能） ----------------

    /// 定義を追加する。同名があればその ID を返す。
    pub(crate) fn insert(&mut self, def: Definition) -> DefinitionId {
        if let Some(existing) = self.by_name.get(&def.name) {
            return *existing;
        }
        let index = u32::try_from(self.defs.len()).expect("定義数が u32 を超えました");
        let id = DefinitionId(index);
        self.by_name.insert(def.name.clone(), id);
        self.defs.push(Some(def));
        id
    }

    /// 定義を取り除く。
    ///
    /// # Errors
    ///
    /// 存在しない ID の場合 [`CadError::DefinitionNotFound`]。
    pub(crate) fn remove(&mut self, id: DefinitionId) -> Result<Definition> {
        let slot = self
            .defs
            .get_mut(id.index() as usize)
            .ok_or(CadError::DefinitionNotFound)?;
        let def = slot.take().ok_or(CadError::DefinitionNotFound)?;
        self.by_name.remove(&def.name);
        Ok(def)
    }

    /// 取り除いた定義を **元の ID のまま** 戻す。Undo で使う。
    ///
    /// # Errors
    ///
    /// スロットが埋まっている場合 [`CadError::SlotOccupied`]。
    pub(crate) fn restore(&mut self, id: DefinitionId, def: Definition) -> Result<()> {
        let index = id.index() as usize;
        if index >= self.defs.len() {
            self.defs.resize(index + 1, None);
        }
        if self.defs[index].is_some() {
            return Err(CadError::SlotOccupied);
        }
        self.by_name.insert(def.name.clone(), id);
        self.defs[index] = Some(def);
        Ok(())
    }

    /// 定義の中身を差し替える。差し替え前の中身を返す。
    ///
    /// これが「定義を編集すると全インスタンスが追従する」の実体。
    /// インスタンス側は定義を ID で参照しているだけなので、ここを変えるだけで済む。
    ///
    /// # Errors
    ///
    /// 存在しない ID の場合 [`CadError::DefinitionNotFound`]。
    pub(crate) fn replace_contents(
        &mut self,
        id: DefinitionId,
        origin: Point2,
        entities: Vec<Entity>,
    ) -> Result<(Point2, Vec<Entity>)> {
        let def = self
            .defs
            .get_mut(id.index() as usize)
            .and_then(|d| d.as_mut())
            .ok_or(CadError::DefinitionNotFound)?;
        let old_origin = std::mem::replace(&mut def.origin, origin);
        let old_entities = std::mem::replace(&mut def.entities, entities);
        Ok((old_origin, old_entities))
    }

    /// 定義名を変更する。古い名前を返す。
    ///
    /// 衝突の検査は呼び出し側（コマンド）の責務。`LayerTable` と同じ約束。
    pub(crate) fn rename(
        &mut self,
        id: DefinitionId,
        new_name: impl Into<String>,
    ) -> Option<String> {
        let new_name = new_name.into();
        let def = self.defs.get_mut(id.index() as usize)?.as_mut()?;
        let old = std::mem::replace(&mut def.name, new_name.clone());
        self.by_name.remove(&old);
        self.by_name.insert(new_name, id);
        Some(old)
    }
}

// ---------------------------------------------------------------------------
// 解決（インスタンス → ワールド座標の図形列）
// ---------------------------------------------------------------------------

/// インスタンスをワールド座標の図形列へ展開する。
///
/// **失敗しない。** 定義が見つからない・入れ子が深すぎる場合は空を返す
/// （`Document` に入っている定義は常に妥当なので、通常は起こらない）。
///
/// 結果は毎フレーム使うには重いので、`cad-app` 側で
/// `Document::revision()` をキーにキャッシュする（ADR-0011）。
#[must_use]
pub fn resolve(inst: &Instance, defs: &DefinitionTable) -> Vec<Geometry> {
    let mut out = Vec::new();
    resolve_into(inst, defs, 0, &mut out);
    out
}

/// [`resolve`] の本体。入れ子のために深さを持ち回る。
fn resolve_into(inst: &Instance, defs: &DefinitionTable, depth: usize, out: &mut Vec<Geometry>) {
    if depth >= MAX_NESTING_DEPTH {
        return;
    }
    let Some(def) = defs.get(inst.definition) else {
        return;
    };

    for entity in &def.entities {
        match &entity.geom {
            // 入れ子。内側のインスタンスを先に展開し、その結果に外側の配置をかける。
            Geometry::Instance(inner) => {
                let start = out.len();
                resolve_into(inner, defs, depth + 1, out);
                for g in &mut out[start..] {
                    *g = place(g, def.origin, inst.placement);
                }
            }
            g => out.push(place(g, def.origin, inst.placement)),
        }
    }
}

/// 定義座標の図形をワールド座標へ移す。
///
/// **順序が重要。**
///
/// 1. 基点を原点へ寄せる
/// 2. 反転（原点を通る水平線での鏡像）
/// 3. 倍率
/// 4. 回転
/// 5. 配置先へ移す
///
/// 反転を回転より**後**にすると別の変換になる（反射と回転は交換しない）。
/// 倍率は一様なので反転・回転のどちらと入れ替えても同じだが、
/// 基点合わせを最初に、平行移動を最後にしないと中心がずれる。
///
/// 反転に `Geometry::mirrored` を使うのは意図的で、
/// **円弧の開始角・終了角の入れ替え（ADR-0020）を再実装しないため**。
fn place(geom: &Geometry, def_origin: Point2, p: Placement) -> Geometry {
    let centered = geom.translated(Point2::ORIGIN - def_origin);
    let flipped = if p.flipped {
        centered.mirrored(&X_AXIS)
    } else {
        centered
    };
    // 拡大縮小と回転はどちらも原点まわり。
    let scaled = flipped.scaled(Point2::ORIGIN, p.scale);
    let rotated = scaled.rotated(Point2::ORIGIN, p.rotation);
    rotated.translated(p.origin - Point2::ORIGIN)
}

/// 反転に使う、原点を通る水平線。
const X_AXIS: Line = Line {
    a: Point2 { x: 0.0, y: 0.0 },
    b: Point2 { x: 1.0, y: 0.0 },
};

/// インスタンスの境界ボックス。
///
/// 中身に作図線があれば [`Aabb::UNBOUNDED`] になる。
#[must_use]
pub fn instance_bbox(inst: &Instance, defs: &DefinitionTable) -> Aabb {
    resolve(inst, defs)
        .iter()
        .fold(Aabb::EMPTY, |acc, g| acc.union(g.bbox(defs)))
}

/// インスタンスと点の最短距離。
///
/// 中身が空なら [`f64::INFINITY`]（ピックに当たらない）。
#[must_use]
pub fn instance_dist_to(inst: &Instance, defs: &DefinitionTable, p: Point2) -> f64 {
    resolve(inst, defs)
        .iter()
        .map(|g| g.dist_to(defs, p))
        .fold(f64::INFINITY, f64::min)
}

/// インスタンスが有界か。中身に作図線が 1 つでもあれば `false`。
#[must_use]
pub fn instance_is_bounded(inst: &Instance, defs: &DefinitionTable) -> bool {
    resolve(inst, defs).iter().all(|g| g.is_bounded(defs))
}

/// `inst` を `target` の定義の中に置くと循環するか。
///
/// **コマンドが挿入前に必ず呼ぶ。** 循環した定義は [`resolve`] を無限再帰させる。
/// `MAX_NESTING_DEPTH` は最後の砦であって、これの代わりにはならない
/// （深さで打ち切ると図形が黙って消える）。
#[must_use]
pub fn would_create_cycle(
    target: DefinitionId,
    inserted: DefinitionId,
    defs: &DefinitionTable,
) -> bool {
    if target == inserted {
        return true;
    }
    // `inserted` の中から `target` へ到達できたら循環する。
    let mut stack = vec![inserted];
    let mut seen = std::collections::BTreeSet::new();
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        if id == target {
            return true;
        }
        if let Some(def) = defs.get(id) {
            for e in &def.entities {
                if let Geometry::Instance(i) = &e.geom {
                    stack.push(i.definition);
                }
            }
        }
    }
    false
}

/// 定義の中で参照されている定義 ID を列挙する（重複あり）。
#[must_use]
pub fn referenced_definitions(def: &Definition) -> Vec<DefinitionId> {
    def.entities
        .iter()
        .filter_map(|e| match &e.geom {
            Geometry::Instance(i) => Some(i.definition),
            _ => None,
        })
        .collect()
}
