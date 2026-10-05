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
