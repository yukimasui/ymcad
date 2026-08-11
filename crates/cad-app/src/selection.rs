//! 選択とヒットテスト。

use std::collections::BTreeSet;

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
        let d = entity.geom.dist_to(pos);
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

/// 矩形に掛かるエンティティを集める。
#[must_use]
pub fn pick_in_rect(doc: &Document, rect: Aabb, mode: WindowMode) -> Vec<EntityId> {
    doc.entities()
        .iter()
        .filter(|(_, e)| doc.layers().is_entity_editable(e))
        .filter(|(_, e)| match mode {
            WindowMode::Window => rect.contains_aabb(&e.bbox()),
            WindowMode::Crossing => crosses_rect(&e.geom, rect),
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
#[must_use]
pub fn crosses_rect(geom: &Geometry, rect: Aabb) -> bool {
    if rect.is_empty() {
        return false;
    }

    // 境界ボックスすら重ならないなら確実に掛かっていない（安価な足切り）。
    if !rect.intersects(&geom.bbox()) {
        return false;
    }

    // 完全に内側なら掛かっている。
    if rect.contains_aabb(&geom.bbox()) {
        return true;
    }

    rect_edges(rect)
        .iter()
        .any(|edge| !intersections(edge, geom).is_empty())
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
fn intersections(edge: &Line, geom: &Geometry) -> Vec<Point2> {
    match geom {
        Geometry::Line(l) => intersect::line_line(edge, l),
        Geometry::Circle(c) => intersect::line_circle(edge, c),
        Geometry::Arc(a) => intersect::line_arc(edge, a),
        Geometry::Polyline(p) => p
            .segments()
            .flat_map(|seg| intersect::line_line(edge, &seg))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
            c.bbox().intersects(&corner),
            "前提: bbox は重なっている（この判定だけだと誤検出する）"
        );
        assert!(!crosses_rect(&c, corner), "円周には掛かっていない");

        // 円周をまたぐ矩形は掛かっている。
        assert!(crosses_rect(&c, rect(9.0, -1.0, 11.0, 1.0)));
        // 円を完全に含む矩形も掛かっている。
        assert!(crosses_rect(&c, rect(-20.0, -20.0, 20.0, 20.0)));
    }

    #[test]
    fn crossing_handles_polyline() {
        let p = Geometry::Polyline(Polyline::rectangle(
            Point2::new(0.0, 0.0),
            Point2::new(10.0, 10.0),
        ));
        // 左辺だけをまたぐ矩形。
        assert!(crosses_rect(&p, rect(-1.0, 4.0, 1.0, 6.0)));
        // ポリラインの内側だけにある矩形は、辺に触れないので掛かっていない。
        assert!(!crosses_rect(&p, rect(4.0, 4.0, 6.0, 6.0)));
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
