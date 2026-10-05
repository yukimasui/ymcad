//! UI の見た目を画面に出さずに PNG へ撮る。
//!
//! PNG にする（`Harness::render()`）には **wgpu のアダプタが要る**ので `#[ignore]`。
//! 実 GPU でなくても、lavapipe 等のソフトウェア Vulkan があれば撮れる。実行:
//! `cargo test -p cad-app -- --ignored ui_snapshot`
//! 出力先は `target/ui-snapshots/`（リポジトリ管理外）。画像の比較はせず撮るだけ。
//! 撮った画像は人（またはエージェント）が開いて見た目を確かめる。
//!
//! **振る舞いの検査はここに置かない。** `egui_kittest` は `.wgpu()` を付けず
//! `render()` を呼ばなければ GPU なしで動くので、確定の回数やクリックの素通しなどは
//! `app/behavior_tests.rs` で通常のテストとして固定している。
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

// ---- 寸法入力（Issue #20 段階 B） -------------------------------------------

fn press(harness: &mut Harness<'_, CadApp>, key: egui::Key) {
    harness.key_press(key);
    harness.run_steps(STEPS);
}

fn click(harness: &mut Harness<'_, CadApp>, pos: egui::Pos2) {
    harness.hover_at(pos);
    harness.run_steps(1);
    harness.event(egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    harness.event(egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    harness.run_steps(STEPS);
}

/// 2 点目の位置。1 点目（キャンバス中央）の右上。
const SECOND: egui::Pos2 = egui::pos2(820.0, 250.0);

/// LINE の 1 点目をキャンバス中央に置き、カーソルを右上へ動かした状態。
fn line_second_point() -> Harness<'static, CadApp> {
    let mut h = harness();
    hover(&mut h, CANVAS_CENTER);
    type_text(&mut h, "L");
    press(&mut h, egui::Key::Enter);
    click(&mut h, CANVAS_CENTER);
    hover(&mut h, SECOND);
    h
}

/// LINE の 2 点目 → 長さ・角度の 2 欄にライブ値が薄く出る。
/// 続けて長さを打つ → 長さの欄に入る。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_dim_line_fields() {
    let mut h = line_second_point();
    shot(&mut h, "dim_a_line_live_values");

    type_text(&mut h, "100");
    shot(&mut h, "dim_b_length_typed");
}

/// 長さを固定（錠前）して角度を入力中 → ラバーバンドは長さ 100 に固定される。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_dim_length_locked() {
    let mut h = line_second_point();
    type_text(&mut h, "100");
    press(&mut h, egui::Key::Tab);
    type_text(&mut h, "30");
    shot(&mut h, "dim_c_length_locked_angle_typing");
}

/// MOVE の 2 点目（目的点）でも 2 欄が出る。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_dim_move() {
    let mut h = harness();
    hover(&mut h, CANVAS_CENTER);
    // 矩形を描き、MOVE でその角をクリックして選ぶ。
    let corner = egui::pos2(560.0, 300.0);
    type_text(&mut h, "REC");
    press(&mut h, egui::Key::Enter);
    click(&mut h, corner);
    click(&mut h, egui::pos2(700.0, 400.0));
    type_text(&mut h, "M");
    press(&mut h, egui::Key::Enter);
    click(&mut h, corner);
    press(&mut h, egui::Key::Enter); // 選択を確定
    click(&mut h, CANVAS_CENTER); // 基点
    hover(&mut h, SECOND);
    shot(&mut h, "dim_d_move_second_point");
}

/// 数値以外（`@`）を打つ → 欄の表示をやめ、通常の入力欄に戻る。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_dim_back_to_plain_input() {
    let mut h = line_second_point();
    type_text(&mut h, "@50,20");
    shot(&mut h, "dim_e_non_number_plain_input");
}

// ---- 直交モード・極トラッキング（Issue #29） ---------------------------------

/// 補助を `keys` で切り替えてから LINE の 1 点目をキャンバス中央に置き、
/// 1 点目から画面上で `deg`°・`r` px の位置を指した状態。
fn line_tracking(keys: &[egui::Key], deg: f32, r: f32) -> Harness<'static, CadApp> {
    let mut h = harness();
    hover(&mut h, CANVAS_CENTER);
    for k in keys {
        press(&mut h, *k);
    }
    type_text(&mut h, "L");
    press(&mut h, egui::Key::Enter);
    click(&mut h, CANVAS_CENTER);
    // 1 点目を置いた後は履歴が増えないので、1 点目の画面位置は中央のまま。
    let (s, c) = deg.to_radians().sin_cos();
    hover(
        &mut h,
        egui::pos2(CANVAS_CENTER.x + r * c, CANVAS_CENTER.y - r * s),
    );
    h
}

/// F8 → LINE の 2 点目を斜め（30°）に指す → ラバーバンドは水平。ステータスバーに `ORTHO`。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_ortho_line() {
    let mut h = line_tracking(&[egui::Key::F8], 30.0, 200.0);
    shot(&mut h, "ortho_a_line_horizontal");
}

/// F10 → 40° 付近を指す → 45° に吸い付き、補助線（点線）と `45°` が出る。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_polar_snapped() {
    let mut h = line_tracking(&[egui::Key::F10], 40.0, 100.0);
    shot(&mut h, "polar_b_snapped_45");
}

/// 両方オン → ステータスバーに `ORTHO` と `POLAR`（直交が優先なので POLAR は控えめの色）。
/// 40° を指しても直交が勝つので水平になり、極の補助線は出ない。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_ortho_and_polar_status() {
    let mut h = line_tracking(&[egui::Key::F8, egui::Key::F10], 40.0, 100.0);
    shot(&mut h, "ortho_polar_c_both_on_status");
}
