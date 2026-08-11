//! 2D の点とベクトル。
//!
//! [`Point2`] は位置（アフィン点）、[`Vec2`] は変位（ベクトル）を表す。
//! 両者を型で区別することで「点同士を足す」といった無意味な演算をコンパイル時に排除する。

use super::tolerance::{eq_len, is_zero_len};
use std::ops::{Add, AddAssign, Div, Mul, Neg, Sub, SubAssign};

/// 2D 上の位置（アフィン点）。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point2 {
    /// X 座標。
    pub x: f64,
    /// Y 座標。
    pub y: f64,
}

/// 2D の変位ベクトル。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec2 {
    /// X 成分。
    pub x: f64,
    /// Y 成分。
    pub y: f64,
}

impl Point2 {
    /// 原点 `(0, 0)`。
    pub const ORIGIN: Self = Self { x: 0.0, y: 0.0 };

    /// 座標から点を作る。
    #[inline]
    #[must_use]
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// `self` から `o` までのユークリッド距離。
    #[inline]
    #[must_use]
    pub fn dist(self, o: Self) -> f64 {
        (o - self).len()
    }

    /// `self` から `o` までの距離の 2 乗（`sqrt` を避けたい比較用）。
    #[inline]
    #[must_use]
    pub fn dist_sq(self, o: Self) -> f64 {
        (o - self).len_sq()
    }

    /// `self` と `o` を `t` で線形補間する（`t = 0` で `self`、`t = 1` で `o`）。
    #[inline]
    #[must_use]
    pub fn lerp(self, o: Self, t: f64) -> Self {
        self + (o - self) * t
    }

    /// 各軸を [`eq_len`] で比較し、トレランス内で等しいか判定する。
    #[inline]
    #[must_use]
    pub fn eq_tol(self, o: Self) -> bool {
        eq_len(self.x, o.x) && eq_len(self.y, o.y)
    }

    /// 原点からの変位ベクトルとして扱う。
    #[inline]
    #[must_use]
    pub fn to_vec(self) -> Vec2 {
        Vec2::new(self.x, self.y)
    }
}

impl Vec2 {
    /// ゼロベクトル。
    pub const ZERO: Self = Self { x: 0.0, y: 0.0 };
    /// X 方向の単位ベクトル。
    pub const X: Self = Self { x: 1.0, y: 0.0 };
    /// Y 方向の単位ベクトル。
    pub const Y: Self = Self { x: 0.0, y: 1.0 };

    /// 成分からベクトルを作る。
    #[inline]
    #[must_use]
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// 角度 `rad` [rad] を向く単位ベクトル。
    #[inline]
    #[must_use]
    pub fn from_angle(rad: f64) -> Self {
        Self::new(rad.cos(), rad.sin())
    }

    /// 角度 `rad` [rad]・長さ `len` の極座標ベクトル（`@100<45` 形式の入力用）。
    #[inline]
    #[must_use]
    pub fn polar(rad: f64, len: f64) -> Self {
        Self::from_angle(rad) * len
    }

    /// ベクトルの長さ。
    #[inline]
    #[must_use]
    pub fn len(self) -> f64 {
        self.len_sq().sqrt()
    }

    /// ベクトルの長さの 2 乗（`sqrt` を避けたい比較用）。
    #[inline]
    #[must_use]
    pub fn len_sq(self) -> f64 {
        self.x * self.x + self.y * self.y
    }

    /// 長さがトレランス内でゼロとみなせるか。
    #[inline]
    #[must_use]
    pub fn is_zero(self) -> bool {
        is_zero_len(self.len())
    }

    /// 単位ベクトルに正規化する。ゼロベクトルの場合は `None`（NaN は返さない）。
    #[inline]
    #[must_use]
    pub fn normalized(self) -> Option<Self> {
        if self.is_zero() {
            None
        } else {
            Some(self / self.len())
        }
    }

    /// 反時計回りに 90 度回転したベクトル。
    #[inline]
    #[must_use]
    pub fn perp(self) -> Self {
        Self::new(-self.y, self.x)
    }

    /// 内積。
    #[inline]
    #[must_use]
    pub fn dot(self, o: Self) -> f64 {
        self.x * o.x + self.y * o.y
    }

    /// 2D の外積（スカラー値）。`self` から `o` への回転が反時計回りなら正。
    #[inline]
    #[must_use]
    pub fn cross(self, o: Self) -> f64 {
        self.x * o.y - self.y * o.x
    }

    /// `atan2` による偏角 [rad]。値域は `(-π, π]`。
    #[inline]
    #[must_use]
    pub fn angle(self) -> f64 {
        self.y.atan2(self.x)
    }

    /// `rad` [rad] だけ回転したベクトル。
    #[inline]
    #[must_use]
    pub fn rotated(self, rad: f64) -> Self {
        let (s, c) = rad.sin_cos();
        Self::new(self.x * c - self.y * s, self.x * s + self.y * c)
    }

    /// 各成分を [`eq_len`] で比較し、トレランス内で等しいか判定する。
    #[inline]
    #[must_use]
    pub fn eq_tol(self, o: Self) -> bool {
        eq_len(self.x, o.x) && eq_len(self.y, o.y)
    }
}

impl Sub for Point2 {
    type Output = Vec2;

    #[inline]
    fn sub(self, o: Self) -> Vec2 {
        Vec2::new(self.x - o.x, self.y - o.y)
    }
}

impl Add<Vec2> for Point2 {
    type Output = Self;

    #[inline]
    fn add(self, v: Vec2) -> Self {
        Self::new(self.x + v.x, self.y + v.y)
    }
}

impl Sub<Vec2> for Point2 {
    type Output = Self;

    #[inline]
    fn sub(self, v: Vec2) -> Self {
        Self::new(self.x - v.x, self.y - v.y)
    }
}

impl AddAssign<Vec2> for Point2 {
    #[inline]
    fn add_assign(&mut self, v: Vec2) {
        *self = *self + v;
    }
}

impl SubAssign<Vec2> for Point2 {
    #[inline]
    fn sub_assign(&mut self, v: Vec2) {
        *self = *self - v;
    }
}

impl Add for Vec2 {
    type Output = Self;

    #[inline]
    fn add(self, o: Self) -> Self {
        Self::new(self.x + o.x, self.y + o.y)
    }
}

impl Sub for Vec2 {
    type Output = Self;

    #[inline]
    fn sub(self, o: Self) -> Self {
        Self::new(self.x - o.x, self.y - o.y)
    }
}

impl Neg for Vec2 {
    type Output = Self;

    #[inline]
    fn neg(self) -> Self {
        Self::new(-self.x, -self.y)
    }
}

impl Mul<f64> for Vec2 {
    type Output = Self;

    #[inline]
    fn mul(self, s: f64) -> Self {
        Self::new(self.x * s, self.y * s)
    }
}

impl Mul<Vec2> for f64 {
    type Output = Vec2;

    #[inline]
    fn mul(self, v: Vec2) -> Vec2 {
        v * self
    }
}

impl Div<f64> for Vec2 {
    type Output = Self;

    #[inline]
    fn div(self, s: f64) -> Self {
        Self::new(self.x / s, self.y / s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::tolerance::EPS_LEN;
    use std::f64::consts::{FRAC_PI_2, PI};

    #[test]
    fn point_new_and_fields() {
        let p = Point2::new(1.0, 2.0);
        assert!(eq_len(p.x, 1.0));
        assert!(eq_len(p.y, 2.0));
    }

    #[test]
    fn point_dist_and_dist_sq() {
        let a = Point2::new(0.0, 0.0);
        let b = Point2::new(3.0, 4.0);
        assert!(eq_len(a.dist(b), 5.0));
        assert!(eq_len(a.dist_sq(b), 25.0));
    }

    #[test]
    fn point_lerp_endpoints_and_mid() {
        let a = Point2::new(0.0, 0.0);
        let b = Point2::new(10.0, 20.0);
        assert!(a.lerp(b, 0.0).eq_tol(a));
        assert!(a.lerp(b, 1.0).eq_tol(b));
        assert!(a.lerp(b, 0.5).eq_tol(Point2::new(5.0, 10.0)));
    }

    #[test]
    fn point_eq_tol_boundary() {
        let a = Point2::new(1.0, 1.0);
        assert!(a.eq_tol(Point2::new(1.0 + EPS_LEN * 0.5, 1.0)));
        assert!(!a.eq_tol(Point2::new(1.0 + EPS_LEN * 100.0, 1.0)));
    }

    #[test]
    fn point_operators_roundtrip() {
        let a = Point2::new(1.0, 2.0);
        let v = Vec2::new(3.0, 4.0);
        let b = a + v;
        assert!((b - a).eq_tol(v));
        assert!((b - v).eq_tol(a));

        let mut c = a;
        c += v;
        assert!(c.eq_tol(b));
        c -= v;
        assert!(c.eq_tol(a));
    }

    #[test]
    fn point_to_vec() {
        let p = Point2::new(3.0, -4.0);
        assert!(p.to_vec().eq_tol(Vec2::new(3.0, -4.0)));
    }

    #[test]
    fn vec_len_and_len_sq() {
        let v = Vec2::new(3.0, 4.0);
        assert!(eq_len(v.len(), 5.0));
        assert!(eq_len(v.len_sq(), 25.0));
    }

    #[test]
    fn vec_is_zero_boundary() {
        assert!(Vec2::ZERO.is_zero());
        assert!(Vec2::new(EPS_LEN * 0.5, 0.0).is_zero());
        assert!(!Vec2::new(EPS_LEN * 100.0, 0.0).is_zero());
    }

    #[test]
    fn vec_normalized_zero_returns_none() {
        assert_eq!(Vec2::ZERO.normalized(), None);
    }

    #[test]
    fn vec_normalized_unit_length() {
        let v = Vec2::new(3.0, 4.0).normalized().expect("非ゼロのはず");
        assert!(eq_len(v.len(), 1.0));
    }

    #[test]
    fn vec_from_angle_and_polar() {
        let v = Vec2::from_angle(0.0);
        assert!(v.eq_tol(Vec2::X));
        let v2 = Vec2::from_angle(FRAC_PI_2);
        assert!(v2.eq_tol(Vec2::Y));
        let p = Vec2::polar(0.0, 100.0);
        assert!(p.eq_tol(Vec2::new(100.0, 0.0)));
    }

    #[test]
    fn vec_perp_is_ccw_90() {
        assert!(Vec2::X.perp().eq_tol(Vec2::Y));
        assert!(Vec2::Y.perp().eq_tol(-Vec2::X));
    }

    #[test]
    fn vec_dot_and_cross() {
        assert!(eq_len(Vec2::X.dot(Vec2::Y), 0.0));
        assert!(eq_len(Vec2::X.dot(Vec2::X), 1.0));
        assert!(eq_len(Vec2::X.cross(Vec2::Y), 1.0));
        assert!(eq_len(Vec2::Y.cross(Vec2::X), -1.0));
    }

    #[test]
    fn vec_angle_range() {
        use crate::geom::tolerance::eq_angle;
        assert!(eq_len(Vec2::X.angle(), 0.0));
        assert!(eq_len(Vec2::Y.angle(), FRAC_PI_2));
        // -X 方向は IEEE754 の符号付きゼロ次第で +π にも -π にもなりうる
        // （`-Vec2::X` は y 成分が `-0.0` になるため）。どちらも同じ向きを表すので
        // 一周ぶんの差を等しいとみなす eq_angle で比較する。
        assert!(eq_angle((-Vec2::X).angle(), PI));
    }

    #[test]
    fn vec_rotated_quarter_turn() {
        let r = Vec2::X.rotated(FRAC_PI_2);
        assert!(r.eq_tol(Vec2::Y));
    }

    #[test]
    fn vec_operators() {
        let a = Vec2::new(1.0, 2.0);
        let b = Vec2::new(3.0, 4.0);
        assert!((a + b).eq_tol(Vec2::new(4.0, 6.0)));
        assert!((b - a).eq_tol(Vec2::new(2.0, 2.0)));
        assert!((-a).eq_tol(Vec2::new(-1.0, -2.0)));
        assert!((a * 2.0).eq_tol(Vec2::new(2.0, 4.0)));
        assert!((2.0 * a).eq_tol(Vec2::new(2.0, 4.0)));
        assert!((b / 2.0).eq_tol(Vec2::new(1.5, 2.0)));
    }

    #[test]
    fn vec_large_and_small_magnitude() {
        let big = Vec2::new(1e6, 0.0);
        assert!(eq_len(big.len(), 1e6));
        let small = Vec2::new(0.000_001, 0.0);
        assert!(eq_len(small.len(), 0.000_001));
        assert!(!small.is_zero());
    }
}
