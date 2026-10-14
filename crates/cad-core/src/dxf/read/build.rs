//! 読んだ中間表現から [`Document`] を組み立てる。
//!
//! # ブロック → コンポーネント（Issue #109、ADR-0050）
//!
//! - `BLOCK` はコンポーネント定義、`INSERT` はインスタンスにする。
//!   ただしモデル空間・ペーパー空間（`*Model_Space` / `*Paper_Space…`、R12 では
//!   `$MODEL_SPACE` / `$PAPER_SPACE`）と外部参照（70 のフラグ 4 / 8）は定義にしない。
//! - 名前の付いたブロックは参照されていなくても定義にする（図面の部品の一覧として残す）。
//!   匿名ブロック（`*U…` `*D…` など）は `INSERT` から参照されているときだけ定義にする
//!   （寸法の `*D…` は `DIMENSION` からしか参照されず、`DIMENSION` は読まないため）。
//! - DXF のブロック名は大文字・小文字を区別しない。名前は**最初に出てきた綴りのまま**
//!   定義名にし、参照は大文字にそろえて引く。同じ名前（大文字・小文字違いを含む）の
//!   2 つ目以降のブロックと、名前の無いブロックは使わずに警告する。
//! - 前方参照・入れ子があるので、先に全部の定義の器（中身が空）を作って ID を決め、
//!   次に**内側が先になる順**で中身を入れる。循環する参照は、その順を決める深さ優先探索で
//!   見つけた後退辺の `INSERT` だけ飛ばして警告する（[`dependency_order`]）。
//!   最後の砦として、インスタンスを置く前に `would_create_cycle` でも確かめる。
//! - `INSERT` の変換が相似（反転を含む）ならインスタンス、縦横の倍率が違えば
//!   その `INSERT` だけ中身を行列で写した通常の図形へ展開する（[`super::xform`]）。
//!
//! 組み立ては**コマンド経由だけ**で行い、最後に Undo 履歴を消して未保存の印も外す
//! （`Document` の変更経路は `Command` しかない。`cad-core` 全体の不変条件）。

use std::collections::{BTreeSet, HashMap, HashSet};

use super::xform::{transform_geometry, Affine, InsertXform, Shape};
use super::{BlockRecord, InsertRecord, LayerRecord, RawEntity, RawKind, ReadReport, Tally};
use crate::command::{
    AddEntities, AddLayer, DefineComponent, SetDefinitionContents, SetLayerProperties,
};
use crate::component::{
    would_create_cycle, DefinitionId, DefinitionTable, Instance, MAX_NESTING_DEPTH,
};
use crate::document::Document;
use crate::entity::{Entity, Geometry};
use crate::error::{CadError, Result};
use crate::geom::tolerance::is_zero_len;
use crate::geom::Vec2;
use crate::layer::{AciColor, LayerId};

/// 読み込みで適用するコマンドの名前（履歴は最後に消すので画面には出ない）。
const COMMAND_NAME: &str = "DXF_IMPORT";

/// ブロックのフラグ（70）のうち外部参照を表すもの（4 = 外部参照、8 = オーバーレイ）。
/// 外部参照は非スコープなので定義にしない。
const XREF_FLAGS: i32 = 4 | 8;

/// ブロックのフラグ（70）の「匿名ブロック」。
const ANONYMOUS_FLAG: i32 = 1;

/// MINSERT 1 個で並べる数の上限。
///
/// 壊れたファイルの 70 / 71 が 32767 × 32767（約 10 億個）のような値でも、
/// メモリを使い果たさないための歯止め。天井の格子のような実用の配列（数百 × 数百）は収まる。
const MAX_MINSERT_ITEMS: u64 = 100_000;

/// 警告に並べるブロック名の最大数。残りは「ほか N 件」にまとめる。
const MAX_LISTED_NAMES: usize = 5;

/// 参照を引くためのキー。DXF のブロック名は大文字・小文字を区別しない。
fn block_key(name: &str) -> String {
    name.to_uppercase()
}

/// モデル空間・ペーパー空間を表すブロック名か。
fn is_layout_block(name: &str) -> bool {
    let upper = name.to_uppercase();
    [
        "*MODEL_SPACE",
        "*PAPER_SPACE",
        "$MODEL_SPACE",
        "$PAPER_SPACE",
    ]
    .iter()
    .any(|prefix| upper.starts_with(prefix))
}

fn is_anonymous(block: &BlockRecord) -> bool {
    block.name.starts_with('*') || block.flags & ANONYMOUS_FLAG != 0
}

fn is_xref(block: &BlockRecord) -> bool {
    block.flags & XREF_FLAGS != 0
}

/// 件数と、その対象の名前（重複なし）。
#[derive(Default)]
struct Names {
    count: usize,
    names: BTreeSet<String>,
}

impl Names {
    fn add(&mut self, name: &str) {
        self.count += 1;
        let shown = if name.trim().is_empty() {
            "（名前なし）"
        } else {
            name
        };
        self.names.insert(shown.to_owned());
    }

    fn list(&self) -> String {
        let mut shown: Vec<&str> = self
            .names
            .iter()
            .take(MAX_LISTED_NAMES)
            .map(String::as_str)
            .collect();
        let rest = self.names.len().saturating_sub(MAX_LISTED_NAMES);
        let tail = format!("ほか {rest} 件");
        if rest > 0 {
            shown.push(&tail);
        }
        shown.join("、")
    }
}

/// 組み立て中に数える、警告のもと。
#[derive(Default)]
struct Log {
    /// 読むときに数えたもの（`ENTITIES` と、定義にしたブロックのぶん）。
    tally: Tally,
    /// 縦横の倍率が違うので展開した `INSERT` の数。
    expanded: usize,
    /// 倍率 0 などで潰れるので読まなかった `INSERT` の数。
    degenerate_inserts: usize,
    /// 展開で形が潰れて表せなかった図形の数。
    degenerate_shapes: usize,
    /// 循環する参照になるので読まなかった `INSERT`。
    cycles: Names,
    /// 定義の無いブロックへの参照。
    undefined: Names,
    /// 外部参照への参照。
    xrefs: Names,
    /// モデル空間・ペーパー空間を指す参照。
    layout_refs: usize,
    /// MINSERT の個数が多すぎて読まなかった数。
    too_many_items: usize,
    /// 展開の入れ子が深すぎて打ち切った数。
    too_deep_expansion: usize,
    /// 名前の無いブロックの数。
    nameless_blocks: usize,
    /// 同じ名前の 2 つ目以降のブロック。
    duplicate_blocks: Names,
    /// 入れ子が [`MAX_NESTING_DEPTH`] 段を超える定義の数。
    deep_definitions: usize,
}

impl Log {
    fn into_warnings(self) -> Vec<String> {
        let mut w = Vec::new();
        if !self.tally.unread.is_empty() {
            let mut kinds: Vec<(&String, &usize)> = self.tally.unread.iter().collect();
            // 多い順、同数なら名前順。
            kinds.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
            let list: Vec<String> = kinds.iter().map(|(k, n)| format!("{k} {n} 件")).collect();
            w.push(format!(
                "ymcad に無い種類の図形は読み込みませんでした: {}",
                list.join("、")
            ));
        }
        if self.expanded > 0 {
            w.push(format!(
                "縦横の倍率が違うブロック参照 {} 件は、コンポーネントでは表せないため普通の図形に展開しました\
                 （円・円弧は楕円になるのでポリラインで近似しています）",
                self.expanded
            ));
        }
        if self.tally.byblock > 0 {
            w.push(format!(
                "色が BYBLOCK の図形 {} 件は、BYLAYER（レイヤの色）として読み込みました",
                self.tally.byblock
            ));
        }
        if self.tally.oblique > 0 {
            w.push(format!(
                "押し出し方向が Z 軸から傾いた図形 {} 件は、2D に落とせないため読み込みませんでした",
                self.tally.oblique
            ));
        }
        if self.cycles.count > 0 {
            w.push(format!(
                "循環参照になるブロック参照 {} 件を読み込みませんでした（{}）",
                self.cycles.count,
                self.cycles.list()
            ));
        }
        if self.undefined.count > 0 {
            w.push(format!(
                "定義の無いブロックへの参照 {} 件を読み込みませんでした（{}）",
                self.undefined.count,
                self.undefined.list()
            ));
        }
        if self.xrefs.count > 0 {
            w.push(format!(
                "外部参照 {} 件は読み込みませんでした（{}）",
                self.xrefs.count,
                self.xrefs.list()
            ));
        }
        if self.layout_refs > 0 {
            w.push(format!(
                "モデル空間・ペーパー空間を指すブロック参照 {} 件を読み込みませんでした",
                self.layout_refs
            ));
        }
        if self.degenerate_inserts > 0 {
            w.push(format!(
                "倍率が 0 などで形が潰れるブロック参照 {} 件を読み込みませんでした",
                self.degenerate_inserts
            ));
        }
        if self.degenerate_shapes > 0 {
            w.push(format!(
                "展開で形が潰れた図形 {} 件を読み込みませんでした",
                self.degenerate_shapes
            ));
        }
        if self.too_many_items > 0 {
            w.push(format!(
                "並べる数が {MAX_MINSERT_ITEMS} 個を超える MINSERT {} 件を読み込みませんでした",
                self.too_many_items
            ));
        }
        if self.too_deep_expansion > 0 {
            w.push(format!(
                "入れ子が {MAX_NESTING_DEPTH} 段を超えるため、展開を打ち切ったブロック参照が {} 件あります",
                self.too_deep_expansion
            ));
        }
        if self.deep_definitions > 0 {
            w.push(format!(
                "入れ子が {MAX_NESTING_DEPTH} 段を超えるコンポーネントが {} 件あり、深い部分は表示されません",
                self.deep_definitions
            ));
        }
        if self.nameless_blocks > 0 {
            w.push(format!(
                "名前の無いブロック {} 件は読み込みませんでした",
                self.nameless_blocks
            ));
        }
        if self.duplicate_blocks.count > 0 {
            w.push(format!(
                "同じ名前のブロックが複数あるため、最初のものだけを使いました（{}）",
                self.duplicate_blocks.list()
            ));
        }
        w
    }
}

/// ブロック名（大文字にそろえたもの）→ ブロック。
struct BlockTable<'a> {
    blocks: HashMap<String, &'a BlockRecord>,
    /// モデル空間・ペーパー空間のブロック名（参照されたときの警告の出し分けに使う）。
    layouts: HashSet<String>,
}

impl<'a> BlockTable<'a> {
    fn new(records: &'a [BlockRecord], log: &mut Log) -> Self {
        let mut blocks = HashMap::new();
        let mut layouts = HashSet::new();
        for b in records {
            if b.name.trim().is_empty() {
                log.nameless_blocks += 1;
                continue;
            }
            let key = block_key(&b.name);
            if is_layout_block(&b.name) {
                layouts.insert(key);
                continue;
            }
            if blocks.contains_key(&key) {
                log.duplicate_blocks.add(&b.name);
                continue;
            }
            blocks.insert(key, b);
        }
        Self { blocks, layouts }
    }

    /// 定義にするブロックを、ファイルに出てきた順で返す。
    fn to_define(
        &self,
        records: &'a [BlockRecord],
        entities: &[RawEntity],
    ) -> Vec<&'a BlockRecord> {
        let mut chosen: HashSet<String> = HashSet::new();
        let mut work: Vec<String> = Vec::new();
        let choose = |key: String, chosen: &mut HashSet<String>, work: &mut Vec<String>| {
            let usable = self.blocks.get(&key).is_some_and(|b| !is_xref(b));
            if usable && chosen.insert(key.clone()) {
                work.push(key);
            }
        };

        for (key, b) in &self.blocks {
            if !is_anonymous(b) {
                choose(key.clone(), &mut chosen, &mut work);
            }
        }
        for key in insert_keys(entities) {
            choose(key, &mut chosen, &mut work);
        }
        while let Some(key) = work.pop() {
            let Some(b) = self.blocks.get(&key) else {
                continue;
            };
            for inner in insert_keys(&b.entities) {
                choose(inner, &mut chosen, &mut work);
            }
        }

        records
            .iter()
            .filter(|b| {
                let key = block_key(&b.name);
                chosen.contains(&key)
                    && self
                        .blocks
                        .get(&key)
                        .is_some_and(|&kept| std::ptr::eq(kept, *b))
            })
            .collect()
    }
}

/// 図形の並びの中の `INSERT` が参照するブロックのキー。
fn insert_keys(raws: &[RawEntity]) -> impl Iterator<Item = String> + '_ {
    raws.iter().filter_map(|r| match &r.kind {
        RawKind::Insert(rec) => Some(block_key(&rec.block)),
        RawKind::Geom(_) => None,
    })
}

/// 循環を断つために落とす参照（親のブロックのキー, 参照先のキー）。
type BackEdges = HashSet<(String, String)>;

/// 定義を「内側が先」の順に並べ（深さ優先の帰りがけ順）、循環を作る参照を見つける。
///
/// 辿り中のブロックへ戻る参照（後退辺）が循環を作る。それを落とせば参照は DAG になる。
/// **図面（`ENTITIES`）から参照されるブロックを根として先に辿る**ので、落ちるのは
/// 「図面から辿って最後に輪を閉じる参照」になる（A → B → A なら B の中の A）。
/// 図面で A を見れば A の中に B が見え、B の中から A へ戻る参照だけが消える。
///
/// 再帰ではなく明示的なスタックで辿る（入れ子の深いファイルでスタックを溢れさせない）。
fn dependency_order<'a>(
    defined: &[&'a BlockRecord],
    entities: &[RawEntity],
) -> (Vec<&'a BlockRecord>, BackEdges) {
    /// 辿りの状態。
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        /// 辿り中（スタックに載っている）。
        Open,
        /// 辿り終わった。
        Done,
    }

    let by_key: HashMap<String, &'a BlockRecord> =
        defined.iter().map(|b| (block_key(&b.name), *b)).collect();
    let roots: Vec<&'a BlockRecord> = insert_keys(entities)
        .filter_map(|k| by_key.get(&k).copied())
        .chain(defined.iter().copied())
        .collect();
    let mut mark: HashMap<String, Mark> = HashMap::new();
    let mut back_edges = BackEdges::new();
    let mut order = Vec::with_capacity(defined.len());

    for root in roots {
        let root_key = block_key(&root.name);
        if mark.contains_key(&root_key) {
            continue;
        }
        mark.insert(root_key.clone(), Mark::Open);
        // (ブロック, そのキー, 次に見る中身の添字)
        let mut stack: Vec<(&'a BlockRecord, String, usize)> = vec![(root, root_key, 0)];
        while let Some(top) = stack.last_mut() {
            let block = top.0;
            let mut child = None;
            while top.2 < block.entities.len() {
                let e = &block.entities[top.2];
                top.2 += 1;
                let RawKind::Insert(rec) = &e.kind else {
                    continue;
                };
                let key = block_key(&rec.block);
                let Some(&c) = by_key.get(&key) else {
                    continue;
                };
                match mark.get(&key) {
                    None => {
                        child = Some((c, key));
                        break;
                    }
                    Some(Mark::Open) => {
                        back_edges.insert((top.1.clone(), key));
                    }
                    Some(Mark::Done) => {}
                }
            }
            match child {
                Some((c, key)) => {
                    mark.insert(key.clone(), Mark::Open);
                    stack.push((c, key, 0));
                }
                None => {
                    mark.insert(top.1.clone(), Mark::Done);
                    order.push(block);
                    stack.pop();
                }
            }
        }
    }
    (order, back_edges)
}

/// レイヤ名 → ID。テーブルに無い名前は最初に使われた順に作る。
struct LayerMap {
    ids: HashMap<String, LayerId>,
}

impl LayerMap {
    fn new(doc: &mut Document, records: &[LayerRecord]) -> Result<Self> {
        let mut ids = HashMap::new();
        ids.insert("0".to_string(), LayerId::ZERO);
        for rec in records {
            if rec.name == "0" {
                apply_layer_props(doc, LayerId::ZERO, rec)?;
                continue;
            }
            doc.apply(Box::new(AddLayer::new(rec.name.clone(), rec.color)))?;
            let id = doc
                .layers()
                .by_name(&rec.name)
                .ok_or(CadError::LayerNotFound)?;
            apply_layer_props(doc, id, rec)?;
            ids.insert(rec.name.clone(), id);
        }
        Ok(Self { ids })
    }

    /// 図形が参照しているのにテーブルに無いレイヤを作る。
    fn ensure<'r>(
        &mut self,
        doc: &mut Document,
        raws: impl Iterator<Item = &'r RawEntity>,
    ) -> Result<()> {
        for raw in raws {
            if self.ids.contains_key(&raw.layer) {
                continue;
            }
            doc.apply(Box::new(AddLayer::new(raw.layer.clone(), AciColor::WHITE)))?;
            let id = doc
                .layers()
                .by_name(&raw.layer)
                .ok_or(CadError::LayerNotFound)?;
            self.ids.insert(raw.layer.clone(), id);
        }
        Ok(())
    }
}

/// レイヤレコードの属性（色・表示・ロック・線種）を対象レイヤへ適用する。
fn apply_layer_props(doc: &mut Document, id: LayerId, rec: &LayerRecord) -> Result<()> {
    doc.apply(Box::new(
        SetLayerProperties::new(id)
            .color(rec.color)
            .visible(rec.visible)
            .locked(rec.locked)
            .linetype(rec.linetype),
    ))
}

/// 中間表現の図形を `Entity` へ変換する。
struct Converter<'a> {
    table: &'a BlockTable<'a>,
    def_ids: &'a HashMap<String, DefinitionId>,
    back_edges: &'a BackEdges,
    layers: &'a HashMap<String, LayerId>,
    defs: &'a DefinitionTable,
    log: &'a mut Log,
}

/// 変換先が定義の中身なら、その定義（循環の検査に使う）。
type Target = Option<DefinitionId>;

impl Converter<'_> {
    /// `raws` を `m`（ブロック座標 → 出力先の座標）で写して `out` に積む。
    ///
    /// `stack` は展開中のブロックのキー（循環の検出用）。`in_expansion` は
    /// 縦横別倍率の展開の内側か（展開した数を外側の `INSERT` 1 件として数えるため）。
    fn convert_all(
        &mut self,
        raws: &[RawEntity],
        m: Affine,
        target: Target,
        stack: &mut Vec<String>,
        in_expansion: bool,
        out: &mut Vec<Entity>,
    ) {
        for raw in raws {
            match &raw.kind {
                RawKind::Geom(g) => {
                    let geom = if m == Affine::IDENTITY {
                        Some(g.clone())
                    } else {
                        transform_geometry(g, m)
                    };
                    match geom {
                        Some(geom) => out.push(self.entity(geom, raw)),
                        None => self.log.degenerate_shapes += 1,
                    }
                }
                RawKind::Insert(rec) => {
                    self.convert_insert(rec, raw, m, target, stack, in_expansion, out);
                }
            }
        }
    }

    fn entity(&self, geom: Geometry, raw: &RawEntity) -> Entity {
        let layer = self
            .layers
            .get(&raw.layer)
            .copied()
            .unwrap_or(LayerId::ZERO);
        let mut e = Entity::new(geom, layer);
        e.color = raw.color;
        e
    }

    /// `INSERT` 1 件（MINSERT なら全部のマス）を変換する。
    #[allow(clippy::too_many_arguments)]
    fn convert_insert(
        &mut self,
        rec: &InsertRecord,
        raw: &RawEntity,
        m: Affine,
        target: Target,
        stack: &mut Vec<String>,
        in_expansion: bool,
        out: &mut Vec<Entity>,
    ) {
        let key = block_key(&rec.block);
        let Some(block) = self.table.blocks.get(&key).copied() else {
            if self.table.layouts.contains(&key) {
                self.log.layout_refs += 1;
            } else {
                self.log.undefined.add(&rec.block);
            }
            return;
        };
        if is_xref(block) {
            self.log.xrefs.add(&block.name);
            return;
        }
        // 循環を作る参照（`dependency_order` が見つけた後退辺）。いま中身を読んでいる
        // ブロック（定義の中身でも展開の中でも、スタックの一番上）からの参照で判定する。
        if let Some(current) = stack.last() {
            if self.back_edges.contains(&(current.clone(), key.clone())) {
                self.log.cycles.add(&block.name);
                return;
            }
        }
        if is_zero_len(rec.sx) || is_zero_len(rec.sy) {
            self.log.degenerate_inserts += 1;
            return;
        }
        if u64::from(rec.cols) * u64::from(rec.rows) > MAX_MINSERT_ITEMS {
            self.log.too_many_items += 1;
            return;
        }

        let xform = InsertXform {
            base: block.base,
            insert: rec.insert,
            sx: rec.sx,
            sy: rec.sy,
            rotation: rec.rotation,
            mirror_ocs: rec.mirror_ocs,
        };
        let def_id = self.def_ids.get(&key).copied();
        let mut expanded = false;

        for row in 0..rec.rows {
            for col in 0..rec.cols {
                let offset = Vec2::new(
                    f64::from(col) * rec.col_spacing,
                    f64::from(row) * rec.row_spacing,
                );
                let total = m * xform.matrix(offset);
                match (total.shape(), def_id) {
                    (Shape::Degenerate, _) => self.log.degenerate_inserts += 1,
                    (Shape::Similar { .. }, Some(id)) => {
                        if target.is_some_and(|t| would_create_cycle(t, id, self.defs)) {
                            self.log.cycles.add(&block.name);
                            continue;
                        }
                        let Some(placement) = total.placement(block.base) else {
                            self.log.degenerate_inserts += 1;
                            continue;
                        };
                        let inst = Geometry::Instance(Instance::new(id, placement));
                        out.push(self.entity(inst, raw));
                    }
                    // 縦横の倍率が違う（または定義にしていない）。中身を写して展開する。
                    _ => {
                        if stack.contains(&key) {
                            self.log.cycles.add(&block.name);
                            continue;
                        }
                        if stack.len() >= MAX_NESTING_DEPTH {
                            self.log.too_deep_expansion += 1;
                            continue;
                        }
                        stack.push(key.clone());
                        self.convert_all(&block.entities, total, target, stack, true, out);
                        stack.pop();
                        expanded = true;
                    }
                }
            }
        }
        if expanded && !in_expansion {
            self.log.expanded += 1;
        }
    }
}

/// 入れ子が [`MAX_NESTING_DEPTH`] 段を超える定義の数。
///
/// `component::resolve` はそれより深い部分を黙って描かない（無限再帰の歯止め）ので、
/// 読み込みの時点で警告しておく。`order` は内側が先の順。
fn count_too_deep(defs: &DefinitionTable, order: &[DefinitionId]) -> usize {
    let mut depth: HashMap<DefinitionId, usize> = HashMap::new();
    for &id in order {
        let Some(def) = defs.get(id) else {
            continue;
        };
        let inner = def
            .entities
            .iter()
            .filter_map(|e| match &e.geom {
                Geometry::Instance(i) => depth.get(&i.definition).copied(),
                _ => None,
            })
            .max()
            .unwrap_or(0);
        depth.insert(id, inner + 1);
    }
    depth.values().filter(|&&d| d > MAX_NESTING_DEPTH).count()
}

/// パース結果から実際の [`Document`] を、コマンド経由でのみ組み立てる。
///
/// `Document` のフィールドは private で、変更経路は [`Command`](crate::Command) しかない
/// （`cad-core` 全体の不変条件）。読み込みも例外ではなく、`AddLayer` /
/// `SetLayerProperties` / `DefineComponent` / `SetDefinitionContents` / `AddEntities` を
/// 順に適用してから [`Document::clear_history`] で Undo 履歴を消し、
/// まっさらな「開いた直後」の状態に整える。
pub(super) fn build_document(
    layer_records: Vec<LayerRecord>,
    entity_records: Vec<RawEntity>,
    entity_tally: Tally,
    block_records: Vec<BlockRecord>,
) -> Result<ReadReport> {
    let mut doc = Document::new();
    let mut log = Log::default();
    log.tally.absorb(&entity_tally);

    let table = BlockTable::new(&block_records, &mut log);
    let defined = table.to_define(&block_records, &entity_records);
    for b in &defined {
        log.tally.absorb(&b.tally);
    }

    let mut layers = LayerMap::new(&mut doc, &layer_records)?;
    layers.ensure(
        &mut doc,
        entity_records
            .iter()
            .chain(defined.iter().flat_map(|b| b.entities.iter())),
    )?;

    // 1. 定義の器（中身は空）を先に全部作る。前方参照があっても ID が引けるように。
    let mut def_ids: HashMap<String, DefinitionId> = HashMap::new();
    for b in &defined {
        doc.apply(Box::new(DefineComponent::new(
            COMMAND_NAME,
            b.name.clone(),
            b.base,
            Vec::new(),
        )))?;
        let id = doc
            .definitions()
            .by_name(&b.name)
            .ok_or(CadError::DefinitionNotFound)?;
        def_ids.insert(block_key(&b.name), id);
    }

    // 2. 内側が先の順に中身を入れる。循環になる参照は Converter が飛ばす。
    let (order, back_edges) = dependency_order(&defined, &entity_records);
    let mut order_ids = Vec::with_capacity(order.len());
    for b in order {
        let key = block_key(&b.name);
        let id = def_ids[&key];
        let mut contents = Vec::new();
        Converter {
            table: &table,
            def_ids: &def_ids,
            back_edges: &back_edges,
            layers: &layers.ids,
            defs: doc.definitions(),
            log: &mut log,
        }
        .convert_all(
            &b.entities,
            Affine::IDENTITY,
            Some(id),
            &mut vec![key],
            false,
            &mut contents,
        );
        doc.apply(Box::new(SetDefinitionContents::new(
            COMMAND_NAME,
            id,
            b.base,
            contents,
        )))?;
        order_ids.push(id);
    }

    // 3. 図面の図形。
    let mut entities = Vec::with_capacity(entity_records.len());
    Converter {
        table: &table,
        def_ids: &def_ids,
        back_edges: &back_edges,
        layers: &layers.ids,
        defs: doc.definitions(),
        log: &mut log,
    }
    .convert_all(
        &entity_records,
        Affine::IDENTITY,
        None,
        &mut Vec::new(),
        false,
        &mut entities,
    );
    if !entities.is_empty() {
        doc.apply(Box::new(AddEntities::many(COMMAND_NAME, entities)))?;
    }

    log.deep_definitions = count_too_deep(doc.definitions(), &order_ids);

    // 開いた直後のファイルは Undo 履歴を持たず、未保存扱いにもしない。
    doc.clear_history();
    doc.mark_saved(None);

    Ok(ReadReport {
        document: doc,
        warnings: log.into_warnings(),
    })
}
