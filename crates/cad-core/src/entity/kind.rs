//! 図形要素の定義。

use crate::geom::{Aabb, Arc, Circle, Line, Point2, Polyline, Vec2};
use crate::layer::{ColorSpec, LayerId};

/// 図形の実体。
#[derive(Clone, Debug, PartialEq)]
pub enum Geometry {
    /// 線分。
    Line(Line),
    /// 円。
    Circle(Circle),
    /// 円弧。
    Arc(Arc),
    /// ポリライン（連結した線分列）。
    Polyline(Polyline),
}

impl Geometry {
    /// 境界ボックス。
    #[must_use]
    pub fn bbox(&self) -> Aabb {
        match self {
            Self::Line(l) => l.bbox(),
            Self::Circle(c) => c.bbox(),
            Self::Arc(a) => a.bbox(),
            Self::Polyline(p) => p.bbox(),
        }
    }

    /// 点との最短距離。ピックやスナップの判定に使う。
    #[must_use]
    pub fn dist_to(&self, p: Point2) -> f64 {
        match self {
            Self::Line(l) => l.dist_to(p),
            Self::Circle(c) => c.dist_to(p),
            Self::Arc(a) => a.dist_to(p),
            Self::Polyline(pl) => pl.dist_to(p),
        }
    }

    /// コマンド名などに使う種別名。
    #[must_use]
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Line(_) => "LINE",
            Self::Circle(_) => "CIRCLE",
            Self::Arc(_) => "ARC",
            // DXF R12 での LWPOLYLINE 相当の名前（Phase 6 の DXF 出力で使う）。
            Self::Polyline(_) => "LWPOLYLINE",
        }
    }

    /// 平行移動した複製を作る。
    #[must_use]
    pub fn translated(&self, v: Vec2) -> Self {
        match self {
            Self::Line(l) => Self::Line(l.translated(v)),
            Self::Circle(c) => Self::Circle(c.translated(v)),
            Self::Arc(a) => Self::Arc(a.translated(v)),
            Self::Polyline(p) => Self::Polyline(p.translated(v)),
        }
    }

    /// `regions` のいずれかに入っている定義点だけを `delta` ぶん動かす。
    ///
    /// `regions` が空のときは図形全体を平行移動する（MOVE と同じ）。
    /// AutoCAD は交差窓を複数回重ねられるので、範囲は 1 つではなくスライスで受ける。
    #[must_use]
    pub fn stretched(&self, regions: &[Aabb], delta: Vec2) -> Self {
        if regions.is_empty() {
            return self.translated(delta);
        }
        let inside = |p: Point2| regions.iter().any(|r| r.contains(p));
        let move_if_inside = |p: Point2| if inside(p) { p + delta } else { p };
        match self {
            Self::Line(l) => Self::Line(Line::new(move_if_inside(l.a), move_if_inside(l.b))),
            // 中心が範囲内のときだけ図形全体を動かす。半径・角度は不変。
            Self::Circle(c) => {
                if inside(c.center) {
                    Self::Circle(c.translated(delta))
                } else {
                    Self::Circle(*c)
                }
            }
            Self::Arc(a) => {
                if inside(a.center) {
                    Self::Arc(a.translated(delta))
                } else {
                    Self::Arc(*a)
                }
            }
            Self::Polyline(p) => {
                let vertices = p.vertices.iter().copied().map(move_if_inside).collect();
                Self::Polyline(Polyline::new(vertices, p.closed))
            }
        }
    }
}

/// 図面を構成する 1 要素。
#[derive(Clone, Debug, PartialEq)]
pub struct Entity {
    /// 図形。
    pub geom: Geometry,
    /// 所属レイヤ。
    pub layer: LayerId,
    /// 色。既定はレイヤの色に従う。
    pub color: ColorSpec,
}

impl Entity {
    /// レイヤの色に従う要素を作る。
    #[must_use]
    pub fn new(geom: Geometry, layer: LayerId) -> Self {
        Self {
            geom,
            layer,
            color: ColorSpec::ByLayer,
        }
    }

    /// 境界ボックス。
    #[must_use]
    pub fn bbox(&self) -> Aabb {
        self.geom.bbox()
    }

    /// 平行移動した複製を作る（レイヤ・色は変わらない）。
    #[must_use]
    pub fn translated(&self, v: Vec2) -> Self {
        Self {
            geom: self.geom.translated(v),
            layer: self.layer,
            color: self.color,
        }
    }

    /// `regions` に入っている定義点だけを動かした複製を作る（レイヤ・色は変わらない）。
    #[must_use]
    pub fn stretched(&self, regions: &[Aabb], delta: Vec2) -> Self {
        Self {
            geom: self.geom.stretched(regions, delta),
            layer: self.layer,
            color: self.color,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::tolerance::eq_len;
    use crate::geom::{Point2 as P, Vec2 as V};

    fn line_geom(a: (f64, f64), b: (f64, f64)) -> Geometry {
        Geometry::Line(Line::new(P::new(a.0, a.1), P::new(b.0, b.1)))
    }

    #[test]
    fn geometry_bbox_dispatches_to_polyline() {
        let g = Geometry::Polyline(Polyline::rectangle(P::new(0.0, 0.0), P::new(2.0, 3.0)));
        let b = g.bbox();
        assert_eq!(b.min, P::new(0.0, 0.0));
        assert_eq!(b.max, P::new(2.0, 3.0));
    }

    #[test]
    fn geometry_dist_to_dispatches_to_polyline() {
        let g = Geometry::Polyline(Polyline::new(
            vec![P::new(0.0, 0.0), P::new(10.0, 0.0)],
            false,
        ));
        assert!(eq_len(g.dist_to(P::new(5.0, 3.0)), 3.0));
    }

    #[test]
    fn geometry_type_name_polyline_is_lwpolyline() {
        let g = Geometry::Polyline(Polyline::new(vec![], false));
        assert_eq!(g.type_name(), "LWPOLYLINE");
    }

    #[test]
    fn geometry_type_name_other_variants() {
        assert_eq!(line_geom((0.0, 0.0), (1.0, 1.0)).type_name(), "LINE");
        assert_eq!(
            Geometry::Circle(Circle::new(P::ORIGIN, 1.0)).type_name(),
            "CIRCLE"
        );
        assert_eq!(
            Geometry::Arc(Arc::new(P::ORIGIN, 1.0, 0.0, 1.0)).type_name(),
            "ARC"
        );
    }

    #[test]
    fn geometry_translated_line() {
        let g = line_geom((0.0, 0.0), (1.0, 1.0));
        let moved = g.translated(V::new(5.0, 5.0));
        assert_eq!(moved, line_geom((5.0, 5.0), (6.0, 6.0)));
    }

    #[test]
    fn entity_translated_keeps_layer_and_color() {
        let e = Entity::new(line_geom((0.0, 0.0), (1.0, 0.0)), LayerId::ZERO);
        let moved = e.translated(V::new(3.0, 4.0));
        assert_eq!(moved.layer, e.layer);
        assert_eq!(moved.color, e.color);
        assert_eq!(moved.geom, line_geom((3.0, 4.0), (4.0, 4.0)));
    }

    // ---- stretched --------------------------------------------------------

    /// `(0,0)-(10,10)` の範囲。
    fn region() -> Aabb {
        Aabb::new(P::new(0.0, 0.0), P::new(10.0, 10.0))
    }

    /// `region()` とは重ならない範囲。
    fn other_region() -> Aabb {
        Aabb::new(P::new(100.0, 100.0), P::new(110.0, 110.0))
    }

    #[test]
    fn geometry_stretched_empty_regions_equals_translated_line() {
        let g = line_geom((1.0, 1.0), (2.0, 2.0));
        let delta = V::new(5.0, -3.0);
        assert_eq!(g.stretched(&[], delta), g.translated(delta));
    }

    #[test]
    fn geometry_stretched_empty_regions_equals_translated_circle() {
        let g = Geometry::Circle(Circle::new(P::new(1.0, 1.0), 3.0));
        let delta = V::new(5.0, -3.0);
        assert_eq!(g.stretched(&[], delta), g.translated(delta));
    }

    #[test]
    fn geometry_stretched_empty_regions_equals_translated_arc() {
        let g = Geometry::Arc(Arc::new(P::new(1.0, 1.0), 3.0, 0.0, 1.0));
        let delta = V::new(5.0, -3.0);
        assert_eq!(g.stretched(&[], delta), g.translated(delta));
    }

    #[test]
    fn geometry_stretched_empty_regions_equals_translated_polyline() {
        let g = Geometry::Polyline(Polyline::rectangle(P::new(0.0, 0.0), P::new(2.0, 3.0)));
        let delta = V::new(5.0, -3.0);
        assert_eq!(g.stretched(&[], delta), g.translated(delta));
    }

    #[test]
    fn geometry_stretched_line_only_a_inside_moves_a_only() {
        // a は region の内側、b はその外側。
        let g = line_geom((1.0, 1.0), (50.0, 50.0));
        let moved = g.stretched(&[region()], V::new(5.0, 5.0));
        assert_eq!(moved, line_geom((6.0, 6.0), (50.0, 50.0)));
    }

    #[test]
    fn geometry_stretched_line_only_b_inside_moves_b_only() {
        let g = line_geom((50.0, 50.0), (1.0, 1.0));
        let moved = g.stretched(&[region()], V::new(5.0, 5.0));
        assert_eq!(moved, line_geom((50.0, 50.0), (6.0, 6.0)));
    }

    #[test]
    fn geometry_stretched_line_both_inside_equals_translated() {
        let g = line_geom((1.0, 1.0), (2.0, 2.0));
        let delta = V::new(5.0, 5.0);
        assert_eq!(g.stretched(&[region()], delta), g.translated(delta));
    }

    #[test]
    fn geometry_stretched_line_neither_inside_is_unchanged() {
        let g = line_geom((50.0, 50.0), (60.0, 60.0));
        let moved = g.stretched(&[region()], V::new(5.0, 5.0));
        assert_eq!(moved, g);
    }

    #[test]
    fn geometry_stretched_polyline_moves_only_vertices_inside_region() {
        // 4 頂点のうち先頭 2 つだけが region の内側。
        let g = Geometry::Polyline(Polyline::new(
            vec![
                P::new(1.0, 1.0),
                P::new(2.0, 2.0),
                P::new(50.0, 50.0),
                P::new(60.0, 60.0),
            ],
            false,
        ));
        let moved = g.stretched(&[region()], V::new(3.0, 3.0));
        let expected = Geometry::Polyline(Polyline::new(
            vec![
                P::new(4.0, 4.0),
                P::new(5.0, 5.0),
                P::new(50.0, 50.0),
                P::new(60.0, 60.0),
            ],
            false,
        ));
        assert_eq!(moved, expected);
    }

    #[test]
    fn geometry_stretched_polyline_preserves_closed_flag_and_vertex_count() {
        let g = Geometry::Polyline(Polyline::rectangle(P::new(0.0, 0.0), P::new(2.0, 3.0)));
        let moved = g.stretched(&[region()], V::new(1.0, 1.0));
        match (&g, &moved) {
            (Geometry::Polyline(orig), Geometry::Polyline(m)) => {
                assert_eq!(m.closed, orig.closed);
                assert_eq!(m.vertex_count(), orig.vertex_count());
            }
            _ => panic!("Polyline のはず"),
        }
    }

    #[test]
    fn geometry_stretched_circle_center_inside_moves_whole_circle_radius_unchanged() {
        let g = Geometry::Circle(Circle::new(P::new(5.0, 5.0), 3.0));
        let moved = g.stretched(&[region()], V::new(2.0, 2.0));
        assert_eq!(moved, Geometry::Circle(Circle::new(P::new(7.0, 7.0), 3.0)));
    }

    #[test]
    fn geometry_stretched_circle_center_outside_is_unchanged_even_when_circumference_crosses_region(
    ) {
        // 中心は region の外だが、半径が大きいため円周は region と交差する。
        let g = Geometry::Circle(Circle::new(P::new(15.0, 5.0), 8.0));
        let moved = g.stretched(&[region()], V::new(2.0, 2.0));
        assert_eq!(moved, g);
    }

    #[test]
    fn geometry_stretched_arc_center_inside_moves_radius_and_angles_unchanged() {
        let g = Geometry::Arc(Arc::new(P::new(5.0, 5.0), 3.0, 0.1, 1.5));
        let moved = g.stretched(&[region()], V::new(2.0, 2.0));
        assert_eq!(
            moved,
            Geometry::Arc(Arc::new(P::new(7.0, 7.0), 3.0, 0.1, 1.5))
        );
    }

    #[test]
    fn geometry_stretched_arc_center_outside_is_unchanged() {
        let g = Geometry::Arc(Arc::new(P::new(50.0, 50.0), 3.0, 0.1, 1.5));
        let moved = g.stretched(&[region()], V::new(2.0, 2.0));
        assert_eq!(moved, g);
    }

    #[test]
    fn geometry_stretched_multiple_regions_point_in_either_moves() {
        // a は region() の内側、b は other_region() の内側。両方とも動くはず。
        let g = line_geom((1.0, 1.0), (105.0, 105.0));
        let moved = g.stretched(&[region(), other_region()], V::new(1.0, 1.0));
        assert_eq!(moved, line_geom((2.0, 2.0), (106.0, 106.0)));
    }

    #[test]
    fn geometry_stretched_multiple_overlapping_regions_moves_point_exactly_once() {
        // 2 つの重なり合う範囲の両方に入る点でも、delta が 2 重にはかからない。
        let overlap_a = Aabb::new(P::new(0.0, 0.0), P::new(10.0, 10.0));
        let overlap_b = Aabb::new(P::new(5.0, 5.0), P::new(15.0, 15.0));
        let g = line_geom((7.0, 7.0), (100.0, 100.0));
        let moved = g.stretched(&[overlap_a, overlap_b], V::new(3.0, 3.0));
        assert_eq!(moved, line_geom((10.0, 10.0), (100.0, 100.0)));
    }

    #[test]
    fn geometry_stretched_large_coordinates() {
        let big_region = Aabb::new(P::new(0.0, 0.0), P::new(2.0e6, 2.0e6));
        let g = line_geom((1.0e6, 1.0e6), (5.0e6, 5.0e6));
        let moved = g.stretched(&[big_region], V::new(1.0e6, 0.0));
        assert_eq!(moved, line_geom((2.0e6, 1.0e6), (5.0e6, 5.0e6)));
    }

    #[test]
    fn entity_stretched_keeps_layer_and_color() {
        let e = Entity::new(line_geom((1.0, 1.0), (50.0, 50.0)), LayerId::ZERO);
        let moved = e.stretched(&[region()], V::new(2.0, 2.0));
        assert_eq!(moved.layer, e.layer);
        assert_eq!(moved.color, e.color);
        assert_eq!(moved.geom, line_geom((3.0, 3.0), (50.0, 50.0)));
    }
}
