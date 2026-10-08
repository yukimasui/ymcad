//! ホバーで強調する選択プレビュー（Issue #34 段階 1）の見た目。
//!
//! 作図領域は履歴が伸びると上下に縮み、画面上の y がずれる（x はずれない）。
//! 乗せる位置が図形から外れないよう、乗せる図形は縦の線分にする。

use egui_kittest::Harness;

use super::{click, harness, hover, press, shot, type_text, CadApp};

/// 画面上の 2 点を結ぶ線分を引く。
fn draw_line(h: &mut Harness<'_, CadApp>, a: egui::Pos2, b: egui::Pos2) {
    type_text(h, "L");
    press(h, egui::Key::Enter);
    click(h, a);
    click(h, b);
    press(h, egui::Key::Escape);
}

/// 待機中: 左の線分に乗せる（紫の縁取り）。右の線分は選択済み（水色）で、見分けられること。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_hover_idle() {
    let mut h = harness();
    draw_line(&mut h, egui::pos2(500.0, 200.0), egui::pos2(500.0, 500.0));
    draw_line(&mut h, egui::pos2(700.0, 200.0), egui::pos2(700.0, 500.0));
    click(&mut h, egui::pos2(700.0, 350.0));
    hover(&mut h, egui::pos2(502.0, 350.0));
    shot(&mut h, "hover_a_idle_line_beside_a_selected_one");
}

/// グループの一員に乗せる → グループ全体（2 本）が強調される。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_hover_group() {
    let mut h = harness();
    draw_line(&mut h, egui::pos2(500.0, 200.0), egui::pos2(500.0, 500.0));
    draw_line(&mut h, egui::pos2(560.0, 200.0), egui::pos2(560.0, 500.0));
    draw_line(&mut h, egui::pos2(700.0, 200.0), egui::pos2(700.0, 500.0));
    click(&mut h, egui::pos2(500.0, 350.0));
    click(&mut h, egui::pos2(560.0, 350.0));
    type_text(&mut h, "G");
    press(&mut h, egui::Key::Enter);
    // 名前は既定のまま。
    press(&mut h, egui::Key::Enter);
    // 何も無い所をクリックして選択を外す。
    click(&mut h, egui::pos2(900.0, 300.0));
    hover(&mut h, egui::pos2(502.0, 350.0));
    shot(&mut h, "hover_b_group_member_highlights_the_group");
}

/// TRIM 中: 交点の近くに乗せても、乗せた縦の線分 1 本だけが強調され、スナップのマーカーは出ない。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_hover_trim() {
    let mut h = harness();
    draw_line(&mut h, egui::pos2(640.0, 200.0), egui::pos2(640.0, 500.0));
    draw_line(&mut h, egui::pos2(500.0, 350.0), egui::pos2(800.0, 350.0));
    type_text(&mut h, "TR");
    press(&mut h, egui::Key::Enter);
    hover(&mut h, egui::pos2(642.0, 340.0));
    shot(&mut h, "hover_c_trim_one_entity_no_snap");
}
