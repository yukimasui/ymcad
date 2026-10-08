//! ホバーで強調する選択プレビュー（Issue #34 段階 1、ADR-0042）。
//!
//! 強調された図形と、そこでクリックしたときに拾われる図形が一致することを、
//! アプリ全体を動かして確かめる。

use cad_core::command::{AddEntities, AddLayer, CreateGroup, SetLayerProperties};
use cad_core::geom::tolerance::eq_len;
use cad_core::geom::{Line, Point2};
use cad_core::layer::AciColor;
use cad_core::{EntityId, Geometry, LayerId};
use egui_kittest::Harness;

use super::{app, click, frame, hover, press, settle, type_text, CadApp};

/// パネルからの変更と同じ入口で図面を変える。
fn external(h: &mut Harness<'_, CadApp>, cmd: Box<dyn cad_core::Command>) {
    let app = h.state_mut();
    app.session.apply_external(cmd, &mut app.doc);
    settle(h);
}

/// 線分を足して、その ID を返す（足した順）。
fn add_lines(h: &mut Harness<'_, CadApp>, lines: &[(Line, LayerId)]) -> Vec<EntityId> {
    let before: Vec<EntityId> = h.state().doc.entities().ids().collect();
    let entities = lines
        .iter()
        .map(|(l, layer)| cad_core::Entity::new(Geometry::Line(*l), *layer))
        .collect();
    external(h, Box::new(AddEntities::many("TEST", entities)));
    h.state()
        .doc
        .entities()
        .ids()
        .filter(|id| !before.contains(id))
        .collect()
}

fn seg(x0: f64, y0: f64, x1: f64, y1: f64) -> Line {
    Line::new(Point2::new(x0, y0), Point2::new(x1, y1))
}

/// モデル座標の点の画面座標。履歴で作図領域が縮むので、使う直前に取る。
fn screen(h: &Harness<'_, CadApp>, p: Point2) -> egui::Pos2 {
    h.state().viewport.model_to_screen(p)
}

fn highlighted(h: &Harness<'_, CadApp>) -> Vec<EntityId> {
    let mut ids = h.state().hover.highlighted().to_vec();
    ids.sort();
    ids
}

fn selected(h: &Harness<'_, CadApp>) -> Vec<EntityId> {
    h.state().session.selection.to_vec()
}

/// `p` に乗せて強調された集合を取り、選択を空にしてからクリックして、選ばれた集合と比べる。
/// 強調された集合を返す。
fn assert_hover_matches_click(h: &mut Harness<'_, CadApp>, p: Point2, what: &str) -> Vec<EntityId> {
    h.state_mut().session.selection.clear();
    let pos = screen(h, p);
    hover(h, pos);
    let shown = highlighted(h);
    assert!(!shown.is_empty(), "{what}: 何か強調されている");
    click(h, pos);
    assert_eq!(
        selected(h),
        shown,
        "{what}: 強調された集合 == クリック後の選択"
    );
    shown
}

/// 待機中: 1 本・同じ形の重なり・グループ・ロックされた一員を含むグループで、
/// 強調された集合とクリックで選ばれる集合が一致する。
#[test]
fn idle_hover_matches_the_click() {
    let mut h = app();
    hover(&mut h, egui::pos2(500.0, 300.0));
    external(&mut h, Box::new(AddLayer::new("L1", AciColor::WHITE)));
    let l1 = h.state().doc.layers().by_name("L1").expect("L1");
    let ids = add_lines(
        &mut h,
        &[
            // 単独の線分。
            (seg(50.0, 250.0, 150.0, 250.0), LayerId::ZERO),
            // 同じ形の重なり（同じ距離）。
            (seg(200.0, 250.0, 300.0, 250.0), LayerId::ZERO),
            (seg(200.0, 250.0, 300.0, 250.0), LayerId::ZERO),
            // グループ。
            (seg(50.0, 150.0, 150.0, 150.0), LayerId::ZERO),
            (seg(50.0, 120.0, 150.0, 120.0), LayerId::ZERO),
            // ロックされた一員を含むグループ。
            (seg(200.0, 150.0, 300.0, 150.0), LayerId::ZERO),
            (seg(200.0, 120.0, 300.0, 120.0), l1),
        ],
    );
    external(
        &mut h,
        Box::new(CreateGroup::new("GROUP", "g1", vec![ids[3], ids[4]])),
    );
    external(
        &mut h,
        Box::new(CreateGroup::new("GROUP", "g2", vec![ids[5], ids[6]])),
    );
    external(&mut h, Box::new(SetLayerProperties::new(l1).locked(true)));

    let got = assert_hover_matches_click(&mut h, Point2::new(100.0, 250.0), "単独");
    assert_eq!(got, vec![ids[0]]);
    let got = assert_hover_matches_click(&mut h, Point2::new(250.0, 250.0), "重なり");
    assert_eq!(got, vec![ids[2]], "後から作った方");
    let got = assert_hover_matches_click(&mut h, Point2::new(100.0, 150.0), "グループ");
    assert_eq!(got, vec![ids[3], ids[4]], "グループ全体");
    let got = assert_hover_matches_click(&mut h, Point2::new(250.0, 150.0), "ロック");
    assert_eq!(got, vec![ids[5]], "ロックされた一員は入らない");
    // ロックされた一員そのものには乗せても何も強調されない（拾われない）。
    let pos = screen(&h, Point2::new(250.0, 120.0));
    hover(&mut h, pos);
    assert!(highlighted(&h).is_empty(), "ロックされた図形は拾われない");
}

/// 選択待ち（ERASE の「オブジェクトを選択」）でも、強調されたものがクリックで選ばれる。
#[test]
fn selection_stage_hover_matches_the_click() {
    let mut h = app();
    hover(&mut h, egui::pos2(500.0, 300.0));
    let ids = add_lines(
        &mut h,
        &[
            (seg(50.0, 150.0, 150.0, 150.0), LayerId::ZERO),
            (seg(50.0, 120.0, 150.0, 120.0), LayerId::ZERO),
        ],
    );
    external(
        &mut h,
        Box::new(CreateGroup::new("GROUP", "g", vec![ids[0], ids[1]])),
    );
    type_text(&mut h, "E");
    press(&mut h, egui::Key::Enter);
    assert_eq!(h.state().session.active_command(), Some("ERASE"), "前提");
    let got = assert_hover_matches_click(&mut h, Point2::new(100.0, 150.0), "選択待ち");
    assert_eq!(got, vec![ids[0], ids[1]]);
}

/// 点の入力待ち・値の入力待ちでは何も強調しない。
#[test]
fn point_and_value_input_highlight_nothing() {
    let mut h = app();
    hover(&mut h, egui::pos2(500.0, 300.0));
    add_lines(&mut h, &[(seg(50.0, 150.0, 150.0, 150.0), LayerId::ZERO)]);
    let on = screen(&h, Point2::new(100.0, 150.0));
    hover(&mut h, on);
    assert_eq!(highlighted(&h).len(), 1, "前提: 待機中は強調される");

    // LINE の点の入力待ち。
    type_text(&mut h, "L");
    press(&mut h, egui::Key::Enter);
    let on = screen(&h, Point2::new(100.0, 150.0));
    hover(&mut h, on + egui::vec2(1.0, 0.0));
    assert!(highlighted(&h).is_empty(), "点の入力待ち");
    press(&mut h, egui::Key::Escape);

    // FILLET の半径の入力待ち。
    type_text(&mut h, "F");
    press(&mut h, egui::Key::Enter);
    let on = screen(&h, Point2::new(100.0, 150.0));
    hover(&mut h, on);
    assert_eq!(
        highlighted(&h).len(),
        1,
        "前提: 図形を指す段階では強調される"
    );
    type_text(&mut h, "R");
    press(&mut h, egui::Key::Enter);
    let on = screen(&h, Point2::new(100.0, 150.0));
    hover(&mut h, on + egui::vec2(1.0, 0.0));
    assert!(!h.state().session.wants_entity(), "前提: 値の入力待ち");
    assert!(highlighted(&h).is_empty(), "値の入力待ち");
}

/// 矩形選択のドラッグ中と、カーソルがキャンバスの外にあるときは強調しない。
#[test]
fn dragging_and_leaving_the_canvas_highlight_nothing() {
    let mut h = app();
    hover(&mut h, egui::pos2(500.0, 300.0));
    add_lines(&mut h, &[(seg(50.0, 150.0, 150.0, 150.0), LayerId::ZERO)]);
    let on = screen(&h, Point2::new(100.0, 150.0));

    // 空いた所で押して、線分の上までドラッグする。
    let from = on + egui::vec2(0.0, 80.0);
    frame(
        &mut h,
        [
            egui::Event::PointerMoved(from),
            egui::Event::PointerButton {
                pos: from,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    for t in [0.3_f32, 0.6, 1.0] {
        frame(&mut h, [egui::Event::PointerMoved(from + (on - from) * t)]);
    }
    settle(&mut h);
    assert!(h.state().rect_drag.is_some(), "前提: 矩形選択のドラッグ中");
    assert!(highlighted(&h).is_empty(), "ドラッグ中");
    frame(
        &mut h,
        [egui::Event::PointerButton {
            pos: on,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    settle(&mut h);

    hover(&mut h, on);
    assert_eq!(highlighted(&h).len(), 1, "前提: 乗せれば強調される");
    // キャンバスの外（ウィンドウの外）へ出る。
    frame(&mut h, [egui::Event::PointerGone]);
    settle(&mut h);
    assert!(highlighted(&h).is_empty(), "キャンバスの外");
}

/// TRIM 中はグループに広げず、乗せた 1 つだけを強調する。
#[test]
fn trim_highlights_a_single_entity() {
    let mut h = app();
    hover(&mut h, egui::pos2(500.0, 300.0));
    let ids = add_lines(
        &mut h,
        &[
            (seg(50.0, 150.0, 150.0, 150.0), LayerId::ZERO),
            (seg(50.0, 120.0, 150.0, 120.0), LayerId::ZERO),
        ],
    );
    external(
        &mut h,
        Box::new(CreateGroup::new("GROUP", "g", vec![ids[0], ids[1]])),
    );
    type_text(&mut h, "TR");
    press(&mut h, egui::Key::Enter);
    let pos = screen(&h, Point2::new(100.0, 150.0));
    hover(&mut h, pos);
    assert_eq!(highlighted(&h), vec![ids[0]], "1 つだけ");
}

/// 交点の近くでも TRIM は指した線分の、指した側を切る（OSNAP が交点へ吸い付かない）。
/// 直交（F8）を入れていても同じ。
///
/// 修正前は交点へ吸い付き、交点では 2 本が同じ距離なので後から引いた縦の線が拾われていた。
#[test]
fn trim_near_an_intersection_trims_the_line_under_the_cursor() {
    for ortho in [false, true] {
        let mut h = app();
        hover(&mut h, egui::pos2(500.0, 300.0));
        if ortho {
            press(&mut h, egui::Key::F8);
        }
        assert!(h.state().snap.is_enabled(), "前提: OSNAP はオン");
        // 横の線分（切られる側）と、後から引いた縦の線分（切断エッジ）。
        let ids = add_lines(
            &mut h,
            &[
                (seg(50.0, 150.0, 250.0, 150.0), LayerId::ZERO),
                (seg(150.0, 50.0, 150.0, 250.0), LayerId::ZERO),
            ],
        );
        type_text(&mut h, "TR");
        press(&mut h, egui::Key::Enter);

        // 交点から右へ 4px（交点スナップの吸着半径 10px の内側、拾い半径 6px の内側）。
        let pos = screen(&h, Point2::new(150.0, 150.0)) + egui::vec2(4.0, 0.0);
        hover(&mut h, pos);
        assert!(h.state().snapped.is_none(), "ortho {ortho}: 吸い付かない");
        assert_eq!(
            highlighted(&h),
            vec![ids[0]],
            "ortho {ortho}: 横の線分を強調"
        );
        click(&mut h, pos);

        // TRIM は切った線分を新しい ID で置き換えるので、形で探す。
        let mut got: Vec<Line> = h
            .state()
            .doc
            .entities()
            .iter()
            .filter_map(|(_, e)| match e.geom {
                Geometry::Line(l) => Some(l),
                _ => None,
            })
            .collect();
        got.sort_by(|a, b| a.a.x.total_cmp(&b.a.x));
        assert_eq!(got.len(), 2, "ortho {ortho}: 線分は 2 本のまま: {got:?}");
        let horizontal = got
            .iter()
            .find(|l| eq_len(l.a.y, 150.0) && eq_len(l.b.y, 150.0))
            .expect("横の線分は残る");
        assert!(
            eq_len(horizontal.a.x.max(horizontal.b.x), 150.0)
                && eq_len(horizontal.a.x.min(horizontal.b.x), 50.0),
            "ortho {ortho}: 右側が交点まで切られる: {horizontal:?}"
        );
        assert!(
            got.contains(&seg(150.0, 50.0, 150.0, 250.0)),
            "ortho {ortho}: 縦の線分は変わらない: {got:?}"
        );
    }
}
