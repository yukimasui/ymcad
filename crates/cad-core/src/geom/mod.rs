//! 2D ジオメトリのプリミティブ。
//!
//! すべての座標・長さ・角度は `f64`。角度の単位は **ラジアン**。
//! 度数への変換が必要なのは DXF 入出力だけなので、変換は `dxf` モジュールに閉じ込める。
//!
//! 浮動小数点の比較は必ず [`tolerance`] の関数を使うこと。
//! 生の `==` や、閾値をその場に直書きした比較は禁止。

pub mod aabb;
pub mod arc;
pub mod intersect;
pub mod line;
pub mod point;
pub mod tolerance;

pub use aabb::Aabb;
pub use arc::{Arc, Circle};
pub use line::Line;
pub use point::{Point2, Vec2};
pub use tolerance::{EPS_ANGLE, EPS_LEN, EPS_REL};
