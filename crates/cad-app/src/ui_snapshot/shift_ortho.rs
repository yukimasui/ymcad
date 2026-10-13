//! Shift で直交を一時的に反転している間（Issue #36、ADR-0049）の見た目。
//!
//! 反転中のステータスバー（ORTHO / POLAR を実際に効いている状態で出し、下に帯を敷く）と
//! 水平に拘束されたラバーバンドを `target/ui-snapshots/shift_ortho_*.png` に撮る。

use super::{line_tracking, shot, STEPS};

/// Shift を押す（修飾キーの状態だけを変える）。
fn hold_shift(harness: &mut egui_kittest::Harness<'_, crate::app::CadApp>) {
    harness.event(egui::Event::ModifiersChanged(egui::Modifiers::SHIFT));
    harness.run_steps(STEPS);
}

/// どちらもオフで LINE の 2 点目を 30° に指し、Shift を押す → 水平。ステータスバーに `ORTHO`（帯つき）。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_shift_ortho_from_off() {
    let mut h = line_tracking(&[], 30.0, 200.0);
    hold_shift(&mut h);
    shot(&mut h, "shift_ortho_a_from_off");
}

/// POLAR だけオンで Shift → 直交が効き、POLAR は「直交に負けている」色（帯つき）。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_shift_ortho_with_polar() {
    let mut h = line_tracking(&[egui::Key::F10], 30.0, 200.0);
    hold_shift(&mut h);
    shot(&mut h, "shift_ortho_b_with_polar");
}

/// 両方オンで Shift → 直交が外れて極が効く（43° を指して 45° に吸い付く）。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_shift_ortho_both_on() {
    let mut h = line_tracking(&[egui::Key::F8, egui::Key::F10], 43.0, 100.0);
    hold_shift(&mut h);
    shot(&mut h, "shift_ortho_c_both_on_polar_wins");
}
