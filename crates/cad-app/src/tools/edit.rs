//! 編集コマンドとビュー操作コマンド。

use cad_core::command::{CopyEntities, DeleteEntities, MoveEntities};
use cad_core::geom::Point2;
use cad_core::Geometry;

use super::{StepInput, StepOutcome, Tool, ToolCtx};
use crate::input::ViewAction;

/// 選択したオブジェクトを削除する。
#[derive(Debug, Default)]
pub struct EraseTool;

impl Tool for EraseTool {
    fn name(&self) -> &'static str {
        "ERASE"
    }

    fn prompt(&self) -> String {
        "オブジェクトを選択 (Enter で削除):".to_owned()
    }

    fn wants_selection(&self) -> bool {
        true
    }

    fn step(&mut self, input: StepInput, ctx: &ToolCtx<'_>) -> StepOutcome {
        match input {
            StepInput::SelectionReady | StepInput::Enter => {
                let targets = ctx.selection.to_vec();
                if targets.is_empty() {
                    return StepOutcome::Finish;
                }
                StepOutcome::Apply(Box::new(DeleteEntities::new("ERASE", targets)))
            }
            _ => StepOutcome::Reject("Enter で削除を実行してください".to_owned()),
        }
    }
}

/// 選択したオブジェクトを平行移動する。
#[derive(Debug, Default)]
pub struct MoveTool {
    base: Option<Point2>,
}

impl Tool for MoveTool {
    fn name(&self) -> &'static str {
        "MOVE"
    }

    fn prompt(&self) -> String {
        if self.base.is_none() {
            "基点を指定:".to_owned()
        } else {
            "目的点を指定:".to_owned()
        }
    }

    fn wants_selection(&self) -> bool {
        true
    }

    fn last_point(&self) -> Option<Point2> {
        self.base
    }

    fn step(&mut self, input: StepInput, ctx: &ToolCtx<'_>) -> StepOutcome {
        match input {
            StepInput::SelectionReady => StepOutcome::Continue,
            StepInput::Point(p) => {
                let Some(base) = self.base else {
                    self.base = Some(p);
                    return StepOutcome::Continue;
                };
                let targets = ctx.selection.to_vec();
                if targets.is_empty() {
                    return StepOutcome::Finish;
                }
                StepOutcome::Apply(Box::new(MoveEntities::new("MOVE", targets, p - base)))
            }
            StepInput::Enter => StepOutcome::Finish,
            StepInput::Word(w) => StepOutcome::Reject(format!("不明なオプションです: {w}")),
            StepInput::Number(_) => StepOutcome::Reject("点を指定してください".to_owned()),
        }
    }

    fn preview(&self, cursor: Point2, ctx: &ToolCtx<'_>) -> Vec<Geometry> {
        translated_selection(self.base, cursor, ctx)
    }
}

/// 選択したオブジェクトを複写する。目的点を指定するたびに複写し、続けて入力できる。
#[derive(Debug, Default)]
pub struct CopyTool {
    base: Option<Point2>,
    copies: usize,
}

impl Tool for CopyTool {
    fn name(&self) -> &'static str {
        "COPY"
    }

    fn prompt(&self) -> String {
        if self.base.is_none() {
            "基点を指定:".to_owned()
        } else {
            format!("目的点を指定 <Enter で終了> (複写 {} 個):", self.copies)
        }
    }

    fn wants_selection(&self) -> bool {
        true
    }

    fn last_point(&self) -> Option<Point2> {
        self.base
    }

    fn step(&mut self, input: StepInput, ctx: &ToolCtx<'_>) -> StepOutcome {
        match input {
            StepInput::SelectionReady => StepOutcome::Continue,
            StepInput::Point(p) => {
                let Some(base) = self.base else {
                    self.base = Some(p);
                    return StepOutcome::Continue;
                };
                let sources = ctx.selection.to_vec();
                if sources.is_empty() {
                    return StepOutcome::Finish;
                }
                self.copies += 1;
                // 基点は保持したまま、続けて複数回コピーできるようにする。
                StepOutcome::ApplyAndContinue(Box::new(CopyEntities::new(
                    "COPY",
                    sources,
                    p - base,
                )))
            }
            StepInput::Enter => StepOutcome::Finish,
            StepInput::Word(w) => StepOutcome::Reject(format!("不明なオプションです: {w}")),
            StepInput::Number(_) => StepOutcome::Reject("点を指定してください".to_owned()),
        }
    }

    fn preview(&self, cursor: Point2, ctx: &ToolCtx<'_>) -> Vec<Geometry> {
        translated_selection(self.base, cursor, ctx)
    }
}

/// 選択中の図形を `base` → `cursor` の分だけずらしたラバーバンドを返す。
fn translated_selection(base: Option<Point2>, cursor: Point2, ctx: &ToolCtx<'_>) -> Vec<Geometry> {
    let Some(base) = base else {
        return Vec::new();
    };
    let delta = cursor - base;
    ctx.selection
        .iter()
        .filter_map(|id| ctx.doc.entities().get(id))
        .map(|e| e.geom.translated(delta))
        .collect()
}

/// ビューのズーム。
///
/// Phase 2 では `Z` → `E` の 2 段キー入力を暫定の状態機械で扱っていたが、
/// コマンドラインができたのでこちらへ統合した。
#[derive(Debug, Default)]
pub struct ZoomTool;

impl Tool for ZoomTool {
    fn name(&self) -> &'static str {
        "ZOOM"
    }

    fn prompt(&self) -> String {
        "ZOOM オプションを指定 [全体(A)/範囲(E)]:".to_owned()
    }

    fn step(&mut self, input: StepInput, _ctx: &ToolCtx<'_>) -> StepOutcome {
        match input {
            StepInput::Word(w) => match w.as_str() {
                "A" | "ALL" => StepOutcome::View(ViewAction::ZoomAll),
                "E" | "EXTENTS" => StepOutcome::View(ViewAction::ZoomExtents),
                other => StepOutcome::Reject(format!("不明なオプションです: {other}")),
            },
            StepInput::Enter => StepOutcome::Finish,
            _ => StepOutcome::Reject("A（全体）または E（範囲）を指定してください".to_owned()),
        }
    }
}
