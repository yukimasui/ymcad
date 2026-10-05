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
