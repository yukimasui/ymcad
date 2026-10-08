//! プロパティパネル（Issue #31 段階 1: 表示とレイヤの変更、段階 2: 数値の編集、
//! 段階 3: インプレース編集中の束縛）。
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
//! # インプレース編集中は、束縛（式）で決まる項目だけ表示だけ（段階 3）
//!
//! コンポーネントの編集中（ADR-0033）に選んだ中身は、定義の束縛で決まる項目を表示だけにし、
//! 値の横に式を出す（長い式は省略し、ツールチップで全体）。束縛の無い項目は変えられ、`ENDCOMP` の
//! 後も定義に残る。どの項目がどの束縛で決まるかは `properties_bind.rs` の対応表が決める。
//!
//! 中身を決める部分は egui に依存しない純粋な関数として `properties.rs`・
//! `properties_edit.rs`・`properties_bind.rs` にある。

use std::cell::Cell;

use cad_core::command::{MoveEntitiesToLayer, ReplaceGeometries};
use cad_core::{Command, Document, EntityId, Geometry};

use crate::editing::EditSession;
use crate::properties::{
    self, fmt_num, kind_of, CommonLayer, Editor, Summary, SummaryCache, BUSY_NOTE, EMPTY_NOTE,
    MIXED_LAYER, MULTI_NOTE,
};
use crate::properties_bind::{EntityBindings, Key, Lock};
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
/// コンポーネントの編集中、束縛（式）で決まる項目があるときの案内。
pub const BOUND_NOTE: &str =
    "「←」の付いた値はコンポーネントの式で決まるため表示だけです（ほかの値は変えられます）";
/// 束縛（式）の案内の色（コマンド実行中の琥珀色・選択から外れた案内の青緑とは別にする）。
pub const BOUND_COLOR: egui::Color32 = egui::Color32::from_rgb(0xb3, 0x9d, 0xdb);
/// 編集中に図面が変わったため、入力を捨てたときの案内。
pub const STALE_NOTE: &str = "編集中に図面が変わったため、入力した値は確定しませんでした";

/// 項目名の欄の幅 [px]。
const LABEL_WIDTH: f32 = 84.0;
/// 表の列と行の間隔 [px]。
const GRID_SPACING: [f32; 2] = [8.0, 4.0];
/// 数値の欄（`DragValue`）の最小の幅 [px]。`-99999.9999°` が入り、最小幅のパネル（200px）でも
/// 項目名の欄と並ぶ。
const FIELD_WIDTH: f32 = 100.0;
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
    /// そのコマンドがグリップ編集か（案内の文言を変える。Esc で選択は外れない）。
    pub gripping: bool,
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
        // 仮の形はドラッグしているフレームだけ描く。前のフレームの分は、表の表示だけの値
        // （直径・円周・中点など）を仮の形から出すために取っておく（1 フレーム遅れる）。
        let last_preview = self.preview.take();
        if !self.open {
            return commands;
        }

        ui.heading("プロパティ");
        ui.separator();
        if input.busy {
            let note = if input.gripping {
                properties::GRIP_BUSY_NOTE
            } else {
                BUSY_NOTE
            };
            ui.colored_label(BUSY_COLOR, note);
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
                    self.show_items(ui, input, last_preview.as_ref(), &mut commands);
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

/// 項目の編集のしかたから、項目のキー。
fn editor_key(editor: Editor) -> Key {
    match editor {
        Editor::Number(field, _) => Key::Number(field),
        Editor::Toggle(toggle, _) => Key::Toggle(toggle),
    }
}

/// 式の案内のうち、省略してよい残り（式）に少なくとも取りたい幅 [px]。これが取れないほど狭ければ、
/// 案内を値の横ではなく次の行に出す。
const BADGE_REST_MIN_WIDTH: f32 = 40.0;

/// 表示だけの値。束縛（式）で決まるなら、横に式の案内を出す（ツールチップで式の全体と理由）。
///
/// 案内は**折り返さずに 1 行**で出し、入り切らなければ式の側だけを省略する（`← 式` は残す）。
/// 折り返すと、表の行の高さが増えた分だけ次の行の欄の下に 2 行目が隠れた（PR #82 の操作レビュー）。
/// 値の横に `← 式` と式の頭も入らないほど狭いとき（3 枚のパネルを開いた狭い画面）は、表に 1 行
/// 足して、値の下の行に出す。表の行は 1 行ずつなので、欄と重ならない。
///
/// 呼び出し側は、いつもどおりこの後で `end_row` する。
fn show_value(ui: &mut egui::Ui, value: String, lock: Option<&Lock>) {
    let width_of = |ui: &egui::Ui, text: &str, style: egui::TextStyle| {
        ui.painter()
            .layout_no_wrap(text.to_owned(), style.resolve(ui.style()), BOUND_COLOR)
            .size()
            .x
    };
    let Some(lock) = lock else {
        ui.add(
            egui::Label::new(egui::RichText::new(value).monospace())
                .selectable(false)
                .extend(),
        );
        return;
    };
    let (head, _) = lock.badge_parts();
    let needed = width_of(ui, &value, egui::TextStyle::Monospace)
        + ui.spacing().item_spacing.x
        + width_of(ui, head, egui::TextStyle::Body)
        + BADGE_REST_MIN_WIDTH;
    let beside = ui.available_width() >= needed;
    ui.horizontal(|ui| {
        ui.add(
            egui::Label::new(egui::RichText::new(value).monospace())
                .selectable(false)
                .extend(),
        );
        if beside {
            show_badge(ui, lock);
        }
    });
    if !beside {
        ui.end_row();
        ui.label("");
        ui.horizontal(|ui| show_badge(ui, lock));
    }
}

/// 式の案内（`← 式` と、入り切らなければ省略する式）。ツールチップは式の全体と理由。
fn show_badge(ui: &mut egui::Ui, lock: &Lock) {
    let (head, rest) = lock.badge_parts();
    let text = |s: String| egui::RichText::new(s).color(BOUND_COLOR);
    let head = ui.add(
        egui::Label::new(text(head.to_owned()))
            .selectable(false)
            .extend(),
    );
    // 頭と残りは続けて 1 つの文に見せる。
    ui.spacing_mut().item_spacing.x = 0.0;
    let rest = ui.add(
        egui::Label::new(text(rest))
            .selectable(false)
            .truncate()
            // 省略されたときの egui 既定のツールチップ（式の文字だけ）は出さず、理由つきの方を出す。
            .show_tooltip_when_elided(false),
    );
    head.union(rest).on_hover_text(lock.tooltip());
}

impl PropertiesPanel {
    /// 1 つ選んだときの項目の一覧。編集できる項目は `DragValue` かチェックボックスにする。
    fn show_items(
        &mut self,
        ui: &mut egui::Ui,
        input: &PanelInput<'_>,
        last_preview: Option<&Geometry>,
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
        let geom = &entity.geom;
        let kind = kind_of(geom);
        // コンポーネントの編集中は、束縛（式）で決まる項目だけを表示だけにする（段階 3）。
        // どの項目がどの束縛で決まるかは `properties_bind` の対応表が決める。
        let bindings = input
            .component_edit
            .map(|s| EntityBindings::new(s.placement(), s.bindings(doc, id)))
            .filter(|b| !b.is_empty());
        let lock_of = |key: Key| bindings.as_ref().and_then(|b| b.lock(kind, key));
        let vertex_rows = bindings
            .as_ref()
            .map(|b| b.vertex_rows(geom))
            .unwrap_or_default();
        let editable = !input.busy;
        if !editable {
            self.discard_edit();
        }
        // 編集中の項目が束縛で決まるようになっていたら（念のため。束縛はコマンドでしか変わらず、
        // コマンド実行中は上で捨てている）、打ちかけの値で確定しない。
        if self
            .editing
            .is_some_and(|e| e.id == id && lock_of(Key::Number(e.field)).is_some())
        {
            self.discard_edit();
        }
        let lock_of_item =
            |item: &properties::Item| item.editor.and_then(|e| lock_of(editor_key(e)));
        let any_locked = properties::items(geom, doc.definitions())
            .iter()
            .any(|item| lock_of_item(item).is_some());
        if any_locked || !vertex_rows.is_empty() {
            ui.colored_label(BOUND_COLOR, BOUND_NOTE);
        }
        // 古い理由（図面が変わった後、別の図形を選んだ後）は出さない。
        if self
            .note
            .as_ref()
            .is_some_and(|n| n.revision != doc.revision() || n.id != id)
        {
            self.note = None;
        }

        let target = Target { doc, id, geom };
        // ドラッグ中は、表示だけの項目（円の直径・円周、円弧の掃引角・弧長、線分の中点など）も
        // 仮の形から出す。図形は仮の形に変わって見えるのに、数字だけ元のままになるのを防ぐ。
        // 仮の形を使うのは、いまの編集（同じ図形・同じ版）に結び付いているときだけ。
        let shown_geom = last_preview
            .filter(|_| {
                self.editing.is_some_and(|e| {
                    e.id == id && e.revision == doc.revision() && e.pending.is_some()
                })
            })
            .unwrap_or(geom);
        let mut has_number = false;
        egui::Grid::new(("properties_items", kind as u8))
            .num_columns(2)
            .min_col_width(LABEL_WIDTH)
            .spacing(GRID_SPACING)
            .show(ui, |ui| {
                for item in properties::items(shown_geom, doc.definitions()) {
                    // 項目の種類と並びは仮の形でも同じ（同じ種類の図形なので）。
                    let lock = lock_of_item(&item);
                    ui.add(egui::Label::new(item.label).selectable(false));
                    let key = match (item.editor, editable, lock) {
                        (Some(Editor::Number(field, value)), true, None) => {
                            let step = match field {
                                Field::Scale => SCALE_STEP,
                                f if f.is_angle() => ANGLE_STEP,
                                _ => input.length_step,
                            };
                            self.number_field(ui, target, field, value, step, commands);
                            has_number = true;
                            Some(Key::Number(field))
                        }
                        (Some(Editor::Toggle(toggle, value)), true, None) => {
                            self.toggle_field(ui, target, toggle, value, commands);
                            Some(Key::Toggle(toggle))
                        }
                        (_, _, lock) => {
                            show_value(ui, item.value, lock.as_ref());
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
                // ポリラインの束縛された頂点（頂点はパネルの項目に無いので、表示だけの行を足す）。
                for row in vertex_rows {
                    ui.add(egui::Label::new(row.label).selectable(false));
                    show_value(ui, row.value, Some(&row.lock));
                    ui.end_row();
                }
            });
        // 数値の欄があるときだけ（ポリラインはチェックボックスだけなので出さない）。
        if has_number {
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
                // 入力欄の幅は `interact_size.x` で決まる（既定の約 40px では 4 桁の値が隠れる）。
                ui.spacing_mut().interact_size.x = FIELD_WIDTH;
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

        if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            // 取り消し。打ちかけの値は捨てる。読み取りの失敗（`parse_error`）より先に見る。
            self.end_edit(id, field);
            return;
        }
        if let Some(reason) = parse_error.take() {
            // 編集を始めた記録が無い欄の読み取りの失敗は、利用者が打った値の失敗ではない。
            // Esc で取り消した次のフレームで、`DragValue` が打ちかけの文字（`abc` など）を
            // もう一度読みにいくため（`lost_focus` が 2 フレーム続く）。理由を出さず、無視する。
            if self.editing.is_some_and(|e| mine(&e)) {
                // 数値として読めない・有限でない。値は元のまま。
                self.end_edit(id, field);
                self.set_note(id, Key::Number(field), revision, reason.to_owned());
            }
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
                if self.preview.is_some() {
                    // 表の表示だけの値は 1 フレーム遅れて追いつく。マウスを止めても追いつかせる。
                    ui.ctx().request_repaint();
                }
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
            // 守りは二重になっている。ここ（呼び出し側）と `commit`（冒頭で編集を始めた記録を
            // 確かめ、無ければ何も確定しない）。重複は意図したもの。この先の `commit` を呼ぶ
            // 3 か所のどれかの条件が変わっても、記録の無い欄から確定が出ないようにするため。
            // どちらも外さないこと。
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
        // 編集を始めた記録が無ければ確定しない。`number_field` の `session.is_none()` と二重の守り。
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
