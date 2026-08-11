//! アプリケーション本体。

use cad_core::geom::Point2;
use cad_core::Document;

use crate::render;
use crate::viewport::Viewport;

/// ymcad のアプリケーション状態。
pub struct CadApp {
    /// 図面。変更は必ず `Document::apply` / `undo` / `redo` 経由で行う。
    doc: Document,
    /// モデル空間とスクリーン空間の対応。
    viewport: Viewport,
    /// 直近フレームのカーソル位置（モデル座標）。ステータスバー表示用。
    cursor_model: Option<Point2>,
    /// 読み込めた日本語フォントの情報。読み込めなかった場合は `None`。
    font_status: Option<String>,
}

impl CadApp {
    /// 初期状態のアプリを作る。
    #[must_use]
    pub fn new(font_status: Option<String>) -> Self {
        Self {
            doc: Document::new(),
            viewport: Viewport::default(),
            cursor_model: None,
            font_status,
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
            ui.monospace(format!("倍率 {:.4}", self.viewport.scale()));
            ui.separator();
            ui.monospace(format!("要素 {}", self.doc.entities().len()));

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

        // 図面領域は常に暗い背景で塗る（AutoCAD のモデル空間に倣う）。
        painter.rect_filled(response.rect, 0.0, ui.visuals().extreme_bg_color);

        render::draw_grid(&painter, &self.viewport, ui.visuals());
        render::draw_origin_marker(&painter, &self.viewport);

        self.cursor_model = response
            .hover_pos()
            .map(|p| self.viewport.screen_to_model(p));
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
