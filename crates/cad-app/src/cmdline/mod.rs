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
pub mod dimension;
pub mod dynamic;

use std::collections::VecDeque;
use std::time::Duration;

use cad_core::geom::Point2;

use self::dimension::{DimKind, DimState, DimValues, Field, Live, TabOutcome};
use self::dynamic::{Activity, Bounds, Point, Size};

use crate::tools::{self, CommandSpec};

/// 履歴に残す行数。
const HISTORY_LIMIT: usize = 200;
/// 画面に見せる履歴の行数。
const HISTORY_VISIBLE_ROWS: f32 = 10.0;
/// 入力欄の `TextEdit` の ID。
const INPUT_ID: &str = "ymcad_cmdline_input";
/// 変換中の目印。
const COMPOSING_BADGE: &str = "[変換中]";
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
/// 寸法入力の欄 1 つの幅 [px]。
const DIM_FIELD_WIDTH: f32 = 110.0;
/// 固定した欄の目印（錠前）と枠の色。
const LOCK_COLOR: egui::Color32 = egui::Color32::from_rgb(0xff, 0xc1, 0x07);
/// 錠前を描く正方形の一辺 [px]。形は 16 単位の格子で定義し、この大きさへ拡大縮小する。
const LOCK_SIZE: f32 = 16.0;
/// 動的入力オフで、寸法入力に参加中のツールの Tab を押したときの案内。
pub const TAB_NEEDS_DYNAMIC: &str = "長さ・角度の固定は動的入力（F12）がオンのときに使えます";

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

/// コマンドラインがこのフレームのキー（Enter / Space / Esc / Tab / ↑↓）を扱ってよいか。
///
/// フォーカスが入力欄にあるか、どこにも無いときだけ扱う。それ以外
/// （レイヤ名・コンポーネント名などパネルの入力欄）を編集している間に扱うと、
/// レイヤ名の Space や Enter で実行中のコマンドが進み（空 Enter なら直前の
/// コマンドが再実行され）、Esc でコマンドが中断される。
///
/// 前のフレームのフォーカスも見る。パネルの入力欄で押した Esc は egui が
/// フレームの最初にフォーカスを外すので、このフレームの状態だけだと
/// 「どこにも無い」に見えるため。
fn owns_keys(input: egui::Id, last_frame: Option<egui::Id>, now: Option<egui::Id>) -> bool {
    let ours = |f: Option<egui::Id>| f.is_none_or(|id| id == input);
    ours(last_frame) && ours(now)
}

/// このフレームでユーザーが行った確定操作。
#[derive(Clone, Debug, PartialEq)]
pub enum Submission {
    /// 何も起きていない。
    None,
    /// 文字列が確定された（コマンド名・座標・オプションのいずれか）。
    Text(String),
    /// 空の状態で確定された。直前のコマンドを再実行する合図。
    Empty,
    /// `Esc` が押された。
    Cancel,
    /// 寸法入力の欄で確定された（Issue #20 段階 B）。
    ///
    /// 欠けている欄（`None`）はカーソルから決める。点にするのは基点とカーソルを
    /// 知っている `Session` の仕事。
    Dimension(DimValues),
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
            self.hide();
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

    /// 候補を片付け、`Esc` で閉じた記憶も捨てる。
    ///
    /// 確定・中断のほか、コマンド実行中（候補を出さない段階）は毎フレーム呼ばれる。
    /// 記憶を残すと、閉じたときと同じ文字列が入力欄に残ったままコマンドが終わったとき、
    /// 候補が出ないことがある（コマンド実行中は [`Self::update`] が呼ばれないため）。
    fn clear(&mut self) {
        self.hide();
        self.dismissed_for = None;
    }

    /// 候補を見えなくする。`Esc` で閉じた記憶は残す。
    fn hide(&mut self) {
        self.items.clear();
        self.selected = None;
    }

    /// `Esc` で閉じる。入力が変わるまで出さない。
    fn dismiss(&mut self, input: &str) {
        self.hide();
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
    /// 次に入力欄を描くとき、キャレットを末尾へ動かす。
    ///
    /// Tab 補完はバッファを外から書き換えるので、`TextEdit` のキャレットは
    /// 補完前の位置（`L` の直後）に残る。そのまま打つと `LZINE` になる。
    caret_to_end: bool,
    /// 前のフレームの [`Self::begin_frame`] の時点でフォーカスを持っていた部品。
    ///
    /// パネルの入力欄で Esc を押すと、egui はフレームの最初にフォーカスを外す。
    /// その時点の状態だけを見ると「誰もフォーカスを持っていない」になり、
    /// Esc をコマンドラインが拾って実行中のコマンドを中断してしまう。
    focused_last_frame: Option<egui::Id>,
    /// このフレームでモーダルが出ているか。[`Self::begin_frame`] で写す。
    modal_open: bool,
    /// このフレームのキーの持ち主がコマンドラインか（[`Self::owns_keys`]）。[`Self::begin_frame`] で決める。
    keys_owned: bool,
    /// コマンド候補。
    suggestions: Suggestions,
    /// [`Self::begin_frame`] で消費したキーが表す確定操作。
    /// [`Self::finish_frame`] で中身を詰めて返す。
    pending: Option<Submission>,
    /// カーソル横の動的入力の状態。
    dynamic: DynamicInput,
    /// カーソル横に出す直近のエラー。
    recent_error: Option<RecentError>,
    /// 寸法入力（長さ・角度の欄）の状態。
    dim: DimensionInput,
    /// [`Self::begin_frame`] で Ctrl+A を全選択として消費したか（[`Self::take_select_all`]）。
    select_all_requested: bool,
    /// 直近に描いた入力欄の矩形。変換開始で入力欄が動かないことのテストに使う。
    #[cfg(test)]
    input_rect: Option<egui::Rect>,
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

/// 寸法入力（Issue #20 段階 B）の状態。
#[derive(Debug, Default)]
struct DimensionInput {
    /// フレームの最初の時点で、寸法入力に参加しているツールの基点。
    /// キー（Tab / Esc / Enter）の扱いを決めるのに使う。
    base: Option<Point2>,
    /// 欄の見せ方（「長さ」「角度」か「半径」だけか）。
    kind: DimKind,
    /// 入力中の欄と固定した値。
    state: DimState,
    /// 直前のフレームで欄を出したか。
    ///
    /// 変換中はバッファに未確定文字列が入っていて分類できない（ADR-0002）ので、
    /// 変換が始まる前の見た目を保つ。見た目が切り替わると入力欄が動き、
    /// 入力欄に付いて出る IME の候補ウィンドウが跳ねる（ADR-0034 決定 4）。
    fields_shown: bool,
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
            caret_to_end: false,
            focused_last_frame: None,
            modal_open: false,
            keys_owned: false,
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
            dim: DimensionInput::default(),
            select_all_requested: false,
            #[cfg(test)]
            input_rect: None,
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

    /// 寸法入力に参加しているツールの基点を伝える。[`Self::begin_frame`] の前に毎フレーム呼ぶ。
    ///
    /// 基点が変わったら（ツールが次の点へ進んだ・終わった・別のツールになった）
    /// 固定を外す。前の点で固定した長さが次の線分に残ると、気づかずに使ってしまう。
    /// 欄の見せ方 `kind`（「半径」だけ、など）が変わったときも外す。
    pub fn set_dimension_base(&mut self, base: Option<Point2>, kind: DimKind) {
        if self.dim.base != base || self.dim.kind != kind {
            self.dim.state.reset();
        }
        self.dim.base = base;
        self.dim.kind = kind;
    }

    /// このフレームで Ctrl+A が全選択として押されたか。読んだら落とす。
    ///
    /// 全選択そのものは `Session::select_all` がする。コマンドラインはキーを消費して印を立てるだけ。
    /// 効く段階か（待機中・選択待ちか）も `Session::select_all` が決める。点や値の入力中に
    /// 空の入力欄で押された Ctrl+A はここで消費されるが、入力欄が受け取っても空の文字列の
    /// 全選択で何も起きないので、奪って困ることは無い。
    pub fn take_select_all(&mut self) -> bool {
        std::mem::take(&mut self.select_all_requested)
    }

    /// 寸法入力の固定を外し、長さの欄へ戻す。
    ///
    /// ツールが点を受け取ったときに呼ぶ。COPY のように基点が変わらないまま
    /// 次の点へ進むツールがあるので、基点の変化だけでは足りない。
    pub fn reset_dimension(&mut self) {
        self.dim.state.reset();
    }

    /// このフレームで寸法入力の欄を扱うか（キーの扱いを決める）。
    fn dimension_active(&self) -> bool {
        self.dynamic.frame_enabled && self.dim.base.is_some()
    }

    /// 寸法入力の欄が出ているか。変換中は直前の見た目を保つ。
    fn fields_visible(&self, participating: bool) -> bool {
        if self.composing {
            return participating && self.dynamic.frame_enabled && self.dim.fields_shown;
        }
        dimension::shows_fields(
            participating,
            self.dynamic.frame_enabled,
            dimension::classify(&self.input),
        )
    }

    /// ラバーバンドとクリックに効かせる固定値。欄が出ていて固定があるときだけ `Some`。
    #[must_use]
    pub fn dimension_locks(&self) -> Option<DimValues> {
        let locks = self.dim.state.locks;
        (self.fields_visible(self.dim.base.is_some()) && !locks.is_empty()).then_some(locks)
    }

    /// 寸法入力で入力中の欄（テスト用）。
    #[cfg(test)]
    pub fn dimension_field(&self) -> Field {
        self.dim.state.field
    }

    /// 履歴（古い順）。
    pub fn history(&self) -> impl Iterator<Item = &HistoryLine> {
        self.history.iter()
    }

    /// このフレームの IME イベントを反映する。
    ///
    /// `TextEdit` を描画する前に呼ぶこと。入力欄が持ち主のとき（[`owns_keys`]）だけ呼ぶ。
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
    /// - `modal_open` … モーダル（未保存確認）が出ているか。出ている間、キーと IME の
    ///   持ち主はモーダルで、コマンドラインは扱わない（Issue #24）。モーダルのボタンに
    ///   フォーカスが無いと、フォーカスだけでは「誰も持っていない」に見えるため
    pub fn begin_frame(&mut self, ctx: &egui::Context, allow_suggestions: bool, modal_open: bool) {
        self.dynamic.frame_enabled = self.dynamic.enabled;
        let now = ctx.input(|i| i.time);
        if let Some(e) = &mut self.recent_error {
            e.shown_at.get_or_insert(now);
        }
        let focused = ctx.memory(|m| m.focused());
        self.modal_open = modal_open;
        let owns_keys =
            !modal_open && owns_keys(egui::Id::new(INPUT_ID), self.focused_last_frame, focused);
        self.focused_last_frame = focused;
        self.keys_owned = owns_keys;

        // IME のイベントもキーと同じく、入力欄が持ち主のときだけ拾う。パネルの入力欄で
        // 変換していると、そちら宛ての Preedit でコマンドラインまで「変換中」になり、
        // カーソル横に `[変換中]` が出て位置が固定され、キーも候補も止まっていた。
        // 持ち主でなくなったら変換中も終える。変換の途中でパネルへ移ると、確定・取り消しは
        // パネルへ届くので、ここで落とさないと立ちっぱなしになる。
        if owns_keys {
            self.track_ime(ctx);
        } else {
            self.composing = false;
        }
        self.refresh_suggestions(allow_suggestions);

        // 変換中はキーを一切奪わない。IME に確定させるのが先。
        // 候補の操作キーもこのブロックの中にあるので、変換中は自動的に無効になる。
        // パネルの入力欄を編集している間も奪わない（Issue #22）。Ctrl+A（全選択）も同じ。
        self.select_all_requested = false;
        let pending = if self.composing || !owns_keys {
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
            self.show_input(ui, f32::INFINITY, None);
        });
    }

    /// 変換中は確定処理を止めているので、その旨をユーザーに見せる。
    fn show_composing_badge(&self, ui: &mut egui::Ui) {
        if self.composing {
            ui.colored_label(
                COMPOSING_COLOR,
                egui::RichText::new(COMPOSING_BADGE).monospace(),
            );
        }
    }

    /// 入力欄を描く。**バッファも `TextEdit` の ID も 1 つだけ。**
    ///
    /// ID を固定しておくと、描く場所（画面下 / カーソル横）を切り替えても
    /// フォーカスとキャレットの位置が引き継がれる。
    ///
    /// `hint` … 空のときに薄く出す文字（寸法入力のライブ値・固定値）。
    fn show_input(&mut self, ui: &mut egui::Ui, width: f32, hint: Option<&str>) {
        let id = egui::Id::new(INPUT_ID);
        if std::mem::take(&mut self.caret_to_end) {
            let mut state = egui::text_edit::TextEditState::load(ui.ctx(), id).unwrap_or_default();
            let end = egui::text::CCursor::new(self.input.chars().count());
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::one(end)));
            state.store(ui.ctx(), id);
        }
        let mut edit = egui::TextEdit::singleline(&mut self.input);
        if let Some(hint) = hint {
            edit = edit.hint_text(egui::RichText::new(hint).monospace());
        }
        let response = ui.add(
            edit.id(id)
                // Tab・矢印・Esc はコマンドラインが自分で扱うキーなので、egui の
                // フォーカス移動に使わせない（Issue #22）。既定では Tab が
                // 「次の部品へ移る」として先に処理され、補完の直後にフォーカスが外れて
                // 続けて打った文字が消えていた。候補が無いときの Tab も、入力欄から
                // 出ていかないほうがよい（打ち間違いで入力先が変わると気づきにくい）。
                .event_filter(egui::EventFilter {
                    tab: true,
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    escape: true,
                })
                .desired_width(width)
                // モーダルが出ている間は文字も受けない（Issue #24）。キー（Enter / Esc / Space）は
                // `begin_frame` で奪わないが、文字は入力欄がフォーカスを持ったままだと入ってしまう。
                // 描くのはやめない（描かないとフォーカスの扱いが変わる）。
                .interactive(!self.modal_open)
                .font(egui::TextStyle::Monospace),
        );
        #[cfg(test)]
        {
            self.input_rect = Some(response.rect);
        }
        // キー入力が常にコマンドラインへ流れるよう、他に入力先が無ければ
        // 毎フレーム自分にフォーカスを戻す。
        // モーダルが出ている間は取り直さない（モーダルのボタンにフォーカスを渡すため。Issue #24）。
        if !self.modal_open && ui.memory(|m| m.focused().is_none()) {
            response.request_focus();
        }
    }

    /// 直近に描いた入力欄の矩形（テスト用）。
    #[cfg(test)]
    pub fn input_rect(&self) -> Option<egui::Rect> {
        self.input_rect
    }

    /// 入力欄の中身（テスト用）。
    #[cfg(test)]
    pub fn input(&self) -> &str {
        &self.input
    }

    /// 入力欄に打ちかけの文字を入れる（テスト用）。
    #[cfg(test)]
    pub fn set_input_for_test(&mut self, text: &str) {
        text.clone_into(&mut self.input);
    }

    /// このフレームのキーの持ち主がコマンドラインか。
    ///
    /// 入力欄にフォーカスがあるか、どこにも無いときだけ真（モーダルが出ている間は偽）。
    /// パネルの入力欄を編集している間は偽になる（ADR-0035）。Ctrl+1 のように、コマンドライン以外が
    /// 拾うショートカットも同じ規則に従うために公開している。[`Self::begin_frame`] の後に使うこと。
    #[must_use]
    pub fn owns_keys(&self) -> bool {
        self.keys_owned
    }

    /// コマンドラインの入力欄で変換中か。
    ///
    /// 変換中はバッファに未確定の文字列が入っているので、外から触ってはいけない（ADR-0002）。
    /// リボンのボタンはこれを見て、変換中なら押されても何もしない。
    #[must_use]
    pub fn is_composing(&self) -> bool {
        self.composing
    }

    /// コマンド候補が出ているか（テスト用）。
    #[cfg(test)]
    pub fn suggestions_visible(&self) -> bool {
        self.suggestions.is_visible()
    }

    /// カーソル横に入力欄を描く（動的入力がオンのときだけ）。
    ///
    /// - `canvas` … キャンバス（ビューポート）の矩形。この中に収める
    /// - `tool_active` … コマンドを実行中か（選択待ちを含む）
    /// - `dimension` … 寸法入力に参加中なら、基点から（固定をかけた後の）カーソルまでの
    ///   長さと角度（カーソルが無ければ `None` の中身）。参加していなければ `None`
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
        dimension: Option<Option<Live>>,
    ) {
        if !self.dynamic.frame_enabled {
            return;
        }
        // 寸法入力の欄を出すか。変換中は直前の見た目を保つ（入力欄を動かさない）。
        let fields = self.fields_visible(dimension.is_some());
        if !self.composing {
            self.dim.fields_shown = fields;
        }
        let live = dimension.flatten();

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
                // **入力欄より前（上・左）に来るものは、見せるときも隠すときも同じ並びで描く。**
                // 並びが変わると変換が始まったフレームで入力欄が動き、入力欄に付いて出る
                // IME の候補ウィンドウが最初の 1 打鍵で跳ねる（Area は固定していても）。
                // 隠すときは同じ並びのまま不透明度 0 にする。
                // モーダルが出ている間も隠す。描くのはやめない（IME とフォーカスの制約）。
                if !visible || self.modal_open {
                    ui.multiply_opacity(0.0);
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
                            egui::RichText::new(prompt).color(ui.visuals().strong_text_color()),
                        );
                        ui.horizontal(|ui| {
                            if fields {
                                self.show_dimension_fields(ui, live);
                            } else {
                                self.show_input(ui, DYN_INPUT_WIDTH, None);
                            }
                            // `[変換中]` は入力欄の後ろに置く。前に置くと入力欄が右へずれる。
                            // 変換していないときも幅は確保して見えなくするだけにする。
                            // 変換が始まって Area が広がると、キャンバスの右端寄りでは
                            // `constrain_to` が次のフレームで Area を左へ押し戻し、
                            // 入力欄（と候補ウィンドウ）が跳ねる。
                            ui.add_visible(
                                self.composing,
                                egui::Label::new(
                                    egui::RichText::new(COMPOSING_BADGE)
                                        .monospace()
                                        .color(COMPOSING_COLOR),
                                ),
                            );
                        });
                        // ここから下は入力欄の位置に影響しない。
                        self.show_suggestions(ui);
                        if let (Some(_), Some(e)) = (error_remaining, &self.recent_error) {
                            ui.colored_label(ERROR_COLOR, egui::RichText::new(&e.text).monospace());
                        }
                    });
            });
    }

    /// 寸法入力の「長さ」「角度」の 2 欄（円の四分点のグリップでは「半径」の 1 欄）を描く。
    ///
    /// **`TextEdit` は 1 つだけ**。入力中の欄の位置に本物の入力欄を置き、もう片方は
    /// 値を描くだけにする。2 つ描くとフォーカスと IME の出力先が 2 つになる（ADR-0034 決定 6）。
    ///
    /// 錠前の幅は固定していなくても常に確保する。固定した瞬間に後ろの欄
    /// （入力欄のことがある）が右へずれないように。
    fn show_dimension_fields(&mut self, ui: &mut egui::Ui, live: Option<Live>) {
        let state = self.dim.state;
        let kind = self.dim.kind;
        for &field in kind.fields() {
            let locked = state.locks.get(field);
            let live_value = live.and_then(|l| match field {
                Field::Length => Some(l.length),
                Field::Angle => l.angle_deg,
            });
            let format = |v: f64| match field {
                Field::Length => dimension::format_length(v),
                Field::Angle => dimension::format_angle(v),
            };
            // 固定値を優先し、無ければライブ値。どちらも無ければ（カーソルが無い・
            // 基点と同じで向きが無い）横棒。
            let shown = locked.or(live_value).map_or_else(|| "-".to_owned(), format);
            let name = kind.name(field);
            let active = field == state.field;
            let name_color = if active {
                ui.visuals().strong_text_color()
            } else {
                ui.visuals().weak_text_color()
            };
            ui.label(egui::RichText::new(name).color(name_color));
            if active {
                self.show_input(ui, DIM_FIELD_WIDTH, Some(&shown));
            } else {
                show_value_field(ui, &shown, locked.is_some());
            }
            lock_icon(ui, locked.is_some());
        }
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
                // 寸法入力の欄で確定した。固定値と入力中の値（無い欄はカーソルから）。
                // 何も無ければ（空 Enter）・数値以外なら、従来どおりの扱いに落とす。
                if self.dimension_active() {
                    let values = self
                        .dim
                        .state
                        .enter_values(dimension::classify(&self.input));
                    if let Some(values) = values {
                        self.input.clear();
                        self.suggestions.clear();
                        return Submission::Dimension(values);
                    }
                }
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

        // Ctrl+A（全選択、Issue #34 段階 3、ADR-0044）。ここへ来るのはキーの持ち主がコマンドラインで、
        // 変換中でないときだけ（パネルの入力欄・モーダル・IME では奪わない）。
        // 打ちかけの文字があるときは奪わない。入力欄（`TextEdit`）が受け取り、文字の全選択になる
        // （ユーザー判断 5）。効く段階か（点や値の入力中は効かない）は `Session::select_all` が決める。
        // 印を立てるだけで return はしない（同じフレームの Enter を取りこぼさない）。
        // 修飾キーは厳密に比べる（Ctrl+Shift+A・Ctrl+Alt+A では全選択しない。Issue #74 の 5）。
        if self.input.is_empty() && consume_key_exact(i, egui::Modifiers::COMMAND, egui::Key::A) {
            self.select_all_requested = true;
        }

        if i.consume_key(NONE, egui::Key::Escape) {
            // 候補が出ていれば、まず候補だけを閉じる。
            // いきなりコマンドを中断すると、打ち間違いのやり直しが面倒になる。
            if self.suggestions.is_visible() {
                self.suggestions.dismiss(&self.input);
                return None;
            }
            // 寸法入力では、固定か入力があればまずそれを全部解除する。
            // 何も無ければ従来どおり中断する。
            if self.dimension_active() && self.dim.state.escape(dimension::classify(&self.input)) {
                self.input.clear();
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
                    self.caret_to_end = true;
                    self.suggestions.update(&self.input);
                }
                return None;
            }
        }

        // 寸法入力の Tab。候補が出ているときの Tab は上で補完に使われるので、ここには来ない
        // （ツール実行中は候補を出さないので、実際には重ならない）。
        if self.dimension_active() && i.consume_key(NONE, egui::Key::Tab) {
            match self
                .dim
                .state
                .tab_in(self.dim.kind, dimension::classify(&self.input))
            {
                TabOutcome::Moved { consumed: true } => self.input.clear(),
                TabOutcome::Moved { consumed: false } | TabOutcome::Ignored => {}
                TabOutcome::Rejected(e) => self.error(e.message()),
            }
            return None;
        }
        // 動的入力オフでは欄が無いので固定できない。Tab が何もしないと、AutoCAD の感覚で
        // `100` Tab `90` と打った人の入力が `10090` につながる。入力は変えずに案内する。
        if !self.dynamic.frame_enabled
            && self.dim.base.is_some()
            && i.consume_key(NONE, egui::Key::Tab)
        {
            self.info(TAB_NEEDS_DYNAMIC);
            return None;
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

/// 寸法入力の、入力中でない欄。値を描くだけ（`TextEdit` にしない）。
///
/// 固定した値ははっきり（枠も錠前の色）、ライブ値は薄く描く。
fn show_value_field(ui: &mut egui::Ui, text: &str, locked: bool) {
    let (color, stroke) = if locked {
        (
            ui.visuals().strong_text_color(),
            egui::Stroke::new(1.0, LOCK_COLOR),
        )
    } else {
        (
            ui.visuals().weak_text_color(),
            ui.visuals().widgets.noninteractive.bg_stroke,
        )
    };
    egui::Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .stroke(stroke)
        .corner_radius(2)
        .inner_margin(egui::Margin::symmetric(4, 2))
        .show(ui, |ui| {
            ui.set_width(DIM_FIELD_WIDTH);
            ui.label(egui::RichText::new(text).monospace().color(color));
        });
}

/// 固定した欄の目印として、塗りの錠前を描く。
///
/// 絵文字の 🔒 は同梱フォントだと小さな丸にしか見えず、数字の 0 と紛らわしかった。
/// ユーザーが SVG の案から選んだ形（案 B: つる + 塗りの本体 + 抜いた鍵穴）を、
/// 16 単位の格子のまま描画命令で再現する。
///
/// 固定していないときも同じ大きさを確保する。目印の有無で欄の並びが変わると、
/// 入力欄が横にずれて日本語の変換候補が跳ねるため（ADR-0034）。
fn lock_icon(ui: &mut egui::Ui, visible: bool) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(LOCK_SIZE, LOCK_SIZE), egui::Sense::hover());
    if !visible || !ui.is_rect_visible(rect) {
        return;
    }
    let unit = LOCK_SIZE / 16.0;
    let at = |x: f32, y: f32| rect.min + egui::vec2(x * unit, y * unit);
    let painter = ui.painter();

    // つる: (5, 7.5) から上へ、中心 (8, 5)・半径 3 の上半円を回って (11, 7.5) へ下りる。
    let steps = 12_usize;
    let step = std::f32::consts::PI / 12.0;
    let mut shackle = vec![at(5.0, 7.5)];
    shackle.extend(
        std::iter::successors(Some(std::f32::consts::PI), |a| Some(a - step))
            .take(steps + 1)
            .map(|a| at(8.0 + 3.0 * a.cos(), 5.0 - 3.0 * a.sin())),
    );
    shackle.push(at(11.0, 7.5));
    painter.add(egui::Shape::line(
        shackle,
        egui::Stroke::new(1.8 * unit, LOCK_COLOR),
    ));

    // 本体と、背景色で抜いた鍵穴。
    let body = egui::Rect::from_min_max(at(2.5, 7.0), at(13.5, 15.0));
    painter.rect_filled(body, 1.6 * unit, LOCK_COLOR);
    let hole = ui.visuals().window_fill();
    painter.circle_filled(at(8.0, 10.3), 1.1 * unit, hole);
    painter.line_segment(
        [at(8.0, 10.8), at(8.0, 12.8)],
        egui::Stroke::new(1.3 * unit, hole),
    );
}

/// `modifiers` と**ちょうど同じ**修飾キーで押された `key` を消費し、あったかを返す。
///
/// egui の `consume_key` は `Modifiers::matches_logically` で比べるので、余分な Shift・Alt が
/// 付いていても一致する（Ctrl+Shift+A も Ctrl+A になる）。こちらは `matches_exact` で比べる。
/// Ctrl と Command の違いは `matches_exact` が吸収する（Linux の Ctrl は `ctrl` と `command` の
/// 両方が立つが、`COMMAND` と一致する）。同じフレームに複数あれば（キーリピート）全部を消費する
/// （`consume_key` と同じ。残すと入力欄へ流れる）。
fn consume_key_exact(i: &mut egui::InputState, modifiers: egui::Modifiers, key: egui::Key) -> bool {
    let mut found = false;
    i.events.retain(|event| {
        let hit = matches!(
            event,
            egui::Event::Key {
                key: k,
                modifiers: m,
                pressed: true,
                ..
            } if *k == key && m.matches_exact(modifiers)
        );
        found |= hit;
        !hit
    });
    found
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

    /// フォーカスが入力欄か無いときだけキーを扱い、パネルの入力欄が
    /// フォーカスを持っている（持っていた）間は扱わないこと。
    #[test]
    fn keys_belong_to_the_command_line_only_when_no_one_else_has_focus() {
        let input = egui::Id::new(INPUT_ID);
        let panel = egui::Id::new("panel");
        assert!(owns_keys(input, None, None), "起動直後");
        assert!(owns_keys(input, Some(input), Some(input)), "入力欄にある");
        assert!(owns_keys(input, Some(input), None), "入力欄から外れた直後");
        assert!(!owns_keys(input, None, Some(panel)), "パネルに移った");
        assert!(
            !owns_keys(input, Some(panel), Some(panel)),
            "パネルを編集中"
        );
        assert!(
            !owns_keys(input, Some(panel), None),
            "パネルで Esc（egui がフレームの最初にフォーカスを外す）"
        );
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

        // 確定・中断・コマンド実行中の clear でも記憶は消える。
        s.dismiss("L");
        s.clear();
        s.update("L");
        assert!(s.is_visible(), "clear の後は同じ入力でも出る");
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
