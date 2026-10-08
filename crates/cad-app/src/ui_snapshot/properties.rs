//! プロパティパネル（Issue #31 段階 1）の見た目。種類ごとの表示、複数選択、コマンド実行中、
//! 3 枚のパネルを同時に開いた状態を `target/ui-snapshots/properties_*.png` に撮る。

use cad_core::command::{
    AddEntities, AddLayer, DefineComponent, InsertInstance, SetBinding, SetDefinitionParams,
    SetLayerProperties,
};
use cad_core::component::{Binding, ParamDecl, Placement, Slot};
use cad_core::expr::parse;
use cad_core::geom::{Arc, Circle, Line, Point2, Polyline, Xline};
use cad_core::{AciColor, Document, Entity, EntityId, Geometry, LayerId};
use egui_kittest::Harness;

use super::{harness, hover, press, shot, type_text, CANVAS_CENTER, STEPS};
use crate::app::CadApp;

/// 図面に足した図形の ID（足した順）。
struct Drawing {
    line: EntityId,
    circle: EntityId,
    arc: EntityId,
    polyline: EntityId,
    xline: EntityId,
    instance: EntityId,
}

/// レイヤ `壁`（普通）と `ロック済み`（ロック）を足し、種類ごとに 1 つずつ図形を並べる。
fn draw(doc: &mut Document) -> Drawing {
    doc.apply(Box::new(AddLayer::new("壁", AciColor::RED)))
        .expect("レイヤ");
    doc.apply(Box::new(AddLayer::new("ロック済み", AciColor::WHITE)))
        .expect("レイヤ");
    let wall = doc.layers().by_name("壁").expect("壁");
    let locked = doc.layers().by_name("ロック済み").expect("ロック済み");
    doc.apply(Box::new(SetLayerProperties::new(locked).locked(true)))
        .expect("ロック");

    let mut add = |geom: Geometry, layer: LayerId| -> EntityId {
        doc.apply(Box::new(AddEntities::one("TEST", Entity::new(geom, layer))))
            .expect("追加");
        doc.entities().ids().last().expect("足した図形")
    };
    let line = add(
        Geometry::Line(Line::new(
            Point2::new(40.0, 40.0),
            Point2::new(160.0, 100.0),
        )),
        LayerId::ZERO,
    );
    let circle = add(
        Geometry::Circle(Circle::new(Point2::new(250.0, 90.0), 40.0)),
        wall,
    );
    let arc = add(
        Geometry::Arc(Arc::new(
            Point2::new(120.0, 200.0),
            50.0,
            0.3,
            std::f64::consts::PI,
        )),
        wall,
    );
    let polyline = add(
        Geometry::Polyline(Polyline::new(
            vec![
                Point2::new(230.0, 170.0),
                Point2::new(330.0, 170.0),
                Point2::new(330.0, 240.0),
            ],
            false,
        )),
        LayerId::ZERO,
    );
    let xline = add(
        Geometry::Xline(Xline::at_angle(Point2::new(60.0, 260.0), 0.4)),
        LayerId::ZERO,
    );
    doc.apply(Box::new(DefineComponent::new(
        "COMPONENT",
        "窓",
        Point2::ORIGIN,
        vec![Entity::new(
            Geometry::Line(Line::new(Point2::ORIGIN, Point2::new(30.0, 0.0))),
            LayerId::ZERO,
        )],
    )))
    .expect("定義");
    let def = doc.definitions().by_name("窓").expect("窓");
    let placement = Placement::new(Point2::new(350.0, 60.0), 0.5, 1.5, false).expect("配置");
    doc.apply(Box::new(InsertInstance::new(
        "INSERT", def, placement, wall,
    )))
    .expect("配置");
    let instance = doc.entities().ids().last().expect("インスタンス");
    Drawing {
        line,
        circle,
        arc,
        polyline,
        xline,
        instance,
    }
}

/// 図形を並べ、`selected` を選んで、Ctrl+1 でパネルを開いた状態。
fn opened(selected: impl FnOnce(&Drawing) -> Vec<EntityId>) -> (Harness<'static, CadApp>, Drawing) {
    let mut h = harness();
    let (doc, session) = h.state_mut().parts_mut();
    let drawing = draw(doc);
    for id in selected(&drawing) {
        session.selection.insert(id);
    }
    h.run_steps(STEPS);
    h.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::Num1);
    h.run_steps(STEPS);
    hover(&mut h, CANVAS_CENTER);
    (h, drawing)
}

/// 種類ごとに 1 つ選んだ状態。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_properties_kinds() {
    type Pick = fn(&Drawing) -> EntityId;
    let kinds: [(&str, Pick); 6] = [
        ("a_line", |d| d.line),
        ("b_circle", |d| d.circle),
        ("c_arc", |d| d.arc),
        ("d_polyline", |d| d.polyline),
        ("e_xline", |d| d.xline),
        ("f_instance", |d| d.instance),
    ];
    for (name, pick) in kinds {
        let (mut h, _) = opened(|d| vec![pick(d)]);
        shot(&mut h, &format!("properties_{name}"));
    }
}

/// 何も選んでいない状態と、複数選択（レイヤが混在）。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_properties_empty_and_multi() {
    let (mut h, _) = opened(|_| Vec::new());
    shot(&mut h, "properties_g_empty");

    let (mut h, _) = opened(|d| vec![d.line, d.circle, d.arc, d.polyline]);
    shot(&mut h, "properties_h_multi_mixed");
}

/// レイヤのドロップダウンを開いたところ（ロック中の印つき）。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_properties_layer_dropdown() {
    use egui_kittest::kittest::Queryable as _;

    let (mut h, _) = opened(|d| vec![d.line, d.circle]);
    h.get_by_role(egui::accesskit::Role::ComboBox).click();
    h.run_steps(STEPS);
    shot(&mut h, "properties_i_layer_dropdown");
}

/// コマンド実行中（LINE の 1 点目の後）は表示だけで、案内が出る。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_properties_while_a_command_runs() {
    let (mut h, _) = opened(|d| vec![d.circle]);
    type_text(&mut h, "L");
    press(&mut h, egui::Key::Enter);
    hover(&mut h, egui::pos2(500.0, 300.0));
    shot(&mut h, "properties_j_busy");
}

/// レイヤ・コンポーネント・プロパティの 3 枚を同時に開いた状態（1280 x 800）。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_properties_three_panels() {
    let (mut h, _) = opened(|d| vec![d.line]);
    for command in ["LA", "CS"] {
        type_text(&mut h, command);
        press(&mut h, egui::Key::Enter);
    }
    hover(&mut h, CANVAS_CENTER);
    shot(&mut h, "properties_k_three_panels");
}

/// ロック中のレイヤへ移して選択から外れた直後（レイヤとプロパティの 2 枚）。案内が両方のパネルに出る。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_properties_moved_out_note() {
    let (mut h, _) = opened(|d| vec![d.line, d.circle]);
    type_text(&mut h, "LA");
    press(&mut h, egui::Key::Enter);
    let (doc, session) = h.state_mut().parts_mut();
    let locked = doc.layers().by_name("ロック済み").expect("ロック済み");
    let ids = session.selection.to_vec();
    session.apply_external(
        Box::new(cad_core::command::MoveEntitiesToLayer::new(ids, locked)),
        doc,
    );
    h.run_steps(STEPS);
    hover(&mut h, CANVAS_CENTER);
    shot(&mut h, "properties_l_moved_out_note");
}

/// コマンド実行中は、レイヤパネルの「移動」の行もグレーで、同じ案内が出る。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_properties_busy_with_the_layer_panel() {
    let (mut h, _) = opened(|d| vec![d.circle]);
    type_text(&mut h, "LA");
    press(&mut h, egui::Key::Enter);
    type_text(&mut h, "L");
    press(&mut h, egui::Key::Enter);
    hover(&mut h, egui::pos2(500.0, 300.0));
    shot(&mut h, "properties_m_busy_with_layers");
}

/// 中身が最大になる 3 枚: パラメータ 3 つと宣言を持つインスタンスを選び、長い名前のレイヤもある。
/// 幅 1280 / 1024 / 800px で、3 枚が重ならず、どのパネルの左側（見出し・「配置」・パラメータ名）も
/// 隠れないこと（`k_three_panels` は線分を選んでいて、コンポーネントパネルがほぼ空だった）。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_properties_crowded_three_panels() {
    for width in [1280.0, 1024.0, 800.0] {
        let mut h = Harness::builder()
            .with_size(egui::vec2(width, 800.0))
            .wgpu()
            .build_eframe(|cc| {
                let font = crate::jp_font::install(&cc.egui_ctx)
                    .map(|f| format!("{} (face {})", f.path.display(), f.index));
                CadApp::new(font)
            });
        h.run_steps(STEPS);
        let (doc, session) = h.state_mut().parts_mut();
        doc.apply(Box::new(AddLayer::new(
            "外壁_RC造_耐火被覆あり_2F",
            AciColor::RED,
        )))
        .expect("レイヤ");
        doc.apply(Box::new(DefineComponent::new(
            "COMPONENT",
            "窓",
            Point2::ORIGIN,
            vec![Entity::new(
                Geometry::Line(Line::new(Point2::ORIGIN, Point2::new(30.0, 0.0))),
                LayerId::ZERO,
            )],
        )))
        .expect("定義");
        let def = doc.definitions().by_name("窓").expect("窓");
        let params = vec![
            ParamDecl::number("開口幅", 900.0).with_range(300.0, 3000.0),
            ParamDecl::boolean("網戸", false),
            ParamDecl::choice("種別", vec!["引違い".to_owned(), "開き".to_owned()]).expect("候補"),
        ];
        doc.apply(Box::new(SetDefinitionParams::new("PARAM", def, params)))
            .expect("宣言");
        doc.apply(Box::new(SetBinding::new(
            "BIND",
            def,
            Binding::new(0, Slot::LineBx, parse("開口幅").expect("解析")),
        )))
        .expect("束縛");
        doc.apply(Box::new(InsertInstance::new(
            "INSERT",
            def,
            Placement::at(Point2::new(100.0, 100.0)),
            LayerId::ZERO,
        )))
        .expect("配置");
        session
            .selection
            .insert(doc.entities().ids().last().expect("インスタンス"));
        h.run_steps(STEPS);
        for command in ["LA", "CS"] {
            type_text(&mut h, command);
            press(&mut h, egui::Key::Enter);
        }
        h.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::Num1);
        h.run_steps(STEPS);
        hover(&mut h, egui::pos2(width / 8.0, 350.0));
        shot(&mut h, &format!("properties_n_crowded_{width}"));
    }
}

/// レイヤパネル 1 枚だけ（既定幅）で、長い名前のレイヤがあるとき。色・線種・削除が名前より先に
/// 幅を確保され、名前が省略される。「移動」の行も長い名前を省略する。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_properties_layer_panel_long_names() {
    let (mut h, _) = opened(|d| vec![d.line]);
    h.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::Num1);
    h.run_steps(STEPS);
    let (doc, _) = h.state_mut().parts_mut();
    for name in ["外壁_RC造_耐火被覆あり_2F", "A-WALL-EXTR-FIRE-RATED-2HR"] {
        doc.apply(Box::new(AddLayer::new(name, AciColor(3))))
            .expect("レイヤ");
    }
    type_text(&mut h, "LA");
    press(&mut h, egui::Key::Enter);
    hover(&mut h, CANVAS_CENTER);
    shot(&mut h, "properties_o_layer_long_names");
}

// ---- 数値の編集（段階 2） ------------------------------------------------------

/// 行の項目名 `label` と同じ高さにある数値の欄の中心（同じ名前のラベルがステータスバーなどに
/// あっても、右に数値の欄が並んでいる行を取る）。
fn field_center(h: &Harness<'_, CadApp>, label: &str) -> egui::Pos2 {
    use egui_kittest::kittest::Queryable as _;

    let fields: Vec<egui::Rect> = h
        .query_all_by_role(egui::accesskit::Role::SpinButton)
        .map(|n| n.rect())
        .collect();
    h.query_all_by_label(label)
        .map(|n| n.rect())
        .find_map(|row| {
            let y = row.center().y;
            // 同じ高さに隣のパネルの欄があることもあるので、項目名にいちばん近いものを取る。
            fields
                .iter()
                .filter(|r| r.min.y <= y && y <= r.max.y && r.min.x > row.min.x)
                .min_by(|a, b| a.min.x.total_cmp(&b.min.x))
                .copied()
        })
        .unwrap_or_else(|| panic!("項目 {label} の欄が無い"))
        .center()
}

/// 欄をクリックして `text` を打ち、`enter` なら Enter で確定する。
fn type_into(h: &mut Harness<'_, CadApp>, label: &str, text: &str, enter: bool) {
    let pos = field_center(h, label);
    super::click(h, pos);
    type_text(h, text);
    if enter {
        press(h, egui::Key::Enter);
    }
}

/// 線分の「始点 X」に値を打っている途中（入力欄になり、まだ確定していない）。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_properties_editing_a_value() {
    let (mut h, _) = opened(|d| vec![d.line]);
    type_into(&mut h, "始点 X", "25.5", false);
    shot(&mut h, "properties_p_editing");
}

/// 不正な値（長さ 0）を確定しようとした後。項目のすぐ下に理由が赤字で出て、値は元のまま。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_properties_invalid_value() {
    let (mut h, _) = opened(|d| vec![d.line]);
    type_into(&mut h, "長さ", "0", true);
    hover(&mut h, CANVAS_CENTER);
    shot(&mut h, "properties_q_invalid");

    // 円弧の開始角を終了角と同じにしようとした（掃引 0°）。
    let (mut h, _) = opened(|d| vec![d.arc]);
    type_into(&mut h, "開始角", "180", true);
    hover(&mut h, CANVAS_CENTER);
    shot(&mut h, "properties_q_invalid_arc");
}

/// 円の半径をドラッグしている途中。図面の円はそのままで、仮の円がラバーバンドの色で出る。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_properties_drag_preview() {
    let (mut h, _) = opened(|d| vec![d.circle]);
    let from = field_center(&h, "半径");
    h.event(egui::Event::PointerMoved(from));
    h.event(egui::Event::PointerButton {
        pos: from,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    let mut at = from;
    for _ in 0..8 {
        at += egui::vec2(8.0, 0.0);
        h.event(egui::Event::PointerMoved(at));
    }
    h.run_steps(STEPS);
    shot(&mut h, "properties_r_drag_preview");
}

/// 3 枚のパネルを開き、中身が最大になる状態（インスタンスを選び、倍率に不正な値を入れて理由の行を
/// 出した）。1280px・1024px・800px で、項目・欄・理由がパネルの中に収まり、隣へはみ出さないこと。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_properties_crowded_with_a_reason() {
    for width in [1280.0, 1024.0, 800.0] {
        let mut h = Harness::builder()
            .with_size(egui::vec2(width, 800.0))
            .wgpu()
            .build_eframe(|cc| {
                let font = crate::jp_font::install(&cc.egui_ctx)
                    .map(|f| format!("{} (face {})", f.path.display(), f.index));
                CadApp::new(font)
            });
        h.run_steps(STEPS);
        let (doc, session) = h.state_mut().parts_mut();
        let drawing = draw(doc);
        session.selection.insert(drawing.instance);
        h.run_steps(STEPS);
        for command in ["LA", "CS"] {
            type_text(&mut h, command);
            press(&mut h, egui::Key::Enter);
        }
        h.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::Num1);
        h.run_steps(STEPS);
        type_into(&mut h, "倍率", "-2", true);
        hover(&mut h, egui::pos2(width / 8.0, 350.0));
        shot(&mut h, &format!("properties_s_crowded_reason_{width}"));
    }
}

// ---- インプレース編集中の束縛（段階 3） ------------------------------------------

/// コンポーネント「窓」（線分 1 本・ポリライン 1 本）の編集に入った状態。線分の終点 X に `幅`、
/// 始点 Y に長い式、ポリラインの頂点 2 の Y に `高さ` を束縛してある。インスタンスは (100, 100) に
/// `rotation_deg` 度回して置く。返り値は（線分, ポリライン）。
fn in_component_edit(h: &mut Harness<'static, CadApp>, rotation_deg: f64) -> (EntityId, EntityId) {
    let (doc, _) = h.state_mut().parts_mut();
    doc.apply(Box::new(DefineComponent::new(
        "COMPONENT",
        "窓",
        Point2::ORIGIN,
        vec![
            Entity::new(
                Geometry::Line(Line::new(Point2::ORIGIN, Point2::new(120.0, 0.0))),
                LayerId::ZERO,
            ),
            Entity::new(
                Geometry::Polyline(Polyline::new(
                    vec![
                        Point2::new(0.0, 20.0),
                        Point2::new(120.0, 20.0),
                        Point2::new(120.0, 80.0),
                    ],
                    false,
                )),
                LayerId::ZERO,
            ),
        ],
    )))
    .expect("定義");
    let def = doc.definitions().by_name("窓").expect("窓");
    let params = vec![
        ParamDecl::number("幅", 120.0),
        ParamDecl::number("高さ", 80.0),
        ParamDecl::number("枠厚", 5.0),
        ParamDecl::boolean("開き", false),
    ];
    doc.apply(Box::new(SetDefinitionParams::new("PARAM", def, params)))
        .expect("宣言");
    for (entity, slot, expr) in [
        (0, Slot::LineBx, "幅"),
        (0, Slot::LineAy, "if 開き then 幅 * 2 + 枠厚 else 0"),
        (1, Slot::PolylineVy(2), "高さ"),
    ] {
        doc.apply(Box::new(SetBinding::new(
            "BIND",
            def,
            Binding::new(entity, slot, parse(expr).expect("解析")),
        )))
        .expect("束縛");
    }
    let placement = Placement::new(
        Point2::new(100.0, 100.0),
        rotation_deg.to_radians(),
        1.0,
        false,
    )
    .expect("配置");
    doc.apply(Box::new(InsertInstance::new(
        "INSERT",
        def,
        placement,
        LayerId::ZERO,
    )))
    .expect("配置");
    h.run_steps(STEPS);
    h.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::Num1);
    h.run_steps(STEPS);
    type_text(h, "EDITCOMP");
    press(h, egui::Key::Enter);
    // インスタンスの線分の上をクリックしたことにする（画面の位置ではなく図面の座標で渡す）。
    let (doc, session) = h.state_mut().parts_mut();
    // 線分の中ほど（定義の (60, 0)）を配置で図面へ移した点。
    let (sin, cos) = rotation_deg.to_radians().sin_cos();
    session.handle_click(
        Point2::new(100.0 + 60.0 * cos, 100.0 + 60.0 * sin),
        false,
        1.0,
        doc,
        &mut crate::selection::ScanAll,
    );
    let edit = session.editing().cloned().expect("編集中");
    let (members, origins) = edit.members(doc);
    let at = |i| members[origins.iter().position(|o| *o == Some(i)).expect("中身")];
    let ids = (at(0), at(1));
    h.run_steps(STEPS);
    ids
}

/// インプレース編集中、束縛（式）で決まる項目は表示だけで横に式が出る（長い式は省略）。
/// 束縛の無い項目は欄のまま。ツールチップには式の全体。ポリラインは束縛された頂点の行が出る。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_properties_bound_in_component_edit() {
    use egui_kittest::kittest::Queryable as _;

    let mut h = harness();
    let (line, polyline) = in_component_edit(&mut h, 0.0);
    h.state_mut().parts_mut().1.selection.insert(line);
    h.run_steps(STEPS);
    hover(&mut h, CANVAS_CENTER);
    shot(&mut h, "properties_t_bound_line");

    let badge = h
        .query_all_by_label_contains("「if")
        .next()
        .expect("長い式の案内")
        .rect()
        .center();
    hover(&mut h, badge);
    // ツールチップは少し待ってから出る。
    for _ in 0..4 {
        h.run_steps(STEPS);
    }
    shot(&mut h, "properties_t_bound_tooltip");

    let sel = &mut h.state_mut().parts_mut().1.selection;
    sel.clear();
    sel.insert(polyline);
    h.run_steps(STEPS);
    hover(&mut h, CANVAS_CENTER);
    shot(&mut h, "properties_t_bound_polyline");
}

/// 長い式の案内が付いた行のすぐ下に、編集できる行が来る並び（90° 回したインスタンスから入ると、
/// 定義の始点 Y の長い式が図面の「始点 X」に付き、「始点 Y」は欄のまま）。案内は折り返さず 1 行で、
/// 入り切らない分は式の側だけ省略される。1280px・パネル 1 枚と、1024px・3 枚（狭いので案内は値の
/// 下の行に出る）。PR #82 の操作レビューで、折り返した 2 行目が次の行の欄の下に隠れた。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_properties_long_expression_above_a_field() {
    for (width, three) in [(1280.0, false), (1024.0, true)] {
        let mut h = Harness::builder()
            .with_size(egui::vec2(width, 800.0))
            .wgpu()
            .build_eframe(|cc| {
                let font = crate::jp_font::install(&cc.egui_ctx)
                    .map(|f| format!("{} (face {})", f.path.display(), f.index));
                CadApp::new(font)
            });
        h.run_steps(STEPS);
        let (line, _) = in_component_edit(&mut h, 90.0);
        if three {
            for command in ["LA", "CS"] {
                type_text(&mut h, command);
                press(&mut h, egui::Key::Enter);
            }
        }
        h.state_mut().parts_mut().1.selection.insert(line);
        h.run_steps(STEPS);
        hover(&mut h, egui::pos2(width / 8.0, 350.0));
        shot(&mut h, &format!("properties_u_long_expression_{width}"));
    }
}
