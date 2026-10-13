//! 動的入力のそばのヒント（Issue #27 段階 1）の見た目。
//!
//! 候補の説明（左にリボンのアイコン）を、画面中央・↓ で選び直したとき・下端・右端・隅で
//! `target/ui-snapshots/hint_*.png` に撮る。

use super::{harness, hover, press, shot, type_text, CANVAS_CENTER};

/// 中央で `L` → ⏎ の LINE の説明が枠の下に出る。
/// `S` → ↓↓ で 2 番目の候補を選ぶと、説明がそちらに切り替わる。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_hint_center() {
    let mut h = harness();
    hover(&mut h, CANVAS_CENTER);
    type_text(&mut h, "L");
    shot(&mut h, "hint_a_center_line");

    press(&mut h, egui::Key::Escape);
    press(&mut h, egui::Key::Escape);
    type_text(&mut h, "S");
    press(&mut h, egui::Key::ArrowDown);
    press(&mut h, egui::Key::ArrowDown);
    shot(&mut h, "hint_b_center_arrow_down_twice");
}

/// 下端寄り（枠が上へ回り込む）・右端寄り・右下の隅。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_hint_edges() {
    let mut h = harness();
    hover(&mut h, egui::pos2(640.0, 715.0));
    type_text(&mut h, "MI");
    shot(&mut h, "hint_c_bottom_edge_above_the_box");

    hover(&mut h, egui::pos2(1250.0, 300.0));
    shot(&mut h, "hint_d_right_edge");

    // 枠の下には入らないが、枠はカーソルの下に入る高さ。ヒントは枠の右へ回る。
    hover(&mut h, egui::pos2(300.0, 560.0));
    shot(&mut h, "hint_e_beside_the_box");

    hover(&mut h, egui::pos2(1265.0, 725.0));
    shot(&mut h, "hint_f_bottom_right_corner");
}
