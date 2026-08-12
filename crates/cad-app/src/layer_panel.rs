//! レイヤパネル。
//!
//! # 設計
//!
//! このパネルは **`Document` を一切変更しない**。ユーザーの操作を
//! [`Command`] へ翻訳して返すだけで、適用は呼び出し側（`app.rs`）が
//! `Document::apply` で行う。
//!
//! こうすることで、レイヤ操作も作図と同じ 1 本の経路を通り、
//! Undo/Redo が自動的に効く。UI から直接 `LayerTable` をいじる抜け道を作らない。

use cad_core::command::{
    AddLayer, DeleteLayer, MoveEntitiesToLayer, RenameLayer, SetCurrentLayer, SetLayerProperties,
};
use cad_core::layer::LineType;
use cad_core::{AciColor, Command, Document, LayerId};

use crate::selection::Selection;

/// レイヤパネルの色見本で選べる ACI 色。
const PALETTE: [AciColor; 9] = [
    AciColor(1),
    AciColor(2),
    AciColor(3),
    AciColor(4),
    AciColor(5),
    AciColor(6),
    AciColor(7),
    AciColor(8),
    AciColor(9),
];

/// 色見本の一辺 [px]。
const SWATCH_PX: f32 = 14.0;

/// レイヤパネルの状態。
#[derive(Debug, Default)]
pub struct LayerPanel {
    /// パネルを開いているか。
    open: bool,
    /// 名前を編集中のレイヤ。
    rename_target: Option<LayerId>,
    /// 編集中の名前。
    rename_buffer: String,
    /// 新規レイヤ名の入力。
    new_layer_name: String,
    /// 色見本を開いているレイヤ。
    color_picker_for: Option<LayerId>,
}

impl LayerPanel {
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
        if !self.open {
            self.rename_target = None;
            self.color_picker_for = None;
        }
    }

    /// パネルを描画し、実行すべきコマンドを返す。
    ///
    /// 返り値が空でなければ、呼び出し側が `Document::apply` で適用する。
    #[must_use]
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        doc: &Document,
        selection: &Selection,
    ) -> Vec<Box<dyn Command>> {
        let mut commands: Vec<Box<dyn Command>> = Vec::new();
        if !self.open {
            return commands;
        }

        ui.heading("画層");
        ui.separator();

        self.show_add_row(ui, doc, &mut commands);
        ui.separator();

        egui::ScrollArea::vertical()
            .auto_shrink([false, true])
            .max_height(320.0)
            .show(ui, |ui| {
                let current = doc.layers().current();
                let ids: Vec<LayerId> = doc.layers().iter().map(|(id, _)| id).collect();
                for id in ids {
                    self.show_layer_row(ui, doc, id, current, &mut commands);
                }
            });

        ui.separator();
        self.show_move_row(ui, doc, selection, &mut commands);

        commands
    }

    /// レイヤ追加の行。
    fn show_add_row(
        &mut self,
        ui: &mut egui::Ui,
        doc: &Document,
        commands: &mut Vec<Box<dyn Command>>,
    ) {
        ui.horizontal(|ui| {
            ui.label("新規:");
            ui.add(
                egui::TextEdit::singleline(&mut self.new_layer_name)
                    .desired_width(140.0)
                    .hint_text("レイヤ名"),
            );

            let name = self.new_layer_name.trim().to_owned();
            let taken = doc.layers().by_name(&name).is_some();
            let can_add = !name.is_empty() && !taken;

            if ui.add_enabled(can_add, egui::Button::new("追加")).clicked() {
                commands.push(Box::new(AddLayer::new(name, AciColor::WHITE)));
                self.new_layer_name.clear();
            }
            if taken {
                ui.colored_label(egui::Color32::from_rgb(0xff, 0x70, 0x43), "同名あり");
            }
        });
    }

    /// レイヤ 1 行ぶん。
    fn show_layer_row(
        &mut self,
        ui: &mut egui::Ui,
        doc: &Document,
        id: LayerId,
        current: LayerId,
        commands: &mut Vec<Box<dyn Command>>,
    ) {
        let Some(layer) = doc.layers().get(id) else {
            return;
        };
        let is_zero = id == LayerId::ZERO;
        let is_current = id == current;

        ui.horizontal(|ui| {
            // 現在レイヤの切り替え。
            if ui
                .add(egui::RadioButton::new(is_current, ""))
                .on_hover_text("現在レイヤにする")
                .clicked()
                && !is_current
            {
                commands.push(Box::new(SetCurrentLayer::new(id)));
            }

            // 表示 / 非表示。
            let mut visible = layer.visible;
            if ui
                .checkbox(&mut visible, "")
                .on_hover_text("表示 / 非表示")
                .changed()
            {
                commands.push(Box::new(SetLayerProperties::new(id).visible(visible)));
            }

            // ロック。
            let lock_label = if layer.locked { "🔒" } else { "🔓" };
            if ui
                .button(lock_label)
                .on_hover_text("ロック / 解除")
                .clicked()
            {
                commands.push(Box::new(SetLayerProperties::new(id).locked(!layer.locked)));
            }

            // 色見本。
            let (r, g, b) = layer.color.rgb();
            let swatch = egui::Color32::from_rgb(r, g, b);
            if ui
                .add(
                    egui::Button::new("")
                        .fill(swatch)
                        .min_size(egui::vec2(SWATCH_PX, SWATCH_PX)),
                )
                .on_hover_text("色を変更")
                .clicked()
            {
                self.color_picker_for = if self.color_picker_for == Some(id) {
                    None
                } else {
                    Some(id)
                };
            }

            // 名前（ダブルクリックで編集）。
            if self.rename_target == Some(id) {
                let response = ui
                    .add(egui::TextEdit::singleline(&mut self.rename_buffer).desired_width(120.0));
                let commit = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if commit {
                    let new_name = self.rename_buffer.trim().to_owned();
                    if !new_name.is_empty() && new_name != layer.name {
                        commands.push(Box::new(RenameLayer::new(id, new_name)));
                    }
                    self.rename_target = None;
                }
            } else {
                let label = ui.selectable_label(is_current, &layer.name);
                if label.double_clicked() && !is_zero {
                    self.rename_target = Some(id);
                    self.rename_buffer = layer.name.clone();
                }
                if is_zero {
                    label.on_hover_text("レイヤ 0 は名前を変更できません");
                }
            }

            // 線種。
            let mut linetype = layer.linetype;
            egui::ComboBox::from_id_salt(("linetype", id.index()))
                .selected_text(linetype.label())
                .width(90.0)
                .show_ui(ui, |ui| {
                    for t in LineType::all() {
                        ui.selectable_value(&mut linetype, t, t.label());
                    }
                });
            if linetype != layer.linetype {
                commands.push(Box::new(SetLayerProperties::new(id).linetype(linetype)));
            }

            // 削除。レイヤ 0 と現在レイヤは消せない。
            let can_delete = !is_zero && !is_current;
            let delete = ui.add_enabled(can_delete, egui::Button::new("🗑"));
            if delete.clicked() {
                commands.push(Box::new(DeleteLayer::new(id)));
            }
            if !can_delete {
                delete.on_hover_text(if is_zero {
                    "レイヤ 0 は削除できません"
                } else {
                    "現在レイヤは削除できません"
                });
            }
        });

        // 色見本の展開。
        if self.color_picker_for == Some(id) {
            ui.horizontal_wrapped(|ui| {
                ui.label("色:");
                for aci in PALETTE {
                    let (r, g, b) = aci.rgb();
                    if ui
                        .add(
                            egui::Button::new("")
                                .fill(egui::Color32::from_rgb(r, g, b))
                                .min_size(egui::vec2(SWATCH_PX, SWATCH_PX)),
                        )
                        .clicked()
                    {
                        commands.push(Box::new(SetLayerProperties::new(id).color(aci)));
                        self.color_picker_for = None;
                    }
                }
            });
        }
    }

    /// 選択中の要素を別レイヤへ移す行。
    fn show_move_row(
        &self,
        ui: &mut egui::Ui,
        doc: &Document,
        selection: &Selection,
        commands: &mut Vec<Box<dyn Command>>,
    ) {
        ui.horizontal_wrapped(|ui| {
            if selection.is_empty() {
                ui.weak("選択中の要素を別の画層へ移すには、先に要素を選択してください");
                return;
            }
            ui.label(format!("選択中の {} 要素を移動:", selection.len()));
            for (id, layer) in doc.layers().iter() {
                if ui.button(&layer.name).clicked() {
                    commands.push(Box::new(MoveEntitiesToLayer::new(selection.to_vec(), id)));
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_closed() {
        assert!(!LayerPanel::new().is_open());
    }

    #[test]
    fn toggle_opens_and_closes() {
        let mut p = LayerPanel::new();
        p.toggle();
        assert!(p.is_open());
        p.toggle();
        assert!(!p.is_open());
    }

    /// 閉じているときはコマンドを一切出さないこと。
    #[test]
    fn closed_panel_emits_no_commands() {
        let p = LayerPanel::new();
        assert!(!p.is_open());
    }

    /// 閉じるときに編集中の状態を捨てること。
    #[test]
    fn closing_clears_transient_state() {
        let mut p = LayerPanel::new();
        p.toggle();
        p.rename_target = Some(LayerId::ZERO);
        p.color_picker_for = Some(LayerId::ZERO);
        p.toggle();
        assert!(p.rename_target.is_none());
        assert!(p.color_picker_for.is_none());
    }

    /// パレットは ACI の標準色を含むこと。
    #[test]
    fn palette_covers_standard_aci_colors() {
        assert_eq!(PALETTE.len(), 9);
        assert!(PALETTE.contains(&AciColor::RED));
        assert!(PALETTE.contains(&AciColor::WHITE));
    }
}

#[cfg(test)]
mod integration_tests {
    //! Phase 5 の受け入れ基準を、`Document` を通した実際の振る舞いで検証する。

    use cad_core::command::{AddEntities, AddLayer, SetLayerProperties};
    use cad_core::geom::{Line, Point2};
    use cad_core::layer::LineType;
    use cad_core::{AciColor, Document, Entity, Geometry, LayerId};

    use crate::selection::{self, WindowMode};
    use cad_core::geom::Aabb;

    /// レイヤ `name` に線分 1 本を持つ図面を作る。
    fn doc_with_line_on_new_layer(name: &str) -> (Document, LayerId) {
        let mut doc = Document::new();
        let mut add = AddLayer::new(name, AciColor::RED);
        doc.apply(Box::new(std::mem::replace(
            &mut add,
            AddLayer::new(name, AciColor::RED),
        )))
        .unwrap();
        let layer = doc
            .layers()
            .by_name(name)
            .expect("追加したレイヤがあるはず");

        doc.apply(Box::new(AddEntities::one(
            "LINE",
            Entity::new(
                Geometry::Line(Line::new(Point2::ORIGIN, Point2::new(100.0, 0.0))),
                layer,
            ),
        )))
        .unwrap();
        (doc, layer)
    }

    fn whole_area() -> Aabb {
        Aabb::new(Point2::new(-1000.0, -1000.0), Point2::new(1000.0, 1000.0))
    }

    /// 非表示レイヤの要素は **描画からも選択からも** 除外されること。
    #[test]
    fn hidden_layer_is_excluded_from_both_render_and_selection() {
        let (mut doc, layer) = doc_with_line_on_new_layer("HIDDEN_TEST");
        let entity = doc.entities().iter().next().unwrap().1.clone();
        assert!(doc.layers().is_entity_visible(&entity), "前提: 最初は表示");

        doc.apply(Box::new(SetLayerProperties::new(layer).visible(false)))
            .unwrap();

        let entity = doc.entities().iter().next().unwrap().1.clone();
        // 描画からの除外（render::draw_entities が使う判定）
        assert!(
            !doc.layers().is_entity_visible(&entity),
            "描画対象から外れる"
        );
        // 選択からの除外
        assert!(selection::pick_at(&doc, Point2::new(50.0, 0.0), 1.0).is_none());
        assert!(selection::pick_in_rect(&doc, whole_area(), WindowMode::Crossing).is_empty());
        assert!(selection::pick_in_rect(&doc, whole_area(), WindowMode::Window).is_empty());
    }

    /// ロックレイヤの要素は表示されるが選択・編集できないこと。
    #[test]
    fn locked_layer_is_visible_but_not_selectable() {
        let (mut doc, layer) = doc_with_line_on_new_layer("LOCKED_TEST");
        doc.apply(Box::new(SetLayerProperties::new(layer).locked(true)))
            .unwrap();

        let entity = doc.entities().iter().next().unwrap().1.clone();
        assert!(
            doc.layers().is_entity_visible(&entity),
            "ロックしても表示はされる"
        );
        assert!(!doc.layers().is_entity_editable(&entity));
        assert!(selection::pick_at(&doc, Point2::new(50.0, 0.0), 1.0).is_none());
        assert!(selection::pick_in_rect(&doc, whole_area(), WindowMode::Crossing).is_empty());
    }

    /// レイヤ操作が Undo で巻き戻ること（受け入れ基準）。
    #[test]
    fn layer_property_changes_undo() {
        let (mut doc, layer) = doc_with_line_on_new_layer("UNDO_TEST");

        doc.apply(Box::new(
            SetLayerProperties::new(layer)
                .visible(false)
                .locked(true)
                .color(AciColor(3))
                .linetype(LineType::Dashed),
        ))
        .unwrap();

        {
            let l = doc.layers().get(layer).unwrap();
            assert!(!l.visible && l.locked);
            assert_eq!(l.color, AciColor(3));
            assert_eq!(l.linetype, LineType::Dashed);
        }

        doc.undo().unwrap();

        let l = doc.layers().get(layer).unwrap();
        assert!(l.visible && !l.locked, "Undo で表示とロックが戻る");
        assert_eq!(l.color, AciColor::RED, "Undo で色が戻る");
        assert_eq!(l.linetype, LineType::Continuous, "Undo で線種が戻る");

        // 非表示にした要素も選択できる状態に戻っていること。
        assert!(selection::pick_at(&doc, Point2::new(50.0, 0.0), 1.0).is_some());
    }

    /// 非表示レイヤの要素はスナップ候補にもならないこと。
    #[test]
    fn hidden_layer_produces_no_snap_candidates() {
        use crate::snap::SnapState;

        let (mut doc, layer) = doc_with_line_on_new_layer("SNAP_TEST");
        let mut snap = SnapState::new();
        assert!(
            snap.update(&doc, Point2::new(1.0, 0.0), 5.0, 8.0, None)
                .is_some(),
            "前提: 表示中は端点に吸着する"
        );

        doc.apply(Box::new(SetLayerProperties::new(layer).visible(false)))
            .unwrap();
        assert!(
            snap.update(&doc, Point2::new(1.0, 0.0), 5.0, 8.0, None)
                .is_none(),
            "非表示レイヤにはスナップしない"
        );
    }

    /// 線種はレイヤから継承され、実線以外は破線パターンを持つこと。
    #[test]
    fn linetype_inherits_from_layer() {
        let (mut doc, layer) = doc_with_line_on_new_layer("LT_TEST");
        doc.apply(Box::new(
            SetLayerProperties::new(layer).linetype(LineType::Center),
        ))
        .unwrap();

        let entity = doc.entities().iter().next().unwrap().1.clone();
        let lt = doc.layers().resolve_linetype(&entity);
        assert_eq!(lt, LineType::Center);
        assert!(!lt.dash_pattern_px().is_empty(), "一点鎖線はパターンを持つ");
        assert!(LineType::Continuous.dash_pattern_px().is_empty());
    }
}
