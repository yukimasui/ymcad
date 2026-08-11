//! エンティティの境界ボックスを保持する四分木。
//!
//! 10,000 要素規模で線形探索を避けるための空間インデックス。
//! [`Document::revision`](crate::document::Document::revision) が変わるたびに
//! 呼び出し側が作り直す想定（このインデックス自身は変更を追跡しない）。
//!
//! # 実装方針
//!
//! 境界ボックスが分割境界をまたぐ要素は、子ノードへは複製せず親ノードに留める
//! （一般的で単純かつ正しい四分木の実装方針）。`query` は対象領域と交差する
//! ノードすべて（そのようにまたいでいる祖先ノードの要素も含む）を辿って集める。
//! false positive（過剰な候補）は許容するが、false negative（見落とし）は禁止。

use crate::document::Document;
use crate::entity::EntityId;
use crate::geom::{Aabb, Point2};

/// 1 ノードを子へ分割する条件となる要素数の閾値。
const SPLIT_THRESHOLD: usize = 8;
/// 分割の最大深さ。無限分割を防ぐ。
const MAX_DEPTH: u32 = 8;

/// エンティティの境界ボックスを保持する四分木。
#[derive(Debug)]
pub struct SpatialIndex {
    root: Node,
    len: usize,
}

#[derive(Debug)]
struct Node {
    /// このノードが担当する空間領域。
    bounds: Aabb,
    /// このノードに留まっている要素（子へ入りきらなかったもの、または葉ノードの全要素）。
    items: Vec<(EntityId, Aabb)>,
    /// 子ノード（左下・右下・左上・右上の順）。`None` なら葉ノード。
    children: Option<Box<[Node; 4]>>,
}

impl Node {
    fn new(bounds: Aabb) -> Self {
        Self {
            bounds,
            items: Vec::new(),
            children: None,
        }
    }

    fn insert(&mut self, id: EntityId, bbox: Aabb, depth: u32) {
        if let Some(children) = &mut self.children {
            if let Some(idx) = child_index_containing(self.bounds, bbox) {
                children[idx].insert(id, bbox, depth + 1);
                return;
            }
            // 境界をまたぐ（または bbox が空の）要素はこのノードに留める。
            self.items.push((id, bbox));
            return;
        }

        self.items.push((id, bbox));
        if self.items.len() > SPLIT_THRESHOLD && depth < MAX_DEPTH {
            self.split(depth);
        }
    }

    /// 葉ノードを 4 分割し、子へ入りきる要素だけを移す。
    fn split(&mut self, depth: u32) {
        let center = self.bounds.center();
        let min = self.bounds.min;
        let max = self.bounds.max;
        let quads = [
            Aabb::new(min, center),
            Aabb::new(Point2::new(center.x, min.y), Point2::new(max.x, center.y)),
            Aabb::new(Point2::new(min.x, center.y), Point2::new(center.x, max.y)),
            Aabb::new(center, max),
        ];
        let mut children = quads.map(Node::new);

        let old_items = std::mem::take(&mut self.items);
        for (id, bbox) in old_items {
            if let Some(idx) = child_index_containing(self.bounds, bbox) {
                children[idx].insert(id, bbox, depth + 1);
            } else {
                self.items.push((id, bbox));
            }
        }
        self.children = Some(Box::new(children));
    }

    fn query(&self, area: Aabb, out: &mut Vec<EntityId>) {
        if !self.bounds.intersects(&area) {
            return;
        }
        for (id, bbox) in &self.items {
            if bbox.intersects(&area) {
                out.push(*id);
            }
        }
        if let Some(children) = &self.children {
            for c in children.iter() {
                c.query(area, out);
            }
        }
    }
}

/// `bbox` が `bounds` を 4 分割したうちのどれか 1 つに完全に収まるなら、その添字を返す。
/// 境界をまたぐ場合、または `bbox` が空の場合は `None`。
///
/// 子の並びは [`Node::split`] の `quads` と対応させること
/// （0: 左下, 1: 右下, 2: 左上, 3: 右上）。
fn child_index_containing(bounds: Aabb, bbox: Aabb) -> Option<usize> {
    if bbox.is_empty() {
        return None;
    }
    let center = bounds.center();
    let left = bbox.max.x <= center.x;
    let right = bbox.min.x >= center.x;
    let bottom = bbox.max.y <= center.y;
    let top = bbox.min.y >= center.y;
    match (left, right, bottom, top) {
        (true, false, true, false) => Some(0),
        (false, true, true, false) => Some(1),
        (true, false, false, true) => Some(2),
        (false, true, false, true) => Some(3),
        _ => None,
    }
}

impl SpatialIndex {
    /// 図面全体から構築する。非表示レイヤの要素は入れない。
    #[must_use]
    pub fn build(doc: &Document) -> Self {
        let items: Vec<(EntityId, Aabb)> = doc
            .entities()
            .iter()
            .filter(|(_, e)| doc.layers().is_entity_visible(e))
            .map(|(id, e)| (id, e.bbox()))
            .collect();

        let bounds = items
            .iter()
            .fold(Aabb::EMPTY, |acc, (_, bbox)| acc.union(*bbox));
        let len = items.len();

        let mut root = Node::new(bounds);
        for (id, bbox) in items {
            root.insert(id, bbox, 0);
        }

        Self { root, len }
    }

    /// 登録されている要素数。
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// 要素が 1 つも無いか。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 指定範囲と bbox が交差する候補を返す（過剰に返してよい＝false positive 可、
    /// false negative は不可）。
    #[must_use]
    pub fn query(&self, area: Aabb) -> Vec<EntityId> {
        let mut out = Vec::new();
        self.root.query(area, &mut out);
        out
    }
}

impl Default for SpatialIndex {
    /// 空のインデックス。[`SpatialIndex::build`] を空の [`Document`] に対して
    /// 呼んだ場合と等価で、まだインデックスを一度も構築していない状態の
    /// プレースホルダとして使う（呼び出し側は `Document::revision()` を見て
    /// 実際のインデックスに差し替える）。
    fn default() -> Self {
        Self {
            root: Node::new(Aabb::EMPTY),
            len: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::AddEntities;
    use crate::entity::{Entity, Geometry};
    use crate::geom::{Circle, Line, Polyline};
    use crate::layer::{AciColor, Layer, LayerId};
    use crate::snap::test_util::Lcg;

    fn point_entity(p: Point2) -> Entity {
        Entity::new(Geometry::Circle(Circle::new(p, 1.0)), LayerId::ZERO)
    }

    fn add_all(doc: &mut Document, entities: Vec<Entity>) {
        doc.apply(Box::new(AddEntities::many("CIRCLE", entities)))
            .unwrap();
    }

    /// ブルートフォースで bbox が `area` と交差するエンティティを列挙する（テスト用の正解実装）。
    fn brute_force_query(doc: &Document, area: Aabb) -> Vec<EntityId> {
        doc.entities()
            .iter()
            .filter(|(_, e)| doc.layers().is_entity_visible(e))
            .filter(|(_, e)| e.bbox().intersects(&area))
            .map(|(id, _)| id)
            .collect()
    }

    #[test]
    fn default_is_equivalent_to_building_an_empty_document() {
        let idx = SpatialIndex::default();
        assert_eq!(idx.len(), 0);
        assert!(idx.is_empty());
        let area = Aabb::new(Point2::new(-100.0, -100.0), Point2::new(100.0, 100.0));
        assert!(idx.query(area).is_empty());
    }

    #[test]
    fn build_empty_document_has_zero_len() {
        let doc = Document::new();
        let idx = SpatialIndex::build(&doc);
        assert_eq!(idx.len(), 0);
        assert!(idx.is_empty());
    }

    #[test]
    fn query_on_empty_document_returns_empty() {
        let doc = Document::new();
        let idx = SpatialIndex::build(&doc);
        let area = Aabb::new(Point2::new(-100.0, -100.0), Point2::new(100.0, 100.0));
        assert!(idx.query(area).is_empty());
    }

    #[test]
    fn build_single_entity() {
        let mut doc = Document::new();
        add_all(&mut doc, vec![point_entity(Point2::new(5.0, 5.0))]);
        let idx = SpatialIndex::build(&doc);
        assert_eq!(idx.len(), 1);
        assert!(!idx.is_empty());

        let hit = idx.query(Aabb::new(Point2::new(0.0, 0.0), Point2::new(10.0, 10.0)));
        assert_eq!(hit.len(), 1);
    }

    #[test]
    fn query_misses_far_away_area() {
        let mut doc = Document::new();
        add_all(&mut doc, vec![point_entity(Point2::new(0.0, 0.0))]);
        let idx = SpatialIndex::build(&doc);

        let far = Aabb::new(Point2::new(1000.0, 1000.0), Point2::new(1001.0, 1001.0));
        assert!(idx.query(far).is_empty());
    }

    /// テスト専用: レイヤを追加して非表示にするコマンド。
    ///
    /// レイヤの変更は [`crate::command::EditCtx`] 経由でしか行えないため、
    /// 最小のコマンドとして定義する（Undo は使わないので no-op でよい）。
    #[derive(Debug)]
    struct AddHiddenLayer {
        name: &'static str,
    }
    impl crate::command::Command for AddHiddenLayer {
        fn execute(&mut self, ctx: &mut crate::command::EditCtx<'_>) -> crate::error::Result<()> {
            let id = ctx.add_layer(Layer::new(self.name, AciColor::WHITE));
            ctx.layer_mut(id)?.visible = false;
            Ok(())
        }
        fn undo(&mut self, _ctx: &mut crate::command::EditCtx<'_>) -> crate::error::Result<()> {
            Ok(())
        }
        fn name(&self) -> &'static str {
            "TEST_HIDDEN_LAYER"
        }
    }

    #[test]
    fn build_skips_invisible_layer_entities() {
        let mut doc = Document::new();
        doc.apply(Box::new(AddHiddenLayer { name: "HIDDEN" }))
            .unwrap();
        let hidden = doc.layers().by_name("HIDDEN").expect("直前に作成したはず");

        let visible_entity = point_entity(Point2::new(0.0, 0.0));
        let hidden_entity = Entity::new(
            Geometry::Circle(Circle::new(Point2::new(0.0, 0.0), 1.0)),
            hidden,
        );
        add_all(&mut doc, vec![visible_entity, hidden_entity]);

        let idx = SpatialIndex::build(&doc);
        assert_eq!(idx.len(), 1, "非表示レイヤの要素は入らないこと");
    }

    #[test]
    fn all_entities_at_one_point_does_not_panic() {
        let mut doc = Document::new();
        let entities: Vec<_> = (0..50)
            .map(|_| point_entity(Point2::new(3.0, 3.0)))
            .collect();
        add_all(&mut doc, entities);

        let idx = SpatialIndex::build(&doc);
        assert_eq!(idx.len(), 50);

        let area = Aabb::new(Point2::new(2.0, 2.0), Point2::new(4.0, 4.0));
        let hit = idx.query(area);
        assert_eq!(hit.len(), 50, "全件がヒットするはず");
    }

    #[test]
    fn empty_bbox_entity_does_not_crash_and_never_matches() {
        let mut doc = Document::new();
        let empty_polyline = Entity::new(
            Geometry::Polyline(Polyline::new(vec![], false)),
            LayerId::ZERO,
        );
        let normal = point_entity(Point2::new(0.0, 0.0));
        add_all(&mut doc, vec![empty_polyline, normal]);

        let idx = SpatialIndex::build(&doc);
        assert_eq!(idx.len(), 2);

        let area = Aabb::new(Point2::new(-1000.0, -1000.0), Point2::new(1000.0, 1000.0));
        let hit = idx.query(area);
        // 空 bbox のエンティティはどんな範囲とも交差しないので 1 件だけヒットする。
        assert_eq!(hit.len(), 1);
    }

    #[test]
    fn large_coordinates_are_found() {
        let mut doc = Document::new();
        add_all(&mut doc, vec![point_entity(Point2::new(1e6, -1e6))]);
        let idx = SpatialIndex::build(&doc);

        let area = Aabb::new(
            Point2::new(1e6 - 10.0, -1e6 - 10.0),
            Point2::new(1e6 + 10.0, -1e6 + 10.0),
        );
        assert_eq!(idx.query(area).len(), 1);
    }

    #[test]
    fn query_boundary_touching_area_is_included() {
        let mut doc = Document::new();
        add_all(&mut doc, vec![point_entity(Point2::new(10.0, 10.0))]);
        let idx = SpatialIndex::build(&doc);

        // 要素の bbox は (9,9)-(11,11)。ちょうど角で接する範囲。
        let area = Aabb::new(Point2::new(11.0, 11.0), Point2::new(20.0, 20.0));
        assert_eq!(idx.query(area).len(), 1);
    }

    #[test]
    fn deep_split_still_finds_correct_entities() {
        // 分割閾値・最大深さを超える件数を密集させて分割を強制する。
        let mut doc = Document::new();
        let mut rng = Lcg::new(7);
        let entities: Vec<_> = (0..500)
            .map(|_| {
                point_entity(Point2::new(
                    rng.next_f64(-50.0, 50.0),
                    rng.next_f64(-50.0, 50.0),
                ))
            })
            .collect();
        add_all(&mut doc, entities);

        let idx = SpatialIndex::build(&doc);
        assert_eq!(idx.len(), 500);

        let area = Aabb::new(Point2::new(-5.0, -5.0), Point2::new(5.0, 5.0));
        let mut got = idx.query(area);
        got.sort();
        let mut expected = brute_force_query(&doc, area);
        expected.sort();

        // superset であること(過剰候補は許容するので contains で確認)。
        for id in &expected {
            assert!(got.contains(id));
        }
    }

    /// 四分木の核心的な正しさ: どんな範囲・どんな配置でもブルートフォースの
    /// 結果を包含する（false negative が無い）こと。
    #[test]
    fn randomized_query_is_superset_of_brute_force() {
        let mut rng = Lcg::new(12345);
        let mut doc = Document::new();

        let mut entities = Vec::new();
        for i in 0..2000u32 {
            let x = rng.next_f64(-1000.0, 1000.0);
            let y = rng.next_f64(-1000.0, 1000.0);
            if i % 3 == 0 {
                entities.push(Entity::new(
                    Geometry::Line(Line::new(
                        Point2::new(x, y),
                        Point2::new(x + rng.next_f64(-20.0, 20.0), y + rng.next_f64(-20.0, 20.0)),
                    )),
                    LayerId::ZERO,
                ));
            } else {
                entities.push(point_entity(Point2::new(x, y)));
            }
        }
        add_all(&mut doc, entities);
        let idx = SpatialIndex::build(&doc);

        for _ in 0..30 {
            let cx = rng.next_f64(-1000.0, 1000.0);
            let cy = rng.next_f64(-1000.0, 1000.0);
            let half = rng.next_f64(1.0, 200.0);
            let area = Aabb::new(
                Point2::new(cx - half, cy - half),
                Point2::new(cx + half, cy + half),
            );

            let mut got = idx.query(area);
            got.sort();
            got.dedup();
            let mut expected = brute_force_query(&doc, area);
            expected.sort();

            for id in &expected {
                assert!(
                    got.contains(id),
                    "四分木が見落とした: {id:?}, area={area:?}"
                );
            }
        }
    }
}
