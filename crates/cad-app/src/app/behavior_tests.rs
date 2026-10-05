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

/// キャンバスの 1 点をクリックする（押す・離すを別フレームで）。
fn click(h: &mut Harness<'_, CadApp>, pos: egui::Pos2) {
    let button = |pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    frame(h, [egui::Event::PointerMoved(pos), button(true)]);
    frame(h, [button(false)]);
    settle(h);
}

/// Esc で閉じた記憶が、コマンドの実行をまたいで残らないこと。
///
/// コマンド実行中は候補を作り直さないので、閉じたときと同じ文字列が入力欄に
/// 残ったままコマンドが終わると、候補が出ないことがあった（PR #21 の再レビュー）。
#[test]
fn dismissed_suggestions_do_not_outlive_a_command() {
    for on in [false, true] {
        let mut h = app_with_dynamic(on);
        hover(&mut h, P1);
        type_text(&mut h, "C");
        press(&mut h, egui::Key::Escape);
        assert!(
            !h.state().session.cmdline.suggestions_visible(),
            "前提: 閉じた"
        );
        // 閉じた後の Enter は打った文字のとおり（C = CIRCLE）に実行される。
        press(&mut h, egui::Key::Enter);
        assert!(h.state().session.has_active_tool(), "前提: CIRCLE 実行中");

        // 実行中に同じ文字を打っておき、クリックだけでコマンドを終える。
        type_text(&mut h, "C");
        click(&mut h, P1);
        click(&mut h, P2);
        assert!(
            !h.state().session.has_active_tool(),
            "前提: CIRCLE が終わった"
        );
        assert_eq!(h.state().doc.entities().len(), 1, "前提: 円ができた");
        assert_eq!(
            h.state().session.cmdline.input(),
            "C",
            "前提: 入力が残っている"
        );

        assert!(
            h.state().session.cmdline.suggestions_visible(),
            "コマンドが終われば候補が出る（動的入力 {on}）"
        );
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

/// キャンバスの右端寄りでも、変換が始まった前後で入力欄が 1px も動かないこと。
///
/// `[変換中]` のぶん Area が広がると、`constrain_to` が次のフレームで
/// Area をキャンバスの内側へ押し戻し、入力欄が左へずれていた（PR #21 の再レビュー。
/// 実測 -21〜-46px）。ずれが起きる帯は Area の幅しだいで動くので、
/// 右端寄りを一定間隔でなめて、どこでも動かないことを確かめる。
#[test]
fn input_does_not_move_when_composition_starts_near_the_right_edge() {
    let mut h = app_with_dynamic(true);
    hover(&mut h, P1);
    type_text(&mut h, "L");
    press(&mut h, egui::Key::Enter);
    assert!(h.state().session.has_active_tool(), "前提");

    let right = h.state().viewport.rect().right();
    let mut x = right - 400.0;
    while x < right {
        let cursor = egui::pos2(x, 300.0);
        hover(&mut h, cursor);
        let before = input_rect(&h);

        frame(&mut h, [preedit("に")]);
        assert_eq!(input_rect(&h), before, "変換開始のフレーム（x = {x}）");
        settle(&mut h);
        assert_eq!(input_rect(&h), before, "変換中・押し戻されない（x = {x}）");

        // 変換を取り消して次の位置へ。
        frame(&mut h, [preedit("")]);
        x += 10.0;
    }
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

// ---- Tab とフォーカス（Issue #22） ------------------------------------------

/// Tab で補完したあとも入力欄にフォーカスが残り、続けて打った文字が入る。
///
/// `TextEdit` の既定では egui が Tab を「次の部品へフォーカスを移す」に使うので、
/// こちらが Tab を処理するより先にフォーカスが外れていた。
#[test]
fn tab_completion_keeps_focus_in_the_command_line() {
    for on in [false, true] {
        let mut h = app_with_dynamic(on);
        hover(&mut h, P1);
        type_text(&mut h, "L");
        press(&mut h, egui::Key::Tab);
        assert_eq!(
            h.state().session.cmdline.input(),
            "LINE",
            "補完（動的入力 {on}）"
        );
        type_text(&mut h, "Z");
        assert_eq!(
            h.state().session.cmdline.input(),
            "LINEZ",
            "補完のあとに打った文字が入る（動的入力 {on}）"
        );
    }
}

/// 候補が出ていないときの Tab でも、入力欄からフォーカスが出ていかない。
#[test]
fn tab_without_suggestions_keeps_focus() {
    for on in [false, true] {
        let mut h = app_with_dynamic(on);
        hover(&mut h, P1);
        // 候補が出ない入力。
        type_text(&mut h, "XYZZY");
        assert!(!h.state().session.cmdline.suggestions_visible(), "前提");
        press(&mut h, egui::Key::Tab);
        type_text(&mut h, "1");
        assert_eq!(h.state().session.cmdline.input(), "XYZZY1", "動的入力 {on}");

        // コマンド実行中（候補を出さない段階）でも同じ。
        press(&mut h, egui::Key::Escape);
        type_text(&mut h, "L");
        press(&mut h, egui::Key::Enter);
        assert!(h.state().session.has_active_tool(), "前提");
        press(&mut h, egui::Key::Tab);
        type_text(&mut h, "1");
        assert_eq!(h.state().session.cmdline.input(), "1", "動的入力 {on}");
    }
}

/// `↓` で候補を選んで Enter すると、選んだ候補が実行される。
///
/// 入力欄が `↑` `↓` を自分で受け取る（`event_filter` の `vertical_arrows`）ので、
/// それを外すと egui がフォーカス移動に使い、動的入力オンでは `↓` で入力欄から
/// フォーカスが外れて Enter が届かなくなる（`vertical_arrows: false` で落ちることを確認済み。
/// オフ側はこの変更では落ちないが、同じ操作を両方で固定しておく）。
#[test]
fn arrow_keys_pick_a_suggestion_in_both_modes() {
    for on in [false, true] {
        let mut h = app_with_dynamic(on);
        hover(&mut h, P1);
        type_text(&mut h, "L");
        assert!(h.state().session.cmdline.suggestions_visible(), "前提");
        press(&mut h, egui::Key::ArrowDown);
        press(&mut h, egui::Key::ArrowDown);
        press(&mut h, egui::Key::Enter);
        assert_eq!(input_lines(&h), vec!["> LAYER"], "動的入力 {on}");
        assert!(h.state().layer_panel.is_open(), "動的入力 {on}");
    }
}

// ---- パネルの入力欄（Issue #22） --------------------------------------------

/// レイヤパネルを開き、「新規」の入力欄をクリックしてフォーカスを移す。
fn focus_layer_name_field(h: &mut Harness<'_, CadApp>) {
    use egui_kittest::kittest::Queryable as _;

    if !h.state().layer_panel.is_open() {
        h.state_mut().layer_panel.toggle();
        settle(h);
    }
    let cmdline = input_rect(h);
    let target = h
        .query_all_by_role(egui::accesskit::Role::TextInput)
        .find(|n| n.rect() != cmdline)
        .expect("レイヤパネルの入力欄があるはず")
        .rect();
    let button = |pressed| egui::Event::PointerButton {
        pos: target.center(),
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    frame(
        h,
        [egui::Event::PointerMoved(target.center()), button(true)],
    );
    frame(h, [button(false)]);
    settle(h);
}

/// レイヤパネルの入力欄の中身。
fn layer_name_field_value(h: &Harness<'_, CadApp>) -> String {
    use egui_kittest::kittest::Queryable as _;

    let cmdline = input_rect(h);
    h.query_all_by_role(egui::accesskit::Role::TextInput)
        .find(|n| n.rect() != cmdline)
        .and_then(|n| n.value())
        .unwrap_or_default()
}

/// パネルの入力欄を編集している間、Enter / Space / Esc をコマンドラインが奪わない。
///
/// 奪うと、レイヤ名の Space や Enter でコマンドが進み（空 Enter なら直前のコマンドを
/// 再実行し）、Esc で実行中のコマンドが中断される。
#[test]
fn panel_text_field_keeps_enter_space_and_escape() {
    for on in [false, true] {
        let mut h = app_with_dynamic(on);
        hover(&mut h, P1);
        type_text(&mut h, "L");
        press(&mut h, egui::Key::Enter);
        assert!(h.state().session.has_active_tool(), "前提: LINE 実行中");
        let lines_before = h.state().session.cmdline.history().count();

        focus_layer_name_field(&mut h);
        type_text(&mut h, "A");
        let mut space = Vec::from(key(egui::Key::Space));
        space.insert(1, egui::Event::Text(" ".to_owned()));
        frame(&mut h, space);
        settle(&mut h);
        type_text(&mut h, "B");
        assert_eq!(layer_name_field_value(&h), "A B", "動的入力 {on}");
        assert_eq!(
            h.state().session.cmdline.input(),
            "",
            "コマンドラインには入らない"
        );

        // Esc はパネルの入力欄のフォーカスを外すだけ。egui がフレームの最初に
        // フォーカスを外すので、このフレームだけを見ると誰も持っていないように見える。
        press(&mut h, egui::Key::Escape);
        assert!(
            h.state().session.has_active_tool(),
            "Space / Esc で LINE が進んだり中断されたりしない（動的入力 {on}）"
        );

        // Enter はパネルの入力欄の確定。編集を終えるとフォーカスはコマンドラインへ戻る
        // （以降のキーはコマンドラインのもの）。
        focus_layer_name_field(&mut h);
        press(&mut h, egui::Key::Enter);
        assert!(
            h.state().session.has_active_tool(),
            "Enter で LINE が終わらない（動的入力 {on}）"
        );
        assert_eq!(
            h.state().session.cmdline.history().count(),
            lines_before,
            "履歴に何も増えない（動的入力 {on}）"
        );

        // パネルの編集を終えたら、キー入力はコマンドラインへ戻っている。
        type_text(&mut h, "10,10");
        press(&mut h, egui::Key::Enter);
        assert_eq!(
            h.state().session.last_point(),
            Some(cad_core::geom::Point2::new(10.0, 10.0)),
            "打った座標が LINE の始点に入る（動的入力 {on}）"
        );
    }
}

/// LINE 実行中にパネルの入力欄で日本語を変換しても、コマンドラインは変換中にならない。
///
/// IME のイベントはフォーカスの持ち主へ届くもので、コマンドラインのものとは限らない。
/// 持ち主を見ずに拾うと、カーソル横に `[変換中]` が出て位置が固定され、
/// キーの扱いや候補の更新も止まっていた（実機の不具合報告）。
#[test]
fn ime_in_a_panel_field_does_not_make_the_command_line_compose() {
    for on in [false, true] {
        let mut h = app_with_dynamic(on);
        hover(&mut h, P1);
        type_text(&mut h, "L");
        press(&mut h, egui::Key::Enter);
        assert!(h.state().session.has_active_tool(), "前提: LINE 実行中");

        focus_layer_name_field(&mut h);
        hover(&mut h, P1);
        frame(&mut h, [preedit("にほん")]);
        settle(&mut h);
        assert!(
            !h.state().session.cmdline.is_composing(),
            "パネルの変換でコマンドラインが変換中にならない（動的入力 {on}）"
        );
        if on {
            // カーソル横は固定されず、マウスに付いていく。
            let before = input_rect(&h);
            hover(&mut h, P2);
            assert_ne!(input_rect(&h), before, "マウスに追従する");
        }

        frame(
            &mut h,
            [egui::Event::Ime(egui::ImeEvent::Commit("日本".to_owned()))],
        );
        settle(&mut h);
        assert_eq!(
            layer_name_field_value(&h),
            "日本",
            "パネルの欄に入る（動的入力 {on}）"
        );
        assert_eq!(
            h.state().session.cmdline.input(),
            "",
            "コマンドラインには入らない"
        );

        // キャンバスへ戻って点を打つと LINE に入る。
        click(&mut h, P1);
        type_text(&mut h, "@10,0");
        press(&mut h, egui::Key::Enter);
        assert_eq!(
            lines(&h).len(),
            1,
            "クリックした始点から打った座標まで線が引かれる（動的入力 {on}）"
        );
    }
}

/// コマンドラインで変換中のままパネルへフォーカスを移しても、変換中が残らない。
///
/// 変換の確定・取り消しはパネル側へ届くので、残るとキーを奪わない・候補を
/// 作り直さない・カーソル横が固定されたままになる。
#[test]
fn moving_focus_to_a_panel_ends_the_command_line_composition() {
    let mut h = app_with_dynamic(true);
    hover(&mut h, P1);
    frame(&mut h, [preedit("に")]);
    settle(&mut h);
    assert!(h.state().session.cmdline.is_composing(), "前提: 変換中");

    focus_layer_name_field(&mut h);
    assert!(
        !h.state().session.cmdline.is_composing(),
        "フォーカスが移ったら変換中ではない"
    );
}

// ---- ステータスバー -----------------------------------------------------------

/// ステータスバーの `DYN` に乗せると指のカーソルになる（文字選択の I ビームにならない）。
///
/// ラベルは既定で文字を選べるので、クリックで切り替える部品なのに I ビームが出ていた
/// （ユーザーの実機確認）。
#[test]
fn hovering_dyn_in_the_status_bar_shows_a_pointing_hand() {
    use egui_kittest::kittest::Queryable as _;

    let mut h = app_with_dynamic(true);
    let target = h.get_by_label("DYN").rect().center();
    // カーソルの形は毎フレーム決め直されるので、落ち着いた後の最後のフレームの出力を見る。
    hover(&mut h, target);
    assert_eq!(
        h.output().platform_output.cursor_icon,
        egui::CursorIcon::PointingHand
    );

    // 選択を切ってもクリックで切り替わる。
    click(&mut h, target);
    assert!(
        !h.state().session.cmdline.is_dynamic(),
        "クリックでオフになる"
    );
}

#[test]
fn osnap_in_the_status_bar_is_clickable_with_a_pointing_hand() {
    use egui_kittest::kittest::Queryable as _;

    let mut h = app_with_dynamic(true);
    assert!(h.state().snap.is_enabled(), "前提: 起動時は OSNAP オン");
    let target = h.get_by_label("OSNAP").rect().center();
    hover(&mut h, target);
    assert_eq!(
        h.output().platform_output.cursor_icon,
        egui::CursorIcon::PointingHand
    );

    // クリックで切り替わり、もう一度で戻る。F3 と同じく履歴にも残る。
    click(&mut h, target);
    assert!(!h.state().snap.is_enabled(), "クリックでオフになる");
    let target = h.get_by_label("osnap").rect().center();
    click(&mut h, target);
    assert!(h.state().snap.is_enabled(), "もう一度クリックでオンに戻る");
    let toggled = h
        .state()
        .session
        .cmdline
        .history()
        .filter(|l| l.text.starts_with("オブジェクトスナップ:"))
        .count();
    assert_eq!(toggled, 2);
}

// ---- 幅が狭いときのステータスバー（Issue #38） --------------------------------

/// 切り替え部品の状態を読む関数。
type ToggleState = fn(&CadApp) -> bool;

/// 幅を指定したアプリ。文字の幅を実機に合わせるため、日本語フォントを読み込む。
/// 読み込まないと漢字が代替の □ になってステータスバーの幅が実機と変わり、境目の幅で結果が違う
/// （元の並びの不具合は、640px ならフォント無しでも再現するが、800px ではフォント無しだと
/// DYN が画面内に収まって再現しなかった）。さらにフォントが無いと「日本語フォント未検出」が
/// 並びに入り、1280px でも描画時間が省かれる。
///
/// **フォントが見つからなければ理由つきで落とす**（fail-closed）。黙ってフォント無しで進めると、
/// 検出力の落ちたテストが通ってしまう。CI では `fonts-noto-cjk` を入れている。
fn app_with_width(width: f32) -> Harness<'static, CadApp> {
    let mut h = Harness::builder()
        .with_size(egui::vec2(width, SCREEN.y))
        .build_eframe(|cc| {
            let font = crate::jp_font::install(&cc.egui_ctx).expect(
                "日本語フォントが見つからない（jp_font::CANDIDATES のどれも無い）。\
                 Ubuntu なら `sudo apt-get install fonts-noto-cjk` で入る",
            );
            CadApp::new(Some(format!(
                "{} (face {})",
                font.path.display(),
                font.index
            )))
        });
    h.run_steps(SETTLE);
    h
}

/// 幅 800px・640px でも、切り替え部品（OSNAP / ORTHO / POLAR / DYN）がすべて画面内にあり、
/// クリックで切り替わる。PR #35 で ORTHO / POLAR が増え、800px で DYN が画面外になっていた。
#[test]
fn status_toggles_fit_and_click_in_a_narrow_window() {
    use egui_kittest::kittest::Queryable as _;

    for width in [800.0, 640.0] {
        let mut h = app_with_width(width);
        // (表示中のラベル, 切り替え後のラベル, 状態を読む関数)
        let toggles: [(&str, &str, ToggleState); 4] = [
            ("OSNAP", "osnap", |a| a.snap.is_enabled()),
            ("ortho", "ORTHO", |a| {
                a.drafting.is_on(crate::drafting::Mode::Ortho)
            }),
            ("polar", "POLAR", |a| {
                a.drafting.is_on(crate::drafting::Mode::Polar)
            }),
            ("DYN", "dyn", |a| a.session.cmdline.is_dynamic()),
        ];
        for (label, after, state) in toggles {
            let rect = h.get_by_label(label).rect();
            assert!(
                rect.min.x >= 0.0 && rect.max.x <= width,
                "幅 {width}: {label} が画面内にある ({rect:?})"
            );
            let before = state(h.state());
            click(&mut h, rect.center());
            assert_ne!(
                state(h.state()),
                before,
                "幅 {width}: {label} のクリックで切り替わる"
            );
            assert!(
                h.query_by_label(after).is_some(),
                "幅 {width}: {label} → {after}"
            );
        }
        // 描画時間は入り切らなければ省く。出すなら途中で切らない。
        if let Some(node) = h.query_by_label_contains("描画 平均") {
            assert!(
                node.rect().max.x <= width,
                "幅 {width}: 描画時間が途中で切れない"
            );
        }
    }
}

/// 情報表示の項目（この順に並ぶ）。ラベルに含まれる文字列。
const INFO_ITEMS: [&str; 5] = ["レイヤ 0", "選択 0", "要素 0", "倍率 ", "描画 平均"];

/// 画面に出ている情報表示の項目の (頭の文字列, 矩形)。
fn shown_info_items(h: &Harness<'_, CadApp>) -> Vec<(&'static str, egui::Rect)> {
    use egui_kittest::kittest::Queryable as _;
    INFO_ITEMS
        .into_iter()
        .filter_map(|prefix| {
            h.query_by_label_contains(prefix)
                .map(|n| (prefix, n.rect()))
        })
        .collect()
}

/// 情報表示は文字の途中で切れない。入らない項目は右から順に省かれ、
/// レイヤ・選択が最後まで残る。1280px ではすべて出る（PR #39）。
#[test]
fn status_info_items_are_dropped_whole_from_the_right() {
    for width in [1280.0, 800.0, 640.0] {
        let h = app_with_width(width);
        let shown = shown_info_items(&h);
        let names: Vec<&str> = shown.iter().map(|(p, _)| *p).collect();
        assert_eq!(
            names,
            INFO_ITEMS[..names.len()].to_vec(),
            "幅 {width}: 前から順に残り、途中が抜けない"
        );
        for (prefix, rect) in &shown {
            assert!(
                rect.max.x <= width,
                "幅 {width}: {prefix} が途中で切れない ({rect:?})"
            );
        }
        for pair in shown.windows(2) {
            assert!(pair[0].1.max.x < pair[1].1.min.x, "幅 {width}: 並び順");
        }
        if width >= SCREEN.x {
            assert_eq!(names.len(), INFO_ITEMS.len(), "1280px ではすべて出る");
        } else if width >= 800.0 {
            assert!(
                names.starts_with(&["レイヤ 0", "選択 0"]),
                "幅 {width}: レイヤと選択は残る: {names:?}"
            );
        }
    }
}

/// 座標の欄は大きい座標で広がり、小さい座標に戻っても縮まない（後ろの部品が跳ねない）。
/// 図面を入れ替えたら最小の幅に戻る（PR #39 の操作レビュー）。
#[test]
fn coordinate_field_does_not_shrink_back() {
    use egui_kittest::kittest::Queryable as _;

    let mut h = app_with_width(SCREEN.x);
    let ortho_x = |h: &Harness<'_, CadApp>| h.get_by_label("ortho").rect().min.x;
    hover(&mut h, P1);
    let small = ortho_x(&h);

    // 2000 万付近を映して指す（13 文字以上の座標）。
    let far = cad_core::geom::Point2::new(2.0e7, 2.0e7);
    h.state_mut().viewport.zoom_to_fit(
        cad_core::geom::Aabb::new(far, far + cad_core::geom::Vec2::new(420.0, 297.0)),
        0.05,
    );
    hover(&mut h, P2);
    let big = ortho_x(&h);
    assert!(big > small, "大きい座標で欄が広がる: {small} → {big}");

    // 原点付近へ戻しても縮まない。
    h.state_mut().viewport.zoom_to_fit(
        cad_core::geom::Aabb::new(Point2::ORIGIN, Point2::new(420.0, 297.0)),
        0.05,
    );
    hover(&mut h, P1);
    assert_eq!(ortho_x(&h), big, "小さい座標に戻っても位置が動かない");

    // 図面を入れ替えたら（新規・開く）最小の幅に戻る。
    h.state_mut()
        .report_file_outcome(crate::file_ops::FileOutcome::Replaced(
            "新規図面を作成しました".to_owned(),
        ));
    hover(&mut h, P2);
    assert_eq!(ortho_x(&h), small, "図面を入れ替えたら戻る");
}

/// 「コマンド実行中」が出ても、スナップの吸着で `OSNAP:端点` になっても、
/// 切り替え部品と情報表示の位置（押す位置）は動かない（PR #39）。
#[test]
fn status_bar_items_do_not_move_with_the_state() {
    use egui_kittest::kittest::Queryable as _;

    let mut h = app_with_width(SCREEN.x);
    let xs = |h: &Harness<'_, CadApp>| -> Vec<f32> {
        ["ortho", "polar", "DYN", "レイヤ 0"]
            .into_iter()
            .map(|l| h.get_by_label(l).rect().min.x)
            .chain(std::iter::once(
                h.query_by_label_contains("OSNAP")
                    .expect("OSNAP")
                    .rect()
                    .min
                    .x,
            ))
            .collect()
    };
    let idle = xs(&h);

    // 線分を 1 本引き、LINE 実行中にその端点へ吸着させる。
    let a = egui::pos2(300.0, 300.0);
    let b = egui::pos2(500.0, 360.0);
    hover(&mut h, a);
    type_text(&mut h, "L");
    press(&mut h, egui::Key::Enter);
    click(&mut h, a);
    click(&mut h, b);
    press(&mut h, egui::Key::Escape);
    type_text(&mut h, "L");
    press(&mut h, egui::Key::Enter);
    assert!(h.state().session.has_active_tool(), "前提: コマンド実行中");
    let end = lines(&h)[0].b;
    let near = h.state().viewport.model_to_screen(end) + egui::vec2(3.0, 3.0);
    hover(&mut h, near);
    assert!(
        h.query_by_label("OSNAP:端点").is_some(),
        "前提: 端点に吸着して表示が伸びている"
    );
    assert!(h.query_by_label("コマンド実行中").is_some(), "前提");
    assert_eq!(
        xs(&h),
        idle,
        "ortho / polar / DYN / レイヤ / OSNAP の位置が動かない"
    );
}

// ---- 保存しても状態を変えない（Issue #41） ------------------------------------

/// 修飾キーつきでキーを押して離す 2 イベント。
fn key_with(k: egui::Key, modifiers: egui::Modifiers) -> [egui::Event; 2] {
    [true, false].map(|pressed| egui::Event::Key {
        key: k,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers,
    })
}

/// 線分を 1 本引いてクリックで選び、座標の欄を広げた状態（大きい座標を映して指した後、原点付近へ戻す）。
fn drawing_with_a_selection_and_a_wide_coordinate_field() -> Harness<'static, CadApp> {
    let mut h = app();
    let a = egui::pos2(300.0, 300.0);
    let b = egui::pos2(500.0, 360.0);
    hover(&mut h, a);
    type_text(&mut h, "L");
    press(&mut h, egui::Key::Enter);
    click(&mut h, a);
    click(&mut h, b);
    press(&mut h, egui::Key::Escape);
    let l = lines(&h)[0];
    let mid = h.state().viewport.model_to_screen(l.a.lerp(l.b, 0.5));
    click(&mut h, mid);
    assert_eq!(h.state().session.selection.len(), 1, "前提: 線分を選んだ");

    let far = Point2::new(2.0e7, 2.0e7);
    h.state_mut().viewport.zoom_to_fit(
        cad_core::geom::Aabb::new(far, far + cad_core::geom::Vec2::new(420.0, 297.0)),
        0.05,
    );
    hover(&mut h, P2);
    h.state_mut().viewport.zoom_to_fit(
        cad_core::geom::Aabb::new(Point2::ORIGIN, Point2::new(420.0, 297.0)),
        0.05,
    );
    hover(&mut h, P2);
    assert!(
        h.state().coord_width > super::COORD_MIN_WIDTH,
        "前提: 座標の欄が広がっている"
    );
    h
}

/// 保存（Ctrl+S、保存先がある図面）しても、選択は外れず、座標の欄の幅も変わらない。
///
/// 以前は保存の成功を図面の入れ替えと同じに扱い、保存のたびに選択が外れ、
/// 座標の欄が最小の幅に戻って切り替え部品が跳ねていた。
#[test]
fn saving_keeps_the_selection_and_the_coordinate_field() {
    let dir = crate::test_util::TempDir::new("issue41");
    let path = dir.join("drawing.ymc");

    let mut h = drawing_with_a_selection_and_a_wide_coordinate_field();
    // 保存先がある図面にしておく（Ctrl+S でファイルダイアログを開かずに保存される）。
    h.state_mut().doc.mark_saved(Some(path.clone()));
    let width = h.state().coord_width;

    frame(&mut h, key_with(egui::Key::S, egui::Modifiers::CTRL));
    settle(&mut h);

    assert!(path.is_file(), "保存された");
    assert!(
        h.state()
            .session
            .cmdline
            .history()
            .any(|l| l.text.starts_with("保存しました")),
        "保存の案内が出る"
    );
    assert_eq!(h.state().session.selection.len(), 1, "選択は外れない");
    assert_eq!(h.state().coord_width, width, "座標の欄の幅は変わらない");
}

/// 履歴に出た「*取り消し*」（実行中のコマンドの中断）の数。
fn cancel_count(h: &Harness<'_, CadApp>) -> usize {
    h.state()
        .session
        .cmdline
        .history()
        .filter(|l| l.text == "*取り消し*")
        .count()
}

/// MOVE の基点待ちでクイックアクセスの保存を押しても、MOVE・選択は残る（`Ctrl+S` とそろえる）。
#[test]
fn quick_access_save_keeps_a_move_waiting_for_its_base_point() {
    let dir = crate::test_util::TempDir::new("save_move");
    let path = dir.join("drawing.ymc");
    let mut h = app();
    let a = egui::pos2(300.0, 300.0);
    let b = egui::pos2(500.0, 360.0);
    hover(&mut h, a);
    type_text(&mut h, "L");
    press(&mut h, egui::Key::Enter);
    click(&mut h, a);
    click(&mut h, b);
    press(&mut h, egui::Key::Escape);
    let l = lines(&h)[0];
    let mid = h.state().viewport.model_to_screen(l.a.lerp(l.b, 0.5));
    click(&mut h, mid);
    type_text(&mut h, "M");
    press(&mut h, egui::Key::Enter);
    assert_eq!(h.state().session.active_command(), Some("MOVE"), "前提");
    assert!(h.state().session.wants_point(), "前提: 基点待ち");
    assert_eq!(h.state().session.selection.len(), 1, "前提: 選択あり");
    h.state_mut().doc.mark_saved(Some(path.clone()));
    let cancels = cancel_count(&h);

    press_ribbon(&mut h, "SAVE");

    assert!(path.is_file(), "保存された");
    assert_eq!(
        h.state().session.active_command(),
        Some("MOVE"),
        "MOVE は続く"
    );
    assert_eq!(h.state().session.selection.len(), 1, "選択は外れない");
    assert_eq!(cancel_count(&h), cancels, "中断しない");

    // そのまま基点と目的点を指せば動く。
    let before = lines(&h)[0];
    click(&mut h, P1);
    click(&mut h, P2);
    assert_ne!(lines(&h)[0], before, "MOVE が最後まで動く");
}

/// POLYLINE の途中でクイックアクセスの保存を押しても、置いた点は残り、続けて描ける。
#[test]
fn quick_access_save_keeps_a_polyline_in_progress() {
    let dir = crate::test_util::TempDir::new("save_pline");
    let path = dir.join("drawing.ymc");
    let mut h = app();
    hover(&mut h, P1);
    h.state_mut().doc.mark_saved(Some(path.clone()));
    type_text(&mut h, "PL");
    press(&mut h, egui::Key::Enter);
    click(&mut h, egui::pos2(300.0, 300.0));
    click(&mut h, egui::pos2(500.0, 300.0));
    let last = h.state().session.last_point().expect("前提: 2 点置いた");

    let cancels = cancel_count(&h);
    press_ribbon(&mut h, "SAVE");

    assert!(path.is_file(), "保存された");
    assert_eq!(h.state().session.active_command(), Some("POLYLINE"), "続く");
    assert_eq!(h.state().session.last_point(), Some(last), "置いた点は残る");
    assert_eq!(cancel_count(&h), cancels, "中断しない");

    click(&mut h, egui::pos2(500.0, 450.0));
    press(&mut h, egui::Key::Enter);
    let polylines: Vec<usize> = h
        .state()
        .doc
        .entities()
        .iter()
        .filter_map(|(_, e)| match &e.geom {
            cad_core::Geometry::Polyline(p) => Some(p.vertices.len()),
            _ => None,
        })
        .collect();
    assert_eq!(polylines, vec![3], "3 点のポリラインが描ける");
}

/// 変更がある図面で新規作成 → 未保存の確認 → 「保存する」で、保存してから新規図面になる。
///
/// 並行して確認のキー操作が変わる（PR #33）ので、ボタンはクリックで押す。
#[test]
fn new_with_unsaved_changes_saves_through_the_confirmation_then_replaces() {
    use egui_kittest::kittest::Queryable as _;

    let dir = crate::test_util::TempDir::new("confirm_save");
    let path = dir.join("drawing.ymc");
    let mut h = app();
    // 保存先がある図面に変更を加える（確認の「保存する」でダイアログを開かずに保存される）。
    h.state_mut().doc.mark_saved(Some(path.clone()));
    let a = egui::pos2(300.0, 300.0);
    hover(&mut h, a);
    type_text(&mut h, "L");
    press(&mut h, egui::Key::Enter);
    click(&mut h, a);
    click(&mut h, egui::pos2(500.0, 360.0));
    press(&mut h, egui::Key::Escape);
    let l = lines(&h)[0];
    let mid = h.state().viewport.model_to_screen(l.a.lerp(l.b, 0.5));
    click(&mut h, mid);
    assert!(h.state().doc.is_dirty(), "前提: 未保存の変更がある");
    assert_eq!(h.state().session.selection.len(), 1, "前提: 選択あり");

    frame(&mut h, key_with(egui::Key::N, egui::Modifiers::CTRL));
    settle(&mut h);
    assert!(h.state().files.is_confirming(), "未保存の確認が出る");
    assert!(!path.exists(), "まだ保存していない");

    let button = h.get_by_label_contains("保存する").rect().center();
    click(&mut h, button);

    assert!(!h.state().files.is_confirming(), "確認は閉じる");
    let saved = cad_core::native::read::read_from_file(&path).expect("保存された図面が読める");
    assert_eq!(saved.entities().len(), 1, "変更ごと保存された");
    assert!(h.state().doc.entities().is_empty(), "新規図面になった");
    assert_eq!(
        h.state().session.selection.len(),
        0,
        "入れ替えたので選択は外れる"
    );
    assert!(
        h.state()
            .session
            .cmdline
            .history()
            .any(|l| l.text == "新規図面を作成しました"),
        "新規作成の案内が出る"
    );
}

/// 図面の入れ替え（Ctrl+N）では、従来どおり選択を外し、座標の欄も最小の幅に戻す。
#[test]
fn replacing_the_drawing_clears_the_selection_and_the_coordinate_field() {
    let mut h = drawing_with_a_selection_and_a_wide_coordinate_field();
    // 変更が無い図面にして、未保存の確認を挟まずに新規作成させる。
    h.state_mut().doc.mark_saved(None);

    frame(&mut h, key_with(egui::Key::N, egui::Modifiers::CTRL));
    settle(&mut h);

    assert!(
        h.state().doc.entities().is_empty(),
        "前提: 新しい図面になった"
    );
    assert_eq!(h.state().session.selection.len(), 0, "選択は外れる");
    assert_eq!(
        h.state().coord_width,
        super::COORD_MIN_WIDTH,
        "座標の欄は最小の幅に戻る"
    );
}

/// ステータスバーの情報表示（座標・レイヤ・選択・要素・倍率・描画時間）に乗せても I ビームにならない。
#[test]
fn status_info_labels_do_not_show_a_text_cursor() {
    use egui_kittest::kittest::Queryable as _;

    // 描画時間まで出る幅とフォントで（フォントが無いと「日本語フォント未検出」で描画時間が押し出される）。
    let mut h = app_with_width(SCREEN.x);
    for text in ["  Y ", "レイヤ 0", "選択 0", "要素 0", "倍率 ", "描画 平均"] {
        let target = h
            .query_by_label_contains(text)
            .unwrap_or_else(|| panic!("{text} が出ている"))
            .rect()
            .center();
        hover(&mut h, target);
        assert_ne!(
            h.output().platform_output.cursor_icon,
            egui::CursorIcon::Text,
            "{text} に乗せても I ビームにならない"
        );
    }
}

// ---- 寸法入力（Issue #20 段階 B） -------------------------------------------

use crate::cmdline::dimension::{DimValues, Field};
use cad_core::geom::tolerance::eq_len;
use cad_core::geom::{Line, Point2};

fn model(h: &Harness<'_, CadApp>, pos: egui::Pos2) -> Point2 {
    h.state().viewport.screen_to_model(pos)
}

fn lines(h: &Harness<'_, CadApp>) -> Vec<Line> {
    h.state()
        .doc
        .entities()
        .iter()
        .filter_map(|(_, e)| match &e.geom {
            cad_core::Geometry::Line(l) => Some(*l),
            _ => None,
        })
        .collect()
}

fn assert_point(actual: Point2, expected: Point2, what: &str) {
    assert!(
        eq_len(actual.x, expected.x) && eq_len(actual.y, expected.y),
        "{what}: {actual:?} != {expected:?}"
    );
}

/// LINE を始めて P1 に 1 点目を置き、カーソルを P2 へ動かした状態。基点（モデル座標）を返す。
fn line_with_first_point(on: bool) -> (Harness<'static, CadApp>, Point2) {
    let mut h = app_with_dynamic(on);
    hover(&mut h, P1);
    type_text(&mut h, "L");
    press(&mut h, egui::Key::Enter);
    click(&mut h, P1);
    hover(&mut h, P2);
    let base = model(&h, P1);
    assert_eq!(h.state().session.dimension_base(), Some(base), "前提: 基点");
    (h, base)
}

/// `100` → Tab → `90` → Enter で、基点の真上に長さ 100 の線が引かれる。
/// 点が入ったら固定は外れ、長さの欄へ戻る。
#[test]
fn length_tab_angle_enter_draws_the_given_segment() {
    let (mut h, base) = line_with_first_point(true);
    type_text(&mut h, "100");
    assert_eq!(h.state().session.cmdline.dimension_field(), Field::Length);
    press(&mut h, egui::Key::Tab);
    assert_eq!(
        h.state().session.cmdline.dimension_field(),
        Field::Angle,
        "Tab で角度の欄へ"
    );
    assert_eq!(
        h.state().session.cmdline.input(),
        "",
        "固定した値は欄へ移る"
    );
    assert_eq!(
        h.state().session.cmdline.dimension_locks(),
        Some(DimValues {
            length: Some(100.0),
            angle_deg: None
        })
    );
    type_text(&mut h, "90");
    assert_eq!(h.state().session.cmdline.input(), "90", "Tab の後も打てる");
    press(&mut h, egui::Key::Enter);

    let l = lines(&h);
    assert_eq!(l.len(), 1, "線分が 1 本");
    assert_point(l[0].a, base, "始点");
    assert_point(
        l[0].b,
        Point2::new(base.x, base.y + 100.0),
        "終点は真上 100",
    );
    assert!(h.state().session.has_active_tool(), "LINE は続く");
    assert_eq!(
        h.state().session.cmdline.dimension_locks(),
        None,
        "次の点へ進んだら固定は外れる"
    );
    assert_eq!(h.state().session.cmdline.dimension_field(), Field::Length);
}

/// 長さだけ固定すると、ラバーバンドの先もクリックした点も、カーソルの向きに固定の長さ。
#[test]
fn a_length_lock_applies_to_the_rubber_band_and_the_click() {
    let (mut h, base) = line_with_first_point(true);
    type_text(&mut h, "100");
    press(&mut h, egui::Key::Tab);
    hover(&mut h, P2);

    let toward = model(&h, P2) - base;
    let expected = base + toward.normalized().expect("前提: P1 と P2 は離れている") * 100.0;
    let cursor = h.state().cursor_model.expect("カーソルはキャンバスの上");
    assert_point(cursor, expected, "ラバーバンドの先");

    click(&mut h, P2);
    let l = lines(&h);
    assert_eq!(l.len(), 1);
    assert_point(l[0].b, expected, "クリックで入った点");
}

/// 角度を固定すると、クリックした点はその角度の半直線へ射影される。
#[test]
fn an_angle_lock_projects_the_click_onto_the_ray() {
    let (mut h, base) = line_with_first_point(true);
    press(&mut h, egui::Key::Tab); // 空の Tab は固定せずに角度の欄へ
    assert_eq!(h.state().session.cmdline.dimension_field(), Field::Angle);
    assert_eq!(h.state().session.cmdline.dimension_locks(), None);
    type_text(&mut h, "0");
    press(&mut h, egui::Key::Tab);
    hover(&mut h, P2);
    let cursor = h.state().cursor_model.expect("カーソルはキャンバスの上");
    assert_point(
        cursor,
        Point2::new(model(&h, P2).x, base.y),
        "ラバーバンドは水平",
    );

    click(&mut h, P2);
    let l = lines(&h);
    assert_eq!(l.len(), 1);
    assert_point(l[0].b, Point2::new(model(&h, P2).x, base.y), "0° の線上");
}

/// 固定していても、座標を打てば座標として入る（欄の表示をやめて従来どおり）。
#[test]
fn coordinates_override_the_fields() {
    let (mut h, base) = line_with_first_point(true);
    type_text(&mut h, "100");
    press(&mut h, egui::Key::Tab);
    type_text(&mut h, "@0,50");
    assert_eq!(
        h.state().session.cmdline.dimension_locks(),
        None,
        "数値以外を打っている間は固定を効かせない"
    );
    press(&mut h, egui::Key::Enter);
    let l = lines(&h);
    assert_eq!(l.len(), 1);
    assert_point(l[0].b, Point2::new(base.x, base.y + 50.0), "相対座標");
}

/// Esc の 1 回目は固定と入力の解除、2 回目で中断。
#[test]
fn escape_releases_the_locks_before_cancelling() {
    let (mut h, _) = line_with_first_point(true);
    type_text(&mut h, "100");
    press(&mut h, egui::Key::Tab);
    type_text(&mut h, "3");
    assert!(
        h.state().session.cmdline.dimension_locks().is_some(),
        "前提"
    );

    press(&mut h, egui::Key::Escape);
    assert_eq!(h.state().session.cmdline.dimension_locks(), None, "解除");
    assert_eq!(h.state().session.cmdline.input(), "", "入力も消える");
    assert_eq!(h.state().session.cmdline.dimension_field(), Field::Length);
    assert!(h.state().session.has_active_tool(), "まだ中断しない");

    press(&mut h, egui::Key::Escape);
    assert!(!h.state().session.has_active_tool(), "2 回目で中断");
    assert!(lines(&h).is_empty());
}

/// 動的入力オフでも直接距離入力が効く（欄は出ない）。オンで Tab を使わずに
/// 長さだけ打って Enter しても同じ結果になる。
#[test]
fn direct_distance_works_in_both_modes() {
    for on in [false, true] {
        let (mut h, base) = line_with_first_point(on);
        type_text(&mut h, "100");
        // 向きは確定した時点のカーソルから決まる。画面下の履歴が伸びると作図領域が
        // 縮んで P2 のモデル座標が変わるので、確定前に取っておく。
        let cursor = h.state().cursor_model.expect("カーソルはキャンバスの上");
        press(&mut h, egui::Key::Enter);
        let toward = cursor - base;
        let expected = base + toward.normalized().expect("前提") * 100.0;
        let l = lines(&h);
        assert_eq!(l.len(), 1, "動的入力 {on}");
        assert_point(l[0].b, expected, &format!("動的入力 {on}"));
    }
}

/// 候補が出ているときの Tab は補完、寸法入力中の Tab は欄の巡回。
#[test]
fn tab_completes_suggestions_but_cycles_dimension_fields() {
    let mut h = app_with_dynamic(true);
    hover(&mut h, P1);
    type_text(&mut h, "L");
    press(&mut h, egui::Key::Tab);
    assert_eq!(h.state().session.cmdline.input(), "LINE", "補完");
    press(&mut h, egui::Key::Enter);
    click(&mut h, P1);
    hover(&mut h, P2);

    type_text(&mut h, "100");
    press(&mut h, egui::Key::Tab);
    assert_eq!(h.state().session.cmdline.input(), "", "補完ではなく固定");
    assert_eq!(h.state().session.cmdline.dimension_field(), Field::Angle);
    press(&mut h, egui::Key::Tab);
    assert_eq!(
        h.state().session.cmdline.dimension_field(),
        Field::Length,
        "巡回"
    );
    type_text(&mut h, "7");
    assert_eq!(h.state().session.cmdline.input(), "7", "フォーカスは残る");
}

/// 角度の欄で変換が始まっても、入力欄が 1px も動かない。
///
/// 変換中はバッファを分類できない（ADR-0002）。分類をやめて通常の見た目に戻すと、
/// 入力欄が角度の欄から左端へ跳び、候補ウィンドウも跳ねる。
#[test]
fn input_does_not_move_when_composition_starts_in_a_dimension_field() {
    let (mut h, _) = line_with_first_point(true);
    press(&mut h, egui::Key::Tab);
    let before = input_rect(&h);

    frame(&mut h, [preedit("に")]);
    assert_eq!(input_rect(&h), before, "変換開始のフレーム");
    settle(&mut h);
    assert_eq!(input_rect(&h), before, "変換中");
}

/// COPY は基点が変わらないまま次の目的点へ進む。点が入ったら固定は外れる
/// （基点の変化だけを見ていると、前の複写で固定した長さが次の複写に残る）。
#[test]
fn copy_releases_the_locks_after_each_copy() {
    let mut h = app_with_dynamic(true);
    hover(&mut h, P1);
    for text in ["L", "0,0", "10,0"] {
        type_text(&mut h, text);
        press(&mut h, egui::Key::Enter);
    }
    press(&mut h, egui::Key::Enter); // LINE を終える
    let id = h.state().doc.entities().ids().next().expect("前提: 線分");
    h.state_mut().session.selection.insert(id);

    type_text(&mut h, "CO");
    press(&mut h, egui::Key::Enter);
    click(&mut h, P1);
    assert!(h.state().session.dimension_base().is_some(), "前提: 基点");
    type_text(&mut h, "100");
    press(&mut h, egui::Key::Tab);
    click(&mut h, P2);
    assert_eq!(lines(&h).len(), 2, "1 つ複写された");
    assert!(h.state().session.has_active_tool(), "COPY は続く");
    assert_eq!(
        h.state().session.cmdline.dimension_locks(),
        None,
        "複写したら固定は外れる"
    );
}

/// 角度だけ固定してその反対側をクリックすると、点は入らずエラーが出る
/// （Enter と同じ扱い）。ラバーバンドは基点に縮む。同じ側なら入る。
#[test]
fn a_click_behind_an_angle_lock_is_refused() {
    let (mut h, base) = line_with_first_point(true);
    press(&mut h, egui::Key::Tab);
    type_text(&mut h, "0");
    press(&mut h, egui::Key::Tab); // 0°（右向き）に固定

    // 基点の左側（反対側）。
    let behind = egui::pos2(P1.x - 150.0, P1.y + 40.0);
    hover(&mut h, behind);
    assert_eq!(
        h.state().cursor_model,
        Some(base),
        "ラバーバンドは基点に縮む"
    );
    let errors_before = h
        .state()
        .session
        .cmdline
        .history()
        .filter(|l| l.kind == LineKind::Error)
        .count();
    click(&mut h, behind);
    assert!(lines(&h).is_empty(), "点は入らない");
    assert!(h.state().session.has_active_tool(), "LINE は続く");
    let errors: Vec<String> = h
        .state()
        .session
        .cmdline
        .history()
        .filter(|l| l.kind == LineKind::Error)
        .map(|l| l.text.clone())
        .collect();
    assert_eq!(errors.len(), errors_before + 1, "エラー行が 1 つ出る");
    assert!(
        errors.last().is_some_and(|e| e.contains("反対側")),
        "{errors:?}"
    );
    assert!(
        h.state().session.cmdline.dimension_locks().is_some(),
        "固定は残る（打ち直さずに同じ側でクリックできる）"
    );

    // 同じ側（右）なら入る。
    click(&mut h, P2);
    let l = lines(&h);
    assert_eq!(l.len(), 1, "同じ側なら入る");
    assert_point(l[0].b, Point2::new(model(&h, P2).x, base.y), "0° の線上");
}

use crate::cmdline::TAB_NEEDS_DYNAMIC;

/// 動的入力オフで参加中のツールの Tab は、入力を変えずに案内を 1 回出す。
///
/// 欄はオンでしか出ないので、オフで AutoCAD の感覚で `100` Tab `90` と打つと
/// `10090` の直接距離入力になっていた（PR #25 レビュー）。
#[test]
fn tab_with_dynamic_input_off_explains_instead_of_locking() {
    let (mut h, base) = line_with_first_point(false);
    let guides = |h: &Harness<'_, CadApp>| {
        h.state()
            .session
            .cmdline
            .history()
            .filter(|l| l.text == TAB_NEEDS_DYNAMIC)
            .count()
    };
    type_text(&mut h, "100");
    press(&mut h, egui::Key::Tab);
    assert_eq!(h.state().session.cmdline.input(), "100", "入力は変わらない");
    assert_eq!(guides(&h), 1, "案内が 1 回出る");
    assert_eq!(
        h.state().session.cmdline.dimension_locks(),
        None,
        "固定しない"
    );

    // 入力はそのまま使える（長さ 100 の直接距離入力）。
    let cursor = h.state().cursor_model.expect("カーソルはキャンバスの上");
    press(&mut h, egui::Key::Enter);
    let l = lines(&h);
    assert_eq!(l.len(), 1);
    assert_point(
        l[0].b,
        base + (cursor - base).normalized().expect("前提") * 100.0,
        "長さ 100",
    );

    // 参加していない段階（LINE の 1 点目）では案内しない。
    press(&mut h, egui::Key::Escape);
    type_text(&mut h, "L");
    press(&mut h, egui::Key::Enter);
    press(&mut h, egui::Key::Tab);
    assert_eq!(guides(&h), 1, "参加していなければ案内しない");
}

// ---- 直交モード・極トラッキング（Issue #29、ADR-0038） ------------------------

use crate::drafting::Mode;

/// 基準点から画面上で `deg`°（モデルの向き。画面の y は下向きなので符号を返す）、`r` px の位置。
fn screen_at(h: &Harness<'_, CadApp>, base: Point2, deg: f32, r: f32) -> egui::Pos2 {
    let from = h.state().viewport.model_to_screen(base);
    let (s, c) = deg.to_radians().sin_cos();
    egui::pos2(from.x + r * c, from.y - r * s)
}

/// 補助を `keys` で切り替えてから LINE を始め、P1 に 1 点目を置く。基準点を返す。
fn line_from_p1_with(keys: &[egui::Key]) -> (Harness<'static, CadApp>, Point2) {
    let mut h = app_with_dynamic(true);
    hover(&mut h, P1);
    for k in keys {
        press(&mut h, *k);
    }
    type_text(&mut h, "L");
    press(&mut h, egui::Key::Enter);
    click(&mut h, P1);
    let base = h
        .state()
        .session
        .tracking_base()
        .expect("前提: 1 点目を置いたので基準点がある");
    (h, base)
}

fn cursor(h: &Harness<'_, CadApp>) -> Point2 {
    h.state().cursor_model.expect("カーソルはキャンバスの上")
}

/// F8 → LINE の 2 点目を斜めに指すと、ラバーバンドもクリックで入る点も水平・垂直になる。
#[test]
fn f8_makes_the_rubber_band_and_the_click_horizontal_or_vertical() {
    let (mut h, base) = line_from_p1_with(&[egui::Key::F8]);
    assert!(h.state().drafting.is_on(Mode::Ortho), "F8 でオン");

    // 20° の向き（水平寄り）→ 水平。
    let pos = screen_at(&h, base, 20.0, 200.0);
    hover(&mut h, pos);
    let expected = Point2::new(model(&h, pos).x, base.y);
    assert_point(cursor(&h), expected, "ラバーバンドの先は水平");
    click(&mut h, pos);
    let l = lines(&h);
    assert_eq!(l.len(), 1);
    assert_point(
        l[0].b,
        expected,
        "クリックで入った点もラバーバンドの先と同じ",
    );
    assert_eq!(l[0].b.y, base.y, "ちょうど水平");

    // 110° の向き（垂直寄り）→ 垂直。
    let base = l[0].b;
    let pos = screen_at(&h, base, 110.0, 150.0);
    hover(&mut h, pos);
    let rubber = cursor(&h);
    assert_point(rubber, Point2::new(base.x, model(&h, pos).y), "垂直");
    click(&mut h, pos);
    let l = lines(&h);
    assert_eq!(l.len(), 2);
    assert_point(l[1].b, rubber, "クリックで入った点");
    assert_eq!(l[1].b.x, base.x, "ちょうど垂直");
}

/// F10 → 43° を指すと 45° に吸い付き、クリックでも 45° の線になる。
/// 半直線から遠い・角度の差が ±3° を超えるときはカーソルのまま。
#[test]
fn f10_snaps_a_click_near_45_degrees_to_45() {
    let (mut h, base) = line_from_p1_with(&[egui::Key::F10]);
    assert!(h.state().drafting.is_on(Mode::Polar), "F10 でオン");

    // 100px 先の 43° は 45° の半直線まで 100·sin2° ≈ 3.5px（10px 以内・差 2° は ±3° 以内）。
    let pos = screen_at(&h, base, 43.0, 100.0);
    hover(&mut h, pos);
    let rubber = cursor(&h);
    let toward = rubber - base;
    assert!(
        eq_len(toward.x, toward.y),
        "ラバーバンドは 45° に吸い付く: {toward:?}"
    );
    click(&mut h, pos);
    let l = lines(&h);
    assert_eq!(l.len(), 1);
    assert_point(l[0].b, rubber, "クリックで入った点もラバーバンドの先と同じ");
    let d = l[0].b - l[0].a;
    assert!(eq_len(d.x, d.y) && d.x > 0.0, "45° の線: {d:?}");

    // 400px 先の 223°（225° から 2°）は角度の差は小さいが、半直線から ≈ 14px 離れている。
    let base = l[0].b;
    let pos = screen_at(&h, base, 223.0, 400.0);
    hover(&mut h, pos);
    assert_point(cursor(&h), model(&h, pos), "遠ければカーソルのまま");
    // 30px 先の 40°（45° から 5°）は半直線まで ≈ 2.6px だが、角度の差が ±3° を超える。
    let pos = screen_at(&h, base, 40.0, 30.0);
    hover(&mut h, pos);
    assert_point(
        cursor(&h),
        model(&h, pos),
        "基準点の近くでも角度が離れていれば吸い付かない",
    );
}

/// 両方オンなら直交が優先される（40° でも 45° ではなく水平）。
#[test]
fn ortho_wins_when_both_are_on() {
    let (mut h, base) = line_from_p1_with(&[egui::Key::F8, egui::Key::F10]);
    assert!(h.state().drafting.is_on(Mode::Ortho) && h.state().drafting.is_on(Mode::Polar));
    let pos = screen_at(&h, base, 40.0, 100.0);
    hover(&mut h, pos);
    let rubber = cursor(&h);
    assert_point(rubber, Point2::new(model(&h, pos).x, base.y), "水平");
    click(&mut h, pos);
    assert_point(lines(&h)[0].b, rubber, "クリックで入った点");
}

/// 片方を切っても他方の状態は変わらない（AutoCAD のように F8 で F10 が切れない）。
#[test]
fn toggling_one_mode_keeps_the_other() {
    let mut h = app();
    hover(&mut h, P1);
    press(&mut h, egui::Key::F10);
    press(&mut h, egui::Key::F8);
    press(&mut h, egui::Key::F8);
    let d = h.state().drafting;
    assert!(!d.is_on(Mode::Ortho), "F8 を 2 回でオフ");
    assert!(d.is_on(Mode::Polar), "F8 を切っても POLAR は残る");

    press(&mut h, egui::Key::F8);
    press(&mut h, egui::Key::F10);
    let d = h.state().drafting;
    assert!(d.is_on(Mode::Ortho), "F10 を切っても ORTHO は残る");
    assert!(!d.is_on(Mode::Polar));

    let toggles: Vec<String> = h
        .state()
        .session
        .cmdline
        .history()
        .filter(|l| l.text.starts_with("直交モード:") || l.text.starts_with("極トラッキング:"))
        .map(|l| l.text.clone())
        .collect();
    assert_eq!(toggles.len(), 5, "切り替えるたびに履歴に残る: {toggles:?}");
    assert_eq!(toggles[0], "極トラッキング: ON");
}

/// スナップに吸着しているときは直交よりスナップが優先される（端点に吸い付く）。
#[test]
fn a_snap_wins_over_ortho() {
    let mut h = app_with_dynamic(true);
    // 先に斜めの線分を 1 本引いておく。
    let a = egui::pos2(300.0, 500.0);
    let b = egui::pos2(700.0, 200.0);
    hover(&mut h, a);
    type_text(&mut h, "L");
    press(&mut h, egui::Key::Enter);
    click(&mut h, a);
    click(&mut h, b);
    press(&mut h, egui::Key::Escape);
    let end = lines(&h)[0].b;

    press(&mut h, egui::Key::F8);
    type_text(&mut h, "L");
    press(&mut h, egui::Key::Enter);
    click(&mut h, P1);
    let base = h.state().session.tracking_base().expect("前提: 基準点");

    // 端点のすぐ近く（P1 から見て斜め）。直交なら水平になるところ、端点に吸い付く。
    let near = h.state().viewport.model_to_screen(end) + egui::vec2(3.0, 3.0);
    hover(&mut h, near);
    assert!(h.state().snapped.is_some(), "前提: 吸着している");
    assert_point(cursor(&h), end, "スナップが優先");
    assert!(cursor(&h).y != base.y, "水平に拘束されていない");
    click(&mut h, near);
    let l = lines(&h);
    assert_eq!(l.len(), 2);
    assert_point(l[1].b, end, "クリックでも端点");
}

/// 寸法入力で角度を固定していれば、直交より固定が優先される。
#[test]
fn an_angle_lock_wins_over_ortho() {
    let (mut h, base) = line_from_p1_with(&[egui::Key::F8]);
    press(&mut h, egui::Key::Tab); // 空の Tab は固定せずに角度の欄へ
    type_text(&mut h, "30");
    press(&mut h, egui::Key::Tab);
    let locks = DimValues {
        length: None,
        angle_deg: Some(30.0),
    };
    assert_eq!(h.state().session.cmdline.dimension_locks(), Some(locks));

    hover(&mut h, P2);
    let expected = crate::cmdline::dimension::constrain(base, model(&h, P2), locks)
        .expect("P2 は 30° の半直線と同じ側");
    assert_point(cursor(&h), expected, "直交ではなく 30° の半直線へ射影");
    click(&mut h, P2);
    assert_point(lines(&h)[0].b, expected, "クリックで入った点");
}

/// 長さだけ固定しているときは、向きを直交が決める（固定していない欄はカーソルから）。
#[test]
fn a_length_lock_follows_the_ortho_direction() {
    let (mut h, base) = line_from_p1_with(&[egui::Key::F8]);
    type_text(&mut h, "100");
    press(&mut h, egui::Key::Tab);
    let pos = screen_at(&h, base, -20.0, 200.0);
    hover(&mut h, pos);
    let expected = Point2::new(base.x + 100.0, base.y);
    assert_point(cursor(&h), expected, "右へ水平に 100");
    click(&mut h, pos);
    assert_point(lines(&h)[0].b, expected, "クリックで入った点");
}

/// 直接距離入力の向きも直交に従う。
#[test]
fn direct_distance_follows_ortho() {
    let (mut h, base) = line_from_p1_with(&[egui::Key::F8]);
    let pos = screen_at(&h, base, 160.0, 200.0);
    hover(&mut h, pos);
    type_text(&mut h, "50");
    press(&mut h, egui::Key::Enter);
    let l = lines(&h);
    assert_eq!(l.len(), 1);
    assert_point(l[0].b, Point2::new(base.x - 50.0, base.y), "左へ水平に 50");
}

/// 基準点が無い段階（LINE の 1 点目）では拘束しない。
#[test]
fn nothing_is_constrained_before_the_first_point() {
    let mut h = app_with_dynamic(true);
    hover(&mut h, P1);
    press(&mut h, egui::Key::F8);
    press(&mut h, egui::Key::F10);
    type_text(&mut h, "L");
    press(&mut h, egui::Key::Enter);
    assert_eq!(
        h.state().session.tracking_base(),
        None,
        "1 点目には基準点が無い"
    );
    hover(&mut h, P2);
    let raw = model(&h, P2);
    assert_point(cursor(&h), raw, "カーソルのまま");
    click(&mut h, P2);
    assert_point(
        h.state().session.tracking_base().expect("1 点目が入った"),
        raw,
        "クリックした位置がそのまま 1 点目",
    );
}

/// RECTANGLE の対角は直交にかけない（かけると面積 0 で必ず断られる）。
#[test]
fn rectangle_is_not_constrained_by_ortho() {
    let mut h = app_with_dynamic(true);
    hover(&mut h, P1);
    press(&mut h, egui::Key::F8);
    type_text(&mut h, "REC");
    press(&mut h, egui::Key::Enter);
    click(&mut h, P1);
    assert_eq!(
        h.state().session.tracking_base(),
        None,
        "対角は基準点を持たない"
    );
    click(&mut h, P2);
    assert_eq!(h.state().doc.entities().len(), 1, "矩形が描ける");
}

/// ステータスバーの `ORTHO` / `POLAR` はクリックで切り替わり、指のカーソルになる。
#[test]
fn ortho_and_polar_in_the_status_bar_are_clickable_with_a_pointing_hand() {
    use egui_kittest::kittest::Queryable as _;

    let mut h = app_with_dynamic(true);
    for (mode, off, on) in [
        (Mode::Ortho, "ortho", "ORTHO"),
        (Mode::Polar, "polar", "POLAR"),
    ] {
        assert!(!h.state().drafting.is_on(mode), "前提: 起動時はオフ");
        let target = h.get_by_label(off).rect().center();
        hover(&mut h, target);
        assert_eq!(
            h.output().platform_output.cursor_icon,
            egui::CursorIcon::PointingHand,
            "{on}"
        );
        click(&mut h, target);
        assert!(h.state().drafting.is_on(mode), "{on}: クリックでオン");
        let target = h.get_by_label(on).rect().center();
        hover(&mut h, target);
        assert_eq!(
            h.output().platform_output.cursor_icon,
            egui::CursorIcon::PointingHand,
            "{on}"
        );
    }
    // 片方のクリックで他方は変わらない。
    let target = h.get_by_label("ORTHO").rect().center();
    click(&mut h, target);
    assert!(
        !h.state().drafting.is_on(Mode::Ortho),
        "もう一度クリックでオフ"
    );
    assert!(h.state().drafting.is_on(Mode::Polar), "POLAR は残る");
    let toggled = h
        .state()
        .session
        .cmdline
        .history()
        .filter(|l| l.text.starts_with("直交モード:") || l.text.starts_with("極トラッキング:"))
        .count();
    assert_eq!(toggled, 3, "クリックでも履歴に残る");
}

/// 変換中（コマンドラインでもパネルの欄でも）は F8 / F10 で切り替えない。
///
/// 日本語 IME では F8 が半角カナ、F10 が半角英数への変換キー。変換のつもりで押したキーで
/// 作図補助が切り替わると、気づかないまま作図が変わる（PR #35 の操作レビュー）。
#[test]
fn f8_and_f10_do_nothing_while_composing() {
    let commit = || egui::Event::Ime(egui::ImeEvent::Commit("線".to_owned()));
    let mut h = app_with_dynamic(true);
    hover(&mut h, P1);

    // コマンドラインで変換中。
    frame(&mut h, [preedit("せん")]);
    settle(&mut h);
    press(&mut h, egui::Key::F8);
    press(&mut h, egui::Key::F10);
    assert!(
        !h.state().drafting.is_on(Mode::Ortho),
        "変換中の F8 で切り替えない"
    );
    assert!(
        !h.state().drafting.is_on(Mode::Polar),
        "変換中の F10 で切り替えない"
    );

    // 同じフレームに Preedit と F8 が来ても切り替えない。
    let mut events = vec![preedit("せんぶん")];
    events.extend(key(egui::Key::F8));
    frame(&mut h, events);
    settle(&mut h);
    assert!(!h.state().drafting.is_on(Mode::Ortho), "同じフレームの F8");

    // 確定した後は効く。
    frame(&mut h, [commit()]);
    settle(&mut h);
    press(&mut h, egui::Key::F8);
    assert!(h.state().drafting.is_on(Mode::Ortho), "確定後の F8 は効く");

    // パネルの欄（レイヤ名）で変換中。
    focus_layer_name_field(&mut h);
    frame(&mut h, [preedit("れいや")]);
    settle(&mut h);
    press(&mut h, egui::Key::F10);
    press(&mut h, egui::Key::F8);
    assert!(
        !h.state().drafting.is_on(Mode::Polar),
        "パネルで変換中の F10"
    );
    assert!(h.state().drafting.is_on(Mode::Ortho), "パネルで変換中の F8");

    // 取り消し（空の Preedit）の後は効く。
    frame(&mut h, [preedit("")]);
    settle(&mut h);
    press(&mut h, egui::Key::F10);
    assert!(
        h.state().drafting.is_on(Mode::Polar),
        "取り消し後の F10 は効く"
    );
}

/// MIRROR で軸を決めた後の「元を消すか Y/N」では、点を指さないので拘束しない。
#[test]
fn mirror_yes_no_prompt_is_not_tracked() {
    let mut h = app_with_dynamic(true);
    hover(&mut h, P1);
    press(&mut h, egui::Key::F10);
    // 選ぶ線分を 1 本引く。
    let a = egui::pos2(300.0, 500.0);
    let b = egui::pos2(400.0, 520.0);
    type_text(&mut h, "L");
    press(&mut h, egui::Key::Enter);
    click(&mut h, a);
    click(&mut h, b);
    press(&mut h, egui::Key::Escape);

    type_text(&mut h, "MI");
    press(&mut h, egui::Key::Enter);
    // 線分の中点を選ぶ。履歴が伸びて作図領域が縮むので、画面位置はモデル座標から取り直す。
    let l = lines(&h)[0];
    let mid = h.state().viewport.model_to_screen(l.a.lerp(l.b, 0.5));
    click(&mut h, mid);
    assert_eq!(h.state().session.selection.len(), 1, "前提: 線分を選んだ");
    press(&mut h, egui::Key::Enter); // 選択を確定
    click(&mut h, P1);
    assert!(
        h.state().session.tracking_base().is_some(),
        "前提: 軸の 2 点目では効く"
    );
    click(&mut h, P2);
    assert!(
        h.state().session.prompt().contains("[はい(Y)/いいえ(N)]"),
        "前提: Y/N の問い合わせ中: {}",
        h.state().session.prompt()
    );
    assert_eq!(
        h.state().session.tracking_base(),
        None,
        "Y/N の間は拘束しない"
    );
    let pos = screen_at(&h, model(&h, P1), 44.0, 100.0);
    hover(&mut h, pos);
    assert_point(cursor(&h), model(&h, pos), "45° に吸い付かない");
}

/// 動的入力がオフでも、直接距離入力の向きは直交に従う（コマンドラインの経路）。
#[test]
fn direct_distance_follows_ortho_with_dynamic_input_off() {
    let mut h = app_with_dynamic(false);
    hover(&mut h, P1);
    press(&mut h, egui::Key::F8);
    type_text(&mut h, "L");
    press(&mut h, egui::Key::Enter);
    click(&mut h, P1);
    let base = h.state().session.tracking_base().expect("前提: 基準点");
    let pos = screen_at(&h, base, 250.0, 200.0);
    hover(&mut h, pos);
    type_text(&mut h, "50");
    press(&mut h, egui::Key::Enter);
    let l = lines(&h);
    assert_eq!(l.len(), 1);
    assert_point(l[0].b, Point2::new(base.x, base.y - 50.0), "下へ垂直に 50");
}

// ---- リボン（Issue #26） ----------------------------------------------------
//
// ボタンは `Ribbon::probe` が記録した矩形の中央をクリックして押す（本物のポインタの経路）。
// 強調やタブの中身も、描いた結果として記録された状態で検査する。

/// 直前のフレームに描いたリボンのボタン。
fn ribbon_buttons(h: &Harness<'_, CadApp>) -> Vec<crate::ribbon::DrawnButton> {
    h.state().ribbon.probe().buttons.clone()
}

/// 今のタブに見えているボタンの名前（タブの行の右端のクイックアクセスを除く）。
fn ribbon_names(h: &Harness<'_, CadApp>) -> Vec<&'static str> {
    ribbon_buttons(h)
        .iter()
        .filter(|b| !b.quick)
        .map(|b| b.name)
        .collect()
}

/// クイックアクセスに見えているボタンの名前（画面の左から順）。
fn quick_names(h: &Harness<'_, CadApp>) -> Vec<&'static str> {
    let mut quick: Vec<_> = ribbon_buttons(h).into_iter().filter(|b| b.quick).collect();
    quick.sort_by(|a, b| a.rect.left().total_cmp(&b.rect.left()));
    quick.iter().map(|b| b.name).collect()
}

/// リボンのボタンを押す。そのボタンが今のタブに無ければ panic。
fn press_ribbon(h: &mut Harness<'_, CadApp>, name: &str) {
    let button = ribbon_buttons(h)
        .into_iter()
        .find(|b| b.name == name)
        .unwrap_or_else(|| panic!("{name} のボタンが見えていない"));
    let rect = button.rect;
    let viewport = h
        .state()
        .ribbon
        .probe()
        .viewport
        .expect("リボンが描かれている");
    assert!(
        button.quick || viewport.contains(rect.center()),
        "{name} はスクロールの外にあって押せない: {rect:?} / {viewport:?}"
    );
    let hints = h.state().ribbon.probe().hints;
    assert!(
        button.quick || !hints.iter().flatten().any(|b| b.contains(rect.center())),
        "{name} は「‹」「›」の帯の下にあって押せない"
    );
    click(h, rect.center());
}

/// タブを切り替える。
fn press_tab(h: &mut Harness<'_, CadApp>, title: &str) {
    let rect = h
        .state()
        .ribbon
        .probe()
        .tabs
        .iter()
        .find(|(t, _, _)| *t == title)
        .unwrap_or_else(|| panic!("{title} のタブが無い"))
        .1;
    click(h, rect.center());
}

/// 実行中のコマンドの印が付いているタブ。
fn marked_tabs(h: &Harness<'_, CadApp>) -> Vec<&'static str> {
    h.state()
        .ribbon
        .probe()
        .tabs
        .iter()
        .filter(|(_, _, mark)| *mark)
        .map(|(t, _, _)| *t)
        .collect()
}

/// 強調されているボタンの名前。
fn highlighted(h: &Harness<'_, CadApp>) -> Vec<&'static str> {
    ribbon_buttons(h)
        .iter()
        .filter(|b| b.highlighted)
        .map(|b| b.name)
        .collect()
}

/// LINE を押すと名前を打ったのと同じく始まり、そのまま打った座標が始点に入る
/// （ボタンがキーボードのフォーカスを奪わない）。動的入力のオン・オフどちらでも。
#[test]
fn ribbon_button_starts_the_command_and_keeps_the_keyboard() {
    for on in [false, true] {
        let mut h = app_with_dynamic(on);
        hover(&mut h, P1);
        press_ribbon(&mut h, "LINE");

        assert_eq!(input_lines(&h), vec!["> LINE"], "動的入力 {on}");
        assert_eq!(h.state().session.active_command(), Some("LINE"));
        assert_eq!(h.state().session.prompt(), "線分の始点を指定:");
        assert_eq!(
            h.state().session.cmdline.frame_is_dynamic(),
            on,
            "プロンプトの出る場所（カーソル横 / 画面下）"
        );

        type_text(&mut h, "10,10");
        press(&mut h, egui::Key::Enter);
        assert_eq!(
            h.state().session.last_point(),
            Some(Point2::new(10.0, 10.0)),
            "続けて打った座標が LINE の始点に入る（動的入力 {on}）"
        );
    }
}

/// LINE の実行中に CIRCLE を押すと、LINE を中断して CIRCLE が始まる。
#[test]
fn ribbon_button_interrupts_the_running_command() {
    for on in [false, true] {
        let mut h = app_with_dynamic(on);
        hover(&mut h, P1);
        press_ribbon(&mut h, "LINE");
        click(&mut h, P1);
        assert!(
            h.state().session.last_point().is_some(),
            "前提: 始点が入った"
        );

        press_ribbon(&mut h, "CIRCLE");
        assert_eq!(
            h.state().session.active_command(),
            Some("CIRCLE"),
            "動的入力 {on}"
        );
        assert_eq!(h.state().session.last_point(), None, "LINE は捨てられた");
        assert_eq!(input_lines(&h), vec!["> LINE", "> CIRCLE"]);
        assert!(
            h.state()
                .session
                .cmdline
                .history()
                .any(|l| l.text == "*取り消し*"),
            "中断が履歴に残る"
        );
        assert!(lines(&h).is_empty(), "線は引かれていない");
    }
}

/// UNDO を押すと 1 つ戻る（即時実行のコマンド）。
#[test]
fn ribbon_undo_undoes_one_step() {
    let mut h = app();
    hover(&mut h, P1);
    type_text(&mut h, "L");
    press(&mut h, egui::Key::Enter);
    click(&mut h, P1);
    click(&mut h, P2);
    press(&mut h, egui::Key::Enter);
    assert_eq!(lines(&h).len(), 1, "前提: 線が 1 本");

    press_ribbon(&mut h, "UNDO");
    assert!(lines(&h).is_empty(), "UNDO で消える");
    assert_eq!(input_lines(&h).last().map(String::as_str), Some("> UNDO"));
}

/// LAYER（ホームにある）を押すとレイヤパネルが開き、もう一度押すと閉じる（UI 要求を出すコマンド）。
#[test]
fn ribbon_layer_toggles_the_layer_panel() {
    let mut h = app();
    assert!(!h.state().layer_panel.is_open(), "前提");
    press_ribbon(&mut h, "LAYER");
    assert!(h.state().layer_panel.is_open());
    press_ribbon(&mut h, "LAYER");
    assert!(!h.state().layer_panel.is_open());
    assert_eq!(input_lines(&h), vec!["> LAYER", "> LAYER"]);
}

/// 起動時はホーム。タブを切り替えると見えるボタンが変わる。
#[test]
fn ribbon_tabs_switch_the_visible_buttons() {
    let mut h = app();
    assert_eq!(h.state().ribbon.tab_title(), "ホーム", "起動時はホーム");
    let home = ribbon_names(&h);
    assert!(home.contains(&"LINE") && home.contains(&"TRIM"), "{home:?}");
    assert!(!home.contains(&"INSERT"), "{home:?}");

    press_tab(&mut h, "コンポーネント");
    let comp = ribbon_names(&h);
    assert!(
        comp.contains(&"INSERT") && comp.contains(&"PSET"),
        "{comp:?}"
    );
    assert!(!comp.contains(&"LINE"), "{comp:?}");

    press_tab(&mut h, "表示・ファイル");
    let view = ribbon_names(&h);
    assert!(
        view.contains(&"ZOOM") && view.contains(&"SAVEAS"),
        "{view:?}"
    );
    assert!(!view.contains(&"QUIT"), "QUIT は出さない");

    press_tab(&mut h, "ホーム");
    assert_eq!(ribbon_names(&h), home);
}

/// 実行中のコマンドのボタンだけが強調され、終われば消える。
/// 打って始めたコマンドでも同じ（強調はリボンの状態ではなく実行中のツールから決まる）。
#[test]
fn ribbon_highlights_only_the_running_command() {
    let mut h = app();
    hover(&mut h, P1);
    assert!(highlighted(&h).is_empty(), "何もしていなければ強調なし");

    press_ribbon(&mut h, "LINE");
    assert_eq!(highlighted(&h), vec!["LINE"]);

    press(&mut h, egui::Key::Escape);
    assert!(highlighted(&h).is_empty(), "中断したら消える");

    type_text(&mut h, "C");
    press(&mut h, egui::Key::Enter);
    assert_eq!(highlighted(&h), vec!["CIRCLE"], "打って始めても強調する");
}

/// 押した後の空 Enter で同じコマンドが再実行される。動的入力のオン・オフどちらでも。
#[test]
fn empty_enter_repeats_the_command_pressed_on_the_ribbon() {
    for on in [false, true] {
        let mut h = app_with_dynamic(on);
        hover(&mut h, P1);
        press_ribbon(&mut h, "CIRCLE");
        press(&mut h, egui::Key::Escape);
        assert!(!h.state().session.has_active_tool(), "前提: 中断した");

        press(&mut h, egui::Key::Enter);
        assert_eq!(
            h.state().session.active_command(),
            Some("CIRCLE"),
            "動的入力 {on}"
        );
        assert_eq!(input_lines(&h), vec!["> CIRCLE", "> CIRCLE"]);
    }
}

/// パネルの入力欄を編集中にボタンを押しても、その後のキーはコマンドラインへ届く。
#[test]
fn ribbon_button_returns_the_keyboard_from_a_panel_field() {
    let mut h = app();
    focus_layer_name_field(&mut h);
    press_ribbon(&mut h, "LINE");
    type_text(&mut h, "5,5");
    press(&mut h, egui::Key::Enter);
    assert_eq!(h.state().session.last_point(), Some(Point2::new(5.0, 5.0)));
    assert_eq!(layer_name_field_value(&h), "", "パネルの欄には入っていない");
}

/// リボンの高さは抑えてある（目安 80px 以下）。
#[test]
fn ribbon_is_compact() {
    let h = app();
    let rect = h.state().ribbon.probe().rect.expect("リボンが描かれている");
    assert!(rect.height() <= 80.0, "リボンの高さ {}", rect.height());
}

/// 幅 800px では全部は見えないが、ホイールで横にスクロールすれば最後のボタンまで届く。
#[test]
fn narrow_window_scrolls_the_ribbon_to_the_last_button() {
    let mut h = app();
    h.set_size(egui::vec2(800.0, 600.0));
    settle(&mut h);
    let last = *crate::ribbon::layout::TABS[0]
        .groups
        .last()
        .and_then(|g| g.commands.last())
        .expect("ホームの最後のボタン");
    let viewport = h.state().ribbon.probe().viewport.expect("リボン");
    let rect_of = |h: &Harness<'_, CadApp>| {
        ribbon_buttons(h)
            .iter()
            .find(|b| b.name == last)
            .expect("ボタンは描かれている（見えていなくても）")
            .rect
    };
    assert!(
        !viewport.contains(rect_of(&h).center()),
        "前提: 800px では {last} は見えていない"
    );

    // リボンの上で縦にホイールを回す（横スクロールしかない領域では横に動く）。
    let over = egui::pos2(400.0, viewport.center().y);
    hover(&mut h, over);
    for _ in 0..10 {
        frame(
            &mut h,
            [egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, -200.0),
                phase: egui::TouchPhase::Move,
                modifiers: egui::Modifiers::NONE,
            }],
        );
    }
    h.run_steps(30);
    let viewport = h.state().ribbon.probe().viewport.expect("リボン");
    assert!(
        viewport.contains(rect_of(&h).center()),
        "スクロール後は {last} が見える: {:?} / {viewport:?}",
        rect_of(&h)
    );
    press_ribbon(&mut h, last);
    assert_eq!(
        input_lines(&h).last().map(String::as_str),
        Some(&*format!("> {last}"))
    );
}

/// コマンドラインで変換中にボタンを押しても何もしない。未確定の文字列は残り、
/// 確定か取り消しを促すエラーが出る。確定した後なら押せる（PR #32 のレビュー）。
///
/// 押した時点で入力欄を空にすると、変換中のまま空になって以後の Enter が効かなかった。
#[test]
fn ribbon_button_does_nothing_while_composing() {
    for on in [false, true] {
        let mut h = app_with_dynamic(on);
        hover(&mut h, P1);
        frame(&mut h, [preedit("える")]);
        settle(&mut h);
        assert!(h.state().session.cmdline.is_composing(), "前提: 変換中");
        let before = h.state().session.cmdline.input().to_owned();

        press_ribbon(&mut h, "LINE");
        assert!(
            !h.state().session.has_active_tool(),
            "始まらない（動的入力 {on}）"
        );
        assert!(input_lines(&h).is_empty());
        assert_eq!(
            h.state().session.cmdline.input(),
            before,
            "未確定の文字列に触らない"
        );
        assert!(
            h.state()
                .session
                .cmdline
                .history()
                .any(|l| l.kind == LineKind::Error && l.text.contains("変換中")),
            "確定か取り消しを促す"
        );

        // 確定して入力を消せば、押せる。
        frame(
            &mut h,
            [egui::Event::Ime(egui::ImeEvent::Commit("える".to_owned()))],
        );
        settle(&mut h);
        press_ribbon(&mut h, "LINE");
        assert_eq!(
            h.state().session.active_command(),
            Some("LINE"),
            "動的入力 {on}"
        );
        type_text(&mut h, "3,4");
        press(&mut h, egui::Key::Enter);
        assert_eq!(
            h.state().session.last_point(),
            Some(Point2::new(3.0, 4.0)),
            "その後のキーも通る（動的入力 {on}）"
        );
    }
}

/// UNDO / REDO / SAVE はタブの行の右端に、どのタブを開いていても出ていて押せる。
#[test]
fn quick_access_is_available_on_every_tab() {
    let mut h = app();
    hover(&mut h, P1);
    type_text(&mut h, "L");
    press(&mut h, egui::Key::Enter);
    click(&mut h, P1);
    click(&mut h, P2);
    press(&mut h, egui::Key::Enter);
    assert_eq!(lines(&h).len(), 1, "前提: 線が 1 本");

    for tab in ["ホーム", "コンポーネント", "表示・ファイル"] {
        press_tab(&mut h, tab);
        assert_eq!(
            quick_names(&h),
            vec!["UNDO", "REDO", "SAVE"],
            "{tab}（左から順）"
        );
        assert!(
            !ribbon_names(&h).contains(&"UNDO"),
            "{tab}: タブの中には置かない"
        );
    }
    // タブの直後（区切り線の右）に並ぶ。右端だと視線の外になっていた。
    let last_tab = h.state().ribbon.probe().tabs.last().expect("タブ").1;
    let first_quick = ribbon_buttons(&h)
        .into_iter()
        .filter(|b| b.quick)
        .map(|b| b.rect.left())
        .fold(f32::INFINITY, f32::min);
    let gap = first_quick - last_tab.right();
    assert!(
        (0.0..=24.0).contains(&gap),
        "最後のタブのすぐ右に置く: 間 {gap}px"
    );
    // ホーム以外のタブから UNDO / REDO。
    press_ribbon(&mut h, "UNDO");
    assert!(lines(&h).is_empty(), "UNDO");
    press_ribbon(&mut h, "REDO");
    assert_eq!(lines(&h).len(), 1, "REDO");
}

/// 実行中のコマンドが別のタブにあるときだけ、そのタブの見出しに印が付く。
#[test]
fn tab_with_the_running_command_is_marked() {
    let mut h = app();
    hover(&mut h, P1);
    assert!(marked_tabs(&h).is_empty(), "何もしていなければ印なし");

    press_ribbon(&mut h, "LINE");
    assert!(
        marked_tabs(&h).is_empty(),
        "開いているタブには付けない（ボタンが強調される）"
    );

    press_tab(&mut h, "コンポーネント");
    assert_eq!(
        marked_tabs(&h),
        vec!["ホーム"],
        "別のタブを開くとホームに印"
    );
    assert_eq!(
        h.state().session.active_command(),
        Some("LINE"),
        "タブの切り替えは中断しない"
    );

    press(&mut h, egui::Key::Escape);
    assert!(marked_tabs(&h).is_empty(), "終われば消える");
}

/// POLYLINE の途中で LAYER を押してもポリラインは消えず、パネルが開いて続きを打てる。
#[test]
fn ribbon_layer_keeps_the_running_polyline() {
    let mut h = app();
    hover(&mut h, P1);
    press_ribbon(&mut h, "POLYLINE");
    click(&mut h, P1);
    click(&mut h, P2);
    press_ribbon(&mut h, "LAYER");
    assert!(h.state().layer_panel.is_open(), "パネルが開く");
    assert_eq!(
        h.state().session.active_command(),
        Some("POLYLINE"),
        "中断しない"
    );
    assert_eq!(highlighted(&h), vec!["POLYLINE"]);

    type_text(&mut h, "0,0");
    press(&mut h, egui::Key::Enter);
    press(&mut h, egui::Key::Enter);
    assert_eq!(
        h.state().doc.entities().len(),
        1,
        "3 点のポリラインが確定する"
    );
}

/// 幅が足りないときだけ、送れる側の端に印が出る。
#[test]
fn overflow_hints_follow_the_scroll_position() {
    let mut h = app();
    assert_eq!(
        h.state().ribbon.probe().overflow,
        (false, false),
        "1280px では収まる"
    );

    h.set_size(egui::vec2(800.0, 600.0));
    settle(&mut h);
    assert_eq!(
        h.state().ribbon.probe().overflow,
        (false, true),
        "右へ送れる"
    );

    let viewport = h.state().ribbon.probe().viewport.expect("リボン");
    hover(&mut h, egui::pos2(400.0, viewport.center().y));
    for _ in 0..10 {
        frame(
            &mut h,
            [egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, -200.0),
                phase: egui::TouchPhase::Move,
                modifiers: egui::Modifiers::NONE,
            }],
        );
    }
    h.run_steps(30);
    assert_eq!(
        h.state().ribbon.probe().overflow,
        (true, false),
        "右端まで送った"
    );
}

/// 幅 800px で「›」の帯を押すと、下に隠れたボタンは押されず、表示範囲が右へ動く。
/// 右端まで送ると「‹」が出て、押すと左へ戻る（PR #32 の操作レビュー）。
#[test]
fn overflow_hints_scroll_instead_of_pressing_the_button_below() {
    let mut h = app();
    h.set_size(egui::vec2(800.0, 600.0));
    settle(&mut h);
    let probe = h.state().ribbon.probe().clone();
    let right = probe.hints[1].expect("前提: 「›」が出ている");
    assert!(
        probe.hints[0].is_some() && probe.overflow == (false, true),
        "前提: 「‹」の帯も置かれているが、左へは送れない"
    );
    // 帯の中で、下にボタンが隠れている点を押す（帯の中央はグループの隙間のことがある）。
    let hidden = ribbon_buttons(&h)
        .into_iter()
        .find(|b| !b.quick && b.rect.intersects(right))
        .expect("前提: 「›」の帯の下にボタンが隠れている");
    let at = egui::pos2(
        hidden
            .rect
            .center()
            .x
            .clamp(right.left() + 1.0, right.right() - 1.0),
        hidden.rect.center().y,
    );
    assert!(hidden.rect.contains(at) && right.contains(at), "前提");
    let before = probe.offset;

    click(&mut h, at);
    settle(&mut h);
    assert!(input_lines(&h).is_empty(), "下のボタンは押されない");
    assert!(!h.state().session.has_active_tool());
    assert!(!h.state().layer_panel.is_open());
    let after = h.state().ribbon.probe().offset;
    assert!(after > before, "右へ送られる: {before} → {after}");

    let left = h.state().ribbon.probe().hints[0].expect("「‹」の帯");
    assert!(h.state().ribbon.probe().overflow.0, "送ったので左へ送れる");
    click(&mut h, left.center());
    settle(&mut h);
    assert!(
        input_lines(&h).is_empty(),
        "「‹」でも下のボタンは押されない"
    );
    let back = h.state().ribbon.probe().offset;
    assert!(back < after, "左へ戻る: {after} → {back}");
}

/// はみ出している間は「‹」「›」の帯を左右両方に常に置き、端まで送った後に続けて押しても
/// 下のボタンへ押し抜けない（PR #32 の 3 回目の操作レビュー）。幅 600 / 700 / 800px。
///
/// 送れる側だけに帯を置いていたときは、端に着くとその側の帯が消えて同じ位置に端のボタンが
/// 現れ、「›」の連打で LAYER が開閉し、「‹」で実行中の POLYLINE が中断されていた。
#[test]
fn repeated_presses_on_the_hints_never_reach_the_buttons_below() {
    for width in [600.0, 700.0, 800.0] {
        let mut h = app();
        h.set_size(egui::vec2(width, 600.0));
        settle(&mut h);
        hover(&mut h, P1);
        type_text(&mut h, "PL");
        press(&mut h, egui::Key::Enter);
        click(&mut h, egui::pos2(300.0, 300.0));
        click(&mut h, egui::pos2(400.0, 350.0));
        let point = h.state().session.last_point();
        assert!(point.is_some(), "前提: POLYLINE に 2 点（{width}px）");
        let history = input_lines(&h);

        // 人がするのと同じく、山形の見えていた位置を同じ場所で押し続ける（帯があるかどうかを
        // 毎回確かめてから押すと、帯が消えて下のボタンに当たる不具合を再現できない）。
        let viewport = h.state().ribbon.probe().viewport.expect("リボン");
        let spots = [
            egui::pos2(viewport.left() + 7.0, viewport.center().y),
            egui::pos2(viewport.right() - 7.0, viewport.center().y),
        ];
        for (side, label) in [(1, "›"), (0, "‹")] {
            for n in 0..4 {
                click(&mut h, spots[side]);
                settle(&mut h);
                assert!(
                    h.state().ribbon.probe().hints[side].is_some(),
                    "{width}px: 「{label}」の帯が消えた（{} 回目）",
                    n + 1
                );
                assert_eq!(
                    h.state().session.active_command(),
                    Some("POLYLINE"),
                    "{width}px: 「{label}」{} 回目でコマンドが変わった",
                    n + 1
                );
                assert_eq!(
                    h.state().session.last_point(),
                    point,
                    "{width}px: 点が変わった"
                );
                assert_eq!(input_lines(&h), history, "{width}px: 履歴が増えた");
                assert!(
                    !h.state().layer_panel.is_open(),
                    "{width}px: レイヤパネルが開いた"
                );
            }
        }
        // 4 回ずつ押せば端まで行って戻っている。
        assert_eq!(
            h.state().ribbon.probe().overflow,
            (false, true),
            "{width}px: 左端に戻った"
        );
    }
}

/// 端まで送ると、端のボタンは帯の下に隠れず押せる（両端に帯の幅の余白を足している）。
#[test]
fn edge_buttons_are_clear_of_the_hints_at_both_ends() {
    let mut h = app();
    h.set_size(egui::vec2(700.0, 600.0));
    settle(&mut h);
    let first = *crate::ribbon::layout::TABS[0].groups[0]
        .commands
        .first()
        .expect("最初のボタン");
    let last = *crate::ribbon::layout::TABS[0]
        .groups
        .last()
        .and_then(|g| g.commands.last())
        .expect("最後のボタン");
    let clear = |h: &Harness<'_, CadApp>, name: &str| {
        let rect = ribbon_buttons(h)
            .into_iter()
            .find(|b| b.name == name)
            .expect("ボタン")
            .rect;
        let probe = h.state().ribbon.probe().clone();
        probe.viewport.expect("リボン").contains_rect(rect)
            && !probe.hints.iter().flatten().any(|b| b.intersects(rect))
    };
    assert!(clear(&h, first), "左端では {first} が帯にかからない");
    for _ in 0..4 {
        let band = h.state().ribbon.probe().hints[1].expect("「›」");
        click(&mut h, band.center());
        settle(&mut h);
    }
    assert_eq!(
        h.state().ribbon.probe().overflow,
        (true, false),
        "右端まで送った"
    );
    assert!(clear(&h, last), "右端では {last} が帯にかからない");
    press_ribbon(&mut h, last);
    assert_eq!(
        input_lines(&h).last().map(String::as_str),
        Some(&*format!("> {last}"))
    );
}

/// はみ出していない幅（1280px のホーム）では、帯も両端の余白も出さない。
///
/// 余白は「はみ出している間だけ」足す（端まで送ったとき端のボタンを帯から出すため）。
/// 常に足すように壊しても他のテストは通ってしまい、収まる幅で並びが 28px 右へずれても
/// 気づけなかった（PR #32 のコード再レビュー）。
#[test]
fn no_hints_or_padding_when_the_ribbon_fits() {
    let h = app();
    let probe = h.state().ribbon.probe().clone();
    assert_eq!(probe.overflow, (false, false), "前提: 1280px では収まる");
    assert!(
        probe.hints.iter().all(Option::is_none),
        "帯は両方とも無い: {:?}",
        probe.hints
    );
    let viewport = probe.viewport.expect("リボン");
    let line = ribbon_buttons(&h)
        .into_iter()
        .find(|b| b.name == "LINE")
        .expect("LINE のボタン")
        .rect;
    let gap = line.left() - viewport.left();
    assert!(
        (0.0..crate::ribbon::FADE_WIDTH).contains(&gap),
        "LINE は表示範囲の左端から帯の幅未満にある（余白が無い）: {gap}px"
    );
}

// ---- 未保存確認のモーダル（Issue #24） ----------------------------------------

/// LINE 実行中に線を 1 本引いて（未保存になる）、Ctrl+N で未保存確認のモーダルを出す。
///
/// 図面に保存先を与えておく。無いと「保存する」が rfd のファイルダイアログを開き、
/// テストが返ってこなくなる。戻り値は保存先。
fn app_with_unsaved_modal(on: bool, name: &str) -> (Harness<'static, CadApp>, std::path::PathBuf) {
    let mut h = app_with_dynamic(on);
    let path = std::env::temp_dir().join(format!(
        "ymcad_modal_{name}_{}_{on}.ymc",
        std::process::id()
    ));
    h.state_mut().doc.mark_saved(Some(path.clone()));
    hover(&mut h, P1);
    type_text(&mut h, "L");
    press(&mut h, egui::Key::Enter);
    click(&mut h, P1);
    click(&mut h, P2);
    assert_eq!(lines(&h).len(), 1, "前提: 線が 1 本ある");
    assert!(h.state().doc.is_dirty(), "前提: 未保存");
    assert!(h.state().session.has_active_tool(), "前提: LINE 実行中");
    let ctrl_n = egui::Event::Key {
        key: egui::Key::N,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::CTRL,
    };
    frame(&mut h, [ctrl_n]);
    settle(&mut h);
    assert!(h.state().files.is_confirming(), "前提: モーダルが出ている");
    (h, path)
}

/// 「保存する」のフォーカスを外す（フォーカスが無いときにキーがどこへ行くかを見るため）。
/// モーダル内の見出しをクリックする。
fn drop_modal_focus(h: &mut Harness<'_, CadApp>) {
    use egui_kittest::kittest::Queryable as _;

    let heading = h
        .get_by_label("保存されていない変更があります")
        .rect()
        .center();
    click(h, heading);
    assert!(h.state().files.is_confirming(), "前提: まだ開いている");
    assert!(
        !h.get_by_label("保存する (Enter)").is_focused(),
        "前提: フォーカスが外れた"
    );
}

/// モーダルが出ている間、コマンドラインは Enter / Space / Esc / 文字キーを扱わない。
///
/// 扱うと、Enter で直前のコマンドが再実行され、Esc で実行中のコマンドが中断され、
/// Space で LINE が進み、文字が入力欄に溜まる。
#[test]
fn modal_keeps_enter_space_escape_and_text_from_the_command_line() {
    for on in [false, true] {
        let (mut h, _path) = app_with_unsaved_modal(on, "keys");
        drop_modal_focus(&mut h);
        let lines_before = h.state().session.cmdline.history().count();

        // Enter はモーダルが「保存する」として扱うので、ここでは押さない（別のテストで固定）。
        press(&mut h, egui::Key::Space);
        type_text(&mut h, "L");
        press(&mut h, egui::Key::ArrowUp);
        press(&mut h, egui::Key::Tab);
        assert!(
            h.state().session.has_active_tool(),
            "LINE が進まない・終わらない（動的入力 {on}）"
        );
        assert_eq!(
            h.state().session.cmdline.history().count(),
            lines_before,
            "履歴に何も増えない（動的入力 {on}）"
        );
        assert_eq!(
            h.state().session.cmdline.input(),
            "",
            "入力欄に文字が入らない（動的入力 {on}）"
        );

        assert!(
            h.state().files.is_confirming(),
            "Enter / Space などでモーダルが勝手に閉じない（動的入力 {on}）"
        );
        let osnap = h.state().snap.is_enabled();
        press(&mut h, egui::Key::F3);
        press(&mut h, egui::Key::F12);
        press(&mut h, egui::Key::F8);
        press(&mut h, egui::Key::F10);
        assert!(!h.state().drafting.is_on(Mode::Ortho), "F8 は効かない");
        assert!(!h.state().drafting.is_on(Mode::Polar), "F10 は効かない");
        assert_eq!(h.state().snap.is_enabled(), osnap, "F3 は効かない");
        assert_eq!(h.state().session.cmdline.is_dynamic(), on, "F12 は効かない");

        // Esc = キャンセル。モーダルだけが閉じ、図面・未保存・実行中のコマンドは残る。
        press(&mut h, egui::Key::Escape);
        assert!(
            !h.state().files.is_confirming(),
            "Esc で閉じる（動的入力 {on}）"
        );
        assert!(
            h.state().session.has_active_tool(),
            "Esc で LINE が中断されない（動的入力 {on}）"
        );
        assert!(h.state().doc.is_dirty(), "未保存の変更は残る");
        assert_eq!(lines(&h).len(), 1, "図面は残る（動的入力 {on}）");
    }
}

/// モーダルを閉じたら、コマンドラインにキーが戻る。図面は消えない。
#[test]
fn command_line_gets_keys_back_after_the_modal_closes() {
    use egui_kittest::kittest::Queryable as _;

    for on in [false, true] {
        let (mut h, _path) = app_with_unsaved_modal(on, "cancel");
        let target = h.get_by_label("キャンセル (Esc)").rect().center();
        click(&mut h, target);
        assert!(!h.state().files.is_confirming(), "キャンセルで閉じる");
        assert_eq!(
            lines(&h).len(),
            1,
            "キャンセルでは図面が残る（動的入力 {on}）"
        );
        assert!(h.state().doc.is_dirty());

        hover(&mut h, P1);
        type_text(&mut h, "10,10");
        press(&mut h, egui::Key::Enter);
        assert_eq!(
            h.state().session.last_point(),
            Some(Point2::new(10.0, 10.0)),
            "打った座標が LINE に入る（動的入力 {on}）"
        );
    }
}

/// 開いたとき「保存する」にフォーカスがあり（Enter = 保存する）、背景のクリックでは閉じない。
#[test]
fn modal_focuses_save_and_ignores_backdrop_clicks() {
    use egui_kittest::kittest::Queryable as _;

    let (mut h, _path) = app_with_unsaved_modal(true, "focus");
    assert!(
        h.get_by_label("保存する (Enter)").is_focused(),
        "「保存する」にフォーカスがある"
    );
    assert!(
        !h.get_by_label("保存しない").is_focused(),
        "「保存しない」にはフォーカスを置かない"
    );
    click(&mut h, egui::pos2(5.0, 5.0));
    assert!(
        h.state().files.is_confirming(),
        "背景のクリックでは閉じない"
    );
}

/// 開いた直後の Enter は「保存する」を押したことになり、保存してから元の操作（NEW）へ進む。
/// 保存しないほうへは進まない。
#[test]
fn enter_in_the_modal_saves_then_continues() {
    for on in [false, true] {
        let (mut h, path) = app_with_unsaved_modal(on, "enter");
        press(&mut h, egui::Key::Enter);
        assert!(path.exists(), "保存先に書かれた（動的入力 {on}）");
        let _ = std::fs::remove_file(&path);
        assert!(!h.state().files.is_confirming(), "モーダルが閉じる");
        assert!(lines(&h).is_empty(), "保存したあと NEW が実行された");
        assert!(!h.state().doc.is_dirty());
    }
}

/// Tab を 0〜3 回押してから Enter を押しても、必ず「保存する」になる（「保存しない」にならない）。
///
/// egui のボタンはフォーカスがあると Enter で押される。Tab 1 回で「保存しない」へ
/// フォーカスが移ると、Enter で図面が保存されずに捨てられていた。
#[test]
fn enter_always_saves_wherever_the_focus_is() {
    for tabs in 0..=3 {
        let (mut h, path) = app_with_unsaved_modal(true, &format!("tab{tabs}"));
        for _ in 0..tabs {
            press(&mut h, egui::Key::Tab);
            assert!(h.state().files.is_confirming(), "Tab で閉じない");
        }
        press(&mut h, egui::Key::Enter);
        let saved = path.exists();
        let _ = std::fs::remove_file(&path);
        assert!(saved, "Tab {tabs} 回のあとでも保存される");
        assert!(!h.state().files.is_confirming());
        assert!(!h.state().doc.is_dirty(), "保存済み（捨てただけではない）");
    }
}

/// Space では何も起きない。「保存しない」はクリックでしか押せず、押したときだけ捨てる。
#[test]
fn space_does_nothing_and_only_a_click_discards() {
    use egui_kittest::kittest::Queryable as _;

    for tabs in 0..=3 {
        let (mut h, path) = app_with_unsaved_modal(true, &format!("space{tabs}"));
        for _ in 0..tabs {
            press(&mut h, egui::Key::Tab);
        }
        press(&mut h, egui::Key::Space);
        assert!(h.state().files.is_confirming(), "Space では何も起きない");
        assert_eq!(lines(&h).len(), 1, "図面は残る");
        assert!(!path.exists(), "保存もされない");
    }

    let (mut h, path) = app_with_unsaved_modal(true, "discard");
    let target = h.get_by_label("保存しない").rect().center();
    click(&mut h, target);
    assert!(!h.state().files.is_confirming());
    assert!(lines(&h).is_empty(), "クリックでは捨てる");
    assert!(!path.exists(), "保存はされない");
}
