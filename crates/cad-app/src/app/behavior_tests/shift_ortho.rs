//! Shift で直交モードを一時的に反転する（Issue #36、ADR-0049）の振る舞い。
//!
//! 反転は `CadApp::track` → `drafting::track_cursor` の 1 本の経路（ADR-0038）に載るので、
//! ラバーバンド・クリック・直接距離入力の向きがそろうこと、効くのは基準点のある点の指定中
//! （グリップを掴んでいる間を含む）だけで、待機中・選択待ちの Shift は今までどおりであること、
//! 変換中・パネルの欄の編集中・モーダル中は反転しないこと、をアプリ全体で固定する。
//! 反転の表（4 通りの設定）と優先順位の純粋な部分は `drafting` の単体テスト。

use cad_core::command::AddEntities;
use cad_core::geom::{Circle, Line, Point2};
use cad_core::{Entity, EntityId, Geometry, LayerId};
use egui_kittest::kittest::Queryable as _;
use egui_kittest::Harness;

use super::{
    app, app_with_dynamic, app_with_unsaved_modal, app_with_width, assert_point, click, cursor,
    focus_layer_name_field, frame, hover, line_from_p1_with, lines, model, preedit, press,
    screen_at, settle, type_text, CadApp, P1, P2, SCREEN,
};
use crate::drafting::{Mode, TEMPORARY_NOTE};

/// Shift を押す・離す（修飾キーの状態だけを変える。egui 0.36 は `ModifiersChanged` で持つ）。
fn hold_shift(h: &mut Harness<'_, CadApp>, down: bool) {
    let m = if down {
        egui::Modifiers::SHIFT
    } else {
        egui::Modifiers::NONE
    };
    frame(h, [egui::Event::ModifiersChanged(m)]);
    settle(h);
}

/// Shift を押したままクリックする（押す・離すを別フレームで）。Shift は押したまま残す。
fn shift_click(h: &mut Harness<'_, CadApp>, pos: egui::Pos2) {
    let button = |pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::SHIFT,
    };
    frame(
        h,
        [
            egui::Event::ModifiersChanged(egui::Modifiers::SHIFT),
            egui::Event::PointerMoved(pos),
            button(true),
        ],
    );
    frame(h, [button(false)]);
    settle(h);
}

fn flipping(h: &Harness<'_, CadApp>) -> bool {
    h.state().shift_flip_active()
}

/// 直交・極の切り替えが履歴に残った回数（Shift の反転は残さない）。
fn toggle_lines(h: &Harness<'_, CadApp>) -> usize {
    h.state()
        .session
        .cmdline
        .history()
        .filter(|l| l.text.starts_with("直交モード:") || l.text.starts_with("極トラッキング:"))
        .count()
}

/// パネルからの変更と同じ入口で図形を足し、ID を返す。
fn add_geom(h: &mut Harness<'_, CadApp>, geom: Geometry) -> EntityId {
    let app = h.state_mut();
    app.session.apply_external(
        Box::new(AddEntities::one("LINE", Entity::new(geom, LayerId::ZERO))),
        &mut app.doc,
    );
    settle(h);
    h.state().doc.entities().ids().last().expect("足した図形")
}

fn line_of(h: &Harness<'_, CadApp>, id: EntityId) -> Line {
    match h.state().doc.entities().get(id).map(|e| &e.geom) {
        Some(Geometry::Line(l)) => *l,
        other => panic!("線分のはず: {other:?}"),
    }
}

fn screen(h: &Harness<'_, CadApp>, at: Point2) -> egui::Pos2 {
    h.state().viewport.model_to_screen(at)
}

fn p(x: f64, y: f64) -> Point2 {
    Point2::new(x, y)
}

// ---- 押している間だけ・離すと戻る ------------------------------------------------

/// 設定がどちらもオフでも、Shift を押している間だけ直交が効く。離すと戻り、設定は変わらず履歴にも残らない。
#[test]
fn shift_turns_ortho_on_only_while_held() {
    let (mut h, base) = line_from_p1_with(&[]);
    let pos = screen_at(&h, base, 20.0, 200.0);
    hover(&mut h, pos);
    assert_point(cursor(&h), model(&h, pos), "前提: 押す前は自由");
    let before = toggle_lines(&h);

    hold_shift(&mut h, true);
    assert!(flipping(&h), "基準点のある点の指定中なので反転する");
    assert_point(
        cursor(&h),
        Point2::new(model(&h, pos).x, base.y),
        "押している間は水平",
    );
    assert!(
        !h.state().drafting.is_on(Mode::Ortho),
        "F8 の設定は書き換えない"
    );

    hold_shift(&mut h, false);
    assert!(!flipping(&h));
    assert_point(cursor(&h), model(&h, pos), "離すと自由に戻る");
    assert_eq!(toggle_lines(&h), before, "履歴に残さない");
}

/// ORTHO がオンなら、Shift を押している間だけ自由に指せる。
#[test]
fn shift_releases_ortho_while_held() {
    let (mut h, base) = line_from_p1_with(&[egui::Key::F8]);
    let pos = screen_at(&h, base, 20.0, 200.0);
    hover(&mut h, pos);
    assert_point(
        cursor(&h),
        Point2::new(model(&h, pos).x, base.y),
        "前提: 水平",
    );
    hold_shift(&mut h, true);
    assert_point(cursor(&h), model(&h, pos), "押している間は自由");
    assert!(h.state().drafting.is_on(Mode::Ortho), "設定はオンのまま");
    hold_shift(&mut h, false);
    assert_point(
        cursor(&h),
        Point2::new(model(&h, pos).x, base.y),
        "離すと水平に戻る",
    );
}

/// 4 通りの設定で表（ADR-0049）のとおりに効く。43° を 100px 先で指す。
/// 両方オフ → 直交 / ORTHO だけ → 自由 / POLAR だけ → 直交（直交が優先） / 両方 → 極（45°）。
#[test]
fn shift_follows_the_table_for_all_four_settings() {
    for (keys, expect) in [
        (&[][..], "ortho"),
        (&[egui::Key::F8][..], "free"),
        (&[egui::Key::F10][..], "ortho"),
        (&[egui::Key::F8, egui::Key::F10][..], "polar"),
    ] {
        let (mut h, base) = line_from_p1_with(keys);
        let pos = screen_at(&h, base, 43.0, 100.0);
        hover(&mut h, pos);
        hold_shift(&mut h, true);
        let raw = model(&h, pos);
        let got = cursor(&h);
        match expect {
            "ortho" => assert_point(got, Point2::new(raw.x, base.y), &format!("{keys:?}")),
            "free" => assert_point(got, raw, &format!("{keys:?}")),
            _ => {
                let d = got - base;
                assert!(
                    cad_core::geom::tolerance::eq_len(d.x, d.y) && d.x > 0.0,
                    "{keys:?}: 45° に吸い付く {d:?}"
                );
            }
        }
    }
}

// ---- ラバーバンド・クリック・Enter の向きがそろう ----------------------------------

/// Shift + クリックで入る点は、押している間のラバーバンドの先と同じ。
#[test]
fn shift_click_places_the_point_at_the_rubber_band() {
    let (mut h, base) = line_from_p1_with(&[]);
    let pos = screen_at(&h, base, 110.0, 150.0);
    hold_shift(&mut h, true);
    hover(&mut h, pos);
    let rubber = cursor(&h);
    assert_point(rubber, Point2::new(base.x, model(&h, pos).y), "垂直");
    shift_click(&mut h, pos);
    let l = lines(&h);
    assert_eq!(l.len(), 1);
    assert_point(l[0].b, rubber, "クリックで入った点");
    assert_eq!(l[0].b.x, base.x, "ちょうど垂直");
}

/// 直接距離入力（`50` Enter）の向きも、押している間の拘束に従う。
#[test]
fn shift_direct_distance_follows_the_flip() {
    let (mut h, base) = line_from_p1_with(&[]);
    let pos = screen_at(&h, base, 160.0, 200.0);
    hold_shift(&mut h, true);
    hover(&mut h, pos);
    type_text(&mut h, "50");
    let enter = [true, false].map(|pressed| egui::Event::Key {
        key: egui::Key::Enter,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::SHIFT,
    });
    frame(&mut h, enter);
    settle(&mut h);
    let l = lines(&h);
    assert_eq!(l.len(), 1, "Shift を押したままの Enter でも確定する");
    assert_point(l[0].b, Point2::new(base.x - 50.0, base.y), "左へ水平に 50");
}

/// 寸法入力の欄のライブ値（長さ・角度）も、押している間のラバーバンドの先から測る。
#[test]
fn shift_dimension_fields_follow_the_flip() {
    let (mut h, base) = line_from_p1_with(&[]);
    let pos = screen_at(&h, base, 80.0, 200.0);
    hold_shift(&mut h, true);
    hover(&mut h, pos);
    let live = crate::cmdline::dimension::live(base, cursor(&h));
    assert!(
        live.angle_deg
            .is_some_and(|a| cad_core::geom::tolerance::eq_len(a, 90.0)),
        "角度の欄は 90°: {live:?}"
    );
}

// ---- 優先順位 ------------------------------------------------------------------

/// 寸法入力で角度を固定していれば、Shift を押しても固定が優先される。
#[test]
fn an_angle_lock_wins_over_the_flip() {
    let (mut h, base) = line_from_p1_with(&[]);
    press(&mut h, egui::Key::Tab);
    type_text(&mut h, "30");
    press(&mut h, egui::Key::Tab);
    let locks = h
        .state()
        .session
        .cmdline
        .dimension_locks()
        .expect("前提: 角度を固定");
    hold_shift(&mut h, true);
    assert!(flipping(&h), "前提: 反転は受け付けている");
    hover(&mut h, P2);
    let expected = crate::cmdline::dimension::constrain(base, model(&h, P2), locks)
        .expect("P2 は 30° の半直線と同じ側");
    assert_point(cursor(&h), expected, "直交ではなく 30° の半直線へ射影");
}

/// スナップに吸着していれば、Shift を押しても吸着点が優先される。
#[test]
fn a_snap_wins_over_the_flip() {
    let mut h = app_with_dynamic(true);
    let a = egui::pos2(300.0, 500.0);
    let b = egui::pos2(700.0, 200.0);
    hover(&mut h, a);
    type_text(&mut h, "L");
    press(&mut h, egui::Key::Enter);
    click(&mut h, a);
    click(&mut h, b);
    press(&mut h, egui::Key::Escape);
    let end = lines(&h)[0].b;

    type_text(&mut h, "L");
    press(&mut h, egui::Key::Enter);
    click(&mut h, P1);
    hold_shift(&mut h, true);
    assert!(flipping(&h), "前提: 反転は受け付けている");
    let near = h.state().viewport.model_to_screen(end) + egui::vec2(3.0, 3.0);
    hover(&mut h, near);
    assert!(h.state().snapped.is_some(), "前提: 吸着している");
    assert_point(cursor(&h), end, "スナップが優先");
}

// ---- グリップ ------------------------------------------------------------------

/// グリップを掴んでいる間も効く。Shift + クリックで確定しても、選択は残る（選択から外さない）。
#[test]
fn shift_flips_while_dragging_a_grip_and_keeps_the_selection() {
    let mut h = app();
    hover(&mut h, P1);
    let id = add_geom(
        &mut h,
        Geometry::Line(Line::new(p(100.0, 100.0), p(200.0, 100.0))),
    );
    let on = screen(&h, p(130.0, 100.0));
    click(&mut h, on);
    let end = screen(&h, p(200.0, 100.0));
    click(&mut h, end);
    assert!(h.state().session.is_gripping(), "前提: 終点を掴んだ");

    hold_shift(&mut h, true);
    assert!(flipping(&h), "グリップの点の指定中も反転する");
    let target = screen(&h, p(240.0, 120.0));
    hover(&mut h, target);
    shift_click(&mut h, target);
    assert!(!h.state().session.is_gripping(), "確定した");
    let l = line_of(&h, id);
    assert_eq!(l.a.y, l.b.y, "反対側の端点から水平: {l:?}");
    assert!(l.b.x > 200.0, "右へ伸びた: {l:?}");
    assert_eq!(
        h.state().session.selection.to_vec(),
        vec![id],
        "Shift + クリックで確定しても選択は残る"
    );
}

/// 円の四分点は直交・極を外している（基準点が無い）ので、Shift を押しても反転しない。
#[test]
fn a_quadrant_grip_is_not_flipped() {
    let mut h = app();
    hover(&mut h, P1);
    add_geom(&mut h, Geometry::Circle(Circle::new(p(150.0, 150.0), 40.0)));
    let on_circle = screen(&h, p(150.0, 190.0));
    click(&mut h, on_circle);
    let quadrant = screen(&h, p(190.0, 150.0));
    click(&mut h, quadrant);
    assert!(h.state().session.is_gripping(), "前提: 四分点を掴んだ");
    hold_shift(&mut h, true);
    assert!(!flipping(&h), "四分点には基準点が無い");
    let somewhere = screen(&h, p(215.0, 165.0));
    hover(&mut h, somewhere);
    assert_point(cursor(&h), model(&h, somewhere), "カーソルのまま");
}

// ---- 待機中・選択待ちは今までどおり --------------------------------------------------

/// 待機中・選択待ちの Shift は反転に使わず、ステータスバーも変えない。Shift + クリックは選択から外す。
#[test]
fn shift_in_idle_and_while_selecting_is_unchanged() {
    let mut h = app();
    hover(&mut h, P1);
    let id = add_geom(
        &mut h,
        Geometry::Line(Line::new(p(100.0, 100.0), p(200.0, 100.0))),
    );
    let on = screen(&h, p(130.0, 100.0));
    click(&mut h, on);
    assert_eq!(h.state().session.selection.to_vec(), vec![id], "前提");

    hold_shift(&mut h, true);
    assert!(!flipping(&h), "待機中は反転しない");
    assert!(
        h.query_by_label("ortho").is_some(),
        "ステータスバーは変えない"
    );
    shift_click(&mut h, on);
    assert!(
        h.state().session.selection.is_empty(),
        "Shift + クリックは選択から外す"
    );
    assert!(!h.state().session.is_gripping(), "グリップは掴まない");
    hold_shift(&mut h, false);

    // 選択待ち（MOVE を選択なしで始める）。
    type_text(&mut h, "MOVE");
    press(&mut h, egui::Key::Enter);
    assert!(
        h.state().session.has_active_tool() && !h.state().session.wants_point(),
        "前提: 選択待ち"
    );
    // 履歴が伸びて作図領域が縮むので、画面上の位置は取り直す。
    let on = screen(&h, p(130.0, 100.0));
    click(&mut h, on);
    assert_eq!(
        h.state().session.selection.to_vec(),
        vec![id],
        "前提: 選んだ"
    );
    hold_shift(&mut h, true);
    assert!(!flipping(&h), "選択待ちは反転しない");
    assert!(
        h.query_by_label("ortho").is_some(),
        "ステータスバーは変えない"
    );
    shift_click(&mut h, on);
    assert!(
        h.state().session.selection.is_empty(),
        "選択待ちの Shift + クリックも選択から外す"
    );
}

// ---- 反転しないとき -------------------------------------------------------------

/// コマンドラインで変換中は反転しない（IME の操作で Shift を押しても作図の拘束は変えない）。
#[test]
fn no_flip_while_composing() {
    let (mut h, base) = line_from_p1_with(&[]);
    frame(&mut h, [preedit("せん")]);
    settle(&mut h);
    assert!(h.state().session.cmdline.is_composing(), "前提: 変換中");
    hold_shift(&mut h, true);
    assert!(!flipping(&h), "変換中は反転しない");
    let pos = screen_at(&h, base, 20.0, 200.0);
    hover(&mut h, pos);
    assert_point(cursor(&h), model(&h, pos), "カーソルのまま");
    assert!(
        h.query_by_label("ortho").is_some(),
        "ステータスバーも変えない"
    );
}

/// パネルの欄を編集している間は反転しない（名前の大文字を Shift で打つ）。
#[test]
fn no_flip_while_editing_a_panel_field() {
    let (mut h, base) = line_from_p1_with(&[]);
    focus_layer_name_field(&mut h);
    assert!(!h.state().session.cmdline.owns_keys(), "前提: 欄が持ち主");
    hold_shift(&mut h, true);
    assert!(
        h.state().session.tracking_base().is_some(),
        "前提: LINE の 2 点目を待っている"
    );
    assert!(!flipping(&h), "欄の編集中は反転しない");
    let pos = screen_at(&h, base, 20.0, 200.0);
    // カーソルを動かすだけ（クリックすると欄からフォーカスが外れる）。
    frame(&mut h, [egui::Event::PointerMoved(pos)]);
    settle(&mut h);
    assert_point(cursor(&h), model(&h, pos), "カーソルのまま");
}

/// 未保存確認のモーダルが出ている間は反転しない。
#[test]
fn no_flip_while_the_modal_is_open() {
    let (mut h, _dir) = app_with_unsaved_modal(true, "shift_flip");
    assert!(
        h.state().session.tracking_base().is_some(),
        "前提: LINE の次の点を待っている"
    );
    hold_shift(&mut h, true);
    assert!(!flipping(&h), "モーダル中は反転しない");
}

// ---- ステータスバー ------------------------------------------------------------

/// 押している間、ORTHO / POLAR は実際に効いている状態で出る。部品の位置（押す位置）は動かない。
#[test]
fn status_bar_shows_the_actual_state_without_moving() {
    let mut h = app_with_width(SCREEN.x);
    hover(&mut h, P1);
    press(&mut h, egui::Key::F10);
    type_text(&mut h, "L");
    press(&mut h, egui::Key::Enter);
    click(&mut h, P1);
    hover(&mut h, P2);
    let xs = |h: &Harness<'_, CadApp>, ortho: &str| -> Vec<f32> {
        [ortho, "POLAR", "DYN", "レイヤ 0"]
            .into_iter()
            .map(|l| h.get_by_label(l).rect().min.x)
            .collect()
    };
    let before = xs(&h, "ortho");
    let polar_before = h.get_by_label("POLAR").rect();

    hold_shift(&mut h, true);
    assert!(flipping(&h), "前提");
    assert!(
        h.query_by_label("ortho").is_none(),
        "小文字の ortho は消える"
    );
    assert_eq!(
        xs(&h, "ORTHO"),
        before,
        "ORTHO / POLAR / DYN / レイヤの位置が動かない"
    );
    assert_eq!(
        h.get_by_label("POLAR").rect(),
        polar_before,
        "POLAR の大きさも変わらない"
    );
    // 一時的であることはツールチップで説明する。
    let ortho = h.get_by_label("ORTHO").rect().center();
    hover(&mut h, ortho);
    // ツールチップは乗せてしばらくしてから出る。
    h.run_steps(8);
    assert!(
        h.query_by_label_contains(TEMPORARY_NOTE).is_some(),
        "ツールチップで一時的な反転だと分かる"
    );

    hold_shift(&mut h, false);
    hover(&mut h, P2);
    assert_eq!(xs(&h, "ortho"), before, "離すと元の表示");
    assert!(h.query_by_label("ORTHO").is_none());
}
