//! 編集コマンドとビュー操作コマンド。

use cad_core::command::{CopyEntities, DeleteEntities, MoveEntities, StretchEntities};
use cad_core::geom::{Aabb, Point2};
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

/// 選択中の図形を、交差範囲に入っている点だけずらしたラバーバンドを返す。
fn stretched_selection(
    base: Option<Point2>,
    cursor: Point2,
    regions: &[Aabb],
    ctx: &ToolCtx<'_>,
) -> Vec<Geometry> {
    let Some(base) = base else {
        return Vec::new();
    };
    let delta = cursor - base;
    ctx.selection
        .iter()
        .filter_map(|id| ctx.doc.entities().get(id))
        .map(|e| e.geom.stretched(regions, delta))
        .collect()
}

/// 選択したオブジェクトのうち、交差範囲に入っている定義点だけを移動する。
///
/// AutoCAD の STRETCH。範囲の外にある点は動かないので、
/// 図形の形そのものが変わる（線分が伸びる、矩形が変形する）。
///
/// 範囲は「選択のときに使われた交差窓」。クリックや窓選択だけで選んだ場合は
/// 範囲が無いので、MOVE と同じく丸ごと移動になる。これは AutoCAD と同じ挙動。
#[derive(Debug, Default)]
pub struct StretchTool {
    base: Option<Point2>,
}

impl Tool for StretchTool {
    fn name(&self) -> &'static str {
        "STRETCH"
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
                StepOutcome::Apply(Box::new(StretchEntities::new(
                    "STRETCH",
                    targets,
                    ctx.crossing_rects.to_vec(),
                    p - base,
                )))
            }
            StepInput::Enter => StepOutcome::Finish,
            StepInput::Word(w) => StepOutcome::Reject(format!("不明なオプションです: {w}")),
            StepInput::Number(_) => StepOutcome::Reject("点を指定してください".to_owned()),
        }
    }

    fn preview(&self, cursor: Point2, ctx: &ToolCtx<'_>) -> Vec<Geometry> {
        stretched_selection(self.base, cursor, ctx.crossing_rects, ctx)
    }
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
