//! 全選択（Ctrl+A / SELECTALL、Issue #34 段階 3）の見た目。
//!
//! 全選択した状態（ロックされたレイヤの図形は選ばれない）と、リボンのホームの「選択」グループを
//! `target/ui-snapshots/select_all_*.png` / `ribbon_j_*.png` に撮る。

use cad_core::command::{AddEntities, AddLayer, SetLayerProperties};
use cad_core::geom::{Circle, Line, Point2, Polyline};
use cad_core::{AciColor, Document, Entity, Geometry, LayerId};

use super::{harness, hover, ribbon_button_rect, shot, CANVAS_CENTER, STEPS};

/// レイヤ 0 に線分・円・ポリライン、レイヤ `ロック済み`（ロック）に線分と円を並べる。
fn draw(doc: &mut Document) {
    doc.apply(Box::new(AddLayer::new("ロック済み", AciColor(2))))
        .expect("レイヤ");
    let locked = doc.layers().by_name("ロック済み").expect("ロック済み");
    doc.apply(Box::new(SetLayerProperties::new(locked).locked(true)))
        .expect("ロック");
    let entities = vec![
        Entity::new(
            Geometry::Line(Line::new(
                Point2::new(40.0, 40.0),
                Point2::new(160.0, 100.0),
            )),
            LayerId::ZERO,
        ),
        Entity::new(
            Geometry::Circle(Circle::new(Point2::new(250.0, 90.0), 40.0)),
            LayerId::ZERO,
        ),
        Entity::new(
            Geometry::Polyline(Polyline::new(
                vec![
                    Point2::new(230.0, 170.0),
                    Point2::new(330.0, 170.0),
                    Point2::new(330.0, 240.0),
                ],
                false,
            )),
            LayerId::ZERO,
        ),
        Entity::new(
            Geometry::Line(Line::new(
                Point2::new(40.0, 220.0),
                Point2::new(160.0, 260.0),
            )),
            locked,
        ),
        Entity::new(
            Geometry::Circle(Circle::new(Point2::new(120.0, 160.0), 25.0)),
            locked,
        ),
    ];
    doc.apply(Box::new(AddEntities::many("TEST", entities)))
        .expect("追加");
}

/// プロパティパネルを開いて Ctrl+A。レイヤ 0 の 3 つだけが選択色になり、ロックされたレイヤの
/// 2 つ（黄色、ACI 2）は選ばれない。パネルに「3 個を選択」、履歴に案内が出る。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_select_all() {
    let mut h = harness();
    let (doc, _) = h.state_mut().parts_mut();
    draw(doc);
    h.run_steps(STEPS);
    h.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::Num1);
    h.run_steps(STEPS);
    hover(&mut h, CANVAS_CENTER);
    h.key_press_modifiers(
        egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
        egui::Key::A,
    );
    h.run_steps(STEPS);
    shot(&mut h, "select_all_a_ctrl_a_with_locked_layer");
}

/// リボンのホームの末尾の「選択」グループ。ホームが収まる幅（1400px）で撮り、
/// SELECTALL に乗せたツールチップも撮る（1280px では「›」の先になる。`ribbon_a_home_1280`）。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_ribbon_select_group() {
    let mut h = harness();
    h.set_size(egui::vec2(1400.0, 800.0));
    hover(&mut h, egui::pos2(700.0, 400.0));
    shot(&mut h, "ribbon_j_home_1400_select_group");

    let button = ribbon_button_rect(&h, "SELECTALL").center();
    h.hover_at(button);
    h.run_steps(60);
    shot(&mut h, "ribbon_k_selectall_tooltip");
}
