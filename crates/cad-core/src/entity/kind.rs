//! 図形要素の定義。

use crate::geom::{Aabb, Arc, Circle, Line, Point2};
use crate::layer::{ColorSpec, LayerId};

/// 図形の実体。
///
/// Phase 3 でポリラインを追加する予定。
#[derive(Clone, Debug, PartialEq)]
pub enum Geometry {
    /// 線分。
    Line(Line),
    /// 円。
    Circle(Circle),
    /// 円弧。
    Arc(Arc),
}

impl Geometry {
    /// 境界ボックス。
    #[must_use]
    pub fn bbox(&self) -> Aabb {
        match self {
            Self::Line(l) => l.bbox(),
            Self::Circle(c) => c.bbox(),
            Self::Arc(a) => a.bbox(),
        }
    }

    /// 点との最短距離。ピックやスナップの判定に使う。
    #[must_use]
    pub fn dist_to(&self, p: Point2) -> f64 {
        match self {
            Self::Line(l) => l.dist_to(p),
            Self::Circle(c) => c.dist_to(p),
            Self::Arc(a) => a.dist_to(p),
        }
    }

    /// コマンド名などに使う種別名。
    #[must_use]
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Line(_) => "LINE",
            Self::Circle(_) => "CIRCLE",
            Self::Arc(_) => "ARC",
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
}
