//! 平行移動系のコマンド（MOVE / COPY）。

use super::{Command, EditCtx};
use crate::entity::{EntityId, Geometry};
use crate::error::{CadError, Result};
use crate::geom::Vec2;

/// 選択した要素を平行移動する。
#[derive(Debug)]
pub struct MoveEntities {
    name: &'static str,
    targets: Vec<EntityId>,
    delta: Vec2,
    /// Undo で元のジオメトリへ正確に戻すための退避先。
    originals: Vec<(EntityId, Geometry)>,
}

impl MoveEntities {
    /// 移動対象と移動量を指定して作る。
    #[must_use]
    pub fn new(name: &'static str, targets: Vec<EntityId>, delta: Vec2) -> Self {
        Self {
            name,
            targets,
            delta,
            originals: Vec::new(),
        }
    }
}

impl Command for MoveEntities {
    fn execute(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        // Redo で 2 度目に実行されることがあるので、必ず作り直す。
        self.originals.clear();
        for id in &self.targets {
            match ctx.entity_mut(*id) {
                Ok(e) => {
                    self.originals.push((*id, e.geom.clone()));
                    e.geom = e.geom.translated(self.delta);
                }
                Err(err) => {
                    // 「全部成功するか、何も変えずに失敗するか」を守るため、
                    // ここまでに移動した分を元のジオメトリへ戻してから Err を返す。
                    for (rid, rg) in self.originals.drain(..) {
                        if let Ok(e) = ctx.entity_mut(rid) {
                            e.geom = rg;
                        }
                    }
                    return Err(err);
                }
            }
        }
        Ok(())
    }

    fn undo(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        // 退避しておいた元のジオメトリをそのまま書き戻す（`-delta` の再計算はしない。
        // 浮動小数点の往復誤差を一切持ち込まないため）。
        for (id, geom) in self.originals.drain(..) {
            let e = ctx.entity_mut(id)?;
            e.geom = geom;
        }
        Ok(())
    }

    fn name(&self) -> &'static str {
        self.name
    }
}

/// 選択した要素を平行移動した複製を作る。
#[derive(Debug)]
pub struct CopyEntities {
    name: &'static str,
    sources: Vec<EntityId>,
    delta: Vec2,
    /// 適用時に作られた要素の ID。
    created: Vec<EntityId>,
}

impl CopyEntities {
    /// 複製元と移動量を指定して作る。
    #[must_use]
    pub fn new(name: &'static str, sources: Vec<EntityId>, delta: Vec2) -> Self {
        Self {
            name,
            sources,
            delta,
            created: Vec::new(),
        }
    }

    /// 適用後に作られた要素の ID。適用前は空。
    #[must_use]
    pub fn created(&self) -> &[EntityId] {
        &self.created
    }
}

impl Command for CopyEntities {
    fn execute(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        // Redo で 2 度目に実行されることがあるので、必ず作り直す。
        self.created.clear();
        for id in &self.sources {
            match ctx.entities().get(*id) {
                Some(e) => {
                    let copy = e.translated(self.delta);
                    self.created.push(ctx.add_entity(copy));
                }
                None => {
                    // ここまでに作った複製を取り除いてから Err を返す（all-or-nothing）。
                    for cid in self.created.drain(..).rev() {
                        let _ = ctx.remove_entity(cid);
                    }
                    return Err(CadError::EntityNotFound);
                }
            }
        }
        Ok(())
    }

    fn undo(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        // 作成と逆順に取り除く。
        for id in self.created.drain(..).rev() {
            ctx.remove_entity(id)?;
        }
        Ok(())
    }

    fn name(&self) -> &'static str {
        self.name
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::{Entity, EntityStore};
    use crate::error::CadError;
    use crate::geom::tolerance::eq_len;
    use crate::geom::{Line, Point2};
    use crate::layer::{LayerId, LayerTable};

    fn line_entity(x: f64) -> Entity {
        Entity::new(
            Geometry::Line(Line::new(Point2::new(x, 0.0), Point2::new(x, 1.0))),
            LayerId::ZERO,
        )
    }

    fn line_x(e: &Entity) -> f64 {
        match &e.geom {
            Geometry::Line(l) => l.a.x,
            other => panic!("Line のはず: {other:?}"),
        }
    }

    /// テスト用の `EntityStore` / `LayerTable` の組。`EditCtx` はこの 2 つの
    /// `&mut` からしか作れない。
    fn new_parts() -> (EntityStore, LayerTable) {
        (EntityStore::new(), LayerTable::new())
    }

    #[test]
    fn move_execute_translates_and_keeps_id() {
        let (mut entities, mut layers) = new_parts();
        let id = entities.insert(line_entity(1.0));

        let mut cmd = MoveEntities::new("MOVE", vec![id], Vec2::new(5.0, 0.0));
        let mut ctx = EditCtx::new(&mut entities, &mut layers);
        cmd.execute(&mut ctx).unwrap();

        let e = ctx.entities().get(id).expect("同じ ID で見つかるはず");
        assert!(eq_len(line_x(e), 6.0));
    }

    #[test]
    fn move_undo_restores_original_position_and_id() {
        let (mut entities, mut layers) = new_parts();
        let id = entities.insert(line_entity(1.0));
        let mut ctx = EditCtx::new(&mut entities, &mut layers);

        let mut cmd = MoveEntities::new("MOVE", vec![id], Vec2::new(5.0, 0.0));
        cmd.execute(&mut ctx).unwrap();
        cmd.undo(&mut ctx).unwrap();

        let e = ctx.entities().get(id).expect("Undo 後も同じ ID のはず");
        assert!(eq_len(line_x(e), 1.0));
    }

    #[test]
    fn move_redo_after_undo_translates_again() {
        let (mut entities, mut layers) = new_parts();
        let id = entities.insert(line_entity(1.0));
        let mut ctx = EditCtx::new(&mut entities, &mut layers);

        let mut cmd = MoveEntities::new("MOVE", vec![id], Vec2::new(5.0, 0.0));
        cmd.execute(&mut ctx).unwrap();
        cmd.undo(&mut ctx).unwrap();
        cmd.execute(&mut ctx).unwrap();

        let e = ctx.entities().get(id).expect("Redo 後も同じ ID のはず");
        assert!(eq_len(line_x(e), 6.0));
    }

    #[test]
    fn move_missing_target_fails_and_leaves_document_unchanged() {
        let (mut entities, mut layers) = new_parts();
        let survivor = entities.insert(line_entity(1.0));
        let doomed = entities.insert(line_entity(2.0));
        entities.remove(doomed).unwrap();

        let mut cmd = MoveEntities::new("MOVE", vec![survivor, doomed], Vec2::new(100.0, 0.0));
        let mut ctx = EditCtx::new(&mut entities, &mut layers);
        let result = cmd.execute(&mut ctx);

        assert_eq!(result, Err(CadError::EntityNotFound));
        let e = ctx.entities().get(survivor).expect("生き残っているはず");
        assert!(eq_len(line_x(e), 1.0), "移動していないこと");
    }

    #[test]
    fn move_large_coordinates() {
        let (mut entities, mut layers) = new_parts();
        let id = entities.insert(line_entity(1e6));
        let mut ctx = EditCtx::new(&mut entities, &mut layers);

        let mut cmd = MoveEntities::new("MOVE", vec![id], Vec2::new(1e6, 0.0));
        cmd.execute(&mut ctx).unwrap();
        assert!(eq_len(line_x(ctx.entities().get(id).unwrap()), 2e6));

        cmd.undo(&mut ctx).unwrap();
        assert!(eq_len(line_x(ctx.entities().get(id).unwrap()), 1e6));
    }

    #[test]
    fn copy_execute_creates_offset_entities() {
        let (mut entities, mut layers) = new_parts();
        let a = entities.insert(line_entity(1.0));
        let b = entities.insert(line_entity(2.0));
        let mut ctx = EditCtx::new(&mut entities, &mut layers);

        let mut cmd = CopyEntities::new("COPY", vec![a, b], Vec2::new(10.0, 0.0));
        assert!(cmd.created().is_empty(), "適用前は空のはず");
        cmd.execute(&mut ctx).unwrap();

        assert_eq!(cmd.created().len(), 2);
        assert_eq!(ctx.entities().len(), 4, "元の 2 つ + 複製の 2 つ");
    }

    #[test]
    fn copy_created_ids_are_offset_and_originals_untouched() {
        let (mut entities, mut layers) = new_parts();
        let a = entities.insert(line_entity(1.0));
        let mut ctx = EditCtx::new(&mut entities, &mut layers);

        let mut cmd = CopyEntities::new("COPY", vec![a], Vec2::new(10.0, 0.0));
        cmd.execute(&mut ctx).unwrap();

        assert!(eq_len(line_x(ctx.entities().get(a).unwrap()), 1.0));
        let created_id = cmd.created()[0];
        assert_ne!(created_id, a, "複製は元とは別の ID を持つ");
        assert!(eq_len(
            line_x(ctx.entities().get(created_id).unwrap()),
            11.0
        ));
    }

    #[test]
    fn copy_undo_removes_exactly_created_and_keeps_originals() {
        let (mut entities, mut layers) = new_parts();
        let a = entities.insert(line_entity(1.0));
        let mut ctx = EditCtx::new(&mut entities, &mut layers);

        let mut cmd = CopyEntities::new("COPY", vec![a], Vec2::new(10.0, 0.0));
        cmd.execute(&mut ctx).unwrap();
        assert_eq!(ctx.entities().len(), 2);

        cmd.undo(&mut ctx).unwrap();
        assert_eq!(ctx.entities().len(), 1);
        assert!(ctx.entities().contains(a), "元の要素は残っているはず");
        assert!(cmd.created().is_empty(), "Undo 後は作成 ID を保持しない");
    }

    #[test]
    fn copy_execute_undo_execute_redo_path_works() {
        let (mut entities, mut layers) = new_parts();
        let a = entities.insert(line_entity(1.0));
        let mut ctx = EditCtx::new(&mut entities, &mut layers);

        let mut cmd = CopyEntities::new("COPY", vec![a], Vec2::new(10.0, 0.0));
        cmd.execute(&mut ctx).unwrap();
        assert_eq!(ctx.entities().len(), 2);
        let first_created = cmd.created()[0];

        cmd.undo(&mut ctx).unwrap();
        assert_eq!(ctx.entities().len(), 1);

        // Redo: 再度 execute しても新しい複製が作られること。
        cmd.execute(&mut ctx).unwrap();
        assert_eq!(ctx.entities().len(), 2);
        assert!(ctx.entities().contains(cmd.created()[0]));
        // スロットは再利用しない方針なので、新しい ID は前回のものとは異なる。
        assert_ne!(cmd.created()[0], first_created);
    }

    #[test]
    fn copy_missing_source_fails_and_creates_nothing() {
        let (mut entities, mut layers) = new_parts();
        let a = entities.insert(line_entity(1.0));
        let doomed = entities.insert(line_entity(2.0));
        entities.remove(doomed).unwrap();
        let mut ctx = EditCtx::new(&mut entities, &mut layers);

        let mut cmd = CopyEntities::new("COPY", vec![a, doomed], Vec2::new(10.0, 0.0));
        let result = cmd.execute(&mut ctx);

        assert_eq!(result, Err(CadError::EntityNotFound));
        assert_eq!(ctx.entities().len(), 1, "複製は 1 つも残っていないはず");
        assert!(cmd.created().is_empty());
    }

    #[test]
    fn copy_large_coordinates() {
        let (mut entities, mut layers) = new_parts();
        let a = entities.insert(line_entity(1e6));
        let mut ctx = EditCtx::new(&mut entities, &mut layers);

        let mut cmd = CopyEntities::new("COPY", vec![a], Vec2::new(1e6, 0.0));
        cmd.execute(&mut ctx).unwrap();

        assert!(eq_len(line_x(ctx.entities().get(a).unwrap()), 1e6));
        let created_id = cmd.created()[0];
        assert!(eq_len(line_x(ctx.entities().get(created_id).unwrap()), 2e6));
    }
}
