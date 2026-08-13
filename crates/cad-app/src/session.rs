//! コマンドライン・ツール・選択をつなぐ層。
//!
//! ここが「ユーザーの操作 → [`Command`](cad_core::Command) の適用」の唯一の流れになる。
//! 図面を変更するのは `Document::apply` / `undo` / `redo` だけで、
//! ラバーバンドなどの途中状態は一切 `Document` に入れない。

use cad_core::command::ExitDefinitionEdit;
use cad_core::geom::{Aabb, Point2};
use cad_core::{Document, Geometry};

use crate::cmdline::{coord, CommandLine, LineKind, Submission};
use crate::editing::EditSession;
use crate::input::ViewAction;
use crate::selection::{self, Selection, WindowMode};
use crate::tools::{self, Immediate, StepInput, StepOutcome, Tool, ToolCtx, ToolSettings};

/// UI に対する要求。図面の変更ではないのでコマンドにはしない。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiAction {
    /// レイヤパネルの開閉。
    ToggleLayerPanel,
    /// コンポーネントパネルの開閉。
    ToggleComponentPanel,
    /// ファイル操作。
    File(crate::file_ops::FileAction),
}

/// 実行中コマンドが無いときのプロンプト。
const IDLE_PROMPT: &str = "コマンド:";
/// 選択待ちのプロンプト。
const SELECT_PROMPT: &str = "オブジェクトを選択 (Enter で確定):";

/// コマンドラインとツールの実行状態。
pub struct Session {
    /// コマンドライン。
    pub cmdline: CommandLine,
    /// 現在の選択。
    pub selection: Selection,
    /// 実行中のツール。
    tool: Option<Box<dyn Tool>>,
    /// ツール開始前の選択待ち段階か。
    awaiting_selection: bool,
    /// コンポーネントの編集セッション。編集中だけ `Some`。
    editing: Option<EditSession>,
    /// 選択に使われた交差窓の矩形（モデル座標）。
    ///
    /// STRETCH が「どの点を動かすか」を決めるのに使う。窓選択やクリックでは増えない。
    /// AutoCAD は交差窓を複数回重ねられるので蓄積する。
    crossing_rects: Vec<Aabb>,
    /// このフレームで発生したビュー操作。
    view_actions: Vec<ViewAction>,
    /// このフレームで発生した UI 要求。
    ui_actions: Vec<UiAction>,
    /// コマンド間で覚える設定（FILLET の半径など）。
    settings: ToolSettings,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    /// 初期状態。
    #[must_use]
    pub fn new() -> Self {
        let mut cmdline = CommandLine::new();
        cmdline.info("ymcad — コマンド名またはエイリアスを入力してください (例: L, C, REC)");
        Self {
            cmdline,
            selection: Selection::new(),
            tool: None,
            awaiting_selection: false,
            editing: None,
            crossing_rects: Vec::new(),
            view_actions: Vec::new(),
            ui_actions: Vec::new(),
            settings: ToolSettings::default(),
        }
    }

    /// いま表示すべきプロンプト。
    #[must_use]
    pub fn prompt(&self) -> String {
        if self.awaiting_selection {
            return SELECT_PROMPT.to_owned();
        }
        match &self.tool {
            Some(t) => t.prompt(),
            None => IDLE_PROMPT.to_owned(),
        }
    }

    /// 実行中のツールがあるか。
    #[must_use]
    pub fn has_active_tool(&self) -> bool {
        self.tool.is_some()
    }

    /// 点の入力を待っている状態か。キャンバスのクリックを点として扱うかの判断に使う。
    #[must_use]
    pub fn wants_point(&self) -> bool {
        self.tool.is_some() && !self.awaiting_selection
    }

    /// 実行中のツールがクリックを「図形の指定」として受け取りたいか。
    #[must_use]
    pub fn wants_entity(&self) -> bool {
        self.tool.as_ref().is_some_and(|t| t.wants_entity())
    }

    /// コンポーネントの編集中か。
    #[must_use]
    pub fn editing(&self) -> Option<&EditSession> {
        self.editing.as_ref()
    }

    /// コンポーネントの編集を終えて定義へ書き戻す。
    ///
    /// 編集中でなければ案内を出すだけ。
    fn end_component_edit(&mut self, doc: &mut Document) {
        let Some(session) = self.editing.take() else {
            self.cmdline
                .error("コンポーネントを編集していません（EDITCOMP で始めます）");
            return;
        };

        let (members, origins) = session.members(doc);
        if members.is_empty() {
            // 中身が全部消されている。空の定義を作ると使い道が無いので断る。
            self.cmdline
                .error("中身がすべて消されています。1 つ以上残してください");
            // 編集は続けられるよう、セッションを戻す。
            self.editing = Some(session);
            return;
        }

        let cmd = Box::new(ExitDefinitionEdit::new(
            "ENDCOMP",
            session.definition(),
            session.placement(),
            members,
            origins,
        ));
        match doc.apply(cmd) {
            Ok(()) => {
                self.selection.clear();
                self.cmdline.info("コンポーネントの編集を終えました");
            }
            Err(e) => {
                self.cmdline.error(format!("書き戻せませんでした: {e}"));
                self.editing = Some(session);
            }
        }
    }

    /// 実行中のツールが**生の文字列**を待っているか。
    ///
    /// `true` の間は座標・数値としての解釈も、大文字化も全角の正規化もしない。
    #[must_use]
    pub fn wants_raw_text(&self) -> bool {
        self.tool.as_ref().is_some_and(|t| t.wants_raw_text())
    }

    /// 溜まったビュー操作を取り出す。
    pub fn take_view_actions(&mut self) -> Vec<ViewAction> {
        std::mem::take(&mut self.view_actions)
    }

    /// 溜まった UI 要求を取り出す。
    pub fn take_ui_actions(&mut self) -> Vec<UiAction> {
        std::mem::take(&mut self.ui_actions)
    }

    /// レイヤ操作など、外部から組み立てたコマンドを適用する。
    ///
    /// 図面を変更する経路は `Document::apply` ただ 1 つなので、
    /// レイヤパネルからの操作もここを通す。
    pub fn apply_external(&mut self, cmd: Box<dyn cad_core::Command>, doc: &mut Document) {
        let name = cmd.name();
        self.apply(cmd, name, doc);
    }

    /// 相対座標入力と垂線スナップの基準となる、直前に確定した点。
    #[must_use]
    pub fn last_point(&self) -> Option<Point2> {
        self.tool.as_ref().and_then(|t| t.last_point())
    }

    fn ctx<'a>(&'a self, doc: &'a Document) -> ToolCtx<'a> {
        ToolCtx {
            doc,
            selection: &self.selection,
            layer: doc.layers().current(),
            crossing_rects: &self.crossing_rects,
            settings: self.settings,
            editing: self.editing.as_ref(),
        }
    }

    /// ラバーバンド。カーソル位置に応じた確定前の図形。
    #[must_use]
    pub fn preview(&self, cursor: Option<Point2>, doc: &Document) -> Vec<Geometry> {
        let (Some(tool), Some(c)) = (self.tool.as_ref(), cursor) else {
            return Vec::new();
        };
        if self.awaiting_selection {
            return Vec::new();
        }
        tool.preview(c, &self.ctx(doc))
    }

    // ---- コマンドラインからの入力 -----------------------------------------

    /// コマンドラインの確定操作を処理する。
    pub fn handle_submission(&mut self, submission: Submission, doc: &mut Document) {
        match submission {
            Submission::None => {}
            Submission::Cancel => self.cancel(),
            Submission::Empty => self.handle_empty_enter(doc),
            Submission::Text(text) => {
                self.cmdline.push_line(LineKind::Input, format!("> {text}"));
                self.handle_text(&text, doc);
            }
        }
    }

    /// `Esc`。実行中コマンドを中断し、選択を解除する。
    pub fn cancel(&mut self) {
        if self.tool.is_some() || self.awaiting_selection {
            self.cmdline.info("*取り消し*");
        }
        self.tool = None;
        self.awaiting_selection = false;
        self.selection.clear();
        self.crossing_rects.clear();
        self.cmdline.clear_input();
    }

    /// 空のまま確定された場合。
    fn handle_empty_enter(&mut self, doc: &mut Document) {
        if self.awaiting_selection {
            self.finish_selection(doc);
            return;
        }
        if self.tool.is_some() {
            self.feed_tool(StepInput::Enter, doc);
            return;
        }
        // 実行中コマンドが無ければ直前のコマンドを再実行する。
        match self.cmdline.last_command() {
            Some(name) => {
                let name = name.to_owned();
                self.cmdline.push_line(LineKind::Input, format!("> {name}"));
                self.start(&name, doc);
            }
            None => self.cmdline.info("再実行できるコマンドがありません"),
        }
    }

    /// 文字列が確定された場合。
    fn handle_text(&mut self, text: &str, doc: &mut Document) {
        if self.awaiting_selection {
            // 選択待ち中は文字入力を受け付けない（誤操作を防ぐ）。
            self.cmdline
                .error("選択中です。オブジェクトをクリックするか Enter で確定してください");
            return;
        }

        if self.tool.is_some() {
            // 名前や式を待っているツールには、打った文字列をそのまま渡す。
            // 大文字化や全角の正規化を通すと `if` が `IF` になり、
            // `データー` の長音が `-` に直されて壊れる。
            if self.wants_raw_text() {
                self.feed_tool(StepInput::Word(text.to_owned()), doc);
                return;
            }
            match Self::interpret(text, self.last_point()) {
                Ok(input) => self.feed_tool(input, doc),
                Err(msg) => self.cmdline.error(msg),
            }
            return;
        }

        self.start(text, doc);
    }

    /// ツールへの入力を解釈する。座標 → 数値 → キーワードの順に試す。
    ///
    /// 相対座標なのに基準点が無い場合だけはエラーを返す。
    /// ここでキーワード扱いに落とすと「不明なオプション」という的外れな案内になるため。
    fn interpret(text: &str, last: Option<Point2>) -> Result<StepInput, String> {
        if let Some(c) = coord::parse(text) {
            if let Some(p) = c.resolve(last) {
                return Ok(StepInput::Point(p));
            }
            if c.needs_last_point() {
                return Err("基準となる直前の点がないため、相対座標は使えません".to_owned());
            }
        }
        if let Some(n) = coord::parse_number(text) {
            return Ok(StepInput::Number(n));
        }
        Ok(StepInput::Word(
            coord::normalize_ascii(text).trim().to_uppercase(),
        ))
    }

    /// できあがったツールをそのまま起動する。
    ///
    /// パネルのボタンから、名前を打たせずにコマンドを始めるために使う。
    /// コマンドラインから起動したときと同じ経路（`start_tool`）を通す。
    pub fn start_tool_directly(&mut self, tool: Box<dyn Tool>, doc: &mut Document) {
        self.cancel();
        self.begin_tool(tool, doc);
    }

    /// コマンド名からツールを起動する。
    fn start(&mut self, name: &str, doc: &mut Document) {
        if let Some(cmd) = tools::immediate(name) {
            self.run_immediate(cmd, doc);
            self.cmdline.remember_command(cmd.name());
            return;
        }

        let Some(tool) = tools::create(name) else {
            self.cmdline.error(format!("不明なコマンドです: {name}"));
            return;
        };

        self.begin_tool(tool, doc);
    }

    /// ツールを実際に走らせる。`start` とパネルからの起動が共有する。
    fn begin_tool(&mut self, tool: Box<dyn Tool>, doc: &mut Document) {
        self.cmdline.remember_command(tool.name());
        let wants_selection = tool.wants_selection();
        self.tool = Some(tool);

        if wants_selection && self.selection.is_empty() {
            // 選択をやり直すので、前のコマンドが使った範囲も捨てる。
            self.crossing_rects.clear();
            self.awaiting_selection = true;
        } else if wants_selection {
            // 既に選択済みならそのまま先へ進む。
            self.feed_tool(StepInput::SelectionReady, doc);
        }
    }

    fn run_immediate(&mut self, cmd: Immediate, doc: &mut Document) {
        // 図面の変更ではないものは UI 要求として外へ渡す。
        match cmd {
            Immediate::LayerPanel => {
                self.ui_actions.push(UiAction::ToggleLayerPanel);
                return;
            }
            Immediate::ComponentPanel => {
                self.ui_actions.push(UiAction::ToggleComponentPanel);
                return;
            }
            Immediate::EndComponentEdit => {
                self.end_component_edit(doc);
                return;
            }
            Immediate::File(action) => {
                self.ui_actions.push(UiAction::File(action));
                return;
            }
            Immediate::Undo | Immediate::Redo => {}
        }

        let result = match cmd {
            Immediate::Undo => doc.undo(),
            Immediate::Redo => doc.redo(),
            _ => unreachable!("直前に処理済み"),
        };
        match result {
            Ok(Some(name)) => self.cmdline.info(format!("{}: {name}", cmd.name())),
            Ok(None) => self.cmdline.info(match cmd {
                Immediate::Undo => "これ以上取り消せません",
                _ => "やり直せる操作がありません",
            }),
            Err(e) => self.cmdline.error(format!("{}: {e}", cmd.name())),
        }
        self.selection.retain_existing(doc);
    }

    /// ツールへ 1 手渡し、結果を処理する。
    fn feed_tool(&mut self, input: StepInput, doc: &mut Document) {
        let Some(mut tool) = self.tool.take() else {
            return;
        };

        let outcome = {
            let ctx = ToolCtx {
                doc,
                selection: &self.selection,
                layer: doc.layers().current(),
                crossing_rects: &self.crossing_rects,
                settings: self.settings,
                editing: self.editing.as_ref(),
            };
            tool.step(input, &ctx)
        };

        let name = tool.name();
        match outcome {
            StepOutcome::Continue => self.tool = Some(tool),
            StepOutcome::Reject(msg) => {
                self.cmdline.error(msg);
                self.tool = Some(tool);
            }
            StepOutcome::Apply(cmd) => {
                self.apply(cmd, name, doc);
            }
            StepOutcome::ApplyAndContinue(cmd) => {
                self.apply(cmd, name, doc);
                self.tool = Some(tool);
            }
            StepOutcome::View(action) => {
                self.view_actions.push(action);
            }
            StepOutcome::Setting(settings) => {
                self.settings = settings;
                self.tool = Some(tool);
            }
            StepOutcome::ApplyAndEdit {
                command,
                definition,
                placement,
            } => {
                // 適用の前後で差分を取り、置かれた要素を編集セッションに記録する。
                // 並びはスロット昇順 = 挿入順 = 定義の中身の順。
                let before: Vec<cad_core::EntityId> = doc.entities().ids().collect();
                self.apply(command, name, doc);
                let entered: Vec<cad_core::EntityId> = doc
                    .entities()
                    .ids()
                    .filter(|id| !before.contains(id))
                    .collect();
                if entered.is_empty() {
                    // 適用に失敗した（エラーは `apply` が出している）。
                    return;
                }
                self.editing = Some(EditSession::new(doc, definition, placement, entered));
                self.cmdline
                    .info("コンポーネントを編集中です（ENDCOMP で確定）");
            }
            StepOutcome::Finish => {}
        }
    }

    fn apply(&mut self, cmd: Box<dyn cad_core::Command>, name: &'static str, doc: &mut Document) {
        match doc.apply(cmd) {
            Ok(()) => {}
            Err(e) => self.cmdline.error(format!("{name}: {e}")),
        }
        self.selection.retain_existing(doc);
    }

    // ---- キャンバスからの入力 ---------------------------------------------

    /// キャンバスがクリックされた。
    ///
    /// `pick_tolerance` はモデル空間での拾い半径。
    pub fn handle_click(
        &mut self,
        model: Point2,
        shift: bool,
        pick_tolerance: f64,
        doc: &mut Document,
    ) {
        if self.wants_point() {
            // 図形を要求しているツールには、拾えたら図形として渡す。
            // 拾えなかったクリックは点のまま渡し、ツール側で案内させる。
            if self.wants_entity() {
                if let Some(id) = selection::pick_at(doc, model, pick_tolerance) {
                    self.feed_tool(StepInput::Entity { id, at: model }, doc);
                    return;
                }
            }
            self.feed_tool(StepInput::Point(model), doc);
            return;
        }

        // 選択待ち、あるいはコマンド無しの状態ではピック選択。
        let Some(id) = selection::pick_at(doc, model, pick_tolerance) else {
            if !shift && !self.awaiting_selection {
                self.selection.clear();
            }
            return;
        };

        // グループの一員を選んだらグループ全体を対象にする（AutoCAD の既定）。
        for member in selection::expand_to_group(doc, id) {
            if shift {
                self.selection.remove(member);
            } else {
                self.selection.insert(member);
            }
        }
    }

    /// キャンバス上で矩形ドラッグによる選択が行われた。
    pub fn handle_rect_select(
        &mut self,
        rect: Aabb,
        mode: WindowMode,
        shift: bool,
        doc: &mut Document,
    ) {
        if self.wants_point() {
            return;
        }
        let hits = selection::pick_in_rect(doc, rect, mode);
        for id in hits {
            for member in selection::expand_to_group(doc, id) {
                if shift {
                    self.selection.remove(member);
                } else {
                    self.selection.insert(member);
                }
            }
        }

        // STRETCH が使う範囲。交差選択のときだけ、追加選択の場合だけ覚える。
        // Shift での選択解除では範囲を増やさない（動かす対象を広げてしまうため）。
        if mode == WindowMode::Crossing && !shift {
            self.crossing_rects.push(rect);
        }
    }

    /// 選択待ちを終える。
    fn finish_selection(&mut self, doc: &mut Document) {
        if self.selection.is_empty() {
            self.cmdline.error("オブジェクトが選択されていません");
            self.tool = None;
            self.awaiting_selection = false;
            return;
        }
        self.awaiting_selection = false;
        self.cmdline
            .info(format!("{} 個のオブジェクトを選択", self.selection.len()));
        self.feed_tool(StepInput::SelectionReady, doc);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cad_core::geom::tolerance::eq_len;

    fn point(x: f64, y: f64) -> Point2 {
        Point2::new(x, y)
    }

    #[test]
    fn interpret_prefers_coordinates() {
        match Session::interpret("100,50", None).unwrap() {
            StepInput::Point(p) => assert!(eq_len(p.x, 100.0) && eq_len(p.y, 50.0)),
            other => panic!("点を期待したが {other:?}"),
        }
    }

    #[test]
    fn interpret_resolves_relative_against_last_point() {
        match Session::interpret("@10,10", Some(point(1.0, 2.0))).unwrap() {
            StepInput::Point(p) => assert!(eq_len(p.x, 11.0) && eq_len(p.y, 12.0)),
            other => panic!("点を期待したが {other:?}"),
        }
    }

    #[test]
    fn interpret_falls_back_to_number_then_word() {
        assert_eq!(
            Session::interpret("42.5", None),
            Ok(StepInput::Number(42.5))
        );
        assert_eq!(
            Session::interpret("2p", None),
            Ok(StepInput::Word("2P".to_owned()))
        );
    }

    /// 全角で入力されたオプションも半角大文字に正規化されること（ADR-0002）。
    #[test]
    fn interpret_normalizes_fullwidth_keywords() {
        assert_eq!(
            Session::interpret("２Ｐ", None),
            Ok(StepInput::Word("2P".to_owned()))
        );
    }

    #[test]
    fn unknown_command_reports_error() {
        let mut doc = Document::new();
        let mut s = Session::new();
        s.handle_submission(Submission::Text("NOPE".to_owned()), &mut doc);
        let last = s.cmdline.history().last().unwrap();
        assert_eq!(last.kind, LineKind::Error);
        assert!(last.text.contains("NOPE"));
        assert!(!s.has_active_tool());
    }

    #[test]
    fn empty_enter_without_history_is_reported() {
        let mut doc = Document::new();
        let mut s = Session::new();
        s.handle_submission(Submission::Empty, &mut doc);
        assert!(s
            .cmdline
            .history()
            .any(|l| l.text.contains("再実行できるコマンド")));
    }

    /// 空 Enter で直前のコマンドが再実行されること（指示書の受け入れ基準）。
    #[test]
    fn empty_enter_repeats_last_command() {
        let mut doc = Document::new();
        let mut s = Session::new();

        s.handle_submission(Submission::Text("LINE".to_owned()), &mut doc);
        assert!(s.has_active_tool());
        s.cancel();
        assert!(!s.has_active_tool());

        s.handle_submission(Submission::Empty, &mut doc);
        assert!(s.has_active_tool(), "直前の LINE が再実行されるはず");
        assert_eq!(s.prompt(), tools::create("LINE").unwrap().prompt());
    }

    #[test]
    fn escape_cancels_tool_and_selection() {
        let mut doc = Document::new();
        let mut s = Session::new();
        s.handle_submission(Submission::Text("LINE".to_owned()), &mut doc);
        assert!(s.has_active_tool());

        s.handle_submission(Submission::Cancel, &mut doc);
        assert!(!s.has_active_tool());
        assert!(s.selection.is_empty());
        assert_eq!(s.prompt(), IDLE_PROMPT);
    }

    #[test]
    fn undo_and_redo_run_immediately() {
        let mut doc = Document::new();
        let mut s = Session::new();

        // 何も無い状態の UNDO は「取り消せません」。
        s.handle_submission(Submission::Text("U".to_owned()), &mut doc);
        assert!(!s.has_active_tool(), "UNDO はツールを起動しない");
        assert!(s
            .cmdline
            .history()
            .any(|l| l.text.contains("これ以上取り消せません")));
    }

    /// 選択が必要なコマンドは、選択が空なら選択待ちに入ること。
    #[test]
    fn editing_command_enters_selection_phase() {
        let mut doc = Document::new();
        let mut s = Session::new();
        s.handle_submission(Submission::Text("ERASE".to_owned()), &mut doc);
        assert_eq!(s.prompt(), SELECT_PROMPT);
        assert!(!s.wants_point(), "選択待ち中は点入力を受け付けない");
    }

    /// 選択待ちで何も選ばずに Enter するとコマンドが終了すること。
    #[test]
    fn empty_selection_aborts_editing_command() {
        let mut doc = Document::new();
        let mut s = Session::new();
        s.handle_submission(Submission::Text("ERASE".to_owned()), &mut doc);
        s.handle_submission(Submission::Empty, &mut doc);
        assert!(!s.has_active_tool());
        assert!(s
            .cmdline
            .history()
            .any(|l| l.text.contains("選択されていません")));
    }
}

#[cfg(test)]
mod flow_tests {
    //! コマンドラインからの入力だけでコマンドを一通り走らせ、
    //! 図面の結果と Undo/Redo の巻き戻りを検証する。
    //!
    //! `Session` はコマンドラインの文字列入力とキャンバスのクリックの両方を
    //! 同じ経路で処理するので、文字列入力だけで作図の全経路を通せる。

    use super::*;
    use cad_core::geom::tolerance::eq_len;

    fn feed(s: &mut Session, doc: &mut Document, text: &str) {
        s.handle_submission(Submission::Text(text.to_owned()), doc);
    }

    fn enter(s: &mut Session, doc: &mut Document) {
        s.handle_submission(Submission::Empty, doc);
    }

    fn setup() -> (Session, Document) {
        (Session::new(), Document::new())
    }

    /// 図面に入っている図形を種別名で列挙する。
    fn type_names(doc: &Document) -> Vec<&'static str> {
        doc.entities()
            .iter()
            .map(|(_, e)| e.geom.type_name())
            .collect()
    }

    #[test]
    fn line_draws_consecutive_segments() {
        let (mut s, mut doc) = setup();
        feed(&mut s, &mut doc, "L");
        feed(&mut s, &mut doc, "0,0");
        feed(&mut s, &mut doc, "10,0");
        feed(&mut s, &mut doc, "10,10");
        enter(&mut s, &mut doc);

        assert_eq!(doc.entities().len(), 2, "連続線分が 2 本できるはず");
        assert!(!s.has_active_tool());
    }

    /// LINE の `C` が始点まで閉じること。
    #[test]
    fn line_close_option_adds_closing_segment() {
        let (mut s, mut doc) = setup();
        feed(&mut s, &mut doc, "L");
        for p in ["0,0", "10,0", "10,10"] {
            feed(&mut s, &mut doc, p);
        }
        feed(&mut s, &mut doc, "C");

        assert_eq!(doc.entities().len(), 3, "2 本 + 閉じる 1 本");
        assert!(!s.has_active_tool());
    }

    /// 相対座標と相対極座標の 3 形式がすべて動くこと（受け入れ基準）。
    #[test]
    fn all_three_coordinate_forms_work() {
        let (mut s, mut doc) = setup();
        feed(&mut s, &mut doc, "L");
        feed(&mut s, &mut doc, "0,0"); // 絶対
        feed(&mut s, &mut doc, "@10,0"); // 相対
        feed(&mut s, &mut doc, "@10<90"); // 相対極
        enter(&mut s, &mut doc);

        assert_eq!(doc.entities().len(), 2);
        // 終点が (10, 10) になっているはず。
        let last = doc.entities().iter().last().unwrap().1;
        let cad_core::Geometry::Line(l) = &last.geom else {
            panic!("線分のはず");
        };
        assert!(eq_len(l.b.x, 10.0), "x = {}", l.b.x);
        assert!(eq_len(l.b.y, 10.0), "y = {}", l.b.y);
    }

    #[test]
    fn circle_center_radius() {
        let (mut s, mut doc) = setup();
        feed(&mut s, &mut doc, "C");
        feed(&mut s, &mut doc, "5,5");
        feed(&mut s, &mut doc, "10");

        assert_eq!(type_names(&doc), vec!["CIRCLE"]);
        let cad_core::Geometry::Circle(c) = &doc.entities().iter().next().unwrap().1.geom else {
            panic!("円のはず");
        };
        assert!(eq_len(c.radius, 10.0));
    }

    /// CIRCLE の `D`（直径）オプション。
    #[test]
    fn circle_diameter_option() {
        let (mut s, mut doc) = setup();
        feed(&mut s, &mut doc, "CIRCLE");
        feed(&mut s, &mut doc, "0,0");
        feed(&mut s, &mut doc, "D");
        feed(&mut s, &mut doc, "20");

        let cad_core::Geometry::Circle(c) = &doc.entities().iter().next().unwrap().1.geom else {
            panic!("円のはず");
        };
        assert!(eq_len(c.radius, 10.0), "直径 20 なら半径 10: {}", c.radius);
    }

    /// CIRCLE の `2P`（2 点）オプション。
    #[test]
    fn circle_two_point_option() {
        let (mut s, mut doc) = setup();
        feed(&mut s, &mut doc, "CIRCLE");
        feed(&mut s, &mut doc, "2P");
        feed(&mut s, &mut doc, "0,0");
        feed(&mut s, &mut doc, "20,0");

        let cad_core::Geometry::Circle(c) = &doc.entities().iter().next().unwrap().1.geom else {
            panic!("円のはず");
        };
        assert!(eq_len(c.radius, 10.0));
        assert!(eq_len(c.center.x, 10.0) && eq_len(c.center.y, 0.0));
    }

    #[test]
    fn arc_from_three_points() {
        let (mut s, mut doc) = setup();
        feed(&mut s, &mut doc, "A");
        feed(&mut s, &mut doc, "10,0");
        feed(&mut s, &mut doc, "0,10");
        feed(&mut s, &mut doc, "-10,0");

        assert_eq!(type_names(&doc), vec!["ARC"]);
    }

    /// 同一直線上の 3 点は弾かれ、やり直せること。
    #[test]
    fn arc_rejects_collinear_points() {
        let (mut s, mut doc) = setup();
        feed(&mut s, &mut doc, "A");
        feed(&mut s, &mut doc, "0,0");
        feed(&mut s, &mut doc, "10,0");
        feed(&mut s, &mut doc, "20,0");

        assert!(doc.entities().is_empty(), "円弧はできないはず");
        assert!(s.has_active_tool(), "3 点目をやり直せるはず");
        assert!(s.cmdline.history().any(|l| l.kind == LineKind::Error));

        // 別の点なら成功する。
        feed(&mut s, &mut doc, "10,10");
        assert_eq!(type_names(&doc), vec!["ARC"]);
    }

    #[test]
    fn rectangle_creates_closed_polyline() {
        let (mut s, mut doc) = setup();
        feed(&mut s, &mut doc, "REC");
        feed(&mut s, &mut doc, "0,0");
        feed(&mut s, &mut doc, "10,20");

        assert_eq!(type_names(&doc), vec!["LWPOLYLINE"]);
        let cad_core::Geometry::Polyline(p) = &doc.entities().iter().next().unwrap().1.geom else {
            panic!("ポリラインのはず");
        };
        assert!(p.closed);
        assert_eq!(p.vertex_count(), 4);
    }

    #[test]
    fn polyline_commits_single_entity_on_enter() {
        let (mut s, mut doc) = setup();
        feed(&mut s, &mut doc, "PL");
        for p in ["0,0", "10,0", "10,10"] {
            feed(&mut s, &mut doc, p);
            // 確定するまで図面には入らない。
            assert!(doc.entities().is_empty());
        }
        enter(&mut s, &mut doc);

        assert_eq!(type_names(&doc), vec!["LWPOLYLINE"], "全体で 1 要素になる");
    }

    /// ERASE が選択したものを消し、Undo で戻ること。
    #[test]
    fn erase_and_undo() {
        let (mut s, mut doc) = setup();
        feed(&mut s, &mut doc, "L");
        feed(&mut s, &mut doc, "0,0");
        feed(&mut s, &mut doc, "10,0");
        enter(&mut s, &mut doc);
        assert_eq!(doc.entities().len(), 1);

        let id = doc.entities().ids().next().unwrap();
        s.selection.insert(id);
        feed(&mut s, &mut doc, "E");

        assert!(doc.entities().is_empty(), "選択済みなら即削除される");

        feed(&mut s, &mut doc, "U");
        assert_eq!(doc.entities().len(), 1);
        assert_eq!(
            doc.entities().ids().next(),
            Some(id),
            "Undo で EntityId も復元されること"
        );
    }

    /// MOVE が図形を動かし、Undo で元に戻ること。
    #[test]
    fn move_and_undo_restores_position() {
        let (mut s, mut doc) = setup();
        feed(&mut s, &mut doc, "L");
        feed(&mut s, &mut doc, "0,0");
        feed(&mut s, &mut doc, "10,0");
        enter(&mut s, &mut doc);

        let id = doc.entities().ids().next().unwrap();
        s.selection.insert(id);
        feed(&mut s, &mut doc, "M");
        feed(&mut s, &mut doc, "0,0"); // 基点
        feed(&mut s, &mut doc, "5,5"); // 目的点

        let cad_core::Geometry::Line(l) = &doc.entities().get(id).unwrap().geom else {
            panic!("線分のはず");
        };
        assert!(eq_len(l.a.x, 5.0) && eq_len(l.a.y, 5.0), "移動しているはず");

        feed(&mut s, &mut doc, "U");
        let cad_core::Geometry::Line(l) = &doc.entities().get(id).unwrap().geom else {
            panic!("線分のはず");
        };
        assert!(eq_len(l.a.x, 0.0) && eq_len(l.a.y, 0.0), "Undo で戻るはず");
    }

    /// COPY は複数回続けて複写できること（指示書の要求）。
    #[test]
    fn copy_continues_for_multiple_copies() {
        let (mut s, mut doc) = setup();
        feed(&mut s, &mut doc, "L");
        feed(&mut s, &mut doc, "0,0");
        feed(&mut s, &mut doc, "10,0");
        enter(&mut s, &mut doc);

        s.selection.insert(doc.entities().ids().next().unwrap());
        feed(&mut s, &mut doc, "CO");
        feed(&mut s, &mut doc, "0,0"); // 基点
        feed(&mut s, &mut doc, "0,10");
        assert_eq!(doc.entities().len(), 2);
        assert!(s.has_active_tool(), "続けて複写できるはず");

        feed(&mut s, &mut doc, "0,20");
        assert_eq!(doc.entities().len(), 3);

        enter(&mut s, &mut doc);
        assert!(!s.has_active_tool());

        // Undo で 1 回分ずつ戻る。
        feed(&mut s, &mut doc, "U");
        assert_eq!(doc.entities().len(), 2);
        feed(&mut s, &mut doc, "U");
        assert_eq!(doc.entities().len(), 1);
    }

    /// すべての作図コマンドが Undo/Redo で正しく巻き戻ること（受け入れ基準）。
    #[test]
    fn every_draw_command_round_trips_through_undo_redo() {
        let cases: Vec<(&str, Vec<&str>)> = vec![
            ("L", vec!["0,0", "10,0", ""]),
            ("C", vec!["0,0", "5"]),
            ("A", vec!["10,0", "0,10", "-10,0"]),
            ("REC", vec!["0,0", "10,10"]),
            ("PL", vec!["0,0", "10,0", "10,10", ""]),
        ];

        for (cmd, inputs) in cases {
            let (mut s, mut doc) = setup();
            feed(&mut s, &mut doc, cmd);
            for i in inputs {
                if i.is_empty() {
                    enter(&mut s, &mut doc);
                } else {
                    feed(&mut s, &mut doc, i);
                }
            }
            let after_draw = doc.entities().len();
            assert!(after_draw > 0, "{cmd}: 図形ができていない");

            // すべて取り消す。
            while doc.history().can_undo() {
                doc.undo().unwrap();
            }
            assert!(doc.entities().is_empty(), "{cmd}: Undo で空にならない");

            // すべてやり直す。
            while doc.history().can_redo() {
                doc.redo().unwrap();
            }
            assert_eq!(
                doc.entities().len(),
                after_draw,
                "{cmd}: Redo で元に戻らない"
            );
        }
    }

    /// 空 Enter による直前コマンドの再実行が、作図まで通ること。
    #[test]
    fn repeat_last_command_draws_again() {
        let (mut s, mut doc) = setup();
        feed(&mut s, &mut doc, "REC");
        feed(&mut s, &mut doc, "0,0");
        feed(&mut s, &mut doc, "10,10");
        assert_eq!(doc.entities().len(), 1);

        enter(&mut s, &mut doc); // 直前の RECTANGLE を再実行
        feed(&mut s, &mut doc, "20,20");
        feed(&mut s, &mut doc, "30,30");
        assert_eq!(doc.entities().len(), 2, "再実行で 2 つ目が描けるはず");
    }

    /// 不正なオプションはエラーになり、コマンドは続行すること。
    #[test]
    fn invalid_option_keeps_tool_running() {
        let (mut s, mut doc) = setup();
        feed(&mut s, &mut doc, "C");
        feed(&mut s, &mut doc, "0,0");
        feed(&mut s, &mut doc, "XYZ");

        assert!(s.has_active_tool(), "エラーでもコマンドは続く");
        assert!(s.cmdline.history().any(|l| l.kind == LineKind::Error));

        feed(&mut s, &mut doc, "5");
        assert_eq!(doc.entities().len(), 1);
    }

    /// 基準点が無い状態の相対座標は、的確なエラーになること。
    #[test]
    fn relative_coordinate_without_base_reports_clearly() {
        let (mut s, mut doc) = setup();
        feed(&mut s, &mut doc, "L");
        feed(&mut s, &mut doc, "@10,10");

        assert!(doc.entities().is_empty());
        assert!(s
            .cmdline
            .history()
            .any(|l| l.text.contains("相対座標は使えません")));
    }

    /// 交差窓で選ぶと、その矩形が STRETCH の範囲として記録されること。
    #[test]
    fn crossing_selection_records_its_rectangle() {
        let (mut s, mut doc) = setup();
        feed(&mut s, &mut doc, "L");
        feed(&mut s, &mut doc, "0,0");
        feed(&mut s, &mut doc, "100,0");
        enter(&mut s, &mut doc);

        let r = Aabb::new(Point2::new(-5.0, -5.0), Point2::new(5.0, 5.0));
        s.handle_rect_select(r, WindowMode::Crossing, false, &mut doc);
        assert_eq!(s.crossing_rects.len(), 1);
        assert_eq!(s.selection.len(), 1);

        // 窓選択では範囲を増やさない。
        s.handle_rect_select(
            Aabb::new(Point2::new(-200.0, -200.0), Point2::new(200.0, 200.0)),
            WindowMode::Window,
            false,
            &mut doc,
        );
        assert_eq!(s.crossing_rects.len(), 1, "窓選択は範囲に数えない");
    }

    /// **STRETCH の本体。** 交差範囲に入っている端点だけが動くこと。
    #[test]
    fn stretch_moves_only_endpoints_inside_the_crossing_region() {
        let (mut s, mut doc) = setup();
        feed(&mut s, &mut doc, "L");
        feed(&mut s, &mut doc, "0,0");
        feed(&mut s, &mut doc, "100,0");
        enter(&mut s, &mut doc);
        let id = doc.entities().ids().next().unwrap();

        // 始点 (0,0) だけを囲む交差窓。
        s.handle_rect_select(
            Aabb::new(Point2::new(-5.0, -5.0), Point2::new(5.0, 5.0)),
            WindowMode::Crossing,
            false,
            &mut doc,
        );

        feed(&mut s, &mut doc, "S");
        feed(&mut s, &mut doc, "0,0"); // 基点
        feed(&mut s, &mut doc, "0,50"); // 目的点

        let cad_core::Geometry::Line(l) = &doc.entities().get(id).unwrap().geom else {
            panic!("線分のはず");
        };
        assert!(
            eq_len(l.a.x, 0.0) && eq_len(l.a.y, 50.0),
            "始点は動く: {:?}",
            l.a
        );
        assert!(
            eq_len(l.b.x, 100.0) && eq_len(l.b.y, 0.0),
            "範囲外の終点は動かない: {:?}",
            l.b
        );
    }

    /// STRETCH が Undo で完全に戻ること。
    #[test]
    fn stretch_undo_restores_original_geometry() {
        let (mut s, mut doc) = setup();
        feed(&mut s, &mut doc, "L");
        feed(&mut s, &mut doc, "0,0");
        feed(&mut s, &mut doc, "100,0");
        enter(&mut s, &mut doc);
        let id = doc.entities().ids().next().unwrap();
        let before = doc.entities().get(id).unwrap().geom.clone();

        s.handle_rect_select(
            Aabb::new(Point2::new(-5.0, -5.0), Point2::new(5.0, 5.0)),
            WindowMode::Crossing,
            false,
            &mut doc,
        );
        feed(&mut s, &mut doc, "S");
        feed(&mut s, &mut doc, "0,0");
        feed(&mut s, &mut doc, "10,20");
        assert_ne!(doc.entities().get(id).unwrap().geom, before);

        feed(&mut s, &mut doc, "U");
        assert_eq!(
            doc.entities().get(id).unwrap().geom,
            before,
            "Undo で完全に元へ戻ること"
        );
    }

    /// 交差窓を使わずクリックで選んだ場合は、図形が丸ごと動くこと（AutoCAD と同じ）。
    #[test]
    fn stretch_without_crossing_region_moves_whole_entity() {
        let (mut s, mut doc) = setup();
        feed(&mut s, &mut doc, "L");
        feed(&mut s, &mut doc, "0,0");
        feed(&mut s, &mut doc, "100,0");
        enter(&mut s, &mut doc);
        let id = doc.entities().ids().next().unwrap();

        // クリック相当。範囲は記録されない。
        s.handle_click(Point2::new(50.0, 0.0), false, 1.0, &mut doc);
        assert_eq!(s.selection.len(), 1);
        assert!(s.crossing_rects.is_empty());

        feed(&mut s, &mut doc, "S");
        feed(&mut s, &mut doc, "0,0");
        feed(&mut s, &mut doc, "0,10");

        let cad_core::Geometry::Line(l) = &doc.entities().get(id).unwrap().geom else {
            panic!("線分のはず");
        };
        assert!(eq_len(l.a.y, 10.0) && eq_len(l.b.y, 10.0), "両端が動くはず");
        assert!(eq_len(l.a.x, 0.0) && eq_len(l.b.x, 100.0), "x は変わらない");
    }

    /// 交差窓を複数回重ねられること（AutoCAD の挙動）。
    #[test]
    fn multiple_crossing_windows_accumulate() {
        let (mut s, mut doc) = setup();
        feed(&mut s, &mut doc, "L");
        feed(&mut s, &mut doc, "0,0");
        feed(&mut s, &mut doc, "100,0");
        enter(&mut s, &mut doc);
        let id = doc.entities().ids().next().unwrap();

        // 両端をそれぞれ別の交差窓で囲む。
        for center in [0.0, 100.0] {
            s.handle_rect_select(
                Aabb::new(
                    Point2::new(center - 5.0, -5.0),
                    Point2::new(center + 5.0, 5.0),
                ),
                WindowMode::Crossing,
                false,
                &mut doc,
            );
        }
        assert_eq!(s.crossing_rects.len(), 2);

        feed(&mut s, &mut doc, "S");
        feed(&mut s, &mut doc, "0,0");
        feed(&mut s, &mut doc, "0,7");

        let cad_core::Geometry::Line(l) = &doc.entities().get(id).unwrap().geom else {
            panic!("線分のはず");
        };
        assert!(eq_len(l.a.y, 7.0) && eq_len(l.b.y, 7.0), "両端とも動くはず");
    }

    /// コマンドを切り替えたら前の範囲を引きずらないこと。
    #[test]
    fn starting_a_new_command_clears_the_previous_region() {
        let (mut s, mut doc) = setup();
        feed(&mut s, &mut doc, "L");
        feed(&mut s, &mut doc, "0,0");
        feed(&mut s, &mut doc, "100,0");
        enter(&mut s, &mut doc);

        s.handle_rect_select(
            Aabb::new(Point2::new(-5.0, -5.0), Point2::new(5.0, 5.0)),
            WindowMode::Crossing,
            false,
            &mut doc,
        );
        assert_eq!(s.crossing_rects.len(), 1);

        s.cancel();
        assert!(s.crossing_rects.is_empty(), "Esc で範囲も捨てる");
    }

    /// 短縮入力がコマンド起動まで届くこと（Issue #5）。
    ///
    /// `CommandLine` が候補を解決して正式名を渡すので、`Session` から見ると
    /// 「LINE が来た」のと同じになる。ここではその正式名で起動できることを確かめる。
    #[test]
    fn canonical_name_from_a_suggestion_starts_the_tool() {
        let (mut s, mut doc) = setup();
        feed(&mut s, &mut doc, "LINE");
        assert!(s.has_active_tool());
        assert_eq!(s.prompt(), tools::create("LINE").unwrap().prompt());
    }

    /// エイリアス 1 文字でも起動できること。
    /// 候補が未選択でも先頭が実行される仕組みと合わせて、`L` + Enter で LINE が始まる。
    #[test]
    fn single_letter_alias_starts_the_tool() {
        for (alias, expected) in [("L", "LINE"), ("C", "CIRCLE"), ("S", "STRETCH")] {
            let (mut s, mut doc) = setup();
            feed(&mut s, &mut doc, alias);
            assert!(s.has_active_tool(), "{alias} でツールが起動しない");
            assert_eq!(
                s.cmdline.last_command(),
                Some(expected),
                "{alias} は {expected} を起動するはず"
            );
        }
    }

    /// 図形を 1 つ作って選択した状態を用意する。
    fn setup_with_selected_line() -> (Session, Document, cad_core::EntityId) {
        let (mut s, mut doc) = setup();
        feed(&mut s, &mut doc, "L");
        feed(&mut s, &mut doc, "10,0");
        feed(&mut s, &mut doc, "20,0");
        enter(&mut s, &mut doc);
        let id = doc.entities().ids().next().unwrap();
        s.selection.insert(id);
        (s, doc, id)
    }

    fn line_of(doc: &Document, id: cad_core::EntityId) -> cad_core::geom::Line {
        match &doc.entities().get(id).unwrap().geom {
            cad_core::Geometry::Line(l) => *l,
            other => panic!("線分のはず: {other:?}"),
        }
    }

    /// ROTATE が原点まわりに 90 度回すこと。
    #[test]
    fn rotate_turns_the_selection_by_the_given_angle() {
        let (mut s, mut doc, id) = setup_with_selected_line();
        feed(&mut s, &mut doc, "RO");
        feed(&mut s, &mut doc, "0,0"); // 基点
        feed(&mut s, &mut doc, "90"); // 度で指定

        let l = line_of(&doc, id);
        // (10,0) -> (0,10) / (20,0) -> (0,20)
        assert!(eq_len(l.a.x, 0.0) && eq_len(l.a.y, 10.0), "始点: {:?}", l.a);
        assert!(eq_len(l.b.x, 0.0) && eq_len(l.b.y, 20.0), "終点: {:?}", l.b);

        feed(&mut s, &mut doc, "U");
        let l = line_of(&doc, id);
        assert!(eq_len(l.a.x, 10.0) && eq_len(l.a.y, 0.0), "Undo で戻る");
    }

    /// ROTATE の C オプションで元図形が残ること。
    #[test]
    fn rotate_copy_option_keeps_the_original() {
        let (mut s, mut doc, id) = setup_with_selected_line();
        feed(&mut s, &mut doc, "RO");
        feed(&mut s, &mut doc, "0,0");
        feed(&mut s, &mut doc, "C");
        feed(&mut s, &mut doc, "90");

        assert_eq!(doc.entities().len(), 2, "複製されて 2 つになるはず");
        let l = line_of(&doc, id);
        assert!(
            eq_len(l.a.x, 10.0) && eq_len(l.a.y, 0.0),
            "元は動かない: {:?}",
            l.a
        );
    }

    /// SCALE が基点を中心に倍率をかけること。
    #[test]
    fn scale_resizes_about_the_base_point() {
        let (mut s, mut doc, id) = setup_with_selected_line();
        feed(&mut s, &mut doc, "SC");
        feed(&mut s, &mut doc, "0,0");
        feed(&mut s, &mut doc, "2");

        let l = line_of(&doc, id);
        assert!(eq_len(l.a.x, 20.0), "始点 x: {}", l.a.x);
        assert!(eq_len(l.b.x, 40.0), "終点 x: {}", l.b.x);
    }

    /// 0 と負の尺度は拒否され、コマンドは続くこと。
    #[test]
    fn scale_rejects_zero_and_negative_factors() {
        for bad in ["0", "-2"] {
            let (mut s, mut doc, id) = setup_with_selected_line();
            feed(&mut s, &mut doc, "SC");
            feed(&mut s, &mut doc, "0,0");
            feed(&mut s, &mut doc, bad);

            assert!(s.has_active_tool(), "{bad}: エラーでもコマンドは続く");
            assert!(
                s.cmdline.history().any(|l| l.kind == LineKind::Error),
                "{bad}: エラーが出るはず"
            );
            let l = line_of(&doc, id);
            assert!(eq_len(l.a.x, 10.0), "{bad}: 図形は変わらない");

            // 続けて正しい値を入れれば通る。
            feed(&mut s, &mut doc, "2");
            assert!(eq_len(line_of(&doc, id).a.x, 20.0));
        }
    }

    /// MIRROR の既定は「元を残す」こと（AutoCAD と同じ）。
    #[test]
    fn mirror_keeps_the_original_by_default() {
        let (mut s, mut doc, id) = setup_with_selected_line();
        feed(&mut s, &mut doc, "MI");
        feed(&mut s, &mut doc, "0,0"); // 軸 1 点目
        feed(&mut s, &mut doc, "0,10"); // 軸 2 点目（Y 軸）
        enter(&mut s, &mut doc); // 既定 = 残す

        assert_eq!(doc.entities().len(), 2, "鏡像が増えて 2 つになる");
        let l = line_of(&doc, id);
        assert!(eq_len(l.a.x, 10.0), "元は動かない: {}", l.a.x);

        // 追加されたほうが反転している。
        let mirrored = doc.entities().iter().last().unwrap().1;
        let cad_core::Geometry::Line(m) = &mirrored.geom else {
            panic!("線分のはず");
        };
        assert!(eq_len(m.a.x, -10.0), "鏡像の x が反転: {}", m.a.x);
    }

    /// MIRROR で Y を指定すると元が消えること。
    #[test]
    fn mirror_with_yes_erases_the_original() {
        let (mut s, mut doc, id) = setup_with_selected_line();
        feed(&mut s, &mut doc, "MI");
        feed(&mut s, &mut doc, "0,0");
        feed(&mut s, &mut doc, "0,10");
        feed(&mut s, &mut doc, "Y");

        assert_eq!(doc.entities().len(), 1, "増えない");
        let l = line_of(&doc, id);
        assert!(eq_len(l.a.x, -10.0), "元の図形が反転している: {}", l.a.x);
    }

    /// 3 コマンドともエイリアスで起動し、Undo で完全に戻ること。
    #[test]
    fn transform_commands_round_trip_through_undo() {
        let cases: Vec<(&str, Vec<&str>)> = vec![
            ("RO", vec!["0,0", "45"]),
            ("SC", vec!["0,0", "3"]),
            ("MI", vec!["0,0", "0,10", "Y"]),
        ];

        for (alias, inputs) in cases {
            let (mut s, mut doc, id) = setup_with_selected_line();
            let before = doc.entities().get(id).unwrap().geom.clone();

            feed(&mut s, &mut doc, alias);
            for i in inputs {
                feed(&mut s, &mut doc, i);
            }
            assert_ne!(
                doc.entities().get(id).unwrap().geom,
                before,
                "{alias}: 変換されていない"
            );

            feed(&mut s, &mut doc, "U");
            assert_eq!(
                doc.entities().get(id).unwrap().geom,
                before,
                "{alias}: Undo で戻らない"
            );
        }
    }

    /// XLINE が 2 点指定で作れること。
    #[test]
    fn xline_from_two_points() {
        let (mut s, mut doc) = setup();
        feed(&mut s, &mut doc, "XL");
        feed(&mut s, &mut doc, "0,0");
        feed(&mut s, &mut doc, "10,10");

        assert_eq!(type_names(&doc), vec!["XLINE"]);
        assert!(!s.has_active_tool());
    }

    /// 水平・垂直・角度のオプションが動くこと。
    #[test]
    fn xline_options_produce_the_expected_direction() {
        use cad_core::geom::tolerance::eq_angle;
        use std::f64::consts::{FRAC_PI_2, FRAC_PI_4};

        for (inputs, expected) in [
            (vec!["H", "0,0"], 0.0),
            (vec!["V", "0,0"], FRAC_PI_2),
            (vec!["A", "45", "0,0"], FRAC_PI_4),
        ] {
            let (mut s, mut doc) = setup();
            feed(&mut s, &mut doc, "XL");
            for i in &inputs {
                feed(&mut s, &mut doc, i);
            }
            let cad_core::Geometry::Xline(x) = &doc.entities().iter().next().unwrap().1.geom else {
                panic!("作図線のはず（入力: {inputs:?}）");
            };
            assert!(
                eq_angle(x.angle(), expected),
                "{inputs:?}: 角度が {} で期待は {expected}",
                x.angle()
            );
        }
    }

    /// **作図線は図面範囲に影響しないこと。**
    ///
    /// 影響すると ZOOM EXTENTS が無限に飛んで使い物にならなくなる。
    #[test]
    fn xline_does_not_affect_drawing_extents() {
        let (mut s, mut doc) = setup();
        feed(&mut s, &mut doc, "L");
        feed(&mut s, &mut doc, "0,0");
        feed(&mut s, &mut doc, "10,0");
        enter(&mut s, &mut doc);
        let before = doc.bbox();

        feed(&mut s, &mut doc, "XL");
        feed(&mut s, &mut doc, "0,0");
        feed(&mut s, &mut doc, "1,1");

        assert_eq!(doc.entities().len(), 2, "作図線は追加されている");
        assert_eq!(doc.bbox(), before, "図面範囲は変わらない");
    }

    /// 作図線が Undo で消えること。
    #[test]
    fn xline_undo() {
        let (mut s, mut doc) = setup();
        feed(&mut s, &mut doc, "XL");
        feed(&mut s, &mut doc, "0,0");
        feed(&mut s, &mut doc, "1,1");
        assert_eq!(doc.entities().len(), 1);

        feed(&mut s, &mut doc, "U");
        assert!(doc.entities().is_empty());
    }

    /// 同じ点を 2 回指定すると拒否され、コマンドが続くこと。
    #[test]
    fn xline_rejects_identical_points() {
        let (mut s, mut doc) = setup();
        feed(&mut s, &mut doc, "XL");
        feed(&mut s, &mut doc, "5,5");
        feed(&mut s, &mut doc, "5,5");

        assert!(doc.entities().is_empty());
        assert!(s.has_active_tool(), "やり直せるはず");
        assert!(s.cmdline.history().any(|l| l.kind == LineKind::Error));
    }

    /// 線分を 2 本描いて両方選択した状態を用意する。
    fn setup_with_two_selected_lines() -> (Session, Document, Vec<cad_core::EntityId>) {
        let (mut s, mut doc) = setup();
        feed(&mut s, &mut doc, "L");
        feed(&mut s, &mut doc, "0,0");
        feed(&mut s, &mut doc, "10,0");
        enter(&mut s, &mut doc);
        feed(&mut s, &mut doc, "L");
        feed(&mut s, &mut doc, "0,10");
        feed(&mut s, &mut doc, "10,10");
        enter(&mut s, &mut doc);

        let ids: Vec<_> = doc.entities().ids().collect();
        for id in &ids {
            s.selection.insert(*id);
        }
        (s, doc, ids)
    }

    /// GROUP が選択をまとめ、既定名が付くこと。
    #[test]
    fn group_creates_a_group_with_a_default_name() {
        let (mut s, mut doc, ids) = setup_with_two_selected_lines();
        feed(&mut s, &mut doc, "G");
        enter(&mut s, &mut doc); // 既定名

        assert_eq!(doc.groups().len(), 1);
        let gid = doc
            .groups()
            .by_name("グループ1")
            .expect("既定名で作られるはず");
        for id in &ids {
            assert_eq!(doc.entities().get(*id).unwrap().group, Some(gid));
        }
    }

    /// 名前を指定してグループ化できること。
    #[test]
    fn group_accepts_an_explicit_name() {
        let (mut s, mut doc, _) = setup_with_two_selected_lines();
        feed(&mut s, &mut doc, "G");
        feed(&mut s, &mut doc, "WALL");
        assert!(doc.groups().by_name("WALL").is_some());
    }

    /// **グループの一員をクリックすると全体が選択されること**（AutoCAD の既定）。
    #[test]
    fn clicking_one_member_selects_the_whole_group() {
        let (mut s, mut doc, ids) = setup_with_two_selected_lines();
        feed(&mut s, &mut doc, "G");
        enter(&mut s, &mut doc);

        s.selection.clear();
        // 1 本目の線の上をクリックする。
        s.handle_click(Point2::new(5.0, 0.0), false, 1.0, &mut doc);

        assert_eq!(s.selection.len(), 2, "グループ全体が選ばれるはず");
        for id in &ids {
            assert!(s.selection.contains(*id));
        }
    }

    /// UNGROUP で解除され、要素は残ること。
    #[test]
    fn ungroup_releases_the_group_but_keeps_entities() {
        let (mut s, mut doc, ids) = setup_with_two_selected_lines();
        feed(&mut s, &mut doc, "G");
        enter(&mut s, &mut doc);
        assert_eq!(doc.groups().len(), 1);

        feed(&mut s, &mut doc, "UNG");
        enter(&mut s, &mut doc);

        assert_eq!(doc.groups().len(), 0, "グループは消える");
        assert_eq!(doc.entities().len(), 2, "要素は残る");
        for id in &ids {
            assert!(doc.entities().get(*id).unwrap().group.is_none());
        }
    }

    /// グループ操作が Undo で完全に戻ること。
    #[test]
    fn group_and_ungroup_round_trip_through_undo() {
        let (mut s, mut doc, ids) = setup_with_two_selected_lines();

        feed(&mut s, &mut doc, "G");
        enter(&mut s, &mut doc);
        let gid = doc.groups().by_name("グループ1").unwrap();

        feed(&mut s, &mut doc, "U"); // グループ化を取り消す
        assert_eq!(doc.groups().len(), 0);
        for id in &ids {
            assert!(doc.entities().get(*id).unwrap().group.is_none());
        }

        feed(&mut s, &mut doc, "REDO");
        assert_eq!(doc.groups().len(), 1);
        assert_eq!(doc.entities().get(ids[0]).unwrap().group, Some(gid));
    }

    /// EXPLODE がポリラインを線分へ分解すること。
    #[test]
    fn explode_splits_a_polyline_into_lines() {
        let (mut s, mut doc) = setup();
        feed(&mut s, &mut doc, "REC");
        feed(&mut s, &mut doc, "0,0");
        feed(&mut s, &mut doc, "10,10");
        assert_eq!(type_names(&doc), vec!["LWPOLYLINE"]);

        let id = doc.entities().ids().next().unwrap();
        s.selection.insert(id);
        feed(&mut s, &mut doc, "X");

        assert_eq!(doc.entities().len(), 4, "閉じた矩形なので 4 本");
        assert!(type_names(&doc).iter().all(|n| *n == "LINE"));
    }

    /// EXPLODE が Undo で戻ること。
    #[test]
    fn explode_undo_restores_the_polyline() {
        let (mut s, mut doc) = setup();
        feed(&mut s, &mut doc, "REC");
        feed(&mut s, &mut doc, "0,0");
        feed(&mut s, &mut doc, "10,10");
        let id = doc.entities().ids().next().unwrap();
        let before = doc.entities().get(id).unwrap().clone();

        s.selection.insert(id);
        feed(&mut s, &mut doc, "X");
        feed(&mut s, &mut doc, "U");

        assert_eq!(doc.entities().len(), 1);
        assert_eq!(doc.entities().get(id), Some(&before), "同じ ID・内容で戻る");
    }

    /// 分解できないものを選んだらエラーになること。
    #[test]
    fn explode_rejects_entities_that_cannot_be_exploded() {
        let (mut s, mut doc, _) = setup_with_two_selected_lines();
        feed(&mut s, &mut doc, "X");

        assert_eq!(doc.entities().len(), 2, "図面は変わらない");
        assert!(s.cmdline.history().any(|l| l.kind == LineKind::Error));
    }

    /// グループに属していない要素で UNGROUP を実行したらエラーになること。
    #[test]
    fn ungroup_without_a_group_reports_an_error() {
        let (mut s, mut doc, _) = setup_with_two_selected_lines();
        feed(&mut s, &mut doc, "UNG");
        enter(&mut s, &mut doc);
        assert!(s
            .cmdline
            .history()
            .any(|l| l.text.contains("グループに属していません")));
    }

    // ---- 段階 3: TRIM / EXTEND / FILLET / CHAMFER -------------------------
    //
    // この 4 つは「図形をクリックして指す」ことが対話の中心なので、
    // 文字列入力だけでは経路を通せない。`handle_click` を直接叩く。

    /// 拾い半径。テストの図形は座標が離れているので、この程度で取り違えない。
    const PICK: f64 = 0.5;

    /// キャンバスのクリックを模す。
    fn click(s: &mut Session, doc: &mut Document, x: f64, y: f64) {
        s.handle_click(Point2::new(x, y), false, PICK, doc);
    }

    /// 2 点を結ぶ線分を 1 本引く。
    fn draw_line(s: &mut Session, doc: &mut Document, a: &str, b: &str) {
        feed(s, doc, "L");
        feed(s, doc, a);
        feed(s, doc, b);
        enter(s, doc);
    }

    /// 図面に入っている線分を、始点の x 昇順で取り出す。
    fn lines_sorted(doc: &Document) -> Vec<cad_core::geom::Line> {
        let mut out: Vec<_> = doc
            .entities()
            .iter()
            .filter_map(|(_, e)| match &e.geom {
                cad_core::Geometry::Line(l) => Some(*l),
                _ => None,
            })
            .collect();
        out.sort_by(|p, q| {
            p.a.x
                .partial_cmp(&q.a.x)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        out
    }

    /// 横線を縦線で切ると、クリックした側が落ちること。
    ///
    /// ```text
    ///            |            |
    ///  0━━━━━━━━━┿━━ ×  →  0━━┥
    ///            10          10
    /// ```
    #[test]
    fn trim_removes_the_clicked_part_up_to_the_crossing() {
        let (mut s, mut doc) = setup();
        draw_line(&mut s, &mut doc, "0,0", "20,0");
        draw_line(&mut s, &mut doc, "10,-5", "10,5");

        feed(&mut s, &mut doc, "TR");
        click(&mut s, &mut doc, 15.0, 0.0);
        enter(&mut s, &mut doc);

        assert_eq!(doc.entities().len(), 2, "横線 1 本 + 縦線 1 本");
        let trimmed = lines_sorted(&doc)[0];
        assert!(eq_len(trimmed.a.x, 0.0), "始点は残る: {}", trimmed.a.x);
        assert!(eq_len(trimmed.b.x, 10.0), "交点で切れる: {}", trimmed.b.x);
    }

    /// 2 本の縦線に挟まれた中央をクリックすると、線分が 2 本に分断されること。
    #[test]
    fn trim_in_the_middle_splits_the_line_in_two() {
        let (mut s, mut doc) = setup();
        draw_line(&mut s, &mut doc, "0,0", "30,0");
        draw_line(&mut s, &mut doc, "10,-5", "10,5");
        draw_line(&mut s, &mut doc, "20,-5", "20,5");

        feed(&mut s, &mut doc, "TR");
        click(&mut s, &mut doc, 15.0, 0.0);
        enter(&mut s, &mut doc);

        assert_eq!(doc.entities().len(), 4, "横線 2 本 + 縦線 2 本");
        let horizontals: Vec<_> = lines_sorted(&doc)
            .into_iter()
            .filter(|l| eq_len(l.a.y, 0.0) && eq_len(l.b.y, 0.0))
            .collect();
        assert_eq!(horizontals.len(), 2, "真ん中が抜けて 2 本になる");
        assert!(eq_len(horizontals[0].b.x, 10.0));
        assert!(eq_len(horizontals[1].a.x, 20.0));
    }

    /// TRIM は続けて何本でも切れること（`ApplyAndContinue`）。
    #[test]
    fn trim_keeps_running_until_enter() {
        let (mut s, mut doc) = setup();
        draw_line(&mut s, &mut doc, "0,0", "20,0");
        draw_line(&mut s, &mut doc, "0,4", "20,4");
        draw_line(&mut s, &mut doc, "10,-5", "10,9");

        feed(&mut s, &mut doc, "TR");
        click(&mut s, &mut doc, 15.0, 0.0);
        assert!(s.has_active_tool(), "1 本切っても終わらない");
        click(&mut s, &mut doc, 15.0, 4.0);
        assert!(s.has_active_tool());
        enter(&mut s, &mut doc);
        assert!(!s.has_active_tool(), "Enter で終わる");

        for l in lines_sorted(&doc).iter().filter(|l| eq_len(l.a.x, 0.0)) {
            assert!(eq_len(l.b.x, 10.0), "2 本とも切れている: {}", l.b.x);
        }
    }

    /// TRIM の Undo が元の 1 本に戻すこと。
    #[test]
    fn trim_undo_restores_the_original_line() {
        let (mut s, mut doc) = setup();
        draw_line(&mut s, &mut doc, "0,0", "20,0");
        draw_line(&mut s, &mut doc, "10,-5", "10,5");
        let before = lines_sorted(&doc);

        feed(&mut s, &mut doc, "TR");
        click(&mut s, &mut doc, 15.0, 0.0);
        enter(&mut s, &mut doc);
        feed(&mut s, &mut doc, "U");

        assert_eq!(lines_sorted(&doc), before, "切る前の形に戻る");
    }

    /// 交点が無い図形をクリックしたらエラーを出し、図面は変わらないこと。
    #[test]
    fn trim_without_any_crossing_reports_an_error() {
        let (mut s, mut doc) = setup();
        draw_line(&mut s, &mut doc, "0,0", "20,0");
        draw_line(&mut s, &mut doc, "0,5", "20,5");

        feed(&mut s, &mut doc, "TR");
        click(&mut s, &mut doc, 10.0, 0.0);

        assert_eq!(doc.entities().len(), 2, "図面は変わらない");
        assert!(s.cmdline.history().any(|l| l.kind == LineKind::Error));
    }

    /// クリックした側の端が、交点まで伸びること。
    #[test]
    fn extend_grows_the_clicked_end_to_the_boundary() {
        let (mut s, mut doc) = setup();
        draw_line(&mut s, &mut doc, "0,0", "5,0");
        draw_line(&mut s, &mut doc, "10,-5", "10,5");

        feed(&mut s, &mut doc, "EX");
        click(&mut s, &mut doc, 4.0, 0.0); // 終点側をクリック
        enter(&mut s, &mut doc);

        let grown = lines_sorted(&doc)[0];
        assert!(eq_len(grown.a.x, 0.0), "始点は動かない: {}", grown.a.x);
        assert!(eq_len(grown.b.x, 10.0), "境界まで伸びる: {}", grown.b.x);
    }

    /// 始点側をクリックしたら、そちら側が伸びること。
    #[test]
    fn extend_grows_the_start_end_when_clicked_near_it() {
        let (mut s, mut doc) = setup();
        draw_line(&mut s, &mut doc, "10,0", "20,0");
        draw_line(&mut s, &mut doc, "0,-5", "0,5");

        feed(&mut s, &mut doc, "EX");
        click(&mut s, &mut doc, 11.0, 0.0); // 始点側をクリック
        enter(&mut s, &mut doc);

        let grown = lines_sorted(&doc)[0];
        assert!(eq_len(grown.a.x, 0.0), "始点が伸びる: {}", grown.a.x);
        assert!(eq_len(grown.b.x, 20.0), "終点は動かない: {}", grown.b.x);
    }

    /// EXTEND の Undo が元の長さに戻すこと。
    #[test]
    fn extend_undo_restores_the_original_length() {
        let (mut s, mut doc) = setup();
        draw_line(&mut s, &mut doc, "0,0", "5,0");
        draw_line(&mut s, &mut doc, "10,-5", "10,5");
        let before = lines_sorted(&doc);

        feed(&mut s, &mut doc, "EX");
        click(&mut s, &mut doc, 4.0, 0.0);
        enter(&mut s, &mut doc);
        feed(&mut s, &mut doc, "U");

        assert_eq!(lines_sorted(&doc), before, "伸ばす前に戻る");
    }

    /// 伸ばす先に何も無ければエラーを出し、図面は変わらないこと。
    #[test]
    fn extend_without_any_boundary_reports_an_error() {
        let (mut s, mut doc) = setup();
        draw_line(&mut s, &mut doc, "0,0", "5,0");
        draw_line(&mut s, &mut doc, "0,5", "5,5");

        feed(&mut s, &mut doc, "EX");
        click(&mut s, &mut doc, 4.0, 0.0);

        assert_eq!(lines_sorted(&doc)[0].b.x, 5.0, "図面は変わらない");
        assert!(s.cmdline.history().any(|l| l.kind == LineKind::Error));
    }

    /// 直角の角を丸めると、2 本が接点まで縮み、円弧が 1 つ増えること。
    #[test]
    fn fillet_shortens_both_lines_and_inserts_an_arc() {
        let (mut s, mut doc) = setup();
        draw_line(&mut s, &mut doc, "0,0", "10,0");
        draw_line(&mut s, &mut doc, "0,0", "0,10");

        feed(&mut s, &mut doc, "F");
        feed(&mut s, &mut doc, "R"); // 半径を指定
        feed(&mut s, &mut doc, "2");
        click(&mut s, &mut doc, 8.0, 0.0);
        click(&mut s, &mut doc, 0.0, 8.0);
        enter(&mut s, &mut doc);

        assert_eq!(type_names(&doc).len(), 3, "線分 2 本 + 円弧 1 つ");
        let arcs: Vec<_> = doc
            .entities()
            .iter()
            .filter_map(|(_, e)| match &e.geom {
                cad_core::Geometry::Arc(a) => Some(*a),
                _ => None,
            })
            .collect();
        assert_eq!(arcs.len(), 1, "円弧が 1 つできる");
        assert!(eq_len(arcs[0].radius, 2.0), "半径 {}", arcs[0].radius);
        // 半径 2 の角丸めなら、両線分は原点から 2 のところで止まる。
        for l in lines_sorted(&doc) {
            let near = if l.a.x.abs() + l.a.y.abs() < l.b.x.abs() + l.b.y.abs() {
                l.a
            } else {
                l.b
            };
            assert!(
                eq_len(near.x + near.y, 2.0),
                "接点まで縮む: ({}, {})",
                near.x,
                near.y
            );
        }
    }

    /// 半径を指定していないと、既定値（10）が使われること。
    ///
    /// AutoCAD と同じく値はコマンドをまたいで残るので、既定値の存在自体を固定する。
    #[test]
    fn fillet_uses_the_remembered_radius() {
        let (mut s, mut doc) = setup();
        draw_line(&mut s, &mut doc, "0,0", "40,0");
        draw_line(&mut s, &mut doc, "0,0", "0,40");

        feed(&mut s, &mut doc, "F");
        feed(&mut s, &mut doc, "R");
        feed(&mut s, &mut doc, "5");
        click(&mut s, &mut doc, 30.0, 0.0);
        click(&mut s, &mut doc, 0.0, 30.0);
        enter(&mut s, &mut doc);

        // 半径を指定し直さずにもう一度。
        draw_line(&mut s, &mut doc, "100,0", "140,0");
        draw_line(&mut s, &mut doc, "100,0", "100,40");
        feed(&mut s, &mut doc, "F");
        click(&mut s, &mut doc, 130.0, 0.0);
        click(&mut s, &mut doc, 100.0, 30.0);
        enter(&mut s, &mut doc);

        let radii: Vec<f64> = doc
            .entities()
            .iter()
            .filter_map(|(_, e)| match &e.geom {
                cad_core::Geometry::Arc(a) => Some(a.radius),
                _ => None,
            })
            .collect();
        assert_eq!(radii.len(), 2, "角丸めが 2 か所");
        assert!(
            radii.iter().all(|r| eq_len(*r, 5.0)),
            "前に指定した半径が残る: {radii:?}"
        );
    }

    /// FILLET の Undo が線分 2 本の状態に戻すこと。
    #[test]
    fn fillet_undo_restores_both_lines_and_removes_the_arc() {
        let (mut s, mut doc) = setup();
        draw_line(&mut s, &mut doc, "0,0", "10,0");
        draw_line(&mut s, &mut doc, "0,0", "0,10");
        let before = lines_sorted(&doc);

        feed(&mut s, &mut doc, "F");
        feed(&mut s, &mut doc, "R");
        feed(&mut s, &mut doc, "2");
        click(&mut s, &mut doc, 8.0, 0.0);
        click(&mut s, &mut doc, 0.0, 8.0);
        enter(&mut s, &mut doc);
        feed(&mut s, &mut doc, "U");

        assert_eq!(doc.entities().len(), 2, "円弧が消える");
        assert_eq!(lines_sorted(&doc), before, "線分が元の長さに戻る");
    }

    /// 面取りは、2 本を縮めて間に線分を 1 本入れること。
    #[test]
    fn chamfer_shortens_both_lines_and_inserts_a_line() {
        let (mut s, mut doc) = setup();
        draw_line(&mut s, &mut doc, "0,0", "10,0");
        draw_line(&mut s, &mut doc, "0,0", "0,10");

        feed(&mut s, &mut doc, "CHA");
        feed(&mut s, &mut doc, "D"); // 距離を指定
        feed(&mut s, &mut doc, "3"); // 1 本目
        feed(&mut s, &mut doc, "3"); // 2 本目
        click(&mut s, &mut doc, 8.0, 0.0);
        click(&mut s, &mut doc, 0.0, 8.0);
        enter(&mut s, &mut doc);

        assert_eq!(doc.entities().len(), 3, "線分 2 本 + 面取り 1 本");
        assert!(
            type_names(&doc).iter().all(|n| *n == "LINE"),
            "円弧はできない: {:?}",
            type_names(&doc)
        );
        // (3, 0) と (0, 3) を結ぶ線分ができているはず。
        let face = lines_sorted(&doc)
            .into_iter()
            .find(|l| eq_len(l.a.x + l.a.y, 3.0) && eq_len(l.b.x + l.b.y, 3.0));
        assert!(face.is_some(), "面取りの線分が見つからない");
    }

    /// 距離を非対称にすると、それぞれの線分が別々の量だけ縮むこと。
    #[test]
    fn chamfer_honours_asymmetric_distances() {
        let (mut s, mut doc) = setup();
        draw_line(&mut s, &mut doc, "0,0", "10,0");
        draw_line(&mut s, &mut doc, "0,0", "0,10");

        feed(&mut s, &mut doc, "CHA");
        feed(&mut s, &mut doc, "D");
        feed(&mut s, &mut doc, "2"); // 1 本目（横線）
        feed(&mut s, &mut doc, "4"); // 2 本目（縦線）
        click(&mut s, &mut doc, 8.0, 0.0);
        click(&mut s, &mut doc, 0.0, 8.0);
        enter(&mut s, &mut doc);

        let face = lines_sorted(&doc)
            .into_iter()
            .find(|l| !eq_len(l.a.x, l.b.x) && !eq_len(l.a.y, l.b.y))
            .expect("面取りの線分があるはず");
        // 横線側で 2、縦線側で 4 の位置を結ぶ。
        let (on_x, on_y) = if eq_len(face.a.y, 0.0) {
            (face.a, face.b)
        } else {
            (face.b, face.a)
        };
        assert!(eq_len(on_x.x, 2.0), "横線は 2 縮む: {}", on_x.x);
        assert!(eq_len(on_y.y, 4.0), "縦線は 4 縮む: {}", on_y.y);
    }

    /// 交わらない 2 本を指定したらエラーになり、図面は変わらないこと。
    #[test]
    fn fillet_on_parallel_lines_reports_an_error() {
        let (mut s, mut doc) = setup();
        draw_line(&mut s, &mut doc, "0,0", "10,0");
        draw_line(&mut s, &mut doc, "0,5", "10,5");

        feed(&mut s, &mut doc, "F");
        feed(&mut s, &mut doc, "R");
        feed(&mut s, &mut doc, "1");
        click(&mut s, &mut doc, 5.0, 0.0);
        click(&mut s, &mut doc, 5.0, 5.0);

        assert_eq!(doc.entities().len(), 2, "図面は変わらない");
        assert!(s.cmdline.history().any(|l| l.kind == LineKind::Error));
    }

    /// 同じ線分を 2 回クリックしたら断られること。
    #[test]
    fn fillet_rejects_the_same_line_twice() {
        let (mut s, mut doc) = setup();
        draw_line(&mut s, &mut doc, "0,0", "10,0");
        draw_line(&mut s, &mut doc, "0,0", "0,10");

        feed(&mut s, &mut doc, "F");
        click(&mut s, &mut doc, 8.0, 0.0);
        click(&mut s, &mut doc, 4.0, 0.0); // 同じ線分

        assert_eq!(doc.entities().len(), 2, "図面は変わらない");
        assert!(s
            .cmdline
            .history()
            .any(|l| l.text.contains("別の線分をクリック")));
    }

    /// 半径や距離に 0 以下を入れたら断られること。
    #[test]
    fn corner_values_must_be_positive() {
        let (mut s, mut doc) = setup();
        feed(&mut s, &mut doc, "F");
        feed(&mut s, &mut doc, "R");
        feed(&mut s, &mut doc, "0");
        assert!(s.cmdline.history().any(|l| l.kind == LineKind::Error));

        feed(&mut s, &mut doc, "-1");
        assert!(s.has_active_tool(), "断られてもコマンドは続く");
    }

    // ---- コンポーネント ---------------------------------------------------

    /// 線分 2 本を引いて、両方を選択した状態を作る。
    fn setup_two_lines_selected() -> (Session, Document) {
        let (mut s, mut doc) = setup();
        draw_line(&mut s, &mut doc, "0,0", "10,0");
        draw_line(&mut s, &mut doc, "0,5", "10,5");
        for id in doc.entities().ids().collect::<Vec<_>>() {
            s.selection.insert(id);
        }
        (s, doc)
    }

    /// 図面内のインスタンスの数。
    fn instance_count(doc: &Document) -> usize {
        doc.entities()
            .iter()
            .filter(|(_, e)| matches!(e.geom, cad_core::Geometry::Instance(_)))
            .count()
    }

    /// **`COMPONENT` は選択をその場でインスタンスに置き換えること。**
    ///
    /// AutoCAD の `BLOCK` は選択を消すだけで画面から図形が無くなる。
    /// Figma と同じく置き換える形にしたことの確認。
    #[test]
    fn component_replaces_the_selection_with_one_instance() {
        let (mut s, mut doc) = setup_two_lines_selected();

        // 選択が既にあるので、`B` の直後に `SelectionReady` が届いて基点待ちになる
        // （`Session::start_tool` の `wants_selection` の分岐）。
        feed(&mut s, &mut doc, "B");
        feed(&mut s, &mut doc, "0,0"); // 基点
        feed(&mut s, &mut doc, "枠"); // 名前

        assert_eq!(doc.definitions().len(), 1, "定義ができる");
        assert!(doc.definitions().by_name("枠").is_some());
        assert_eq!(
            doc.entities().len(),
            1,
            "線分 2 本がインスタンス 1 つに置き換わる"
        );
        assert_eq!(instance_count(&doc), 1);
        let def = doc.definitions().by_name("枠").expect("あるはず");
        assert_eq!(
            doc.definitions().get(def).expect("引ける").entities.len(),
            2,
            "定義の中身は線分 2 本"
        );
    }

    /// 名前を省略すると既定名が付くこと。
    #[test]
    fn component_uses_a_default_name_on_enter() {
        let (mut s, mut doc) = setup_two_lines_selected();

        feed(&mut s, &mut doc, "B");
        feed(&mut s, &mut doc, "0,0");
        enter(&mut s, &mut doc); // 既定名

        assert!(
            doc.definitions().by_name("コンポーネント1").is_some(),
            "既定名が付く。実際: {:?}",
            doc.definitions()
                .iter()
                .map(|(_, d)| d.name.clone())
                .collect::<Vec<_>>()
        );
    }

    /// **`COMPONENT` の Undo が 1 回で戻ること。**
    ///
    /// 定義作成・選択削除・配置の 3 コマンドを `MacroCommand` で 1 手にまとめている。
    /// 別々に積むと Undo が 3 回必要になり「1 操作 = 1 Undo」が崩れる。
    #[test]
    fn component_undoes_in_a_single_step() {
        let (mut s, mut doc) = setup_two_lines_selected();
        feed(&mut s, &mut doc, "B");
        feed(&mut s, &mut doc, "0,0");
        feed(&mut s, &mut doc, "枠");

        feed(&mut s, &mut doc, "U");

        assert_eq!(doc.entities().len(), 2, "線分 2 本が戻る");
        assert_eq!(instance_count(&doc), 0);
        assert_eq!(doc.definitions().len(), 0, "定義も消える");
    }

    /// 同名のコンポーネントを断ること。
    #[test]
    fn component_rejects_a_duplicate_name() {
        let (mut s, mut doc) = setup_two_lines_selected();
        feed(&mut s, &mut doc, "B");
        feed(&mut s, &mut doc, "0,0");
        feed(&mut s, &mut doc, "枠");

        // もう一度、線分を引いて同じ名前で作ろうとする。
        draw_line(&mut s, &mut doc, "0,20", "10,20");
        let id = doc.entities().ids().last().expect("あるはず");
        s.selection.clear();
        s.selection.insert(id);

        feed(&mut s, &mut doc, "B");
        feed(&mut s, &mut doc, "0,20");
        feed(&mut s, &mut doc, "枠");

        assert_eq!(doc.definitions().len(), 1, "定義は増えない");
        assert!(s.cmdline.history().any(|l| l.text.contains("既にあります")));
    }

    /// `INSERT` で配置でき、回転と倍率が効くこと。
    #[test]
    fn insert_places_a_second_instance_with_rotation_and_scale() {
        let (mut s, mut doc) = setup_two_lines_selected();
        feed(&mut s, &mut doc, "B");
        feed(&mut s, &mut doc, "0,0");
        feed(&mut s, &mut doc, "枠");
        assert_eq!(instance_count(&doc), 1);

        feed(&mut s, &mut doc, "I");
        feed(&mut s, &mut doc, "枠");
        feed(&mut s, &mut doc, "100,0"); // 位置
        feed(&mut s, &mut doc, "90"); // 回転（度）
        feed(&mut s, &mut doc, "2"); // 倍率

        assert_eq!(instance_count(&doc), 2, "2 つ目が置かれる");
        let placed = doc
            .entities()
            .iter()
            .filter_map(|(_, e)| match &e.geom {
                cad_core::Geometry::Instance(i) => Some(i.placement),
                _ => None,
            })
            .last()
            .expect("あるはず");
        assert!(eq_len(placed.origin.x, 100.0));
        assert!(
            eq_len(placed.rotation, std::f64::consts::FRAC_PI_2),
            "度で入力した 90 がラジアンになる: {}",
            placed.rotation
        );
        assert!(eq_len(placed.scale, 2.0));
        assert!(!placed.flipped);
    }

    /// `INSERT` の回転と倍率は Enter で既定値になること。
    #[test]
    fn insert_defaults_rotation_to_zero_and_scale_to_one() {
        let (mut s, mut doc) = setup_two_lines_selected();
        feed(&mut s, &mut doc, "B");
        feed(&mut s, &mut doc, "0,0");
        feed(&mut s, &mut doc, "枠");

        feed(&mut s, &mut doc, "I");
        feed(&mut s, &mut doc, "枠");
        feed(&mut s, &mut doc, "50,50");
        enter(&mut s, &mut doc); // 回転 0
        enter(&mut s, &mut doc); // 倍率 1

        let placed = doc
            .entities()
            .iter()
            .filter_map(|(_, e)| match &e.geom {
                cad_core::Geometry::Instance(i) => Some(i.placement),
                _ => None,
            })
            .last()
            .expect("あるはず");
        assert!(eq_len(placed.rotation, 0.0));
        assert!(eq_len(placed.scale, 1.0));
    }

    /// 存在しないコンポーネント名を断り、あるものを案内すること。
    #[test]
    fn insert_reports_the_available_component_names() {
        let (mut s, mut doc) = setup_two_lines_selected();
        feed(&mut s, &mut doc, "B");
        feed(&mut s, &mut doc, "0,0");
        feed(&mut s, &mut doc, "枠");

        feed(&mut s, &mut doc, "I");
        feed(&mut s, &mut doc, "ない名前");

        assert!(
            s.cmdline.history().any(|l| l.text.contains("枠")),
            "あるコンポーネント名を案内する"
        );
        assert!(s.has_active_tool(), "断られてもコマンドは続く");
    }

    /// 倍率 0 と負値を断ること（反転は MIRROR）。
    #[test]
    fn insert_rejects_zero_and_negative_scale() {
        let (mut s, mut doc) = setup_two_lines_selected();
        feed(&mut s, &mut doc, "B");
        feed(&mut s, &mut doc, "0,0");
        feed(&mut s, &mut doc, "枠");
        let before = instance_count(&doc);

        for bad in ["0", "-2"] {
            feed(&mut s, &mut doc, "I");
            feed(&mut s, &mut doc, "枠");
            feed(&mut s, &mut doc, "10,10");
            enter(&mut s, &mut doc); // 回転は 0
            feed(&mut s, &mut doc, bad);

            assert_eq!(instance_count(&doc), before, "倍率 {bad} では置かれない");
            assert!(s.has_active_tool(), "断られてもコマンドは続く");
            // **Reject 後もツールは生きている。** 中断しないと次の入力が
            // このツールに食われる。
            s.cancel();
        }
        assert!(s.cmdline.history().any(|l| l.kind == LineKind::Error));
    }

    /// **`REDEFINE` で定義を差し替えると全インスタンスが変わること。**
    ///
    /// これが「ブロックの再定義」の中核。
    #[test]
    fn redefine_updates_every_instance() {
        let (mut s, mut doc) = setup_two_lines_selected();
        feed(&mut s, &mut doc, "B");
        feed(&mut s, &mut doc, "0,0");
        feed(&mut s, &mut doc, "枠");

        // 2 つ目を配置。
        feed(&mut s, &mut doc, "I");
        feed(&mut s, &mut doc, "枠");
        feed(&mut s, &mut doc, "100,0");
        enter(&mut s, &mut doc);
        enter(&mut s, &mut doc);
        assert_eq!(instance_count(&doc), 2);

        let def = doc.definitions().by_name("枠").expect("あるはず");
        assert_eq!(
            doc.definitions().get(def).expect("引ける").entities.len(),
            2
        );

        // 新しい中身（円 1 つ）を描いて選び、定義を差し替える。
        feed(&mut s, &mut doc, "C");
        feed(&mut s, &mut doc, "0,0");
        feed(&mut s, &mut doc, "3");
        let circle = doc.entities().ids().last().expect("あるはず");
        s.selection.clear();
        s.selection.insert(circle);

        feed(&mut s, &mut doc, "RD");
        feed(&mut s, &mut doc, "0,0"); // 基点
        feed(&mut s, &mut doc, "枠"); // 差し替え先

        let def = doc.definitions().by_name("枠").expect("あるはず");
        assert_eq!(
            doc.definitions().get(def).expect("引ける").entities.len(),
            1,
            "定義の中身が円 1 つになる"
        );
        assert_eq!(
            instance_count(&doc),
            2,
            "**インスタンスは 2 つのまま**（触っていない）"
        );

        // 解決結果が円になっていること = 両方のインスタンスに反映されている。
        for (_, e) in doc.entities().iter() {
            let cad_core::Geometry::Instance(i) = &e.geom else {
                continue;
            };
            let parts = cad_core::component::resolve(i, doc.definitions());
            assert_eq!(parts.len(), 1, "中身は 1 つ");
            assert!(
                matches!(parts[0], cad_core::Geometry::Circle(_)),
                "円に変わっている: {:?}",
                parts[0]
            );
        }
    }

    /// 差し替え先のコンポーネント自身を中身にしようとしたら断ること（循環）。
    #[test]
    fn redefine_rejects_a_self_reference() {
        let (mut s, mut doc) = setup_two_lines_selected();
        feed(&mut s, &mut doc, "B");
        feed(&mut s, &mut doc, "0,0");
        feed(&mut s, &mut doc, "枠");

        // 置き換わったインスタンス自身を選んで、同じ定義へ差し替えようとする。
        let inst = doc.entities().ids().next().expect("あるはず");
        s.selection.clear();
        s.selection.insert(inst);

        feed(&mut s, &mut doc, "RD");
        feed(&mut s, &mut doc, "0,0");
        feed(&mut s, &mut doc, "枠");

        let def = doc.definitions().by_name("枠").expect("あるはず");
        assert_eq!(
            doc.definitions().get(def).expect("引ける").entities.len(),
            2,
            "定義は変わらない"
        );
        assert!(s.cmdline.history().any(|l| l.kind == LineKind::Error));
    }

    /// **`EXPLODE` でインスタンスが中身へ戻ること。**
    #[test]
    fn explode_expands_a_component_instance() {
        let (mut s, mut doc) = setup_two_lines_selected();
        feed(&mut s, &mut doc, "B");
        feed(&mut s, &mut doc, "0,0");
        feed(&mut s, &mut doc, "枠");
        assert_eq!(doc.entities().len(), 1);

        let inst = doc.entities().ids().next().expect("あるはず");
        s.selection.clear();
        s.selection.insert(inst);
        feed(&mut s, &mut doc, "X");
        enter(&mut s, &mut doc);

        assert_eq!(doc.entities().len(), 2, "線分 2 本に戻る");
        assert_eq!(instance_count(&doc), 0);
        assert_eq!(doc.definitions().len(), 1, "定義は残る");
    }

    /// インスタンスに MOVE / ROTATE / MIRROR が効くこと。
    #[test]
    fn transforms_apply_to_an_instance() {
        let (mut s, mut doc) = setup_two_lines_selected();
        feed(&mut s, &mut doc, "B");
        feed(&mut s, &mut doc, "0,0");
        feed(&mut s, &mut doc, "枠");

        let inst = doc.entities().ids().next().expect("あるはず");
        let placement_of = |d: &Document| {
            d.entities()
                .iter()
                .filter_map(|(_, e)| match &e.geom {
                    cad_core::Geometry::Instance(i) => Some(i.placement),
                    _ => None,
                })
                .next()
                .expect("インスタンスがあるはず")
        };

        // MOVE
        s.selection.clear();
        s.selection.insert(inst);
        feed(&mut s, &mut doc, "M");
        feed(&mut s, &mut doc, "0,0");
        feed(&mut s, &mut doc, "10,20");
        let pl = placement_of(&doc);
        assert!(
            eq_len(pl.origin.x, 10.0) && eq_len(pl.origin.y, 20.0),
            "{pl:?}"
        );

        // ROTATE 90 度
        let inst = doc.entities().ids().next().expect("あるはず");
        s.selection.clear();
        s.selection.insert(inst);
        feed(&mut s, &mut doc, "RO");
        feed(&mut s, &mut doc, "0,0");
        feed(&mut s, &mut doc, "90");
        let pl = placement_of(&doc);
        assert!(
            eq_len(pl.rotation, std::f64::consts::FRAC_PI_2),
            "回転が配置へ合成される: {}",
            pl.rotation
        );

        // MIRROR（既定で元を残すので、反転したものが 1 つ増える）
        let inst = doc.entities().ids().next().expect("あるはず");
        s.selection.clear();
        s.selection.insert(inst);
        feed(&mut s, &mut doc, "MI");
        feed(&mut s, &mut doc, "0,0");
        feed(&mut s, &mut doc, "0,1");
        enter(&mut s, &mut doc); // 既定 = 元を残す
        assert_eq!(instance_count(&doc), 2, "鏡像が増える");
        assert!(
            doc.entities().iter().any(|(_, e)| matches!(
                &e.geom,
                cad_core::Geometry::Instance(i) if i.placement.flipped
            )),
            "**反転フラグが立ったインスタンスができる**"
        );
    }

    /// インスタンスをクリックで選択できること（`dist_to` が中身へ届いている）。
    #[test]
    fn an_instance_can_be_picked_by_clicking_its_contents() {
        let (mut s, mut doc) = setup_two_lines_selected();
        feed(&mut s, &mut doc, "B");
        feed(&mut s, &mut doc, "0,0");
        feed(&mut s, &mut doc, "枠");
        s.selection.clear();

        // 定義の中身の線分（0,0)-(10,0) の上をクリックする。
        click(&mut s, &mut doc, 5.0, 0.0);
        assert_eq!(s.selection.len(), 1, "インスタンスが選ばれる");
    }

    /// ZOOM EXTENTS がインスタンスの範囲を含むこと。
    #[test]
    fn the_drawing_bbox_covers_instances() {
        let (mut s, mut doc) = setup_two_lines_selected();
        feed(&mut s, &mut doc, "B");
        feed(&mut s, &mut doc, "0,0");
        feed(&mut s, &mut doc, "枠");

        // 遠くにもう 1 つ置く。
        feed(&mut s, &mut doc, "I");
        feed(&mut s, &mut doc, "枠");
        feed(&mut s, &mut doc, "1000,0");
        enter(&mut s, &mut doc);
        enter(&mut s, &mut doc);

        let b = doc.bbox();
        assert!(!b.is_empty(), "範囲が空でない");
        assert!(
            b.max.x >= 1000.0,
            "遠くのインスタンスが含まれる: max.x = {}",
            b.max.x
        );
    }

    // ---- ラバーバンド（確定前のプレビュー） -------------------------------
    //
    // 2026-08-13 に「MOVE で確定前のプレビューが出ない」との報告を受けて追加。
    // 描画そのものは目で見るしかないが、**`Session::preview` が図形を返すか**は
    // ここで固定できる。返ってさえいれば、あとは描画経路の問題に絞り込める。

    /// 線分 1 本を選択した状態を作る。
    fn setup_one_line_selected() -> (Session, Document) {
        let (mut s, mut doc) = setup();
        draw_line(&mut s, &mut doc, "0,0", "10,0");
        let id = doc.entities().ids().next().expect("あるはず");
        s.selection.insert(id);
        (s, doc)
    }

    /// **MOVE の基点を指定したあと、プレビューが返ること。**
    #[test]
    fn move_returns_a_preview_after_the_base_point() {
        let (mut s, mut doc) = setup_one_line_selected();

        feed(&mut s, &mut doc, "M");
        feed(&mut s, &mut doc, "0,0"); // 基点

        let preview = s.preview(Some(Point2::new(5.0, 5.0)), &doc);
        assert_eq!(preview.len(), 1, "選択 1 件ぶんのラバーバンドが返る");
        let cad_core::Geometry::Line(l) = &preview[0] else {
            panic!("線分のはず: {:?}", preview[0]);
        };
        assert!(
            eq_len(l.a.x, 5.0) && eq_len(l.a.y, 5.0),
            "カーソルまで動く: {:?}",
            l.a
        );
    }

    /// 基点を指定する前はプレビューが空であること（動かす量が決まらない）。
    #[test]
    fn move_has_no_preview_before_the_base_point() {
        let (mut s, mut doc) = setup_one_line_selected();
        feed(&mut s, &mut doc, "M");
        assert!(s.preview(Some(Point2::new(5.0, 5.0)), &doc).is_empty());
    }

    /// カーソルが画面外なら空であること。
    #[test]
    fn no_preview_without_a_cursor() {
        let (mut s, mut doc) = setup_one_line_selected();
        feed(&mut s, &mut doc, "M");
        feed(&mut s, &mut doc, "0,0");
        assert!(s.preview(None, &doc).is_empty());
    }

    /// COPY / STRETCH / ROTATE / SCALE でもプレビューが返ること。
    #[test]
    fn transform_tools_return_a_preview() {
        for (cmd, steps) in [
            ("CO", vec!["0,0"]),
            ("S", vec!["0,0"]),
            ("RO", vec!["0,0"]),
            ("SC", vec!["0,0"]),
        ] {
            let (mut s, mut doc) = setup_one_line_selected();
            feed(&mut s, &mut doc, cmd);
            for step in &steps {
                feed(&mut s, &mut doc, step);
            }
            assert!(
                !s.preview(Some(Point2::new(5.0, 5.0)), &doc).is_empty(),
                "{cmd} でプレビューが返らない"
            );
        }
    }

    /// 作図コマンドでもプレビューが返ること。
    #[test]
    fn draw_tools_return_a_preview() {
        for (cmd, steps) in [
            ("L", vec!["0,0"]),
            ("C", vec!["0,0"]),
            ("REC", vec!["0,0"]),
            ("PL", vec!["0,0"]),
        ] {
            let (mut s, mut doc) = setup();
            feed(&mut s, &mut doc, cmd);
            for step in &steps {
                feed(&mut s, &mut doc, step);
            }
            assert!(
                !s.preview(Some(Point2::new(5.0, 5.0)), &doc).is_empty(),
                "{cmd} でプレビューが返らない"
            );
        }
    }

    /// **クリックで基点を置いた場合もプレビューが返ること。**
    ///
    /// 実際の操作はクリックなので、文字入力とは別に経路を押さえる
    /// （`handle_click` は `wants_entity` の分岐を通るため、文字入力と経路が違う）。
    #[test]
    fn move_returns_a_preview_when_the_base_point_is_clicked() {
        let (mut s, mut doc) = setup_one_line_selected();

        feed(&mut s, &mut doc, "M");
        click(&mut s, &mut doc, 0.0, 0.0); // 基点をクリックで置く

        let preview = s.preview(Some(Point2::new(5.0, 5.0)), &doc);
        assert_eq!(preview.len(), 1, "クリック経路でもラバーバンドが返る");
    }

    /// **オブジェクト選択を待っている間はプレビューを出さないこと。**
    ///
    /// 選択中にラバーバンドが出ると邪魔になる。
    #[test]
    fn no_preview_while_awaiting_selection() {
        let (mut s, mut doc) = setup();
        draw_line(&mut s, &mut doc, "0,0", "10,0");
        // 選択せずに MOVE を始めると、選択待ちになる。
        feed(&mut s, &mut doc, "M");
        assert!(s.preview(Some(Point2::new(5.0, 5.0)), &doc).is_empty());
    }

    // ---- パラメータ（段階 2） ---------------------------------------------
    //
    // **コマンドラインだけで一通り試せること**を固定する。
    // パラメータパネル（段階 3）が入るまでの唯一の入口なので、
    // ここが動かないと段階 2 は使えない。

    /// 線分 1 本のコンポーネント「窓」を作り、インスタンス 1 つが置かれた状態にする。
    fn setup_component() -> (Session, Document) {
        let (mut s, mut doc) = setup();
        draw_line(&mut s, &mut doc, "0,0", "10,0");
        let id = doc.entities().ids().next().expect("あるはず");
        s.selection.insert(id);

        feed(&mut s, &mut doc, "B");
        feed(&mut s, &mut doc, "0,0");
        feed(&mut s, &mut doc, "窓");
        (s, doc)
    }

    /// 解決された線分の終点 X。
    fn resolved_end_x(doc: &Document) -> f64 {
        let (_, e) = doc
            .entities()
            .iter()
            .find(|(_, e)| matches!(e.geom, cad_core::Geometry::Instance(_)))
            .expect("インスタンスがあるはず");
        let cad_core::Geometry::Instance(i) = &e.geom else {
            panic!()
        };
        match &cad_core::component::resolve(i, doc.definitions())[0] {
            cad_core::Geometry::Line(l) => l.b.x,
            other => panic!("線分のはず: {other:?}"),
        }
    }

    /// **`PARAM` → `BIND` → `PSET` の一連が動くこと。**
    ///
    /// これが段階 2 の受け入れ基準そのもの。
    #[test]
    fn param_bind_and_pset_drive_the_geometry() {
        let (mut s, mut doc) = setup_component();

        // パラメータを宣言する。
        feed(&mut s, &mut doc, "PA");
        feed(&mut s, &mut doc, "窓");
        feed(&mut s, &mut doc, "幅");
        feed(&mut s, &mut doc, "900");
        let def = doc.definitions().by_name("窓").expect("あるはず");
        assert_eq!(
            doc.definitions().get(def).expect("引ける").params.len(),
            1,
            "パラメータが宣言される"
        );

        // 線分の終点 X に束縛する。
        feed(&mut s, &mut doc, "BI");
        feed(&mut s, &mut doc, "窓");
        feed(&mut s, &mut doc, "0"); // 要素番号
        feed(&mut s, &mut doc, "終点X"); // スロット
        feed(&mut s, &mut doc, "幅"); // 式
        assert_eq!(
            doc.definitions().get(def).expect("引ける").bindings.len(),
            1,
            "束縛ができる"
        );
        assert!(eq_len(resolved_end_x(&doc), 900.0), "既定値が効く");

        // インスタンスのパラメータを変える。
        let inst = doc.entities().ids().next().expect("あるはず");
        let _ = inst;
        feed(&mut s, &mut doc, "PS");
        click(&mut s, &mut doc, 5.0, 0.0); // インスタンスをクリック
        feed(&mut s, &mut doc, "幅");
        feed(&mut s, &mut doc, "1800");
        assert!(eq_len(resolved_end_x(&doc), 1800.0), "上書きが効く");

        // リセットで既定値へ戻る。
        feed(&mut s, &mut doc, "PS");
        click(&mut s, &mut doc, 5.0, 0.0);
        feed(&mut s, &mut doc, "幅");
        feed(&mut s, &mut doc, "R");
        assert!(eq_len(resolved_end_x(&doc), 900.0), "リセットで既定値へ");
    }

    /// **式が書けること。** 値の入力にも式が使える。
    #[test]
    fn expressions_can_be_used_for_values() {
        let (mut s, mut doc) = setup_component();
        feed(&mut s, &mut doc, "PA");
        feed(&mut s, &mut doc, "窓");
        feed(&mut s, &mut doc, "幅");
        feed(&mut s, &mut doc, "900");

        feed(&mut s, &mut doc, "BI");
        feed(&mut s, &mut doc, "窓");
        feed(&mut s, &mut doc, "0");
        feed(&mut s, &mut doc, "終点X");
        feed(&mut s, &mut doc, "幅 * 2 + 10"); // 式を束縛
        assert!(eq_len(resolved_end_x(&doc), 1810.0), "900*2+10");

        // 値の入力にも式が使える。
        feed(&mut s, &mut doc, "PS");
        click(&mut s, &mut doc, 5.0, 0.0);
        feed(&mut s, &mut doc, "幅");
        feed(&mut s, &mut doc, "100 * 3");
        assert!(eq_len(resolved_end_x(&doc), 610.0), "300*2+10");
    }

    /// 真偽のパラメータと条件式で形が切り替わること。
    #[test]
    fn a_boolean_parameter_switches_the_shape() {
        let (mut s, mut doc) = setup_component();
        for (name, default) in [("幅", "900"), ("両開き", "偽")] {
            feed(&mut s, &mut doc, "PA");
            feed(&mut s, &mut doc, "窓");
            feed(&mut s, &mut doc, name);
            feed(&mut s, &mut doc, default);
        }

        feed(&mut s, &mut doc, "BI");
        feed(&mut s, &mut doc, "窓");
        feed(&mut s, &mut doc, "0");
        feed(&mut s, &mut doc, "終点X");
        feed(&mut s, &mut doc, "if 両開き then 幅 / 2 else 幅");
        assert!(eq_len(resolved_end_x(&doc), 900.0));

        feed(&mut s, &mut doc, "PS");
        click(&mut s, &mut doc, 5.0, 0.0);
        feed(&mut s, &mut doc, "両開き");
        feed(&mut s, &mut doc, "真");
        assert!(eq_len(resolved_end_x(&doc), 450.0), "条件で切り替わる");
    }

    /// 選択肢のパラメータが宣言でき、候補外を断ること。
    #[test]
    fn a_choice_parameter_only_accepts_its_options() {
        let (mut s, mut doc) = setup_component();
        feed(&mut s, &mut doc, "PA");
        feed(&mut s, &mut doc, "窓");
        feed(&mut s, &mut doc, "種別");
        feed(&mut s, &mut doc, "引違い|開き|FIX");

        let def = doc.definitions().by_name("窓").expect("あるはず");
        let decl = doc.definitions().get(def).expect("引ける").param("種別");
        assert!(decl.is_some(), "選択肢として宣言される");

        // 候補外は断られる。
        feed(&mut s, &mut doc, "PS");
        click(&mut s, &mut doc, 5.0, 0.0);
        feed(&mut s, &mut doc, "種別");
        feed(&mut s, &mut doc, "上げ下げ");
        assert!(s.cmdline.history().any(|l| l.kind == LineKind::Error));
        assert!(s.has_active_tool(), "断られてもコマンドは続く");
        s.cancel();

        // 候補内なら通る。
        feed(&mut s, &mut doc, "PS");
        click(&mut s, &mut doc, 5.0, 0.0);
        feed(&mut s, &mut doc, "種別");
        feed(&mut s, &mut doc, "開き");
        let inst = doc.entities().ids().next().expect("あるはず");
        let cad_core::Geometry::Instance(i) = &doc.entities().get(inst).expect("あるはず").geom
        else {
            panic!()
        };
        assert_eq!(
            i.overrides.get("種別"),
            Some(&cad_core::expr::Value::Choice("開き".to_owned()))
        );
    }

    /// **`BIND` が要素の一覧をプロンプトに出すこと。**
    ///
    /// 定義の中身は図面に出ていないので、番号を見られないと指せない。
    #[test]
    fn bind_lists_the_entities_in_its_prompt() {
        let (mut s, mut doc) = setup_component();
        feed(&mut s, &mut doc, "BI");
        feed(&mut s, &mut doc, "窓");

        let prompt = s.prompt();
        assert!(prompt.contains("0:"), "番号が出る: {prompt}");
        assert!(prompt.contains("LINE"), "種別が出る: {prompt}");
    }

    /// `BIND` がスロットの一覧を出すこと。
    #[test]
    fn bind_lists_the_available_slots() {
        let (mut s, mut doc) = setup_component();
        feed(&mut s, &mut doc, "BI");
        feed(&mut s, &mut doc, "窓");
        feed(&mut s, &mut doc, "0");

        let prompt = s.prompt();
        assert!(prompt.contains("終点X"), "線分のスロットが出る: {prompt}");
        assert!(!prompt.contains("半径"), "円のスロットは出ない: {prompt}");
    }

    /// **`PSET` がいまの値と上書き中の印を出すこと。**
    #[test]
    fn pset_shows_the_current_values_and_marks_overrides() {
        let (mut s, mut doc) = setup_component();
        feed(&mut s, &mut doc, "PA");
        feed(&mut s, &mut doc, "窓");
        feed(&mut s, &mut doc, "幅");
        feed(&mut s, &mut doc, "900");

        feed(&mut s, &mut doc, "PS");
        click(&mut s, &mut doc, 5.0, 0.0);
        let prompt = s.prompt();
        assert!(prompt.contains("幅 = 900"), "いまの値が出る: {prompt}");
        // 凡例の行（「＊ は上書き中」）は常にあるので、**値の行**だけを見る。
        assert!(
            !prompt.contains("＊幅"),
            "まだ上書きしていないので印は付かない: {prompt}"
        );
        s.cancel();

        // 上書きしてから見ると印が付く。
        feed(&mut s, &mut doc, "PS");
        click(&mut s, &mut doc, 5.0, 0.0);
        feed(&mut s, &mut doc, "幅");
        feed(&mut s, &mut doc, "1800");
        feed(&mut s, &mut doc, "PS");
        click(&mut s, &mut doc, 5.0, 0.0);
        let prompt = s.prompt();
        assert!(prompt.contains("＊幅"), "上書き中の印が付く: {prompt}");
    }

    /// 範囲外・型違いを断ること（コマンド層の検証がここまで届くこと）。
    #[test]
    fn pset_rejects_values_the_command_layer_refuses() {
        let (mut s, mut doc) = setup_component();
        feed(&mut s, &mut doc, "PA");
        feed(&mut s, &mut doc, "窓");
        feed(&mut s, &mut doc, "両開き");
        feed(&mut s, &mut doc, "偽");

        feed(&mut s, &mut doc, "PS");
        click(&mut s, &mut doc, 5.0, 0.0);
        feed(&mut s, &mut doc, "両開き");
        feed(&mut s, &mut doc, "900"); // 真偽に数値
        assert!(s.cmdline.history().any(|l| l.kind == LineKind::Error));
    }

    /// 存在しないコンポーネント名を断り、あるものを案内すること。
    #[test]
    fn param_reports_the_available_component_names() {
        let (mut s, mut doc) = setup_component();
        feed(&mut s, &mut doc, "PA");
        feed(&mut s, &mut doc, "ない名前");
        assert!(
            s.cmdline.history().any(|l| l.text.contains("窓")),
            "あるコンポーネント名を案内する"
        );
    }

    /// **束縛が参照しているパラメータを消せないこと**が UI まで届くこと。
    #[test]
    fn a_parameter_used_by_a_binding_cannot_be_redeclared_away() {
        let (mut s, mut doc) = setup_component();
        feed(&mut s, &mut doc, "PA");
        feed(&mut s, &mut doc, "窓");
        feed(&mut s, &mut doc, "幅");
        feed(&mut s, &mut doc, "900");
        feed(&mut s, &mut doc, "BI");
        feed(&mut s, &mut doc, "窓");
        feed(&mut s, &mut doc, "0");
        feed(&mut s, &mut doc, "終点X");
        feed(&mut s, &mut doc, "幅");

        // 「幅」を真偽に変えようとすると、束縛の式が数値でなくなる。
        feed(&mut s, &mut doc, "PA");
        feed(&mut s, &mut doc, "窓");
        feed(&mut s, &mut doc, "幅");
        feed(&mut s, &mut doc, "偽");
        assert!(s.cmdline.history().any(|l| l.kind == LineKind::Error));
    }

    /// パラメータの操作が Undo で戻ること。
    #[test]
    fn parameter_operations_undo() {
        let (mut s, mut doc) = setup_component();
        feed(&mut s, &mut doc, "PA");
        feed(&mut s, &mut doc, "窓");
        feed(&mut s, &mut doc, "幅");
        feed(&mut s, &mut doc, "900");
        let def = doc.definitions().by_name("窓").expect("あるはず");
        assert_eq!(doc.definitions().get(def).expect("引ける").params.len(), 1);

        feed(&mut s, &mut doc, "U");
        assert!(
            doc.definitions()
                .get(def)
                .expect("引ける")
                .params
                .is_empty(),
            "宣言が戻る"
        );
    }

    // ---- インプレース編集（段階 3） ----------------------------------------

    /// 線分 2 本のコンポーネント「窓」を作り、インスタンス 1 つが置かれた状態にする。
    fn setup_two_line_component() -> (Session, Document) {
        let (mut s, mut doc) = setup();
        draw_line(&mut s, &mut doc, "0,0", "10,0");
        draw_line(&mut s, &mut doc, "0,5", "10,5");
        for id in doc.entities().ids().collect::<Vec<_>>() {
            s.selection.insert(id);
        }
        feed(&mut s, &mut doc, "B");
        feed(&mut s, &mut doc, "0,0");
        feed(&mut s, &mut doc, "窓");
        s.selection.clear();
        (s, doc)
    }

    fn definition_len(doc: &Document) -> usize {
        let def = doc.definitions().by_name("窓").expect("あるはず");
        doc.definitions().get(def).expect("引ける").entities.len()
    }

    /// **編集に入ると中身が実エンティティになり、出ると戻ること。**
    #[test]
    fn editing_a_component_round_trips() {
        let (mut s, mut doc) = setup_two_line_component();
        assert_eq!(doc.entities().len(), 1, "インスタンス 1 つ");

        feed(&mut s, &mut doc, "BE");
        click(&mut s, &mut doc, 5.0, 0.0);
        assert!(s.editing().is_some(), "編集中になる");
        assert_eq!(doc.entities().len(), 2, "線分 2 本が出る");
        assert_eq!(instance_count(&doc), 0, "インスタンスは外れる");

        feed(&mut s, &mut doc, "BC");
        assert!(s.editing().is_none(), "編集が終わる");
        assert_eq!(doc.entities().len(), 1, "インスタンスへ戻る");
        assert_eq!(instance_count(&doc), 1);
        assert_eq!(definition_len(&doc), 2, "定義の中身は 2 本のまま");
    }

    /// **編集中に描いたものが定義に入ること。**
    #[test]
    fn entities_drawn_while_editing_join_the_definition() {
        let (mut s, mut doc) = setup_two_line_component();
        feed(&mut s, &mut doc, "BE");
        click(&mut s, &mut doc, 5.0, 0.0);

        // 編集中に 1 本描く。
        draw_line(&mut s, &mut doc, "0,10", "10,10");
        assert_eq!(doc.entities().len(), 3);

        feed(&mut s, &mut doc, "BC");
        assert_eq!(definition_len(&doc), 3, "描いた 1 本が定義に入る");
        assert_eq!(doc.entities().len(), 1);
    }

    /// **編集中に消したものが定義から外れること。**
    #[test]
    fn entities_deleted_while_editing_leave_the_definition() {
        let (mut s, mut doc) = setup_two_line_component();
        feed(&mut s, &mut doc, "BE");
        click(&mut s, &mut doc, 5.0, 0.0);

        let member = s.editing().expect("編集中").members(&doc).0[0];
        s.selection.insert(member);
        feed(&mut s, &mut doc, "E");

        feed(&mut s, &mut doc, "BC");
        assert_eq!(definition_len(&doc), 1, "1 本になる");
    }

    /// **全部消して出ようとしたら断ること。**
    ///
    /// 空の定義を作っても使い道が無い。編集は続けられる。
    #[test]
    fn exiting_with_nothing_left_is_refused() {
        let (mut s, mut doc) = setup_two_line_component();
        feed(&mut s, &mut doc, "BE");
        click(&mut s, &mut doc, 5.0, 0.0);

        for id in doc.entities().ids().collect::<Vec<_>>() {
            s.selection.insert(id);
        }
        feed(&mut s, &mut doc, "E");
        assert_eq!(doc.entities().len(), 0);

        feed(&mut s, &mut doc, "BC");
        assert!(s.editing().is_some(), "編集は続く");
        assert!(s.cmdline.history().any(|l| l.kind == LineKind::Error));
    }

    /// **編集の結果が全インスタンスに反映されること。**
    #[test]
    fn editing_updates_every_instance() {
        let (mut s, mut doc) = setup_two_line_component();
        // 2 つ目を配置する。
        feed(&mut s, &mut doc, "I");
        feed(&mut s, &mut doc, "窓");
        feed(&mut s, &mut doc, "100,0");
        enter(&mut s, &mut doc);
        enter(&mut s, &mut doc);
        assert_eq!(instance_count(&doc), 2);

        // 1 つ目を編集して 1 本消す。
        feed(&mut s, &mut doc, "BE");
        click(&mut s, &mut doc, 5.0, 0.0);
        // **編集対象から選ぶこと。** `ids().next()` だと、編集で 1 つ目の
        // スロットが空いたぶん、もう 1 つのインスタンスを拾ってしまう。
        let member = s.editing().expect("編集中").members(&doc).0[0];
        s.selection.insert(member);
        feed(&mut s, &mut doc, "E");
        feed(&mut s, &mut doc, "BC");

        assert_eq!(instance_count(&doc), 2, "インスタンスは 2 つのまま");
        // 両方とも中身が 1 本になっていること。
        for (_, e) in doc.entities().iter() {
            let cad_core::Geometry::Instance(i) = &e.geom else {
                continue;
            };
            assert_eq!(
                cad_core::component::resolve(i, doc.definitions()).len(),
                1,
                "**触っていない側も変わる**"
            );
        }
    }

    /// **束縛が編集を通しても残ること。**
    #[test]
    fn bindings_survive_in_place_editing() {
        let (mut s, mut doc) = setup_two_line_component();
        feed(&mut s, &mut doc, "PA");
        feed(&mut s, &mut doc, "窓");
        feed(&mut s, &mut doc, "幅");
        feed(&mut s, &mut doc, "900");
        feed(&mut s, &mut doc, "BI");
        feed(&mut s, &mut doc, "窓");
        feed(&mut s, &mut doc, "0");
        feed(&mut s, &mut doc, "終点X");
        feed(&mut s, &mut doc, "幅");

        let def = doc.definitions().by_name("窓").expect("あるはず");
        assert_eq!(
            doc.definitions().get(def).expect("引ける").bindings.len(),
            1
        );

        // 編集に入って、何もせず出る。
        feed(&mut s, &mut doc, "BE");
        click(&mut s, &mut doc, 5.0, 0.0);
        feed(&mut s, &mut doc, "BC");

        assert_eq!(
            doc.definitions().get(def).expect("引ける").bindings.len(),
            1,
            "束縛が残る"
        );
    }

    /// インスタンス以外をクリックしたら断ること。
    #[test]
    fn editing_a_plain_entity_is_refused() {
        let (mut s, mut doc) = setup_two_line_component();
        draw_line(&mut s, &mut doc, "0,50", "10,50");

        feed(&mut s, &mut doc, "BE");
        click(&mut s, &mut doc, 5.0, 50.0);
        assert!(s.editing().is_none(), "編集に入らない");
        assert!(s.cmdline.history().any(|l| l.kind == LineKind::Error));
    }

    /// 編集していないのに `ENDCOMP` を打ったら案内すること。
    #[test]
    fn ending_without_editing_is_reported() {
        let (mut s, mut doc) = setup_two_line_component();
        feed(&mut s, &mut doc, "BC");
        assert!(s
            .cmdline
            .history()
            .any(|l| l.text.contains("編集していません")));
    }

    /// **編集中に「編集外」を判定できること**（描画で淡くするのに使う）。
    #[test]
    fn the_edit_session_knows_what_is_inside() {
        let (mut s, mut doc) = setup_two_line_component();
        // 編集の外に 1 本描いておく。
        draw_line(&mut s, &mut doc, "0,50", "10,50");
        let outside = doc.entities().ids().last().expect("あるはず");

        feed(&mut s, &mut doc, "BE");
        click(&mut s, &mut doc, 5.0, 0.0);
        let session = s.editing().expect("編集中");

        assert!(!session.contains(outside), "外側は編集対象ではない");
        let (members, _) = session.members(&doc);
        assert_eq!(members.len(), 2, "編集対象は定義の中身だけ");
        assert!(!members.contains(&outside));
    }

    /// 編集の出入りが Undo で戻ること。
    #[test]
    fn in_place_editing_undoes() {
        let (mut s, mut doc) = setup_two_line_component();
        feed(&mut s, &mut doc, "BE");
        click(&mut s, &mut doc, 5.0, 0.0);
        feed(&mut s, &mut doc, "BC");
        assert_eq!(instance_count(&doc), 1);

        // 出るのを取り消す → 編集中の要素が戻る。
        feed(&mut s, &mut doc, "U");
        assert_eq!(doc.entities().len(), 2, "線分 2 本へ戻る");
        // 入るのを取り消す → インスタンスへ戻る。
        feed(&mut s, &mut doc, "U");
        assert_eq!(instance_count(&doc), 1);
    }

    // ---- BIND のクリック操作（Issue #15） ----------------------------------

    /// **編集中に図形をクリックして束縛できること。**
    ///
    /// 要素番号もスロット名も打たない。番号でパラメータを選ぶところまで
    /// **すべて ASCII** なので日本語入力を通さない。
    #[test]
    fn bind_by_clicking_while_editing() {
        let (mut s, mut doc) = setup_two_line_component();
        feed(&mut s, &mut doc, "PA");
        feed(&mut s, &mut doc, "窓");
        feed(&mut s, &mut doc, "幅");
        feed(&mut s, &mut doc, "900");

        // 編集に入る。
        feed(&mut s, &mut doc, "BE");
        click(&mut s, &mut doc, 5.0, 0.0);
        assert!(s.editing().is_some());

        // 線分の終点あたりをクリック → X を選ぶ → パラメータを番号で選ぶ。
        feed(&mut s, &mut doc, "BI");
        click(&mut s, &mut doc, 10.0, 0.0);
        feed(&mut s, &mut doc, "X");
        feed(&mut s, &mut doc, "1");

        let def = doc.definitions().by_name("窓").expect("あるはず");
        let bindings = &doc.definitions().get(def).expect("引ける").bindings;
        assert_eq!(bindings.len(), 1, "束縛ができる");
        assert_eq!(bindings[0].slot, cad_core::component::Slot::LineBx, "終点X");
    }

    /// **クリック位置に近いつまみが選ばれること。**
    ///
    /// 始点側をクリックすれば始点、終点側なら終点。
    #[test]
    fn the_nearest_handle_is_chosen() {
        let (mut s, mut doc) = setup_two_line_component();
        feed(&mut s, &mut doc, "PA");
        feed(&mut s, &mut doc, "窓");
        feed(&mut s, &mut doc, "幅");
        feed(&mut s, &mut doc, "900");
        feed(&mut s, &mut doc, "BE");
        click(&mut s, &mut doc, 5.0, 0.0);

        // 始点 (0,0) 側をクリックする。
        feed(&mut s, &mut doc, "BI");
        click(&mut s, &mut doc, 0.5, 0.0);
        feed(&mut s, &mut doc, "Y");
        feed(&mut s, &mut doc, "1");

        let def = doc.definitions().by_name("窓").expect("あるはず");
        let bindings = &doc.definitions().get(def).expect("引ける").bindings;
        assert_eq!(
            bindings[0].slot,
            cad_core::component::Slot::LineAy,
            "始点Y が選ばれる"
        );
    }

    /// 式も打てること（番号でなくてもよい）。
    #[test]
    fn bind_by_click_still_accepts_an_expression() {
        let (mut s, mut doc) = setup_two_line_component();
        feed(&mut s, &mut doc, "PA");
        feed(&mut s, &mut doc, "窓");
        feed(&mut s, &mut doc, "幅");
        feed(&mut s, &mut doc, "900");
        feed(&mut s, &mut doc, "BE");
        click(&mut s, &mut doc, 5.0, 0.0);

        feed(&mut s, &mut doc, "BI");
        click(&mut s, &mut doc, 10.0, 0.0);
        feed(&mut s, &mut doc, "X");
        feed(&mut s, &mut doc, "幅 * 2");

        let def = doc.definitions().by_name("窓").expect("あるはず");
        let bindings = &doc.definitions().get(def).expect("引ける").bindings;
        assert_eq!(bindings.len(), 1);
        assert_eq!(
            bindings[0].expr,
            cad_core::expr::parse("幅 * 2").expect("解析")
        );
    }

    /// 番号が範囲外なら断ること。
    #[test]
    fn an_out_of_range_parameter_number_is_reported() {
        let (mut s, mut doc) = setup_two_line_component();
        feed(&mut s, &mut doc, "PA");
        feed(&mut s, &mut doc, "窓");
        feed(&mut s, &mut doc, "幅");
        feed(&mut s, &mut doc, "900");
        feed(&mut s, &mut doc, "BE");
        click(&mut s, &mut doc, 5.0, 0.0);

        feed(&mut s, &mut doc, "BI");
        click(&mut s, &mut doc, 10.0, 0.0);
        feed(&mut s, &mut doc, "X");
        feed(&mut s, &mut doc, "9");
        assert!(s.cmdline.history().any(|l| l.text.contains("番号は 1〜1")));
    }

    /// X / Y 以外を入力したら断ること。
    #[test]
    fn the_axis_must_be_x_or_y() {
        let (mut s, mut doc) = setup_two_line_component();
        feed(&mut s, &mut doc, "PA");
        feed(&mut s, &mut doc, "窓");
        feed(&mut s, &mut doc, "幅");
        feed(&mut s, &mut doc, "900");
        feed(&mut s, &mut doc, "BE");
        click(&mut s, &mut doc, 5.0, 0.0);

        feed(&mut s, &mut doc, "BI");
        click(&mut s, &mut doc, 10.0, 0.0);
        feed(&mut s, &mut doc, "Z");
        assert!(s.cmdline.history().any(|l| l.kind == LineKind::Error));
        assert!(s.has_active_tool(), "断られてもコマンドは続く");
    }

    /// **編集中に描いた図形はまだ束縛できないこと。**
    ///
    /// 定義にはまだ入っていないので、指す添字が決まらない。
    #[test]
    fn a_freshly_drawn_entity_cannot_be_bound_yet() {
        let (mut s, mut doc) = setup_two_line_component();
        feed(&mut s, &mut doc, "PA");
        feed(&mut s, &mut doc, "窓");
        feed(&mut s, &mut doc, "幅");
        feed(&mut s, &mut doc, "900");
        feed(&mut s, &mut doc, "BE");
        click(&mut s, &mut doc, 5.0, 0.0);

        // 編集中に 1 本描く。
        draw_line(&mut s, &mut doc, "0,20", "10,20");

        feed(&mut s, &mut doc, "BI");
        click(&mut s, &mut doc, 5.0, 20.0);
        assert!(s
            .cmdline
            .history()
            .any(|l| l.text.contains("まだ束縛できません")));
    }

    /// 編集していないときは、これまでどおり名前から辿れること。
    #[test]
    fn bind_still_works_from_the_command_line_when_not_editing() {
        let (mut s, mut doc) = setup_two_line_component();
        feed(&mut s, &mut doc, "PA");
        feed(&mut s, &mut doc, "窓");
        feed(&mut s, &mut doc, "幅");
        feed(&mut s, &mut doc, "900");

        feed(&mut s, &mut doc, "BI");
        feed(&mut s, &mut doc, "窓");
        feed(&mut s, &mut doc, "0");
        feed(&mut s, &mut doc, "終点X");
        feed(&mut s, &mut doc, "1"); // 番号でも選べる

        let def = doc.definitions().by_name("窓").expect("あるはず");
        assert_eq!(
            doc.definitions().get(def).expect("引ける").bindings.len(),
            1
        );
    }

    /// 編集中でないのに図形をクリックしたら案内すること。
    #[test]
    fn clicking_outside_an_edit_is_guided() {
        let (mut s, mut doc) = setup_two_line_component();
        feed(&mut s, &mut doc, "BI");
        click(&mut s, &mut doc, 5.0, 0.0);
        assert!(s.cmdline.history().any(|l| l.text.contains("EDITCOMP")));
    }
}
