//! カーソル近傍のスナップ候補の検出。

use crate::document::Document;
use crate::entity::{EntityId, Geometry};
use crate::geom::intersect::{
    arc_arc, circle_arc, circle_circle, line_arc, line_circle, line_line,
};
use crate::geom::tolerance::{gt_len, is_zero_len, lt_len};
use crate::geom::{Aabb, Arc, Circle, Line, Point2, Polyline};

use super::index::SpatialIndex;
use super::{SnapCandidate, SnapKind, SnapModes};

/// 1 回の検出の設定。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnapQuery {
    /// カーソル位置（モデル座標）。
    pub cursor: Point2,
    /// 拾い半径（モデル空間）。呼び出し側が画面 px から換算して渡す。
    pub radius: f64,
    /// 有効なスナップ種類。
    pub modes: SnapModes,
    /// 垂線スナップの基準点（直前に確定した点）。無ければ垂線は検出しない。
    pub from: Option<Point2>,
}

/// カーソル近傍のスナップ候補を検出する。
///
/// 戻り値は「最も良い候補が先頭」の順。良さは (優先順位, 距離) の辞書順で決める。
#[must_use]
pub fn detect(doc: &Document, index: &SpatialIndex, q: &SnapQuery) -> Vec<SnapCandidate> {
    let mut out = Vec::new();

    let area = Aabb::new(q.cursor, q.cursor).expanded(q.radius);
    let ids = index.query(area);

    // 交点検出用に、近傍のエンティティを線分/円/円弧へ平坦化して集める
    // （ポリラインはセグメントごとの `Line` に分解する）。
    let mut curves: Vec<(EntityId, Curve)> = Vec::new();

    for id in ids {
        let Some(entity) = doc.entities().get(id) else {
            continue;
        };
        if !doc.layers().is_entity_visible(entity) {
            continue;
        }

        collect_point_candidates(&mut out, q, id, &entity.geom);

        if q.modes.is_enabled(SnapKind::Intersection) {
            push_curves(id, &entity.geom, &mut curves);
        }
    }

    if q.modes.is_enabled(SnapKind::Intersection) {
        collect_intersections(&mut out, q, &curves);
    }

    out.sort_by(|a, b| {
        a.kind
            .priority()
            .cmp(&b.kind.priority())
            .then_with(|| a.distance.total_cmp(&b.distance))
    });
    out
}

/// 最も良い候補 1 つ。
#[must_use]
pub fn detect_best(doc: &Document, index: &SpatialIndex, q: &SnapQuery) -> Option<SnapCandidate> {
    detect(doc, index, q).into_iter().next()
}

/// `q.radius` 以内なら候補を積む。
fn maybe_push(
    out: &mut Vec<SnapCandidate>,
    q: &SnapQuery,
    entity: EntityId,
    kind: SnapKind,
    point: Point2,
) {
    let distance = q.cursor.dist(point);
    if !gt_len(distance, q.radius) {
        out.push(SnapCandidate {
            point,
            kind,
            entity,
            distance,
        });
    }
}

/// エンティティ 1 つぶんの端点・中点・中心・垂線・最近点候補を集める。
fn collect_point_candidates(
    out: &mut Vec<SnapCandidate>,
    q: &SnapQuery,
    id: EntityId,
    geom: &Geometry,
) {
    match geom {
        Geometry::Line(l) => {
            if q.modes.is_enabled(SnapKind::Endpoint) {
                maybe_push(out, q, id, SnapKind::Endpoint, l.a);
                maybe_push(out, q, id, SnapKind::Endpoint, l.b);
            }
            if q.modes.is_enabled(SnapKind::Midpoint) {
                maybe_push(out, q, id, SnapKind::Midpoint, l.midpoint());
            }
            if q.modes.is_enabled(SnapKind::Perpendicular) {
                if let Some(base) = q.from {
                    if let Some(p) = perpendicular_foot_on_line(l, base) {
                        maybe_push(out, q, id, SnapKind::Perpendicular, p);
                    }
                }
            }
            if q.modes.is_enabled(SnapKind::Nearest) {
                maybe_push(out, q, id, SnapKind::Nearest, l.closest_point(q.cursor));
            }
        }
        Geometry::Circle(c) => {
            if q.modes.is_enabled(SnapKind::Center) {
                maybe_push(out, q, id, SnapKind::Center, c.center);
            }
            if q.modes.is_enabled(SnapKind::Perpendicular) {
                if let Some(base) = q.from {
                    if let Some(p) = perpendicular_foot_on_circle(c, base) {
                        maybe_push(out, q, id, SnapKind::Perpendicular, p);
                    }
                }
            }
            if q.modes.is_enabled(SnapKind::Nearest) {
                maybe_push(
                    out,
                    q,
                    id,
                    SnapKind::Nearest,
                    nearest_on_circle(c, q.cursor),
                );
            }
        }
        Geometry::Arc(a) => {
            if q.modes.is_enabled(SnapKind::Endpoint) {
                maybe_push(out, q, id, SnapKind::Endpoint, a.start_point());
                maybe_push(out, q, id, SnapKind::Endpoint, a.end_point());
            }
            if q.modes.is_enabled(SnapKind::Midpoint) {
                maybe_push(out, q, id, SnapKind::Midpoint, a.mid_point());
            }
            if q.modes.is_enabled(SnapKind::Center) {
                maybe_push(out, q, id, SnapKind::Center, a.center);
            }
            if q.modes.is_enabled(SnapKind::Perpendicular) {
                if let Some(base) = q.from {
                    if let Some(p) = perpendicular_foot_on_arc(a, base) {
                        maybe_push(out, q, id, SnapKind::Perpendicular, p);
                    }
                }
            }
            if q.modes.is_enabled(SnapKind::Nearest) {
                maybe_push(out, q, id, SnapKind::Nearest, nearest_on_arc(a, q.cursor));
            }
        }
        Geometry::Polyline(pl) => {
            if q.modes.is_enabled(SnapKind::Endpoint) {
                for v in &pl.vertices {
                    maybe_push(out, q, id, SnapKind::Endpoint, *v);
                }
            }
            if q.modes.is_enabled(SnapKind::Midpoint) {
                for s in pl.segments() {
                    maybe_push(out, q, id, SnapKind::Midpoint, s.midpoint());
                }
            }
            if q.modes.is_enabled(SnapKind::Perpendicular) {
                if let Some(base) = q.from {
                    for s in pl.segments() {
                        if let Some(p) = perpendicular_foot_on_line(&s, base) {
                            maybe_push(out, q, id, SnapKind::Perpendicular, p);
                        }
                    }
                }
            }
            if q.modes.is_enabled(SnapKind::Nearest) {
                if let Some(p) = nearest_on_polyline(pl, q.cursor) {
                    maybe_push(out, q, id, SnapKind::Nearest, p);
                }
            }
        }
    }
}

/// パラメータ `t` が `[0, 1]` にトレランス込みで収まるか（線分の範囲判定用）。
///
/// `geom::intersect` の同名の内部ヘルパーと同じ考え方。あちらは非公開なので
/// ここで作り直す。
fn in_unit_range(t: f64) -> bool {
    !lt_len(t, 0.0) && !gt_len(t, 1.0)
}

/// 線分 `l` への `base` からの垂線の足。垂線の足が線分の範囲外なら `None`。
fn perpendicular_foot_on_line(l: &Line, base: Point2) -> Option<Point2> {
    let t = l.closest_param(base);
    if in_unit_range(t) {
        Some(l.point_at(t))
    } else {
        None
    }
}

/// 円 `c` の中心から `base` を通る直線が円周と交わる点。`base` が中心なら `None`。
fn perpendicular_foot_on_circle(c: &Circle, base: Point2) -> Option<Point2> {
    if is_zero_len(base.dist(c.center)) {
        return None;
    }
    let angle = (base - c.center).angle();
    Some(c.point_at_angle(angle))
}

/// 円弧 `a` の中心から `base` を通る直線が弧と交わる点。
/// `base` が中心の場合、または交点が掃引範囲外の場合は `None`。
fn perpendicular_foot_on_arc(a: &Arc, base: Point2) -> Option<Point2> {
    if is_zero_len(base.dist(a.center)) {
        return None;
    }
    let angle = (base - a.center).angle();
    if a.contains_angle(angle) {
        Some(Circle::new(a.center, a.radius).point_at_angle(angle))
    } else {
        None
    }
}

/// カーソルから円への最近点。
///
/// カーソルがちょうど中心と一致する場合、方向は不定だが `atan2(0, 0) = 0` により
/// 角度 0 の点を返す（NaN にはならない、決定的な挙動）。
fn nearest_on_circle(c: &Circle, cursor: Point2) -> Point2 {
    c.point_at_angle((cursor - c.center).angle())
}

/// カーソルから円弧への最近点。円としての最近点が掃引範囲外なら、
/// より近い方の端点にクランプする。
fn nearest_on_arc(a: &Arc, cursor: Point2) -> Point2 {
    let angle = (cursor - a.center).angle();
    if a.contains_angle(angle) {
        Circle::new(a.center, a.radius).point_at_angle(angle)
    } else {
        let sp = a.start_point();
        let ep = a.end_point();
        if !gt_len(cursor.dist(sp), cursor.dist(ep)) {
            sp
        } else {
            ep
        }
    }
}

/// カーソルからポリラインへの最近点。線分を持たない（頂点 0〜1）場合は `None`。
fn nearest_on_polyline(pl: &Polyline, cursor: Point2) -> Option<Point2> {
    let mut best: Option<(Point2, f64)> = None;
    for s in pl.segments() {
        let p = s.closest_point(cursor);
        let d = cursor.dist(p);
        best = match best {
            Some((_, bd)) if !gt_len(d, bd) => Some((p, d)),
            Some(b) => Some(b),
            None => Some((p, d)),
        };
    }
    best.map(|(p, _)| p)
}

/// 交点検出のために平坦化した図形。ポリラインはセグメントごとの `Line` になる。
#[derive(Clone, Copy, Debug)]
enum Curve {
    Line(Line),
    Circle(Circle),
    Arc(Arc),
}

/// エンティティを [`Curve`] へ平坦化して `out` に積む。
fn push_curves(id: EntityId, geom: &Geometry, out: &mut Vec<(EntityId, Curve)>) {
    match geom {
        Geometry::Line(l) => out.push((id, Curve::Line(*l))),
        Geometry::Circle(c) => out.push((id, Curve::Circle(*c))),
        Geometry::Arc(a) => out.push((id, Curve::Arc(*a))),
        Geometry::Polyline(pl) => {
            for s in pl.segments() {
                out.push((id, Curve::Line(s)));
            }
        }
    }
}

/// 2 つの [`Curve`] の交点。
fn curve_intersect(a: &Curve, b: &Curve) -> Vec<Point2> {
    match (a, b) {
        (Curve::Line(l1), Curve::Line(l2)) => line_line(l1, l2),
        (Curve::Line(l), Curve::Circle(c)) | (Curve::Circle(c), Curve::Line(l)) => {
            line_circle(l, c)
        }
        (Curve::Line(l), Curve::Arc(ar)) | (Curve::Arc(ar), Curve::Line(l)) => line_arc(l, ar),
        (Curve::Circle(c1), Curve::Circle(c2)) => circle_circle(c1, c2),
        (Curve::Circle(c), Curve::Arc(ar)) | (Curve::Arc(ar), Curve::Circle(c)) => {
            circle_arc(c, ar)
        }
        (Curve::Arc(a1), Curve::Arc(a2)) => arc_arc(a1, a2),
    }
}

/// 近傍のエンティティ組すべての交点候補を集める。異なるエンティティ同士のみを対象にする
/// （同一エンティティの隣接セグメント同士が端点で「交わる」ことを交点として扱わないため）。
fn collect_intersections(
    out: &mut Vec<SnapCandidate>,
    q: &SnapQuery,
    curves: &[(EntityId, Curve)],
) {
    for i in 0..curves.len() {
        for j in (i + 1)..curves.len() {
            let (id_a, ca) = &curves[i];
            let (id_b, cb) = &curves[j];
            if id_a == id_b {
                continue;
            }
            for p in curve_intersect(ca, cb) {
                maybe_push(out, q, *id_a, SnapKind::Intersection, p);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::AddEntities;
    use crate::entity::Entity;
    use crate::geom::tolerance::eq_len;
    use crate::layer::LayerId;
    use crate::snap::test_util::Lcg;
    use std::f64::consts::{FRAC_PI_2, PI};
    use std::time::{Duration, Instant};

    fn line_entity(a: (f64, f64), b: (f64, f64)) -> Entity {
        Entity::new(
            Geometry::Line(Line::new(Point2::new(a.0, a.1), Point2::new(b.0, b.1))),
            LayerId::ZERO,
        )
    }

    fn circle_entity(center: (f64, f64), r: f64) -> Entity {
        Entity::new(
            Geometry::Circle(Circle::new(Point2::new(center.0, center.1), r)),
            LayerId::ZERO,
        )
    }

    fn arc_entity(center: (f64, f64), r: f64, start: f64, end: f64) -> Entity {
        Entity::new(
            Geometry::Arc(Arc::new(Point2::new(center.0, center.1), r, start, end)),
            LayerId::ZERO,
        )
    }

    fn polyline_entity(verts: &[(f64, f64)], closed: bool) -> Entity {
        Entity::new(
            Geometry::Polyline(Polyline::new(
                verts.iter().map(|&(x, y)| Point2::new(x, y)).collect(),
                closed,
            )),
            LayerId::ZERO,
        )
    }

    fn doc_with(entities: Vec<Entity>) -> Document {
        let mut doc = Document::new();
        doc.apply(Box::new(AddEntities::many("TEST", entities)))
            .unwrap();
        doc
    }

    fn q(cursor: (f64, f64), radius: f64) -> SnapQuery {
        SnapQuery {
            cursor: Point2::new(cursor.0, cursor.1),
            radius,
            modes: SnapModes::all(),
            from: None,
        }
    }

    fn q_with_from(cursor: (f64, f64), radius: f64, from: (f64, f64)) -> SnapQuery {
        SnapQuery {
            from: Some(Point2::new(from.0, from.1)),
            ..q(cursor, radius)
        }
    }

    fn contains_point(cands: &[SnapCandidate], kind: SnapKind, p: (f64, f64)) -> bool {
        let p = Point2::new(p.0, p.1);
        cands.iter().any(|c| c.kind == kind && c.point.eq_tol(p))
    }

    // ---- 基本の候補生成 -----------------------------------------------------

    #[test]
    fn endpoint_on_line() {
        let doc = doc_with(vec![line_entity((0.0, 0.0), (10.0, 0.0))]);
        let idx = SpatialIndex::build(&doc);
        let cands = detect(&doc, &idx, &q((0.1, 0.1), 5.0));
        assert!(contains_point(&cands, SnapKind::Endpoint, (0.0, 0.0)));
    }

    #[test]
    fn midpoint_on_line() {
        let doc = doc_with(vec![line_entity((0.0, 0.0), (10.0, 0.0))]);
        let idx = SpatialIndex::build(&doc);
        let cands = detect(&doc, &idx, &q((5.1, 0.1), 5.0));
        assert!(contains_point(&cands, SnapKind::Midpoint, (5.0, 0.0)));
    }

    #[test]
    fn endpoint_on_arc() {
        let a = Arc::new(Point2::ORIGIN, 5.0, 0.0, PI);
        let doc = doc_with(vec![Entity::new(Geometry::Arc(a), LayerId::ZERO)]);
        let idx = SpatialIndex::build(&doc);
        let cands = detect(&doc, &idx, &q((5.1, 0.1), 5.0));
        assert!(contains_point(&cands, SnapKind::Endpoint, (5.0, 0.0)));
        let cands2 = detect(&doc, &idx, &q((-5.1, 0.1), 5.0));
        assert!(contains_point(&cands2, SnapKind::Endpoint, (-5.0, 0.0)));
    }

    #[test]
    fn midpoint_on_arc() {
        let a = Arc::new(Point2::ORIGIN, 5.0, 0.0, PI);
        let doc = doc_with(vec![Entity::new(Geometry::Arc(a), LayerId::ZERO)]);
        let idx = SpatialIndex::build(&doc);
        let cands = detect(&doc, &idx, &q((0.1, 5.1), 5.0));
        assert!(contains_point(&cands, SnapKind::Midpoint, (0.0, 5.0)));
    }

    #[test]
    fn center_on_arc() {
        let doc = doc_with(vec![arc_entity((3.0, 4.0), 5.0, 0.0, PI)]);
        let idx = SpatialIndex::build(&doc);
        let cands = detect(&doc, &idx, &q((3.1, 4.1), 5.0));
        assert!(contains_point(&cands, SnapKind::Center, (3.0, 4.0)));
    }

    #[test]
    fn center_on_circle() {
        let doc = doc_with(vec![circle_entity((2.0, 2.0), 3.0)]);
        let idx = SpatialIndex::build(&doc);
        let cands = detect(&doc, &idx, &q((2.1, 2.1), 5.0));
        assert!(contains_point(&cands, SnapKind::Center, (2.0, 2.0)));
    }

    #[test]
    fn circle_has_no_endpoint_or_midpoint_candidates() {
        let doc = doc_with(vec![circle_entity((0.0, 0.0), 5.0)]);
        let idx = SpatialIndex::build(&doc);
        let cands = detect(&doc, &idx, &q((5.0, 0.0), 5.0));
        assert!(!cands.iter().any(|c| c.kind == SnapKind::Endpoint));
        assert!(!cands.iter().any(|c| c.kind == SnapKind::Midpoint));
    }

    #[test]
    fn endpoint_on_polyline_every_vertex() {
        let doc = doc_with(vec![polyline_entity(
            &[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)],
            false,
        )]);
        let idx = SpatialIndex::build(&doc);
        let cands = detect(&doc, &idx, &q((10.1, 0.1), 5.0));
        assert!(contains_point(&cands, SnapKind::Endpoint, (10.0, 0.0)));
    }

    #[test]
    fn midpoint_on_polyline_each_segment() {
        let doc = doc_with(vec![polyline_entity(
            &[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)],
            false,
        )]);
        let idx = SpatialIndex::build(&doc);
        let cands = detect(&doc, &idx, &q((5.1, 0.1), 5.0));
        assert!(contains_point(&cands, SnapKind::Midpoint, (5.0, 0.0)));
        let cands2 = detect(&doc, &idx, &q((10.1, 5.1), 5.0));
        assert!(contains_point(&cands2, SnapKind::Midpoint, (10.0, 5.0)));
    }

    #[test]
    fn candidates_outside_radius_are_excluded() {
        let doc = doc_with(vec![line_entity((0.0, 0.0), (10.0, 0.0))]);
        let idx = SpatialIndex::build(&doc);
        // カーソルは端点から半径よりずっと遠い。
        let cands = detect(&doc, &idx, &q((0.0, 0.0), 100.0));
        assert!(!cands.is_empty());
        let cands_far = detect(&doc, &idx, &q((0.0, 1000.0), 1.0));
        assert!(cands_far.is_empty());
    }

    // ---- 優先順位 -----------------------------------------------------------

    #[test]
    fn detect_orders_by_priority_when_coincident_distance() {
        // 端点かつ (別の要素の) 中心が同じ座標に重なるケース。
        let doc = doc_with(vec![
            line_entity((0.0, 0.0), (10.0, 0.0)),
            circle_entity((0.0, 0.0), 3.0),
        ]);
        let idx = SpatialIndex::build(&doc);
        let cands = detect(&doc, &idx, &q((0.0, 0.0), 1.0));
        assert!(cands.len() >= 2);
        // 先頭は Endpoint（Center より優先度が高い）。
        assert_eq!(cands[0].kind, SnapKind::Endpoint);
    }

    #[test]
    fn detect_best_returns_first_of_detect() {
        let doc = doc_with(vec![line_entity((0.0, 0.0), (10.0, 0.0))]);
        let idx = SpatialIndex::build(&doc);
        let query = q((0.1, 0.1), 5.0);
        let all = detect(&doc, &idx, &query);
        let best = detect_best(&doc, &idx, &query);
        assert_eq!(best, all.into_iter().next());
    }

    #[test]
    fn modes_disable_specific_kind() {
        let doc = doc_with(vec![line_entity((0.0, 0.0), (10.0, 0.0))]);
        let idx = SpatialIndex::build(&doc);
        let mut modes = SnapModes::all();
        modes.set(SnapKind::Endpoint, false);
        let query = SnapQuery {
            cursor: Point2::new(0.1, 0.1),
            radius: 5.0,
            modes,
            from: None,
        };
        let cands = detect(&doc, &idx, &query);
        assert!(!cands.iter().any(|c| c.kind == SnapKind::Endpoint));
    }

    // ---- 交点 -----------------------------------------------------------

    #[test]
    fn intersection_line_line() {
        let doc = doc_with(vec![
            line_entity((0.0, 0.0), (10.0, 10.0)),
            line_entity((0.0, 10.0), (10.0, 0.0)),
        ]);
        let idx = SpatialIndex::build(&doc);
        let cands = detect(&doc, &idx, &q((5.1, 5.1), 5.0));
        assert!(contains_point(&cands, SnapKind::Intersection, (5.0, 5.0)));
    }

    #[test]
    fn intersection_line_circle() {
        let doc = doc_with(vec![
            line_entity((-10.0, 0.0), (10.0, 0.0)),
            circle_entity((0.0, 0.0), 5.0),
        ]);
        let idx = SpatialIndex::build(&doc);
        let cands = detect(&doc, &idx, &q((5.1, 0.1), 5.0));
        assert!(contains_point(&cands, SnapKind::Intersection, (5.0, 0.0)));
    }

    #[test]
    fn intersection_circle_circle() {
        let doc = doc_with(vec![
            circle_entity((-1.0, 0.0), 2.0),
            circle_entity((1.0, 0.0), 2.0),
        ]);
        let idx = SpatialIndex::build(&doc);
        let cands = detect(&doc, &idx, &q((0.0, 1.7), 5.0));
        assert!(cands.iter().any(|c| c.kind == SnapKind::Intersection));
    }

    #[test]
    fn intersection_line_arc() {
        // 上半円の円弧と、それを貫く水平線分。
        let doc = doc_with(vec![
            arc_entity((0.0, 0.0), 5.0, 0.0, PI),
            line_entity((-10.0, 3.0), (10.0, 3.0)),
        ]);
        let idx = SpatialIndex::build(&doc);
        let x = (25.0_f64 - 9.0).sqrt(); // r=5, y=3 -> x=4
        let cands = detect(&doc, &idx, &q((x + 0.1, 3.1), 5.0));
        assert!(cands.iter().any(|c| c.kind == SnapKind::Intersection
            && eq_len(c.point.y, 3.0)
            && c.point.x > 0.0));
    }

    #[test]
    fn intersection_skips_same_entity_adjacent_segments() {
        // 同一ポリラインの隣接セグメントの共有端点は Intersection として重複計上しない。
        let doc = doc_with(vec![polyline_entity(
            &[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)],
            false,
        )]);
        let idx = SpatialIndex::build(&doc);
        let cands = detect(&doc, &idx, &q((10.1, 0.1), 5.0));
        assert!(!cands.iter().any(|c| c.kind == SnapKind::Intersection));
    }

    // ---- 垂線 -----------------------------------------------------------

    #[test]
    fn perpendicular_none_when_from_is_none() {
        let doc = doc_with(vec![line_entity((0.0, -10.0), (0.0, 10.0))]);
        let idx = SpatialIndex::build(&doc);
        let cands = detect(&doc, &idx, &q((0.1, 0.1), 5.0));
        assert!(!cands.iter().any(|c| c.kind == SnapKind::Perpendicular));
    }

    #[test]
    fn perpendicular_foot_on_line_within_segment() {
        let doc = doc_with(vec![line_entity((0.0, -10.0), (0.0, 10.0))]);
        let idx = SpatialIndex::build(&doc);
        let query = q_with_from((0.1, 5.1), 5.0, (5.0, 5.0));
        let cands = detect(&doc, &idx, &query);
        assert!(contains_point(&cands, SnapKind::Perpendicular, (0.0, 5.0)));
    }

    #[test]
    fn perpendicular_skipped_when_foot_outside_segment() {
        // 線分は y in [-10, 10]。基準点 (5, 100) からの垂線の足は (0, 100) で範囲外。
        let doc = doc_with(vec![line_entity((0.0, -10.0), (0.0, 10.0))]);
        let idx = SpatialIndex::build(&doc);
        let query = SnapQuery {
            cursor: Point2::new(0.0, 100.0),
            radius: 200.0,
            modes: SnapModes::all(),
            from: Some(Point2::new(5.0, 100.0)),
        };
        let cands = detect(&doc, &idx, &query);
        assert!(!cands.iter().any(|c| c.kind == SnapKind::Perpendicular));
    }

    #[test]
    fn perpendicular_on_circle() {
        let doc = doc_with(vec![circle_entity((0.0, 0.0), 5.0)]);
        let idx = SpatialIndex::build(&doc);
        let query = q_with_from((5.1, 0.1), 5.0, (20.0, 0.0));
        let cands = detect(&doc, &idx, &query);
        assert!(contains_point(&cands, SnapKind::Perpendicular, (5.0, 0.0)));
    }

    #[test]
    fn perpendicular_on_arc() {
        let doc = doc_with(vec![arc_entity((0.0, 0.0), 5.0, 0.0, PI)]);
        let idx = SpatialIndex::build(&doc);
        let query = q_with_from((0.1, 5.1), 5.0, (0.0, 20.0));
        let cands = detect(&doc, &idx, &query);
        assert!(contains_point(&cands, SnapKind::Perpendicular, (0.0, 5.0)));
    }

    #[test]
    fn perpendicular_on_arc_skipped_outside_sweep() {
        // 下半分方向 (0,-20) からの垂線の足は (0,-5) だが、上半円の弧の掃引範囲外。
        let doc = doc_with(vec![arc_entity((0.0, 0.0), 5.0, 0.0, PI)]);
        let idx = SpatialIndex::build(&doc);
        let query = SnapQuery {
            cursor: Point2::new(0.0, -5.0),
            radius: 5.0,
            modes: SnapModes::all(),
            from: Some(Point2::new(0.0, -20.0)),
        };
        let cands = detect(&doc, &idx, &query);
        assert!(!cands.iter().any(|c| c.kind == SnapKind::Perpendicular));
    }

    #[test]
    fn perpendicular_skipped_when_base_is_circle_center() {
        let doc = doc_with(vec![circle_entity((0.0, 0.0), 5.0)]);
        let idx = SpatialIndex::build(&doc);
        let query = q_with_from((5.0, 0.0), 5.0, (0.0, 0.0));
        let cands = detect(&doc, &idx, &query);
        assert!(!cands.iter().any(|c| c.kind == SnapKind::Perpendicular));
    }

    #[test]
    fn perpendicular_on_polyline_segment() {
        let doc = doc_with(vec![polyline_entity(&[(0.0, -10.0), (0.0, 10.0)], false)]);
        let idx = SpatialIndex::build(&doc);
        let query = q_with_from((0.1, 5.1), 5.0, (5.0, 5.0));
        let cands = detect(&doc, &idx, &query);
        assert!(contains_point(&cands, SnapKind::Perpendicular, (0.0, 5.0)));
    }

    // ---- 最近点 -----------------------------------------------------------

    #[test]
    fn nearest_on_line() {
        let doc = doc_with(vec![line_entity((0.0, 0.0), (10.0, 0.0))]);
        let idx = SpatialIndex::build(&doc);
        let cands = detect(&doc, &idx, &q((3.0, 2.0), 5.0));
        assert!(contains_point(&cands, SnapKind::Nearest, (3.0, 0.0)));
    }

    #[test]
    fn nearest_on_circle() {
        let doc = doc_with(vec![circle_entity((0.0, 0.0), 5.0)]);
        let idx = SpatialIndex::build(&doc);
        let cands = detect(&doc, &idx, &q((10.0, 0.0), 6.0));
        assert!(contains_point(&cands, SnapKind::Nearest, (5.0, 0.0)));
    }

    #[test]
    fn nearest_on_arc_clamps_to_sweep() {
        // 上半円弧。カーソルが下方向 (掃引範囲外) にあると、最近点は近い方の端点にクランプされる。
        let doc = doc_with(vec![arc_entity((0.0, 0.0), 5.0, 0.0, PI)]);
        let idx = SpatialIndex::build(&doc);
        // 円としての最近点は (5,0) 付近の方向だが、下方向 (5, -1) からは
        // 掃引範囲外なので端点 (5, 0) にクランプされるはず。
        let cands = detect(&doc, &idx, &q((6.0, -0.5), 5.0));
        assert!(contains_point(&cands, SnapKind::Nearest, (5.0, 0.0)));
    }

    #[test]
    fn nearest_on_arc_inside_sweep_projects_normally() {
        let doc = doc_with(vec![arc_entity((0.0, 0.0), 5.0, 0.0, PI)]);
        let idx = SpatialIndex::build(&doc);
        let cands = detect(&doc, &idx, &q((0.0, 10.0), 6.0));
        assert!(contains_point(&cands, SnapKind::Nearest, (0.0, 5.0)));
    }

    #[test]
    fn nearest_on_polyline() {
        let doc = doc_with(vec![polyline_entity(
            &[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)],
            false,
        )]);
        let idx = SpatialIndex::build(&doc);
        let cands = detect(&doc, &idx, &q((3.0, 2.0), 5.0));
        assert!(contains_point(&cands, SnapKind::Nearest, (3.0, 0.0)));
    }

    // ---- レイヤ・空文書 -----------------------------------------------------

    #[test]
    fn invisible_layer_entity_produces_no_candidates() {
        use crate::layer::{AciColor, Layer};

        #[derive(Debug)]
        struct AddHiddenLayer {
            name: &'static str,
        }
        impl crate::command::Command for AddHiddenLayer {
            fn execute(
                &mut self,
                ctx: &mut crate::command::EditCtx<'_>,
            ) -> crate::error::Result<()> {
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

        let mut doc = Document::new();
        doc.apply(Box::new(AddHiddenLayer { name: "HIDDEN" }))
            .unwrap();
        let hidden = doc.layers().by_name("HIDDEN").unwrap();
        let mut e = line_entity((0.0, 0.0), (10.0, 0.0));
        e.layer = hidden;
        doc.apply(Box::new(AddEntities::one("LINE", e))).unwrap();

        let idx = SpatialIndex::build(&doc);
        let cands = detect(&doc, &idx, &q((0.0, 0.0), 5.0));
        assert!(cands.is_empty());
    }

    #[test]
    fn detect_on_empty_document_returns_empty() {
        let doc = Document::new();
        let idx = SpatialIndex::build(&doc);
        let cands = detect(&doc, &idx, &q((0.0, 0.0), 5.0));
        assert!(cands.is_empty());
    }

    #[test]
    fn radius_zero_only_matches_exact_point() {
        let doc = doc_with(vec![line_entity((0.0, 0.0), (10.0, 0.0))]);
        let idx = SpatialIndex::build(&doc);
        let exact = detect(&doc, &idx, &q((0.0, 0.0), 0.0));
        assert!(contains_point(&exact, SnapKind::Endpoint, (0.0, 0.0)));

        // カーソルが線分から離れていれば（Nearest も含めて）半径 0 では何も拾わない。
        // (0.5, 0.0) のように線上の点を選ぶと Nearest がカーソルそのものを
        // 距離 0 で拾ってしまうため、意図的に線分から外れた点を使う。
        let miss = detect(&doc, &idx, &q((0.5, 1.0), 0.0));
        assert!(miss.is_empty());
    }

    #[test]
    fn large_coordinates_detection() {
        let doc = doc_with(vec![line_entity((1e6, 1e6), (1e6 + 10.0, 1e6))]);
        let idx = SpatialIndex::build(&doc);
        let cands = detect(&doc, &idx, &q((1e6 + 0.1, 1e6 + 0.1), 5.0));
        assert!(contains_point(&cands, SnapKind::Endpoint, (1e6, 1e6)));
    }

    #[test]
    fn arc_start_and_end_points_use_perpendicular_direction() {
        // sanity: `FRAC_PI_2` を使った 1/4 円でも中点・端点が正しく出ること。
        let doc = doc_with(vec![arc_entity((0.0, 0.0), 4.0, 0.0, FRAC_PI_2)]);
        let idx = SpatialIndex::build(&doc);
        let cands = detect(&doc, &idx, &q((0.1, 4.1), 5.0));
        assert!(contains_point(&cands, SnapKind::Endpoint, (0.0, 4.0)));
    }

    // ---- ランダム化・大量データ ---------------------------------------------

    #[test]
    fn randomized_nearest_is_always_within_radius() {
        let mut rng = Lcg::new(99);
        let mut entities = Vec::new();
        for _ in 0..100 {
            let x = rng.next_f64(-500.0, 500.0);
            let y = rng.next_f64(-500.0, 500.0);
            entities.push(line_entity((x, y), (x + 10.0, y)));
        }
        let doc = doc_with(entities);
        let idx = SpatialIndex::build(&doc);

        for _ in 0..20 {
            let cursor = (rng.next_f64(-500.0, 500.0), rng.next_f64(-500.0, 500.0));
            let cands = detect(&doc, &idx, &q(cursor, 50.0));
            for c in &cands {
                assert!(!gt_len(c.distance, 50.0), "半径外の候補が混入した: {c:?}");
            }
        }
    }

    /// 性能確認（受け入れ基準）: 10,000 エンティティに対して 1 回の検出が
    /// 平均 16ms 未満であること。
    ///
    /// デバッグビルドは最適化が効かないため実測値は悲観的（release よりかなり遅い）。
    /// アサーションは複数回の平均で判定しており、単発のブレでは落ちない。
    #[test]
    fn snap_detection_meets_frame_budget() {
        let mut rng = Lcg::new(2024);
        let mut entities = Vec::with_capacity(10_000);
        for i in 0..10_000u32 {
            let cx = rng.next_f64(-1e5, 1e5);
            let cy = rng.next_f64(-1e5, 1e5);
            let geom = match i % 4 {
                0 => Geometry::Line(Line::new(
                    Point2::new(cx, cy),
                    Point2::new(cx + 10.0, cy + 10.0),
                )),
                1 => Geometry::Circle(Circle::new(Point2::new(cx, cy), 5.0)),
                2 => Geometry::Arc(Arc::new(Point2::new(cx, cy), 5.0, 0.0, PI)),
                _ => Geometry::Polyline(Polyline::new(
                    vec![
                        Point2::new(cx, cy),
                        Point2::new(cx + 5.0, cy),
                        Point2::new(cx + 5.0, cy + 5.0),
                    ],
                    false,
                )),
            };
            entities.push(Entity::new(geom, LayerId::ZERO));
        }
        let doc = doc_with(entities);
        let index = SpatialIndex::build(&doc);
        assert_eq!(index.len(), 10_000);

        let iterations = 100u32;
        let mut total = Duration::ZERO;
        for _ in 0..iterations {
            let cursor = (rng.next_f64(-1e5, 1e5), rng.next_f64(-1e5, 1e5));
            let query = SnapQuery {
                cursor: Point2::new(cursor.0, cursor.1),
                radius: 50.0,
                modes: SnapModes::all(),
                from: Some(Point2::new(cursor.0 - 20.0, cursor.1)),
            };
            let start = Instant::now();
            let _ = detect(&doc, &index, &query);
            total += start.elapsed();
        }
        let avg = total / iterations;
        println!("snap detect average over {iterations} iterations: {avg:?}");
        assert!(
            avg.as_millis() < 16,
            "average snap detection took {avg:?}, expected < 16ms"
        );
    }
}
