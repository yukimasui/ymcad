//! プロパティパネルの数値の編集（Issue #31 段階 2、ADR-0041）の振る舞い。
//!
//! 確定は Enter・フォーカス外れ・ドラッグを離したときの 1 回だけ（Undo 1 回）、Esc で取り消し、
//! 不正な値は理由つきで確定しない、入力欄のキーをコマンドラインが奪わない、を画面なしで固定する。
//! 「形 × 項目 × 値 → 新しい形」の中身は `properties_edit.rs` の単体テスト。

use super::*;
use crate::properties::fmt_num;
use crate::properties_edit::{LENGTH_NOT_POSITIVE, NOT_A_NUMBER, NOT_FINITE, ZERO_SWEEP};
use crate::properties_panel::{BOUND_NOTE, STALE_NOTE};
use cad_core::command::AddEntities;
use cad_core::geom::{Arc, Circle, Polyline};

/// 行の項目名 `label` と同じ高さにある、数値の欄の矩形（ボタン表示でも入力中でも）。
fn field_rect(h: &Harness<'_, CadApp>, label: &str) -> egui::Rect {
    let row = h
        .query_all_by_label(label)
        .next()
        .unwrap_or_else(|| panic!("項目 {label} が無い"))
        .rect();
    let y = row.center().y;
    let cmdline = input_rect(h);
    h.query_all_by_role(egui::accesskit::Role::SpinButton)
        .chain(h.query_all_by_role(egui::accesskit::Role::TextInput))
        .map(|n| n.rect())
        .filter(|r| *r != cmdline && r.min.y <= y && y <= r.max.y && r.min.x > row.min.x)
        // 同じ高さに隣のパネルの欄があることもあるので、項目名にいちばん近いものを取る。
        .min_by(|a, b| a.min.x.total_cmp(&b.min.x))
        .unwrap_or_else(|| panic!("項目 {label} の欄が無い（表示だけ？）"))
}

/// 項目 `label` の欄が編集できる（`DragValue` として出ている）か。
fn editable(h: &Harness<'_, CadApp>, label: &str) -> bool {
    let Some(row) = h.query_all_by_label(label).next().map(|n| n.rect()) else {
        return false;
    };
    let y = row.center().y;
    h.query_all_by_role(egui::accesskit::Role::SpinButton)
        .any(|n| n.rect().min.y <= y && y <= n.rect().max.y)
}

/// 項目 `label` の欄に出ている文字（ボタン表示なら値、入力中なら打っている文字）。
fn field_value(h: &Harness<'_, CadApp>, label: &str) -> String {
    let rect = field_rect(h, label);
    h.query_all_by_role(egui::accesskit::Role::SpinButton)
        .chain(h.query_all_by_role(egui::accesskit::Role::TextInput))
        .find(|n| n.rect() == rect)
        .and_then(|n| n.value())
        .unwrap_or_default()
}

/// 項目の欄をクリックして入力を始める（全体が選ばれた状態になる）。
fn click_field(h: &mut Harness<'_, CadApp>, label: &str) {
    let pos = field_rect(h, label).center();
    click(h, pos);
}

/// 項目の欄をクリックし、`text` を打って Enter で確定する。
fn enter_value(h: &mut Harness<'_, CadApp>, label: &str, text: &str) {
    click_field(h, label);
    type_text(h, text);
    press(h, egui::Key::Enter);
}

fn pointer(pos: egui::Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

/// 欄を押して右へ `steps` 回 `dx` px ずつ動かす（まだ離さない）。離す位置を返す。
fn drag_field(h: &mut Harness<'_, CadApp>, label: &str, dx: f32, steps: usize) -> egui::Pos2 {
    let from = field_rect(h, label).center();
    frame(h, [egui::Event::PointerMoved(from), pointer(from, true)]);
    let mut at = from;
    for _ in 0..steps {
        at += egui::vec2(dx, 0.0);
        frame(h, [egui::Event::PointerMoved(at)]);
    }
    at
}

fn release(h: &mut Harness<'_, CadApp>, at: egui::Pos2) {
    frame(h, [pointer(at, false)]);
    settle(h);
}

fn line_of(h: &Harness<'_, CadApp>, id: EntityId) -> Line {
    match &h.state().doc.entities().get(id).expect("図形がある").geom {
        Geometry::Line(l) => *l,
        other => panic!("線分ではない: {other:?}"),
    }
}

fn geom_of(h: &Harness<'_, CadApp>, id: EntityId) -> Geometry {
    h.state()
        .doc
        .entities()
        .get(id)
        .expect("図形がある")
        .geom
        .clone()
}

fn undo_depth(h: &Harness<'_, CadApp>) -> usize {
    h.state().doc.history().len()
}

/// 線分 (0,10)-(100,10) を 1 本だけ選んでパネルを開いた状態。
fn one_line() -> (Harness<'static, CadApp>, EntityId) {
    let Scene { mut h, ids } = scene();
    select(&mut h, &ids[..1]);
    hover(&mut h, P1);
    assert!(editable(&h, "始点 X"), "前提: 編集できる");
    (h, ids[0])
}

/// 図形を 1 つ足して選び、パネルを開いた状態。
fn one_geometry(geom: Geometry) -> (Harness<'static, CadApp>, EntityId) {
    let mut h = app();
    external(
        &mut h,
        Box::new(AddEntities::one("TEST", Entity::new(geom, LayerId::ZERO))),
    );
    let id = h.state().doc.entities().ids().last().expect("足した図形");
    press_ctrl_1(&mut h);
    select(&mut h, &[id]);
    hover(&mut h, P1);
    (h, id)
}

// ---- 確定は 1 回だけ ----------------------------------------------------------

/// 項目をクリックして打って Enter → 形が変わる（1 項目だけ）。Undo 1 回で戻る。
#[test]
fn typing_a_value_and_enter_commits_once_and_u_undoes_it() {
    let (mut h, id) = one_line();
    let before = line_of(&h, id);
    let depth = undo_depth(&h);

    enter_value(&mut h, "始点 X", "25");
    let after = line_of(&h, id);
    assert_eq!(after.a, Point2::new(25.0, 10.0), "始点 X だけが変わる");
    assert_eq!(after.b, before.b, "終点はそのまま");
    assert_eq!(undo_depth(&h), depth + 1, "履歴は 1 つだけ増える");
    assert_eq!(field_value(&h, "始点 X"), "25.0000", "欄も新しい値");
    assert!(
        h.state().session.selection.contains(id),
        "選択はそのまま（続けて直せる）"
    );

    // 確定した後は、キー入力はコマンドラインへ戻っている。
    type_text(&mut h, "U");
    press(&mut h, egui::Key::Enter);
    assert_eq!(line_of(&h, id), before, "U 1 回で元に戻る");
    assert_eq!(undo_depth(&h), depth);
}

/// ドラッグ中は仮の形が出るだけで図面は変わらない。離すと 1 回だけ確定し、Undo 1 回で戻る。
#[test]
fn dragging_shows_a_preview_and_commits_once_on_release() {
    let (mut h, id) = one_line();
    let before = line_of(&h, id);
    let depth = undo_depth(&h);
    let revision = h.state().doc.revision();

    let at = drag_field(&mut h, "長さ", 10.0, 6);
    assert_eq!(line_of(&h, id), before, "ドラッグ中は図面を変えない");
    assert_eq!(h.state().doc.revision(), revision, "版番号も進まない");
    assert_eq!(undo_depth(&h), depth, "履歴も増えない");
    let Some(Geometry::Line(preview)) = h.state().properties_panel.drag_preview().cloned() else {
        panic!("ドラッグ中は仮の形がある");
    };
    assert_eq!(preview.a, before.a, "仮の形は始点を保つ");
    assert!(preview.length() > before.length(), "右へ動かすと長くなる");

    release(&mut h, at);
    let after = line_of(&h, id);
    assert_eq!(after, preview, "離したら仮の形のとおりに確定する");
    assert_eq!(undo_depth(&h), depth + 1, "Undo 1 段だけ");
    assert!(
        h.state().properties_panel.drag_preview().is_none(),
        "離したら仮の形は消える"
    );

    type_text(&mut h, "U");
    press(&mut h, egui::Key::Enter);
    assert_eq!(line_of(&h, id), before, "U 1 回で戻る");
}

/// 表示を触っただけ・Enter を押しただけでは確定しない（表示は丸めた値なので、そのまま
/// 確定すると丸めの分だけ図形が動き、履歴も増える）。
#[test]
fn touching_or_pressing_enter_does_not_add_history() {
    let (mut h, id) = one_geometry(Geometry::Line(Line::new(
        Point2::new(1.234_567_89, 10.0),
        Point2::new(100.0, 10.0),
    )));
    let before = line_of(&h, id);
    let depth = undo_depth(&h);

    // クリックして Enter（丸めた `1.2346` が入力欄に入っている）。
    click_field(&mut h, "始点 X");
    press(&mut h, egui::Key::Enter);
    // クリックして、何もせず別の場所（パネルの見出し）をクリックしてフォーカスを外す。
    // （作図領域を空振りでクリックすると選択が外れるので、パネルの中で外す。）
    click_field(&mut h, "長さ");
    // リボンにも「プロパティ」があるので、いちばん下（パネルの見出し）を取る。
    let heading = h
        .query_all_by_label("プロパティ")
        .map(|n| n.rect())
        .max_by(|a, b| a.min.y.total_cmp(&b.min.y))
        .expect("見出し");
    click(&mut h, heading.center());
    assert!(
        h.state().session.selection.contains(id),
        "前提: 選択はそのまま"
    );
    assert_eq!(line_of(&h, id), before, "図形は 1 ビットも動かない");
    assert_eq!(undo_depth(&h), depth, "履歴は増えない");
    // 少しドラッグして元の値へ戻してから離しても同じ。
    let at = drag_field(&mut h, "終点 Y", 10.0, 3);
    let back = drag_field_continue(&mut h, at, -10.0, 3);
    release(&mut h, back);
    assert_eq!(undo_depth(&h), depth, "元の値へ戻して離しても増えない");
}

/// `drag_field` の続き（押したまま動かす）。
fn drag_field_continue(
    h: &mut Harness<'_, CadApp>,
    from: egui::Pos2,
    dx: f32,
    steps: usize,
) -> egui::Pos2 {
    let mut at = from;
    for _ in 0..steps {
        at += egui::vec2(dx, 0.0);
        frame(h, [egui::Event::PointerMoved(at)]);
    }
    at
}

/// ドラッグ中は、表示だけの項目（円の直径・円周）も仮の形の値になる（Issue #78 の 1）。
/// 図形（仮の円）は大きくなっているのに、数字だけ元のままにならない。離したら確定した値と一致する。
#[test]
fn derived_values_follow_the_preview_while_dragging() {
    let (mut h, id) = one_geometry(Geometry::Circle(Circle::new(
        Point2::new(200.0, 150.0),
        50.0,
    )));
    assert!(has(&h, "100.0000"), "前提: 直径は 100");

    let at = drag_field(&mut h, "半径", 10.0, 6);
    // 表示だけの項目は 1 フレーム遅れて追いつくので、マウスを止めたまま数フレーム回す。
    settle(&mut h);
    let Some(Geometry::Circle(preview)) = h.state().properties_panel.drag_preview().cloned() else {
        panic!("ドラッグ中は仮の円がある");
    };
    assert!(preview.radius > 50.0, "右へ動かすと半径が増える");
    let diameter = fmt_num(preview.radius * 2.0);
    let circumference = fmt_num(std::f64::consts::TAU * preview.radius);
    assert!(has(&h, &diameter), "直径は仮の形の値 {diameter}");
    assert!(has(&h, &circumference), "円周は仮の形の値 {circumference}");
    assert!(!has(&h, "100.0000"), "元の直径は残らない");
    assert_eq!(
        geom_of(&h, id),
        Geometry::Circle(Circle::new(Point2::new(200.0, 150.0), 50.0)),
        "図面はまだ変わっていない"
    );

    release(&mut h, at);
    let Geometry::Circle(after) = geom_of(&h, id) else {
        unreachable!()
    };
    assert_eq!(after, preview, "離したら仮の形のとおりに確定する");
    assert!(
        has(&h, &diameter) && has(&h, &circumference),
        "確定後も同じ値"
    );
}

/// 線分の長さをドラッグすると、終点と中点の表示も仮の形に追いつく。
#[test]
fn line_midpoint_follows_the_preview_while_dragging() {
    let (mut h, _) = one_line();
    assert!(has(&h, "50.0000"), "前提: 中点 X は 50");
    let at = drag_field(&mut h, "長さ", 10.0, 6);
    settle(&mut h);
    let Some(Geometry::Line(preview)) = h.state().properties_panel.drag_preview().cloned() else {
        panic!("ドラッグ中は仮の形がある");
    };
    let mid = preview.midpoint();
    assert!(mid.x > 50.0, "長くなると中点も動く");
    assert!(has(&h, &fmt_num(mid.x)), "中点 X は仮の形の値");
    assert!(!has(&h, "50.0000"), "元の中点 X は残らない");
    assert_eq!(
        field_value(&h, "終点 X"),
        fmt_num(preview.b.x),
        "編集できる終点 X も仮の形の値"
    );
    release(&mut h, at);
}

// ---- 取り消しとキーの持ち主 ---------------------------------------------------

/// Esc で打ちかけの値を捨てる。図形も履歴も変わらず、コマンドラインの Esc（選択の解除）にも
/// ならない。egui の `DragValue` は Esc の次のフレームで打ちかけの文字を確定してしまう
/// （`lost_focus` が 2 フレーム続く）ので、それも拾わないこと。
#[test]
fn escape_cancels_the_input_and_keeps_the_selection() {
    let (mut h, id) = one_line();
    let before = line_of(&h, id);
    let depth = undo_depth(&h);

    click_field(&mut h, "始点 X");
    type_text(&mut h, "55");
    assert_eq!(field_value(&h, "始点 X"), "55", "前提: 入力中");
    press(&mut h, egui::Key::Escape);
    h.run_steps(10);
    assert_eq!(line_of(&h, id), before, "形は変わらない");
    assert_eq!(undo_depth(&h), depth, "履歴は増えない");
    assert!(
        h.state().session.selection.contains(id),
        "Esc がコマンドラインへ渡って選択が外れたりしない"
    );
    assert_eq!(field_value(&h, "始点 X"), "0.0000", "欄は元の値に戻る");
}

/// 入力欄で押した Space・Enter・Esc をコマンドラインが奪わない（ADR-0035）。Space は欄の文字、
/// Enter は欄の確定。どちらもコマンドの実行（空 Enter での直前のコマンドの再実行など）にならない。
#[test]
fn the_command_line_does_not_take_space_enter_or_escape_from_a_field() {
    let (mut h, id) = one_line();
    // 直前のコマンドを作っておく（コマンドラインが空 Enter を受け取ると、これを再実行する）。
    type_text(&mut h, "OSNAP");
    press(&mut h, egui::Key::Enter);
    let lines_before = input_lines(&h);

    click_field(&mut h, "始点 X");
    type_text(&mut h, "1");
    let mut space = Vec::from(key(egui::Key::Space));
    space.insert(1, egui::Event::Text(" ".to_owned()));
    frame(&mut h, space);
    settle(&mut h);
    type_text(&mut h, "2");
    assert_eq!(field_value(&h, "始点 X"), "1 2", "Space は欄の文字になる");
    press(&mut h, egui::Key::Enter);
    assert!(has(&h, NOT_A_NUMBER), "`1 2` は数値として読めない");
    assert_eq!(line_of(&h, id).a.x, 0.0, "確定しない");

    click_field(&mut h, "始点 X");
    type_text(&mut h, "3");
    press(&mut h, egui::Key::Escape);
    assert!(
        h.state().session.selection.contains(id),
        "Esc で選択が外れない"
    );

    assert_eq!(
        input_lines(&h),
        lines_before,
        "コマンドラインの履歴は増えない"
    );
    assert_eq!(
        h.state().session.cmdline.input(),
        "",
        "コマンドラインに文字が入らない"
    );
    assert!(!h.state().session.has_active_tool(), "コマンドが始まらない");
}

/// 日本語入力で全角数字を確定（Preedit → Commit）してから Enter で確定する（ADR-0002）。
#[test]
fn full_width_digits_from_the_ime_are_accepted() {
    let (mut h, id) = one_line();
    click_field(&mut h, "始点 Y");
    frame(&mut h, [preedit("１２")]);
    settle(&mut h);
    assert_eq!(line_of(&h, id).a.y, 10.0, "変換中は確定しない");
    frame(
        &mut h,
        [egui::Event::Ime(egui::ImeEvent::Commit(
            "－１２．５".to_owned(),
        ))],
    );
    settle(&mut h);
    assert_eq!(
        line_of(&h, id).a.y,
        10.0,
        "変換を確定しただけでは確定しない"
    );
    press(&mut h, egui::Key::Enter);
    assert_eq!(line_of(&h, id).a.y, -12.5, "Enter で全角の値が入る");
    assert!(
        !h.state().session.cmdline.is_composing(),
        "コマンドラインは変換中にならない"
    );
}

// ---- 不正な値 -----------------------------------------------------------------

/// 不正な値は確定せず、項目のすぐ下に理由が出て、値は元に戻る。正しい値を入れ直すと理由は消える。
///
/// 前の項目の理由の行が消えて並びがずれても、次の項目の入力が途切れないことも見ている
/// （欄の ID が並び順に依存すると、`abc` を打つ前にフォーカスが外れた）。
#[test]
fn invalid_values_are_refused_with_a_reason() {
    let (mut h, id) = one_line();
    let before = line_of(&h, id);
    let depth = undo_depth(&h);

    for (label, text, reason) in [
        ("長さ", "0", LENGTH_NOT_POSITIVE),
        ("長さ", "-5", LENGTH_NOT_POSITIVE),
        ("始点 X", "nan", NOT_FINITE),
        ("始点 X", "inf", NOT_FINITE),
        ("終点 X", "abc", NOT_A_NUMBER),
        ("終点 X", "0", "線分の長さが 0 です"),
    ] {
        enter_value(&mut h, label, text);
        assert!(has(&h, reason), "{label} = {text}: 理由「{reason}」が出る");
        assert_eq!(line_of(&h, id), before, "{label} = {text}: 確定しない");
        assert_eq!(undo_depth(&h), depth, "{label} = {text}: 履歴も増えない");
        // 理由は項目のすぐ下（その項目の行と、次の項目の行の間）。
        let reason_y = h
            .query_all_by_label(reason)
            .next()
            .expect("理由")
            .rect()
            .center()
            .y;
        assert!(
            reason_y > field_rect(&h, label).max.y,
            "{label}: 項目の下に出る"
        );
    }
    assert_eq!(field_value(&h, "終点 X"), "100.0000", "欄は元の値に戻る");

    enter_value(&mut h, "終点 X", "80");
    assert_eq!(line_of(&h, id).b.x, 80.0);
    assert!(
        !has(&h, "線分の長さが 0 です"),
        "正しく確定したら理由は消える"
    );
}

/// 1 周でない円弧の開始角を終了角と同じにする入力は「掃引が 0°」で拒む（判断 7）。
#[test]
fn an_arc_cannot_be_given_a_zero_sweep() {
    let (mut h, id) = one_geometry(Geometry::Arc(Arc::new(
        Point2::new(200.0, 150.0),
        50.0,
        std::f64::consts::FRAC_PI_2,
        std::f64::consts::PI,
    )));
    let before = geom_of(&h, id);
    enter_value(&mut h, "開始角", "180");
    assert!(has(&h, ZERO_SWEEP));
    assert_eq!(geom_of(&h, id), before, "確定しない");
    enter_value(&mut h, "開始角", "45");
    let Geometry::Arc(a) = geom_of(&h, id) else {
        unreachable!()
    };
    assert_eq!(a.start_angle, 45f64.to_radians());
    assert_eq!(a.end_angle, std::f64::consts::PI, "終了角は元の値のまま");
}

/// 式（`100+20`）を打ったときの理由は、式が使えないことも伝える（Issue #78 の 2）。
#[test]
fn an_expression_is_refused_and_the_reason_says_expressions_are_not_supported() {
    let (mut h, id) = one_line();
    let before = line_of(&h, id);
    let depth = undo_depth(&h);

    enter_value(&mut h, "終点 X", "100+20");
    assert!(has(&h, NOT_A_NUMBER), "理由が出る");
    assert!(
        NOT_A_NUMBER.contains("数値として読めません")
            && NOT_A_NUMBER.contains("数値だけを入力してください。式は使えません"),
        "文言: {NOT_A_NUMBER}"
    );
    assert_eq!(line_of(&h, id), before, "確定しない");
    assert_eq!(undo_depth(&h), depth, "履歴も増えない");
}

/// `abc` を打って Esc で取り消しても、理由は出ない（Issue #78 の 5）。Esc の次のフレームで
/// `DragValue` が打ちかけの文字をもう一度読みにいくが、編集を始めた記録が無い欄の失敗は無視する。
/// 図形も履歴も変わらない。取り消しの後でも、次に Enter で打った `abc` には理由が出る。
#[test]
fn escape_after_typing_garbage_shows_no_reason() {
    let (mut h, id) = one_line();
    let before = line_of(&h, id);
    let depth = undo_depth(&h);

    click_field(&mut h, "終点 X");
    type_text(&mut h, "abc");
    press(&mut h, egui::Key::Escape);
    h.run_steps(10);
    assert!(!has(&h, NOT_A_NUMBER), "取り消したのに理由が出ない");
    assert_eq!(line_of(&h, id), before, "形は変わらない");
    assert_eq!(undo_depth(&h), depth, "履歴は増えない");
    assert_eq!(field_value(&h, "終点 X"), "100.0000", "欄は元の値に戻る");
    assert!(
        !h.state().properties_panel.has_edit_state(),
        "編集中の状態も残らない"
    );

    // 取り消さずに Enter で確定しようとしたときは、これまでどおり理由が出る。
    enter_value(&mut h, "終点 X", "abc");
    assert!(has(&h, NOT_A_NUMBER), "Enter なら理由が出る");
    assert_eq!(line_of(&h, id), before);
}

/// 角度は 360° 違いを同じ値と見る（Issue #78 の 6）。0° の線分に `360` / `-360` を打って
/// Enter を押しても、履歴は増えず終点も動かない。
#[test]
fn typing_a_full_turn_on_a_zero_degree_line_does_not_commit() {
    let (mut h, id) = one_line();
    let before = line_of(&h, id);
    let depth = undo_depth(&h);
    assert_eq!(field_value(&h, "角度"), "0.0000°", "前提: 0°");

    for text in ["360", "-360", "720", "359.99995"] {
        enter_value(&mut h, "角度", text);
        assert_eq!(line_of(&h, id), before, "{text}: 終点が動かない");
        assert_eq!(undo_depth(&h), depth, "{text}: 履歴は増えない");
        assert!(!has(&h, NOT_A_NUMBER), "{text}: 理由も出ない");
    }
    // 角度が違えば確定する。
    enter_value(&mut h, "角度", "90");
    assert_eq!(undo_depth(&h), depth + 1, "90 は確定する");
}

// ---- 編集途中の状態の寿命 -----------------------------------------------------

/// 編集の途中でリボンの UNDO（など）で図面が変わったら、打った値は確定せず案内する。
/// 確定先の図形の値が変わっていなくても、編集を始めたときの図面ではないので捨てる。
#[test]
fn undo_from_the_ribbon_while_editing_discards_the_input() {
    let Scene { mut h, ids } = scene();
    select(&mut h, &ids[..1]);
    hover(&mut h, P1);
    let before = line_of(&h, ids[0]);

    click_field(&mut h, "始点 X");
    type_text(&mut h, "77");
    // リボンの UNDO と同じ入口（押すのはマウスだが、入力欄のフォーカスを外さずに届く経路として
    // 直接呼ぶ）。最後の操作（3 本目の線分の追加）が取り消される。
    {
        let app = h.state_mut();
        app.session.start_command_from_ui("UNDO", &mut app.doc);
    }
    settle(&mut h);
    assert!(
        !h.state().doc.entities().contains(ids[2]),
        "前提: UNDO で 3 本目が消えた"
    );
    let depth = undo_depth(&h);
    press(&mut h, egui::Key::Enter);
    assert_eq!(line_of(&h, ids[0]), before, "確定しない");
    assert_eq!(undo_depth(&h), depth, "履歴も増えない");
    assert!(has(&h, STALE_NOTE), "捨てたことを案内する");
}

/// 編集の途中（ドラッグ中）に図面を入れ替えたら（NEW / OPEN）、編集途中の状態を捨てる。
/// 版番号は図面をまたいで比べられないので、残すと新しい図面の同じ番号の図形へ確定しうる。
#[test]
fn replacing_the_drawing_discards_the_edit_in_progress() {
    let (mut h, _) = one_line();
    let _at = drag_field(&mut h, "長さ", 10.0, 4);
    assert!(
        h.state().properties_panel.has_edit_state(),
        "前提: ドラッグ中の状態がある"
    );

    h.state_mut()
        .report_file_outcome(crate::file_ops::FileOutcome::Replaced("新規".to_owned()));
    assert!(
        !h.state().properties_panel.has_edit_state(),
        "図面の入れ替えで編集途中の状態を捨てる"
    );
    assert!(h.state().properties_panel.drag_preview().is_none());
}

/// 選択を別の図形へ変えても、入力中のフォーカスや文字が同じ位置の欄へずれない。
/// 欄の ID が並び順で決まると、次の図形の同じ項目が前の図形の入力を引き継ぎ、Enter で
/// 選び直した図形のほうが書き換わる。
#[test]
fn changing_the_selection_does_not_move_the_input_to_another_shape() {
    let Scene { mut h, ids } = scene();
    // 3 本とも始点 X は 0。
    select(&mut h, &ids[..1]);
    hover(&mut h, P1);
    click_field(&mut h, "始点 X");
    type_text(&mut h, "5");

    select(&mut h, &ids[1..2]);
    let cmdline = input_rect(&h);
    assert!(
        h.query_all_by_role(egui::accesskit::Role::TextInput)
            .all(|n| n.rect() == cmdline),
        "別の図形の欄が入力中にならない"
    );
    press(&mut h, egui::Key::Enter);
    assert_eq!(line_of(&h, ids[0]).a.x, 0.0, "前の図形は変わらない");
    assert_eq!(line_of(&h, ids[1]).a.x, 0.0, "選び直した図形も変わらない");
}

// ---- 表示だけのとき -----------------------------------------------------------

/// コマンド実行中（選択待ちを含む）は、数値もチェックボックスも表示だけ（判断 3）。
#[test]
fn fields_are_display_only_while_a_command_runs() {
    let (mut h, id) = one_line();
    select(&mut h, &[]);
    type_text(&mut h, "LINE");
    press(&mut h, egui::Key::Enter);
    select(&mut h, &[id]);
    assert!(has(&h, BUSY_NOTE), "前提: 実行中");
    assert!(!editable(&h, "始点 X"), "数値の欄は出ない");
    assert!(has(&h, "0.0000"), "値は表示する");
    // 押しても何も起きない（表示のラベルをクリックするだけ）。
    let depth = undo_depth(&h);
    let label = h.query_all_by_label("始点 X").next().expect("項目").rect();
    click(&mut h, label.right_center() + egui::vec2(40.0, 0.0));
    type_text(&mut h, "9");
    assert_eq!(undo_depth(&h), depth);
    assert!(h.state().session.has_active_tool(), "LINE は続いている");

    press(&mut h, egui::Key::Escape);
    select(&mut h, &[id]);
    assert!(editable(&h, "始点 X"), "終われば編集できる（対照）");
}

/// 打っている途中にコマンドが始まったら（リボンの LINE など）、打ちかけの値は確定せずに捨てる。
/// 始まったコマンドは中断されない。
#[test]
fn a_command_started_while_typing_discards_the_input() {
    let (mut h, id) = one_line();
    let before = line_of(&h, id);
    let depth = undo_depth(&h);
    click_field(&mut h, "始点 X");
    type_text(&mut h, "77");
    {
        let app = h.state_mut();
        app.session.start_command_from_ui("LINE", &mut app.doc);
    }
    settle(&mut h);
    assert_eq!(
        h.state().session.active_command(),
        Some("LINE"),
        "LINE は続く"
    );
    assert!(has(&h, BUSY_NOTE), "パネルは表示だけになる");
    assert!(
        !h.state().properties_panel.has_edit_state(),
        "打ちかけの値は捨てる"
    );
    assert_eq!(line_of(&h, id), before, "確定しない");
    assert_eq!(undo_depth(&h), depth);
}

/// ポリラインの「閉じ」はチェックボックス。押すと 1 回で確定し、Undo 1 回で戻る。
/// 頂点が 2 つなら表示だけ。
#[test]
fn closing_a_polyline_is_one_click_and_one_undo() {
    let open = Polyline::new(
        vec![
            Point2::new(0.0, 0.0),
            Point2::new(100.0, 0.0),
            Point2::new(100.0, 80.0),
        ],
        false,
    );
    let (mut h, id) = one_geometry(Geometry::Polyline(open.clone()));
    let depth = undo_depth(&h);
    let checkbox = h.get_by_role(egui::accesskit::Role::CheckBox).rect();
    click(&mut h, checkbox.center());
    assert_eq!(
        geom_of(&h, id),
        Geometry::Polyline(Polyline {
            closed: true,
            ..open.clone()
        })
    );
    assert_eq!(undo_depth(&h), depth + 1);
    type_text(&mut h, "U");
    press(&mut h, egui::Key::Enter);
    assert_eq!(geom_of(&h, id), Geometry::Polyline(open));

    let (h, _) = one_geometry(Geometry::Polyline(Polyline::new(
        vec![Point2::new(0.0, 0.0), Point2::new(100.0, 0.0)],
        false,
    )));
    assert!(
        h.query_all_by_role(egui::accesskit::Role::CheckBox)
            .next()
            .is_none(),
        "頂点 2 つでは閉じられないので表示だけ"
    );
}

// ---- インプレース編集中の束縛（段階 3） -----------------------------------------

/// コンポーネント「窓」の編集に入った状態。中身は線分 2 本（添字 0: (0,0)-(30,0)、
/// 添字 1: (0,20)-(30,20)）で、添字 0 の終点 X に式 `幅` を束縛してある。インスタンスは
/// (100, 100) に `rotation_deg` 度回して置き、その上をクリックして EDITCOMP で入る。
/// 返り値は（ハーネス, 定義, 束縛のある線分, 束縛の無い線分）。パネルは開いて、何も選んでいない。
fn in_component_edit(
    rotation_deg: f64,
) -> (
    Harness<'static, CadApp>,
    cad_core::component::DefinitionId,
    EntityId,
    EntityId,
) {
    component_edit_in(app(), rotation_deg, &[])
}

/// [`in_component_edit`] と同じ図面を `h` に作って編集に入る。添字 0 の線分には、終点 X の `幅` に
/// 加えて `extra` の束縛も付ける（パラメータは `幅`・`枠厚`・`開き`）。
fn component_edit_in(
    mut h: Harness<'static, CadApp>,
    rotation_deg: f64,
    extra: &[(Slot, &str)],
) -> (
    Harness<'static, CadApp>,
    cad_core::component::DefinitionId,
    EntityId,
    EntityId,
) {
    let def = {
        let app = h.state_mut();
        let doc = &mut app.doc;
        doc.apply(Box::new(DefineComponent::new(
            "COMPONENT",
            "窓",
            Point2::ORIGIN,
            vec![
                Entity::new(
                    Geometry::Line(Line::new(Point2::ORIGIN, Point2::new(30.0, 0.0))),
                    LayerId::ZERO,
                ),
                Entity::new(
                    Geometry::Line(Line::new(Point2::new(0.0, 20.0), Point2::new(30.0, 20.0))),
                    LayerId::ZERO,
                ),
            ],
        )))
        .expect("定義");
        let def = doc.definitions().by_name("窓").expect("窓");
        doc.apply(Box::new(SetDefinitionParams::new(
            "PARAM",
            def,
            vec![
                ParamDecl::number("幅", 30.0),
                ParamDecl::number("枠厚", 5.0),
                ParamDecl::boolean("開き", false),
            ],
        )))
        .expect("宣言");
        for (slot, expr) in std::iter::once((Slot::LineBx, "幅")).chain(extra.iter().copied()) {
            doc.apply(Box::new(SetBinding::new(
                "BIND",
                def,
                Binding::new(0, slot, parse(expr).expect("解析")),
            )))
            .expect("束縛");
        }
        let placement = Placement::new(
            Point2::new(100.0, 100.0),
            rotation_deg.to_radians(),
            1.0,
            false,
        )
        .expect("配置");
        doc.apply(Box::new(InsertInstance::new(
            "INSERT",
            def,
            placement,
            LayerId::ZERO,
        )))
        .expect("配置");
        def
    };
    settle(&mut h);
    press_ctrl_1(&mut h);
    type_text(&mut h, "EDITCOMP");
    press(&mut h, egui::Key::Enter);
    // 添字 0 の線分の中ほど（定義の (15, 0)）を、配置で図面へ移した点。
    let (sin, cos) = rotation_deg.to_radians().sin_cos();
    let on_instance = h
        .state()
        .viewport
        .model_to_screen(Point2::new(100.0 + 15.0 * cos, 100.0 + 15.0 * sin));
    click(&mut h, on_instance);
    let session = h.state().session.editing().cloned().expect("前提: 編集中");
    let (members, origins) = session.members(&h.state().doc);
    let bound = members[origins.iter().position(|o| *o == Some(0)).expect("添字 0")];
    let free = members[origins.iter().position(|o| *o == Some(1)).expect("添字 1")];
    (h, def, bound, free)
}

/// **束縛された項目だけが表示だけになり、横に式が出る。** 同じ中身の束縛の無い項目は編集できる。
/// 端点が束縛されていれば、長さ・角度（両端点から決まる）も表示だけ。
#[test]
fn bound_items_show_their_expression_and_the_rest_stay_editable() {
    let (mut h, _, bound, free) = in_component_edit(0.0);
    select(&mut h, &[bound]);
    hover(&mut h, P1);
    assert!(has(&h, BOUND_NOTE), "表示だけの項目がある旨の案内");
    assert!(!editable(&h, "終点 X"), "束縛された項目は表示だけ");
    assert!(has(&h, "← 式「幅」"), "横に式が出る");
    for label in ["始点 X", "始点 Y", "終点 Y"] {
        assert!(editable(&h, label), "{label}: 束縛の無い項目は編集できる");
    }
    for label in ["長さ", "角度"] {
        assert!(
            !editable(&h, label),
            "{label}: 端点が束縛されているので表示だけ"
        );
    }
    assert!(has(&h, "← 端点の式から"), "長さ・角度には元が式である旨");

    // 束縛を持たない中身は、すべて編集できる。
    select(&mut h, &[free]);
    assert!(!has(&h, BOUND_NOTE));
    assert!(!has(&h, "← 式「幅」"));
    for label in ["始点 X", "始点 Y", "終点 X", "終点 Y", "長さ", "角度"] {
        assert!(editable(&h, label), "{label}");
    }
}

/// **束縛の無い項目は編集中でも変えられ、ENDCOMP の後も定義に残る。** 束縛はそのまま。
#[test]
fn a_free_item_changed_during_a_component_edit_survives_endcomp() {
    use cad_core::geom::tolerance::eq_len;

    let (mut h, def, bound, _) = in_component_edit(0.0);
    select(&mut h, &[bound]);
    hover(&mut h, P1);
    enter_value(&mut h, "始点 Y", "112.5");
    assert_eq!(
        line_of(&h, bound).a,
        Point2::new(100.0, 112.5),
        "編集中の図面が変わる"
    );

    hover(&mut h, P1);
    type_text(&mut h, "ENDCOMP");
    press(&mut h, egui::Key::Enter);
    assert!(h.state().session.editing().is_none(), "前提: 編集を終えた");
    let d = h.state().doc.definitions().get(def).expect("定義");
    let Geometry::Line(l) = &d.entities[0].geom else {
        panic!("線分のはず");
    };
    assert!(
        eq_len(l.a.y, 12.5),
        "定義の座標（配置の基点を引いた値）で残る: {:?}",
        l.a
    );
    assert!(
        eq_len(l.a.x, 0.0) && eq_len(l.b.y, 0.0),
        "ほかの値はそのまま"
    );
    assert_eq!(d.bindings.len(), 1, "束縛は残る");
    assert_eq!(
        (d.bindings[0].entity, d.bindings[0].slot),
        (0, Slot::LineBx)
    );
}

/// 回した配置で入ったときは、図面の軸と定義の軸の対応で決める。90° 回すと、定義の終点 X は
/// 図面の終点 Y になる。
#[test]
fn a_rotated_component_edit_locks_the_item_on_the_turned_axis() {
    let (mut h, _, bound, _) = in_component_edit(90.0);
    select(&mut h, &[bound]);
    hover(&mut h, P1);
    assert!(!editable(&h, "終点 Y"), "定義の終点 X は図面の Y");
    assert!(editable(&h, "終点 X"), "定義の終点 Y（束縛なし）は図面の X");
    assert!(has(&h, "← 式「幅」"));
}

/// 編集中でなければ従来どおり。束縛を持つ定義のインスタンスも、ただの図形も、すべて編集できる。
#[test]
fn outside_a_component_edit_nothing_is_locked() {
    let (mut h, _, _, _) = in_component_edit(0.0);
    hover(&mut h, P1);
    type_text(&mut h, "ENDCOMP");
    press(&mut h, egui::Key::Enter);
    assert!(h.state().session.editing().is_none(), "前提: 編集を終えた");
    let instance = h
        .state()
        .doc
        .entities()
        .ids()
        .last()
        .expect("置き直したインスタンス");
    select(&mut h, &[instance]);
    hover(&mut h, P1);
    for label in ["基点 X", "基点 Y", "回転", "倍率"] {
        assert!(editable(&h, label), "{label}");
    }
    assert!(!has(&h, BOUND_NOTE));

    let line = add_line(&mut h, LayerId::ZERO, 10.0);
    select(&mut h, &[line]);
    for label in ["始点 X", "終点 X", "長さ", "角度"] {
        assert!(editable(&h, label), "{label}");
    }
    assert!(!has(&h, BOUND_NOTE));
}

/// 長い式。省略しないとパネルに収まらない。
const LONG_EXPR: &str = "if 開き then 幅 * 2 + 枠厚 else 0";

/// 式の案内（`←` で始まる文字か、式を囲む「」）の矩形。上の案内文（「←」の付いた値は…）は拾わない。
fn badge_rects(h: &Harness<'_, CadApp>) -> Vec<(String, egui::Rect)> {
    h.query_all_by_label_contains("←")
        .chain(h.query_all_by_label_contains("「"))
        .filter_map(|n| Some((n.value()?, n.rect())))
        .filter(|(text, _)| {
            // 式は「」で囲んだ形（コンポーネントパネルの「窓」の宣言 などは拾わない）。
            text.starts_with('←') || (text.starts_with('「') && text.ends_with('」'))
        })
        .fold(Vec::new(), |mut all, b| {
            if !all.contains(&b) {
                all.push(b);
            }
            all
        })
}

/// **長い式の案内が、次の行の欄に重ならない。** 1 行に収め、入り切らない分は式の側だけを省略する
/// （`← 式` は必ず残る）。90° 回した配置で入ると、定義の始点 Y の長い式が図面の「始点 X」に付き、
/// すぐ下の「始点 Y」は編集できる欄になる（操作レビューで、折り返した 2 行目が欄の下に隠れた並び）。
/// 1280px・パネル 1 枚と、1024px・3 枚で見る。
#[test]
fn a_long_expression_badge_stays_on_one_line_and_clear_of_the_next_field() {
    for (width, three) in [(1280.0, false), (1024.0, true)] {
        let (mut h, _, bound, _) =
            component_edit_in(app_with_width(width), 90.0, &[(Slot::LineAy, LONG_EXPR)]);
        if three {
            open_layer_panel(&mut h);
            h.state_mut().component_panel.toggle();
        }
        select(&mut h, &[bound]);
        hover(&mut h, P1);
        assert!(
            editable(&h, "始点 Y"),
            "{width}: 前提: 次の行は編集できる欄"
        );
        let next = field_rect(&h, "始点 Y");
        let start_x = h.get_by_label("始点 X").rect();
        let badges = badge_rects(&h);
        for (text, rect) in &badges {
            assert!(
                !rect.intersects(next.shrink(1.0)),
                "{width}: 案内 {text:?} {rect:?} が次の行の欄 {next:?} に重なる"
            );
        }
        // 案内は折り返さない（項目名と同じ 1 行の高さに収まる）。
        for (text, rect) in &badges {
            assert!(
                rect.height() <= start_x.height() + 2.0,
                "{width}: 案内 {text:?} が折り返している: {rect:?} / 項目名 {start_x:?}"
            );
        }
        // 省略するのは式の部分だけ。
        assert!(
            badges.iter().any(|(t, _)| t == "← 式"),
            "{width}: 「← 式」は省略されずに残る: {badges:?}"
        );
        // パネルの右端（レイヤのドロップダウンの右端）からはみ出さない。
        let panel_right = h
            .query_all_by_role(egui::accesskit::Role::ComboBox)
            .map(|n| n.rect())
            .filter(|r| r.left() > start_x.left())
            .min_by(|a, b| a.left().total_cmp(&b.left()))
            .expect("プロパティのレイヤのドロップダウン")
            .right();
        for (text, rect) in &badges {
            assert!(
                rect.right() <= panel_right + 8.0,
                "{width}: 案内 {text:?} {rect:?} がパネルの右端 {panel_right} からはみ出す"
            );
        }
    }
}
