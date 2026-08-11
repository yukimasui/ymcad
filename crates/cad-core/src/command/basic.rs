//! 基本的な追加・削除コマンド。
//!
//! 作図コマンド（LINE / CIRCLE / …）は Phase 3 でこれらを組み合わせて実装する。

use super::{Command, EditCtx};
use crate::entity::{Entity, EntityId};
use crate::error::Result;

/// エンティティをまとめて追加する。
#[derive(Debug)]
pub struct AddEntities {
    name: &'static str,
    /// 追加する内容。Undo 後の Redo でも同じものを入れ直せるよう保持し続ける。
    entities: Vec<Entity>,
    /// 適用時に割り当てられた ID。Undo 対象を特定するために覚える。
    ids: Vec<EntityId>,
}

impl AddEntities {
    /// 1 要素だけ追加する。
    #[must_use]
    pub fn one(name: &'static str, entity: Entity) -> Self {
        Self::many(name, vec![entity])
    }

    /// 複数要素をまとめて追加する。
    #[must_use]
    pub fn many(name: &'static str, entities: Vec<Entity>) -> Self {
        Self {
            name,
            entities,
            ids: Vec::new(),
        }
    }

    /// 適用後に割り当てられた ID。適用前は空。
    #[must_use]
    pub fn ids(&self) -> &[EntityId] {
        &self.ids
    }
}

impl Command for AddEntities {
    fn execute(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        // Redo で 2 度目に実行されることがあるので、必ず作り直す。
        self.ids.clear();
        self.ids.reserve(self.entities.len());
        for e in &self.entities {
            self.ids.push(ctx.add_entity(e.clone()));
        }
        Ok(())
    }

    fn undo(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        // 追加と逆順に取り除く。
        for id in self.ids.iter().rev() {
            ctx.remove_entity(*id)?;
        }
        self.ids.clear();
        Ok(())
    }

    fn name(&self) -> &'static str {
        self.name
    }
}

/// エンティティをまとめて削除する。
#[derive(Debug)]
pub struct DeleteEntities {
    name: &'static str,
    targets: Vec<EntityId>,
    /// Undo で元の ID のまま戻すために、削除した中身を保持する。
    removed: Vec<(EntityId, Entity)>,
}

impl DeleteEntities {
    /// 削除対象を指定して作る。
    #[must_use]
    pub fn new(name: &'static str, targets: Vec<EntityId>) -> Self {
        Self {
            name,
            targets,
            removed: Vec::new(),
        }
    }
}

impl Command for DeleteEntities {
    fn execute(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        // 「全部成功するか、何も変えずに失敗するか」を守るため、
        // 途中で失敗したらそこまでの削除を戻してから Err を返す。
        self.removed.clear();
        for id in &self.targets {
            match ctx.remove_entity(*id) {
                Ok(e) => self.removed.push((*id, e)),
                Err(err) => {
                    for (rid, re) in self.removed.drain(..).rev() {
                        let _ = ctx.restore_entity(rid, re);
                    }
                    return Err(err);
                }
            }
        }
        Ok(())
    }

    fn undo(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        // 削除と逆順に、元の ID のまま戻す。
        for (id, entity) in self.removed.drain(..).rev() {
            ctx.restore_entity(id, entity)?;
        }
        Ok(())
    }

    fn name(&self) -> &'static str {
        self.name
    }
}
