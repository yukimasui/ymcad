//! 軸並行境界ボックス（Axis-Aligned Bounding Box）。

use super::point::{Point2, Vec2};
use super::tolerance::{gt_len, lt_len};

/// 軸並行境界ボックス。
///
/// `min` は左下、`max` は右上のコーナー。[`Aabb::new`] や [`Aabb::union`] を経由する限り
/// `min.x <= max.x && min.y <= max.y` が保たれる（[`Aabb::EMPTY`] を除く）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    /// 左下コーナー。
    pub min: Point2,
    /// 右上コーナー。
    pub max: Point2,
}

impl Aabb {
    /// 空のボックス。`min = +∞`, `max = -∞` として、[`Aabb::union`] の単位元にする。
    ///
    /// `union(EMPTY, x) == x` が常に成り立つ。
    pub const EMPTY: Self = Self {
        min: Point2 {
            x: f64::INFINITY,
            y: f64::INFINITY,
        },
        max: Point2 {
            x: f64::NEG_INFINITY,
            y: f64::NEG_INFINITY,
        },
    };

    /// 2 点からボックスを作る。コーナーの大小関係は自動的に正規化される。
    #[inline]
    #[must_use]
    pub fn new(a: Point2, b: Point2) -> Self {
        Self {
            min: Point2::new(a.x.min(b.x), a.y.min(b.y)),
            max: Point2::new(a.x.max(b.x), a.y.max(b.y)),
        }
    }

    /// 点列からそれらすべてを包む最小のボックスを作る。空の場合は [`Aabb::EMPTY`]。
    #[must_use]
    pub fn from_points(it: impl IntoIterator<Item = Point2>) -> Self {
        it.into_iter()
            .fold(Self::EMPTY, |acc, p| acc.union_point(p))
    }

    /// 2 つのボックスを包む最小のボックス。
    #[inline]
    #[must_use]
    pub fn union(self, o: Self) -> Self {
        Self {
            min: Point2::new(self.min.x.min(o.min.x), self.min.y.min(o.min.y)),
            max: Point2::new(self.max.x.max(o.max.x), self.max.y.max(o.max.y)),
        }
    }

    /// 1 点を含むように広げたボックス。
    #[inline]
    #[must_use]
    pub fn union_point(self, p: Point2) -> Self {
        self.union(Self { min: p, max: p })
    }

    /// 全方向に `d` だけ広げたボックス（`d` が負なら縮む）。
    #[inline]
    #[must_use]
    pub fn expanded(self, d: f64) -> Self {
        Self {
            min: self.min - Vec2::new(d, d),
            max: self.max + Vec2::new(d, d),
        }
    }

    /// 点 `p` を（トレランス込みで）内包するか。
    #[must_use]
    pub fn contains(&self, p: Point2) -> bool {
        if self.is_empty() {
            return false;
        }
        !lt_len(p.x, self.min.x)
            && !gt_len(p.x, self.max.x)
            && !lt_len(p.y, self.min.y)
            && !gt_len(p.y, self.max.y)
    }

    /// `o` を（トレランス込みで）完全に内包するか。フェーズ 3 のウィンドウ選択に使う。
    #[must_use]
    pub fn contains_aabb(&self, o: &Self) -> bool {
        if self.is_empty() || o.is_empty() {
            return false;
        }
        !gt_len(self.min.x, o.min.x)
            && !lt_len(self.max.x, o.max.x)
            && !gt_len(self.min.y, o.min.y)
            && !lt_len(self.max.y, o.max.y)
    }

    /// `o` と（トレランス込みで）重なりを持つか。フェーズ 3 の交差選択に使う。
    #[must_use]
    pub fn intersects(&self, o: &Self) -> bool {
        if self.is_empty() || o.is_empty() {
            return false;
        }
        !gt_len(self.min.x, o.max.x)
            && !gt_len(o.min.x, self.max.x)
            && !gt_len(self.min.y, o.max.y)
            && !gt_len(o.min.y, self.max.y)
    }

    /// ボックスの中心。
    #[inline]
    #[must_use]
    pub fn center(&self) -> Point2 {
        self.min.lerp(self.max, 0.5)
    }

    /// ボックスの大きさ（幅・高さ）。
    #[inline]
    #[must_use]
    pub fn size(&self) -> Vec2 {
        self.max - self.min
    }

    /// ボックスの幅。
    #[inline]
    #[must_use]
    pub fn width(&self) -> f64 {
        self.size().x
    }

    /// ボックスの高さ。
    #[inline]
    #[must_use]
    pub fn height(&self) -> f64 {
        self.size().y
    }

    /// 空のボックスか。
    ///
    /// これは幾何的な近似判定ではなく、`min > max` という構造的な不変条件の破れを
    /// 検出するための判定なので、意図的にトレランス関数を経由しない生の比較を使う
    /// （[`Aabb::EMPTY`] は `±∞` を使う番兵値であり、トレランス関数を通すと
    /// `∞ - ∞` が絡んで判定が壊れるため）。
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.min.x > self.max.x || self.min.y > self.max.y
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::tolerance::EPS_LEN;

    #[test]
    fn empty_is_empty() {
        assert!(Aabb::EMPTY.is_empty());
    }

    #[test]
    fn single_point_box_is_not_empty() {
        let b = Aabb::new(Point2::new(1.0, 1.0), Point2::new(1.0, 1.0));
        assert!(!b.is_empty());
    }

    #[test]
    fn new_normalizes_corners() {
        let b = Aabb::new(Point2::new(5.0, -1.0), Point2::new(-3.0, 4.0));
        assert_eq!(b.min, Point2::new(-3.0, -1.0));
        assert_eq!(b.max, Point2::new(5.0, 4.0));
    }

    #[test]
    fn union_with_empty_is_identity() {
        let b = Aabb::new(Point2::new(0.0, 0.0), Point2::new(2.0, 3.0));
        assert_eq!(Aabb::EMPTY.union(b), b);
        assert_eq!(b.union(Aabb::EMPTY), b);
    }

    #[test]
    fn union_point_grows_box() {
        let b = Aabb::new(Point2::new(0.0, 0.0), Point2::new(1.0, 1.0));
        let grown = b.union_point(Point2::new(5.0, -2.0));
        assert_eq!(grown.min, Point2::new(0.0, -2.0));
        assert_eq!(grown.max, Point2::new(5.0, 1.0));
    }

    #[test]
    fn from_points_empty_iter() {
        let b = Aabb::from_points(std::iter::empty());
        assert!(b.is_empty());
    }

    #[test]
    fn from_points_builds_bbox() {
        let pts = [
            Point2::new(1.0, 1.0),
            Point2::new(-2.0, 3.0),
            Point2::new(4.0, -5.0),
        ];
        let b = Aabb::from_points(pts);
        assert_eq!(b.min, Point2::new(-2.0, -5.0));
        assert_eq!(b.max, Point2::new(4.0, 3.0));
    }

    #[test]
    fn expanded_grows_and_shrinks() {
        let b = Aabb::new(Point2::new(0.0, 0.0), Point2::new(10.0, 10.0));
        let grown = b.expanded(1.0);
        assert_eq!(grown.min, Point2::new(-1.0, -1.0));
        assert_eq!(grown.max, Point2::new(11.0, 11.0));

        let shrunk = b.expanded(-1.0);
        assert_eq!(shrunk.min, Point2::new(1.0, 1.0));
        assert_eq!(shrunk.max, Point2::new(9.0, 9.0));
    }

    #[test]
    fn contains_interior_and_exterior() {
        let b = Aabb::new(Point2::new(0.0, 0.0), Point2::new(10.0, 10.0));
        assert!(b.contains(Point2::new(5.0, 5.0)));
        assert!(!b.contains(Point2::new(11.0, 5.0)));
    }

    #[test]
    fn contains_boundary_tolerance() {
        let b = Aabb::new(Point2::new(0.0, 0.0), Point2::new(10.0, 10.0));
        assert!(b.contains(Point2::new(10.0 + EPS_LEN * 0.5, 5.0)));
        assert!(!b.contains(Point2::new(10.0 + EPS_LEN * 1000.0, 5.0)));
    }

    #[test]
    fn contains_on_empty_is_false() {
        assert!(!Aabb::EMPTY.contains(Point2::new(0.0, 0.0)));
    }

    #[test]
    fn contains_aabb_full_containment() {
        let outer = Aabb::new(Point2::new(0.0, 0.0), Point2::new(10.0, 10.0));
        let inner = Aabb::new(Point2::new(2.0, 2.0), Point2::new(8.0, 8.0));
        assert!(outer.contains_aabb(&inner));
        assert!(!inner.contains_aabb(&outer));
    }

    #[test]
    fn contains_aabb_partial_overlap_is_false() {
        let a = Aabb::new(Point2::new(0.0, 0.0), Point2::new(5.0, 5.0));
        let b = Aabb::new(Point2::new(4.0, 4.0), Point2::new(9.0, 9.0));
        assert!(!a.contains_aabb(&b));
    }

    #[test]
    fn intersects_overlapping_and_disjoint() {
        let a = Aabb::new(Point2::new(0.0, 0.0), Point2::new(5.0, 5.0));
        let b = Aabb::new(Point2::new(4.0, 4.0), Point2::new(9.0, 9.0));
        let c = Aabb::new(Point2::new(100.0, 100.0), Point2::new(200.0, 200.0));
        assert!(a.intersects(&b));
        assert!(!a.intersects(&c));
    }

    #[test]
    fn intersects_touching_edge_tolerant() {
        let a = Aabb::new(Point2::new(0.0, 0.0), Point2::new(5.0, 5.0));
        let b = Aabb::new(Point2::new(5.0, 0.0), Point2::new(10.0, 5.0));
        assert!(a.intersects(&b));
    }

    #[test]
    fn center_size_width_height() {
        let b = Aabb::new(Point2::new(0.0, 0.0), Point2::new(4.0, 2.0));
        assert_eq!(b.center(), Point2::new(2.0, 1.0));
        assert_eq!(b.size(), Vec2::new(4.0, 2.0));
        assert_eq!(b.width(), 4.0);
        assert_eq!(b.height(), 2.0);
    }

    #[test]
    fn large_coordinates_do_not_degenerate() {
        let b = Aabb::new(Point2::new(-1e6, -1e6), Point2::new(1e6, 1e6));
        assert!(!b.is_empty());
        assert!(b.contains(Point2::new(0.0, 0.0)));
        assert!(b.contains(Point2::new(1e6, 1e6)));
    }

    #[test]
    fn tiny_coordinates_do_not_degenerate() {
        let b = Aabb::new(
            Point2::new(-0.000_001, -0.000_001),
            Point2::new(0.000_001, 0.000_001),
        );
        assert!(!b.is_empty());
        assert!(b.contains(Point2::new(0.0, 0.0)));
    }
}
