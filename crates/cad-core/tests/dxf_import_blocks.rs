//! DXF のブロック（`BLOCK` / `INSERT`）の読み込み（Issue #109、ADR-0050）。
//!
//! 変換の検査は**解決後のワールド座標**（`component::resolve`）で期待値と比べる。
//! `Placement` の項目どうしを比べるだけだと、分解の順序を取り違えても
//! 「書いた値と読んだ値が同じ」で通ってしまうため。
//!
//! トレランスのマジックナンバー禁止規約はこのファイルにも適用される。

use std::f64::consts::{FRAC_PI_2, PI};

use cad_core::command::{DefineComponent, InsertInstance};
use cad_core::component::{resolve, Instance, Placement};
use cad_core::dxf::{read, write};
use cad_core::geom::tolerance::{eq_angle, eq_len};
use cad_core::geom::{Arc, Circle, Line, Point2};
use cad_core::{AciColor, ColorSpec, Document, Entity, Geometry, LayerId};

// ---- 手書きの DXF を組み立てるヘルパー ---------------------------------------

/// グループコード 1 組。
fn g(code: i32, v: impl std::fmt::Display) -> String {
    format!("{code}\n{v}\n")
}

fn line(a: (f64, f64), b: (f64, f64)) -> String {
    g(0, "LINE") + &g(8, "0") + &g(10, a.0) + &g(20, a.1) + &g(11, b.0) + &g(21, b.1)
}

fn arc(c: (f64, f64), r: f64, start_deg: f64, end_deg: f64, extra: &str) -> String {
    g(0, "ARC")
        + &g(8, "0")
        + extra
        + &g(10, c.0)
        + &g(20, c.1)
        + &g(40, r)
        + &g(50, start_deg)
        + &g(51, end_deg)
}

fn circle(c: (f64, f64), r: f64, extra: &str) -> String {
    g(0, "CIRCLE") + &g(8, "0") + extra + &g(10, c.0) + &g(20, c.1) + &g(40, r)
}

/// `INSERT`。`extra` に 41/42/50/210… などを足す。
fn insert(name: &str, at: (f64, f64), extra: &str) -> String {
    g(0, "INSERT") + &g(8, "0") + &g(2, name) + &g(10, at.0) + &g(20, at.1) + extra
}

fn block(name: &str, base: (f64, f64), body: &str) -> String {
    block_with_flags(name, 0, base, body)
}

fn block_with_flags(name: &str, flags: i32, base: (f64, f64), body: &str) -> String {
    g(0, "BLOCK")
        + &g(8, "0")
        + &g(2, name)
        + &g(70, flags)
        + &g(10, base.0)
        + &g(20, base.1)
        + body
        + &g(0, "ENDBLK")
        + &g(8, "0")
}

fn dxf(blocks: &str, entities: &str) -> String {
    g(0, "SECTION")
        + &g(2, "BLOCKS")
        + blocks
        + &g(0, "ENDSEC")
        + &g(0, "SECTION")
        + &g(2, "ENTITIES")
        + entities
        + &g(0, "ENDSEC")
        + &g(0, "EOF")
}

fn read_ok(text: &str) -> read::ReadReport {
    read::read_from_str(text).expect("読めるはず")
}

// ---- 検査のヘルパー ---------------------------------------------------------

/// 図面の図形をワールド座標に展開した一覧（インスタンスは解決する）。
fn world(doc: &Document) -> Vec<Geometry> {
    let mut out = Vec::new();
    for (_, e) in doc.entities().iter() {
        match &e.geom {
            Geometry::Instance(i) => out.extend(resolve(i, doc.definitions())),
            g => out.push(g.clone()),
        }
    }
    out
}

fn instances(doc: &Document) -> Vec<&Instance> {
    doc.entities()
        .iter()
        .filter_map(|(_, e)| match &e.geom {
            Geometry::Instance(i) => Some(i),
            _ => None,
        })
        .collect()
}

fn p(x: f64, y: f64) -> Point2 {
    Point2::new(x, y)
}

fn assert_line(g: &Geometry, a: Point2, b: Point2) {
    let Geometry::Line(l) = g else {
        panic!("線分のはず: {g:?}")
    };
    assert!(l.a.eq_tol(a), "始点 {:?} != {a:?}", l.a);
    assert!(l.b.eq_tol(b), "終点 {:?} != {b:?}", l.b);
}

fn assert_arc(g: &Geometry, center: Point2, radius: f64, start: Point2, end: Point2) {
    let Geometry::Arc(a) = g else {
        panic!("円弧のはず: {g:?}")
    };
    assert!(a.center.eq_tol(center), "中心 {:?} != {center:?}", a.center);
    assert!(eq_len(a.radius, radius), "半径 {} != {radius}", a.radius);
    assert!(
        a.start_point().eq_tol(start),
        "始点 {:?} != {start:?}",
        a.start_point()
    );
    assert!(
        a.end_point().eq_tol(end),
        "終点 {:?} != {end:?}",
        a.end_point()
    );
}

fn assert_geom_eq(a: &Geometry, b: &Geometry) {
    match (a, b) {
        (Geometry::Line(la), Geometry::Line(lb)) => {
            assert!(la.a.eq_tol(lb.a) && la.b.eq_tol(lb.b), "{la:?} vs {lb:?}");
        }
        (Geometry::Circle(ca), Geometry::Circle(cb)) => {
            assert!(ca.center.eq_tol(cb.center), "{ca:?} vs {cb:?}");
            assert!(eq_len(ca.radius, cb.radius), "{ca:?} vs {cb:?}");
        }
        (Geometry::Arc(aa), Geometry::Arc(ab)) => {
            assert!(aa.center.eq_tol(ab.center), "{aa:?} vs {ab:?}");
            assert!(eq_len(aa.radius, ab.radius), "{aa:?} vs {ab:?}");
            assert!(
                aa.start_point().eq_tol(ab.start_point()),
                "{aa:?} vs {ab:?}"
            );
            assert!(aa.end_point().eq_tol(ab.end_point()), "{aa:?} vs {ab:?}");
        }
        _ => panic!("種類が違います: {a:?} vs {b:?}"),
    }
}

fn warning_with<'a>(report: &'a read::ReadReport, needle: &str) -> &'a str {
    report
        .warnings
        .iter()
        .find(|w| w.contains(needle))
        .unwrap_or_else(|| panic!("「{needle}」を含む警告が無い: {:?}", report.warnings))
}

// ---- 受け入れ基準: ymcad が書いたコンポーネントの往復 ------------------------------

/// 入れ子・回転・倍率・反転を含むコンポーネントを DXF に書いて読み戻すと、
/// 定義・インスタンスとして戻り、配置もワールド座標の形も一致すること。
#[test]
fn components_written_by_ymcad_round_trip_as_components() {
    let mut doc = Document::new();
    let part = vec![
        Entity::new(
            Geometry::Line(Line::new(p(0.0, 0.0), p(4.0, 0.0))),
            LayerId::ZERO,
        ),
        Entity::new(
            Geometry::Arc(Arc::new(p(2.0, 1.0), 1.0, 0.0, FRAC_PI_2)),
            LayerId::ZERO,
        ),
        Entity::new(
            Geometry::Circle(Circle::new(p(1.0, 2.0), 0.5)),
            LayerId::ZERO,
        ),
    ];
    doc.apply(Box::new(DefineComponent::new(
        "T",
        "PART",
        p(1.0, 0.5),
        part,
    )))
    .unwrap();
    let part_id = doc.definitions().by_name("PART").unwrap();
    let inner = Instance::new(
        part_id,
        Placement::new(p(3.0, 4.0), 0.5, 2.0, true).unwrap(),
    );
    let outer = vec![
        Entity::new(Geometry::Instance(inner), LayerId::ZERO),
        Entity::new(
            Geometry::Line(Line::new(p(0.0, 0.0), p(0.0, 5.0))),
            LayerId::ZERO,
        ),
    ];
    doc.apply(Box::new(DefineComponent::new(
        "T",
        "OUTER",
        p(0.0, 0.0),
        outer,
    )))
    .unwrap();
    let outer_id = doc.definitions().by_name("OUTER").unwrap();
    doc.apply(Box::new(InsertInstance::new(
        "T",
        outer_id,
        Placement::new(p(100.0, 50.0), 0.785, 0.5, true).unwrap(),
        LayerId::ZERO,
    )))
    .unwrap();
    doc.apply(Box::new(InsertInstance::new(
        "T",
        part_id,
        Placement::new(p(-20.0, 10.0), 3.5, 3.0, false).unwrap(),
        LayerId::ZERO,
    )))
    .unwrap();

    let text = write::write_to_string(&doc).text;
    let report = read_ok(&text);
    let loaded = &report.document;

    assert_eq!(loaded.definitions().len(), 2, "定義の数");
    let before = instances(&doc);
    let after = instances(loaded);
    assert_eq!(after.len(), 2, "インスタンスの数");
    for (b, a) in before.iter().zip(&after) {
        let bn = &doc.definitions().get(b.definition).unwrap().name;
        let an = &loaded.definitions().get(a.definition).unwrap().name;
        assert_eq!(bn, an, "参照する定義");
        let (bp, ap) = (b.placement, a.placement);
        assert!(bp.origin.eq_tol(ap.origin), "{bp:?} vs {ap:?}");
        assert!(eq_len(bp.scale, ap.scale), "{bp:?} vs {ap:?}");
        assert!(eq_angle(bp.rotation, ap.rotation), "{bp:?} vs {ap:?}");
        assert_eq!(bp.flipped, ap.flipped, "{bp:?} vs {ap:?}");
    }

    // 入れ子も定義の中のインスタンスとして戻る。
    let outer_loaded = loaded
        .definitions()
        .get(loaded.definitions().by_name("OUTER").unwrap())
        .unwrap();
    assert_eq!(outer_loaded.entities.len(), 2);
    assert!(matches!(
        outer_loaded.entities[0].geom,
        Geometry::Instance(_)
    ));

    // ワールド座標の形も一致する。
    let (wb, wa) = (world(&doc), world(loaded));
    assert_eq!(wb.len(), wa.len());
    for (b, a) in wb.iter().zip(&wa) {
        assert_geom_eq(b, a);
    }
}

// ---- 手書きの DXF ------------------------------------------------------------

/// 前方参照（後ろで定義されるブロックを先に参照）と入れ子。
#[test]
fn forward_reference_and_nesting_resolve_to_world_coordinates() {
    let blocks = block(
        "OUTER",
        (0.0, 0.0),
        &insert("INNER", (10.0, 0.0), &g(50, 90.0)),
    ) + &block("INNER", (0.0, 0.0), &line((0.0, 0.0), (1.0, 0.0)));
    let report = read_ok(&dxf(&blocks, &insert("OUTER", (100.0, 0.0), "")));
    let doc = &report.document;
    assert_eq!(doc.definitions().len(), 2);
    assert_eq!(instances(doc).len(), 1);
    let w = world(doc);
    assert_eq!(w.len(), 1);
    assert_line(&w[0], p(110.0, 0.0), p(110.0, 1.0));
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
}

/// 鏡像の検査に使うブロック: 基点 (0, 0) に線分 (1,0)-(1,1) と円弧（中心 (2,0)、半径 1、0°→90°）。
fn mirror_probe_blocks() -> String {
    block(
        "B",
        (0.0, 0.0),
        &(line((1.0, 0.0), (1.0, 1.0)) + &arc((2.0, 0.0), 1.0, 0.0, 90.0, "")),
    )
}

#[test]
fn negative_x_scale_mirrors_about_the_y_axis() {
    let ins = insert("B", (10.0, 20.0), &(g(41, -2.0) + &g(42, 2.0)));
    let doc = read_ok(&dxf(&mirror_probe_blocks(), &ins)).document;
    let i = instances(&doc);
    assert_eq!(i.len(), 1, "相似なのでインスタンスのまま");
    let w = world(&doc);
    assert_line(&w[0], p(8.0, 20.0), p(8.0, 22.0));
    // (3,0)→(4,20)、(2,1)→(6,22)。鏡像で向きが逆になるので 90°→180°。
    assert_arc(&w[1], p(6.0, 20.0), 2.0, p(6.0, 22.0), p(4.0, 20.0));
}

#[test]
fn negative_y_scale_mirrors_about_the_x_axis() {
    let ins = insert("B", (10.0, 20.0), &(g(41, 2.0) + &g(42, -2.0)));
    let doc = read_ok(&dxf(&mirror_probe_blocks(), &ins)).document;
    let i = instances(&doc);
    assert_eq!(i.len(), 1);
    assert!(i[0].placement.flipped);
    let w = world(&doc);
    assert_line(&w[0], p(12.0, 20.0), p(12.0, 18.0));
    // (3,0)→(16,20)、(2,1)→(14,18)。270°→360°。
    assert_arc(&w[1], p(14.0, 20.0), 2.0, p(14.0, 18.0), p(16.0, 20.0));
}

#[test]
fn both_scales_negative_is_a_half_turn() {
    let ins = insert("B", (10.0, 20.0), &(g(41, -2.0) + &g(42, -2.0)));
    let doc = read_ok(&dxf(&mirror_probe_blocks(), &ins)).document;
    let i = instances(&doc);
    assert!(!i[0].placement.flipped, "180° 回転で、反転ではない");
    let w = world(&doc);
    assert_line(&w[0], p(8.0, 20.0), p(8.0, 18.0));
    // (3,0)→(4,20)、(2,1)→(6,18)。180°→270°。
    assert_arc(&w[1], p(6.0, 20.0), 2.0, p(4.0, 20.0), p(6.0, 18.0));
}

/// 押し出し方向 Z=-1 の INSERT。挿入点も OCS なので X が反転する。
#[test]
fn insert_with_negative_extrusion_mirrors_in_world() {
    let ins = insert(
        "B",
        (10.0, 20.0),
        &(g(210, 0.0) + &g(220, 0.0) + &g(230, -1.0)),
    );
    let doc = read_ok(&dxf(&mirror_probe_blocks(), &ins)).document;
    assert_eq!(instances(&doc).len(), 1);
    let w = world(&doc);
    assert_line(&w[0], p(-11.0, 20.0), p(-11.0, 21.0));
    // OCS (13,20)→(-13,20)、(12,21)→(-12,21)。90°→180°。
    assert_arc(&w[1], p(-12.0, 20.0), 1.0, p(-12.0, 21.0), p(-13.0, 20.0));
}

/// 押し出し方向 Z=-1 の ARC と CIRCLE（ブロックの外）。
#[test]
fn arc_and_circle_with_negative_extrusion_are_mirrored() {
    let z = g(210, 0.0) + &g(220, 0.0) + &g(230, -1.0);
    let ents = arc((5.0, 0.0), 1.0, 0.0, 90.0, &z) + &circle((3.0, 4.0), 2.0, &z);
    let doc = read_ok(&dxf("", &ents)).document;
    let w = world(&doc);
    assert_arc(&w[0], p(-5.0, 0.0), 1.0, p(-5.0, 1.0), p(-6.0, 0.0));
    let Geometry::Circle(c) = &w[1] else {
        panic!("円のはず")
    };
    assert!(c.center.eq_tol(p(-3.0, 4.0)));
}

/// LINE の座標はワールド座標。押し出し方向で鏡像にしてはいけない。
#[test]
fn line_ignores_extrusion_because_it_is_in_wcs() {
    let ents = g(0, "LINE")
        + &g(8, "0")
        + &g(10, 1.0)
        + &g(20, 2.0)
        + &g(11, 3.0)
        + &g(21, 4.0)
        + &g(230, -1.0);
    let doc = read_ok(&dxf("", &ents)).document;
    assert_line(&world(&doc)[0], p(1.0, 2.0), p(3.0, 4.0));
}

/// 回転と基点。基点 (1,1) が挿入点へ来てから回る。
#[test]
fn rotation_turns_around_the_base_point() {
    let blocks = block("R", (1.0, 1.0), &line((1.0, 1.0), (2.0, 1.0)));
    let ins = insert("R", (5.0, 5.0), &(g(50, 90.0) + &g(41, 3.0) + &g(42, 3.0)));
    let doc = read_ok(&dxf(&blocks, &ins)).document;
    assert_line(&world(&doc)[0], p(5.0, 5.0), p(5.0, 8.0));
}

/// 縦横の倍率が違う INSERT は展開される。円は楕円のポリラインになり、
/// 入れ子の中身も同じ行列で写る。
#[test]
fn non_uniform_scale_is_expanded_with_the_right_shape() {
    let blocks = block(
        "E",
        (0.0, 0.0),
        &(circle((0.0, 0.0), 1.0, "")
            + &line((0.0, 0.0), (1.0, 1.0))
            + &insert("SUB", (0.0, 0.0), "")),
    ) + &block("SUB", (0.0, 0.0), &line((0.0, 0.0), (0.0, 1.0)));
    let ins = insert("E", (10.0, 0.0), &(g(41, 2.0) + &g(42, 1.0)));
    let report = read_ok(&dxf(&blocks, &ins));
    let doc = &report.document;
    assert!(instances(doc).is_empty(), "インスタンスにはならない");
    let w = world(doc);
    assert_eq!(w.len(), 3, "{w:?}");
    let Geometry::Polyline(pl) = &w[0] else {
        panic!("楕円のポリラインのはず: {:?}", w[0])
    };
    assert!(pl.closed);
    for v in &pl.vertices {
        let on = ((v.x - 10.0) / 2.0).powi(2) + v.y.powi(2);
        assert!(eq_len(on, 1.0), "楕円の上に無い: {v:?}");
    }
    assert_line(&w[1], p(10.0, 0.0), p(12.0, 1.0));
    assert_line(&w[2], p(10.0, 0.0), p(10.0, 1.0));
    warning_with(&report, "展開");
    // 定義は残る（他の INSERT から使えるように）。
    assert_eq!(doc.definitions().len(), 2);
}

/// MINSERT は列・行の数だけ並ぶ。間隔は回転した座標系で測る。
#[test]
fn minsert_places_every_cell() {
    let blocks = block("M", (0.0, 0.0), &line((0.0, 0.0), (1.0, 0.0)));
    let ins = insert(
        "M",
        (100.0, 0.0),
        &(g(50, 90.0) + &g(70, 3) + &g(71, 2) + &g(44, 10.0) + &g(45, 5.0)),
    );
    let doc = read_ok(&dxf(&blocks, &ins)).document;
    let origins: Vec<Point2> = instances(&doc).iter().map(|i| i.placement.origin).collect();
    assert_eq!(origins.len(), 6);
    for row in 0..2 {
        for col in 0..3 {
            // 回転 90° なので列は +Y、行は -X へ並ぶ。
            let expect = p(100.0 - 5.0 * f64::from(row), 10.0 * f64::from(col));
            assert!(
                origins.iter().any(|o| o.eq_tol(expect)),
                "{expect:?} が無い: {origins:?}"
            );
        }
    }
}

/// 属性付きの INSERT（66=1 の後ろの ATTRIB … SEQEND）を読み飛ばし、後ろの図形を食べない。
#[test]
fn attributes_after_insert_are_skipped() {
    let blocks = block("A", (0.0, 0.0), &line((0.0, 0.0), (1.0, 0.0)));
    let attrib = |tag: &str| g(0, "ATTRIB") + &g(8, "0") + &g(1, "値") + &g(2, tag);
    let ents = insert("A", (0.0, 0.0), &g(66, 1))
        + &attrib("X")
        + &attrib("Y")
        + &g(0, "SEQEND")
        + &g(8, "0")
        + &line((5.0, 5.0), (6.0, 6.0));
    let report = read_ok(&dxf(&blocks, &ents));
    let doc = &report.document;
    assert_eq!(instances(doc).len(), 1);
    assert_eq!(doc.entities().len(), 2, "INSERT と後ろの LINE");
    assert!(warning_with(&report, "ATTRIB").contains("ATTRIB 2 件"));
}

/// 循環（A → B → A と自己参照 C → C）は飛ばして警告し、解決は有限で終わる。
#[test]
fn cyclic_references_are_dropped_with_a_warning() {
    let blocks = block(
        "A",
        (0.0, 0.0),
        &(line((0.0, 0.0), (1.0, 0.0)) + &insert("B", (0.0, 1.0), "")),
    ) + &block(
        "B",
        (0.0, 0.0),
        &(line((0.0, 0.0), (0.0, 1.0)) + &insert("A", (1.0, 0.0), "")),
    ) + &block(
        "C",
        (0.0, 0.0),
        &(line((0.0, 0.0), (2.0, 0.0)) + &insert("C", (1.0, 0.0), "")),
    );
    let ents = insert("A", (0.0, 0.0), "") + &insert("C", (0.0, 10.0), "");
    let report = read_ok(&dxf(&blocks, &ents));
    warning_with(&report, "循環");
    let w = world(&report.document);
    // A の線分・B の線分（A の中の B）・C の線分。循環する辺は 1 本ずつ落ちる。
    assert_eq!(w.len(), 3, "{w:?}");
}

/// 縦横別倍率の展開の中で循環しても止まる。
#[test]
fn cycle_inside_an_expansion_terminates() {
    let blocks = block(
        "A",
        (0.0, 0.0),
        &(line((0.0, 0.0), (1.0, 0.0)) + &insert("A", (1.0, 0.0), &g(41, 2.0))),
    );
    let report = read_ok(&dxf(&blocks, &insert("A", (0.0, 0.0), &g(42, 3.0))));
    warning_with(&report, "循環");
}

#[test]
fn reference_to_an_undefined_block_is_reported() {
    let report = read_ok(&dxf("", &insert("NOPE", (0.0, 0.0), "")));
    assert!(report.document.entities().is_empty());
    assert!(warning_with(&report, "定義の無い").contains("NOPE"));
}

/// 色: 256 = BYLAYER、0 = BYBLOCK（BYLAYER に寄せて警告）、1..255 = ACI。
#[test]
fn color_256_is_bylayer_and_0_is_approximated() {
    let colored = |c: i32| {
        g(0, "LINE")
            + &g(8, "0")
            + &g(62, c)
            + &g(10, 0.0)
            + &g(20, 0.0)
            + &g(11, 1.0)
            + &g(21, 0.0)
    };
    let report = read_ok(&dxf("", &(colored(256) + &colored(0) + &colored(5))));
    let colors: Vec<ColorSpec> = report
        .document
        .entities()
        .iter()
        .map(|(_, e)| e.color)
        .collect();
    assert_eq!(
        colors,
        vec![
            ColorSpec::ByLayer,
            ColorSpec::ByLayer,
            ColorSpec::Aci(AciColor(5))
        ]
    );
    assert!(warning_with(&report, "BYBLOCK").contains('1'));
}

/// モデル空間・ペーパー空間・参照されない匿名ブロックは定義にしない。
/// 参照される匿名ブロックと、参照されない名前付きブロックは定義にする。
#[test]
fn layout_and_unreferenced_anonymous_blocks_are_not_defined() {
    let body = line((0.0, 0.0), (1.0, 0.0));
    let blocks = block("*Model_Space", (0.0, 0.0), &body)
        + &block("*Paper_Space0", (0.0, 0.0), &body)
        + &block("$MODEL_SPACE", (0.0, 0.0), &body)
        + &block_with_flags("*D1", 1, (0.0, 0.0), &body)
        + &block_with_flags("*U2", 1, (0.0, 0.0), &body)
        + &block("LIBRARY", (0.0, 0.0), &body);
    let report = read_ok(&dxf(&blocks, &insert("*U2", (0.0, 0.0), "")));
    let names: Vec<&str> = report
        .document
        .definitions()
        .iter()
        .map(|(_, d)| d.name.as_str())
        .collect();
    assert_eq!(names, vec!["*U2", "LIBRARY"]);
}

/// 外部参照（フラグ 4）は定義にせず、参照は警告する。
#[test]
fn xref_blocks_are_not_read() {
    let blocks = block_with_flags("EXT", 4, (0.0, 0.0), "");
    let report = read_ok(&dxf(&blocks, &insert("EXT", (0.0, 0.0), "")));
    assert!(report.document.definitions().is_empty());
    assert!(warning_with(&report, "外部参照").contains("EXT"));
}

/// 名前: 日本語はそのまま。大文字・小文字違いは同じブロック（最初の綴りを使い、2 つ目は警告）。
/// 名前の無いブロックは読まずに警告する。
#[test]
fn block_names_are_case_insensitive_and_kept_as_written() {
    let body = line((0.0, 0.0), (1.0, 0.0));
    let blocks = block("扉", (0.0, 0.0), &body)
        + &block("Door", (0.0, 0.0), &body)
        + &block("DOOR", (0.0, 0.0), &line((0.0, 0.0), (9.0, 0.0)))
        + &block("", (0.0, 0.0), &body);
    let ents = insert("door", (0.0, 0.0), "") + &insert("扉", (0.0, 5.0), "");
    let report = read_ok(&dxf(&blocks, &ents));
    let doc = &report.document;
    let names: Vec<&str> = doc
        .definitions()
        .iter()
        .map(|(_, d)| d.name.as_str())
        .collect();
    assert_eq!(names, vec!["扉", "Door"]);
    assert_eq!(instances(doc).len(), 2);
    // 小文字の参照が最初の "Door" の中身（長さ 1 の線分）を指す。
    assert_line(&world(doc)[0], p(0.0, 0.0), p(1.0, 0.0));
    assert!(warning_with(&report, "同じ名前").contains("DOOR"));
    warning_with(&report, "名前の無いブロック");
}

/// 押し出し方向が傾いた図形は 2D に落とせないので読まずに警告する。
#[test]
fn oblique_extrusion_is_skipped_with_a_warning() {
    let tilted = g(210, 0.0) + &g(220, 1.0) + &g(230, 1.0);
    let report = read_ok(&dxf("", &circle((0.0, 0.0), 1.0, &tilted)));
    assert!(report.document.entities().is_empty());
    warning_with(&report, "傾いた");
}

/// 倍率 0 の INSERT は潰れるので読まずに警告する。
#[test]
fn zero_scale_insert_is_skipped_with_a_warning() {
    let blocks = block("Z", (0.0, 0.0), &line((0.0, 0.0), (1.0, 0.0)));
    let report = read_ok(&dxf(&blocks, &insert("Z", (0.0, 0.0), &g(41, 0.0))));
    assert!(report.document.entities().is_empty());
    warning_with(&report, "倍率が 0");
}

/// 読まなかった種類は 1 行にまとめ、件数を添える。
#[test]
fn unread_entity_kinds_are_summarised_in_one_line() {
    let text = |s: &str| g(0, "TEXT") + &g(8, "0") + &g(1, s);
    let ents = text("a") + &text("b") + &g(0, "MTEXT") + &g(8, "0");
    let report = read_ok(&dxf("", &ents));
    let w = warning_with(&report, "TEXT 2 件");
    assert!(w.contains("MTEXT 1 件"), "{w}");
    assert_eq!(
        report
            .warnings
            .iter()
            .filter(|w| w.contains("TEXT"))
            .count(),
        1
    );
}

/// 定義の中の図形にも同じ読み込み（ポリライン・円弧）を使う。
#[test]
fn polylines_inside_blocks_are_read_too() {
    let lw = g(0, "LWPOLYLINE")
        + &g(8, "0")
        + &g(70, 1)
        + &g(10, 0.0)
        + &g(20, 0.0)
        + &g(10, 1.0)
        + &g(20, 0.0)
        + &g(10, 1.0)
        + &g(20, 1.0);
    let blocks = block("P", (0.0, 0.0), &lw);
    let ins = insert("P", (0.0, 0.0), &g(50, 180.0));
    let doc = read_ok(&dxf(&blocks, &ins)).document;
    let Geometry::Polyline(pl) = &world(&doc)[0] else {
        panic!("ポリラインのはず")
    };
    assert!(pl.closed);
    assert!(pl.vertices[2].eq_tol(p(-1.0, -1.0)));
    // 半回転の確認（回転は度で読まれる）。
    let i = instances(&doc);
    assert!(eq_angle(i[0].placement.rotation, PI));
}
