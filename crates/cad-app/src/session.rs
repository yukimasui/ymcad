//! コマンドライン・ツール・選択をつなぐ層。
//!
//! ここが「ユーザーの操作 → [`Command`](cad_core::Command) の適用」の唯一の流れになる。
//! 図面を変更するのは `Document::apply` / `undo` / `redo` だけで、
//! ラバーバンドなどの途中状態は一切 `Document` に入れない。

use cad_core::geom::{Aabb, Point2};
use cad_core::{Document, Geometry};

use crate::cmdline::{coord, CommandLine, LineKind, Submission};
use crate::input::ViewAction;
use crate::selection::{self, Selection, WindowMode};
use crate::tools::{self, Immediate, StepInput, StepOutcome, Tool, ToolCtx};

/// UI に対する要求。図面の変更ではないのでコマンドにはしない。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiAction {
    /// レイヤパネルの開閉。
    ToggleLayerPanel,
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
    /// 選択に使われた交差窓の矩形（モデル座標）。
    ///
    /// STRETCH が「どの点を動かすか」を決めるのに使う。窓選択やクリックでは増えない。
    /// AutoCAD は交差窓を複数回重ねられるので蓄積する。
    crossing_rects: Vec<Aabb>,
    /// このフレームで発生したビュー操作。
    view_actions: Vec<ViewAction>,
    /// このフレームで発生した UI 要求。
    ui_actions: Vec<UiAction>,
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
            crossing_rects: Vec::new(),
            view_actions: Vec::new(),
            ui_actions: Vec::new(),
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

        if shift {
            self.selection.remove(id);
        } else {
            self.selection.insert(id);
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
            if shift {
                self.selection.remove(id);
            } else {
                self.selection.insert(id);
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
}
