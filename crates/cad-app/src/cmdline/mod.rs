//! 常設のコマンドライン。
//!
//! AutoCAD のコマンドウィンドウ相当。**キー入力は常にここへ流れる**ので、
//! ユーザーが明示的に入力欄をクリックする必要はない。
//!
//! # IME への配慮（ADR-0002）
//!
//! egui の `TextEdit` は **未確定文字列をアプリのバッファへ直接書き込む**。
//! そのため変換中にバッファを解釈すると、確定前の「にほんご」のような文字列を
//! コマンドとして扱ってしまう。
//!
//! ここでは [`egui::ImeEvent`] を監視して変換中かどうかを追跡し、
//! **変換中は一切確定処理を行わない**。
//! また変換を確定する `Enter` と、コマンドを確定する `Enter` は別の打鍵になる
//! （変換中は winit がキー入力イベントを送らないため、自然にそうなる）。

pub mod coord;

use std::collections::VecDeque;

use crate::tools::{self, CommandSpec};

/// 履歴に残す行数。
const HISTORY_LIMIT: usize = 200;
/// 画面に見せる履歴の行数。
const HISTORY_VISIBLE_ROWS: f32 = 10.0;

/// 履歴 1 行の種別。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    /// ユーザーが入力した内容。
    Input,
    /// コマンドからの案内。
    Info,
    /// エラー。
    Error,
}

/// 履歴 1 行。
#[derive(Clone, Debug)]
pub struct HistoryLine {
    pub kind: LineKind,
    pub text: String,
}

/// このフレームでユーザーが行った確定操作。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Submission {
    /// 何も起きていない。
    None,
    /// 文字列が確定された（コマンド名・座標・オプションのいずれか）。
    Text(String),
    /// 空の状態で確定された。直前のコマンドを再実行する合図。
    Empty,
    /// `Esc` が押された。
    Cancel,
}

/// コマンド候補の一覧と選択状態。
///
/// 入力が変わるたびに [`Self::update`] で作り直す。
#[derive(Debug, Default)]
struct Suggestions {
    items: Vec<&'static CommandSpec>,
    /// 明示的に選ばれている候補。`None` なら未選択。
    ///
    /// 未選択のときに `Enter` を押すと、候補ではなく入力文字列がそのまま確定される。
    /// 「打った通りに実行される」のが既定で、候補選択は明示的な操作にする。
    selected: Option<usize>,
}

impl Suggestions {
    /// 入力に合わせて候補を作り直す。
    ///
    /// 候補の顔ぶれが変わったら選択を解除する。選択位置だけ残ると、
    /// 別のコマンドを選んだつもりになる事故が起きる。
    fn update(&mut self, input: &str) {
        let next = tools::suggestions(input);
        let changed = next.len() != self.items.len()
            || next
                .iter()
                .zip(&self.items)
                .any(|(a, b)| !std::ptr::eq(*a, *b));
        if changed {
            self.selected = None;
        }
        self.items = next;
        // 件数が減って選択が範囲外になった場合の保険。
        if self.selected.is_some_and(|i| i >= self.items.len()) {
            self.selected = None;
        }
    }

    fn clear(&mut self) {
        self.items.clear();
        self.selected = None;
    }

    fn is_visible(&self) -> bool {
        !self.items.is_empty()
    }

    /// 選択を上下に動かす。端では未選択へ戻る（一覧から抜けられるように）。
    fn move_selection(&mut self, delta: i32) {
        if self.items.is_empty() {
            return;
        }
        let last = self.items.len() - 1;
        self.selected = match (self.selected, delta) {
            (None, d) if d > 0 => Some(0),
            (None, _) => Some(last),
            (Some(i), d) if d > 0 => (i < last).then_some(i + 1),
            (Some(i), _) => (i > 0).then(|| i - 1),
        };
    }

    /// 明示的に選ばれている候補の名前。
    fn selected_name(&self) -> Option<String> {
        self.items.get(self.selected?).map(|c| c.name.to_owned())
    }

    /// `Tab` で補完する名前。未選択なら先頭候補。
    fn completion(&self) -> Option<String> {
        let index = self.selected.unwrap_or(0);
        self.items.get(index).map(|c| c.name.to_owned())
    }
}

/// コマンドラインの状態。
#[derive(Debug)]
pub struct CommandLine {
    /// 入力中の文字列。**変換中は未確定文字列を含む**ので、
    /// `composing` が真の間は解釈してはいけない。
    input: String,
    history: VecDeque<HistoryLine>,
    /// 直前に実行したコマンド名（空 Enter による再実行用）。
    last_command: Option<String>,
    /// IME で変換中か。
    composing: bool,
    /// コマンド候補。
    suggestions: Suggestions,
}

impl Default for CommandLine {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandLine {
    /// 空の状態で作る。
    #[must_use]
    pub fn new() -> Self {
        Self {
            input: String::new(),
            history: VecDeque::new(),
            last_command: None,
            composing: false,
            suggestions: Suggestions::default(),
        }
    }

    /// 直前に実行したコマンド名。
    #[must_use]
    pub fn last_command(&self) -> Option<&str> {
        self.last_command.as_deref()
    }

    /// 直前のコマンド名を覚える。
    pub fn remember_command(&mut self, name: impl Into<String>) {
        self.last_command = Some(name.into());
    }

    /// 履歴へ 1 行足す。
    pub fn push_line(&mut self, kind: LineKind, text: impl Into<String>) {
        self.history.push_back(HistoryLine {
            kind,
            text: text.into(),
        });
        while self.history.len() > HISTORY_LIMIT {
            self.history.pop_front();
        }
    }

    /// 案内を表示する。
    pub fn info(&mut self, text: impl Into<String>) {
        self.push_line(LineKind::Info, text);
    }

    /// エラーを表示する。
    pub fn error(&mut self, text: impl Into<String>) {
        self.push_line(LineKind::Error, text);
    }

    /// 入力欄を空にする。
    pub fn clear_input(&mut self) {
        self.input.clear();
    }

    /// 履歴（古い順）。
    pub fn history(&self) -> impl Iterator<Item = &HistoryLine> {
        self.history.iter()
    }

    /// このフレームの IME イベントを反映する。
    ///
    /// `TextEdit` を描画する前に呼ぶこと。
    fn track_ime(&mut self, ui: &egui::Ui) {
        ui.input(|i| {
            for ev in &i.events {
                if let egui::Event::Ime(ime) = ev {
                    match ime {
                        // 空の Preedit は変換の取り消しを意味する。
                        egui::ImeEvent::Preedit { text, .. } => self.composing = !text.is_empty(),
                        egui::ImeEvent::Commit(_) => self.composing = false,
                        _ => {}
                    }
                }
            }
        });
    }

    /// コマンドラインを描画し、確定操作を返す。
    ///
    /// - `prompt` … 実行中コマンドの案内（例: `線分の始点を指定:`）
    /// - `allow_suggestions` … コマンド候補を出してよいか。
    ///   ツール実行中や選択待ち中は座標やオプションを打っている段階なので `false` を渡す
    pub fn show(&mut self, ui: &mut egui::Ui, prompt: &str, allow_suggestions: bool) -> Submission {
        self.track_ime(ui);
        self.refresh_suggestions(allow_suggestions);

        // 変換中はキーを一切奪わない。IME に確定させるのが先。
        // 候補の操作キーもこのブロックの中にあるので、変換中は自動的に無効になる。
        let submitted = if self.composing {
            None
        } else {
            ui.input_mut(|i| self.consume_keys(i))
        };

        self.show_history(ui);
        self.show_suggestions(ui);

        ui.horizontal(|ui| {
            ui.monospace(prompt);
            // 変換中は確定処理を止めているので、その旨をユーザーに見せる。
            if self.composing {
                ui.colored_label(
                    egui::Color32::from_rgb(0xff, 0xc1, 0x07),
                    egui::RichText::new("[変換中]").monospace(),
                );
            }
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.input)
                    .desired_width(f32::INFINITY)
                    .font(egui::TextStyle::Monospace),
            );
            // キー入力が常にコマンドラインへ流れるよう、他に入力先が無ければ
            // 毎フレーム自分にフォーカスを戻す。
            if ui.memory(|m| m.focused().is_none()) {
                response.request_focus();
            }
        });

        match submitted {
            Some(Submission::Cancel) => {
                self.input.clear();
                self.suggestions.clear();
                Submission::Cancel
            }
            Some(Submission::Text(_)) => {
                // 候補が明示的に選ばれていれば、入力文字列ではなくその名前を確定する。
                let text = self
                    .suggestions
                    .selected_name()
                    .unwrap_or_else(|| self.input.trim().to_owned());
                self.input.clear();
                self.suggestions.clear();
                if text.is_empty() {
                    Submission::Empty
                } else {
                    Submission::Text(text)
                }
            }
            _ => Submission::None,
        }
    }

    /// 入力に合わせて候補を作り直す。
    ///
    /// 変換中は入力欄に未確定文字列が入っているので触らない（ADR-0002）。
    fn refresh_suggestions(&mut self, allow: bool) {
        if self.composing {
            return;
        }
        if !allow {
            self.suggestions.clear();
            return;
        }
        self.suggestions.update(&self.input);
    }

    /// このフレームのキー入力を消費し、確定操作があれば返す。
    ///
    /// 候補の操作キー（`Tab` / `↑` / `↓`）は `TextEdit` を描く前に奪う。
    /// あとから処理すると `TextEdit` にカーソル移動として取られてしまう。
    fn consume_keys(&mut self, i: &mut egui::InputState) -> Option<Submission> {
        const NONE: egui::Modifiers = egui::Modifiers::NONE;

        if i.consume_key(NONE, egui::Key::Escape) {
            // 候補が出ていれば、まず候補だけを閉じる。
            // いきなりコマンドを中断すると、打ち間違いのやり直しが面倒になる。
            if self.suggestions.is_visible() {
                self.suggestions.clear();
                return None;
            }
            return Some(Submission::Cancel);
        }

        if self.suggestions.is_visible() {
            if i.consume_key(NONE, egui::Key::ArrowDown) {
                self.suggestions.move_selection(1);
                return None;
            }
            if i.consume_key(NONE, egui::Key::ArrowUp) {
                self.suggestions.move_selection(-1);
                return None;
            }
            if i.consume_key(NONE, egui::Key::Tab) {
                // 選択中（未選択なら先頭）の候補を入力欄へ入れる。
                if let Some(name) = self.suggestions.completion() {
                    self.input = name;
                    self.suggestions.update(&self.input);
                }
                return None;
            }
        }

        // AutoCAD では Space も Enter と同じく確定として働く。
        let enter = i.consume_key(NONE, egui::Key::Enter) || i.consume_key(NONE, egui::Key::Space);
        enter.then(|| Submission::Text(String::new())) // 中身は呼び出し側で詰める
    }

    /// 候補一覧を描く。
    fn show_suggestions(&self, ui: &mut egui::Ui) {
        if !self.suggestions.is_visible() {
            return;
        }
        egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::symmetric(6, 3))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                for (index, spec) in self.suggestions.items.iter().enumerate() {
                    let selected = self.suggestions.selected == Some(index);
                    let name_color = if selected {
                        ui.visuals().strong_text_color()
                    } else {
                        ui.visuals().text_color()
                    };
                    let row = ui.horizontal(|ui| {
                        ui.monospace(
                            egui::RichText::new(format!("{:<10}", spec.name)).color(name_color),
                        );
                        let alias = spec.alias_text();
                        ui.monospace(
                            egui::RichText::new(format!("{alias:<10}"))
                                .color(ui.visuals().weak_text_color()),
                        );
                        ui.monospace(
                            egui::RichText::new(spec.summary).color(ui.visuals().weak_text_color()),
                        );
                    });
                    if selected {
                        // 選択行は背景を敷いて分かるようにする。
                        ui.painter().rect_filled(
                            row.response.rect.expand(1.0),
                            2.0,
                            ui.visuals().selection.bg_fill.gamma_multiply(0.5),
                        );
                    }
                }
                ui.monospace(
                    egui::RichText::new("↑↓ 選択  Tab 補完  Enter 実行")
                        .small()
                        .color(ui.visuals().weak_text_color()),
                );
            });
    }

    fn show_history(&self, ui: &mut egui::Ui) {
        let row_height = ui.text_style_height(&egui::TextStyle::Monospace);
        egui::ScrollArea::vertical()
            .stick_to_bottom(true)
            .max_height(row_height * HISTORY_VISIBLE_ROWS)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for line in self.history() {
                    let color = match line.kind {
                        LineKind::Input => ui.visuals().text_color(),
                        LineKind::Info => ui.visuals().weak_text_color(),
                        LineKind::Error => egui::Color32::from_rgb(0xff, 0x70, 0x43),
                    };
                    ui.colored_label(color, egui::RichText::new(&line.text).monospace());
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_is_capped() {
        let mut c = CommandLine::new();
        for i in 0..(HISTORY_LIMIT + 50) {
            c.info(format!("line {i}"));
        }
        assert_eq!(c.history().count(), HISTORY_LIMIT);
        // 古い方から捨てられること。
        assert_eq!(c.history().next().unwrap().text, "line 50");
    }

    #[test]
    fn remembers_last_command() {
        let mut c = CommandLine::new();
        assert!(c.last_command().is_none());
        c.remember_command("LINE");
        assert_eq!(c.last_command(), Some("LINE"));
        c.remember_command("CIRCLE");
        assert_eq!(c.last_command(), Some("CIRCLE"));
    }

    #[test]
    fn line_kinds_are_recorded() {
        let mut c = CommandLine::new();
        c.info("案内");
        c.error("失敗");
        let kinds: Vec<_> = c.history().map(|l| l.kind).collect();
        assert_eq!(kinds, vec![LineKind::Info, LineKind::Error]);
    }
}
