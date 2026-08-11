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
}
