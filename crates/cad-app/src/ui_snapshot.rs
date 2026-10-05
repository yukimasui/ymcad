//! UI の見た目を画面に出さずに PNG へ撮るスパイク。
//!
//! GPU（lavapipe 等のソフトウェア Vulkan でも可）が要るので `#[ignore]`。実行:
//! `cargo test -p cad-app -- --ignored ui_snapshot`
//! 出力先は `target/ui-snapshots/`（リポジトリ管理外）。画像の比較はせず撮るだけ。

use egui_kittest::Harness;
use std::path::PathBuf;

use crate::app::CadApp;

fn out_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/ui-snapshots");
    std::fs::create_dir_all(&dir).expect("出力先を作れない");
    dir
}

fn shot(harness: &mut Harness<'_, CadApp>, name: &str) {
    let img = harness.render().expect("描画に失敗");
    let path = out_dir().join(format!("{name}.png"));
    img.save(&path).expect("PNG を保存できない");
    println!("saved {}", path.display());
}

#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_startup_line_command() {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .wgpu()
        .build_eframe(|cc| {
            let font = crate::jp_font::install(&cc.egui_ctx)
                .map(|f| format!("{} (face {})", f.path.display(), f.index));
            CadApp::new(font)
        });
    harness.run_steps(5);
    shot(&mut harness, "a_startup");

    // キャンバス中央付近へマウスを移動し、L を打つ。
    harness.hover_at(egui::pos2(640.0, 400.0));
    harness.run_steps(5);
    harness.event(egui::Event::Text("L".to_owned()));
    harness.run_steps(5);
    shot(&mut harness, "b_typed_l");

    // Enter で LINE を開始し、マウスを少し動かす。
    harness.key_press(egui::Key::Enter);
    harness.run_steps(5);
    harness.hover_at(egui::pos2(700.0, 430.0));
    harness.run_steps(5);
    harness.hover_at(egui::pos2(760.0, 450.0));
    harness.run_steps(5);
    shot(&mut harness, "c_line_started");
}
