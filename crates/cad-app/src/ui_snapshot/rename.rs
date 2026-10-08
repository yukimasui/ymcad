//! レイヤ名の改名（Issue #68）の見た目。ダブルクリック直後は名前全体が選ばれていて、
//! 打った文字は改名の欄に入る（コマンドラインは空のまま）。同名のレイヤにしようとすると
//! 欄は開いたままで、案内が出る。

use cad_core::command::AddLayer;
use cad_core::AciColor;
use egui_kittest::kittest::Queryable as _;
use egui_kittest::Harness;

use super::{harness, hover, press, shot, type_text, CANVAS_CENTER, STEPS};
use crate::app::CadApp;

/// レイヤパネルの名前 `name` をダブルクリックする。1 クリックを 1 フレームに収める
/// （egui_kittest のフレームは 0.25 秒進み、別フレームだとダブルクリックの間隔を越える）。
fn double_click(h: &mut Harness<'_, CadApp>, name: &str) {
    let pos = h
        .query_all_by_role_and_label(egui::accesskit::Role::Button, name)
        .next()
        .expect("レイヤパネルの名前")
        .rect()
        .center();
    let button = |pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    h.input_mut()
        .events
        .extend([egui::Event::PointerMoved(pos), button(true), button(false)]);
    h.step();
    h.input_mut().events.extend([button(true), button(false)]);
    h.step();
    h.run_steps(STEPS);
}

#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_layer_rename() {
    let mut h = harness();
    let (doc, _) = h.state_mut().parts_mut();
    doc.apply(Box::new(AddLayer::new("壁", AciColor::RED)))
        .expect("レイヤ");
    type_text(&mut h, "LA");
    press(&mut h, egui::Key::Enter);
    double_click(&mut h, "壁");
    // マウスを作図領域へ逃がして欄を見えるようにする。カーソル横のコマンドラインが空のまま
    // であることも写る（乗せるだけではフォーカスは動かない）。
    hover(&mut h, CANVAS_CENTER);
    shot(&mut h, "rename_a_whole_name_selected");
    type_text(&mut h, "LINE");
    shot(&mut h, "rename_b_typed_into_the_field");

    // 同名のレイヤ（0）にしようとして Enter → 欄は開いたまま、打った文字も残り、
    // コマンドラインに利用者向けの言葉で案内が出る。
    press(&mut h, egui::Key::Escape);
    double_click(&mut h, "壁");
    hover(&mut h, CANVAS_CENTER);
    type_text(&mut h, "0");
    press(&mut h, egui::Key::Enter);
    shot(&mut h, "rename_c_duplicate_keeps_the_field");
}
