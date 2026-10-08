//! 図面を入れ替えたとき（NEW / OPEN）、版番号をキーにした派生データが前の図面の値を使わないこと
//! （Issue #65）。
//!
//! `Document::revision()` は同じ図面の中でしか比べられない。読み込んだ図面の版番号は
//! コマンドの適用回数で決まり中身によらないので、別の図面どうしで一致しうる。
//! ここでは「同じ版番号・別の中身」の図面へ差し替えて確かめる。

use cad_core::command::{AddEntities, DefineComponent, InsertInstance};
use cad_core::component::Placement;
use cad_core::geom::{Line, Point2};
use cad_core::{Document, Entity, EntityId, Geometry, LayerId};
use egui_kittest::Harness;

use super::{app, hover, press, settle, type_text, CadApp};

fn seg(x0: f64, y0: f64, x1: f64, y1: f64) -> Line {
    Line::new(Point2::new(x0, y0), Point2::new(x1, y1))
}

/// 線分 1 本だけの図面。
fn doc_with_line(l: Line) -> Document {
    let mut doc = Document::new();
    doc.apply(Box::new(AddEntities::one(
        "TEST",
        Entity::new(Geometry::Line(l), LayerId::ZERO),
    )))
    .expect("追加できるはず");
    doc
}

/// 線分 1 本の定義と、それを `at` に置いたインスタンス 1 つの図面。
fn doc_with_instance(contents: Line, at: Point2) -> (Document, EntityId) {
    let mut doc = Document::new();
    doc.apply(Box::new(DefineComponent::new(
        "COMPONENT",
        "部品",
        Point2::ORIGIN,
        vec![Entity::new(Geometry::Line(contents), LayerId::ZERO)],
    )))
    .expect("定義を作れるはず");
    let def = doc.definitions().by_name("部品").expect("作ったはず");
    doc.apply(Box::new(InsertInstance::new(
        "INSERT",
        def,
        Placement::at(at),
        LayerId::ZERO,
    )))
    .expect("配置できるはず");
    let id = doc.entities().ids().next().expect("1 件あるはず");
    (doc, id)
}

/// ファイル操作が図面を入れ替えたときと同じ入口。
fn replace_drawing(h: &mut Harness<'_, CadApp>, next: Document) {
    let app = h.state_mut();
    app.doc = next;
    app.report_file_outcome(crate::file_ops::FileOutcome::Replaced(
        "開きました".to_owned(),
    ));
    settle(h);
}

fn screen(h: &Harness<'_, CadApp>, p: Point2) -> egui::Pos2 {
    h.state().viewport.model_to_screen(p)
}

/// LINE を始めて、点の入力待ちにする。
fn start_line(h: &mut Harness<'_, CadApp>) {
    type_text(h, "L");
    press(h, egui::Key::Enter);
    assert!(h.state().session.has_active_tool(), "前提: LINE 実行中");
}

/// 点の入力待ちで `p` の近く（3px ずらし）にカーソルを置いて、吸着した点を返す。
fn snapped_near(h: &mut Harness<'_, CadApp>, p: Point2) -> Option<Point2> {
    let pos = screen(h, p) + egui::vec2(3.0, 3.0);
    hover(h, pos);
    h.state().snapped.map(|s| s.point)
}

/// 前の図面でスナップの索引を作った後に、版番号が同じ別の図面へ入れ替えたら、
/// 新しい図面の点に吸い付き、前の図面の点には吸い付かない。
#[test]
fn snap_uses_the_new_drawings_points_after_replacing() {
    let mut h = app();
    hover(&mut h, egui::pos2(500.0, 300.0));
    let old = doc_with_line(seg(50.0, 100.0, 150.0, 100.0));
    let new = doc_with_line(seg(50.0, 250.0, 150.0, 250.0));
    assert_eq!(old.revision(), new.revision(), "前提: 版番号が同じ");
    h.state_mut().doc = old;
    settle(&mut h);

    // 前の図面の端点に吸着させて、索引を前の図面で作らせる。
    start_line(&mut h);
    let got = snapped_near(&mut h, Point2::new(50.0, 100.0));
    assert_eq!(got, Some(Point2::new(50.0, 100.0)), "前提: 前の図面の端点");
    press(&mut h, egui::Key::Escape);

    replace_drawing(&mut h, new);
    start_line(&mut h);

    let got = snapped_near(&mut h, Point2::new(50.0, 250.0));
    assert_eq!(
        got,
        Some(Point2::new(50.0, 250.0)),
        "新しい図面の端点に吸い付く"
    );
    let got = snapped_near(&mut h, Point2::new(50.0, 100.0));
    assert_eq!(got, None, "前の図面の端点には吸い付かない: {got:?}");
}

/// インスタンスの展開結果と、ホバーでのピックが、入れ替えた後の図面の定義で行われる。
#[test]
fn instances_resolve_from_the_new_drawings_definition_after_replacing() {
    let mut h = app();
    hover(&mut h, egui::pos2(500.0, 300.0));
    let (old, old_id) = doc_with_instance(seg(0.0, 0.0, 100.0, 0.0), Point2::new(50.0, 100.0));
    let (new, new_id) = doc_with_instance(seg(0.0, 0.0, 100.0, 0.0), Point2::new(50.0, 250.0));
    assert_eq!(old.revision(), new.revision(), "前提: 版番号が同じ");
    assert_eq!(old_id, new_id, "前提: 同じ ID");
    h.state_mut().doc = old;
    settle(&mut h);
    let before = h.state().resolved.get(old_id).expect("展開されている");
    assert!(
        matches!(&before[0], Geometry::Line(l) if cad_core::geom::tolerance::eq_len(l.a.y, 100.0)),
        "前提: 前の図面の位置で展開されている: {before:?}"
    );

    replace_drawing(&mut h, new);

    let after = h.state().resolved.get(new_id).expect("展開されている");
    assert!(
        matches!(&after[0], Geometry::Line(l) if cad_core::geom::tolerance::eq_len(l.a.y, 250.0)),
        "新しい図面の定義と置き場所で展開される: {after:?}"
    );

    // ホバーの強調（ピック）も新しい図面のインスタンスを拾う。
    let pos = screen(&h, Point2::new(100.0, 250.0));
    hover(&mut h, pos);
    assert_eq!(h.state().hover.highlighted(), [new_id], "新しい位置を拾う");
}
