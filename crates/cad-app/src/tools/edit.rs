//! 編集コマンドとビュー操作コマンド。

use cad_core::command::{
    CopyEntities, CornerEntities, CornerKind, CreateGroup, DeleteEntities, ExplodeEntities,
    ExtendEntity, MirrorCopyEntities, MirrorEntities, MoveEntities, RotateCopyEntities,
    RotateEntities, ScaleCopyEntities, ScaleEntities, StretchEntities, TrimEntity, Ungroup,
};
use cad_core::geom::tolerance::is_zero_len;
use cad_core::geom::{Aabb, Line, Point2};
use cad_core::Geometry;

use super::{StepInput, StepOutcome, Tool, ToolCtx, ToolSettings};
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
            StepInput::Entity { .. } => {
                StepOutcome::Reject("図形ではなく点を指定してください".to_owned())
            }
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
            StepInput::Entity { .. } => {
                StepOutcome::Reject("図形ではなく点を指定してください".to_owned())
            }
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
            StepInput::Entity { .. } => {
                StepOutcome::Reject("図形ではなく点を指定してください".to_owned())
            }
        }
    }

    fn preview(&self, cursor: Point2, ctx: &ToolCtx<'_>) -> Vec<Geometry> {
        stretched_selection(self.base, cursor, ctx.crossing_rects, ctx)
    }
}

// ---------------------------------------------------------------------------
// ROTATE / SCALE / MIRROR
// ---------------------------------------------------------------------------

/// 選択中の図形を変換したラバーバンドを返す。
///
/// `base` が未指定なら何も返さない（変換の基準が決まっていないため）。
fn transformed_selection(
    ctx: &ToolCtx<'_>,
    transform: impl Fn(&Geometry) -> Geometry,
) -> Vec<Geometry> {
    ctx.selection
        .iter()
        .filter_map(|id| ctx.doc.entities().get(id))
        .map(|e| transform(&e.geom))
        .collect()
}

/// 基点から見たカーソルの角度 [rad]。基点と重なっていれば `None`。
fn angle_from(base: Point2, cursor: Point2) -> Option<f64> {
    (cursor - base).normalized().map(|d| d.angle())
}

/// 選択したオブジェクトを基点まわりに回転する。
///
/// AutoCAD の ROTATE。角度は**度**で入力する（座標入力の `@100<45` と同じ約束）。
/// 点で指定した場合は「基点から見たその点の方向」が角度になる。
#[derive(Debug, Default)]
pub struct RotateTool {
    base: Option<Point2>,
    /// `C` が指定されたら元図形を残す。
    copy: bool,
}

impl Tool for RotateTool {
    fn name(&self) -> &'static str {
        "ROTATE"
    }

    fn prompt(&self) -> String {
        match (self.base, self.copy) {
            (None, _) => "基点を指定:".to_owned(),
            (Some(_), false) => "回転角度を指定 [コピー(C)]:".to_owned(),
            (Some(_), true) => "回転角度を指定 <コピー>:".to_owned(),
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

            // 基点の指定。
            StepInput::Point(p) if self.base.is_none() => {
                self.base = Some(p);
                StepOutcome::Continue
            }
            // 角度を点で示した場合。
            StepInput::Point(p) => {
                let base = self.base.expect("直前の腕で None は除外済み");
                match angle_from(base, p) {
                    Some(angle) => self.commit(base, angle, ctx),
                    None => StepOutcome::Reject("基点と同じ点です".to_owned()),
                }
            }
            // 角度を数値（度）で入力した場合。
            StepInput::Number(deg) => match self.base {
                Some(base) => self.commit(base, deg.to_radians(), ctx),
                None => StepOutcome::Reject("先に基点を指定してください".to_owned()),
            },

            StepInput::Word(w) if w == "C" => {
                if self.base.is_none() {
                    return StepOutcome::Reject("先に基点を指定してください".to_owned());
                }
                self.copy = true;
                StepOutcome::Continue
            }
            StepInput::Word(w) => StepOutcome::Reject(format!("不明なオプションです: {w}")),
            StepInput::Enter => StepOutcome::Finish,
            StepInput::Entity { .. } => {
                StepOutcome::Reject("図形ではなく点を指定してください".to_owned())
            }
        }
    }

    fn preview(&self, cursor: Point2, ctx: &ToolCtx<'_>) -> Vec<Geometry> {
        let Some(base) = self.base else {
            return Vec::new();
        };
        let Some(angle) = angle_from(base, cursor) else {
            return Vec::new();
        };
        transformed_selection(ctx, |g| g.rotated(base, angle))
    }
}

impl RotateTool {
    fn commit(&self, base: Point2, angle: f64, ctx: &ToolCtx<'_>) -> StepOutcome {
        let targets = ctx.selection.to_vec();
        if targets.is_empty() {
            return StepOutcome::Finish;
        }
        if self.copy {
            StepOutcome::Apply(Box::new(RotateCopyEntities::new(
                "ROTATE", targets, base, angle,
            )))
        } else {
            StepOutcome::Apply(Box::new(RotateEntities::new(
                "ROTATE", targets, base, angle,
            )))
        }
    }
}

/// 選択したオブジェクトを基点を中心に拡大縮小する。
///
/// AutoCAD の SCALE。倍率は数値で入力する。
/// 点で指定した場合は「基点からの距離」が倍率になる。
#[derive(Debug, Default)]
pub struct ScaleTool {
    base: Option<Point2>,
    /// `C` が指定されたら元図形を残す。
    copy: bool,
}

impl Tool for ScaleTool {
    fn name(&self) -> &'static str {
        "SCALE"
    }

    fn prompt(&self) -> String {
        match (self.base, self.copy) {
            (None, _) => "基点を指定:".to_owned(),
            (Some(_), false) => "尺度を指定 [コピー(C)]:".to_owned(),
            (Some(_), true) => "尺度を指定 <コピー>:".to_owned(),
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

            StepInput::Point(p) if self.base.is_none() => {
                self.base = Some(p);
                StepOutcome::Continue
            }
            // 基点からの距離を倍率とみなす。
            StepInput::Point(p) => {
                let base = self.base.expect("直前の腕で None は除外済み");
                self.commit(base, base.dist(p), ctx)
            }
            StepInput::Number(factor) => match self.base {
                Some(base) => self.commit(base, factor, ctx),
                None => StepOutcome::Reject("先に基点を指定してください".to_owned()),
            },

            StepInput::Word(w) if w == "C" => {
                if self.base.is_none() {
                    return StepOutcome::Reject("先に基点を指定してください".to_owned());
                }
                self.copy = true;
                StepOutcome::Continue
            }
            StepInput::Word(w) => StepOutcome::Reject(format!("不明なオプションです: {w}")),
            StepInput::Enter => StepOutcome::Finish,
            StepInput::Entity { .. } => {
                StepOutcome::Reject("図形ではなく点を指定してください".to_owned())
            }
        }
    }

    fn preview(&self, cursor: Point2, ctx: &ToolCtx<'_>) -> Vec<Geometry> {
        let Some(base) = self.base else {
            return Vec::new();
        };
        let factor = base.dist(cursor);
        if !Self::is_valid_factor(factor) {
            return Vec::new();
        }
        transformed_selection(ctx, |g| g.scaled(base, factor))
    }
}

impl ScaleTool {
    /// 使える倍率か。
    ///
    /// 0 は図形が点に潰れ、負値は鏡像になってしまう。どちらも SCALE の意図ではないので拒否する
    /// （鏡像が欲しいなら MIRROR を使う）。
    fn is_valid_factor(factor: f64) -> bool {
        factor.is_finite() && !is_zero_len(factor) && factor > 0.0
    }

    fn commit(&self, base: Point2, factor: f64, ctx: &ToolCtx<'_>) -> StepOutcome {
        if !Self::is_valid_factor(factor) {
            return StepOutcome::Reject(
                "尺度は 0 より大きい値を指定してください（反転は MIRROR を使います）".to_owned(),
            );
        }
        let targets = ctx.selection.to_vec();
        if targets.is_empty() {
            return StepOutcome::Finish;
        }
        if self.copy {
            StepOutcome::Apply(Box::new(ScaleCopyEntities::new(
                "SCALE", targets, base, factor,
            )))
        } else {
            StepOutcome::Apply(Box::new(ScaleEntities::new("SCALE", targets, base, factor)))
        }
    }
}

/// 選択したオブジェクトを鏡像反転する。
///
/// AutoCAD の MIRROR。鏡像軸を 2 点で指定し、最後に元図形を消すか聞く（既定は残す）。
#[derive(Debug, Default)]
pub struct MirrorTool {
    first: Option<Point2>,
    /// 軸が確定した状態。元を消すかの返事待ち。
    axis: Option<Line>,
}

impl Tool for MirrorTool {
    fn name(&self) -> &'static str {
        "MIRROR"
    }

    fn prompt(&self) -> String {
        if self.axis.is_some() {
            "元のオブジェクトを消去しますか? [はい(Y)/いいえ(N)] <N>:".to_owned()
        } else if self.first.is_none() {
            "対称軸の 1 点目を指定:".to_owned()
        } else {
            "対称軸の 2 点目を指定:".to_owned()
        }
    }

    fn wants_selection(&self) -> bool {
        true
    }

    fn last_point(&self) -> Option<Point2> {
        self.first
    }

    fn step(&mut self, input: StepInput, ctx: &ToolCtx<'_>) -> StepOutcome {
        // 軸が決まった後は Y / N の返事だけを受ける。
        if let Some(axis) = self.axis {
            return match input {
                // 既定は「残す」。AutoCAD と同じ。
                StepInput::Enter => self.commit(axis, false, ctx),
                StepInput::Word(w) if w == "N" || w == "NO" => self.commit(axis, false, ctx),
                StepInput::Word(w) if w == "Y" || w == "YES" => self.commit(axis, true, ctx),
                _ => StepOutcome::Reject("はい(Y) または いいえ(N) を指定してください".to_owned()),
            };
        }

        match input {
            StepInput::SelectionReady => StepOutcome::Continue,
            StepInput::Point(p) => {
                let Some(first) = self.first else {
                    self.first = Some(p);
                    return StepOutcome::Continue;
                };
                let axis = Line::new(first, p);
                if axis.is_degenerate() {
                    return StepOutcome::Reject("同じ点が指定されました".to_owned());
                }
                self.axis = Some(axis);
                StepOutcome::Continue
            }
            StepInput::Enter => StepOutcome::Finish,
            StepInput::Word(w) => StepOutcome::Reject(format!("不明なオプションです: {w}")),
            StepInput::Number(_) => StepOutcome::Reject("点を指定してください".to_owned()),
            StepInput::Entity { .. } => {
                StepOutcome::Reject("図形ではなく点を指定してください".to_owned())
            }
        }
    }

    fn preview(&self, cursor: Point2, ctx: &ToolCtx<'_>) -> Vec<Geometry> {
        // 軸が決まる前は、1 点目とカーソルを結ぶ仮の軸で鏡像を見せる。
        let axis = match (self.axis, self.first) {
            (Some(axis), _) => axis,
            (None, Some(first)) => Line::new(first, cursor),
            (None, None) => return Vec::new(),
        };
        if axis.is_degenerate() {
            return Vec::new();
        }
        transformed_selection(ctx, |g| g.mirrored(&axis))
    }
}

impl MirrorTool {
    fn commit(&self, axis: Line, erase_source: bool, ctx: &ToolCtx<'_>) -> StepOutcome {
        let targets = ctx.selection.to_vec();
        if targets.is_empty() {
            return StepOutcome::Finish;
        }
        if erase_source {
            StepOutcome::Apply(Box::new(MirrorEntities::new("MIRROR", targets, axis)))
        } else {
            StepOutcome::Apply(Box::new(MirrorCopyEntities::new("MIRROR", targets, axis)))
        }
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

// ---------------------------------------------------------------------------
// GROUP / UNGROUP / EXPLODE
// ---------------------------------------------------------------------------

/// 選択した要素を 1 つのグループにまとめる。
///
/// AutoCAD の GROUP。名前を省略すると連番の既定名が付く。
/// グループの一員をクリックすると全体が選択されるようになる。
#[derive(Debug, Default)]
pub struct GroupTool {
    /// 選択が終わり、名前の入力待ちか。
    naming: bool,
}

impl Tool for GroupTool {
    fn name(&self) -> &'static str {
        "GROUP"
    }

    fn prompt(&self) -> String {
        if self.naming {
            "グループ名を入力 <Enter で既定名>:".to_owned()
        } else {
            "オブジェクトを選択 (Enter で確定):".to_owned()
        }
    }

    fn wants_selection(&self) -> bool {
        true
    }

    fn step(&mut self, input: StepInput, ctx: &ToolCtx<'_>) -> StepOutcome {
        match input {
            StepInput::SelectionReady => {
                self.naming = true;
                StepOutcome::Continue
            }
            // 既定名で作る。
            StepInput::Enter => self.commit(ctx.doc.groups().next_default_name(), ctx),
            // 打った文字列をそのまま名前にする。
            StepInput::Word(w) => self.commit(w, ctx),
            // 数字だけの名前も許す（`interpret` が数値として渡してくるため）。
            StepInput::Number(n) => self.commit(format_number_name(n), ctx),
            StepInput::Point(_) => StepOutcome::Reject("グループ名を入力してください".to_owned()),
            StepInput::Entity { .. } => {
                StepOutcome::Reject("図形ではなく点を指定してください".to_owned())
            }
        }
    }
}

/// 数値として解釈された入力をグループ名に戻す。
///
/// `1` のような入力は `Session::interpret` が数値にしてしまうので、
/// 名前として扱えるよう文字列へ直す。整数なら小数点を付けない。
fn format_number_name(n: f64) -> String {
    if n.fract() == 0.0 && n.is_finite() {
        format!("{n:.0}")
    } else {
        n.to_string()
    }
}

impl GroupTool {
    fn commit(&self, group_name: String, ctx: &ToolCtx<'_>) -> StepOutcome {
        let targets = ctx.selection.to_vec();
        if targets.is_empty() {
            return StepOutcome::Finish;
        }
        if ctx.doc.groups().by_name(&group_name).is_some() {
            return StepOutcome::Reject(format!("同じ名前のグループがあります: {group_name}"));
        }
        StepOutcome::Apply(Box::new(CreateGroup::new("GROUP", group_name, targets)))
    }
}

/// グループを解除する。
///
/// AutoCAD の UNGROUP。選択した要素が属するグループを解除する。
/// 要素そのものは残る。
#[derive(Debug, Default)]
pub struct UngroupTool;

impl Tool for UngroupTool {
    fn name(&self) -> &'static str {
        "UNGROUP"
    }

    fn prompt(&self) -> String {
        "解除するグループの要素を選択 (Enter で確定):".to_owned()
    }

    fn wants_selection(&self) -> bool {
        true
    }

    fn step(&mut self, input: StepInput, ctx: &ToolCtx<'_>) -> StepOutcome {
        match input {
            StepInput::SelectionReady | StepInput::Enter => {
                // 選択されている要素が属するグループを集める。
                let mut groups: Vec<_> = ctx
                    .selection
                    .iter()
                    .filter_map(|id| ctx.doc.entities().get(id))
                    .filter_map(|e| e.group)
                    .collect();
                groups.sort_unstable();
                groups.dedup();

                match groups.len() {
                    0 => StepOutcome::Reject("選択した要素はグループに属していません".to_owned()),
                    // 複数のグループにまたがる場合は、まとめて解除する。
                    _ => {
                        let commands: Vec<Box<dyn cad_core::Command>> = groups
                            .into_iter()
                            .map(|g| {
                                Box::new(Ungroup::new("UNGROUP", g)) as Box<dyn cad_core::Command>
                            })
                            .collect();
                        StepOutcome::Apply(Box::new(cad_core::command::MacroCommand::new(
                            "UNGROUP", commands,
                        )))
                    }
                }
            }
            _ => StepOutcome::Reject("Enter で解除を実行してください".to_owned()),
        }
    }
}

/// 選択した要素を分解する。
///
/// AutoCAD の EXPLODE。ポリラインを線分の集合にする。
#[derive(Debug, Default)]
pub struct ExplodeTool;

impl Tool for ExplodeTool {
    fn name(&self) -> &'static str {
        "EXPLODE"
    }

    fn prompt(&self) -> String {
        "分解するオブジェクトを選択 (Enter で確定):".to_owned()
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
                StepOutcome::Apply(Box::new(ExplodeEntities::new("EXPLODE", targets)))
            }
            _ => StepOutcome::Reject("Enter で分解を実行してください".to_owned()),
        }
    }
}

// ---------------------------------------------------------------------------
// TRIM / EXTEND / FILLET / CHAMFER
// ---------------------------------------------------------------------------

/// 線分を切り取る。
///
/// AutoCAD の TRIM。**切断エッジは選ばない。** 図面上の他のすべての図形を
/// 暗黙のエッジとして使い、いきなり「切る部分をクリック」で始める
/// （近年の AutoCAD のクイックモード相当）。
///
/// 続けて何本でも切れる。`Enter` か `Esc` で終わる。
#[derive(Debug, Default)]
pub struct TrimTool;

impl Tool for TrimTool {
    fn name(&self) -> &'static str {
        "TRIM"
    }

    fn prompt(&self) -> String {
        "切り取る部分をクリック <Enter で終了>:".to_owned()
    }

    fn wants_entity(&self) -> bool {
        true
    }

    fn step(&mut self, input: StepInput, _ctx: &ToolCtx<'_>) -> StepOutcome {
        match input {
            StepInput::Entity { id, at } => {
                StepOutcome::ApplyAndContinue(Box::new(TrimEntity::new("TRIM", id, at)))
            }
            // 図形を拾えなかったクリック。外したことを伝えて続行する。
            StepInput::Point(_) => StepOutcome::Reject("図形の上をクリックしてください".to_owned()),
            StepInput::Enter | StepInput::SelectionReady => StepOutcome::Finish,
            StepInput::Word(w) => StepOutcome::Reject(format!("不明なオプションです: {w}")),
            StepInput::Number(_) => {
                StepOutcome::Reject("図形の上をクリックしてください".to_owned())
            }
        }
    }
}

/// 線分を伸ばす。
///
/// AutoCAD の EXTEND。TRIM と同じく、図面上の他のすべての図形を暗黙の境界にする。
/// **クリックした側の端**が伸びる。
#[derive(Debug, Default)]
pub struct ExtendTool;

impl Tool for ExtendTool {
    fn name(&self) -> &'static str {
        "EXTEND"
    }

    fn prompt(&self) -> String {
        "伸ばす側の端をクリック <Enter で終了>:".to_owned()
    }

    fn wants_entity(&self) -> bool {
        true
    }

    fn step(&mut self, input: StepInput, _ctx: &ToolCtx<'_>) -> StepOutcome {
        match input {
            StepInput::Entity { id, at } => {
                StepOutcome::ApplyAndContinue(Box::new(ExtendEntity::new("EXTEND", id, at)))
            }
            StepInput::Point(_) => StepOutcome::Reject("図形の上をクリックしてください".to_owned()),
            StepInput::Enter | StepInput::SelectionReady => StepOutcome::Finish,
            StepInput::Word(w) => StepOutcome::Reject(format!("不明なオプションです: {w}")),
            StepInput::Number(_) => {
                StepOutcome::Reject("図形の上をクリックしてください".to_owned())
            }
        }
    }
}

/// 角丸めと面取りの共通のツール。
///
/// どちらも「値を決める → 2 本の線分をクリック」という同じ流れなので 1 つにまとめる。
/// 値は AutoCAD と同じくコマンド間で記憶される。
#[derive(Debug)]
pub struct CornerTool {
    /// 角丸めか面取りか。
    fillet: bool,
    /// 1 本目のクリック。2 本目待ちなら `Some`。
    first: Option<(cad_core::EntityId, Point2)>,
    /// 値の入力待ちか（`R` / `D` オプション）。
    editing: Option<CornerSetting>,
}

/// 値の入力待ちの内訳。
#[derive(Debug, Clone, Copy, PartialEq)]
enum CornerSetting {
    Radius,
    Distance1,
    Distance2 { d1: f64 },
}

impl CornerTool {
    /// 角丸め用。
    #[must_use]
    pub fn fillet() -> Self {
        Self {
            fillet: true,
            first: None,
            editing: None,
        }
    }

    /// 面取り用。
    #[must_use]
    pub fn chamfer() -> Self {
        Self {
            fillet: false,
            first: None,
            editing: None,
        }
    }

    fn kind(&self, s: &ToolSettings) -> CornerKind {
        if self.fillet {
            CornerKind::Fillet {
                radius: s.fillet_radius,
            }
        } else {
            CornerKind::Chamfer {
                d1: s.chamfer_d1,
                d2: s.chamfer_d2,
            }
        }
    }
}

impl Tool for CornerTool {
    fn name(&self) -> &'static str {
        if self.fillet {
            "FILLET"
        } else {
            "CHAMFER"
        }
    }

    fn prompt(&self) -> String {
        match self.editing {
            Some(CornerSetting::Radius) => "半径を指定:".to_owned(),
            Some(CornerSetting::Distance1) => "1 本目の距離を指定:".to_owned(),
            Some(CornerSetting::Distance2 { .. }) => "2 本目の距離を指定:".to_owned(),
            None if self.first.is_some() => "2 本目の線分をクリック:".to_owned(),
            None if self.fillet => "1 本目の線分をクリック [半径(R)]:".to_owned(),
            None => "1 本目の線分をクリック [距離(D)]:".to_owned(),
        }
    }

    fn wants_entity(&self) -> bool {
        // 値の入力待ちのあいだは図形を拾わない。
        self.editing.is_none()
    }

    fn step(&mut self, input: StepInput, ctx: &ToolCtx<'_>) -> StepOutcome {
        // ---- 値の入力待ち ----
        if let Some(pending) = self.editing {
            let StepInput::Number(v) = input else {
                return match input {
                    StepInput::Enter => {
                        self.editing = None;
                        StepOutcome::Continue
                    }
                    _ => StepOutcome::Reject("数値を指定してください".to_owned()),
                };
            };
            if !v.is_finite() || v <= 0.0 {
                return StepOutcome::Reject("0 より大きい値を指定してください".to_owned());
            }
            let mut s = ctx.settings;
            match pending {
                CornerSetting::Radius => {
                    s.fillet_radius = v;
                    self.editing = None;
                }
                CornerSetting::Distance1 => {
                    s.chamfer_d1 = v;
                    self.editing = Some(CornerSetting::Distance2 { d1: v });
                }
                CornerSetting::Distance2 { d1 } => {
                    s.chamfer_d1 = d1;
                    s.chamfer_d2 = v;
                    self.editing = None;
                }
            }
            return StepOutcome::Setting(s);
        }

        match input {
            StepInput::Word(w) if self.fillet && w == "R" => {
                self.editing = Some(CornerSetting::Radius);
                StepOutcome::Continue
            }
            StepInput::Word(w) if !self.fillet && w == "D" => {
                self.editing = Some(CornerSetting::Distance1);
                StepOutcome::Continue
            }
            StepInput::Word(w) => StepOutcome::Reject(format!("不明なオプションです: {w}")),

            StepInput::Entity { id, at } => {
                let Some((first_id, pick1)) = self.first else {
                    self.first = Some((id, at));
                    return StepOutcome::Continue;
                };
                if first_id == id {
                    return StepOutcome::Reject("別の線分をクリックしてください".to_owned());
                }
                let cmd = CornerEntities::new(
                    self.name(),
                    first_id,
                    id,
                    pick1,
                    at,
                    self.kind(&ctx.settings),
                );
                self.first = None;
                StepOutcome::ApplyAndContinue(Box::new(cmd))
            }

            StepInput::Point(_) => StepOutcome::Reject("線分の上をクリックしてください".to_owned()),
            StepInput::Enter | StepInput::SelectionReady => StepOutcome::Finish,
            StepInput::Number(_) => StepOutcome::Reject(
                "先に R（半径）または D（距離）で値を指定してください".to_owned(),
            ),
        }
    }
}
