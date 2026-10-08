//! レイヤ名の改名（Issue #68）。
//!
//! ダブルクリックで出る入力欄にフォーカスが入り、打った文字・Enter・Esc・IME の変換が
//! コマンドラインへ流れないことを、アプリ全体を画面なしで動かして固定する。

use egui_kittest::kittest::Queryable as _;
use egui_kittest::Harness;

use super::{
    app_with_dynamic, click, frame, hover, input_lines, key_with, lines, preedit, press, settle,
    type_text, CadApp, P1, P2,
};
use crate::cmdline::LineKind;
use crate::layer_panel::RENAME_DROPPED_NOTE;
use cad_core::command::AddLayer;
use cad_core::{AciColor, LayerId};

/// レイヤ `L1` を足してレイヤパネルを開く。
fn with_layer(on: bool) -> (Harness<'static, CadApp>, LayerId) {
    let mut h = app_with_dynamic(on);
    hover(&mut h, P1);
    {
        let app = h.state_mut();
        app.session
            .apply_external(Box::new(AddLayer::new("L1", AciColor::WHITE)), &mut app.doc);
        app.layer_panel.toggle();
    }
    settle(&mut h);
    let id = h.state().doc.layers().by_name("L1").expect("L1");
    (h, id)
}

fn name_of(h: &Harness<'_, CadApp>, id: LayerId) -> String {
    h.state()
        .doc
        .layers()
        .get(id)
        .expect("レイヤがあるはず")
        .name
        .clone()
}

/// レイヤパネルの名前 `name` をダブルクリックする（押す・離すを 2 回、別フレームで）。
fn double_click_layer_name(h: &mut Harness<'_, CadApp>, name: &str) {
    let pos = h
        .query_all_by_role_and_label(egui::accesskit::Role::Button, name)
        .next()
        .unwrap_or_else(|| panic!("レイヤパネルに {name} の名前があるはず"))
        .rect()
        .center();
    let button = |pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    // 1 クリックを 1 フレームに収める。egui_kittest のフレームは 0.25 秒進むので、
    // 押す・離すを別フレームにすると 2 回目の離しがダブルクリックの間隔（0.3 秒）を越える。
    frame(
        h,
        [egui::Event::PointerMoved(pos), button(true), button(false)],
    );
    frame(h, [button(true), button(false)]);
    settle(h);
}

fn start_line(h: &mut Harness<'_, CadApp>) {
    type_text(h, "L");
    press(h, egui::Key::Enter);
    assert!(h.state().session.has_active_tool(), "前提: LINE 実行中");
}

/// ダブルクリックしてそのまま打つと、元の名前が置き換わり、Enter で改名が確定する。
/// 打った文字はコマンドラインに入らず、Enter でコマンドも実行されない。
///
/// 打つ名前はコマンド名（LINE）にしてある。コマンドラインへ流れると LINE が始まる。
#[test]
fn double_click_then_typing_renames_the_layer() {
    for on in [false, true] {
        let (mut h, id) = with_layer(on);
        double_click_layer_name(&mut h, "L1");
        assert_eq!(
            h.state().layer_panel.renaming(),
            Some(id),
            "前提: 改名中（動的入力 {on}）"
        );

        type_text(&mut h, "LINE");
        assert_eq!(
            h.state().session.cmdline.input(),
            "",
            "コマンドラインには入らない（動的入力 {on}）"
        );
        press(&mut h, egui::Key::Enter);

        assert_eq!(
            name_of(&h, id),
            "LINE",
            "元の名前全体が置き換わる（動的入力 {on}）"
        );
        assert_eq!(h.state().layer_panel.renaming(), None, "改名を終える");
        assert!(
            input_lines(&h).is_empty(),
            "コマンドは実行されない（動的入力 {on}）"
        );
        assert!(!h.state().session.has_active_tool(), "動的入力 {on}");

        // 改名を終えたら、キー入力はコマンドラインへ戻っている。
        type_text(&mut h, "L");
        press(&mut h, egui::Key::Enter);
        assert_eq!(input_lines(&h), vec!["> LINE"], "動的入力 {on}");
    }
}

/// Esc で改名をやめる。名前は元のままで、実行中のコマンドも中断されない。
#[test]
fn escape_cancels_the_rename_but_not_the_running_command() {
    for on in [false, true] {
        let (mut h, id) = with_layer(on);
        start_line(&mut h);
        let history_before = h.state().session.cmdline.history().count();

        double_click_layer_name(&mut h, "L1");
        type_text(&mut h, "X");
        press(&mut h, egui::Key::Escape);

        assert_eq!(name_of(&h, id), "L1", "名前は元のまま（動的入力 {on}）");
        assert_eq!(
            h.state().layer_panel.renaming(),
            None,
            "改名をやめる（動的入力 {on}）"
        );
        assert!(
            h.state().session.has_active_tool(),
            "LINE は中断されない（動的入力 {on}）"
        );
        assert_eq!(
            h.state().session.cmdline.history().count(),
            history_before,
            "履歴に何も増えない（動的入力 {on}）"
        );
        assert_eq!(h.state().session.cmdline.input(), "", "動的入力 {on}");

        // キー入力はコマンドラインへ戻り、打った座標が LINE の始点に入る。
        type_text(&mut h, "10,10");
        press(&mut h, egui::Key::Enter);
        assert_eq!(
            h.state().session.last_point(),
            Some(cad_core::geom::Point2::new(10.0, 10.0)),
            "動的入力 {on}"
        );
    }
}

/// 日本語の変換（Preedit → Commit）でも名前が入り、コマンドラインは変換中にならない。
#[test]
fn rename_accepts_ime_composition() {
    for on in [false, true] {
        let (mut h, id) = with_layer(on);
        double_click_layer_name(&mut h, "L1");

        frame(&mut h, [preedit("かべ")]);
        settle(&mut h);
        assert!(
            !h.state().session.cmdline.is_composing(),
            "コマンドラインは変換中にならない（動的入力 {on}）"
        );
        frame(
            &mut h,
            [egui::Event::Ime(egui::ImeEvent::Commit("壁".to_owned()))],
        );
        settle(&mut h);
        press(&mut h, egui::Key::Enter);

        assert_eq!(name_of(&h, id), "壁", "動的入力 {on}");
        assert_eq!(h.state().session.cmdline.input(), "", "動的入力 {on}");
        assert!(input_lines(&h).is_empty(), "動的入力 {on}");
    }
}

/// LINE の途中で改名しても LINE は続き、改名の後のクリックで線が引ける。
#[test]
fn renaming_during_line_keeps_the_line_going() {
    for on in [false, true] {
        let (mut h, id) = with_layer(on);
        start_line(&mut h);
        click(&mut h, P1);
        assert!(h.state().session.last_point().is_some(), "前提: 始点");

        double_click_layer_name(&mut h, "L1");
        type_text(&mut h, "WALL");
        press(&mut h, egui::Key::Enter);
        assert_eq!(name_of(&h, id), "WALL", "動的入力 {on}");
        assert!(
            h.state().session.has_active_tool(),
            "LINE は続いている（動的入力 {on}）"
        );

        click(&mut h, P2);
        assert_eq!(lines(&h).len(), 1, "始点から線が引かれる（動的入力 {on}）");
    }
}

/// 改名中のレイヤが Undo で消えたら改名をやめる。Redo で戻ってきても、フォーカスの無い
/// 入力欄が出たままにならず、キー入力はコマンドラインへ届く。
///
/// リボンの UNDO はパネルより先に描くので、押したフレームでは行が描かれず、
/// フォーカスが外れたことを入力欄が知る機会が無い。
#[test]
fn undoing_the_layer_under_rename_ends_the_rename() {
    let (mut h, _id) = with_layer(false);
    double_click_layer_name(&mut h, "L1");
    assert!(h.state().layer_panel.renaming().is_some(), "前提: 改名中");

    h.state_mut().doc.undo().expect("AddLayer を戻す");
    settle(&mut h);
    h.state_mut().doc.redo().expect("AddLayer をやり直す");
    settle(&mut h);
    assert_eq!(h.state().layer_panel.renaming(), None);

    type_text(&mut h, "L");
    press(&mut h, egui::Key::Enter);
    assert_eq!(input_lines(&h), vec!["> LINE"]);
}

/// 履歴のうち `kind` の行。
fn history_of(h: &Harness<'_, CadApp>, kind: LineKind) -> Vec<String> {
    h.state()
        .session
        .cmdline
        .history()
        .filter(|l| l.kind == kind)
        .map(|l| l.text.clone())
        .collect()
}

/// 改名の欄の Tab は欄の中で何もしない（フォーカスを隣の部品へ移さない）。
///
/// 移すと、同じ行の線種のドロップダウンへフォーカスが行き、その後に打った文字は
/// どこにも入らず、Enter はドロップダウンを開いた。フォーカスが「無い」わけではないので
/// コマンドラインも取り直さない（PR #71 のコードレビュー）。
#[test]
fn tab_stays_in_the_rename_field() {
    for shift in [false, true] {
        let (mut h, id) = with_layer(false);
        double_click_layer_name(&mut h, "L1");
        type_text(&mut h, "X");
        let modifiers = if shift {
            egui::Modifiers::SHIFT
        } else {
            egui::Modifiers::NONE
        };
        frame(&mut h, key_with(egui::Key::Tab, modifiers));
        settle(&mut h);
        assert_eq!(
            h.state().layer_panel.renaming(),
            Some(id),
            "改名は続いている（Shift {shift}）"
        );
        type_text(&mut h, "Y");
        press(&mut h, egui::Key::Enter);
        assert_eq!(name_of(&h, id), "XY", "Tab の後も欄に入る（Shift {shift}）");

        // 改名を終えたら、打ったコマンドが実行される。
        type_text(&mut h, "L");
        press(&mut h, egui::Key::Enter);
        assert_eq!(input_lines(&h), vec!["> LINE"], "Shift {shift}");
    }
}

/// 欄の外をクリックしてやめたとき、名前を変えていれば案内を 1 行出す（黙って捨てない）。
/// 名前を変えていなければ出さない。Esc でやめたときは本人が分かっているので出さない。
#[test]
fn clicking_away_after_typing_tells_that_the_rename_was_dropped() {
    use egui_kittest::kittest::Queryable as _;

    for (typed, escape, expect_note) in [
        ("WALL", false, true),
        ("", false, false),
        ("WALL", true, false),
    ] {
        let (mut h, id) = with_layer(false);
        double_click_layer_name(&mut h, "L1");
        if !typed.is_empty() {
            type_text(&mut h, typed);
        }
        if escape {
            press(&mut h, egui::Key::Escape);
        } else {
            // パネルの見出しをクリックする（作図領域だと窓選択が始まり、案内が混ざる）。
            let pos = h.get_by_label("レイヤ").rect().center();
            let button = |pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            frame(
                &mut h,
                [egui::Event::PointerMoved(pos), button(true), button(false)],
            );
            settle(&mut h);
        }
        let what = format!("打った {typed:?} / Esc {escape}");
        assert_eq!(name_of(&h, id), "L1", "{what}");
        assert_eq!(h.state().layer_panel.renaming(), None, "{what}");
        let notes = history_of(&h, LineKind::Info);
        assert_eq!(
            notes.iter().any(|t| t.contains(RENAME_DROPPED_NOTE)),
            expect_note,
            "{what}: {notes:?}"
        );
    }
}

/// 同じ名前のレイヤがあるときの Enter は、欄を閉じずに打った文字を残し、フォーカスも戻す。
/// 案内には内部のコマンド名（`LAYER_RENAME`）を出さない。
#[test]
fn duplicate_name_keeps_the_field_open() {
    for on in [false, true] {
        let (mut h, id) = with_layer(on);
        {
            let app = h.state_mut();
            app.session
                .apply_external(Box::new(AddLayer::new("L2", AciColor::WHITE)), &mut app.doc);
        }
        settle(&mut h);

        double_click_layer_name(&mut h, "L1");
        type_text(&mut h, "L2");
        press(&mut h, egui::Key::Enter);

        assert_eq!(name_of(&h, id), "L1", "改名されない（動的入力 {on}）");
        assert_eq!(
            h.state().layer_panel.renaming(),
            Some(id),
            "欄は開いたまま（動的入力 {on}）"
        );
        assert_eq!(
            h.state().layer_panel.rename_text(),
            "L2",
            "打った文字が残る"
        );
        let errors = history_of(&h, LineKind::Error);
        assert_eq!(errors.len(), 1, "動的入力 {on}: {errors:?}");
        assert!(
            errors[0].starts_with("レイヤ名の変更:") && !errors[0].contains("LAYER_RENAME"),
            "利用者向けの言葉で案内する: {errors:?}"
        );
        assert!(input_lines(&h).is_empty(), "動的入力 {on}");

        // フォーカスは欄に戻っていて、続けて直して Enter で改名できる。
        type_text(&mut h, "X");
        assert_eq!(h.state().session.cmdline.input(), "", "動的入力 {on}");
        press(&mut h, egui::Key::Enter);
        assert_eq!(name_of(&h, id), "L2X", "動的入力 {on}");
        assert_eq!(h.state().layer_panel.renaming(), None, "動的入力 {on}");
    }
}
