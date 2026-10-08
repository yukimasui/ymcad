//! プロパティパネル（Issue #31 段階 1: 表示とレイヤの変更、段階 2: 数値の編集）。
//!
//! 選んだ図形の種類ごとの値を出し、レイヤをドロップダウンで変える。AutoCAD の PROPERTIES
//! （Ctrl+1）にあたるが、クイックプロパティのようにカーソル横へ飛び出さず、レイヤ・
//! コンポーネントと同じ右側のパネルに置く（Issue #31 の「AutoCAD での不満と対策」）。
//!
//! # `Document` は変更しない
//!
//! レイヤパネルと同じく、[`Command`] を返すだけで適用は呼び出し側（ADR-0012）。
//!
//! # 数値の編集は 1 回だけ確定する（ADR-0041）
//!
//! 値は `DragValue` で、ドラッグでも、クリックして打ち込んでも変えられる。確定は
//! Enter・フォーカスが外れたとき・ドラッグを離したときの **1 回だけ**で、
//! `ReplaceGeometries`（ADR-0040）1 つを返す（Undo 1 回）。ドラッグ中は図面を変えず、
//! 仮の形を [`PropertiesPanel::drag_preview`] で渡してラバーバンドと同じ経路で描く。
//! Esc で取り消す。不正な値は確定せず、項目のすぐ下に理由を出す。
//!
//! 編集中の項目は「図形の ID・項目・編集を始めたときの図面の版番号」に結び付ける。
//! 確定先は、いまの選択ではなく編集を始めた図形。版番号が変わっていたら（編集中に
//! リボンの UNDO など）確定せずに案内する。NEW / OPEN では [`PropertiesPanel::invalidate`]
//! で編集中の状態を捨てる（版番号は図面をまたいで比べられない）。
//!
//! # コマンド実行中は表示だけ
//!
//! 選択待ちを含めてコマンドを実行している間は、パネルのレイヤ変更も数値の編集も無効にして
//! 案内を出す。実行中のツールが覚えている選択や図形を、パネルが横から書き換えないため
//! （#37 の教訓。パネルの開閉は実行中のコマンドを中断しないので、開いたままコマンドを
//! 始められる）。
//!
//! 中身を決める部分は egui に依存しない純粋な関数として `properties.rs` と
//! `properties_edit.rs` にある。

use std::cell::Cell;

use cad_core::command::{MoveEntitiesToLayer, ReplaceGeometries};
use cad_core::{Command, Document, EntityId, Geometry};

use crate::editing::EditSession;
use crate::properties::{
    self, fmt_num, kind_of, CommonLayer, Editor, Summary, SummaryCache, BUSY_NOTE, EMPTY_NOTE,
    MIXED_LAYER, MULTI_NOTE,
};
use crate::properties_edit::{
    edit_number, edit_toggle, parse_number, same_on_screen, Field, Toggle,
};
use crate::selection::Selection;

/// コマンド実行中の案内の色（ステータスバーの「コマンド実行中」と同じ琥珀色）。
pub const BUSY_COLOR: egui::Color32 = egui::Color32::from_rgb(0xff, 0xc1, 0x07);

/// 選択から外れた案内の色（コマンド実行中の琥珀色とは別にして、状態の案内と区別する）。
pub const DROP_NOTE_COLOR: egui::Color32 = egui::Color32::from_rgb(0x80, 0xcb, 0xc4);

/// 確定したときの `ReplaceGeometries` の名前（Undo の表示などに出る）。
pub const EDIT_COMMAND: &str = "PROPERTIES";
/// 編集できるときに、項目の表の下に出す操作の案内。
pub const EDIT_HINT: &str = "値はドラッグで増減、クリックで入力（Enter で確定・Esc で取り消し）";
/// コンポーネントの編集中、束縛（式）を持つ中身を選んだときの案内。
pub const BOUND_NOTE: &str = "この図形はコンポーネントの式で決まる値を持つため、数値は表示だけです";
/// 編集中に図面が変わったため、入力を捨てたときの案内。
pub const STALE_NOTE: &str = "編集中に図面が変わったため、入力した値は確定しませんでした";

/// 項目名の欄の幅 [px]。
const LABEL_WIDTH: f32 = 84.0;
/// 表の列と行の間隔 [px]。
const GRID_SPACING: [f32; 2] = [8.0, 4.0];
/// 角度のドラッグの速さ [度/px]。
const ANGLE_STEP: f64 = 0.5;
/// 倍率のドラッグの速さ [/px]。
const SCALE_STEP: f64 = 0.01;

/// パネルに渡す、描くのに要るもの。
pub struct PanelInput<'a> {
    /// 図面。
    pub doc: &'a Document,
    /// 選択。
    pub selection: &'a Selection,
    /// コマンド（選択待ちを含む）を実行中か。真の間は表示だけになる。
    pub busy: bool,
    /// 図形をロック・非表示のレイヤへ移して選択から外れたときの案内（`Session::drop_note`）。
    /// 選択が空の表示の上に出す。
    pub drop_note: Option<&'a str>,
    /// コンポーネントの編集中ならその編集（束縛を持つ中身は表示だけにする）。
    pub component_edit: Option<&'a EditSession>,
    /// 長さ・座標のドラッグの速さ [図面の長さ/px]。画面の 1px に当たる長さにする
    /// （ズームしても、マウスの動きと値の動きの感覚が変わらない）。
    pub length_step: f64,
}

/// 項目の欄を置く `Ui`。ID を（図形, 項目）だけから決める。
///
/// egui の `DragValue` の ID は、親の `Ui` の中での並び順から決まる。`push_id` で塩を足しても、
/// 子の ID には親の「次の自動 ID」（並び順）が混ざる（egui 0.36 の `Ui::new_child`）ので、
/// 上の行に理由の行が出入りすると ID が変わり、入力中の欄がフォーカスを失う（テストで実測）。
/// `UiBuilder::id` は親に依存しない ID を与える。
fn field_scope(key: Key, id: EntityId) -> egui::UiBuilder {
    egui::UiBuilder::new().id(egui::Id::new(("properties_field", id, key)))
}

/// 編集する図形（図面・ID・いまの形）。
#[derive(Clone, Copy)]
struct Target<'a> {
    doc: &'a Document,
    id: EntityId,
    geom: &'a Geometry,
}

/// 項目（数値か、はい・いいえか）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Key {
    Number(Field),
    Toggle(Toggle),
}

/// 編集中の項目。フォーカスを得たとき・ドラッグを始めたときに作る。
#[derive(Clone, Copy, Debug, PartialEq)]
struct Editing {
    /// 編集を始めた図形。確定先はいまの選択ではなくこれ。
    id: EntityId,
    field: Field,
    /// 編集を始めたときの図面の版番号。確定時に違えば捨てる。
    revision: u64,
    /// ドラッグ中（や矢印キーの増減）の、まだ確定していない値。
    pending: Option<f64>,
}

/// 項目のすぐ下に出す理由（不正な値など）。図面が変わったら古くなるので出さない。
#[derive(Clone, Debug, PartialEq)]
struct Note {
    id: EntityId,
    key: Key,
    revision: u64,
    text: String,
}

/// プロパティパネルの状態。
#[derive(Debug, Default)]
pub struct PropertiesPanel {
    /// パネルを開いているか。
    open: bool,
    /// 選択の要約。図面と選択の版番号をキーに作り直す。
    summary: SummaryCache,
    /// 編集中の項目。
    editing: Option<Editing>,
    /// 項目の下に出す理由。
    note: Option<Note>,
    /// ドラッグ中の仮の形（このフレームで描く分）。
    preview: Option<Geometry>,
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

    /// 開閉を切り替える。編集中の値は捨てる。
    pub fn toggle(&mut self) {
        self.open = !self.open;
        self.discard_edit();
    }

    /// 選択の要約を作り直した回数（テスト用）。
    #[cfg(test)]
    pub fn summary_recomputed(&self) -> usize {
        self.summary.recomputed()
    }

    /// 編集中の項目か、項目の下の理由か、仮の形を持っているか（テスト用）。
    #[cfg(test)]
    pub fn has_edit_state(&self) -> bool {
        self.editing.is_some() || self.note.is_some() || self.preview.is_some()
    }

    /// 図面が丸ごと入れ替わったとき（NEW / OPEN）に呼ぶ。要約のキャッシュと、編集中の値・
    /// 理由・仮の形を捨てる。
    ///
    /// どれも図面の版番号に結び付けているが、版番号は新しい図面と重なりうる。残すと、
    /// 前の図面で打ちかけた値が、新しい図面の同じ番号の図形へ確定されうる。
    pub fn invalidate(&mut self) {
        self.summary.invalidate();
        self.discard_edit();
    }

    /// ドラッグ中の仮の形。キャンバスがラバーバンドと同じ経路で描く（図面はまだ変えていない）。
    #[must_use]
    pub fn drag_preview(&self) -> Option<&Geometry> {
        self.preview.as_ref()
    }

    fn discard_edit(&mut self) {
        self.editing = None;
        self.note = None;
        self.preview = None;
    }

    /// パネルを描画し、実行すべきコマンドを返す。
    #[must_use]
    pub fn show(&mut self, ui: &mut egui::Ui, input: &PanelInput<'_>) -> Vec<Box<dyn Command>> {
        let mut commands: Vec<Box<dyn Command>> = Vec::new();
        // 仮の形はドラッグしているフレームだけ描く。
        self.preview = None;
        if !self.open {
            return commands;
        }

        ui.heading("プロパティ");
        ui.separator();
        if input.busy {
            ui.colored_label(BUSY_COLOR, BUSY_NOTE);
            ui.separator();
            // 実行中は編集しない。打ちかけの値も残さない（終わった後に古い値で確定しない）。
            self.discard_edit();
        }

        let summary = self.summary.get(input.doc, input.selection).clone();
        let (Some(layer), true) = (summary.layer, summary.total > 0) else {
            if let Some(note) = input.drop_note {
                ui.colored_label(DROP_NOTE_COLOR, note);
                ui.separator();
            }
            ui.weak(EMPTY_NOTE);
            self.discard_edit();
            return commands;
        };

        egui::ScrollArea::vertical()
            .auto_shrink([false, true])
            .show(ui, |ui| {
                show_header(ui, &summary);
                show_layer_row(
                    ui,
                    input.doc,
                    input.selection,
                    layer,
                    input.busy,
                    &mut commands,
                );
                ui.separator();
                if summary.total == 1 {
                    self.show_items(ui, input, &mut commands);
                } else {
                    // 複数選択はレイヤだけ（判断 4）。1 つのときの編集途中の値は持ち越さない。
                    self.discard_edit();
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

impl PropertiesPanel {
    /// 1 つ選んだときの項目の一覧。編集できる項目は `DragValue` かチェックボックスにする。
    fn show_items(
        &mut self,
        ui: &mut egui::Ui,
        input: &PanelInput<'_>,
        commands: &mut Vec<Box<dyn Command>>,
    ) {
        let doc = input.doc;
        // 見出しの「1 つ」は図面に実在する数（`summarize`）なので、実在する最初の図形を取る。
        let Some((id, entity)) = input
            .selection
            .iter()
            .find_map(|id| doc.entities().get(id).map(|e| (id, e)))
        else {
            return;
        };
        // コンポーネントの編集中、束縛（式）を持つ中身は数値を表示だけにする（段階 3 で項目ごとにする）。
        let bound = input.component_edit.is_some_and(|s| s.is_bound(doc, id));
        if bound {
            ui.colored_label(BUSY_COLOR, BOUND_NOTE);
        }
        let editable = !input.busy && !bound;
        if !editable {
            self.discard_edit();
        }
        // 古い理由（図面が変わった後、別の図形を選んだ後）は出さない。
        if self
            .note
            .as_ref()
            .is_some_and(|n| n.revision != doc.revision() || n.id != id)
        {
            self.note = None;
        }

        let geom = &entity.geom;
        let target = Target { doc, id, geom };
        egui::Grid::new(("properties_items", kind_of(geom) as u8))
            .num_columns(2)
            .min_col_width(LABEL_WIDTH)
            .spacing(GRID_SPACING)
            .show(ui, |ui| {
                for item in properties::items(geom, doc.definitions()) {
                    ui.add(egui::Label::new(item.label).selectable(false));
                    let key = match (item.editor, editable) {
                        (Some(Editor::Number(field, value)), true) => {
                            let step = match field {
                                Field::Scale => SCALE_STEP,
                                f if f.is_angle() => ANGLE_STEP,
                                _ => input.length_step,
                            };
                            self.number_field(ui, target, field, value, step, commands);
                            Some(Key::Number(field))
                        }
                        (Some(Editor::Toggle(toggle, value)), true) => {
                            self.toggle_field(ui, target, toggle, value, commands);
                            Some(Key::Toggle(toggle))
                        }
                        _ => {
                            ui.add(
                                egui::Label::new(egui::RichText::new(item.value).monospace())
                                    .selectable(false),
                            );
                            None
                        }
                    };
                    ui.end_row();
                    let note = self
                        .note
                        .as_ref()
                        .filter(|n| Some(n.key) == key && n.id == id)
                        .map(|n| n.text.clone());
                    if let Some(text) = note {
                        // 項目のすぐ下に、理由を赤字で出す。
                        ui.label("");
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(text).color(ui.visuals().error_fg_color),
                            )
                            .wrap(),
                        );
                        ui.end_row();
                    }
                }
            });
        if editable {
            ui.weak(EDIT_HINT);
        }
    }

    /// 数値の項目 1 つ。確定は Enter・フォーカス外れ・ドラッグを離したときの 1 回だけ。
    ///
    /// `value` はいま表示している値（角度は度）。
    fn number_field(
        &mut self,
        ui: &mut egui::Ui,
        target: Target<'_>,
        field: Field,
        value: f64,
        step: f64,
        commands: &mut Vec<Box<dyn Command>>,
    ) {
        let Target { doc, id, geom } = target;
        let revision = doc.revision();
        let mine = |e: &Editing| e.id == id && e.field == field;
        let pending = self
            .editing
            .filter(|e| mine(e) && e.revision == revision)
            .and_then(|e| e.pending);
        let shown = pending.unwrap_or(value);
        let mut v = shown;
        let parse_error = Cell::new(None);

        // ID は並び順ではなく（図形, 項目）で決める。選択が別の図形へ変わったとき、同じ位置の
        // 欄が前の図形の入力中の文字やフォーカスを引き継がない。理由の行が出入りして並びが
        // ずれても、入力中の欄がフォーカスを失わない（[`field_scope`]）。
        let response = ui
            .scope_builder(field_scope(Key::Number(field), id), |ui| {
                let mut drag = egui::DragValue::new(&mut v)
                    .speed(step)
                    // 表示はステータスバー・段階 1 と同じ 4 桁。`range` は付けない（黙って丸め込まない）。
                    .custom_formatter(|n, _| fmt_num(n))
                    // 既定の読み取りは `nan`・`inf` を受け、全角数字を受けない。
                    .custom_parser(|s| parse_number(s).map_err(|e| parse_error.set(Some(e))).ok())
                    // 打っている途中の値で図形を変えない。
                    .update_while_editing(false);
                if field.is_angle() {
                    drag = drag.suffix("°");
                }
                ui.add(drag)
            })
            .inner;

        // 編集の始まり。版番号を覚える。
        let started = response.gained_focus() || response.drag_started() || response.clicked();
        let editing_now = response.has_focus() || response.dragged();
        if started || (editing_now && !self.editing.is_some_and(|e| mine(&e))) {
            self.editing = Some(Editing {
                id,
                field,
                revision,
                pending: None,
            });
            // 別の項目の理由も消す（新しい編集を始めた）。
            self.note = None;
        }

        if let Some(reason) = parse_error.take() {
            // 数値として読めない・有限でない。値は元のまま。
            self.end_edit(id, field);
            self.set_note(id, Key::Number(field), revision, reason.to_owned());
            return;
        }
        if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            // 取り消し。打ちかけの値は捨てる。
            self.end_edit(id, field);
            return;
        }
        if response.dragged() || response.has_focus() {
            // ドラッグ中（と、入力欄での矢印キーの増減）は仮の値として持つだけ。
            if v != shown {
                if let Some(e) = self.editing.as_mut().filter(|e| mine(e)) {
                    e.pending = Some(v);
                }
            }
            if response.dragged() {
                self.update_preview(doc, id, geom, field);
            }
            return;
        }
        let session = self.editing.filter(|e| mine(e));
        if response.drag_stopped() {
            match session.and_then(|e| e.pending) {
                Some(p) => self.commit(target, field, p, commands),
                None => self.end_edit(id, field),
            }
            return;
        }
        if session.is_none() {
            // 編集を始めていない（フォーカスが外れた次のフレームなど）。何もしない。
            return;
        }
        if v != shown {
            // Enter かフォーカス外れで、入力欄の文字が値になった。欄に出ていた丸めた値と
            // 同じに見えるなら、打ち直していない（いまの値のまま）とみなす。
            let entered = if same_on_screen(v, shown) { shown } else { v };
            self.commit(target, field, entered, commands);
        } else if response.lost_focus() {
            match session.and_then(|e| e.pending) {
                // 矢印キーで増減してから Enter を押した。
                Some(p) => self.commit(target, field, p, commands),
                // 触っただけ・Enter を押しただけ。版が変わっていたら知らせる。
                None => {
                    let stale = session.is_some_and(|e| e.revision != revision);
                    self.end_edit(id, field);
                    if stale {
                        self.set_note(id, Key::Number(field), revision, STALE_NOTE.to_owned());
                    }
                }
            }
        }
    }

    /// はい・いいえの項目 1 つ。チェックボックスを押したら確定する（1 回の押下 = Undo 1 回）。
    fn toggle_field(
        &mut self,
        ui: &mut egui::Ui,
        target: Target<'_>,
        toggle: Toggle,
        value: bool,
        commands: &mut Vec<Box<dyn Command>>,
    ) {
        let Target { doc, id, geom } = target;
        let mut checked = value;
        let response = ui
            .scope_builder(field_scope(Key::Toggle(toggle), id), |ui| {
                ui.checkbox(&mut checked, "")
            })
            .inner;
        if !response.changed() {
            return;
        }
        let key = Key::Toggle(toggle);
        match edit_toggle(geom, toggle, checked) {
            Ok(Some(g)) => {
                self.clear_note(id, key);
                commands.push(Box::new(ReplaceGeometries::one(EDIT_COMMAND, id, g)));
            }
            Ok(None) => self.clear_note(id, key),
            Err(e) => self.set_note(id, key, doc.revision(), e),
        }
    }

    /// ドラッグ中の仮の形を作る。不正な値なら仮の形は出さず、理由を出す。
    fn update_preview(&mut self, doc: &Document, id: EntityId, geom: &Geometry, field: Field) {
        let Some(p) = self
            .editing
            .filter(|e| e.id == id && e.field == field && e.revision == doc.revision())
            .and_then(|e| e.pending)
        else {
            return;
        };
        match edit_number(geom, field, p) {
            Ok(g) => {
                self.preview = g;
                self.clear_note(id, Key::Number(field));
            }
            Err(e) => self.set_note(id, Key::Number(field), doc.revision(), e),
        }
    }

    /// `value` で確定する。編集を始めた後に図面が変わっていたら確定しない。
    fn commit(
        &mut self,
        target: Target<'_>,
        field: Field,
        value: f64,
        commands: &mut Vec<Box<dyn Command>>,
    ) {
        let Target { doc, id, geom } = target;
        let key = Key::Number(field);
        let revision = doc.revision();
        let session = self.editing.filter(|e| e.id == id && e.field == field);
        self.end_edit(id, field);
        let Some(session) = session else {
            return;
        };
        if session.revision != revision {
            self.set_note(id, key, revision, STALE_NOTE.to_owned());
            return;
        }
        match edit_number(geom, field, value) {
            Ok(Some(g)) => {
                self.clear_note(id, key);
                commands.push(Box::new(ReplaceGeometries::one(EDIT_COMMAND, id, g)));
            }
            Ok(None) => self.clear_note(id, key),
            Err(e) => self.set_note(id, key, revision, e),
        }
    }

    fn end_edit(&mut self, id: EntityId, field: Field) {
        if self.editing.is_some_and(|e| e.id == id && e.field == field) {
            self.editing = None;
        }
        self.preview = None;
    }

    fn set_note(&mut self, id: EntityId, key: Key, revision: u64, text: String) {
        self.note = Some(Note {
            id,
            key,
            revision,
            text,
        });
    }

    fn clear_note(&mut self, id: EntityId, key: Key) {
        if self
            .note
            .as_ref()
            .is_some_and(|n| n.id == id && n.key == key)
        {
            self.note = None;
        }
    }
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
