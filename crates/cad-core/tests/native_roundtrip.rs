//! ネイティブ形式（`.ymc`）の往復と、壊れた入力の扱いの検証。
//!
//! この形式の存在理由は**無損失であること**なので、往復して図面が
//! 完全に一致することを固定するのが主目的。
//! 特に **DXF R12 が落としていた 4 つ**（作図線・グループ・線種・日本語名）を
//! 個別のテストで押さえる。

use cad_core::command::{AddEntities, AddLayer, CreateGroup, SetLayerProperties};
use cad_core::geom::{Arc, Circle, Line, Point2, Polyline, Vec2, Xline};
use cad_core::layer::LineType;
use cad_core::native::{read, write};
use cad_core::{AciColor, CadError, ColorSpec, Document, Entity, EntityId, Geometry, LayerId};

// ---- 下準備 ---------------------------------------------------------------

fn p(x: f64, y: f64) -> Point2 {
    Point2::new(x, y)
}

fn add(doc: &mut Document, entities: Vec<Entity>) {
    doc.apply(Box::new(AddEntities::many("TEST", entities)))
        .expect("エンティティを追加できるはず");
}

fn add_layer(doc: &mut Document, name: &str, color: AciColor) -> LayerId {
    doc.apply(Box::new(AddLayer::new(name.to_owned(), color)))
        .expect("レイヤを追加できるはず");
    doc.layers().by_name(name).expect("追加したはず")
}

/// 往復させた図面を返す。
fn roundtrip(doc: &Document) -> Document {
    let bytes = write::write_to_bytes(doc);
    read::read_from_bytes(&bytes).expect("書いたものは読めるはず")
}

/// 図面の内容が一致することを、要素ごとに突き合わせて確かめる。
///
/// `Document` は `PartialEq` を持たない（Undo 履歴や dirty を含むため）ので、
/// 永続化の対象だけを比べる。
fn assert_same_drawing(a: &Document, b: &Document) {
    let layers_a: Vec<_> = a.layers().iter().map(|(_, l)| l.clone()).collect();
    let layers_b: Vec<_> = b.layers().iter().map(|(_, l)| l.clone()).collect();
    assert_eq!(layers_a, layers_b, "レイヤ表が一致すること");

    let groups_a: Vec<_> = a.groups().iter().map(|(_, g)| g.name.clone()).collect();
    let groups_b: Vec<_> = b.groups().iter().map(|(_, g)| g.name.clone()).collect();
    assert_eq!(groups_a, groups_b, "グループ表が一致すること");

    // 走査順（= 描画順）まで含めて比べる。
    let ents_a: Vec<_> = a.entities().iter().map(|(_, e)| e.clone()).collect();
    let ents_b: Vec<_> = b.entities().iter().map(|(_, e)| e.clone()).collect();
    assert_eq!(ents_a.len(), ents_b.len(), "エンティティ数が一致すること");
    for (i, (x, y)) in ents_a.iter().zip(ents_b.iter()).enumerate() {
        assert_eq!(x, y, "{i} 番目のエンティティが一致すること");
    }
}

/// 全変種・全属性を含む図面。往復の総合テストに使う。
fn build_sample_doc() -> Document {
    let mut doc = Document::new();

    // レイヤ 0 の属性も変えて、既定レイヤの往復も確かめる。
    doc.apply(Box::new(
        SetLayerProperties::new(LayerId::ZERO)
            .color(AciColor(3))
            .linetype(LineType::Center),
    ))
    .expect("レイヤ 0 の属性を変えられるはず");

    let walls = add_layer(&mut doc, "壁", AciColor(1));
    let hidden = add_layer(&mut doc, "補助線", AciColor(5));
    doc.apply(Box::new(
        SetLayerProperties::new(hidden)
            .visible(false)
            .locked(true)
            .linetype(LineType::Hidden),
    ))
    .expect("属性を変えられるはず");

    let xline = Xline::new(p(1.0, 2.0), Vec2::new(3.0, 4.0)).expect("作図線を作れるはず");
    let mut colored = Entity::new(Geometry::Circle(Circle::new(p(5.0, 5.0), 2.5)), walls);
    colored.color = ColorSpec::Aci(AciColor(4));

    add(
        &mut doc,
        vec![
            Entity::new(
                Geometry::Line(Line::new(p(0.0, 0.0), p(10.0, 0.0))),
                LayerId::ZERO,
            ),
            colored,
            Entity::new(Geometry::Arc(Arc::new(p(1.0, 1.0), 3.0, 0.25, 2.75)), walls),
            Entity::new(Geometry::Xline(xline), hidden),
            Entity::new(
                Geometry::Polyline(Polyline::new(
                    vec![p(0.0, 0.0), p(1.0, 2.0), p(3.0, 1.0)],
                    true,
                )),
                LayerId::ZERO,
            ),
        ],
    );

    // 先頭 2 つをグループにする。
    let ids: Vec<EntityId> = doc.entities().ids().take(2).collect();
    doc.apply(Box::new(CreateGroup::new("GROUP", "外周", ids)))
        .expect("グループを作れるはず");

    doc
}

// ---- 往復の完全性 ---------------------------------------------------------

/// 全変種・全属性を含む図面が完全に往復すること。
#[test]
fn full_drawing_survives_a_round_trip() {
    let doc = build_sample_doc();
    let loaded = roundtrip(&doc);
    assert_same_drawing(&doc, &loaded);
}

/// **DXF が落としていたもの その 1。** 作図線が作図線のまま戻ること。
#[test]
fn xline_stays_an_xline() {
    let mut doc = Document::new();
    let x = Xline::new(p(1.5, -2.5), Vec2::new(1.0, 1.0)).expect("作図線を作れるはず");
    add(
        &mut doc,
        vec![Entity::new(Geometry::Xline(x), LayerId::ZERO)],
    );

    let loaded = roundtrip(&doc);
    let (_, e) = loaded.entities().iter().next().expect("1 件あるはず");
    let Geometry::Xline(got) = &e.geom else {
        panic!(
            "作図線のまま戻ること（DXF では LINE に化けていた）: {:?}",
            e.geom
        );
    };
    assert_eq!(got.origin, x.origin, "通過点が一致すること");
    assert_eq!(got.direction, x.direction, "方向が一致すること");
}

/// **DXF が落としていたもの その 2。** グループ所属が戻ること。
#[test]
fn group_membership_survives() {
    let mut doc = Document::new();
    add(
        &mut doc,
        vec![
            Entity::new(
                Geometry::Line(Line::new(p(0.0, 0.0), p(1.0, 0.0))),
                LayerId::ZERO,
            ),
            Entity::new(
                Geometry::Line(Line::new(p(0.0, 1.0), p(1.0, 1.0))),
                LayerId::ZERO,
            ),
            Entity::new(
                Geometry::Line(Line::new(p(0.0, 2.0), p(1.0, 2.0))),
                LayerId::ZERO,
            ),
        ],
    );
    // 1 番目と 3 番目だけをグループにする（連続していない所属も戻ること）。
    let ids: Vec<EntityId> = doc.entities().ids().collect();
    doc.apply(Box::new(CreateGroup::new(
        "GROUP",
        "ばらばら",
        vec![ids[0], ids[2]],
    )))
    .expect("グループを作れるはず");

    let loaded = roundtrip(&doc);
    assert_eq!(loaded.groups().len(), 1, "グループが残ること");
    let gid = loaded
        .groups()
        .by_name("ばらばら")
        .expect("同じ名前で残ること");

    let memberships: Vec<bool> = loaded
        .entities()
        .iter()
        .map(|(_, e)| e.group == Some(gid))
        .collect();
    assert_eq!(
        memberships,
        vec![true, false, true],
        "どれが属していたかが正確に戻ること"
    );
}

/// **DXF が落としていたもの その 3。** 線種が戻ること。
#[test]
fn layer_linetypes_survive() {
    let mut doc = Document::new();
    for (name, lt) in [
        ("実線", LineType::Continuous),
        ("破線", LineType::Dashed),
        ("一点鎖線", LineType::Center),
        ("隠線", LineType::Hidden),
    ] {
        let id = add_layer(&mut doc, name, AciColor::WHITE);
        doc.apply(Box::new(SetLayerProperties::new(id).linetype(lt)))
            .expect("線種を設定できるはず");
    }

    let loaded = roundtrip(&doc);
    for (name, want) in [
        ("実線", LineType::Continuous),
        ("破線", LineType::Dashed),
        ("一点鎖線", LineType::Center),
        ("隠線", LineType::Hidden),
    ] {
        let id = loaded.layers().by_name(name).expect("レイヤが残ること");
        assert_eq!(
            loaded.layers().get(id).expect("引けるはず").linetype,
            want,
            "{name} の線種が戻ること（DXF では CONTINUOUS 固定だった）"
        );
    }
}

/// **DXF が壊していたもの。** 日本語の名前がサニタイズされずに戻ること。
#[test]
fn non_ascii_names_are_not_sanitised() {
    let mut doc = Document::new();
    // DXF R12 のサニタイズは大文字化と空白除去をするので、
    // 「小文字を含む」「空白を含む」「非 ASCII」をすべて含む名前で試す。
    let layer_name = "通り芯 level 1";
    let id = add_layer(&mut doc, layer_name, AciColor(2));
    add(
        &mut doc,
        vec![Entity::new(
            Geometry::Line(Line::new(p(0.0, 0.0), p(1.0, 0.0))),
            id,
        )],
    );
    let ids: Vec<EntityId> = doc.entities().ids().collect();
    doc.apply(Box::new(CreateGroup::new("GROUP", "建具 まわり", ids)))
        .expect("グループを作れるはず");

    let loaded = roundtrip(&doc);
    assert!(
        loaded.layers().by_name(layer_name).is_some(),
        "レイヤ名がそのまま戻ること。実際の名前: {:?}",
        loaded
            .layers()
            .iter()
            .map(|(_, l)| l.name.clone())
            .collect::<Vec<_>>()
    );
    assert!(
        loaded.groups().by_name("建具 まわり").is_some(),
        "グループ名がそのまま戻ること"
    );
}

/// 座標が `f64` のビット一致で戻ること。
///
/// テキスト形式には桁数の丸めがあるが、この形式は `to_le_bytes` で
/// ビット単位に書くので完全に一致する。
#[test]
fn coordinates_survive_bit_exactly() {
    let mut doc = Document::new();
    // 巨大・微小・割り切れない値・負のゼロを混ぜる。
    let a = p(1.234_567_890_123_456_7e12, -9.876_543_210_987_654e-12);
    let b = p(1.0 / 3.0, std::f64::consts::PI);
    add(
        &mut doc,
        vec![
            Entity::new(Geometry::Line(Line::new(a, b)), LayerId::ZERO),
            Entity::new(
                Geometry::Arc(Arc::new(a, 1.0 / 7.0, 1.0 / 9.0, 2.0 / 11.0)),
                LayerId::ZERO,
            ),
        ],
    );

    let loaded = roundtrip(&doc);
    let mut it = loaded.entities().iter();

    let (_, e) = it.next().expect("線分があるはず");
    let Geometry::Line(l) = &e.geom else {
        panic!("線分のはず")
    };
    assert_eq!(l.a.x.to_bits(), a.x.to_bits(), "巨大な座標がビット一致");
    assert_eq!(l.a.y.to_bits(), a.y.to_bits(), "微小な座標がビット一致");
    assert_eq!(l.b.x.to_bits(), b.x.to_bits(), "1/3 がビット一致");
    assert_eq!(l.b.y.to_bits(), b.y.to_bits(), "π がビット一致");

    let (_, e) = it.next().expect("円弧があるはず");
    let Geometry::Arc(arc) = &e.geom else {
        panic!("円弧のはず")
    };
    // 角度はラジアンのまま書くので、度への往復による誤差が入らない。
    assert_eq!(arc.radius.to_bits(), (1.0f64 / 7.0).to_bits());
    assert_eq!(arc.start_angle.to_bits(), (1.0f64 / 9.0).to_bits());
    assert_eq!(arc.end_angle.to_bits(), (2.0f64 / 11.0).to_bits());
}

/// エンティティの並び（= 描画順）が保たれること。
///
/// CAD では後に描いたものが上に来るので、順序は見た目を決める。
#[test]
fn entity_order_is_preserved() {
    let mut doc = Document::new();
    let entities: Vec<Entity> = (0..12)
        .map(|i| {
            let y = f64::from(i);
            Entity::new(
                Geometry::Line(Line::new(p(0.0, y), p(1.0, y))),
                LayerId::ZERO,
            )
        })
        .collect();
    add(&mut doc, entities);

    let loaded = roundtrip(&doc);
    let ys: Vec<f64> = loaded
        .entities()
        .iter()
        .map(|(_, e)| match &e.geom {
            Geometry::Line(l) => l.a.y,
            other => panic!("線分のはず: {other:?}"),
        })
        .collect();
    let want: Vec<f64> = (0..12).map(f64::from).collect();
    assert_eq!(ys, want, "並びが入れ替わっていないこと");
}

/// 空の図面も往復できること。
#[test]
fn empty_drawing_survives() {
    let doc = Document::new();
    let loaded = roundtrip(&doc);
    assert_eq!(loaded.entities().len(), 0);
    assert_eq!(loaded.layers().len(), 1, "レイヤ 0 だけがあること");
    assert!(loaded.groups().is_empty());
}

/// 読み込んだ図面は Undo 履歴を持たず、未保存扱いでもないこと。
#[test]
fn loaded_document_has_no_undo_history_and_is_not_dirty() {
    let doc = build_sample_doc();
    let loaded = roundtrip(&doc);
    assert!(!loaded.is_dirty(), "開いた直後は未保存ではない");
    assert_eq!(loaded.history().len(), 0, "Undo 履歴を引き継がない");
    assert!(!loaded.history().can_redo());
}

// ---- ファイル入出力 -------------------------------------------------------

#[test]
fn write_to_file_and_read_from_file_roundtrip() {
    let doc = build_sample_doc();
    let path =
        std::env::temp_dir().join(format!("ymcad_native_roundtrip_{}.ymc", std::process::id()));

    write::write_to_file(&doc, &path).expect("保存できるはず");
    let loaded = read::read_from_file(&path).expect("読めるはず");

    assert_same_drawing(&doc, &loaded);
    assert_eq!(loaded.path(), Some(path.as_path()), "パスが記録されること");
    assert!(!loaded.is_dirty());

    let _ = std::fs::remove_file(&path);
}

#[test]
fn read_from_missing_file_is_io_error() {
    let path = std::env::temp_dir().join("ymcad_native_does_not_exist_98765.ymc");
    let err = read::read_from_file(&path).expect_err("無いファイルは読めない");
    assert!(matches!(err, CadError::Io(_)), "{err:?}");
}

// ---- 壊れた入力（panic しないこと） ---------------------------------------

#[test]
fn empty_input_is_rejected() {
    let err = read::read_from_bytes(&[]).expect_err("空は読めない");
    assert!(matches!(err, CadError::Format { .. }), "{err:?}");
}

#[test]
fn wrong_magic_is_rejected_with_a_clear_message() {
    let mut bytes = write::write_to_bytes(&build_sample_doc());
    bytes[0] = b'X';
    let err = read::read_from_bytes(&bytes).expect_err("識別子が違えば読めない");
    match err {
        CadError::Format { message, .. } => {
            assert!(
                message.contains("ymcad の図面ファイルではありません"),
                "何が悪いか分かる説明であること: {message}"
            );
        }
        other => panic!("形式エラーのはず: {other:?}"),
    }
}

/// DXF ファイルを間違って開いても、形式エラーとして断ること。
#[test]
fn a_dxf_file_is_rejected_as_a_native_file() {
    let text = cad_core::dxf::write::write_to_string(&build_sample_doc()).text;
    let err = read::read_from_bytes(text.as_bytes()).expect_err("DXF は .ymc として読めない");
    assert!(matches!(err, CadError::Format { .. }), "{err:?}");
}

/// **未来のバージョンは読まずに断ること。**
///
/// 中途半端に読んで壊れた図面を見せるより、開けないと言うほうがよい。
#[test]
fn a_future_format_version_is_rejected() {
    let mut bytes = write::write_to_bytes(&build_sample_doc());
    // magic の直後 4 バイトがバージョン。
    bytes[8..12].copy_from_slice(&999u32.to_le_bytes());

    let err = read::read_from_bytes(&bytes).expect_err("未来のバージョンは読めない");
    match err {
        CadError::Format { message, .. } => {
            assert!(
                message.contains("新しいバージョン"),
                "更新を促す説明であること: {message}"
            );
        }
        other => panic!("形式エラーのはず: {other:?}"),
    }
}

/// **どこで切られても panic しないこと。**
///
/// 1 バイトずつ切り詰めた全長で回す。バイナリ形式で最も起こりやすい壊れ方で、
/// 範囲外アクセスが 1 箇所でもあれば必ずここで露見する。
#[test]
fn every_truncation_is_an_error_and_never_panics() {
    let bytes = write::write_to_bytes(&build_sample_doc());
    assert!(bytes.len() > 32, "十分な長さのサンプルであること");

    for n in 0..bytes.len() {
        assert!(
            read::read_from_bytes(&bytes[..n]).is_err(),
            "{n} バイトに切り詰めたものが読めてしまった（完全な長さは {}）",
            bytes.len()
        );
    }
}

/// 未知の図形種別を拒否すること。
#[test]
fn an_unknown_geometry_kind_is_rejected() {
    let mut doc = Document::new();
    add(
        &mut doc,
        vec![Entity::new(
            Geometry::Line(Line::new(p(0.0, 0.0), p(1.0, 0.0))),
            LayerId::ZERO,
        )],
    );
    let mut bytes = write::write_to_bytes(&doc);
    // 最初のエンティティの種別タグを、あり得ない値に書き換える。
    let tag = find_first_entity_tag_offset(&doc);
    bytes[tag] = 200;

    let err = read::read_from_bytes(&bytes).expect_err("未知の種別は読めない");
    match err {
        CadError::Format { message, .. } => {
            assert!(message.contains("未知の図形種別"), "{message}");
        }
        other => panic!("形式エラーのはず: {other:?}"),
    }
}

/// 範囲外のレイヤ参照を拒否すること。
#[test]
fn an_out_of_range_layer_reference_is_rejected() {
    let mut doc = Document::new();
    add(
        &mut doc,
        vec![Entity::new(
            Geometry::Line(Line::new(p(0.0, 0.0), p(1.0, 0.0))),
            LayerId::ZERO,
        )],
    );
    let mut bytes = write::write_to_bytes(&doc);
    // 線分の種別タグ 1 バイト + 座標 4 個 (32 バイト) の直後がレイヤ添字。
    let at = find_first_entity_tag_offset(&doc) + 1 + 32;
    bytes[at..at + 4].copy_from_slice(&7u32.to_le_bytes());

    let err = read::read_from_bytes(&bytes).expect_err("範囲外の参照は読めない");
    match err {
        CadError::Format { message, .. } => {
            assert!(message.contains("レイヤの参照が範囲外"), "{message}");
        }
        other => panic!("形式エラーのはず: {other:?}"),
    }
}

/// 単位ベクトルでない方向の作図線を拒否すること（不変条件を外から壊せないこと）。
#[test]
fn a_non_unit_xline_direction_is_rejected() {
    let mut doc = Document::new();
    let x = Xline::new(p(0.0, 0.0), Vec2::new(1.0, 0.0)).expect("作図線を作れるはず");
    add(
        &mut doc,
        vec![Entity::new(Geometry::Xline(x), LayerId::ZERO)],
    );
    let mut bytes = write::write_to_bytes(&doc);
    // 種別タグ 1 + 通過点 16 の直後が方向ベクトル（f64 が 2 個）。
    let at = find_first_entity_tag_offset(&doc) + 1 + 16;
    bytes[at..at + 16].copy_from_slice(&[0u8; 16]);

    let err = read::read_from_bytes(&bytes).expect_err("零ベクトルの作図線は読めない");
    match err {
        CadError::Format { message, .. } => {
            assert!(message.contains("単位ベクトルではありません"), "{message}");
        }
        other => panic!("形式エラーのはず: {other:?}"),
    }
}

/// エンティティ部の先頭（最初の種別タグ）のバイトオフセットを求める。
///
/// 形式を知っている前提で数えるので、レイアウトを変えたらここも直す。
fn find_first_entity_tag_offset(doc: &Document) -> usize {
    let mut at = 8 + 4; // magic + version
    at += 4; // layer_count
    for (_, l) in doc.layers().iter() {
        at += 4 + l.name.len() + 3; // 名前 + color + flags + linetype
    }
    at += 4; // group_count
    for (_, g) in doc.groups().iter() {
        at += 4 + g.name.len();
    }
    at += 4; // entity_count
    at
}

// ---- コンポーネント（形式 v2） ---------------------------------------------

/// 定義とインスタンスを含む図面。
fn build_component_doc() -> Document {
    use cad_core::command::{DefineComponent, InsertInstance};
    use cad_core::component::Placement;

    let mut doc = Document::new();
    let walls = add_layer(&mut doc, "壁 outer", AciColor(1));

    // 内側の定義（線分 + 円）。中身はレイヤを混ぜる。
    let inner_contents = vec![
        Entity::new(Geometry::Line(Line::new(p(0.0, 0.0), p(10.0, 0.0))), walls),
        Entity::new(
            Geometry::Circle(Circle::new(p(5.0, 0.0), 2.0)),
            LayerId::ZERO,
        ),
    ];
    doc.apply(Box::new(DefineComponent::new(
        "COMPONENT",
        "内部品",
        p(1.0, 1.0),
        inner_contents,
    )))
    .expect("内側の定義");
    let inner = doc.definitions().by_name("内部品").expect("あるはず");

    // 外側の定義は内側のインスタンスを含む（入れ子）。
    let nested = Entity::new(
        Geometry::Instance(cad_core::Instance::new(
            inner,
            Placement::new(p(20.0, 0.0), 0.5, 2.0, true).expect("妥当な配置"),
        )),
        LayerId::ZERO,
    );
    doc.apply(Box::new(DefineComponent::new(
        "COMPONENT",
        "外 assembly",
        Point2::ORIGIN,
        vec![nested],
    )))
    .expect("外側の定義");
    let outer = doc.definitions().by_name("外 assembly").expect("あるはず");

    // 図面には両方を配置する。反転・回転・倍率を混ぜる。
    for (def, pl) in [
        (inner, Placement::at(p(100.0, 0.0))),
        (
            outer,
            Placement::new(p(200.0, 50.0), 1.25, 0.5, true).expect("妥当な配置"),
        ),
    ] {
        doc.apply(Box::new(InsertInstance::new("INSERT", def, pl, walls)))
            .expect("配置できるはず");
    }

    doc
}

/// 定義・入れ子・配置（反転含む）が完全に往復すること。
#[test]
fn components_survive_a_round_trip() {
    let doc = build_component_doc();
    let loaded = roundtrip(&doc);
    assert_same_drawing(&doc, &loaded);

    assert_eq!(loaded.definitions().len(), 2, "定義が 2 件戻る");
    assert!(
        loaded.definitions().by_name("外 assembly").is_some(),
        "**日本語 + 空白を含む定義名がサニタイズされずに戻る**"
    );
}

/// 入れ子の解決結果が往復後も一致すること。
///
/// 定義の添字と ID の対応が崩れていたらここで露見する。
#[test]
fn nested_component_resolution_is_unchanged_by_a_round_trip() {
    let doc = build_component_doc();
    let loaded = roundtrip(&doc);

    let resolve_all = |d: &Document| -> Vec<Point2> {
        d.entities()
            .iter()
            .filter_map(|(_, e)| match &e.geom {
                Geometry::Instance(i) => Some(cad_core::component::resolve(i, d.definitions())),
                _ => None,
            })
            .flatten()
            .flat_map(|g| match g {
                Geometry::Line(l) => vec![l.a, l.b],
                Geometry::Circle(c) => vec![c.center],
                _ => Vec::new(),
            })
            .collect()
    };

    let before = resolve_all(&doc);
    let after = resolve_all(&loaded);
    assert!(!before.is_empty(), "解決結果が空でないこと");
    assert_eq!(before.len(), after.len());
    for (i, (a, b)) in before.iter().zip(after.iter()).enumerate() {
        assert_eq!(a.x.to_bits(), b.x.to_bits(), "{i} 番目の x がビット一致");
        assert_eq!(a.y.to_bits(), b.y.to_bits(), "{i} 番目の y がビット一致");
    }
}

/// **反転フラグが往復すること。**
///
/// フラグを落とすと鏡像のコンポーネントが元に戻ってしまう。
#[test]
fn the_flipped_flag_survives() {
    let doc = build_component_doc();
    let loaded = roundtrip(&doc);

    let flags: Vec<bool> = loaded
        .entities()
        .iter()
        .filter_map(|(_, e)| match &e.geom {
            Geometry::Instance(i) => Some(i.placement.flipped),
            _ => None,
        })
        .collect();
    assert_eq!(flags, vec![false, true], "配置ごとの反転が保たれる");
}

/// 定義の中身のレイヤが往復すること。
#[test]
fn the_layers_inside_a_definition_survive() {
    let doc = build_component_doc();
    let loaded = roundtrip(&doc);

    let walls = loaded
        .layers()
        .by_name("壁 outer")
        .expect("レイヤがあるはず");
    let inner = loaded.definitions().by_name("内部品").expect("定義");
    let def = loaded.definitions().get(inner).expect("引ける");
    assert_eq!(def.entities.len(), 2);
    assert_eq!(def.entities[0].layer, walls, "中身のレイヤが保たれる");
    assert_eq!(def.entities[1].layer, LayerId::ZERO);
}

/// **形式 v1 のファイルが引き続き読めること（後方互換）。**
///
/// v1 には定義セクションが無い。前半の表現は変えていないので、
/// 「そこで終わり」として読めなければならない。
#[test]
fn a_version_1_file_is_still_readable() {
    // コンポーネントを含まない図面を書き、ヘッダのバージョンを 1 に落として
    // 定義セクション（末尾の 4 バイト = 定義数 0）を削る。
    let doc = build_sample_doc();
    let v2 = write::write_to_bytes(&doc);

    let mut v1 = v2[..v2.len() - 4].to_vec();
    v1[8..12].copy_from_slice(&1u32.to_le_bytes());
    assert_eq!(
        &v2[v2.len() - 4..],
        &0u32.to_le_bytes(),
        "削ったのは「定義 0 件」の 4 バイトであること"
    );

    let loaded = read::read_from_bytes(&v1).expect("v1 として読めるはず");
    assert_same_drawing(&doc, &loaded);
    assert_eq!(loaded.definitions().len(), 0);
}

/// v1 のファイルを読んで書き直すと v2 になること（保存で最新版に上がる）。
#[test]
fn reading_a_version_1_file_and_saving_writes_version_2() {
    let doc = build_sample_doc();
    let v2 = write::write_to_bytes(&doc);
    let mut v1 = v2[..v2.len() - 4].to_vec();
    v1[8..12].copy_from_slice(&1u32.to_le_bytes());

    let loaded = read::read_from_bytes(&v1).expect("読めるはず");
    let written = write::write_to_bytes(&loaded);
    assert_eq!(&written[8..12], &2u32.to_le_bytes(), "書き出しは常に現行版");
    // 内容は変わらない。
    assert_same_drawing(&loaded, &read::read_from_bytes(&written).expect("読める"));
}

/// 範囲外の定義参照を拒否すること。
#[test]
fn an_out_of_range_definition_reference_is_rejected() {
    let doc = build_component_doc();
    let mut bytes = write::write_to_bytes(&doc);

    // 最初のインスタンスの定義添字を、あり得ない値へ書き換える。
    // インスタンスは種別タグ 5 なので、そのバイトを探して直後の u32 を潰す。
    let at = bytes
        .iter()
        .position(|b| *b == 5)
        .expect("インスタンスの種別タグがあるはず");
    bytes[at + 1..at + 5].copy_from_slice(&99u32.to_le_bytes());

    let err = read::read_from_bytes(&bytes).expect_err("範囲外の参照は読めない");
    match err {
        CadError::Format { message, .. } => {
            assert!(
                message.contains("コンポーネント定義の参照が範囲外"),
                "何が悪いか分かる説明であること: {message}"
            );
        }
        other => panic!("形式エラーのはず: {other:?}"),
    }
}

/// 倍率 0 の配置を拒否すること（不変条件を外から壊せないこと）。
#[test]
fn a_zero_scale_placement_is_rejected() {
    let doc = build_component_doc();
    let mut bytes = write::write_to_bytes(&doc);

    // インスタンスのペイロードは: 種別 1 + 定義添字 4 + 基点 16 + 回転 8 + 倍率 8。
    let at = bytes
        .iter()
        .position(|b| *b == 5)
        .expect("インスタンスの種別タグ");
    let scale_at = at + 1 + 4 + 16 + 8;
    bytes[scale_at..scale_at + 8].copy_from_slice(&0.0f64.to_le_bytes());

    let err = read::read_from_bytes(&bytes).expect_err("倍率 0 は読めない");
    assert!(matches!(err, CadError::Format { .. }), "{err:?}");
}

/// **コンポーネントを含む図面でも、どこで切られても panic しないこと。**
#[test]
fn every_truncation_of_a_component_file_is_an_error() {
    let bytes = write::write_to_bytes(&build_component_doc());
    for n in 0..bytes.len() {
        assert!(
            read::read_from_bytes(&bytes[..n]).is_err(),
            "{n} バイトに切り詰めたものが読めてしまった（完全な長さは {}）",
            bytes.len()
        );
    }
}
