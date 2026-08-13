//! コンポーネントの定義と配置のツール。
//!
//! # `COMPONENT` は選択をその場でインスタンスに置き換える
//!
//! AutoCAD の `BLOCK` は選択した要素を**消す**。定義はできているが画面から
//! 図形が消えるので、続けて `INSERT` するまで何も無い状態になる。
//!
//! ここでは Figma の「コンポーネント化」と同じく、
//! **選択をその場で 1 つのインスタンスに置き換える**。
//! 見た目は変わらないまま、以後は定義を編集すれば全体に反映される。
//! 「作ったのに消えた」という驚きが無く、モーダルな往復も要らない。
//!
//! # 対話
//!
//! ```text
//! COMPONENT → 選択 → Enter → 基点 → 名前（Enter で既定名）
//! INSERT    → 名前 → 位置 → 回転角（Enter で 0）→ 倍率（Enter で 1）
//! ```

use cad_core::command::{
    DefineComponent, DeleteEntities, EnterDefinitionEdit, InsertInstance, MacroCommand,
    SetDefinitionContents,
};
use cad_core::component::Placement;
use cad_core::geom::Point2;
use cad_core::{Entity, Geometry};

use super::{StepInput, StepOutcome, Tool, ToolCtx};

/// 選択からコンポーネント定義を作り、その場でインスタンスに置き換える。
#[derive(Debug, Default)]
pub struct ComponentTool {
    state: State,
}

#[derive(Debug, Default, Clone, Copy, PartialEq)]
enum State {
    /// 選択待ち。
    #[default]
    Selecting,
    /// 基点の指定待ち。
    Origin,
    /// 名前の入力待ち。
    Naming { origin: Point2 },
}

impl Tool for ComponentTool {
    fn name(&self) -> &'static str {
        "COMPONENT"
    }

    fn prompt(&self) -> String {
        match self.state {
            State::Selecting => "オブジェクトを選択 (Enter で確定):".to_owned(),
            State::Origin => "基点を指定:".to_owned(),
            State::Naming { .. } => "コンポーネント名を入力 <Enter で既定名>:".to_owned(),
        }
    }

    fn wants_selection(&self) -> bool {
        true
    }

    fn step(&mut self, input: StepInput, ctx: &ToolCtx<'_>) -> StepOutcome {
        match (self.state, input) {
            (State::Selecting, StepInput::SelectionReady) => {
                if ctx.selection.is_empty() {
                    return StepOutcome::Finish;
                }
                self.state = State::Origin;
                StepOutcome::Continue
            }
            (State::Origin, StepInput::Point(p)) => {
                self.state = State::Naming { origin: p };
                StepOutcome::Continue
            }
            (State::Naming { origin }, StepInput::Enter) => {
                self.commit(ctx, origin, ctx.doc.definitions().next_default_name())
            }
            (State::Naming { origin }, StepInput::Word(w)) => self.commit(ctx, origin, w),
            // `1` のような入力は数値として渡ってくるので、名前へ戻す。
            (State::Naming { origin }, StepInput::Number(n)) => {
                self.commit(ctx, origin, super::edit::format_number_name(n))
            }

            (_, StepInput::Enter | StepInput::SelectionReady) => StepOutcome::Finish,
            (State::Origin, _) => StepOutcome::Reject("基点を指定してください".to_owned()),
            (_, _) => StepOutcome::Reject("入力を解釈できません".to_owned()),
        }
    }
}

impl ComponentTool {
    /// 定義を作り、選択を 1 つのインスタンスへ置き換える。
    ///
    /// 3 つのコマンドを [`MacroCommand`] で 1 手にまとめる。
    /// 別々に積むと Undo が 3 回必要になり、「1 操作 = 1 Undo」が崩れる。
    fn commit(&self, ctx: &ToolCtx<'_>, origin: Point2, def_name: String) -> StepOutcome {
        let targets = ctx.selection.to_vec();
        if targets.is_empty() {
            return StepOutcome::Finish;
        }
        if ctx.doc.definitions().by_name(&def_name).is_some() {
            return StepOutcome::Reject(format!("コンポーネント「{def_name}」は既にあります"));
        }

        // 定義の中身は選択の複製。**インスタンスは中身に含めない**
        // （自分を含む定義になり、循環する）。
        let mut contents: Vec<Entity> = Vec::with_capacity(targets.len());
        for id in &targets {
            let Some(entity) = ctx.doc.entities().get(*id) else {
                return StepOutcome::Reject("選択が古くなっています".to_owned());
            };
            contents.push(entity.clone());
        }

        // 置き換えるインスタンスは、いま選択が置かれているレイヤに載せる。
        // 複数レイヤに散っている場合は現在レイヤを使う。
        let layer = single_layer(&contents).unwrap_or(ctx.layer);

        StepOutcome::Apply(Box::new(MacroCommand::new(
            "COMPONENT",
            vec![
                Box::new(DefineComponent::new(
                    "COMPONENT",
                    def_name.clone(),
                    origin,
                    contents,
                )),
                Box::new(DeleteEntities::new("COMPONENT", targets)),
                // 定義の ID は前のコマンドから受け取れないので、名前で引き直す。
                Box::new(InsertNewlyDefined::new(def_name, origin, layer)),
            ],
        )))
    }
}

/// 中身が全部同じレイヤなら、そのレイヤ。
fn single_layer(contents: &[Entity]) -> Option<cad_core::LayerId> {
    let first = contents.first()?.layer;
    contents.iter().all(|e| e.layer == first).then_some(first)
}

/// 名前で定義を引いてから配置するコマンド。
///
/// [`MacroCommand`] の中では前のコマンドの結果（`DefinitionId`）を受け取れない。
/// **`DefinitionId` を作る側と使う側が同じ手に入っているので、名前で橋渡しする。**
///
/// 「定義表の最後の 1 件」を使う手もあるが、Redo では `DefineComponent` が
/// `restore_definition` で元の ID に戻すため「最後」であることが自明でなくなる。
/// 名前は一意で安定なので、そちらで引く。
#[derive(Debug)]
struct InsertNewlyDefined {
    def_name: String,
    origin: Point2,
    layer: cad_core::LayerId,
    inner: Option<InsertInstance>,
}

impl InsertNewlyDefined {
    fn new(def_name: String, origin: Point2, layer: cad_core::LayerId) -> Self {
        Self {
            def_name,
            origin,
            layer,
            inner: None,
        }
    }
}

impl cad_core::Command for InsertNewlyDefined {
    fn execute(&mut self, ctx: &mut cad_core::EditCtx<'_>) -> cad_core::Result<()> {
        let Some(def) = ctx.definitions().by_name(&self.def_name) else {
            return Err(cad_core::CadError::DefinitionNotFound);
        };
        let mut cmd = InsertInstance::new("COMPONENT", def, Placement::at(self.origin), self.layer);
        cmd.execute(ctx)?;
        self.inner = Some(cmd);
        Ok(())
    }

    fn undo(&mut self, ctx: &mut cad_core::EditCtx<'_>) -> cad_core::Result<()> {
        if let Some(cmd) = &mut self.inner {
            cmd.undo(ctx)?;
        }
        self.inner = None;
        Ok(())
    }

    fn name(&self) -> &'static str {
        "COMPONENT"
    }
}

// ---------------------------------------------------------------------------
// INSERT
// ---------------------------------------------------------------------------

/// コンポーネントを配置する。
#[derive(Debug, Default)]
pub struct InsertTool {
    state: InsertState,
}

impl InsertTool {
    /// **定義を決めた状態で始める。**
    ///
    /// パネルの「配置」ボタンから呼ぶ。名前を打つ段階を飛ばせるので、
    /// 日本語入力を通さずに配置できる。
    #[must_use]
    pub fn for_definition(def: cad_core::DefinitionId) -> Self {
        Self {
            state: InsertState::Position { def },
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq)]
enum InsertState {
    /// 定義名の入力待ち。
    #[default]
    Naming,
    /// 位置の指定待ち。
    Position { def: cad_core::DefinitionId },
    /// 回転角の入力待ち。
    Rotation {
        def: cad_core::DefinitionId,
        origin: Point2,
    },
    /// 倍率の入力待ち。
    Scale {
        def: cad_core::DefinitionId,
        origin: Point2,
        rotation: f64,
    },
}

impl Tool for InsertTool {
    fn name(&self) -> &'static str {
        "INSERT"
    }

    fn prompt(&self) -> String {
        match self.state {
            InsertState::Naming => "コンポーネント名を入力:".to_owned(),
            InsertState::Position { .. } => "挿入位置を指定:".to_owned(),
            InsertState::Rotation { .. } => "回転角を指定 <Enter で 0>:".to_owned(),
            InsertState::Scale { .. } => "倍率を指定 <Enter で 1>:".to_owned(),
        }
    }

    fn step(&mut self, input: StepInput, ctx: &ToolCtx<'_>) -> StepOutcome {
        match (self.state, input) {
            // ---- 定義名 ----
            (InsertState::Naming, StepInput::Word(w)) => self.pick_definition(ctx, &w),
            (InsertState::Naming, StepInput::Number(n)) => {
                self.pick_definition(ctx, &super::edit::format_number_name(n))
            }
            (InsertState::Naming, StepInput::Enter) => {
                if ctx.doc.definitions().is_empty() {
                    return StepOutcome::Reject(
                        "コンポーネントがまだありません（COMPONENT で作成してください）".to_owned(),
                    );
                }
                StepOutcome::Reject("コンポーネント名を入力してください".to_owned())
            }

            // ---- 位置 ----
            (InsertState::Position { def }, StepInput::Point(p)) => {
                self.state = InsertState::Rotation { def, origin: p };
                StepOutcome::Continue
            }

            // ---- 回転角（度で入力する。座標入力の `@100<45` と同じ約束）----
            (InsertState::Rotation { def, origin }, StepInput::Enter) => {
                self.state = InsertState::Scale {
                    def,
                    origin,
                    rotation: 0.0,
                };
                StepOutcome::Continue
            }
            (InsertState::Rotation { def, origin }, StepInput::Number(deg)) => {
                if !deg.is_finite() {
                    return StepOutcome::Reject("回転角が数値として不正です".to_owned());
                }
                self.state = InsertState::Scale {
                    def,
                    origin,
                    rotation: deg.to_radians(),
                };
                StepOutcome::Continue
            }
            // 点で回転を示す。基点からの方向を角度にする。
            (InsertState::Rotation { def, origin }, StepInput::Point(p)) => {
                let dir = p - origin;
                let rotation = if dir.is_zero() { 0.0 } else { dir.angle() };
                self.state = InsertState::Scale {
                    def,
                    origin,
                    rotation,
                };
                StepOutcome::Continue
            }

            // ---- 倍率 ----
            (
                InsertState::Scale {
                    def,
                    origin,
                    rotation,
                },
                StepInput::Enter,
            ) => Self::commit(ctx, def, origin, rotation, 1.0),
            (
                InsertState::Scale {
                    def,
                    origin,
                    rotation,
                },
                StepInput::Number(s),
            ) => Self::commit(ctx, def, origin, rotation, s),

            (_, StepInput::SelectionReady) => StepOutcome::Finish,
            (InsertState::Naming, _) => {
                StepOutcome::Reject("コンポーネント名を入力してください".to_owned())
            }
            (InsertState::Position { .. }, _) => {
                StepOutcome::Reject("挿入位置を指定してください".to_owned())
            }
            (_, _) => StepOutcome::Reject("数値を指定してください".to_owned()),
        }
    }
}

impl InsertTool {
    fn pick_definition(&mut self, ctx: &ToolCtx<'_>, name: &str) -> StepOutcome {
        match ctx.doc.definitions().by_name(name) {
            Some(def) => {
                self.state = InsertState::Position { def };
                StepOutcome::Continue
            }
            None => {
                let known: Vec<&str> = ctx
                    .doc
                    .definitions()
                    .iter()
                    .map(|(_, d)| d.name.as_str())
                    .collect();
                if known.is_empty() {
                    StepOutcome::Reject(
                        "コンポーネントがまだありません（COMPONENT で作成してください）".to_owned(),
                    )
                } else {
                    StepOutcome::Reject(format!(
                        "「{name}」がありません。あるのは: {}",
                        known.join(" / ")
                    ))
                }
            }
        }
    }

    fn commit(
        ctx: &ToolCtx<'_>,
        def: cad_core::DefinitionId,
        origin: Point2,
        rotation: f64,
        scale: f64,
    ) -> StepOutcome {
        // 倍率 0 は図形を点に潰し、負値は鏡像になる（反転は MIRROR を使う）。
        // `SCALE` コマンドと同じ約束。
        match Placement::new(origin, rotation, scale, false) {
            Ok(placement) => StepOutcome::Apply(Box::new(InsertInstance::new(
                "INSERT", def, placement, ctx.layer,
            ))),
            Err(_) => StepOutcome::Reject(
                "倍率は 0 より大きい値を指定してください（反転は MIRROR）".to_owned(),
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// EDITCOMP — 定義の中身を選択で差し替える
// ---------------------------------------------------------------------------

/// 選択した要素で、既存のコンポーネント定義の中身を差し替える。
///
/// インプレース編集（定義に入って編集する）は段階 3。ここでは
/// **「新しい中身を選んで差し替える」**という最小の形を先に用意する。
/// これだけで「定義を編集すると全インスタンスが追従する」を UI から試せる。
#[derive(Debug, Default)]
pub struct RedefineTool {
    state: RedefineState,
}

#[derive(Debug, Default, Clone, Copy, PartialEq)]
enum RedefineState {
    #[default]
    Selecting,
    Origin,
    Naming {
        origin: Point2,
    },
}

impl Tool for RedefineTool {
    fn name(&self) -> &'static str {
        "REDEFINE"
    }

    fn prompt(&self) -> String {
        match self.state {
            RedefineState::Selecting => "新しい中身を選択 (Enter で確定):".to_owned(),
            RedefineState::Origin => "基点を指定:".to_owned(),
            RedefineState::Naming { .. } => "差し替えるコンポーネント名:".to_owned(),
        }
    }

    fn wants_selection(&self) -> bool {
        true
    }

    fn step(&mut self, input: StepInput, ctx: &ToolCtx<'_>) -> StepOutcome {
        match (self.state, input) {
            (RedefineState::Selecting, StepInput::SelectionReady) => {
                if ctx.selection.is_empty() {
                    return StepOutcome::Finish;
                }
                self.state = RedefineState::Origin;
                StepOutcome::Continue
            }
            (RedefineState::Origin, StepInput::Point(p)) => {
                self.state = RedefineState::Naming { origin: p };
                StepOutcome::Continue
            }
            (RedefineState::Naming { origin }, StepInput::Word(w)) => Self::commit(ctx, origin, &w),
            (RedefineState::Naming { origin }, StepInput::Number(n)) => {
                Self::commit(ctx, origin, &super::edit::format_number_name(n))
            }
            (_, StepInput::Enter | StepInput::SelectionReady) => StepOutcome::Finish,
            (RedefineState::Origin, _) => StepOutcome::Reject("基点を指定してください".to_owned()),
            (_, _) => StepOutcome::Reject("コンポーネント名を入力してください".to_owned()),
        }
    }
}

impl RedefineTool {
    fn commit(ctx: &ToolCtx<'_>, origin: Point2, name: &str) -> StepOutcome {
        let Some(def) = ctx.doc.definitions().by_name(name) else {
            return StepOutcome::Reject(format!("「{name}」がありません"));
        };

        let targets = ctx.selection.to_vec();
        let mut contents: Vec<Entity> = Vec::with_capacity(targets.len());
        for id in &targets {
            let Some(entity) = ctx.doc.entities().get(*id) else {
                return StepOutcome::Reject("選択が古くなっています".to_owned());
            };
            // 差し替える定義自身のインスタンスを中身にすると循環する。
            // コマンド側でも弾かれるが、先に分かる案内を出す。
            if let Geometry::Instance(i) = &entity.geom {
                if i.definition == def {
                    return StepOutcome::Reject(
                        "差し替え先のコンポーネント自身は中身にできません".to_owned(),
                    );
                }
            }
            contents.push(entity.clone());
        }

        // 選んだ要素は定義へ移すので図面からは消す。
        StepOutcome::Apply(Box::new(MacroCommand::new(
            "REDEFINE",
            vec![
                Box::new(SetDefinitionContents::new(
                    "REDEFINE", def, origin, contents,
                )),
                Box::new(DeleteEntities::new("REDEFINE", targets)),
            ],
        )))
    }
}

// ---------------------------------------------------------------------------
// EDITCOMP — 定義をその場で編集する
// ---------------------------------------------------------------------------

/// インスタンスをクリックして、その定義の編集に入る。
///
/// 編集そのものは既存のツールで行い、終わったら `ENDCOMP` で書き戻す。
/// **モーダルにしない**（周りの図形が見えたまま、いつものコマンドが使える）。
#[derive(Debug, Default)]
pub struct EditComponentTool;

impl Tool for EditComponentTool {
    fn name(&self) -> &'static str {
        "EDITCOMP"
    }

    fn prompt(&self) -> String {
        "編集するコンポーネントのインスタンスをクリック:".to_owned()
    }

    fn wants_entity(&self) -> bool {
        true
    }

    fn step(&mut self, input: StepInput, ctx: &ToolCtx<'_>) -> StepOutcome {
        match input {
            StepInput::Entity { id, .. } => {
                let Some(entity) = ctx.doc.entities().get(id) else {
                    return StepOutcome::Reject("選択が古くなっています".to_owned());
                };
                let Geometry::Instance(inst) = &entity.geom else {
                    return StepOutcome::Reject(
                        "コンポーネントのインスタンスをクリックしてください".to_owned(),
                    );
                };
                if ctx.doc.definitions().get(inst.definition).is_none() {
                    return StepOutcome::Reject("コンポーネントが見つかりません".to_owned());
                }
                StepOutcome::ApplyAndEdit {
                    command: Box::new(EnterDefinitionEdit::new("EDITCOMP", id)),
                    definition: inst.definition,
                    placement: inst.placement,
                }
            }
            StepInput::Point(_) => {
                StepOutcome::Reject("インスタンスの上をクリックしてください".to_owned())
            }
            StepInput::Enter | StepInput::SelectionReady => StepOutcome::Finish,
            _ => StepOutcome::Reject("インスタンスをクリックしてください".to_owned()),
        }
    }
}
