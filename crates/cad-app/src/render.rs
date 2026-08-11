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

/// 指定した刻みで縦横のグリッド線を引く。
fn draw_grid_lines(
    painter: &egui::Painter,
    vp: &Viewport,
    step: f64,
    color: egui::Color32,
    width: f32,
) {
    if !step.is_finite() || step <= 0.0 {
        return;
    }
    let rect = vp.rect();
    let view = vp.visible_model_rect();
    let stroke = egui::Stroke::new(width, color);

    // 垂直線（モデル空間の一定 x）
    let mut i = (view.min.x / step).ceil();
    let mut drawn = 0;
    while i * step <= view.max.x && drawn < MAX_GRID_LINES {
        let x = vp.model_to_screen(Point2::new(i * step, view.min.y)).x;
        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            stroke,
        );
        i += 1.0;
        drawn += 1;
    }

    // 水平線（モデル空間の一定 y）
    let mut j = (view.min.y / step).ceil();
    let mut drawn = 0;
    while j * step <= view.max.y && drawn < MAX_GRID_LINES {
        let y = vp.model_to_screen(Point2::new(view.min.x, j * step)).y;
        painter.line_segment(
            [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
            stroke,
        );
        j += 1.0;
        drawn += 1;
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
    fn nice_step_rejects_invalid_input() {
        assert!((nice_step(0.0) - 1.0).abs() < 1e-12);
        assert!((nice_step(-5.0) - 1.0).abs() < 1e-12);
        assert!((nice_step(f64::NAN) - 1.0).abs() < 1e-12);
        assert!((nice_step(f64::INFINITY) - 1.0).abs() < 1e-12);
    }
}
