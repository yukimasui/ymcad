//! アプリケーション本体。

use std::time::{Duration, Instant};

use cad_core::geom::{Aabb, Point2};
use cad_core::Document;

use crate::input::{self, KeySequence, ViewAction};
use crate::render;
use crate::viewport::Viewport;

/// ZOOM ALL で使う既定の図面範囲（A3 横 420 × 297 mm）。
///
/// AutoCAD の ZOOM ALL は図面限界と図形範囲の広い方に合わせる。
/// 図面限界は本来 DXF の `$LIMMIN` / `$LIMMAX` に対応するドキュメントの属性なので、
/// Phase 6 で `Document` へ移して DXF と入出力する。それまでは定数で代用する。
fn default_drawing_limits() -> Aabb {
    Aabb::new(Point2::ORIGIN, Point2::new(420.0, 297.0))
}

/// ZOOM 時に取る余白の割合。
const FIT_MARGIN: f64 = 0.05;

/// 直近の描画時間を保持して平均と最大を出す。
///
/// 「10,000 要素で 60fps」という性能目標に対する実測値を画面に出すために使う。
/// egui は必要なときだけ再描画するので、フレーム間隔ではなく
/// **こちらが描画に費やした時間**を測る。60fps の予算は 16.6ms。
#[derive(Debug)]
struct DrawTimer {
    samples: [Duration; Self::WINDOW],
    next: usize,
    filled: usize,
}

impl DrawTimer {
    const WINDOW: usize = 60;

    fn new() -> Self {
        Self {
            samples: [Duration::ZERO; Self::WINDOW],
            next: 0,
            filled: 0,
        }
    }

    fn push(&mut self, d: Duration) {
        self.samples[self.next] = d;
        self.next = (self.next + 1) % Self::WINDOW;
        self.filled = (self.filled + 1).min(Self::WINDOW);
    }

    /// (平均, 最大) をミリ秒で返す。
    fn stats_ms(&self) -> (f64, f64) {
        if self.filled == 0 {
            return (0.0, 0.0);
        }
        let used = &self.samples[..self.filled];
        let sum: Duration = used.iter().sum();
        let max = used.iter().max().copied().unwrap_or_default();
        let avg = sum.as_secs_f64() * 1000.0 / f64::from(u32::try_from(self.filled).unwrap_or(1));
        (avg, max.as_secs_f64() * 1000.0)
    }
}

/// ymcad のアプリケーション状態。
pub struct CadApp {
    /// 図面。変更は必ず `Document::apply` / `undo` / `redo` 経由で行う。
    doc: Document,
    /// モデル空間とスクリーン空間の対応。
    viewport: Viewport,
    /// 2 段キー入力の途中状態。
    keys: KeySequence,
    /// 直近フレームのカーソル位置（モデル座標）。ステータスバー表示用。
    cursor_model: Option<Point2>,
    /// 読み込めた日本語フォントの情報。読み込めなかった場合は `None`。
    font_status: Option<String>,
    /// 描画時間の実測。
    draw_timer: DrawTimer,
    /// 起動直後に一度だけ図面範囲へフィットさせるためのフラグ。
    initialized: bool,
}

impl CadApp {
    /// 初期状態のアプリを作る。
    #[must_use]
    pub fn new(font_status: Option<String>) -> Self {
        Self {
            doc: Document::new(),
            viewport: Viewport::default(),
            keys: KeySequence::default(),
            cursor_model: None,
            font_status,
            draw_timer: DrawTimer::new(),
            initialized: false,
        }
    }

    /// ZOOM EXTENTS の対象範囲。図面が空なら既定の図面範囲を使う。
    fn extents(&self) -> Aabb {
        let b = self.doc.bbox();
        if b.is_empty() {
            default_drawing_limits()
        } else {
            b
        }
    }

    /// ZOOM ALL の対象範囲。図面限界と図形範囲の広い方。
    fn all_bounds(&self) -> Aabb {
        default_drawing_limits().union(self.doc.bbox())
    }

    fn apply_view_action(&mut self, action: ViewAction) {
        match action {
            ViewAction::Pan(delta) => self.viewport.pan_px(delta),
            ViewAction::ZoomAt { anchor, factor } => self.viewport.zoom_about(anchor, factor),
            ViewAction::ZoomExtents => {
                let b = self.extents();
                self.viewport.zoom_to_fit(b, FIT_MARGIN);
            }
            ViewAction::ZoomAll => {
                let b = self.all_bounds();
                self.viewport.zoom_to_fit(b, FIT_MARGIN);
            }
        }
    }

    fn status_bar(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            // 座標は小数点以下 4 桁で表示する。
            match self.cursor_model {
                Some(p) => ui.monospace(format!("X {:>14.4}   Y {:>14.4}", p.x, p.y)),
                None => ui.monospace(format!("X {:>14}   Y {:>14}", "-", "-")),
            };
            ui.separator();
            ui.monospace(format!("倍率 {:.6}", self.viewport.scale()));
            ui.separator();
            ui.monospace(format!("要素 {}", self.doc.entities().len()));
            ui.separator();

            // 60fps の予算は 16.6ms。実測がそれを大きく下回っていることを見せる。
            let (avg, max) = self.draw_timer.stats_ms();
            ui.monospace(format!("描画 平均{avg:.2}ms 最大{max:.2}ms"));

            if let Some(prompt) = self.keys.prompt() {
                ui.separator();
                ui.colored_label(egui::Color32::from_rgb(0xff, 0xc1, 0x07), prompt);
            }

            if self.font_status.is_none() {
                ui.separator();
                ui.colored_label(
                    egui::Color32::from_rgb(0xff, 0x70, 0x43),
                    "日本語フォント未検出",
                );
            }
        });
    }

    fn canvas(&mut self, ui: &mut egui::Ui) {
        let (response, painter) =
            ui.allocate_painter(ui.available_size(), egui::Sense::click_and_drag());
        self.viewport.set_rect(response.rect);

        // 初回だけ図面範囲へ合わせる。以降はユーザーの操作に任せる。
        if !self.initialized && response.rect.width() > 0.0 {
            let b = self.all_bounds();
            self.viewport.zoom_to_fit(b, FIT_MARGIN);
            self.initialized = true;
        }

        for action in input::collect_view_actions(&response, ui, &self.viewport, &mut self.keys) {
            self.apply_view_action(action);
        }

        let started = Instant::now();

        // 図面領域は常に暗い背景で塗る（AutoCAD のモデル空間に倣う）。
        painter.rect_filled(response.rect, 0.0, ui.visuals().extreme_bg_color);
        render::draw_grid(&painter, &self.viewport, ui.visuals());
        render::draw_origin_marker(&painter, &self.viewport);

        self.draw_timer.push(started.elapsed());

        self.cursor_model = response
            .hover_pos()
            .map(|p| self.viewport.screen_to_model(p));

        // パン中はカーソルを掴んだ形にする。
        if response.dragged_by(egui::PointerButton::Middle) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        }
    }
}

impl eframe::App for CadApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // ステータスバーはカーソル座標を出すので、キャンバスより先に配置しても
        // 表示するのは 1 フレーム前の値になる。目視では差が分からない。
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));

        egui::CentralPanel::no_frame().show(ui, |ui| self.canvas(ui));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draw_timer_reports_zero_when_empty() {
        let t = DrawTimer::new();
        assert_eq!(t.stats_ms(), (0.0, 0.0));
    }

    #[test]
    fn draw_timer_averages_and_takes_max() {
        let mut t = DrawTimer::new();
        t.push(Duration::from_micros(1000)); // 1.0ms
        t.push(Duration::from_micros(3000)); // 3.0ms
        let (avg, max) = t.stats_ms();
        assert!((avg - 2.0).abs() < 1e-9, "平均は 2.0ms のはず: {avg}");
        assert!((max - 3.0).abs() < 1e-9, "最大は 3.0ms のはず: {max}");
    }

    /// 窓を越えても古いサンプルで壊れないこと。
    #[test]
    fn draw_timer_wraps_around() {
        let mut t = DrawTimer::new();
        for _ in 0..(DrawTimer::WINDOW * 3) {
            t.push(Duration::from_micros(500));
        }
        let (avg, max) = t.stats_ms();
        assert!((avg - 0.5).abs() < 1e-9);
        assert!((max - 0.5).abs() < 1e-9);
    }

    /// 既定の図面範囲は空でないこと（ZOOM ALL が無反応にならない）。
    #[test]
    fn default_limits_are_not_empty() {
        let b = default_drawing_limits();
        assert!(!b.is_empty());
        assert!(b.width() > 0.0 && b.height() > 0.0);
    }
}
