//! 2 線分の角の処理（FILLET の角丸め、CHAMFER の面取り）。
//!
//! どちらも「2 本の線分を交点まで延長し、角の手前で切って何かを挟む」という
//! 同じ形をしている。共通部分をここにまとめる。
//!
//! # 対象は線分同士だけ
//!
//! 円弧や円を含む面取りは、接線の場合分けと退化ケースが一気に増える。
//! 本プロトタイプでは線分同士に限る（ユーザーと合意済み）。

use crate::geom::tolerance::{eq_len, is_zero_len};
use crate::geom::{Arc, Line, Point2, Vec2};

/// 2 線分の角を処理した結果。
///
/// 元の 2 線分をどう切り詰めるかと、間に挟む図形を返す。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CornerResult {
    /// 切り詰めた 1 本目。
    pub first: Line,
    /// 切り詰めた 2 本目。
    pub second: Line,
}

/// 2 線分の交点と、それぞれの「角に向かう向き」を求めた結果。
#[derive(Clone, Copy, Debug)]
struct Corner {
    /// 交点（延長線どうしの交点）。
    apex: Point2,
    /// 交点から見た 1 本目の残る側の向き（単位ベクトル）。
    away1: Vec2,
    /// 交点から見た 2 本目の残る側の向き（単位ベクトル）。
    away2: Vec2,
    /// 1 本目の「残る側」の端点。
    keep1: Point2,
    /// 2 本目の「残る側」の端点。
    keep2: Point2,
}

/// `apex` から `dir` の向きに、より遠いほうの端点を返す。
fn farther_along(apex: Point2, dir: Vec2, a: Point2, b: Point2) -> Point2 {
    if (b - apex).dot(dir) > (a - apex).dot(dir) {
        b
    } else {
        a
    }
}

/// 2 線分の角を解析する。
///
/// - `pick1` / `pick2` … それぞれの線分上でユーザーがクリックした位置。
///   **クリックした側が残る**（AutoCAD と同じ）。
///
/// 平行な 2 線分では交点が無いので `None`。
fn analyze(first: &Line, second: &Line, pick1: Point2, pick2: Point2) -> Option<Corner> {
    let d1 = first.dir()?;
    let d2 = second.dir()?;

    // 平行なら角ができない。
    let denom = d1.cross(d2);
    if is_zero_len(denom) {
        return None;
    }

    // 無限直線として交点を求める（線分の範囲外で交わる場合も面取りできる）。
    let diff = second.a - first.a;
    let t = diff.cross(d2) / denom;
    let apex = first.a + d1 * t;

    // 「クリックした側」を交点から見た向きで決める。
    //
    // 「クリックに近い端点を残す」という決め方だと、**交点自体が端点のとき**に
    // 交点を選んでしまい、向きが求まらなくなる（長さ 0 のベクトル）。
    // クリック位置そのものが交点のどちら側かを示しているので、そちらを使う。
    let away1 = (pick1 - apex).normalized()?;
    let away2 = (pick2 - apex).normalized()?;

    // その向きに最も遠い端点を「残る側」とする。
    let keep1 = farther_along(apex, away1, first.a, first.b);
    let keep2 = farther_along(apex, away2, second.a, second.b);

    Some(Corner {
        apex,
        away1,
        away2,
        keep1,
        keep2,
    })
}

/// 面取り（CHAMFER）。
///
/// 交点から `d1` / `d2` だけ戻った点どうしを結ぶ線分を返す。
///
/// # Errors 相当
///
/// 平行な 2 線分、距離が 0 以下、距離が線分より長い場合は `None`。
#[must_use]
pub fn chamfer(
    first: &Line,
    second: &Line,
    pick1: Point2,
    pick2: Point2,
    d1: f64,
    d2: f64,
) -> Option<(CornerResult, Line)> {
    if !d1.is_finite() || !d2.is_finite() || d1 <= 0.0 || d2 <= 0.0 {
        return None;
    }
    let c = analyze(first, second, pick1, pick2)?;

    // 交点から残る側へ d だけ戻った点。
    let p1 = c.apex + c.away1 * d1;
    let p2 = c.apex + c.away2 * d2;

    // 距離が線分の長さを超えていたら成立しない。
    if d1 > c.apex.dist(c.keep1) || d2 > c.apex.dist(c.keep2) {
        return None;
    }

    Some((
        CornerResult {
            first: Line::new(c.keep1, p1),
            second: Line::new(c.keep2, p2),
        },
        Line::new(p1, p2),
    ))
}

/// 角丸め（FILLET）。
///
/// 半径 `radius` の円弧を 2 線分に接するように挿入し、線分を接点まで切り詰める。
///
/// # 求め方
///
/// 2 直線のなす角を `θ` とすると、接点は交点から `radius / tan(θ/2)` だけ戻った位置にある。
/// 円弧の中心は角の二等分線上、交点から `radius / sin(θ/2)` の位置。
///
/// # Errors 相当
///
/// 平行な 2 線分、半径が 0 以下、接点が線分からはみ出す場合は `None`。
#[must_use]
pub fn fillet(
    first: &Line,
    second: &Line,
    pick1: Point2,
    pick2: Point2,
    radius: f64,
) -> Option<(CornerResult, Arc)> {
    if !radius.is_finite() || radius <= 0.0 {
        return None;
    }
    let c = analyze(first, second, pick1, pick2)?;

    // 2 つの向きのなす角。half は θ/2。
    let cos_theta = c.away1.dot(c.away2).clamp(-1.0, 1.0);
    let theta = cos_theta.acos();
    let half = theta / 2.0;
    let (sin_half, tan_half) = (half.sin(), half.tan());
    if is_zero_len(sin_half) || is_zero_len(tan_half) {
        // 2 線分が重なっている（角が 0 か π）。
        return None;
    }

    // 交点から接点までの距離。
    let setback = radius / tan_half;
    if setback > c.apex.dist(c.keep1) || setback > c.apex.dist(c.keep2) {
        return None;
    }

    let t1 = c.apex + c.away1 * setback;
    let t2 = c.apex + c.away2 * setback;

    // 中心は角の二等分線上。
    let bisector = (c.away1 + c.away2).normalized()?;
    let center = c.apex + bisector * (radius / sin_half);

    // 接点の角度から円弧を作る。反時計回りが正なので、向きを確かめて並べる。
    let a1 = (t1 - center).angle();
    let a2 = (t2 - center).angle();
    // 短いほうの弧を選ぶ。角丸めは常に劣弧になる。
    let arc = if shorter_sweep_is_ccw(a1, a2) {
        Arc::new(center, radius, a1, a2)
    } else {
        Arc::new(center, radius, a2, a1)
    };

    Some((
        CornerResult {
            first: Line::new(c.keep1, t1),
            second: Line::new(c.keep2, t2),
        },
        arc,
    ))
}

/// `from` から `to` へ反時計回りに進むほうが短いか。
///
/// 角丸めの弧は必ず劣弧（半周未満）になるので、これで向きを決められる。
fn shorter_sweep_is_ccw(from: f64, to: f64) -> bool {
    let ccw = crate::geom::tolerance::wrap_2pi(to - from);
    ccw <= std::f64::consts::PI
}

/// 2 線分の交点が求まるか（平行でないか）だけを調べる。
#[must_use]
pub fn has_corner(first: &Line, second: &Line) -> bool {
    match (first.dir(), second.dir()) {
        (Some(d1), Some(d2)) => !is_zero_len(d1.cross(d2)),
        _ => false,
    }
}

/// 2 つの長さが等しいか（面取りの対称判定などに使う）。
#[must_use]
pub fn same_distance(a: f64, b: f64) -> bool {
    eq_len(a, b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::tolerance::{eq_angle, eq_len};
    use std::f64::consts::FRAC_PI_2;

    /// 原点で直交する 2 線分。1 本目は +X、2 本目は +Y へ伸びる。
    fn right_angle() -> (Line, Line) {
        (
            Line::new(Point2::new(10.0, 0.0), Point2::ORIGIN),
            Line::new(Point2::ORIGIN, Point2::new(0.0, 10.0)),
        )
    }

    #[test]
    fn parallel_lines_have_no_corner() {
        let a = Line::new(Point2::ORIGIN, Point2::new(10.0, 0.0));
        let b = Line::new(Point2::new(0.0, 5.0), Point2::new(10.0, 5.0));
        assert!(!has_corner(&a, &b));
        assert!(chamfer(&a, &b, Point2::ORIGIN, Point2::new(0.0, 5.0), 1.0, 1.0).is_none());
        assert!(fillet(&a, &b, Point2::ORIGIN, Point2::new(0.0, 5.0), 1.0).is_none());
    }

    #[test]
    fn right_angle_has_a_corner() {
        let (a, b) = right_angle();
        assert!(has_corner(&a, &b));
    }

    /// 面取りが交点から指定距離だけ戻った点を結ぶこと。
    #[test]
    fn chamfer_cuts_back_by_the_given_distances() {
        let (a, b) = right_angle();
        // 交点は原点。残す側はそれぞれ (10,0) と (0,10)。
        let (result, bridge) = chamfer(
            &a,
            &b,
            Point2::new(9.0, 0.0),
            Point2::new(0.0, 9.0),
            3.0,
            4.0,
        )
        .unwrap();

        assert!(
            result.first.b.eq_tol(Point2::new(3.0, 0.0)),
            "{:?}",
            result.first
        );
        assert!(
            result.second.b.eq_tol(Point2::new(0.0, 4.0)),
            "{:?}",
            result.second
        );
        assert!(bridge.a.eq_tol(Point2::new(3.0, 0.0)));
        assert!(bridge.b.eq_tol(Point2::new(0.0, 4.0)));
    }

    /// 残す側はクリックした位置で決まること。
    #[test]
    fn chamfer_keeps_the_side_that_was_clicked() {
        // 1 本目を原点をまたぐ形にして、どちら側をクリックするかで結果が変わることを見る。
        let a = Line::new(Point2::new(-10.0, 0.0), Point2::new(10.0, 0.0));
        let b = Line::new(Point2::ORIGIN, Point2::new(0.0, 10.0));

        let (plus, _) = chamfer(
            &a,
            &b,
            Point2::new(5.0, 0.0),
            Point2::new(0.0, 5.0),
            2.0,
            2.0,
        )
        .unwrap();
        assert!(plus.first.a.eq_tol(Point2::new(10.0, 0.0)), "+X 側が残る");

        let (minus, _) = chamfer(
            &a,
            &b,
            Point2::new(-5.0, 0.0),
            Point2::new(0.0, 5.0),
            2.0,
            2.0,
        )
        .unwrap();
        assert!(minus.first.a.eq_tol(Point2::new(-10.0, 0.0)), "-X 側が残る");
    }

    #[test]
    fn chamfer_rejects_invalid_distances() {
        let (a, b) = right_angle();
        let (p1, p2) = (Point2::new(9.0, 0.0), Point2::new(0.0, 9.0));
        for (d1, d2) in [(0.0, 1.0), (1.0, 0.0), (-1.0, 1.0), (f64::NAN, 1.0)] {
            assert!(chamfer(&a, &b, p1, p2, d1, d2).is_none(), "{d1} / {d2}");
        }
    }

    /// 線分より長い距離は成立しないこと。
    #[test]
    fn chamfer_rejects_distances_longer_than_the_lines() {
        let (a, b) = right_angle();
        let (p1, p2) = (Point2::new(9.0, 0.0), Point2::new(0.0, 9.0));
        assert!(chamfer(&a, &b, p1, p2, 20.0, 1.0).is_none());
        assert!(chamfer(&a, &b, p1, p2, 1.0, 20.0).is_none());
    }

    /// 直角の角丸めで、接点が交点から半径ぶん戻った位置に来ること。
    ///
    /// 直角なら θ/2 = 45° で tan = 1 なので、戻り量はちょうど半径に等しい。
    #[test]
    fn fillet_on_a_right_angle_sets_back_by_the_radius() {
        let (a, b) = right_angle();
        let (result, arc) =
            fillet(&a, &b, Point2::new(9.0, 0.0), Point2::new(0.0, 9.0), 3.0).unwrap();

        assert!(
            result.first.b.eq_tol(Point2::new(3.0, 0.0)),
            "{:?}",
            result.first
        );
        assert!(
            result.second.b.eq_tol(Point2::new(0.0, 3.0)),
            "{:?}",
            result.second
        );
        // 中心は (3,3)、半径 3。
        assert!(arc.center.eq_tol(Point2::new(3.0, 3.0)), "{:?}", arc.center);
        assert!(eq_len(arc.radius, 3.0));
    }

    /// 円弧が両方の接点を通ること。
    #[test]
    fn fillet_arc_passes_through_both_tangent_points() {
        let (a, b) = right_angle();
        let (result, arc) =
            fillet(&a, &b, Point2::new(9.0, 0.0), Point2::new(0.0, 9.0), 3.0).unwrap();

        assert!(eq_len(arc.center.dist(result.first.b), arc.radius));
        assert!(eq_len(arc.center.dist(result.second.b), arc.radius));
        // 接点は円弧の端点でもある。
        let ends = [arc.start_point(), arc.end_point()];
        assert!(
            ends.iter().any(|p| p.eq_tol(result.first.b)),
            "端点: {ends:?}"
        );
        assert!(ends.iter().any(|p| p.eq_tol(result.second.b)));
    }

    /// 角丸めの弧は劣弧（半周未満）になること。
    #[test]
    fn fillet_arc_is_the_minor_arc() {
        let (a, b) = right_angle();
        let (_, arc) = fillet(&a, &b, Point2::new(9.0, 0.0), Point2::new(0.0, 9.0), 3.0).unwrap();
        assert!(
            arc.sweep() <= std::f64::consts::PI,
            "掃引が半周を超えている: {}",
            arc.sweep()
        );
        // 直角なので弧は 90 度。
        assert!(eq_angle(arc.sweep(), FRAC_PI_2), "掃引: {}", arc.sweep());
    }

    #[test]
    fn fillet_rejects_invalid_radius() {
        let (a, b) = right_angle();
        let (p1, p2) = (Point2::new(9.0, 0.0), Point2::new(0.0, 9.0));
        for r in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(fillet(&a, &b, p1, p2, r).is_none(), "半径 {r}");
        }
    }

    /// 半径が大きすぎて接点が線分からはみ出す場合は成立しないこと。
    #[test]
    fn fillet_rejects_a_radius_that_does_not_fit() {
        let (a, b) = right_angle();
        let (p1, p2) = (Point2::new(9.0, 0.0), Point2::new(0.0, 9.0));
        assert!(fillet(&a, &b, p1, p2, 100.0).is_none());
    }

    /// 鋭角でも成立すること。
    #[test]
    fn fillet_works_on_an_acute_angle() {
        let a = Line::new(Point2::new(10.0, 0.0), Point2::ORIGIN);
        let b = Line::new(Point2::ORIGIN, Point2::new(10.0, 10.0));
        let (result, arc) =
            fillet(&a, &b, Point2::new(9.0, 0.0), Point2::new(9.0, 9.0), 1.0).unwrap();

        assert!(eq_len(arc.center.dist(result.first.b), arc.radius));
        assert!(eq_len(arc.center.dist(result.second.b), arc.radius));
        assert!(arc.sweep() <= std::f64::consts::PI);
    }

    /// 大きな座標でも成立すること。
    #[test]
    fn corner_operations_work_at_large_coordinates() {
        let o = Point2::new(1_000_000.0, 1_000_000.0);
        let a = Line::new(o + Vec2::new(100.0, 0.0), o);
        let b = Line::new(o, o + Vec2::new(0.0, 100.0));

        let (result, arc) = fillet(
            &a,
            &b,
            o + Vec2::new(90.0, 0.0),
            o + Vec2::new(0.0, 90.0),
            10.0,
        )
        .expect("大きな座標でも成立するはず");
        assert!(eq_len(arc.radius, 10.0));
        assert!(result.first.b.eq_tol(o + Vec2::new(10.0, 0.0)));
    }

    #[test]
    fn same_distance_uses_the_shared_tolerance() {
        assert!(same_distance(1.0, 1.0));
        assert!(!same_distance(1.0, 2.0));
    }
}
