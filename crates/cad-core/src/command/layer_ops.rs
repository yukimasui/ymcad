//! レイヤ操作のコマンド（追加・属性変更・改名・現在レイヤ切替・削除・移動）。

use super::{Command, EditCtx};
use crate::entity::{Entity, EntityId};
use crate::error::{CadError, Result};
use crate::layer::{AciColor, Layer, LayerId, LineType};

/// レイヤを追加する。
#[derive(Debug)]
pub struct AddLayer {
    name: String,
    color: AciColor,
    /// 適用後に割り当てられた ID。適用前は `None`。
    created: Option<LayerId>,
}

impl AddLayer {
    /// 名前と色を指定して作る。
    #[must_use]
    pub fn new(name: impl Into<String>, color: AciColor) -> Self {
        Self {
            name: name.into(),
            color,
            created: None,
        }
    }

    /// 適用後に割り当てられた ID。適用前は `None`。
    #[must_use]
    pub fn created(&self) -> Option<LayerId> {
        self.created
    }
}

impl Command for AddLayer {
    fn execute(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        // Redo で 2 度目に実行されることがあるので、必ず作り直す。
        let layer = Layer::new(self.name.clone(), self.color);
        self.created = Some(ctx.add_layer(layer));
        Ok(())
    }

    fn undo(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        if let Some(id) = self.created.take() {
            ctx.remove_layer(id)?;
        }
        Ok(())
    }

    fn name(&self) -> &'static str {
        "LAYER_ADD"
    }
}

/// レイヤの属性を変更する（色 / 表示 / ロック / 線種 のうち指定されたものだけ）。
#[derive(Debug)]
pub struct SetLayerProperties {
    target: LayerId,
    color: Option<AciColor>,
    visible: Option<bool>,
    locked: Option<bool>,
    linetype: Option<LineType>,
    /// Undo で戻すための、適用直前の全属性。
    prev: Option<Layer>,
}

impl SetLayerProperties {
    /// 対象レイヤを指定して作る。ビルダーメソッドで変更したい属性だけ指定する。
    #[must_use]
    pub fn new(target: LayerId) -> Self {
        Self {
            target,
            color: None,
            visible: None,
            locked: None,
            linetype: None,
            prev: None,
        }
    }

    /// 色を変更する。
    #[must_use]
    pub fn color(mut self, c: AciColor) -> Self {
        self.color = Some(c);
        self
    }

    /// 表示/非表示を変更する。
    #[must_use]
    pub fn visible(mut self, v: bool) -> Self {
        self.visible = Some(v);
        self
    }

    /// ロック状態を変更する。
    #[must_use]
    pub fn locked(mut self, v: bool) -> Self {
        self.locked = Some(v);
        self
    }

    /// 線種を変更する。
    #[must_use]
    pub fn linetype(mut self, t: LineType) -> Self {
        self.linetype = Some(t);
        self
    }
}

impl Command for SetLayerProperties {
    fn execute(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        let layer = ctx.layer_mut(self.target)?;
        // Redo で 2 度目に実行されることがあるので、必ず作り直す。
        self.prev = Some(layer.clone());
        if let Some(c) = self.color {
            layer.color = c;
        }
        if let Some(v) = self.visible {
            layer.visible = v;
        }
        if let Some(v) = self.locked {
            layer.locked = v;
        }
        if let Some(t) = self.linetype {
            layer.linetype = t;
        }
        Ok(())
    }

    fn undo(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        if let Some(prev) = self.prev.take() {
            let layer = ctx.layer_mut(self.target)?;
            *layer = prev;
        }
        Ok(())
    }

    fn name(&self) -> &'static str {
        "LAYER_PROPS"
    }
}

/// レイヤ名を変更する。
#[derive(Debug)]
pub struct RenameLayer {
    target: LayerId,
    new_name: String,
    /// Undo で戻すための、変更前の名前。
    prev_name: Option<String>,
}

impl RenameLayer {
    /// 対象レイヤと新しい名前を指定して作る。
    #[must_use]
    pub fn new(target: LayerId, new_name: impl Into<String>) -> Self {
        Self {
            target,
            new_name: new_name.into(),
            prev_name: None,
        }
    }
}

impl Command for RenameLayer {
    fn execute(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        if self.target == LayerId::ZERO {
            return Err(CadError::NotEditable("レイヤ \"0\" の名前は変更できません"));
        }
        if ctx.layers().get(self.target).is_none() {
            return Err(CadError::LayerNotFound);
        }
        // 自分自身への改名（no-op）は許可し、別レイヤとの名前衝突だけ拒否する。
        if let Some(existing) = ctx.layers().by_name(&self.new_name) {
            if existing != self.target {
                return Err(CadError::NotEditable("同名のレイヤが既に存在します"));
            }
        }
        let old = ctx.rename_layer(self.target, self.new_name.clone())?;
        self.prev_name = Some(old);
        Ok(())
    }

    fn undo(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        if let Some(prev) = self.prev_name.take() {
            ctx.rename_layer(self.target, prev)?;
        }
        Ok(())
    }

    fn name(&self) -> &'static str {
        "LAYER_RENAME"
    }
}

/// 現在レイヤを切り替える。
#[derive(Debug)]
pub struct SetCurrentLayer {
    target: LayerId,
    /// Undo で戻すための、変更前の現在レイヤ。
    prev: Option<LayerId>,
}

impl SetCurrentLayer {
    /// 切り替え先を指定して作る。
    #[must_use]
    pub fn new(target: LayerId) -> Self {
        Self { target, prev: None }
    }
}

impl Command for SetCurrentLayer {
    fn execute(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        if ctx.layers().get(self.target).is_none() {
            return Err(CadError::LayerNotFound);
        }
        self.prev = Some(ctx.layers().current());
        ctx.set_current_layer(self.target);
        Ok(())
    }

    fn undo(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        if let Some(prev) = self.prev.take() {
            ctx.set_current_layer(prev);
        }
        Ok(())
    }

    fn name(&self) -> &'static str {
        "LAYER_CURRENT"
    }
}

/// レイヤを削除する。所属エンティティも一緒に消し、Undo で両方戻す。
#[derive(Debug)]
pub struct DeleteLayer {
    target: LayerId,
    /// Undo で元の `LayerId` のまま戻すために保持する、削除したレイヤ本体。
    removed_layer: Option<Layer>,
    /// Undo で元の `EntityId` のまま戻すために保持する、削除したエンティティ。
    removed_entities: Vec<(EntityId, Entity)>,
}

impl DeleteLayer {
    /// 削除対象を指定して作る。
    #[must_use]
    pub fn new(target: LayerId) -> Self {
        Self {
            target,
            removed_layer: None,
            removed_entities: Vec::new(),
        }
    }
}

impl Command for DeleteLayer {
    fn execute(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        if self.target == LayerId::ZERO {
            return Err(CadError::NotEditable("レイヤ \"0\" は削除できません"));
        }
        if ctx.layers().current() == self.target {
            return Err(CadError::NotEditable("現在レイヤは削除できません"));
        }
        if ctx.layers().get(self.target).is_none() {
            return Err(CadError::LayerNotFound);
        }

        // 所属エンティティを先に確定させてから削除する。
        let ids: Vec<EntityId> = ctx
            .entities()
            .iter()
            .filter(|(_, e)| e.layer == self.target)
            .map(|(id, _)| id)
            .collect();

        // 「全部成功するか、何も変えずに失敗するか」を守るため、途中で失敗したら
        // ここまでに削除した分を元の ID のまま戻してから Err を返す。
        self.removed_entities.clear();
        for id in ids {
            match ctx.remove_entity(id) {
                Ok(e) => self.removed_entities.push((id, e)),
                Err(err) => {
                    for (rid, re) in self.removed_entities.drain(..).rev() {
                        let _ = ctx.restore_entity(rid, re);
                    }
                    return Err(err);
                }
            }
        }

        match ctx.remove_layer(self.target) {
            Ok(layer) => {
                self.removed_layer = Some(layer);
                Ok(())
            }
            Err(err) => {
                for (rid, re) in self.removed_entities.drain(..).rev() {
                    let _ = ctx.restore_entity(rid, re);
                }
                Err(err)
            }
        }
    }

    fn undo(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        if let Some(layer) = self.removed_layer.take() {
            ctx.restore_layer(self.target, layer)?;
        }
        // レイヤ削除と逆順に、元の ID のままエンティティを戻す。
        for (id, entity) in self.removed_entities.drain(..).rev() {
            ctx.restore_entity(id, entity)?;
        }
        Ok(())
    }

    fn name(&self) -> &'static str {
        "LAYER_DELETE"
    }
}

/// 選択したエンティティを別のレイヤへ移す。
#[derive(Debug)]
pub struct MoveEntitiesToLayer {
    targets: Vec<EntityId>,
    dest: LayerId,
    /// Undo で元のレイヤへ正確に戻すための退避先。
    prev: Vec<(EntityId, LayerId)>,
}

impl MoveEntitiesToLayer {
    /// 移動対象と移動先レイヤを指定して作る。
    #[must_use]
    pub fn new(targets: Vec<EntityId>, dest: LayerId) -> Self {
        Self {
            targets,
            dest,
            prev: Vec::new(),
        }
    }
}

impl Command for MoveEntitiesToLayer {
    fn execute(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        if ctx.layers().get(self.dest).is_none() {
            return Err(CadError::LayerNotFound);
        }

        // Redo で 2 度目に実行されることがあるので、必ず作り直す。
        self.prev.clear();
        for id in &self.targets {
            match ctx.entity_mut(*id) {
                Ok(e) => {
                    self.prev.push((*id, e.layer));
                    e.layer = self.dest;
                }
                Err(err) => {
                    // 「全部成功するか、何も変えずに失敗するか」を守るため、
                    // ここまでに移動した分を元のレイヤへ戻してから Err を返す。
                    for (rid, rl) in self.prev.drain(..) {
                        if let Ok(e) = ctx.entity_mut(rid) {
                            e.layer = rl;
                        }
                    }
                    return Err(err);
                }
            }
        }
        Ok(())
    }

    fn undo(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        for (id, layer) in self.prev.drain(..) {
            let e = ctx.entity_mut(id)?;
            e.layer = layer;
        }
        Ok(())
    }

    fn name(&self) -> &'static str {
        "LAYER_MOVE_ENTITIES"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::{EntityStore, Geometry};
    use crate::geom::{Line, Point2};
    use crate::group::GroupTable;
    use crate::layer::LayerTable;

    fn line_entity(layer: LayerId) -> Entity {
        Entity::new(
            Geometry::Line(Line::new(Point2::ORIGIN, Point2::new(1.0, 0.0))),
            layer,
        )
    }

    /// テスト用の `EntityStore` / `LayerTable` の組。`EditCtx` はこの 2 つの
    /// `&mut` からしか作れない。
    fn new_parts() -> (EntityStore, LayerTable, GroupTable) {
        (EntityStore::new(), LayerTable::new(), GroupTable::new())
    }

    // ---- AddLayer -----------------------------------------------------

    #[test]
    fn add_layer_execute_creates_and_returns_id() {
        let (mut entities, mut layers, mut groups) = new_parts();
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups);

        let mut cmd = AddLayer::new("WALL", AciColor::RED);
        assert!(cmd.created().is_none(), "適用前は None のはず");
        cmd.execute(&mut ctx).unwrap();

        let id = cmd.created().expect("適用後は Some のはず");
        assert_eq!(ctx.layers().get(id).unwrap().name, "WALL");
        assert_eq!(ctx.layers().get(id).unwrap().color, AciColor::RED);
        assert_eq!(ctx.layers().by_name("WALL"), Some(id));
    }

    #[test]
    fn add_layer_undo_removes_it_exactly() {
        let (mut entities, mut layers, mut groups) = new_parts();
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups);
        let before = ctx.layers().len();

        let mut cmd = AddLayer::new("WALL", AciColor::RED);
        cmd.execute(&mut ctx).unwrap();
        cmd.undo(&mut ctx).unwrap();

        assert_eq!(ctx.layers().len(), before);
        assert_eq!(ctx.layers().by_name("WALL"), None);
        assert!(cmd.created().is_none());
    }

    #[test]
    fn add_layer_redo_after_undo_recreates() {
        let (mut entities, mut layers, mut groups) = new_parts();
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups);

        let mut cmd = AddLayer::new("WALL", AciColor::RED);
        cmd.execute(&mut ctx).unwrap();
        cmd.undo(&mut ctx).unwrap();
        cmd.execute(&mut ctx).unwrap();

        let id = cmd.created().expect("Redo 後も Some のはず");
        assert_eq!(ctx.layers().get(id).unwrap().name, "WALL");
    }

    // ---- SetLayerProperties --------------------------------------------

    #[test]
    fn set_layer_properties_execute_changes_only_specified_fields() {
        let (mut entities, mut layers, mut groups) = new_parts();
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups);
        let id = ctx.add_layer(Layer::new("WALL", AciColor::WHITE));

        let mut cmd = SetLayerProperties::new(id)
            .color(AciColor::RED)
            .locked(true);
        cmd.execute(&mut ctx).unwrap();

        let layer = ctx.layers().get(id).unwrap();
        assert_eq!(layer.color, AciColor::RED);
        assert!(layer.locked);
        assert!(layer.visible, "指定していない属性は変わらないこと");
        assert_eq!(layer.linetype, LineType::Continuous);
    }

    #[test]
    fn set_layer_properties_undo_restores_all_fields() {
        let (mut entities, mut layers, mut groups) = new_parts();
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups);
        let id = ctx.add_layer(Layer::new("WALL", AciColor::WHITE));

        let mut cmd = SetLayerProperties::new(id)
            .color(AciColor::RED)
            .visible(false)
            .locked(true)
            .linetype(LineType::Dashed);
        cmd.execute(&mut ctx).unwrap();
        cmd.undo(&mut ctx).unwrap();

        let layer = ctx.layers().get(id).unwrap();
        assert_eq!(layer.color, AciColor::WHITE);
        assert!(layer.visible);
        assert!(!layer.locked);
        assert_eq!(layer.linetype, LineType::Continuous);
    }

    #[test]
    fn set_layer_properties_redo_after_undo_reapplies() {
        let (mut entities, mut layers, mut groups) = new_parts();
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups);
        let id = ctx.add_layer(Layer::new("WALL", AciColor::WHITE));

        let mut cmd = SetLayerProperties::new(id).linetype(LineType::Center);
        cmd.execute(&mut ctx).unwrap();
        cmd.undo(&mut ctx).unwrap();
        cmd.execute(&mut ctx).unwrap();

        assert_eq!(ctx.layers().get(id).unwrap().linetype, LineType::Center);
    }

    #[test]
    fn set_layer_properties_missing_layer_fails() {
        let (mut entities, mut layers, mut groups) = new_parts();
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups);
        // 追加してすぐ削除し、存在しない ID を用意する。
        let ghost = ctx.add_layer(Layer::new("GHOST", AciColor::WHITE));
        ctx.remove_layer(ghost).unwrap();

        let mut cmd = SetLayerProperties::new(ghost).color(AciColor::RED);
        let result = cmd.execute(&mut ctx);
        assert_eq!(result, Err(CadError::LayerNotFound));
    }

    #[test]
    fn hidden_layer_still_hides_and_disables_entity() {
        let (mut entities, mut layers, mut groups) = new_parts();
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups);
        let id = ctx.add_layer(Layer::new("HIDDEN", AciColor::WHITE));
        let mut cmd = SetLayerProperties::new(id).visible(false);
        cmd.execute(&mut ctx).unwrap();

        let e = line_entity(id);
        assert!(!ctx.layers().is_entity_visible(&e));
        assert!(!ctx.layers().is_entity_editable(&e));
    }

    #[test]
    fn locked_layer_still_visible_but_not_editable() {
        let (mut entities, mut layers, mut groups) = new_parts();
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups);
        let id = ctx.add_layer(Layer::new("LOCKED", AciColor::WHITE));
        let mut cmd = SetLayerProperties::new(id).locked(true);
        cmd.execute(&mut ctx).unwrap();

        let e = line_entity(id);
        assert!(ctx.layers().is_entity_visible(&e));
        assert!(!ctx.layers().is_entity_editable(&e));
    }

    // ---- RenameLayer -----------------------------------------------------

    #[test]
    fn rename_layer_execute_updates_name_and_by_name_map() {
        let (mut entities, mut layers, mut groups) = new_parts();
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups);
        let id = ctx.add_layer(Layer::new("WALL", AciColor::WHITE));

        let mut cmd = RenameLayer::new(id, "STRUCTURE");
        cmd.execute(&mut ctx).unwrap();

        assert_eq!(ctx.layers().get(id).unwrap().name, "STRUCTURE");
        assert_eq!(ctx.layers().by_name("WALL"), None);
        assert_eq!(ctx.layers().by_name("STRUCTURE"), Some(id));
    }

    #[test]
    fn rename_layer_undo_restores_name_and_by_name_map() {
        let (mut entities, mut layers, mut groups) = new_parts();
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups);
        let id = ctx.add_layer(Layer::new("WALL", AciColor::WHITE));

        let mut cmd = RenameLayer::new(id, "STRUCTURE");
        cmd.execute(&mut ctx).unwrap();
        cmd.undo(&mut ctx).unwrap();

        assert_eq!(ctx.layers().get(id).unwrap().name, "WALL");
        assert_eq!(ctx.layers().by_name("STRUCTURE"), None);
        assert_eq!(ctx.layers().by_name("WALL"), Some(id));
    }

    #[test]
    fn rename_layer_redo_after_undo_renames_again() {
        let (mut entities, mut layers, mut groups) = new_parts();
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups);
        let id = ctx.add_layer(Layer::new("WALL", AciColor::WHITE));

        let mut cmd = RenameLayer::new(id, "STRUCTURE");
        cmd.execute(&mut ctx).unwrap();
        cmd.undo(&mut ctx).unwrap();
        cmd.execute(&mut ctx).unwrap();

        assert_eq!(ctx.layers().get(id).unwrap().name, "STRUCTURE");
        assert_eq!(ctx.layers().by_name("STRUCTURE"), Some(id));
    }

    #[test]
    fn rename_layer_zero_is_rejected() {
        let (mut entities, mut layers, mut groups) = new_parts();
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups);

        let mut cmd = RenameLayer::new(LayerId::ZERO, "NOT_ZERO");
        let result = cmd.execute(&mut ctx);

        assert!(matches!(result, Err(CadError::NotEditable(_))));
        assert_eq!(ctx.layers().get(LayerId::ZERO).unwrap().name, "0");
    }

    #[test]
    fn rename_layer_to_existing_other_name_is_rejected() {
        let (mut entities, mut layers, mut groups) = new_parts();
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups);
        let a = ctx.add_layer(Layer::new("A", AciColor::WHITE));
        let _b = ctx.add_layer(Layer::new("B", AciColor::WHITE));

        let mut cmd = RenameLayer::new(a, "B");
        let result = cmd.execute(&mut ctx);

        assert!(matches!(result, Err(CadError::NotEditable(_))));
        assert_eq!(ctx.layers().get(a).unwrap().name, "A", "変更されないこと");
    }

    #[test]
    fn rename_layer_to_own_name_is_a_noop() {
        let (mut entities, mut layers, mut groups) = new_parts();
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups);
        let a = ctx.add_layer(Layer::new("A", AciColor::WHITE));

        let mut cmd = RenameLayer::new(a, "A");
        cmd.execute(&mut ctx).unwrap();

        assert_eq!(ctx.layers().get(a).unwrap().name, "A");
        assert_eq!(ctx.layers().by_name("A"), Some(a));
    }

    #[test]
    fn rename_layer_missing_layer_fails() {
        let (mut entities, mut layers, mut groups) = new_parts();
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups);
        let id = ctx.add_layer(Layer::new("GHOST", AciColor::WHITE));
        ctx.remove_layer(id).unwrap();

        let mut cmd = RenameLayer::new(id, "ANYTHING");
        let result = cmd.execute(&mut ctx);
        assert_eq!(result, Err(CadError::LayerNotFound));
    }

    // ---- SetCurrentLayer ---------------------------------------------------

    #[test]
    fn set_current_layer_execute_switches_current() {
        let (mut entities, mut layers, mut groups) = new_parts();
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups);
        let id = ctx.add_layer(Layer::new("WALL", AciColor::WHITE));

        let mut cmd = SetCurrentLayer::new(id);
        cmd.execute(&mut ctx).unwrap();

        assert_eq!(ctx.layers().current(), id);
    }

    #[test]
    fn set_current_layer_undo_restores_previous_current() {
        let (mut entities, mut layers, mut groups) = new_parts();
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups);
        let id = ctx.add_layer(Layer::new("WALL", AciColor::WHITE));

        let mut cmd = SetCurrentLayer::new(id);
        cmd.execute(&mut ctx).unwrap();
        cmd.undo(&mut ctx).unwrap();

        assert_eq!(ctx.layers().current(), LayerId::ZERO);
    }

    #[test]
    fn set_current_layer_redo_after_undo_switches_again() {
        let (mut entities, mut layers, mut groups) = new_parts();
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups);
        let id = ctx.add_layer(Layer::new("WALL", AciColor::WHITE));

        let mut cmd = SetCurrentLayer::new(id);
        cmd.execute(&mut ctx).unwrap();
        cmd.undo(&mut ctx).unwrap();
        cmd.execute(&mut ctx).unwrap();

        assert_eq!(ctx.layers().current(), id);
    }

    #[test]
    fn set_current_layer_missing_layer_fails_and_leaves_current_unchanged() {
        let (mut entities, mut layers, mut groups) = new_parts();
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups);
        let id = ctx.add_layer(Layer::new("GHOST", AciColor::WHITE));
        ctx.remove_layer(id).unwrap();

        let mut cmd = SetCurrentLayer::new(id);
        let result = cmd.execute(&mut ctx);

        assert_eq!(result, Err(CadError::LayerNotFound));
        assert_eq!(ctx.layers().current(), LayerId::ZERO);
    }

    // ---- DeleteLayer -----------------------------------------------------

    #[test]
    fn delete_layer_execute_removes_layer_and_its_entities() {
        let (mut entities, mut layers, mut groups) = new_parts();
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups);
        let id = ctx.add_layer(Layer::new("WALL", AciColor::WHITE));
        let e1 = ctx.add_entity(line_entity(id));
        let e2 = ctx.add_entity(line_entity(id));
        let survivor = ctx.add_entity(line_entity(LayerId::ZERO));

        let mut cmd = DeleteLayer::new(id);
        cmd.execute(&mut ctx).unwrap();

        assert!(ctx.layers().get(id).is_none());
        assert!(!ctx.entities().contains(e1));
        assert!(!ctx.entities().contains(e2));
        assert!(ctx.entities().contains(survivor), "他レイヤの要素は残る");
    }

    #[test]
    fn delete_layer_undo_restores_layer_and_entities_with_same_ids() {
        let (mut entities, mut layers, mut groups) = new_parts();
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups);
        let id = ctx.add_layer(Layer::new("WALL", AciColor::RED));
        let e1 = ctx.add_entity(line_entity(id));
        let e2 = ctx.add_entity(line_entity(id));

        let mut cmd = DeleteLayer::new(id);
        cmd.execute(&mut ctx).unwrap();
        cmd.undo(&mut ctx).unwrap();

        let layer = ctx.layers().get(id).expect("レイヤが元の ID で戻る");
        assert_eq!(layer.name, "WALL");
        assert_eq!(layer.color, AciColor::RED);
        assert!(
            ctx.entities().contains(e1) && ctx.entities().contains(e2),
            "エンティティが元の ID で戻ること"
        );
        assert_eq!(ctx.layers().by_name("WALL"), Some(id));
    }

    #[test]
    fn delete_layer_redo_after_undo_removes_again() {
        let (mut entities, mut layers, mut groups) = new_parts();
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups);
        let id = ctx.add_layer(Layer::new("WALL", AciColor::WHITE));
        let e1 = ctx.add_entity(line_entity(id));

        let mut cmd = DeleteLayer::new(id);
        cmd.execute(&mut ctx).unwrap();
        cmd.undo(&mut ctx).unwrap();
        cmd.execute(&mut ctx).unwrap();

        assert!(ctx.layers().get(id).is_none());
        assert!(!ctx.entities().contains(e1));
    }

    #[test]
    fn delete_layer_zero_is_rejected() {
        let (mut entities, mut layers, mut groups) = new_parts();
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups);

        let mut cmd = DeleteLayer::new(LayerId::ZERO);
        let result = cmd.execute(&mut ctx);

        assert!(matches!(result, Err(CadError::NotEditable(_))));
        assert!(ctx.layers().get(LayerId::ZERO).is_some());
    }

    #[test]
    fn delete_current_layer_is_rejected() {
        let (mut entities, mut layers, mut groups) = new_parts();
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups);
        let id = ctx.add_layer(Layer::new("WALL", AciColor::WHITE));
        ctx.set_current_layer(id);

        let mut cmd = DeleteLayer::new(id);
        let result = cmd.execute(&mut ctx);

        assert!(matches!(result, Err(CadError::NotEditable(_))));
        assert!(ctx.layers().get(id).is_some(), "拒否されたので残っている");
    }

    #[test]
    fn delete_layer_missing_layer_fails() {
        let (mut entities, mut layers, mut groups) = new_parts();
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups);
        let id = ctx.add_layer(Layer::new("GHOST", AciColor::WHITE));
        ctx.remove_layer(id).unwrap();

        let mut cmd = DeleteLayer::new(id);
        let result = cmd.execute(&mut ctx);
        assert_eq!(result, Err(CadError::LayerNotFound));
    }

    // ---- MoveEntitiesToLayer -----------------------------------------------

    #[test]
    fn move_entities_to_layer_execute_changes_layer() {
        let (mut entities, mut layers, mut groups) = new_parts();
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups);
        let dest = ctx.add_layer(Layer::new("WALL", AciColor::WHITE));
        let a = ctx.add_entity(line_entity(LayerId::ZERO));
        let b = ctx.add_entity(line_entity(LayerId::ZERO));

        let mut cmd = MoveEntitiesToLayer::new(vec![a, b], dest);
        cmd.execute(&mut ctx).unwrap();

        assert_eq!(ctx.entities().get(a).unwrap().layer, dest);
        assert_eq!(ctx.entities().get(b).unwrap().layer, dest);
    }

    #[test]
    fn move_entities_to_layer_undo_restores_original_layers() {
        let (mut entities, mut layers, mut groups) = new_parts();
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups);
        let dest = ctx.add_layer(Layer::new("WALL", AciColor::WHITE));
        let a = ctx.add_entity(line_entity(LayerId::ZERO));

        let mut cmd = MoveEntitiesToLayer::new(vec![a], dest);
        cmd.execute(&mut ctx).unwrap();
        cmd.undo(&mut ctx).unwrap();

        assert_eq!(ctx.entities().get(a).unwrap().layer, LayerId::ZERO);
    }

    #[test]
    fn move_entities_to_layer_redo_after_undo_moves_again() {
        let (mut entities, mut layers, mut groups) = new_parts();
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups);
        let dest = ctx.add_layer(Layer::new("WALL", AciColor::WHITE));
        let a = ctx.add_entity(line_entity(LayerId::ZERO));

        let mut cmd = MoveEntitiesToLayer::new(vec![a], dest);
        cmd.execute(&mut ctx).unwrap();
        cmd.undo(&mut ctx).unwrap();
        cmd.execute(&mut ctx).unwrap();

        assert_eq!(ctx.entities().get(a).unwrap().layer, dest);
    }

    #[test]
    fn move_entities_to_layer_missing_destination_fails_and_leaves_document_unchanged() {
        let (mut entities, mut layers, mut groups) = new_parts();
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups);
        let ghost = ctx.add_layer(Layer::new("GHOST", AciColor::WHITE));
        ctx.remove_layer(ghost).unwrap();
        let a = ctx.add_entity(line_entity(LayerId::ZERO));

        let mut cmd = MoveEntitiesToLayer::new(vec![a], ghost);
        let result = cmd.execute(&mut ctx);

        assert_eq!(result, Err(CadError::LayerNotFound));
        assert_eq!(ctx.entities().get(a).unwrap().layer, LayerId::ZERO);
    }

    #[test]
    fn move_entities_to_layer_missing_entity_leaves_document_unchanged() {
        let (mut entities, mut layers, mut groups) = new_parts();
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups);
        let dest = ctx.add_layer(Layer::new("WALL", AciColor::WHITE));
        let survivor = ctx.add_entity(line_entity(LayerId::ZERO));
        let doomed = ctx.add_entity(line_entity(LayerId::ZERO));
        ctx.remove_entity(doomed).unwrap();

        let mut cmd = MoveEntitiesToLayer::new(vec![survivor, doomed], dest);
        let result = cmd.execute(&mut ctx);

        assert_eq!(result, Err(CadError::EntityNotFound));
        assert_eq!(
            ctx.entities().get(survivor).unwrap().layer,
            LayerId::ZERO,
            "失敗したので移動していないこと"
        );
    }

    // ---- LineType ----------------------------------------------------------

    #[test]
    fn linetype_dash_pattern_continuous_is_empty() {
        assert!(LineType::Continuous.dash_pattern_px().is_empty());
    }

    #[test]
    fn linetype_dash_pattern_others_are_nonempty() {
        assert!(!LineType::Dashed.dash_pattern_px().is_empty());
        assert!(!LineType::Center.dash_pattern_px().is_empty());
        assert!(!LineType::Hidden.dash_pattern_px().is_empty());
    }

    #[test]
    fn linetype_dxf_names() {
        assert_eq!(LineType::Continuous.dxf_name(), "CONTINUOUS");
        assert_eq!(LineType::Dashed.dxf_name(), "DASHED");
        assert_eq!(LineType::Center.dxf_name(), "CENTER");
        assert_eq!(LineType::Hidden.dxf_name(), "HIDDEN");
    }

    #[test]
    fn linetype_all_has_four_distinct_variants() {
        let all = LineType::all();
        assert_eq!(all.len(), 4);
        assert_eq!(all[0], LineType::Continuous);
        let mut sorted = all.to_vec();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), 4, "重複がないこと");
    }

    #[test]
    fn resolve_linetype_inherits_from_layer() {
        let (mut entities, mut layers, mut groups) = new_parts();
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups);
        let id = ctx.add_layer(Layer::new("WALL", AciColor::WHITE));
        let mut set = SetLayerProperties::new(id).linetype(LineType::Hidden);
        set.execute(&mut ctx).unwrap();

        let e = line_entity(id);
        assert_eq!(ctx.layers().resolve_linetype(&e), LineType::Hidden);
    }

    #[test]
    fn resolve_linetype_defaults_to_continuous_for_new_layer() {
        let (mut entities, mut layers, mut groups) = new_parts();
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups);
        let id = ctx.add_layer(Layer::new("WALL", AciColor::WHITE));

        let e = line_entity(id);
        assert_eq!(ctx.layers().resolve_linetype(&e), LineType::Continuous);
    }
}
