//! 全ツールの全段階のプロンプトから「今できること」を組み立てられること（Issue #27 段階 2）。
//!
//! 2 通りで確かめる。
//!
//! 1. **ソースの走査。** `tools/*.rs` の `fn prompt` の本体にある文字列をすべて取り出し、
//!    1 行目を `cmdline/prompt.rs` で読む。どれも「何を求めているか」が読め、括弧が残らず、
//!    「今できること」が空にならないこと。プロンプトを足して書式を崩すと落ちる
//! 2. **実際に動かす。** `Session` で各コマンドを段階ごとに進め、出てきたプロンプトから組み立てる。
//!    出てきたプロンプトがすべて 1 の走査で見つかったもの（の雛形）に当てはまること
//!    （走査の取りこぼしが無いこと）も確かめる

use super::*;
use crate::cmdline::actions::{self, Assists, DimHint, DimTab, Tracking};
use crate::cmdline::dimension::{DimKind, Field};
use crate::cmdline::prompt::{self, Ask};
use cad_core::command::AddEntities;
use cad_core::geom::Line;
use cad_core::{Entity, LayerId};

/// クリックの拾い半径（モデル空間）。
const PICK: f64 = 0.5;
/// 雛形の中の置き換え（`{name}` など）を表す文字。プロンプトに出てこない制御文字を使う。
const HOLE: char = '\u{1}';

// ---- ソースの走査 ------------------------------------------------------------

/// プロンプトを持つツールのソース。
const TOOL_SOURCES: [&str; 5] = [
    include_str!("../tools/draw.rs"),
    include_str!("../tools/edit.rs"),
    include_str!("../tools/component.rs"),
    include_str!("../tools/param.rs"),
    include_str!("../tools/grip.rs"),
];

/// `fn prompt(&self) -> String {` の本体にある文字列リテラル（エスケープを戻し、`{..}` を [`HOLE`] にしたもの）。
fn prompt_literals(src: &str) -> Vec<String> {
    const HEAD: &str = "fn prompt(&self) -> String {";
    let mut out = Vec::new();
    let mut rest = src;
    while let Some(at) = rest.find(HEAD) {
        let body = &rest[at + HEAD.len()..];
        let (literals, consumed) = literals_in_block(body);
        out.extend(literals);
        rest = &body[consumed..];
    }
    out
}

/// 開き括弧の直後から、対応する `}` までの文字列リテラルを集める。戻り値の 2 つ目は読んだバイト数。
fn literals_in_block(body: &str) -> (Vec<String>, usize) {
    let mut depth = 1;
    let mut literals = Vec::new();
    let mut chars = body.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return (literals, i + 1);
                }
            }
            '"' => {
                let mut lit = String::new();
                while let Some((_, c)) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => match chars.next().map(|(_, c)| c) {
                            Some('n') => lit.push('\n'),
                            Some('"') => lit.push('"'),
                            Some('\\') => lit.push('\\'),
                            Some('u') => {
                                // \u{3000}
                                let mut hex = String::new();
                                for (_, c) in chars.by_ref() {
                                    if c == '}' {
                                        break;
                                    }
                                    if c != '{' {
                                        hex.push(c);
                                    }
                                }
                                let code = u32::from_str_radix(&hex, 16).unwrap_or(0);
                                lit.push(char::from_u32(code).unwrap_or(HOLE));
                            }
                            // 行の継続（`\` + 改行 + 先頭の空白）。
                            Some('\n') => {
                                while chars.peek().is_some_and(|(_, c)| c.is_whitespace()) {
                                    chars.next();
                                }
                            }
                            _ => {}
                        },
                        _ => lit.push(c),
                    }
                }
                literals.push(fill_holes(&lit));
            }
            _ => {}
        }
    }
    (literals, body.len())
}

/// `format!` の置き換え `{..}` を [`HOLE`] 1 文字にする（`{{` `}}` は括弧に戻す）。
fn fill_holes(lit: &str) -> String {
    let mut out = String::new();
    let mut chars = lit.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                out.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                out.push('}');
            }
            '{' => {
                for c in chars.by_ref() {
                    if c == '}' {
                        break;
                    }
                }
                out.push(HOLE);
            }
            _ => out.push(c),
        }
    }
    out
}

/// プロンプトの雛形（1 行目）。ツールのソースと、`Session` の選択待ちのプロンプト。
fn prompt_templates() -> Vec<String> {
    let mut templates: Vec<String> = TOOL_SOURCES
        .iter()
        .flat_map(|src| prompt_literals(src))
        .filter_map(|lit| lit.lines().next().map(str::trim).map(str::to_owned))
        .filter(|first| first.ends_with(':'))
        .collect();
    templates.push(SELECT_PROMPT.to_owned());
    templates.sort();
    templates.dedup();
    templates
}

/// 雛形に当てはまるか（[`HOLE`] は空でない任意の文字列）。
fn matches_template(template: &str, actual: &str) -> bool {
    let parts: Vec<&str> = template.split(HOLE).collect();
    let mut rest = actual;
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            let Some(r) = rest.strip_prefix(part) else {
                return false;
            };
            rest = r;
        } else if i == parts.len() - 1 {
            return rest.len() > part.len() && rest.ends_with(part);
        } else {
            // 置き換えの後ろは空でない文字列のあとに `part` が続く。
            let Some(at) = rest.get(1..).and_then(|r| r.find(part)) else {
                return false;
            };
            rest = &rest[1 + at + part.len()..];
        }
    }
    rest.is_empty()
}

/// 点の段階らしい、どこにでもある作図補助。
const ASSISTS: Assists = Assists {
    osnap: Some(true),
    tracking: Some(Tracking {
        ortho: false,
        polar: false,
        shift: false,
    }),
};

/// 組み立てた「今できること」が意味を持つこと: 空でなく、最後は `Esc`（または Esc を含む取り消し）。
fn assert_buildable(prompt_text: &str, built: &actions::Actions) {
    assert!(!built.items.is_empty(), "空: {prompt_text:?}");
    let last = built
        .items
        .last()
        .map(|a| a.key.as_str())
        .unwrap_or_default();
    assert!(
        last.contains("Esc"),
        "最後が Esc でない: {prompt_text:?} {built:?}"
    );
}

/// ソースにある全プロンプトの 1 行目が読め、「今できること」を組み立てられる。
#[test]
fn every_prompt_in_the_source_can_be_read() {
    let templates = prompt_templates();
    assert!(
        templates.len() >= 50,
        "前提: 走査でプロンプトが見つかっている（{} 件）: {templates:#?}",
        templates.len()
    );
    for t in &templates {
        let sample = t.replace(HOLE, "幅");
        let p = prompt::parse(&sample);
        assert_ne!(p.ask, Ask::Unknown, "何を求めているか読めない: {t:?}");
        assert_ne!(p.ask, Ask::Command, "{t:?}");
        for open in ['[', '<', '(', '（'] {
            assert!(
                !p.head.contains(open),
                "括弧が本文に残った: {t:?} → {:?}",
                p.head
            );
        }
        assert!(
            p.options.iter().all(|o| !o.key.is_empty()),
            "キーの読めないオプション: {t:?} {:?}",
            p.options
        );
        assert_buildable(
            &sample,
            &actions::build(&sample, actions::Stage::default(), None, false, ASSISTS),
        );
    }
}

#[test]
fn template_matching() {
    let t = format!("目的点を指定 <Enter で終了> (複写 {HOLE} 個):");
    assert!(matches_template(
        &t,
        "目的点を指定 <Enter で終了> (複写 12 個):"
    ));
    assert!(!matches_template(
        &t,
        "目的点を指定 <Enter で終了> (複写  個)X"
    ));
    assert!(matches_template(
        &format!("「{HOLE}」の値を入力:"),
        "「幅」の値を入力:"
    ));
    assert!(!matches_template("基点を指定:", "目的点を指定:"));
    assert!(matches_template("基点を指定:", "基点を指定:"));
}

// ---- 実際に動かす --------------------------------------------------------------

/// 段階ごとのプロンプトを集めながら、コマンドを進める。
struct Walk {
    s: Session,
    doc: Document,
    seen: Vec<String>,
}

impl Walk {
    fn new() -> Self {
        let mut doc = Document::new();
        let line = |a: (f64, f64), b: (f64, f64)| {
            Entity::new(
                Geometry::Line(Line::new(Point2::new(a.0, a.1), Point2::new(b.0, b.1))),
                LayerId::ZERO,
            )
        };
        // 横線と、それに交わる縦線（TRIM / FILLET が効く形）。離れたところに 1 本。
        for e in [
            line((0.0, 0.0), (10.0, 0.0)),
            line((5.0, -5.0), (5.0, 5.0)),
            line((30.0, 0.0), (40.0, 0.0)),
        ] {
            doc.apply(Box::new(AddEntities::one("LINE", e)))
                .expect("足せる");
        }
        Self {
            s: Session::new(),
            doc,
            seen: Vec::new(),
        }
    }

    /// 今のプロンプトから組み立てて確かめ、覚える。
    fn look(&mut self) {
        let Some(stage) = self.s.hint_stage() else {
            return;
        };
        let text = self.s.prompt();
        let dim = self.s.dimension_base().map(|_| DimHint {
            kind: DimKind::LengthAngle,
            field: Some(Field::Length),
            tab: Some(DimTab::Move(Field::Angle)),
            enter_confirms: false,
            esc: None,
        });
        let assists = Assists {
            osnap: Some(true),
            tracking: self.s.tracking_base().map(|_| Tracking::default()),
        };
        assert_buildable(&text, &actions::build(&text, stage, dim, false, assists));
        let first = text.lines().next().unwrap_or_default().trim().to_owned();
        if !self.seen.contains(&first) {
            self.seen.push(first);
        }
    }

    fn text(&mut self, t: &str) -> &mut Self {
        self.s
            .handle_submission(Submission::Text(t.to_owned()), &mut self.doc);
        self.look();
        self
    }

    fn enter(&mut self) -> &mut Self {
        self.s.handle_submission(Submission::Empty, &mut self.doc);
        self.look();
        self
    }

    fn esc(&mut self) -> &mut Self {
        self.s.handle_submission(Submission::Cancel, &mut self.doc);
        self.s.handle_submission(Submission::Cancel, &mut self.doc);
        self
    }

    fn click(&mut self, x: f64, y: f64) -> &mut Self {
        self.s.handle_click(
            Point2::new(x, y),
            false,
            PICK,
            &mut self.doc,
            &mut selection::ScanAll,
        );
        self.look();
        self
    }
}

/// 各コマンドを段階ごとに進めて、出てきたプロンプトから組み立てる。
/// 出てきたプロンプトは、どれもソースの走査で見つかった雛形に当てはまる。
#[test]
fn every_stage_reached_by_running_the_tools_can_be_built() {
    let mut w = Walk::new();
    // グリップ（何も実行していないときに選んで、端点を掴む）
    w.s.handle_click(
        Point2::new(37.0, 0.0),
        false,
        PICK,
        &mut w.doc,
        &mut selection::ScanAll,
    );
    w.click(40.0, 0.0);
    assert!(w.s.is_gripping(), "前提: グリップを掴んだ");
    w.esc().esc();
    // 作図
    w.text("L")
        .click(0.0, 20.0)
        .click(10.0, 20.0)
        .click(10.0, 30.0)
        .enter();
    w.text("C").click(0.0, 50.0).text("D").text("5");
    w.text("C").text("2P").click(0.0, 60.0).click(4.0, 60.0);
    w.text("A")
        .click(0.0, 70.0)
        .click(3.0, 73.0)
        .click(6.0, 70.0);
    w.text("REC").click(0.0, 80.0).click(4.0, 84.0);
    w.text("PL")
        .click(0.0, 90.0)
        .click(5.0, 90.0)
        .click(5.0, 95.0)
        .enter();
    w.text("XL").click(0.0, 100.0).click(1.0, 101.0).esc();
    w.text("XL").text("H").esc();
    w.text("XL").text("V").esc();
    w.text("XL").text("A").text("30").esc();
    w.text("XL")
        .text("O")
        .text("2")
        .click(2.0, 0.0)
        .click(2.0, 3.0)
        .esc();
    // 図形を指す（線の位置が変わる前に）
    w.esc().text("TR").enter();
    w.esc().text("EX").enter();
    w.esc().text("F").text("R").text("2").click(2.0, 0.0).esc();
    w.esc()
        .text("CHA")
        .text("D")
        .text("1")
        .text("1")
        .click(2.0, 0.0)
        .esc();
    // 修正（選択 → Enter → …）。選択は `ALL`（選択待ちで打つと全選択）。各コマンドの前に選択を外す
    w.esc()
        .text("M")
        .text("ALL")
        .enter()
        .click(30.0, 0.0)
        .click(30.0, 1.0);
    w.esc()
        .text("CO")
        .text("ALL")
        .enter()
        .click(30.0, 1.0)
        .click(30.0, 2.0)
        .enter();
    w.esc()
        .text("S")
        .text("ALL")
        .enter()
        .click(30.0, 1.0)
        .click(30.0, 2.0);
    w.esc()
        .text("RO")
        .text("ALL")
        .enter()
        .click(30.0, 1.0)
        .text("C")
        .text("10");
    w.esc()
        .text("SC")
        .text("ALL")
        .enter()
        .click(30.0, 1.0)
        .text("C")
        .text("2");
    w.esc()
        .text("MI")
        .text("ALL")
        .enter()
        .click(30.0, 1.0)
        .click(30.0, 5.0)
        .enter();
    w.esc().text("E").esc();
    w.esc().text("G").text("ALL").enter().enter();
    w.esc().text("UNG").esc();
    w.esc().text("X").esc();
    w.esc().text("Z").text("E");
    // コンポーネント
    w.esc()
        .text("B")
        .text("ALL")
        .enter()
        .click(0.0, 0.0)
        .text("BOX");
    let def_name = "BOX";
    assert!(
        w.doc.definitions().by_name(def_name).is_some(),
        "前提: 定義ができている"
    );
    w.esc()
        .text("I")
        .text(def_name)
        .click(500.0, 0.0)
        .enter()
        .enter();
    w.esc().text("RD").text("ALL").enter().click(0.0, 0.0).esc();
    w.esc().text("BE").esc();
    w.esc().text("PA").text(def_name).text("幅").text("900");
    w.esc()
        .text("BI")
        .text(def_name)
        .text("0")
        .text("終点X")
        .esc();
    w.esc().text("PS").click(505.0, 2.0).text("幅").esc();

    let templates = prompt_templates();
    for seen in &w.seen {
        assert!(
            templates.iter().any(|t| matches_template(t, seen)),
            "走査で見つからないプロンプト（走査の取りこぼし）: {seen:?}"
        );
    }
    // 動かしても届かない段階（雛形）。届かない理由がはっきりしているものだけをここに置き、
    // それ以外はすべて動かして確かめる。ここに無い段階に届かなくなったら落ちる。
    let unreached: Vec<String> = templates
        .iter()
        .filter(|t| !w.seen.iter().any(|s| matches_template(t, s)))
        .map(|t| t.replace(HOLE, "#"))
        .collect();
    assert_eq!(
        unreached,
        vec![
            // BIND の軸の選択は、インプレース編集中に点をクリックしたときだけ出る。
            "「#」のどちらを動かすか [X / Y]:",
            // ツールが自分で持つ選択のプロンプト。選択待ちの間は `Session` の
            // `オブジェクトを選択 (Enter で確定):` が出るので、画面には出ない（走査では読めることを確かめている）。
            "オブジェクトを選択 (Enter で削除):",
            "分解するオブジェクトを選択 (Enter で確定):",
            "新しい中身を選択 (Enter で確定):",
            "解除するグループの要素を選択 (Enter で確定):",
        ],
        "動かして届いた段階: {:#?}",
        w.seen
    );
}
