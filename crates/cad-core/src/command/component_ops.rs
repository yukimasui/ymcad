//! コンポーネントの定義・配置・削除。
//!
//! # 妥当性はここで保証する
//!
//! [`crate::component`] のモジュールドキュメントの約束どおり、
//! **入れ子の循環検出はこの層で行う**。`Document` に入っている定義は常に妥当なので、
//! [`component::resolve`] は失敗しない。
//!
//! # ID の使い回し
//!
//! [`DefineComponent`] は Undo したあとも確保した [`DefinitionId`] を捨てず、
//! Redo で**同じ ID を再利用**する。ID が変わると、Undo スタックに残る他のコマンド
//! （その定義を指す [`InsertInstance`] など）の参照が壊れる。
//! [`CreateGroup`](super::CreateGroup) と同じ理由（ADR-0004 / ADR-0022）。

use super::{Command, EditCtx};
use crate::component::{self, Definition, DefinitionId, Instance, Placement};
use crate::entity::{Entity, EntityId, Geometry};
use crate::error::{CadError, Result};
use crate::geom::Point2;

/// コンポーネント定義を作る。
///
/// **図面のエンティティには触らない。** 選択を差し替える動きは
/// [`MacroCommand`](super::MacroCommand) で組み立てる。
#[derive(Debug)]
pub struct DefineComponent {
    name: &'static str,
    def_name: String,
    origin: Point2,
    contents: Vec<Entity>,
    /// 確保した ID。**Undo でも捨てない**（Redo で使い回すため）。
    allocated: Option<DefinitionId>,
    /// いま図面に存在しているか。`allocated` が `Some` でもこれが `false` なら
    /// Undo 済み。
    active: bool,
}

impl DefineComponent {
    /// 定義名・基点・中身を指定して作る。
    #[must_use]
    pub fn new(
        name: &'static str,
        def_name: impl Into<String>,
        origin: Point2,
        contents: Vec<Entity>,
    ) -> Self {
        Self {
            name,
            def_name: def_name.into(),
            origin,
            contents,
            allocated: None,
            active: false,
        }
    }

    /// 作られた定義の ID。適用前は `None`。
    ///
    /// Undo したあとも同じ ID を返す（Redo で同じ ID を使い回すため）。
    #[must_use]
    pub fn created(&self) -> Option<DefinitionId> {
        self.active.then_some(self.allocated).flatten()
    }
}

impl Command for DefineComponent {
    fn execute(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        if self.def_name.trim().is_empty() {
            return Err(CadError::DegenerateGeometry("コンポーネント名が空です"));
        }

        let def = Definition::new(self.def_name.clone(), self.origin, self.contents.clone());

        match self.allocated {
            // Redo。取り除いた ID をそのまま戻す。
            Some(id) => ctx.restore_definition(id, def)?,
            None => {
                if ctx.definitions().by_name(&self.def_name).is_some() {
                    return Err(CadError::DegenerateGeometry(
                        "同じ名前のコンポーネントが既にあります",
                    ));
                }
                self.allocated = Some(ctx.add_definition(def));
            }
        }
        self.active = true;
        Ok(())
    }

    fn undo(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        let Some(id) = self.allocated else {
            return Ok(());
        };
        ctx.remove_definition(id)?;
        // `allocated` は**消さない**。Redo で同じ ID を使う。
        self.active = false;
        Ok(())
    }

    fn name(&self) -> &'static str {
        self.name
    }
}

/// 定義の中身を差し替える。
///
/// **これが「定義を編集すると全インスタンスが追従する」の入口。**
/// インスタンスは定義を ID で参照しているだけなので、中身を差し替えれば
/// 次の解決で全インスタンスに反映される。
#[derive(Debug)]
pub struct SetDefinitionContents {
    name: &'static str,
    target: DefinitionId,
    origin: Point2,
    contents: Vec<Entity>,
    /// Undo 用に控えた差し替え前の基点と中身。
    previous: Option<(Point2, Vec<Entity>)>,
}

impl SetDefinitionContents {
    /// 対象と新しい基点・中身を指定して作る。
    #[must_use]
    pub fn new(
        name: &'static str,
        target: DefinitionId,
        origin: Point2,
        contents: Vec<Entity>,
    ) -> Self {
        Self {
            name,
            target,
            origin,
            contents,
            previous: None,
        }
    }
}

impl Command for SetDefinitionContents {
    fn execute(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        // **循環検出。** 新しい中身が自分自身へ到達できてはいけない。
        // 深さ上限（`MAX_NESTING_DEPTH`）は最後の砦であって、これの代わりにはならない
        // （深さで打ち切ると図形が黙って消える）。
        for inner in instance_definitions(&self.contents) {
            if component::would_create_cycle(self.target, inner, ctx.definitions()) {
                return Err(CadError::DefinitionCycle);
            }
        }

        let previous =
            ctx.replace_definition_contents(self.target, self.origin, self.contents.clone())?;
        self.previous = Some(previous);
        Ok(())
    }

    fn undo(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        let Some((origin, contents)) = self.previous.take() else {
            return Ok(());
        };
        ctx.replace_definition_contents(self.target, origin, contents)?;
        Ok(())
    }

    fn name(&self) -> &'static str {
        self.name
    }
}

/// コンポーネントを配置する。
#[derive(Debug)]
pub struct InsertInstance {
    name: &'static str,
    definition: DefinitionId,
    placement: Placement,
    layer: crate::layer::LayerId,
    /// 適用で作った要素。Undo で消す。
    created: Option<EntityId>,
}

impl InsertInstance {
    /// 定義・配置・レイヤを指定して作る。
    #[must_use]
    pub fn new(
        name: &'static str,
        definition: DefinitionId,
        placement: Placement,
        layer: crate::layer::LayerId,
    ) -> Self {
        Self {
            name,
            definition,
            placement,
            layer,
            created: None,
        }
    }

    /// 作られた要素の ID。適用前は `None`。
    #[must_use]
    pub fn created(&self) -> Option<EntityId> {
        self.created
    }
}

impl Command for InsertInstance {
    fn execute(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        if ctx.definitions().get(self.definition).is_none() {
            return Err(CadError::DefinitionNotFound);
        }
        let geom = Geometry::Instance(Instance::new(self.definition, self.placement));
        let mut entity = Entity::new(geom, self.layer);
        entity.group = None;
        self.created = Some(ctx.add_entity(entity));
        Ok(())
    }

    fn undo(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        if let Some(id) = self.created.take() {
            ctx.remove_entity(id)?;
        }
        Ok(())
    }

    fn name(&self) -> &'static str {
        self.name
    }
}

/// コンポーネント定義を削除する。
///
/// **使われている定義は削除できない。** 参照だけ残った状態を作ると、
/// 解決が空を返して図形が黙って消える。
#[derive(Debug)]
pub struct DeleteDefinition {
    name: &'static str,
    target: DefinitionId,
    /// Undo 用に控えた定義。
    removed: Option<Definition>,
}

impl DeleteDefinition {
    /// 対象を指定して作る。
    #[must_use]
    pub fn new(name: &'static str, target: DefinitionId) -> Self {
        Self {
            name,
            target,
            removed: None,
        }
    }
}

impl Command for DeleteDefinition {
    fn execute(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        // 図面直下から参照されていないか。
        let used_in_drawing = ctx
            .entities()
            .iter()
            .any(|(_, e)| references(&e.geom, self.target));
        // 他の定義の中から参照されていないか。
        let used_in_definitions = ctx
            .definitions()
            .iter()
            .filter(|(id, _)| *id != self.target)
            .any(|(_, d)| d.entities.iter().any(|e| references(&e.geom, self.target)));

        if used_in_drawing || used_in_definitions {
            return Err(CadError::NotEditable(
                "使用中のコンポーネントは削除できません",
            ));
        }

        self.removed = Some(ctx.remove_definition(self.target)?);
        Ok(())
    }

    fn undo(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        let Some(def) = self.removed.take() else {
            return Ok(());
        };
        ctx.restore_definition(self.target, def)?;
        Ok(())
    }

    fn name(&self) -> &'static str {
        self.name
    }
}

/// 図形が特定の定義を参照しているか。
fn references(geom: &Geometry, id: DefinitionId) -> bool {
    matches!(geom, Geometry::Instance(i) if i.definition == id)
}

/// エンティティ列の中のインスタンスが参照している定義 ID。
fn instance_definitions(entities: &[Entity]) -> Vec<DefinitionId> {
    entities
        .iter()
        .filter_map(|e| match &e.geom {
            Geometry::Instance(i) => Some(i.definition),
            _ => None,
        })
        .collect()
}
