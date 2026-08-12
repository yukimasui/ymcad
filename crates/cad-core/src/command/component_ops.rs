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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::DefinitionTable;
    use crate::entity::EntityStore;
    use crate::geom::tolerance::eq_len;
    use crate::geom::{Circle, Line};
    use crate::group::GroupTable;
    use crate::layer::{LayerId, LayerTable};

    fn new_parts() -> (EntityStore, LayerTable, GroupTable, DefinitionTable) {
        (
            EntityStore::new(),
            LayerTable::new(),
            GroupTable::new(),
            DefinitionTable::new(),
        )
    }

    fn p(x: f64, y: f64) -> Point2 {
        Point2::new(x, y)
    }

    fn line_entity(x: f64) -> Entity {
        Entity::new(
            Geometry::Line(Line::new(p(x, 0.0), p(x + 1.0, 0.0))),
            LayerId::ZERO,
        )
    }

    fn circle_entity() -> Entity {
        Entity::new(
            Geometry::Circle(Circle::new(p(0.0, 0.0), 1.0)),
            LayerId::ZERO,
        )
    }

    // ---- DefineComponent --------------------------------------------------

    #[test]
    fn define_execute_creates_a_definition_and_leaves_entities_alone() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let existing = e.insert(line_entity(0.0));

        let mut cmd =
            DefineComponent::new("COMPONENT", "部品", p(1.0, 2.0), vec![line_entity(5.0)]);
        {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
            cmd.execute(&mut ctx).expect("定義を作れるはず");
        }

        let id = cmd.created().expect("ID が返るはず");
        let def = d.get(id).expect("引けるはず");
        assert_eq!(def.name, "部品");
        assert!(eq_len(def.origin.x, 1.0), "基点が保たれる");
        assert_eq!(def.entities.len(), 1);
        assert_eq!(e.len(), 1, "図面のエンティティには触らない");
        assert!(e.contains(existing));
    }

    #[test]
    fn define_undo_removes_the_definition() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let mut cmd =
            DefineComponent::new("COMPONENT", "部品", Point2::ORIGIN, vec![line_entity(0.0)]);
        {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
            cmd.execute(&mut ctx).expect("作れる");
            cmd.undo(&mut ctx).expect("戻せる");
        }
        assert_eq!(d.len(), 0);
        assert!(d.by_name("部品").is_none());
        assert!(cmd.created().is_none(), "Undo 後は created が None");
    }

    /// **Redo で同じ `DefinitionId` を使い回すこと。**
    ///
    /// ID が変わると、Undo スタックに残る `InsertInstance` の参照が壊れる
    /// （ADR-0004 / ADR-0022 と同じ理由）。
    #[test]
    fn define_redo_reuses_the_same_definition_id() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let mut cmd =
            DefineComponent::new("COMPONENT", "部品", Point2::ORIGIN, vec![line_entity(0.0)]);

        let first = {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
            cmd.execute(&mut ctx).expect("作れる");
            cmd.created().expect("ID")
        };
        {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
            cmd.undo(&mut ctx).expect("戻せる");
            cmd.execute(&mut ctx).expect("やり直せる");
        }
        assert_eq!(cmd.created(), Some(first), "Redo で同じ ID になること");
    }

    #[test]
    fn define_rejects_a_duplicate_name() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let mut first = DefineComponent::new("COMPONENT", "部品", Point2::ORIGIN, Vec::new());
        let mut second = DefineComponent::new("COMPONENT", "部品", Point2::ORIGIN, Vec::new());

        let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
        first.execute(&mut ctx).expect("1 つ目は作れる");
        assert!(second.execute(&mut ctx).is_err(), "同名は拒否される");
    }

    #[test]
    fn define_rejects_an_empty_name() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let mut cmd = DefineComponent::new("COMPONENT", "   ", Point2::ORIGIN, Vec::new());
        let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
        assert!(cmd.execute(&mut ctx).is_err());
        assert_eq!(d.len(), 0, "図面は変わらない");
    }

    #[test]
    fn define_large_coordinates() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let far = p(1.0e12, -1.0e12);
        let mut cmd = DefineComponent::new("COMPONENT", "遠方", far, vec![line_entity(1.0e12)]);
        {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
            cmd.execute(&mut ctx).expect("作れる");
        }
        let def = d.get(cmd.created().expect("ID")).expect("引ける");
        assert!(eq_len(def.origin.x, 1.0e12));
    }

    // ---- InsertInstance ---------------------------------------------------

    /// 定義を作って配置する下準備。
    fn define_and_insert(
        e: &mut EntityStore,
        l: &mut LayerTable,
        g: &mut GroupTable,
        d: &mut DefinitionTable,
    ) -> (DefinitionId, EntityId) {
        let mut def =
            DefineComponent::new("COMPONENT", "部品", Point2::ORIGIN, vec![circle_entity()]);
        let mut ins;
        let def_id;
        {
            let mut ctx = EditCtx::new(e, l, g, d);
            def.execute(&mut ctx).expect("定義を作れる");
            def_id = def.created().expect("ID");
            ins = InsertInstance::new("INSERT", def_id, Placement::at(p(10.0, 0.0)), LayerId::ZERO);
            ins.execute(&mut ctx).expect("配置できる");
        }
        (def_id, ins.created().expect("ID"))
    }

    #[test]
    fn insert_execute_adds_an_instance_entity() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let (def_id, id) = define_and_insert(&mut e, &mut l, &mut g, &mut d);

        let entity = e.get(id).expect("あるはず");
        let Geometry::Instance(i) = &entity.geom else {
            panic!("インスタンスのはず: {:?}", entity.geom);
        };
        assert_eq!(i.definition, def_id);
        assert!(eq_len(i.placement.origin.x, 10.0));
        assert!(i.overrides.is_empty(), "上書きは無い");
    }

    #[test]
    fn insert_undo_removes_the_instance() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let mut def =
            DefineComponent::new("COMPONENT", "部品", Point2::ORIGIN, vec![circle_entity()]);
        let mut ctx_scope = |e: &mut EntityStore,
                             l: &mut LayerTable,
                             g: &mut GroupTable,
                             d: &mut DefinitionTable| {
            let mut ctx = EditCtx::new(e, l, g, d);
            def.execute(&mut ctx).expect("定義");
            let def_id = def.created().expect("ID");
            let mut ins =
                InsertInstance::new("INSERT", def_id, Placement::at(p(1.0, 1.0)), LayerId::ZERO);
            ins.execute(&mut ctx).expect("配置");
            assert_eq!(ctx.entities().len(), 1);
            ins.undo(&mut ctx).expect("戻せる");
        };
        ctx_scope(&mut e, &mut l, &mut g, &mut d);
        assert_eq!(e.len(), 0, "インスタンスが消える");
        assert_eq!(d.len(), 1, "定義は残る");
    }

    #[test]
    fn insert_missing_definition_fails_and_leaves_document_unchanged() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        // 別の表で作った ID を使う（この表には存在しない）。
        let mut other = DefinitionTable::new();
        let dangling = other.insert(Definition::new("よそ", Point2::ORIGIN, Vec::new()));

        let mut cmd = InsertInstance::new(
            "INSERT",
            dangling,
            Placement::at(Point2::ORIGIN),
            LayerId::ZERO,
        );
        let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
        assert_eq!(cmd.execute(&mut ctx), Err(CadError::DefinitionNotFound));
        assert_eq!(ctx.entities().len(), 0, "図面は変わらない");
    }

    #[test]
    fn insert_redo_after_undo_works() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let (_, _) = define_and_insert(&mut e, &mut l, &mut g, &mut d);
        let def_id = d.by_name("部品").expect("あるはず");

        let mut cmd =
            InsertInstance::new("INSERT", def_id, Placement::at(p(5.0, 5.0)), LayerId::ZERO);
        {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
            cmd.execute(&mut ctx).expect("配置");
            cmd.undo(&mut ctx).expect("戻す");
            cmd.execute(&mut ctx).expect("やり直す");
        }
        assert_eq!(e.len(), 2, "元の 1 つ + やり直した 1 つ");
    }

    // ---- SetDefinitionContents --------------------------------------------

    #[test]
    fn set_contents_replaces_and_undo_restores() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let mut def =
            DefineComponent::new("COMPONENT", "部品", Point2::ORIGIN, vec![circle_entity()]);
        let def_id;
        {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
            def.execute(&mut ctx).expect("定義");
            def_id = def.created().expect("ID");
        }

        let mut cmd = SetDefinitionContents::new(
            "EDITCOMP",
            def_id,
            p(1.0, 1.0),
            vec![line_entity(0.0), line_entity(2.0)],
        );
        {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
            cmd.execute(&mut ctx).expect("差し替えられる");
        }
        assert_eq!(d.get(def_id).expect("引ける").entities.len(), 2);
        assert!(eq_len(d.get(def_id).expect("引ける").origin.x, 1.0));

        {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
            cmd.undo(&mut ctx).expect("戻せる");
        }
        let def = d.get(def_id).expect("引ける");
        assert_eq!(def.entities.len(), 1, "元の中身に戻る");
        assert!(eq_len(def.origin.x, 0.0), "元の基点に戻る");
    }

    /// **循環を拒否すること。** 深さ上限は最後の砦であって、これの代わりにはならない。
    #[test]
    fn set_contents_rejects_a_self_reference() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let mut def = DefineComponent::new("COMPONENT", "部品", Point2::ORIGIN, Vec::new());
        let def_id;
        {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
            def.execute(&mut ctx).expect("定義");
            def_id = def.created().expect("ID");
        }

        // 自分自身を含めようとする。
        let itself = Entity::new(
            Geometry::Instance(Instance::new(def_id, Placement::at(Point2::ORIGIN))),
            LayerId::ZERO,
        );
        let mut cmd = SetDefinitionContents::new("EDITCOMP", def_id, Point2::ORIGIN, vec![itself]);
        {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
            assert_eq!(cmd.execute(&mut ctx), Err(CadError::DefinitionCycle));
        }
        assert_eq!(
            d.get(def_id).expect("引ける").entities.len(),
            0,
            "拒否されたら中身は変わらない"
        );
    }

    #[test]
    fn set_contents_rejects_an_indirect_cycle() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let (a, b);
        {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
            let mut da = DefineComponent::new("COMPONENT", "A", Point2::ORIGIN, Vec::new());
            da.execute(&mut ctx).expect("A");
            a = da.created().expect("ID");
            let mut db = DefineComponent::new("COMPONENT", "B", Point2::ORIGIN, Vec::new());
            db.execute(&mut ctx).expect("B");
            b = db.created().expect("ID");
        }

        let inst_of = |id: DefinitionId| {
            Entity::new(
                Geometry::Instance(Instance::new(id, Placement::at(Point2::ORIGIN))),
                LayerId::ZERO,
            )
        };

        // A の中に B を入れるのは安全。
        {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
            SetDefinitionContents::new("EDITCOMP", a, Point2::ORIGIN, vec![inst_of(b)])
                .execute(&mut ctx)
                .expect("A ← B は循環しない");
        }
        // B の中に A を入れると A → B → A で循環する。
        {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
            let mut cmd =
                SetDefinitionContents::new("EDITCOMP", b, Point2::ORIGIN, vec![inst_of(a)]);
            assert_eq!(cmd.execute(&mut ctx), Err(CadError::DefinitionCycle));
        }
    }

    #[test]
    fn set_contents_missing_definition_fails() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let mut other = DefinitionTable::new();
        let dangling = other.insert(Definition::new("よそ", Point2::ORIGIN, Vec::new()));

        let mut cmd = SetDefinitionContents::new("EDITCOMP", dangling, Point2::ORIGIN, Vec::new());
        let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
        assert_eq!(cmd.execute(&mut ctx), Err(CadError::DefinitionNotFound));
    }

    // ---- DeleteDefinition -------------------------------------------------

    #[test]
    fn delete_removes_an_unused_definition_and_undo_restores_it() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let mut def = DefineComponent::new("COMPONENT", "部品", p(1.0, 2.0), vec![circle_entity()]);
        let def_id;
        {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
            def.execute(&mut ctx).expect("定義");
            def_id = def.created().expect("ID");
        }

        let mut cmd = DeleteDefinition::new("PURGE", def_id);
        {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
            cmd.execute(&mut ctx).expect("消せる");
        }
        assert_eq!(d.len(), 0);

        {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
            cmd.undo(&mut ctx).expect("戻せる");
        }
        let restored = d.get(def_id).expect("同じ ID で戻る");
        assert_eq!(restored.name, "部品");
        assert!(eq_len(restored.origin.x, 1.0), "基点も戻る");
        assert_eq!(restored.entities.len(), 1);
    }

    /// **使用中の定義は削除できないこと。**
    ///
    /// 参照だけ残った状態を作ると、解決が空を返して図形が黙って消える。
    #[test]
    fn delete_refuses_a_definition_used_in_the_drawing() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let (def_id, _) = define_and_insert(&mut e, &mut l, &mut g, &mut d);

        let mut cmd = DeleteDefinition::new("PURGE", def_id);
        let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
        assert!(
            matches!(cmd.execute(&mut ctx), Err(CadError::NotEditable(_))),
            "図面で使われている定義は消せない"
        );
        assert_eq!(ctx.definitions().len(), 1, "定義は残る");
    }

    #[test]
    fn delete_refuses_a_definition_used_inside_another_definition() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let (a, b);
        {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
            let mut da =
                DefineComponent::new("COMPONENT", "内", Point2::ORIGIN, vec![circle_entity()]);
            da.execute(&mut ctx).expect("内");
            a = da.created().expect("ID");
            let inner = Entity::new(
                Geometry::Instance(Instance::new(a, Placement::at(Point2::ORIGIN))),
                LayerId::ZERO,
            );
            let mut db = DefineComponent::new("COMPONENT", "外", Point2::ORIGIN, vec![inner]);
            db.execute(&mut ctx).expect("外");
            b = db.created().expect("ID");
        }
        let _ = b;

        let mut cmd = DeleteDefinition::new("PURGE", a);
        let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
        assert!(
            matches!(cmd.execute(&mut ctx), Err(CadError::NotEditable(_))),
            "他の定義から使われている定義は消せない"
        );
    }

    #[test]
    fn delete_missing_definition_fails_and_leaves_document_unchanged() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let mut other = DefinitionTable::new();
        let dangling = other.insert(Definition::new("よそ", Point2::ORIGIN, Vec::new()));

        let mut cmd = DeleteDefinition::new("PURGE", dangling);
        let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
        assert_eq!(cmd.execute(&mut ctx), Err(CadError::DefinitionNotFound));
        assert_eq!(ctx.definitions().len(), 0);
    }
}
