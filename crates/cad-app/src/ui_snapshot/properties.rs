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
