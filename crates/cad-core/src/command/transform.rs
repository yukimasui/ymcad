//! 変形系のコマンド（MOVE / COPY / ROTATE / SCALE / MIRROR とそれぞれの複製版）。

use super::{Command, EditCtx};
use crate::entity::{EntityId, Geometry};
use crate::error::{CadError, Result};
use crate::geom::{Line, Point2, Vec2};

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

/// 選択した要素を `center` まわりに回転する。
#[derive(Debug)]
pub struct RotateEntities {
    name: &'static str,
    targets: Vec<EntityId>,
    center: Point2,
    angle: f64,
    /// Undo で元のジオメトリへ正確に戻すための退避先。
    originals: Vec<(EntityId, Geometry)>,
}

impl RotateEntities {
    /// 回転対象・中心・回転角を指定して作る。
    #[must_use]
    pub fn new(name: &'static str, targets: Vec<EntityId>, center: Point2, angle: f64) -> Self {
        Self {
            name,
            targets,
            center,
            angle,
            originals: Vec::new(),
        }
    }
}

impl Command for RotateEntities {
    fn execute(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        // Redo で 2 度目に実行されることがあるので、必ず作り直す。
        self.originals.clear();
        for id in &self.targets {
            match ctx.entity_mut(*id) {
                Ok(e) => {
                    self.originals.push((*id, e.geom.clone()));
                    e.geom = e.geom.rotated(self.center, self.angle);
                }
                Err(err) => {
                    // 「全部成功するか、何も変えずに失敗するか」を守るため、
                    // ここまでに回転した分を元のジオメトリへ戻してから Err を返す。
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
        // 退避しておいた元のジオメトリをそのまま書き戻す（逆回転の再計算はしない）。
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

/// 選択した要素を回転した複製を追加する（元の要素は残る）。
#[derive(Debug)]
pub struct RotateCopyEntities {
    name: &'static str,
    sources: Vec<EntityId>,
    center: Point2,
    angle: f64,
    /// 適用時に作られた要素の ID。
    created: Vec<EntityId>,
}

impl RotateCopyEntities {
    /// 複製元・中心・回転角を指定して作る。
    #[must_use]
    pub fn new(name: &'static str, sources: Vec<EntityId>, center: Point2, angle: f64) -> Self {
        Self {
            name,
            sources,
            center,
            angle,
            created: Vec::new(),
        }
    }

    /// 適用後に作られた要素の ID。適用前は空。
    #[must_use]
    pub fn created(&self) -> &[EntityId] {
        &self.created
    }
}

impl Command for RotateCopyEntities {
    fn execute(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        // Redo で 2 度目に実行されることがあるので、必ず作り直す。
        self.created.clear();
        for id in &self.sources {
            match ctx.entities().get(*id) {
                Some(e) => {
                    let copy = e.rotated(self.center, self.angle);
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

/// 選択した要素を `center` を中心に拡大縮小する。
#[derive(Debug)]
pub struct ScaleEntities {
    name: &'static str,
    targets: Vec<EntityId>,
    center: Point2,
    factor: f64,
    /// Undo で元のジオメトリへ正確に戻すための退避先。
    originals: Vec<(EntityId, Geometry)>,
}

impl ScaleEntities {
    /// 拡大縮小対象・中心・倍率を指定して作る。
    #[must_use]
    pub fn new(name: &'static str, targets: Vec<EntityId>, center: Point2, factor: f64) -> Self {
        Self {
            name,
            targets,
            center,
            factor,
            originals: Vec::new(),
        }
    }
}

impl Command for ScaleEntities {
    fn execute(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        // Redo で 2 度目に実行されることがあるので、必ず作り直す。
        self.originals.clear();
        for id in &self.targets {
            match ctx.entity_mut(*id) {
                Ok(e) => {
                    self.originals.push((*id, e.geom.clone()));
                    e.geom = e.geom.scaled(self.center, self.factor);
                }
                Err(err) => {
                    // 「全部成功するか、何も変えずに失敗するか」を守るため、
                    // ここまでに拡大縮小した分を元のジオメトリへ戻してから Err を返す。
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
        // 退避しておいた元のジオメトリをそのまま書き戻す（逆拡大縮小の再計算はしない）。
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

/// 選択した要素を拡大縮小した複製を追加する（元の要素は残る）。
#[derive(Debug)]
pub struct ScaleCopyEntities {
    name: &'static str,
    sources: Vec<EntityId>,
    center: Point2,
    factor: f64,
    /// 適用時に作られた要素の ID。
    created: Vec<EntityId>,
}

impl ScaleCopyEntities {
    /// 複製元・中心・倍率を指定して作る。
    #[must_use]
    pub fn new(name: &'static str, sources: Vec<EntityId>, center: Point2, factor: f64) -> Self {
        Self {
            name,
            sources,
            center,
            factor,
            created: Vec::new(),
        }
    }

    /// 適用後に作られた要素の ID。適用前は空。
    #[must_use]
    pub fn created(&self) -> &[EntityId] {
        &self.created
    }
}

impl Command for ScaleCopyEntities {
    fn execute(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        // Redo で 2 度目に実行されることがあるので、必ず作り直す。
        self.created.clear();
        for id in &self.sources {
            match ctx.entities().get(*id) {
                Some(e) => {
                    let copy = e.scaled(self.center, self.factor);
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

/// 選択した要素を `axis` で鏡像反転する（元の要素は変換される）。
#[derive(Debug)]
pub struct MirrorEntities {
    name: &'static str,
    targets: Vec<EntityId>,
    axis: Line,
    /// Undo で元のジオメトリへ正確に戻すための退避先。
    originals: Vec<(EntityId, Geometry)>,
}

impl MirrorEntities {
    /// 反転対象と鏡像軸を指定して作る。
    #[must_use]
    pub fn new(name: &'static str, targets: Vec<EntityId>, axis: Line) -> Self {
        Self {
            name,
            targets,
            axis,
            originals: Vec::new(),
        }
    }
}

impl Command for MirrorEntities {
    fn execute(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        // Redo で 2 度目に実行されることがあるので、必ず作り直す。
        self.originals.clear();
        for id in &self.targets {
            match ctx.entity_mut(*id) {
                Ok(e) => {
                    self.originals.push((*id, e.geom.clone()));
                    e.geom = e.geom.mirrored(&self.axis);
                }
                Err(err) => {
                    // 「全部成功するか、何も変えずに失敗するか」を守るため、
                    // ここまでに反転した分を元のジオメトリへ戻してから Err を返す。
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
        // 退避しておいた元のジオメトリをそのまま書き戻す（再反転はしない）。
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

/// 選択した要素の鏡像を複製として追加する（元の要素は残る）。
#[derive(Debug)]
pub struct MirrorCopyEntities {
    name: &'static str,
    sources: Vec<EntityId>,
    axis: Line,
    /// 適用時に作られた要素の ID。
    created: Vec<EntityId>,
}

impl MirrorCopyEntities {
    /// 複製元と鏡像軸を指定して作る。
    #[must_use]
    pub fn new(name: &'static str, sources: Vec<EntityId>, axis: Line) -> Self {
        Self {
            name,
            sources,
            axis,
            created: Vec::new(),
        }
    }

    /// 適用後に作られた要素の ID。適用前は空。
    #[must_use]
    pub fn created(&self) -> &[EntityId] {
        &self.created
    }
}

impl Command for MirrorCopyEntities {
    fn execute(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        // Redo で 2 度目に実行されることがあるので、必ず作り直す。
        self.created.clear();
        for id in &self.sources {
            match ctx.entities().get(*id) {
                Some(e) => {
                    let copy = e.mirrored(&self.axis);
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
    use crate::component::DefinitionTable;
    use crate::entity::{Entity, EntityStore};
    use crate::error::CadError;
    use crate::geom::tolerance::eq_len;
    use crate::geom::{Line, Point2};
    use crate::group::GroupTable;
    use crate::layer::{LayerId, LayerTable};
    use std::f64::consts::FRAC_PI_2;

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

    /// `Entity` から `Line` を取り出す（Rotate/Scale/Mirror のテストでは
    /// 両端点を確認する必要があるため、`line_x` より詳しい情報が要る）。
    fn line_of(e: &Entity) -> Line {
        match &e.geom {
            Geometry::Line(l) => *l,
            other => panic!("Line のはず: {other:?}"),
        }
    }

    /// 両端点をトレランス込みで比較する。
    fn assert_line_close(e: &Entity, a: (f64, f64), b: (f64, f64)) {
        let l = line_of(e);
        assert!(l.a.eq_tol(Point2::new(a.0, a.1)), "a: {:?} vs {:?}", l.a, a);
        assert!(l.b.eq_tol(Point2::new(b.0, b.1)), "b: {:?} vs {:?}", l.b, b);
    }

    /// X 軸そのものを表す線分（ミラー軸として使う）。
    fn x_axis() -> Line {
        Line::new(Point2::new(0.0, 0.0), Point2::new(1.0, 0.0))
    }

    /// テスト用の `EntityStore` / `LayerTable` の組。`EditCtx` はこの 2 つの
    /// `&mut` からしか作れない。
    fn new_parts() -> (EntityStore, LayerTable, GroupTable, DefinitionTable) {
        (
            EntityStore::new(),
            LayerTable::new(),
            GroupTable::new(),
            DefinitionTable::new(),
        )
    }

    #[test]
    fn move_execute_translates_and_keeps_id() {
        let (mut entities, mut layers, mut groups, mut definitions) = new_parts();
        let id = entities.insert(line_entity(1.0));

        let mut cmd = MoveEntities::new("MOVE", vec![id], Vec2::new(5.0, 0.0));
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups, &mut definitions);
        cmd.execute(&mut ctx).unwrap();

        let e = ctx.entities().get(id).expect("同じ ID で見つかるはず");
        assert!(eq_len(line_x(e), 6.0));
    }

    #[test]
    fn move_undo_restores_original_position_and_id() {
        let (mut entities, mut layers, mut groups, mut definitions) = new_parts();
        let id = entities.insert(line_entity(1.0));
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups, &mut definitions);

        let mut cmd = MoveEntities::new("MOVE", vec![id], Vec2::new(5.0, 0.0));
        cmd.execute(&mut ctx).unwrap();
        cmd.undo(&mut ctx).unwrap();

        let e = ctx.entities().get(id).expect("Undo 後も同じ ID のはず");
        assert!(eq_len(line_x(e), 1.0));
    }

    #[test]
    fn move_redo_after_undo_translates_again() {
        let (mut entities, mut layers, mut groups, mut definitions) = new_parts();
        let id = entities.insert(line_entity(1.0));
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups, &mut definitions);

        let mut cmd = MoveEntities::new("MOVE", vec![id], Vec2::new(5.0, 0.0));
        cmd.execute(&mut ctx).unwrap();
        cmd.undo(&mut ctx).unwrap();
        cmd.execute(&mut ctx).unwrap();

        let e = ctx.entities().get(id).expect("Redo 後も同じ ID のはず");
        assert!(eq_len(line_x(e), 6.0));
    }

    #[test]
    fn move_missing_target_fails_and_leaves_document_unchanged() {
        let (mut entities, mut layers, mut groups, mut definitions) = new_parts();
        let survivor = entities.insert(line_entity(1.0));
        let doomed = entities.insert(line_entity(2.0));
        entities.remove(doomed).unwrap();

        let mut cmd = MoveEntities::new("MOVE", vec![survivor, doomed], Vec2::new(100.0, 0.0));
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups, &mut definitions);
        let result = cmd.execute(&mut ctx);

        assert_eq!(result, Err(CadError::EntityNotFound));
        let e = ctx.entities().get(survivor).expect("生き残っているはず");
        assert!(eq_len(line_x(e), 1.0), "移動していないこと");
    }

    #[test]
    fn move_large_coordinates() {
        let (mut entities, mut layers, mut groups, mut definitions) = new_parts();
        let id = entities.insert(line_entity(1e6));
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups, &mut definitions);

        let mut cmd = MoveEntities::new("MOVE", vec![id], Vec2::new(1e6, 0.0));
        cmd.execute(&mut ctx).unwrap();
        assert!(eq_len(line_x(ctx.entities().get(id).unwrap()), 2e6));

        cmd.undo(&mut ctx).unwrap();
        assert!(eq_len(line_x(ctx.entities().get(id).unwrap()), 1e6));
    }

    #[test]
    fn copy_execute_creates_offset_entities() {
        let (mut entities, mut layers, mut groups, mut definitions) = new_parts();
        let a = entities.insert(line_entity(1.0));
        let b = entities.insert(line_entity(2.0));
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups, &mut definitions);

        let mut cmd = CopyEntities::new("COPY", vec![a, b], Vec2::new(10.0, 0.0));
        assert!(cmd.created().is_empty(), "適用前は空のはず");
        cmd.execute(&mut ctx).unwrap();

        assert_eq!(cmd.created().len(), 2);
        assert_eq!(ctx.entities().len(), 4, "元の 2 つ + 複製の 2 つ");
    }

    #[test]
    fn copy_created_ids_are_offset_and_originals_untouched() {
        let (mut entities, mut layers, mut groups, mut definitions) = new_parts();
        let a = entities.insert(line_entity(1.0));
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups, &mut definitions);

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
        let (mut entities, mut layers, mut groups, mut definitions) = new_parts();
        let a = entities.insert(line_entity(1.0));
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups, &mut definitions);

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
        let (mut entities, mut layers, mut groups, mut definitions) = new_parts();
        let a = entities.insert(line_entity(1.0));
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups, &mut definitions);

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
        let (mut entities, mut layers, mut groups, mut definitions) = new_parts();
        let a = entities.insert(line_entity(1.0));
        let doomed = entities.insert(line_entity(2.0));
        entities.remove(doomed).unwrap();
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups, &mut definitions);

        let mut cmd = CopyEntities::new("COPY", vec![a, doomed], Vec2::new(10.0, 0.0));
        let result = cmd.execute(&mut ctx);

        assert_eq!(result, Err(CadError::EntityNotFound));
        assert_eq!(ctx.entities().len(), 1, "複製は 1 つも残っていないはず");
        assert!(cmd.created().is_empty());
    }

    #[test]
    fn copy_large_coordinates() {
        let (mut entities, mut layers, mut groups, mut definitions) = new_parts();
        let a = entities.insert(line_entity(1e6));
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups, &mut definitions);

        let mut cmd = CopyEntities::new("COPY", vec![a], Vec2::new(1e6, 0.0));
        cmd.execute(&mut ctx).unwrap();

        assert!(eq_len(line_x(ctx.entities().get(a).unwrap()), 1e6));
        let created_id = cmd.created()[0];
        assert!(eq_len(line_x(ctx.entities().get(created_id).unwrap()), 2e6));
    }

    // ==== RotateEntities =====================================================

    #[test]
    fn rotate_execute_rotates_and_keeps_id() {
        let (mut entities, mut layers, mut groups, mut definitions) = new_parts();
        let id = entities.insert(line_entity(1.0));

        let mut cmd = RotateEntities::new("ROTATE", vec![id], Point2::new(0.0, 0.0), FRAC_PI_2);
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups, &mut definitions);
        cmd.execute(&mut ctx).unwrap();

        let e = ctx.entities().get(id).expect("同じ ID で見つかるはず");
        assert_line_close(e, (0.0, 1.0), (-1.0, 1.0));
    }

    #[test]
    fn rotate_undo_restores_original_position_and_id() {
        let (mut entities, mut layers, mut groups, mut definitions) = new_parts();
        let id = entities.insert(line_entity(1.0));
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups, &mut definitions);

        let mut cmd = RotateEntities::new("ROTATE", vec![id], Point2::new(0.0, 0.0), FRAC_PI_2);
        cmd.execute(&mut ctx).unwrap();
        cmd.undo(&mut ctx).unwrap();

        let e = ctx.entities().get(id).expect("Undo 後も同じ ID のはず");
        assert_line_close(e, (1.0, 0.0), (1.0, 1.0));
    }

    #[test]
    fn rotate_redo_after_undo_rotates_again() {
        let (mut entities, mut layers, mut groups, mut definitions) = new_parts();
        let id = entities.insert(line_entity(1.0));
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups, &mut definitions);

        let mut cmd = RotateEntities::new("ROTATE", vec![id], Point2::new(0.0, 0.0), FRAC_PI_2);
        cmd.execute(&mut ctx).unwrap();
        cmd.undo(&mut ctx).unwrap();
        cmd.execute(&mut ctx).unwrap();

        let e = ctx.entities().get(id).expect("Redo 後も同じ ID のはず");
        assert_line_close(e, (0.0, 1.0), (-1.0, 1.0));
    }

    #[test]
    fn rotate_missing_target_fails_and_leaves_document_unchanged() {
        let (mut entities, mut layers, mut groups, mut definitions) = new_parts();
        let survivor = entities.insert(line_entity(1.0));
        let doomed = entities.insert(line_entity(2.0));
        entities.remove(doomed).unwrap();

        let mut cmd = RotateEntities::new(
            "ROTATE",
            vec![survivor, doomed],
            Point2::new(0.0, 0.0),
            FRAC_PI_2,
        );
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups, &mut definitions);
        let result = cmd.execute(&mut ctx);

        assert_eq!(result, Err(CadError::EntityNotFound));
        let e = ctx.entities().get(survivor).expect("生き残っているはず");
        assert_line_close(e, (1.0, 0.0), (1.0, 1.0));
    }

    #[test]
    fn rotate_large_coordinates() {
        let (mut entities, mut layers, mut groups, mut definitions) = new_parts();
        let id = entities.insert(line_entity(1e6));
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups, &mut definitions);

        let mut cmd = RotateEntities::new("ROTATE", vec![id], Point2::new(0.0, 0.0), FRAC_PI_2);
        cmd.execute(&mut ctx).unwrap();
        assert_line_close(ctx.entities().get(id).unwrap(), (0.0, 1e6), (-1.0, 1e6));

        cmd.undo(&mut ctx).unwrap();
        assert_line_close(ctx.entities().get(id).unwrap(), (1e6, 0.0), (1e6, 1.0));
    }

    // ==== RotateCopyEntities ==================================================

    #[test]
    fn rotate_copy_execute_creates_rotated_entities() {
        let (mut entities, mut layers, mut groups, mut definitions) = new_parts();
        let a = entities.insert(line_entity(1.0));
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups, &mut definitions);

        let mut cmd = RotateCopyEntities::new("ROTATE", vec![a], Point2::new(0.0, 0.0), FRAC_PI_2);
        assert!(cmd.created().is_empty(), "適用前は空のはず");
        cmd.execute(&mut ctx).unwrap();

        assert_eq!(cmd.created().len(), 1);
        assert_eq!(ctx.entities().len(), 2, "元の 1 つ + 複製の 1 つ");
        let created_id = cmd.created()[0];
        assert_ne!(created_id, a, "複製は元とは別の ID を持つ");
        assert_line_close(
            ctx.entities().get(created_id).unwrap(),
            (0.0, 1.0),
            (-1.0, 1.0),
        );
    }

    #[test]
    fn rotate_copy_leaves_sources_untouched() {
        let (mut entities, mut layers, mut groups, mut definitions) = new_parts();
        let a = entities.insert(line_entity(1.0));
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups, &mut definitions);

        let mut cmd = RotateCopyEntities::new("ROTATE", vec![a], Point2::new(0.0, 0.0), FRAC_PI_2);
        cmd.execute(&mut ctx).unwrap();

        assert_line_close(ctx.entities().get(a).unwrap(), (1.0, 0.0), (1.0, 1.0));
    }

    #[test]
    fn rotate_copy_undo_removes_exactly_created_and_keeps_originals() {
        let (mut entities, mut layers, mut groups, mut definitions) = new_parts();
        let a = entities.insert(line_entity(1.0));
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups, &mut definitions);

        let mut cmd = RotateCopyEntities::new("ROTATE", vec![a], Point2::new(0.0, 0.0), FRAC_PI_2);
        cmd.execute(&mut ctx).unwrap();
        assert_eq!(ctx.entities().len(), 2);

        cmd.undo(&mut ctx).unwrap();
        assert_eq!(ctx.entities().len(), 1);
        assert!(ctx.entities().contains(a), "元の要素は残っているはず");
        assert!(cmd.created().is_empty(), "Undo 後は作成 ID を保持しない");
    }

    #[test]
    fn rotate_copy_redo_after_undo_creates_again() {
        let (mut entities, mut layers, mut groups, mut definitions) = new_parts();
        let a = entities.insert(line_entity(1.0));
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups, &mut definitions);

        let mut cmd = RotateCopyEntities::new("ROTATE", vec![a], Point2::new(0.0, 0.0), FRAC_PI_2);
        cmd.execute(&mut ctx).unwrap();
        let first_created = cmd.created()[0];

        cmd.undo(&mut ctx).unwrap();
        assert_eq!(ctx.entities().len(), 1);

        cmd.execute(&mut ctx).unwrap();
        assert_eq!(ctx.entities().len(), 2);
        assert!(ctx.entities().contains(cmd.created()[0]));
        assert_ne!(cmd.created()[0], first_created);
    }

    #[test]
    fn rotate_copy_missing_source_fails_and_creates_nothing() {
        let (mut entities, mut layers, mut groups, mut definitions) = new_parts();
        let a = entities.insert(line_entity(1.0));
        let doomed = entities.insert(line_entity(2.0));
        entities.remove(doomed).unwrap();
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups, &mut definitions);

        let mut cmd =
            RotateCopyEntities::new("ROTATE", vec![a, doomed], Point2::new(0.0, 0.0), FRAC_PI_2);
        let result = cmd.execute(&mut ctx);

        assert_eq!(result, Err(CadError::EntityNotFound));
        assert_eq!(ctx.entities().len(), 1, "複製は 1 つも残っていないはず");
        assert!(cmd.created().is_empty());
    }

    #[test]
    fn rotate_copy_large_coordinates() {
        let (mut entities, mut layers, mut groups, mut definitions) = new_parts();
        let a = entities.insert(line_entity(1e6));
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups, &mut definitions);

        let mut cmd = RotateCopyEntities::new("ROTATE", vec![a], Point2::new(0.0, 0.0), FRAC_PI_2);
        cmd.execute(&mut ctx).unwrap();

        assert_line_close(ctx.entities().get(a).unwrap(), (1e6, 0.0), (1e6, 1.0));
        let created_id = cmd.created()[0];
        assert_line_close(
            ctx.entities().get(created_id).unwrap(),
            (0.0, 1e6),
            (-1.0, 1e6),
        );
    }

    // ==== ScaleEntities =======================================================

    #[test]
    fn scale_execute_scales_and_keeps_id() {
        let (mut entities, mut layers, mut groups, mut definitions) = new_parts();
        let id = entities.insert(line_entity(2.0));

        let mut cmd = ScaleEntities::new("SCALE", vec![id], Point2::new(0.0, 0.0), 3.0);
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups, &mut definitions);
        cmd.execute(&mut ctx).unwrap();

        let e = ctx.entities().get(id).expect("同じ ID で見つかるはず");
        assert_line_close(e, (6.0, 0.0), (6.0, 3.0));
    }

    #[test]
    fn scale_undo_restores_original_position_and_id() {
        let (mut entities, mut layers, mut groups, mut definitions) = new_parts();
        let id = entities.insert(line_entity(2.0));
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups, &mut definitions);

        let mut cmd = ScaleEntities::new("SCALE", vec![id], Point2::new(0.0, 0.0), 3.0);
        cmd.execute(&mut ctx).unwrap();
        cmd.undo(&mut ctx).unwrap();

        let e = ctx.entities().get(id).expect("Undo 後も同じ ID のはず");
        assert_line_close(e, (2.0, 0.0), (2.0, 1.0));
    }

    #[test]
    fn scale_redo_after_undo_scales_again() {
        let (mut entities, mut layers, mut groups, mut definitions) = new_parts();
        let id = entities.insert(line_entity(2.0));
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups, &mut definitions);

        let mut cmd = ScaleEntities::new("SCALE", vec![id], Point2::new(0.0, 0.0), 3.0);
        cmd.execute(&mut ctx).unwrap();
        cmd.undo(&mut ctx).unwrap();
        cmd.execute(&mut ctx).unwrap();

        let e = ctx.entities().get(id).expect("Redo 後も同じ ID のはず");
        assert_line_close(e, (6.0, 0.0), (6.0, 3.0));
    }

    #[test]
    fn scale_missing_target_fails_and_leaves_document_unchanged() {
        let (mut entities, mut layers, mut groups, mut definitions) = new_parts();
        let survivor = entities.insert(line_entity(2.0));
        let doomed = entities.insert(line_entity(3.0));
        entities.remove(doomed).unwrap();

        let mut cmd =
            ScaleEntities::new("SCALE", vec![survivor, doomed], Point2::new(0.0, 0.0), 3.0);
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups, &mut definitions);
        let result = cmd.execute(&mut ctx);

        assert_eq!(result, Err(CadError::EntityNotFound));
        let e = ctx.entities().get(survivor).expect("生き残っているはず");
        assert_line_close(e, (2.0, 0.0), (2.0, 1.0));
    }

    #[test]
    fn scale_large_coordinates() {
        let (mut entities, mut layers, mut groups, mut definitions) = new_parts();
        let id = entities.insert(line_entity(1e6));
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups, &mut definitions);

        let mut cmd = ScaleEntities::new("SCALE", vec![id], Point2::new(0.0, 0.0), 2.0);
        cmd.execute(&mut ctx).unwrap();
        assert_line_close(ctx.entities().get(id).unwrap(), (2e6, 0.0), (2e6, 2.0));

        cmd.undo(&mut ctx).unwrap();
        assert_line_close(ctx.entities().get(id).unwrap(), (1e6, 0.0), (1e6, 1.0));
    }

    // ==== ScaleCopyEntities ===================================================

    #[test]
    fn scale_copy_execute_creates_scaled_entities() {
        let (mut entities, mut layers, mut groups, mut definitions) = new_parts();
        let a = entities.insert(line_entity(2.0));
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups, &mut definitions);

        let mut cmd = ScaleCopyEntities::new("SCALE", vec![a], Point2::new(0.0, 0.0), 3.0);
        assert!(cmd.created().is_empty(), "適用前は空のはず");
        cmd.execute(&mut ctx).unwrap();

        assert_eq!(cmd.created().len(), 1);
        assert_eq!(ctx.entities().len(), 2, "元の 1 つ + 複製の 1 つ");
        let created_id = cmd.created()[0];
        assert_ne!(created_id, a, "複製は元とは別の ID を持つ");
        assert_line_close(
            ctx.entities().get(created_id).unwrap(),
            (6.0, 0.0),
            (6.0, 3.0),
        );
    }

    #[test]
    fn scale_copy_leaves_sources_untouched() {
        let (mut entities, mut layers, mut groups, mut definitions) = new_parts();
        let a = entities.insert(line_entity(2.0));
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups, &mut definitions);

        let mut cmd = ScaleCopyEntities::new("SCALE", vec![a], Point2::new(0.0, 0.0), 3.0);
        cmd.execute(&mut ctx).unwrap();

        assert_line_close(ctx.entities().get(a).unwrap(), (2.0, 0.0), (2.0, 1.0));
    }

    #[test]
    fn scale_copy_undo_removes_exactly_created_and_keeps_originals() {
        let (mut entities, mut layers, mut groups, mut definitions) = new_parts();
        let a = entities.insert(line_entity(2.0));
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups, &mut definitions);

        let mut cmd = ScaleCopyEntities::new("SCALE", vec![a], Point2::new(0.0, 0.0), 3.0);
        cmd.execute(&mut ctx).unwrap();
        assert_eq!(ctx.entities().len(), 2);

        cmd.undo(&mut ctx).unwrap();
        assert_eq!(ctx.entities().len(), 1);
        assert!(ctx.entities().contains(a), "元の要素は残っているはず");
        assert!(cmd.created().is_empty(), "Undo 後は作成 ID を保持しない");
    }

    #[test]
    fn scale_copy_redo_after_undo_creates_again() {
        let (mut entities, mut layers, mut groups, mut definitions) = new_parts();
        let a = entities.insert(line_entity(2.0));
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups, &mut definitions);

        let mut cmd = ScaleCopyEntities::new("SCALE", vec![a], Point2::new(0.0, 0.0), 3.0);
        cmd.execute(&mut ctx).unwrap();
        let first_created = cmd.created()[0];

        cmd.undo(&mut ctx).unwrap();
        assert_eq!(ctx.entities().len(), 1);

        cmd.execute(&mut ctx).unwrap();
        assert_eq!(ctx.entities().len(), 2);
        assert!(ctx.entities().contains(cmd.created()[0]));
        assert_ne!(cmd.created()[0], first_created);
    }

    #[test]
    fn scale_copy_missing_source_fails_and_creates_nothing() {
        let (mut entities, mut layers, mut groups, mut definitions) = new_parts();
        let a = entities.insert(line_entity(2.0));
        let doomed = entities.insert(line_entity(3.0));
        entities.remove(doomed).unwrap();
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups, &mut definitions);

        let mut cmd = ScaleCopyEntities::new("SCALE", vec![a, doomed], Point2::new(0.0, 0.0), 3.0);
        let result = cmd.execute(&mut ctx);

        assert_eq!(result, Err(CadError::EntityNotFound));
        assert_eq!(ctx.entities().len(), 1, "複製は 1 つも残っていないはず");
        assert!(cmd.created().is_empty());
    }

    #[test]
    fn scale_copy_large_coordinates() {
        let (mut entities, mut layers, mut groups, mut definitions) = new_parts();
        let a = entities.insert(line_entity(1e6));
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups, &mut definitions);

        let mut cmd = ScaleCopyEntities::new("SCALE", vec![a], Point2::new(0.0, 0.0), 2.0);
        cmd.execute(&mut ctx).unwrap();

        assert_line_close(ctx.entities().get(a).unwrap(), (1e6, 0.0), (1e6, 1.0));
        let created_id = cmd.created()[0];
        assert_line_close(
            ctx.entities().get(created_id).unwrap(),
            (2e6, 0.0),
            (2e6, 2.0),
        );
    }

    // ==== MirrorEntities ======================================================

    #[test]
    fn mirror_execute_mirrors_and_keeps_id() {
        let (mut entities, mut layers, mut groups, mut definitions) = new_parts();
        let id = entities.insert(line_entity(2.0));

        let mut cmd = MirrorEntities::new("MIRROR", vec![id], x_axis());
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups, &mut definitions);
        cmd.execute(&mut ctx).unwrap();

        let e = ctx.entities().get(id).expect("同じ ID で見つかるはず");
        assert_line_close(e, (2.0, 0.0), (2.0, -1.0));
    }

    #[test]
    fn mirror_undo_restores_original_position_and_id() {
        let (mut entities, mut layers, mut groups, mut definitions) = new_parts();
        let id = entities.insert(line_entity(2.0));
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups, &mut definitions);

        let mut cmd = MirrorEntities::new("MIRROR", vec![id], x_axis());
        cmd.execute(&mut ctx).unwrap();
        cmd.undo(&mut ctx).unwrap();

        let e = ctx.entities().get(id).expect("Undo 後も同じ ID のはず");
        assert_line_close(e, (2.0, 0.0), (2.0, 1.0));
    }

    #[test]
    fn mirror_redo_after_undo_mirrors_again() {
        let (mut entities, mut layers, mut groups, mut definitions) = new_parts();
        let id = entities.insert(line_entity(2.0));
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups, &mut definitions);

        let mut cmd = MirrorEntities::new("MIRROR", vec![id], x_axis());
        cmd.execute(&mut ctx).unwrap();
        cmd.undo(&mut ctx).unwrap();
        cmd.execute(&mut ctx).unwrap();

        let e = ctx.entities().get(id).expect("Redo 後も同じ ID のはず");
        assert_line_close(e, (2.0, 0.0), (2.0, -1.0));
    }

    #[test]
    fn mirror_missing_target_fails_and_leaves_document_unchanged() {
        let (mut entities, mut layers, mut groups, mut definitions) = new_parts();
        let survivor = entities.insert(line_entity(2.0));
        let doomed = entities.insert(line_entity(3.0));
        entities.remove(doomed).unwrap();

        let mut cmd = MirrorEntities::new("MIRROR", vec![survivor, doomed], x_axis());
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups, &mut definitions);
        let result = cmd.execute(&mut ctx);

        assert_eq!(result, Err(CadError::EntityNotFound));
        let e = ctx.entities().get(survivor).expect("生き残っているはず");
        assert_line_close(e, (2.0, 0.0), (2.0, 1.0));
    }

    #[test]
    fn mirror_large_coordinates() {
        let (mut entities, mut layers, mut groups, mut definitions) = new_parts();
        let id = entities.insert(line_entity(1e6));
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups, &mut definitions);

        let mut cmd = MirrorEntities::new("MIRROR", vec![id], x_axis());
        cmd.execute(&mut ctx).unwrap();
        assert_line_close(ctx.entities().get(id).unwrap(), (1e6, 0.0), (1e6, -1.0));

        cmd.undo(&mut ctx).unwrap();
        assert_line_close(ctx.entities().get(id).unwrap(), (1e6, 0.0), (1e6, 1.0));
    }

    // ==== MirrorCopyEntities ==================================================

    #[test]
    fn mirror_copy_execute_creates_mirrored_entities() {
        let (mut entities, mut layers, mut groups, mut definitions) = new_parts();
        let a = entities.insert(line_entity(1.0));
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups, &mut definitions);

        let mut cmd = MirrorCopyEntities::new("MIRROR", vec![a], x_axis());
        assert!(cmd.created().is_empty(), "適用前は空のはず");
        cmd.execute(&mut ctx).unwrap();

        assert_eq!(cmd.created().len(), 1);
        assert_eq!(ctx.entities().len(), 2, "元の 1 つ + 複製の 1 つ");
        let created_id = cmd.created()[0];
        assert_ne!(created_id, a, "複製は元とは別の ID を持つ");
        assert_line_close(
            ctx.entities().get(created_id).unwrap(),
            (1.0, 0.0),
            (1.0, -1.0),
        );
    }

    #[test]
    fn mirror_copy_leaves_sources_untouched() {
        let (mut entities, mut layers, mut groups, mut definitions) = new_parts();
        let a = entities.insert(line_entity(1.0));
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups, &mut definitions);

        let mut cmd = MirrorCopyEntities::new("MIRROR", vec![a], x_axis());
        cmd.execute(&mut ctx).unwrap();

        assert_line_close(ctx.entities().get(a).unwrap(), (1.0, 0.0), (1.0, 1.0));
    }

    #[test]
    fn mirror_copy_undo_removes_exactly_created_and_keeps_originals() {
        let (mut entities, mut layers, mut groups, mut definitions) = new_parts();
        let a = entities.insert(line_entity(1.0));
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups, &mut definitions);

        let mut cmd = MirrorCopyEntities::new("MIRROR", vec![a], x_axis());
        cmd.execute(&mut ctx).unwrap();
        assert_eq!(ctx.entities().len(), 2);

        cmd.undo(&mut ctx).unwrap();
        assert_eq!(ctx.entities().len(), 1);
        assert!(ctx.entities().contains(a), "元の要素は残っているはず");
        assert!(cmd.created().is_empty(), "Undo 後は作成 ID を保持しない");
    }

    #[test]
    fn mirror_copy_redo_after_undo_creates_again() {
        let (mut entities, mut layers, mut groups, mut definitions) = new_parts();
        let a = entities.insert(line_entity(1.0));
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups, &mut definitions);

        let mut cmd = MirrorCopyEntities::new("MIRROR", vec![a], x_axis());
        cmd.execute(&mut ctx).unwrap();
        let first_created = cmd.created()[0];

        cmd.undo(&mut ctx).unwrap();
        assert_eq!(ctx.entities().len(), 1);

        cmd.execute(&mut ctx).unwrap();
        assert_eq!(ctx.entities().len(), 2);
        assert!(ctx.entities().contains(cmd.created()[0]));
        assert_ne!(cmd.created()[0], first_created);
    }

    #[test]
    fn mirror_copy_missing_source_fails_and_creates_nothing() {
        let (mut entities, mut layers, mut groups, mut definitions) = new_parts();
        let a = entities.insert(line_entity(1.0));
        let doomed = entities.insert(line_entity(2.0));
        entities.remove(doomed).unwrap();
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups, &mut definitions);

        let mut cmd = MirrorCopyEntities::new("MIRROR", vec![a, doomed], x_axis());
        let result = cmd.execute(&mut ctx);

        assert_eq!(result, Err(CadError::EntityNotFound));
        assert_eq!(ctx.entities().len(), 1, "複製は 1 つも残っていないはず");
        assert!(cmd.created().is_empty());
    }

    #[test]
    fn mirror_copy_large_coordinates() {
        let (mut entities, mut layers, mut groups, mut definitions) = new_parts();
        let a = entities.insert(line_entity(1e6));
        let mut ctx = EditCtx::new(&mut entities, &mut layers, &mut groups, &mut definitions);

        let mut cmd = MirrorCopyEntities::new("MIRROR", vec![a], x_axis());
        cmd.execute(&mut ctx).unwrap();

        assert_line_close(ctx.entities().get(a).unwrap(), (1e6, 0.0), (1e6, 1.0));
        let created_id = cmd.created()[0];
        assert_line_close(
            ctx.entities().get(created_id).unwrap(),
            (1e6, 0.0),
            (1e6, -1.0),
        );
    }
}
