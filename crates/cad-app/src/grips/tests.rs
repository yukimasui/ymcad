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
        ]
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
    assert_eq!(got.id, ids[1], "同じ距離なら新しい方");
    let got = hit(&grips, p(0.9, 0.0), 0.5).expect("当たる");
    assert_eq!(got.handle, Handle::LineEnd, "近い方");
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
