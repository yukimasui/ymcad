//! 作図コマンド。
//!
//! いずれも確定するまで [`Document`](cad_core::Document) を変更しない。
//! 確定前の図形は [`Tool::preview`] が返すラバーバンドとして描くだけ。

use cad_core::command::AddEntities;
use cad_core::geom::{Arc, Circle, Line, Point2, Polyline};
use cad_core::{Entity, Geometry};

use super::{StepInput, StepOutcome, Tool, ToolCtx};

/// 図形 1 つを現在レイヤへ追加するコマンドを組み立てる。
fn add_one(name: &'static str, geom: Geometry, ctx: &ToolCtx<'_>) -> StepOutcome {
    StepOutcome::ApplyAndContinue(Box::new(AddEntities::one(
        name,
        Entity::new(geom, ctx.layer),
    )))
}

/// 図形 1 つを追加してコマンドを終える。
fn add_one_and_finish(name: &'static str, geom: Geometry, ctx: &ToolCtx<'_>) -> StepOutcome {
    StepOutcome::Apply(Box::new(AddEntities::one(
        name,
        Entity::new(geom, ctx.layer),
    )))
}

// ---------------------------------------------------------------------------
// LINE
// ---------------------------------------------------------------------------

/// 連続線分。`Enter` で終了、`C` で始点へ閉じる。
#[derive(Debug, Default)]
pub struct LineTool {
    /// 最初の点（`C` で閉じる先）。
    first: Option<Point2>,
    /// 直前の点（次の線分の始点）。
    last: Option<Point2>,
    /// 引いた線分の本数。閉じられるかの判断に使う。
    segments: usize,
}

impl Tool for LineTool {
    fn name(&self) -> &'static str {
        "LINE"
    }

    fn prompt(&self) -> String {
        if self.last.is_none() {
            "線分の始点を指定:".to_owned()
        } else if self.segments >= 2 {
            "線分の次の点を指定 [閉じる(C)] <Enter で終了>:".to_owned()
        } else {
            "線分の次の点を指定 <Enter で終了>:".to_owned()
        }
    }

    fn last_point(&self) -> Option<Point2> {
        self.last
    }

    fn step(&mut self, input: StepInput, ctx: &ToolCtx<'_>) -> StepOutcome {
        match input {
            StepInput::Point(p) => {
                let Some(from) = self.last else {
                    self.first = Some(p);
                    self.last = Some(p);
                    return StepOutcome::Continue;
                };
                let line = Line::new(from, p);
                if line.is_degenerate() {
                    return StepOutcome::Reject("同じ点が指定されました".to_owned());
                }
                self.last = Some(p);
                self.segments += 1;
                add_one("LINE", Geometry::Line(line), ctx)
            }
            StepInput::Word(w) if w == "C" => self.close(ctx),
            StepInput::Word(w) => StepOutcome::Reject(format!("不明なオプションです: {w}")),
            StepInput::Number(_) => {
                StepOutcome::Reject("座標を指定してください (例: 100,50 / @100<45)".to_owned())
            }
            StepInput::Enter | StepInput::SelectionReady => StepOutcome::Finish,
        }
    }

    fn preview(&self, cursor: Point2, _ctx: &ToolCtx<'_>) -> Vec<Geometry> {
        self.last
            .map(|from| vec![Geometry::Line(Line::new(from, cursor))])
            .unwrap_or_default()
    }
}

impl LineTool {
    fn close(&mut self, ctx: &ToolCtx<'_>) -> StepOutcome {
        // 2 本以上引いていないと「閉じる」意味がない。
        let (Some(first), Some(last)) = (self.first, self.last) else {
            return StepOutcome::Reject("まだ線分がありません".to_owned());
        };
        if self.segments < 2 {
            return StepOutcome::Reject("閉じるには線分が 2 本以上必要です".to_owned());
        }
        let line = Line::new(last, first);
        if line.is_degenerate() {
            return StepOutcome::Finish;
        }
        add_one_and_finish("LINE", Geometry::Line(line), ctx)
    }
}

// ---------------------------------------------------------------------------
// CIRCLE
// ---------------------------------------------------------------------------

/// 円。中心 + 半径が既定。`D` で直径指定、`2P` で 2 点指定。
#[derive(Debug, Default)]
pub struct CircleTool {
    state: CircleState,
}

#[derive(Debug, Default, PartialEq)]
enum CircleState {
    /// 中心（または `2P`）待ち。
    #[default]
    Center,
    /// 半径待ち。
    Radius { center: Point2 },
    /// 直径待ち。
    Diameter { center: Point2 },
    /// 2 点指定の 1 点目待ち。
    TwoPointsFirst,
    /// 2 点指定の 2 点目待ち。
    TwoPointsSecond { first: Point2 },
}

impl Tool for CircleTool {
    fn name(&self) -> &'static str {
        "CIRCLE"
    }

    fn prompt(&self) -> String {
        match self.state {
            CircleState::Center => "円の中心点を指定 [2点(2P)]:".to_owned(),
            CircleState::Radius { .. } => "円の半径を指定 [直径(D)]:".to_owned(),
            CircleState::Diameter { .. } => "円の直径を指定:".to_owned(),
            CircleState::TwoPointsFirst => "円の直径の 1 点目を指定:".to_owned(),
            CircleState::TwoPointsSecond { .. } => "円の直径の 2 点目を指定:".to_owned(),
        }
    }

    fn last_point(&self) -> Option<Point2> {
        match self.state {
            CircleState::Radius { center } | CircleState::Diameter { center } => Some(center),
            CircleState::TwoPointsSecond { first } => Some(first),
            _ => None,
        }
    }

    fn step(&mut self, input: StepInput, ctx: &ToolCtx<'_>) -> StepOutcome {
        match (&self.state, input) {
            (CircleState::Center, StepInput::Point(p)) => {
                self.state = CircleState::Radius { center: p };
                StepOutcome::Continue
            }
            (CircleState::Center, StepInput::Word(w)) if w == "2P" => {
                self.state = CircleState::TwoPointsFirst;
                StepOutcome::Continue
            }

            (CircleState::Radius { center }, StepInput::Word(w)) if w == "D" => {
                self.state = CircleState::Diameter { center: *center };
                StepOutcome::Continue
            }
            (CircleState::Radius { center }, StepInput::Number(r)) => Self::finish(*center, r, ctx),
            (CircleState::Radius { center }, StepInput::Point(p)) => {
                Self::finish(*center, center.dist(p), ctx)
            }

            (CircleState::Diameter { center }, StepInput::Number(d)) => {
                Self::finish(*center, d / 2.0, ctx)
            }
            (CircleState::Diameter { center }, StepInput::Point(p)) => {
                Self::finish(*center, center.dist(p) / 2.0, ctx)
            }

            (CircleState::TwoPointsFirst, StepInput::Point(p)) => {
                self.state = CircleState::TwoPointsSecond { first: p };
                StepOutcome::Continue
            }
            (CircleState::TwoPointsSecond { first }, StepInput::Point(p)) => {
                let center = first.lerp(p, 0.5);
                Self::finish(center, first.dist(p) / 2.0, ctx)
            }

            (_, StepInput::Enter | StepInput::SelectionReady) => StepOutcome::Finish,
            (_, StepInput::Word(w)) => StepOutcome::Reject(format!("不明なオプションです: {w}")),
            (_, StepInput::Number(_)) => StepOutcome::Reject("点を指定してください".to_owned()),
        }
    }

    fn preview(&self, cursor: Point2, _ctx: &ToolCtx<'_>) -> Vec<Geometry> {
        match self.state {
            CircleState::Radius { center } => {
                vec![Geometry::Circle(Circle::new(center, center.dist(cursor)))]
            }
            CircleState::Diameter { center } => vec![Geometry::Circle(Circle::new(
                center,
                center.dist(cursor) / 2.0,
            ))],
            CircleState::TwoPointsSecond { first } => {
                let center = first.lerp(cursor, 0.5);
                vec![Geometry::Circle(Circle::new(
                    center,
                    first.dist(cursor) / 2.0,
                ))]
            }
            _ => Vec::new(),
        }
    }
}

impl CircleTool {
    fn finish(center: Point2, radius: f64, ctx: &ToolCtx<'_>) -> StepOutcome {
        let circle = Circle::new(center, radius);
        if circle.is_degenerate() {
            return StepOutcome::Reject("半径が 0 です".to_owned());
        }
        add_one_and_finish("CIRCLE", Geometry::Circle(circle), ctx)
    }
}

// ---------------------------------------------------------------------------
// ARC
// ---------------------------------------------------------------------------

/// 円弧。3 点（始点・通過点・終点）で指定する。
#[derive(Debug, Default)]
pub struct ArcTool {
    points: Vec<Point2>,
}

impl Tool for ArcTool {
    fn name(&self) -> &'static str {
        "ARC"
    }

    fn prompt(&self) -> String {
        match self.points.len() {
            0 => "円弧の始点を指定:".to_owned(),
            1 => "円弧の通過点を指定:".to_owned(),
            _ => "円弧の終点を指定:".to_owned(),
        }
    }

    fn last_point(&self) -> Option<Point2> {
        self.points.last().copied()
    }

    fn step(&mut self, input: StepInput, ctx: &ToolCtx<'_>) -> StepOutcome {
        match input {
            StepInput::Point(p) => {
                self.points.push(p);
                if self.points.len() < 3 {
                    return StepOutcome::Continue;
                }
                let (a, b, c) = (self.points[0], self.points[1], self.points[2]);
                match Arc::from_3_points(a, b, c) {
                    Some(arc) => add_one_and_finish("ARC", Geometry::Arc(arc), ctx),
                    None => {
                        // 3 点目をやり直させる。
                        self.points.pop();
                        StepOutcome::Reject("3 点が同一直線上にあるため円弧を作れません".to_owned())
                    }
                }
            }
            StepInput::Word(w) => StepOutcome::Reject(format!("不明なオプションです: {w}")),
            StepInput::Number(_) => StepOutcome::Reject("点を指定してください".to_owned()),
            StepInput::Enter | StepInput::SelectionReady => StepOutcome::Finish,
        }
    }

    fn preview(&self, cursor: Point2, _ctx: &ToolCtx<'_>) -> Vec<Geometry> {
        match self.points.len() {
            1 => vec![Geometry::Line(Line::new(self.points[0], cursor))],
            2 => Arc::from_3_points(self.points[0], self.points[1], cursor)
                .map(|a| vec![Geometry::Arc(a)])
                // 同一直線上のときは弧にならないので、代わりに補助線を出す。
                .unwrap_or_else(|| vec![Geometry::Line(Line::new(self.points[1], cursor))]),
            _ => Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// RECTANGLE
// ---------------------------------------------------------------------------

/// 矩形。対角 2 点で指定し、閉じたポリラインとして作る。
#[derive(Debug, Default)]
pub struct RectangleTool {
    first: Option<Point2>,
}

impl Tool for RectangleTool {
    fn name(&self) -> &'static str {
        "RECTANGLE"
    }

    fn prompt(&self) -> String {
        if self.first.is_none() {
            "矩形の 1 つ目の角を指定:".to_owned()
        } else {
            "矩形のもう一方の角を指定:".to_owned()
        }
    }

    fn last_point(&self) -> Option<Point2> {
        self.first
    }

    fn step(&mut self, input: StepInput, ctx: &ToolCtx<'_>) -> StepOutcome {
        match input {
            StepInput::Point(p) => {
                let Some(a) = self.first else {
                    self.first = Some(p);
                    return StepOutcome::Continue;
                };
                let poly = Polyline::rectangle(a, p);
                if poly.is_degenerate() {
                    return StepOutcome::Reject("面積が 0 の矩形です".to_owned());
                }
                add_one_and_finish("RECTANGLE", Geometry::Polyline(poly), ctx)
            }
            StepInput::Word(w) => StepOutcome::Reject(format!("不明なオプションです: {w}")),
            StepInput::Number(_) => StepOutcome::Reject("点を指定してください".to_owned()),
            StepInput::Enter | StepInput::SelectionReady => StepOutcome::Finish,
        }
    }

    fn preview(&self, cursor: Point2, _ctx: &ToolCtx<'_>) -> Vec<Geometry> {
        self.first
            .map(|a| vec![Geometry::Polyline(Polyline::rectangle(a, cursor))])
            .unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
// POLYLINE
// ---------------------------------------------------------------------------

/// 連結ポリライン。`Enter` で確定、`C` で閉じる。
///
/// LINE と違い、確定するまで 1 本も図面に入らない（全体で 1 要素になるため）。
#[derive(Debug, Default)]
pub struct PolylineTool {
    vertices: Vec<Point2>,
}

impl Tool for PolylineTool {
    fn name(&self) -> &'static str {
        "POLYLINE"
    }

    fn prompt(&self) -> String {
        match self.vertices.len() {
            0 => "ポリラインの始点を指定:".to_owned(),
            1 => "次の点を指定 <Enter で終了>:".to_owned(),
            _ => "次の点を指定 [閉じる(C)] <Enter で終了>:".to_owned(),
        }
    }

    fn last_point(&self) -> Option<Point2> {
        self.vertices.last().copied()
    }

    fn step(&mut self, input: StepInput, ctx: &ToolCtx<'_>) -> StepOutcome {
        match input {
            StepInput::Point(p) => {
                self.vertices.push(p);
                StepOutcome::Continue
            }
            StepInput::Word(w) if w == "C" => self.commit(true, ctx),
            StepInput::Word(w) => StepOutcome::Reject(format!("不明なオプションです: {w}")),
            StepInput::Number(_) => StepOutcome::Reject("点を指定してください".to_owned()),
            StepInput::Enter | StepInput::SelectionReady => self.commit(false, ctx),
        }
    }

    fn preview(&self, cursor: Point2, _ctx: &ToolCtx<'_>) -> Vec<Geometry> {
        if self.vertices.is_empty() {
            return Vec::new();
        }
        let mut v = self.vertices.clone();
        v.push(cursor);
        vec![Geometry::Polyline(Polyline::new(v, false))]
    }
}

impl PolylineTool {
    fn commit(&mut self, closed: bool, ctx: &ToolCtx<'_>) -> StepOutcome {
        if closed && self.vertices.len() < 3 {
            return StepOutcome::Reject("閉じるには頂点が 3 つ以上必要です".to_owned());
        }
        let poly = Polyline::new(std::mem::take(&mut self.vertices), closed);
        if poly.is_degenerate() {
            return StepOutcome::Finish;
        }
        add_one_and_finish("POLYLINE", Geometry::Polyline(poly), ctx)
    }
}
