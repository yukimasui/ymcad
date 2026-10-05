//! UI の見た目を画面に出さずに PNG へ撮る。
//!
//! GPU（lavapipe 等のソフトウェア Vulkan でも可）が要るので `#[ignore]`。実行:
//! `cargo test -p cad-app -- --ignored ui_snapshot`
//! 出力先は `target/ui-snapshots/`（リポジトリ管理外）。画像の比較はせず撮るだけ。
//! 撮った画像は人（またはエージェント）が開いて見た目を確かめる。
//!
//! `Harness::run()` は再描画の要求が止まるまで回すので、カーソルの点滅などで
//! 終わらないことがある。**`run_steps(n)` を使う。**

use egui_kittest::Harness;
use std::path::PathBuf;

use crate::app::CadApp;

/// 画面の大きさ [px]。
const SCREEN: egui::Vec2 = egui::vec2(1280.0, 800.0);
/// キャンバスの中央付近（画面下のコマンドライン・ステータスバーを避けた位置）。
const CANVAS_CENTER: egui::Pos2 = egui::pos2(640.0, 350.0);
/// 1 操作ごとに回すフレーム数。Area の大きさが前フレームの実寸で決まるので、
/// 複数フレーム回して落ち着かせる。
const STEPS: usize = 5;

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

fn harness() -> Harness<'static, CadApp> {
    let mut harness = Harness::builder()
        .with_size(SCREEN)
        .wgpu()
        .build_eframe(|cc| {
            let font = crate::jp_font::install(&cc.egui_ctx)
                .map(|f| format!("{} (face {})", f.path.display(), f.index));
            CadApp::new(font)
        });
    harness.run_steps(STEPS);
    harness
}

fn type_text(harness: &mut Harness<'_, CadApp>, text: &str) {
    harness.event(egui::Event::Text(text.to_owned()));
    harness.run_steps(STEPS);
}

fn hover(harness: &mut Harness<'_, CadApp>, pos: egui::Pos2) {
    harness.hover_at(pos);
    harness.run_steps(STEPS);
}

/// 何もしていない状態。カーソル横には何も見えないこと。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_dyn_idle() {
    let mut h = harness();
    hover(&mut h, CANVAS_CENTER);
    shot(&mut h, "dyn_a_idle_nothing_beside_cursor");
}

/// キャンバス中央で `L` を打つ → カーソル横に入力と候補が出る。
/// 続けて LINE を始めてマウスを動かす → カーソル横にプロンプトが出て追従する。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_dyn_line_command() {
    let mut h = harness();
    hover(&mut h, CANVAS_CENTER);
    type_text(&mut h, "L");
    shot(&mut h, "dyn_b_typed_l_suggestions_beside_cursor");

    h.key_press(egui::Key::Enter);
    h.run_steps(STEPS);
    hover(&mut h, egui::pos2(700.0, 380.0));
    hover(&mut h, egui::pos2(760.0, 400.0));
    shot(&mut h, "dyn_c_line_running_prompt_beside_cursor");
}

/// キャンバス右下隅の近くで `L` → 左上側へ回り込む。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_dyn_wraps_at_corner() {
    let mut h = harness();
    // キャンバスの右下隅の近く（ステータスバーのすぐ上）。右にも下にも入らない。
    hover(&mut h, egui::pos2(1265.0, 725.0));
    type_text(&mut h, "L");
    shot(&mut h, "dyn_d_corner_wraps_up_left");
}

/// F12 でオフにして `L` → 従来どおり画面下の入力欄と候補になる。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_dyn_off_falls_back_to_bottom() {
    let mut h = harness();
    hover(&mut h, CANVAS_CENTER);
    h.key_press(egui::Key::F12);
    h.run_steps(STEPS);
    type_text(&mut h, "L");
    shot(&mut h, "dyn_e_off_bottom_cmdline");
}

/// 不明なコマンドを確定した直後 → カーソル横にエラーが赤で出て、履歴にも残る。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_dyn_error() {
    let mut h = harness();
    hover(&mut h, CANVAS_CENTER);
    type_text(&mut h, "QQQ");
    h.key_press(egui::Key::Enter);
    h.run_steps(STEPS);
    shot(&mut h, "dyn_f_unknown_command_error");
}

/// 変換中（Preedit あり）→ カーソル横に `[変換中]` と未確定文字列が出る。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_dyn_composing() {
    let mut h = harness();
    hover(&mut h, CANVAS_CENTER);
    h.event(egui::Event::Ime(egui::ImeEvent::Preedit {
        text: "にほんご".to_owned(),
        active_range_chars: None,
    }));
    h.run_steps(STEPS);
    shot(&mut h, "dyn_g_composing");
}
