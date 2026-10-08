//! プロパティパネル（Issue #31 段階 1）の振る舞い。
//!
//! 開閉（コマンド・リボン・Ctrl+1）、レイヤの変更、コマンド実行中は表示だけ、を
//! 画面なしで動かして固定する。値の並びや要約は `properties.rs` の単体テスト。

use egui_kittest::kittest::Queryable as _;

use super::*;
use crate::properties::{BUSY_NOTE, EMPTY_NOTE, MIXED_LAYER};
use cad_core::command::{AddEntities, AddLayer, SetLayerProperties};
use cad_core::geom::Line;
use cad_core::{AciColor, Entity, EntityId, Geometry, LayerId};

fn open(h: &Harness<'_, CadApp>) -> bool {
    h.state().properties_panel.is_open()
}

fn ctrl_1() -> [egui::Event; 2] {
    key_with(egui::Key::Num1, egui::Modifiers::CTRL)
}

fn press_ctrl_1(h: &mut Harness<'_, CadApp>) {
    frame(h, ctrl_1());
    settle(h);
}

/// パネルからの変更と同じ入口で図面を変える。
fn external(h: &mut Harness<'_, CadApp>, cmd: Box<dyn cad_core::Command>) {
    let app = h.state_mut();
    app.session.apply_external(cmd, &mut app.doc);
    settle(h);
}

fn layer(h: &Harness<'_, CadApp>, name: &str) -> LayerId {
    h.state()
        .doc
        .layers()
        .by_name(name)
        .unwrap_or_else(|| panic!("レイヤ {name} が無い"))
}

/// レイヤ `L1`（普通）、`LOCKED`（ロック）、`HIDDEN`（非表示）を足す。
fn add_layers(h: &mut Harness<'_, CadApp>) {
    for name in ["L1", "LOCKED", "HIDDEN"] {
        external(h, Box::new(AddLayer::new(name, AciColor::WHITE)));
    }
    let (locked, hidden) = (layer(h, "LOCKED"), layer(h, "HIDDEN"));
    external(h, Box::new(SetLayerProperties::new(locked).locked(true)));
    external(h, Box::new(SetLayerProperties::new(hidden).visible(false)));
}

/// 線分を `layer` に足して ID を返す。
fn add_line(h: &mut Harness<'_, CadApp>, layer: LayerId, y: f64) -> EntityId {
    let line = Geometry::Line(Line::new(Point2::new(0.0, y), Point2::new(100.0, y)));
    external(
        h,
        Box::new(AddEntities::one("LINE", Entity::new(line, layer))),
    );
    h.state()
        .doc
        .entities()
        .ids()
        .last()
        .expect("足した図形がある")
}

/// 画面にその文字列のラベルが 1 つ以上ある。
fn has(h: &Harness<'_, CadApp>, text: &str) -> bool {
    h.query_all_by_label(text).next().is_some()
}

/// レイヤのドロップダウンの表示（選んでいるレイヤ名、または「（混在）」）。
fn dropdown_text(h: &Harness<'_, CadApp>) -> String {
    h.get_by_role(egui::accesskit::Role::ComboBox)
        .value()
        .expect("ドロップダウンは選択中の文字を持つ")
}

/// ドロップダウンが押せない状態か（kittest の `Node` は公開の取り出し口が無いので Debug 表示で見る）。
fn dropdown_disabled(h: &Harness<'_, CadApp>) -> bool {
    format!("{:?}", h.get_by_role(egui::accesskit::Role::ComboBox)).contains("disabled: true")
}

fn select(h: &mut Harness<'_, CadApp>, ids: &[EntityId]) {
    let sel = &mut h.state_mut().session.selection;
    sel.clear();
    for id in ids {
        sel.insert(*id);
    }
    settle(h);
}

fn layers_of(h: &Harness<'_, CadApp>, ids: &[EntityId]) -> Vec<LayerId> {
    ids.iter()
        .map(|id| h.state().doc.entities().get(*id).expect("図形").layer)
        .collect()
}

/// パネルを開いて、レイヤを `ids` の 3 本に割り当てた状態を返す。
/// 3 本とも普通のレイヤ `L1` / `0` にあり、選択はしていない。
struct Scene {
    h: Harness<'static, CadApp>,
    ids: Vec<EntityId>,
}

fn scene() -> Scene {
    let mut h = app();
    add_layers(&mut h);
    let l1 = layer(&h, "L1");
    let ids = vec![
        add_line(&mut h, LayerId::ZERO, 10.0),
        add_line(&mut h, LayerId::ZERO, 20.0),
        add_line(&mut h, l1, 30.0),
    ];
    press_ctrl_1(&mut h);
    assert!(open(&h), "前提: Ctrl+1 でパネルが開く");
    Scene { h, ids }
}

/// ドロップダウンを開いて `label` の項目を押す。
fn choose_layer(h: &mut Harness<'_, CadApp>, label: &str) {
    h.get_by_role(egui::accesskit::Role::ComboBox).click();
    settle(h);
    if let Some(item) = h.query_all_by_label(label).last() {
        item.click();
    }
    settle(h);
}

// ---- 開閉 ---------------------------------------------------------------------

/// Ctrl+1 で開閉する。どちらも履歴には何も残さない。
#[test]
fn ctrl_1_toggles_the_panel() {
    for on in [false, true] {
        let mut h = app_with_dynamic(on);
        hover(&mut h, P1);
        assert!(!open(&h));
        press_ctrl_1(&mut h);
        assert!(open(&h), "開く（動的入力 {on}）");
        press_ctrl_1(&mut h);
        assert!(!open(&h), "閉じる（動的入力 {on}）");
        assert!(
            input_lines(&h).is_empty(),
            "履歴に残さない（動的入力 {on}）"
        );
    }
}

/// 日本語の変換中は Ctrl+1 を奪わない。確定か取り消した後なら効く。
#[test]
fn ctrl_1_does_nothing_while_composing() {
    for on in [false, true] {
        let mut h = app_with_dynamic(on);
        hover(&mut h, P1);
        frame(&mut h, [preedit("に")]);
        settle(&mut h);
        assert!(h.state().session.cmdline.is_composing(), "前提: 変換中");

        press_ctrl_1(&mut h);
        assert!(!open(&h), "変換中は開かない（動的入力 {on}）");

        frame(&mut h, [preedit("")]);
        settle(&mut h);
        assert!(
            !h.state().session.cmdline.is_composing(),
            "前提: 変換が終わった"
        );
        press_ctrl_1(&mut h);
        assert!(open(&h), "終わった後は開く（動的入力 {on}）");
    }
}

/// パネルの入力欄にフォーカスがあるときは Ctrl+1 を奪わない（ADR-0035）。
#[test]
fn ctrl_1_does_nothing_while_a_panel_field_has_focus() {
    let mut h = app();
    focus_layer_name_field(&mut h);
    press_ctrl_1(&mut h);
    assert!(!open(&h), "レイヤパネルの入力欄を編集中は開かない");
}

/// モーダル（未保存確認）が出ている間は Ctrl+1 を奪わない。
#[test]
fn ctrl_1_does_nothing_while_the_modal_is_open() {
    let (mut h, _dir) = app_with_unsaved_modal(true, "ctrl1");
    press_ctrl_1(&mut h);
    assert!(!open(&h), "モーダル表示中は開かない");
    assert!(h.state().files.is_confirming(), "モーダルはそのまま");
}

/// リボンのボタンとコマンド名（PR / PROPERTIES）でも開閉する。
#[test]
fn ribbon_button_and_command_toggle_the_panel() {
    let mut h = app();
    hover(&mut h, P1);
    press_ribbon(&mut h, "PROPERTIES");
    assert!(open(&h), "ボタンで開く");
    assert_eq!(input_lines(&h), vec!["> PROPERTIES"]);
    press_ribbon(&mut h, "PROPERTIES");
    assert!(!open(&h), "もう一度で閉じる");

    type_text(&mut h, "PR");
    press(&mut h, egui::Key::Enter);
    assert!(open(&h), "PR で開く");
    type_text(&mut h, "properties");
    press(&mut h, egui::Key::Enter);
    assert!(!open(&h), "PROPERTIES で閉じる");
}

/// 開閉は実行中のコマンドを中断しない（LINE の 1 点目も、打ちかけの文字も残る）。
#[test]
fn toggling_keeps_the_running_line() {
    for via_key in [true, false] {
        let mut h = app();
        hover(&mut h, P1);
        type_text(&mut h, "L");
        press(&mut h, egui::Key::Enter);
        click(&mut h, P1);
        let first = h.state().session.last_point();
        assert!(first.is_some(), "前提: LINE の 1 点目がある");
        let history = input_lines(&h);

        if via_key {
            press_ctrl_1(&mut h);
        } else {
            press_ribbon(&mut h, "PROPERTIES");
        }
        assert!(open(&h), "開く（キー {via_key}）");
        assert_eq!(
            h.state().session.active_command(),
            Some("LINE"),
            "LINE は続く（キー {via_key}）"
        );
        assert_eq!(h.state().session.last_point(), first, "1 点目が残る");
        assert!(
            !h.state()
                .session
                .cmdline
                .history()
                .any(|l| l.text == "*取り消し*"),
            "中断の履歴が無い（キー {via_key}）"
        );
        let mut expected = history;
        if !via_key {
            expected.push("> PROPERTIES".to_owned());
        }
        assert_eq!(input_lines(&h), expected);

        // 続きを描ける。
        click(&mut h, P2);
        press(&mut h, egui::Key::Enter);
        assert_eq!(lines(&h).len(), 1, "線が引ける（キー {via_key}）");
    }
}

// ---- 表示 ---------------------------------------------------------------------

/// 選択が無ければ案内、1 つ選べば種類ごとの項目、複数なら個数と種類ごとの件数。
#[test]
fn panel_shows_a_note_values_or_a_summary() {
    let Scene { mut h, ids } = scene();
    assert!(has(&h, EMPTY_NOTE));

    select(&mut h, &ids[..1]);
    assert!(!has(&h, EMPTY_NOTE));
    assert!(has(&h, "線分"));
    assert!(has(&h, "長さ"));
    assert!(has(&h, "100.0000"), "終点 X と長さ");
    assert!(has(&h, "角度"));
    assert!(has(&h, "0.0000°"));

    select(&mut h, &ids);
    assert!(has(&h, "3 個を選択"));
    assert!(has(&h, "線分 3"));
    assert!(!has(&h, "長さ"), "複数選択では個々の値を出さない");
    assert_eq!(dropdown_text(&h), MIXED_LAYER, "レイヤがまたがる");
}

// ---- レイヤの変更 -------------------------------------------------------------

/// 複数選択でレイヤを変えると、選択全部がまとめて移り、Undo 1 回で全部戻る。
#[test]
fn changing_the_layer_moves_the_whole_selection_with_one_undo() {
    let Scene { mut h, ids } = scene();
    let before = layers_of(&h, &ids);
    let l1 = layer(&h, "L1");
    assert_ne!(before[0], l1, "前提: 移し先と違うレイヤの図形がある");
    select(&mut h, &ids);

    choose_layer(&mut h, "L1");
    assert_eq!(layers_of(&h, &ids), vec![l1; 3], "3 本とも L1 へ移る");
    assert_eq!(
        h.state().session.selection.len(),
        3,
        "L1 は普通のレイヤ。選択は残る"
    );
    assert_eq!(dropdown_text(&h), "L1", "共通のレイヤの名前が出る");

    type_text(&mut h, "U");
    press(&mut h, egui::Key::Enter);
    assert_eq!(
        layers_of(&h, &ids),
        before,
        "Undo 1 回で全部が元のレイヤへ戻る"
    );
}

/// 今のレイヤを選び直しても何もしない（Undo の履歴に空の操作を積まない）。
#[test]
fn choosing_the_current_layer_again_does_nothing() {
    let Scene { mut h, ids } = scene();
    select(&mut h, &ids[2..]);
    let undo_depth = h.state().doc.history().len();
    let dirty = h.state().doc.is_dirty();
    let before = layers_of(&h, &ids);
    choose_layer(&mut h, "L1");
    assert_eq!(h.state().doc.history().len(), undo_depth, "履歴は増えない");
    assert_eq!(h.state().doc.is_dirty(), dirty, "未保存の印も変わらない");
    assert_eq!(layers_of(&h, &ids), before, "レイヤは変わらない");
    assert_eq!(h.state().session.selection.len(), 1, "選択もそのまま");
}

/// レイヤパネルの「移動」の行で `name` のボタンを押す（行は一番下なので最後に見つかるラベル）。
fn press_move_button(h: &mut Harness<'_, CadApp>, name: &str) {
    let button = h
        .query_all_by_label(name)
        .last()
        .unwrap_or_else(|| panic!("「移動」の行に {name} のボタンが無い"));
    button.click();
    settle(h);
}

fn open_layer_panel(h: &mut Harness<'_, CadApp>) {
    if !h.state().layer_panel.is_open() {
        h.state_mut().layer_panel.toggle();
    }
    settle(h);
}

fn info_lines(h: &Harness<'_, CadApp>) -> Vec<String> {
    h.state()
        .session
        .cmdline
        .history()
        .filter(|l| l.kind == LineKind::Info)
        .map(|l| l.text.clone())
        .collect()
}

/// ロック・非表示のレイヤへ移すと選択から外れる。移し先の名前と理由つきの案内が、コマンドラインと
/// 開いている両方のパネルに出る。プロパティパネルのドロップダウンからでも、レイヤパネルの
/// 「移動」の行からでも同じ（案内は `Session::apply_external` が出す）。
fn moving_to_an_uneditable_layer_drops_the_selection_with_a_note(via_layer_panel: bool) {
    for (target, label, note) in [
        (
            "LOCKED",
            "LOCKED（ロック中）",
            "2 個を「LOCKED」へ移しました。ロック中のレイヤなので選択から外れます（U で戻せます）",
        ),
        (
            "HIDDEN",
            "HIDDEN（非表示）",
            "2 個を「HIDDEN」へ移しました。非表示のレイヤなので画面から消え、選択から外れます（U で戻せます）",
        ),
    ] {
        let Scene { mut h, ids } = scene();
        // ドロップダウンはレイヤパネルの線種の欄と区別がつかなくなるので、レイヤパネルは後で開く。
        if via_layer_panel {
            open_layer_panel(&mut h);
        }
        select(&mut h, &ids[..2]);
        assert!(!has(&h, note), "{target}: 前提: 移す前は案内が無い");
        if via_layer_panel {
            press_move_button(&mut h, target);
        } else {
            choose_layer(&mut h, label);
            open_layer_panel(&mut h);
        }

        let dest = layer(&h, target);
        assert_eq!(layers_of(&h, &ids[..2]), vec![dest; 2], "{target}: 移る");
        assert!(
            h.state().session.selection.is_empty(),
            "{target}: 選択から外れる"
        );
        assert!(has(&h, EMPTY_NOTE));
        assert!(
            info_lines(&h).iter().any(|t| t == note),
            "{target}: コマンドラインに案内が出る: {:?}",
            info_lines(&h)
        );
        assert_eq!(
            h.query_all_by_label(note).count(),
            3,
            "{target}: コマンドラインの履歴に加えて、プロパティとレイヤの両方のパネルにも出る"
        );

        // 選び直したら古い案内は消える（次の選択まで出し続けるだけ）。
        select(&mut h, &ids[2..]);
        assert!(h.state().session.drop_note().is_none());
        assert_eq!(
            h.query_all_by_label(note).count(),
            1,
            "{target}: 選び直したらパネルの案内は消える（履歴の 1 行だけ残る）"
        );

        type_text(&mut h, "U");
        press(&mut h, egui::Key::Enter);
        assert_eq!(
            layers_of(&h, &ids[..2]),
            vec![LayerId::ZERO; 2],
            "{target}: Undo で戻る"
        );
    }
}

/// ロック・非表示のレイヤへも移せる（プロパティパネルのドロップダウンから）。
#[test]
fn moving_to_a_locked_or_hidden_layer_drops_the_selection_with_a_note() {
    moving_to_an_uneditable_layer_drops_the_selection_with_a_note(false);
}

/// 同じことをレイヤパネルの「移動」の行からしても、同じ案内が出る
/// （修正前はプロパティパネルだけが、しかも曖昧な文言で案内していた）。
#[test]
fn moving_from_the_layer_panel_gives_the_same_note() {
    moving_to_an_uneditable_layer_drops_the_selection_with_a_note(true);
}

/// 普通のレイヤへ移したときは案内を出さない（選択も残る）。
#[test]
fn moving_to_an_ordinary_layer_gives_no_note() {
    let Scene { mut h, ids } = scene();
    open_layer_panel(&mut h);
    select(&mut h, &ids[..2]);
    let infos = info_lines(&h);
    press_move_button(&mut h, "L1");
    assert_eq!(h.state().session.selection.len(), 2, "選択は残る");
    assert_eq!(info_lines(&h), infos, "案内は増えない");
    assert!(h.state().session.drop_note().is_none());
}

// ---- コマンド実行中は表示だけ -----------------------------------------------------

/// コマンド実行中（選択待ちを含む）は案内が出て、レイヤは変えられない。
/// 終わればまた変えられる。
#[test]
fn the_panel_is_display_only_while_a_command_runs() {
    let Scene { mut h, ids } = scene();
    select(&mut h, &ids[..2]);
    hover(&mut h, P1);
    assert!(!has(&h, BUSY_NOTE), "前提: 待機中は案内が出ない");

    // 点の入力待ち（LINE）と、選択待ち（ERASE。選択を空にして始め、選択待ちの間に選び直す）。
    for name in ["LINE", "ERASE"] {
        let undo_depth = h.state().doc.history().len();
        let before = layers_of(&h, &ids);
        select(&mut h, &[]);
        type_text(&mut h, name);
        press(&mut h, egui::Key::Enter);
        assert!(
            h.state().session.active_command().is_some(),
            "前提: {name} 実行中"
        );
        select(&mut h, &ids[..2]);
        assert!(has(&h, BUSY_NOTE), "{name}: 案内が出る");
        assert!(dropdown_disabled(&h), "{name}: ドロップダウンは押せない");

        choose_layer(&mut h, "L1");
        assert_eq!(layers_of(&h, &ids), before, "{name}: 変わらない");
        assert_eq!(
            h.state().doc.history().len(),
            undo_depth,
            "{name}: 履歴も増えない"
        );

        press(&mut h, egui::Key::Escape);
        assert!(h.state().session.active_command().is_none());
        assert!(!has(&h, BUSY_NOTE), "{name}: 終われば案内が消える");
    }

    // 終わった後は変えられる（上の検査が、操作の失敗で偶然通ったのではないことの対照）。
    select(&mut h, &ids[..2]);
    assert!(!dropdown_disabled(&h), "終われば押せる");
    choose_layer(&mut h, "L1");
    let l1 = layer(&h, "L1");
    assert_eq!(layers_of(&h, &ids[..2]), vec![l1; 2]);
}

/// コマンド実行中（選択待ちを含む）は、レイヤパネルの「移動」の行も押せない。プロパティパネルと
/// 同じ案内が出る。修正前は「移動」の行だけ押せて、実行中のコマンドの下でレイヤが変わった。
#[test]
fn the_layer_panels_move_row_is_disabled_while_a_command_runs() {
    let Scene { mut h, ids } = scene();
    open_layer_panel(&mut h);
    assert!(
        BUSY_NOTE.contains("中断すると選択も外れます"),
        "案内に、中断すると選択も外れることを添える"
    );
    // MOVE（基点待ち。選択済みで始める）と ERASE（選択待ち。選択を空にして始め、待つ間に選び直す）。
    for (name, select_first) in [("MOVE", true), ("ERASE", false)] {
        let before = layers_of(&h, &ids);
        let undo_depth = h.state().doc.history().len();
        select(&mut h, if select_first { &ids[..2] } else { &[] });
        hover(&mut h, P1);
        type_text(&mut h, name);
        press(&mut h, egui::Key::Enter);
        assert_eq!(h.state().session.active_command(), Some(name), "前提");
        select(&mut h, &ids[..2]);
        assert_eq!(
            h.query_all_by_label(BUSY_NOTE).count(),
            2,
            "{name}: 両方のパネルに案内が出る"
        );

        press_move_button(&mut h, "L1");
        assert_eq!(layers_of(&h, &ids), before, "{name}: レイヤは変わらない");
        assert_eq!(
            h.state().doc.history().len(),
            undo_depth,
            "{name}: 履歴も同じ"
        );
        assert_eq!(
            h.state().session.active_command(),
            Some(name),
            "{name}: 続く"
        );

        press(&mut h, egui::Key::Escape);
        assert!(!has(&h, BUSY_NOTE), "{name}: 終われば案内が消える");
    }

    // 対照: 終われば押せる。
    select(&mut h, &ids[..2]);
    press_move_button(&mut h, "L1");
    let l1 = layer(&h, "L1");
    assert_eq!(layers_of(&h, &ids[..2]), vec![l1; 2], "終われば移せる");
}

/// 右側のパネルを 3 枚開いても、作図領域は最小幅を保ち、どのパネルも中身が隣へはみ出さない。
/// 修正前は 1280px で作図領域が約 60px になった（レイヤ・コンポーネント各 460px ＋ プロパティ 300px）。
#[test]
fn three_side_panels_leave_the_canvas_its_minimum_width() {
    let Scene { mut h, ids } = scene();
    let wide = h.state().viewport.rect().width();
    open_layer_panel(&mut h);
    let with_layers = h.state().viewport.rect().width();
    assert!(with_layers < wide, "前提: パネルを開くと作図領域は狭まる");

    h.state_mut().component_panel.toggle();
    select(&mut h, &ids[..1]);
    assert!(open(&h), "前提: 3 枚目まで開いた");
    assert!(h.state().layer_panel.is_open() && h.state().component_panel.is_open());
    let canvas = h.state().viewport.rect();
    assert!(
        canvas.width() >= crate::app::MIN_CANVAS_WIDTH - 1.0,
        "作図領域が最小幅を保つ: {}px",
        canvas.width()
    );
    assert!(
        canvas.width() < 800.0,
        "前提: 3 枚開いたので、確かに狭まっている: {}px",
        canvas.width()
    );

    // 内側のパネル（プロパティ）が、最小幅も取れずに隣へはみ出していない。
    // 項目名の欄はパネルの左端に置かれるので、作図領域の右端より右にあるはず。
    let label = h.get_by_label("長さ").rect();
    assert!(
        label.left() >= canvas.right(),
        "プロパティの中身が作図領域に食い込んでいる: 項目名 {label:?} / 作図領域 {canvas:?}"
    );
}

/// 狭い画面（1024px）でも、3 枚開いて作図領域の最小幅が残る。
#[test]
fn the_canvas_minimum_width_holds_on_a_narrow_screen() {
    let mut h = app_with_width(1024.0);
    open_layer_panel(&mut h);
    h.state_mut().component_panel.toggle();
    press_ctrl_1(&mut h);
    assert!(open(&h) && h.state().layer_panel.is_open() && h.state().component_panel.is_open());
    let canvas = h.state().viewport.rect().width();
    assert!(
        canvas >= crate::app::MIN_CANVAS_WIDTH - 1.0,
        "作図領域が最小幅を保つ: {canvas}px"
    );
}

/// 要約のキャッシュは、同じ図面・同じ選択のフレームでは作り直さない。
/// 図面か選択が変わると作り直す。
#[test]
fn the_summary_is_cached_across_frames() {
    let Scene { mut h, ids } = scene();
    select(&mut h, &ids);
    let n = h.state().properties_panel.summary_recomputed();
    h.run_steps(10);
    assert_eq!(
        h.state().properties_panel.summary_recomputed(),
        n,
        "変化が無ければ数え直さない"
    );
    select(&mut h, &ids[..1]);
    assert!(
        h.state().properties_panel.summary_recomputed() > n,
        "選択が変わった"
    );
}
