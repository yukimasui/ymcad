//! 動的入力のそばのヒント（Issue #27 段階 1・2）の見た目。
//!
//! 候補の説明（左にリボンのアイコン）を、画面中央・↓ で選び直したとき・下端・右端・隅で
//! `target/ui-snapshots/hint_*.png` に撮る。

use super::{click, harness, hover, press, shot, type_text, CANVAS_CENTER};

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

/// 実行中に今できること（段階 2）: LINE の始点 → 2 点目（寸法入力・作図補助の 2 行目）→
/// 数字を打った（Tab・Enter・Esc の意味が変わる）→ F8 で直交オン → 3 本目（C 閉じる）。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_hint_actions_line() {
    let mut h = harness();
    hover(&mut h, CANVAS_CENTER);
    type_text(&mut h, "L");
    press(&mut h, egui::Key::Enter);
    hover(&mut h, CANVAS_CENTER);
    shot(&mut h, "hint_g_actions_line_first_point");

    click(&mut h, CANVAS_CENTER);
    hover(&mut h, egui::pos2(760.0, 300.0));
    shot(&mut h, "hint_h_actions_line_second_point");

    type_text(&mut h, "100");
    shot(&mut h, "hint_i_actions_length_typed");

    press(&mut h, egui::Key::Escape);
    press(&mut h, egui::Key::F8);
    click(&mut h, egui::pos2(760.0, 300.0));
    hover(&mut h, egui::pos2(700.0, 450.0));
    click(&mut h, egui::pos2(700.0, 450.0));
    hover(&mut h, egui::pos2(500.0, 420.0));
    shot(&mut h, "hint_j_actions_third_segment_ortho_on");
}

/// 選択待ち（MOVE）・図形を指す（TRIM）・オプションだけ（ZOOM）・オプション付きの点（XLINE）。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_hint_actions_stages() {
    let mut h = harness();
    hover(&mut h, CANVAS_CENTER);
    type_text(&mut h, "M");
    press(&mut h, egui::Key::Enter);
    hover(&mut h, CANVAS_CENTER);
    shot(&mut h, "hint_k_actions_select");

    press(&mut h, egui::Key::Escape);
    type_text(&mut h, "TR");
    press(&mut h, egui::Key::Enter);
    hover(&mut h, CANVAS_CENTER);
    shot(&mut h, "hint_l_actions_trim");

    press(&mut h, egui::Key::Escape);
    type_text(&mut h, "XL");
    press(&mut h, egui::Key::Enter);
    hover(&mut h, CANVAS_CENTER);
    shot(&mut h, "hint_m_actions_xline_options");

    press(&mut h, egui::Key::Escape);
    type_text(&mut h, "Z");
    press(&mut h, egui::Key::Enter);
    hover(&mut h, egui::pos2(1200.0, 700.0));
    shot(&mut h, "hint_n_actions_zoom_corner");
}
