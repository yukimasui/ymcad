//! グリップ編集（Issue #30 段階 1・2）の見た目。
//!
//! 図形はモデル座標で置き、画面上の位置は撮る直前に表示範囲から求める（履歴が伸びると
//! 作図領域が上下に縮むので）。

use cad_core::command::AddEntities;
use cad_core::geom::{Arc, Circle, Line, Point2, Polyline};
use cad_core::{Entity, EntityId, Geometry, LayerId};
use egui_kittest::Harness;

use super::{click, harness, hover, press, shot, CadApp, CANVAS_CENTER};

fn p(x: f64, y: f64) -> Point2 {
    Point2::new(x, y)
}

/// 図形を足して選んだ状態にする。
fn add_and_select(h: &mut Harness<'_, CadApp>, geoms: Vec<Geometry>) -> Vec<EntityId> {
    let (doc, session) = h.state_mut().parts_mut();
    let entities = geoms
        .into_iter()
        .map(|g| Entity::new(g, LayerId::ZERO))
        .collect();
    session.apply_external(Box::new(AddEntities::many("TEST", entities)), doc);
    let ids: Vec<EntityId> = doc.entities().ids().collect();
    for id in &ids {
        session.selection.insert(*id);
    }
    h.run_steps(5);
    ids
}

fn screen(h: &Harness<'_, CadApp>, at: Point2) -> egui::Pos2 {
    h.state().viewport().model_to_screen(at)
}

/// モデル座標の点（から画面上で `off` ずらした位置）に乗せる。
fn hover_model(h: &mut Harness<'_, CadApp>, at: Point2, off: egui::Vec2) {
    let pos = screen(h, at) + off;
    hover(h, pos);
}

/// モデル座標の点をクリックする。
fn click_model(h: &mut Harness<'_, CadApp>, at: Point2) {
    let pos = screen(h, at);
    click(h, pos);
}

/// 線分・円・円弧・ポリライン（矩形）を選ぶ。
fn kinds() -> Harness<'static, CadApp> {
    let mut h = harness();
    hover(&mut h, CANVAS_CENTER);
    add_and_select(
        &mut h,
        vec![
            Geometry::Line(Line::new(p(40.0, 60.0), p(150.0, 120.0))),
            Geometry::Circle(Circle::new(p(230.0, 90.0), 40.0)),
            Geometry::Arc(Arc::new(p(340.0, 80.0), 40.0, 0.2, 2.6)),
            Geometry::Polyline(Polyline::rectangle(p(60.0, 170.0), p(160.0, 240.0))),
        ],
    );
    h
}

/// 種類ごとのグリップ（青の四角）。線分は両端と中点、円は中心と四分点、円弧は両端・中点・中心、
/// 矩形は頂点。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_grips_kinds() {
    let mut h = kinds();
    // カーソルは何も無い所に置く。
    hover_model(&mut h, p(250.0, 220.0), egui::Vec2::ZERO);
    shot(&mut h, "grips_a_kinds");
}

/// グリップに乗せる → 大きく紫になり、右上に「端点を動かす」。四分点では「半径を変える」、
/// 中点では「図形ごと移動」。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_grips_hover() {
    let mut h = kinds();
    hover_model(&mut h, p(150.0, 120.0), egui::vec2(2.0, 1.0));
    shot(&mut h, "grips_b_hover_line_end");
    hover_model(&mut h, p(270.0, 90.0), egui::vec2(-1.0, 2.0));
    shot(&mut h, "grips_c_hover_quadrant");
    hover_model(&mut h, p(95.0, 90.0), egui::Vec2::ZERO);
    shot(&mut h, "grips_d_hover_line_mid");
}

/// 掴んで動かしているとき → 掴んだグリップは赤、仮の形は琥珀色。円の四分点では半径が変わる。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_grips_dragging() {
    let mut h = kinds();
    click_model(&mut h, p(150.0, 120.0));
    hover_model(&mut h, p(190.0, 200.0), egui::Vec2::ZERO);
    shot(&mut h, "grips_e_moving_line_end");
    press(&mut h, egui::Key::Escape);

    click_model(&mut h, p(270.0, 90.0));
    hover_model(&mut h, p(290.0, 90.0), egui::Vec2::ZERO);
    shot(&mut h, "grips_f_moving_quadrant");
    press(&mut h, egui::Key::Escape);

    // 円弧の端点 → 中点ともう一方の端点を通る 3 点円弧。
    let arc = Arc::new(p(340.0, 80.0), 40.0, 0.2, 2.6);
    click_model(&mut h, arc.start_point());
    hover_model(&mut h, p(400.0, 40.0), egui::Vec2::ZERO);
    shot(&mut h, "grips_g_moving_arc_end");
}

/// 選択が 100 個を超える → グリップは出ず、ステータスバーに「選択 N（グリップなし）」。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_grips_too_many() {
    let mut h = harness();
    hover(&mut h, CANVAS_CENTER);
    let geoms = (0..150_u32)
        .map(|i| {
            let x = f64::from(i % 15) * 25.0 + 20.0;
            let y = f64::from(i / 15) * 25.0 + 20.0;
            Geometry::Line(Line::new(p(x, y), p(x + 15.0, y + 10.0)))
        })
        .collect();
    add_and_select(&mut h, geoms);
    hover(&mut h, CANVAS_CENTER);
    shot(&mut h, "grips_h_too_many");
}

// ---- 段階 2 ------------------------------------------------------------------

/// 線分 4 本の矩形と、ポリラインの矩形を選ぶ。
fn shared_and_edges() -> Harness<'static, CadApp> {
    let mut h = harness();
    hover(&mut h, CANVAS_CENTER);
    add_and_select(
        &mut h,
        vec![
            Geometry::Line(Line::new(p(60.0, 60.0), p(180.0, 60.0))),
            Geometry::Line(Line::new(p(180.0, 60.0), p(180.0, 140.0))),
            Geometry::Line(Line::new(p(180.0, 140.0), p(60.0, 140.0))),
            Geometry::Line(Line::new(p(60.0, 140.0), p(60.0, 60.0))),
            Geometry::Polyline(Polyline::rectangle(p(260.0, 60.0), p(380.0, 140.0))),
        ],
    );
    h
}

/// 重なったグリップ → 線分 4 本の矩形の角は 1 つの四角。乗せると 1 つの紫の四角と
/// 「端点を動かす（2 個）」。ポリラインには頂点と辺の中点のグリップ。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_grips_shared_corner() {
    let mut h = shared_and_edges();
    hover_model(&mut h, p(220.0, 200.0), egui::Vec2::ZERO);
    shot(&mut h, "grips_i_shared_and_edges");
    hover_model(&mut h, p(180.0, 60.0), egui::vec2(2.0, 1.0));
    shot(&mut h, "grips_j_hover_shared_corner");
}

/// 重なったグリップを掴んで動かしているとき → 隣り合う 2 本の仮の形が琥珀色で、角でつながっている。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_grips_moving_shared_corner() {
    let mut h = shared_and_edges();
    click_model(&mut h, p(180.0, 60.0));
    hover_model(&mut h, p(215.0, 35.0), egui::Vec2::ZERO);
    shot(&mut h, "grips_k_moving_shared_corner");
}

/// ポリラインの辺の中点 → 乗せると「辺を動かす」、掴んで動かすと辺が平行に動いた仮の形。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_grips_polyline_edge() {
    let mut h = shared_and_edges();
    hover_model(&mut h, p(380.0, 100.0), egui::vec2(1.0, 1.0));
    shot(&mut h, "grips_l_hover_edge");
    click_model(&mut h, p(380.0, 100.0));
    hover_model(&mut h, p(430.0, 110.0), egui::Vec2::ZERO);
    shot(&mut h, "grips_m_moving_edge");
}

/// 円の四分点を掴む → カーソル横の寸法入力は「半径」の 1 欄（角度の欄は無い）。
#[test]
#[ignore = "GPU(またはソフトウェア Vulkan)が必要。--ignored で明示実行する"]
fn ui_snapshot_grips_quadrant_radius_field() {
    let mut h = kinds();
    click_model(&mut h, p(270.0, 90.0));
    hover_model(&mut h, p(295.0, 70.0), egui::Vec2::ZERO);
    shot(&mut h, "grips_n_quadrant_radius_field");
}
