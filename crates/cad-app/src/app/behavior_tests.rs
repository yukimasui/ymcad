//! アプリ全体を画面なしで動かして、フレーム内の順序に依存する振る舞いを固定する。
//!
//! `egui_kittest` は `.wgpu()` を付けなければ GPU なしで動く（描画はしない）。
//! 通常の `cargo test` で走らせる。PNG を撮るほうは `ui_snapshot.rs`（`#[ignore]`）。
//!
//! # イベントの入れ方
//!
//! `Harness::event` / `key_press` / `hover_at` で積んだイベントは、
//! **1 イベントにつき 1 フレーム**で処理される（`Harness::step` の実装）。
//! 「同じフレームに F12 と Enter」のように 1 フレームへまとめたいときは
//! [`frame`] で `input_mut().events` へ直接入れてから 1 回だけ回す。

use egui_kittest::Harness;

use super::CadApp;
use crate::cmdline::LineKind;

/// 画面の大きさ [px]。
const SCREEN: egui::Vec2 = egui::vec2(1280.0, 800.0);
/// キャンバスの中央付近。
const P1: egui::Pos2 = egui::pos2(500.0, 300.0);
/// P1 とは別のキャンバス上の点。
const P2: egui::Pos2 = egui::pos2(800.0, 450.0);
/// 1 操作ごとに回すフレーム数。Area の大きさは前フレームの実寸なので落ち着かせる。
const SETTLE: usize = 4;

fn app() -> Harness<'static, CadApp> {
    let mut h = Harness::builder()
        .with_size(SCREEN)
        .build_eframe(|_cc| CadApp::new(None));
    h.run_steps(SETTLE);
    h
}

/// 渡したイベントを 1 フレームでまとめて処理する。
fn frame(h: &mut Harness<'_, CadApp>, events: impl IntoIterator<Item = egui::Event>) {
    h.input_mut().events.extend(events);
    h.step();
}

fn settle(h: &mut Harness<'_, CadApp>) {
    h.run_steps(SETTLE);
}

/// キーを押して離す 2 イベント。
fn key(k: egui::Key) -> [egui::Event; 2] {
    [true, false].map(|pressed| egui::Event::Key {
        key: k,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    })
}

fn hover(h: &mut Harness<'_, CadApp>, pos: egui::Pos2) {
    frame(h, [egui::Event::PointerMoved(pos)]);
    settle(h);
}

fn type_text(h: &mut Harness<'_, CadApp>, text: &str) {
    frame(h, [egui::Event::Text(text.to_owned())]);
    settle(h);
}

fn press(h: &mut Harness<'_, CadApp>, k: egui::Key) {
    frame(h, key(k));
    settle(h);
}

fn preedit(text: &str) -> egui::Event {
    egui::Event::Ime(egui::ImeEvent::Preedit {
        text: text.to_owned(),
        active_range_chars: None,
    })
}

/// 履歴のうち、ユーザーの入力として記録された行。
fn input_lines(h: &Harness<'_, CadApp>) -> Vec<String> {
    h.state()
        .session
        .cmdline
        .history()
        .filter(|l| l.kind == LineKind::Input)
        .map(|l| l.text.clone())
        .collect()
}

fn input_rect(h: &Harness<'_, CadApp>) -> egui::Rect {
    h.state()
        .session
        .cmdline
        .input_rect()
        .expect("入力欄が描かれているはず")
}

/// 動的入力を指定の状態にしてから返す。
fn app_with_dynamic(on: bool) -> Harness<'static, CadApp> {
    let mut h = app();
    if h.state().session.cmdline.is_dynamic() != on {
        h.state_mut().toggle_dynamic_input();
        settle(&mut h);
    }
    assert_eq!(h.state().session.cmdline.is_dynamic(), on, "前提");
    h
}

// ---- 確定は 1 回だけ ------------------------------------------------------

/// `L` + Enter で LINE が 1 回だけ起動する。動的入力のオン・オフどちらでも。
#[test]
fn enter_submits_exactly_once_in_both_modes() {
    for on in [false, true] {
        let mut h = app_with_dynamic(on);
        hover(&mut h, P1);
        type_text(&mut h, "L");
        press(&mut h, egui::Key::Enter);
        assert_eq!(input_lines(&h), vec!["> LINE"], "動的入力 {on}");
        assert!(h.state().session.has_active_tool(), "動的入力 {on}");
        assert_eq!(h.state().session.cmdline.input(), "", "確定後は空になる");
    }
}

/// 同じフレームに F12 と Enter が来ても確定は 1 回だけで、切り替えは次のフレームから効く。
///
/// オン/オフをフレームの途中で使うと、画面下とカーソル横の両方で確定を処理しうる
/// （ADR-0034 決定 7）。
#[test]
fn f12_and_enter_in_the_same_frame_submit_once() {
    for on in [false, true] {
        let mut h = app_with_dynamic(on);
        hover(&mut h, P1);
        type_text(&mut h, "L");
        let mut events = Vec::new();
        events.extend(key(egui::Key::F12));
        events.extend(key(egui::Key::Enter));
        frame(&mut h, events);
        settle(&mut h);

        assert_eq!(input_lines(&h), vec!["> LINE"], "開始時の動的入力 {on}");
        assert!(h.state().session.has_active_tool());
        assert_eq!(
            h.state().session.cmdline.is_dynamic(),
            !on,
            "切り替わっている"
        );
    }
}

// ---- Esc ------------------------------------------------------------------

/// Esc の 1 回目は候補を閉じるだけで入力は残り、2 回目で入力を捨てる。
#[test]
fn escape_closes_suggestions_first_then_clears_the_input() {
    for on in [false, true] {
        let mut h = app_with_dynamic(on);
        hover(&mut h, P1);
        type_text(&mut h, "L");
        assert!(h.state().session.cmdline.suggestions_visible(), "前提");

        press(&mut h, egui::Key::Escape);
        assert!(
            !h.state().session.cmdline.suggestions_visible(),
            "1 回目で候補が閉じ、数フレーム後も閉じたまま（動的入力 {on}）"
        );
        assert_eq!(h.state().session.cmdline.input(), "L", "入力は残る");

        press(&mut h, egui::Key::Escape);
        assert_eq!(
            h.state().session.cmdline.input(),
            "",
            "2 回目で入力を捨てる"
        );
        assert!(input_lines(&h).is_empty(), "何も実行されていない");

        // 打ち直せば候補はまた出る。
        type_text(&mut h, "C");
        assert!(h.state().session.cmdline.suggestions_visible());
    }
}

/// Esc で実行中のコマンドを中断し、空 Enter で直前のコマンドを再実行する。
#[test]
fn escape_cancels_and_empty_enter_repeats_the_last_command() {
    for on in [false, true] {
        let mut h = app_with_dynamic(on);
        hover(&mut h, P1);
        type_text(&mut h, "L");
        press(&mut h, egui::Key::Enter);
        assert!(h.state().session.has_active_tool(), "前提");

        press(&mut h, egui::Key::Escape);
        assert!(
            !h.state().session.has_active_tool(),
            "中断（動的入力 {on}）"
        );

        press(&mut h, egui::Key::Enter);
        assert!(
            h.state().session.has_active_tool(),
            "再実行（動的入力 {on}）"
        );
        assert_eq!(input_lines(&h), vec!["> LINE", "> LINE"]);
    }
}

// ---- クリックの素通し -----------------------------------------------------

/// カーソル横の入力欄の上をクリックしても、キャンバスに点として届く。
///
/// egui の当たり判定は前フレームの矩形を使うので、Area が `interactable` だと
/// 入力欄があった場所のクリックを Area が奪う（ADR-0034 決定 2）。
#[test]
fn click_on_the_floating_box_reaches_the_canvas() {
    let mut h = app_with_dynamic(true);
    hover(&mut h, P1);
    type_text(&mut h, "L");
    press(&mut h, egui::Key::Enter);
    hover(&mut h, P1);
    assert!(
        h.state().session.last_point().is_none(),
        "前提: 始点はまだ無い"
    );

    // 前フレームに入力欄が描かれていた場所をクリックする。
    let target = input_rect(&h).center();
    assert!(
        h.state().viewport.rect().contains(target),
        "前提: キャンバスの上"
    );
    let button = |pressed| egui::Event::PointerButton {
        pos: target,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    frame(&mut h, [egui::Event::PointerMoved(target), button(true)]);
    frame(&mut h, [button(false)]);
    settle(&mut h);

    let expected = h.state().viewport.screen_to_model(target);
    assert_eq!(
        h.state().session.last_point(),
        Some(expected),
        "クリックした位置が始点として入る"
    );
}

// ---- 変換中 ---------------------------------------------------------------

/// 変換が始まった瞬間に入力欄が 1px も動かないこと（何もしていない状態から）。
///
/// 動くと、入力欄に付いて出る IME の候補ウィンドウが最初の 1 打鍵で跳ねる。
#[test]
fn input_does_not_move_when_composition_starts_from_idle() {
    let mut h = app_with_dynamic(true);
    hover(&mut h, P1);
    let before = input_rect(&h);

    frame(&mut h, [preedit("に")]);
    assert_eq!(input_rect(&h), before, "変換開始のフレーム");
    settle(&mut h);
    assert_eq!(input_rect(&h), before, "変換中");
}

/// 変換が始まった瞬間に入力欄が 1px も動かないこと（コマンド実行中から）。
#[test]
fn input_does_not_move_when_composition_starts_while_a_tool_runs() {
    let mut h = app_with_dynamic(true);
    hover(&mut h, P1);
    type_text(&mut h, "L");
    press(&mut h, egui::Key::Enter);
    hover(&mut h, P1);
    assert!(h.state().session.has_active_tool(), "前提");
    let before = input_rect(&h);

    frame(&mut h, [preedit("に")]);
    assert_eq!(input_rect(&h), before, "変換開始のフレーム");
    settle(&mut h);
    assert_eq!(input_rect(&h), before, "変換中");
}

/// 変換中はマウスを動かしても入力欄が動かず、Enter で確定しない。
/// 確定（Commit）で追従に戻る。
#[test]
fn composing_freezes_the_box_and_blocks_enter() {
    let mut h = app_with_dynamic(true);
    hover(&mut h, P1);
    frame(&mut h, [preedit("にほんご")]);
    settle(&mut h);
    let frozen = input_rect(&h);

    hover(&mut h, P2);
    assert_eq!(input_rect(&h), frozen, "変換中はマウスに付いていかない");

    press(&mut h, egui::Key::Enter);
    assert!(input_lines(&h).is_empty(), "変換中の Enter で確定しない");
    assert!(!h.state().session.has_active_tool());

    frame(
        &mut h,
        [egui::Event::Ime(egui::ImeEvent::Commit("日本".to_owned()))],
    );
    hover(&mut h, P2);
    assert_ne!(input_rect(&h), frozen, "確定後は追従に戻る");
    assert!(
        input_rect(&h).min.x > P2.x && input_rect(&h).min.y > P2.y,
        "カーソルの右下"
    );
}
