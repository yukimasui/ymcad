//! グリップ編集のツール（Issue #30 段階 1、ADR-0045）。
//!
//! 待機中に選択した図形のグリップをクリックすると `Session::start_grip` がこれを始める。
//! 次のクリック（または座標・長さの打ち込み）で、掴んだ点をそこへ動かした形に置き換える
//! （`ReplaceGeometries`、Undo 1 回）。形の計算は [`crate::grips`] の純粋関数。
//!
//! ほかのツールと同じ `Tool` なので、スナップ・直交・極トラッキング（`tracking_base`）・
//! 寸法入力と直接距離入力（`dimension_base`）・座標の打ち込みがそのまま効く。
//! 名前で始めるコマンドではない（コマンド表に無く、空の Enter で再実行されない）。

use cad_core::command::ReplaceGeometries;
use cad_core::geom::Point2;
use cad_core::{Document, EntityId, Geometry};

use super::{StepInput, StepOutcome, Tool, ToolCtx};
use crate::grips::{self, Grip, Handle};

/// ツールの名前（Undo の表示・案内に出る）。コマンド表には無い。
pub const GRIP_COMMAND: &str = "GRIP";

/// 掴んだグリップ 1 つと、その図形の元の形。
#[derive(Debug, Clone)]
struct Target {
    id: EntityId,
    handle: Handle,
    /// 掴んだときの形の写し。仮の形と確定する形はここから計算する。
    original: Geometry,
}

/// 掴んでいる間のツール。
///
/// 掴んだグリップは**列**で持つ（段階 1 は 1 件。段階 2 で重なったグリップをまとめて動かす）。
#[derive(Debug)]
pub struct GripTool {
    targets: Vec<Target>,
    /// 掴んだ点の元の位置（相対座標 `@` の基準）。
    from: Point2,
    /// 掴んだときの図面の版番号。違っていたら確定を断る（元の形の写しが古い）。
    revision: u64,
}

impl GripTool {
    /// `grip` を掴む。図形が無い・そのグリップが無いなら `None`。
    #[must_use]
    pub fn new(doc: &Document, grip: &Grip) -> Option<Self> {
        let original = doc.entities().get(grip.id)?.geom.clone();
        let from = grips::position(&original, grip.handle)?;
        Some(Self {
            targets: vec![Target {
                id: grip.id,
                handle: grip.handle,
                original,
            }],
            from,
            revision: doc.revision(),
        })
    }

    /// 掴んでいるグリップ（描画で「ホット」にする）。
    #[must_use]
    pub fn grips(&self) -> Vec<Grip> {
        self.targets
            .iter()
            .map(|t| Grip {
                id: t.id,
                handle: t.handle,
                at: self.from,
            })
            .collect()
    }

    /// 掴んでいるグリップの役目（先頭）。
    fn handle(&self) -> Handle {
        self.targets[0].handle
    }

    /// 先頭の図形の元の形。長さ・角度の基点を決める。
    fn first(&self) -> &Target {
        &self.targets[0]
    }

    /// `to` へ動かした形の列。どれか 1 つでも成り立たなければ `Err`（全部か無しか）。
    fn moved(&self, to: Point2) -> Result<Vec<(EntityId, Geometry)>, grips::GripError> {
        self.targets
            .iter()
            .map(|t| grips::apply(&t.original, t.handle, to).map(|g| (t.id, g)))
            .collect()
    }
}

impl Tool for GripTool {
    fn name(&self) -> &'static str {
        GRIP_COMMAND
    }

    fn prompt(&self) -> String {
        format!(
            "** {} ** 点を指定 (Esc・Enter で取り消し):",
            self.handle().label()
        )
    }

    /// 相対座標 `@dx,dy` は掴んだ点の元の位置から。
    fn last_point(&self) -> Option<Point2> {
        Some(self.from)
    }

    /// 長さ・角度は形の基準から（ユーザー判断 1。線分の端点なら反対側の端点）。
    fn dimension_base(&self) -> Option<Point2> {
        let t = self.first();
        grips::dimension_base(&t.original, t.handle)
    }

    /// 直交・極も寸法入力と同じ基点から。円の四分点（半径）だけ外す。
    fn tracking_base(&self) -> Option<Point2> {
        if grips::tracks(self.handle()) {
            self.dimension_base()
        } else {
            None
        }
    }

    fn held_entities(&self) -> Vec<EntityId> {
        self.targets.iter().map(|t| t.id).collect()
    }

    fn step(&mut self, input: StepInput, ctx: &ToolCtx<'_>) -> StepOutcome {
        let StepInput::Point(to) = input else {
            // 空の Enter・`U`・コマンド名は `Session` が先に扱う。
            return StepOutcome::Reject("点を指定するか Esc で取り消してください".to_owned());
        };
        if ctx.doc.revision() != self.revision {
            return StepOutcome::Abort(
                "掴んだ後に図面が変わったため確定できません。グリップを取り消しました".to_owned(),
            );
        }
        let moved = match self.moved(to) {
            Ok(m) => m,
            Err(e) => return StepOutcome::Reject(e.message()),
        };
        let changed: Vec<(EntityId, Geometry)> = moved
            .into_iter()
            .zip(&self.targets)
            .filter(|((_, g), t)| *g != t.original)
            .map(|(m, _)| m)
            .collect();
        if changed.is_empty() {
            // 元の位置でのクリック。履歴を増やさずに終える（ADR-0040 の補足）。
            return StepOutcome::Finish;
        }
        StepOutcome::Apply(Box::new(ReplaceGeometries::new(GRIP_COMMAND, changed)))
    }

    /// 琥珀色の仮の形。成り立たない位置では何も出さない（クリックしても断られる）。
    fn preview(&self, cursor: Point2, _ctx: &ToolCtx<'_>) -> Vec<Geometry> {
        self.moved(cursor)
            .map(|m| m.into_iter().map(|(_, g)| g).collect())
            .unwrap_or_default()
    }

    fn grip(&self) -> Option<&GripTool> {
        Some(self)
    }
}
