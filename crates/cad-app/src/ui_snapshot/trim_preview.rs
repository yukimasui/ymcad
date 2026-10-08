//! TRIM / EXTEND の結果プレビュー（Issue #34 段階 2）の見た目。
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

/// TRIM 中: 2 本の横線の間に乗せる → 縦の線分の間の部分が赤の破線になる（下の実線は見えない）。
/// 続けて上の端に乗せる → 端から交点までが赤の破線になる。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_trim_preview() {
    let mut h = harness();
    draw_line(&mut h, egui::pos2(640.0, 180.0), egui::pos2(640.0, 520.0));
    draw_line(&mut h, egui::pos2(500.0, 280.0), egui::pos2(800.0, 280.0));
    draw_line(&mut h, egui::pos2(500.0, 420.0), egui::pos2(800.0, 420.0));
    type_text(&mut h, "TR");
    press(&mut h, egui::Key::Enter);
    hover(&mut h, egui::pos2(642.0, 350.0));
    shot(&mut h, "trim_preview_a_middle_goes_away");
    hover(&mut h, egui::pos2(642.0, 230.0));
    shot(&mut h, "trim_preview_b_end_goes_away");
}

/// EXTEND 中: 短い縦の線分の上の端寄りに乗せる → 上の横線まで琥珀色の破線が伸びる。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_extend_preview() {
    let mut h = harness();
    draw_line(&mut h, egui::pos2(640.0, 320.0), egui::pos2(640.0, 440.0));
    draw_line(&mut h, egui::pos2(500.0, 200.0), egui::pos2(800.0, 200.0));
    type_text(&mut h, "EX");
    press(&mut h, egui::Key::Enter);
    hover(&mut h, egui::pos2(642.0, 340.0));
    shot(&mut h, "trim_preview_c_extend_grows_to_the_boundary");
}
