//! 実行中に今できること（Issue #27 段階 2、ADR-0048 の追記）。
//!
//! 段階が進むと中身が変わること、寸法入力の欄で Tab / Enter / Esc の意味が変わること、
//! 2 行目の作図補助（F3 / F8 / F10）が効く段階だけ・実際に効いている状態で出ること
//! （Shift の一時反転、Issue #36）、そして**入力欄の矩形がヒントの有無で 1px も変わらないこと**を
//! アプリ全体を動かして確かめる。組み立ての規則は `cmdline/actions.rs`、プロンプトの読み方は
//! `cmdline/prompt.rs`、全ツールの全段階は `session/hint_tests.rs`。

use egui_kittest::Harness;

use super::hint::{assert_hint_is_clear, hint_actions};
use super::{
    app_with_dynamic, click, frame, hover, input_rect, key, lines, preedit, press, settle,
    type_text, CadApp, P1, P2, SETTLE,
};
use crate::cmdline::actions::{Action, Actions};

fn actions(h: &Harness<'_, CadApp>) -> Actions {
    hint_actions(h).expect("今できることが出ている")
}

/// キーに対応する意味。
fn text_of(a: &Actions, key: &str) -> Option<String> {
    a.items
        .iter()
        .find(|i| i.key == key)
        .map(|i| i.text.clone())
}

fn status(a: &Actions) -> Vec<(String, String)> {
    a.status
        .iter()
        .map(|Action { key, text }| (key.clone(), text.clone()))
        .collect()
}

fn start(h: &mut Harness<'_, CadApp>, command: &str) {
    hover(h, P1);
    type_text(h, command);
    press(h, egui::Key::Enter);
    hover(h, P1);
}

/// LINE: 始点 → 2 点目（寸法入力・直交と極の状態）→ 3 本目（C 閉じる）と、段階が進むと中身が変わる。
#[test]
fn actions_follow_the_stages_of_line() {
    let mut h = app_with_dynamic(true);
    start(&mut h, "L");

    let first = actions(&h);
    assert_eq!(text_of(&first, "クリック").as_deref(), Some("点"));
    assert_eq!(text_of(&first, "数字"), None, "始点では寸法入力は効かない");
    assert_eq!(text_of(&first, "Esc").as_deref(), Some("中断"));
    assert_eq!(
        status(&first),
        vec![("F3".to_owned(), "スナップ オン".to_owned())],
        "始点には基準点が無いので直交・極は出さない"
    );

    click(&mut h, P1);
    hover(&mut h, P2);
    let second = actions(&h);
    assert_eq!(text_of(&second, "数字").as_deref(), Some("長さ"));
    assert_eq!(text_of(&second, "Tab").as_deref(), Some("角度の欄へ"));
    assert_eq!(text_of(&second, "Enter").as_deref(), Some("終了"));
    assert_eq!(text_of(&second, "C"), None, "1 本目ではまだ閉じられない");
    assert_eq!(
        status(&second),
        vec![
            ("F3".to_owned(), "スナップ オン".to_owned()),
            ("F8".to_owned(), "直交 オフ".to_owned()),
            ("F10".to_owned(), "極 オフ".to_owned()),
        ]
    );

    click(&mut h, P2);
    click(&mut h, egui::pos2(P2.x, P1.y));
    hover(&mut h, egui::pos2(600.0, 500.0));
    let third = actions(&h);
    assert_eq!(
        text_of(&third, "C").as_deref(),
        Some("閉じる"),
        "2 本引いたら C"
    );
}

/// 寸法入力: 数字を打つと Tab は「固定して角度へ」、Enter は「この値で確定」、Esc は「固定と入力を外す」。
/// Tab で角度の欄へ移ると「数字 角度」になり、固定があるので Esc は「固定を外す」。
#[test]
fn dimension_fields_change_the_meaning_of_tab_enter_and_escape() {
    let mut h = app_with_dynamic(true);
    start(&mut h, "L");
    click(&mut h, P1);
    hover(&mut h, P2);

    type_text(&mut h, "100");
    let typed = actions(&h);
    assert_eq!(
        text_of(&typed, "Tab").as_deref(),
        Some("長さを固定して角度へ")
    );
    assert_eq!(text_of(&typed, "Enter").as_deref(), Some("この値で確定"));
    assert_eq!(
        text_of(&typed, "Esc").as_deref(),
        Some("入力を消す"),
        "固定はまだ無い"
    );

    press(&mut h, egui::Key::Tab);
    let angle = actions(&h);
    assert_eq!(text_of(&angle, "数字").as_deref(), Some("角度"));
    assert_eq!(text_of(&angle, "Tab").as_deref(), Some("長さの欄へ"));
    assert_eq!(text_of(&angle, "Esc").as_deref(), Some("固定を外す"));

    press(&mut h, egui::Key::Escape);
    let released = actions(&h);
    assert_eq!(text_of(&released, "数字").as_deref(), Some("長さ"));
    assert_eq!(
        text_of(&released, "Esc").as_deref(),
        Some("中断"),
        "固定を外したら中断"
    );
}

// ---- ヒントが実際のキーの動きと一致すること（PR #104 のレビュー） ----------------

/// LINE の 2 点目（寸法入力に参加）まで進める。
fn line_second_point(h: &mut Harness<'_, CadApp>) {
    start(h, "L");
    click(h, P1);
    hover(h, P2);
}

fn field(h: &Harness<'_, CadApp>) -> crate::cmdline::dimension::Field {
    h.state().session.cmdline.dimension_field()
}

/// (a) `-` だけ（数値の打ちかけ）は「まだ打っていない」と同じ: Tab は固定できない（実際に断られて
/// 欄も移らない）ので出さず、Enter も（終了にもならず断られるので）出さない。Esc は実際どおり入力を消す。
#[test]
fn an_incomplete_number_is_not_offered_as_a_value() {
    use crate::cmdline::dimension::Field;
    let mut h = app_with_dynamic(true);
    line_second_point(&mut h);
    type_text(&mut h, "-");
    let a = actions(&h);
    assert_eq!(text_of(&a, "Tab"), None, "Tab は断られるので出さない");
    assert_eq!(text_of(&a, "Enter"), None, "Enter も断られるので出さない");
    assert_eq!(text_of(&a, "Esc").as_deref(), Some("入力を消す"));

    // 実際のキー: Tab では欄が移らない。Enter は「不明なオプション」で断られ、LINE は続く。
    // Esc は入力を消すだけで、LINE は続く。
    press(&mut h, egui::Key::Tab);
    assert_eq!(field(&h), Field::Length, "Tab は断られた");
    press(&mut h, egui::Key::Enter);
    assert!(
        h.state().session.has_active_tool(),
        "Enter で終了していない"
    );
    if h.state().session.cmdline.input().is_empty() {
        type_text(&mut h, "-");
    }
    press(&mut h, egui::Key::Escape);
    assert_eq!(h.state().session.cmdline.input(), "");
    assert!(h.state().session.has_active_tool(), "中断していない");
}

/// (b) 座標（`@3,4`）を打ちかけのときの Esc は入力を消すだけ（中断しない）。Tab は何もしない。
/// Enter は打った座標で確定する。
#[test]
fn escape_while_typing_coordinates_only_clears_the_input() {
    let mut h = app_with_dynamic(true);
    line_second_point(&mut h);
    type_text(&mut h, "@3,4");
    let a = actions(&h);
    assert_eq!(text_of(&a, "数字"), None, "欄は消えている");
    assert_eq!(text_of(&a, "Tab"), None);
    assert_eq!(text_of(&a, "Enter").as_deref(), Some("入力を確定"));
    assert_eq!(text_of(&a, "Esc").as_deref(), Some("入力を消す"));

    press(&mut h, egui::Key::Escape);
    assert_eq!(h.state().session.cmdline.input(), "", "入力が消えた");
    assert!(
        h.state().session.has_active_tool(),
        "中断していない（ヒントどおり）"
    );
}

/// (c) 固定がある状態でオプションの文字（`C`）を打つと、Enter はオプションとして確定する
/// （「この値で確定」ではない）。実際に C で閉じて LINE が終わる。
#[test]
fn an_option_letter_with_a_lock_is_entered_as_the_option() {
    let mut h = app_with_dynamic(true);
    line_second_point(&mut h);
    click(&mut h, P2);
    hover(&mut h, egui::pos2(P2.x, P1.y));
    click(&mut h, egui::pos2(P2.x, P1.y));
    hover(&mut h, egui::pos2(600.0, 500.0));
    type_text(&mut h, "100");
    press(&mut h, egui::Key::Tab);
    assert!(
        h.state().session.cmdline.dimension_locks().is_some(),
        "前提: 長さを固定した"
    );
    type_text(&mut h, "C");
    let a = actions(&h);
    assert_eq!(text_of(&a, "Enter").as_deref(), Some("入力を確定"));
    assert_eq!(text_of(&a, "Esc").as_deref(), Some("固定と入力を外す"));

    let before = lines(&h).len();
    press(&mut h, egui::Key::Enter);
    assert!(!h.state().session.has_active_tool(), "C で閉じて終わった");
    assert_eq!(lines(&h).len(), before + 1, "閉じる線分が足された");
}

/// 選択待ち（MOVE）→ 基点 → 目的点。選択待ちでは選び方が並び、作図補助は出ない。
#[test]
fn selection_then_points_for_move() {
    let mut h = app_with_dynamic(true);
    start(&mut h, "M");
    let selecting = actions(&h);
    for k in [
        "クリック",
        "左→右ドラッグ",
        "右→左ドラッグ",
        "Shift+クリック",
        "Ctrl+A",
    ] {
        assert!(text_of(&selecting, k).is_some(), "{k}");
    }
    assert_eq!(text_of(&selecting, "Enter").as_deref(), Some("確定"));
    assert!(selecting.status.is_empty(), "選択では作図補助は効かない");
}

/// 図形を指す段階（TRIM）: クリックで指す・Enter で終了。スナップは効かないので出さない。
#[test]
fn picking_stage_for_trim() {
    let mut h = app_with_dynamic(true);
    start(&mut h, "TR");
    let a = actions(&h);
    assert_eq!(text_of(&a, "クリック").as_deref(), Some("図形を指す"));
    assert_eq!(text_of(&a, "Enter").as_deref(), Some("終了"));
    assert!(a.status.is_empty());
}

/// オプションはプロンプトから読んでキーを先に並べる（XLINE）。
#[test]
fn options_are_listed_with_their_keys() {
    let mut h = app_with_dynamic(true);
    start(&mut h, "XL");
    let a = actions(&h);
    for (k, label) in [
        ("H", "水平"),
        ("V", "垂直"),
        ("A", "角度"),
        ("O", "オフセット"),
    ] {
        assert_eq!(text_of(&a, k).as_deref(), Some(label), "{k}");
    }
}

/// 2 行目は実際に効いている状態: F8 で直交がオン、Shift の間は一時的にオフ、F10 は直交が優先と出る。
#[test]
fn assists_follow_toggles_and_the_shift_flip() {
    let mut h = app_with_dynamic(true);
    start(&mut h, "L");
    click(&mut h, P1);
    hover(&mut h, P2);

    press(&mut h, egui::Key::F8);
    press(&mut h, egui::Key::F10);
    hover(&mut h, P2);
    let on = actions(&h);
    assert!(status(&on).contains(&("F8".to_owned(), "直交 オン".to_owned())));
    assert!(status(&on).contains(&("F10".to_owned(), "極 オン（直交が優先）".to_owned())));

    frame(
        &mut h,
        [egui::Event::ModifiersChanged(egui::Modifiers::SHIFT)],
    );
    settle(&mut h);
    assert!(
        h.state().shift_flip_active(),
        "前提: Shift の一時反転が効いている"
    );
    let flipped = actions(&h);
    assert!(
        status(&flipped).contains(&("F8".to_owned(), "直交 オフ（Shift の間）".to_owned())),
        "{:?}",
        status(&flipped)
    );
    assert!(status(&flipped).contains(&("F10".to_owned(), "極 オン".to_owned())));

    frame(
        &mut h,
        [egui::Event::ModifiersChanged(egui::Modifiers::NONE)],
    );
    settle(&mut h);
    assert!(status(&actions(&h)).contains(&("F8".to_owned(), "直交 オン".to_owned())));
}

/// 変換中・HINT オフでは出さない（段階 1 と同じ条件）。
#[test]
fn hidden_while_composing_and_when_turned_off() {
    let mut h = app_with_dynamic(true);
    start(&mut h, "L");
    assert!(hint_actions(&h).is_some(), "前提");

    frame(&mut h, [preedit("に")]);
    assert!(hint_actions(&h).is_none(), "変換が始まったフレームで消える");
    frame(&mut h, [preedit("")]);
    settle(&mut h);
    assert!(hint_actions(&h).is_some(), "取り消すと戻る");

    h.state_mut().session.cmdline.toggle_hint();
    settle(&mut h);
    assert!(hint_actions(&h).is_none(), "HINT オフ");
}

/// 入力欄の矩形を、LINE の段階を進めながら 1 フレームごとに記録する。
fn input_rects_through_a_line(hint_on: bool, cursor: egui::Pos2) -> Vec<egui::Rect> {
    let mut h = app_with_dynamic(true);
    if h.state().session.cmdline.is_hint_enabled() != hint_on {
        h.state_mut().session.cmdline.toggle_hint();
    }
    hover(&mut h, cursor);
    let mut rects = Vec::new();
    let mut run = |h: &mut Harness<'_, CadApp>, events: Vec<egui::Event>| {
        frame(h, events);
        rects.push(input_rect(h));
        for _ in 0..SETTLE {
            h.step();
            rects.push(input_rect(h));
            if hint_on {
                assert_hint_is_clear(h, cursor, &format!("カーソル {cursor:?}"));
            }
        }
    };
    run(&mut h, vec![egui::Event::Text("L".to_owned())]);
    run(&mut h, key(egui::Key::Enter).to_vec());
    // 始点をクリック（カーソルは動かさない）。
    let button = |pressed| egui::Event::PointerButton {
        pos: cursor,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    run(&mut h, vec![button(true)]);
    run(&mut h, vec![button(false)]);
    run(&mut h, vec![egui::Event::Text("100".to_owned())]);
    run(&mut h, key(egui::Key::Tab).to_vec());
    run(&mut h, key(egui::Key::F8).to_vec());
    run(&mut h, key(egui::Key::Escape).to_vec());
    run(&mut h, key(egui::Key::Escape).to_vec());
    rects
}

/// **「今できること」が出ても変わっても消えても、入力欄の矩形が 1px も変わらない**
/// （画面中央・右端・下端・隅で）。段階 1 の `input_box_never_moves_with_the_hint` の段階 2 版。
#[test]
fn input_box_never_moves_with_the_actions_hint() {
    let h = app_with_dynamic(true);
    let canvas = h.state().viewport.rect();
    drop(h);
    let inset = 15.0;
    let places = [
        ("中央", P1),
        (
            "右端",
            egui::pos2(canvas.right() - inset, canvas.center().y),
        ),
        (
            "下端",
            egui::pos2(canvas.center().x, canvas.bottom() - inset),
        ),
        (
            "右下の隅",
            egui::pos2(canvas.right() - inset, canvas.bottom() - inset),
        ),
        (
            "左上の隅",
            egui::pos2(canvas.left() + inset, canvas.top() + inset),
        ),
    ];
    for (what, cursor) in places {
        let with_hint = input_rects_through_a_line(true, cursor);
        let without = input_rects_through_a_line(false, cursor);
        assert_eq!(with_hint.len(), without.len());
        for (i, (a, b)) in with_hint.iter().zip(&without).enumerate() {
            assert_eq!(a, b, "{what}: {i} フレーム目で入力欄の矩形が違う");
        }
    }
}
