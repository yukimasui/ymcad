//! TRIM / EXTEND の結果プレビュー（Issue #34 段階 2、ADR-0043）。
//!
//! アプリ全体を動かし、乗せたときのプレビューと、そこでクリックした後の図面が一致することを確かめる。

use cad_core::command::AddEntities;
use cad_core::geom::{Circle, Line, Point2};
use cad_core::{EntityId, Geometry, LayerId};
use egui_kittest::Harness;

use super::{app, click, hover, press, settle, type_text, CadApp};
use crate::tools::EntityPreview;

fn seg(x0: f64, y0: f64, x1: f64, y1: f64) -> Line {
    Line::new(Point2::new(x0, y0), Point2::new(x1, y1))
}

/// 図形をまとめて 1 回の `AddEntities` で足す（版番号が 1 つだけ進む）。足した ID を返す。
fn add(h: &mut Harness<'_, CadApp>, geoms: Vec<Geometry>) -> Vec<EntityId> {
    let before: Vec<EntityId> = h.state().doc.entities().ids().collect();
    let entities = geoms
        .into_iter()
        .map(|g| cad_core::Entity::new(g, LayerId::ZERO))
        .collect();
    {
        let app = h.state_mut();
        app.session
            .apply_external(Box::new(AddEntities::many("TEST", entities)), &mut app.doc);
    }
    settle(h);
    h.state()
        .doc
        .entities()
        .ids()
        .filter(|id| !before.contains(id))
        .collect()
}

/// モデル座標の点の画面座標。履歴で作図領域が縮むので、使う直前に取る。
fn screen(h: &Harness<'_, CadApp>, p: Point2) -> egui::Pos2 {
    h.state().viewport.model_to_screen(p)
}

/// モデル座標の点をクリックする。
fn click_at(h: &mut Harness<'_, CadApp>, p: Point2) {
    let pos = screen(h, p);
    click(h, pos);
}

/// `p` に乗せて、そのときの強調とプレビューを返す。
fn hover_at(h: &mut Harness<'_, CadApp>, p: Point2) -> (Vec<EntityId>, Option<EntityPreview>) {
    let pos = screen(h, p);
    hover(h, pos);
    let state = h.state();
    (
        state.hover.highlighted().to_vec(),
        state.hover.entity_preview().cloned(),
    )
}

/// 図面の線分（向きを揃えて x, y の順に並べる）。
fn lines(h: &Harness<'_, CadApp>) -> Vec<Line> {
    let mut got: Vec<Line> = h
        .state()
        .doc
        .entities()
        .iter()
        .filter_map(|(_, e)| match e.geom {
            Geometry::Line(l) => Some(normalized(l)),
            _ => None,
        })
        .collect();
    got.sort_by(|p, q| {
        (p.a.x, p.a.y)
            .partial_cmp(&(q.a.x, q.a.y))
            .expect("NaN なし")
    });
    got
}

/// 始点が左下になるよう向きを揃える。
fn normalized(l: Line) -> Line {
    if (l.a.x, l.a.y) <= (l.b.x, l.b.y) {
        l
    } else {
        Line::new(l.b, l.a)
    }
}

fn start(h: &mut Harness<'_, CadApp>, command: &str) {
    type_text(h, command);
    press(h, egui::Key::Enter);
}

fn removed_line(p: &EntityPreview) -> Line {
    match p.removed.as_slice() {
        [Geometry::Line(l)] => normalized(*l),
        other => panic!("消える部分は線分 1 本のはず: {other:?}"),
    }
}

/// TRIM: 乗せると消える部分（交点から右端まで）が出て、クリックするとその部分が消える。
/// 両側に交点がある所では間が出て、クリックで 2 本に分かれる。
#[test]
fn trim_preview_matches_the_click() {
    let mut h = app();
    hover(&mut h, egui::pos2(500.0, 300.0));
    let ids = add(
        &mut h,
        vec![
            Geometry::Line(seg(50.0, 150.0, 250.0, 150.0)),
            Geometry::Line(seg(100.0, 50.0, 100.0, 250.0)),
            Geometry::Line(seg(200.0, 50.0, 200.0, 250.0)),
        ],
    );
    start(&mut h, "TR");

    // 右端側（交点 200 の右）。
    let (shown, preview) = hover_at(&mut h, Point2::new(225.0, 150.0));
    assert_eq!(shown, vec![ids[0]], "前提: 横の線分を強調");
    let preview = preview.expect("消える部分が出る");
    assert!(preview.added.is_empty());
    assert_eq!(removed_line(&preview), seg(200.0, 150.0, 250.0, 150.0));
    click_at(&mut h, Point2::new(225.0, 150.0));
    assert_eq!(
        lines(&h),
        vec![
            seg(50.0, 150.0, 200.0, 150.0),
            seg(100.0, 50.0, 100.0, 250.0),
            seg(200.0, 50.0, 200.0, 250.0),
        ],
        "プレビューどおり右端が消えた"
    );

    // 両側に交点がある間（100〜200）。切った後の線分（新しい ID）に乗せる。
    let (_, preview) = hover_at(&mut h, Point2::new(150.0, 150.0));
    let preview = preview.expect("間が出る");
    assert_eq!(removed_line(&preview), seg(100.0, 150.0, 200.0, 150.0));
    click_at(&mut h, Point2::new(150.0, 150.0));
    assert_eq!(
        lines(&h),
        vec![
            seg(50.0, 150.0, 100.0, 150.0),
            seg(100.0, 50.0, 100.0, 250.0),
            seg(200.0, 50.0, 200.0, 250.0),
        ],
        "プレビューどおり間が消えた"
    );
}

/// EXTEND: 乗せると伸びる部分（元の端から交点まで）が出て、クリックするとそこまで伸びる。
#[test]
fn extend_preview_matches_the_click() {
    let mut h = app();
    hover(&mut h, egui::pos2(500.0, 300.0));
    let ids = add(
        &mut h,
        vec![
            Geometry::Line(seg(50.0, 150.0, 150.0, 150.0)),
            Geometry::Line(seg(200.0, 50.0, 200.0, 250.0)),
        ],
    );
    start(&mut h, "EX");

    let (shown, preview) = hover_at(&mut h, Point2::new(140.0, 150.0));
    assert_eq!(shown, vec![ids[0]], "前提: 横の線分を強調");
    let preview = preview.expect("伸びる部分が出る");
    assert!(preview.removed.is_empty());
    assert_eq!(
        preview.added,
        vec![Geometry::Line(seg(150.0, 150.0, 200.0, 150.0))],
        "元の端（150）から交点（200）まで"
    );
    click_at(&mut h, Point2::new(140.0, 150.0));
    assert_eq!(
        lines(&h),
        vec![
            seg(50.0, 150.0, 200.0, 150.0),
            seg(200.0, 50.0, 200.0, 250.0),
        ],
        "プレビューどおり伸びた"
    );

    // 左端側には何も無いので伸ばせない → 強調だけ出て、プレビューは出ない。
    let (shown, preview) = hover_at(&mut h, Point2::new(60.0, 150.0));
    assert_eq!(shown, vec![ids[0]], "強調は出る");
    assert_eq!(preview, None, "伸ばす先が無ければ出ない");
}

/// 線分以外（円）と、交点の無い線分には、強調だけが出て結果プレビューは出ない。
#[test]
fn only_the_highlight_for_non_lines_and_lines_without_crossings() {
    let mut h = app();
    hover(&mut h, egui::pos2(500.0, 300.0));
    let ids = add(
        &mut h,
        vec![
            Geometry::Circle(Circle::new(Point2::new(100.0, 150.0), 40.0)),
            Geometry::Line(seg(40.0, 150.0, 160.0, 150.0)),
            Geometry::Line(seg(300.0, 150.0, 400.0, 150.0)),
            // 円の内側の短い線分（EXTEND で円まで伸ばせる）。
            Geometry::Line(seg(105.0, 160.0, 125.0, 160.0)),
        ],
    );
    // 前提の確認に使う、プレビューが出る位置（TRIM は円を貫く線分、EXTEND は円の内側の線分）。
    for (command, works_at) in [
        ("TR", Point2::new(150.0, 150.0)),
        ("EX", Point2::new(122.0, 160.0)),
    ] {
        start(&mut h, command);
        let (shown, preview) = hover_at(&mut h, Point2::new(100.0, 190.0));
        assert_eq!(shown, vec![ids[0]], "{command}: 円を強調");
        assert_eq!(preview, None, "{command}: 円には出ない");
        let (shown, preview) = hover_at(&mut h, Point2::new(350.0, 150.0));
        assert_eq!(shown, vec![ids[2]], "{command}: 離れた線分を強調");
        assert_eq!(preview, None, "{command}: 交点が無ければ出ない");
        // 円が境界になる線分には出る（前提の確認）。
        let (_, preview) = hover_at(&mut h, works_at);
        assert!(preview.is_some(), "{command}: 円が境界になる線分には出る");
        press(&mut h, egui::Key::Escape);
    }
}

/// TRIM した後に Undo しても、戻った図面でプレビューが正しく出る（境界も戻っている）。
#[test]
fn the_preview_is_right_after_undo() {
    let mut h = app();
    hover(&mut h, egui::pos2(500.0, 300.0));
    add(
        &mut h,
        vec![
            Geometry::Line(seg(50.0, 150.0, 250.0, 150.0)),
            Geometry::Line(seg(100.0, 50.0, 100.0, 250.0)),
            Geometry::Line(seg(200.0, 50.0, 200.0, 250.0)),
        ],
    );
    start(&mut h, "TR");
    // 縦の線分（x = 200）の上半分を切る。下半分が横の線分に端で接したまま残り、境界であり続ける。
    click_at(&mut h, Point2::new(200.0, 200.0));
    let (_, preview) = hover_at(&mut h, Point2::new(225.0, 150.0));
    assert_eq!(
        removed_line(&preview.expect("切った後も出る")),
        seg(200.0, 150.0, 250.0, 150.0)
    );
    // 縦の線分（x = 200）の下半分も切ると、x = 200 には境界が無くなる。
    click_at(&mut h, Point2::new(200.0, 100.0));
    let (_, preview) = hover_at(&mut h, Point2::new(225.0, 150.0));
    assert_eq!(
        removed_line(&preview.expect("x = 100 まで消える")),
        seg(100.0, 150.0, 250.0, 150.0),
        "消した境界は使わない"
    );

    // Undo（Ctrl+Z ではなくコマンドで。TRIM は中断される）。
    press(&mut h, egui::Key::Escape);
    start(&mut h, "U");
    start(&mut h, "TR");
    let (_, preview) = hover_at(&mut h, Point2::new(225.0, 150.0));
    assert_eq!(
        removed_line(&preview.expect("Undo の後も出る")),
        seg(200.0, 150.0, 250.0, 150.0),
        "戻った境界（下半分）を使う"
    );
    click_at(&mut h, Point2::new(225.0, 150.0));
    assert!(
        lines(&h).contains(&seg(50.0, 150.0, 200.0, 150.0)),
        "クリックの結果もプレビューどおり: {:?}",
        lines(&h)
    );
}

/// 図面を入れ替えたら（NEW / OPEN）、前の図面の境界を使わない。
///
/// 境界の列は `(版番号, 対象の ID)` を鍵にしているが、版番号は図面をまたいで比べられない（#65）。
/// 同じ版番号・同じ ID の線分で、境界の位置だけが違う図面に入れ替えて確かめる。
#[test]
fn replacing_the_drawing_drops_the_old_boundaries() {
    let mut h = app();
    hover(&mut h, egui::pos2(500.0, 300.0));
    let old = add(
        &mut h,
        vec![
            Geometry::Line(seg(50.0, 150.0, 250.0, 150.0)),
            Geometry::Line(seg(200.0, 50.0, 200.0, 250.0)),
        ],
    );
    start(&mut h, "TR");
    let (_, preview) = hover_at(&mut h, Point2::new(225.0, 150.0));
    assert_eq!(
        removed_line(&preview.expect("前提: 前の図面で出る")),
        seg(200.0, 150.0, 250.0, 150.0)
    );

    // 同じ版番号・同じ ID の横の線分で、縦の線分だけが x = 100 にある図面。
    let mut next = cad_core::Document::new();
    next.apply(Box::new(AddEntities::many(
        "TEST",
        [
            seg(50.0, 150.0, 250.0, 150.0),
            seg(100.0, 50.0, 100.0, 250.0),
        ]
        .into_iter()
        .map(|l| cad_core::Entity::new(Geometry::Line(l), LayerId::ZERO))
        .collect(),
    )))
    .expect("追加できるはず");
    assert_eq!(
        next.revision(),
        h.state().doc.revision(),
        "前提: 版番号が同じ"
    );
    assert_eq!(
        next.entities().ids().next(),
        Some(old[0]),
        "前提: 横の線分の ID が同じ"
    );
    {
        let app = h.state_mut();
        app.doc = next;
        app.report_file_outcome(crate::file_ops::FileOutcome::Replaced(
            "開きました".to_owned(),
        ));
    }
    settle(&mut h);
    // 入れ替えでツールは中断されるので、もう一度始める。
    start(&mut h, "TR");
    let (shown, preview) = hover_at(&mut h, Point2::new(225.0, 150.0));
    assert_eq!(shown, vec![old[0]], "前提: 同じ ID の横の線分を強調");
    assert_eq!(
        removed_line(&preview.expect("新しい図面で出る")),
        seg(100.0, 150.0, 250.0, 150.0),
        "新しい図面の境界（x = 100）を使う"
    );
}
