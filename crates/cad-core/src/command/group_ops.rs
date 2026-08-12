//! グループの操作と分解。

use super::{Command, EditCtx};
use crate::entity::{Entity, EntityId, Geometry};
use crate::error::{CadError, Result};
use crate::group::{Group, GroupId};

/// 選択した要素を 1 つのグループにまとめる。
///
/// 既に別のグループに属している要素も、この操作で新しいグループへ移る
/// （AutoCAD は入れ子のグループを許すが、本プロトタイプでは 1 段だけ扱う）。
#[derive(Debug)]
pub struct CreateGroup {
    name: &'static str,
    group_name: String,
    targets: Vec<EntityId>,
    /// このコマンドが確保したグループ ID。
    ///
    /// **Undo でも捨てない。** Redo で作り直すときに同じ ID を使うため。
    /// ID が変わると、Undo スタックに残る他のコマンド（そのグループを指す
    /// `Ungroup` など）の参照が壊れる（[ADR-0004] と同じ理由）。
    ///
    /// [ADR-0004]: ../../../../docs/DECISIONS.md
    allocated: Option<GroupId>,
    /// いま図面にグループが存在するか。`undo` で false になる。
    active: bool,
    /// Undo 用に、各要素の元の所属を控える。
    previous: Vec<(EntityId, Option<GroupId>)>,
}

impl CreateGroup {
    /// グループ名と対象を指定して作る。
    #[must_use]
    pub fn new(name: &'static str, group_name: impl Into<String>, targets: Vec<EntityId>) -> Self {
        Self {
            name,
            group_name: group_name.into(),
            targets,
            allocated: None,
            active: false,
            previous: Vec::new(),
        }
    }

    /// 適用後に作られたグループの ID。適用前は `None`。
    ///
    /// Undo したあとも同じ ID を返す（Redo で同じ ID を使い回すため）。
    #[must_use]
    pub fn created(&self) -> Option<GroupId> {
        self.active.then_some(self.allocated).flatten()
    }
}

impl Command for CreateGroup {
    fn execute(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        if self.targets.is_empty() {
            return Err(CadError::NotEditable("グループにする要素がありません"));
        }
        // Redo で再実行されるので毎回作り直す。
        self.previous.clear();

        // 2 回目以降（Redo）は最初に確保した ID をそのまま使う。
        let group = match self.allocated {
            Some(id) => {
                ctx.restore_group(id, Group::new(self.group_name.clone()))?;
                id
            }
            None => {
                let id = ctx.add_group(Group::new(self.group_name.clone()));
                self.allocated = Some(id);
                id
            }
        };
        self.active = true;

        for id in &self.targets {
            match ctx.entity_mut(*id) {
                Ok(entity) => {
                    self.previous.push((*id, entity.group));
                    entity.group = Some(group);
                }
                Err(e) => {
                    // 全部成功するか、何も変えずに失敗するか。
                    for (rid, prev) in self.previous.drain(..).rev() {
                        if let Ok(entity) = ctx.entity_mut(rid) {
                            entity.group = prev;
                        }
                    }
                    let _ = ctx.remove_group(group);
                    self.active = false;
                    return Err(e);
                }
            }
        }
        Ok(())
    }

    fn undo(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        for (id, prev) in self.previous.drain(..).rev() {
            ctx.entity_mut(id)?.group = prev;
        }
        if self.active {
            if let Some(group) = self.allocated {
                ctx.remove_group(group)?;
            }
            self.active = false;
        }
        Ok(())
    }

    fn name(&self) -> &'static str {
        self.name
    }
}

/// グループを解除する。所属していた要素は残る。
#[derive(Debug)]
pub struct Ungroup {
    name: &'static str,
    target: GroupId,
    /// Undo 用に、取り除いたグループそのものを控える。
    removed: Option<Group>,
    /// Undo 用に、所属していた要素を控える。
    members: Vec<EntityId>,
}

impl Ungroup {
    /// 解除するグループを指定して作る。
    #[must_use]
    pub fn new(name: &'static str, target: GroupId) -> Self {
        Self {
            name,
            target,
            removed: None,
            members: Vec::new(),
        }
    }
}

impl Command for Ungroup {
    fn execute(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        self.members.clear();

        // 所属はエンティティ側が持っているので、走査して集める。
        let members: Vec<EntityId> = ctx
            .entities()
            .iter()
            .filter(|(_, e)| e.group == Some(self.target))
            .map(|(id, _)| id)
            .collect();

        let removed = ctx.remove_group(self.target)?;

        for id in &members {
            // 直前に走査して得た ID なので必ず存在する。
            ctx.entity_mut(*id)?.group = None;
        }
        self.members = members;
        self.removed = Some(removed);
        Ok(())
    }

    fn undo(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        let Some(group) = self.removed.take() else {
            return Ok(());
        };
        ctx.restore_group(self.target, group)?;
        for id in self.members.drain(..) {
            ctx.entity_mut(id)?.group = Some(self.target);
        }
        Ok(())
    }

    fn name(&self) -> &'static str {
        self.name
    }
}

/// ポリラインを線分の集合へ分解する。
///
/// 分解できない要素（線分・円・円弧・作図線）は素通しする。
/// レイヤ・色・グループ所属は元の要素から引き継ぐ。
#[derive(Debug)]
pub struct ExplodeEntities {
    name: &'static str,
    targets: Vec<EntityId>,
    /// Undo 用に、取り除いた元の要素を控える。
    removed: Vec<(EntityId, Entity)>,
    /// 適用で作られた要素。Undo で消す。
    created: Vec<EntityId>,
}

impl ExplodeEntities {
    /// 分解する対象を指定して作る。
    #[must_use]
    pub fn new(name: &'static str, targets: Vec<EntityId>) -> Self {
        Self {
            name,
            targets,
            removed: Vec::new(),
            created: Vec::new(),
        }
    }

    /// 適用後に作られた要素の ID。適用前は空。
    #[must_use]
    pub fn created(&self) -> &[EntityId] {
        &self.created
    }

    /// 分解した結果の図形。分解できないものは空を返す。
    fn pieces(entity: &Entity) -> Vec<Geometry> {
        match &entity.geom {
            Geometry::Polyline(p) => p.segments().map(Geometry::Line).collect(),
            _ => Vec::new(),
        }
    }
}

impl Command for ExplodeEntities {
    fn execute(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        // Redo で再実行されるので毎回作り直す。
        self.removed.clear();
        self.created.clear();

        for id in &self.targets {
            // `?` で早期リターンすると、それまでに分解した分が残ってしまう。
            // all-or-nothing を守るため、必ず巻き戻してから返す。
            let Some(entity) = ctx.entities().get(*id).cloned() else {
                self.rollback(ctx);
                return Err(CadError::EntityNotFound);
            };
            let pieces = Self::pieces(&entity);
            if pieces.is_empty() {
                // 分解できない種類は触らない。
                continue;
            }

            let removed = match ctx.remove_entity(*id) {
                Ok(e) => e,
                Err(e) => {
                    self.rollback(ctx);
                    return Err(e);
                }
            };
            self.removed.push((*id, removed));

            for geom in pieces {
                let mut piece = entity.clone();
                piece.geom = geom;
                self.created.push(ctx.add_entity(piece));
            }
        }

        if self.removed.is_empty() {
            return Err(CadError::NotEditable("分解できる要素がありません"));
        }
        Ok(())
    }

    fn undo(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        for id in self.created.drain(..).rev() {
            ctx.remove_entity(id)?;
        }
        for (id, entity) in self.removed.drain(..).rev() {
            ctx.restore_entity(id, entity)?;
        }
        Ok(())
    }

    fn name(&self) -> &'static str {
        self.name
    }
}

impl ExplodeEntities {
    /// 途中で失敗したときに、それまでの変更を巻き戻す。
    fn rollback(&mut self, ctx: &mut EditCtx<'_>) {
        for id in self.created.drain(..).rev() {
            let _ = ctx.remove_entity(id);
        }
        for (id, entity) in self.removed.drain(..).rev() {
            let _ = ctx.restore_entity(id, entity);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::EntityStore;
    use crate::geom::{Line, Point2, Polyline};
    use crate::group::GroupTable;
    use crate::layer::{LayerId, LayerTable};

    fn new_parts() -> (EntityStore, LayerTable, GroupTable) {
        (EntityStore::new(), LayerTable::new(), GroupTable::new())
    }

    fn line_entity(x: f64) -> Entity {
        Entity::new(
            Geometry::Line(Line::new(Point2::new(x, 0.0), Point2::new(x, 1.0))),
            LayerId::ZERO,
        )
    }

    fn rect_entity() -> Entity {
        Entity::new(
            Geometry::Polyline(Polyline::rectangle(Point2::ORIGIN, Point2::new(10.0, 10.0))),
            LayerId::ZERO,
        )
    }

    // ---- CreateGroup ----

    #[test]
    fn create_group_execute_assigns_all_targets() {
        let (mut e, mut l, mut g) = new_parts();
        let (a, b) = {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
            (
                ctx.add_entity(line_entity(0.0)),
                ctx.add_entity(line_entity(1.0)),
            )
        };
        let mut cmd = CreateGroup::new("GROUP", "壁", vec![a, b]);
        let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
        cmd.execute(&mut ctx).unwrap();

        let gid = cmd.created().expect("グループができるはず");
        assert_eq!(ctx.entities().get(a).unwrap().group, Some(gid));
        assert_eq!(ctx.entities().get(b).unwrap().group, Some(gid));
        assert_eq!(ctx.groups().get(gid).unwrap().name, "壁");
    }

    #[test]
    fn create_group_undo_restores_previous_membership() {
        let (mut e, mut l, mut g) = new_parts();
        let a = {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
            ctx.add_entity(line_entity(0.0))
        };

        let mut first = CreateGroup::new("GROUP", "A", vec![a]);
        let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
        first.execute(&mut ctx).unwrap();
        let first_id = first.created().unwrap();

        // 2 つ目のグループへ移す。
        let mut second = CreateGroup::new("GROUP", "B", vec![a]);
        second.execute(&mut ctx).unwrap();
        assert_eq!(ctx.entities().get(a).unwrap().group, second.created());

        second.undo(&mut ctx).unwrap();
        assert_eq!(
            ctx.entities().get(a).unwrap().group,
            Some(first_id),
            "元の所属へ戻ること"
        );
        assert!(
            ctx.groups().by_name("B").is_none(),
            "作ったグループも消える"
        );
    }

    /// **Redo で同じグループ ID が使われること。**
    ///
    /// ID が変わると、Undo スタックに残る他コマンド（そのグループを指す `Ungroup` など）の
    /// 参照が壊れる。エンティティで `restore` を用意したのと同じ理由（ADR-0004）。
    #[test]
    fn create_group_redo_reuses_the_same_group_id() {
        let (mut e, mut l, mut g) = new_parts();
        let a = {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
            ctx.add_entity(line_entity(0.0))
        };
        let mut cmd = CreateGroup::new("GROUP", "壁", vec![a]);
        let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);

        cmd.execute(&mut ctx).unwrap();
        let first = cmd.created().expect("最初の適用で ID が決まる");

        cmd.undo(&mut ctx).unwrap();
        cmd.execute(&mut ctx).unwrap();

        assert_eq!(cmd.created(), Some(first), "Redo でも同じ ID を使うこと");
        assert_eq!(ctx.entities().get(a).unwrap().group, Some(first));
    }

    #[test]
    fn create_group_redo_after_undo_works() {
        let (mut e, mut l, mut g) = new_parts();
        let a = {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
            ctx.add_entity(line_entity(0.0))
        };
        let mut cmd = CreateGroup::new("GROUP", "壁", vec![a]);
        let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);

        cmd.execute(&mut ctx).unwrap();
        cmd.undo(&mut ctx).unwrap();
        assert!(ctx.entities().get(a).unwrap().group.is_none());

        cmd.execute(&mut ctx).unwrap();
        assert!(ctx.entities().get(a).unwrap().group.is_some());
    }

    #[test]
    fn create_group_missing_target_fails_and_leaves_document_unchanged() {
        let (mut e, mut l, mut g) = new_parts();
        let (alive, dead) = {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
            let alive = ctx.add_entity(line_entity(0.0));
            let dead = ctx.add_entity(line_entity(1.0));
            ctx.remove_entity(dead).unwrap();
            (alive, dead)
        };

        let mut cmd = CreateGroup::new("GROUP", "壁", vec![alive, dead]);
        let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
        assert_eq!(cmd.execute(&mut ctx), Err(CadError::EntityNotFound));
        assert!(
            ctx.entities().get(alive).unwrap().group.is_none(),
            "生きている要素の所属が変わっていないこと"
        );
        assert!(ctx.groups().by_name("壁").is_none(), "グループも作られない");
    }

    #[test]
    fn create_group_with_no_targets_is_rejected() {
        let (mut e, mut l, mut g) = new_parts();
        let mut cmd = CreateGroup::new("GROUP", "空", Vec::new());
        let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
        assert!(cmd.execute(&mut ctx).is_err());
    }

    // ---- Ungroup ----

    #[test]
    fn ungroup_execute_clears_membership_but_keeps_entities() {
        let (mut e, mut l, mut g) = new_parts();
        let (a, b, gid) = {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
            let a = ctx.add_entity(line_entity(0.0));
            let b = ctx.add_entity(line_entity(1.0));
            let mut create = CreateGroup::new("GROUP", "壁", vec![a, b]);
            create.execute(&mut ctx).unwrap();
            (a, b, create.created().unwrap())
        };

        let mut cmd = Ungroup::new("UNGROUP", gid);
        let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
        cmd.execute(&mut ctx).unwrap();

        assert!(ctx.entities().get(a).unwrap().group.is_none());
        assert!(ctx.entities().get(b).unwrap().group.is_none());
        assert_eq!(ctx.entities().len(), 2, "要素そのものは残る");
        assert!(ctx.groups().get(gid).is_none());
    }

    #[test]
    fn ungroup_undo_restores_the_group_and_membership() {
        let (mut e, mut l, mut g) = new_parts();
        let (a, b, gid) = {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
            let a = ctx.add_entity(line_entity(0.0));
            let b = ctx.add_entity(line_entity(1.0));
            let mut create = CreateGroup::new("GROUP", "壁", vec![a, b]);
            create.execute(&mut ctx).unwrap();
            (a, b, create.created().unwrap())
        };

        let mut cmd = Ungroup::new("UNGROUP", gid);
        let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
        cmd.execute(&mut ctx).unwrap();
        cmd.undo(&mut ctx).unwrap();

        assert_eq!(ctx.entities().get(a).unwrap().group, Some(gid));
        assert_eq!(ctx.entities().get(b).unwrap().group, Some(gid));
        assert_eq!(
            ctx.groups().get(gid).unwrap().name,
            "壁",
            "同じ ID・同じ名前で戻ること"
        );
    }

    /// 既に解除されたグループをもう一度解除しようとしたら失敗すること。
    #[test]
    fn ungroup_missing_group_fails() {
        let (mut e, mut l, mut g) = new_parts();
        let gid = {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
            let a = ctx.add_entity(line_entity(0.0));
            let mut create = CreateGroup::new("GROUP", "壁", vec![a]);
            create.execute(&mut ctx).unwrap();
            create.created().unwrap()
        };

        let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
        Ungroup::new("UNGROUP", gid).execute(&mut ctx).unwrap();

        let mut again = Ungroup::new("UNGROUP", gid);
        assert_eq!(again.execute(&mut ctx), Err(CadError::GroupNotFound));
    }

    // ---- ExplodeEntities ----

    #[test]
    fn explode_execute_splits_a_polyline_into_lines() {
        let (mut e, mut l, mut g) = new_parts();
        let id = {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
            ctx.add_entity(rect_entity())
        };

        let mut cmd = ExplodeEntities::new("EXPLODE", vec![id]);
        let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
        cmd.execute(&mut ctx).unwrap();

        // 閉じた矩形なので 4 本になる。
        assert_eq!(ctx.entities().len(), 4);
        assert_eq!(cmd.created().len(), 4);
        assert!(ctx.entities().get(id).is_none(), "元のポリラインは消える");
        for (_, entity) in ctx.entities().iter() {
            assert!(matches!(entity.geom, Geometry::Line(_)));
        }
    }

    /// 分解した破片が元の属性を引き継ぐこと。
    #[test]
    fn explode_keeps_layer_color_and_group() {
        let (mut e, mut l, mut g) = new_parts();
        let (id, gid) = {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
            let id = ctx.add_entity(rect_entity());
            let mut create = CreateGroup::new("GROUP", "枠", vec![id]);
            create.execute(&mut ctx).unwrap();
            (id, create.created().unwrap())
        };
        let original = e.get(id).unwrap().clone();

        let mut cmd = ExplodeEntities::new("EXPLODE", vec![id]);
        let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
        cmd.execute(&mut ctx).unwrap();

        for (_, entity) in ctx.entities().iter() {
            assert_eq!(entity.layer, original.layer);
            assert_eq!(entity.color, original.color);
            assert_eq!(entity.group, Some(gid), "グループ所属も引き継ぐ");
        }
    }

    #[test]
    fn explode_undo_restores_the_original_entity_with_the_same_id() {
        let (mut e, mut l, mut g) = new_parts();
        let id = {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
            ctx.add_entity(rect_entity())
        };
        let original = e.get(id).unwrap().clone();

        let mut cmd = ExplodeEntities::new("EXPLODE", vec![id]);
        let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
        cmd.execute(&mut ctx).unwrap();
        cmd.undo(&mut ctx).unwrap();

        assert_eq!(ctx.entities().len(), 1);
        assert_eq!(
            ctx.entities().get(id),
            Some(&original),
            "同じ ID・同じ内容で戻ること"
        );
    }

    #[test]
    fn explode_redo_after_undo_works() {
        let (mut e, mut l, mut g) = new_parts();
        let id = {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
            ctx.add_entity(rect_entity())
        };
        let mut cmd = ExplodeEntities::new("EXPLODE", vec![id]);
        let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);

        cmd.execute(&mut ctx).unwrap();
        cmd.undo(&mut ctx).unwrap();
        cmd.execute(&mut ctx).unwrap();
        assert_eq!(ctx.entities().len(), 4);
    }

    /// 分解できない種類しか無ければ失敗すること（黙って何もしないより親切）。
    #[test]
    fn explode_with_nothing_explodable_is_rejected() {
        let (mut e, mut l, mut g) = new_parts();
        let id = {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
            ctx.add_entity(line_entity(0.0))
        };
        let mut cmd = ExplodeEntities::new("EXPLODE", vec![id]);
        let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
        assert!(cmd.execute(&mut ctx).is_err());
        assert_eq!(ctx.entities().len(), 1, "図面は変わらない");
    }

    #[test]
    fn explode_missing_target_fails_and_leaves_document_unchanged() {
        let (mut e, mut l, mut g) = new_parts();
        let (alive, dead) = {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
            let alive = ctx.add_entity(rect_entity());
            let dead = ctx.add_entity(rect_entity());
            ctx.remove_entity(dead).unwrap();
            (alive, dead)
        };

        let mut cmd = ExplodeEntities::new("EXPLODE", vec![alive, dead]);
        let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
        assert_eq!(cmd.execute(&mut ctx), Err(CadError::EntityNotFound));
        assert_eq!(ctx.entities().len(), 1, "分解されていないこと");
        assert!(matches!(
            ctx.entities().get(alive).unwrap().geom,
            Geometry::Polyline(_)
        ));
    }
}
