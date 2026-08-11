//! 図面の描画。
//!
//! ここでは `&Document` と [`Viewport`] を読むだけで、状態を変更しない。
//! モデル座標からスクリーン座標への変換は必ず [`Viewport`] を経由すること
//! （このモジュールに `as f32` を書かない）。

use crate::viewport::Viewport;
use cad_core::geom::Point2;

/// 細グリッドの目標間隔 [px]。この値に最も近い 1/2/5 系列の刻みを選ぶ。
const MINOR_GRID_TARGET_PX: f32 = 12.0;
/// 太グリッドは細グリッドの何倍か。
const MAJOR_GRID_RATIO: f64 = 10.0;
/// これより密になった細グリッドは描かない（潰れて面になるため）。
const MIN_GRID_SPACING_PX: f32 = 4.0;
/// 1 方向あたりに描くグリッド線の上限。極端なズームで描画が止まるのを防ぐ。
const MAX_GRID_LINES: usize = 2000;

/// 原点マーカーの腕の長さ [px]。
const ORIGIN_MARKER_PX: f32 = 14.0;

/// `raw` 以上で最も小さい 1/2/5 × 10^k の値を返す。
///
/// グリッド間隔をズームに応じて段階的に変えるために使う。
#[must_use]
pub fn nice_step(raw: f64) -> f64 {
    if !raw.is_finite() || raw <= 0.0 {
        return 1.0;
    }
    let exp = raw.log10().floor();
    let base = 10f64.powf(exp);
    let mantissa = raw / base;
    let mult = if mantissa <= 1.0 {
        1.0
    } else if mantissa <= 2.0 {
        2.0
    } else if mantissa <= 5.0 {
        5.0
    } else {
        10.0
    };
    mult * base
}

/// グリッドと座標軸を描く。
pub fn draw_grid(painter: &egui::Painter, vp: &Viewport, visuals: &egui::Visuals) {
    let rect = vp.rect();
    let view = vp.visible_model_rect();
    if view.is_empty() {
        return;
    }

    let minor = nice_step(vp.px_to_model_len(MINOR_GRID_TARGET_PX));
    let major = minor * MAJOR_GRID_RATIO;

    let bg = visuals.extreme_bg_color;
    let minor_color = blend(bg, visuals.text_color(), 0.10);
    let major_color = blend(bg, visuals.text_color(), 0.22);
    let axis_color = blend(bg, visuals.text_color(), 0.45);

    // 細グリッドは潰れる手前で打ち切る。太グリッドは常に描く。
    if vp.model_len_to_px(minor) >= MIN_GRID_SPACING_PX {
        draw_grid_lines(painter, vp, minor, minor_color, 1.0);
    }
    draw_grid_lines(painter, vp, major, major_color, 1.0);

    // 座標軸（モデル座標の x = 0 / y = 0）。
    let origin = vp.model_to_screen(Point2::ORIGIN);
    if rect.x_range().contains(origin.x) {
        painter.line_segment(
            [
                egui::pos2(origin.x, rect.top()),
                egui::pos2(origin.x, rect.bottom()),
            ],
            egui::Stroke::new(1.0, axis_color),
        );
    }
    if rect.y_range().contains(origin.y) {
        painter.line_segment(
            [
                egui::pos2(rect.left(), origin.y),
                egui::pos2(rect.right(), origin.y),
            ],
            egui::Stroke::new(1.0, axis_color),
        );
    }
}

/// `min`..=`max` の範囲を `step` 刻みで走るモデル座標を返す。
///
/// 本数は [`MAX_GRID_LINES`] で頭打ちにする。極端なズームや不正な刻みで
/// 描画が停止しないことを保証するのが目的で、この上限があるおかげで
/// グリッド描画のコストはズーム倍率によらず一定に抑えられる。
///
/// 添字を f64 で持って毎回 `i * step` を計算するのは、`x += step` の累積加算だと
/// 誤差が溜まって格子が歪むため。
fn grid_coords(min: f64, max: f64, step: f64) -> impl Iterator<Item = f64> {
    let valid = step.is_finite() && step > 0.0 && min.is_finite() && max.is_finite() && min <= max;
    let first = if valid { (min / step).ceil() } else { 0.0 };
    // 本数の見積もり。f64 → 整数のキャストを避けるため、上限まで数え上げて決める。
    // 上限が 2000 本なので数え上げのコストは無視できる。
    let count = if valid {
        let span = (max - min) / step;
        let mut n = 0u32;
        while f64::from(n) <= span && (n as usize) < MAX_GRID_LINES {
            n += 1;
        }
        n
    } else {
        0
    };

    (0..count)
        .map(move |k| (first + f64::from(k)) * step)
        .take_while(move |v| *v <= max)
}

/// 指定した刻みで縦横のグリッド線を引く。
fn draw_grid_lines(
    painter: &egui::Painter,
    vp: &Viewport,
    step: f64,
    color: egui::Color32,
    width: f32,
) {
    let rect = vp.rect();
    let view = vp.visible_model_rect();
    let stroke = egui::Stroke::new(width, color);

    // 垂直線（モデル空間の一定 x）
    for x in grid_coords(view.min.x, view.max.x, step) {
        let sx = vp.model_to_screen(Point2::new(x, view.min.y)).x;
        painter.line_segment(
            [egui::pos2(sx, rect.top()), egui::pos2(sx, rect.bottom())],
            stroke,
        );
    }

    // 水平線（モデル空間の一定 y）
    for y in grid_coords(view.min.y, view.max.y, step) {
        let sy = vp.model_to_screen(Point2::new(view.min.x, y)).y;
        painter.line_segment(
            [egui::pos2(rect.left(), sy), egui::pos2(rect.right(), sy)],
            stroke,
        );
    }
}

/// 原点マーカーを描く。
pub fn draw_origin_marker(painter: &egui::Painter, vp: &Viewport) {
    let o = vp.model_to_screen(Point2::ORIGIN);
    if !vp.rect().expand(ORIGIN_MARKER_PX).contains(o) {
        return;
    }

    // AutoCAD の UCS アイコンに倣い、X 軸を赤・Y 軸を緑にする。
    let x_axis = egui::Color32::from_rgb(0xd0, 0x45, 0x3c);
    let y_axis = egui::Color32::from_rgb(0x4c, 0xaf, 0x50);

    painter.line_segment(
        [o, egui::pos2(o.x + ORIGIN_MARKER_PX, o.y)],
        egui::Stroke::new(1.6, x_axis),
    );
    painter.line_segment(
        [o, egui::pos2(o.x, o.y - ORIGIN_MARKER_PX)],
        egui::Stroke::new(1.6, y_axis),
    );
    painter.circle_stroke(o, 3.0, egui::Stroke::new(1.2, x_axis));
}

/// 2 色を `t` の割合で混ぜる。グリッドの濃淡をテーマに追従させるために使う。
fn blend(from: egui::Color32, to: egui::Color32, t: f32) -> egui::Color32 {
    egui::Color32::from_rgb(
        lerp_u8(from.r(), to.r(), t),
        lerp_u8(from.g(), to.g(), t),
        lerp_u8(from.b(), to.b(), t),
    )
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "0..=255 に収まる色成分の補間。範囲は clamp で保証している"
)]
fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    let a = f32::from(a);
    let b = f32::from(b);
    (a + (b - a) * t.clamp(0.0, 1.0)).round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nice_step_returns_1_2_5_series() {
        assert!((nice_step(1.0) - 1.0).abs() < 1e-12);
        assert!((nice_step(1.5) - 2.0).abs() < 1e-12);
        assert!((nice_step(3.0) - 5.0).abs() < 1e-12);
        assert!((nice_step(7.0) - 10.0).abs() < 1e-12);
        assert!((nice_step(0.03) - 0.05).abs() < 1e-12);
        assert!((nice_step(23_000.0) - 50_000.0).abs() < 1e-6);
    }

    /// 返り値は必ず入力以上（グリッドが目標間隔より細かくならない）。
    #[test]
    fn nice_step_is_never_smaller_than_input() {
        let mut x = 1e-9;
        while x < 1e9 {
            let s = nice_step(x);
            assert!(s >= x, "nice_step({x:e}) = {s:e} が入力より小さい");
            x *= 1.37;
        }
    }

    /// 全ズーム域で 1/2/5 × 10^k の形を保つこと。
    #[test]
    fn nice_step_keeps_mantissa_in_series() {
        for exp in -9..=9 {
            for m in [1.0, 1.3, 2.7, 4.9, 6.1, 9.9] {
                let s = nice_step(m * 10f64.powi(exp));
                let mantissa = s / 10f64.powf(s.log10().floor());
                let ok = [1.0, 2.0, 5.0]
                    .iter()
                    .any(|v: &f64| (mantissa - v).abs() < 1e-9);
                assert!(ok, "nice_step の仮数 {mantissa} が 1/2/5 系列でない");
            }
        }
    }

    #[test]
    fn grid_coords_covers_range_and_is_aligned() {
        let v: Vec<f64> = grid_coords(-3.0, 7.0, 2.0).collect();
        assert_eq!(v, vec![-2.0, 0.0, 2.0, 4.0, 6.0]);
        // すべて step の整数倍であること（格子が歪んでいない）。
        for x in v {
            assert!((x / 2.0).fract().abs() < 1e-12);
        }
    }

    #[test]
    fn grid_coords_handles_empty_and_invalid() {
        assert_eq!(grid_coords(5.0, 1.0, 1.0).count(), 0, "min > max");
        assert_eq!(grid_coords(0.0, 10.0, 0.0).count(), 0, "step = 0");
        assert_eq!(grid_coords(0.0, 10.0, -1.0).count(), 0, "step < 0");
        assert_eq!(grid_coords(0.0, 10.0, f64::NAN).count(), 0, "step = NaN");
        assert_eq!(grid_coords(f64::NEG_INFINITY, 10.0, 1.0).count(), 0);
    }

    /// 本数は必ず上限で頭打ちになること。これが無いと極端なズームで描画が停止する。
    #[test]
    fn grid_coords_is_bounded() {
        let v: Vec<f64> = grid_coords(0.0, 1e12, 1.0).collect();
        assert_eq!(v.len(), MAX_GRID_LINES);
    }

    /// ズーム倍率 1e-6〜1e6、原点近傍と 1e6 の両方で、
    /// グリッド線の本数が実用的な範囲に収まること（Phase 2 の受け入れ基準）。
    #[test]
    fn grid_line_count_stays_sane_across_zoom_range() {
        // 800x600 の画面を想定。
        for exp in -6..=6 {
            let scale = 10f64.powi(exp);
            let view_w = 800.0 / scale;
            let minor = nice_step(12.0 / scale);
            let major = minor * MAJOR_GRID_RATIO;

            for origin in [0.0, 1e6, -1e6] {
                let n_minor = grid_coords(origin, origin + view_w, minor).count();
                let n_major = grid_coords(origin, origin + view_w, major).count();

                assert!(
                    n_minor <= MAX_GRID_LINES && n_major <= MAX_GRID_LINES,
                    "scale=1e{exp} origin={origin:e}: 上限を超えた (minor={n_minor}, major={n_major})"
                );
                // 目標間隔 12px なので 800px の画面には 70 本前後が妥当。
                assert!(
                    n_minor <= 200,
                    "scale=1e{exp} origin={origin:e}: 細グリッドが多すぎる ({n_minor} 本)"
                );
            }
        }
    }

    #[test]
    fn nice_step_rejects_invalid_input() {
        assert!((nice_step(0.0) - 1.0).abs() < 1e-12);
        assert!((nice_step(-5.0) - 1.0).abs() < 1e-12);
        assert!((nice_step(f64::NAN) - 1.0).abs() < 1e-12);
        assert!((nice_step(f64::INFINITY) - 1.0).abs() < 1e-12);
    }
}
