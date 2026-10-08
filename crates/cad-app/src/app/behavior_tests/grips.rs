//! グリップ編集（Issue #30 段階 1、ADR-0045）の振る舞い。
//!
//! 実際のクリック・キーで掴んで確定し、スナップ・直交・寸法入力が通常の点指定と同じに効くこと、
//! グリップの上で押してドラッグしても矩形選択にならないこと、などをアプリ全体で固定する。
//! 形の計算は `grips` の単体テスト、`Session` の分岐は `session/grip_tests.rs`。

use std::time::{Duration, Instant};

use cad_core::command::AddEntities;
use cad_core::geom::tolerance::eq_len;
use cad_core::geom::{Line, Point2, Polyline};
use cad_core::{Entity, EntityId, Geometry, LayerId};
use egui_kittest::kittest::Queryable as _;
use egui_kittest::Harness;

use super::{
    app, app_with_dynamic, click, frame, hover, key_with, preedit, press, settle, type_text,
    CadApp, P1,
};
use crate::grips::Handle;
use crate::layer_panel::{MOVE_BUSY_NOTE, MOVE_GRIP_NOTE};
use crate::properties::{BUSY_NOTE, GRIP_BUSY_NOTE};

/// パネルからの変更と同じ入口で図面を変える。
fn external(h: &mut Harness<'_, CadApp>, cmd: Box<dyn cad_core::Command>) {
    let app = h.state_mut();
    app.session.apply_external(cmd, &mut app.doc);
    settle(h);
}

fn p(x: f64, y: f64) -> Point2 {
    Point2::new(x, y)
}

/// 線分を足して ID を返す。
fn add_line(h: &mut Harness<'_, CadApp>, a: Point2, b: Point2) -> EntityId {
    external(
        h,
        Box::new(AddEntities::one(
            "LINE",
            Entity::new(Geometry::Line(Line::new(a, b)), LayerId::ZERO),
        )),
    );
    h.state().doc.entities().ids().last().expect("足した図形")
}

fn line_of(h: &Harness<'_, CadApp>, id: EntityId) -> Line {
    match h.state().doc.entities().get(id).map(|e| &e.geom) {
        Some(Geometry::Line(l)) => *l,
        other => panic!("線分のはず: {other:?}"),
    }
}

/// モデル座標の点の画面座標。履歴で作図領域が縮むので、使う直前に取る。
fn screen(h: &Harness<'_, CadApp>, at: Point2) -> egui::Pos2 {
    h.state().viewport.model_to_screen(at)
}

fn gripping(h: &Harness<'_, CadApp>) -> bool {
    h.state().session.is_gripping()
}

fn assert_pt(actual: Point2, expected: Point2, what: &str) {
    assert!(
        eq_len(actual.x, expected.x) && eq_len(actual.y, expected.y),
        "{what}: {actual:?} != {expected:?}"
    );
}

/// (100,100)-(200,100) の線分を引いてクリックで選んだ状態。
fn selected_line() -> (Harness<'static, CadApp>, EntityId) {
    let mut h = app();
    hover(&mut h, P1);
    let id = add_line(&mut h, p(100.0, 100.0), p(200.0, 100.0));
    let on = screen(&h, p(130.0, 100.0));
    click(&mut h, on);
    assert_eq!(
        h.state().session.selection.to_vec(),
        vec![id],
        "前提: 選んだ"
    );
    (h, id)
}

/// 線分の終点のグリップをクリックで掴む。
fn grab_end(h: &mut Harness<'_, CadApp>) {
    let end = screen(h, p(200.0, 100.0));
    click(h, end);
    assert!(gripping(h), "前提: 掴んだ");
    assert_eq!(
        h.state().session.hot_grips()[0].handle,
        Handle::LineEnd,
        "前提: 終点"
    );
}

/// 乗せた位置に入る点（スナップ・直交・寸法の固定の後）。確定の直前に取る。
fn cursor(h: &Harness<'_, CadApp>) -> Point2 {
    h.state().cursor_model.expect("キャンバスの上")
}

// ---- 掴む・確定 --------------------------------------------------------------

/// 選んだ線分の終点のグリップをクリックで掴み、次のクリックで確定する。選択は残る。
#[test]
fn clicking_a_grip_and_then_a_point_moves_the_end() {
    let (mut h, id) = selected_line();
    grab_end(&mut h);
    let target = screen(&h, p(230.0, 160.0));
    hover(&mut h, target);
    let expected = cursor(&h);
    click(&mut h, target);
    assert!(!gripping(&h), "確定したら終わる");
    let l = line_of(&h, id);
    assert_pt(l.a, p(100.0, 100.0), "始点は動かない");
    assert_pt(l.b, expected, "終点がクリックした点へ");
    assert_eq!(h.state().session.selection.to_vec(), vec![id], "選択は残る");

    // Undo 1 回で戻る。
    type_text(&mut h, "U");
    press(&mut h, egui::Key::Enter);
    assert_pt(line_of(&h, id).b, p(200.0, 100.0), "Undo 1 回で戻る");
}

/// F8（直交）で、反対側の端点から水平な線分になる（長さ・角度の基点は形の基準。ユーザー判断 1）。
#[test]
fn f8_makes_the_line_horizontal() {
    let (mut h, id) = selected_line();
    grab_end(&mut h);
    press(&mut h, egui::Key::F8);
    let target = screen(&h, p(240.0, 120.0));
    hover(&mut h, target);
    click(&mut h, target);
    let l = line_of(&h, id);
    assert_eq!(l.a.y, l.b.y, "水平: {l:?}");
    assert!(l.b.x > 200.0, "右へ伸びた: {l:?}");
}

/// `100` Tab `0` Enter で、反対側の端点から長さ 150・角度 0 の線分になる。
#[test]
fn length_tab_angle_enter_sets_the_length_from_the_other_end() {
    let mut h = app_with_dynamic(true);
    hover(&mut h, P1);
    let id = add_line(&mut h, p(100.0, 100.0), p(200.0, 100.0));
    let on = screen(&h, p(130.0, 100.0));
    click(&mut h, on);
    grab_end(&mut h);
    let somewhere = screen(&h, p(230.0, 160.0));
    hover(&mut h, somewhere);
    type_text(&mut h, "150");
    press(&mut h, egui::Key::Tab);
    type_text(&mut h, "0");
    press(&mut h, egui::Key::Enter);
    assert!(!gripping(&h));
    let l = line_of(&h, id);
    assert_pt(l.a, p(100.0, 100.0), "始点");
    assert_pt(l.b, p(250.0, 100.0), "長さ 150・0°");
}

/// 別の線分の端点へスナップする。
#[test]
fn the_moved_end_snaps_to_another_endpoint() {
    let (mut h, id) = selected_line();
    add_line(&mut h, p(260.0, 180.0), p(300.0, 260.0));
    grab_end(&mut h);
    let near = screen(&h, p(260.0, 180.0)) + egui::vec2(3.0, -2.0);
    hover(&mut h, near);
    click(&mut h, near);
    assert_eq!(line_of(&h, id).b, p(260.0, 180.0), "端点へぴったり");
}

/// Esc の 1 回目は寸法の固定、2 回目はグリップ、3 回目で選択を解除する。
#[test]
fn escape_releases_locks_then_the_grip_then_the_selection() {
    let mut h = app_with_dynamic(true);
    hover(&mut h, P1);
    let id = add_line(&mut h, p(100.0, 100.0), p(200.0, 100.0));
    let on = screen(&h, p(130.0, 100.0));
    click(&mut h, on);
    grab_end(&mut h);
    type_text(&mut h, "50");
    press(&mut h, egui::Key::Tab);
    assert!(
        h.state().session.cmdline.dimension_locks().is_some(),
        "前提: 長さを固定"
    );

    press(&mut h, egui::Key::Escape);
    assert!(h.state().session.cmdline.dimension_locks().is_none());
    assert!(gripping(&h), "1 回目は固定だけ");
    press(&mut h, egui::Key::Escape);
    assert!(!gripping(&h), "2 回目でグリップ");
    assert_eq!(h.state().session.selection.to_vec(), vec![id], "選択は残る");
    press(&mut h, egui::Key::Escape);
    assert!(h.state().session.selection.is_empty(), "3 回目で選択を解除");
    assert_pt(line_of(&h, id).b, p(200.0, 100.0), "形は変わらない");
}

// ---- ドラッグ ------------------------------------------------------------------

/// グリップの上で押してドラッグしても矩形選択にならず、離した時点で掴んだ状態。確定は次のクリック
/// （ユーザー判断 2）。ドラッグの矩形に掛かる別の線分は選ばれない。
#[test]
fn dragging_from_a_grip_grabs_it_instead_of_selecting_a_rectangle() {
    let (mut h, id) = selected_line();
    let other = add_line(&mut h, p(150.0, 130.0), p(150.0, 170.0));
    let from = screen(&h, p(200.0, 100.0));
    let to = screen(&h, p(120.0, 160.0));
    let button = |pos, pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    frame(
        &mut h,
        [egui::Event::PointerMoved(from), button(from, true)],
    );
    for t in [0.25_f32, 0.5, 0.75, 1.0] {
        frame(&mut h, [egui::Event::PointerMoved(from.lerp(to, t))]);
    }
    assert!(gripping(&h), "ドラッグの途中で掴んでいる");
    frame(&mut h, [button(to, false)]);
    settle(&mut h);

    assert!(gripping(&h), "離した時点で掴んだまま（確定しない）");
    assert_eq!(
        h.state().session.selection.to_vec(),
        vec![id],
        "矩形選択にならない（右から左のドラッグでも別の線分を選ばない）"
    );
    assert!(!h.state().session.selection.contains(other));
    assert_pt(line_of(&h, id).b, p(200.0, 100.0), "まだ変わらない");

    let target = screen(&h, p(240.0, 60.0));
    hover(&mut h, target);
    let expected = cursor(&h);
    click(&mut h, target);
    assert!(!gripping(&h));
    assert_pt(line_of(&h, id).b, expected, "次のクリックで確定");
}

// ---- ズーム ------------------------------------------------------------------

/// どの倍率でも、グリップの当たりは画面上で同じ大きさ（乗せて 5px ずれは当たり、9px ずれは外れ）。
#[test]
fn grips_are_hit_at_the_same_pixel_distance_at_any_zoom() {
    let (mut h, _) = selected_line();
    for factor in [1.0, 8.0, 0.125] {
        let anchor = screen(&h, p(200.0, 100.0));
        h.state_mut().viewport.zoom_about(anchor, factor);
        settle(&mut h);
        let end = screen(&h, p(200.0, 100.0));
        hover(&mut h, end + egui::vec2(5.0, 5.0));
        assert_eq!(
            h.state()
                .hover
                .hovered_grip()
                .map(|g| g.representative().handle),
            Some(Handle::LineEnd),
            "倍率 {factor}: 5px ずれは当たる"
        );
        hover(&mut h, end + egui::vec2(9.0, 0.0));
        assert_eq!(
            h.state().hover.hovered_grip(),
            None,
            "倍率 {factor}: 9px ずれは外れる"
        );
    }
}

// ---- 選択が多いとき -------------------------------------------------------------

/// Ctrl+A で 1 万図形を選んでもフレーム時間が収まり、グリップは出さずにステータスバーで知らせる。
#[test]
fn selecting_ten_thousand_entities_shows_no_grips_and_says_so() {
    const N: u32 = 10_000;
    let mut h = app();
    let entities = (0..N)
        .map(|i| {
            let y = f64::from(i) * 5.0;
            Entity::new(
                Geometry::Line(Line::new(p(0.0, y), p(100.0, y))),
                LayerId::ZERO,
            )
        })
        .collect();
    external(&mut h, Box::new(AddEntities::many("TEST", entities)));
    hover(&mut h, P1);
    frame(
        &mut h,
        key_with(
            egui::Key::A,
            egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
        ),
    );
    settle(&mut h);
    assert_eq!(h.state().session.selection.len(), 10_000, "前提: 全選択");
    assert!(h.state().session.grips_suppressed());
    assert!(h.state().session.grips(&h.state().doc).is_empty());
    assert!(
        h.query_by_label_contains("グリップなし").is_some(),
        "ステータスバーで知らせる"
    );

    let frames = Instant::now();
    h.run_steps(10);
    let per_frame = frames.elapsed() / 10;
    eprintln!("1 万図形を選んだ後の 1 フレーム平均: {per_frame:?}");
    assert!(
        per_frame < Duration::from_secs(1),
        "1 フレームが 1 秒を超えた（固まっている）: {per_frame:?}"
    );

    // 端点をクリックしても掴まない（図形のピックになる）。
    let end = screen(&h, p(100.0, 0.0));
    click(&mut h, end);
    assert!(!gripping(&h));
}

// ---- パネル・変換中 -----------------------------------------------------------

/// 掴んでいる間、プロパティパネルは表示だけ（コマンド実行中と同じ）。取り消せば戻る。
/// 案内はグリップ用（「Esc で取り消し。選択は残ります」。コマンドの「中断すると選択も外れます」ではない）。
#[test]
fn the_properties_panel_is_display_only_while_gripping() {
    let (mut h, _) = selected_line();
    frame(&mut h, key_with(egui::Key::Num1, egui::Modifiers::CTRL));
    settle(&mut h);
    assert!(h.state().properties_panel.is_open(), "前提: 開いた");
    assert!(
        h.query_all_by_label(GRIP_BUSY_NOTE).next().is_none(),
        "前提: 待機中は案内が無い"
    );
    grab_end(&mut h);
    assert!(
        h.query_all_by_label(GRIP_BUSY_NOTE).next().is_some(),
        "掴んでいる間は表示だけ"
    );
    assert!(
        h.query_all_by_label(BUSY_NOTE).next().is_none(),
        "コマンドの案内（中断すると選択も外れます）は出さない"
    );
    press(&mut h, egui::Key::Escape);
    assert!(h.query_all_by_label(GRIP_BUSY_NOTE).next().is_none());
}

/// レイヤパネルの「移動」の行も、掴んでいる間はグリップ用の案内。
#[test]
fn the_layer_panel_move_row_says_the_grip_keeps_the_selection() {
    let (mut h, _) = selected_line();
    type_text(&mut h, "LA");
    press(&mut h, egui::Key::Enter);
    assert!(h.state().layer_panel.is_open(), "前提: 開いた");
    grab_end(&mut h);
    assert!(h.query_all_by_label(MOVE_GRIP_NOTE).next().is_some());
    assert!(h.query_all_by_label(MOVE_BUSY_NOTE).next().is_none());
    press(&mut h, egui::Key::Escape);
    assert!(h.query_all_by_label(MOVE_GRIP_NOTE).next().is_none());
}

/// コマンドラインで変換中でも、クリックで掴み・確定できる。変換中の文字列には触れない（ADR-0002）。
#[test]
fn grips_work_while_composing_without_touching_the_preedit() {
    let (mut h, id) = selected_line();
    frame(&mut h, [preedit("に")]);
    settle(&mut h);
    assert!(h.state().session.cmdline.is_composing(), "前提: 変換中");
    let before = h.state().session.cmdline.input().to_owned();

    let end = screen(&h, p(200.0, 100.0));
    click(&mut h, end);
    assert!(gripping(&h), "掴める");
    assert!(h.state().session.cmdline.is_composing(), "変換中のまま");
    assert_eq!(
        h.state().session.cmdline.input(),
        before,
        "文字列はそのまま"
    );

    let target = screen(&h, p(230.0, 160.0));
    hover(&mut h, target);
    let expected = cursor(&h);
    click(&mut h, target);
    assert!(!gripping(&h), "確定できる");
    assert_pt(line_of(&h, id).b, expected, "確定した");
    assert!(h.state().session.cmdline.is_composing(), "変換中のまま");
    assert_eq!(
        h.state().session.cmdline.input(),
        before,
        "文字列はそのまま"
    );
    frame(&mut h, [preedit("")]);
    settle(&mut h);
}

// ---- 段階 2: 重なったグリップ・辺の中点・段階 1 のレビューの残り ----------------------

/// 図形を足して ID を返す。
fn add_geom(h: &mut Harness<'_, CadApp>, geom: Geometry) -> EntityId {
    external(
        h,
        Box::new(AddEntities::one("TEST", Entity::new(geom, LayerId::ZERO))),
    );
    h.state().doc.entities().ids().last().expect("足した図形")
}

fn geom_of(h: &Harness<'_, CadApp>, id: EntityId) -> Geometry {
    h.state().doc.entities().get(id).expect("ある").geom.clone()
}

/// 線分 4 本の矩形（(100,100)-(200,100)-(200,160)-(100,160)）を足して全部選ぶ（Ctrl+A）。
fn selected_rectangle_of_lines() -> (Harness<'static, CadApp>, Vec<EntityId>) {
    let mut h = app();
    hover(&mut h, P1);
    let ids = vec![
        add_line(&mut h, p(100.0, 100.0), p(200.0, 100.0)),
        add_line(&mut h, p(200.0, 100.0), p(200.0, 160.0)),
        add_line(&mut h, p(200.0, 160.0), p(100.0, 160.0)),
        add_line(&mut h, p(100.0, 160.0), p(100.0, 100.0)),
    ];
    frame(
        &mut h,
        key_with(
            egui::Key::A,
            egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
        ),
    );
    settle(&mut h);
    assert_eq!(h.state().session.selection.len(), 4, "前提: 全部選んだ");
    (h, ids)
}

/// 線分 4 本の矩形の角を掴んで動かすと、隣り合う 2 本が一緒に動いてつながったまま。
/// 乗せたときの案内は「端点を動かす（2 個）」。Undo 1 回で 2 本とも戻る。
#[test]
fn dragging_a_corner_of_four_lines_moves_both_lines() {
    let (mut h, ids) = selected_rectangle_of_lines();
    let corner = screen(&h, p(200.0, 100.0));
    hover(&mut h, corner);
    let hovered = h.state().hover.hovered_grip().expect("角に乗せた").clone();
    assert_eq!(hovered.grips().len(), 2, "2 本の端点の束");
    assert_eq!(hovered.label(), "端点を動かす（2 個）");
    click(&mut h, corner);
    assert!(gripping(&h));
    assert_eq!(
        h.state().session.hot_grips(),
        hovered.grips(),
        "乗せた束を掴む"
    );

    let target = screen(&h, p(230.0, 80.0));
    hover(&mut h, target);
    let expected = cursor(&h);
    click(&mut h, target);
    assert!(!gripping(&h), "確定した");
    let (l0, l1) = (line_of(&h, ids[0]), line_of(&h, ids[1]));
    assert_pt(l0.b, expected, "1 本目の終点");
    assert_pt(l1.a, expected, "2 本目の始点");
    assert_eq!(l0.b, l1.a, "つながったまま");
    assert_pt(l0.a, p(100.0, 100.0), "1 本目の始点は動かない");
    assert_pt(l1.b, p(200.0, 160.0), "2 本目の終点は動かない");
    assert_pt(
        line_of(&h, ids[2]).a,
        p(200.0, 160.0),
        "ほかの線分は動かない",
    );

    type_text(&mut h, "U");
    press(&mut h, egui::Key::Enter);
    assert_pt(line_of(&h, ids[0]).b, p(200.0, 100.0), "Undo 1 回で戻る");
    assert_pt(line_of(&h, ids[1]).a, p(200.0, 100.0), "Undo 1 回で戻る");
}

/// ポリラインの矩形の右の辺の中点を掴んで右へ動かすと、矩形の幅が変わる（頂点の数は 4 のまま）。
#[test]
fn moving_the_right_edge_of_a_polyline_rectangle_changes_its_width() {
    let mut h = app();
    hover(&mut h, P1);
    let id = add_geom(
        &mut h,
        Geometry::Polyline(Polyline::rectangle(p(100.0, 100.0), p(200.0, 160.0))),
    );
    let inside = screen(&h, p(150.0, 100.0)) + egui::vec2(20.0, 0.0);
    click(&mut h, inside);
    assert_eq!(
        h.state().session.selection.to_vec(),
        vec![id],
        "前提: 選んだ"
    );

    let edge = screen(&h, p(200.0, 130.0));
    hover(&mut h, edge);
    assert_eq!(
        h.state()
            .hover
            .hovered_grip()
            .map(|g| (g.representative().handle, g.label())),
        Some((Handle::Edge(1), "辺を動かす".to_owned()))
    );
    click(&mut h, edge);
    assert!(gripping(&h));
    press(&mut h, egui::Key::F8);
    let target = screen(&h, p(260.0, 137.0));
    hover(&mut h, target);
    click(&mut h, target);
    assert!(!gripping(&h));
    let Geometry::Polyline(pl) = geom_of(&h, id) else {
        panic!("ポリラインのまま");
    };
    assert_eq!(pl.vertices.len(), 4, "頂点の数は変わらない");
    assert!(pl.closed);
    let right = pl.vertices[1].x;
    assert!(right > 250.0, "右の辺が右へ: {pl:?}");
    assert_eq!(pl.vertices[1].x, pl.vertices[2].x, "右の辺は縦のまま");
    assert_pt(
        pl.vertices[1],
        p(right, 100.0),
        "高さは変わらない（F8 で水平）",
    );
    assert_pt(
        pl.vertices[2],
        p(right, 160.0),
        "高さは変わらない（F8 で水平）",
    );
    assert_pt(pl.vertices[0], p(100.0, 100.0), "左の辺は動かない");
}

/// 円の四分点を掴むと、カーソル横の寸法入力の欄は「半径」だけ（角度の欄は出ない）。
/// 線分の端点では「長さ」「角度」。
#[test]
fn a_quadrant_shows_only_a_radius_field() {
    let mut h = app_with_dynamic(true);
    hover(&mut h, P1);
    add_geom(
        &mut h,
        Geometry::Circle(cad_core::geom::Circle::new(p(150.0, 150.0), 40.0)),
    );
    add_line(&mut h, p(300.0, 100.0), p(380.0, 100.0));
    // 円と線分を選ぶ。
    let on_circle = screen(&h, p(150.0, 190.0));
    click(&mut h, on_circle);
    let on_line = screen(&h, p(330.0, 100.0));
    click(&mut h, on_line);
    assert_eq!(h.state().session.selection.len(), 2, "前提: 2 つ選んだ");

    let quadrant = screen(&h, p(190.0, 150.0));
    click(&mut h, quadrant);
    assert!(gripping(&h));
    let somewhere = screen(&h, p(210.0, 170.0));
    hover(&mut h, somewhere);
    assert!(h.query_by_label("半径").is_some(), "欄の名前は「半径」");
    assert!(h.query_by_label("長さ").is_none());
    assert!(h.query_by_label("角度").is_none(), "角度の欄は出さない");
    // Tab で半径を固定しても、角度の欄へは移らない。
    type_text(&mut h, "55");
    press(&mut h, egui::Key::Tab);
    assert!(h.query_by_label("角度").is_none());
    press(&mut h, egui::Key::Enter);
    assert!(!gripping(&h), "固定した半径で確定");
    let circle = h
        .state()
        .doc
        .entities()
        .iter()
        .find_map(|(_, e)| match e.geom {
            Geometry::Circle(c) => Some(c),
            _ => None,
        })
        .expect("円");
    assert!(eq_len(circle.radius, 55.0), "{circle:?}");

    // 線分の端点では「長さ」「角度」。
    let end = screen(&h, p(380.0, 100.0));
    click(&mut h, end);
    assert!(gripping(&h));
    hover(&mut h, somewhere);
    assert!(h.query_by_label("長さ").is_some());
    assert!(h.query_by_label("角度").is_some());
    assert!(h.query_by_label("半径").is_none());
}

/// 掴んでいる間のプロンプトは「空の Enter・Esc で取り消し」。
#[test]
fn the_grip_prompt_says_an_empty_enter_cancels() {
    let (mut h, _) = selected_line();
    grab_end(&mut h);
    // 履歴の上のプロンプトとカーソル横の両方に出ることがある。
    assert!(
        h.query_all_by_label_contains("** 端点を動かす ** 点を指定 (空の Enter・Esc で取り消し):")
            .next()
            .is_some(),
        "プロンプト"
    );
}

/// 選択が上限を超えたときの「選択 N（グリップなし）」に乗せると、理由がツールチップで出る。
#[test]
fn the_no_grips_status_item_explains_why() {
    let mut h = app();
    let n = crate::session::MAX_GRIP_SELECTION + 1;
    let entities = (0..n)
        .map(|i| {
            let y = f64::from(u32::try_from(i).expect("小さい")) * 5.0;
            Entity::new(
                Geometry::Line(Line::new(p(0.0, y), p(100.0, y))),
                LayerId::ZERO,
            )
        })
        .collect();
    external(&mut h, Box::new(AddEntities::many("TEST", entities)));
    hover(&mut h, P1);
    frame(
        &mut h,
        key_with(
            egui::Key::A,
            egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
        ),
    );
    settle(&mut h);
    let item = h
        .query_by_label_contains("グリップなし")
        .expect("ステータスバーで知らせる")
        .rect()
        .center();
    let tip = "グリップは選択が 100 個以下のときだけ出ます";
    assert!(h.query_by_label(tip).is_none(), "前提: 乗せる前は出ない");
    hover(&mut h, item);
    h.run_steps(4);
    assert!(h.query_by_label(tip).is_some(), "乗せると理由が出る");
}
