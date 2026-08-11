//! 対話的なコマンド（ツール）の枠組み。
//!
//! AutoCAD のコマンドは「プロンプトを出す → 点やオプションを受け取る →
//! 図形を作る」という多段の対話になる。その状態機械を [`Tool`] として表す。
//!
//! # 責務の分け方
//!
//! - [`Tool`] は **入力を溜めて、確定したら [`Command`] を組み立てて返す**だけ。
//!   図面を直接いじらない。適用は `Document::apply` の仕事。
//! - 確定前のラバーバンドは [`Tool::preview`] が返す。これは
//!   **`Document` に入らない派生データ**で、Undo の対象にもならない。
//!   「途中の図形をとりあえず図面に入れて後で消す」ことは絶対にしない
//!   （Undo 履歴が汚れ、`EditCtx` を唯一の変更経路にした意味が失われる）。

pub mod draw;
pub mod edit;

use cad_core::geom::Point2;
use cad_core::{Command, Document, Geometry, LayerId};

use crate::file_ops::FileAction;
use crate::input::ViewAction;
use crate::selection::Selection;

/// ツールに渡す 1 手ぶんの入力。
#[derive(Clone, Debug, PartialEq)]
pub enum StepInput {
    /// 点が指定された（クリック、または座標の直接入力）。
    Point(Point2),
    /// オプションや文字列が入力された（`C`、`D`、`2P` など）。大文字化済み。
    Word(String),
    /// 数値が入力された（半径など）。
    Number(f64),
    /// 空のまま確定された（コマンドの終了を意味することが多い）。
    Enter,
    /// 選択が確定した。
    ///
    /// [`Tool::wants_selection`] が真のツールにだけ送られる。
    /// これを受け取った時点で `ctx.selection` に対象が入っている。
    SelectionReady,
}

/// ツールが 1 手を処理した結果。
#[derive(Debug)]
pub enum StepOutcome {
    /// まだ入力を待つ。
    Continue,
    /// コマンドを適用して終了する。
    Apply(Box<dyn Command>),
    /// コマンドを適用して、同じツールのまま入力を続ける。
    ///
    /// LINE の各セグメントや COPY の複数回コピーで使う。
    ApplyAndContinue(Box<dyn Command>),
    /// ビュー操作を行って終了する（ZOOM）。
    View(ViewAction),
    /// 何も適用せずに終了する。
    Finish,
    /// 入力を受け付けず、メッセージを出して同じ状態のまま待つ。
    Reject(String),
}

/// ツールが図面を読むための文脈。
#[derive(Clone, Copy)]
pub struct ToolCtx<'a> {
    /// 図面（読み取り専用）。
    pub doc: &'a Document,
    /// 現在の選択。
    pub selection: &'a Selection,
    /// 新規作成する要素が入るレイヤ。
    pub layer: LayerId,
}

/// 対話的コマンドの状態機械。
pub trait Tool: std::fmt::Debug {
    /// コマンド名（`"LINE"` など）。履歴表示と再実行に使う。
    fn name(&self) -> &'static str;

    /// いま表示すべきプロンプト（例: `線分の次の点を指定:`）。
    fn prompt(&self) -> String;

    /// 相対座標入力（`@100,50`）の基準となる直前の点。
    fn last_point(&self) -> Option<Point2> {
        None
    }

    /// 開始時に選択を必要とするか（ERASE / MOVE / COPY）。
    fn wants_selection(&self) -> bool {
        false
    }

    /// 1 手ぶんの入力を処理する。
    fn step(&mut self, input: StepInput, ctx: &ToolCtx<'_>) -> StepOutcome;

    /// カーソル位置に応じたラバーバンド。`Document` には入らない。
    fn preview(&self, _cursor: Point2, _ctx: &ToolCtx<'_>) -> Vec<Geometry> {
        Vec::new()
    }
}

/// コマンド名またはエイリアスからツールを作る。
///
/// 大文字小文字は区別しない。
#[must_use]
pub fn create(input: &str) -> Option<Box<dyn Tool>> {
    let upper = input.trim().to_uppercase();
    match upper.as_str() {
        "LINE" | "L" => Some(Box::new(draw::LineTool::default())),
        "CIRCLE" | "C" => Some(Box::new(draw::CircleTool::default())),
        "ARC" | "A" => Some(Box::new(draw::ArcTool::default())),
        "RECTANGLE" | "RECTANG" | "REC" => Some(Box::new(draw::RectangleTool::default())),
        "POLYLINE" | "PLINE" | "PL" => Some(Box::new(draw::PolylineTool::default())),
        "ERASE" | "E" | "DEL" => Some(Box::new(edit::EraseTool)),
        "MOVE" | "M" => Some(Box::new(edit::MoveTool::default())),
        "COPY" | "CO" | "CP" => Some(Box::new(edit::CopyTool::default())),
        "ZOOM" | "Z" => Some(Box::new(edit::ZoomTool)),
        _ => None,
    }
}

/// コマンド名でもエイリアスでもない、即座に実行できるコマンドか調べる。
///
/// UNDO / REDO は対話が無いのでツールにしない。
#[must_use]
pub fn immediate(input: &str) -> Option<Immediate> {
    match input.trim().to_uppercase().as_str() {
        "UNDO" | "U" => Some(Immediate::Undo),
        "REDO" => Some(Immediate::Redo),
        "LAYER" | "LA" => Some(Immediate::LayerPanel),
        "NEW" => Some(Immediate::File(FileAction::New)),
        "OPEN" => Some(Immediate::File(FileAction::Open)),
        "SAVE" | "QSAVE" => Some(Immediate::File(FileAction::Save)),
        "SAVEAS" => Some(Immediate::File(FileAction::SaveAs)),
        "QUIT" | "EXIT" => Some(Immediate::File(FileAction::Quit)),
        _ => None,
    }
}

/// 対話を伴わないコマンド。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Immediate {
    /// 直前の操作を取り消す。
    Undo,
    /// 取り消した操作をやり直す。
    Redo,
    /// レイヤパネルの開閉。
    LayerPanel,
    /// ファイル操作。
    File(FileAction),
}

impl Immediate {
    /// 履歴表示に使う名前。
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Undo => "UNDO",
            Self::Redo => "REDO",
            Self::LayerPanel => "LAYER",
            Self::File(a) => a.command_name(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_resolve_to_expected_tools() {
        for (alias, expected) in [
            ("L", "LINE"),
            ("line", "LINE"),
            ("C", "CIRCLE"),
            ("A", "ARC"),
            ("REC", "RECTANGLE"),
            ("PL", "POLYLINE"),
            ("E", "ERASE"),
            ("M", "MOVE"),
            ("CO", "COPY"),
            ("Z", "ZOOM"),
        ] {
            let tool = create(alias).unwrap_or_else(|| panic!("{alias} が解決できない"));
            assert_eq!(tool.name(), expected, "エイリアス {alias}");
        }
    }

    /// 指示書のコマンド表にある正式名がすべて起動できること。
    #[test]
    fn canonical_names_resolve() {
        for name in [
            "LINE",
            "CIRCLE",
            "ARC",
            "RECTANGLE",
            "POLYLINE",
            "ERASE",
            "MOVE",
            "COPY",
        ] {
            assert!(create(name).is_some(), "{name} が起動できない");
        }
    }

    #[test]
    fn case_is_ignored() {
        assert!(create("  line  ").is_some());
        assert!(create("LiNe").is_some());
    }

    #[test]
    fn unknown_command_is_rejected() {
        assert!(create("NOPE").is_none());
        assert!(create("").is_none());
    }

    #[test]
    fn undo_and_redo_are_immediate() {
        assert_eq!(immediate("U"), Some(Immediate::Undo));
        assert_eq!(immediate("undo"), Some(Immediate::Undo));
        assert_eq!(immediate("REDO"), Some(Immediate::Redo));
        assert_eq!(immediate("LA"), Some(Immediate::LayerPanel));
        assert_eq!(immediate("save"), Some(Immediate::File(FileAction::Save)));
        assert_eq!(immediate("OPEN"), Some(Immediate::File(FileAction::Open)));
        assert_eq!(immediate("LINE"), None);
    }

    /// UNDO は即時コマンド、ツールとしては存在しないこと。
    #[test]
    fn undo_is_not_a_tool() {
        assert!(create("UNDO").is_none());
        assert!(create("REDO").is_none());
    }

    #[test]
    fn editing_tools_want_selection() {
        for name in ["ERASE", "MOVE", "COPY"] {
            assert!(
                create(name).unwrap().wants_selection(),
                "{name} は選択を必要とするはず"
            );
        }
        for name in ["LINE", "CIRCLE", "ARC"] {
            assert!(
                !create(name).unwrap().wants_selection(),
                "{name} は選択を必要としないはず"
            );
        }
    }
}
