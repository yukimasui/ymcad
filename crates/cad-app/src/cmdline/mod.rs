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
pub mod dynamic;

use std::collections::VecDeque;
use std::time::Duration;

use self::dynamic::{Activity, Bounds, Point, Size};

use crate::tools::{self, CommandSpec};

/// 履歴に残す行数。
const HISTORY_LIMIT: usize = 200;
/// 画面に見せる履歴の行数。
const HISTORY_VISIBLE_ROWS: f32 = 10.0;
/// 入力欄の `TextEdit` の ID。
const INPUT_ID: &str = "ymcad_cmdline_input";
/// `[変換中]` の色。
const COMPOSING_COLOR: egui::Color32 = egui::Color32::from_rgb(0xff, 0xc1, 0x07);
/// エラーの色。
const ERROR_COLOR: egui::Color32 = egui::Color32::from_rgb(0xff, 0x70, 0x43);
/// カーソル横の `Area` の ID。
const DYN_AREA_ID: &str = "ymcad_dyn_input";
/// カーソル横の入力欄の幅 [px]。
const DYN_INPUT_WIDTH: f32 = 240.0;
/// カーソル横の表示の最大幅 [px]。長いエラーやプロンプトはここで折り返す。
const DYN_MAX_WIDTH: f32 = 480.0;
/// カーソル横の大きさの見積もり [px]。前フレームの実寸がまだ無いときだけ使う。
const DYN_SIZE_ESTIMATE: Size = Size { w: 260.0, h: 40.0 };
/// カーソル横の背景の不透明度。下の図形が透けて見える程度にする。
const DYN_BACKGROUND_OPACITY: f32 = 0.8;

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
    /// `Esc` で閉じたときの入力。入力がこれと同じ間は候補を出さない。
    ///
    /// 候補は毎フレーム入力から作り直すので、閉じた状態を覚えておかないと
    /// 次のフレームで同じ候補がまた出る。そうなると `Esc` の 2 回目も
    /// 「候補を閉じる」に化けて、いつまでも中断できない。
    dismissed_for: Option<String>,
}

impl Suggestions {
    /// 入力に合わせて候補を作り直す。
    ///
    /// 候補の顔ぶれが変わったら選択を解除する。選択位置だけ残ると、
    /// 別のコマンドを選んだつもりになる事故が起きる。
    fn update(&mut self, input: &str) {
        if self.dismissed_for.as_deref() == Some(input) {
            self.clear();
            return;
        }
        self.dismissed_for = None;
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

    /// `Esc` で閉じる。入力が変わるまで出さない。
    fn dismiss(&mut self, input: &str) {
        self.clear();
        self.dismissed_for = Some(input.to_owned());
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
    /// [`Self::begin_frame`] で消費したキーが表す確定操作。
    /// [`Self::finish_frame`] で中身を詰めて返す。
    pending: Option<Submission>,
    /// カーソル横の動的入力の状態。
    dynamic: DynamicInput,
    /// カーソル横に出す直近のエラー。
    recent_error: Option<RecentError>,
}

/// カーソル横に出す直近のエラー。
#[derive(Debug)]
struct RecentError {
    text: String,
    /// 最初に表示した時刻（`egui::InputState::time`）。
    ///
    /// [`CommandLine::error`] は時刻を知らない場所からも呼ばれるので、
    /// 描画の側で初めて見たときに埋める。
    shown_at: Option<f64>,
}

/// カーソル横の動的入力（Issue #20 段階 A）の状態。
#[derive(Debug)]
struct DynamicInput {
    /// オンか。既定はオン。F12 / ステータスバーで切り替える。
    enabled: bool,
    /// このフレームの描画に使うオン/オフ。
    ///
    /// [`CommandLine::begin_frame`] で `enabled` を写し、フレームの途中では変えない。
    /// F12 やステータスバーで途中に切り替わると、画面下とカーソル横の両方に
    /// 入力欄を描いて（または両方とも描かずに）確定操作を二重に処理しかねないため。
    frame_enabled: bool,
    /// 基準にするマウス位置。キャンバスの外にいる間は最後の位置のまま。
    anchor: Option<egui::Pos2>,
    /// 直前のフレームで置いた位置。変換が始まったときの固定先になる。
    previous: Option<Point>,
    /// 変換中に固定している位置。
    frozen: Option<Point>,
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
            pending: None,
            dynamic: DynamicInput {
                enabled: true,
                frame_enabled: true,
                anchor: None,
                previous: None,
                frozen: None,
            },
            recent_error: None,
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
    ///
    /// 履歴には必ず残す。動的入力がオンならカーソル横にも数秒出す。
    pub fn error(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.recent_error = Some(RecentError {
            text: text.clone(),
            shown_at: None,
        });
        self.push_line(LineKind::Error, text);
    }

    /// 動的入力（カーソル横の入力欄）がオンか。
    #[must_use]
    pub fn is_dynamic(&self) -> bool {
        self.dynamic.enabled
    }

    /// 動的入力のオン/オフを切り替え、切り替え後の状態を返す。
    ///
    /// 描く場所が変わるのは次のフレームから（[`DynamicInput::frame_enabled`]）。
    pub fn toggle_dynamic(&mut self) -> bool {
        self.dynamic.enabled = !self.dynamic.enabled;
        self.dynamic.enabled
    }

    /// カーソル横の基準にするマウス位置を伝える。キャンバス上にあるときだけ呼ぶ。
    pub fn set_cursor_anchor(&mut self, pos: egui::Pos2) {
        self.dynamic.anchor = Some(pos);
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
    fn track_ime(&mut self, ctx: &egui::Context) {
        ctx.input(|i| {
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

    /// フレームの最初に呼ぶ。IME の状態を追い、キー入力を消費する。
    ///
    /// **入力欄（`TextEdit`）を描くより前に呼ぶこと。** 候補の操作キーや
    /// `Enter` を先に奪わないと、`TextEdit` にカーソル移動や確定として取られる。
    ///
    /// 確定操作はここでは決めず、入力欄を描いたあとの [`Self::finish_frame`] で決める。
    /// 同じフレームで打たれた文字が入力欄に入ってから確定させるため
    /// （分ける前の `show` と同じ順序）。
    ///
    /// - `allow_suggestions` … コマンド候補を出してよいか。
    ///   ツール実行中や選択待ち中は座標やオプションを打っている段階なので `false` を渡す
    pub fn begin_frame(&mut self, ctx: &egui::Context, allow_suggestions: bool) {
        self.dynamic.frame_enabled = self.dynamic.enabled;
        let now = ctx.input(|i| i.time);
        if let Some(e) = &mut self.recent_error {
            e.shown_at.get_or_insert(now);
        }
        self.track_ime(ctx);
        self.refresh_suggestions(allow_suggestions);

        // 変換中はキーを一切奪わない。IME に確定させるのが先。
        // 候補の操作キーもこのブロックの中にあるので、変換中は自動的に無効になる。
        let pending = if self.composing {
            None
        } else {
            ctx.input_mut(|i| self.consume_keys(i))
        };
        self.pending = pending;
    }

    /// このフレームで動的入力がオンか（描く場所の判断に使う）。
    #[must_use]
    pub fn frame_is_dynamic(&self) -> bool {
        self.dynamic.frame_enabled
    }

    /// 画面下のコマンドラインを描く。
    ///
    /// - 動的入力がオフ … 履歴・候補・プロンプト・入力欄（従来どおり）
    /// - 動的入力がオン … 履歴と、読み取り専用のプロンプト 1 行だけ。
    ///   入力欄と候補はカーソル横（[`Self::show_floating`]）に出る
    ///
    /// `prompt` … 実行中コマンドの案内（例: `線分の始点を指定:`）
    pub fn show_bottom(&mut self, ui: &mut egui::Ui, prompt: &str) {
        self.show_history(ui);
        if self.dynamic.frame_enabled {
            // プロンプトは下にも残す。カーソル横を見落としても、履歴の続きとして読める。
            ui.horizontal(|ui| {
                ui.monospace(prompt);
                ui.weak(egui::RichText::new("（入力はカーソル横  F12 で切替）").small());
            });
            return;
        }
        self.show_suggestions(ui);

        ui.horizontal(|ui| {
            ui.monospace(prompt);
            self.show_composing_badge(ui);
            self.show_input(ui, f32::INFINITY);
        });
    }

    /// 変換中は確定処理を止めているので、その旨をユーザーに見せる。
    fn show_composing_badge(&self, ui: &mut egui::Ui) {
        if self.composing {
            ui.colored_label(COMPOSING_COLOR, egui::RichText::new("[変換中]").monospace());
        }
    }

    /// 入力欄を描く。**バッファも `TextEdit` の ID も 1 つだけ。**
    ///
    /// ID を固定しておくと、描く場所（画面下 / カーソル横）を切り替えても
    /// フォーカスとキャレットの位置が引き継がれる。
    fn show_input(&mut self, ui: &mut egui::Ui, width: f32) {
        let response = ui.add(
            egui::TextEdit::singleline(&mut self.input)
                .id(egui::Id::new(INPUT_ID))
                .desired_width(width)
                .font(egui::TextStyle::Monospace),
        );
        // キー入力が常にコマンドラインへ流れるよう、他に入力先が無ければ
        // 毎フレーム自分にフォーカスを戻す。
        if ui.memory(|m| m.focused().is_none()) {
            response.request_focus();
        }
    }

    /// カーソル横に入力欄を描く（動的入力がオンのときだけ）。
    ///
    /// - `canvas` … キャンバス（ビューポート）の矩形。この中に収める
    /// - `tool_active` … コマンドを実行中か（選択待ちを含む）
    ///
    /// **何もしていないときも入力欄は毎フレーム描く。** egui は描かれなかった
    /// ウィジェットのフォーカスを外すので、描くのをやめるとキー入力が流れなくなる。
    /// そのときは不透明度 0 で描いて見た目だけ消す。
    /// `set_invisible` は使わない。`TextEdit` は「見えている」ときしか IME へ
    /// 入力欄の位置を伝えない（`PlatformOutput::ime`）ので、変換が始まらなくなる。
    pub fn show_floating(
        &mut self,
        ctx: &egui::Context,
        prompt: &str,
        canvas: egui::Rect,
        tool_active: bool,
    ) {
        if !self.dynamic.frame_enabled {
            return;
        }

        let now = ctx.input(|i| i.time);
        let error_remaining = self.recent_error.as_mut().and_then(|e| {
            let shown_at = *e.shown_at.get_or_insert(now);
            dynamic::error_remaining(shown_at, now)
        });
        if let Some(remaining) = error_remaining {
            // 期限が来たら消えるように再描画を予約する。
            ctx.request_repaint_after(Duration::from_secs_f64(remaining));
        }
        let visible = dynamic::is_visible(Activity {
            tool_active,
            has_input: !self.input.is_empty(),
            composing: self.composing,
            error_shown: error_remaining.is_some(),
        });

        let area_id = egui::Id::new(DYN_AREA_ID);
        // 大きさは前フレームの実寸を使う。中身で大きさが変わるので、このフレームの
        // 大きさは描き終わるまで分からない。
        let size = ctx
            .memory(|m| m.area_rect(area_id))
            .map_or(DYN_SIZE_ESTIMATE, |r| Size {
                w: r.width(),
                h: r.height(),
            });
        let bounds = Bounds {
            left: canvas.left(),
            top: canvas.top(),
            right: canvas.right(),
            bottom: canvas.bottom(),
        };
        let anchor = self
            .dynamic
            .anchor
            .map_or_else(|| bounds.center(), |p| Point { x: p.x, y: p.y });
        let computed = dynamic::place(anchor, size, bounds);
        let pos = dynamic::resolve_position(
            self.composing,
            &mut self.dynamic.frozen,
            self.dynamic.previous,
            computed,
        );
        self.dynamic.previous = Some(pos);

        egui::Area::new(area_id)
            .order(egui::Order::Foreground)
            .fixed_pos(egui::pos2(pos.x, pos.y))
            .constrain_to(canvas)
            // クリックを素通しする。キャンバスの点の指定やホバーを奪わないように。
            // キー入力はフォーカスで届くので、ここを切っても入力欄は動く。
            .interactable(false)
            .fade_in(false)
            .show(ctx, |ui| {
                if !visible {
                    ui.multiply_opacity(0.0);
                    self.show_input(ui, DYN_INPUT_WIDTH);
                    return;
                }
                // Area の中身の最大幅は前フレームの大きさになっているので、
                // 広げないと候補やエラーが前フレームの幅で折り返される。
                ui.set_max_width(DYN_MAX_WIDTH);
                let fill = ui
                    .visuals()
                    .window_fill
                    .gamma_multiply(DYN_BACKGROUND_OPACITY);
                let stroke = ui.visuals().window_stroke;
                egui::Frame::new()
                    .fill(fill)
                    .stroke(stroke)
                    .corner_radius(4)
                    .inner_margin(egui::Margin::same(6))
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(prompt)
                                .small()
                                .color(ui.visuals().strong_text_color()),
                        );
                        ui.horizontal(|ui| {
                            self.show_composing_badge(ui);
                            self.show_input(ui, DYN_INPUT_WIDTH);
                        });
                        self.show_suggestions(ui);
                        if let (Some(_), Some(e)) = (error_remaining, &self.recent_error) {
                            ui.colored_label(ERROR_COLOR, egui::RichText::new(&e.text).monospace());
                        }
                    });
            });
    }

    /// 入力欄を描いたあとに呼び、このフレームの確定操作を返す。
    pub fn finish_frame(&mut self) -> Submission {
        let submission = self.resolve_pending();
        if submission != Submission::None {
            // 次の操作に移ったので、前のエラーはカーソル横から下げる（履歴には残る）。
            // この後の処理で新しいエラーが出れば、それが改めて出る。
            self.recent_error = None;
        }
        submission
    }

    fn resolve_pending(&mut self) -> Submission {
        match self.pending.take() {
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
                self.suggestions.dismiss(&self.input);
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
                        LineKind::Error => ERROR_COLOR,
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

    /// `Esc` で閉じた候補は、入力が同じ間は作り直しても出ないこと。
    /// 入力が変われば再び出ること。
    #[test]
    fn dismissed_suggestions_stay_closed_until_the_input_changes() {
        let mut s = suggestions_for("L");
        assert!(s.is_visible(), "前提");
        s.dismiss("L");
        s.update("L");
        assert!(!s.is_visible(), "同じ入力では出ない");
        s.update("LA");
        assert!(s.is_visible(), "入力が変われば出る");
        s.update("L");
        assert!(s.is_visible(), "一度変わったら閉じた記憶は消える");
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
