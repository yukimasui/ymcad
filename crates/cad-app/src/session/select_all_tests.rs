//! 全選択（Ctrl+A / SELECTALL、Issue #34 段階 3、ADR-0044）。
//!
//! `Session::select_all` の対象（何が入り、何が入らないか）と、効く段階・効かない段階を固定する。
//! キー（Ctrl+A）の扱いは `app/behavior_tests/select_all.rs`。

use super::*;
use cad_core::command::{AddEntities, AddLayer, CreateGroup, SetLayerProperties};
use cad_core::geom::Line;
use cad_core::{AciColor, Entity, LayerId};

/// クリックの拾い半径（モデル空間）。
const PICK: f64 = 0.5;

fn feed(s: &mut Session, doc: &mut Document, text: &str) {
    s.handle_submission(Submission::Text(text.to_owned()), doc);
}

fn enter(s: &mut Session, doc: &mut Document) {
    s.handle_submission(Submission::Empty, doc);
}

fn click(s: &mut Session, doc: &mut Document, x: f64, y: f64) {
    s.handle_click(Point2::new(x, y), false, PICK, doc, &mut selection::ScanAll);
}

fn apply(doc: &mut Document, cmd: Box<dyn cad_core::Command>) {
    doc.apply(cmd).expect("適用できる");
}

/// 横の線分（`y` の高さ、x = 0..10）を `layer` に足して ID を返す。
fn add_line(doc: &mut Document, layer: LayerId, y: f64) -> EntityId {
    let line = Geometry::Line(Line::new(Point2::new(0.0, y), Point2::new(10.0, y)));
    apply(
        doc,
        Box::new(AddEntities::one("LINE", Entity::new(line, layer))),
    );
    doc.entities().ids().last().expect("足した図形")
}

/// レイヤ `LOCKED`（ロック）と `HIDDEN`（非表示）を足して返す。
fn locked_and_hidden(doc: &mut Document) -> (LayerId, LayerId) {
    apply(doc, Box::new(AddLayer::new("LOCKED", AciColor::WHITE)));
    apply(doc, Box::new(AddLayer::new("HIDDEN", AciColor::WHITE)));
    let locked = doc.layers().by_name("LOCKED").expect("LOCKED");
    let hidden = doc.layers().by_name("HIDDEN").expect("HIDDEN");
    apply(doc, Box::new(SetLayerProperties::new(locked).locked(true)));
    apply(
        doc,
        Box::new(SetLayerProperties::new(hidden).visible(false)),
    );
    (locked, hidden)
}

fn selected(s: &Session) -> Vec<EntityId> {
    s.selection.to_vec()
}

fn infos(s: &Session) -> Vec<String> {
    s.cmdline
        .history()
        .filter(|l| l.kind == LineKind::Info)
        .map(|l| l.text.clone())
        .collect()
}

/// 表示中でロックされていないレイヤの図形だけが入り、選んだ数と除いた数を案内する。
/// 図面も Undo の履歴も変えない（選択は UI の状態）。
#[test]
fn selects_only_entities_on_editable_layers() {
    let mut doc = Document::new();
    let mut s = Session::new();
    let (locked, hidden) = locked_and_hidden(&mut doc);
    let a = add_line(&mut doc, LayerId::ZERO, 0.0);
    let b = add_line(&mut doc, LayerId::ZERO, 5.0);
    add_line(&mut doc, locked, 10.0);
    add_line(&mut doc, hidden, 15.0);
    let history = doc.history().len();
    let revision = doc.revision();

    assert!(s.select_all(&doc), "待機中は効く");
    assert_eq!(selected(&s), vec![a, b]);
    assert!(
        infos(&s)
            .last()
            .is_some_and(|t| t.contains("2 個") && t.contains("2 個は除く")),
        "選んだ数と除いた数を案内する: {:?}",
        infos(&s)
    );
    assert_eq!(doc.history().len(), history, "Undo の履歴に積まない");
    assert_eq!(doc.revision(), revision, "図面を変えない");
}

/// 既に選んでいたものは残したまま、残りを足す（全部選ばれた状態になる）。
#[test]
fn adds_to_the_existing_selection() {
    let mut doc = Document::new();
    let mut s = Session::new();
    let a = add_line(&mut doc, LayerId::ZERO, 0.0);
    let b = add_line(&mut doc, LayerId::ZERO, 5.0);
    s.selection.insert(b);
    assert!(s.select_all(&doc));
    assert_eq!(selected(&s), vec![a, b]);
}

/// 選べる図形が無ければ、そう案内する（選択は空のまま）。
#[test]
fn reports_when_nothing_can_be_selected() {
    let mut doc = Document::new();
    let mut s = Session::new();
    let (locked, _) = locked_and_hidden(&mut doc);
    add_line(&mut doc, locked, 0.0);
    assert!(s.select_all(&doc));
    assert!(s.selection.is_empty());
    assert!(
        infos(&s).last().is_some_and(
            |t| t.contains("選べるオブジェクトがありません") && t.contains("1 個は除く")
        ),
        "{:?}",
        infos(&s)
    );
}

/// グループのうち、ロックされたレイヤの一員は入らない（Issue #51 案 A。クリックと同じ）。
#[test]
fn locked_group_members_stay_out() {
    let mut doc = Document::new();
    let mut s = Session::new();
    let (locked, _) = locked_and_hidden(&mut doc);
    let free = add_line(&mut doc, LayerId::ZERO, 0.0);
    let held = add_line(&mut doc, locked, 5.0);
    apply(
        &mut doc,
        Box::new(CreateGroup::new("GROUP", "g", vec![free, held])),
    );

    assert!(s.select_all(&doc));
    assert_eq!(selected(&s), vec![free], "ロックされた一員は入らない");

    // クリックでグループを選んだときと同じ結果になる。
    let mut clicked = Session::new();
    click(&mut clicked, &mut doc, 5.0, 0.0);
    assert_eq!(selected(&clicked), selected(&s));
}

/// 選択待ち（ERASE の「オブジェクトを選択」）でも効き、選択待ちのまま。Enter で全部が消える。
/// ロックされたレイヤの図形は残る。
#[test]
fn works_while_waiting_for_a_selection() {
    let mut doc = Document::new();
    let mut s = Session::new();
    let (locked, _) = locked_and_hidden(&mut doc);
    add_line(&mut doc, LayerId::ZERO, 0.0);
    add_line(&mut doc, LayerId::ZERO, 5.0);
    let kept = add_line(&mut doc, locked, 10.0);
    feed(&mut s, &mut doc, "ERASE");
    assert_eq!(s.prompt(), SELECT_PROMPT, "前提: 選択待ち");

    assert!(s.select_all(&doc), "選択待ちでも効く");
    assert_eq!(s.selection.len(), 2);
    assert_eq!(s.prompt(), SELECT_PROMPT, "選択待ちのまま");

    enter(&mut s, &mut doc);
    assert_eq!(
        doc.entities().ids().collect::<Vec<_>>(),
        vec![kept],
        "選べた 2 本が消え、ロックされた 1 本が残る"
    );
}

/// 点や値の入力中・図形を指す段階では効かない（選択は変わらない）。効かなかったことが分かるよう、
/// 使える段階を灰色の案内で 1 行だけ知らせる（Issue #74 の 1）。押し続けても履歴は増えない。
#[test]
fn does_nothing_while_a_point_or_an_entity_is_wanted() {
    for name in ["LINE", "TRIM", "ZOOM"] {
        let mut doc = Document::new();
        let mut s = Session::new();
        add_line(&mut doc, LayerId::ZERO, 0.0);
        feed(&mut s, &mut doc, name);
        assert!(s.wants_point(), "前提: {name} は点か値を待つ");
        let lines = s.cmdline.history().count();

        assert!(!s.can_select_all(), "{name}");
        assert!(!s.select_all(&doc), "{name}: 効かない");
        assert!(s.selection.is_empty(), "{name}: 選ばれない");
        assert_eq!(s.active_command(), Some(name), "{name}: 続いている");
        assert_eq!(
            s.cmdline.history().count(),
            lines + 1,
            "{name}: 案内を 1 行"
        );
        assert_eq!(
            infos(&s).last().map(String::as_str),
            Some(SELECT_ALL_UNAVAILABLE),
            "{name}: 使える段階を案内する"
        );
        assert!(
            !s.cmdline.history().any(|l| l.kind == LineKind::Error),
            "{name}: エラー（赤）にはしない"
        );

        // 続けて押しても（キーの連打・押しっぱなし）、同じ案内は積まない。
        for _ in 0..5 {
            assert!(!s.select_all(&doc));
        }
        assert_eq!(
            s.cmdline.history().count(),
            lines + 1,
            "{name}: 連打で履歴が増えない"
        );
    }
}

/// 間に別の行が入った後なら、もう一度案内する（直前の行と同じときだけ省く）。
#[test]
fn the_unavailable_notice_comes_back_after_another_line() {
    let mut doc = Document::new();
    let mut s = Session::new();
    feed(&mut s, &mut doc, "LINE");
    assert!(!s.select_all(&doc));
    feed(&mut s, &mut doc, "0,0");
    s.cmdline.info("別の行");
    assert!(!s.select_all(&doc));
    let notices = s
        .cmdline
        .history()
        .filter(|l| l.text == SELECT_ALL_UNAVAILABLE)
        .count();
    assert_eq!(notices, 2);
}

/// 選択待ち（「オブジェクトを選択」）の `ALL` は `SELECTALL` と同じ（AutoCAD の習慣。Issue #74 の 2）。
/// 大文字・小文字は問わない。選択待ちのまま選び、Enter で全部が消える。
#[test]
fn typing_all_while_waiting_for_a_selection_selects_everything() {
    for word in ["ALL", "all", " All "] {
        let mut doc = Document::new();
        let mut s = Session::new();
        let (locked, _) = locked_and_hidden(&mut doc);
        add_line(&mut doc, LayerId::ZERO, 0.0);
        add_line(&mut doc, LayerId::ZERO, 5.0);
        let kept = add_line(&mut doc, locked, 10.0);
        feed(&mut s, &mut doc, "ERASE");
        assert_eq!(s.prompt(), SELECT_PROMPT, "前提: 選択待ち");

        feed(&mut s, &mut doc, word);
        assert_eq!(s.selection.len(), 2, "{word:?}: 選べる 2 本を選ぶ");
        assert_eq!(s.prompt(), SELECT_PROMPT, "{word:?}: 選択待ちのまま");
        assert!(
            !s.cmdline.history().any(|l| l.kind == LineKind::Error),
            "{word:?}: エラーにしない"
        );
        assert_eq!(
            s.cmdline.last_command(),
            Some("ERASE"),
            "再実行は ERASE のまま"
        );
        enter(&mut s, &mut doc);
        assert_eq!(
            doc.entities().ids().collect::<Vec<_>>(),
            vec![kept],
            "{word:?}: Enter で選べた 2 本が消える"
        );
    }
}

/// 選択待ち以外の `ALL` は今までどおり。待機中は不明なコマンド、点の入力中は座標として読めない、
/// ZOOM の中では ZOOM のオプション（全体表示）。どれも図形を選ばない。
#[test]
fn all_outside_a_selection_wait_keeps_its_meaning() {
    let mut doc = Document::new();
    let mut s = Session::new();
    add_line(&mut doc, LayerId::ZERO, 0.0);

    feed(&mut s, &mut doc, "ALL");
    assert!(s.selection.is_empty(), "待機中: 選ばない");
    assert!(!s.has_active_tool());
    assert!(
        s.cmdline
            .history()
            .any(|l| l.kind == LineKind::Error && l.text.contains("不明なコマンドです: ALL")),
        "待機中: 不明なコマンド"
    );

    feed(&mut s, &mut doc, "LINE");
    feed(&mut s, &mut doc, "ALL");
    assert!(s.selection.is_empty(), "LINE: 選ばない");
    assert_eq!(s.active_command(), Some("LINE"), "LINE は続く");
    assert!(s.last_point().is_none(), "LINE: 点にもならない");
    s.cancel();

    feed(&mut s, &mut doc, "ZOOM");
    feed(&mut s, &mut doc, "ALL");
    assert!(s.selection.is_empty(), "ZOOM: 選ばない");
    assert!(!s.has_active_tool(), "ZOOM: 全体表示で終わる");
    assert!(
        s.take_view_actions().contains(&ViewAction::ZoomAll),
        "ZOOM: 全体表示"
    );
}

/// インプレース編集中は、薄く表示されている編集の外の図形も入る（クリックで拾えるので。
/// ユーザー判断 6）。全選択の対象は、待機中のクリックで拾える図形と一致する。
#[test]
fn includes_entities_dimmed_by_in_place_editing() {
    let mut doc = Document::new();
    let mut s = Session::new();
    // 線分 2 本のコンポーネントと、その外の線分 1 本。
    let a = add_line(&mut doc, LayerId::ZERO, 0.0);
    let b = add_line(&mut doc, LayerId::ZERO, 5.0);
    s.selection.insert(a);
    s.selection.insert(b);
    feed(&mut s, &mut doc, "B");
    feed(&mut s, &mut doc, "0,0");
    feed(&mut s, &mut doc, "窓");
    s.selection.clear();
    let outside = add_line(&mut doc, LayerId::ZERO, 20.0);

    feed(&mut s, &mut doc, "BE");
    click(&mut s, &mut doc, 5.0, 0.0);
    let editing = s.editing().expect("前提: 編集中").clone();
    assert!(
        !editing.contains(outside),
        "前提: 外の線分は編集の外（薄く表示）"
    );
    let ids: Vec<EntityId> = doc.entities().ids().collect();
    assert_eq!(ids.len(), 3, "前提: 中身 2 本と外の 1 本");

    assert!(s.select_all(&doc));
    assert_eq!(selected(&s), ids, "薄く表示されている外の線分も入る");
    assert_eq!(
        infos(&s).last().map(String::as_str),
        Some("全選択: 3 個のオブジェクトを選択（編集の外の 1 個を含む）"),
        "続けて ERASE すると外も消えることが読めるよう、外の数を添える（Issue #74 の 3）"
    );

    // 待機中のクリックで拾える図形と同じ（各線分の中点をクリックして集める）。
    let mut clicked = Session::new();
    for y in [0.0, 5.0, 20.0] {
        click(&mut clicked, &mut doc, 5.0, y);
    }
    assert_eq!(selected(&clicked), selected(&s));
}

/// `SELECTALL` と打っても同じ。待機中は直前のコマンドとして覚え、選択待ちでは選択待ちのまま選ぶ。
#[test]
fn typing_selectall_goes_through_the_same_path() {
    let mut doc = Document::new();
    let mut s = Session::new();
    let a = add_line(&mut doc, LayerId::ZERO, 0.0);
    let b = add_line(&mut doc, LayerId::ZERO, 5.0);

    feed(&mut s, &mut doc, "selectall");
    assert_eq!(selected(&s), vec![a, b]);
    assert_eq!(s.cmdline.last_command(), Some("SELECTALL"));
    assert!(!s.has_active_tool());

    // 選択待ち。ほかの文字は断られるが、SELECTALL は受け付ける。
    s.cancel();
    feed(&mut s, &mut doc, "ERASE");
    assert_eq!(s.prompt(), SELECT_PROMPT, "前提: 選択待ち");
    feed(&mut s, &mut doc, "SELECTALL");
    assert_eq!(s.selection.len(), 2, "選択待ちでも選ぶ");
    assert_eq!(s.prompt(), SELECT_PROMPT, "選択待ちのまま");
    assert_eq!(
        s.cmdline.last_command(),
        Some("ERASE"),
        "再実行の対象は ERASE のまま"
    );
    enter(&mut s, &mut doc);
    assert_eq!(doc.entities().len(), 0, "Enter で全部消える");
}

/// リボンのボタン（`start_command_from_ui`）: 選択待ちでは中断せずに選び足し、点の入力中は
/// ほかのボタンと同じく中断してから全選択する。
#[test]
fn the_ribbon_button_keeps_a_selection_wait_but_interrupts_a_point_input() {
    let mut doc = Document::new();
    let mut s = Session::new();
    add_line(&mut doc, LayerId::ZERO, 0.0);
    add_line(&mut doc, LayerId::ZERO, 5.0);

    feed(&mut s, &mut doc, "ERASE");
    s.start_command_from_ui("SELECTALL", &mut doc);
    assert_eq!(s.active_command(), Some("ERASE"), "ERASE は続く");
    assert_eq!(s.prompt(), SELECT_PROMPT, "選択待ちのまま");
    assert_eq!(s.selection.len(), 2);
    assert!(
        !s.cmdline.history().any(|l| l.text == "*取り消し*"),
        "中断していない"
    );
    enter(&mut s, &mut doc);
    assert_eq!(doc.entities().len(), 0, "Enter で全部消える");

    add_line(&mut doc, LayerId::ZERO, 0.0);
    feed(&mut s, &mut doc, "LINE");
    feed(&mut s, &mut doc, "0,0");
    s.start_command_from_ui("SELECTALL", &mut doc);
    assert!(!s.has_active_tool(), "LINE は中断される");
    assert_eq!(s.selection.len(), 1, "中断してから全選択する");
}

/// 案内の添え書きは 1 組の括弧にまとめる。編集の外の数は編集中（0 でない）ときだけ。
#[test]
fn the_notice_puts_the_notes_in_one_pair_of_brackets() {
    assert_eq!(
        select_all_notice(3, 0, 0),
        "全選択: 3 個のオブジェクトを選択"
    );
    assert_eq!(
        select_all_notice(3, 1, 0),
        "全選択: 3 個のオブジェクトを選択（編集の外の 1 個を含む）"
    );
    assert_eq!(
        select_all_notice(3, 1, 2),
        "全選択: 3 個のオブジェクトを選択（編集の外の 1 個を含む。非表示・ロック中のレイヤの 2 個は除く）"
    );
    assert_eq!(
        select_all_notice(0, 0, 2),
        "全選択: 選べるオブジェクトがありません（非表示・ロック中のレイヤの 2 個は除く）"
    );
}
