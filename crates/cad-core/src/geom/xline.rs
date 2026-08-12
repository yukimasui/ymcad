//! 無限に伸びる作図線（AutoCAD の XLINE）。

use crate::geom::tolerance::is_zero_len;
use crate::geom::{Aabb, Line, Point2, Vec2};

/// 表示範囲へクリップするとき、範囲の対角長の何倍まで伸ばすか。
///
/// 対角長ぶんあれば矩形は必ず跨げるが、原点が範囲の外にある場合や
/// 丸め誤差を考えて少し余裕を持たせる。
const CLIP_SPAN_FACTOR: f64 = 2.0;

/// 無限に伸びる作図線。
///
/// `origin` を通り `direction` の向きに **両方向へ無限に** 伸びる。
/// `direction` は常に正規化して保持する。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Xline {
    /// 通過点。
    pub origin: Point2,
    /// 方向（正規化済み）。
    pub direction: Vec2,
}

impl Xline {
    /// 方向ベクトルを正規化して作る。長さ 0 の方向は `None`。
    #[must_use]
    pub fn new(origin: Point2, direction: Vec2) -> Option<Self> {
        Some(Self {
            origin,
            direction: direction.normalized()?,
        })
    }

    /// 2 点を通る作図線。2 点が同じなら `None`。
    #[must_use]
    pub fn through(a: Point2, b: Point2) -> Option<Self> {
        Self::new(a, b - a)
    }

    /// `origin` を通る水平線。
    #[must_use]
    pub fn horizontal(origin: Point2) -> Self {
        Self {
            origin,
            direction: Vec2::X,
        }
    }

    /// `origin` を通る垂直線。
    #[must_use]
    pub fn vertical(origin: Point2) -> Self {
        Self {
            origin,
            direction: Vec2::Y,
        }
    }

    /// `origin` を通り角度 `rad` の作図線。
    #[must_use]
    pub fn at_angle(origin: Point2, rad: f64) -> Self {
        Self {
            origin,
            direction: Vec2::from_angle(rad),
        }
    }

    /// 直線の傾き [rad]。
    #[must_use]
    pub fn angle(&self) -> f64 {
        self.direction.angle()
    }

    /// 点を直線上へ射影したときのパラメータ（`origin` からの符号つき距離）。
    ///
    /// `direction` が正規化されているので、値はそのまま距離になる。
    #[must_use]
    pub fn param_at(&self, p: Point2) -> f64 {
        (p - self.origin).dot(self.direction)
    }

    /// パラメータ `t` の位置の点。
    #[must_use]
    pub fn point_at(&self, t: f64) -> Point2 {
        self.origin + self.direction * t
    }

    /// 点を直線上へ射影した点。
    #[must_use]
    pub fn closest_point(&self, p: Point2) -> Point2 {
        self.point_at(self.param_at(p))
    }

    /// 点との距離（無限直線への垂線距離）。
    #[must_use]
    pub fn dist_to(&self, p: Point2) -> f64 {
        // direction は単位ベクトルなので、外積の絶対値がそのまま距離になる。
        (p - self.origin).cross(self.direction).abs()
    }

    /// 平行移動。
    #[must_use]
    pub fn translated(&self, v: Vec2) -> Self {
        Self {
            origin: self.origin + v,
            direction: self.direction,
        }
    }

    /// `center` を中心に `angle` [rad] 回転する。
    #[must_use]
    pub fn rotated(&self, center: Point2, angle: f64) -> Self {
        Self {
            origin: center + (self.origin - center).rotated(angle),
            direction: self.direction.rotated(angle),
        }
    }

    /// `center` を中心に `factor` 倍に拡大縮小する。
    ///
    /// 一様な拡大縮小では向きが変わらないので、`direction` はそのまま。
    /// 倍率が 0 や非有限な場合は何もしない（呼び出し側で弾く前提の安全網）。
    #[must_use]
    pub fn scaled(&self, center: Point2, factor: f64) -> Self {
        if !factor.is_finite() || factor == 0.0 {
            return *self;
        }
        Self {
            origin: center + (self.origin - center) * factor,
            direction: self.direction,
        }
    }

    /// `axis` を鏡像軸として反転する。
    ///
    /// 軸が退化していれば何もしない。
    #[must_use]
    pub fn mirrored(&self, axis: &Line) -> Self {
        let Some(dir) = axis.dir() else {
            return *self;
        };
        // 点は軸への垂線の足を挟んで反対側へ、方向ベクトルは軸に対して反射する。
        let foot = axis.point_at(axis.closest_param(self.origin));
        let origin = foot + (foot - self.origin);
        // v' = 2(v・d)d - v
        let v = self.direction;
        let reflected = dir * (v.dot(dir) * 2.0) - v;
        Self {
            origin,
            direction: reflected,
        }
    }

    /// 与えた矩形の内側に収まる有限な線分。矩形と交わらなければ `None`。
    ///
    /// 描画側が表示範囲を渡して使う。
    ///
    /// # なぜ矩形から長さを決めるのか
    ///
    /// 「十分長い線分」を `1e9` のような定数で作ると、極端にズームしたときに
    /// スクリーン座標が破綻する。矩形の対角長から導けば、常に矩形を跨ぐのに
    /// 必要なぶんだけの長さで済む。
    ///
    /// 原点が矩形から遠い場合に備えて、**矩形の中心を直線上へ射影した位置**を
    /// 中心にして伸ばす。原点を中心にすると、遠い原点では矩形に届かない。
    #[must_use]
    pub fn clip_to(&self, rect: Aabb) -> Option<Line> {
        if rect.is_empty() {
            return None;
        }
        let span = rect.size().len() * CLIP_SPAN_FACTOR;
        if is_zero_len(span) {
            return None;
        }
        let center = self.param_at(rect.center());
        let a = self.point_at(center - span);
        let b = self.point_at(center + span);
        Line::new(a, b).clip_to(rect)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::tolerance::{eq_angle, eq_len};
    use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI, TAU};

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Aabb {
        Aabb::new(Point2::new(x0, y0), Point2::new(x1, y1))
    }

    #[test]
    fn through_two_points() {
        let x = Xline::through(Point2::ORIGIN, Point2::new(10.0, 0.0)).unwrap();
        assert!(x.direction.eq_tol(Vec2::X));
    }

    #[test]
    fn degenerate_inputs_are_rejected() {
        assert!(Xline::through(Point2::ORIGIN, Point2::ORIGIN).is_none());
        assert!(Xline::new(Point2::ORIGIN, Vec2::ZERO).is_none());
    }

    #[test]
    fn horizontal_and_vertical() {
        let h = Xline::horizontal(Point2::new(5.0, 7.0));
        assert!(h.direction.eq_tol(Vec2::X));
        assert!(eq_len(h.dist_to(Point2::new(1000.0, 7.0)), 0.0));

        let v = Xline::vertical(Point2::new(5.0, 7.0));
        assert!(v.direction.eq_tol(Vec2::Y));
        assert!(eq_len(v.dist_to(Point2::new(5.0, -1000.0)), 0.0));
    }

    #[test]
    fn at_angle_matches_the_requested_slope() {
        let x = Xline::at_angle(Point2::ORIGIN, FRAC_PI_4);
        assert!(eq_angle(x.angle(), FRAC_PI_4));
    }

    /// 方向は常に正規化されていること。
    #[test]
    fn direction_is_normalized() {
        let x = Xline::new(Point2::ORIGIN, Vec2::new(300.0, 400.0)).unwrap();
        assert!(eq_len(x.direction.len(), 1.0));
    }

    /// 無限直線なので、有限の範囲のはるか外の点でも距離が測れること。
    #[test]
    fn distance_works_far_outside_any_finite_extent() {
        let x = Xline::horizontal(Point2::ORIGIN);
        assert!(eq_len(x.dist_to(Point2::new(1_000_000.0, 3.0)), 3.0));
        assert!(eq_len(x.dist_to(Point2::new(-1_000_000.0, -3.0)), 3.0));
    }

    #[test]
    fn closest_point_projects_onto_the_line() {
        let x = Xline::horizontal(Point2::new(0.0, 5.0));
        let p = x.closest_point(Point2::new(42.0, 100.0));
        assert!(eq_len(p.x, 42.0) && eq_len(p.y, 5.0), "{p:?}");
    }

    #[test]
    fn clip_returns_a_segment_crossing_the_rect() {
        let x = Xline::horizontal(Point2::new(0.0, 5.0));
        let clipped = x.clip_to(rect(0.0, 0.0, 10.0, 10.0)).unwrap();
        assert!(eq_len(clipped.a.y, 5.0) && eq_len(clipped.b.y, 5.0));
        assert!(eq_len(clipped.a.x.min(clipped.b.x), 0.0));
        assert!(eq_len(clipped.a.x.max(clipped.b.x), 10.0));
    }

    #[test]
    fn clip_misses_a_rect_the_line_does_not_reach() {
        let x = Xline::horizontal(Point2::new(0.0, 100.0));
        assert!(x.clip_to(rect(0.0, 0.0, 10.0, 10.0)).is_none());
    }

    #[test]
    fn clip_of_empty_rect_is_none() {
        let x = Xline::horizontal(Point2::ORIGIN);
        assert!(x.clip_to(Aabb::EMPTY).is_none());
    }

    /// **矩形から長さを決めているかの検査。**
    ///
    /// 原点が矩形から遠く離れていても、また矩形の大きさが極端に違っても、
    /// クリップ結果は必ず直線上に乗り、矩形を跨ぐこと。
    /// 原点を中心に固定長で伸ばす実装だと、遠い原点で矩形に届かず落ちる。
    #[test]
    fn clip_works_for_very_different_rect_sizes_and_far_origins() {
        /// ごく小さい矩形の一辺。トレランスではなく「極端に狭い表示範囲」の意味。
        const TINY: f64 = 0.001;

        let x = Xline::at_angle(Point2::new(1_000_000.0, 1_000_000.0), FRAC_PI_4);

        for (x0, y0, x1, y1) in [
            (-1.0, -1.0, 1.0, 1.0),
            (-1e5, -1e5, 1e5, 1e5),
            (-TINY, -TINY, TINY, TINY),
        ] {
            let r = rect(x0, y0, x1, y1);
            let clipped = x.clip_to(r).expect("45 度線は原点付近を通るので交わるはず");
            // 両端が直線上にあること。
            assert!(
                eq_len(x.dist_to(clipped.a), 0.0),
                "a が直線上にない: {clipped:?}"
            );
            assert!(
                eq_len(x.dist_to(clipped.b), 0.0),
                "b が直線上にない: {clipped:?}"
            );
        }
    }

    #[test]
    fn translate_moves_the_origin_and_keeps_direction() {
        let x = Xline::horizontal(Point2::ORIGIN);
        let t = x.translated(Vec2::new(3.0, 4.0));
        assert!(t.origin.eq_tol(Point2::new(3.0, 4.0)));
        assert!(t.direction.eq_tol(Vec2::X));
    }

    #[test]
    fn rotate_by_full_turn_is_identity() {
        let x = Xline::at_angle(Point2::new(3.0, 4.0), FRAC_PI_4);
        let r = x.rotated(Point2::ORIGIN, TAU);
        assert!(r.origin.eq_tol(x.origin), "{:?}", r.origin);
        assert!(eq_angle(r.angle(), x.angle()));
    }

    #[test]
    fn rotate_turns_the_direction() {
        let x = Xline::horizontal(Point2::ORIGIN);
        let r = x.rotated(Point2::ORIGIN, FRAC_PI_2);
        assert!(eq_angle(r.angle(), FRAC_PI_2), "角度: {}", r.angle());
    }

    #[test]
    fn scale_by_one_is_identity() {
        let x = Xline::at_angle(Point2::new(3.0, 4.0), FRAC_PI_4);
        let s = x.scaled(Point2::ORIGIN, 1.0);
        assert!(s.origin.eq_tol(x.origin));
        assert!(eq_angle(s.angle(), x.angle()));
    }

    #[test]
    fn scale_moves_the_origin_but_not_the_direction() {
        let x = Xline::horizontal(Point2::new(2.0, 3.0));
        let s = x.scaled(Point2::ORIGIN, 2.0);
        assert!(s.origin.eq_tol(Point2::new(4.0, 6.0)));
        assert!(s.direction.eq_tol(Vec2::X), "向きは変わらない");
    }

    #[test]
    fn scale_by_zero_or_non_finite_is_a_no_op() {
        let x = Xline::horizontal(Point2::new(2.0, 3.0));
        for bad in [0.0, f64::NAN, f64::INFINITY] {
            assert_eq!(x.scaled(Point2::ORIGIN, bad), x, "倍率 {bad}");
        }
    }

    #[test]
    fn mirror_twice_about_the_same_axis_is_identity() {
        let axis = Line::new(Point2::ORIGIN, Point2::new(0.0, 10.0));
        let x = Xline::at_angle(Point2::new(3.0, 4.0), FRAC_PI_4);
        let back = x.mirrored(&axis).mirrored(&axis);
        assert!(back.origin.eq_tol(x.origin), "{:?}", back.origin);
        assert!(eq_angle(back.angle(), x.angle()));
    }

    /// Y 軸で鏡像すると x が反転し、傾きも反転すること。
    #[test]
    fn mirror_about_the_y_axis_flips_x_and_slope() {
        let axis = Line::new(Point2::ORIGIN, Point2::new(0.0, 10.0));
        let x = Xline::at_angle(Point2::new(3.0, 0.0), FRAC_PI_4);
        let m = x.mirrored(&axis);
        assert!(eq_len(m.origin.x, -3.0), "x が反転: {}", m.origin.x);
        // 45 度 → 135 度。
        assert!(
            eq_angle(m.angle(), PI - FRAC_PI_4) || eq_angle(m.angle(), -FRAC_PI_4 - PI),
            "傾き: {}",
            m.angle()
        );
    }

    #[test]
    fn mirror_with_degenerate_axis_is_a_no_op() {
        let axis = Line::new(Point2::ORIGIN, Point2::ORIGIN);
        let x = Xline::horizontal(Point2::new(1.0, 2.0));
        assert_eq!(x.mirrored(&axis), x);
    }

    #[test]
    fn param_and_point_round_trip() {
        let x = Xline::at_angle(Point2::new(1.0, 2.0), FRAC_PI_4);
        for t in [-1e6, -1.0, 0.0, 1.0, 1e6] {
            let p = x.point_at(t);
            assert!(eq_len(x.param_at(p), t), "t={t} で往復しない");
        }
    }
}
