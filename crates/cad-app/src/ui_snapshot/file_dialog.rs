//! ファイルダイアログを待っている間のモーダルと、出せなかったときの案内（Issue #98）の見た目。

use egui_kittest::Harness;

use super::{harness, shot, STEPS};
use crate::app::CadApp;
use crate::file_dialog::{DialogReply, FakeDialogs};
use crate::file_ops::FileOps;

fn ctrl_o(h: &mut Harness<'_, CadApp>) {
    for pressed in [true, false] {
        h.event(egui::Event::Key {
            key: egui::Key::O,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::CTRL,
        });
    }
    h.run_steps(STEPS);
}

/// 「開く」のダイアログを待っている間。「ファイルを選んでいます…」のモーダルが出る。
/// 次に、出せなかったときの案内がコマンドラインに出る。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_file_dialog_waiting_and_unavailable() {
    let mut h = harness();
    let fake = FakeDialogs::default();
    h.state_mut()
        .set_file_ops(FileOps::with_backend(Box::new(fake.clone())));
    ctrl_o(&mut h);
    shot(&mut h, "file_dialog_waiting");

    assert!(fake.reply(DialogReply::Unavailable(
        "Failed to pick file with zenity: No such file or directory (os error 2)".into()
    )));
    h.run_steps(STEPS);
    shot(&mut h, "file_dialog_unavailable");
}
