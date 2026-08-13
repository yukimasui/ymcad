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
use std::collections::BTreeMap;

use crate::component::{self, Binding, Definition, DefinitionId, Instance, ParamDecl, Placement};
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
    /// 座標への束縛。**中身と必ず一緒に持ち替える**（添字がずれるため）。
    bindings: Vec<Binding>,
    /// Undo 用に控えた差し替え前の基点・中身・束縛。
    previous: Option<(Point2, Vec<Entity>, Vec<Binding>)>,
}

impl SetDefinitionContents {
    /// 対象と新しい基点・中身を指定して作る。**束縛は空**。
    ///
    /// 中身を丸ごと差し替えると、既存の束縛が指す先は別の図形になる。
    /// 保つ道が無いので捨てる（`component::binding` のモジュールドキュメント）。
    #[must_use]
    pub fn new(
        name: &'static str,
        target: DefinitionId,
        origin: Point2,
        contents: Vec<Entity>,
    ) -> Self {
        Self::with_bindings(name, target, origin, contents, Vec::new())
    }

    /// 束縛も指定して作る。
    #[must_use]
    pub fn with_bindings(
        name: &'static str,
        target: DefinitionId,
        origin: Point2,
        contents: Vec<Entity>,
        bindings: Vec<Binding>,
    ) -> Self {
        Self {
            name,
            target,
            origin,
            contents,
            bindings,
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

        // 束縛が指す先があること、スロットが図形に合うことを先に検査する。
        // 通してしまうと、解決のたびに黙って無視される束縛が図面に残る。
        for b in &self.bindings {
            if !b.fits(&self.contents) {
                return Err(CadError::DegenerateGeometry(
                    "束縛が指す図形が無いか、スロットが図形の種類に合いません",
                ));
            }
        }

        let previous = ctx.replace_definition_contents(
            self.target,
            self.origin,
            self.contents.clone(),
            self.bindings.clone(),
        )?;
        self.previous = Some(previous);
        Ok(())
    }

    fn undo(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        let Some((origin, contents, bindings)) = self.previous.take() else {
            return Ok(());
        };
        ctx.replace_definition_contents(self.target, origin, contents, bindings)?;
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

    // ---- パラメータと束縛のコマンド ---------------------------------------
    //
    // **検証がこの層に集まっていること**を固定する。
    // 通ったものだけが Document に入るので、解決は妥当性を気にしなくてよい。

    use crate::component::Slot;
    use crate::expr::{parse, ParamType, Value};

    /// 線分 1 本を持つ定義を作り、その ID を返す。
    fn define_line(
        e: &mut EntityStore,
        l: &mut LayerTable,
        g: &mut GroupTable,
        d: &mut DefinitionTable,
    ) -> DefinitionId {
        let mut cmd =
            DefineComponent::new("COMPONENT", "窓", Point2::ORIGIN, vec![line_entity(0.0)]);
        let mut ctx = EditCtx::new(e, l, g, d);
        cmd.execute(&mut ctx).expect("定義を作れる");
        cmd.created().expect("ID")
    }

    fn set_params(
        e: &mut EntityStore,
        l: &mut LayerTable,
        g: &mut GroupTable,
        d: &mut DefinitionTable,
        id: DefinitionId,
        params: Vec<ParamDecl>,
    ) -> Result<()> {
        let mut ctx = EditCtx::new(e, l, g, d);
        SetDefinitionParams::new("PARAM", id, params).execute(&mut ctx)
    }

    // ---- SetDefinitionParams ----------------------------------------------

    #[test]
    fn set_params_stores_the_declarations() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let id = define_line(&mut e, &mut l, &mut g, &mut d);
        set_params(
            &mut e,
            &mut l,
            &mut g,
            &mut d,
            id,
            vec![ParamDecl::number("幅", 900.0).with_range(300.0, 3000.0)],
        )
        .expect("宣言できる");

        let def = d.get(id).expect("引ける");
        assert_eq!(def.params.len(), 1);
        assert_eq!(def.params[0].name, "幅");
        assert_eq!(def.params[0].range, Some((300.0, 3000.0)));
    }

    #[test]
    fn set_params_undo_restores_the_previous_declarations() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let id = define_line(&mut e, &mut l, &mut g, &mut d);
        set_params(
            &mut e,
            &mut l,
            &mut g,
            &mut d,
            id,
            vec![ParamDecl::number("幅", 1.0)],
        )
        .expect("1 回目");

        let mut cmd = SetDefinitionParams::new("PARAM", id, vec![ParamDecl::number("高さ", 2.0)]);
        {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
            cmd.execute(&mut ctx).expect("2 回目");
        }
        assert_eq!(d.get(id).expect("引ける").params[0].name, "高さ");

        {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
            cmd.undo(&mut ctx).expect("戻せる");
        }
        assert_eq!(d.get(id).expect("引ける").params[0].name, "幅");
    }

    #[test]
    fn duplicate_parameter_names_are_rejected() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let id = define_line(&mut e, &mut l, &mut g, &mut d);
        let r = set_params(
            &mut e,
            &mut l,
            &mut g,
            &mut d,
            id,
            vec![ParamDecl::number("幅", 1.0), ParamDecl::number("幅", 2.0)],
        );
        assert!(r.is_err(), "同名のパラメータは拒否");
        assert!(
            d.get(id).expect("引ける").params.is_empty(),
            "図面は変わらない"
        );
    }

    #[test]
    fn an_empty_parameter_name_is_rejected() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let id = define_line(&mut e, &mut l, &mut g, &mut d);
        assert!(set_params(
            &mut e,
            &mut l,
            &mut g,
            &mut d,
            id,
            vec![ParamDecl::number("  ", 1.0)]
        )
        .is_err());
    }

    #[test]
    fn a_reversed_range_is_rejected() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let id = define_line(&mut e, &mut l, &mut g, &mut d);
        assert!(set_params(
            &mut e,
            &mut l,
            &mut g,
            &mut d,
            id,
            vec![ParamDecl::number("幅", 1.0).with_range(100.0, 10.0)]
        )
        .is_err());
    }

    /// **既定値が範囲の外なら拒否すること。**
    #[test]
    fn a_default_outside_its_range_is_rejected() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let id = define_line(&mut e, &mut l, &mut g, &mut d);
        assert!(set_params(
            &mut e,
            &mut l,
            &mut g,
            &mut d,
            id,
            vec![ParamDecl::number("幅", 5000.0).with_range(300.0, 3000.0)]
        )
        .is_err());
    }

    /// **既定値の型が宣言と合わなければ拒否すること。**
    #[test]
    fn a_default_of_the_wrong_type_is_rejected() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let id = define_line(&mut e, &mut l, &mut g, &mut d);
        let bad = ParamDecl {
            name: "幅".to_owned(),
            ty: ParamType::Number,
            default: parse("真").expect("解析"),
            range: None,
        };
        assert!(set_params(&mut e, &mut l, &mut g, &mut d, id, vec![bad]).is_err());
    }

    #[test]
    fn a_default_referencing_an_undeclared_parameter_is_rejected() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let id = define_line(&mut e, &mut l, &mut g, &mut d);
        let bad = ParamDecl {
            name: "幅".to_owned(),
            ty: ParamType::Number,
            default: parse("ない名前 * 2").expect("解析"),
            range: None,
        };
        assert!(set_params(&mut e, &mut l, &mut g, &mut d, id, vec![bad]).is_err());
    }

    /// **パラメータ間の循環を拒否すること。**
    #[test]
    fn cyclic_parameter_defaults_are_rejected() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let id = define_line(&mut e, &mut l, &mut g, &mut d);
        let a = ParamDecl {
            name: "a".to_owned(),
            ty: ParamType::Number,
            default: parse("b + 1").expect("解析"),
            range: None,
        };
        let b = ParamDecl {
            name: "b".to_owned(),
            ty: ParamType::Number,
            default: parse("a + 1").expect("解析"),
            range: None,
        };
        let r = set_params(&mut e, &mut l, &mut g, &mut d, id, vec![a, b]);
        assert_eq!(r, Err(CadError::DefinitionCycle));
    }

    /// 自分自身を参照するのも循環。
    #[test]
    fn a_self_referencing_default_is_rejected() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let id = define_line(&mut e, &mut l, &mut g, &mut d);
        let a = ParamDecl {
            name: "a".to_owned(),
            ty: ParamType::Number,
            default: parse("a + 1").expect("解析"),
            range: None,
        };
        assert_eq!(
            set_params(&mut e, &mut l, &mut g, &mut d, id, vec![a]),
            Err(CadError::DefinitionCycle)
        );
    }

    /// 循環していない連鎖は通ること。
    #[test]
    fn a_chain_of_defaults_is_allowed() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let id = define_line(&mut e, &mut l, &mut g, &mut d);
        let params = vec![
            ParamDecl::number("a", 3.0),
            ParamDecl {
                name: "b".to_owned(),
                ty: ParamType::Number,
                default: parse("a * 2").expect("解析"),
                range: None,
            },
            ParamDecl {
                name: "c".to_owned(),
                ty: ParamType::Number,
                default: parse("b + 1").expect("解析"),
                range: None,
            },
        ];
        set_params(&mut e, &mut l, &mut g, &mut d, id, params).expect("連鎖は通る");
        let env = d.get(id).expect("引ける").param_env(&BTreeMap::new());
        assert_eq!(env.get("c"), Some(&Value::Number(7.0)), "3*2+1");
    }

    /// **束縛が参照しているパラメータは消せないこと。**
    ///
    /// 消せると、その束縛が黙って効かなくなる。
    #[test]
    fn a_parameter_used_by_a_binding_cannot_be_removed() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let id = define_line(&mut e, &mut l, &mut g, &mut d);
        set_params(
            &mut e,
            &mut l,
            &mut g,
            &mut d,
            id,
            vec![ParamDecl::number("幅", 5.0)],
        )
        .expect("宣言");
        {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
            SetBinding::new(
                "BIND",
                id,
                Binding::new(0, Slot::LineBx, parse("幅").expect("解析")),
            )
            .execute(&mut ctx)
            .expect("束縛できる");
        }

        // 「幅」を消そうとする。
        let r = set_params(
            &mut e,
            &mut l,
            &mut g,
            &mut d,
            id,
            vec![ParamDecl::number("高さ", 1.0)],
        );
        assert!(r.is_err(), "束縛が参照しているので消せない");
        assert_eq!(
            d.get(id).expect("引ける").params[0].name,
            "幅",
            "変わらない"
        );
    }

    // ---- SetBinding -------------------------------------------------------

    #[test]
    fn set_binding_stores_and_replaces() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let id = define_line(&mut e, &mut l, &mut g, &mut d);
        set_params(
            &mut e,
            &mut l,
            &mut g,
            &mut d,
            id,
            vec![ParamDecl::number("幅", 5.0)],
        )
        .expect("宣言");

        for expr in ["幅", "幅 * 2"] {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
            SetBinding::new(
                "BIND",
                id,
                Binding::new(0, Slot::LineBx, parse(expr).expect("解析")),
            )
            .execute(&mut ctx)
            .expect("束縛できる");
        }

        let def = d.get(id).expect("引ける");
        assert_eq!(def.bindings.len(), 1, "同じ座標への束縛は置き換わる");
        assert_eq!(def.bindings[0].expr, parse("幅 * 2").expect("解析"));
    }

    #[test]
    fn set_binding_undo_restores_the_previous_bindings() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let id = define_line(&mut e, &mut l, &mut g, &mut d);
        set_params(
            &mut e,
            &mut l,
            &mut g,
            &mut d,
            id,
            vec![ParamDecl::number("幅", 5.0)],
        )
        .expect("宣言");

        let mut cmd = SetBinding::new(
            "BIND",
            id,
            Binding::new(0, Slot::LineBx, parse("幅").expect("解析")),
        );
        {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
            cmd.execute(&mut ctx).expect("束縛");
        }
        assert_eq!(d.get(id).expect("引ける").bindings.len(), 1);
        {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
            cmd.undo(&mut ctx).expect("戻す");
        }
        assert!(d.get(id).expect("引ける").bindings.is_empty());
    }

    #[test]
    fn a_binding_to_a_missing_entity_is_rejected() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let id = define_line(&mut e, &mut l, &mut g, &mut d);
        set_params(
            &mut e,
            &mut l,
            &mut g,
            &mut d,
            id,
            vec![ParamDecl::number("幅", 5.0)],
        )
        .expect("宣言");

        let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
        let r = SetBinding::new(
            "BIND",
            id,
            Binding::new(9, Slot::LineBx, parse("幅").expect("解析")),
        )
        .execute(&mut ctx);
        assert!(r.is_err(), "添字が範囲外");
    }

    /// **図形の種類に合わないスロットを拒否すること。**
    #[test]
    fn a_binding_with_a_mismatched_slot_is_rejected() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let id = define_line(&mut e, &mut l, &mut g, &mut d);
        set_params(
            &mut e,
            &mut l,
            &mut g,
            &mut d,
            id,
            vec![ParamDecl::number("幅", 5.0)],
        )
        .expect("宣言");

        let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
        // 線分に半径は無い。
        let r = SetBinding::new(
            "BIND",
            id,
            Binding::new(0, Slot::CircleR, parse("幅").expect("解析")),
        )
        .execute(&mut ctx);
        assert!(r.is_err());
    }

    #[test]
    fn a_binding_referencing_an_undeclared_parameter_is_rejected() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let id = define_line(&mut e, &mut l, &mut g, &mut d);

        let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
        let r = SetBinding::new(
            "BIND",
            id,
            Binding::new(0, Slot::LineBx, parse("ない名前").expect("解析")),
        )
        .execute(&mut ctx);
        assert!(r.is_err());
    }

    /// **座標に入るのは数値だけ。** 真偽を返す式は拒否すること。
    #[test]
    fn a_binding_that_is_not_a_number_is_rejected() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let id = define_line(&mut e, &mut l, &mut g, &mut d);
        set_params(
            &mut e,
            &mut l,
            &mut g,
            &mut d,
            id,
            vec![ParamDecl::number("幅", 5.0)],
        )
        .expect("宣言");

        let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
        let r = SetBinding::new(
            "BIND",
            id,
            Binding::new(0, Slot::LineBx, parse("幅 > 1").expect("解析")),
        )
        .execute(&mut ctx);
        assert!(r.is_err(), "真偽の式は座標に入れられない");
    }

    // ---- SetInstanceOverride ----------------------------------------------

    /// 定義とインスタンスを 1 つずつ作る。
    fn define_and_place(
        e: &mut EntityStore,
        l: &mut LayerTable,
        g: &mut GroupTable,
        d: &mut DefinitionTable,
    ) -> (DefinitionId, EntityId) {
        let id = define_line(e, l, g, d);
        {
            let mut ctx = EditCtx::new(e, l, g, d);
            SetDefinitionParams::new(
                "PARAM",
                id,
                vec![ParamDecl::number("幅", 900.0).with_range(300.0, 3000.0)],
            )
            .execute(&mut ctx)
            .expect("宣言");
            SetBinding::new(
                "BIND",
                id,
                Binding::new(0, Slot::LineBx, parse("幅").expect("解析")),
            )
            .execute(&mut ctx)
            .expect("束縛");
        }
        let mut ins =
            InsertInstance::new("INSERT", id, Placement::at(Point2::ORIGIN), LayerId::ZERO);
        {
            let mut ctx = EditCtx::new(e, l, g, d);
            ins.execute(&mut ctx).expect("配置");
        }
        (id, ins.created().expect("ID"))
    }

    fn overrides_of(e: &EntityStore, id: EntityId) -> BTreeMap<String, Value> {
        match &e.get(id).expect("あるはず").geom {
            Geometry::Instance(i) => i.overrides.clone(),
            other => panic!("インスタンスのはず: {other:?}"),
        }
    }

    #[test]
    fn set_override_stores_the_value() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let (_, inst) = define_and_place(&mut e, &mut l, &mut g, &mut d);

        {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
            SetInstanceOverride::set("PARAM", inst, "幅", Value::Number(1800.0))
                .execute(&mut ctx)
                .expect("上書きできる");
        }

        assert_eq!(
            overrides_of(&e, inst).get("幅"),
            Some(&Value::Number(1800.0))
        );
    }

    /// **リセットで上書きが消えること。**
    #[test]
    fn reset_removes_the_override() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let (_, inst) = define_and_place(&mut e, &mut l, &mut g, &mut d);
        {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
            SetInstanceOverride::set("PARAM", inst, "幅", Value::Number(1800.0))
                .execute(&mut ctx)
                .expect("上書き");
            SetInstanceOverride::reset("PARAM", inst, "幅")
                .execute(&mut ctx)
                .expect("リセット");
        }
        assert!(overrides_of(&e, inst).is_empty(), "既定値へ戻る");
    }

    #[test]
    fn set_override_undo_restores_the_previous_overrides() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let (_, inst) = define_and_place(&mut e, &mut l, &mut g, &mut d);

        let mut cmd = SetInstanceOverride::set("PARAM", inst, "幅", Value::Number(1800.0));
        {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
            cmd.execute(&mut ctx).expect("上書き");
        }
        assert!(!overrides_of(&e, inst).is_empty());
        {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
            cmd.undo(&mut ctx).expect("戻す");
        }
        assert!(overrides_of(&e, inst).is_empty());
    }

    /// **範囲外の値を拒否すること。** 通すと解決で黙って捨てられる。
    #[test]
    fn an_override_outside_the_range_is_rejected() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let (_, inst) = define_and_place(&mut e, &mut l, &mut g, &mut d);

        let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
        let r = SetInstanceOverride::set("PARAM", inst, "幅", Value::Number(99_999.0))
            .execute(&mut ctx);
        assert!(r.is_err(), "範囲外は拒否");
    }

    #[test]
    fn an_override_of_the_wrong_type_is_rejected() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let (_, inst) = define_and_place(&mut e, &mut l, &mut g, &mut d);

        let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
        let r = SetInstanceOverride::set("PARAM", inst, "幅", Value::Bool(true)).execute(&mut ctx);
        assert!(r.is_err());
    }

    #[test]
    fn an_override_of_an_unknown_parameter_is_rejected() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let (_, inst) = define_and_place(&mut e, &mut l, &mut g, &mut d);

        let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
        let r = SetInstanceOverride::set("PARAM", inst, "ない名前", Value::Number(1.0))
            .execute(&mut ctx);
        assert!(r.is_err());
    }

    #[test]
    fn overriding_a_plain_entity_is_rejected() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let plain = e.insert(line_entity(0.0));

        let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
        let r =
            SetInstanceOverride::set("PARAM", plain, "幅", Value::Number(1.0)).execute(&mut ctx);
        assert!(r.is_err(), "インスタンスでなければ拒否");
    }

    /// **上書きが図形へ反映されること**（コマンド経由での通し確認）。
    #[test]
    fn an_override_changes_the_resolved_geometry() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let (_, inst) = define_and_place(&mut e, &mut l, &mut g, &mut d);

        let width = |e: &EntityStore, d: &DefinitionTable| -> f64 {
            let Geometry::Instance(i) = &e.get(inst).expect("あるはず").geom else {
                panic!()
            };
            match &component::resolve(i, d)[0] {
                Geometry::Line(l) => l.b.x,
                other => panic!("線分のはず: {other:?}"),
            }
        };

        assert!(crate::geom::tolerance::eq_len(width(&e, &d), 900.0), "既定");
        {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
            SetInstanceOverride::set("PARAM", inst, "幅", Value::Number(1800.0))
                .execute(&mut ctx)
                .expect("上書き");
        }
        assert!(
            crate::geom::tolerance::eq_len(width(&e, &d), 1800.0),
            "反映される"
        );
    }

    // ---- インプレース編集 -------------------------------------------------

    /// 線分 2 本の定義に「幅」を宣言し、1 本目の終点 X に束縛して配置する。
    fn setup_editable(
        e: &mut EntityStore,
        l: &mut LayerTable,
        g: &mut GroupTable,
        d: &mut DefinitionTable,
    ) -> (DefinitionId, EntityId) {
        let contents = vec![line_entity(0.0), line_entity(5.0)];
        let mut def = DefineComponent::new("COMPONENT", "窓", Point2::ORIGIN, contents);
        let def_id;
        {
            let mut ctx = EditCtx::new(e, l, g, d);
            def.execute(&mut ctx).expect("定義");
            def_id = def.created().expect("ID");
            SetDefinitionParams::new("PARAM", def_id, vec![ParamDecl::number("幅", 3.0)])
                .execute(&mut ctx)
                .expect("宣言");
            SetBinding::new(
                "BIND",
                def_id,
                Binding::new(0, Slot::LineBx, parse("幅").expect("解析")),
            )
            .execute(&mut ctx)
            .expect("束縛");
        }
        let mut ins = InsertInstance::new(
            "INSERT",
            def_id,
            Placement::new(p(100.0, 50.0), 0.5, 2.0, false).expect("妥当"),
            LayerId::ZERO,
        );
        {
            let mut ctx = EditCtx::new(e, l, g, d);
            ins.execute(&mut ctx).expect("配置");
        }
        (def_id, ins.created().expect("ID"))
    }

    /// **編集に入ると中身が実エンティティになること。**
    #[test]
    fn entering_replaces_the_instance_with_its_contents() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let (_, inst) = setup_editable(&mut e, &mut l, &mut g, &mut d);
        assert_eq!(e.len(), 1, "はじめはインスタンス 1 つ");

        let mut cmd = EnterDefinitionEdit::new("EDITCOMP", inst);
        {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
            cmd.execute(&mut ctx).expect("編集に入れる");
        }

        assert_eq!(e.len(), 2, "線分 2 本になる");
        assert!(!e.contains(inst), "インスタンスは外れる");
        assert_eq!(cmd.created().len(), 2, "定義の中身と同じ数");
    }

    /// **画面上の見た目が変わらないこと。**
    ///
    /// 元のインスタンスを解決した図形と、編集に入って置かれた図形が一致すること。
    /// ここがずれると「編集に入った瞬間に図形が飛ぶ」。
    #[test]
    fn entering_keeps_the_shapes_where_they_were() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let (_, inst) = setup_editable(&mut e, &mut l, &mut g, &mut d);

        // 入る前の見え方（束縛は「幅 = 3」で評価される）。
        let Geometry::Instance(i) = &e.get(inst).expect("あるはず").geom else {
            panic!()
        };
        let before = component::resolve(i, &d);

        let mut cmd = EnterDefinitionEdit::new("EDITCOMP", inst);
        {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
            cmd.execute(&mut ctx).expect("編集に入れる");
        }
        let after: Vec<Geometry> = cmd
            .created()
            .iter()
            .map(|id| e.get(*id).expect("あるはず").geom.clone())
            .collect();

        assert_eq!(before.len(), after.len());
        // 束縛が効いている 1 本目だけは、定義そのままの形が置かれる
        // （評価後の形を置くと、書き戻したときに束縛が固定されてしまう）。
        let Geometry::Line(b1) = &before[1] else {
            panic!()
        };
        let Geometry::Line(a1) = &after[1] else {
            panic!()
        };
        assert!(
            eq_len(b1.a.x, a1.a.x) && eq_len(b1.b.x, a1.b.x),
            "束縛の無い要素は同じ位置"
        );
    }

    /// **編集を通しても束縛が保たれること。**
    ///
    /// 束縛は添字で座標を指すので、順序が変わると指す先がずれる。
    #[test]
    fn bindings_survive_a_round_trip_through_editing() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let (def_id, inst) = setup_editable(&mut e, &mut l, &mut g, &mut d);
        let placement = match &e.get(inst).expect("あるはず").geom {
            Geometry::Instance(i) => i.placement,
            other => panic!("インスタンスのはず: {other:?}"),
        };

        let mut enter = EnterDefinitionEdit::new("EDITCOMP", inst);
        {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
            enter.execute(&mut ctx).expect("入る");
        }
        let members: Vec<EntityId> = enter.created().to_vec();

        // そのまま出る（何も編集しない）。
        let origins = vec![Some(0), Some(1)];
        {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
            ExitDefinitionEdit::new("EDITCOMP", def_id, placement, members, origins)
                .execute(&mut ctx)
                .expect("出る");
        }

        let def = d.get(def_id).expect("引ける");
        assert_eq!(def.entities.len(), 2, "中身は 2 本のまま");
        assert_eq!(def.bindings.len(), 1, "**束縛が残る**");
        assert_eq!(def.bindings[0].entity, 0);
        assert_eq!(def.bindings[0].slot, Slot::LineBx);
        assert_eq!(e.len(), 1, "インスタンスが置き直される");
    }

    /// **順序が入れ替わっても束縛が付いて回ること。**
    #[test]
    fn bindings_follow_their_entity_when_the_order_changes() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let (def_id, inst) = setup_editable(&mut e, &mut l, &mut g, &mut d);
        let placement = match &e.get(inst).expect("あるはず").geom {
            Geometry::Instance(i) => i.placement,
            other => panic!("{other:?}"),
        };

        let mut enter = EnterDefinitionEdit::new("EDITCOMP", inst);
        {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
            enter.execute(&mut ctx).expect("入る");
        }
        // 並びを逆にして出る（元の 0 番が新しい 1 番になる）。
        let mut members: Vec<EntityId> = enter.created().to_vec();
        members.reverse();
        let origins = vec![Some(1), Some(0)];
        {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
            ExitDefinitionEdit::new("EDITCOMP", def_id, placement, members, origins)
                .execute(&mut ctx)
                .expect("出る");
        }

        let def = d.get(def_id).expect("引ける");
        assert_eq!(def.bindings.len(), 1);
        assert_eq!(def.bindings[0].entity, 1, "**添字が付け替わる**");
    }

    /// 消された要素の束縛は捨てられること（指す先が無いので残せない）。
    #[test]
    fn bindings_of_deleted_entities_are_dropped() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let (def_id, inst) = setup_editable(&mut e, &mut l, &mut g, &mut d);
        let placement = match &e.get(inst).expect("あるはず").geom {
            Geometry::Instance(i) => i.placement,
            other => panic!("{other:?}"),
        };

        let mut enter = EnterDefinitionEdit::new("EDITCOMP", inst);
        {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
            enter.execute(&mut ctx).expect("入る");
        }
        // 束縛が付いている 0 番を残さずに出る。
        let members = vec![enter.created()[1]];
        let origins = vec![Some(1)];
        {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
            ExitDefinitionEdit::new("EDITCOMP", def_id, placement, members, origins)
                .execute(&mut ctx)
                .expect("出る");
        }

        let def = d.get(def_id).expect("引ける");
        assert_eq!(def.entities.len(), 1);
        assert!(def.bindings.is_empty(), "指す先が無い束縛は残さない");
    }

    /// 新しく描いた要素も定義に入ること。
    #[test]
    fn newly_drawn_entities_join_the_definition() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let (def_id, inst) = setup_editable(&mut e, &mut l, &mut g, &mut d);
        let placement = match &e.get(inst).expect("あるはず").geom {
            Geometry::Instance(i) => i.placement,
            other => panic!("{other:?}"),
        };

        let mut enter = EnterDefinitionEdit::new("EDITCOMP", inst);
        {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
            enter.execute(&mut ctx).expect("入る");
        }
        // 編集中に 1 本描く。
        let extra = e.insert(line_entity(99.0));
        let mut members: Vec<EntityId> = enter.created().to_vec();
        members.push(extra);
        let origins = vec![Some(0), Some(1), None];
        {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
            ExitDefinitionEdit::new("EDITCOMP", def_id, placement, members, origins)
                .execute(&mut ctx)
                .expect("出る");
        }

        let def = d.get(def_id).expect("引ける");
        assert_eq!(def.entities.len(), 3, "描いた 1 本が加わる");
        assert_eq!(def.bindings.len(), 1, "既存の束縛は残る");
    }

    /// **編集して書き戻しても図形がずれないこと。**
    ///
    /// `place` と `unplace` が対になっていることの結合確認。
    #[test]
    fn editing_and_writing_back_does_not_move_the_shapes() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let (def_id, inst) = setup_editable(&mut e, &mut l, &mut g, &mut d);
        let before = d.get(def_id).expect("引ける").entities.clone();
        let placement = match &e.get(inst).expect("あるはず").geom {
            Geometry::Instance(i) => i.placement,
            other => panic!("{other:?}"),
        };

        let mut enter = EnterDefinitionEdit::new("EDITCOMP", inst);
        {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
            enter.execute(&mut ctx).expect("入る");
        }
        let members: Vec<EntityId> = enter.created().to_vec();
        {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
            ExitDefinitionEdit::new(
                "EDITCOMP",
                def_id,
                placement,
                members,
                vec![Some(0), Some(1)],
            )
            .execute(&mut ctx)
            .expect("出る");
        }

        let after = &d.get(def_id).expect("引ける").entities;
        assert_eq!(after.len(), before.len());
        for (i, (a, b)) in before.iter().zip(after.iter()).enumerate() {
            let (Geometry::Line(x), Geometry::Line(y)) = (&a.geom, &b.geom) else {
                panic!("線分のはず")
            };
            assert!(
                eq_len(x.a.x, y.a.x),
                "{i} 番目の始点 X: {} → {}",
                x.a.x,
                y.a.x
            );
            assert!(
                eq_len(x.b.x, y.b.x),
                "{i} 番目の終点 X: {} → {}",
                x.b.x,
                y.b.x
            );
            assert!(eq_len(x.a.y, y.a.y), "{i} 番目の始点 Y");
            assert!(eq_len(x.b.y, y.b.y), "{i} 番目の終点 Y");
        }
    }

    #[test]
    fn entering_undo_restores_the_instance() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let (_, inst) = setup_editable(&mut e, &mut l, &mut g, &mut d);
        let before = e.get(inst).cloned().expect("あるはず");

        let mut cmd = EnterDefinitionEdit::new("EDITCOMP", inst);
        {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
            cmd.execute(&mut ctx).expect("入る");
            cmd.undo(&mut ctx).expect("戻す");
        }
        assert_eq!(e.len(), 1);
        assert_eq!(e.get(inst), Some(&before), "同じ ID・内容で戻る");
    }

    #[test]
    fn exiting_undo_restores_the_edited_entities_and_the_definition() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let (def_id, inst) = setup_editable(&mut e, &mut l, &mut g, &mut d);
        let placement = match &e.get(inst).expect("あるはず").geom {
            Geometry::Instance(i) => i.placement,
            other => panic!("{other:?}"),
        };

        let mut enter = EnterDefinitionEdit::new("EDITCOMP", inst);
        {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
            enter.execute(&mut ctx).expect("入る");
        }
        let members: Vec<EntityId> = enter.created().to_vec();

        // 1 本消して出る。
        let mut exit = ExitDefinitionEdit::new(
            "EDITCOMP",
            def_id,
            placement,
            vec![members[0]],
            vec![Some(0)],
        );
        {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
            exit.execute(&mut ctx).expect("出る");
        }
        assert_eq!(d.get(def_id).expect("引ける").entities.len(), 1);

        {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
            exit.undo(&mut ctx).expect("戻す");
        }
        assert_eq!(
            d.get(def_id).expect("引ける").entities.len(),
            2,
            "定義が戻る"
        );
        assert_eq!(e.len(), 2, "編集中の要素が戻る");
        assert!(
            e.contains(members[0]) && e.contains(members[1]),
            "同じ ID で戻る"
        );
    }

    #[test]
    fn entering_a_plain_entity_is_rejected() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let plain = e.insert(line_entity(0.0));

        let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
        let r = EnterDefinitionEdit::new("EDITCOMP", plain).execute(&mut ctx);
        assert!(matches!(r, Err(CadError::NotEditable(_))), "{r:?}");
    }

    /// 対応表の長さが合わなければ拒否すること（内部の取り違えを早く出す）。
    #[test]
    fn a_mismatched_origins_table_is_rejected() {
        let (mut e, mut l, mut g, mut d) = new_parts();
        let (def_id, inst) = setup_editable(&mut e, &mut l, &mut g, &mut d);
        let placement = match &e.get(inst).expect("あるはず").geom {
            Geometry::Instance(i) => i.placement,
            other => panic!("{other:?}"),
        };

        let mut ctx = EditCtx::new(&mut e, &mut l, &mut g, &mut d);
        let r = ExitDefinitionEdit::new("EDITCOMP", def_id, placement, vec![inst], Vec::new())
            .execute(&mut ctx);
        assert!(matches!(r, Err(CadError::NotEditable(_))), "{r:?}");
    }
}

// ---------------------------------------------------------------------------
// パラメータと束縛
// ---------------------------------------------------------------------------

/// 定義のパラメータ宣言を差し替える。
///
/// **検証はここに集める**（[`crate::component`] の約束）。通ったものだけが
/// `Document` に入るので、解決は妥当性を気にしなくてよい。
#[derive(Debug)]
pub struct SetDefinitionParams {
    name: &'static str,
    target: DefinitionId,
    params: Vec<ParamDecl>,
    /// Undo 用に控えた差し替え前の宣言。
    previous: Option<Vec<ParamDecl>>,
}

impl SetDefinitionParams {
    /// 対象と新しい宣言を指定して作る。
    #[must_use]
    pub fn new(name: &'static str, target: DefinitionId, params: Vec<ParamDecl>) -> Self {
        Self {
            name,
            target,
            params,
            previous: None,
        }
    }
}

impl Command for SetDefinitionParams {
    fn execute(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        let def = ctx
            .definitions()
            .get(self.target)
            .ok_or(CadError::DefinitionNotFound)?;

        validate_params(&self.params)?;

        // **消すパラメータを参照している束縛が残らないこと。**
        // 残すと、その束縛が黙って効かなくなる。
        let declared: Vec<&str> = self.params.iter().map(|p| p.name.as_str()).collect();
        for b in &def.bindings {
            for used in b.expr.referenced_vars() {
                if !declared.contains(&used) {
                    return Err(CadError::NotEditable(
                        "束縛が参照しているパラメータは消せません",
                    ));
                }
            }
        }

        // **既存の束縛が新しい宣言でも数値になること。**
        //
        // 名前を残したまま型だけ変える（数値 → 真偽など）と、上の検査は通るのに
        // 束縛の式が数値でなくなり、その座標が黙って定義のままになる。
        // 消すのを禁じておきながら型変更を許すと、抜け道になる。
        let probe = Definition {
            name: def.name.clone(),
            origin: def.origin,
            params: self.params.clone(),
            entities: Vec::new(),
            bindings: Vec::new(),
        };
        let env = probe.param_env(&BTreeMap::new());
        for b in &def.bindings {
            match crate::expr::eval(&b.expr, &env) {
                Ok(crate::expr::Value::Number(_)) => {}
                _ => {
                    return Err(CadError::NotEditable(
                        "この宣言にすると既存の束縛が数値になりません",
                    ))
                }
            }
        }

        self.previous = Some(ctx.replace_definition_params(self.target, self.params.clone())?);
        Ok(())
    }

    fn undo(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        let Some(params) = self.previous.take() else {
            return Ok(());
        };
        ctx.replace_definition_params(self.target, params)?;
        Ok(())
    }

    fn name(&self) -> &'static str {
        self.name
    }
}

/// 宣言そのものの妥当性を見る。
///
/// # Errors
///
/// 名前が空・重複、範囲が逆、既定値が未宣言のパラメータを参照、
/// 既定値が宣言した型にならない、既定値どうしが循環する場合。
fn validate_params(params: &[ParamDecl]) -> Result<()> {
    let mut seen: Vec<&str> = Vec::with_capacity(params.len());
    for p in params {
        if p.name.trim().is_empty() {
            return Err(CadError::DegenerateGeometry("パラメータ名が空です"));
        }
        if seen.contains(&p.name.as_str()) {
            return Err(CadError::DegenerateGeometry("パラメータ名が重複しています"));
        }
        seen.push(&p.name);

        if let Some((lo, hi)) = p.range {
            if !lo.is_finite() || !hi.is_finite() || lo > hi {
                return Err(CadError::DegenerateGeometry("パラメータの範囲が不正です"));
            }
        }
    }

    // 既定値が未宣言の名前を参照していないこと。
    for p in params {
        for used in p.default.referenced_vars() {
            if !seen.contains(&used) {
                return Err(CadError::NotEditable(
                    "既定値が宣言されていないパラメータを参照しています",
                ));
            }
        }
    }

    // 循環していないこと。**深さ上限ではなくここで弾く。**
    if component::param_cycle(params).is_some() {
        return Err(CadError::DefinitionCycle);
    }

    // 既定値が宣言した型と範囲に収まること。
    // 上書き無しで評価できるので、ここで確かめられる。
    let probe = Definition {
        name: String::new(),
        origin: Point2::ORIGIN,
        params: params.to_vec(),
        entities: Vec::new(),
        bindings: Vec::new(),
    };
    let env = probe.param_env(&BTreeMap::new());
    for p in params {
        match env.get(&p.name) {
            Some(v) if p.accepts(v) => {}
            _ => {
                return Err(CadError::NotEditable(
                    "既定値が宣言した型または範囲に合いません",
                ))
            }
        }
    }

    Ok(())
}

/// 座標への束縛を 1 つ設定する（同じ座標への束縛があれば置き換える）。
#[derive(Debug)]
pub struct SetBinding {
    name: &'static str,
    target: DefinitionId,
    binding: Binding,
    /// Undo 用に控えた差し替え前の束縛一式。
    previous: Option<Vec<Binding>>,
}

impl SetBinding {
    /// 対象と束縛を指定して作る。
    #[must_use]
    pub fn new(name: &'static str, target: DefinitionId, binding: Binding) -> Self {
        Self {
            name,
            target,
            binding,
            previous: None,
        }
    }
}

impl Command for SetBinding {
    fn execute(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        let def = ctx
            .definitions()
            .get(self.target)
            .ok_or(CadError::DefinitionNotFound)?;

        // 指す先があり、スロットが図形の種類に合うこと。
        if !self.binding.fits(&def.entities) {
            return Err(CadError::DegenerateGeometry(
                "束縛が指す図形が無いか、スロットが図形の種類に合いません",
            ));
        }

        // 宣言されたパラメータだけを参照していること。
        let declared: Vec<&str> = def.params.iter().map(|p| p.name.as_str()).collect();
        for used in self.binding.expr.referenced_vars() {
            if !declared.contains(&used) {
                return Err(CadError::NotEditable(
                    "束縛が宣言されていないパラメータを参照しています",
                ));
            }
        }

        // 既定のパラメータで数値になること。
        // **座標に入るのは数値だけ**なので、ここで型を確かめる。
        let env = def.param_env(&BTreeMap::new());
        match crate::expr::eval(&self.binding.expr, &env) {
            Ok(crate::expr::Value::Number(_)) => {}
            Ok(_) => return Err(CadError::NotEditable("束縛の式は数値でなければなりません")),
            Err(_) => return Err(CadError::NotEditable("束縛の式を評価できません")),
        }

        // 同じ座標への束縛は置き換える。
        let mut bindings = def.bindings.clone();
        self.previous = Some(bindings.clone());
        bindings.retain(|b| !(b.entity == self.binding.entity && b.slot == self.binding.slot));
        bindings.push(self.binding.clone());

        let (origin, entities) = (def.origin, def.entities.clone());
        ctx.replace_definition_contents(self.target, origin, entities, bindings)?;
        Ok(())
    }

    fn undo(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        let Some(bindings) = self.previous.take() else {
            return Ok(());
        };
        let def = ctx
            .definitions()
            .get(self.target)
            .ok_or(CadError::DefinitionNotFound)?;
        let (origin, entities) = (def.origin, def.entities.clone());
        ctx.replace_definition_contents(self.target, origin, entities, bindings)?;
        Ok(())
    }

    fn name(&self) -> &'static str {
        self.name
    }
}

/// インスタンスのパラメータ上書きを設定する。
///
/// 値を `None` にすると上書きを消す（**既定値へのリセット**）。
#[derive(Debug)]
pub struct SetInstanceOverride {
    name: &'static str,
    target: EntityId,
    param: String,
    value: Option<crate::expr::Value>,
    /// Undo 用に控えた上書き一式。
    previous: Option<BTreeMap<String, crate::expr::Value>>,
}

impl SetInstanceOverride {
    /// 上書きを設定する。
    #[must_use]
    pub fn set(
        name: &'static str,
        target: EntityId,
        param: impl Into<String>,
        value: crate::expr::Value,
    ) -> Self {
        Self {
            name,
            target,
            param: param.into(),
            value: Some(value),
            previous: None,
        }
    }

    /// 上書きを消して既定値へ戻す。
    #[must_use]
    pub fn reset(name: &'static str, target: EntityId, param: impl Into<String>) -> Self {
        Self {
            name,
            target,
            param: param.into(),
            value: None,
            previous: None,
        }
    }
}

impl Command for SetInstanceOverride {
    fn execute(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        let entity = ctx
            .entities()
            .get(self.target)
            .ok_or(CadError::EntityNotFound)?;
        let Geometry::Instance(inst) = &entity.geom else {
            return Err(CadError::NotEditable(
                "コンポーネントのインスタンスではありません",
            ));
        };

        let def = ctx
            .definitions()
            .get(inst.definition)
            .ok_or(CadError::DefinitionNotFound)?;
        let decl = def
            .param(&self.param)
            .ok_or(CadError::NotEditable("そのパラメータはありません"))?;

        // 型と範囲を確かめる。**通すと解決で黙って捨てられる。**
        if let Some(v) = &self.value {
            if !decl.accepts(v) {
                return Err(CadError::NotEditable(
                    "値がパラメータの型または範囲に合いません",
                ));
            }
        }

        let mut overrides = inst.overrides.clone();
        self.previous = Some(overrides.clone());
        match &self.value {
            Some(v) => {
                overrides.insert(self.param.clone(), v.clone());
            }
            None => {
                overrides.remove(&self.param);
            }
        }

        let new_inst = Instance {
            definition: inst.definition,
            placement: inst.placement,
            overrides,
        };
        ctx.entity_mut(self.target)?.geom = Geometry::Instance(new_inst);
        Ok(())
    }

    fn undo(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        let Some(overrides) = self.previous.take() else {
            return Ok(());
        };
        let entity = ctx
            .entities()
            .get(self.target)
            .ok_or(CadError::EntityNotFound)?;
        let Geometry::Instance(inst) = &entity.geom else {
            return Err(CadError::NotEditable(
                "コンポーネントのインスタンスではありません",
            ));
        };
        let new_inst = Instance {
            definition: inst.definition,
            placement: inst.placement,
            overrides,
        };
        ctx.entity_mut(self.target)?.geom = Geometry::Instance(new_inst);
        Ok(())
    }

    fn name(&self) -> &'static str {
        self.name
    }
}

// ---------------------------------------------------------------------------
// インプレース編集
// ---------------------------------------------------------------------------

/// 定義の編集を始める。
///
/// インスタンスを図面から外し、**定義の中身を実エンティティとして図面へ置く**。
/// こうすると LINE / TRIM / MOVE といった既存のツールがそのまま使える。
/// 専用の編集モードを作るより、経路が 1 本で済む。
///
/// 置く位置は元のインスタンスの配置なので、**画面上の見た目は変わらない**。
/// 「編集に入った瞬間に図形が飛ぶ」ことがない。
#[derive(Debug)]
pub struct EnterDefinitionEdit {
    name: &'static str,
    /// 編集の入口になったインスタンス。
    instance: EntityId,
    /// Undo 用に控えた元のインスタンス。
    removed: Option<Entity>,
    /// 図面へ置いた要素。**定義の中身と同じ順**。
    created: Vec<EntityId>,
}

impl EnterDefinitionEdit {
    /// 編集の入口になるインスタンスを指定して作る。
    #[must_use]
    pub fn new(name: &'static str, instance: EntityId) -> Self {
        Self {
            name,
            instance,
            removed: None,
            created: Vec::new(),
        }
    }

    /// 図面へ置いた要素の ID。**定義の中身と同じ順**なので、
    /// 呼び出し側は添字で元の要素と対応づけられる（束縛の付け替えに使う）。
    #[must_use]
    pub fn created(&self) -> &[EntityId] {
        &self.created
    }
}

impl Command for EnterDefinitionEdit {
    fn execute(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        self.created.clear();

        let entity = ctx
            .entities()
            .get(self.instance)
            .cloned()
            .ok_or(CadError::EntityNotFound)?;
        let Geometry::Instance(inst) = &entity.geom else {
            return Err(CadError::NotEditable(
                "コンポーネントのインスタンスではありません",
            ));
        };
        let def = ctx
            .definitions()
            .get(inst.definition)
            .ok_or(CadError::DefinitionNotFound)?;

        // **束縛を評価した形ではなく、定義そのままの形を置く。**
        // 評価後の形を置いて書き戻すと、束縛が「いまの値」で固定されてしまう。
        let placed: Vec<Entity> = def
            .entities
            .iter()
            .map(|e| {
                let mut moved = e.clone();
                moved.geom = component::place(&e.geom, def.origin, inst.placement);
                moved
            })
            .collect();

        self.removed = Some(ctx.remove_entity(self.instance)?);
        for e in placed {
            self.created.push(ctx.add_entity(e));
        }
        Ok(())
    }

    fn undo(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        for id in self.created.drain(..).rev() {
            ctx.remove_entity(id)?;
        }
        if let Some(entity) = self.removed.take() {
            ctx.restore_entity(self.instance, entity)?;
        }
        Ok(())
    }

    fn name(&self) -> &'static str {
        self.name
    }
}

/// 定義の編集を終える。
///
/// 図面に置かれていた要素を**定義座標へ戻して**定義の中身にし、
/// 図面からは外してインスタンスを置き直す。
///
/// # 束縛の付け替え
///
/// 束縛は「中身への添字」で座標を指すので、編集で順序が変わると指す先がずれる。
/// `origins` に「図面の要素 → 元の定義での添字」を渡すことで、
/// **編集を通しても束縛が保たれる**。
///
/// - 消された要素の束縛は捨てる（指す先が無いので残せない）
/// - 新しく描いた要素には束縛が無い
#[derive(Debug)]
pub struct ExitDefinitionEdit {
    name: &'static str,
    definition: DefinitionId,
    /// 編集に入ったときの配置。定義座標へ戻すのに使う。
    placement: Placement,
    /// 定義の中身にする要素。**図面での並び順**。
    members: Vec<EntityId>,
    /// `members` と同じ長さ。元の定義での添字（新しい要素は `None`）。
    origins: Vec<Option<usize>>,
    /// Undo 用に控えた、取り除いた要素。
    removed: Vec<(EntityId, Entity)>,
    /// Undo 用に控えた差し替え前の定義。
    previous: Option<(Point2, Vec<Entity>, Vec<Binding>)>,
    /// 置き直したインスタンス。
    created: Option<EntityId>,
}

impl ExitDefinitionEdit {
    /// 編集の結果を書き戻すコマンドを作る。
    ///
    /// `members` は図面に置かれている要素、`origins` はそれぞれの
    /// 「元の定義での添字」（新しく描いたものは `None`）。長さは同じであること。
    #[must_use]
    pub fn new(
        name: &'static str,
        definition: DefinitionId,
        placement: Placement,
        members: Vec<EntityId>,
        origins: Vec<Option<usize>>,
    ) -> Self {
        Self {
            name,
            definition,
            placement,
            members,
            origins,
            removed: Vec::new(),
            previous: None,
            created: None,
        }
    }

    /// 置き直したインスタンスの ID。適用前は `None`。
    #[must_use]
    pub fn created(&self) -> Option<EntityId> {
        self.created
    }
}

impl Command for ExitDefinitionEdit {
    fn execute(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        self.removed.clear();
        self.created = None;

        if self.members.len() != self.origins.len() {
            return Err(CadError::NotEditable(
                "内部エラー: 編集中の要素と対応表の数が合いません",
            ));
        }

        let def = ctx
            .definitions()
            .get(self.definition)
            .ok_or(CadError::DefinitionNotFound)?;
        let (def_origin, layer) = (def.origin, ctx.layers().current());
        let old_bindings = def.bindings.clone();

        // ---- 定義座標へ戻す ----
        let mut contents = Vec::with_capacity(self.members.len());
        for id in &self.members {
            let entity = ctx
                .entities()
                .get(*id)
                .cloned()
                .ok_or(CadError::EntityNotFound)?;
            let mut back = entity;
            back.geom = component::unplace(&back.geom, def_origin, self.placement);
            contents.push(back);
        }

        // ---- 束縛を付け替える ----
        //
        // 元の添字 → 新しい添字の対応を作る。編集で消えた要素は入らない。
        let mut remap: BTreeMap<usize, usize> = BTreeMap::new();
        for (new_index, origin) in self.origins.iter().enumerate() {
            if let Some(old) = origin {
                remap.insert(*old, new_index);
            }
        }
        let bindings: Vec<Binding> = old_bindings
            .iter()
            .filter_map(|b| {
                let new_index = remap.get(&b.entity)?;
                let moved = Binding::new(*new_index, b.slot, b.expr.clone());
                // 編集で図形の種類が変わっていたら、その束縛は捨てる。
                moved.fits(&contents).then_some(moved)
            })
            .collect();

        // ---- 書き戻す ----
        self.previous = Some(ctx.replace_definition_contents(
            self.definition,
            def_origin,
            contents,
            bindings,
        )?);

        for id in &self.members {
            let removed = ctx.remove_entity(*id)?;
            self.removed.push((*id, removed));
        }

        let geom = Geometry::Instance(Instance::new(self.definition, self.placement));
        self.created = Some(ctx.add_entity(Entity::new(geom, layer)));
        Ok(())
    }

    fn undo(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        if let Some(id) = self.created.take() {
            ctx.remove_entity(id)?;
        }
        for (id, entity) in self.removed.drain(..).rev() {
            ctx.restore_entity(id, entity)?;
        }
        if let Some((origin, contents, bindings)) = self.previous.take() {
            ctx.replace_definition_contents(self.definition, origin, contents, bindings)?;
        }
        Ok(())
    }

    fn name(&self) -> &'static str {
        self.name
    }
}
