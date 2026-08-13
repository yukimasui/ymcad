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

pub mod binding;

pub use binding::{Binding, ParamDecl, Slot};

use std::collections::BTreeMap;

use crate::entity::{Entity, Geometry};
use crate::error::{CadError, Result};
use crate::expr::{eval, Env};
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

pub use crate::expr::Value;

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
    /// パラメータの宣言。**宣言順がパネルの表示順**になる。
    pub params: Vec<ParamDecl>,
    /// 中身。**ふつうの [`Entity`]** で、式は入っていない。
    pub entities: Vec<Entity>,
    /// 座標への式の束縛。**パラメトリックにしたい座標だけ**を疎に上書きする。
    ///
    /// `entity` は [`Self::entities`] への添字。**中身と必ず一緒に持ち替える**
    /// （[`binding`] のモジュールドキュメントを参照）。
    pub bindings: Vec<Binding>,
}

impl Definition {
    /// 名前・基点・中身から作る。パラメータと束縛は空。
    #[must_use]
    pub fn new(name: impl Into<String>, origin: Point2, entities: Vec<Entity>) -> Self {
        Self {
            name: name.into(),
            origin,
            params: Vec::new(),
            entities,
            bindings: Vec::new(),
        }
    }

    /// 名前でパラメータの宣言を引く。
    #[must_use]
    pub fn param(&self, name: &str) -> Option<&ParamDecl> {
        self.params.iter().find(|p| p.name == name)
    }

    /// パラメータの値を決める。
    ///
    /// **上書きが優先され、無い項目は既定値の式を評価する。**
    /// これが Figma のインスタンスと同じ継承。項目を消せば既定値へ戻る。
    ///
    /// 既定値は他のパラメータを参照してよいので、**依存の順に評価する**。
    /// 循環はコマンドが弾くが、万一残っていても無限ループしないよう
    /// 「1 周して 1 つも解決しなければ打ち切る」形にしてある。
    ///
    /// 型に合わない上書きや評価に失敗した既定値は**捨てる**。
    /// その名前は環境に入らないので、参照している式が失敗し、
    /// その座標は定義のままになる（図形は消えない）。
    #[must_use]
    pub fn param_env(&self, overrides: &BTreeMap<String, Value>) -> Env {
        let mut env = Env::new();

        // 1. 妥当な上書きを先に入れる。
        for decl in &self.params {
            if let Some(v) = overrides.get(&decl.name) {
                if decl.accepts(v) {
                    env.insert(decl.name.clone(), v.clone());
                }
            }
        }

        // 2. 残りを依存の順に評価する。
        let mut pending: Vec<&ParamDecl> = self
            .params
            .iter()
            .filter(|d| !env.contains_key(&d.name))
            .collect();

        while !pending.is_empty() {
            let before = pending.len();
            pending.retain(|decl| {
                // 参照先がまだ決まっていなければ後回し。
                let ready = decl
                    .default
                    .referenced_vars()
                    .iter()
                    .all(|v| env.contains_key(*v));
                if !ready {
                    return true;
                }
                if let Ok(value) = eval(&decl.default, &env) {
                    if decl.accepts(&value) {
                        env.insert(decl.name.clone(), value);
                    }
                }
                false
            });
            // 1 周して 1 つも減らなければ、残りは循環しているか参照先が無い。
            if pending.len() == before {
                break;
            }
        }

        env
    }

    /// 束縛を適用した中身を返す。
    ///
    /// 評価に失敗した束縛は**その座標だけ定義のまま**にする。
    /// 図形ごと消すと「パラメータを変えたら図形が消えた」という最悪の壊れ方になる。
    #[must_use]
    pub fn evaluated_entities(&self, env: &Env) -> Vec<Entity> {
        let mut out = self.entities.clone();
        for b in &self.bindings {
            let Some(entity) = out.get_mut(b.entity) else {
                continue;
            };
            let Ok(Value::Number(n)) = eval(&b.expr, env) else {
                continue;
            };
            b.slot.apply(&mut entity.geom, n);
        }
        out
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
        bindings: Vec<Binding>,
    ) -> Result<(Point2, Vec<Entity>, Vec<Binding>)> {
        let def = self
            .defs
            .get_mut(id.index() as usize)
            .and_then(|d| d.as_mut())
            .ok_or(CadError::DefinitionNotFound)?;
        let old_origin = std::mem::replace(&mut def.origin, origin);
        let old_entities = std::mem::replace(&mut def.entities, entities);
        let old_bindings = std::mem::replace(&mut def.bindings, bindings);
        Ok((old_origin, old_entities, old_bindings))
    }

    /// パラメータの宣言を差し替える。差し替え前を返す。
    pub(crate) fn replace_params(
        &mut self,
        id: DefinitionId,
        params: Vec<ParamDecl>,
    ) -> Result<Vec<ParamDecl>> {
        let def = self
            .defs
            .get_mut(id.index() as usize)
            .and_then(|d| d.as_mut())
            .ok_or(CadError::DefinitionNotFound)?;
        Ok(std::mem::replace(&mut def.params, params))
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
    resolve_entities(inst, defs)
        .into_iter()
        .map(|e| e.geom)
        .collect()
}

/// インスタンスをワールド座標の**エンティティ列**へ展開する。
///
/// [`resolve`] と違い、**定義の中身が持っていたレイヤ・色・グループを保つ**。
/// `EXPLODE` はこちらを使う（AutoCAD もブロックを分解すると中身のレイヤに戻る）。
/// 入れ子では**内側のエンティティの属性が勝つ**。
#[must_use]
pub fn resolve_entities(inst: &Instance, defs: &DefinitionTable) -> Vec<Entity> {
    let mut out = Vec::new();
    resolve_into(inst, defs, 0, &mut out);
    out
}

/// 展開の本体。入れ子のために深さを持ち回る。
fn resolve_into(inst: &Instance, defs: &DefinitionTable, depth: usize, out: &mut Vec<Entity>) {
    if depth >= MAX_NESTING_DEPTH {
        return;
    }
    let Some(def) = defs.get(inst.definition) else {
        return;
    };

    // パラメータを決めてから中身へ束縛を適用する。
    // 束縛が 1 つも無ければ、これは中身の複製と同じ（クラシックなブロック）。
    let entities = if def.bindings.is_empty() {
        def.entities.clone()
    } else {
        def.evaluated_entities(&def.param_env(&inst.overrides))
    };

    for entity in &entities {
        match &entity.geom {
            // 入れ子。内側のインスタンスを先に展開し、その結果に外側の配置をかける。
            // 属性は内側のものをそのまま残す。
            Geometry::Instance(inner) => {
                let start = out.len();
                resolve_into(inner, defs, depth + 1, out);
                for e in &mut out[start..] {
                    e.geom = place(&e.geom, def.origin, inst.placement);
                }
            }
            g => {
                let mut placed = entity.clone();
                placed.geom = place(g, def.origin, inst.placement);
                out.push(placed);
            }
        }
    }
}

/// 定義座標の図形をワールド座標へ移す。
///
/// [`unplace`] と**必ず対で読むこと**。片方だけ直すと、
/// インプレース編集で書き戻すたびに図形がずれる。
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
#[must_use]
pub fn place(geom: &Geometry, def_origin: Point2, p: Placement) -> Geometry {
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

/// [`place`] の逆。ワールド座標の図形を定義座標へ戻す。
///
/// **インプレース編集の要。** 画面上で編集した図形を定義へ書き戻すのに使う。
/// `place` と**必ず対で読むこと**。片方だけ直すと、編集するたびに図形がずれる。
///
/// 手順は `place` の逆順・逆操作:
/// 配置先から原点へ → 逆回転 → 逆倍率 → 反転 → 基点へ戻す。
#[must_use]
pub fn unplace(geom: &Geometry, def_origin: Point2, p: Placement) -> Geometry {
    let back = geom.translated(Point2::ORIGIN - p.origin);
    let unrotated = back.rotated(Point2::ORIGIN, -p.rotation);
    // 倍率は `Placement::new` が正の有限値を保証しているので、逆数は安全。
    let unscaled = unrotated.scaled(Point2::ORIGIN, 1.0 / p.scale);
    let unflipped = if p.flipped {
        // 鏡像は自分自身が逆変換。
        unscaled.mirrored(&X_AXIS)
    } else {
        unscaled
    };
    unflipped.translated(def_origin - Point2::ORIGIN)
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

/// パラメータの既定値どうしが循環していないか調べる。
///
/// 循環していれば、その輪に含まれる名前を 1 つ返す。
///
/// **コマンドが宣言を受け取る前に必ず呼ぶ。** 循環したまま入れると
/// [`Definition::param_env`] がそのパラメータを解決できず、
/// 参照している束縛が黙って効かなくなる。
#[must_use]
pub fn param_cycle(params: &[ParamDecl]) -> Option<String> {
    /// 探索の状態。
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        /// 未訪問。
        White,
        /// 探索中（ここへ戻ってきたら循環）。
        Grey,
        /// 探索済み。
        Black,
    }

    let mut mark: BTreeMap<&str, Mark> = params
        .iter()
        .map(|p| (p.name.as_str(), Mark::White))
        .collect();

    // 明示的なスタックで深さ優先探索する（再帰だと深い依存でスタックを溢れさせる）。
    for start in params {
        if mark.get(start.name.as_str()) != Some(&Mark::White) {
            continue;
        }
        let mut stack = vec![(start.name.as_str(), 0usize)];
        while let Some((name, index)) = stack.pop() {
            let Some(decl) = params.iter().find(|p| p.name == name) else {
                continue;
            };
            let deps = decl.default.referenced_vars();

            if index == 0 {
                mark.insert(name, Mark::Grey);
            }
            if index >= deps.len() {
                mark.insert(name, Mark::Black);
                continue;
            }

            // 次の依存へ進む前に、自分を「続きから」戻す。
            stack.push((name, index + 1));
            let dep = deps[index];
            match mark.get(dep) {
                Some(Mark::Grey) => return Some(dep.to_owned()),
                Some(Mark::White) => stack.push((dep, 0)),
                // 探索済み、または宣言されていない名前（別のエラーで弾かれる）。
                _ => {}
            }
        }
    }
    None
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::tolerance::eq_len;
    use crate::geom::{Arc, Circle, Line, Polyline, Vec2, Xline};
    use crate::layer::LayerId;
    use std::f64::consts::{FRAC_PI_2, PI};

    fn p(x: f64, y: f64) -> Point2 {
        Point2::new(x, y)
    }

    fn ent(geom: Geometry) -> Entity {
        Entity::new(geom, LayerId::ZERO)
    }

    /// 定義を 1 つだけ持つ表を作る。
    fn table_with(entities: Vec<Entity>, origin: Point2) -> (DefinitionTable, DefinitionId) {
        let mut t = DefinitionTable::new();
        let id = t.insert(Definition::new("部品", origin, entities));
        (t, id)
    }

    // ---- 図形の近似比較 ---------------------------------------------------
    //
    // 回転や鏡像は三角関数を通るので、係数を直接比べると 1 ULP では収まらない。
    // **弧上の点を標本して比べる**ことで、角度の比較を長さの比較に落とす
    // （角度の折り返しも自動的に吸収される）。

    /// 図形の代表点を並べる。同じ図形なら同じ列になる。
    fn probe(geom: &Geometry) -> Vec<Point2> {
        match geom {
            Geometry::Line(l) => vec![l.a, l.b],
            Geometry::Circle(c) => vec![
                c.center,
                p(c.center.x + c.radius, c.center.y),
                p(c.center.x, c.center.y + c.radius),
            ],
            // 始点・1/4・中点・3/4・終点。向きが逆転すると列が反転するので検出できる。
            Geometry::Arc(a) => vec![
                a.center,
                a.point_at(0.0),
                a.point_at(0.25),
                a.point_at(0.5),
                a.point_at(0.75),
                a.point_at(1.0),
            ],
            // 通過点と、そこから両方向へ 1 進んだ点。
            Geometry::Xline(x) => vec![x.origin, x.point_at(1.0), x.point_at(-1.0)],
            Geometry::Polyline(pl) => pl.vertices.clone(),
            Geometry::Instance(i) => vec![i.placement.origin],
        }
    }

    fn same_points(a: &[Point2], b: &[Point2]) -> bool {
        a.len() == b.len()
            && a.iter()
                .zip(b.iter())
                .all(|(x, y)| eq_len(x.x, y.x) && eq_len(x.y, y.y))
    }

    /// 変換後の図形を解決する。インスタンスなら展開し、それ以外はそのまま。
    fn resolve_geom(g: &Geometry, defs: &DefinitionTable) -> Vec<Geometry> {
        match g {
            Geometry::Instance(i) => resolve(i, defs),
            other => vec![other.clone()],
        }
    }

    /// 図形列が（順序込みで）一致するか。
    fn same_geoms(a: &[Geometry], b: &[Geometry]) -> bool {
        a.len() == b.len()
            && a.iter().zip(b.iter()).all(|(x, y)| {
                std::mem::discriminant(x) == std::mem::discriminant(y)
                    && same_points(&probe(x), &probe(y))
            })
    }

    /// 全変種を含む定義の中身。
    fn sample_contents() -> Vec<Entity> {
        vec![
            ent(Geometry::Line(Line::new(p(0.0, 0.0), p(10.0, 0.0)))),
            ent(Geometry::Circle(Circle::new(p(2.0, 3.0), 1.5))),
            ent(Geometry::Arc(Arc::new(p(1.0, 1.0), 3.0, 0.25, 2.75))),
            ent(Geometry::Polyline(Polyline::new(
                vec![p(0.0, 0.0), p(1.0, 2.0), p(3.0, 1.0)],
                true,
            ))),
        ]
    }

    // ---- 配置の基本 -------------------------------------------------------

    /// 基点が指定点へ来ること。
    #[test]
    fn the_definition_origin_lands_on_the_placement_origin() {
        let (defs, id) = table_with(
            vec![ent(Geometry::Line(Line::new(p(5.0, 5.0), p(6.0, 5.0))))],
            p(5.0, 5.0),
        );
        let inst = Instance::new(id, Placement::at(p(100.0, 200.0)));

        let out = resolve(&inst, &defs);
        let Geometry::Line(l) = &out[0] else {
            panic!("線分のはず")
        };
        assert!(eq_len(l.a.x, 100.0), "x = {}", l.a.x);
        assert!(eq_len(l.a.y, 200.0), "y = {}", l.a.y);
        assert!(eq_len(l.b.x, 101.0), "基点からの相対が保たれる: {}", l.b.x);
    }

    /// **円は円のまま。** 一様倍率に限った理由がここ。
    #[test]
    fn a_circle_stays_a_circle_and_scales_its_radius() {
        let (defs, id) = table_with(
            vec![ent(Geometry::Circle(Circle::new(p(0.0, 0.0), 2.0)))],
            Point2::ORIGIN,
        );
        let placement = Placement::new(p(0.0, 0.0), FRAC_PI_2, 3.0, false).expect("妥当な配置");
        let out = resolve(&Instance::new(id, placement), &defs);

        let Geometry::Circle(c) = &out[0] else {
            panic!("円のまま戻ること（楕円にならない）: {:?}", out[0]);
        };
        assert!(eq_len(c.radius, 6.0), "半径が倍率ぶん変わる: {}", c.radius);
    }

    /// **円弧の掃引の向きが保たれること。** ADR-0020 の罠の再来を防ぐ。
    #[test]
    fn an_arc_keeps_its_sweep_through_placement() {
        let arc = Arc::new(p(0.0, 0.0), 5.0, 0.0, FRAC_PI_2);
        let sweep_before = arc.sweep();
        let (defs, id) = table_with(vec![ent(Geometry::Arc(arc))], Point2::ORIGIN);

        for flipped in [false, true] {
            let placement = Placement::new(p(3.0, 4.0), 0.7, 2.0, flipped).expect("妥当な配置");
            let out = resolve(&Instance::new(id, placement), &defs);
            let Geometry::Arc(a) = &out[0] else {
                panic!("円弧のはず")
            };
            assert!(
                eq_len(a.sweep(), sweep_before),
                "反転={flipped} で掃引角が変わった: {} → {}",
                sweep_before,
                a.sweep()
            );
        }
    }

    /// **反転は回転より先に適用すること。**
    ///
    /// 反射と回転は交換しないので、順序を入れ替えると別の図形になる。
    /// （一様倍率は回転・反転のどちらとも交換するので、そちらの順序は問わない。）
    #[test]
    fn flipping_happens_before_rotation() {
        // 非対称な図形。順序を入れ替えると位置が変わる。
        let (defs, id) = table_with(
            vec![ent(Geometry::Line(Line::new(p(1.0, 0.0), p(3.0, 2.0))))],
            Point2::ORIGIN,
        );
        let placement = Placement::new(Point2::ORIGIN, FRAC_PI_2, 1.0, true).expect("妥当");
        let out = resolve(&Instance::new(id, placement), &defs);
        let Geometry::Line(l) = &out[0] else {
            panic!("線分のはず")
        };

        // 手計算: (1,0) を x 軸で反転 → (1,0)。90 度回転 → (0,1)。
        //         (3,2) を x 軸で反転 → (3,-2)。90 度回転 → (2,3)。
        // 順序が逆だと (3,2)→回転(-2,3)→反転(-2,-3) で全く違う点になる。
        assert!(eq_len(l.a.x, 0.0) && eq_len(l.a.y, 1.0), "a = {:?}", l.a);
        assert!(eq_len(l.b.x, 2.0) && eq_len(l.b.y, 3.0), "b = {:?}", l.b);
    }

    // ---- 変換と解決が可換であること（合成の数式の検証） -------------------
    //
    // 「インスタンスを変換してから解決」と「解決してから各図形を変換」が
    // 一致すれば、`Placement` への合成規則が正しい。
    // 変換ごとに個別の期待値を書くより、この 1 つの性質で全部を押さえられる。

    #[test]
    fn translation_commutes_with_resolution() {
        let (defs, id) = table_with(sample_contents(), p(1.0, 1.0));
        let inst = Instance::new(id, Placement::at(p(7.0, -3.0)));
        let v = Vec2::new(4.0, 9.0);

        let via_instance = resolve_geom(&Geometry::Instance(inst.clone()).translated(v), &defs);
        let via_geometry: Vec<Geometry> = resolve(&inst, &defs)
            .iter()
            .map(|g| g.translated(v))
            .collect();

        assert!(same_geoms(&via_instance, &via_geometry));
    }

    #[test]
    fn rotation_commutes_with_resolution() {
        let (defs, id) = table_with(sample_contents(), p(1.0, 1.0));
        let inst = Instance::new(id, Placement::at(p(7.0, -3.0)));
        let center = p(2.0, 2.0);

        for angle in [0.3, FRAC_PI_2, PI, -1.1] {
            let via_instance = resolve_geom(
                &Geometry::Instance(inst.clone()).rotated(center, angle),
                &defs,
            );
            let via_geometry: Vec<Geometry> = resolve(&inst, &defs)
                .iter()
                .map(|g| g.rotated(center, angle))
                .collect();
            assert!(same_geoms(&via_instance, &via_geometry), "angle = {angle}");
        }
    }

    #[test]
    fn scaling_commutes_with_resolution() {
        let (defs, id) = table_with(sample_contents(), p(1.0, 1.0));
        let inst = Instance::new(id, Placement::at(p(7.0, -3.0)));
        let center = p(-1.0, 0.5);

        for factor in [0.25, 1.0, 3.5] {
            let via_instance = resolve_geom(
                &Geometry::Instance(inst.clone()).scaled(center, factor),
                &defs,
            );
            let via_geometry: Vec<Geometry> = resolve(&inst, &defs)
                .iter()
                .map(|g| g.scaled(center, factor))
                .collect();
            assert!(
                same_geoms(&via_instance, &via_geometry),
                "factor = {factor}"
            );
        }
    }

    /// **鏡像。** `Placement` に反転フラグが必要だった理由の検証。
    ///
    /// 反射は (基点・回転・正の倍率) では表現できないので、
    /// フラグが無いとここが必ず落ちる（実際に外して確認済み）。
    #[test]
    fn mirroring_commutes_with_resolution() {
        let (defs, id) = table_with(sample_contents(), p(1.0, 1.0));
        let inst = Instance::new(id, Placement::at(p(7.0, -3.0)));

        let axes = [
            Line::new(p(0.0, 0.0), p(1.0, 0.0)),  // 水平
            Line::new(p(0.0, 0.0), p(0.0, 1.0)),  // 垂直
            Line::new(p(0.0, 0.0), p(1.0, 1.0)),  // 斜め
            Line::new(p(2.0, -1.0), p(5.0, 4.0)), // 原点を通らない斜め
        ];
        for axis in &axes {
            let via_instance =
                resolve_geom(&Geometry::Instance(inst.clone()).mirrored(axis), &defs);
            let via_geometry: Vec<Geometry> = resolve(&inst, &defs)
                .iter()
                .map(|g| g.mirrored(axis))
                .collect();
            assert!(same_geoms(&via_instance, &via_geometry), "axis = {axis:?}");
        }
    }

    /// 反転済みのインスタンスをさらに鏡像しても可換であること。
    ///
    /// 合成規則は「反転していたかどうか」で場合分けしていないので、
    /// 両方の入口を通しておく。
    #[test]
    fn mirroring_an_already_flipped_instance_commutes() {
        let (defs, id) = table_with(sample_contents(), p(1.0, 1.0));
        let placement = Placement::new(p(7.0, -3.0), 0.4, 1.5, true).expect("妥当");
        let inst = Instance::new(id, placement);
        let axis = Line::new(p(2.0, -1.0), p(5.0, 4.0));

        let via_instance = resolve_geom(&Geometry::Instance(inst.clone()).mirrored(&axis), &defs);
        let via_geometry: Vec<Geometry> = resolve(&inst, &defs)
            .iter()
            .map(|g| g.mirrored(&axis))
            .collect();
        assert!(same_geoms(&via_instance, &via_geometry));
    }

    /// 同じ軸で 2 回鏡像すると元に戻ること（反転フラグが正しく戻る）。
    #[test]
    fn mirroring_twice_is_the_identity() {
        let (defs, id) = table_with(sample_contents(), p(1.0, 1.0));
        let inst = Instance::new(id, Placement::at(p(7.0, -3.0)));
        let axis = Line::new(p(2.0, -1.0), p(5.0, 4.0));

        let twice = Geometry::Instance(inst.clone())
            .mirrored(&axis)
            .mirrored(&axis);

        assert!(same_geoms(
            &resolve(&inst, &defs),
            &resolve_geom(&twice, &defs)
        ));
    }

    /// 回転を 4 回積んで 360 度で戻ること。
    #[test]
    fn four_quarter_turns_return_to_the_start() {
        let (defs, id) = table_with(sample_contents(), p(1.0, 1.0));
        let inst = Instance::new(id, Placement::at(p(7.0, -3.0)));

        let mut g = Geometry::Instance(inst.clone());
        for _ in 0..4 {
            g = g.rotated(Point2::ORIGIN, FRAC_PI_2);
        }
        assert!(same_geoms(&resolve(&inst, &defs), &resolve_geom(&g, &defs)));
    }

    /// 負の一様倍率は**反射ではなく 180 度回転**（2 次元では行列式が +1）。
    ///
    /// 反転フラグを立てると別の図形になる。
    #[test]
    fn a_negative_scale_is_a_half_turn_not_a_reflection() {
        let (defs, id) = table_with(sample_contents(), p(1.0, 1.0));
        let inst = Instance::new(id, Placement::at(p(7.0, -3.0)));
        let center = p(0.0, 0.0);

        let negative = Geometry::Instance(inst.clone()).scaled(center, -2.0);
        let Geometry::Instance(i) = &negative else {
            panic!("インスタンスのはず")
        };
        assert!(!i.placement.flipped, "反転しないこと");
        assert!(eq_len(i.placement.scale, 2.0), "倍率は絶対値");

        // 図形としても「等倍 2 倍 + 180 度回転」と一致すること。
        let expected = Geometry::Instance(inst)
            .scaled(center, 2.0)
            .rotated(center, PI);
        assert!(same_geoms(
            &resolve_geom(&negative, &defs),
            &resolve_geom(&expected, &defs)
        ));
    }

    /// STRETCH はインスタンスを変形せず、基点が窓に入れば平行移動すること。
    #[test]
    fn stretch_moves_the_instance_only_when_its_origin_is_inside() {
        let (defs, id) = table_with(sample_contents(), Point2::ORIGIN);
        let inst = Instance::new(id, Placement::at(p(5.0, 5.0)));
        let geom = Geometry::Instance(inst.clone());
        let delta = Vec2::new(10.0, 0.0);

        // 基点を含む窓 → 動く。
        let inside = [Aabb::new(p(0.0, 0.0), p(10.0, 10.0))];
        let moved = geom.stretched(&inside, delta);
        assert!(same_geoms(
            &resolve_geom(&moved, &defs),
            &resolve_geom(&geom.translated(delta), &defs)
        ));

        // 基点を含まない窓 → 動かない。
        let outside = [Aabb::new(p(100.0, 100.0), p(110.0, 110.0))];
        let kept = geom.stretched(&outside, delta);
        assert!(same_geoms(
            &resolve_geom(&kept, &defs),
            &resolve_geom(&geom, &defs)
        ));
    }

    // ---- 入れ子 -----------------------------------------------------------

    /// 内側の配置と外側の配置が合成されること。
    #[test]
    fn nesting_composes_the_placements() {
        let mut t = DefinitionTable::new();
        let inner = t.insert(Definition::new(
            "内",
            Point2::ORIGIN,
            vec![ent(Geometry::Line(Line::new(p(0.0, 0.0), p(1.0, 0.0))))],
        ));
        let outer = t.insert(Definition::new(
            "外",
            Point2::ORIGIN,
            vec![ent(Geometry::Instance(Instance::new(
                inner,
                Placement::at(p(10.0, 0.0)),
            )))],
        ));

        let out = resolve(&Instance::new(outer, Placement::at(p(100.0, 0.0))), &t);
        assert_eq!(out.len(), 1);
        let Geometry::Line(l) = &out[0] else {
            panic!("線分のはず")
        };
        assert!(eq_len(l.a.x, 110.0), "10 + 100 になる: {}", l.a.x);
    }

    /// 入れ子でも倍率が積になること。
    #[test]
    fn nesting_multiplies_the_scales() {
        let mut t = DefinitionTable::new();
        let inner = t.insert(Definition::new(
            "内",
            Point2::ORIGIN,
            vec![ent(Geometry::Circle(Circle::new(p(0.0, 0.0), 1.0)))],
        ));
        let outer = t.insert(Definition::new(
            "外",
            Point2::ORIGIN,
            vec![ent(Geometry::Instance(Instance::new(
                inner,
                Placement::new(Point2::ORIGIN, 0.0, 2.0, false).expect("妥当"),
            )))],
        ));

        let placement = Placement::new(Point2::ORIGIN, 0.0, 3.0, false).expect("妥当");
        let out = resolve(&Instance::new(outer, placement), &t);
        let Geometry::Circle(c) = &out[0] else {
            panic!("円のはず")
        };
        assert!(eq_len(c.radius, 6.0), "2 × 3 になる: {}", c.radius);
    }

    /// **深さ上限で打ち切ること（無限再帰しないこと）。**
    ///
    /// 循環はコマンドが弾くので通常ここには来ないが、
    /// 万一来てもスタックを溢れさせないための最後の砦。
    #[test]
    fn a_cyclic_table_is_cut_off_instead_of_recursing_forever() {
        let mut t = DefinitionTable::new();
        let a = t.insert(Definition::new("A", Point2::ORIGIN, Vec::new()));
        let b = t.insert(Definition::new("B", Point2::ORIGIN, Vec::new()));
        // A が B を、B が A を含む（コマンドを通さず直接組む）。
        for (target, other) in [(a, b), (b, a)] {
            t.replace_contents(
                target,
                Point2::ORIGIN,
                vec![ent(Geometry::Instance(Instance::new(
                    other,
                    Placement::at(Point2::ORIGIN),
                )))],
                Vec::new(),
            )
            .expect("差し替えられる");
        }

        // panic せず、有限時間で空を返すこと。
        let out = resolve(&Instance::new(a, Placement::at(Point2::ORIGIN)), &t);
        assert!(out.is_empty(), "図形は 1 つも無いので空: {}", out.len());
    }

    // ---- 循環検出 ---------------------------------------------------------

    #[test]
    fn a_definition_cannot_contain_itself() {
        let (t, id) = table_with(Vec::new(), Point2::ORIGIN);
        assert!(would_create_cycle(id, id, &t), "自分自身は循環");
    }

    #[test]
    fn an_indirect_cycle_is_detected() {
        let mut t = DefinitionTable::new();
        let a = t.insert(Definition::new("A", Point2::ORIGIN, Vec::new()));
        let b = t.insert(Definition::new("B", Point2::ORIGIN, Vec::new()));
        let c = t.insert(Definition::new("C", Point2::ORIGIN, Vec::new()));
        // B が C を含む。
        t.replace_contents(
            b,
            Point2::ORIGIN,
            vec![ent(Geometry::Instance(Instance::new(
                c,
                Placement::at(Point2::ORIGIN),
            )))],
            Vec::new(),
        )
        .expect("差し替えられる");

        // C の中に A を入れるのは安全（A は誰も含んでいない）。
        assert!(!would_create_cycle(c, a, &t));
        // A の中に B を入れるのも安全。
        assert!(!would_create_cycle(a, b, &t));
        // C の中に B を入れると B → C → B で循環する。
        assert!(would_create_cycle(c, b, &t), "B → C → B の循環");
    }

    // ---- 境界ボックスと距離 -----------------------------------------------

    #[test]
    fn instance_bbox_covers_the_placed_contents() {
        let (defs, id) = table_with(
            vec![ent(Geometry::Line(Line::new(p(0.0, 0.0), p(2.0, 0.0))))],
            Point2::ORIGIN,
        );
        let placement = Placement::new(p(10.0, 10.0), 0.0, 5.0, false).expect("妥当");
        let b = instance_bbox(&Instance::new(id, placement), &defs);

        assert!(eq_len(b.min.x, 10.0), "min.x = {}", b.min.x);
        assert!(
            eq_len(b.max.x, 20.0),
            "長さ 2 × 倍率 5: max.x = {}",
            b.max.x
        );
    }

    /// **作図線を含むインスタンスは有界でない。**
    ///
    /// ZOOM EXTENTS から外れる必要がある（無限になると意味を失う）。
    #[test]
    fn an_instance_containing_an_xline_is_unbounded() {
        let x = Xline::new(Point2::ORIGIN, Vec2::new(1.0, 1.0)).expect("作図線");
        let (defs, id) = table_with(vec![ent(Geometry::Xline(x))], Point2::ORIGIN);
        let inst = Instance::new(id, Placement::at(Point2::ORIGIN));

        assert!(!instance_is_bounded(&inst, &defs));
        assert!(instance_bbox(&inst, &defs).is_unbounded());
    }

    #[test]
    fn an_instance_of_plain_geometry_is_bounded() {
        let (defs, id) = table_with(sample_contents(), Point2::ORIGIN);
        let inst = Instance::new(id, Placement::at(Point2::ORIGIN));
        assert!(instance_is_bounded(&inst, &defs));
    }

    #[test]
    fn instance_dist_to_finds_the_nearest_content() {
        let (defs, id) = table_with(
            vec![ent(Geometry::Line(Line::new(p(0.0, 0.0), p(10.0, 0.0))))],
            Point2::ORIGIN,
        );
        let inst = Instance::new(id, Placement::at(p(0.0, 0.0)));
        assert!(eq_len(instance_dist_to(&inst, &defs, p(5.0, 3.0)), 3.0));
    }

    /// 中身が空なら当たらないこと（ピックに拾われない）。
    #[test]
    fn an_empty_instance_is_infinitely_far() {
        let (defs, id) = table_with(Vec::new(), Point2::ORIGIN);
        let inst = Instance::new(id, Placement::at(Point2::ORIGIN));
        assert!(instance_dist_to(&inst, &defs, p(1.0, 1.0)).is_infinite());
        assert!(instance_bbox(&inst, &defs).is_empty());
    }

    /// 存在しない定義を指しても panic しないこと。
    #[test]
    fn a_dangling_definition_reference_resolves_to_nothing() {
        let defs = DefinitionTable::new();
        let mut other = DefinitionTable::new();
        let id = other.insert(Definition::new("よそ", Point2::ORIGIN, Vec::new()));

        let inst = Instance::new(id, Placement::at(Point2::ORIGIN));
        assert!(resolve(&inst, &defs).is_empty());
        assert!(instance_bbox(&inst, &defs).is_empty());
    }

    // ---- Placement の入力検証 ---------------------------------------------

    #[test]
    fn placement_rejects_non_positive_and_non_finite_scale() {
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(
                Placement::new(Point2::ORIGIN, 0.0, bad, false).is_err(),
                "倍率 {bad} は拒否されるべき"
            );
        }
    }

    #[test]
    fn placement_rejects_non_finite_rotation_and_origin() {
        assert!(Placement::new(Point2::ORIGIN, f64::NAN, 1.0, false).is_err());
        assert!(Placement::new(p(f64::INFINITY, 0.0), 0.0, 1.0, false).is_err());
    }

    #[test]
    fn placement_at_is_the_identity_placement() {
        let pl = Placement::at(p(1.0, 2.0));
        assert!(eq_len(pl.scale, 1.0));
        assert!(eq_len(pl.rotation, 0.0));
        assert!(!pl.flipped);
    }

    // ---- 表の操作 ---------------------------------------------------------

    #[test]
    fn insert_is_idempotent_by_name() {
        let mut t = DefinitionTable::new();
        let a = t.insert(Definition::new("部品", Point2::ORIGIN, Vec::new()));
        let b = t.insert(Definition::new("部品", p(1.0, 1.0), Vec::new()));
        assert_eq!(a, b, "同名の定義を二重に作らないこと");
        assert_eq!(t.len(), 1);
    }

    /// Undo の要。取り除いた定義を元の ID のまま戻せること。
    #[test]
    fn restore_preserves_the_definition_id() {
        let mut t = DefinitionTable::new();
        let a = t.insert(Definition::new("A", Point2::ORIGIN, Vec::new()));
        let b = t.insert(Definition::new("B", Point2::ORIGIN, Vec::new()));
        let removed = t.remove(a).expect("取り除ける");

        t.restore(a, removed).expect("戻せる");
        assert_eq!(t.by_name("A"), Some(a), "同じ ID で戻ること");
        assert_eq!(t.by_name("B"), Some(b), "他の定義は無事");
    }

    #[test]
    fn restore_into_occupied_slot_is_rejected() {
        let (mut t, id) = table_with(Vec::new(), Point2::ORIGIN);
        assert_eq!(
            t.restore(id, Definition::new("別", Point2::ORIGIN, Vec::new())),
            Err(CadError::SlotOccupied)
        );
    }

    #[test]
    fn remove_drops_the_name_index() {
        let (mut t, id) = table_with(Vec::new(), Point2::ORIGIN);
        t.remove(id).expect("取り除ける");
        assert!(t.by_name("部品").is_none(), "名前の索引も消えること");
        assert_eq!(t.remove(id), Err(CadError::DefinitionNotFound));
    }

    #[test]
    fn replace_contents_returns_the_previous_state() {
        let (mut t, id) = table_with(sample_contents(), p(1.0, 1.0));
        let (old_origin, old, old_bindings) = t
            .replace_contents(id, p(2.0, 2.0), Vec::new(), Vec::new())
            .expect("差し替えられる");
        assert!(old_bindings.is_empty());

        assert!(eq_len(old_origin.x, 1.0), "元の基点が返る");
        assert_eq!(old.len(), 4, "元の中身が返る");
        assert_eq!(t.get(id).expect("あるはず").entities.len(), 0);
    }

    #[test]
    fn rename_updates_the_name_index() {
        let (mut t, id) = table_with(Vec::new(), Point2::ORIGIN);
        let old = t.rename(id, "新").expect("改名できる");
        assert_eq!(old, "部品");
        assert!(t.by_name("部品").is_none());
        assert_eq!(t.by_name("新"), Some(id));
    }

    #[test]
    fn default_name_skips_taken_ones() {
        let mut t = DefinitionTable::new();
        assert_eq!(t.next_default_name(), "コンポーネント1");
        t.insert(Definition::new(
            "コンポーネント1",
            Point2::ORIGIN,
            Vec::new(),
        ));
        assert_eq!(t.next_default_name(), "コンポーネント2");
    }

    #[test]
    fn referenced_definitions_lists_nested_ids() {
        let mut t = DefinitionTable::new();
        let inner = t.insert(Definition::new("内", Point2::ORIGIN, Vec::new()));
        let def = Definition::new(
            "外",
            Point2::ORIGIN,
            vec![
                ent(Geometry::Line(Line::new(p(0.0, 0.0), p(1.0, 0.0)))),
                ent(Geometry::Instance(Instance::new(
                    inner,
                    Placement::at(Point2::ORIGIN),
                ))),
            ],
        );
        assert_eq!(referenced_definitions(&def), vec![inner]);
    }

    // ---- パラメータと束縛 -------------------------------------------------
    //
    // ここが段階 2 の主眼。ダイナミックブロックの「アクション」を
    // 型付きパラメータ + 式で置き換えたことの検証。

    use crate::expr::{parse, Value};

    /// 幅で長さが決まる線分を持つ定義。
    ///
    /// ```text
    /// パラメータ  幅: Number = 900
    /// 幾何        LINE (0,0) - (幅, 0)
    /// ```
    fn parametric_table() -> (DefinitionTable, DefinitionId) {
        let mut t = DefinitionTable::new();
        let mut def = Definition::new(
            "窓",
            Point2::ORIGIN,
            vec![ent(Geometry::Line(Line::new(p(0.0, 0.0), p(1.0, 0.0))))],
        );
        def.params = vec![ParamDecl::number("幅", 900.0).with_range(300.0, 3000.0)];
        def.bindings = vec![Binding::new(0, Slot::LineBx, parse("幅").expect("解析"))];
        let id = t.insert(def);
        (t, id)
    }

    /// 解決された線分の終点 X。
    fn resolved_width(inst: &Instance, defs: &DefinitionTable) -> f64 {
        let out = resolve(inst, defs);
        let Geometry::Line(l) = &out[0] else {
            panic!("線分のはず: {:?}", out[0])
        };
        l.b.x
    }

    /// **既定値が効くこと。**
    #[test]
    fn a_parameter_default_drives_the_geometry() {
        let (defs, id) = parametric_table();
        let inst = Instance::new(id, Placement::at(Point2::ORIGIN));
        assert!(eq_len(resolved_width(&inst, &defs), 900.0));
    }

    /// **インスタンスごとの上書きが効くこと。**
    ///
    /// これがダイナミックブロックの「アクション」で難しかったこと。
    #[test]
    fn an_instance_override_changes_only_that_instance() {
        let (defs, id) = parametric_table();

        let plain = Instance::new(id, Placement::at(Point2::ORIGIN));
        let mut wide = Instance::new(id, Placement::at(Point2::ORIGIN));
        wide.overrides
            .insert("幅".to_owned(), Value::Number(1800.0));

        assert!(eq_len(resolved_width(&plain, &defs), 900.0), "既定のまま");
        assert!(eq_len(resolved_width(&wide, &defs), 1800.0), "上書きが効く");
    }

    /// **上書きを消すと既定値へ戻ること（リセット）。**
    #[test]
    fn removing_an_override_returns_to_the_default() {
        let (defs, id) = parametric_table();
        let mut inst = Instance::new(id, Placement::at(Point2::ORIGIN));
        inst.overrides
            .insert("幅".to_owned(), Value::Number(1800.0));
        assert!(eq_len(resolved_width(&inst, &defs), 1800.0));

        inst.overrides.remove("幅");
        assert!(eq_len(resolved_width(&inst, &defs), 900.0), "既定値へ戻る");
    }

    /// **範囲外の上書きは捨てて既定値を使うこと。**
    ///
    /// 通すと縮退した図形になる。範囲はコマンドが弾くが、
    /// ファイルを手で書き換えられても壊れないようにここでも見る。
    #[test]
    fn an_out_of_range_override_falls_back_to_the_default() {
        let (defs, id) = parametric_table();
        let mut inst = Instance::new(id, Placement::at(Point2::ORIGIN));
        inst.overrides
            .insert("幅".to_owned(), Value::Number(999_999.0));
        assert!(eq_len(resolved_width(&inst, &defs), 900.0), "既定値を使う");
    }

    /// 型の違う上書きも捨てること。
    #[test]
    fn an_override_of_the_wrong_type_is_discarded() {
        let (defs, id) = parametric_table();
        let mut inst = Instance::new(id, Placement::at(Point2::ORIGIN));
        inst.overrides.insert("幅".to_owned(), Value::Bool(true));
        assert!(eq_len(resolved_width(&inst, &defs), 900.0));
    }

    /// **式が合成できること。** `幅 = 高さ × 2 + 10` の類。
    #[test]
    fn expressions_compose_over_several_parameters() {
        let mut t = DefinitionTable::new();
        let mut def = Definition::new(
            "棚",
            Point2::ORIGIN,
            vec![ent(Geometry::Line(Line::new(p(0.0, 0.0), p(1.0, 0.0))))],
        );
        def.params = vec![
            ParamDecl::number("高さ", 50.0),
            // 既定値が別のパラメータを参照する。
            ParamDecl {
                name: "幅".to_owned(),
                ty: crate::expr::ParamType::Number,
                default: parse("高さ * 2 + 10").expect("解析"),
                range: None,
            },
        ];
        def.bindings = vec![Binding::new(0, Slot::LineBx, parse("幅").expect("解析"))];
        let id = t.insert(def);

        let inst = Instance::new(id, Placement::at(Point2::ORIGIN));
        assert!(eq_len(resolved_width(&inst, &t), 110.0), "50 * 2 + 10");

        // 参照元を変えると連動する。
        let mut taller = Instance::new(id, Placement::at(Point2::ORIGIN));
        taller
            .overrides
            .insert("高さ".to_owned(), Value::Number(100.0));
        assert!(eq_len(resolved_width(&taller, &t), 210.0), "連動する");
    }

    /// **依存の順に評価すること（宣言順に依存しない）。**
    #[test]
    fn defaults_are_evaluated_in_dependency_order() {
        let mut t = DefinitionTable::new();
        let mut def = Definition::new(
            "順序",
            Point2::ORIGIN,
            vec![ent(Geometry::Line(Line::new(p(0.0, 0.0), p(1.0, 0.0))))],
        );
        // 依存する側を**先に**宣言する。
        def.params = vec![
            ParamDecl {
                name: "合計".to_owned(),
                ty: crate::expr::ParamType::Number,
                default: parse("a + b").expect("解析"),
                range: None,
            },
            ParamDecl::number("a", 3.0),
            ParamDecl::number("b", 4.0),
        ];
        def.bindings = vec![Binding::new(0, Slot::LineBx, parse("合計").expect("解析"))];
        let id = t.insert(def);

        let inst = Instance::new(id, Placement::at(Point2::ORIGIN));
        assert!(eq_len(resolved_width(&inst, &t), 7.0));
    }

    /// **条件式で形が切り替わること。**
    ///
    /// ダイナミックブロックの「表示状態」に相当するものが式で書ける。
    #[test]
    fn a_condition_switches_the_shape() {
        let mut t = DefinitionTable::new();
        let mut def = Definition::new(
            "扉",
            Point2::ORIGIN,
            vec![ent(Geometry::Line(Line::new(p(0.0, 0.0), p(1.0, 0.0))))],
        );
        def.params = vec![
            ParamDecl::boolean("両開き", false),
            ParamDecl::number("幅", 900.0),
        ];
        def.bindings = vec![Binding::new(
            0,
            Slot::LineBx,
            parse("if 両開き then 幅 / 2 else 幅").expect("解析"),
        )];
        let id = t.insert(def);

        let single = Instance::new(id, Placement::at(Point2::ORIGIN));
        assert!(eq_len(resolved_width(&single, &t), 900.0));

        let mut double = Instance::new(id, Placement::at(Point2::ORIGIN));
        double
            .overrides
            .insert("両開き".to_owned(), Value::Bool(true));
        assert!(eq_len(resolved_width(&double, &t), 450.0));
    }

    /// **循環した既定値でも無限ループしないこと。**
    ///
    /// 循環はコマンドが弾くが、最後の砦としてここでも止まる必要がある。
    /// 解決できなかったパラメータは環境に入らず、参照する束縛が失敗して
    /// その座標は定義のままになる。
    #[test]
    fn cyclic_defaults_do_not_loop_forever() {
        let mut t = DefinitionTable::new();
        let mut def = Definition::new(
            "循環",
            Point2::ORIGIN,
            vec![ent(Geometry::Line(Line::new(p(0.0, 0.0), p(7.0, 0.0))))],
        );
        def.params = vec![
            ParamDecl {
                name: "a".to_owned(),
                ty: crate::expr::ParamType::Number,
                default: parse("b").expect("解析"),
                range: None,
            },
            ParamDecl {
                name: "b".to_owned(),
                ty: crate::expr::ParamType::Number,
                default: parse("a").expect("解析"),
                range: None,
            },
        ];
        def.bindings = vec![Binding::new(0, Slot::LineBx, parse("a").expect("解析"))];
        let id = t.insert(def);

        let inst = Instance::new(id, Placement::at(Point2::ORIGIN));
        // 有限時間で返り、束縛は効かず定義のまま。
        assert!(eq_len(resolved_width(&inst, &t), 7.0), "定義のまま");
    }

    /// **評価に失敗した束縛は、その座標だけ定義のままにすること。**
    ///
    /// 図形ごと消すと「パラメータを変えたら図形が消えた」という最悪の壊れ方になる。
    #[test]
    fn a_failing_binding_leaves_that_scalar_at_its_literal() {
        let mut t = DefinitionTable::new();
        let mut def = Definition::new(
            "壊れた式",
            Point2::ORIGIN,
            vec![ent(Geometry::Line(Line::new(p(0.0, 0.0), p(5.0, 0.0))))],
        );
        // 宣言していないパラメータを参照する束縛。
        def.bindings = vec![Binding::new(
            0,
            Slot::LineBx,
            parse("ない名前").expect("解析"),
        )];
        let id = t.insert(def);

        let inst = Instance::new(id, Placement::at(Point2::ORIGIN));
        let out = resolve(&inst, &t);
        assert_eq!(out.len(), 1, "図形は消えない");
        assert!(eq_len(resolved_width(&inst, &t), 5.0), "定義のまま");
    }

    /// 0 除算になる束縛でも図形が壊れないこと。
    #[test]
    fn a_division_by_zero_binding_keeps_the_literal() {
        let mut t = DefinitionTable::new();
        let mut def = Definition::new(
            "0除算",
            Point2::ORIGIN,
            vec![ent(Geometry::Line(Line::new(p(0.0, 0.0), p(5.0, 0.0))))],
        );
        def.params = vec![ParamDecl::number("分母", 0.0)];
        def.bindings = vec![Binding::new(
            0,
            Slot::LineBx,
            parse("100 / 分母").expect("解析"),
        )];
        let id = t.insert(def);

        let inst = Instance::new(id, Placement::at(Point2::ORIGIN));
        assert!(eq_len(resolved_width(&inst, &t), 5.0), "定義のまま");
    }

    /// **束縛が無ければクラシックなブロックとして振る舞うこと。**
    ///
    /// 段階 1 と段階 2 が地続きであることの確認。
    #[test]
    fn a_definition_without_bindings_behaves_like_a_classic_block() {
        let (defs, id) = table_with(sample_contents(), p(1.0, 1.0));
        let inst = Instance::new(id, Placement::at(p(7.0, -3.0)));
        assert_eq!(resolve(&inst, &defs).len(), 4, "中身がそのまま出る");
    }

    /// パラメータと配置の変換が両立すること。
    #[test]
    fn parameters_and_placement_compose() {
        let (defs, id) = parametric_table();
        let mut inst = Instance::new(
            id,
            Placement::new(p(10.0, 0.0), 0.0, 2.0, false).expect("妥当"),
        );
        inst.overrides.insert("幅".to_owned(), Value::Number(500.0));

        // 幅 500 の線分を 2 倍にして (10,0) へ置く → 終点 X = 10 + 1000。
        assert!(eq_len(resolved_width(&inst, &defs), 1010.0));
    }

    // ---- 逆変換（インプレース編集の要） -----------------------------------

    /// **`place` → `unplace` が恒等変換になること。**
    ///
    /// ここがずれると、定義を編集して書き戻すたびに図形が少しずつ動く。
    /// 反転・回転・倍率・基点をすべて混ぜて確かめる。
    #[test]
    fn unplace_undoes_place() {
        let origins = [Point2::ORIGIN, p(3.0, -7.0)];
        let placements = [
            Placement::at(p(100.0, 200.0)),
            Placement::new(p(-50.0, 20.0), 0.7, 2.5, false).expect("妥当"),
            Placement::new(p(10.0, 10.0), -1.3, 0.25, true).expect("妥当"),
            Placement::new(Point2::ORIGIN, FRAC_PI_2, 1.0, true).expect("妥当"),
        ];

        for def_origin in origins {
            for pl in placements {
                for entity in sample_contents() {
                    let placed = place(&entity.geom, def_origin, pl);
                    let back = unplace(&placed, def_origin, pl);
                    assert!(
                        same_points(&probe(&entity.geom), &probe(&back)),
                        "基点={def_origin:?} 配置={pl:?} で戻らない\n元: {:?}\n戻り: {back:?}",
                        entity.geom
                    );
                }
            }
        }
    }

    /// 作図線でも恒等になること（方向の単位ベクトルが崩れないこと）。
    #[test]
    fn unplace_undoes_place_for_xlines() {
        let x = Xline::new(p(1.0, 2.0), Vec2::new(3.0, 4.0)).expect("作図線");
        let geom = Geometry::Xline(x);
        let pl = Placement::new(p(5.0, -5.0), 1.1, 3.0, true).expect("妥当");

        let placed = place(&geom, p(1.0, 1.0), pl);
        let back = unplace(&placed, p(1.0, 1.0), pl);
        let Geometry::Xline(got) = &back else {
            panic!("作図線のはず: {back:?}")
        };
        assert!(eq_len(got.direction.len(), 1.0), "単位ベクトルのまま");
        assert!(same_points(&probe(&geom), &probe(&back)));
    }

    /// 円弧の掃引の向きも戻ること（反転を通しても補角にならない）。
    #[test]
    fn unplace_preserves_the_arc_sweep() {
        let arc = Arc::new(p(0.0, 0.0), 5.0, 0.25, 2.0);
        let geom = Geometry::Arc(arc);
        let pl = Placement::new(p(3.0, 4.0), 0.9, 2.0, true).expect("妥当");

        let placed = place(&geom, Point2::ORIGIN, pl);
        let back = unplace(&placed, Point2::ORIGIN, pl);
        let Geometry::Arc(got) = &back else { panic!() };
        assert!(
            eq_len(got.sweep(), arc.sweep()),
            "掃引角が戻る: {} → {}",
            arc.sweep(),
            got.sweep()
        );
    }

    /// 入れ子のインスタンスも戻ること（配置の合成が対称であること）。
    #[test]
    fn unplace_undoes_place_for_instances() {
        let mut t = DefinitionTable::new();
        let inner = t.insert(Definition::new("内", Point2::ORIGIN, sample_contents()));
        let geom = Geometry::Instance(Instance::new(
            inner,
            Placement::new(p(7.0, 8.0), 0.5, 1.5, true).expect("妥当"),
        ));
        let pl = Placement::new(p(-2.0, 3.0), -0.6, 4.0, true).expect("妥当");

        let placed = place(&geom, p(1.0, 1.0), pl);
        let back = unplace(&placed, p(1.0, 1.0), pl);
        // 中身まで展開して比べる（配置の数値だけでなく、見た目が戻ること）。
        let (Geometry::Instance(a), Geometry::Instance(b)) = (&geom, &back) else {
            panic!("インスタンスのはず")
        };
        assert!(same_geoms(&resolve(a, &t), &resolve(b, &t)));
    }

    #[test]
    fn param_lookup_by_name() {
        let (defs, id) = parametric_table();
        let def = defs.get(id).expect("引ける");
        assert!(def.param("幅").is_some());
        assert!(def.param("ない").is_none());
    }
}
