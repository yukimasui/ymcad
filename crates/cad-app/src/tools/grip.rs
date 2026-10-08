//! グリップ編集のツール（Issue #30 段階 1・2、ADR-0045）。
//!
//! 待機中に選択した図形のグリップをクリックすると `Session::start_grip` がこれを始める。
//! 同じ位置に重なったグリップ（選んだ図形どうしの共有点）はまとめて掴む（段階 2）。
//! 次のクリック（または座標・長さの打ち込み）で、掴んだ点をそこへ動かした形に置き換える
//! （1 回の `ReplaceGeometries`、Undo 1 回。全部か無しか）。形の計算は [`crate::grips`] の純粋関数。
//!
//! ほかのツールと同じ `Tool` なので、スナップ・直交・極トラッキング（`tracking_base`）・
//! 寸法入力と直接距離入力（`dimension_base`）・座標の打ち込みがそのまま効く。
//! 名前で始めるコマンドではない（コマンド表に無く、空の Enter で再実行されない）。

use cad_core::command::ReplaceGeometries;
use cad_core::geom::Point2;
use cad_core::{Document, EntityId, Geometry};

use super::{StepInput, StepOutcome, Tool, ToolCtx};
use crate::cmdline::dimension::DimKind;
use crate::grips::{self, Grip, GripGroup, Handle};
use crate::properties::kind_of;
use crate::selection;

/// ツールの名前（Undo の表示・案内に出る）。コマンド表には無い。
pub const GRIP_COMMAND: &str = "GRIP";

/// 掴んだ図形 1 つと、動かすグリップと、その図形の元の形。
#[derive(Debug, Clone)]
struct Target {
    id: EntityId,
    /// 動かすグリップ（[`GripGroup::parts`]）。ふつうは 1 つ。代表の図形なら先頭が代表。
    handles: Vec<Handle>,
    /// 掴んだときの形の写し。仮の形と確定する形はここから計算する。
    original: Geometry,
}

/// 掴んでいる間のツール。
///
/// 掴んだグリップは図形ごとの**列**で持つ（段階 2: 重なったグリップをまとめて動かす）。
/// 先頭が代表（クリックで拾われたグリップの図形）で、長さ・角度の基点と直交・極は代表の規則で決まる。
#[derive(Debug)]
pub struct GripTool {
    targets: Vec<Target>,
    /// 掴んだ点の元の位置（相対座標 `@` の基準）。
    from: Point2,
    /// 乗せたときと同じ案内（「端点を動かす（2 個）」など）。プロンプトに出す。
    label: String,
}

impl GripTool {
    /// `group` を掴む。図形が無い・そのグリップが無いものが 1 つでもあれば `None`。
    #[must_use]
    pub fn new(doc: &Document, group: &GripGroup) -> Option<Self> {
        let rep = group.representative();
        let mut targets = Vec::new();
        for (id, handles) in group.parts() {
            let original = doc.entities().get(id)?.geom.clone();
            for h in &handles {
                grips::position(&original, *h)?;
            }
            targets.push(Target {
                id,
                handles,
                original,
            });
        }
        let from = grips::position(&targets.first()?.original, rep.handle)?;
        Some(Self {
            targets,
            from,
            label: group.label(),
        })
    }

    /// 掴んでいるグリップ（描画で「ホット」にする）。
    #[must_use]
    pub fn grips(&self) -> Vec<Grip> {
        self.targets
            .iter()
            .flat_map(|t| {
                t.handles.iter().map(|h| Grip {
                    id: t.id,
                    handle: *h,
                    at: self.from,
                })
            })
            .collect()
    }

    /// 代表のグリップの役目。
    fn handle(&self) -> Handle {
        self.first().handles[0]
    }

    /// 代表の図形。長さ・角度の基点を決める。
    fn first(&self) -> &Target {
        &self.targets[0]
    }

    /// 寸法入力の欄の見せ方。代表が円の四分点なら「半径」1 欄（角度に意味が無い）。
    #[must_use]
    pub fn dimension_kind(&self) -> DimKind {
        if matches!(self.handle(), Handle::CircleQuadrant(_)) {
            DimKind::Radius
        } else {
            DimKind::LengthAngle
        }
    }

    /// `to` へ動かした形の列。どれか 1 つでも成り立たなければ、どの図形のどの理由かを書いた
    /// 文言で `Err`（全部か無しか）。
    fn moved(&self, to: Point2) -> Result<Vec<(EntityId, Geometry)>, String> {
        let parts: Vec<(&Geometry, &[Handle])> = self
            .targets
            .iter()
            .map(|t| (&t.original, t.handles.as_slice()))
            .collect();
        match grips::apply_group(&parts, to) {
            Ok(moved) => Ok(self.targets.iter().map(|t| t.id).zip(moved).collect()),
            Err(e) if self.targets.len() == 1 => Err(e.error.message()),
            Err(e) => {
                let t = &self.targets[e.index];
                Err(format!(
                    "まとめて動かす {} 個のうち{}が動かせません: {}",
                    self.targets.len(),
                    kind_of(&t.original).label(),
                    e.error.message()
                ))
            }
        }
    }

    /// 掴んだ図形が、掴んだときのまま（形が同じで、編集できる）か。
    ///
    /// 確定の前に確かめる（Issue #30 段階 1 のレビュー N1）。関係の無い変更（別のレイヤの色など）
    /// では中断しない。レイヤのロック・非表示・削除は `Session::revalidate` が `held_entities` で
    /// その場で中断するが、念のためここでも見る。
    fn unchanged(&self, doc: &Document) -> bool {
        self.targets.iter().all(|t| {
            selection::is_editable(doc, t.id)
                && doc
                    .entities()
                    .get(t.id)
                    .is_some_and(|e| e.geom == t.original)
        })
    }
}

impl Tool for GripTool {
    fn name(&self) -> &'static str {
        GRIP_COMMAND
    }

    fn prompt(&self) -> String {
        format!(
            "** {} ** 点を指定 (空の Enter・Esc で取り消し):",
            self.label
        )
    }

    /// 相対座標 `@dx,dy` は掴んだ点の元の位置から。
    fn last_point(&self) -> Option<Point2> {
        Some(self.from)
    }

    /// 長さ・角度は代表の形の基準から（ユーザー判断 1。線分の端点なら反対側の端点）。
    fn dimension_base(&self) -> Option<Point2> {
        let t = self.first();
        grips::dimension_base(&t.original, self.handle())
    }

    /// 直交・極も寸法入力と同じ基点から。代表が円の四分点（半径）なら外す。
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
        if !self.unchanged(ctx.doc) {
            return StepOutcome::Abort(
                "掴んだ後に図形が変わったため確定できません。グリップを取り消しました".to_owned(),
            );
        }
        let moved = match self.moved(to) {
            Ok(m) => m,
            Err(why) => return StepOutcome::Reject(why),
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
