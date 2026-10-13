//! ファイルダイアログを開いている間もメインループが回り、アプリはモーダルになること（Issue #98）。
//!
//! rfd のダイアログはテストでは開けない（返ってこない）ので、[`FakeDialogs`] に差し替え、
//! 結果はテストが返す。修正前は `FileOps` が rfd を UI スレッドで同期に呼んでいて、
//! 差し替える口も「待っている」状態も無かった（このファイルのテストは書けなかった）。

use std::path::Path;

use cad_core::command::AddEntities;
use cad_core::geom::{Line, Point2};
use cad_core::{native, Document, Entity, Geometry, LayerId};
use egui_kittest::kittest::Queryable as _;
use egui_kittest::Harness;

use super::{
    app, click, frame, hover, input_lines, key_with, lines, press, settle, type_text, CadApp, P1,
    P2,
};
use crate::cmdline::LineKind;
use crate::drafting::Mode;
use crate::file_dialog::{DialogReply, DialogRequest, FakeDialogs};
use crate::file_ops::FileOps;

/// ダイアログを差し替えたアプリ。
fn app_with_fake_dialogs() -> (Harness<'static, CadApp>, FakeDialogs) {
    let mut h = app();
    let fake = FakeDialogs::default();
    h.state_mut().files = FileOps::with_backend(Box::new(fake.clone()));
    (h, fake)
}

/// 線分を 1 本引く（図面が未保存になる）。
fn draw_a_line(h: &mut Harness<'_, CadApp>) {
    hover(h, P1);
    type_text(h, "L");
    press(h, egui::Key::Enter);
    click(h, P1);
    click(h, P2);
    press(h, egui::Key::Escape);
    assert_eq!(lines(h).len(), 1, "前提: 線が 1 本ある");
    assert!(h.state().doc.is_dirty(), "前提: 未保存");
}

/// 線分を `n` 本含む図面を `path` に書く（開く対象）。
fn write_drawing(path: &Path, n: usize) {
    let mut doc = Document::new();
    for i in 0..n {
        let y = f64::from(u32::try_from(i).expect("小さい"));
        doc.apply(Box::new(AddEntities::one(
            "TEST",
            Entity::new(
                Geometry::Line(Line::new(Point2::new(0.0, y), Point2::new(10.0, y))),
                LayerId::ZERO,
            ),
        )))
        .expect("追加できる");
    }
    native::write::write_to_file(&doc, path).expect("書ける");
}

fn ctrl(h: &mut Harness<'_, CadApp>, k: egui::Key) {
    frame(h, key_with(k, egui::Modifiers::CTRL));
    settle(h);
}

fn ctrl_shift(h: &mut Harness<'_, CadApp>, k: egui::Key) {
    frame(
        h,
        key_with(k, egui::Modifiers::CTRL.plus(egui::Modifiers::SHIFT)),
    );
    settle(h);
}

/// ウィンドウの ✕ ボタン（終了の要求）。
fn request_close(h: &mut Harness<'_, CadApp>) {
    h.input_mut()
        .viewports
        .entry(egui::ViewportId::ROOT)
        .or_default()
        .events
        .push(egui::ViewportEvent::Close);
    h.step();
    settle(h);
}

fn lines_of(h: &Harness<'_, CadApp>, kind: LineKind) -> Vec<String> {
    h.state()
        .session
        .cmdline
        .history()
        .filter(|l| l.kind == kind)
        .map(|l| l.text.clone())
        .collect()
}

fn waiting_is_shown(h: &Harness<'_, CadApp>) -> bool {
    h.query_by_label("ファイルを選んでいます…").is_some()
}

/// 開くのダイアログを待っている間もフレームは回り、作図・別のファイル操作・終了を受け付けない。
/// 選ばれたら開く。
#[test]
fn the_app_stays_modal_while_the_open_dialog_is_up_and_opens_the_pick() {
    let dir = crate::test_util::TempDir::new("dlg_open");
    let path = dir.join("other.ymc");
    write_drawing(&path, 2);
    let (mut h, fake) = app_with_fake_dialogs();

    ctrl(&mut h, egui::Key::O);
    assert_eq!(
        fake.opened(),
        vec![DialogRequest::Open],
        "ダイアログを開いた"
    );
    assert!(h.state().files.is_choosing_file(), "結果を待っている");
    assert!(waiting_is_shown(&h), "待っていることが画面に出る");

    // フレームは回り続ける（UI スレッドがダイアログで止まっていない）。
    for _ in 0..10 {
        settle(&mut h);
    }
    assert!(h.state().files.is_choosing_file(), "まだ待っている");

    // 作図を受け付けない（コマンド名・クリック・F8）。
    hover(&mut h, P1);
    type_text(&mut h, "L");
    press(&mut h, egui::Key::Enter);
    click(&mut h, P1);
    click(&mut h, P2);
    press(&mut h, egui::Key::F8);
    assert!(!h.state().session.has_active_tool(), "LINE が始まらない");
    assert!(lines(&h).is_empty(), "線が引かれない");
    assert!(input_lines(&h).is_empty(), "履歴に入力が残らない");
    assert_eq!(
        h.state().session.cmdline.input(),
        "",
        "入力欄に文字が入らない"
    );
    assert!(!h.state().drafting.is_on(Mode::Ortho), "F8 は効かない");

    // 別のファイル操作を受け付けない。
    ctrl(&mut h, egui::Key::N);
    ctrl(&mut h, egui::Key::S);
    ctrl_shift(&mut h, egui::Key::S);
    ctrl(&mut h, egui::Key::O);
    assert_eq!(fake.opened().len(), 1, "別のダイアログを開かない");

    // 終了を受け付けない。
    request_close(&mut h);
    assert!(!h.state().quitting, "終了しない");
    assert!(h.state().files.is_choosing_file(), "待ち続ける");
    assert!(
        lines_of(&h, LineKind::Error)
            .iter()
            .any(|l| l.contains("終了できません")),
        "終了できない理由が出る"
    );

    assert!(fake.reply(DialogReply::Picked(path.clone())));
    settle(&mut h);
    assert!(!h.state().files.is_modal(), "待つのが終わる");
    assert!(!waiting_is_shown(&h));
    assert_eq!(lines(&h).len(), 2, "選んだ図面が開かれた");
    assert_eq!(h.state().doc.path(), Some(path.as_path()));
    assert!(lines_of(&h, LineKind::Info)
        .iter()
        .any(|l| l.contains("開きました")));

    // 閉じた後はまた作図できる。
    hover(&mut h, P1);
    type_text(&mut h, "L");
    press(&mut h, egui::Key::Enter);
    assert!(h.state().session.has_active_tool(), "キーが戻る");
}

/// 開くのダイアログをキャンセルしたら何も起きない。
#[test]
fn cancelling_the_open_dialog_changes_nothing() {
    let (mut h, fake) = app_with_fake_dialogs();
    let history = h.state().session.cmdline.history().count();
    ctrl(&mut h, egui::Key::O);
    assert!(fake.reply(DialogReply::Cancelled));
    settle(&mut h);
    assert!(!h.state().files.is_modal());
    assert!(h.state().doc.path().is_none(), "図面はそのまま");
    assert_eq!(
        h.state().session.cmdline.history().count(),
        history,
        "何も出さない"
    );
}

/// ダイアログを出せなかったら、黙らずに理由と対処を出す。図面はそのまま。
#[test]
fn an_unavailable_dialog_reports_the_reason_and_the_remedy() {
    let (mut h, fake) = app_with_fake_dialogs();
    ctrl(&mut h, egui::Key::O);
    assert!(fake.reply(DialogReply::Unavailable(
        "Failed to pick file with zenity: No such file or directory (os error 2)".into()
    )));
    settle(&mut h);
    assert!(!h.state().files.is_modal(), "待ち続けない");
    let errors = lines_of(&h, LineKind::Error);
    let msg = errors.last().expect("エラーが出る");
    assert!(msg.contains("os error 2"), "理由: {msg}");
    assert!(msg.contains("sudo apt install zenity"), "対処: {msg}");
}

/// 名前を付けて保存: 拡張子を省いたら `.ymc`、`.dxf` なら DXF（今までどおり）。
#[test]
fn save_as_keeps_the_extension_rules() {
    let dir = crate::test_util::TempDir::new("dlg_saveas");
    let (mut h, fake) = app_with_fake_dialogs();
    draw_a_line(&mut h);

    ctrl_shift(&mut h, egui::Key::S);
    assert_eq!(
        fake.opened(),
        vec![DialogRequest::Save {
            default_name: "drawing.ymc".into()
        }]
    );
    assert!(h.state().files.is_choosing_file());
    assert!(fake.reply(DialogReply::Picked(dir.join("plan"))));
    settle(&mut h);
    let ymc = dir.join("plan.ymc");
    assert!(
        native::read::read_from_file(&ymc).is_ok(),
        "拡張子を補ってネイティブ形式で保存"
    );
    assert_eq!(h.state().doc.path(), Some(ymc.as_path()));
    assert!(!h.state().doc.is_dirty());
    assert_eq!(h.state().session.selection.len(), 0, "前提: 選択なし");

    ctrl_shift(&mut h, egui::Key::S);
    assert_eq!(
        fake.opened()[1],
        DialogRequest::Save {
            default_name: "plan.ymc".into()
        },
        "今のファイル名を出す"
    );
    assert!(fake.reply(DialogReply::Picked(dir.join("plan.dxf"))));
    settle(&mut h);
    assert!(
        cad_core::dxf::read::read_from_file(&dir.join("plan.dxf")).is_ok(),
        "DXF で保存"
    );
}

/// 名前を付けて保存をキャンセルしたら何も保存しない。
#[test]
fn cancelling_save_as_saves_nothing() {
    let (mut h, fake) = app_with_fake_dialogs();
    draw_a_line(&mut h);
    ctrl_shift(&mut h, egui::Key::S);
    assert!(fake.reply(DialogReply::Cancelled));
    settle(&mut h);
    assert!(!h.state().files.is_modal());
    assert!(h.state().doc.is_dirty(), "未保存のまま");
    assert!(h.state().doc.path().is_none());
    assert_eq!(lines(&h).len(), 1, "図面はそのまま");
}

/// 未保存の確認 →「保存する」→ 名前を付けて保存 → 元の操作（開く）へ続く。
#[test]
fn confirm_then_save_as_then_open_runs_as_a_chain() {
    let dir = crate::test_util::TempDir::new("dlg_chain");
    let other = dir.join("other.ymc");
    write_drawing(&other, 3);
    let (mut h, fake) = app_with_fake_dialogs();
    draw_a_line(&mut h);

    ctrl(&mut h, egui::Key::O);
    assert!(h.state().files.is_confirming(), "未保存の確認が出る");
    assert!(fake.opened().is_empty(), "まだダイアログは開かない");

    press(&mut h, egui::Key::Enter);
    assert!(!h.state().files.is_confirming(), "確認は閉じる");
    assert!(h.state().files.is_choosing_file(), "保存先を選んでいる");
    assert!(matches!(fake.opened()[..], [DialogRequest::Save { .. }]));

    assert!(fake.reply(DialogReply::Picked(dir.join("mine"))));
    settle(&mut h);
    let saved = native::read::read_from_file(&dir.join("mine.ymc")).expect("保存された");
    assert_eq!(saved.entities().len(), 1, "変更ごと保存");
    assert_eq!(
        fake.opened().get(1),
        Some(&DialogRequest::Open),
        "保存できたら開くへ続く"
    );
    assert!(h.state().files.is_choosing_file(), "開く図面を選んでいる");
    assert!(lines_of(&h, LineKind::Info)
        .iter()
        .any(|l| l.contains("保存しました")));

    assert!(fake.reply(DialogReply::Picked(other.clone())));
    settle(&mut h);
    assert!(!h.state().files.is_modal());
    assert_eq!(lines(&h).len(), 3, "選んだ図面が開かれた");
    assert_eq!(h.state().doc.path(), Some(other.as_path()));
}

/// 確認から来た保存をキャンセルしたら、確認へ戻る（今までどおり）。「保存しない」も選べる。
#[test]
fn cancelling_the_save_from_the_confirmation_returns_to_it() {
    let (mut h, fake) = app_with_fake_dialogs();
    draw_a_line(&mut h);
    ctrl(&mut h, egui::Key::N);
    press(&mut h, egui::Key::Enter);
    assert!(
        h.state().files.is_choosing_file(),
        "前提: 保存先を選んでいる"
    );

    assert!(fake.reply(DialogReply::Cancelled));
    settle(&mut h);
    assert!(h.state().files.is_confirming(), "確認へ戻る");
    assert!(!h.state().files.is_choosing_file());
    assert_eq!(lines(&h).len(), 1, "図面は捨てない");

    let discard = h.get_by_label("保存しない").rect().center();
    click(&mut h, discard);
    assert!(!h.state().files.is_modal());
    assert!(lines(&h).is_empty(), "保存しないで新規図面へ");
}

/// 確認から来た保存でダイアログを出せなかったら、理由を出して止まる。図面は捨てない。
#[test]
fn an_unavailable_save_dialog_from_the_confirmation_keeps_the_drawing() {
    let (mut h, fake) = app_with_fake_dialogs();
    draw_a_line(&mut h);
    ctrl(&mut h, egui::Key::N);
    press(&mut h, egui::Key::Enter);
    assert!(fake.reply(DialogReply::Unavailable("os error 2".into())));
    settle(&mut h);
    assert!(!h.state().files.is_modal());
    assert_eq!(lines(&h).len(), 1, "新規図面へ進まない");
    assert!(h.state().doc.is_dirty());
    assert!(lines_of(&h, LineKind::Error)
        .iter()
        .any(|l| l.contains("zenity")));
}

/// ✕ → 確認 →「保存する」→ 名前を付けて保存 → 終了。
#[test]
fn quitting_saves_through_the_dialog_then_quits() {
    let dir = crate::test_util::TempDir::new("dlg_quit");
    let (mut h, fake) = app_with_fake_dialogs();
    draw_a_line(&mut h);
    request_close(&mut h);
    assert!(h.state().files.is_confirming(), "前提: 未保存の確認");
    press(&mut h, egui::Key::Enter);
    assert!(!h.state().quitting, "保存先を選ぶまで終わらない");
    assert!(fake.reply(DialogReply::Picked(dir.join("last.ymc"))));
    settle(&mut h);
    assert!(dir.join("last.ymc").exists(), "保存してから");
    assert!(h.state().quitting, "終了する");
}

/// 「待つのをやめる」でモーダルを抜けられる。その後に届いた結果は使わない。
#[test]
fn giving_up_the_wait_ignores_a_late_pick() {
    let dir = crate::test_util::TempDir::new("dlg_abandon");
    let path = dir.join("late.ymc");
    write_drawing(&path, 2);
    let (mut h, fake) = app_with_fake_dialogs();
    ctrl(&mut h, egui::Key::O);
    let button = h.get_by_label("待つのをやめる").rect().center();
    click(&mut h, button);
    assert!(!h.state().files.is_modal(), "モーダルを抜ける");
    assert!(
        !fake.reply(DialogReply::Picked(path)),
        "受け手はもう待っていない"
    );
    settle(&mut h);
    assert!(lines(&h).is_empty(), "遅れて届いた結果では開かない");
    assert!(h.state().doc.path().is_none());
}
