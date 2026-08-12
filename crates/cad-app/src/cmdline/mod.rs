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
    /// `↑` `↓` で明示的に選ばれている候補。`None` なら未選択。
    ///
    /// 未選択でも `Enter` は候補を実行する（[`Self::effective_index`] 参照）。
    /// これは選択の由来を区別するためだけの状態で、
    /// 「実行されるのはどれか」とは別物。
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

    /// `Enter` で実際に実行される候補の位置。
    ///
    /// 優先順位:
    ///
    /// 1. `↑` `↓` で明示的に選ばれていればそれ
    /// 2. 入力と完全一致する候補があればそれ
    /// 3. どちらでもなければ先頭候補
    ///
    /// **2 を挟むのが肝。** 候補の並び順は「エイリアス完全一致 → 名前の先頭一致 → …」
    /// なので今は 3 でも同じ結果になるが、`COMMANDS` の並びを変えた瞬間に壊れる。
    /// 完全一致を明示的に優先しておけば並び順に依存しない。
    fn effective_index(&self, input: &str) -> Option<usize> {
        if self.items.is_empty() {
            return None;
        }
        if let Some(index) = self.selected {
            return Some(index);
        }
        let upper = input.trim().to_uppercase();
        let exact = self
            .items
            .iter()
            .position(|c| c.name == upper || c.aliases.contains(&upper.as_str()));
        Some(exact.unwrap_or(0))
    }

    /// `Enter` で実際に実行される候補の名前。
    fn effective_name(&self, input: &str) -> Option<String> {
        self.items
            .get(self.effective_index(input)?)
            .map(|c| c.name.to_owned())
    }

    /// `Tab` で補完する名前。実行される候補と同じものを入れる。
    fn completion(&self, input: &str) -> Option<String> {
        self.effective_name(input)
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
                // 候補が出ていればそれを実行する。候補が無いときだけ入力文字列を使う。
                // 未選択でも先頭候補が実行されるので、`L` + Enter で LINE が起動する。
                let text = self
                    .suggestions
                    .effective_name(&self.input)
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
                // Enter で実行される候補をそのまま入力欄へ入れる。
                if let Some(name) = self.suggestions.completion(&self.input) {
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
    ///
    /// **`Enter` で実行される行が必ず分かる**ようにするのが目的。
    /// 未選択でも先頭候補が実行されるので、そのことが見えていないと
    /// 打ち間違いで意図しないコマンドが走ったときに原因が分からない。
    ///
    /// 目印は行頭の `⏎` と背景の 2 つ。**背景色とは別の視覚チャンネル**を併用するので、
    /// テーマや配色に関わらず読み取れる。
    fn show_suggestions(&self, ui: &mut egui::Ui) {
        if !self.suggestions.is_visible() {
            return;
        }
        let effective = self.suggestions.effective_index(&self.input);

        egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::symmetric(6, 3))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                for (index, spec) in self.suggestions.items.iter().enumerate() {
                    let runs = effective == Some(index);
                    let picked = self.suggestions.selected == Some(index);

                    // 明示的に選んだ行は濃く、既定で選ばれている行は薄く敷く。
                    // Frame の fill なので背景が文字の下に来る
                    // （行を描いた後に rect_filled すると文字の上に乗ってしまう）。
                    let fill = if picked {
                        ui.visuals().selection.bg_fill
                    } else if runs {
                        ui.visuals().selection.bg_fill.gamma_multiply(0.35)
                    } else {
                        egui::Color32::TRANSPARENT
                    };

                    egui::Frame::new()
                        .fill(fill)
                        .corner_radius(2)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let name_color = if runs {
                                    ui.visuals().strong_text_color()
                                } else {
                                    ui.visuals().text_color()
                                };
                                let weak = ui.visuals().weak_text_color();

                                // Enter で走る行の目印。幅を固定して桁が揃うようにする。
                                ui.monospace(
                                    egui::RichText::new(if runs { "⏎ " } else { "  " })
                                        .color(name_color),
                                );
                                ui.monospace(
                                    egui::RichText::new(format!("{:<10}", spec.name))
                                        .color(name_color),
                                );
                                let alias = spec.alias_text();
                                ui.monospace(
                                    egui::RichText::new(format!("{alias:<10}")).color(weak),
                                );
                                ui.monospace(egui::RichText::new(spec.summary).color(weak));
                            });
                        });
                }
                ui.monospace(
                    egui::RichText::new("Enter で ⏎ の行を実行  ↑↓ 選択  Tab 補完")
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

    /// 候補を作った状態を用意する。
    fn suggestions_for(input: &str) -> Suggestions {
        let mut s = Suggestions::default();
        s.update(input);
        s
    }

    fn effective(input: &str) -> Option<&'static str> {
        let s = suggestions_for(input);
        s.effective_index(input).map(|i| s.items[i].name)
    }

    /// **Issue #5 の本体。** 未選択でも先頭候補が実行対象になること。
    #[test]
    fn unselected_enter_runs_the_top_suggestion() {
        assert_eq!(effective("L"), Some("LINE"));
        assert_eq!(effective("REC"), Some("RECTANGLE"));
    }

    /// 完全一致は先頭候補より優先されること。
    ///
    /// `SAVE` は `SAVEAS` の接頭辞でもあるので、並び順に関わらず SAVE が走ってほしい。
    #[test]
    fn exact_match_wins_over_the_first_row() {
        let s = suggestions_for("SAVE");
        assert!(
            s.items.len() >= 2,
            "前提: SAVE と SAVEAS が候補に出る（実際: {:?}）",
            s.items.iter().map(|c| c.name).collect::<Vec<_>>()
        );
        assert_eq!(effective("SAVE"), Some("SAVE"));
    }

    /// 完全一致の優先が候補の並び順に依存していないこと。
    ///
    /// 先頭以外の位置に完全一致があっても、そちらが選ばれる。
    #[test]
    fn exact_match_is_found_regardless_of_position() {
        let mut s = suggestions_for("S");
        assert!(s.items.len() >= 2, "前提: 候補が複数ある");

        // 完全一致（エイリアス "S" を持つ STRETCH）をわざと末尾へ動かす。
        let pos = s
            .items
            .iter()
            .position(|c| c.aliases.contains(&"S"))
            .expect("S を持つコマンドがあるはず");
        let spec = s.items.remove(pos);
        let name = spec.name;
        s.items.push(spec);

        let index = s.effective_index("S").unwrap();
        assert_eq!(s.items[index].name, name, "並び順に関わらず完全一致が勝つ");
        assert_eq!(index, s.items.len() - 1, "末尾に置いたものが選ばれている");
    }

    /// 明示的な選択がすべてに優先すること。
    #[test]
    fn explicit_selection_wins_over_exact_match() {
        let mut s = suggestions_for("SAVE");
        assert!(s.items.len() >= 2, "前提: 候補が複数ある");
        s.selected = Some(1);
        assert_eq!(s.effective_index("SAVE"), Some(1));
    }

    #[test]
    fn no_suggestions_means_no_effective_row() {
        let s = suggestions_for("XYZZY");
        assert!(!s.is_visible());
        assert_eq!(s.effective_index("XYZZY"), None);
        assert_eq!(s.effective_name("XYZZY"), None);
    }

    /// 上下キーで選択が動き、端では未選択へ戻ること。
    #[test]
    fn move_selection_falls_off_at_the_ends() {
        let mut s = suggestions_for("S");
        let last = s.items.len() - 1;

        s.move_selection(1);
        assert_eq!(s.selected, Some(0));
        s.move_selection(-1);
        assert_eq!(s.selected, None, "先頭から上へ抜けると未選択");

        s.move_selection(-1);
        assert_eq!(s.selected, Some(last), "未選択から上へ行くと末尾");
        s.move_selection(1);
        assert_eq!(s.selected, None, "末尾から下へ抜けると未選択");
    }

    /// 候補の顔ぶれが変わったら選択を解除すること。
    /// 位置だけ残ると別のコマンドを選んだつもりになる。
    #[test]
    fn changing_the_input_clears_the_selection() {
        let mut s = suggestions_for("S");
        s.selected = Some(1);
        s.update("L");
        assert_eq!(s.selected, None);
    }

    /// Tab の補完先は Enter で実行される候補と一致すること。
    /// 補完した結果と実行される内容が食い違うと混乱する。
    #[test]
    fn tab_completion_matches_what_enter_runs() {
        for input in ["L", "REC", "SAVE", "C"] {
            let s = suggestions_for(input);
            assert_eq!(
                s.completion(input),
                s.effective_name(input),
                "入力 {input:?}"
            );
        }
    }
}
