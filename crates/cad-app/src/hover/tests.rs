//! `hover` のテスト（GPU なし）。

use std::f64::consts::PI;
use std::time::{Duration, Instant};

use cad_core::command::{AddEntities, AddLayer, CreateGroup, DefineComponent, SetLayerProperties};
use cad_core::component::{Instance, Placement};
use cad_core::geom::{Arc, Circle, Line, Point2, Polyline, Vec2, Xline};
use cad_core::layer::AciColor;
use cad_core::{Document, Entity, EntityId, Geometry, LayerId};

use super::*;
use crate::selection::{pick_at, ScanAll};
use crate::session::Session;
use crate::test_util::Lcg;

fn line(x0: f64, y0: f64, x1: f64, y1: f64) -> Geometry {
    Geometry::Line(Line::new(Point2::new(x0, y0), Point2::new(x1, y1)))
}

fn add(doc: &mut Document, geoms: Vec<(Geometry, LayerId)>) -> Vec<EntityId> {
    let before: Vec<EntityId> = doc.entities().ids().collect();
    let entities = geoms
        .into_iter()
        .map(|(g, layer)| Entity::new(g, layer))
        .collect();
    doc.apply(Box::new(AddEntities::many("TEST", entities)))
        .expect("追加できるはず");
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

// ---- 索引で絞った候補と全走査の一致 ------------------------------------------

/// 乱数で作った図面と、拾いを試す位置。
struct Random {
    doc: Document,
    probes: Vec<Point2>,
}

/// 線分・円・円弧・ポリライン・インスタンス・同じ形の重なり（同距離）・作図線を混ぜ、
/// 一部をロック・非表示のレイヤに置く。
fn random_drawing(rng: &mut Lcg, n: usize, with_xline: bool) -> Random {
    let mut doc = Document::new();
    let locked = add_layer(&mut doc, "LOCKED");
    let hidden = add_layer(&mut doc, "HIDDEN");

    // 線分 2 本と円 1 つの部品。中身は図面に入らない（定義の側に入る）。
    doc.apply(Box::new(DefineComponent::new(
        "COMPONENT",
        "部品",
        Point2::ORIGIN,
        vec![
            Entity::new(line(0.0, 0.0, 6.0, 0.0), LayerId::ZERO),
            Entity::new(line(6.0, 0.0, 6.0, 4.0), LayerId::ZERO),
            Entity::new(
                Geometry::Circle(Circle::new(Point2::new(2.0, 2.0), 1.5)),
                LayerId::ZERO,
            ),
        ],
    )))
    .expect("定義を作れるはず");
    let def = doc.definitions().by_name("部品").expect("作ったはず");

    let mut geoms: Vec<(Geometry, LayerId)> = Vec::with_capacity(n);
    let mut probes = Vec::new();
    for _ in 0..n {
        let c = Point2::new(rng.next_f64(-100.0, 100.0), rng.next_f64(-100.0, 100.0));
        let r = rng.next_f64(1.0, 15.0);
        let a0 = rng.next_f64(0.0, 2.0 * PI);
        let geom = match rng.below(7) {
            0 => Geometry::Line(Line::new(
                c,
                c + Vec2::new(rng.next_f64(-20.0, 20.0), rng.next_f64(-20.0, 20.0)),
            )),
            1 => Geometry::Circle(Circle::new(c, r)),
            2 => Geometry::Arc(Arc::new(c, r, a0, a0 + rng.next_f64(0.2, 5.0))),
            3 => Geometry::Polyline(Polyline::new(
                (0..4)
                    .map(|_| c + Vec2::new(rng.next_f64(-15.0, 15.0), rng.next_f64(-15.0, 15.0)))
                    .collect(),
                rng.below(2) == 0,
            )),
            4 => Geometry::Instance(Instance::new(
                def,
                Placement::new(c, a0, rng.next_f64(0.5, 3.0), rng.below(2) == 0)
                    .expect("有効な配置"),
            )),
            // 直前と同じ形（ちょうど同じ距離で重なる）。
            5 if !geoms.is_empty() => geoms[geoms.len() - 1].0.clone(),
            6 if with_xline && rng.below(20) == 0 => {
                Geometry::Xline(Xline::new(c, Vec2::new(a0.cos(), a0.sin())).expect("単位ベクトル"))
            }
            _ => Geometry::Circle(Circle::new(c, r)),
        };
        let layer = match rng.below(10) {
            0 => locked,
            1 => hidden,
            _ => LayerId::ZERO,
        };
        // 図形の上と、そのすぐ近く。
        probes.push(c);
        probes.push(c + Vec2::new(r, 0.0));
        geoms.push((geom, layer));
    }
    if with_xline {
        // 少なくとも 1 本は作図線を入れる（#61: 索引の分割が止まる場合）。
        let x = Xline::new(Point2::new(3.0, -7.0), Vec2::new(0.6, 0.8)).expect("単位ベクトル");
        geoms.push((Geometry::Xline(x), LayerId::ZERO));
        probes.push(Point2::new(3.0, -7.0));
    }
    add(&mut doc, geoms);
    doc.apply(Box::new(SetLayerProperties::new(locked).locked(true)))
        .expect("ロックできるはず");
    doc.apply(Box::new(SetLayerProperties::new(hidden).visible(false)))
        .expect("隠せるはず");

    for _ in 0..200 {
        probes.push(Point2::new(
            rng.next_f64(-120.0, 120.0),
            rng.next_f64(-120.0, 120.0),
        ));
    }
    Random { doc, probes }
}

/// 索引で絞った候補を採点した結果が、全走査（`pick_at`）と必ず一致する。
/// 線分・円・円弧・ポリライン・作図線・インスタンス・ロック・非表示・同距離の重なりを含む。
#[test]
fn index_pick_matches_a_full_scan() {
    for (seed, with_xline) in [(1, false), (2, true), (3, false), (4, true)] {
        let mut rng = Lcg::new(seed);
        let Random { doc, probes } = random_drawing(&mut rng, 400, with_xline);
        let mut index = PickIndex::default();
        let mut hits = 0;
        for p in probes {
            for tol in [0.3, 1.0, 4.0] {
                let jitter = Vec2::new(rng.next_f64(-tol, tol), rng.next_f64(-tol, tol));
                let at = p + jitter;
                let expected = pick_at(&doc, at, tol);
                assert_eq!(
                    index.pick(&doc, at, tol),
                    expected,
                    "seed {seed} xline {with_xline}: {at:?} tol {tol}"
                );
                hits += usize::from(expected.is_some());
            }
        }
        assert!(hits > 100, "拾える位置を十分に試している: {hits}");
    }
}

/// 同じ距離で重なった図形は、候補の並び順によらず後から作られた方が拾われる。
#[test]
fn ties_go_to_the_later_entity_whatever_the_candidate_order() {
    let mut doc = Document::new();
    let ids = add(
        &mut doc,
        vec![
            (line(0.0, 0.0, 10.0, 0.0), LayerId::ZERO),
            (line(0.0, 0.0, 10.0, 0.0), LayerId::ZERO),
            (line(0.0, 0.0, 10.0, 0.0), LayerId::ZERO),
        ],
    );
    let at = Point2::new(5.0, 0.5);
    for order in [
        vec![ids[0], ids[1], ids[2]],
        vec![ids[2], ids[1], ids[0]],
        vec![ids[1], ids[2], ids[0], ids[2]],
    ] {
        assert_eq!(
            selection::pick_among(&doc, order.clone(), at, 1.0),
            Some(ids[2]),
            "{order:?}"
        );
    }
    assert_eq!(pick_at(&doc, at, 1.0), Some(ids[2]), "全走査も後勝ち");
    assert_eq!(PickIndex::default().pick(&doc, at, 1.0), Some(ids[2]));
}

/// 図面が変わったら索引を作り直す（足した図形が拾え、消した図形は拾われない）。
#[test]
fn the_index_follows_the_revision() {
    let mut doc = Document::new();
    let a = add(&mut doc, vec![(line(0.0, 0.0, 10.0, 0.0), LayerId::ZERO)])[0];
    let mut index = PickIndex::default();
    let far = Point2::new(5.0, 50.0);
    assert_eq!(index.pick(&doc, Point2::new(5.0, 0.0), 1.0), Some(a));
    assert_eq!(index.pick(&doc, far, 1.0), None);

    let b = add(&mut doc, vec![(line(0.0, 50.0, 10.0, 50.0), LayerId::ZERO)])[0];
    assert_eq!(index.pick(&doc, far, 1.0), Some(b), "足した図形が拾える");

    doc.undo().expect("戻せるはず");
    assert_eq!(index.pick(&doc, far, 1.0), None, "消えた図形は拾われない");
}

// ---- クリックの段階ごとの強調 -------------------------------------------------

/// 線分 2 本のグループ（1 本目はロックされたレイヤ L1 にもできる）と、離れた線分 1 本。
fn grouped(lock_member: bool) -> (Document, Vec<EntityId>) {
    let mut doc = Document::new();
    let l1 = add_layer(&mut doc, "L1");
    let ids = add(
        &mut doc,
        vec![
            (line(0.0, 0.0, 10.0, 0.0), l1),
            (line(0.0, 5.0, 10.0, 5.0), LayerId::ZERO),
            (line(0.0, 20.0, 10.0, 20.0), LayerId::ZERO),
        ],
    );
    doc.apply(Box::new(CreateGroup::new(
        "GROUP",
        "g",
        vec![ids[0], ids[1]],
    )))
    .expect("グループにできるはず");
    if lock_member {
        doc.apply(Box::new(SetLayerProperties::new(l1).locked(true)))
            .expect("ロックできるはず");
    }
    (doc, ids)
}

fn highlighted_at(s: &Session, doc: &Document, at: Point2) -> Vec<EntityId> {
    let mut h = Hover::new();
    h.update(s, doc, Some(at), 1.0);
    h.highlighted().to_vec()
}

/// 待機中は、グループの一員に乗せるとグループの（編集できる）一員すべてが強調される。
/// クリックした後の選択と同じ集合。
#[test]
fn idle_hover_highlights_what_a_click_selects() {
    for lock_member in [false, true] {
        let (mut doc, ids) = grouped(lock_member);
        let mut s = Session::new();
        let at = Point2::new(5.0, 5.0);
        let mut got = highlighted_at(&s, &doc, at);
        got.sort();
        let expected = if lock_member {
            vec![ids[1]]
        } else {
            vec![ids[0], ids[1]]
        };
        assert_eq!(got, expected, "ロック {lock_member}");

        s.handle_click(at, false, 1.0, &mut doc, &mut ScanAll);
        assert_eq!(s.selection.to_vec(), got, "クリックで選ばれる集合と同じ");
    }
}

/// 図形を指す段階（TRIM）ではグループに広げず、乗せた 1 つだけを強調する。
#[test]
fn trim_highlights_only_the_entity_under_the_cursor() {
    let (mut doc, ids) = grouped(false);
    let mut s = Session::new();
    s.start_command_from_ui("TRIM", &mut doc);
    assert_eq!(s.pick_stage(), PickStage::Entity, "前提");
    assert_eq!(
        highlighted_at(&s, &doc, Point2::new(5.0, 5.0)),
        vec![ids[1]]
    );
    assert!(highlighted_at(&s, &doc, Point2::new(5.0, 12.0)).is_empty());
}

/// 点の入力待ちでは何も強調しない。
#[test]
fn point_input_highlights_nothing() {
    let (mut doc, _) = grouped(false);
    let mut s = Session::new();
    s.start_command_from_ui("LINE", &mut doc);
    assert_eq!(s.pick_stage(), PickStage::Point, "前提");
    assert!(highlighted_at(&s, &doc, Point2::new(5.0, 5.0)).is_empty());
}

/// 強調しない場面（カーソルがキャンバスの外など）では `None` を渡し、何も強調しない。
#[test]
fn no_position_highlights_nothing() {
    let (doc, ids) = grouped(false);
    let s = Session::new();
    let mut h = Hover::new();
    h.update(&s, &doc, Some(Point2::new(5.0, 20.0)), 1.0);
    assert_eq!(h.highlighted(), [ids[2]]);
    h.update(&s, &doc, None, 1.0);
    assert!(h.highlighted().is_empty());
}

/// 図面・位置・半径・段階が同じなら計算し直さない。どれかが変われば計算し直す。
#[test]
fn the_result_is_reused_while_nothing_changes() {
    let (mut doc, ids) = grouped(false);
    let mut s = Session::new();
    let mut h = Hover::new();
    let at = Point2::new(5.0, 20.0);

    h.update(&s, &doc, Some(at), 1.0);
    h.update(&s, &doc, Some(at), 1.0);
    assert_eq!(h.computed, 1, "同じなら使い回す");
    // 一度外へ出て戻っても、同じなら使い回す。
    h.update(&s, &doc, None, 1.0);
    h.update(&s, &doc, Some(at), 1.0);
    assert_eq!(h.computed, 1);

    h.update(&s, &doc, Some(Point2::new(5.0, 20.5)), 1.0);
    assert_eq!(h.computed, 2, "位置が変われば計算し直す");
    h.update(&s, &doc, Some(Point2::new(5.0, 20.5)), 2.0);
    assert_eq!(h.computed, 3, "半径が変われば計算し直す");

    s.start_command_from_ui("TRIM", &mut doc);
    h.update(&s, &doc, Some(Point2::new(5.0, 20.5)), 2.0);
    assert_eq!(h.computed, 4, "段階が変われば計算し直す");
    assert_eq!(h.highlighted(), [ids[2]]);

    doc.apply(Box::new(cad_core::command::DeleteEntities::new(
        "ERASE",
        vec![ids[2]],
    )))
    .expect("消せるはず");
    h.update(&s, &doc, Some(Point2::new(5.0, 20.5)), 2.0);
    assert_eq!(h.computed, 5, "図面が変われば計算し直す");
    assert!(h.highlighted().is_empty(), "消えた図形は強調しない");
}

// ---- 性能 -------------------------------------------------------------------

/// 1 万図形の図面（作図線を 1 本混ぜられる）。
fn ten_thousand(with_xline: bool) -> Document {
    let mut rng = Lcg::new(2024);
    let mut geoms = Vec::with_capacity(10_001);
    for i in 0..10_000u32 {
        let cx = rng.next_f64(-1e5, 1e5);
        let cy = rng.next_f64(-1e5, 1e5);
        let geom = match i % 4 {
            0 => line(cx, cy, cx + 10.0, cy + 10.0),
            1 => Geometry::Circle(Circle::new(Point2::new(cx, cy), 5.0)),
            2 => Geometry::Arc(Arc::new(Point2::new(cx, cy), 5.0, 0.0, PI)),
            _ => Geometry::Polyline(Polyline::new(
                vec![
                    Point2::new(cx, cy),
                    Point2::new(cx + 5.0, cy),
                    Point2::new(cx + 5.0, cy + 5.0),
                ],
                false,
            )),
        };
        geoms.push((geom, LayerId::ZERO));
    }
    if with_xline {
        let x = Xline::new(Point2::ORIGIN, Vec2::new(1.0, 0.0)).expect("単位ベクトル");
        geoms.push((Geometry::Xline(x), LayerId::ZERO));
    }
    let mut doc = Document::new();
    add(&mut doc, geoms);
    doc
}

/// 性能確認（受け入れ基準）: 1 万図形で、ホバーの計算 1 回が平均 16ms（60fps の 1 フレーム）未満。
///
/// 毎回違う位置にして使い回しを効かせない。索引の構築（図面が変わったときだけ）は別に測って出す。
/// 作図線が 1 本あると索引の分割が止まりほぼ線形になる（#61）が、その場合も予算に収まること。
/// デバッグビルドの実測なので悲観的。平均で判定するので単発のブレでは落ちない。
#[test]
fn hover_meets_the_frame_budget_with_ten_thousand_entities() {
    for with_xline in [false, true] {
        let doc = ten_thousand(with_xline);
        let s = Session::new();
        let mut h = Hover::new();
        let mut rng = Lcg::new(7);

        let start = Instant::now();
        h.update(&s, &doc, Some(Point2::ORIGIN), 5.0);
        println!(
            "hover first update (index build) xline {with_xline}: {:?}",
            start.elapsed()
        );

        let iterations = 200u32;
        let mut total = Duration::ZERO;
        for _ in 0..iterations {
            let at = Point2::new(rng.next_f64(-1e5, 1e5), rng.next_f64(-1e5, 1e5));
            let start = Instant::now();
            h.update(&s, &doc, Some(at), 5.0);
            total += start.elapsed();
        }
        assert_eq!(h.computed, 1 + 200, "毎回計算している");
        let avg = total / iterations;
        println!("hover update average xline {with_xline}: {avg:?}");
        assert!(
            avg.as_millis() < 16,
            "xline {with_xline}: average hover update took {avg:?}, expected < 16ms"
        );
    }
}

// ---- 結果プレビュー（段階 2） -----------------------------------------------------

/// 横の線分（0〜20）と、x = 10 の縦の線分。横の線分の右寄りに乗せる位置も返す。
fn cross() -> (Document, Vec<EntityId>, Point2) {
    let mut doc = Document::new();
    let ids = add(
        &mut doc,
        vec![
            (line(0.0, 0.0, 20.0, 0.0), LayerId::ZERO),
            (line(10.0, -5.0, 10.0, 5.0), LayerId::ZERO),
        ],
    );
    (doc, ids, Point2::new(15.0, 0.0))
}

fn removed_of(h: &Hover) -> Vec<Geometry> {
    h.entity_preview()
        .map(|p| p.removed.clone())
        .unwrap_or_default()
}

/// TRIM 中に乗せると、強調している線分の消える部分が出る。待機中・カーソルが外では出ない。
#[test]
fn trim_hover_shows_the_part_that_goes_away() {
    let (mut doc, ids, at) = cross();
    let mut s = Session::new();
    let mut h = Hover::new();
    h.update(&s, &doc, Some(at), 1.0);
    assert_eq!(h.highlighted(), [ids[0]], "前提: 待機中も強調はする");
    assert!(h.entity_preview().is_none(), "待機中は結果プレビューなし");

    s.start_command_from_ui("TRIM", &mut doc);
    h.update(&s, &doc, Some(at), 1.0);
    assert_eq!(h.highlighted(), [ids[0]]);
    assert_eq!(removed_of(&h), vec![line(10.0, 0.0, 20.0, 0.0)]);

    h.update(&s, &doc, None, 1.0);
    assert!(h.entity_preview().is_none(), "カーソルが外なら出さない");
}

/// 同じ位置・同じ図面のまま TRIM から EXTEND へ替えたら、プレビューも替わる
/// （強調の結果は使い回すが、結果プレビューはツールに聞き直す）。
#[test]
fn switching_tools_in_place_updates_the_preview() {
    let mut doc = Document::new();
    let ids = add(
        &mut doc,
        vec![
            (line(0.0, 0.0, 6.0, 0.0), LayerId::ZERO),
            (line(10.0, -5.0, 10.0, 5.0), LayerId::ZERO),
            (line(3.0, -5.0, 3.0, 5.0), LayerId::ZERO),
        ],
    );
    let at = Point2::new(5.0, 0.0);
    let mut s = Session::new();
    let mut h = Hover::new();
    s.start_command_from_ui("TRIM", &mut doc);
    h.update(&s, &doc, Some(at), 1.0);
    assert_eq!(h.highlighted(), [ids[0]]);
    assert_eq!(removed_of(&h), vec![line(3.0, 0.0, 6.0, 0.0)]);

    s.start_command_from_ui("EXTEND", &mut doc);
    h.update(&s, &doc, Some(at), 1.0);
    assert_eq!(h.computed, 1, "前提: 強調の結果は使い回している");
    let preview = h.entity_preview().expect("EXTEND のプレビュー");
    assert!(preview.removed.is_empty());
    assert_eq!(preview.added, vec![line(6.0, 0.0, 10.0, 0.0)]);
}

/// 同じ線分の上で動かしても境界の列は作り直さない。別の線分・図面の変更で作り直す。
#[test]
fn boundaries_are_built_once_per_target_and_revision() {
    let (mut doc, ids, _) = cross();
    let mut s = Session::new();
    s.start_command_from_ui("TRIM", &mut doc);
    let mut h = Hover::new();
    for x in [12.0, 14.0, 16.0, 18.0, 4.0] {
        h.update(&s, &doc, Some(Point2::new(x, 0.0)), 1.0);
        assert!(h.entity_preview().is_some(), "x = {x}");
    }
    assert_eq!(h.boundaries().built, 1, "同じ線分の上では使い回す");
    h.update(&s, &doc, Some(Point2::new(10.0, 3.0)), 1.0);
    assert_eq!(h.highlighted(), [ids[1]], "前提: 縦の線分");
    assert_eq!(h.boundaries().built, 2, "対象が変われば作り直す");

    // 横の線分の右側を切る（版番号が進む）。
    s.handle_click(Point2::new(15.0, 0.0), false, 1.0, &mut doc, &mut ScanAll);
    h.update(&s, &doc, Some(Point2::new(10.0, 3.0)), 1.0);
    assert_eq!(h.boundaries().built, 3, "図面が変われば作り直す");
}

/// 性能確認: 1 万図形で、TRIM の強調と結果プレビュー（同じ線分の上を動かす）が
/// 平均 16ms 未満。境界の列の複製は対象が替わったときだけで、動かすたびの計算は交点だけ。
#[test]
fn trim_preview_meets_the_frame_budget_with_ten_thousand_entities() {
    let mut doc = ten_thousand(false);
    // 1 万図形を横切る長い線分（交点が多い場合）。
    let long = add(&mut doc, vec![(line(-1e5, 0.0, 1e5, 0.0), LayerId::ZERO)])[0];
    let mut s = Session::new();
    s.start_command_from_ui("TRIM", &mut doc);
    let mut h = Hover::new();
    let mut rng = Lcg::new(11);

    let start = Instant::now();
    h.update(&s, &doc, Some(Point2::ORIGIN), 5.0);
    println!("trim preview first update: {:?}", start.elapsed());

    let iterations = 200u32;
    let mut total = Duration::ZERO;
    let mut shown = 0;
    for _ in 0..iterations {
        let at = Point2::new(rng.next_f64(-1e5, 1e5), 0.0);
        let start = Instant::now();
        h.update(&s, &doc, Some(at), 5.0);
        total += start.elapsed();
        shown += usize::from(h.highlighted() == [long] && h.entity_preview().is_some());
    }
    let avg = total / iterations;
    println!("trim preview update average: {avg:?} (shown {shown})");
    assert!(shown > 150, "長い線分のプレビューを出している: {shown}");
    assert!(
        avg.as_millis() < 16,
        "average trim preview update took {avg:?}, expected < 16ms"
    );
}
