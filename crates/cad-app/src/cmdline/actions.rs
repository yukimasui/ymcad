//! 「今できること」（Issue #27 段階 2）を組み立てる。
//!
//! コマンドの実行中、今の段階で使える操作を短く並べる。**各ツールは書き換えない。**
//! 今ある情報だけから組み立てる:
//!
//! - プロンプト（`cmdline/prompt.rs` で読む）… 何を求めているか・オプション・`Enter` の意味
//! - `Session` の状態（[`Stage`]）… 選択待ちか・図形を指す段階か・グリップを掴んでいるか・基準点があるか
//! - 寸法入力の欄（[`DimHint`]）… 入力中の欄・固定の有無で `Tab` / `Esc` の意味が変わる
//! - 作図補助（[`Assists`]）… F3 / F8 / F10 が効く段階だけ、今の状態を 2 行目に薄く出す
//!
//! **egui に依存しない純粋関数だけを置く。** 設計判断は ADR-0048（段階 2 の追記）。

use super::dimension::{DimKind, Field};
use super::prompt::{self, Ask};

/// 操作 1 つ。キーを先に書いて目で拾いやすくする（`C 閉じる`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Action {
    /// 押すもの・すること（`クリック`・`C`・`Enter`）。空なら添え書きだけ。
    pub key: String,
    /// 何が起きるか（`閉じる`）。空ならキーだけ（`[X / Y]` の `X`）。
    pub text: String,
}

impl Action {
    fn new(key: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            text: text.into(),
        }
    }
}

/// `Session` から見た、いまの段階。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stage {
    /// 図形の選択を待っている（`オブジェクトを選択`）。
    pub selecting: bool,
    /// グリップを掴んでいる（Issue #30）。
    pub gripping: bool,
    /// クリックを図形の指定として受け取る段階（TRIM・FILLET など）。
    pub wants_entity: bool,
    /// 相対座標・値をクリックで指すときの基準点がある（`Tool::last_point`）。
    pub has_base: bool,
}

/// 寸法入力（Issue #20 段階 B）に参加している段階での、`Tab` / `Enter` / `Esc` の効き目。
///
/// **ここで判定し直さない。** コマンドラインが実際のキー処理（`DimState::tab_in` / `enter_values` /
/// `escape`）を状態の写しで動かした結果を詰めて渡す（`CommandLine::dim_hint`）。判定を二重に書くと、
/// `-` だけ（打ちかけ）・座標を打ちかけ・オプションの文字を打った、のような場合にずれる（PR #104 のレビュー）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DimHint {
    /// 欄の見せ方（「長さ」「角度」か「半径」だけか）。
    pub kind: DimKind,
    /// 欄が出ていれば、入力中の欄（「数字 長さ」）。座標などを打って欄が消えていれば `None`。
    pub field: Option<Field>,
    /// `Tab` を押したら起きること。何も起きない（座標を打ちかけ・打ちかけの数値で断られる）なら `None`。
    pub tab: Option<DimTab>,
    /// `Enter` が欄の値で確定するか。
    pub enter_confirms: bool,
    /// `Esc` が固定・入力を外すか（外すなら中断はしない）。外さなければ `None`（中断）。
    pub esc: Option<EscClears>,
}

/// 寸法入力の `Tab` の効き目。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DimTab {
    /// 打った値で今の欄を固定して、次の欄へ移る。
    LockAndMove { locked: Field, to: Field },
    /// 打った値で今の欄を固定する（欄は移らない。半径の 1 欄）。
    Lock(Field),
    /// 固定せずに次の欄へ移る。
    Move(Field),
}

/// 寸法入力の `Esc` が外すもの。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EscClears {
    /// 固定（錠前）がある。
    pub locks: bool,
    /// 入力欄に文字がある。
    pub input: bool,
}

/// 作図補助の、いま実際に効いている状態。効かない段階では `None`。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Assists {
    /// オブジェクトスナップ（F3）が効く段階なら、オンか。
    pub osnap: Option<bool>,
    /// 直交（F8）・極（F10）が効く段階（基準点のある点の指定）なら、その状態。
    pub tracking: Option<Tracking>,
}

/// 直交・極の状態。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tracking {
    /// 直交が**実際に効いているか**（Shift の一時反転を反映した後。Issue #36、ADR-0049）。
    pub ortho: bool,
    /// 極の設定がオンか（直交が効いていれば、直交が優先されて効かない）。
    pub polar: bool,
    /// Shift で直交を一時反転している。
    pub shift: bool,
}

/// 組み立てた「今できること」。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Actions {
    /// 操作。入力のしかた → オプション → 添え書き → `Enter` → `Esc` の順。
    pub items: Vec<Action>,
    /// 2 行目に薄く出す作図補助の状態（効かない段階では空）。
    pub status: Vec<Action>,
}

/// プロンプトと段階から「今できること」を組み立てる。
///
/// `typed` … 入力欄に、確定すれば送られる文字がある（空・数値の打ちかけ `-` だけ、は含まない）。
/// このとき `Enter` は打った内容で確定する（プロンプトの `<Enter で終了>` などは空の Enter の意味）。
#[must_use]
pub fn build(
    prompt_text: &str,
    stage: Stage,
    dim: Option<DimHint>,
    typed: bool,
    assists: Assists,
) -> Actions {
    let p = prompt::parse(prompt_text);
    let mut items = Vec::new();
    let mut enter: Option<Action> = None;
    let mut esc = Some(Action::new("Esc", "中断"));
    // 点やクリックで決める段階か（作図補助の状態を出すか）。
    let mut pointing = false;

    if stage.selecting || p.ask == Ask::Select {
        items.extend([
            Action::new("クリック", "選ぶ"),
            Action::new("左→右ドラッグ", "窓選択"),
            Action::new("右→左ドラッグ", "交差選択"),
            Action::new("Shift+クリック", "外す"),
            Action::new("Ctrl+A", "全選択"),
        ]);
    } else if stage.wants_entity || p.ask == Ask::Click {
        items.push(Action::new("クリック", "図形を指す"));
    } else if stage.gripping || p.ask == Ask::Point {
        pointing = true;
        let what = if stage.gripping { "移動先" } else { "点" };
        items.push(Action::new("クリック", what));
        if let Some(d) = dim {
            items.extend(dimension_items(d));
        }
        items.push(Action::new("x,y", "座標"));
        items.push(Action::new("@x,y", "相対"));
    } else {
        match &p.ask {
            Ask::Value(name) => {
                items.push(Action::new("数字", name.clone()));
                if stage.has_base {
                    pointing = true;
                    items.push(Action::new("クリック", "指して決める"));
                }
            }
            Ask::Text(name) => {
                items.push(Action::new("", text_to_type(name)));
                enter = Some(Action::new("Enter", "確定"));
            }
            // 選ぶだけ（オプションを下で並べる）・読めないもの。Point / Select / Click は上で扱い済み。
            _ => {}
        }
    }

    // オプション（キーを先に）。
    items.extend(
        p.options
            .iter()
            .filter(|o| !o.key.is_empty())
            .map(|o| Action::new(o.key.clone(), o.label.clone())),
    );
    // `<Enter で終了>`・`<R でリセット>`・`(空の Enter・Esc で取り消し)`。
    for k in &p.keys {
        if k.key.contains("Enter") || k.key.contains("Esc") {
            if k.key.contains("Esc") {
                esc = None;
            }
            enter = Some(Action::new(k.key.clone(), k.meaning.clone()));
        } else {
            items.push(Action::new(k.key.clone(), k.meaning.clone()));
        }
    }
    if let Some(d) = &p.default {
        enter = Some(Action::new("Enter", format!("{d}（既定）")));
    }
    items.extend(p.notes.iter().map(|n| Action::new("", n.clone())));

    // 文字を打っていれば、Enter は打った内容で確定する（空の Enter の意味ではない）。
    // 空の Enter と Esc がまとめて書かれていた（グリップ）なら、Esc の意味は残す。
    if typed {
        if let Some(e) = enter.take().filter(|e| e.key.contains("Esc")) {
            esc = Some(Action::new("Esc", e.text));
        }
        enter = Some(Action::new("Enter", "入力を確定"));
    }
    // 寸法入力: 実際のキー処理の結果どおりに、Enter・Esc の意味を変える。
    if let Some(d) = dim {
        if d.enter_confirms {
            enter = Some(Action::new("Enter", "この値で確定"));
        }
        if let Some(e) = d.esc {
            let what = match (e.locks, e.input) {
                (true, true) => "固定と入力を外す",
                (true, false) => "固定を外す",
                _ => "入力を消す",
            };
            esc = Some(Action::new("Esc", what));
        }
    }
    items.extend(enter);
    items.extend(esc);

    let status = if pointing {
        status_items(assists)
    } else {
        Vec::new()
    };
    Actions { items, status }
}

/// 寸法入力の欄の操作（「数字 長さ」と `Tab`）。
fn dimension_items(d: DimHint) -> Vec<Action> {
    let name = |f| d.kind.name(f);
    let mut items = Vec::new();
    if let Some(field) = d.field {
        items.push(Action::new("数字", name(field)));
    }
    let tab = d.tab.map(|t| match t {
        DimTab::LockAndMove { locked, to } => format!("{}を固定して{}へ", name(locked), name(to)),
        DimTab::Lock(f) => format!("{}を固定", name(f)),
        DimTab::Move(to) => format!("{}の欄へ", name(to)),
    });
    items.extend(tab.map(|t| Action::new("Tab", t)));
    items
}

/// 文字を打つ段階の案内。値（`「幅」の値`）は数値・式で打てることを添える。
fn text_to_type(name: &str) -> String {
    if name.ends_with("の既定値") {
        format!("{name}を打つ（数値・式・真偽・選択肢）")
    } else if name.ends_with("の値") {
        format!("{name}を打つ（数値・式）")
    } else {
        format!("{name}を打つ")
    }
}

/// 作図補助の状態（F3 / F8 / F10）。効く段階のものだけ。
fn status_items(a: Assists) -> Vec<Action> {
    let on_off = |on: bool| if on { "オン" } else { "オフ" };
    let mut status = Vec::new();
    if let Some(on) = a.osnap {
        status.push(Action::new("F3", format!("スナップ {}", on_off(on))));
    }
    if let Some(t) = a.tracking {
        let shift = if t.shift { "（Shift の間）" } else { "" };
        status.push(Action::new(
            "F8",
            format!("直交 {}{shift}", on_off(t.ortho)),
        ));
        let polar = if t.polar && t.ortho {
            "極 オン（直交が優先）".to_owned()
        } else {
            format!("極 {}", on_off(t.polar))
        };
        status.push(Action::new("F10", polar));
    }
    status
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(a: &Actions) -> Vec<String> {
        a.items.iter().map(|i| i.key.clone()).collect()
    }

    fn find<'a>(a: &'a Actions, key: &str) -> Option<&'a str> {
        a.items
            .iter()
            .find(|i| i.key == key)
            .map(|i| i.text.as_str())
    }

    const POINT: Stage = Stage {
        selecting: false,
        gripping: false,
        wants_entity: false,
        has_base: true,
    };

    const BOTH_ON: Assists = Assists {
        osnap: Some(true),
        tracking: Some(Tracking {
            ortho: false,
            polar: true,
            shift: false,
        }),
    };

    /// 長さの欄で何も打っていない（ふだんの寸法入力）。
    const IDLE_DIM: DimHint = DimHint {
        kind: DimKind::LengthAngle,
        field: Some(Field::Length),
        tab: Some(DimTab::Move(Field::Angle)),
        enter_confirms: false,
        esc: None,
    };

    /// LINE の 3 本目: クリック・寸法入力・座標・C 閉じる・Enter 終了・Esc 中断の順。
    #[test]
    fn line_next_point_with_dimension_input() {
        let a = build(
            "線分の次の点を指定 [閉じる(C)] <Enter で終了>:",
            POINT,
            Some(IDLE_DIM),
            false,
            BOTH_ON,
        );
        assert_eq!(
            keys(&a),
            vec![
                "クリック",
                "数字",
                "Tab",
                "x,y",
                "@x,y",
                "C",
                "Enter",
                "Esc"
            ]
        );
        assert_eq!(find(&a, "数字"), Some("長さ"));
        assert_eq!(find(&a, "Tab"), Some("角度の欄へ"));
        assert_eq!(find(&a, "x,y"), Some("座標"));
        assert_eq!(find(&a, "@x,y"), Some("相対"));
        assert_eq!(find(&a, "C"), Some("閉じる"));
        assert_eq!(find(&a, "Enter"), Some("終了"));
        assert_eq!(find(&a, "Esc"), Some("中断"));
        assert_eq!(
            a.status,
            vec![
                Action::new("F3", "スナップ オン"),
                Action::new("F8", "直交 オフ"),
                Action::new("F10", "極 オン"),
            ]
        );
    }

    /// 寸法入力: 渡されたキーの効き目どおりに Tab / Enter / Esc を書く。
    #[test]
    fn dimension_keys_follow_what_the_keys_do() {
        let prompt = "線分の次の点を指定 <Enter で終了>:";
        let typed = build(
            prompt,
            POINT,
            Some(DimHint {
                tab: Some(DimTab::LockAndMove {
                    locked: Field::Length,
                    to: Field::Angle,
                }),
                enter_confirms: true,
                esc: Some(EscClears {
                    locks: false,
                    input: true,
                }),
                ..IDLE_DIM
            }),
            true,
            BOTH_ON,
        );
        assert_eq!(find(&typed, "Tab"), Some("長さを固定して角度へ"));
        assert_eq!(find(&typed, "Enter"), Some("この値で確定"));
        assert_eq!(find(&typed, "Esc"), Some("入力を消す"));

        let angle = build(
            prompt,
            POINT,
            Some(DimHint {
                field: Some(Field::Angle),
                tab: Some(DimTab::Move(Field::Length)),
                enter_confirms: true,
                esc: Some(EscClears {
                    locks: true,
                    input: false,
                }),
                ..IDLE_DIM
            }),
            false,
            BOTH_ON,
        );
        assert_eq!(find(&angle, "数字"), Some("角度"));
        assert_eq!(find(&angle, "Tab"), Some("長さの欄へ"));
        assert_eq!(find(&angle, "Esc"), Some("固定を外す"));

        // 座標を打ちかけ: 欄は消え、Tab は何もしない。Esc は入力を消し、Enter は打った内容で確定。
        let coords = build(
            prompt,
            POINT,
            Some(DimHint {
                field: None,
                tab: None,
                enter_confirms: false,
                esc: Some(EscClears {
                    locks: true,
                    input: true,
                }),
                ..IDLE_DIM
            }),
            true,
            BOTH_ON,
        );
        assert_eq!(find(&coords, "数字"), None);
        assert_eq!(find(&coords, "Tab"), None);
        assert_eq!(find(&coords, "Enter"), Some("入力を確定"));
        assert_eq!(find(&coords, "Esc"), Some("固定と入力を外す"));

        let radius = DimHint {
            kind: DimKind::Radius,
            tab: None,
            ..IDLE_DIM
        };
        let grip = Stage {
            gripping: true,
            ..POINT
        };
        let r = build(
            "** 四分点 ** 点を指定 (空の Enter・Esc で取り消し):",
            grip,
            Some(radius),
            false,
            BOTH_ON,
        );
        assert_eq!(find(&r, "数字"), Some("半径"));
        assert_eq!(find(&r, "Tab"), None);
        assert_eq!(find(&r, "クリック"), Some("移動先"));
        assert_eq!(find(&r, "空の Enter・Esc"), Some("取り消し"));
        assert_eq!(find(&r, "Esc"), None, "Esc は取り消しにまとめる");

        let locking = build(
            "** 四分点 ** 点を指定 (空の Enter・Esc で取り消し):",
            grip,
            Some(DimHint {
                tab: Some(DimTab::Lock(Field::Length)),
                ..radius
            }),
            true,
            BOTH_ON,
        );
        assert_eq!(find(&locking, "Tab"), Some("半径を固定"));
        assert_eq!(find(&locking, "Enter"), Some("入力を確定"));
        assert_eq!(
            find(&locking, "Esc"),
            Some("取り消し"),
            "打っている間も Esc の意味は残る"
        );
    }

    #[test]
    fn selection_stage() {
        let a = build(
            "オブジェクトを選択 (Enter で確定):",
            Stage {
                selecting: true,
                ..Stage::default()
            },
            None,
            false,
            BOTH_ON,
        );
        assert_eq!(
            keys(&a),
            vec![
                "クリック",
                "左→右ドラッグ",
                "右→左ドラッグ",
                "Shift+クリック",
                "Ctrl+A",
                "Enter",
                "Esc"
            ]
        );
        assert!(a.status.is_empty(), "選択では作図補助は効かない");
    }

    #[test]
    fn picking_an_entity_with_an_option() {
        let a = build(
            "1 本目の線分をクリック [半径(R)]:",
            Stage {
                wants_entity: true,
                ..Stage::default()
            },
            None,
            false,
            BOTH_ON,
        );
        assert_eq!(keys(&a), vec!["クリック", "R", "Esc"]);
        assert_eq!(find(&a, "R"), Some("半径"));
        assert!(a.status.is_empty(), "図形を指す段階ではスナップは効かない");
    }

    #[test]
    fn values_can_be_clicked_only_with_a_base() {
        let with = build("円の半径を指定 [直径(D)]:", POINT, None, false, BOTH_ON);
        assert_eq!(keys(&with), vec!["数字", "クリック", "D", "Esc"]);
        assert_eq!(find(&with, "数字"), Some("円の半径"));
        assert!(
            !with.status.is_empty(),
            "クリックで指せるなら作図補助が効く"
        );

        let without = build("半径を指定:", Stage::default(), None, false, BOTH_ON);
        assert_eq!(keys(&without), vec!["数字", "Esc"]);
        assert!(without.status.is_empty());
    }

    #[test]
    fn text_and_choice() {
        let name = build(
            "グループ名を入力 <Enter で既定名>:",
            Stage::default(),
            None,
            false,
            Assists::default(),
        );
        assert_eq!(keys(&name), vec!["", "Enter", "Esc"]);
        assert_eq!(find(&name, ""), Some("グループ名を打つ"));
        let typed_name = build(
            "グループ名を入力 <Enter で既定名>:",
            Stage::default(),
            None,
            true,
            Assists::default(),
        );
        assert_eq!(
            find(&typed_name, "Enter"),
            Some("入力を確定"),
            "打ったら既定名ではない"
        );
        assert_eq!(find(&name, "Enter"), Some("既定名"));

        let yn = build(
            "元のオブジェクトを消去しますか? [はい(Y)/いいえ(N)] <N>:",
            Stage::default(),
            None,
            false,
            BOTH_ON,
        );
        assert_eq!(keys(&yn), vec!["Y", "N", "Enter", "Esc"]);
        assert_eq!(find(&yn, "Enter"), Some("N（既定）"));
        assert!(yn.status.is_empty());

        let pset = build(
            "「幅」の値を入力 <R でリセット>:\n\u{3000}900",
            Stage::default(),
            None,
            false,
            Assists::default(),
        );
        assert_eq!(keys(&pset), vec!["", "R", "Enter", "Esc"]);
        assert_eq!(find(&pset, ""), Some("「幅」の値を打つ（数値・式）"));
    }

    /// 2 行目の作図補助: 実際に効いている状態に合わせる（Shift の一時反転・直交の優先）。
    #[test]
    fn assists_reflect_what_is_in_effect() {
        let shift = Assists {
            osnap: Some(false),
            tracking: Some(Tracking {
                ortho: true,
                polar: true,
                shift: true,
            }),
        };
        let a = build("基点を指定:", POINT, None, false, shift);
        assert_eq!(
            a.status,
            vec![
                Action::new("F3", "スナップ オフ"),
                Action::new("F8", "直交 オン（Shift の間）"),
                Action::new("F10", "極 オン（直交が優先）"),
            ]
        );

        // 基準点が無い点（直交・極が効かない）ではスナップだけ。
        let first = build(
            "線分の始点を指定:",
            Stage::default(),
            None,
            false,
            Assists {
                osnap: Some(true),
                tracking: None,
            },
        );
        assert_eq!(first.status, vec![Action::new("F3", "スナップ オン")]);
    }

    #[test]
    fn notes_are_kept() {
        let a = build(
            "目的点を指定 <Enter で終了> (複写 2 個):",
            POINT,
            None,
            false,
            BOTH_ON,
        );
        assert_eq!(
            keys(&a),
            vec!["クリック", "x,y", "@x,y", "", "Enter", "Esc"]
        );
        assert_eq!(find(&a, ""), Some("複写 2 個"));
    }
}
