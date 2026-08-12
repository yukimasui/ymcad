//! 図形そのものを切り貼りするコマンド（TRIM / EXTEND / FILLET / CHAMFER）。

use super::{Command, EditCtx};
use crate::entity::{Entity, EntityId, Geometry};
use crate::error::{CadError, Result};
use crate::geom::corner::{chamfer, fillet, CornerResult};
use crate::geom::intersect::{line_params_against, line_params_extended};
use crate::geom::tolerance::{eq_len, is_zero_len};
use crate::geom::{Line, Point2};

/// 線分を切る位置を求める。
///
/// `at` を含む区間を、両隣の交点で切り落とした結果を返す。
///
/// 返り値は「残る線分」の列。0 本（全部消える）、1 本（端を落とした）、
/// 2 本（真ん中を抜いて分断された）のいずれか。
#[must_use]
pub fn trim_line(target: &Line, cutters: &[Geometry], at: Point2) -> Option<Vec<Line>> {
    // 対象自身は切断エッジに含めない。
    let mut cuts: Vec<f64> = Vec::new();
    for cutter in cutters {
        cuts.extend(line_params_against(target, cutter));
    }
    if cuts.is_empty() {
        return None;
    }
    cuts.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    cuts.dedup_by(|a, b| eq_len(*a, *b));

    // クリック位置のパラメータ。
    let t = target.closest_param(at);

    // t を挟む区間 [lo, hi] を求める。両端は線分の端（0 / 1）で閉じる。
    let lo = cuts.iter().copied().filter(|c| *c <= t).fold(0.0, f64::max);
    let hi = cuts.iter().copied().filter(|c| *c >= t).fold(1.0, f64::min);

    // 交点がクリック位置の片側にしか無い場合、その区間は端まで伸びる。
    if eq_len(lo, hi) {
        return None;
    }

    let mut keep = Vec::new();
    if !is_zero_len(lo) {
        keep.push(Line::new(target.a, target.point_at(lo)));
    }
    if !eq_len(hi, 1.0) {
        keep.push(Line::new(target.point_at(hi), target.b));
    }
    Some(keep)
}

/// 線分を伸ばした結果を求める。
///
/// `at` に近いほうの端を、最も近い交点まで伸ばす。伸ばせなければ `None`。
#[must_use]
pub fn extend_line(target: &Line, cutters: &[Geometry], at: Point2) -> Option<Line> {
    let mut params: Vec<f64> = Vec::new();
    for cutter in cutters {
        params.extend(line_params_extended(target, cutter));
    }
    if params.is_empty() {
        return None;
    }

    // クリック位置がどちらの端に近いかで、伸ばす向きを決める。
    let t = target.closest_param(at);
    let extend_forward = t > 0.5;

    if extend_forward {
        // 終点側（t > 1）で最も近い交点。
        let best = params
            .iter()
            .copied()
            .filter(|p| *p > 1.0 && !eq_len(*p, 1.0))
            .fold(f64::INFINITY, f64::min);
        best.is_finite()
            .then(|| Line::new(target.a, target.point_at(best)))
    } else {
        // 始点側（t < 0）で最も近い交点。
        let best = params
            .iter()
            .copied()
            .filter(|p| *p < 0.0 && !is_zero_len(*p))
            .fold(f64::NEG_INFINITY, f64::max);
        best.is_finite()
            .then(|| Line::new(target.point_at(best), target.b))
    }
}

/// 線分を切り取る（TRIM）。
///
/// 切断エッジは指定せず、**図面上の他のすべての図形**を暗黙のエッジとして使う
/// （近年の AutoCAD のクイックモード相当）。
#[derive(Debug)]
pub struct TrimEntity {
    name: &'static str,
    target: EntityId,
    /// 切る位置（クリックされた点）。
    at: Point2,
    /// Undo 用に、取り除いた元の要素を控える。
    removed: Option<(EntityId, Entity)>,
    /// 適用で作られた要素。Undo で消す。
    created: Vec<EntityId>,
}

impl TrimEntity {
    /// 対象と切る位置を指定して作る。
    #[must_use]
    pub fn new(name: &'static str, target: EntityId, at: Point2) -> Self {
        Self {
            name,
            target,
            at,
            removed: None,
            created: Vec::new(),
        }
    }

    /// 適用後に作られた要素の ID。適用前は空。
    #[must_use]
    pub fn created(&self) -> &[EntityId] {
        &self.created
    }
}

/// 対象以外の図形を切断エッジとして集める。
fn cutters_except(ctx: &EditCtx<'_>, exclude: EntityId) -> Vec<Geometry> {
    ctx.entities()
        .iter()
        .filter(|(id, e)| *id != exclude && ctx.layers().is_entity_visible(e))
        .map(|(_, e)| e.geom.clone())
        .collect()
}

impl Command for TrimEntity {
    fn execute(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        self.removed = None;
        self.created.clear();

        let entity = ctx
            .entities()
            .get(self.target)
            .ok_or(CadError::EntityNotFound)?
            .clone();
        let Geometry::Line(line) = entity.geom else {
            return Err(CadError::NotEditable("TRIM は線分にのみ対応しています"));
        };

        let cutters = cutters_except(ctx, self.target);
        let keep = trim_line(&line, &cutters, self.at)
            .ok_or(CadError::NotEditable("切断する交点が見つかりません"))?;

        let removed = ctx.remove_entity(self.target)?;
        self.removed = Some((self.target, removed));

        for seg in keep {
            if seg.is_degenerate() {
                continue;
            }
            let mut piece = entity.clone();
            piece.geom = Geometry::Line(seg);
            self.created.push(ctx.add_entity(piece));
        }
        Ok(())
    }

    fn undo(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        for id in self.created.drain(..).rev() {
            ctx.remove_entity(id)?;
        }
        if let Some((id, entity)) = self.removed.take() {
            ctx.restore_entity(id, entity)?;
        }
        Ok(())
    }

    fn name(&self) -> &'static str {
        self.name
    }
}

/// 線分を伸ばす（EXTEND）。
#[derive(Debug)]
pub struct ExtendEntity {
    name: &'static str,
    target: EntityId,
    /// 伸ばす向きを示す位置（クリックされた点）。
    at: Point2,
    /// Undo 用に、実行前の図形を控える。
    original: Option<Geometry>,
}

impl ExtendEntity {
    /// 対象と、伸ばす端を示す位置を指定して作る。
    #[must_use]
    pub fn new(name: &'static str, target: EntityId, at: Point2) -> Self {
        Self {
            name,
            target,
            at,
            original: None,
        }
    }
}

impl Command for ExtendEntity {
    fn execute(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        let entity = ctx
            .entities()
            .get(self.target)
            .ok_or(CadError::EntityNotFound)?
            .clone();
        let Geometry::Line(line) = entity.geom else {
            return Err(CadError::NotEditable("EXTEND は線分にのみ対応しています"));
        };

        let cutters = cutters_except(ctx, self.target);
        let extended = extend_line(&line, &cutters, self.at)
            .ok_or(CadError::NotEditable("伸ばす先の交点が見つかりません"))?;

        let slot = ctx.entity_mut(self.target)?;
        self.original = Some(slot.geom.clone());
        slot.geom = Geometry::Line(extended);
        Ok(())
    }

    fn undo(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        if let Some(geom) = self.original.take() {
            ctx.entity_mut(self.target)?.geom = geom;
        }
        Ok(())
    }

    fn name(&self) -> &'static str {
        self.name
    }
}

/// 角を処理する（FILLET / CHAMFER 共通）。
///
/// 2 本の線分を切り詰め、間に図形を 1 つ挟む。
#[derive(Debug)]
pub struct CornerEntities {
    name: &'static str,
    first: EntityId,
    second: EntityId,
    pick1: Point2,
    pick2: Point2,
    kind: CornerKind,
    /// Undo 用に、実行前の 2 線分を控える。
    originals: Vec<(EntityId, Geometry)>,
    /// 挟んだ図形。Undo で消す。
    created: Option<EntityId>,
}

/// 角の処理の種類。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CornerKind {
    /// 半径を指定して丸める。
    Fillet { radius: f64 },
    /// 2 つの距離を指定して面取りする。
    Chamfer { d1: f64, d2: f64 },
}

impl CornerEntities {
    /// 2 本の線分と、それぞれのクリック位置、処理の種類を指定して作る。
    #[must_use]
    pub fn new(
        name: &'static str,
        first: EntityId,
        second: EntityId,
        pick1: Point2,
        pick2: Point2,
        kind: CornerKind,
    ) -> Self {
        Self {
            name,
            first,
            second,
            pick1,
            pick2,
            kind,
            originals: Vec::new(),
            created: None,
        }
    }

    /// 適用後に挟まれた図形の ID。適用前は `None`。
    #[must_use]
    pub fn created(&self) -> Option<EntityId> {
        self.created
    }

    fn line_of(ctx: &EditCtx<'_>, id: EntityId) -> Result<(Entity, Line)> {
        let entity = ctx
            .entities()
            .get(id)
            .ok_or(CadError::EntityNotFound)?
            .clone();
        match entity.geom {
            Geometry::Line(l) => Ok((entity, l)),
            _ => Err(CadError::NotEditable(
                "FILLET / CHAMFER は線分同士にのみ対応しています",
            )),
        }
    }
}

impl Command for CornerEntities {
    fn execute(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        self.originals.clear();
        self.created = None;

        if self.first == self.second {
            return Err(CadError::NotEditable("同じ線分が 2 回指定されました"));
        }

        let (entity1, l1) = Self::line_of(ctx, self.first)?;
        let (_, l2) = Self::line_of(ctx, self.second)?;

        let (result, bridge): (CornerResult, Geometry) = match self.kind {
            CornerKind::Fillet { radius } => {
                let (r, arc) = fillet(&l1, &l2, self.pick1, self.pick2, radius).ok_or(
                    CadError::NotEditable("この角は指定した半径で丸められません"),
                )?;
                (r, Geometry::Arc(arc))
            }
            CornerKind::Chamfer { d1, d2 } => {
                let (r, seg) = chamfer(&l1, &l2, self.pick1, self.pick2, d1, d2).ok_or(
                    CadError::NotEditable("この角は指定した距離で面取りできません"),
                )?;
                (r, Geometry::Line(seg))
            }
        };

        // 2 線分を切り詰める。
        for (id, line) in [(self.first, result.first), (self.second, result.second)] {
            let slot = ctx.entity_mut(id)?;
            self.originals.push((id, slot.geom.clone()));
            slot.geom = Geometry::Line(line);
        }

        // 間に挟む図形は 1 本目の属性を引き継ぐ。
        let mut piece = entity1;
        piece.geom = bridge;
        self.created = Some(ctx.add_entity(piece));
        Ok(())
    }

    fn undo(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        if let Some(id) = self.created.take() {
            ctx.remove_entity(id)?;
        }
        for (id, geom) in self.originals.drain(..).rev() {
            ctx.entity_mut(id)?.geom = geom;
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
    use crate::entity::EntityStore;
    use crate::geom::tolerance::eq_len;
    use crate::group::GroupTable;
    use crate::layer::{LayerId, LayerTable};

    fn new_parts() -> (EntityStore, LayerTable, GroupTable) {
        (EntityStore::new(), LayerTable::new(), GroupTable::new())
    }

    fn line(x0: f64, y0: f64, x1: f64, y1: f64) -> Line {
        Line::new(Point2::new(x0, y0), Point2::new(x1, y1))
    }

    fn line_entity(l: Line) -> Entity {
        Entity::new(Geometry::Line(l), LayerId::ZERO)
    }

    fn geom_line(g: &Geometry) -> Line {
        match g {
            Geometry::Line(l) => *l,
            other => panic!("線分のはず: {other:?}"),
        }
    }

    // ---- trim_line ----

    /// 両側に交点がある部分を切ると、2 本に分断されること。
    #[test]
    fn trim_line_between_two_cutters_splits_into_two() {
        let target = line(0.0, 0.0, 10.0, 0.0);
        let cutters = vec![
            Geometry::Line(line(3.0, -1.0, 3.0, 1.0)),
            Geometry::Line(line(7.0, -1.0, 7.0, 1.0)),
        ];
        // 真ん中をクリック。
        let keep = trim_line(&target, &cutters, Point2::new(5.0, 0.0)).unwrap();
        assert_eq!(keep.len(), 2, "{keep:?}");
        assert!(eq_len(keep[0].b.x, 3.0), "{:?}", keep[0]);
        assert!(eq_len(keep[1].a.x, 7.0), "{:?}", keep[1]);
    }

    /// 端の側を切ると 1 本だけ残ること。
    #[test]
    fn trim_line_at_an_end_keeps_one_piece() {
        let target = line(0.0, 0.0, 10.0, 0.0);
        let cutters = vec![Geometry::Line(line(4.0, -1.0, 4.0, 1.0))];

        // 始点側をクリック → 始点から交点までが消える。
        let keep = trim_line(&target, &cutters, Point2::new(1.0, 0.0)).unwrap();
        assert_eq!(keep.len(), 1, "{keep:?}");
        assert!(
            eq_len(keep[0].a.x, 4.0) && eq_len(keep[0].b.x, 10.0),
            "{:?}",
            keep[0]
        );

        // 終点側をクリック → 交点から終点までが消える。
        let keep = trim_line(&target, &cutters, Point2::new(9.0, 0.0)).unwrap();
        assert_eq!(keep.len(), 1, "{keep:?}");
        assert!(
            eq_len(keep[0].a.x, 0.0) && eq_len(keep[0].b.x, 4.0),
            "{:?}",
            keep[0]
        );
    }

    #[test]
    fn trim_line_without_cutters_is_none() {
        let target = line(0.0, 0.0, 10.0, 0.0);
        assert!(trim_line(&target, &[], Point2::new(5.0, 0.0)).is_none());
        // 交差しないエッジしか無い場合も同じ。
        let far = vec![Geometry::Line(line(0.0, 5.0, 10.0, 5.0))];
        assert!(trim_line(&target, &far, Point2::new(5.0, 0.0)).is_none());
    }

    /// 円を切断エッジにできること。
    #[test]
    fn trim_line_works_with_a_circle_cutter() {
        use crate::geom::Circle;
        let target = line(-10.0, 0.0, 10.0, 0.0);
        let cutters = vec![Geometry::Circle(Circle::new(Point2::ORIGIN, 5.0))];
        let keep = trim_line(&target, &cutters, Point2::ORIGIN).unwrap();
        assert_eq!(keep.len(), 2, "円の内側を抜くと 2 本: {keep:?}");
    }

    // ---- extend_line ----

    #[test]
    fn extend_line_reaches_the_nearest_intersection() {
        let target = line(0.0, 0.0, 5.0, 0.0);
        let cutters = vec![
            Geometry::Line(line(8.0, -1.0, 8.0, 1.0)),
            Geometry::Line(line(12.0, -1.0, 12.0, 1.0)),
        ];
        // 終点側をクリック → 最も近い x=8 まで伸びる。
        let extended = extend_line(&target, &cutters, Point2::new(4.0, 0.0)).unwrap();
        assert!(eq_len(extended.b.x, 8.0), "{extended:?}");
        assert!(eq_len(extended.a.x, 0.0), "始点は動かない");
    }

    #[test]
    fn extend_line_can_grow_backwards() {
        let target = line(5.0, 0.0, 10.0, 0.0);
        let cutters = vec![Geometry::Line(line(2.0, -1.0, 2.0, 1.0))];
        // 始点側をクリック。
        let extended = extend_line(&target, &cutters, Point2::new(6.0, 0.0)).unwrap();
        assert!(eq_len(extended.a.x, 2.0), "{extended:?}");
        assert!(eq_len(extended.b.x, 10.0), "終点は動かない");
    }

    #[test]
    fn extend_line_without_a_target_is_none() {
        let target = line(0.0, 0.0, 5.0, 0.0);
        // 伸ばす先に何も無い。
        assert!(extend_line(&target, &[], Point2::new(4.0, 0.0)).is_none());
        // 交点はあるが線分の内側なので伸ばせない。
        let inside = vec![Geometry::Line(line(2.0, -1.0, 2.0, 1.0))];
        assert!(extend_line(&target, &inside, Point2::new(4.0, 0.0)).is_none());
    }

    // ---- TrimEntity ----

    #[test]
    fn trim_execute_replaces_the_target_with_the_remaining_pieces() {
        let (mut e, mut l, mut g) = new_parts();
        let target = {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
            let t = ctx.add_entity(line_entity(line(0.0, 0.0, 10.0, 0.0)));
            ctx.add_entity(line_entity(line(3.0, -1.0, 3.0, 1.0)));
            ctx.add_entity(line_entity(line(7.0, -1.0, 7.0, 1.0)));
            t
        };

        let mut cmd = TrimEntity::new("TRIM", target, Point2::new(5.0, 0.0));
        let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
        cmd.execute(&mut ctx).unwrap();

        assert!(ctx.entities().get(target).is_none(), "元の線分は消える");
        assert_eq!(cmd.created().len(), 2, "2 本に分断される");
        assert_eq!(ctx.entities().len(), 4, "エッジ 2 本 + 破片 2 本");
    }

    #[test]
    fn trim_undo_restores_the_original_line_with_the_same_id() {
        let (mut e, mut l, mut g) = new_parts();
        let target = {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
            let t = ctx.add_entity(line_entity(line(0.0, 0.0, 10.0, 0.0)));
            ctx.add_entity(line_entity(line(4.0, -1.0, 4.0, 1.0)));
            t
        };
        let original = e.get(target).unwrap().clone();

        let mut cmd = TrimEntity::new("TRIM", target, Point2::new(1.0, 0.0));
        let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
        cmd.execute(&mut ctx).unwrap();
        cmd.undo(&mut ctx).unwrap();

        assert_eq!(
            ctx.entities().get(target),
            Some(&original),
            "同じ ID・同じ内容で戻ること"
        );
    }

    #[test]
    fn trim_redo_after_undo_works() {
        let (mut e, mut l, mut g) = new_parts();
        let target = {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
            let t = ctx.add_entity(line_entity(line(0.0, 0.0, 10.0, 0.0)));
            ctx.add_entity(line_entity(line(4.0, -1.0, 4.0, 1.0)));
            t
        };
        let mut cmd = TrimEntity::new("TRIM", target, Point2::new(1.0, 0.0));
        let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);

        cmd.execute(&mut ctx).unwrap();
        cmd.undo(&mut ctx).unwrap();
        cmd.execute(&mut ctx).unwrap();
        assert!(ctx.entities().get(target).is_none());
    }

    #[test]
    fn trim_missing_target_fails() {
        let (mut e, mut l, mut g) = new_parts();
        let dead = {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
            let d = ctx.add_entity(line_entity(line(0.0, 0.0, 1.0, 0.0)));
            ctx.remove_entity(d).unwrap();
            d
        };
        let mut cmd = TrimEntity::new("TRIM", dead, Point2::ORIGIN);
        let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
        assert_eq!(cmd.execute(&mut ctx), Err(CadError::EntityNotFound));
    }

    /// 線分以外は対象外であることを、はっきりエラーで伝えること。
    #[test]
    fn trim_rejects_non_line_geometry() {
        use crate::geom::Circle;
        let (mut e, mut l, mut g) = new_parts();
        let id = {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
            ctx.add_entity(Entity::new(
                Geometry::Circle(Circle::new(Point2::ORIGIN, 5.0)),
                LayerId::ZERO,
            ))
        };
        let mut cmd = TrimEntity::new("TRIM", id, Point2::new(5.0, 0.0));
        let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
        assert!(matches!(
            cmd.execute(&mut ctx),
            Err(CadError::NotEditable(_))
        ));
    }

    // ---- ExtendEntity ----

    #[test]
    fn extend_execute_grows_the_line_and_keeps_its_id() {
        let (mut e, mut l, mut g) = new_parts();
        let target = {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
            let t = ctx.add_entity(line_entity(line(0.0, 0.0, 5.0, 0.0)));
            ctx.add_entity(line_entity(line(8.0, -1.0, 8.0, 1.0)));
            t
        };

        let mut cmd = ExtendEntity::new("EXTEND", target, Point2::new(4.0, 0.0));
        let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
        cmd.execute(&mut ctx).unwrap();

        let l = geom_line(&ctx.entities().get(target).unwrap().geom);
        assert!(eq_len(l.b.x, 8.0), "{l:?}");
        assert_eq!(ctx.entities().len(), 2, "要素は増えない");
    }

    #[test]
    fn extend_undo_restores_the_original_geometry() {
        let (mut e, mut l, mut g) = new_parts();
        let target = {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
            let t = ctx.add_entity(line_entity(line(0.0, 0.0, 5.0, 0.0)));
            ctx.add_entity(line_entity(line(8.0, -1.0, 8.0, 1.0)));
            t
        };
        let original = e.get(target).unwrap().geom.clone();

        let mut cmd = ExtendEntity::new("EXTEND", target, Point2::new(4.0, 0.0));
        let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
        cmd.execute(&mut ctx).unwrap();
        cmd.undo(&mut ctx).unwrap();

        assert_eq!(ctx.entities().get(target).unwrap().geom, original);
    }

    #[test]
    fn extend_without_a_reachable_edge_fails() {
        let (mut e, mut l, mut g) = new_parts();
        let target = {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
            ctx.add_entity(line_entity(line(0.0, 0.0, 5.0, 0.0)))
        };
        let mut cmd = ExtendEntity::new("EXTEND", target, Point2::new(4.0, 0.0));
        let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
        assert!(matches!(
            cmd.execute(&mut ctx),
            Err(CadError::NotEditable(_))
        ));
    }

    // ---- CornerEntities ----

    /// 直角の角丸めで、2 線分が切り詰められ円弧が 1 本入ること。
    #[test]
    fn fillet_execute_trims_both_lines_and_inserts_an_arc() {
        let (mut e, mut l, mut g) = new_parts();
        let (a, b) = {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
            (
                ctx.add_entity(line_entity(line(10.0, 0.0, 0.0, 0.0))),
                ctx.add_entity(line_entity(line(0.0, 0.0, 0.0, 10.0))),
            )
        };

        let mut cmd = CornerEntities::new(
            "FILLET",
            a,
            b,
            Point2::new(9.0, 0.0),
            Point2::new(0.0, 9.0),
            CornerKind::Fillet { radius: 3.0 },
        );
        let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
        cmd.execute(&mut ctx).unwrap();

        assert_eq!(ctx.entities().len(), 3, "2 線分 + 円弧");
        let arc_id = cmd.created().expect("円弧ができるはず");
        assert!(matches!(
            ctx.entities().get(arc_id).unwrap().geom,
            Geometry::Arc(_)
        ));
        // 線分が接点まで縮んでいる。
        let la = geom_line(&ctx.entities().get(a).unwrap().geom);
        assert!(eq_len(la.b.x, 3.0), "{la:?}");
    }

    #[test]
    fn chamfer_execute_inserts_a_line() {
        let (mut e, mut l, mut g) = new_parts();
        let (a, b) = {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
            (
                ctx.add_entity(line_entity(line(10.0, 0.0, 0.0, 0.0))),
                ctx.add_entity(line_entity(line(0.0, 0.0, 0.0, 10.0))),
            )
        };

        let mut cmd = CornerEntities::new(
            "CHAMFER",
            a,
            b,
            Point2::new(9.0, 0.0),
            Point2::new(0.0, 9.0),
            CornerKind::Chamfer { d1: 3.0, d2: 4.0 },
        );
        let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
        cmd.execute(&mut ctx).unwrap();

        let bridge = geom_line(&ctx.entities().get(cmd.created().unwrap()).unwrap().geom);
        assert!(bridge.a.eq_tol(Point2::new(3.0, 0.0)), "{bridge:?}");
        assert!(bridge.b.eq_tol(Point2::new(0.0, 4.0)), "{bridge:?}");
    }

    #[test]
    fn corner_undo_restores_both_lines_and_removes_the_bridge() {
        let (mut e, mut l, mut g) = new_parts();
        let (a, b) = {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
            (
                ctx.add_entity(line_entity(line(10.0, 0.0, 0.0, 0.0))),
                ctx.add_entity(line_entity(line(0.0, 0.0, 0.0, 10.0))),
            )
        };
        let (ga, gb) = (
            e.get(a).unwrap().geom.clone(),
            e.get(b).unwrap().geom.clone(),
        );

        let mut cmd = CornerEntities::new(
            "FILLET",
            a,
            b,
            Point2::new(9.0, 0.0),
            Point2::new(0.0, 9.0),
            CornerKind::Fillet { radius: 3.0 },
        );
        let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
        cmd.execute(&mut ctx).unwrap();
        cmd.undo(&mut ctx).unwrap();

        assert_eq!(ctx.entities().len(), 2, "円弧が消える");
        assert_eq!(ctx.entities().get(a).unwrap().geom, ga);
        assert_eq!(ctx.entities().get(b).unwrap().geom, gb);
    }

    #[test]
    fn corner_redo_after_undo_works() {
        let (mut e, mut l, mut g) = new_parts();
        let (a, b) = {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
            (
                ctx.add_entity(line_entity(line(10.0, 0.0, 0.0, 0.0))),
                ctx.add_entity(line_entity(line(0.0, 0.0, 0.0, 10.0))),
            )
        };
        let mut cmd = CornerEntities::new(
            "FILLET",
            a,
            b,
            Point2::new(9.0, 0.0),
            Point2::new(0.0, 9.0),
            CornerKind::Fillet { radius: 3.0 },
        );
        let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);

        cmd.execute(&mut ctx).unwrap();
        cmd.undo(&mut ctx).unwrap();
        cmd.execute(&mut ctx).unwrap();
        assert_eq!(ctx.entities().len(), 3);
    }

    #[test]
    fn corner_with_the_same_line_twice_is_rejected() {
        let (mut e, mut l, mut g) = new_parts();
        let a = {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
            ctx.add_entity(line_entity(line(10.0, 0.0, 0.0, 0.0)))
        };
        let mut cmd = CornerEntities::new(
            "FILLET",
            a,
            a,
            Point2::new(9.0, 0.0),
            Point2::new(1.0, 0.0),
            CornerKind::Fillet { radius: 1.0 },
        );
        let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
        assert!(matches!(
            cmd.execute(&mut ctx),
            Err(CadError::NotEditable(_))
        ));
    }

    /// 半径が大きすぎて成立しない場合、図面が変わらないこと。
    #[test]
    fn corner_that_does_not_fit_leaves_document_unchanged() {
        let (mut e, mut l, mut g) = new_parts();
        let (a, b) = {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
            (
                ctx.add_entity(line_entity(line(10.0, 0.0, 0.0, 0.0))),
                ctx.add_entity(line_entity(line(0.0, 0.0, 0.0, 10.0))),
            )
        };
        let ga = e.get(a).unwrap().geom.clone();

        let mut cmd = CornerEntities::new(
            "FILLET",
            a,
            b,
            Point2::new(9.0, 0.0),
            Point2::new(0.0, 9.0),
            CornerKind::Fillet { radius: 1000.0 },
        );
        let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
        assert!(cmd.execute(&mut ctx).is_err());
        assert_eq!(ctx.entities().len(), 2, "何も増えていない");
        assert_eq!(ctx.entities().get(a).unwrap().geom, ga, "線分も変わらない");
    }

    #[test]
    fn corner_at_large_coordinates() {
        let (mut e, mut l, mut g) = new_parts();
        let o = Point2::new(1_000_000.0, 1_000_000.0);
        let (a, b) = {
            let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
            (
                ctx.add_entity(line_entity(Line::new(Point2::new(o.x + 100.0, o.y), o))),
                ctx.add_entity(line_entity(Line::new(o, Point2::new(o.x, o.y + 100.0)))),
            )
        };
        let mut cmd = CornerEntities::new(
            "FILLET",
            a,
            b,
            Point2::new(o.x + 90.0, o.y),
            Point2::new(o.x, o.y + 90.0),
            CornerKind::Fillet { radius: 10.0 },
        );
        let mut ctx = EditCtx::new(&mut e, &mut l, &mut g);
        cmd.execute(&mut ctx).unwrap();
        assert_eq!(ctx.entities().len(), 3);
    }
}
