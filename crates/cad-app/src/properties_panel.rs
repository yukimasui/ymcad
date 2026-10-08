//! プロパティパネル（Issue #31 段階 1: 表示とレイヤの変更）。
//!
//! 選んだ図形の種類ごとの値を出し、レイヤをドロップダウンで変える。AutoCAD の PROPERTIES
//! （Ctrl+1）にあたるが、クイックプロパティのようにカーソル横へ飛び出さず、レイヤ・
//! コンポーネントと同じ右側のパネルに置く（Issue #31 の「AutoCAD での不満と対策」）。
//!
//! # `Document` は変更しない
//!
//! レイヤパネルと同じく、[`Command`] を返すだけで適用は呼び出し側（ADR-0012）。
//!
//! # コマンド実行中は表示だけ
//!
//! 選択待ちを含めてコマンドを実行している間は、パネルのレイヤ変更を無効にして案内を出す。
//! 実行中のツールが覚えている選択や図形を、パネルが横から書き換えないため
//! （#37 の教訓。パネルの開閉は実行中のコマンドを中断しないので、開いたままコマンドを
//! 始められる）。
//!
//! 中身を決める部分は egui に依存しない純粋な関数として `properties.rs` にある。

use cad_core::command::MoveEntitiesToLayer;
use cad_core::{Command, Document};

use crate::properties::{
    self, kind_of, CommonLayer, Summary, SummaryCache, BUSY_NOTE, EMPTY_NOTE, MIXED_LAYER,
    MULTI_NOTE,
};
use crate::selection::Selection;

/// コマンド実行中の案内の色（ステータスバーの「コマンド実行中」と同じ琥珀色）。
pub const BUSY_COLOR: egui::Color32 = egui::Color32::from_rgb(0xff, 0xc1, 0x07);

/// 選択から外れた案内の色（コマンド実行中の琥珀色とは別にして、状態の案内と区別する）。
pub const DROP_NOTE_COLOR: egui::Color32 = egui::Color32::from_rgb(0x80, 0xcb, 0xc4);

/// 項目名の欄の幅 [px]。
const LABEL_WIDTH: f32 = 84.0;
/// 表の列と行の間隔 [px]。
const GRID_SPACING: [f32; 2] = [8.0, 4.0];

/// プロパティパネルの状態。
#[derive(Debug, Default)]
pub struct PropertiesPanel {
    /// パネルを開いているか。
    open: bool,
    /// 選択の要約。図面と選択の版番号をキーに作り直す。
    summary: SummaryCache,
}

impl PropertiesPanel {
    /// 初期状態（閉じている）。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 開いているか。
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// 開閉を切り替える。
    pub fn toggle(&mut self) {
        self.open = !self.open;
    }

    /// 選択の要約を作り直した回数（テスト用）。
    #[cfg(test)]
    pub fn summary_recomputed(&self) -> usize {
        self.summary.recomputed()
    }

    /// 要約のキャッシュを捨てる。図面が丸ごと入れ替わったとき（版番号が重なりうる）に呼ぶ。
    pub fn invalidate(&mut self) {
        self.summary.invalidate();
    }

    /// パネルを描画し、実行すべきコマンドを返す。
    ///
    /// `busy` … コマンド（選択待ちを含む）を実行中か。真の間は表示だけになる。
    /// `drop_note` … 図形をロック・非表示のレイヤへ移して選択から外れたときの案内
    /// （`Session::drop_note`）。選択が空の表示の上に出す。
    #[must_use]
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        doc: &Document,
        selection: &Selection,
        busy: bool,
        drop_note: Option<&str>,
    ) -> Vec<Box<dyn Command>> {
        let mut commands: Vec<Box<dyn Command>> = Vec::new();
        if !self.open {
            return commands;
        }

        ui.heading("プロパティ");
        ui.separator();
        if busy {
            ui.colored_label(BUSY_COLOR, BUSY_NOTE);
            ui.separator();
        }

        let summary = self.summary.get(doc, selection);
        let (Some(layer), true) = (summary.layer, summary.total > 0) else {
            if let Some(note) = drop_note {
                ui.colored_label(DROP_NOTE_COLOR, note);
                ui.separator();
            }
            ui.weak(EMPTY_NOTE);
            return commands;
        };

        egui::ScrollArea::vertical()
            .auto_shrink([false, true])
            .show(ui, |ui| {
                show_header(ui, summary);
                show_layer_row(ui, doc, selection, layer, busy, &mut commands);
                ui.separator();
                if summary.total == 1 {
                    show_items(ui, doc, selection);
                } else {
                    ui.weak(MULTI_NOTE);
                }
            });
        commands
    }
}

/// 見出し。1 つなら種類、複数なら個数と種類ごとの件数。
fn show_header(ui: &mut egui::Ui, summary: &Summary) {
    if let [(kind, _)] = summary.kinds.as_slice() {
        if summary.total == 1 {
            ui.strong(kind.label());
            return;
        }
    }
    ui.strong(format!("{} 個を選択", summary.total));
    ui.weak(summary.kinds_text());
}

/// レイヤのドロップダウン。選んだレイヤへ選択全部を移す（1 回の Undo で戻る）。
///
/// ロック・非表示のレイヤも選べる（印つき）。移した結果、選択は外れる。
fn show_layer_row(
    ui: &mut egui::Ui,
    doc: &Document,
    selection: &Selection,
    layer: CommonLayer,
    busy: bool,
    commands: &mut Vec<Box<dyn Command>>,
) {
    let current = match layer {
        CommonLayer::One(id) => doc
            .layers()
            .get(id)
            .map_or_else(|| "?".to_owned(), properties::layer_label),
        CommonLayer::Mixed => MIXED_LAYER.to_owned(),
    };
    // 項目の表と同じ幅の項目名の欄にそろえるため、同じ形の 2 列の表にする。
    egui::Grid::new("properties_layer_row")
        .num_columns(2)
        .min_col_width(LABEL_WIDTH)
        .spacing(GRID_SPACING)
        .show(ui, |ui| {
            ui.add(egui::Label::new("レイヤ").selectable(false));
            ui.add_enabled_ui(!busy, |ui| {
                egui::ComboBox::from_id_salt("properties_layer")
                    .selected_text(current)
                    .width(ui.available_width() - ui.spacing().item_spacing.x)
                    .show_ui(ui, |ui| {
                        for (id, l) in doc.layers().iter() {
                            let selected = layer == CommonLayer::One(id);
                            let clicked = ui
                                .selectable_label(selected, properties::layer_label(l))
                                .clicked();
                            // 今のレイヤを選び直しても何もしない（履歴に空の操作を積まない）。
                            if clicked && !selected {
                                commands.push(Box::new(MoveEntitiesToLayer::new(
                                    selection.to_vec(),
                                    id,
                                )));
                            }
                        }
                    });
            });
            ui.end_row();
        });
}

/// 1 つ選んだときの項目の一覧（段階 1 では表示だけ）。
fn show_items(ui: &mut egui::Ui, doc: &Document, selection: &Selection) {
    // 見出しの「1 つ」は図面に実在する数（`summarize`）なので、実在する最初の図形を取る。
    let Some(entity) = selection.iter().find_map(|id| doc.entities().get(id)) else {
        return;
    };
    egui::Grid::new(("properties_items", kind_of(&entity.geom) as u8))
        .num_columns(2)
        .min_col_width(LABEL_WIDTH)
        .spacing(GRID_SPACING)
        .show(ui, |ui| {
            for item in properties::items(&entity.geom, doc.definitions()) {
                ui.add(egui::Label::new(item.label).selectable(false));
                ui.add(
                    egui::Label::new(egui::RichText::new(item.value).monospace()).selectable(false),
                );
                ui.end_row();
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_closed_and_toggles() {
        let mut p = PropertiesPanel::new();
        assert!(!p.is_open());
        p.toggle();
        assert!(p.is_open());
        p.toggle();
        assert!(!p.is_open());
    }
}
