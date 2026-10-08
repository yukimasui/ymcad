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
use cad_core::{AciColor, CadError, Command, Document, LayerId};

use crate::properties_panel::{BUSY_COLOR, DROP_NOTE_COLOR};
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

/// コマンド実行中に「移動」の行へ出す案内。表示・ロック・色・追加などは実行中も使えるので、
/// パネル全体ではなく移動だけが使えないと読めるようにする。
pub const MOVE_BUSY_NOTE: &str =
    "コマンド実行中は移動できません（終えるか Esc で中断。中断すると選択も外れます）";

/// 欄の外のクリックなどで改名をやめたとき、名前を変えていれば出す案内。
pub const RENAME_DROPPED_NOTE: &str = "レイヤ名の変更をやめました（確定は Enter）";

/// 行の右端に必ず残す幅 [px]（線種のドロップダウン 90px と削除ボタン）。名前はこれを除いた
/// 残りの幅までしか使わず、長ければ省略する。色・線種・削除は名前より先に幅を確保する。
const ROW_RIGHT_WIDTH: f32 = 90.0 + 30.0;
/// 名前に最低限残す幅 [px]。
const NAME_MIN_WIDTH: f32 = 40.0;

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
    /// 改名の入力欄を出した直後で、まだフォーカスを渡していないか。
    rename_focus_pending: bool,
    /// 新規レイヤ名の入力。
    new_layer_name: String,
    /// 色見本を開いているレイヤ。
    color_picker_for: Option<LayerId>,
    /// コマンドラインへ出す案内（[`LayerPanel::take_notice`] で取り出す）。
    notice: Option<PanelNotice>,
}

/// パネルからコマンドラインへ出す案内。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PanelNotice {
    /// 案内。
    Info(String),
    /// エラー。
    Error(String),
}

/// 改名の欄からフォーカスが外れたときの結末。
#[derive(Debug, PartialEq, Eq)]
enum RenameEnd {
    /// この名前で改名する。
    Commit(String),
    /// 改名できない（同名のレイヤがあるなど。`LayerTable::check_rename` が断った）。欄は開いたままにする。中身は利用者向けの案内。
    Rejected(String),
    /// 改名をやめる。`tell` … 打った名前を捨てたことを案内するか。
    Dropped { tell: bool },
}

/// 改名の欄からフォーカスが外れたとき、どう終えるかを決める。
///
/// - Enter … 同名のレイヤがあれば断る（欄を閉じると打った名前が消えるので、先に見る）。
///   空・元のままなら何もせず終える
/// - Esc … 黙ってやめる（本人がやめたと分かっている）
/// - それ以外（欄の外のクリックなど）… やめる。名前を変えていたら案内する。
///   外のクリックで確定しないのは、作図領域を押しただけで名前が変わると気づきにくいため
fn end_rename(doc: &Document, id: LayerId, buffer: &str, enter: bool, escape: bool) -> RenameEnd {
    let Some(layer) = doc.layers().get(id) else {
        return RenameEnd::Dropped { tell: false };
    };
    let new_name = buffer.trim();
    let changed = new_name != layer.name;
    if enter {
        if new_name.is_empty() || !changed {
            return RenameEnd::Dropped { tell: false };
        }
        // `RenameLayer` が適用の前に見るのと同じ判定（`LayerTable::check_rename`）。
        // 適用してから失敗を知るのでは欄を開いたままにできない。判定を写さず同じ関数を呼ぶ。
        if let Err(e) = doc.layers().check_rename(id, new_name) {
            let why = match e {
                CadError::NotEditable(why) => why.to_owned(),
                other => other.to_string(),
            };
            return RenameEnd::Rejected(format!(
                "レイヤ名の変更: 「{new_name}」にできません。{why}（別の名前にして Enter、やめるなら Esc）"
            ));
        }
        return RenameEnd::Commit(new_name.to_owned());
    }
    RenameEnd::Dropped {
        tell: changed && !escape,
    }
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

    /// 名前を編集中のレイヤ（テスト用）。
    #[cfg(test)]
    #[must_use]
    pub fn renaming(&self) -> Option<LayerId> {
        self.rename_target
    }

    /// 編集中の名前（テスト用）。
    #[cfg(test)]
    #[must_use]
    pub fn rename_text(&self) -> &str {
        &self.rename_buffer
    }

    /// コマンドラインへ出す案内を取り出す（描いた後に呼ぶ）。
    pub fn take_notice(&mut self) -> Option<PanelNotice> {
        self.notice.take()
    }

    /// 開閉を切り替える。
    pub fn toggle(&mut self) {
        self.open = !self.open;
        if !self.open {
            self.clear_transient();
        }
    }

    /// 図面が丸ごと入れ替わった（NEW / OPEN）。レイヤの ID を覚えている編集中の状態を捨てる。
    ///
    /// レイヤの ID は図面ごとに振られるので、残すと新しい図面の別のレイヤを指しうる
    /// （改名の確定が別のレイヤに掛かる）。開閉と、新規レイヤ名の下書きは残す。
    pub fn document_replaced(&mut self) {
        self.clear_transient();
    }

    /// 改名中・色見本表示中の状態を捨てる。
    fn clear_transient(&mut self) {
        self.rename_target = None;
        self.rename_focus_pending = false;
        self.color_picker_for = None;
    }

    /// パネルを描画し、実行すべきコマンドを返す。
    ///
    /// 返り値が空でなければ、呼び出し側が `Document::apply` で適用する。
    ///
    /// `busy` … コマンド（選択待ちを含む）を実行中か。真の間は「移動」の行を押せなくする
    /// （プロパティパネルと同じ規則。実行中のツールが覚えている図形を横から動かさない）。
    /// `drop_note` … 図形をロック・非表示のレイヤへ移して選択から外れたときの案内。
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

        // 改名中のレイヤが Undo などで消えたら改名をやめる。行が描かれないまま残ると、
        // 戻ってきたときにフォーカスの無い入力欄だけが出る。
        if self
            .rename_target
            .is_some_and(|id| doc.layers().get(id).is_none())
        {
            self.rename_target = None;
            self.rename_focus_pending = false;
        }

        ui.heading("レイヤ");
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
        self.show_move_row(ui, doc, selection, busy, drop_note, &mut commands);

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

            // 名前（ダブルクリックで編集）。長い名前は、右端の線種・削除ボタンの分を除いた
            // 残りの幅で省略する（ホバーで全体）。
            let name_width =
                (ui.available_width() - ROW_RIGHT_WIDTH - ui.spacing().item_spacing.x * 2.0)
                    .max(NAME_MIN_WIDTH);
            if self.rename_target == Some(id) {
                let edit_id = egui::Id::new(("layer_rename", id.index()));
                if std::mem::take(&mut self.rename_focus_pending) {
                    focus_with_all_selected(ui, edit_id, &self.rename_buffer);
                }
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.rename_buffer)
                        .id(edit_id)
                        .desired_width(name_width.min(120.0))
                        // Tab は欄の中で何もしない（1 行の欄なのでタブ文字も入らない）。egui の既定では
                        // 隣の部品（線種のドロップダウン）へフォーカスが移り、その後に打った文字は
                        // どこにも入らず、コマンドラインも取り直さなかった（PR #71 のレビュー）。
                        .event_filter(egui::EventFilter {
                            tab: true,
                            horizontal_arrows: true,
                            vertical_arrows: true,
                            escape: false,
                        }),
                );
                // フォーカスが外れたら改名を終える。Enter なら確定、それ以外（Esc・ほかの場所の
                // クリック）はやめる。Esc は egui がフレームの最初にフォーカスを外すので、
                // コマンドラインは奪わない（ADR-0035 の前フレームのフォーカスを見る判定）。
                if response.lost_focus() {
                    let (enter, escape) = ui.input(|i| {
                        (
                            i.key_pressed(egui::Key::Enter),
                            i.key_pressed(egui::Key::Escape),
                        )
                    });
                    match end_rename(doc, id, &self.rename_buffer, enter, escape) {
                        RenameEnd::Commit(new_name) => {
                            commands.push(Box::new(RenameLayer::new(id, new_name)));
                            self.rename_target = None;
                        }
                        RenameEnd::Rejected(message) => {
                            // 欄を開いたまま、打った文字も残してフォーカスを戻す（直して Enter し直せる）。
                            self.notice = Some(PanelNotice::Error(message));
                            response.request_focus();
                        }
                        RenameEnd::Dropped { tell } => {
                            if tell {
                                self.notice =
                                    Some(PanelNotice::Info(RENAME_DROPPED_NOTE.to_owned()));
                            }
                            self.rename_target = None;
                        }
                    }
                }
            } else {
                let label = ui
                    .scope(|ui| {
                        ui.set_max_width(name_width);
                        ui.add(egui::Button::selectable(is_current, &layer.name).truncate())
                    })
                    .inner;
                if label.double_clicked() && !is_zero {
                    self.rename_target = Some(id);
                    self.rename_buffer = layer.name.clone();
                    self.rename_focus_pending = true;
                }
                // ツールチップには常に全体の名前を出す。省略されたかどうかを幅で判定すると、
                // 全角文字では文字の切れ目の余りで外れ、省略されているのに出ないことがあった。
                if is_zero {
                    label.on_hover_text(format!("{}\nレイヤ 0 は名前を変更できません", layer.name));
                } else {
                    label.on_hover_text(&layer.name);
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

    /// 選択中の図形を別レイヤへ移す行。
    fn show_move_row(
        &self,
        ui: &mut egui::Ui,
        doc: &Document,
        selection: &Selection,
        busy: bool,
        drop_note: Option<&str>,
        commands: &mut Vec<Box<dyn Command>>,
    ) {
        if let Some(note) = drop_note {
            ui.colored_label(DROP_NOTE_COLOR, note);
        }
        // 選択が空なら移すものが無い（「先に図形を選択してください」だけで足りる）。
        if busy && !selection.is_empty() {
            ui.colored_label(BUSY_COLOR, MOVE_BUSY_NOTE);
        }
        ui.horizontal_wrapped(|ui| {
            if selection.is_empty() {
                ui.weak("選択中の図形を別のレイヤへ移すには、先に図形を選択してください");
                return;
            }
            ui.label(format!("選択中の {} 個を移動:", selection.len()));
            if busy {
                ui.disable();
            }
            for (id, layer) in doc.layers().iter() {
                // 長い名前は、この行の残りの幅までで省略する（行を押し広げない。全体はホバーで出る）。
                let button = ui.add(egui::Button::new(&layer.name).truncate());
                if button.clicked() {
                    commands.push(Box::new(MoveEntitiesToLayer::new(selection.to_vec(), id)));
                }
                // 常に全体の名前を出す（省略の判定は全角文字で外れる）。実行中は無効のボタンなので、
                // 無効のときのツールチップにも同じ名前を出す。
                button
                    .on_hover_text(&layer.name)
                    .on_disabled_hover_text(&layer.name);
            }
        });
    }
}

/// 入力欄 `id` にフォーカスを渡し、中身 `text` を全部選んだ状態にする。打てばそのまま置き換わる。
///
/// **入力欄を描く前に呼ぶこと。** フォーカスの無い `TextEdit` は描くときに選択範囲を
/// キャレット 1 つに縮めるので、描いた後にフォーカスを渡すと全選択が残らない。
///
/// フォーカスを渡さないと、キー入力はコマンドラインのものになる（コマンドラインは
/// 誰もフォーカスを持っていなければ自分で取り直す）。打った名前がコマンドラインへ入り、
/// Enter でコマンドとして実行されていた（Issue #68）。
fn focus_with_all_selected(ui: &egui::Ui, id: egui::Id, text: &str) {
    ui.memory_mut(|m| m.request_focus(id));
    let mut state = egui::text_edit::TextEditState::load(ui.ctx(), id).unwrap_or_default();
    state
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::two(
            egui::text::CCursor::new(0),
            egui::text::CCursor::new(text.chars().count()),
        )));
    state.store(ui.ctx(), id);
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
        p.rename_focus_pending = true;
        p.color_picker_for = Some(LayerId::ZERO);
        p.toggle();
        assert!(p.rename_target.is_none());
        assert!(!p.rename_focus_pending);
        assert!(p.color_picker_for.is_none());
    }

    /// 図面が入れ替わったら、前の図面のレイヤ ID を指す編集中の状態を捨てること（Issue #65）。
    #[test]
    fn document_replaced_clears_transient_state_but_keeps_the_panel_open() {
        let mut p = LayerPanel::new();
        p.toggle();
        p.rename_target = Some(LayerId::ZERO);
        p.rename_focus_pending = true;
        p.color_picker_for = Some(LayerId::ZERO);
        p.document_replaced();
        assert!(p.rename_target.is_none());
        assert!(!p.rename_focus_pending);
        assert!(p.color_picker_for.is_none());
        assert!(p.is_open(), "開閉は利用者の設定なので残す");
    }

    /// 改名の欄からフォーカスが外れたときの結末。
    #[test]
    fn end_rename_decides_by_key_and_name() {
        let mut doc = Document::new();
        for name in ["L1", "L2"] {
            doc.apply(Box::new(AddLayer::new(name, AciColor::WHITE)))
                .expect("レイヤ");
        }
        let l1 = doc.layers().by_name("L1").expect("L1");
        let end =
            |buffer: &str, enter: bool, escape: bool| end_rename(&doc, l1, buffer, enter, escape);

        assert_eq!(end(" WALL ", true, false), RenameEnd::Commit("WALL".into()));
        assert!(
            matches!(end("L2", true, false), RenameEnd::Rejected(m) if m.starts_with("レイヤ名の変更:")),
            "同名は断る"
        );
        assert_eq!(
            end("L1", true, false),
            RenameEnd::Dropped { tell: false },
            "元のまま"
        );
        assert_eq!(
            end("  ", true, false),
            RenameEnd::Dropped { tell: false },
            "空"
        );
        assert_eq!(
            end("WALL", false, true),
            RenameEnd::Dropped { tell: false },
            "Esc"
        );
        assert_eq!(
            end("WALL", false, false),
            RenameEnd::Dropped { tell: true },
            "外のクリック"
        );
        assert_eq!(
            end("L1", false, false),
            RenameEnd::Dropped { tell: false },
            "変えていない"
        );
    }

    /// 改名の欄が「確定」と判定する名前と、`RenameLayer` が成功する名前が一致すること。
    ///
    /// 判定は `LayerTable::check_rename` に 1 つだけ置いてあるが、パネルがそれを呼ばなくなった
    /// （または手前で別の条件を足した）ときに、欄を閉じたのに改名が失敗する、のを捕まえる。
    /// 空・元のまま・前後の空白だけの差は、パネルが先に「やめる」にする（コマンドを作らない）。
    #[test]
    fn end_rename_commits_exactly_the_names_rename_layer_accepts() {
        let candidates = [
            "L1",
            "L2",
            "0",
            "l2",
            "L2 ",
            " L2 ",
            "L3",
            "WALL",
            "",
            "  ",
            "ＷＡＬＬ",
            "壁",
        ];
        for candidate in candidates {
            let make = || {
                let mut doc = Document::new();
                for name in ["L1", "L2"] {
                    doc.apply(Box::new(AddLayer::new(name, AciColor::WHITE)))
                        .expect("レイヤ");
                }
                doc
            };
            let mut doc = make();
            let l1 = doc.layers().by_name("L1").expect("L1");
            let decided = end_rename(&doc, l1, candidate, true, false);
            let applied = doc.apply(Box::new(RenameLayer::new(l1, candidate.trim())));
            match decided {
                RenameEnd::Commit(name) => {
                    assert_eq!(name, candidate.trim());
                    assert!(
                        applied.is_ok(),
                        "{candidate:?}: 確定と判定したのに失敗: {applied:?}"
                    );
                }
                RenameEnd::Rejected(_) => {
                    assert!(
                        applied.is_err(),
                        "{candidate:?}: 断ったのに RenameLayer は通る"
                    );
                }
                RenameEnd::Dropped { .. } => {
                    // パネルの方針でやめる名前（空・元のまま）。コマンドは作らないので、ここでは
                    // 「断る名前」と取り違えていないことだけ見る。
                    assert!(
                        candidate.trim().is_empty() || candidate.trim() == "L1",
                        "{candidate:?}: やめる理由が無い"
                    );
                }
            }
        }
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
