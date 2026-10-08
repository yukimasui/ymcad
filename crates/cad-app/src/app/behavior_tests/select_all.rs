//! 全選択（Ctrl+A / SELECTALL、Issue #34 段階 3、ADR-0044）。
//!
//! Ctrl+A が効く場面（待機中・選択待ち）と効かない場面（点の入力中・変換中・パネルの入力欄・
//! 打ちかけの文字・モーダル）を、アプリ全体を動かして確かめる。何が選ばれるか（対象の条件）は
//! `session/select_all_tests.rs` の単体テスト。

use std::time::{Duration, Instant};

use cad_core::command::{AddEntities, AddLayer, SetLayerProperties};
use cad_core::geom::{Line, Point2};
use cad_core::layer::AciColor;
use cad_core::{Entity, EntityId, Geometry, LayerId};
use egui_kittest::kittest::Queryable as _;
use egui_kittest::Harness;

use super::{
    app, app_with_dynamic, focus_layer_name_field, frame, hover, input_lines, key_with,
    layer_name_field_value, preedit, press, press_ribbon, settle, type_text, CadApp, LineKind, P1,
    P2, RIBBON_FITS_WIDTH, SCREEN,
};

/// Ctrl+A を押して離す。
///
/// egui-winit は Linux で Ctrl を `ctrl` と `command` の両方に写す。`TextEdit` の文字の全選択は
/// `command` を見るので、実機と同じく両方を立てる（片方だけだと実機と違う結果になる）。
fn ctrl_a() -> [egui::Event; 2] {
    key_with(
        egui::Key::A,
        egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
    )
}

fn press_ctrl_a(h: &mut Harness<'_, CadApp>) {
    frame(h, ctrl_a());
    settle(h);
}

/// パネルからの変更と同じ入口で図面を変える。
fn external(h: &mut Harness<'_, CadApp>, cmd: Box<dyn cad_core::Command>) {
    let app = h.state_mut();
    app.session.apply_external(cmd, &mut app.doc);
    settle(h);
}

/// 横の線分を `n` 本、`layer` に足して ID を返す（1 回のコマンドで）。
fn add_lines(h: &mut Harness<'_, CadApp>, layer: LayerId, n: usize) -> Vec<EntityId> {
    let before: Vec<EntityId> = h.state().doc.entities().ids().collect();
    let entities = (0..n)
        .map(|i| {
            let y = f64::from(u32::try_from(i).expect("本数")) * 5.0;
            let line = Line::new(Point2::new(0.0, y), Point2::new(100.0, y));
            Entity::new(Geometry::Line(line), layer)
        })
        .collect();
    external(h, Box::new(AddEntities::many("TEST", entities)));
    h.state()
        .doc
        .entities()
        .ids()
        .filter(|id| !before.contains(id))
        .collect()
}

/// 普通のレイヤ 0 に 3 本、ロックされたレイヤに 1 本の図面。選べるのは 3 本。
fn drawing(h: &mut Harness<'_, CadApp>) -> Vec<EntityId> {
    external(h, Box::new(AddLayer::new("LOCKED", AciColor::WHITE)));
    let locked = h.state().doc.layers().by_name("LOCKED").expect("LOCKED");
    external(h, Box::new(SetLayerProperties::new(locked).locked(true)));
    let free = add_lines(h, LayerId::ZERO, 3);
    add_lines(h, locked, 1);
    free
}

fn selected(h: &Harness<'_, CadApp>) -> Vec<EntityId> {
    h.state().session.selection.to_vec()
}

fn last_info(h: &Harness<'_, CadApp>) -> Option<String> {
    h.state()
        .session
        .cmdline
        .history()
        .filter(|l| l.kind == LineKind::Info)
        .last()
        .map(|l| l.text.clone())
}

// ---- 効く場面 -------------------------------------------------------------------

/// 待機中に Ctrl+A で、選べる図形がすべて選ばれ、数を案内する。履歴に入力の行は残さない。
/// 動的入力のオン・オフどちらでも。
#[test]
fn ctrl_a_selects_everything_when_idle() {
    for on in [false, true] {
        let mut h = app_with_dynamic(on);
        let free = drawing(&mut h);
        hover(&mut h, P1);
        press_ctrl_a(&mut h);
        assert_eq!(selected(&h), free, "動的入力 {on}");
        assert!(
            last_info(&h).is_some_and(|t| t.contains("3 個") && t.contains("1 個は除く")),
            "案内（動的入力 {on}）: {:?}",
            last_info(&h)
        );
        assert!(input_lines(&h).is_empty(), "打った扱いにしない");
        assert_eq!(h.state().session.cmdline.input(), "", "入力欄は空のまま");
    }
}

/// ERASE の選択待ちで Ctrl+A → Enter で、選べる図形が全部消える（ロックされた 1 本は残る）。
#[test]
fn erase_then_ctrl_a_then_enter_erases_everything() {
    for on in [false, true] {
        let mut h = app_with_dynamic(on);
        drawing(&mut h);
        hover(&mut h, P1);
        type_text(&mut h, "E");
        press(&mut h, egui::Key::Enter);
        assert_eq!(
            h.state().session.active_command(),
            Some("ERASE"),
            "前提: 選択待ち"
        );

        press_ctrl_a(&mut h);
        assert_eq!(h.state().session.selection.len(), 3, "動的入力 {on}");
        assert_eq!(
            h.state().session.active_command(),
            Some("ERASE"),
            "選択待ちのまま"
        );
        press(&mut h, egui::Key::Enter);
        assert_eq!(
            h.state().doc.entities().len(),
            1,
            "ロックされた 1 本だけが残る（動的入力 {on}）"
        );
        assert!(
            h.state().session.active_command().is_none(),
            "ERASE は終わる"
        );
    }
}

/// 同じフレームに Ctrl+A と Enter が来ても、選んでから確定する（ERASE が全部に効く）。
#[test]
fn ctrl_a_and_enter_in_the_same_frame_select_before_submitting() {
    let mut h = app();
    drawing(&mut h);
    hover(&mut h, P1);
    type_text(&mut h, "E");
    press(&mut h, egui::Key::Enter);
    let mut events = Vec::new();
    events.extend(ctrl_a());
    events.extend(super::key(egui::Key::Enter));
    frame(&mut h, events);
    settle(&mut h);
    assert_eq!(h.state().doc.entities().len(), 1, "3 本が消える");
}

/// `SELECTALL` と打っても同じ。待機中も選択待ちでも。
#[test]
fn typing_selectall_selects_everything() {
    let mut h = app();
    let free = drawing(&mut h);
    hover(&mut h, P1);
    type_text(&mut h, "SELECTALL");
    press(&mut h, egui::Key::Enter);
    assert_eq!(selected(&h), free);
    assert_eq!(input_lines(&h), vec!["> SELECTALL"]);

    press(&mut h, egui::Key::Escape);
    assert!(h.state().session.selection.is_empty(), "前提: Esc で外れる");
    type_text(&mut h, "E");
    press(&mut h, egui::Key::Enter);
    type_text(&mut h, "SELECTALL");
    press(&mut h, egui::Key::Enter);
    assert_eq!(selected(&h), free, "選択待ちでも選ぶ");
    press(&mut h, egui::Key::Enter);
    assert_eq!(h.state().doc.entities().len(), 1, "Enter で 3 本が消える");
}

/// リボンのボタン（ホームの「選択」）でも同じ。選択待ちの ERASE は中断しない。
///
/// 1280px ではボタンが「›」の先にあるので、ホームが収まる幅で押す。
#[test]
fn the_ribbon_button_selects_everything() {
    let mut h = app();
    h.set_size(egui::vec2(RIBBON_FITS_WIDTH, SCREEN.y));
    settle(&mut h);
    let free = drawing(&mut h);
    hover(&mut h, P1);
    press_ribbon(&mut h, "SELECTALL");
    assert_eq!(selected(&h), free, "待機中");
    assert_eq!(input_lines(&h), vec!["> SELECTALL"]);

    press(&mut h, egui::Key::Escape);
    press_ribbon(&mut h, "ERASE");
    assert_eq!(
        h.state().session.active_command(),
        Some("ERASE"),
        "前提: 選択待ち"
    );
    press_ribbon(&mut h, "SELECTALL");
    assert_eq!(
        h.state().session.active_command(),
        Some("ERASE"),
        "ERASE は中断されない"
    );
    assert_eq!(selected(&h), free);
    press(&mut h, egui::Key::Enter);
    assert_eq!(h.state().doc.entities().len(), 1, "Enter で 3 本が消える");
}

// ---- 効かない場面 -----------------------------------------------------------------

/// LINE の点の入力中は効かない。LINE も 1 点目もそのまま。
#[test]
fn ctrl_a_does_nothing_while_a_point_is_wanted() {
    for on in [false, true] {
        let mut h = app_with_dynamic(on);
        drawing(&mut h);
        hover(&mut h, P1);
        type_text(&mut h, "L");
        press(&mut h, egui::Key::Enter);
        super::click(&mut h, P1);
        let first = h.state().session.last_point();
        assert!(first.is_some(), "前提: 1 点目がある");

        press_ctrl_a(&mut h);
        assert!(h.state().session.selection.is_empty(), "動的入力 {on}");
        assert_eq!(h.state().session.active_command(), Some("LINE"));
        assert_eq!(h.state().session.last_point(), first, "1 点目が残る");

        // 続きを描ける（キーもクリックも奪われていない）。
        super::click(&mut h, P2);
        press(&mut h, egui::Key::Enter);
        assert_eq!(h.state().doc.entities().len(), 5, "線が 1 本増える");
    }
}

/// 日本語の変換中は効かず、未確定の文字も残る。変換を取り消せば効く。
#[test]
fn ctrl_a_does_nothing_while_composing() {
    for on in [false, true] {
        let mut h = app_with_dynamic(on);
        let free = drawing(&mut h);
        hover(&mut h, P1);
        frame(&mut h, [preedit("に")]);
        settle(&mut h);
        assert!(h.state().session.cmdline.is_composing(), "前提: 変換中");

        press_ctrl_a(&mut h);
        assert!(
            h.state().session.selection.is_empty(),
            "変換中は選ばない（動的入力 {on}）"
        );
        assert!(h.state().session.cmdline.is_composing(), "変換中のまま");

        frame(&mut h, [preedit("")]);
        settle(&mut h);
        assert!(
            !h.state().session.cmdline.is_composing(),
            "前提: 取り消した"
        );
        press_ctrl_a(&mut h);
        assert_eq!(selected(&h), free, "取り消した後は選ぶ（動的入力 {on}）");
    }
}

/// レイヤ名の欄を編集中の Ctrl+A は、その欄の文字の全選択になる（図形は選ばない）。
/// 続けて打つと、選んだ文字が置き換わる。
#[test]
fn ctrl_a_in_a_panel_field_selects_the_field_text() {
    let mut h = app();
    drawing(&mut h);
    focus_layer_name_field(&mut h);
    type_text(&mut h, "abc");
    assert_eq!(layer_name_field_value(&h), "abc", "前提: 欄に入る");

    press_ctrl_a(&mut h);
    assert!(h.state().session.selection.is_empty(), "図形は選ばない");
    type_text(&mut h, "x");
    assert_eq!(
        layer_name_field_value(&h),
        "x",
        "欄の文字が全選択されていて、打った文字に置き換わる"
    );
    assert_eq!(
        h.state().session.cmdline.input(),
        "",
        "コマンドラインには入らない"
    );
}

/// コマンドラインに打ちかけの文字があるときの Ctrl+A は、入力欄の文字の全選択になる
/// （図形は選ばない。ユーザー判断 5）。続けて打つと、打ちかけの文字が置き換わる。
#[test]
fn ctrl_a_with_typed_text_selects_the_command_line_text() {
    for on in [false, true] {
        let mut h = app_with_dynamic(on);
        drawing(&mut h);
        hover(&mut h, P1);
        type_text(&mut h, "LI");

        press_ctrl_a(&mut h);
        assert!(
            h.state().session.selection.is_empty(),
            "図形は選ばない（動的入力 {on}）"
        );
        assert_eq!(h.state().session.cmdline.input(), "LI", "文字は残る");
        type_text(&mut h, "C");
        assert_eq!(
            h.state().session.cmdline.input(),
            "C",
            "全選択された文字が置き換わる（動的入力 {on}）"
        );
    }
}

/// モーダル（未保存確認）が出ている間は効かない。閉じた後なら効く（効かなかった理由がモーダルだと
/// 分かるよう、待機中の状態でモーダルを出す）。
#[test]
fn ctrl_a_does_nothing_while_the_modal_is_open() {
    let mut h = app();
    let free = drawing(&mut h);
    hover(&mut h, P1);
    // 図面を変えて未保存にする（線を 1 本引いて LINE を終える）。
    type_text(&mut h, "L");
    press(&mut h, egui::Key::Enter);
    super::click(&mut h, P1);
    super::click(&mut h, P2);
    press(&mut h, egui::Key::Escape);
    assert!(h.state().doc.is_dirty(), "前提: 未保存");
    assert!(h.state().session.active_command().is_none(), "前提: 待機中");
    let all = h.state().doc.entities().len();

    frame(&mut h, key_with(egui::Key::N, egui::Modifiers::CTRL));
    settle(&mut h);
    assert!(h.state().files.is_confirming(), "前提: モーダルが出ている");

    press_ctrl_a(&mut h);
    assert!(
        h.state().session.selection.is_empty(),
        "モーダル表示中は選ばない"
    );
    assert!(h.state().files.is_confirming(), "モーダルはそのまま");

    press(&mut h, egui::Key::Escape);
    assert!(!h.state().files.is_confirming(), "前提: Esc で閉じた");
    press_ctrl_a(&mut h);
    assert_eq!(
        h.state().session.selection.len(),
        free.len() + 1,
        "閉じた後は選ぶ（引いた 1 本も入る。全 {all} 本のうちロックの 1 本を除く）"
    );
}

// ---- 1 万図形 -------------------------------------------------------------------

/// 1 万図形を Ctrl+A で選んでも、プロパティパネルの要約は 1 回作り直すだけで、
/// その後のフレームでは作り直さない（固まらない）。
///
/// 時間は悲観的なデバッグビルドで測るので、上限は「固まっていない」ことが分かる程度に緩く取る。
#[test]
fn selecting_ten_thousand_entities_keeps_the_properties_panel_responsive() {
    const N: usize = 10_000;
    let mut h = app();
    add_lines(&mut h, LayerId::ZERO, N);
    frame(&mut h, key_with(egui::Key::Num1, egui::Modifiers::CTRL));
    settle(&mut h);
    assert!(
        h.state().properties_panel.is_open(),
        "前提: パネルが開いている"
    );
    hover(&mut h, P1);
    let before = h.state().properties_panel.summary_recomputed();

    let start = Instant::now();
    press_ctrl_a(&mut h);
    let elapsed = start.elapsed();
    assert_eq!(h.state().session.selection.len(), N);
    assert_eq!(
        h.state().properties_panel.summary_recomputed(),
        before + 1,
        "要約は 1 回だけ作り直す"
    );
    assert!(
        h.query_all_by_label(&format!("{N} 個を選択"))
            .next()
            .is_some(),
        "パネルに個数が出る"
    );

    let frames = Instant::now();
    h.run_steps(10);
    let per_frame = frames.elapsed() / 10;
    assert_eq!(
        h.state().properties_panel.summary_recomputed(),
        before + 1,
        "選択が変わらなければ作り直さない"
    );
    eprintln!("Ctrl+A と 4 フレーム: {elapsed:?}、その後の 1 フレーム平均: {per_frame:?}");
    assert!(
        per_frame < Duration::from_secs(1),
        "1 フレームが 1 秒を超えた（固まっている）: {per_frame:?}"
    );
}
