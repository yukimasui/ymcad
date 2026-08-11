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
    /// `prompt` は実行中コマンドの案内（例: `線分の始点を指定:`）。
    pub fn show(&mut self, ui: &mut egui::Ui, prompt: &str) -> Submission {
        self.track_ime(ui);

        // 変換中はキーを一切奪わない。IME に確定させるのが先。
        let submitted = if self.composing {
            None
        } else {
            ui.input_mut(|i| {
                let esc = i.consume_key(egui::Modifiers::NONE, egui::Key::Escape);
                // AutoCAD では Space も Enter と同じく確定として働く。
                let enter = i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
                    || i.consume_key(egui::Modifiers::NONE, egui::Key::Space);
                if esc {
                    Some(Submission::Cancel)
                } else if enter {
                    Some(Submission::Text(String::new())) // 中身は後で詰める
                } else {
                    None
                }
            })
        };

        self.show_history(ui);

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
                Submission::Cancel
            }
            Some(Submission::Text(_)) => {
                let text = self.input.trim().to_owned();
                self.input.clear();
                if text.is_empty() {
                    Submission::Empty
                } else {
                    Submission::Text(text)
                }
            }
            _ => Submission::None,
        }
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
