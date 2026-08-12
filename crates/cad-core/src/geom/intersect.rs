//! 図形同士の交点計算。
//!
//! すべて **有界な実体**（線分・円弧）の実際の範囲上の交点だけを返す。
//! 無限直線や全周の円としての交点ではないことに注意。
//! 接する場合はほぼ同一の 2 点ではなく、必ず 1 点だけを返す。

use super::arc::{Arc, Circle};
use super::line::Line;
use super::point::Point2;
use super::tolerance::{eq_len, gt_len, is_zero_len, lt_len};
use super::xline::Xline;

/// パラメータ `t` が `[0, 1]` にトレランス込みで収まるか（線分・円弧の範囲判定用）。
fn in_unit_range(t: f64) -> bool {
    !lt_len(t, 0.0) && !gt_len(t, 1.0)
}

/// 2 線分の交点。0 個または 1 個。
///
/// 平行（外積がゼロ）な場合は、重なっていても空を返す
/// （無限個の交点は表現できないため）。
#[must_use]
pub fn line_line(a: &Line, b: &Line) -> Vec<Point2> {
    let d1 = a.vector();
    let d2 = b.vector();
    let denom = d1.cross(d2);
    if is_zero_len(denom) {
        return Vec::new();
    }

    let diff = b.a - a.a;
    let t = diff.cross(d2) / denom;
    let u = diff.cross(d1) / denom;

    if in_unit_range(t) && in_unit_range(u) {
        vec![a.point_at(t)]
    } else {
        Vec::new()
    }
}

/// 線分と円の交点。0〜2 個。
#[must_use]
pub fn line_circle(l: &Line, c: &Circle) -> Vec<Point2> {
    if l.is_degenerate() {
        return if is_zero_len(c.dist_to(l.a)) {
            vec![l.a]
        } else {
            Vec::new()
        };
    }

    let d = l.vector();
    let f = l.a - c.center;
    let a_coef = d.dot(d);
    let b_coef = 2.0 * f.dot(d);
    let c_coef = f.dot(f) - c.radius * c.radius;

    let disc = b_coef * b_coef - 4.0 * a_coef * c_coef;
    if lt_len(disc, 0.0) {
        return Vec::new();
    }

    // わずかに負になった丸め誤差を吸収するための数値的な安全策（幾何判定ではない）。
    let sqrt_disc = disc.max(0.0).sqrt();
    let t1 = (-b_coef - sqrt_disc) / (2.0 * a_coef);
    let t2 = (-b_coef + sqrt_disc) / (2.0 * a_coef);

    let mut pts = Vec::new();
    if eq_len(t1, t2) {
        // 接する場合はほぼ同一の 2 点ではなく 1 点だけを返す。
        if in_unit_range(t1) {
            pts.push(l.point_at(t1));
        }
    } else {
        if in_unit_range(t1) {
            pts.push(l.point_at(t1));
        }
        if in_unit_range(t2) {
            pts.push(l.point_at(t2));
        }
    }
    pts
}

/// 線分と円弧の交点。0〜2 個。円との交点のうち円弧の掃引範囲内のものだけを残す。
#[must_use]
pub fn line_arc(l: &Line, a: &Arc) -> Vec<Point2> {
    let c = Circle::new(a.center, a.radius);
    line_circle(l, &c)
        .into_iter()
        .filter(|&p| a.contains_angle((p - a.center).angle()))
        .collect()
}

/// 2 円の交点。0〜2 個。同一円・同心円は無限個の交点になるため空を返す。
#[must_use]
pub fn circle_circle(a: &Circle, b: &Circle) -> Vec<Point2> {
    let d = a.center.dist(b.center);
    if is_zero_len(d) {
        return Vec::new();
    }
    if gt_len(d, a.radius + b.radius) || lt_len(d, (a.radius - b.radius).abs()) {
        return Vec::new();
    }

    let dir = (b.center - a.center) / d;
    let aa = (d * d - b.radius * b.radius + a.radius * a.radius) / (2.0 * d);
    // わずかに負になった丸め誤差を吸収するための数値的な安全策（幾何判定ではない）。
    let h_sq = (a.radius * a.radius - aa * aa).max(0.0);
    let h = h_sq.sqrt();
    let mid = a.center + dir * aa;

    if is_zero_len(h) {
        // 接する場合はほぼ同一の 2 点ではなく 1 点だけを返す。
        vec![mid]
    } else {
        let perp = dir.perp();
        vec![mid + perp * h, mid - perp * h]
    }
}

/// 2 円弧の交点。0〜2 個。互いの掃引範囲内のものだけを残す。
#[must_use]
pub fn arc_arc(a: &Arc, b: &Arc) -> Vec<Point2> {
    let ca = Circle::new(a.center, a.radius);
    let cb = Circle::new(b.center, b.radius);
    circle_circle(&ca, &cb)
        .into_iter()
        .filter(|&p| {
            a.contains_angle((p - a.center).angle()) && b.contains_angle((p - b.center).angle())
        })
        .collect()
}

/// 円と円弧の交点。0〜2 個。円弧の掃引範囲内のものだけを残す。
#[must_use]
pub fn circle_arc(c: &Circle, a: &Arc) -> Vec<Point2> {
    let ca = Circle::new(a.center, a.radius);
    circle_circle(c, &ca)
        .into_iter()
        .filter(|&p| a.contains_angle((p - a.center).angle()))
        .collect()
}

// ---------------------------------------------------------------------------
// 作図線（無限直線）との交点
// ---------------------------------------------------------------------------
//
// 作図線は無限に伸びるので、相手側の範囲だけを見ればよい。
// 実装は「作図線から十分な長さの線分を作って既存の関数へ渡す」形にはしない。
// 十分な長さを決められないうえ、相手が遠方にあると届かないため。
// 代わりに、作図線をパラメータ表現のまま扱う。

/// 作図線と線分の交点。0 個または 1 個。
///
/// 線分の範囲内にある交点だけを返す。平行なら空。
#[must_use]
pub fn xline_line(x: &Xline, l: &Line) -> Vec<Point2> {
    let d = l.vector();
    let denom = x.direction.cross(d);
    if is_zero_len(denom) {
        return Vec::new();
    }
    // x.origin + s * x.direction == l.a + u * d を解く。線分側の u だけ範囲を見る。
    let diff = l.a - x.origin;
    let u = diff.cross(x.direction) / denom;
    if in_unit_range(u) {
        vec![l.point_at(u)]
    } else {
        Vec::new()
    }
}

/// 作図線と円の交点。0〜2 個。接する場合は 1 個。
#[must_use]
pub fn xline_circle(x: &Xline, c: &Circle) -> Vec<Point2> {
    // 中心から直線への垂線の足を基準に、半弦長を求める。
    let foot = x.closest_point(c.center);
    let dist = c.center.dist(foot);
    if gt_len(dist, c.radius) {
        return Vec::new();
    }
    if eq_len(dist, c.radius) {
        return vec![foot];
    }
    let half_chord = (c.radius * c.radius - dist * dist).max(0.0).sqrt();
    if is_zero_len(half_chord) {
        return vec![foot];
    }
    vec![
        foot + x.direction * half_chord,
        foot - x.direction * half_chord,
    ]
}

/// 作図線と円弧の交点。円弧の掃引範囲にある交点だけを返す。
#[must_use]
pub fn xline_arc(x: &Xline, a: &Arc) -> Vec<Point2> {
    let circle = Circle::new(a.center, a.radius);
    xline_circle(x, &circle)
        .into_iter()
        .filter(|p| a.contains_angle((*p - a.center).angle()))
        .collect()
}

/// 作図線同士の交点。0 個または 1 個。平行なら空。
#[must_use]
pub fn xline_xline(a: &Xline, b: &Xline) -> Vec<Point2> {
    let denom = a.direction.cross(b.direction);
    if is_zero_len(denom) {
        return Vec::new();
    }
    let s = (b.origin - a.origin).cross(b.direction) / denom;
    vec![a.point_at(s)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::point::Point2;
    use std::f64::consts::PI;

    #[test]
    fn line_line_crossing() {
        let a = Line::new(Point2::new(0.0, 0.0), Point2::new(10.0, 10.0));
        let b = Line::new(Point2::new(0.0, 10.0), Point2::new(10.0, 0.0));
        let pts = line_line(&a, &b);
        assert_eq!(pts.len(), 1);
        assert!(pts[0].eq_tol(Point2::new(5.0, 5.0)));
    }

    #[test]
    fn line_line_parallel_no_intersection() {
        let a = Line::new(Point2::new(0.0, 0.0), Point2::new(10.0, 0.0));
        let b = Line::new(Point2::new(0.0, 1.0), Point2::new(10.0, 1.0));
        assert!(line_line(&a, &b).is_empty());
    }

    #[test]
    fn line_line_segments_not_reaching_each_other() {
        // 延長すれば交わるが、線分としては交わらないケース。
        let a = Line::new(Point2::new(0.0, 0.0), Point2::new(1.0, 1.0));
        let b = Line::new(Point2::new(0.0, 10.0), Point2::new(1.0, 9.0));
        assert!(line_line(&a, &b).is_empty());
    }

    #[test]
    fn line_line_touching_at_endpoint() {
        let a = Line::new(Point2::new(0.0, 0.0), Point2::new(10.0, 0.0));
        let b = Line::new(Point2::new(10.0, 0.0), Point2::new(10.0, 10.0));
        let pts = line_line(&a, &b);
        assert_eq!(pts.len(), 1);
        assert!(pts[0].eq_tol(Point2::new(10.0, 0.0)));
    }

    #[test]
    fn line_circle_two_points() {
        let l = Line::new(Point2::new(-10.0, 0.0), Point2::new(10.0, 0.0));
        let c = Circle::new(Point2::ORIGIN, 5.0);
        let pts = line_circle(&l, &c);
        assert_eq!(pts.len(), 2);
    }

    #[test]
    fn line_circle_tangent_single_point() {
        let l = Line::new(Point2::new(-10.0, 5.0), Point2::new(10.0, 5.0));
        let c = Circle::new(Point2::ORIGIN, 5.0);
        let pts = line_circle(&l, &c);
        assert_eq!(pts.len(), 1);
        assert!(pts[0].eq_tol(Point2::new(0.0, 5.0)));
    }

    #[test]
    fn line_circle_no_intersection() {
        let l = Line::new(Point2::new(-10.0, 100.0), Point2::new(10.0, 100.0));
        let c = Circle::new(Point2::ORIGIN, 5.0);
        assert!(line_circle(&l, &c).is_empty());
    }

    #[test]
    fn line_circle_segment_too_short_to_reach() {
        // 直線としては円と交わるが、線分の範囲には届かないケース。
        let l = Line::new(Point2::new(-10.0, 0.0), Point2::new(-6.0, 0.0));
        let c = Circle::new(Point2::ORIGIN, 5.0);
        assert!(line_circle(&l, &c).is_empty());
    }

    #[test]
    fn line_circle_degenerate_line_on_circumference() {
        let l = Line::new(Point2::new(5.0, 0.0), Point2::new(5.0, 0.0));
        let c = Circle::new(Point2::ORIGIN, 5.0);
        let pts = line_circle(&l, &c);
        assert_eq!(pts.len(), 1);
    }

    #[test]
    fn line_arc_filters_by_sweep() {
        // 上半円の円弧。水平線分は円の左右 2 点と交わるが、円弧上にあるのは上側の点だけ。
        let a = Arc::new(Point2::ORIGIN, 5.0, 0.0, PI);
        let l = Line::new(Point2::new(-10.0, 0.0), Point2::new(10.0, 0.0));
        let pts = line_arc(&l, &a);
        assert_eq!(pts.len(), 2);
        for p in &pts {
            assert!(eq_len(p.y.abs(), 0.0));
        }
    }

    #[test]
    fn line_arc_quarter_circle_one_point() {
        let a = Arc::new(Point2::ORIGIN, 5.0, 0.0, std::f64::consts::FRAC_PI_2);
        let l = Line::new(Point2::new(-10.0, 3.0), Point2::new(10.0, 3.0));
        let pts = line_arc(&l, &a);
        assert_eq!(pts.len(), 1);
        assert!(pts[0].x > 0.0);
    }

    #[test]
    fn circle_circle_two_points() {
        let a = Circle::new(Point2::new(-1.0, 0.0), 2.0);
        let b = Circle::new(Point2::new(1.0, 0.0), 2.0);
        let pts = circle_circle(&a, &b);
        assert_eq!(pts.len(), 2);
    }

    #[test]
    fn circle_circle_external_tangent() {
        let a = Circle::new(Point2::new(0.0, 0.0), 3.0);
        let b = Circle::new(Point2::new(6.0, 0.0), 3.0);
        let pts = circle_circle(&a, &b);
        assert_eq!(pts.len(), 1);
        assert!(pts[0].eq_tol(Point2::new(3.0, 0.0)));
    }

    #[test]
    fn circle_circle_internal_tangent() {
        let a = Circle::new(Point2::new(0.0, 0.0), 5.0);
        let b = Circle::new(Point2::new(2.0, 0.0), 3.0);
        let pts = circle_circle(&a, &b);
        assert_eq!(pts.len(), 1);
        assert!(pts[0].eq_tol(Point2::new(5.0, 0.0)));
    }

    #[test]
    fn circle_circle_too_far_apart() {
        let a = Circle::new(Point2::new(0.0, 0.0), 1.0);
        let b = Circle::new(Point2::new(100.0, 0.0), 1.0);
        assert!(circle_circle(&a, &b).is_empty());
    }

    #[test]
    fn circle_circle_one_inside_other() {
        let a = Circle::new(Point2::new(0.0, 0.0), 10.0);
        let b = Circle::new(Point2::new(0.0, 0.0), 1.0);
        assert!(circle_circle(&a, &b).is_empty());
    }

    #[test]
    fn circle_circle_identical_is_empty() {
        let a = Circle::new(Point2::new(1.0, 2.0), 5.0);
        let b = a;
        assert!(circle_circle(&a, &b).is_empty());
    }

    #[test]
    fn circle_circle_concentric_different_radius_is_empty() {
        let a = Circle::new(Point2::ORIGIN, 5.0);
        let b = Circle::new(Point2::ORIGIN, 3.0);
        assert!(circle_circle(&a, &b).is_empty());
    }

    #[test]
    fn arc_arc_filters_both_sweeps() {
        let a = Arc::new(Point2::new(-1.0, 0.0), 2.0, 0.0, PI);
        let b = Arc::new(Point2::new(1.0, 0.0), 2.0, 0.0, PI);
        let pts = arc_arc(&a, &b);
        // 円同士は上下 2 点で交わるが、両方とも上半円弧なので残るのは上側の 1 点だけ。
        assert_eq!(pts.len(), 1);
        assert!(pts[0].y > 0.0);
    }

    #[test]
    fn circle_arc_filters_by_sweep() {
        // 上半円の円弧。もう一方の円とは左右対称な 2 点で交わりうるが、
        // 円弧の掃引範囲（上半分）に入るのは 1 点だけ。
        let a = Arc::new(Point2::new(-1.0, 0.0), 2.0, 0.0, PI);
        let other = Circle::new(Point2::new(1.0, 0.0), 2.0);
        let pts = circle_arc(&other, &a);
        assert_eq!(pts.len(), 1);
        for p in &pts {
            assert!(a.contains_angle((*p - a.center).angle()));
            assert!(p.y > 0.0);
        }
    }

    #[test]
    fn large_and_small_scale_intersections() {
        let a = Line::new(Point2::new(-1e6, 0.0), Point2::new(1e6, 0.0));
        let c = Circle::new(Point2::ORIGIN, 1e5);
        let pts = line_circle(&a, &c);
        assert_eq!(pts.len(), 2);

        let a2 = Line::new(Point2::new(-0.000_001, 0.0), Point2::new(0.000_001, 0.0));
        let c2 = Circle::new(Point2::ORIGIN, 0.000_000_1);
        let pts2 = line_circle(&a2, &c2);
        assert_eq!(pts2.len(), 2);
    }

    // ---- 作図線との交点 ----

    fn xl(ox: f64, oy: f64, dx: f64, dy: f64) -> Xline {
        Xline::new(Point2::new(ox, oy), crate::geom::Vec2::new(dx, dy)).unwrap()
    }

    #[test]
    fn xline_crosses_a_segment_within_its_extent() {
        let x = xl(0.0, 0.0, 0.0, 1.0); // Y 軸
        let l = Line::new(Point2::new(-5.0, 3.0), Point2::new(5.0, 3.0));
        let hits = xline_line(&x, &l);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].eq_tol(Point2::new(0.0, 3.0)), "{:?}", hits[0]);
    }

    /// 線分の範囲外で交わる場合は拾わないこと（作図線は無限でも相手は有限）。
    #[test]
    fn xline_misses_a_segment_that_ends_before_it() {
        let x = xl(0.0, 0.0, 0.0, 1.0);
        let l = Line::new(Point2::new(2.0, 3.0), Point2::new(5.0, 3.0));
        assert!(xline_line(&x, &l).is_empty());
    }

    #[test]
    fn xline_parallel_to_a_segment_yields_nothing() {
        let x = xl(0.0, 0.0, 1.0, 0.0);
        let l = Line::new(Point2::new(-5.0, 3.0), Point2::new(5.0, 3.0));
        assert!(xline_line(&x, &l).is_empty());
    }

    /// 作図線は無限なので、線分がはるか遠くにあっても交点を拾えること。
    #[test]
    fn xline_reaches_a_far_away_segment() {
        let x = xl(0.0, 0.0, 1.0, 0.0);
        let l = Line::new(Point2::new(1e6, -5.0), Point2::new(1e6, 5.0));
        let hits = xline_line(&x, &l);
        assert_eq!(hits.len(), 1);
        assert!(eq_len(hits[0].x, 1e6), "{:?}", hits[0]);
    }

    #[test]
    fn xline_crosses_a_circle_at_two_points() {
        let x = xl(0.0, 0.0, 1.0, 0.0);
        let c = Circle::new(Point2::ORIGIN, 5.0);
        let hits = xline_circle(&x, &c);
        assert_eq!(hits.len(), 2);
        let xs: Vec<f64> = hits.iter().map(|p| p.x).collect();
        assert!(xs.iter().any(|v| eq_len(*v, 5.0)));
        assert!(xs.iter().any(|v| eq_len(*v, -5.0)));
    }

    #[test]
    fn xline_tangent_to_a_circle_yields_one_point() {
        let x = xl(0.0, 5.0, 1.0, 0.0);
        let c = Circle::new(Point2::ORIGIN, 5.0);
        let hits = xline_circle(&x, &c);
        assert_eq!(hits.len(), 1, "接するので 1 点だけ: {hits:?}");
        assert!(hits[0].eq_tol(Point2::new(0.0, 5.0)));
    }

    #[test]
    fn xline_missing_a_circle_yields_nothing() {
        let x = xl(0.0, 10.0, 1.0, 0.0);
        assert!(xline_circle(&x, &Circle::new(Point2::ORIGIN, 5.0)).is_empty());
    }

    /// 円弧では掃引の外にある交点を落とすこと。
    #[test]
    fn xline_arc_keeps_only_points_within_the_sweep() {
        use std::f64::consts::PI;
        // 上半円だけの円弧。
        let a = Arc::new(Point2::ORIGIN, 5.0, 0.0, PI);
        let x = xl(0.0, 0.0, 1.0, 0.0); // X 軸: 円とは (5,0) と (-5,0) で交わる
        let hits = xline_arc(&x, &a);
        // 端点はどちらも掃引に含まれる。
        assert_eq!(hits.len(), 2, "{hits:?}");

        // Y 軸は上半円の頂点だけで交わる。
        let y = xl(0.0, 0.0, 0.0, 1.0);
        let hits = xline_arc(&y, &a);
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert!(hits[0].eq_tol(Point2::new(0.0, 5.0)));
    }

    #[test]
    fn two_xlines_cross_once() {
        let a = xl(0.0, 0.0, 1.0, 0.0);
        let b = xl(3.0, 0.0, 0.0, 1.0);
        let hits = xline_xline(&a, &b);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].eq_tol(Point2::new(3.0, 0.0)), "{:?}", hits[0]);
    }

    #[test]
    fn parallel_xlines_yield_nothing() {
        let a = xl(0.0, 0.0, 1.0, 0.0);
        let b = xl(0.0, 5.0, 1.0, 0.0);
        assert!(xline_xline(&a, &b).is_empty());
        // 完全に重なっていても、無限個の交点は表現できないので空。
        let c = xl(10.0, 0.0, 1.0, 0.0);
        assert!(xline_xline(&a, &c).is_empty());
    }
}
