//! 既存の図形の形（[`Geometry`]）を、ID を保ったまま置き換えるコマンド。
//!
//! プロパティパネル（Issue #31）とグリップ編集（Issue #30）が共用する土台
//! （ADR-0040）。どちらも「新しい形はアプリ側で計算し、それを丸ごと渡す」だけで済む。

use std::collections::BTreeSet;
use std::mem::discriminant;

use super::{Command, EditCtx};
use crate::entity::{EntityId, Geometry};
use crate::error::{CadError, Result};

/// 複数の図形の [`Geometry`] をまとめて置き換える。1 回の適用 = Undo 1 回。
///
/// # 保たれるもの
///
/// - **[`EntityId`]**。削除して追加し直すのではなく、その場で形だけ差し替える。
///   選択や Undo スタックに残る他のコマンドの参照が生き残る
/// - **形以外の属性**（レイヤ・色・グループ）。触らない
///
/// # 受け付ける置き換え（ADR-0040）
///
/// - **同じ種類どうしだけ**。線分を円にするような置き換えは
///   [`CadError::NotEditable`]。#30・#31 の用途では要らず、許すと
///   インプレース編集中の束縛（種類ごとのスロットを指す）が黙って外れる
/// - **ポリラインは頂点の数が同じものどうしだけ**。数が違えば
///   [`CadError::NotEditable`]。束縛は頂点を添字で指す（`PolylineVx(i)` など）ので、
///   減らすと指す先の無い束縛が黙って消え、途中に足すと別の頂点を指すようになる。
///   開いている・閉じている（`closed`）の切り替えは許す（頂点の添字は変わらない）
/// - **インスタンスは配置（基点・回転・倍率・反転）だけ**。参照する定義と
///   パラメータの上書きが元と違えば [`CadError::NotEditable`]。
///   上書きは [`SetInstanceOverride`](super::SetInstanceOverride) の経路で
///   型と範囲を検証するので、ここを抜け道にしない
/// - **形として成立しているもの**（[`Geometry::validate`]）。
///   成立しなければ [`CadError::DegenerateGeometry`]
///
/// # 全部か無しか
///
/// 検証は**書き換える前に全件**行う。1 件でも通らなければ何も変えずに `Err` を返す
/// （[`Document::apply`](crate::Document::apply) は失敗したコマンドを履歴に積まず、
/// 版番号も進めない）。
///
/// # ロックされたレイヤ
///
/// **このコマンドはレイヤのロックを見ない。** 既存の変形系コマンド
/// （[`MoveEntities`](super::MoveEntities)・[`StretchEntities`](super::StretchEntities) など）と
/// 同じく、ロックされたレイヤの図形を対象に含めないのは**呼び出し側（`cad-app`）の責任**。
///
/// # 形が変わらない置き換え
///
/// 元と同じ形を渡しても `Ok` になり、履歴に 1 件積まれる。値を確定しても形が
/// 変わらない場合に Undo を無駄に増やしたくなければ、呼び出し側で比べて適用しないこと。
#[derive(Debug)]
pub struct ReplaceGeometries {
    name: &'static str,
    /// 置き換え後の形。Redo で再び使うので手放さない。
    replacements: Vec<(EntityId, Geometry)>,
    /// Undo で元の形へ正確に戻すための退避先。
    originals: Vec<(EntityId, Geometry)>,
}

impl ReplaceGeometries {
    /// 置き換えの列から作る。
    ///
    /// `name` は Undo 表示などに出す名前（例: `"PROPERTIES"`, `"GRIP"`）。
    #[must_use]
    pub fn new(name: &'static str, replacements: Vec<(EntityId, Geometry)>) -> Self {
        Self {
            name,
            replacements,
            originals: Vec::new(),
        }
    }

    /// 1 つだけ置き換える。
    #[must_use]
    pub fn one(name: &'static str, id: EntityId, geom: Geometry) -> Self {
        Self::new(name, vec![(id, geom)])
    }

    /// 書き換える前に全件を検証する。何も変えない。
    fn check(&self, ctx: &EditCtx<'_>) -> Result<()> {
        if self.replacements.is_empty() {
            return Err(CadError::NotEditable("置き換える図形がありません"));
        }

        let mut seen = BTreeSet::new();
        for (id, new_geom) in &self.replacements {
            if !seen.insert(*id) {
                return Err(CadError::NotEditable("同じ図形が 2 回指定されました"));
            }

            let old_geom = &ctx
                .entities()
                .get(*id)
                .ok_or(CadError::EntityNotFound)?
                .geom;

            if discriminant(old_geom) != discriminant(new_geom) {
                return Err(CadError::NotEditable("図形の種類は変えられません"));
            }
            if let (Geometry::Polyline(old), Geometry::Polyline(new)) = (old_geom, new_geom) {
                if old.vertices.len() != new.vertices.len() {
                    return Err(CadError::NotEditable(
                        "ポリラインの頂点の数は変えられません",
                    ));
                }
            }
            if let (Geometry::Instance(old), Geometry::Instance(new)) = (old_geom, new_geom) {
                if old.definition != new.definition {
                    return Err(CadError::NotEditable(
                        "インスタンスの参照する定義は変えられません",
                    ));
                }
                if old.overrides != new.overrides {
                    return Err(CadError::NotEditable(
                        "インスタンスのパラメータはここでは変えられません",
                    ));
                }
            }

            new_geom.validate()?;
        }
        Ok(())
    }
}

impl Command for ReplaceGeometries {
    fn execute(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        // Redo で 2 度目に実行されることがあるので、必ず作り直す。
        self.originals.clear();
        self.check(ctx)?;

        for (id, new_geom) in &self.replacements {
            match ctx.entity_mut(*id) {
                Ok(e) => {
                    let old = std::mem::replace(&mut e.geom, new_geom.clone());
                    self.originals.push((*id, old));
                }
                Err(err) => {
                    // 検証を通ったので起きないはずだが、「全部か無しか」を守るため
                    // ここまでに書き換えた分を戻してから返す。
                    for (rid, rg) in self.originals.drain(..).rev() {
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
        // 退避しておいた元の形をそのまま書き戻す。逆順に戻す。
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
    use crate::command::{
        AddEntities, CreateGroup, DefineComponent, DeleteEntities, EnterDefinitionEdit,
        ExitDefinitionEdit, InsertInstance, SetBinding, SetDefinitionParams,
    };
    use crate::component::{Binding, DefinitionId, ParamDecl, Placement, Slot};
    use crate::entity::Entity;
    use crate::expr::parse;
    use crate::geom::tolerance::{eq_angle, EPS_LEN};
    use crate::geom::{Arc, Circle, Line, Point2, Polyline, Vec2, Xline};
    use crate::layer::{AciColor, ColorSpec, LayerId};
    use crate::native::{read, write};
    use crate::Document;
    use std::f64::consts::{FRAC_PI_2, PI, TAU};

    fn p(x: f64, y: f64) -> Point2 {
        Point2::new(x, y)
    }

    fn line(ax: f64, ay: f64, bx: f64, by: f64) -> Geometry {
        Geometry::Line(Line::new(p(ax, ay), p(bx, by)))
    }

    fn circle(cx: f64, cy: f64, r: f64) -> Geometry {
        Geometry::Circle(Circle::new(p(cx, cy), r))
    }

    fn arc(r: f64, start: f64, end: f64) -> Geometry {
        Geometry::Arc(Arc::new(Point2::ORIGIN, r, start, end))
    }

    fn poly(vertices: Vec<Point2>, closed: bool) -> Geometry {
        Geometry::Polyline(Polyline::new(vertices, closed))
    }

    /// 1 つ追加して ID を返す。
    fn add(doc: &mut Document, entity: Entity) -> EntityId {
        let before: BTreeSet<_> = doc.entities().ids().collect();
        doc.apply(Box::new(AddEntities::one("TEST", entity)))
            .expect("追加できるはず");
        doc.entities()
            .ids()
            .find(|id| !before.contains(id))
            .expect("増えたはず")
    }

    /// レイヤ 0 以外・色つき・グループつきの線分を置いた図面。
    ///
    /// 形以外の属性が保たれることを確かめるため、既定値でない属性を持たせる。
    fn doc_with_decorated_line() -> (Document, EntityId, Entity) {
        let mut doc = Document::new();
        doc.apply(Box::new(crate::command::AddLayer::new(
            "壁".to_owned(),
            AciColor(1),
        )))
        .unwrap();
        let layer = doc.layers().by_name("壁").unwrap();
        let mut e = Entity::new(line(0.0, 0.0, 10.0, 0.0), layer);
        e.color = ColorSpec::Aci(AciColor(3));
        let id = add(&mut doc, e);
        doc.apply(Box::new(CreateGroup::new("GROUP", "まとまり", vec![id])))
            .unwrap();
        let decorated = doc.entities().get(id).unwrap().clone();
        assert!(decorated.group.is_some(), "グループに入っていること");
        (doc, id, decorated)
    }

    /// 定義 1 つとそのインスタンス 1 つを置いた図面。
    fn doc_with_instance() -> (Document, DefinitionId, EntityId) {
        let mut doc = Document::new();
        let contents = vec![Entity::new(line(0.0, 0.0, 1.0, 0.0), LayerId::ZERO)];
        doc.apply(Box::new(DefineComponent::new(
            "DEF",
            "部品",
            Point2::ORIGIN,
            contents,
        )))
        .unwrap();
        let def = doc.definitions().iter().next().unwrap().0;
        doc.apply(Box::new(InsertInstance::new(
            "INSERT",
            def,
            Placement::at(p(5.0, 5.0)),
            LayerId::ZERO,
        )))
        .unwrap();
        let id = doc
            .entities()
            .iter()
            .find(|(_, e)| matches!(e.geom, Geometry::Instance(_)))
            .unwrap()
            .0;
        (doc, def, id)
    }

    fn geom_of(doc: &Document, id: EntityId) -> Geometry {
        doc.entities().get(id).unwrap().geom.clone()
    }

    /// 不正な置き換えが、図面・履歴・版番号のどれも変えないこと。
    fn assert_rejected(doc: &mut Document, cmd: ReplaceGeometries) -> CadError {
        let entities: Vec<_> = doc
            .entities()
            .iter()
            .map(|(id, e)| (id, e.clone()))
            .collect();
        let revision = doc.revision();
        let depth = doc.history().len();

        let err = doc.apply(Box::new(cmd)).unwrap_err();

        let after: Vec<_> = doc
            .entities()
            .iter()
            .map(|(id, e)| (id, e.clone()))
            .collect();
        assert_eq!(after, entities, "図面が変わっていないこと");
        assert_eq!(doc.revision(), revision, "版番号が進んでいないこと");
        assert_eq!(doc.history().len(), depth, "履歴に積まれていないこと");
        err
    }

    // ---- 往復 -----------------------------------------------------------

    #[test]
    fn replace_undo_redo_round_trips_shape_and_keeps_id_and_attributes() {
        let (mut doc, id, original) = doc_with_decorated_line();
        let new_geom = line(1.0, 2.0, 30.0, 40.0);

        doc.apply(Box::new(ReplaceGeometries::one(
            "GRIP",
            id,
            new_geom.clone(),
        )))
        .unwrap();
        let replaced = doc.entities().get(id).expect("同じ ID のまま残ること");
        assert_eq!(replaced.geom, new_geom);
        assert_eq!(replaced.layer, original.layer, "レイヤは変えない");
        assert_eq!(replaced.color, original.color, "色は変えない");
        assert_eq!(replaced.group, original.group, "グループは変えない");

        doc.undo().unwrap();
        assert_eq!(doc.entities().get(id), Some(&original), "Undo で元のとおり");

        doc.redo().unwrap();
        let redone = doc.entities().get(id).expect("Redo でも同じ ID");
        assert_eq!(redone.geom, new_geom);
        assert_eq!(redone.layer, original.layer);
    }

    #[test]
    fn several_entities_are_replaced_and_undone_as_one_step() {
        let mut doc = Document::new();
        let a = add(
            &mut doc,
            Entity::new(line(0.0, 0.0, 1.0, 0.0), LayerId::ZERO),
        );
        let b = add(&mut doc, Entity::new(circle(0.0, 0.0, 1.0), LayerId::ZERO));
        let c = add(
            &mut doc,
            Entity::new(arc(1.0, 0.0, FRAC_PI_2), LayerId::ZERO),
        );
        let before = [geom_of(&doc, a), geom_of(&doc, b), geom_of(&doc, c)];
        let depth = doc.history().len();

        let after = [
            line(0.0, 0.0, 5.0, 5.0),
            circle(3.0, 3.0, 2.5),
            arc(4.0, PI, 0.0),
        ];
        doc.apply(Box::new(ReplaceGeometries::new(
            "PROPERTIES",
            vec![
                (a, after[0].clone()),
                (b, after[1].clone()),
                (c, after[2].clone()),
            ],
        )))
        .unwrap();
        assert_eq!(doc.history().len(), depth + 1, "履歴は 1 件だけ増える");
        assert_eq!(
            [geom_of(&doc, a), geom_of(&doc, b), geom_of(&doc, c)],
            after
        );

        doc.undo().unwrap();
        assert_eq!(
            [geom_of(&doc, a), geom_of(&doc, b), geom_of(&doc, c)],
            before,
            "Undo 1 回で全部戻ること"
        );
    }

    #[test]
    fn revision_advances_on_replace_undo_and_redo() {
        let mut doc = Document::new();
        let id = add(
            &mut doc,
            Entity::new(line(0.0, 0.0, 1.0, 0.0), LayerId::ZERO),
        );

        let r0 = doc.revision();
        doc.apply(Box::new(ReplaceGeometries::one(
            "GRIP",
            id,
            line(0.0, 0.0, 2.0, 0.0),
        )))
        .unwrap();
        let r1 = doc.revision();
        assert!(r1 > r0, "置き換えで版番号が進むこと");
        doc.undo().unwrap();
        let r2 = doc.revision();
        assert!(r2 > r1, "Undo で版番号が進むこと");
        doc.redo().unwrap();
        assert!(doc.revision() > r2, "Redo で版番号が進むこと");
    }

    #[test]
    fn every_kind_accepts_a_same_kind_replacement() {
        let mut doc = Document::new();
        let cases = [
            (line(0.0, 0.0, 1.0, 0.0), line(0.0, 0.0, 0.0, 7.0)),
            (circle(0.0, 0.0, 1.0), circle(1.0, 1.0, 9.0)),
            (arc(1.0, 0.0, PI), arc(2.0, PI, FRAC_PI_2)),
            (
                Geometry::Xline(Xline::horizontal(Point2::ORIGIN)),
                Geometry::Xline(Xline::new(p(1.0, 1.0), Vec2::new(3.0, 4.0)).unwrap()),
            ),
            (
                poly(vec![p(0.0, 0.0), p(1.0, 0.0)], false),
                poly(vec![p(0.0, 0.0), p(1.0, 5.0)], false),
            ),
            // 頂点の数が同じなら、開閉の切り替えも受け付ける（両方向）。
            (
                poly(vec![p(0.0, 0.0), p(1.0, 0.0), p(1.0, 1.0)], false),
                poly(vec![p(0.0, 0.0), p(2.0, 0.0), p(2.0, 2.0)], true),
            ),
            (
                poly(vec![p(0.0, 0.0), p(1.0, 0.0), p(1.0, 1.0)], true),
                poly(vec![p(0.0, 0.0), p(1.0, 0.0), p(1.0, 1.0)], false),
            ),
        ];
        for (before, after) in cases {
            let id = add(&mut doc, Entity::new(before, LayerId::ZERO));
            doc.apply(Box::new(ReplaceGeometries::one("TEST", id, after.clone())))
                .unwrap_or_else(|e| panic!("{after:?} を受け付けること: {e}"));
            assert_eq!(geom_of(&doc, id), after);
        }
    }

    // ---- インスタンス -----------------------------------------------------

    #[test]
    fn instance_placement_can_change_and_undo() {
        let (mut doc, _def, id) = doc_with_instance();
        let before = geom_of(&doc, id);
        let Geometry::Instance(inst) = &before else {
            unreachable!()
        };
        let moved = Geometry::Instance(
            inst.with_placement(Placement::new(p(9.0, -3.0), FRAC_PI_2, 2.0, true).unwrap()),
        );

        doc.apply(Box::new(ReplaceGeometries::one("GRIP", id, moved.clone())))
            .unwrap();
        assert_eq!(geom_of(&doc, id), moved);
        doc.undo().unwrap();
        assert_eq!(geom_of(&doc, id), before);
    }

    #[test]
    fn instance_definition_or_overrides_change_is_rejected() {
        let (mut doc, def, id) = doc_with_instance();
        // 別の定義を作っておく。
        doc.apply(Box::new(DefineComponent::new(
            "DEF",
            "別の部品",
            Point2::ORIGIN,
            vec![Entity::new(circle(0.0, 0.0, 1.0), LayerId::ZERO)],
        )))
        .unwrap();
        let other = doc
            .definitions()
            .iter()
            .map(|(d, _)| d)
            .find(|d| *d != def)
            .unwrap();
        let Geometry::Instance(inst) = geom_of(&doc, id) else {
            unreachable!()
        };

        let mut swapped = inst.clone();
        swapped.definition = other;
        let err = assert_rejected(
            &mut doc,
            ReplaceGeometries::one("TEST", id, Geometry::Instance(swapped)),
        );
        assert!(matches!(err, CadError::NotEditable(_)), "{err:?}");

        let mut overridden = inst;
        overridden
            .overrides
            .insert("w".to_owned(), crate::expr::Value::Number(3.0));
        let err = assert_rejected(
            &mut doc,
            ReplaceGeometries::one("TEST", id, Geometry::Instance(overridden)),
        );
        assert!(matches!(err, CadError::NotEditable(_)), "{err:?}");
    }

    #[test]
    fn instance_with_invalid_placement_is_rejected() {
        let (mut doc, _def, id) = doc_with_instance();
        let Geometry::Instance(inst) = geom_of(&doc, id) else {
            unreachable!()
        };
        // `Placement::new` を通さずに壊れた配置を作る（フィールドは公開されている）。
        for bad in [
            Placement {
                scale: 0.0,
                ..inst.placement
            },
            Placement {
                scale: -1.0,
                ..inst.placement
            },
            Placement {
                rotation: f64::NAN,
                ..inst.placement
            },
            Placement {
                origin: p(f64::INFINITY, 0.0),
                ..inst.placement
            },
        ] {
            let err = assert_rejected(
                &mut doc,
                ReplaceGeometries::one("TEST", id, Geometry::Instance(inst.with_placement(bad))),
            );
            assert!(matches!(err, CadError::DegenerateGeometry(_)), "{err:?}");
        }
    }

    // ---- 拒否（全部か無しか） --------------------------------------------

    #[test]
    fn invalid_shapes_are_rejected_without_any_change() {
        let mut doc = Document::new();
        let l = add(
            &mut doc,
            Entity::new(line(0.0, 0.0, 1.0, 0.0), LayerId::ZERO),
        );
        let c = add(&mut doc, Entity::new(circle(0.0, 0.0, 1.0), LayerId::ZERO));
        let a = add(&mut doc, Entity::new(arc(1.0, 0.0, PI), LayerId::ZERO));
        let x = add(
            &mut doc,
            Entity::new(
                Geometry::Xline(Xline::horizontal(Point2::ORIGIN)),
                LayerId::ZERO,
            ),
        );
        let pl = add(
            &mut doc,
            Entity::new(poly(vec![p(0.0, 0.0), p(1.0, 0.0)], false), LayerId::ZERO),
        );
        // 頂点の数は変えられない（`NotEditable`）ので、3 頂点の形の不正は 3 頂点の元で見る。
        let pl3 = add(
            &mut doc,
            Entity::new(
                poly(vec![p(0.0, 0.0), p(1.0, 0.0), p(1.0, 1.0)], false),
                LayerId::ZERO,
            ),
        );

        let bad: Vec<(EntityId, Geometry)> = vec![
            (l, line(f64::NAN, 0.0, 1.0, 0.0)),
            (l, line(0.0, 0.0, f64::INFINITY, 0.0)),
            (l, line(2.0, 2.0, 2.0, 2.0)),
            (l, line(2.0, 2.0, 2.0 + EPS_LEN * 0.5, 2.0)),
            (c, circle(0.0, 0.0, 0.0)),
            (c, circle(0.0, 0.0, -1.0)),
            (c, circle(0.0, 0.0, EPS_LEN * 0.5)),
            (c, circle(0.0, 0.0, f64::NAN)),
            (c, circle(f64::NEG_INFINITY, 0.0, 1.0)),
            (a, arc(0.0, 0.0, PI)),
            (a, arc(1.0, f64::NAN, PI)),
            (a, arc(1.0, 0.0, f64::INFINITY)),
            (
                x,
                Geometry::Xline(Xline {
                    origin: Point2::ORIGIN,
                    direction: Vec2::new(3.0, 4.0),
                }),
            ),
            (
                x,
                Geometry::Xline(Xline {
                    origin: Point2::ORIGIN,
                    direction: Vec2::new(0.0, 0.0),
                }),
            ),
            (
                x,
                Geometry::Xline(Xline {
                    origin: p(f64::NAN, 0.0),
                    direction: Vec2::X,
                }),
            ),
            (pl, poly(vec![p(0.0, 0.0), p(1.0, 0.0)], true)),
            (pl, poly(vec![p(0.0, 0.0), p(f64::NAN, 0.0)], false)),
            (
                pl3,
                poly(vec![p(1.0, 1.0), p(1.0, 1.0), p(1.0, 1.0)], false),
            ),
        ];
        for (id, geom) in bad {
            let err = assert_rejected(&mut doc, ReplaceGeometries::one("TEST", id, geom.clone()));
            assert!(
                matches!(err, CadError::DegenerateGeometry(_)),
                "{geom:?} は形の不正として拒むこと: {err:?}"
            );
        }

        // 頂点が 1 つのポリラインは、元と数が違うので上の経路では `NotEditable` が先に出る。
        // 形の判定そのものが拒むことは直接確かめる。
        assert!(matches!(
            poly(vec![p(0.0, 0.0)], false).validate(),
            Err(CadError::DegenerateGeometry(_))
        ));
    }

    #[test]
    fn one_bad_entry_rejects_the_whole_batch() {
        let mut doc = Document::new();
        let good = add(
            &mut doc,
            Entity::new(line(0.0, 0.0, 1.0, 0.0), LayerId::ZERO),
        );
        let other = add(&mut doc, Entity::new(circle(0.0, 0.0, 1.0), LayerId::ZERO));

        // 先頭は正しく、後ろが不正。先頭も書き換わっていてはいけない。
        let err = assert_rejected(
            &mut doc,
            ReplaceGeometries::new(
                "TEST",
                vec![
                    (good, line(0.0, 0.0, 9.0, 9.0)),
                    (other, circle(0.0, 0.0, -2.0)),
                ],
            ),
        );
        assert!(matches!(err, CadError::DegenerateGeometry(_)), "{err:?}");
    }

    #[test]
    fn kind_change_is_rejected() {
        let mut doc = Document::new();
        let id = add(
            &mut doc,
            Entity::new(line(0.0, 0.0, 1.0, 0.0), LayerId::ZERO),
        );
        let err = assert_rejected(
            &mut doc,
            ReplaceGeometries::one("TEST", id, circle(0.0, 0.0, 1.0)),
        );
        assert!(matches!(err, CadError::NotEditable(_)), "{err:?}");
    }

    // ---- 1 周の円弧（Issue #55） -------------------------------------------

    /// **開始角と終了角が一致する円弧（1 周）も置き換えられること。**
    ///
    /// DXF から読んだ 0°→360° の円弧はこの形になる。`Arc::sweep` は一致を 1 周と
    /// 約束しているので、中心を動かす・半径を変えるだけの置き換えを拒んではいけない。
    #[test]
    fn full_turn_arc_can_be_moved_and_resized() {
        let mut doc = Document::new();
        // DXF の 0°→360° と、開始角 = 終了角そのものの 2 通り。
        for (start, end) in [(0.0, TAU), (1.0, 1.0)] {
            let full = Geometry::Arc(Arc::new(Point2::ORIGIN, 2.0, start, end));
            let id = add(&mut doc, Entity::new(full.clone(), LayerId::ZERO));

            let moved = Geometry::Arc(Arc::new(p(5.0, -3.0), 2.0, start, end));
            doc.apply(Box::new(ReplaceGeometries::one("GRIP", id, moved.clone())))
                .unwrap_or_else(|e| panic!("中心の移動を受け付けること: {e}"));
            assert_eq!(geom_of(&doc, id), moved);

            let resized = Geometry::Arc(Arc::new(p(5.0, -3.0), 7.0, start, end));
            doc.apply(Box::new(ReplaceGeometries::one(
                "PROPERTIES",
                id,
                resized.clone(),
            )))
            .unwrap_or_else(|e| panic!("半径の変更を受け付けること: {e}"));
            assert_eq!(geom_of(&doc, id), resized);
            let Geometry::Arc(a) = geom_of(&doc, id) else {
                unreachable!()
            };
            assert!(eq_angle(a.sweep(), TAU), "1 周のまま: {}", a.sweep());

            doc.undo().unwrap();
            doc.undo().unwrap();
            assert_eq!(geom_of(&doc, id), full, "Undo で元の 1 周の円弧に戻る");
        }
    }

    // ---- ポリラインの頂点の数（Issue #55） ---------------------------------

    #[test]
    fn polyline_vertex_count_change_is_rejected() {
        let mut doc = Document::new();
        let id = add(
            &mut doc,
            Entity::new(
                poly(vec![p(0.0, 0.0), p(1.0, 0.0), p(1.0, 1.0)], false),
                LayerId::ZERO,
            ),
        );
        for (geom, what) in [
            (poly(vec![p(0.0, 0.0), p(1.0, 0.0)], false), "減らす"),
            (
                poly(
                    vec![p(0.0, 0.0), p(1.0, 0.0), p(1.0, 1.0), p(0.0, 1.0)],
                    false,
                ),
                "増やす",
            ),
            (
                poly(
                    vec![p(0.0, 0.0), p(1.0, 0.0), p(1.0, 1.0), p(0.0, 1.0)],
                    true,
                ),
                "増やして閉じる",
            ),
        ] {
            let err = assert_rejected(&mut doc, ReplaceGeometries::one("TEST", id, geom));
            assert!(matches!(err, CadError::NotEditable(_)), "{what}: {err:?}");
        }
    }

    /// 定義 1 つ（3 頂点のポリライン、頂点 2 の X に束縛）を置き、インプレース編集に入った図面。
    ///
    /// 返すのは（図面, 定義, 編集に入ったときの配置, 図面に出たポリラインの ID）。
    fn doc_editing_bound_polyline() -> (Document, DefinitionId, Placement, EntityId) {
        let mut doc = Document::new();
        let contents = vec![Entity::new(
            poly(vec![p(0.0, 0.0), p(4.0, 0.0), p(4.0, 3.0)], false),
            LayerId::ZERO,
        )];
        doc.apply(Box::new(DefineComponent::new(
            "DEF",
            "枠",
            Point2::ORIGIN,
            contents,
        )))
        .unwrap();
        let def = doc.definitions().iter().next().unwrap().0;
        doc.apply(Box::new(SetDefinitionParams::new(
            "PARAM",
            def,
            vec![ParamDecl::number("幅", 4.0)],
        )))
        .unwrap();
        doc.apply(Box::new(SetBinding::new(
            "BIND",
            def,
            Binding::new(0, Slot::PolylineVx(2), parse("幅").unwrap()),
        )))
        .unwrap();
        let placement = Placement::at(p(10.0, 20.0));
        doc.apply(Box::new(InsertInstance::new(
            "INSERT",
            def,
            placement,
            LayerId::ZERO,
        )))
        .unwrap();
        let inst = doc.entities().ids().next().unwrap();

        doc.apply(Box::new(EnterDefinitionEdit::new("EDITCOMP", inst)))
            .unwrap();
        let member = doc
            .entities()
            .iter()
            .find(|(_, e)| matches!(e.geom, Geometry::Polyline(_)))
            .expect("中身が図面に出ていること")
            .0;
        (doc, def, placement, member)
    }

    /// 編集を抜けて、定義に残った束縛を返す。
    fn exit_and_bindings(
        doc: &mut Document,
        def: DefinitionId,
        placement: Placement,
        member: EntityId,
    ) -> Vec<Binding> {
        doc.apply(Box::new(ExitDefinitionEdit::new(
            "EDITCOMP",
            def,
            placement,
            vec![member],
            vec![Some(0)],
        )))
        .unwrap();
        doc.definitions().get(def).unwrap().bindings.clone()
    }

    /// **インプレース編集中に頂点を減らす置き換えは拒まれ、束縛も形も残ること**（Issue #55 の再現）。
    ///
    /// 通してしまうと、編集を抜けたときに `Binding::fits` に落ちて束縛が黙って消える。
    #[test]
    fn in_place_edit_rejects_dropping_a_bound_polyline_vertex() {
        let (mut doc, def, placement, member) = doc_editing_bound_polyline();
        let before = geom_of(&doc, member);

        let err = assert_rejected(
            &mut doc,
            ReplaceGeometries::one(
                "GRIP",
                member,
                poly(vec![p(10.0, 20.0), p(14.0, 20.0)], false),
            ),
        );
        assert!(matches!(err, CadError::NotEditable(_)), "{err:?}");
        assert_eq!(geom_of(&doc, member), before, "形は変わらない");

        let bindings = exit_and_bindings(&mut doc, def, placement, member);
        assert_eq!(bindings.len(), 1, "**束縛が残る**");
        assert_eq!(bindings[0].slot, Slot::PolylineVx(2));
    }

    /// 頂点の数が同じなら、インプレース編集中でも今までどおり置き換えられ、束縛も残ること。
    #[test]
    fn in_place_edit_accepts_same_vertex_count_and_keeps_the_binding() {
        let (mut doc, def, placement, member) = doc_editing_bound_polyline();
        let moved = poly(vec![p(10.0, 20.0), p(14.0, 20.0), p(14.0, 25.0)], true);

        doc.apply(Box::new(ReplaceGeometries::one(
            "GRIP",
            member,
            moved.clone(),
        )))
        .unwrap();
        assert_eq!(geom_of(&doc, member), moved);

        let bindings = exit_and_bindings(&mut doc, def, placement, member);
        assert_eq!(bindings.len(), 1, "束縛が残る");
        assert_eq!(bindings[0].slot, Slot::PolylineVx(2));
    }

    #[test]
    fn missing_deleted_duplicate_and_empty_targets_are_rejected() {
        let mut doc = Document::new();
        let live = add(
            &mut doc,
            Entity::new(line(0.0, 0.0, 1.0, 0.0), LayerId::ZERO),
        );
        let dead = add(
            &mut doc,
            Entity::new(line(0.0, 1.0, 1.0, 1.0), LayerId::ZERO),
        );
        doc.apply(Box::new(DeleteEntities::new("ERASE", vec![dead])))
            .unwrap();
        let err = assert_rejected(
            &mut doc,
            ReplaceGeometries::new(
                "TEST",
                vec![
                    (live, line(0.0, 0.0, 5.0, 0.0)),
                    (dead, line(0.0, 0.0, 5.0, 5.0)),
                ],
            ),
        );
        assert_eq!(err, CadError::EntityNotFound, "削除済みの ID");

        // 生きているスロットでも、世代が違う ID は別物として扱うこと。
        let other_generation = EntityId::new(live.index(), live.generation() + 1);
        let err = assert_rejected(
            &mut doc,
            ReplaceGeometries::one("TEST", other_generation, line(0.0, 0.0, 5.0, 0.0)),
        );
        assert_eq!(err, CadError::EntityNotFound, "世代違いの ID");

        let err = assert_rejected(
            &mut doc,
            ReplaceGeometries::new(
                "TEST",
                vec![
                    (live, line(0.0, 0.0, 5.0, 0.0)),
                    (live, line(0.0, 0.0, 6.0, 0.0)),
                ],
            ),
        );
        assert!(matches!(err, CadError::NotEditable(_)), "重複 ID: {err:?}");

        let err = assert_rejected(&mut doc, ReplaceGeometries::new("TEST", vec![]));
        assert!(matches!(err, CadError::NotEditable(_)), "空: {err:?}");
    }

    // ---- 保存 -----------------------------------------------------------

    #[test]
    fn replaced_shape_survives_ymc_round_trip() {
        let (mut doc, id, original) = doc_with_decorated_line();
        let (def_id, inst_id) = {
            // 同じ図面にインスタンスも置き、配置の置き換えも保存されることを見る。
            let contents = vec![Entity::new(line(0.0, 0.0, 1.0, 0.0), LayerId::ZERO)];
            doc.apply(Box::new(DefineComponent::new(
                "DEF",
                "部品",
                Point2::ORIGIN,
                contents,
            )))
            .unwrap();
            let def = doc.definitions().iter().next().unwrap().0;
            let inst = add(
                &mut doc,
                Entity::new(
                    Geometry::Instance(crate::component::Instance::new(
                        def,
                        Placement::at(p(1.0, 1.0)),
                    )),
                    LayerId::ZERO,
                ),
            );
            (def, inst)
        };

        let new_line = line(-3.0, 4.5, 12.25, -7.0);
        let new_inst = Geometry::Instance(crate::component::Instance::new(
            def_id,
            Placement::new(p(20.0, 30.0), FRAC_PI_2, 3.0, true).unwrap(),
        ));
        doc.apply(Box::new(ReplaceGeometries::new(
            "PROPERTIES",
            vec![(id, new_line.clone()), (inst_id, new_inst.clone())],
        )))
        .unwrap();

        let back = read::read_from_bytes(&write::write_to_bytes(&doc)).expect("読めるはず");
        let ents: Vec<Entity> = back.entities().iter().map(|(_, e)| e.clone()).collect();

        let saved_line = ents
            .iter()
            .find(|e| matches!(e.geom, Geometry::Line(_)))
            .expect("線分が保存されていること");
        assert_eq!(saved_line.geom, new_line, "置き換え後の形で保存されること");
        assert_eq!(saved_line.color, original.color);
        assert_eq!(
            back.layers().get(saved_line.layer).map(|l| l.name.as_str()),
            Some("壁")
        );

        let saved_inst = ents
            .iter()
            .find(|e| matches!(e.geom, Geometry::Instance(_)))
            .expect("インスタンスが保存されていること");
        assert_eq!(
            saved_inst.geom, new_inst,
            "置き換え後の配置で保存されること"
        );
    }
}
