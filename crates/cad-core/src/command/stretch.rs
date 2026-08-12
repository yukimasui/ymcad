//! 範囲選択の中に入っている定義点だけを動かす STRETCH コマンド。

use super::{Command, EditCtx};
use crate::entity::{EntityId, Geometry};
use crate::error::Result;
use crate::geom::{Aabb, Vec2};

/// 選択した要素のうち、指定範囲に入っている定義点だけを平行移動する。
#[derive(Debug)]
pub struct StretchEntities {
    name: &'static str,
    targets: Vec<EntityId>,
    /// ストレッチ範囲。空なら図形全体を平行移動する。
    regions: Vec<Aabb>,
    delta: Vec2,
    /// Undo 用に実行前の Geometry を控える。
    originals: Vec<(EntityId, Geometry)>,
}

impl StretchEntities {
    /// 対象・範囲・移動量を指定して作る。
    #[must_use]
    pub fn new(
        name: &'static str,
        targets: Vec<EntityId>,
        regions: Vec<Aabb>,
        delta: Vec2,
    ) -> Self {
        Self {
            name,
            targets,
            regions,
            delta,
            originals: Vec::new(),
        }
    }
}

impl Command for StretchEntities {
    fn execute(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        // Redo で 2 度目に実行されることがあるので、必ず作り直す。
        self.originals.clear();
        for id in &self.targets {
            match ctx.entity_mut(*id) {
                Ok(e) => {
                    self.originals.push((*id, e.geom.clone()));
                    e.geom = e.geom.stretched(&self.regions, self.delta);
                }
                Err(err) => {
                    // 「全部成功するか、何も変えずに失敗するか」を守るため、
                    // ここまでに動かした分を元のジオメトリへ戻してから Err を返す。
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
        // 浮動小数点の往復誤差を一切持ち込まないため）。逆順に戻す。
        for (id, geom) in self.originals.drain(..).rev() {
            let e = ctx.entity_mut(id)?;
            e.geom = geom;
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
    use crate::group::GroupTable;
    use crate::layer::{LayerId, LayerTable};

    fn line_entity(ax: f64, ay: f64, bx: f64, by: f64) -> Entity {
        Entity::new(
            Geometry::Line(Line::new(Point2::new(ax, ay), Point2::new(bx, by))),
            LayerId::ZERO,
        )
    }

    fn line_coords(e: &Entity) -> (Point2, Point2) {
        match &e.geom {
            Geometry::Line(l) => (l.a, l.b),
            other => panic!("Line のはず: {other:?}"),
        }
    }

    /// テスト用の `EntityStore` / `LayerTable` の組。`EditCtx` はこの 2 つの
    /// `&mut` からしか作れない。
    fn new_parts() -> (EntityStore, LayerTable, GroupTable) {
        (EntityStore::new(), LayerTable::new(), GroupTable::new())
    }

    /// `(0,0)-(10,10)` の範囲。
    fn region() -> Aabb {
        Aabb::new(Point2::new(0.0, 0.0), Point2::new(10.0, 10.0))
    }

    #[test]
    fn stretch_execute_moves_only_points_inside_region() {
        let (mut entities, mut layers, mut groups) = new_parts();
        // a は region 内、b は外。
        let id = entities.insert(line_entity(1.0, 1.0, 50.0, 50.0));
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups);

        let mut cmd =
            StretchEntities::new("STRETCH", vec![id], vec![region()], Vec2::new(5.0, 5.0));
        cmd.execute(&mut ctx).unwrap();

        let (a, b) = line_coords(ctx.entities().get(id).unwrap());
        assert!(eq_len(a.x, 6.0) && eq_len(a.y, 6.0));
        assert!(eq_len(b.x, 50.0) && eq_len(b.y, 50.0));
    }

    #[test]
    fn stretch_undo_restores_original_geometry_exactly() {
        let (mut entities, mut layers, mut groups) = new_parts();
        let id = entities.insert(line_entity(1.0, 1.0, 50.0, 50.0));
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups);
        let original = ctx.entities().get(id).unwrap().geom.clone();

        let mut cmd =
            StretchEntities::new("STRETCH", vec![id], vec![region()], Vec2::new(5.0, 5.0));
        cmd.execute(&mut ctx).unwrap();
        cmd.undo(&mut ctx).unwrap();

        assert_eq!(ctx.entities().get(id).unwrap().geom, original);
    }

    #[test]
    fn stretch_redo_after_undo_moves_again() {
        let (mut entities, mut layers, mut groups) = new_parts();
        let id = entities.insert(line_entity(1.0, 1.0, 50.0, 50.0));
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups);

        let mut cmd =
            StretchEntities::new("STRETCH", vec![id], vec![region()], Vec2::new(5.0, 5.0));
        cmd.execute(&mut ctx).unwrap();
        cmd.undo(&mut ctx).unwrap();
        cmd.execute(&mut ctx).unwrap();

        let (a, b) = line_coords(ctx.entities().get(id).unwrap());
        assert!(eq_len(a.x, 6.0) && eq_len(a.y, 6.0));
        assert!(eq_len(b.x, 50.0) && eq_len(b.y, 50.0));
    }

    #[test]
    fn stretch_missing_target_fails_and_leaves_document_unchanged() {
        let (mut entities, mut layers, mut groups) = new_parts();
        let survivor = entities.insert(line_entity(1.0, 1.0, 50.0, 50.0));
        let doomed = entities.insert(line_entity(2.0, 2.0, 3.0, 3.0));
        entities.remove(doomed).unwrap();
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups);

        let mut cmd = StretchEntities::new(
            "STRETCH",
            vec![survivor, doomed],
            vec![region()],
            Vec2::new(5.0, 5.0),
        );
        let result = cmd.execute(&mut ctx);

        assert_eq!(result, Err(CadError::EntityNotFound));
        let (a, b) = line_coords(ctx.entities().get(survivor).unwrap());
        assert!(eq_len(a.x, 1.0) && eq_len(a.y, 1.0), "移動していないこと");
        assert!(eq_len(b.x, 50.0) && eq_len(b.y, 50.0), "移動していないこと");
    }

    #[test]
    fn stretch_empty_regions_behaves_like_move() {
        let (mut entities, mut layers, mut groups) = new_parts();
        let id = entities.insert(line_entity(1.0, 1.0, 50.0, 50.0));
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups);

        let mut cmd = StretchEntities::new("STRETCH", vec![id], vec![], Vec2::new(5.0, 5.0));
        cmd.execute(&mut ctx).unwrap();

        let (a, b) = line_coords(ctx.entities().get(id).unwrap());
        assert!(eq_len(a.x, 6.0) && eq_len(a.y, 6.0));
        assert!(eq_len(b.x, 55.0) && eq_len(b.y, 55.0));
    }

    #[test]
    fn stretch_keeps_entity_id() {
        let (mut entities, mut layers, mut groups) = new_parts();
        let id = entities.insert(line_entity(1.0, 1.0, 50.0, 50.0));
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups);

        let mut cmd =
            StretchEntities::new("STRETCH", vec![id], vec![region()], Vec2::new(5.0, 5.0));
        cmd.execute(&mut ctx).unwrap();

        assert!(ctx.entities().contains(id));
        assert_eq!(ctx.entities().get(id).unwrap().layer, LayerId::ZERO);
    }

    #[test]
    fn stretch_multiple_regions_moves_points_inside_either() {
        let (mut entities, mut layers, mut groups) = new_parts();
        // a は region() 内、b は other 内。
        let other = Aabb::new(Point2::new(100.0, 100.0), Point2::new(110.0, 110.0));
        let id = entities.insert(line_entity(1.0, 1.0, 105.0, 105.0));
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups);

        let mut cmd = StretchEntities::new(
            "STRETCH",
            vec![id],
            vec![region(), other],
            Vec2::new(1.0, 1.0),
        );
        cmd.execute(&mut ctx).unwrap();

        let (a, b) = line_coords(ctx.entities().get(id).unwrap());
        assert!(eq_len(a.x, 2.0) && eq_len(a.y, 2.0));
        assert!(eq_len(b.x, 106.0) && eq_len(b.y, 106.0));
    }

    #[test]
    fn stretch_large_coordinates() {
        let (mut entities, mut layers, mut groups) = new_parts();
        let big_region = Aabb::new(Point2::new(0.0, 0.0), Point2::new(2.0e6, 2.0e6));
        let id = entities.insert(line_entity(1.0e6, 1.0e6, 5.0e6, 5.0e6));
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups);

        let mut cmd =
            StretchEntities::new("STRETCH", vec![id], vec![big_region], Vec2::new(1.0e6, 0.0));
        cmd.execute(&mut ctx).unwrap();

        let (a, b) = line_coords(ctx.entities().get(id).unwrap());
        assert!(eq_len(a.x, 2.0e6) && eq_len(a.y, 1.0e6));
        assert!(eq_len(b.x, 5.0e6) && eq_len(b.y, 5.0e6));

        cmd.undo(&mut ctx).unwrap();
        let (a2, _b2) = line_coords(ctx.entities().get(id).unwrap());
        assert!(eq_len(a2.x, 1.0e6) && eq_len(a2.y, 1.0e6));
    }

    #[test]
    fn stretch_partial_rollback_with_multiple_targets_on_missing_id() {
        let (mut entities, mut layers, mut groups) = new_parts();
        let first = entities.insert(line_entity(1.0, 1.0, 50.0, 50.0));
        let second = entities.insert(line_entity(2.0, 2.0, 60.0, 60.0));
        let doomed = entities.insert(line_entity(3.0, 3.0, 70.0, 70.0));
        entities.remove(doomed).unwrap();
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups);

        let mut cmd = StretchEntities::new(
            "STRETCH",
            vec![first, second, doomed],
            vec![region()],
            Vec2::new(5.0, 5.0),
        );
        let result = cmd.execute(&mut ctx);

        assert_eq!(result, Err(CadError::EntityNotFound));
        let (a1, _) = line_coords(ctx.entities().get(first).unwrap());
        let (a2, _) = line_coords(ctx.entities().get(second).unwrap());
        assert!(
            eq_len(a1.x, 1.0) && eq_len(a1.y, 1.0),
            "1 つ目も戻っていること"
        );
        assert!(
            eq_len(a2.x, 2.0) && eq_len(a2.y, 2.0),
            "2 つ目も戻っていること"
        );
    }
}
