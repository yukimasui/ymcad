//! 選択とヒットテスト。

use std::collections::BTreeSet;

use cad_core::component::{self, DefinitionTable};
use cad_core::geom::{intersect, Aabb, Line, Point2};
use cad_core::{Document, EntityId, Geometry};

/// 選択中のエンティティ。
///
/// `BTreeSet` なので走査順は `EntityId` 昇順（= 作成順）で決定的。
#[derive(Debug, Default, Clone)]
pub struct Selection {
    ids: BTreeSet<EntityId>,
}

impl Selection {
    /// 空の選択。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 選択数。
    #[must_use]
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    /// 何も選択されていないか。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// 含まれているか。
    #[must_use]
    pub fn contains(&self, id: EntityId) -> bool {
        self.ids.contains(&id)
    }

    /// 走査する。
    pub fn iter(&self) -> impl Iterator<Item = EntityId> + '_ {
        self.ids.iter().copied()
    }

    /// `Vec` として取り出す。コマンドへ渡すときに使う。
    #[must_use]
    pub fn to_vec(&self) -> Vec<EntityId> {
        self.ids.iter().copied().collect()
    }

    /// 追加する。
    pub fn insert(&mut self, id: EntityId) {
        self.ids.insert(id);
    }

    /// 取り除く。
    pub fn remove(&mut self, id: EntityId) {
        self.ids.remove(&id);
    }

    /// すべて解除する。
    pub fn clear(&mut self) {
        self.ids.clear();
    }

    /// 消えたエンティティを選択から外す。
    ///
    /// Undo / Redo で図面が変わった後に呼ぶ。世代が変わった ID もここで落ちる。
    pub fn retain_existing(&mut self, doc: &Document) {
        self.ids.retain(|id| doc.entities().contains(*id));
    }

    /// 編集できなくなったエンティティ（削除・ロック・非表示のレイヤ）を選択から外す。
    /// 選ぶときと同じ判定（`is_entity_editable`）を使う。
    ///
    /// パネルの操作など、実行中のコマンドの外で図面が変わった後に呼ぶ（ADR-0039）。
    pub fn retain_editable(&mut self, doc: &Document) {
        self.ids.retain(|id| is_editable(doc, *id));
    }
}

/// エンティティがあり、編集できるレイヤ（表示中でロックされていない）にあるか。
#[must_use]
pub fn is_editable(doc: &Document, id: EntityId) -> bool {
    doc.entities()
        .get(id)
        .is_some_and(|e| doc.layers().is_entity_editable(e))
}

/// 窓選択の種類。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowMode {
    /// 左→右ドラッグ。矩形に **完全に含まれる** ものだけ選ぶ。
    Window,
    /// 右→左ドラッグ。矩形に **少しでも掛かる** ものを選ぶ。
    Crossing,
}

impl WindowMode {
    /// ドラッグの向きから決める。左から右なら窓選択。
    #[must_use]
    pub fn from_drag(from_x: f64, to_x: f64) -> Self {
        if to_x >= from_x {
            Self::Window
        } else {
            Self::Crossing
        }
    }
}

/// クリック位置にあるエンティティを 1 つ拾う。
///
/// `tolerance` はモデル空間での拾い半径（画面上で一定になるよう、
/// 呼び出し側が `Viewport::px_to_model_len` で換算して渡すこと）。
///
/// 複数が範囲内にある場合は **最も近いもの**、距離が同じなら後から作られた
/// （= 手前に描かれる）ものを選ぶ。
#[must_use]
pub fn pick_at(doc: &Document, pos: Point2, tolerance: f64) -> Option<EntityId> {
    let mut best: Option<(EntityId, f64)> = None;
    for (id, entity) in doc.entities().iter() {
        // 非表示・ロックされたレイヤの要素は選択対象外。
        if !doc.layers().is_entity_editable(entity) {
            continue;
        }
        let d = entity.geom.dist_to(doc.definitions(), pos);
        if d > tolerance {
            continue;
        }
        match best {
            // 同距離なら後勝ち = 手前のものが選ばれる。
            Some((_, bd)) if d > bd => {}
            _ => best = Some((id, d)),
        }
    }
    best.map(|(id, _)| id)
}

/// `id` と同じグループに属する要素のうち、**編集できるものだけ**を返す。
///
/// グループに属していなければ `id` 自身だけ。
///
/// AutoCAD の既定と同じく、**グループの一員を選ぶと全体が選ばれる**。ただし
/// ロック・非表示のレイヤにある一員は入らない（Issue #51、ADR-0022）。
/// 編集できるかは [`is_editable`] と同じ判定なので、クリック直後の選択と、
/// パネル操作・UNDO / REDO 後の [`Selection::retain_editable`] の結果が一致する。
/// 所属はエンティティ側が持っているので、走査して求める。
///
/// 前提: `id` は `pick_at` / `pick_in_rect` が返した、編集できる図形。
/// 編集できない `id` を渡すと、`id` 自身は結果に含まれない
/// （グループ所属なら他の編集できる一員だけ、所属なしなら空）。
#[must_use]
pub fn expand_to_group(doc: &Document, id: EntityId) -> Vec<EntityId> {
    let Some(group) = doc.entities().get(id).and_then(|e| e.group) else {
        return if is_editable(doc, id) {
            vec![id]
        } else {
            Vec::new()
        };
    };
    doc.entities()
        .iter()
        .filter(|(_, e)| e.group == Some(group) && doc.layers().is_entity_editable(e))
        .map(|(other, _)| other)
        .collect()
}

/// 矩形に掛かるエンティティを集める。
#[must_use]
pub fn pick_in_rect(doc: &Document, rect: Aabb, mode: WindowMode) -> Vec<EntityId> {
    doc.entities()
        .iter()
        .filter(|(_, e)| doc.layers().is_entity_editable(e))
        .filter(|(_, e)| match mode {
            WindowMode::Window => rect.contains_aabb(&e.bbox(doc.definitions())),
            WindowMode::Crossing => crosses_rect(&e.geom, rect, doc.definitions()),
        })
        .map(|(id, _)| id)
        .collect()
}

/// 図形が矩形に掛かるか（交差選択の判定）。
///
/// 境界ボックスの重なりだけで判定すると、円のように bbox に対して隙間の多い図形で
/// 「掛かっていないのに選ばれる」ことが起きる。そこで
///
/// 1. 図形全体が矩形に収まっていれば掛かっている
/// 2. そうでなければ矩形の 4 辺と実際に交点を持つか調べる
///
/// の 2 段で厳密に判定する。
/// `defs` はコンポーネントインスタンスを中身へ展開するために必要
/// （[`cad_core::Geometry::bbox`] と同じ理由）。
#[must_use]
pub fn crosses_rect(geom: &Geometry, rect: Aabb, defs: &DefinitionTable) -> bool {
    if rect.is_empty() {
        return false;
    }

    // 境界ボックスすら重ならないなら確実に掛かっていない（安価な足切り）。
    if !rect.intersects(&geom.bbox(defs)) {
        return false;
    }

    // 完全に内側なら掛かっている。
    if rect.contains_aabb(&geom.bbox(defs)) {
        return true;
    }

    rect_edges(rect)
        .iter()
        .any(|edge| !intersections(edge, geom, defs).is_empty())
}

/// 矩形の 4 辺。
fn rect_edges(rect: Aabb) -> [Line; 4] {
    let (min, max) = (rect.min, rect.max);
    let bl = min;
    let br = Point2::new(max.x, min.y);
    let tr = max;
    let tl = Point2::new(min.x, max.y);
    [
        Line::new(bl, br),
        Line::new(br, tr),
        Line::new(tr, tl),
        Line::new(tl, bl),
    ]
}

/// 線分と図形の交点。
fn intersections(edge: &Line, geom: &Geometry, defs: &DefinitionTable) -> Vec<Point2> {
    match geom {
        Geometry::Line(l) => intersect::line_line(edge, l),
        Geometry::Circle(c) => intersect::line_circle(edge, c),
        Geometry::Arc(a) => intersect::line_arc(edge, a),
        Geometry::Xline(x) => intersect::xline_line(x, edge),
        Geometry::Polyline(p) => p
            .segments()
            .flat_map(|seg| intersect::line_line(edge, &seg))
            .collect(),
        // 中身をワールド座標へ展開して、その各図形との交点を集める。
        // 展開結果にインスタンスは含まれないので、再帰は 1 段で止まる。
        Geometry::Instance(i) => component::resolve(i, defs)
            .iter()
            .flat_map(|g| intersections(edge, g, defs))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// テスト用の空の定義テーブル。
    ///
    /// ここのテストはコンポーネントを含まないので空でよい。
    /// インスタンスの選択は `resolved.rs` と結合テストで確かめる。
    fn defs() -> DefinitionTable {
        DefinitionTable::new()
    }
    use cad_core::command::AddEntities;
    use cad_core::geom::{Circle, Polyline};
    use cad_core::{Entity, LayerId};

    fn doc_with(geoms: Vec<Geometry>) -> Document {
        let mut d = Document::new();
        let entities = geoms
            .into_iter()
            .map(|g| Entity::new(g, LayerId::ZERO))
            .collect();
        d.apply(Box::new(AddEntities::many("TEST", entities)))
            .unwrap();
        d
    }

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Aabb {
        Aabb::new(Point2::new(x0, y0), Point2::new(x1, y1))
    }

    fn line(x0: f64, y0: f64, x1: f64, y1: f64) -> Geometry {
        Geometry::Line(Line::new(Point2::new(x0, y0), Point2::new(x1, y1)))
    }

    #[test]
    fn window_mode_follows_drag_direction() {
        assert_eq!(WindowMode::from_drag(0.0, 10.0), WindowMode::Window);
        assert_eq!(WindowMode::from_drag(10.0, 0.0), WindowMode::Crossing);
    }

    #[test]
    fn selection_basic_operations() {
        let d = doc_with(vec![line(0.0, 0.0, 1.0, 0.0)]);
        let id = d.entities().ids().next().unwrap();

        let mut s = Selection::new();
        assert!(s.is_empty());
        s.insert(id);
        assert!(s.contains(id) && s.len() == 1);
        s.remove(id);
        assert!(s.is_empty());
    }

    /// Undo などで消えた要素が選択に残らないこと。
    #[test]
    fn retain_existing_drops_deleted_entities() {
        let mut d = doc_with(vec![line(0.0, 0.0, 1.0, 0.0)]);
        let id = d.entities().ids().next().unwrap();
        let mut s = Selection::new();
        s.insert(id);

        d.undo().unwrap(); // 追加を取り消す
        s.retain_existing(&d);
        assert!(s.is_empty(), "消えた要素は選択から外れること");
    }

    /// ロック・非表示のレイヤの要素は、選ぶときと同じく選択から外れる（ADR-0039）。
    #[test]
    fn retain_editable_drops_locked_and_hidden_entities() {
        use cad_core::command::SetLayerProperties;
        for lock in [true, false] {
            let mut d = doc_with(vec![line(0.0, 0.0, 1.0, 0.0)]);
            let id = d.entities().ids().next().unwrap();
            let mut s = Selection::new();
            s.insert(id);
            s.retain_editable(&d);
            assert_eq!(s.len(), 1, "編集できるうちは残る");

            let cmd = if lock {
                SetLayerProperties::new(LayerId::ZERO).locked(true)
            } else {
                SetLayerProperties::new(LayerId::ZERO).visible(false)
            };
            d.apply(Box::new(cmd)).unwrap();
            s.retain_editable(&d);
            assert!(s.is_empty(), "ロック {lock}: 外れる");
            assert!(!is_editable(&d, id));
        }
    }

    /// グループの一員のうち、ロック・非表示のレイヤにあるものは展開結果に入らない（Issue #51）。
    #[test]
    fn expand_to_group_skips_locked_and_hidden_members() {
        use cad_core::command::{AddLayer, CreateGroup, SetLayerProperties};
        use cad_core::layer::AciColor;
        for op in [None, Some(true), Some(false)] {
            let mut d = Document::new();
            d.apply(Box::new(AddLayer::new("L1", AciColor::WHITE)))
                .unwrap();
            let l1 = d.layers().by_name("L1").unwrap();
            let ents = vec![
                Entity::new(line(0.0, 0.0, 1.0, 0.0), LayerId::ZERO),
                Entity::new(line(0.0, 1.0, 1.0, 1.0), l1),
            ];
            d.apply(Box::new(AddEntities::many("TEST", ents))).unwrap();
            let ids: Vec<_> = d.entities().ids().collect();
            d.apply(Box::new(CreateGroup::new("GROUP", "g", ids.clone())))
                .unwrap();

            match op {
                None => {}
                Some(lock) => {
                    let cmd = if lock {
                        SetLayerProperties::new(l1).locked(true)
                    } else {
                        SetLayerProperties::new(l1).visible(false)
                    };
                    d.apply(Box::new(cmd)).unwrap();
                }
            }
            let got = expand_to_group(&d, ids[0]);
            if op.is_none() {
                assert_eq!(got.len(), 2, "両方編集できれば両方返る");
                assert!(got.contains(&ids[0]) && got.contains(&ids[1]));
            } else {
                assert_eq!(got, vec![ids[0]], "{op:?}: 編集できない一員は含まれない");
            }
            // retain_editable と同じ結果になる。
            let mut s = Selection::new();
            for id in &ids {
                s.insert(*id);
            }
            s.retain_editable(&d);
            let mut a = got.clone();
            a.sort();
            let mut b = s.to_vec();
            b.sort();
            assert_eq!(a, b, "{op:?}: 後から確かめ直した選択と一致する");
        }
    }

    #[test]
    fn pick_at_finds_entity_within_tolerance() {
        let d = doc_with(vec![line(0.0, 0.0, 10.0, 0.0)]);
        let id = d.entities().ids().next().unwrap();

        assert_eq!(pick_at(&d, Point2::new(5.0, 0.4), 0.5), Some(id));
        assert_eq!(pick_at(&d, Point2::new(5.0, 2.0), 0.5), None);
    }

    /// 重なっている場合は後から作られた（手前の）ものが選ばれること。
    #[test]
    fn pick_at_prefers_topmost_on_tie() {
        let d = doc_with(vec![line(0.0, 0.0, 10.0, 0.0), line(0.0, 0.0, 10.0, 0.0)]);
        let ids: Vec<_> = d.entities().ids().collect();
        assert_eq!(pick_at(&d, Point2::new(5.0, 0.0), 0.5), Some(ids[1]));
    }

    /// より近いものが優先されること。
    #[test]
    fn pick_at_prefers_nearest() {
        let d = doc_with(vec![line(0.0, 0.0, 10.0, 0.0), line(0.0, 1.0, 10.0, 1.0)]);
        let ids: Vec<_> = d.entities().ids().collect();
        assert_eq!(pick_at(&d, Point2::new(5.0, 0.1), 2.0), Some(ids[0]));
        assert_eq!(pick_at(&d, Point2::new(5.0, 0.9), 2.0), Some(ids[1]));
    }

    /// 窓選択は完全に内包されるものだけ、交差選択は掛かるものすべて。
    #[test]
    fn window_requires_containment_crossing_does_not() {
        let d = doc_with(vec![
            line(1.0, 1.0, 2.0, 2.0),     // 内側
            line(1.0, 1.0, 100.0, 1.0),   // またぐ
            line(50.0, 50.0, 60.0, 60.0), // 外側
        ]);
        let ids: Vec<_> = d.entities().ids().collect();
        let r = rect(0.0, 0.0, 10.0, 10.0);

        assert_eq!(pick_in_rect(&d, r, WindowMode::Window), vec![ids[0]]);
        assert_eq!(
            pick_in_rect(&d, r, WindowMode::Crossing),
            vec![ids[0], ids[1]]
        );
    }

    /// 円のように bbox に隙間がある図形で、bbox だけの判定では誤検出すること、
    /// そして実装がそれを避けていることを確認する。
    #[test]
    fn crossing_is_exact_for_circles() {
        let c = Geometry::Circle(Circle::new(Point2::ORIGIN, 10.0));
        // 円の左上の「角」にある小さな矩形。bbox とは重なるが円周とは交わらない。
        let corner = rect(-10.0, 9.5, -9.5, 10.0);
        assert!(
            c.bbox(&defs()).intersects(&corner),
            "前提: bbox は重なっている（この判定だけだと誤検出する）"
        );
        assert!(!crosses_rect(&c, corner, &defs()), "円周には掛かっていない");

        // 円周をまたぐ矩形は掛かっている。
        assert!(crosses_rect(&c, rect(9.0, -1.0, 11.0, 1.0), &defs()));
        // 円を完全に含む矩形も掛かっている。
        assert!(crosses_rect(&c, rect(-20.0, -20.0, 20.0, 20.0), &defs()));
    }

    #[test]
    fn crossing_handles_polyline() {
        let p = Geometry::Polyline(Polyline::rectangle(
            Point2::new(0.0, 0.0),
            Point2::new(10.0, 10.0),
        ));
        // 左辺だけをまたぐ矩形。
        assert!(crosses_rect(&p, rect(-1.0, 4.0, 1.0, 6.0), &defs()));
        // ポリラインの内側だけにある矩形は、辺に触れないので掛かっていない。
        assert!(!crosses_rect(&p, rect(4.0, 4.0, 6.0, 6.0), &defs()));
    }

    #[test]
    fn empty_rect_selects_nothing() {
        let d = doc_with(vec![line(0.0, 0.0, 1.0, 1.0)]);
        assert!(pick_in_rect(&d, Aabb::EMPTY, WindowMode::Crossing).is_empty());
        assert!(pick_in_rect(&d, Aabb::EMPTY, WindowMode::Window).is_empty());
    }

    /// ロックされたレイヤの要素は選択できないこと。
    #[test]
    fn locked_layer_entities_are_not_selectable() {
        use cad_core::command::{Command, EditCtx};
        use cad_core::error::Result;

        /// テスト用にレイヤをロックするコマンド。
        #[derive(Debug)]
        struct LockLayer(LayerId);
        impl Command for LockLayer {
            fn execute(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
                ctx.layer_mut(self.0)?.locked = true;
                Ok(())
            }
            fn undo(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
                ctx.layer_mut(self.0)?.locked = false;
                Ok(())
            }
            fn name(&self) -> &'static str {
                "LOCK"
            }
        }

        let mut d = doc_with(vec![line(0.0, 0.0, 10.0, 0.0)]);
        d.apply(Box::new(LockLayer(LayerId::ZERO))).unwrap();

        assert_eq!(pick_at(&d, Point2::new(5.0, 0.0), 1.0), None);
        assert!(pick_in_rect(&d, rect(-1.0, -1.0, 11.0, 1.0), WindowMode::Crossing).is_empty());
    }
}
