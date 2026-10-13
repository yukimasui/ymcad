//! 動的入力のそばのヒント（Issue #27 段階 1、ADR-0048）。
//!
//! 候補の説明が `Enter` で実行される候補に合わせて出ること、ステータスバーの `HINT` と
//! 動的入力のオン/オフ・変換中で出なくなること、そして**ヒントが出ても消えても
//! 入力欄の矩形が 1px も変わらないこと**（画面中央・右端・下端・隅）を、アプリ全体を
//! 動かして確かめる。置き場所の規則そのものは `cmdline/hint.rs` の単体テスト。

use egui_kittest::kittest::Queryable as _;
use egui_kittest::Harness;

use super::{
    app_with_dynamic, click, frame, hover, input_rect, key, preedit, press, settle, type_text,
    CadApp, P1, SETTLE,
};
use crate::cmdline::dynamic::CURSOR_OFFSET_PX;
use crate::cmdline::hint::Hint;

/// このフレームで見せているヒントのコマンド名。
fn hint_name(h: &Harness<'_, CadApp>) -> Option<&'static str> {
    h.state()
        .session
        .cmdline
        .hint_shown()
        .map(|hint| match hint {
            Hint::Command { name, .. } => *name,
        })
}

/// ヒントがキャンバスの内側にあり、入力欄の枠にもカーソルの周りにも重ならないこと。
fn assert_hint_is_clear(h: &Harness<'_, CadApp>, cursor: egui::Pos2, what: &str) {
    let cmdline = &h.state().session.cmdline;
    let Some(hint) = cmdline.hint_rect() else {
        return;
    };
    let canvas = h.state().viewport.rect();
    let floating = cmdline.floating_rect().expect("枠が描かれている");
    let cursor_zone =
        egui::Rect::from_center_size(cursor, egui::Vec2::splat(2.0 * CURSOR_OFFSET_PX));
    assert!(
        canvas.contains_rect(hint),
        "{what}: キャンバスの内側 {hint:?}"
    );
    assert!(
        !hint.intersects(floating) || hint.intersect(floating).area() == 0.0,
        "{what}: 入力欄の枠に重ならない {hint:?} / {floating:?}"
    );
    assert!(
        !hint.intersects(cursor_zone) || hint.intersect(cursor_zone).area() == 0.0,
        "{what}: カーソルの周りに重ならない {hint:?}"
    );
}

/// `S` を打つと `Enter` で実行される候補（エイリアスが一致する STRETCH）の説明が出て、
/// ↑↓ で選んだ候補に合わせて切り替わる。
#[test]
fn hint_explains_the_candidate_that_enter_runs_and_follows_the_selection() {
    let mut h = app_with_dynamic(true);
    hover(&mut h, P1);
    assert_eq!(hint_name(&h), None, "何もしていないときは出ない");

    type_text(&mut h, "S");
    let items: Vec<&str> = crate::tools::suggestions("S")
        .iter()
        .map(|c| c.name)
        .collect();
    assert!(items.len() >= 3, "前提: 候補が 3 つ以上 {items:?}");
    assert_eq!(hint_name(&h), Some("STRETCH"), "エイリアスの完全一致が ⏎");

    press(&mut h, egui::Key::ArrowDown);
    // 先頭は ⏎ の行と同じ STRETCH なので、この ↓ では中身は変わらない（追従は次の ↓ から確かめる）。
    assert_eq!(items[0], "STRETCH", "前提: 先頭は ⏎ の行と同じ");
    assert_eq!(
        hint_name(&h),
        Some(items[0]),
        "↓ で先頭を選んでも STRETCH のまま"
    );
    press(&mut h, egui::Key::ArrowDown);
    assert_eq!(hint_name(&h), Some(items[1]), "もう一度 ↓ で 2 番目");
    press(&mut h, egui::Key::ArrowDown);
    assert_eq!(hint_name(&h), Some(items[2]), "3 番目");
    press(&mut h, egui::Key::ArrowUp);
    assert_eq!(hint_name(&h), Some(items[1]), "↑ で戻る");

    // 打ち足すと候補が変わり、説明もそれに付いていく。
    type_text(&mut h, "AVE");
    assert_eq!(hint_name(&h), Some("SAVE"), "SAVE と完全一致");

    // Esc で候補を閉じるとヒントも消える。
    press(&mut h, egui::Key::Escape);
    assert_eq!(hint_name(&h), None, "候補を閉じたら出ない");
}

/// 1 文字目を打った直後から落ち着くまで、どのフレームでも、出ていればヒントは
/// 入力欄の枠にもカーソルにも重ならない。
#[test]
fn hint_is_clear_from_the_first_frame() {
    let mut h = app_with_dynamic(true);
    hover(&mut h, P1);
    frame(&mut h, [egui::Event::Text("L".to_owned())]);
    for _ in 0..SETTLE {
        assert_hint_is_clear(&h, P1, "L を打った直後");
        h.step();
    }
    assert_eq!(hint_name(&h), Some("LINE"));
    assert_hint_is_clear(&h, P1, "落ち着いた後");
}

/// **中身が変わったフレームでも、その中身の大きさを測ってから置く**（ADR-0048 決定 3）。
///
/// 見せる `Area` の大きさは前フレームの実寸なので、それを使うと中身が変わったフレームに
/// 古い大きさで置き、枠やカーソルに重なりうる。測るのは見えない別の `Area` で、
/// **空白のフレームを挟まずに**新しい中身を出す（↑↓ を押し続けてもちらつかない）。
///
/// `S` の候補を ↓ でたどり、中身が変わるフレームごとに次を確かめる。
/// - そのフレームで新しい中身が見えている（1 フレーム消えない）
/// - 置き場所を決めた大きさが、描いた矩形の大きさと一致する（大きさを知らずに置いていない）
/// - 測る `Area` の中は見えていない
#[test]
fn hint_is_placed_with_the_size_of_its_new_content_in_the_same_frame() {
    let mut h = app_with_dynamic(true);
    hover(&mut h, P1);
    type_text(&mut h, "S");
    let items: Vec<&str> = crate::tools::suggestions("S")
        .iter()
        .map(|c| c.name)
        .collect();
    press(&mut h, egui::Key::ArrowDown);
    assert_eq!(hint_name(&h), Some(items[0]), "前提");

    let mut sizes = Vec::new();
    for expected in &items[1..] {
        frame(&mut h, key(egui::Key::ArrowDown).to_vec());
        let cmdline = &h.state().session.cmdline;
        assert_eq!(
            hint_name(&h),
            Some(*expected),
            "中身が変わったフレームで新しい中身が見えている"
        );
        let drawn = cmdline.hint_drawn().expect("描いている");
        assert_eq!(
            (drawn.placed_size.w, drawn.placed_size.h),
            (drawn.rect.width(), drawn.rect.height()),
            "{expected}: 置いた大きさと描いた大きさが一致する"
        );
        assert_eq!(
            cmdline.hint_measure_visible(),
            Some(false),
            "{expected}: 測る Area は見えない"
        );
        assert_hint_is_clear(&h, P1, expected);
        sizes.push(drawn.rect.size());
    }
    assert!(
        sizes.windows(2).any(|w| w[0] != w[1]),
        "前提: 大きさの違う中身をたどっている {sizes:?}"
    );
}

/// 入力欄の矩形を、操作の 1 フレームごとに記録する。
///
/// 中央・右端・下端・隅に置いたカーソルで、候補を出す → ↓↑ で選び直す（ヒントの中身が
/// 変わる）→ Esc で閉じる（ヒントが消える）→ また打つ（ヒントが出る）を順に行う。
fn input_rects_through_hint_changes(hint_on: bool, cursor: egui::Pos2) -> Vec<egui::Rect> {
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
    run(&mut h, vec![egui::Event::Text("S".to_owned())]);
    run(&mut h, key(egui::Key::ArrowDown).to_vec());
    run(&mut h, key(egui::Key::ArrowDown).to_vec());
    run(&mut h, key(egui::Key::ArrowUp).to_vec());
    run(&mut h, key(egui::Key::Escape).to_vec());
    run(&mut h, vec![egui::Event::Text("C".to_owned())]);
    rects
}

/// **ヒントが出ても消えても、入力欄の矩形が 1px も変わらない**（画面中央・右端・下端・隅で）。
///
/// ヒントを入力欄と同じ `Area` に入れると、枠の大きさが変わって画面端で `constrain_to` が
/// 入力欄を押し戻し、入力欄に付いて出る IME の候補ウィンドウが跳ねる（ADR-0034・ADR-0048）。
/// ヒントのオン・オフで同じ操作をして、入力欄の矩形がフレームごとに一致することを確かめる。
#[test]
fn input_box_never_moves_with_the_hint() {
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
        (
            "右上の隅",
            egui::pos2(canvas.right() - inset, canvas.top() + inset),
        ),
        (
            "左下の隅",
            egui::pos2(canvas.left() + inset, canvas.bottom() - inset),
        ),
    ];
    for (what, cursor) in places {
        let with_hint = input_rects_through_hint_changes(true, cursor);
        let without = input_rects_through_hint_changes(false, cursor);
        assert_eq!(with_hint.len(), without.len());
        for (i, (a, b)) in with_hint.iter().zip(&without).enumerate() {
            assert_eq!(a, b, "{what}: {i} フレーム目で入力欄の矩形が違う");
        }
    }
}

/// 画面の中央では、ヒントは入力欄の枠の下（カーソルから遠い側）に出る。
/// 下端では枠が上へ回り込むので、ヒントは枠の上に出る。
#[test]
fn hint_sits_on_the_far_side_of_the_box_from_the_cursor() {
    let mut h = app_with_dynamic(true);
    hover(&mut h, P1);
    type_text(&mut h, "L");
    let floating = h.state().session.cmdline.floating_rect().expect("枠");
    let hint = h.state().session.cmdline.hint_rect().expect("中央では出る");
    assert!(
        hint.min.y >= floating.max.y,
        "枠の下 {hint:?} / {floating:?}"
    );

    let canvas = h.state().viewport.rect();
    let low = egui::pos2(canvas.center().x, canvas.bottom() - 15.0);
    hover(&mut h, low);
    settle(&mut h);
    let floating = h.state().session.cmdline.floating_rect().expect("枠");
    assert!(floating.max.y < low.y, "前提: 枠はカーソルの上");
    let hint = h
        .state()
        .session
        .cmdline
        .hint_rect()
        .expect("下端でも上に出る");
    assert!(
        hint.max.y <= floating.min.y,
        "枠の上 {hint:?} / {floating:?}"
    );
}

/// 動的入力がオフなら、ヒントは出ない。オンに戻せば出る。
#[test]
fn no_hint_while_dynamic_input_is_off() {
    let mut h = app_with_dynamic(false);
    hover(&mut h, P1);
    type_text(&mut h, "L");
    assert!(
        h.state().session.cmdline.suggestions_visible(),
        "前提: 候補は出ている"
    );
    assert_eq!(hint_name(&h), None);

    press(&mut h, egui::Key::F12);
    settle(&mut h);
    assert_eq!(hint_name(&h), Some("LINE"), "動的入力をオンに戻すと出る");
}

/// ステータスバーの `HINT` をクリックするとオフになり、ヒントが出なくなる。もう一度でオンに戻る。
/// 切り替えは履歴に残り、`DYN` と同じく指のカーソルになる。
#[test]
fn hint_toggles_from_the_status_bar() {
    let mut h = app_with_dynamic(true);
    assert!(h.state().session.cmdline.is_hint_enabled(), "既定はオン");
    let target = h.get_by_label("HINT").rect().center();
    hover(&mut h, target);
    assert_eq!(
        h.output().platform_output.cursor_icon,
        egui::CursorIcon::PointingHand
    );
    click(&mut h, target);
    assert!(
        !h.state().session.cmdline.is_hint_enabled(),
        "クリックでオフ"
    );

    hover(&mut h, P1);
    type_text(&mut h, "L");
    assert_eq!(hint_name(&h), None, "オフの間は出ない");

    let target = h.get_by_label("hint").rect().center();
    click(&mut h, target);
    assert!(
        h.state().session.cmdline.is_hint_enabled(),
        "もう一度でオン"
    );
    hover(&mut h, P1);
    assert_eq!(hint_name(&h), Some("LINE"), "オンに戻すと出る");

    let toggled: Vec<String> = h
        .state()
        .session
        .cmdline
        .history()
        .filter(|l| l.text.starts_with("ヒント:"))
        .map(|l| l.text.clone())
        .collect();
    assert_eq!(toggled, vec!["ヒント: OFF", "ヒント: ON"]);
}

/// 動的入力がオフの間、`HINT` に乗せると「今は出ない」ことがツールチップで分かる
/// （`HINT` は ON の色を薄くして描く。PR #100 の操作レビュー）。
#[test]
fn hint_toggle_explains_that_it_is_dormant_while_dynamic_input_is_off() {
    let mut h = app_with_dynamic(false);
    assert!(
        h.state().session.cmdline.is_hint_enabled(),
        "前提: HINT はオン"
    );
    let target = h.get_by_label("HINT").rect().center();
    hover(&mut h, target);
    settle(&mut h);
    assert!(
        h.query_by_label_contains("今は出ません").is_some(),
        "動的入力がオフなので今は出ない、と案内する"
    );

    let mut h = app_with_dynamic(true);
    let target = h.get_by_label("HINT").rect().center();
    hover(&mut h, target);
    settle(&mut h);
    assert!(
        h.query_by_label_contains("ON/OFF").is_some(),
        "前提: ツールチップが出ている"
    );
    assert!(
        h.query_by_label_contains("今は出ません").is_none(),
        "動的入力がオンなら言わない"
    );
}

/// 変換中はヒントを消す。確定すると（候補が出ていれば）また出る。
#[test]
fn no_hint_while_composing() {
    let mut h = app_with_dynamic(true);
    hover(&mut h, P1);
    type_text(&mut h, "L");
    assert_eq!(hint_name(&h), Some("LINE"), "前提");

    frame(&mut h, [preedit("に")]);
    assert_eq!(hint_name(&h), None, "変換が始まったフレームで消える");
    settle(&mut h);
    assert_eq!(hint_name(&h), None, "変換中");

    frame(&mut h, [preedit("")]);
    settle(&mut h);
    assert_eq!(hint_name(&h), Some("LINE"), "変換を取り消すと戻る");
}

/// 段階 1 では、コマンドの実行中（候補を出さない段階）にはヒントを出さない。
#[test]
fn no_hint_while_a_command_runs() {
    let mut h = app_with_dynamic(true);
    hover(&mut h, P1);
    type_text(&mut h, "L");
    press(&mut h, egui::Key::Enter);
    assert!(h.state().session.has_active_tool(), "前提");
    hover(&mut h, P1);
    assert_eq!(hint_name(&h), None);
}

/// ヒントはクリックを受け取らない（`interactable(false)`）。上をクリックしても
/// キャンバスに届く（カーソル横の枠と同じ。ADR-0034 決定 2）。
#[test]
fn hint_does_not_take_the_pointer() {
    let mut h = app_with_dynamic(true);
    hover(&mut h, P1);
    type_text(&mut h, "L");
    let hint = h.state().session.cmdline.hint_rect().expect("出ている");
    let at = h.ctx.layer_id_at(hint.center());
    assert!(
        at.is_none_or(|l| l.order == egui::Order::Background),
        "ヒントの上はキャンバス（背景）が受け取る: {at:?}"
    );
}
