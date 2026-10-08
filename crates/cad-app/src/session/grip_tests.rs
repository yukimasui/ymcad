//! グリップ編集（Issue #30 段階 1、ADR-0045）の `Session` 単体の振る舞い。
//!
//! 形の計算そのものは `grips` のテスト、実際のクリック・キーは `app/behavior_tests/grips.rs`。

use super::*;
use crate::grips::Handle;
use cad_core::command::{AddEntities, AddLayer, SetLayerProperties};
use cad_core::geom::{Arc, Circle, Line};
use cad_core::{AciColor, Entity, LayerId};

/// クリックの拾い半径（モデル空間）。グリップの当たり（正方形の半分の辺）も同じ。
const PICK: f64 = 0.5;

fn p(x: f64, y: f64) -> Point2 {
    Point2::new(x, y)
}

fn feed(s: &mut Session, doc: &mut Document, text: &str) {
    s.handle_submission(Submission::Text(text.to_owned()), doc);
}

fn enter(s: &mut Session, doc: &mut Document) {
    s.handle_submission(Submission::Empty, doc);
}

fn esc(s: &mut Session, doc: &mut Document) {
    s.handle_submission(Submission::Cancel, doc);
}

fn click(s: &mut Session, doc: &mut Document, at: Point2) {
    s.handle_click(at, false, PICK, doc, &mut selection::ScanAll);
}

fn add(doc: &mut Document, geom: Geometry) -> EntityId {
    doc.apply(Box::new(AddEntities::one(
        "LINE",
        Entity::new(geom, LayerId::ZERO),
    )))
    .expect("足せる");
    doc.entities().ids().last().expect("足した図形")
}

fn line_geom(a: Point2, b: Point2) -> Geometry {
    Geometry::Line(Line::new(a, b))
}

/// (0,0)-(10,0) の線分を足して選んだ状態。
fn selected_line() -> (Session, Document, EntityId) {
    let mut doc = Document::new();
    let id = add(&mut doc, line_geom(p(0.0, 0.0), p(10.0, 0.0)));
    let mut s = Session::new();
    s.selection.insert(id);
    (s, doc, id)
}

fn geom(doc: &Document, id: EntityId) -> Geometry {
    doc.entities().get(id).expect("ある").geom.clone()
}

fn errors(s: &Session) -> Vec<String> {
    s.cmdline
        .history()
        .filter(|l| l.kind == LineKind::Error)
        .map(|l| l.text.clone())
        .collect()
}

fn inputs(s: &Session) -> Vec<String> {
    s.cmdline
        .history()
        .filter(|l| l.kind == LineKind::Input)
        .map(|l| l.text.clone())
        .collect()
}

// ---- 掴む・確定・Undo ------------------------------------------------------

/// 端点のグリップをクリックで掴み、次のクリックで確定する。選択は残り、Undo 1 回で戻る。
#[test]
fn grab_and_confirm_then_one_undo_restores() {
    let (mut s, mut doc, id) = selected_line();
    click(&mut s, &mut doc, p(10.1, 0.1));
    assert!(s.is_gripping(), "端点のグリップを掴んだ");
    assert_eq!(s.active_command(), Some("GRIP"));
    assert_eq!(
        s.hot_grips().first().map(|g| g.handle),
        Some(Handle::LineEnd)
    );

    click(&mut s, &mut doc, p(20.0, 5.0));
    assert!(!s.is_gripping(), "確定したら終わる");
    assert_eq!(geom(&doc, id), line_geom(p(0.0, 0.0), p(20.0, 5.0)));
    assert!(s.selection.contains(id), "選択は残る");
    assert!(s.grips_enabled(), "続けて掴める");

    assert_eq!(
        doc.undo().expect("戻せる"),
        Some("GRIP"),
        "1 回の操作 = Undo 1 回"
    );
    assert_eq!(geom(&doc, id), line_geom(p(0.0, 0.0), p(10.0, 0.0)));
}

/// 中点のグリップは図形ごと移動。
#[test]
fn the_midpoint_grip_moves_the_whole_line() {
    let (mut s, mut doc, id) = selected_line();
    click(&mut s, &mut doc, p(5.0, 0.0));
    assert_eq!(s.hot_grips()[0].handle, Handle::LineMid);
    click(&mut s, &mut doc, p(5.0, 3.0));
    assert_eq!(geom(&doc, id), line_geom(p(0.0, 3.0), p(10.0, 3.0)));
}

/// 元の位置でクリックしても形は変わらず、履歴も増えない。
#[test]
fn a_click_that_changes_nothing_adds_no_history() {
    let (mut s, mut doc, id) = selected_line();
    let revision = doc.revision();
    click(&mut s, &mut doc, p(10.0, 0.0));
    assert!(s.is_gripping());
    click(&mut s, &mut doc, p(10.0, 0.0));
    assert!(!s.is_gripping(), "終わる");
    assert_eq!(doc.revision(), revision, "図面は変わらない");
    assert_eq!(doc.undo().expect("戻せる"), Some("LINE"), "Undo は前の操作");
    assert!(doc.entities().get(id).is_none());
}

/// 断られる位置（線分の長さ 0）では点を入れず、掴んだまま。
#[test]
fn a_refused_position_keeps_the_grip() {
    let (mut s, mut doc, id) = selected_line();
    click(&mut s, &mut doc, p(10.0, 0.0));
    click(&mut s, &mut doc, p(0.0, 0.0));
    assert!(s.is_gripping(), "掴んだまま");
    assert!(
        errors(&s).iter().any(|e| e.contains("長さが 0")),
        "{:?}",
        errors(&s)
    );
    assert_eq!(geom(&doc, id), line_geom(p(0.0, 0.0), p(10.0, 0.0)));
}

/// 円弧の両端が重なる位置は「円弧の端点が重なります」で断る（1 周の円弧にしない）。
#[test]
fn an_arc_end_onto_the_other_end_is_refused() {
    let mut doc = Document::new();
    let arc = Arc::new(p(0.0, 0.0), 5.0, 0.0, std::f64::consts::PI);
    let id = add(&mut doc, Geometry::Arc(arc));
    let mut s = Session::new();
    s.selection.insert(id);
    click(&mut s, &mut doc, arc.end_point());
    assert_eq!(s.hot_grips()[0].handle, Handle::ArcEnd);
    click(&mut s, &mut doc, arc.start_point());
    assert!(s.is_gripping());
    assert!(
        errors(&s)
            .iter()
            .any(|e| e.contains("円弧の端点が重なります")),
        "{:?}",
        errors(&s)
    );
    assert_eq!(geom(&doc, id), Geometry::Arc(arc));
}

// ---- 打ち込み ----------------------------------------------------------------

/// 長さだけ打つと、反対側の端点からカーソルの向きにその長さ（形の基準から。ユーザー判断 1）。
#[test]
fn direct_distance_measures_from_the_other_end() {
    let (mut s, mut doc, id) = selected_line();
    click(&mut s, &mut doc, p(10.0, 0.0));
    assert_eq!(s.dimension_base(), Some(p(0.0, 0.0)), "基点は反対側の端点");
    assert_eq!(s.tracking_base(), Some(p(0.0, 0.0)), "直交・極も同じ基点");
    s.set_cursor(Some(p(3.0, 4.0)));
    feed(&mut s, &mut doc, "100");
    assert_eq!(geom(&doc, id), line_geom(p(0.0, 0.0), p(60.0, 80.0)));
}

/// `@` は掴んだ点の元の位置から。
#[test]
fn relative_coordinates_start_from_the_grabbed_point() {
    let (mut s, mut doc, id) = selected_line();
    click(&mut s, &mut doc, p(10.0, 0.0));
    feed(&mut s, &mut doc, "@5,2");
    assert_eq!(geom(&doc, id), line_geom(p(0.0, 0.0), p(15.0, 2.0)));
}

/// 円の四分点は直交・極を外す（基点は中心で、長さ = 半径）。
#[test]
fn a_quadrant_is_not_tracked_but_measures_from_the_center() {
    let mut doc = Document::new();
    let id = add(&mut doc, Geometry::Circle(Circle::new(p(0.0, 0.0), 5.0)));
    let mut s = Session::new();
    s.selection.insert(id);
    click(&mut s, &mut doc, p(5.0, 0.0));
    assert!(matches!(s.hot_grips()[0].handle, Handle::CircleQuadrant(0)));
    assert_eq!(s.tracking_base(), None, "直交・極は効かない");
    assert_eq!(s.dimension_base(), Some(p(0.0, 0.0)));
    s.set_cursor(Some(p(0.0, 1.0)));
    feed(&mut s, &mut doc, "7");
    assert_eq!(
        geom(&doc, id),
        Geometry::Circle(Circle::new(p(0.0, 0.0), 7.0))
    );
}

/// 点でも `U` でもコマンド名でもない文字は「点を指定するか Esc」で、掴んだまま。
#[test]
fn other_text_asks_for_a_point() {
    let (mut s, mut doc, _) = selected_line();
    click(&mut s, &mut doc, p(10.0, 0.0));
    feed(&mut s, &mut doc, "ほげ");
    assert!(s.is_gripping());
    assert!(errors(&s).iter().any(|e| e.contains("点を指定するか Esc")));
}

/// `U` はグリップの取り消し（図面の Undo はしない）。選択は残る。
#[test]
fn u_cancels_the_grip_without_undoing() {
    let (mut s, mut doc, id) = selected_line();
    let revision = doc.revision();
    click(&mut s, &mut doc, p(10.0, 0.0));
    feed(&mut s, &mut doc, "U");
    assert!(!s.is_gripping());
    assert_eq!(doc.revision(), revision, "Undo していない");
    assert!(doc.entities().get(id).is_some());
    assert!(s.selection.contains(id), "選択は残る");
}

/// コマンド名を打つと、グリップを取り消して（選択は残して）そのコマンドを始める。
#[test]
fn a_command_name_cancels_the_grip_and_starts_with_the_selection() {
    let (mut s, mut doc, id) = selected_line();
    click(&mut s, &mut doc, p(10.0, 0.0));
    feed(&mut s, &mut doc, "M");
    assert_eq!(s.active_command(), Some("MOVE"));
    assert!(s.selection.contains(id), "選択は残る");
    assert!(s.wants_point(), "選択済みなので基点の指定へ進む");
}

// ---- Esc・空の Enter -------------------------------------------------------

/// Esc の 1 回目はグリップだけ、2 回目で選択を解除する（ユーザー判断 3）。
#[test]
fn escape_cancels_the_grip_first_then_the_selection() {
    let (mut s, mut doc, id) = selected_line();
    click(&mut s, &mut doc, p(10.0, 0.0));
    esc(&mut s, &mut doc);
    assert!(!s.is_gripping());
    assert!(s.selection.contains(id), "1 回目は選択を残す");
    esc(&mut s, &mut doc);
    assert!(s.selection.is_empty(), "2 回目で選択を解除");
}

/// 空の Enter はグリップの取り消し。GRIP は再実行の対象にならない（その後の空の Enter は前のコマンド）。
#[test]
fn empty_enter_cancels_and_never_repeats_the_grip() {
    let (mut s, mut doc, id) = selected_line();
    feed(&mut s, &mut doc, "LINE");
    esc(&mut s, &mut doc);
    s.selection.insert(id);

    click(&mut s, &mut doc, p(10.0, 0.0));
    assert!(s.is_gripping());
    enter(&mut s, &mut doc);
    assert!(!s.is_gripping(), "取り消し");
    assert!(s.selection.contains(id), "選択は残る");

    enter(&mut s, &mut doc);
    assert_eq!(s.active_command(), Some("LINE"), "再実行は前のコマンド");
    assert!(
        !inputs(&s).iter().any(|l| l.contains("GRIP")),
        "{:?}",
        inputs(&s)
    );
}

/// 確定した後の空の Enter も GRIP を繰り返さない。
#[test]
fn empty_enter_after_a_grip_edit_does_not_repeat_it() {
    let (mut s, mut doc, id) = selected_line();
    click(&mut s, &mut doc, p(10.0, 0.0));
    click(&mut s, &mut doc, p(12.0, 0.0));
    let revision = doc.revision();
    enter(&mut s, &mut doc);
    assert!(!s.has_active_tool());
    assert_eq!(doc.revision(), revision);
    assert_eq!(geom(&doc, id), line_geom(p(0.0, 0.0), p(12.0, 0.0)));
}

// ---- 出ない場面 ------------------------------------------------------------

/// コマンドの実行中と選択待ちにはグリップが出ず、その位置のクリックは掴まない。
#[test]
fn no_grips_while_a_command_runs_or_waits_for_a_selection() {
    let (mut s, mut doc, id) = selected_line();
    assert!(!s.grips(&doc).is_empty(), "前提: 待機中は出る");

    feed(&mut s, &mut doc, "LINE");
    assert!(s.grips(&doc).is_empty(), "LINE の実行中");
    assert!(!s.grips_enabled());
    assert_eq!(
        s.click_target(p(10.0, 0.0), PICK, false, &doc, &mut selection::ScanAll),
        ClickTarget::Point(p(10.0, 0.0)),
        "点として渡る"
    );
    esc(&mut s, &mut doc);

    // 選択待ち（ERASE を空の選択で始めてから選ぶ）。
    feed(&mut s, &mut doc, "ERASE");
    click(&mut s, &mut doc, p(5.0, 0.0));
    assert!(s.selection.contains(id), "前提: 選択待ちで選んだ");
    assert_eq!(s.active_command(), Some("ERASE"));
    assert!(s.grips(&doc).is_empty(), "選択待ち");
    click(&mut s, &mut doc, p(10.0, 0.0));
    assert!(!s.is_gripping(), "選択待ちでは掴まない");
}

/// Shift を押している間はグリップを無視する（選択から外す操作になる）。
#[test]
fn shift_ignores_grips() {
    let (mut s, mut doc, id) = selected_line();
    assert!(matches!(
        s.click_target(p(10.0, 0.0), PICK, false, &doc, &mut selection::ScanAll),
        ClickTarget::Grip(_)
    ));
    assert_eq!(
        s.click_target(p(10.0, 0.0), PICK, true, &doc, &mut selection::ScanAll),
        ClickTarget::Select(vec![id])
    );
    s.handle_click(p(10.0, 0.0), true, PICK, &mut doc, &mut selection::ScanAll);
    assert!(!s.is_gripping());
    assert!(s.selection.is_empty(), "Shift + クリックで外れた");
}

/// 選択数が上限を超えるとグリップを出さない（ステータスバーで知らせる。ユーザー判断 4）。
#[test]
fn too_many_selected_entities_show_no_grips() {
    let mut doc = Document::new();
    let entities = (0..=MAX_GRIP_SELECTION)
        .map(|i| {
            let y = f64::from(u32::try_from(i).expect("小さい"));
            Entity::new(line_geom(p(0.0, y), p(10.0, y)), LayerId::ZERO)
        })
        .collect();
    doc.apply(Box::new(AddEntities::many("TEST", entities)))
        .expect("足せる");
    let mut s = Session::new();
    let ids: Vec<EntityId> = doc.entities().ids().collect();
    for id in &ids[..MAX_GRIP_SELECTION] {
        s.selection.insert(*id);
    }
    assert!(s.grips_enabled(), "上限ちょうどは出る");
    assert!(!s.grips_suppressed());

    s.selection.insert(ids[MAX_GRIP_SELECTION]);
    assert!(!s.grips_enabled(), "上限を超えたら出ない");
    assert!(s.grips_suppressed(), "知らせる");
    assert!(s.grips(&doc).is_empty());
    assert!(matches!(
        s.click_target(p(10.0, 0.0), PICK, false, &doc, &mut selection::ScanAll),
        ClickTarget::Select(_)
    ));
}

/// インプレース編集中、束縛（式）の付いた図形にはグリップを出さない。
#[test]
fn bound_entities_in_a_component_edit_have_no_grips() {
    use cad_core::command::{
        DefineComponent, EnterDefinitionEdit, InsertInstance, SetBinding, SetDefinitionParams,
    };
    use cad_core::component::{Binding, ParamDecl, Placement, Slot};

    let mut doc = Document::new();
    doc.apply(Box::new(DefineComponent::new(
        "COMPONENT",
        "窓",
        Point2::ORIGIN,
        vec![
            Entity::new(line_geom(p(0.0, 0.0), p(1.0, 0.0)), LayerId::ZERO),
            Entity::new(line_geom(p(5.0, 0.0), p(6.0, 0.0)), LayerId::ZERO),
        ],
    )))
    .expect("定義");
    let def = doc.definitions().by_name("窓").expect("ある");
    doc.apply(Box::new(SetDefinitionParams::new(
        "PARAM",
        def,
        vec![ParamDecl::number("幅", 1.0)],
    )))
    .expect("宣言");
    doc.apply(Box::new(SetBinding::new(
        "BIND",
        def,
        Binding::new(1, Slot::LineBx, cad_core::expr::parse("幅").expect("解析")),
    )))
    .expect("束縛");
    doc.apply(Box::new(InsertInstance::new(
        "INSERT",
        def,
        Placement::at(Point2::ORIGIN),
        LayerId::ZERO,
    )))
    .expect("配置");
    let inst = doc.entities().ids().next().expect("ある");
    let before: Vec<EntityId> = doc.entities().ids().collect();
    doc.apply(Box::new(EnterDefinitionEdit::new("EDITCOMP", inst)))
        .expect("編集に入る");
    let entered: Vec<EntityId> = doc
        .entities()
        .ids()
        .filter(|id| !before.contains(id))
        .collect();

    let mut s = Session::new();
    s.editing = Some(EditSession::new(
        &doc,
        def,
        Placement::at(Point2::ORIGIN),
        entered.clone(),
    ));
    s.selection.insert(entered[0]);
    s.selection.insert(entered[1]);
    let with_grips: Vec<EntityId> = s.grips(&doc).iter().map(|g| g.id).collect();
    assert!(with_grips.contains(&entered[0]), "束縛の無い中身には出る");
    assert!(
        !with_grips.contains(&entered[1]),
        "束縛の付いた中身には出ない"
    );
}

// ---- 前提が崩れたとき ------------------------------------------------------

/// 掴んでいる図形のレイヤがパネルでロックされたら中断する（ADR-0039 の `held_entities`）。
#[test]
fn locking_the_layer_aborts_the_grip() {
    let mut doc = Document::new();
    doc.apply(Box::new(AddLayer::new("L1", AciColor::WHITE)))
        .expect("足せる");
    let l1 = doc.layers().by_name("L1").expect("ある");
    doc.apply(Box::new(AddEntities::one(
        "LINE",
        Entity::new(line_geom(p(0.0, 0.0), p(10.0, 0.0)), l1),
    )))
    .expect("足せる");
    let id = doc.entities().ids().last().expect("ある");
    let mut s = Session::new();
    s.selection.insert(id);
    click(&mut s, &mut doc, p(10.0, 0.0));
    assert!(s.is_gripping());

    s.apply_external(Box::new(SetLayerProperties::new(l1).locked(true)), &mut doc);
    assert!(!s.is_gripping(), "中断した");
    assert!(
        errors(&s).iter().any(|e| e.contains("GRIP: 対象の図形")),
        "{:?}",
        errors(&s)
    );
    click(&mut s, &mut doc, p(20.0, 0.0));
    assert_eq!(geom(&doc, id), line_geom(p(0.0, 0.0), p(10.0, 0.0)));
}

/// 掴んだ後に図面が変わったら（掴んだ図形は編集できるままでも）確定を断り、グリップを取り消す。
#[test]
fn a_changed_drawing_refuses_to_confirm() {
    let (mut s, mut doc, id) = selected_line();
    doc.apply(Box::new(AddLayer::new("L1", AciColor::WHITE)))
        .expect("足せる");
    let l1 = doc.layers().by_name("L1").expect("ある");
    click(&mut s, &mut doc, p(10.0, 0.0));
    s.apply_external(
        Box::new(SetLayerProperties::new(l1).color(AciColor::RED)),
        &mut doc,
    );
    assert!(s.is_gripping(), "前提: 掴んだ図形は編集できるまま");
    let revision = doc.revision();
    click(&mut s, &mut doc, p(20.0, 0.0));
    assert!(!s.is_gripping(), "取り消した");
    assert_eq!(doc.revision(), revision, "図面は変えない");
    assert_eq!(geom(&doc, id), line_geom(p(0.0, 0.0), p(10.0, 0.0)));
    assert!(
        errors(&s).iter().any(|e| e.contains("図面が変わったため")),
        "{:?}",
        errors(&s)
    );
    assert!(s.selection.contains(id), "選択は残る");
}

/// 図面を入れ替えたら（NEW / OPEN）グリップを中断し、選択も消す。
#[test]
fn replacing_the_document_drops_the_grip_and_the_selection() {
    let (mut s, mut doc, _) = selected_line();
    click(&mut s, &mut doc, p(10.0, 0.0));
    s.document_replaced();
    assert!(!s.is_gripping());
    assert!(s.selection.is_empty(), "前の図面の ID を残さない");
}

/// リボンを押したらグリップを取り消し、選択を残したままそのコマンドを始める。
#[test]
fn a_ribbon_button_cancels_the_grip_and_keeps_the_selection() {
    let (mut s, mut doc, id) = selected_line();
    click(&mut s, &mut doc, p(10.0, 0.0));
    s.start_command_from_ui("MOVE", &mut doc);
    assert_eq!(s.active_command(), Some("MOVE"));
    assert!(s.selection.contains(id));
    assert!(s.wants_point(), "選択済みなので基点の指定へ");
}

/// パネルの開閉（リボン・打った名前）は掴んだまま。
#[test]
fn toggling_a_panel_keeps_the_grip() {
    let (mut s, mut doc, _) = selected_line();
    click(&mut s, &mut doc, p(10.0, 0.0));
    s.start_command_from_ui("LAYER", &mut doc);
    assert!(s.is_gripping(), "リボン");
    feed(&mut s, &mut doc, "LA");
    assert!(s.is_gripping(), "打った名前");
}

// ---- ホバーとクリックの一致 --------------------------------------------------

/// グリップの上では、ホバー（`hover::Hover`）とクリックが同じ `ClickTarget::Grip` を返し、
/// 縁取りの強調は出さない。
#[test]
fn hover_and_click_agree_on_a_grip() {
    let (mut s, mut doc, _) = selected_line();
    let mut h = crate::hover::Hover::new();
    h.update(&s, &doc, Some(p(10.2, 0.1)), PICK, false);
    let hovered = h.hovered_grip().expect("グリップに乗せている");
    assert!(h.highlighted().is_empty(), "紫の縁取りは出さない");
    click(&mut s, &mut doc, p(10.2, 0.1));
    assert_eq!(s.hot_grips(), vec![hovered], "乗せたグリップを掴む");
}

/// 選び直したら、同じ位置でもホバーの結果を作り直す（グリップは選択から作る）。
#[test]
fn reselecting_recomputes_the_hover_at_the_same_position() {
    let mut doc = Document::new();
    let a = add(&mut doc, line_geom(p(0.0, 0.0), p(10.0, 0.0)));
    let b = add(&mut doc, line_geom(p(10.0, 0.0), p(10.0, 10.0)));
    let mut s = Session::new();
    s.selection.insert(a);
    let mut h = crate::hover::Hover::new();
    let at = p(10.0, 0.0);
    h.update(&s, &doc, Some(at), PICK, false);
    assert_eq!(h.hovered_grip().map(|g| g.id), Some(a));

    s.selection.clear();
    s.selection.insert(b);
    h.update(&s, &doc, Some(at), PICK, false);
    assert_eq!(
        h.hovered_grip().map(|g| g.id),
        Some(b),
        "同じ位置でも作り直す"
    );

    h.update(&s, &doc, Some(at), PICK, true);
    assert_eq!(h.hovered_grip(), None, "Shift 中はグリップを拾わない");
}
