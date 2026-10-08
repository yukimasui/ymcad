//! `grips` のテスト（GPU なし）。

use std::f64::consts::{FRAC_PI_2, PI, TAU};

use cad_core::command::{AddEntities, DefineComponent, ReplaceGeometries};
use cad_core::component::{Instance, Placement};
use cad_core::geom::tolerance::{eq_len, EPS_LEN};
use cad_core::geom::{Arc, Circle, Line, Point2, Polyline, Vec2, Xline};
use cad_core::{Document, Entity, EntityId, Geometry, LayerId};

use super::*;
use crate::test_util::Lcg;

fn p(x: f64, y: f64) -> Point2 {
    Point2::new(x, y)
}

fn line(x0: f64, y0: f64, x1: f64, y1: f64) -> Geometry {
    Geometry::Line(Line::new(p(x0, y0), p(x1, y1)))
}

fn assert_pt(actual: Point2, expected: Point2, what: &str) {
    assert!(
        eq_len(actual.x, expected.x) && eq_len(actual.y, expected.y),
        "{what}: {actual:?} != {expected:?}"
    );
}

fn handles(geom: &Geometry) -> Vec<Handle> {
    grips_of(geom).into_iter().map(|(h, _)| h).collect()
}

/// 図形にインスタンスの定義を 1 つ用意して、その ID を返す。
fn with_definition(doc: &mut Document) -> cad_core::DefinitionId {
    doc.apply(Box::new(DefineComponent::new(
        "COMPONENT",
        "部品",
        Point2::ORIGIN,
        vec![Entity::new(line(0.0, 0.0, 5.0, 0.0), LayerId::ZERO)],
    )))
    .expect("定義を作れる");
    doc.definitions().by_name("部品").expect("作った")
}

// ---- 位置と種類 ------------------------------------------------------------

#[test]
fn line_has_two_ends_and_a_midpoint() {
    let g = line(0.0, 0.0, 10.0, 4.0);
    let got = grips_of(&g);
    assert_eq!(
        handles(&g),
        vec![Handle::LineStart, Handle::LineEnd, Handle::LineMid]
    );
    assert_pt(got[0].1, p(0.0, 0.0), "始点");
    assert_pt(got[1].1, p(10.0, 4.0), "終点");
    assert_pt(got[2].1, p(5.0, 2.0), "中点");
}

#[test]
fn circle_has_a_center_and_four_quadrants() {
    let g = Geometry::Circle(Circle::new(p(1.0, 2.0), 3.0));
    let got = grips_of(&g);
    assert_eq!(got[0], (Handle::CircleCenter, p(1.0, 2.0)));
    // 四分点はちょうどの値（cos(90°) の誤差が無い）。
    assert_eq!(got[1], (Handle::CircleQuadrant(0), p(4.0, 2.0)));
    assert_eq!(got[2], (Handle::CircleQuadrant(1), p(1.0, 5.0)));
    assert_eq!(got[3], (Handle::CircleQuadrant(2), p(-2.0, 2.0)));
    assert_eq!(got[4], (Handle::CircleQuadrant(3), p(1.0, -1.0)));
    assert_eq!(got.len(), 5);
}

#[test]
fn arc_has_ends_midpoint_and_center() {
    let g = Geometry::Arc(Arc::new(p(0.0, 0.0), 2.0, 0.0, FRAC_PI_2));
    let got = grips_of(&g);
    assert_eq!(
        handles(&g),
        vec![
            Handle::ArcStart,
            Handle::ArcEnd,
            Handle::ArcMid,
            Handle::ArcCenter
        ]
    );
    assert_pt(got[0].1, p(2.0, 0.0), "始点");
    assert_pt(got[1].1, p(0.0, 2.0), "終点");
    let h = 2.0 / 2.0_f64.sqrt();
    assert_pt(got[2].1, p(h, h), "中点");
    assert_pt(got[3].1, p(0.0, 0.0), "中心");
}

/// 1 周の円弧（開始角 = 終了角）は中心だけ。
#[test]
fn a_full_arc_only_has_its_center() {
    for (s, e) in [(0.0, 0.0), (0.0, TAU), (1.0, 1.0)] {
        let g = Geometry::Arc(Arc::new(p(3.0, 3.0), 2.0, s, e));
        assert_eq!(grips_of(&g), vec![(Handle::ArcCenter, p(3.0, 3.0))]);
        assert_eq!(
            apply(&g, Handle::ArcStart, p(0.0, 0.0)),
            Err(GripError::NotAGrip),
            "端点は動かせない"
        );
    }
}

#[test]
fn polyline_xline_and_instance() {
    let pl = Geometry::Polyline(Polyline::new(
        vec![p(0.0, 0.0), p(5.0, 0.0), p(5.0, 5.0)],
        false,
    ));
    assert_eq!(
        grips_of(&pl),
        vec![
            (Handle::Vertex(0), p(0.0, 0.0)),
            (Handle::Vertex(1), p(5.0, 0.0)),
            (Handle::Vertex(2), p(5.0, 5.0)),
            (Handle::Edge(0), p(2.5, 0.0)),
            (Handle::Edge(1), p(5.0, 2.5)),
        ],
        "頂点と、開いたポリラインの辺（頂点の数 − 1 本）の中点"
    );
    let x = Geometry::Xline(Xline::horizontal(p(2.0, 3.0)));
    assert_eq!(grips_of(&x), vec![(Handle::XlineOrigin, p(2.0, 3.0))]);

    let mut doc = Document::new();
    let def = with_definition(&mut doc);
    let i = Geometry::Instance(Instance::new(def, Placement::at(p(7.0, 8.0))));
    assert_eq!(grips_of(&i), vec![(Handle::InstanceOrigin, p(7.0, 8.0))]);
}

#[test]
fn labels_say_what_happens() {
    assert_eq!(Handle::LineStart.label(), "端点を動かす");
    assert_eq!(Handle::ArcEnd.label(), "端点を動かす");
    assert_eq!(Handle::LineMid.label(), "図形ごと移動");
    assert_eq!(Handle::CircleCenter.label(), "図形ごと移動");
    assert_eq!(Handle::CircleQuadrant(2).label(), "半径を変える");
    assert_eq!(Handle::Vertex(3).label(), "頂点を動かす");
    assert_eq!(Handle::Edge(0).label(), "辺を動かす");
}

/// 閉じたポリラインは最後の辺（最後の頂点 → 先頭の頂点）の中点も持つ。
#[test]
fn a_closed_polyline_has_a_midpoint_on_its_closing_edge() {
    let rect = Geometry::Polyline(Polyline::rectangle(p(0.0, 0.0), p(4.0, 2.0)));
    let edges: Vec<(Handle, Point2)> = grips_of(&rect)
        .into_iter()
        .filter(|(h, _)| matches!(h, Handle::Edge(_)))
        .collect();
    assert_eq!(
        edges,
        vec![
            (Handle::Edge(0), p(2.0, 0.0)),
            (Handle::Edge(1), p(4.0, 1.0)),
            (Handle::Edge(2), p(2.0, 2.0)),
            (Handle::Edge(3), p(0.0, 1.0)),
        ]
    );
}

// ---- 形の変え方 ------------------------------------------------------------

#[test]
fn moving_a_line_end_moves_only_that_end() {
    let g = line(0.0, 0.0, 10.0, 0.0);
    assert_eq!(
        apply(&g, Handle::LineEnd, p(3.0, 4.0)),
        Ok(line(0.0, 0.0, 3.0, 4.0))
    );
    assert_eq!(
        apply(&g, Handle::LineStart, p(-1.0, 2.0)),
        Ok(line(-1.0, 2.0, 10.0, 0.0))
    );
}

#[test]
fn moving_the_midpoint_or_center_translates_the_whole_shape() {
    let g = line(0.0, 0.0, 10.0, 0.0);
    assert_eq!(
        apply(&g, Handle::LineMid, p(5.0, 3.0)),
        Ok(line(0.0, 3.0, 10.0, 3.0))
    );
    let c = Geometry::Circle(Circle::new(p(0.0, 0.0), 2.0));
    assert_eq!(
        apply(&c, Handle::CircleCenter, p(1.0, 1.0)),
        Ok(Geometry::Circle(Circle::new(p(1.0, 1.0), 2.0)))
    );
    let a = Geometry::Arc(Arc::new(p(0.0, 0.0), 2.0, 0.0, PI));
    assert_eq!(
        apply(&a, Handle::ArcCenter, p(0.0, -1.0)),
        Ok(Geometry::Arc(Arc::new(p(0.0, -1.0), 2.0, 0.0, PI)))
    );
    let x = Geometry::Xline(Xline::horizontal(p(2.0, 3.0)));
    assert_eq!(
        apply(&x, Handle::XlineOrigin, p(0.0, 0.0)),
        Ok(Geometry::Xline(Xline::horizontal(p(0.0, 0.0))))
    );

    let mut doc = Document::new();
    let def = with_definition(&mut doc);
    let i = Geometry::Instance(Instance::new(def, Placement::at(p(7.0, 8.0))));
    let Ok(Geometry::Instance(moved)) = apply(&i, Handle::InstanceOrigin, p(1.0, 2.0)) else {
        panic!("インスタンスは動かせる");
    };
    assert_eq!(moved.placement.origin, p(1.0, 2.0));
    assert_eq!(moved.definition, def, "定義は変わらない");
}

#[test]
fn a_quadrant_sets_the_radius_to_the_distance_from_the_center() {
    let c = Geometry::Circle(Circle::new(p(1.0, 1.0), 2.0));
    assert_eq!(
        apply(&c, Handle::CircleQuadrant(1), p(4.0, 5.0)),
        Ok(Geometry::Circle(Circle::new(p(1.0, 1.0), 5.0)))
    );
}

#[test]
fn a_polyline_vertex_moves_alone_and_keeps_the_count() {
    let pl = Geometry::Polyline(Polyline::new(
        vec![p(0.0, 0.0), p(5.0, 0.0), p(5.0, 5.0)],
        true,
    ));
    let got = apply(&pl, Handle::Vertex(1), p(6.0, -1.0));
    assert_eq!(
        got,
        Ok(Geometry::Polyline(Polyline::new(
            vec![p(0.0, 0.0), p(6.0, -1.0), p(5.0, 5.0)],
            true
        )))
    );
}

/// 辺の中点は辺を平行移動する（両端の頂点を同じだけ動かす。頂点の数は変えない）。
#[test]
fn moving_an_edge_translates_both_of_its_ends() {
    let open = Geometry::Polyline(Polyline::new(
        vec![p(0.0, 0.0), p(4.0, 0.0), p(4.0, 2.0)],
        false,
    ));
    assert_eq!(
        apply(&open, Handle::Edge(0), p(2.0, -1.0)),
        Ok(Geometry::Polyline(Polyline::new(
            vec![p(0.0, -1.0), p(4.0, -1.0), p(4.0, 2.0)],
            false
        ))),
        "開いたポリラインの最初の辺"
    );
    // 矩形の右の辺を右へ 3 → 幅が 4 から 7 に。
    let rect = Geometry::Polyline(Polyline::rectangle(p(0.0, 0.0), p(4.0, 2.0)));
    assert_eq!(
        apply(&rect, Handle::Edge(1), p(7.0, 1.0)),
        Ok(Geometry::Polyline(Polyline::rectangle(
            p(0.0, 0.0),
            p(7.0, 2.0)
        )))
    );
    // 閉じたポリラインの最後の辺（頂点 3 → 頂点 0）。
    assert_eq!(
        apply(&rect, Handle::Edge(3), p(-1.0, 1.0)),
        Ok(Geometry::Polyline(Polyline::new(
            vec![p(-1.0, 0.0), p(4.0, 0.0), p(4.0, 2.0), p(-1.0, 2.0)],
            true
        ))),
        "最後の辺は最後の頂点と先頭の頂点を動かす"
    );
    assert_eq!(dimension_base(&rect, Handle::Edge(1)), Some(p(4.0, 1.0)));
    assert!(tracks(Handle::Edge(1)), "辺は直交・極が効く");
    assert!(!Handle::Edge(0).moves_whole());
}

/// 辺を動かして線分が 1 本も残らない形（全部の頂点が重なる）は断る。
#[test]
fn moving_an_edge_onto_the_rest_is_refused() {
    // 長さ 0 の辺（頂点 0 と 1 が重なる）を頂点 2 へ重ねる。
    let pl = Geometry::Polyline(Polyline::new(
        vec![p(0.0, 0.0), p(0.0, 0.0), p(3.0, 0.0)],
        false,
    ));
    assert_eq!(
        apply(&pl, Handle::Edge(0), p(3.0, 0.0)),
        Err(GripError::ZeroLength)
    );
}

/// 円弧の端点は、もう一方の端点と中点を通る 3 点円弧になる（ユーザー判断 7）。
#[test]
fn an_arc_end_passes_through_the_midpoint_and_the_other_end() {
    let a = Arc::new(p(0.0, 0.0), 2.0, 0.0, PI);
    let g = Geometry::Arc(a);
    let to = p(-3.0, 0.5);
    let Ok(Geometry::Arc(moved)) = apply(&g, Handle::ArcEnd, to) else {
        panic!("動かせる");
    };
    let ends = [moved.start_point(), moved.end_point()];
    assert!(ends.iter().any(|e| e.eq_tol(a.start_point())), "始点は残る");
    assert!(ends.iter().any(|e| e.eq_tol(to)), "終点は行き先");
    assert!(
        is_zero(moved.dist_to(a.mid_point())),
        "元の中点を通る: {}",
        moved.dist_to(a.mid_point())
    );
}

/// 円弧の中点は、両端を通り行き先を通る 3 点円弧になる。
#[test]
fn an_arc_midpoint_bends_the_arc_through_the_new_point() {
    let a = Arc::new(p(0.0, 0.0), 2.0, 0.0, PI);
    let g = Geometry::Arc(a);
    let to = p(0.0, 5.0);
    let Ok(Geometry::Arc(moved)) = apply(&g, Handle::ArcMid, to) else {
        panic!("動かせる");
    };
    assert!(is_zero(moved.dist_to(to)), "行き先を通る");
    let ends = [moved.start_point(), moved.end_point()];
    assert!(ends.iter().any(|e| e.eq_tol(a.start_point())));
    assert!(ends.iter().any(|e| e.eq_tol(a.end_point())));
}

fn is_zero(d: f64) -> bool {
    // 3 点からの作り直しは丸めが乗るので、少し緩めて見る（トレランスの 1000 倍）。
    d.abs() <= EPS_LEN * 1000.0
}

// ---- 断るケース ------------------------------------------------------------

#[test]
fn refusals() {
    let l = line(0.0, 0.0, 10.0, 0.0);
    assert_eq!(
        apply(&l, Handle::LineEnd, p(0.0, 0.0)),
        Err(GripError::ZeroLength),
        "線分の長さ 0"
    );
    let c = Geometry::Circle(Circle::new(p(1.0, 1.0), 2.0));
    assert_eq!(
        apply(&c, Handle::CircleQuadrant(0), p(1.0, 1.0)),
        Err(GripError::ZeroRadius),
        "半径 0"
    );
    let a = Arc::new(p(0.0, 0.0), 2.0, 0.0, PI);
    let g = Geometry::Arc(a);
    assert_eq!(
        apply(&g, Handle::ArcEnd, a.start_point()),
        Err(GripError::ArcEndsMeet),
        "円弧の両端が重なる"
    );
    assert_eq!(
        apply(&g, Handle::ArcStart, a.end_point()),
        Err(GripError::ArcEndsMeet),
        "始点を終点へ"
    );
    // 中点ともう一方の端点を結ぶ直線の上。
    let mid = a.mid_point();
    let other = a.start_point();
    let on_line = mid + (mid - other) * 0.5;
    assert_eq!(
        apply(&g, Handle::ArcEnd, on_line),
        Err(GripError::Collinear),
        "3 点が一直線"
    );
    assert_eq!(
        apply(&g, Handle::ArcEnd, mid),
        Err(GripError::Collinear),
        "中点に重ねる"
    );
    assert_eq!(
        apply(&g, Handle::ArcMid, p(0.0, 0.0)),
        Err(GripError::Collinear),
        "中点を弦の上へ"
    );
    let two = Geometry::Polyline(Polyline::new(vec![p(0.0, 0.0), p(1.0, 0.0)], false));
    assert_eq!(
        apply(&two, Handle::Vertex(1), p(0.0, 0.0)),
        Err(GripError::ZeroLength),
        "2 頂点のポリラインの長さ 0"
    );
    assert_eq!(
        apply(&l, Handle::CircleCenter, p(1.0, 1.0)),
        Err(GripError::NotAGrip),
        "種類の違うグリップ"
    );
    assert_eq!(
        apply(&l, Handle::LineEnd, p(f64::NAN, 0.0)),
        Err(GripError::NotFinite),
        "非有限の行き先"
    );
    // 行き先は有限でも、計算の途中で桁があふれる（中心からの距離が無限大）。
    // 個別の検査では止まらず、最後の `Geometry::validate` が止める。
    assert!(
        matches!(
            apply(&c, Handle::CircleQuadrant(0), p(f64::MAX, f64::MAX)),
            Err(GripError::Invalid(_))
        ),
        "あふれた半径"
    );
}

/// 半径がとても大きい円弧で、両端の距離はトレランスより大きいのに開始角と終了角が角度のトレランスで
/// 一致する場合も、1 周の円弧にせずに断る。
#[test]
fn a_huge_arc_whose_ends_meet_in_angle_is_refused() {
    let r = 1.0e6;
    let a = Arc::new(p(0.0, 0.0), r, 0.0, PI);
    let g = Geometry::Arc(a);
    // 始点から角度でごくわずか（角度のトレランスの半分）手前。距離は r × 角度で長さのトレランスを超える。
    let tiny = cad_core::geom::tolerance::EPS_ANGLE * 0.5;
    let to = p(r * (-tiny).cos(), r * (-tiny).sin());
    assert!(!to.eq_tol(a.start_point()), "前提: 距離では重ならない");
    assert_eq!(apply(&g, Handle::ArcEnd, to), Err(GripError::ArcEndsMeet));
}

// ---- 基点 ------------------------------------------------------------------

#[test]
fn bases_come_from_the_shape() {
    let l = line(0.0, 0.0, 10.0, 0.0);
    assert_eq!(dimension_base(&l, Handle::LineEnd), Some(p(0.0, 0.0)));
    assert_eq!(dimension_base(&l, Handle::LineStart), Some(p(10.0, 0.0)));
    assert_eq!(dimension_base(&l, Handle::LineMid), Some(p(5.0, 0.0)));
    let c = Geometry::Circle(Circle::new(p(1.0, 1.0), 2.0));
    assert_eq!(
        dimension_base(&c, Handle::CircleQuadrant(0)),
        Some(p(1.0, 1.0))
    );
    assert!(!tracks(Handle::CircleQuadrant(0)), "四分点は直交・極を外す");
    assert!(tracks(Handle::LineEnd));
    let open = Geometry::Polyline(Polyline::new(
        vec![p(0.0, 0.0), p(5.0, 0.0), p(5.0, 5.0)],
        false,
    ));
    assert_eq!(dimension_base(&open, Handle::Vertex(0)), Some(p(5.0, 0.0)));
    assert_eq!(dimension_base(&open, Handle::Vertex(2)), Some(p(5.0, 0.0)));
    assert_eq!(dimension_base(&open, Handle::Vertex(1)), Some(p(5.0, 0.0)));
    let closed = Geometry::Polyline(Polyline::new(
        vec![p(0.0, 0.0), p(5.0, 0.0), p(5.0, 5.0)],
        true,
    ));
    assert_eq!(
        dimension_base(&closed, Handle::Vertex(0)),
        Some(p(0.0, 0.0))
    );
    let a = Arc::new(p(0.0, 0.0), 2.0, 0.0, PI);
    assert_pt(
        dimension_base(&Geometry::Arc(a), Handle::ArcEnd).expect("ある"),
        a.end_point(),
        "円弧の端点は元の位置",
    );
    assert_eq!(dimension_base(&l, Handle::CircleCenter), None);
}

// ---- 当たり ----------------------------------------------------------------

fn ids(n: usize) -> Vec<EntityId> {
    let mut doc = Document::new();
    let entities = (0..n)
        .map(|_| Entity::new(line(0.0, 0.0, 1.0, 0.0), LayerId::ZERO))
        .collect();
    doc.apply(Box::new(AddEntities::many("TEST", entities)))
        .expect("足せる");
    doc.entities().ids().collect()
}

#[test]
fn hit_takes_the_nearest_inside_the_square_and_the_newest_on_a_tie() {
    let ids = ids(2);
    let grips = vec![
        Grip {
            id: ids[0],
            handle: Handle::LineStart,
            at: p(0.0, 0.0),
        },
        Grip {
            id: ids[1],
            handle: Handle::LineStart,
            at: p(0.0, 0.0),
        },
        Grip {
            id: ids[0],
            handle: Handle::LineEnd,
            at: p(1.0, 0.0),
        },
    ];
    let got = hit(&grips, p(0.1, 0.0), 0.5).expect("当たる");
    assert_eq!(
        got.representative().id,
        ids[1],
        "同じ位置なら新しい方が代表"
    );
    assert_eq!(got.grips().len(), 2, "同じ位置の 2 つを束ねる");
    let got = hit(&grips, p(0.9, 0.0), 0.5).expect("当たる");
    assert_eq!(got.representative().handle, Handle::LineEnd, "近い方");
    assert_eq!(got.grips().len(), 1);
    // 正方形の角（距離は半径より大きいが、正方形の中）。
    assert!(hit(&grips, p(0.45, 0.45), 0.5).is_some(), "正方形の角");
    assert!(hit(&grips, p(0.0, 0.6), 0.5).is_none(), "正方形の外");
}

// ---- 乱数: 恒等性と、`ReplaceGeometries` との一致 --------------------------

/// 全種類を混ぜた乱数の図形。
fn random_geometry(rng: &mut Lcg, def: cad_core::DefinitionId) -> Geometry {
    let c = p(rng.next_f64(-100.0, 100.0), rng.next_f64(-100.0, 100.0));
    let r = rng.next_f64(0.5, 30.0);
    match rng.below(7) {
        0 => Geometry::Line(Line::new(
            c,
            c + Vec2::polar(rng.next_f64(0.0, TAU), rng.next_f64(0.5, 50.0)),
        )),
        1 => Geometry::Circle(Circle::new(c, r)),
        2 => {
            let s = rng.next_f64(0.0, TAU);
            Geometry::Arc(Arc::new(c, r, s, s + rng.next_f64(0.1, TAU - 0.1)))
        }
        3 => {
            let s = rng.next_f64(-TAU, TAU);
            // 1 周の円弧（DXF から読んだ 0°→360° の形も混ぜる）。
            Geometry::Arc(Arc::new(
                c,
                r,
                s,
                if rng.below(2) == 0 { s } else { s + TAU },
            ))
        }
        4 => {
            let n = 2 + rng.below(4) as usize;
            let vertices = (0..n)
                .map(|_| c + Vec2::new(rng.next_f64(-20.0, 20.0), rng.next_f64(-20.0, 20.0)))
                .collect();
            Geometry::Polyline(Polyline::new(vertices, n >= 3 && rng.below(2) == 0))
        }
        5 => Geometry::Xline(Xline::at_angle(c, rng.next_f64(0.0, TAU))),
        _ => Geometry::Instance(Instance::new(def, Placement::at(c))),
    }
}

/// 掴んだグリップの行き先の候補。乱数の点に加えて、断られやすい点（もう一方の端点・中点・中心・
/// 一直線上・ほぼ重なる点）と、元の位置を混ぜる。
fn targets(rng: &mut Lcg, geom: &Geometry, handle: Handle, from: Point2) -> Vec<Point2> {
    let mut v = vec![
        from,
        p(rng.next_f64(-150.0, 150.0), rng.next_f64(-150.0, 150.0)),
        from + Vec2::new(rng.next_f64(-1.0, 1.0), rng.next_f64(-1.0, 1.0)),
        // 計算の途中で桁があふれる行き先（距離・移動量が無限大になる）。
        p(f64::MAX, f64::MAX),
        p(-f64::MAX, f64::MAX),
    ];
    for (_, q) in grips_of(geom) {
        v.push(q);
        v.push(q + Vec2::new(EPS_LEN * 0.5, 0.0));
    }
    if let Geometry::Arc(a) = geom {
        let mid = a.mid_point();
        let other = if handle == Handle::ArcStart {
            a.end_point()
        } else {
            a.start_point()
        };
        v.push(mid + (mid - other) * rng.next_f64(-2.0, 2.0));
        v.push(a.start_point().lerp(a.end_point(), rng.next_f64(-1.0, 2.0)));
    }
    if let Geometry::Circle(c) = geom {
        v.push(c.center + Vec2::new(EPS_LEN * 0.5, 0.0));
    }
    v
}

/// 元の位置へ動かすと、元の形そのもの（ビットまで同じ）が返る。
#[test]
fn moving_a_grip_to_where_it_is_returns_the_same_shape() {
    let mut doc = Document::new();
    let def = with_definition(&mut doc);
    let mut rng = Lcg::new(30);
    for _ in 0..2000 {
        let g = random_geometry(&mut rng, def);
        for (handle, at) in grips_of(&g) {
            assert_eq!(apply(&g, handle, at), Ok(g.clone()), "{handle:?} of {g:?}");
        }
    }
}

/// 検査を一切しない素朴な形の変え方（`apply` と同じ計算の順）。円弧の 3 点が一直線なら `None`。
fn naive(geom: &Geometry, handle: Handle, to: Point2) -> Option<Geometry> {
    let from = position(geom, handle)?;
    if to.eq_tol(from) {
        return Some(geom.clone());
    }
    Some(match (geom, handle) {
        (Geometry::Line(l), Handle::LineStart) => Geometry::Line(Line::new(to, l.b)),
        (Geometry::Line(l), Handle::LineEnd) => Geometry::Line(Line::new(l.a, to)),
        (Geometry::Circle(c), Handle::CircleQuadrant(_)) => {
            Geometry::Circle(Circle::new(c.center, c.center.dist(to)))
        }
        (Geometry::Arc(a), Handle::ArcStart) => {
            Geometry::Arc(Arc::from_3_points(to, a.mid_point(), a.end_point())?)
        }
        (Geometry::Arc(a), Handle::ArcEnd) => {
            Geometry::Arc(Arc::from_3_points(a.start_point(), a.mid_point(), to)?)
        }
        (Geometry::Arc(a), Handle::ArcMid) => {
            Geometry::Arc(Arc::from_3_points(a.start_point(), to, a.end_point())?)
        }
        (Geometry::Polyline(pl), Handle::Vertex(i)) => {
            let mut v = pl.vertices.clone();
            v[i] = to;
            Geometry::Polyline(Polyline::new(v, pl.closed))
        }
        (Geometry::Polyline(pl), Handle::Edge(i)) => {
            let mut v = pl.vertices.clone();
            let n = v.len();
            v[i] += to - from;
            v[(i + 1) % n] += to - from;
            Geometry::Polyline(Polyline::new(v, pl.closed))
        }
        _ => geom.translated(to - from),
    })
}

/// 円弧の端点・中点を動かして、両端が重なるか（`apply` が意図して断る。図面は 1 周として受け付ける）。
/// 両端の点が重なる場合と、3 点から作った円弧の開始角と終了角が一致する場合。
fn arc_ends_meet(geom: &Geometry, handle: Handle, to: Point2, naive: Option<&Geometry>) -> bool {
    let Geometry::Arc(a) = geom else {
        return false;
    };
    let ends_given = match handle {
        Handle::ArcStart => to.eq_tol(a.end_point()),
        Handle::ArcEnd => to.eq_tol(a.start_point()),
        Handle::ArcMid => a.start_point().eq_tol(a.end_point()),
        // 中心は図形ごと移動するだけ（1 周の円弧は 1 周のまま動かせる）。
        _ => return false,
    };
    let full = matches!(
        naive,
        Some(Geometry::Arc(n)) if cad_core::geom::tolerance::eq_angle(n.start_angle, n.end_angle)
    );
    ends_given || full
}

/// `apply` が成功する ⇔ 素朴に作った形を実際の図面で `ReplaceGeometries` に渡して成功する
/// （円弧の両端が重なる場合と 3 点が一直線の場合を除く。前者は図面が 1 周の円弧として受け付けて
/// しまうので `apply` が意図して断り、後者は形が作れない）。成功したら図面の形は `apply` の結果。
///
/// 全種類の図形を混ぜ、乱数の行き先に、断られやすい点（他のグリップの位置・ほぼ重なる点・
/// 中心・一直線上）を混ぜる。
#[test]
fn apply_succeeds_exactly_when_replace_geometries_does() {
    let mut doc = Document::new();
    let def = with_definition(&mut doc);
    let mut rng = Lcg::new(4242);
    let (mut ok, mut refused_by_doc, mut arc_refusals) = (0, 0, 0);
    for _ in 0..400 {
        let g = random_geometry(&mut rng, def);
        doc.apply(Box::new(AddEntities::one(
            "TEST",
            Entity::new(g.clone(), LayerId::ZERO),
        )))
        .expect("足せる");
        let id = doc.entities().ids().last().expect("足した");
        for (handle, from) in grips_of(&g) {
            for to in targets(&mut rng, &g, handle, from) {
                let ours = apply(&g, handle, to);
                if to.eq_tol(from) {
                    assert_eq!(ours, Ok(g.clone()), "元の位置は恒等");
                    continue;
                }
                let what = format!("{handle:?} → {to:?} of {g:?}");
                let n = naive(&g, handle, to);
                if arc_ends_meet(&g, handle, to, n.as_ref()) {
                    assert_eq!(ours, Err(GripError::ArcEndsMeet), "{what}");
                    arc_refusals += 1;
                    continue;
                }
                let Some(n) = n else {
                    assert_eq!(ours, Err(GripError::Collinear), "{what}");
                    arc_refusals += 1;
                    continue;
                };
                let result = doc.apply(Box::new(ReplaceGeometries::one("GRIP", id, n.clone())));
                assert_eq!(
                    ours.is_ok(),
                    result.is_ok(),
                    "{what}: {ours:?} / {result:?}"
                );
                if result.is_ok() {
                    assert_eq!(ours.as_ref().ok(), Some(&n), "{what}: 同じ形");
                    assert_eq!(doc.entities().get(id).map(|e| &e.geom), Some(&n));
                    doc.undo().expect("戻せる");
                    assert_eq!(doc.entities().get(id).map(|e| &e.geom), Some(&g));
                    ok += 1;
                } else {
                    refused_by_doc += 1;
                }
            }
        }
    }
    assert!(
        ok > 1000 && refused_by_doc > 50 && arc_refusals > 50,
        "どの場合も十分に試した: ok {ok}, 図面が断った {refused_by_doc}, 円弧 {arc_refusals}"
    );
}

/// 断ったケースのうち、`validate` で断られる種類（長さ 0・半径 0・非有限）は、同じ形を
/// `ReplaceGeometries` に渡しても断られる（`apply` だけが厳しすぎない）。
#[test]
fn shapes_refused_for_degeneracy_are_refused_by_the_document_too() {
    let mut doc = Document::new();
    let ids: Vec<EntityId> = {
        doc.apply(Box::new(AddEntities::many(
            "TEST",
            vec![
                Entity::new(line(0.0, 0.0, 10.0, 0.0), LayerId::ZERO),
                Entity::new(
                    Geometry::Circle(Circle::new(p(1.0, 1.0), 2.0)),
                    LayerId::ZERO,
                ),
                Entity::new(
                    Geometry::Polyline(Polyline::new(vec![p(0.0, 0.0), p(1.0, 0.0)], false)),
                    LayerId::ZERO,
                ),
            ],
        )))
        .expect("足せる");
        doc.entities().ids().collect()
    };
    let cases = [
        (ids[0], line(0.0, 0.0, 0.0, 0.0)),
        (ids[1], Geometry::Circle(Circle::new(p(1.0, 1.0), 0.0))),
        (
            ids[2],
            Geometry::Polyline(Polyline::new(vec![p(0.0, 0.0), p(0.0, 0.0)], false)),
        ),
    ];
    for (id, geom) in cases {
        assert!(
            doc.apply(Box::new(ReplaceGeometries::one("GRIP", id, geom.clone())))
                .is_err(),
            "{geom:?}"
        );
    }
}

// ---- 重なったグリップの束（段階 2） --------------------------------------------

/// 図形の列を図面に足し、全部のグリップと ID を返す（`Session::grips` と同じ並び: 図形ごとに
/// `grips_of` の順）。
fn scene(doc: &mut Document, geoms: &[Geometry]) -> (Vec<EntityId>, Vec<Grip>) {
    let before: Vec<EntityId> = doc.entities().ids().collect();
    doc.apply(Box::new(AddEntities::many(
        "TEST",
        geoms
            .iter()
            .map(|g| Entity::new(g.clone(), LayerId::ZERO))
            .collect(),
    )))
    .expect("足せる");
    let ids: Vec<EntityId> = doc
        .entities()
        .ids()
        .filter(|id| !before.contains(id))
        .collect();
    let grips = ids
        .iter()
        .zip(geoms)
        .flat_map(|(id, g)| {
            grips_of(g).into_iter().map(move |(handle, at)| Grip {
                id: *id,
                handle,
                at,
            })
        })
        .collect();
    (ids, grips)
}

/// 線分 4 本の矩形（(0,0)-(10,0)-(10,5)-(0,5)、つながった順）。
fn four_lines() -> Vec<Geometry> {
    vec![
        line(0.0, 0.0, 10.0, 0.0),
        line(10.0, 0.0, 10.0, 5.0),
        line(10.0, 5.0, 0.0, 5.0),
        line(0.0, 5.0, 0.0, 0.0),
    ]
}

/// 束の図形ごとの形を `to` へ動かした結果（`apply_group` の引数を作って呼ぶ）。
fn move_group(doc: &Document, group: &GripGroup, to: Point2) -> Result<Vec<Geometry>, PartError> {
    let parts = group.parts();
    let originals: Vec<Geometry> = parts
        .iter()
        .map(|(id, _)| doc.entities().get(*id).expect("ある").geom.clone())
        .collect();
    let args: Vec<(&Geometry, &[Handle])> = originals
        .iter()
        .zip(&parts)
        .map(|(g, (_, h))| (g, h.as_slice()))
        .collect();
    apply_group(&args, to)
}

/// 矩形の角では、隣り合う 2 本の端点が 1 つの束になる。代表は EntityId の大きい方。
#[test]
fn a_shared_corner_is_one_group_of_two_grips() {
    let mut doc = Document::new();
    let (ids, grips) = scene(&mut doc, &four_lines());
    let group = hit(&grips, p(10.1, 0.1), 0.5).expect("当たる");
    assert_eq!(group.grips().len(), 2, "{group:?}");
    let rep = group.representative();
    assert_eq!(
        (rep.id, rep.handle),
        (ids[1], Handle::LineStart),
        "代表は新しい方"
    );
    assert_eq!(
        group.parts(),
        vec![
            (ids[1], vec![Handle::LineStart]),
            (ids[0], vec![Handle::LineEnd])
        ]
    );
    assert_eq!(group.label(), "端点を動かす（2 個）");
    // 辺の中点は 1 本だけ。
    let mid = hit(&grips, p(5.0, 0.0), 0.5).expect("当たる");
    assert_eq!(mid.grips().len(), 1);
    assert_eq!(mid.label(), "図形ごと移動");
}

/// 矩形の角を動かすと、隣り合う 2 本がつながったまま動く（ほかの 2 本は変わらない）。
#[test]
fn moving_a_shared_corner_keeps_the_rectangle_connected() {
    let mut doc = Document::new();
    let (_, grips) = scene(&mut doc, &four_lines());
    let group = hit(&grips, p(10.0, 0.0), 0.5).expect("当たる");
    let moved = move_group(&doc, &group, p(12.0, -1.0)).expect("動かせる");
    assert_eq!(
        moved,
        vec![line(12.0, -1.0, 10.0, 5.0), line(0.0, 0.0, 12.0, -1.0)],
        "代表（2 本目の始点）と 1 本目の終点がどちらも行き先へ"
    );
}

/// 種類が混ざった束は「点を動かす」。線分の端点と円の中心が重なっていれば、端点が動き、円は移動する。
#[test]
fn a_mixed_group_moves_each_by_its_own_rule() {
    let mut doc = Document::new();
    let (ids, grips) = scene(
        &mut doc,
        &[
            line(0.0, 0.0, 10.0, 0.0),
            Geometry::Circle(Circle::new(p(10.0, 0.0), 2.0)),
        ],
    );
    let group = hit(&grips, p(10.0, 0.0), 0.5).expect("当たる");
    assert_eq!(group.representative().id, ids[1], "代表は円の中心");
    assert_eq!(group.label(), "点を動かす（2 個）");
    let moved = move_group(&doc, &group, p(10.0, 4.0)).expect("動かせる");
    assert_eq!(
        moved,
        vec![
            Geometry::Circle(Circle::new(p(10.0, 4.0), 2.0)),
            line(0.0, 0.0, 10.0, 4.0),
        ]
    );
}

/// 始点と終点が同じ位置の開いたポリラインは、その角の頂点 2 つを一緒に動かす（閉じた形のまま）。
#[test]
fn coincident_vertices_of_one_polyline_move_together() {
    let mut doc = Document::new();
    let pl = Geometry::Polyline(Polyline::new(
        vec![p(0.0, 0.0), p(4.0, 0.0), p(4.0, 3.0), p(0.0, 0.0)],
        false,
    ));
    let (ids, grips) = scene(&mut doc, &[pl]);
    let group = hit(&grips, p(0.0, 0.0), 0.5).expect("当たる");
    assert_eq!(
        group.parts(),
        vec![(ids[0], vec![Handle::Vertex(0), Handle::Vertex(3)])]
    );
    assert_eq!(group.label(), "頂点を動かす", "図形は 1 つ");
    let moved = move_group(&doc, &group, p(-1.0, -1.0)).expect("動かせる");
    assert_eq!(
        moved,
        vec![Geometry::Polyline(Polyline::new(
            vec![p(-1.0, -1.0), p(4.0, 0.0), p(4.0, 3.0), p(-1.0, -1.0)],
            false
        ))]
    );
}

/// 全部か無しか。1 つでも断られたら全体を断り、どの図形が断ったか（添字）と理由を返す。
#[test]
fn a_group_is_all_or_nothing() {
    let mut doc = Document::new();
    let arc = Arc::new(p(0.0, 0.0), 5.0, 0.0, PI);
    // 線分の終点を円弧の終点 (-5,0) に重ねる。行き先を円弧の始点 (5,0) にすると円弧の両端が重なる。
    let (ids, grips) = scene(&mut doc, &[line(-8.0, 3.0, -5.0, 0.0), Geometry::Arc(arc)]);
    let group = hit(&grips, arc.end_point(), 0.5).expect("当たる");
    assert_eq!(group.parts().len(), 2);
    assert_eq!(group.representative().id, ids[1]);
    let err = move_group(&doc, &group, arc.start_point()).expect_err("断る");
    assert_eq!(
        err,
        PartError {
            index: 0,
            error: GripError::ArcEndsMeet
        },
        "代表（円弧）が断った"
    );
    // 線分の側が断る場合（長さ 0）。
    let err = move_group(&doc, &group, p(-8.0, 3.0)).expect_err("断る");
    assert_eq!(
        err,
        PartError {
            index: 1,
            error: GripError::ZeroLength
        }
    );
}

/// 同じ位置でない（トレランスより離れた）グリップは束にしない。
#[test]
fn nearby_but_distinct_grips_are_not_grouped() {
    let mut doc = Document::new();
    let (ids, grips) = scene(
        &mut doc,
        &[line(0.0, 0.0, 10.0, 0.0), line(10.1, 0.0, 20.0, 0.0)],
    );
    let group = hit(&grips, p(10.02, 0.0), 0.5).expect("当たる");
    assert_eq!(group.grips().len(), 1);
    assert_eq!(group.representative().id, ids[0], "近い方");
}

/// 共有点を持つ図形を乱数で作る。共有点の列も返す。全種類を混ぜ、同じ図形の重なった頂点
/// （始点と終点が同じ開いたポリライン）や、辺の中点が共有点になるポリラインも混ぜる。
fn random_scene(rng: &mut Lcg, def: cad_core::DefinitionId) -> (Vec<Geometry>, Vec<Point2>) {
    let rnd = |rng: &mut Lcg| p(rng.next_f64(-100.0, 100.0), rng.next_f64(-100.0, 100.0));
    let shared: Vec<Point2> = (0..3).map(|_| rnd(rng)).collect();
    let mut out = Vec::new();
    for _ in 0..7 {
        let s = shared[rng.below(3) as usize];
        let s2 = shared[rng.below(3) as usize];
        let g = match rng.below(10) {
            0 => Geometry::Line(Line::new(s, rnd(rng))),
            1 => Geometry::Line(Line::new(rnd(rng), s)),
            2 => Geometry::Circle(Circle::new(s, rng.next_f64(0.5, 30.0))),
            3 => {
                // 端点が共有点の円弧。
                let c = s + Vec2::polar(rng.next_f64(0.0, TAU), rng.next_f64(1.0, 30.0));
                let at = (s - c).angle();
                let sweep = rng.next_f64(0.2, TAU - 0.2);
                if rng.below(2) == 0 {
                    Geometry::Arc(Arc::new(c, c.dist(s), at, at + sweep))
                } else {
                    Geometry::Arc(Arc::new(c, c.dist(s), at - sweep, at))
                }
            }
            4 => Geometry::Polyline(Polyline::new(vec![s, rnd(rng), rnd(rng)], false)),
            5 => Geometry::Polyline(Polyline::new(vec![s, rnd(rng), rnd(rng), s], false)),
            6 => Geometry::Polyline(Polyline::new(vec![rnd(rng), s, rnd(rng)], true)),
            7 => {
                // 辺の中点が共有点。
                let d = Vec2::new(rng.next_f64(-10.0, 10.0), rng.next_f64(-10.0, 10.0));
                Geometry::Polyline(Polyline::new(vec![s - d, s + d, rnd(rng)], false))
            }
            8 => {
                if s.eq_tol(s2) {
                    Geometry::Xline(Xline::at_angle(s, rng.next_f64(0.0, TAU)))
                } else {
                    Geometry::Line(Line::new(s, s2))
                }
            }
            _ => Geometry::Instance(Instance::new(def, Placement::at(s))),
        };
        out.push(g);
    }
    (out, shared)
}

/// 束の行き先の候補。乱数の点・元の位置・束の図形のほかのグリップ（断られやすい）・ほぼ重なる点。
fn group_targets(rng: &mut Lcg, doc: &Document, group: &GripGroup) -> Vec<Point2> {
    let from = group.representative().at;
    let mut v = vec![
        from,
        p(rng.next_f64(-150.0, 150.0), rng.next_f64(-150.0, 150.0)),
        from + Vec2::new(rng.next_f64(-1.0, 1.0), rng.next_f64(-1.0, 1.0)),
        from + Vec2::new(EPS_LEN * 0.5, 0.0),
        p(f64::MAX, f64::MAX),
    ];
    for (id, _) in group.parts() {
        let g = &doc.entities().get(id).expect("ある").geom;
        for (_, q) in grips_of(g) {
            v.push(q);
        }
        if let Geometry::Arc(a) = g {
            v.push(a.start_point().lerp(a.end_point(), rng.next_f64(-1.0, 2.0)));
        }
    }
    v
}

/// 元の位置へ動かすと、束のどの図形も元の形そのもの。
#[test]
fn moving_a_group_to_where_it_is_returns_the_same_shapes() {
    let mut doc = Document::new();
    let def = with_definition(&mut doc);
    let mut rng = Lcg::new(3030);
    let mut grouped = 0;
    for _ in 0..300 {
        let (geoms, shared) = random_scene(&mut rng, def);
        let (_, grips) = scene(&mut doc, &geoms);
        for s in shared {
            // 共有点を使った図形が無いこともある。
            let Some(group) = hit(&grips, s, 0.001) else {
                continue;
            };
            let originals: Vec<Geometry> = group
                .parts()
                .iter()
                .map(|(id, _)| doc.entities().get(*id).expect("ある").geom.clone())
                .collect();
            for g in group.grips() {
                assert_eq!(
                    move_group(&doc, &group, g.at),
                    Ok(originals.clone()),
                    "{group:?}"
                );
            }
            if group.parts().len() > 1 {
                grouped += 1;
            }
        }
    }
    assert!(grouped > 300, "束を十分に試した: {grouped}");
}

/// `apply_group` が成功する ⇔ 素朴に作った形の列を実際の図面で 1 回の `ReplaceGeometries` に渡して
/// 成功する（どれかの図形で円弧の両端が重なる・3 点が一直線になる場合は、`apply_group` が必ず断る）。
/// 成功したら図面の形は `apply_group` の結果で、Undo 1 回で全部戻る。
#[test]
fn apply_group_succeeds_exactly_when_replace_geometries_does() {
    let mut doc = Document::new();
    let def = with_definition(&mut doc);
    let mut rng = Lcg::new(4343);
    let (mut ok, mut refused_by_doc, mut excluded) = (0, 0, 0);
    for _ in 0..300 {
        let (geoms, shared) = random_scene(&mut rng, def);
        let (_, grips) = scene(&mut doc, &geoms);
        for s in shared {
            // 共有点を使った図形が無いこともある。
            let Some(group) = hit(&grips, s, 0.001) else {
                continue;
            };
            let parts = group.parts();
            for to in group_targets(&mut rng, &doc, &group) {
                let ours = move_group(&doc, &group, to);
                let what = format!("{group:?} → {to:?}");
                // 図形ごとに素朴に動かす。断られて当然のもの（円弧の両端・一直線）があれば
                // `apply_group` も断ること。
                let mut naive_all = Vec::new();
                let mut must_refuse = false;
                for (id, handles) in &parts {
                    let mut g = doc.entities().get(*id).expect("ある").geom.clone();
                    for h in handles {
                        let n = naive(&g, *h, to);
                        if arc_ends_meet(&g, *h, to, n.as_ref()) || n.is_none() {
                            must_refuse = true;
                            break;
                        }
                        g = n.expect("上で見た");
                    }
                    naive_all.push((*id, g));
                }
                if must_refuse {
                    assert!(ours.is_err(), "{what}: {ours:?}");
                    excluded += 1;
                    continue;
                }
                let before: Vec<Geometry> = parts
                    .iter()
                    .map(|(id, _)| doc.entities().get(*id).expect("ある").geom.clone())
                    .collect();
                let result = doc.apply(Box::new(ReplaceGeometries::new("GRIP", naive_all.clone())));
                assert_eq!(
                    ours.is_ok(),
                    result.is_ok(),
                    "{what}: {ours:?} / {result:?}"
                );
                if result.is_ok() {
                    let expected: Vec<Geometry> =
                        naive_all.iter().map(|(_, g)| g.clone()).collect();
                    assert_eq!(ours.as_ref().ok(), Some(&expected), "{what}: 同じ形");
                    doc.undo().expect("戻せる");
                    let after: Vec<Geometry> = parts
                        .iter()
                        .map(|(id, _)| doc.entities().get(*id).expect("ある").geom.clone())
                        .collect();
                    assert_eq!(after, before, "{what}: Undo 1 回で全部戻る");
                    ok += 1;
                } else {
                    refused_by_doc += 1;
                }
            }
        }
    }
    assert!(
        ok > 1000 && refused_by_doc > 50 && excluded > 50,
        "どの場合も十分に試した: ok {ok}, 図面が断った {refused_by_doc}, 円弧 {excluded}"
    );
}
