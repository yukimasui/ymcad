//! `entity_preview` のテスト（GPU なし）。
//!
//! いちばん大事なのは「プレビュー == クリックの結果」。プレビューを計算してから、同じ `at` で
//! `TrimEntity` / `ExtendEntity` を実際に適用し、残った形・伸びた形と照合する。境界には
//! インスタンス・作図線・非表示・ロックのレイヤを混ぜ、`cad-core` の `cutters_except` と
//! 条件が同じことも固定する（条件がずれると、どこかで結果が食い違って落ちる）。

use std::f64::consts::PI;

use cad_core::command::{
    AddEntities, AddLayer, DefineComponent, ExtendEntity, InsertInstance, SetLayerProperties,
    TrimEntity,
};
use cad_core::component::Placement;
use cad_core::geom::{Arc, Circle, Line, Point2, Polyline, Vec2, Xline};
use cad_core::layer::AciColor;
use cad_core::{Document, Entity, EntityId, Geometry, LayerId};

use super::*;
use crate::session::{ClickTarget, Session};
use crate::test_util::Lcg;

fn seg(x0: f64, y0: f64, x1: f64, y1: f64) -> Line {
    Line::new(Point2::new(x0, y0), Point2::new(x1, y1))
}

fn add(doc: &mut Document, geoms: Vec<(Geometry, LayerId)>) -> Vec<EntityId> {
    let before: Vec<EntityId> = doc.entities().ids().collect();
    let entities = geoms
        .into_iter()
        .map(|(g, layer)| Entity::new(g, layer))
        .collect();
    doc.apply(Box::new(AddEntities::many("TEST", entities)))
        .expect("追加できるはず");
    new_ids(doc, &before)
}

fn new_ids(doc: &Document, before: &[EntityId]) -> Vec<EntityId> {
    doc.entities()
        .ids()
        .filter(|id| !before.contains(id))
        .collect()
}

fn add_layer(doc: &mut Document, name: &str) -> LayerId {
    doc.apply(Box::new(AddLayer::new(name, AciColor::WHITE)))
        .expect("レイヤを足せるはず");
    doc.layers().by_name(name).expect("足したはず")
}

/// `command`（"TRIM" / "EXTEND"）を実行中の `Session` で、`id` を `at` でクリックしたときのプレビュー。
fn preview(
    command: &str,
    doc: &mut Document,
    boundaries: &mut Boundaries,
    id: EntityId,
    at: Point2,
) -> Option<EntityPreview> {
    let mut s = Session::new();
    s.start_command_from_ui(command, doc);
    assert_eq!(s.active_command(), Some(command), "前提");
    s.entity_preview(&ClickTarget::Entity { id, at }, doc, boundaries)
}

fn only_line(geoms: &[Geometry]) -> Line {
    match geoms {
        [Geometry::Line(l)] => *l,
        other => panic!("線分 1 本のはず: {other:?}"),
    }
}

// ---- trimmed_away / extended_part ----------------------------------------------

/// 残る線分が 0 本なら、線分全体が消える。
#[test]
fn trimmed_away_with_nothing_left_is_the_whole_line() {
    let target = seg(0.0, 0.0, 10.0, 0.0);
    assert_eq!(trimmed_away(&target, &[]), Some(target));
}

/// 1 本残るなら、その反対側が消える（始点側が残る場合・終点側が残る場合）。
#[test]
fn trimmed_away_with_one_left_is_the_other_side() {
    let target = seg(0.0, 0.0, 10.0, 0.0);
    // 始点側（0〜4）が残る → 4〜10 が消える。
    assert_eq!(
        trimmed_away(&target, &[seg(0.0, 0.0, 4.0, 0.0)]),
        Some(seg(4.0, 0.0, 10.0, 0.0))
    );
    // 終点側（6〜10）が残る → 0〜6 が消える。
    assert_eq!(
        trimmed_away(&target, &[seg(6.0, 0.0, 10.0, 0.0)]),
        Some(seg(0.0, 0.0, 6.0, 0.0))
    );
}

/// 2 本残るなら、その間が消える。
#[test]
fn trimmed_away_with_two_left_is_the_middle() {
    let target = seg(0.0, 0.0, 10.0, 0.0);
    assert_eq!(
        trimmed_away(
            &target,
            &[seg(0.0, 0.0, 3.0, 0.0), seg(7.0, 0.0, 10.0, 0.0)]
        ),
        Some(seg(3.0, 0.0, 7.0, 0.0))
    );
}

/// `trim_line` が返さない形（3 本以上）は計算しない。
#[test]
fn trimmed_away_rejects_three_pieces() {
    let target = seg(0.0, 0.0, 10.0, 0.0);
    let pieces = [
        seg(0.0, 0.0, 1.0, 0.0),
        seg(2.0, 0.0, 3.0, 0.0),
        seg(4.0, 0.0, 10.0, 0.0),
    ];
    assert_eq!(trimmed_away(&target, &pieces), None);
}

/// 伸びる部分は、元の端から新しい端まで（終点側・始点側）。
#[test]
fn extended_part_runs_from_the_old_end_to_the_new_end() {
    let target = seg(0.0, 0.0, 10.0, 0.0);
    assert_eq!(
        extended_part(&target, &seg(0.0, 0.0, 15.0, 0.0)),
        seg(10.0, 0.0, 15.0, 0.0)
    );
    assert_eq!(
        extended_part(&target, &seg(-5.0, 0.0, 10.0, 0.0)),
        seg(0.0, 0.0, -5.0, 0.0)
    );
}

// ---- 境界の条件（cutters_except と同じ） -------------------------------------------

/// 横の線分 1 本と、それを縦に横切る境界 1 つだけの図面。境界の置き方を変えて試す。
fn crossing(kind: &str) -> (Document, EntityId) {
    let mut doc = Document::new();
    let target = add(
        &mut doc,
        vec![(Geometry::Line(seg(0.0, 0.0, 10.0, 0.0)), LayerId::ZERO)],
    )[0];
    let vertical = Geometry::Line(seg(5.0, -5.0, 5.0, 5.0));
    match kind {
        "plain" => {
            add(&mut doc, vec![(vertical, LayerId::ZERO)]);
        }
        "locked" => {
            let layer = add_layer(&mut doc, "LOCKED");
            add(&mut doc, vec![(vertical, layer)]);
            doc.apply(Box::new(SetLayerProperties::new(layer).locked(true)))
                .expect("ロックできるはず");
        }
        "hidden" => {
            let layer = add_layer(&mut doc, "HIDDEN");
            add(&mut doc, vec![(vertical, layer)]);
            doc.apply(Box::new(SetLayerProperties::new(layer).visible(false)))
                .expect("隠せるはず");
        }
        "instance" => {
            // 中身が縦の線分 1 本の部品を、同じ位置に置く（#60: 今は境界にならない）。
            doc.apply(Box::new(DefineComponent::new(
                "COMPONENT",
                "縦",
                Point2::ORIGIN,
                vec![Entity::new(vertical, LayerId::ZERO)],
            )))
            .expect("定義を作れるはず");
            let def = doc.definitions().by_name("縦").expect("作ったはず");
            doc.apply(Box::new(InsertInstance::new(
                "INSERT",
                def,
                Placement::at(Point2::ORIGIN),
                LayerId::ZERO,
            )))
            .expect("配置できるはず");
        }
        other => panic!("{other}"),
    }
    (doc, target)
}

/// ロックされたレイヤの図形は境界になり、非表示のレイヤの図形とインスタンスはならない。
/// プレビューの有無と、実行の成否が一致する（`cutters_except` と同じ条件）。
#[test]
fn boundaries_follow_the_same_rules_as_the_command() {
    for (kind, expected) in [
        ("plain", true),
        ("locked", true),
        ("hidden", false),
        ("instance", false),
    ] {
        for command in ["TRIM", "EXTEND"] {
            let (mut doc, target) = crossing(kind);
            // TRIM は右寄りを切る。EXTEND は右端が縦線より手前にあるよう短くした線で試す。
            let (id, at) = if command == "TRIM" {
                (target, Point2::new(8.0, 0.0))
            } else {
                let short = add(
                    &mut doc,
                    vec![(Geometry::Line(seg(0.0, 1.0, 3.0, 1.0)), LayerId::ZERO)],
                )[0];
                (short, Point2::new(2.5, 1.0))
            };
            let got = preview(command, &mut doc, &mut Boundaries::default(), id, at);
            assert_eq!(got.is_some(), expected, "{kind} {command}: プレビュー");
            let result = if command == "TRIM" {
                doc.apply(Box::new(TrimEntity::new("TRIM", id, at)))
            } else {
                doc.apply(Box::new(ExtendEntity::new("EXTEND", id, at)))
            };
            assert_eq!(
                result.is_ok(),
                expected,
                "{kind} {command}: 実行 {result:?}"
            );
        }
    }
}

/// 線分以外（円）には結果プレビューを出さない（強調だけ）。
#[test]
fn non_lines_have_no_preview() {
    let mut doc = Document::new();
    let ids = add(
        &mut doc,
        vec![
            (
                Geometry::Circle(Circle::new(Point2::ORIGIN, 5.0)),
                LayerId::ZERO,
            ),
            (Geometry::Line(seg(-10.0, 0.0, 10.0, 0.0)), LayerId::ZERO),
        ],
    );
    let at = Point2::new(0.0, 5.0);
    for command in ["TRIM", "EXTEND"] {
        assert_eq!(
            preview(command, &mut doc, &mut Boundaries::default(), ids[0], at),
            None,
            "{command}"
        );
    }
}

/// TRIM / EXTEND 以外のツールは結果プレビューを出さない（既定の `None`）。
#[test]
fn other_tools_have_no_preview() {
    let (mut doc, target) = crossing("plain");
    let at = Point2::new(8.0, 0.0);
    assert!(preview("TRIM", &mut doc, &mut Boundaries::default(), target, at).is_some());
    let mut s = Session::new();
    s.start_command_from_ui("FILLET", &mut doc);
    assert!(s.wants_entity(), "前提: 図形を指す段階");
    assert_eq!(
        s.entity_preview(
            &ClickTarget::Entity { id: target, at },
            &doc,
            &mut Boundaries::default()
        ),
        None
    );
}

// ---- プレビューと実行結果の一致（乱数） --------------------------------------------

/// 線分を中心に、円・円弧・ポリライン・作図線・インスタンスを混ぜ、一部を非表示・ロックのレイヤに置く。
/// 返り値は図面と、対象にする線分（表示中）の ID。
fn random_drawing(rng: &mut Lcg) -> (Document, Vec<EntityId>) {
    let mut doc = Document::new();
    let locked = add_layer(&mut doc, "LOCKED");
    let hidden = add_layer(&mut doc, "HIDDEN");

    // 長い 2 本の線分の部品。インスタンスを置くと、いろいろな線分を横切る。
    doc.apply(Box::new(DefineComponent::new(
        "COMPONENT",
        "十字",
        Point2::ORIGIN,
        vec![
            Entity::new(Geometry::Line(seg(-40.0, 0.0, 40.0, 0.0)), LayerId::ZERO),
            Entity::new(Geometry::Line(seg(0.0, -40.0, 0.0, 40.0)), LayerId::ZERO),
        ],
    )))
    .expect("定義を作れるはず");
    let def = doc.definitions().by_name("十字").expect("作ったはず");

    let mut geoms = Vec::new();
    let mut is_target = Vec::new();
    for _ in 0..40 {
        let c = Point2::new(rng.next_f64(-50.0, 50.0), rng.next_f64(-50.0, 50.0));
        let r = rng.next_f64(5.0, 30.0);
        let a0 = rng.next_f64(0.0, 2.0 * PI);
        let kind = rng.below(10);
        let geom = match kind {
            0..=4 => Geometry::Line(Line::new(c, c + Vec2::polar(a0, rng.next_f64(10.0, 70.0)))),
            5 => Geometry::Circle(Circle::new(c, r)),
            6 => Geometry::Arc(Arc::new(c, r, a0, a0 + rng.next_f64(0.5, 4.0))),
            7 => Geometry::Polyline(Polyline::new(
                (0..4)
                    .map(|_| c + Vec2::new(rng.next_f64(-30.0, 30.0), rng.next_f64(-30.0, 30.0)))
                    .collect(),
                rng.below(2) == 0,
            )),
            8 if rng.below(3) == 0 => {
                Geometry::Xline(Xline::new(c, Vec2::from_angle(a0)).expect("単位ベクトル"))
            }
            _ => Geometry::Instance(cad_core::component::Instance::new(
                def,
                Placement::new(c, a0, 1.0, false).expect("有効な配置"),
            )),
        };
        let layer = match rng.below(8) {
            0 => locked,
            1 => hidden,
            _ => LayerId::ZERO,
        };
        is_target.push(matches!(geom, Geometry::Line(_)) && layer != hidden);
        geoms.push((geom, layer));
    }
    let ids = add(&mut doc, geoms);
    doc.apply(Box::new(SetLayerProperties::new(locked).locked(true)))
        .expect("ロックできるはず");
    doc.apply(Box::new(SetLayerProperties::new(hidden).visible(false)))
        .expect("隠せるはず");
    let targets = ids
        .into_iter()
        .zip(is_target)
        .filter_map(|(id, t)| t.then_some(id))
        .collect();
    (doc, targets)
}

/// 線分上の点（と、少し外れた点）。クリック位置として試す。
fn clicks_on(rng: &mut Lcg, line: &Line) -> Vec<Point2> {
    let normal = line.dir().map_or(Vec2::new(0.0, 1.0), Vec2::perp);
    (0..6)
        .map(|_| line.point_at(rng.next_f64(0.0, 1.0)) + normal * rng.next_f64(-0.5, 0.5))
        .collect()
}

/// `parts` を `target` の向きに揃えて並べ、`target` の始点から終点まで隙間も重なりも無く
/// つながっているか。
fn tiles(target: &Line, parts: &[Line]) -> bool {
    let mut parts: Vec<Line> = parts
        .iter()
        .map(|p| {
            if target.closest_param(p.a) <= target.closest_param(p.b) {
                *p
            } else {
                Line::new(p.b, p.a)
            }
        })
        .collect();
    parts.sort_by(|p, q| {
        target
            .closest_param(p.a)
            .total_cmp(&target.closest_param(q.a))
    });
    let mut at = target.a;
    for p in &parts {
        if !p.a.eq_tol(at) {
            return false;
        }
        at = p.b;
    }
    at.eq_tol(target.b)
}

/// TRIM: プレビューで消える部分と、実際に適用して残った線分を合わせると、元の線分に
/// ぴったり戻る（消える部分 = 元 − 残り）。プレビューが出ないのは、実行が失敗するときだけ。
#[test]
fn trim_preview_matches_the_result() {
    let (mut ok, mut failed) = (0, 0);
    for seed in 1..=8 {
        let mut rng = Lcg::new(seed);
        let (mut doc, targets) = random_drawing(&mut rng);
        let mut boundaries = Boundaries::default();
        for id in targets {
            let target = line_of(&doc, id).expect("線分");
            for at in clicks_on(&mut rng, &target) {
                let shown = preview("TRIM", &mut doc, &mut boundaries, id, at);
                let before: Vec<EntityId> = doc.entities().ids().collect();
                let result = doc.apply(Box::new(TrimEntity::new("TRIM", id, at)));
                let Some(shown) = shown else {
                    assert!(result.is_err(), "seed {seed}: プレビューなし → 失敗する");
                    failed += 1;
                    continue;
                };
                assert!(result.is_ok(), "seed {seed}: プレビューあり → 成功する");
                assert!(shown.added.is_empty());
                let removed = only_line(&shown.removed);
                let mut parts: Vec<Line> = new_ids(&doc, &before)
                    .into_iter()
                    .map(|n| line_of(&doc, n).expect("残りは線分"))
                    .collect();
                assert!(doc.entities().get(id).is_none(), "元の線分は消える");
                parts.push(removed);
                assert!(
                    tiles(&target, &parts),
                    "seed {seed}: 消える部分 {removed:?} と残り {parts:?} が {target:?} にならない"
                );
                doc.undo().expect("戻せるはず");
                ok += 1;
            }
        }
    }
    assert!(
        ok > 100 && failed > 20,
        "成功 {ok} 件・失敗 {failed} 件を試している"
    );
}

/// EXTEND: 実際に伸ばした線分 = 元の線分 + プレビューで伸びる部分。
/// プレビューが出ないのは、実行が失敗するときだけ。
#[test]
fn extend_preview_matches_the_result() {
    let (mut ok, mut failed) = (0, 0);
    for seed in 11..=18 {
        let mut rng = Lcg::new(seed);
        let (mut doc, targets) = random_drawing(&mut rng);
        let mut boundaries = Boundaries::default();
        for id in targets {
            let target = line_of(&doc, id).expect("線分");
            for at in clicks_on(&mut rng, &target) {
                let shown = preview("EXTEND", &mut doc, &mut boundaries, id, at);
                let result = doc.apply(Box::new(ExtendEntity::new("EXTEND", id, at)));
                let Some(shown) = shown else {
                    assert!(result.is_err(), "seed {seed}: プレビューなし → 失敗する");
                    failed += 1;
                    continue;
                };
                assert!(result.is_ok(), "seed {seed}: プレビューあり → 成功する");
                assert!(shown.removed.is_empty());
                let added = only_line(&shown.added);
                let got = line_of(&doc, id).expect("伸ばした線分");
                assert!(
                    tiles(&got, &[target, added]),
                    "seed {seed}: {got:?} != {target:?} + {added:?}"
                );
                // 伸びる部分は元の端のどちらかから始まる。
                assert!(added.a.eq_tol(target.a) || added.a.eq_tol(target.b));
                doc.undo().expect("戻せるはず");
                ok += 1;
            }
        }
    }
    assert!(
        ok > 50 && failed > 20,
        "成功 {ok} 件・失敗 {failed} 件を試している"
    );
}

// ---- キャッシュ ---------------------------------------------------------------

/// 境界の列は、版番号と対象が同じなら作り直さない。どちらかが変われば作り直す。
#[test]
fn boundaries_are_reused_per_revision_and_target() {
    let (mut doc, target) = crossing("plain");
    let other = add(
        &mut doc,
        vec![(Geometry::Line(seg(0.0, 3.0, 10.0, 3.0)), LayerId::ZERO)],
    )[0];
    let mut b = Boundaries::default();
    assert_eq!(b.except(&doc, target).len(), 2);
    assert_eq!(b.except(&doc, target).len(), 2);
    assert_eq!(b.built, 1, "同じなら使い回す");
    assert_eq!(b.except(&doc, other).len(), 2);
    assert_eq!(b.built, 2, "対象が変われば作り直す");
    add(
        &mut doc,
        vec![(Geometry::Line(seg(0.0, 9.0, 10.0, 9.0)), LayerId::ZERO)],
    );
    assert_eq!(b.except(&doc, other).len(), 3, "足した図形が入る");
    assert_eq!(b.built, 3, "版番号が変われば作り直す");
    doc.undo().expect("戻せるはず");
    assert_eq!(b.except(&doc, other).len(), 2, "戻した図形は入らない");
    assert_eq!(b.built, 4, "Undo でも版番号は進む");
}
