//! 円と円弧。
//!
//! 角度はすべてラジアン。[`Arc`] は `start_angle` から `end_angle` へ **反時計回り (CCW)**
//! に掃引する。これは DXF R12 の ARC エンティティの規約に一致する。

use super::aabb::Aabb;
use super::point::{Point2, Vec2};
use super::tolerance::{eq_angle, gt_len, is_zero_len, wrap_2pi};
use std::f64::consts::{FRAC_PI_2, PI, TAU};

/// テッセレーション分割数の下限。2 点未満では線分にならない。
const MIN_TESS_SEGMENTS: u32 = 2;
/// テッセレーション分割数の上限。`max_sagitta` が極端に小さくてもレンダラをハングさせない。
const MAX_TESS_SEGMENTS: u32 = 4096;

/// 円。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Circle {
    /// 中心。
    pub center: Point2,
    /// 半径。
    pub radius: f64,
}

impl Circle {
    /// 中心と半径から円を作る。
    #[inline]
    #[must_use]
    pub fn new(center: Point2, radius: f64) -> Self {
        Self { center, radius }
    }

    /// 角度 `rad` [rad] における円周上の点。
    #[inline]
    #[must_use]
    pub fn point_at_angle(&self, rad: f64) -> Point2 {
        self.center + Vec2::polar(rad, self.radius)
    }

    /// 半径がトレランス内でゼロとみなせるか（退化円か）。
    #[inline]
    #[must_use]
    pub fn is_degenerate(&self) -> bool {
        is_zero_len(self.radius)
    }

    /// 円を包む境界ボックス。
    #[inline]
    #[must_use]
    pub fn bbox(&self) -> Aabb {
        let r = Vec2::new(self.radius, self.radius);
        Aabb::new(self.center - r, self.center + r)
    }

    /// 点 `p` が円の内部（境界含む、トレランス込み）にあるか。
    #[inline]
    #[must_use]
    pub fn contains(&self, p: Point2) -> bool {
        !gt_len(self.center.dist(p), self.radius)
    }

    /// 点 `p` から円周までの距離。
    #[inline]
    #[must_use]
    pub fn dist_to(&self, p: Point2) -> f64 {
        (self.center.dist(p) - self.radius).abs()
    }

    /// 真の円との誤差（サジッタ）が `max_sagitta` 以下になるポリライン近似。
    ///
    /// 退化円（半径 ~ 0）では中心 1 点のみを返す。
    #[must_use]
    pub fn tessellate(&self, max_sagitta: f64) -> Vec<Point2> {
        if self.is_degenerate() {
            return vec![self.center];
        }
        let n = tessellation_segment_count(self.radius, TAU, max_sagitta);
        (0..n)
            .map(|i| self.point_at_angle(TAU * f64::from(i) / f64::from(n)))
            .collect()
    }
}

/// 円弧。`start_angle` から `end_angle` へ反時計回りに掃引する（DXF R12 ARC 準拠）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Arc {
    /// 中心。
    pub center: Point2,
    /// 半径。
    pub radius: f64,
    /// 開始角 [rad]。
    pub start_angle: f64,
    /// 終了角 [rad]。
    pub end_angle: f64,
}

impl Arc {
    /// 中心・半径・開始角・終了角から円弧を作る。
    #[inline]
    #[must_use]
    pub fn new(center: Point2, radius: f64, start_angle: f64, end_angle: f64) -> Self {
        Self {
            center,
            radius,
            start_angle,
            end_angle,
        }
    }

    /// 掃引角。常に `(0, 2π]` の範囲になる。
    ///
    /// `start_angle` と `end_angle` がトレランス内で一致する場合は「1 周まるごと」
    /// （`2π`）と解釈する。`0` にはならない。
    #[must_use]
    pub fn sweep(&self) -> f64 {
        let raw = wrap_2pi(self.end_angle - self.start_angle);
        if eq_angle(raw, 0.0) {
            TAU
        } else {
            raw
        }
    }

    /// 円周上の角度 `rad` に対応する点（円弧の内外は問わない）。
    #[inline]
    fn point_at_angle(&self, rad: f64) -> Point2 {
        self.center + Vec2::polar(rad, self.radius)
    }

    /// 始点。
    #[inline]
    #[must_use]
    pub fn start_point(&self) -> Point2 {
        self.point_at_angle(self.start_angle)
    }

    /// 終点。
    #[inline]
    #[must_use]
    pub fn end_point(&self) -> Point2 {
        self.point_at_angle(self.start_angle + self.sweep())
    }

    /// 掃引の中点（弧の真ん中）。フェーズ 4 の OSNAP 中点スナップに使う。
    #[inline]
    #[must_use]
    pub fn mid_point(&self) -> Point2 {
        self.point_at(0.5)
    }

    /// 角度 `rad` が円弧の掃引範囲内（境界含む、トレランス込み）にあるか。
    #[must_use]
    pub fn contains_angle(&self, rad: f64) -> bool {
        let offset = wrap_2pi(rad - self.start_angle);
        le_angle(offset, self.sweep())
    }

    /// 掃引に沿ったパラメータ `t`（`0` で始点、`1` で終点）上の点。
    #[inline]
    #[must_use]
    pub fn point_at(&self, t: f64) -> Point2 {
        self.point_at_angle(self.start_angle + self.sweep() * t)
    }

    /// 円弧の弧長。
    #[inline]
    #[must_use]
    pub fn length(&self) -> f64 {
        self.radius * self.sweep()
    }

    /// 円弧を包む境界ボックス。
    ///
    /// 単純に始点・終点だけでは不十分。掃引範囲に含まれる象限の頂点（角度
    /// `0, π/2, π, 3π/2`）も走査して含める必要がある。
    #[must_use]
    pub fn bbox(&self) -> Aabb {
        let mut b = Aabb::new(self.start_point(), self.end_point());
        for q in [0.0, FRAC_PI_2, PI, 3.0 * FRAC_PI_2] {
            if self.contains_angle(q) {
                b = b.union_point(self.point_at_angle(q));
            }
        }
        b
    }

    /// 3 点 `a`, `b`, `c` を通る円弧を作る。`b` を通るように掃引方向を選ぶ。
    ///
    /// 3 点が共線の場合は `None`。
    #[must_use]
    pub fn from_3_points(a: Point2, b: Point2, c: Point2) -> Option<Self> {
        let d = 2.0 * (a.x * (b.y - c.y) + b.x * (c.y - a.y) + c.x * (a.y - b.y));
        if is_zero_len(d) {
            return None;
        }

        let a_sq = a.x * a.x + a.y * a.y;
        let b_sq = b.x * b.x + b.y * b.y;
        let c_sq = c.x * c.x + c.y * c.y;

        let ux = (a_sq * (b.y - c.y) + b_sq * (c.y - a.y) + c_sq * (a.y - b.y)) / d;
        let uy = (a_sq * (c.x - b.x) + b_sq * (a.x - c.x) + c_sq * (b.x - a.x)) / d;
        let center = Point2::new(ux, uy);
        let radius = center.dist(a);

        let ang_a = (a - center).angle();
        let ang_b = (b - center).angle();
        let ang_c = (c - center).angle();

        let sweep_ac = wrap_2pi(ang_c - ang_a);
        let offset_b = wrap_2pi(ang_b - ang_a);

        Some(if le_angle(offset_b, sweep_ac) {
            Self::new(center, radius, ang_a, ang_c)
        } else {
            Self::new(center, radius, ang_c, ang_a)
        })
    }

    /// 点 `p` から円弧（有界）への距離。
    #[must_use]
    pub fn dist_to(&self, p: Point2) -> f64 {
        let ang = (p - self.center).angle();
        if self.contains_angle(ang) {
            (self.center.dist(p) - self.radius).abs()
        } else {
            p.dist(self.start_point()).min(p.dist(self.end_point()))
        }
    }

    /// 真の円弧との誤差（サジッタ）が `max_sagitta` 以下になるポリライン近似。
    ///
    /// 始点と終点を両方含む（`n + 1` 点）。退化円弧（半径 ~ 0）では中心 1 点のみを返す。
    #[must_use]
    pub fn tessellate(&self, max_sagitta: f64) -> Vec<Point2> {
        if is_zero_len(self.radius) {
            return vec![self.center];
        }
        let n = tessellation_segment_count(self.radius, self.sweep(), max_sagitta);
        (0..=n)
            .map(|i| self.point_at(f64::from(i) / f64::from(n)))
            .collect()
    }
}

/// 角度の `<=` をトレランス込みで判定する（[`Arc::contains_angle`] などの内部ヘルパー）。
///
/// `tolerance` モジュールの `lt_len` / `gt_len` と同じ「等価帯を優先し、その外側は
/// 生の比較に委ねる」という方針を [`eq_angle`] で角度に適用したもの。
fn le_angle(a: f64, b: f64) -> bool {
    eq_angle(a, b) || a < b
}

/// `sweep` 全体を弧長誤差 `max_sagitta` 以下で近似するために必要な分割数を求める。
///
/// サジッタの式 `s = r * (1 - cos(θ/2))` を満たす最小の分割数を、
/// 下限 [`MIN_TESS_SEGMENTS`] から上限 [`MAX_TESS_SEGMENTS`] まで走査して探す
/// （閉形式の `ceil` 計算では `f64` → 整数の丸め込みキャストが避けられないため、
/// 整数を直接インクリメントするループで代替している）。
fn tessellation_segment_count(radius: f64, sweep: f64, max_sagitta: f64) -> u32 {
    let mut n = MIN_TESS_SEGMENTS;
    while n < MAX_TESS_SEGMENTS {
        let seg_angle = sweep / f64::from(n);
        let sagitta = radius * (1.0 - (seg_angle * 0.5).cos());
        if !gt_len(sagitta, max_sagitta) {
            break;
        }
        n += 1;
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::tolerance::{eq_len, EPS_LEN};
    use std::f64::consts::FRAC_PI_4;

    #[test]
    fn circle_point_at_angle() {
        let c = Circle::new(Point2::ORIGIN, 2.0);
        assert!(c.point_at_angle(0.0).eq_tol(Point2::new(2.0, 0.0)));
        assert!(c.point_at_angle(FRAC_PI_2).eq_tol(Point2::new(0.0, 2.0)));
    }

    #[test]
    fn circle_is_degenerate_boundary() {
        assert!(Circle::new(Point2::ORIGIN, EPS_LEN * 0.5).is_degenerate());
        assert!(!Circle::new(Point2::ORIGIN, EPS_LEN * 1000.0).is_degenerate());
    }

    #[test]
    fn circle_bbox() {
        let c = Circle::new(Point2::new(1.0, 2.0), 3.0);
        let b = c.bbox();
        assert!(b.min.eq_tol(Point2::new(-2.0, -1.0)));
        assert!(b.max.eq_tol(Point2::new(4.0, 5.0)));
    }

    #[test]
    fn circle_contains_and_dist_to() {
        let c = Circle::new(Point2::ORIGIN, 5.0);
        assert!(c.contains(Point2::new(3.0, 0.0)));
        assert!(!c.contains(Point2::new(6.0, 0.0)));
        assert!(eq_len(c.dist_to(Point2::new(8.0, 0.0)), 3.0));
        assert!(eq_len(c.dist_to(Point2::ORIGIN), 5.0));
    }

    #[test]
    fn circle_tessellate_degenerate() {
        let c = Circle::new(Point2::new(1.0, 1.0), EPS_LEN * 0.1);
        let pts = c.tessellate(0.01);
        assert_eq!(pts.len(), 1);
    }

    #[test]
    fn circle_tessellate_stays_within_sagitta() {
        let c = Circle::new(Point2::ORIGIN, 10.0);
        let max_sagitta = 0.01;
        let pts = c.tessellate(max_sagitta);
        assert!(pts.len() >= 3);
        // 隣接点を結ぶ弦の中点は、実際の円周からサジッタ以下しか離れていないはず。
        for i in 0..pts.len() {
            let p0 = pts[i];
            let p1 = pts[(i + 1) % pts.len()];
            let mid = p0.lerp(p1, 0.5);
            let sagitta = c.dist_to(mid);
            assert!(!gt_len(sagitta, max_sagitta * 1.01));
        }
    }

    #[test]
    fn arc_sweep_normal_and_full_circle() {
        let a = Arc::new(Point2::ORIGIN, 1.0, 0.0, FRAC_PI_2);
        assert!(eq_len(a.sweep(), FRAC_PI_2));

        let full = Arc::new(Point2::ORIGIN, 1.0, 0.0, 0.0);
        assert!(eq_len(full.sweep(), TAU));
    }

    #[test]
    fn arc_start_end_points() {
        let a = Arc::new(Point2::ORIGIN, 1.0, 0.0, FRAC_PI_2);
        assert!(a.start_point().eq_tol(Point2::new(1.0, 0.0)));
        assert!(a.end_point().eq_tol(Point2::new(0.0, 1.0)));
    }

    #[test]
    fn arc_mid_point() {
        let a = Arc::new(Point2::ORIGIN, 1.0, 0.0, PI);
        assert!(a.mid_point().eq_tol(Point2::new(0.0, 1.0)));
    }

    #[test]
    fn arc_contains_angle_inclusive_boundaries() {
        let a = Arc::new(Point2::ORIGIN, 1.0, 0.0, PI);
        assert!(a.contains_angle(0.0));
        assert!(a.contains_angle(PI));
        assert!(a.contains_angle(FRAC_PI_2));
        assert!(!a.contains_angle(PI + FRAC_PI_2));
    }

    #[test]
    fn arc_point_at_parametrizes_sweep() {
        let a = Arc::new(Point2::ORIGIN, 1.0, 0.0, PI);
        assert!(a.point_at(0.0).eq_tol(a.start_point()));
        assert!(a.point_at(1.0).eq_tol(a.end_point()));
        assert!(a.point_at(0.5).eq_tol(Point2::new(0.0, 1.0)));
    }

    #[test]
    fn arc_length() {
        let a = Arc::new(Point2::ORIGIN, 2.0, 0.0, PI);
        assert!(eq_len(a.length(), 2.0 * PI));
    }

    #[test]
    fn arc_bbox_quadrant_crossing_plus_x_axis() {
        // -45度から+45度まで、+X軸をまたぐ円弧。
        let a = Arc::new(Point2::ORIGIN, 5.0, -FRAC_PI_4, FRAC_PI_4);
        let b = a.bbox();
        assert!(eq_len(b.max.x, 5.0));
    }

    #[test]
    fn arc_bbox_not_crossing_axis_uses_endpoints_only() {
        // 第一象限だけの円弧（象限頂点をまたがない）。
        let a = Arc::new(Point2::ORIGIN, 5.0, FRAC_PI_4 * 0.5, FRAC_PI_4 * 1.5);
        let b = a.bbox();
        assert!(b.max.x < 5.0);
        assert!(b.max.y < 5.0);
    }

    #[test]
    fn arc_bbox_full_circle_includes_all_quadrants() {
        let a = Arc::new(Point2::new(1.0, 1.0), 2.0, 0.0, 0.0);
        let b = a.bbox();
        assert!(b.min.eq_tol(Point2::new(-1.0, -1.0)));
        assert!(b.max.eq_tol(Point2::new(3.0, 3.0)));
    }

    #[test]
    fn arc_from_3_points_recovers_circle() {
        let a = Point2::new(1.0, 0.0);
        let b = Point2::new(0.0, 1.0);
        let c = Point2::new(-1.0, 0.0);
        let arc = Arc::from_3_points(a, b, c).expect("共線ではないはず");
        assert!(arc.center.eq_tol(Point2::ORIGIN));
        assert!(eq_len(arc.radius, 1.0));
        // b を通るように掃引方向が選ばれているはず。
        assert!(arc.contains_angle((b - arc.center).angle()));
    }

    #[test]
    fn arc_from_3_points_collinear_is_none() {
        let a = Point2::new(0.0, 0.0);
        let b = Point2::new(1.0, 0.0);
        let c = Point2::new(2.0, 0.0);
        assert_eq!(Arc::from_3_points(a, b, c), None);
    }

    #[test]
    fn arc_from_3_points_picks_correct_direction() {
        // b が短い弧側にあるケースと長い弧側にあるケースの両方を確認する。
        let a = Point2::new(1.0, 0.0);
        let c = Point2::new(-1.0, 0.0);
        let short_b = Point2::new(0.0, 1.0);
        let long_b = Point2::new(0.0, -1.0);

        let short_arc = Arc::from_3_points(a, short_b, c).unwrap();
        assert!(short_arc.contains_angle((short_b - short_arc.center).angle()));

        let long_arc = Arc::from_3_points(a, long_b, c).unwrap();
        assert!(long_arc.contains_angle((long_b - long_arc.center).angle()));
    }

    #[test]
    fn arc_dist_to_on_sweep_and_off_sweep() {
        let a = Arc::new(Point2::ORIGIN, 5.0, 0.0, FRAC_PI_2);
        // 掃引範囲内の方向: 円周までの距離。
        assert!(eq_len(a.dist_to(Point2::new(10.0, 0.0)), 5.0));
        // 掃引範囲外の方向: 最寄りの端点までの距離。
        let far = Point2::new(0.0, -100.0);
        let expected = far.dist(a.start_point()).min(far.dist(a.end_point()));
        assert!(eq_len(a.dist_to(far), expected));
    }

    #[test]
    fn arc_tessellate_endpoints_included() {
        let a = Arc::new(Point2::ORIGIN, 10.0, 0.0, PI);
        let pts = a.tessellate(0.1);
        assert!(pts.first().unwrap().eq_tol(a.start_point()));
        assert!(pts.last().unwrap().eq_tol(a.end_point()));
        assert!(pts.len() >= 3);
    }

    #[test]
    fn arc_tessellate_degenerate_radius() {
        let a = Arc::new(Point2::new(2.0, 2.0), EPS_LEN * 0.1, 0.0, PI);
        let pts = a.tessellate(0.1);
        assert_eq!(pts.len(), 1);
    }

    #[test]
    fn arc_large_and_small_radius() {
        let big = Arc::new(Point2::ORIGIN, 1e6, 0.0, FRAC_PI_2);
        assert!(eq_len(big.length(), 1e6 * FRAC_PI_2));
        let small = Arc::new(Point2::ORIGIN, 0.000_001, 0.0, FRAC_PI_2);
        assert!(!is_zero_len(small.radius));
        assert!(eq_len(small.length(), 0.000_001 * FRAC_PI_2));
    }
}
