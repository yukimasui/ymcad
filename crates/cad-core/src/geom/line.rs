//! 線分（有界な線）。

use super::aabb::Aabb;
use super::point::{Point2, Vec2};
use super::tolerance::{gt_len, is_zero_len, lt_len};

/// 2 点 `a`, `b` を結ぶ線分。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line {
    /// 始点。
    pub a: Point2,
    /// 終点。
    pub b: Point2,
}

impl Line {
    /// 2 点から線分を作る。
    #[inline]
    #[must_use]
    pub fn new(a: Point2, b: Point2) -> Self {
        Self { a, b }
    }

    /// `a` から `b` への正規化された方向ベクトル。退化線分（長さ 0）では `None`。
    #[inline]
    #[must_use]
    pub fn dir(&self) -> Option<Vec2> {
        self.vector().normalized()
    }

    /// `a` から `b` への変位ベクトル（正規化しない）。
    #[inline]
    #[must_use]
    pub fn vector(&self) -> Vec2 {
        self.b - self.a
    }

    /// 線分の長さ。
    #[inline]
    #[must_use]
    pub fn length(&self) -> f64 {
        self.vector().len()
    }

    /// 長さがトレランス内でゼロとみなせるか（退化線分か）。
    #[inline]
    #[must_use]
    pub fn is_degenerate(&self) -> bool {
        is_zero_len(self.length())
    }

    /// 線分の中点。
    #[inline]
    #[must_use]
    pub fn midpoint(&self) -> Point2 {
        self.a.lerp(self.b, 0.5)
    }

    /// パラメータ `t`（`0` で `a`、`1` で `b`）上の点。`t` は `[0, 1]` の外でもよい。
    #[inline]
    #[must_use]
    pub fn point_at(&self, t: f64) -> Point2 {
        self.a.lerp(self.b, t)
    }

    /// 点 `p` から線分（を含む無限直線）への最近傍パラメータ。**クランプしない。**
    ///
    /// 退化線分では `0.0` を返す（NaN を返さない）。
    #[must_use]
    pub fn closest_param(&self, p: Point2) -> f64 {
        if self.is_degenerate() {
            return 0.0;
        }
        let v = self.vector();
        (p - self.a).dot(v) / v.len_sq()
    }

    /// 点 `p` から線分への最近傍点。パラメータは `[0, 1]` にクランプされる。
    #[inline]
    #[must_use]
    pub fn closest_point(&self, p: Point2) -> Point2 {
        self.point_at(self.closest_param(p).clamp(0.0, 1.0))
    }

    /// 点 `p` から線分（無限直線ではない）への距離。
    #[inline]
    #[must_use]
    pub fn dist_to(&self, p: Point2) -> f64 {
        p.dist(self.closest_point(p))
    }

    /// 線分を包む境界ボックス。
    #[inline]
    #[must_use]
    pub fn bbox(&self) -> Aabb {
        Aabb::new(self.a, self.b)
    }

    /// 平行移動した複製を作る。
    #[inline]
    #[must_use]
    pub fn translated(&self, v: Vec2) -> Self {
        Self::new(self.a + v, self.b + v)
    }

    /// Liang–Barsky 法で矩形 `r`（モデル座標系）にクリップする。
    ///
    /// 完全に矩形の外にある場合は `None`。
    /// レンダラに巨大な座標を渡さないため（フェーズ 2 のズームアウト対策）に使う。
    #[must_use]
    pub fn clip_to(&self, r: Aabb) -> Option<Self> {
        let d = self.vector();
        let checks = [
            (-d.x, self.a.x - r.min.x),
            (d.x, r.max.x - self.a.x),
            (-d.y, self.a.y - r.min.y),
            (d.y, r.max.y - self.a.y),
        ];

        let mut t0 = 0.0_f64;
        let mut t1 = 1.0_f64;

        for (p, q) in checks {
            if is_zero_len(p) {
                // この境界と平行。矩形の外側にあるなら交わりようがない。
                if lt_len(q, 0.0) {
                    return None;
                }
            } else {
                let t = q / p;
                if lt_len(p, 0.0) {
                    if gt_len(t, t0) {
                        t0 = t;
                    }
                } else if lt_len(t, t1) {
                    t1 = t;
                }
            }
        }

        if gt_len(t0, t1) {
            None
        } else {
            Some(Self::new(self.point_at(t0), self.point_at(t1)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::tolerance::{eq_len, EPS_LEN};

    #[test]
    fn new_and_vector() {
        let l = Line::new(Point2::new(0.0, 0.0), Point2::new(3.0, 4.0));
        assert!(l.vector().eq_tol(Vec2::new(3.0, 4.0)));
        assert!(eq_len(l.length(), 5.0));
    }

    #[test]
    fn dir_normalized() {
        let l = Line::new(Point2::new(0.0, 0.0), Point2::new(10.0, 0.0));
        let d = l.dir().expect("非退化のはず");
        assert!(eq_len(d.len(), 1.0));
    }

    #[test]
    fn dir_none_on_degenerate() {
        let l = Line::new(Point2::new(1.0, 1.0), Point2::new(1.0, 1.0));
        assert_eq!(l.dir(), None);
    }

    #[test]
    fn is_degenerate_boundary() {
        let l = Line::new(Point2::new(0.0, 0.0), Point2::new(EPS_LEN * 0.5, 0.0));
        assert!(l.is_degenerate());
        let l2 = Line::new(Point2::new(0.0, 0.0), Point2::new(EPS_LEN * 1000.0, 0.0));
        assert!(!l2.is_degenerate());
    }

    #[test]
    fn midpoint_and_point_at() {
        let l = Line::new(Point2::new(0.0, 0.0), Point2::new(10.0, 0.0));
        assert_eq!(l.midpoint(), Point2::new(5.0, 0.0));
        assert_eq!(l.point_at(0.0), l.a);
        assert_eq!(l.point_at(1.0), l.b);
        // t の範囲外（延長線上）も許容する。
        assert_eq!(l.point_at(2.0), Point2::new(20.0, 0.0));
    }

    #[test]
    fn closest_param_unclamped() {
        let l = Line::new(Point2::new(0.0, 0.0), Point2::new(10.0, 0.0));
        // 延長線上の点でも範囲外のパラメータを返す。
        let t = l.closest_param(Point2::new(20.0, 5.0));
        assert!(eq_len(t, 2.0));
    }

    #[test]
    fn closest_param_degenerate_is_zero() {
        let l = Line::new(Point2::new(3.0, 3.0), Point2::new(3.0, 3.0));
        assert_eq!(l.closest_param(Point2::new(100.0, 100.0)), 0.0);
    }

    #[test]
    fn closest_point_is_clamped() {
        let l = Line::new(Point2::new(0.0, 0.0), Point2::new(10.0, 0.0));
        assert_eq!(l.closest_point(Point2::new(20.0, 5.0)), l.b);
        assert_eq!(l.closest_point(Point2::new(-5.0, 5.0)), l.a);
        assert_eq!(
            l.closest_point(Point2::new(5.0, 5.0)),
            Point2::new(5.0, 0.0)
        );
    }

    #[test]
    fn dist_to_segment_not_infinite_line() {
        let l = Line::new(Point2::new(0.0, 0.0), Point2::new(10.0, 0.0));
        // 延長線上ではなく端点までの距離になるはず。
        let d = l.dist_to(Point2::new(20.0, 0.0));
        assert!(eq_len(d, 10.0));
    }

    #[test]
    fn bbox_matches_endpoints() {
        let l = Line::new(Point2::new(5.0, -1.0), Point2::new(-3.0, 4.0));
        let b = l.bbox();
        assert_eq!(b.min, Point2::new(-3.0, -1.0));
        assert_eq!(b.max, Point2::new(5.0, 4.0));
    }

    #[test]
    fn clip_fully_inside_is_unchanged() {
        let l = Line::new(Point2::new(1.0, 1.0), Point2::new(2.0, 2.0));
        let r = Aabb::new(Point2::new(0.0, 0.0), Point2::new(10.0, 10.0));
        let clipped = l.clip_to(r).expect("内部にあるので Some のはず");
        assert!(clipped.a.eq_tol(l.a));
        assert!(clipped.b.eq_tol(l.b));
    }

    #[test]
    fn clip_fully_outside_is_none() {
        let l = Line::new(Point2::new(-5.0, -5.0), Point2::new(-1.0, -1.0));
        let r = Aabb::new(Point2::new(0.0, 0.0), Point2::new(10.0, 10.0));
        assert_eq!(l.clip_to(r), None);
    }

    #[test]
    fn clip_crossing_one_edge() {
        let l = Line::new(Point2::new(-5.0, 5.0), Point2::new(5.0, 5.0));
        let r = Aabb::new(Point2::new(0.0, 0.0), Point2::new(10.0, 10.0));
        let clipped = l.clip_to(r).expect("左端で交差するので Some のはず");
        assert!(clipped.a.eq_tol(Point2::new(0.0, 5.0)));
        assert!(clipped.b.eq_tol(Point2::new(5.0, 5.0)));
    }

    #[test]
    fn clip_crossing_two_edges() {
        let l = Line::new(Point2::new(-5.0, 5.0), Point2::new(15.0, 5.0));
        let r = Aabb::new(Point2::new(0.0, 0.0), Point2::new(10.0, 10.0));
        let clipped = l.clip_to(r).expect("両端が矩形外なので Some のはず");
        assert!(clipped.a.eq_tol(Point2::new(0.0, 5.0)));
        assert!(clipped.b.eq_tol(Point2::new(10.0, 5.0)));
    }

    #[test]
    fn clip_diagonal_through_corner_region() {
        let l = Line::new(Point2::new(-5.0, -5.0), Point2::new(15.0, 15.0));
        let r = Aabb::new(Point2::new(0.0, 0.0), Point2::new(10.0, 10.0));
        let clipped = l.clip_to(r).expect("対角線が矩形を貫くので Some のはず");
        assert!(clipped.a.eq_tol(Point2::new(0.0, 0.0)));
        assert!(clipped.b.eq_tol(Point2::new(10.0, 10.0)));
    }

    #[test]
    fn clip_degenerate_point_inside_and_outside() {
        let r = Aabb::new(Point2::new(0.0, 0.0), Point2::new(10.0, 10.0));
        let inside = Line::new(Point2::new(5.0, 5.0), Point2::new(5.0, 5.0));
        assert!(inside.clip_to(r).is_some());
        let outside = Line::new(Point2::new(50.0, 50.0), Point2::new(50.0, 50.0));
        assert!(outside.clip_to(r).is_none());
    }

    #[test]
    fn large_and_small_magnitude_lines() {
        let big = Line::new(Point2::new(0.0, 0.0), Point2::new(1e6, 0.0));
        assert!(eq_len(big.length(), 1e6));
        let small = Line::new(Point2::new(0.0, 0.0), Point2::new(0.000_001, 0.0));
        assert!(eq_len(small.length(), 0.000_001));
        assert!(!small.is_degenerate());
    }

    #[test]
    fn translated_moves_both_endpoints() {
        let l = Line::new(Point2::new(1.0, 2.0), Point2::new(3.0, 4.0));
        let moved = l.translated(Vec2::new(10.0, -1.0));
        assert!(moved.a.eq_tol(Point2::new(11.0, 1.0)));
        assert!(moved.b.eq_tol(Point2::new(13.0, 3.0)));
        // 長さは変わらない。
        assert!(eq_len(moved.length(), l.length()));
    }

    #[test]
    fn translated_by_zero_is_unchanged() {
        let l = Line::new(Point2::new(1.0, 2.0), Point2::new(3.0, 4.0));
        let moved = l.translated(Vec2::ZERO);
        assert_eq!(moved, l);
    }
}
