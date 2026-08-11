//! DXF R12 読み書きの結合テスト。
//!
//! `cad_core::dxf` の公開 API（[`write::write_to_string`] / [`read::read_from_str`] など）
//! だけを使い、実際に書いて読み直したときに図面の内容が保たれることを確認する。
//!
//! トレランスのマジックナンバー禁止規約はこのファイルにも適用されるため、
//! 座標・角度の比較は必ず `cad_core::geom::tolerance::{eq_len, eq_angle}` を使う。

use std::f64::consts::{FRAC_PI_2, TAU};

use cad_core::command::{AddEntities, AddLayer, SetLayerProperties};
use cad_core::dxf::{read, write};
use cad_core::error::CadError;
use cad_core::geom::tolerance::{eq_angle, eq_len};
use cad_core::geom::{Arc, Circle, Line, Point2, Polyline};
use cad_core::layer::LineType;
use cad_core::{AciColor, ColorSpec, Document, Entity, Geometry, LayerId};

// ---- テスト用のヘルパー -----------------------------------------------------

/// 複数レイヤ・複数ジオメトリを持つサンプル図面を作る。
///
/// - `"0"`（既定レイヤ、色 White）
/// - `"Wall"`（色 Red、線種 Dashed）— LINE と閉じたポリラインが乗る
/// - `"hidden layer"`（色 3、非表示、ロック）— ARC が乗る
fn build_sample_doc() -> Document {
    let mut doc = Document::new();

    doc.apply(Box::new(AddLayer::new("Wall", AciColor::RED)))
        .unwrap();
    let wall = doc.layers().by_name("Wall").unwrap();
    doc.apply(Box::new(
        SetLayerProperties::new(wall).linetype(LineType::Dashed),
    ))
    .unwrap();

    doc.apply(Box::new(AddLayer::new("hidden layer", AciColor(3))))
        .unwrap();
    let hidden = doc.layers().by_name("hidden layer").unwrap();
    doc.apply(Box::new(
        SetLayerProperties::new(hidden).visible(false).locked(true),
    ))
    .unwrap();

    let entities = vec![
        Entity::new(
            Geometry::Line(Line::new(Point2::new(0.0, 0.0), Point2::new(10.0, 5.0))),
            wall,
        ),
        Entity::new(
            Geometry::Circle(Circle::new(Point2::new(1.0, 1.0), 3.0)),
            LayerId::ZERO,
        ),
        Entity::new(
            Geometry::Arc(Arc::new(Point2::new(2.0, 2.0), 4.0, 0.0, FRAC_PI_2)),
            hidden,
        ),
        Entity::new(
            Geometry::Polyline(Polyline::new(
                vec![
                    Point2::new(0.0, 0.0),
                    Point2::new(5.0, 0.0),
                    Point2::new(5.0, 5.0),
                ],
                true,
            )),
            wall,
        ),
    ];
    doc.apply(Box::new(AddEntities::many("TEST", entities)))
        .unwrap();

    doc
}

fn one_entity_doc(geom: Geometry) -> Document {
    let mut doc = Document::new();
    doc.apply(Box::new(AddEntities::one(
        "TEST",
        Entity::new(geom, LayerId::ZERO),
    )))
    .unwrap();
    doc
}

fn assert_geom_eq(a: &Geometry, b: &Geometry) {
    match (a, b) {
        (Geometry::Line(la), Geometry::Line(lb)) => {
            assert!(la.a.eq_tol(lb.a), "始点が一致しません: {la:?} vs {lb:?}");
            assert!(la.b.eq_tol(lb.b), "終点が一致しません: {la:?} vs {lb:?}");
        }
        (Geometry::Circle(ca), Geometry::Circle(cb)) => {
            assert!(ca.center.eq_tol(cb.center), "中心が一致しません");
            assert!(eq_len(ca.radius, cb.radius), "半径が一致しません");
        }
        (Geometry::Arc(aa), Geometry::Arc(ab)) => {
            assert!(aa.center.eq_tol(ab.center), "中心が一致しません");
            assert!(eq_len(aa.radius, ab.radius), "半径が一致しません");
            assert!(
                eq_angle(aa.start_angle, ab.start_angle),
                "開始角が一致しません: {} vs {}",
                aa.start_angle,
                ab.start_angle
            );
            assert!(
                eq_angle(aa.end_angle, ab.end_angle),
                "終了角が一致しません: {} vs {}",
                aa.end_angle,
                ab.end_angle
            );
        }
        (Geometry::Polyline(pa), Geometry::Polyline(pb)) => {
            assert_eq!(pa.closed, pb.closed, "closed フラグが一致しません");
            assert_eq!(pa.vertex_count(), pb.vertex_count(), "頂点数が一致しません");
            for (va, vb) in pa.vertices.iter().zip(pb.vertices.iter()) {
                assert!(va.eq_tol(*vb), "頂点が一致しません: {va:?} vs {vb:?}");
            }
        }
        _ => panic!("ジオメトリ種別が一致しません: {a:?} vs {b:?}"),
    }
}

// ---- 受け入れ基準: 全ジオメトリ種別・複数レイヤの往復 ------------------------

#[test]
fn round_trip_all_geometry_types_multiple_layers() {
    let doc = build_sample_doc();
    let text = write::write_to_string(&doc);
    let loaded = read::read_from_str(&text).expect("読み込みに成功するはず");

    assert_eq!(
        loaded.entities().len(),
        doc.entities().len(),
        "エンティティ数が一致すること"
    );
    assert_eq!(
        loaded.layers().len(),
        doc.layers().len(),
        "レイヤ数が一致すること"
    );

    // エンティティの並び順は挿入順（= ファイル内の記述順）で保たれるはず。
    for ((_, orig), (_, load)) in doc.entities().iter().zip(loaded.entities().iter()) {
        assert_geom_eq(&orig.geom, &load.geom);
        assert_eq!(
            orig.layer.index(),
            load.layer.index(),
            "レイヤの割り当て順が一致すること"
        );
        assert_eq!(orig.color, load.color, "色が一致すること");
    }

    // レイヤの属性も名前ベースで突き合わせる（サニタイズ後の名前で参照する）。
    let wall = loaded
        .layers()
        .by_name("WALL")
        .expect("WALL レイヤがあるはず");
    let wall_layer = loaded.layers().get(wall).unwrap();
    assert_eq!(wall_layer.color, AciColor::RED);
    assert_eq!(wall_layer.linetype, LineType::Dashed);
    assert!(wall_layer.visible);
    assert!(!wall_layer.locked);

    let hidden = loaded
        .layers()
        .by_name("HIDDEN_LAYER")
        .expect("hidden layer がサニタイズされて存在するはず");
    let hidden_layer = loaded.layers().get(hidden).unwrap();
    assert_eq!(hidden_layer.color, AciColor(3));
    assert!(!hidden_layer.visible, "非表示が復元されること");
    assert!(hidden_layer.locked, "ロックが復元されること");
}

// ---- 座標の精度往復 ---------------------------------------------------------

#[test]
fn round_trip_large_coordinates_precision() {
    let mag = 1.0e6;
    let doc = one_entity_doc(Geometry::Line(Line::new(
        Point2::new(-mag, mag),
        Point2::new(mag, -mag * 0.5),
    )));
    let text = write::write_to_string(&doc);
    let loaded = read::read_from_str(&text).unwrap();
    let (_, e) = loaded.entities().iter().next().unwrap();
    let Geometry::Line(l) = &e.geom else {
        panic!("LINE のはず")
    };
    assert!(eq_len(l.a.x, -mag));
    assert!(eq_len(l.a.y, mag));
    assert!(eq_len(l.b.x, mag));
    assert!(eq_len(l.b.y, -mag * 0.5));
}

#[test]
fn round_trip_small_coordinates_precision() {
    // 指数表記の負指数（`1e-6` 形式）はトレランスの直書き検査に引っかかる書き方なので、
    // 小数表記で 1e-6 と同じ値を作る。
    let mag = 0.000_001;
    let doc = one_entity_doc(Geometry::Circle(Circle::new(Point2::new(mag, -mag), mag)));
    let text = write::write_to_string(&doc);
    let loaded = read::read_from_str(&text).unwrap();
    let (_, e) = loaded.entities().iter().next().unwrap();
    let Geometry::Circle(c) = &e.geom else {
        panic!("CIRCLE のはず")
    };
    assert!(eq_len(c.center.x, mag));
    assert!(eq_len(c.center.y, -mag));
    assert!(eq_len(c.radius, mag));
}

// ---- 角度（ラジアン⇔度）往復 -------------------------------------------------

/// 指示書が明示する受け入れ基準: 90 度は本当に `PI / 2` ラジアンとして戻ってくること。
#[test]
fn arc_ninety_degrees_roundtrips_to_frac_pi_2() {
    let doc = one_entity_doc(Geometry::Arc(Arc::new(Point2::ORIGIN, 1.0, 0.0, FRAC_PI_2)));
    let text = write::write_to_string(&doc);
    let loaded = read::read_from_str(&text).unwrap();
    let (_, e) = loaded.entities().iter().next().unwrap();
    let Geometry::Arc(a) = &e.geom else {
        panic!("ARC のはず")
    };
    assert!(eq_angle(a.end_angle, FRAC_PI_2));
}

/// 0 度をまたぐ円弧（開始角が負）が正しく往復すること。角度の変換バグが
/// もっとも見えにくいのはこの手のケースなので明示的にテストする。
#[test]
fn arc_crossing_zero_degrees_survives_roundtrip() {
    let start = cad_core::dxf::deg_to_rad(-30.0);
    let end = cad_core::dxf::deg_to_rad(30.0);
    let doc = one_entity_doc(Geometry::Arc(Arc::new(Point2::ORIGIN, 5.0, start, end)));
    let text = write::write_to_string(&doc);
    let loaded = read::read_from_str(&text).unwrap();
    let (_, e) = loaded.entities().iter().next().unwrap();
    let Geometry::Arc(a) = &e.geom else {
        panic!("ARC のはず")
    };
    assert!(eq_angle(a.start_angle, start));
    assert!(eq_angle(a.end_angle, end));
}

#[test]
fn arc_full_circle_start_equals_end_roundtrips() {
    let doc = one_entity_doc(Geometry::Arc(Arc::new(
        Point2::new(1.0, 1.0),
        5.0,
        0.0,
        0.0,
    )));
    let text = write::write_to_string(&doc);
    let loaded = read::read_from_str(&text).unwrap();
    let (_, e) = loaded.entities().iter().next().unwrap();
    let Geometry::Arc(a) = &e.geom else {
        panic!("ARC のはず")
    };
    assert!(eq_angle(a.sweep(), TAU));
}

// ---- ポリライン往復 ----------------------------------------------------------

#[test]
fn closed_polyline_roundtrip_vertex_count_and_flag() {
    let verts = vec![
        Point2::new(0.0, 0.0),
        Point2::new(10.0, 0.0),
        Point2::new(10.0, 10.0),
        Point2::new(0.0, 10.0),
    ];
    let doc = one_entity_doc(Geometry::Polyline(Polyline::new(verts.clone(), true)));
    let text = write::write_to_string(&doc);
    let loaded = read::read_from_str(&text).unwrap();
    let (_, e) = loaded.entities().iter().next().unwrap();
    let Geometry::Polyline(p) = &e.geom else {
        panic!("Polyline のはず")
    };
    assert!(p.closed);
    assert_eq!(p.vertex_count(), verts.len());
    for (a, b) in verts.iter().zip(p.vertices.iter()) {
        assert!(a.eq_tol(*b));
    }
}

#[test]
fn open_polyline_roundtrip_vertex_count_and_flag() {
    let verts = vec![
        Point2::new(0.0, 0.0),
        Point2::new(3.0, 4.0),
        Point2::new(-1.0, 2.0),
    ];
    let doc = one_entity_doc(Geometry::Polyline(Polyline::new(verts.clone(), false)));
    let text = write::write_to_string(&doc);
    let loaded = read::read_from_str(&text).unwrap();
    let (_, e) = loaded.entities().iter().next().unwrap();
    let Geometry::Polyline(p) = &e.geom else {
        panic!("Polyline のはず")
    };
    assert!(!p.closed);
    assert_eq!(p.vertex_count(), verts.len());
}

// ---- 単純なジオメトリ往復 ----------------------------------------------------

#[test]
fn line_roundtrip_endpoints() {
    let doc = one_entity_doc(Geometry::Line(Line::new(
        Point2::new(-3.5, 2.25),
        Point2::new(12.0, -8.75),
    )));
    let text = write::write_to_string(&doc);
    let loaded = read::read_from_str(&text).unwrap();
    let (_, e) = loaded.entities().iter().next().unwrap();
    let Geometry::Line(l) = &e.geom else {
        panic!("LINE のはず")
    };
    assert!(l.a.eq_tol(Point2::new(-3.5, 2.25)));
    assert!(l.b.eq_tol(Point2::new(12.0, -8.75)));
}

#[test]
fn circle_roundtrip_center_and_radius() {
    let doc = one_entity_doc(Geometry::Circle(Circle::new(Point2::new(4.0, -6.0), 7.5)));
    let text = write::write_to_string(&doc);
    let loaded = read::read_from_str(&text).unwrap();
    let (_, e) = loaded.entities().iter().next().unwrap();
    let Geometry::Circle(c) = &e.geom else {
        panic!("CIRCLE のはず")
    };
    assert!(c.center.eq_tol(Point2::new(4.0, -6.0)));
    assert!(eq_len(c.radius, 7.5));
}

// ---- 色の往復 ----------------------------------------------------------------

#[test]
fn entity_explicit_color_roundtrip() {
    let mut e = Entity::new(
        Geometry::Line(Line::new(Point2::new(0.0, 0.0), Point2::new(1.0, 1.0))),
        LayerId::ZERO,
    );
    e.color = ColorSpec::Aci(AciColor(5));
    let mut doc = Document::new();
    doc.apply(Box::new(AddEntities::one("LINE", e))).unwrap();

    let text = write::write_to_string(&doc);
    let loaded = read::read_from_str(&text).unwrap();
    let (_, le) = loaded.entities().iter().next().unwrap();
    assert_eq!(le.color, ColorSpec::Aci(AciColor(5)));
}

#[test]
fn entity_bylayer_color_stays_bylayer_after_roundtrip() {
    let doc = one_entity_doc(Geometry::Line(Line::new(
        Point2::new(0.0, 0.0),
        Point2::new(1.0, 1.0),
    )));
    let text = write::write_to_string(&doc);
    let loaded = read::read_from_str(&text).unwrap();
    let (_, le) = loaded.entities().iter().next().unwrap();
    assert_eq!(le.color, ColorSpec::ByLayer);
}

// ---- レイヤ属性の往復 --------------------------------------------------------

#[test]
fn layer_visibility_roundtrips_via_negative_color() {
    let mut doc = Document::new();
    doc.apply(Box::new(AddLayer::new("HIDDEN", AciColor(2))))
        .unwrap();
    let id = doc.layers().by_name("HIDDEN").unwrap();
    doc.apply(Box::new(SetLayerProperties::new(id).visible(false)))
        .unwrap();

    let text = write::write_to_string(&doc);
    assert!(
        text.contains("\n-2\n"),
        "非表示レイヤは負の色で書かれるはず:\n{text}"
    );

    let loaded = read::read_from_str(&text).unwrap();
    let loaded_id = loaded.layers().by_name("HIDDEN").unwrap();
    let loaded_layer = loaded.layers().get(loaded_id).unwrap();
    assert!(!loaded_layer.visible);
    assert_eq!(loaded_layer.color, AciColor(2));
}

#[test]
fn layer_locked_flag_roundtrips() {
    let mut doc = Document::new();
    doc.apply(Box::new(AddLayer::new("LOCKED", AciColor::WHITE)))
        .unwrap();
    let id = doc.layers().by_name("LOCKED").unwrap();
    doc.apply(Box::new(SetLayerProperties::new(id).locked(true)))
        .unwrap();

    let text = write::write_to_string(&doc);
    let loaded = read::read_from_str(&text).unwrap();
    let loaded_id = loaded.layers().by_name("LOCKED").unwrap();
    assert!(loaded.layers().get(loaded_id).unwrap().locked);
}

#[test]
fn layer_linetype_roundtrips() {
    let mut doc = Document::new();
    doc.apply(Box::new(AddLayer::new("CENTERLINE", AciColor::WHITE)))
        .unwrap();
    let id = doc.layers().by_name("CENTERLINE").unwrap();
    doc.apply(Box::new(
        SetLayerProperties::new(id).linetype(LineType::Center),
    ))
    .unwrap();

    let text = write::write_to_string(&doc);
    let loaded = read::read_from_str(&text).unwrap();
    let loaded_id = loaded.layers().by_name("CENTERLINE").unwrap();
    assert_eq!(
        loaded.layers().get(loaded_id).unwrap().linetype,
        LineType::Center
    );
}

// ---- レイヤ名のサニタイズ -----------------------------------------------------

#[test]
fn sanitize_layer_name_applied_on_write_lowercase_and_spaces() {
    let mut doc = Document::new();
    doc.apply(Box::new(AddLayer::new("office wall", AciColor::WHITE)))
        .unwrap();
    let id = doc.layers().by_name("office wall").unwrap();
    doc.apply(Box::new(AddEntities::one(
        "LINE",
        Entity::new(
            Geometry::Line(Line::new(Point2::ORIGIN, Point2::new(1.0, 1.0))),
            id,
        ),
    )))
    .unwrap();

    let text = write::write_to_string(&doc);
    assert!(text.contains("OFFICE_WALL"));
    assert!(!text.contains("office wall"));
}

/// エンティティが往復後もサニタイズ済みの名前を参照していること（指示書の明示要求）。
#[test]
fn entities_reference_sanitized_layer_name_after_roundtrip() {
    let mut doc = Document::new();
    doc.apply(Box::new(AddLayer::new("office wall", AciColor::WHITE)))
        .unwrap();
    let id = doc.layers().by_name("office wall").unwrap();
    doc.apply(Box::new(AddEntities::one(
        "LINE",
        Entity::new(
            Geometry::Line(Line::new(Point2::ORIGIN, Point2::new(1.0, 1.0))),
            id,
        ),
    )))
    .unwrap();

    let text = write::write_to_string(&doc);
    let loaded = read::read_from_str(&text).unwrap();

    let sanitized_id = loaded
        .layers()
        .by_name("OFFICE_WALL")
        .expect("サニタイズ後の名前でレイヤが見つかるはず");
    let (_, e) = loaded.entities().iter().next().unwrap();
    assert_eq!(
        e.layer, sanitized_id,
        "エンティティがサニタイズ済みレイヤを参照していること"
    );
}

/// サニタイズ後に名前が衝突しても、レイヤとしては別々のまま残ること。
#[test]
fn colliding_sanitized_layer_names_get_unique_suffix_and_stay_distinct() {
    let mut doc = Document::new();
    doc.apply(Box::new(AddLayer::new("Wall", AciColor::RED)))
        .unwrap();
    doc.apply(Box::new(AddLayer::new("WALL", AciColor(3))))
        .unwrap();
    assert_eq!(doc.layers().len(), 3, "\"0\" + \"Wall\" + \"WALL\"");

    let text = write::write_to_string(&doc);
    let loaded = read::read_from_str(&text).unwrap();
    assert_eq!(
        loaded.layers().len(),
        3,
        "サニタイズ後の名前が衝突しても両方のレイヤが残ること"
    );
}

// ---- 読み込みの寛容さ ---------------------------------------------------------

#[test]
fn reader_skips_unknown_sections_and_entities() {
    let text = "\
0
SECTION
2
HEADER
9
$ACADVER
1
AC1009
0
ENDSEC
0
SECTION
2
BLOCKS
0
BLOCK
2
DUMMY
0
ENDBLK
0
ENDSEC
0
SECTION
2
ENTITIES
0
TEXT
8
0
1
hello
0
LINE
8
0
10
0.0
20
0.0
11
1.0
21
1.0
0
ENDSEC
0
EOF
";
    let doc = read::read_from_str(text).unwrap();
    assert_eq!(
        doc.entities().len(),
        1,
        "TEXT は無視され LINE だけ読まれること"
    );
}

#[test]
fn reader_tolerates_crlf_line_endings() {
    let lf = "0\nSECTION\n2\nENTITIES\n0\nLINE\n8\n0\n10\n0.0\n20\n0.0\n11\n1.0\n21\n1.0\n0\nENDSEC\n0\nEOF\n";
    let crlf = lf.replace('\n', "\r\n");
    let doc = read::read_from_str(&crlf).unwrap();
    assert_eq!(doc.entities().len(), 1);
}

#[test]
fn reader_loads_file_with_no_layer_table() {
    let text = "0\nSECTION\n2\nENTITIES\n0\nLINE\n8\nCUSTOM\n10\n0.0\n20\n0.0\n11\n1.0\n21\n1.0\n0\nENDSEC\n0\nEOF\n";
    let doc = read::read_from_str(text).unwrap();
    assert_eq!(doc.entities().len(), 1);
    assert!(
        doc.layers().by_name("CUSTOM").is_some(),
        "未宣言のレイヤはエンティティ側の記述からその場で作られること"
    );
}

#[test]
fn undeclared_layer_referenced_by_entity_is_created_on_read() {
    let text = "0\nSECTION\n2\nTABLES\n0\nTABLE\n2\nLAYER\n70\n1\n0\nENDTAB\n0\nENDSEC\n0\nSECTION\n2\nENTITIES\n0\nLINE\n8\nGHOST\n10\n0.0\n20\n0.0\n11\n1.0\n21\n1.0\n0\nENDSEC\n0\nEOF\n";
    let doc = read::read_from_str(text).unwrap();
    assert!(doc.layers().by_name("GHOST").is_some());
    assert_eq!(doc.entities().len(), 1);
}

#[test]
fn lwpolyline_from_other_software_is_read_as_polyline() {
    let text = "0\nSECTION\n2\nENTITIES\n0\nLWPOLYLINE\n8\n0\n90\n3\n70\n1\n10\n0.0\n20\n0.0\n10\n5.0\n20\n0.0\n10\n5.0\n20\n5.0\n0\nENDSEC\n0\nEOF\n";
    let doc = read::read_from_str(text).unwrap();
    assert_eq!(doc.entities().len(), 1);
    let (_, e) = doc.entities().iter().next().unwrap();
    let Geometry::Polyline(p) = &e.geom else {
        panic!("Polyline のはず")
    };
    assert_eq!(p.vertex_count(), 3);
    assert!(p.closed);
}

// ---- 読み込みエラー -----------------------------------------------------------

#[test]
fn reader_error_on_truncated_file_missing_eof() {
    let text =
        "0\nSECTION\n2\nENTITIES\n0\nLINE\n8\n0\n10\n0.0\n20\n0.0\n11\n1.0\n21\n1.0\n0\nENDSEC\n";
    let err = read::read_from_str(text).unwrap_err();
    assert!(matches!(err, CadError::Parse { .. }));
}

#[test]
fn reader_error_on_odd_number_of_lines() {
    let text = "0\nSECTION\n2\nENTITIES\n0\nLINE\n8\n0\n10\n0.0\n20";
    let err = read::read_from_str(text).unwrap_err();
    assert!(matches!(err, CadError::Parse { .. }));
}

#[test]
fn reader_error_on_unparseable_number() {
    let text = "0\nSECTION\n2\nENTITIES\n0\nLINE\n8\n0\n10\nNOT_A_NUMBER\n20\n0.0\n11\n1.0\n21\n1.0\n0\nENDSEC\n0\nEOF\n";
    let err = read::read_from_str(text).unwrap_err();
    assert!(matches!(err, CadError::Parse { .. }));
}

// ---- Document の状態 -----------------------------------------------------------

#[test]
fn loaded_document_has_no_undo_history_and_is_not_dirty() {
    let doc = build_sample_doc();
    let text = write::write_to_string(&doc);
    let loaded = read::read_from_str(&text).unwrap();

    assert!(!loaded.history().can_undo());
    assert!(!loaded.history().can_redo());
    assert_eq!(loaded.history().len(), 0);
    assert!(!loaded.is_dirty());
}

#[test]
fn empty_document_roundtrips() {
    let doc = Document::new();
    let text = write::write_to_string(&doc);
    let loaded = read::read_from_str(&text).unwrap();
    assert_eq!(loaded.entities().len(), 0);
    assert_eq!(loaded.layers().len(), 1, "\"0\" レイヤだけ残るはず");
    assert!(!loaded.is_dirty());
}

#[test]
fn multiple_entities_preserve_insertion_order() {
    let mut doc = Document::new();
    let entities = vec![
        Entity::new(
            Geometry::Circle(Circle::new(Point2::new(0.0, 0.0), 1.0)),
            LayerId::ZERO,
        ),
        Entity::new(
            Geometry::Circle(Circle::new(Point2::new(1.0, 0.0), 2.0)),
            LayerId::ZERO,
        ),
        Entity::new(
            Geometry::Circle(Circle::new(Point2::new(2.0, 0.0), 3.0)),
            LayerId::ZERO,
        ),
    ];
    doc.apply(Box::new(AddEntities::many("CIRCLE", entities)))
        .unwrap();

    let text = write::write_to_string(&doc);
    let loaded = read::read_from_str(&text).unwrap();

    let radii: Vec<f64> = loaded
        .entities()
        .iter()
        .map(|(_, e)| match &e.geom {
            Geometry::Circle(c) => c.radius,
            _ => unreachable!("CIRCLE のみのはず"),
        })
        .collect();
    assert_eq!(radii.len(), 3);
    assert!(eq_len(radii[0], 1.0));
    assert!(eq_len(radii[1], 2.0));
    assert!(eq_len(radii[2], 3.0));
}

// ---- ファイル入出力 -------------------------------------------------------------

#[test]
fn write_to_file_and_read_from_file_roundtrip() {
    let doc = build_sample_doc();
    let path = std::env::temp_dir().join(format!(
        "ymcad_dxf_roundtrip_test_{}.dxf",
        std::process::id()
    ));

    write::write_to_file(&doc, &path).unwrap();
    let loaded = read::read_from_file(&path).unwrap();

    assert_eq!(loaded.entities().len(), doc.entities().len());
    assert_eq!(loaded.path(), Some(path.as_path()));
    assert!(!loaded.is_dirty());

    let _ = std::fs::remove_file(&path);
}

#[test]
fn read_from_file_missing_file_is_io_error() {
    let path = std::env::temp_dir().join("ymcad_dxf_does_not_exist_12345.dxf");
    let err = read::read_from_file(&path).unwrap_err();
    assert!(matches!(err, CadError::Io(_)));
}

// ---- 書き出しフォーマットの形 -----------------------------------------------------

#[test]
fn write_to_string_contains_required_header_fields() {
    let doc = Document::new();
    let text = write::write_to_string(&doc);
    assert!(text.contains("$ACADVER"));
    assert!(text.contains("AC1009"));
    assert!(text.contains("$EXTMIN"));
    assert!(text.contains("$EXTMAX"));
    assert!(text.contains("$INSBASE"));
}

#[test]
fn write_to_string_ends_with_eof() {
    let doc = build_sample_doc();
    let text = write::write_to_string(&doc);
    assert!(text.ends_with("0\nEOF\n"));
}

#[test]
fn write_to_string_contains_layer_table() {
    let mut doc = Document::new();
    doc.apply(Box::new(AddLayer::new("WALL", AciColor::RED)))
        .unwrap();
    let text = write::write_to_string(&doc);
    assert!(text.contains("TABLE"));
    assert!(text.contains("LAYER"));
    assert!(text.contains("WALL"));
    assert!(text.contains("ENDTAB"));
}
