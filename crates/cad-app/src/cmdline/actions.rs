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

/// 寸法入力の欄（Issue #20 段階 B）の状態。欄が出ているときだけ渡す。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DimHint {
    /// 欄の見せ方（「長さ」「角度」か「半径」だけか）。
    pub kind: DimKind,
    /// 入力中の欄。
    pub field: Field,
    /// 固定した値（錠前）がある。
    pub locked: bool,
    /// 入力欄に打ちかけの数値がある。
    pub has_input: bool,
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
#[must_use]
pub fn build(prompt_text: &str, stage: Stage, dim: Option<DimHint>, assists: Assists) -> Actions {
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
        items.push(Action::new("x,y / @x,y", "座標"));
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
                items.push(Action::new("文字", format!("{name}を打つ")));
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

    // 寸法入力で固定・入力があると、Enter はその値で確定し、Esc はまず固定と入力を外す。
    if dim.is_some_and(|d| d.locked || d.has_input) {
        enter = Some(Action::new("Enter", "この値で確定"));
        esc = Some(Action::new("Esc", "固定と入力を外す"));
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

/// 寸法入力の欄の操作。入力中の欄と固定の有無で `Tab` の意味が変わる。
fn dimension_items(d: DimHint) -> Vec<Action> {
    let name = d.kind.name(d.field);
    let mut items = vec![Action::new("数字", name)];
    let tab = match d.kind {
        DimKind::Radius => "半径を固定".to_owned(),
        DimKind::LengthAngle => {
            let next = d.kind.name(d.field.next());
            if d.has_input {
                format!("{name}を固定して{next}へ")
            } else {
                format!("{next}の欄へ")
            }
        }
    };
    items.push(Action::new("Tab", tab));
    items
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

    /// LINE の 3 本目: クリック・寸法入力・C 閉じる・Enter 終了・Esc 中断の順。
    #[test]
    fn line_next_point_with_dimension_input() {
        let dim = DimHint {
            kind: DimKind::LengthAngle,
            field: Field::Length,
            locked: false,
            has_input: false,
        };
        let a = build(
            "線分の次の点を指定 [閉じる(C)] <Enter で終了>:",
            POINT,
            Some(dim),
            BOTH_ON,
        );
        assert_eq!(
            keys(&a),
            vec!["クリック", "数字", "Tab", "x,y / @x,y", "C", "Enter", "Esc"]
        );
        assert_eq!(find(&a, "数字"), Some("長さ"));
        assert_eq!(find(&a, "Tab"), Some("角度の欄へ"));
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

    /// 寸法入力: 入力中の欄と固定の有無で Tab / Enter / Esc の意味が変わる。
    #[test]
    fn dimension_fields_change_tab_enter_and_escape() {
        let prompt = "線分の次の点を指定 <Enter で終了>:";
        let d = |field, locked, has_input| DimHint {
            kind: DimKind::LengthAngle,
            field,
            locked,
            has_input,
        };
        let typed = build(prompt, POINT, Some(d(Field::Length, false, true)), BOTH_ON);
        assert_eq!(find(&typed, "Tab"), Some("長さを固定して角度へ"));
        assert_eq!(find(&typed, "Enter"), Some("この値で確定"));
        assert_eq!(find(&typed, "Esc"), Some("固定と入力を外す"));

        let angle = build(prompt, POINT, Some(d(Field::Angle, true, false)), BOTH_ON);
        assert_eq!(find(&angle, "数字"), Some("角度"));
        assert_eq!(find(&angle, "Tab"), Some("長さの欄へ"));
        assert_eq!(find(&angle, "Esc"), Some("固定と入力を外す"), "固定がある");

        let radius = DimHint {
            kind: DimKind::Radius,
            field: Field::Length,
            locked: false,
            has_input: false,
        };
        let r = build(
            "** 四分点 ** 点を指定 (空の Enter・Esc で取り消し):",
            Stage {
                gripping: true,
                ..POINT
            },
            Some(radius),
            BOTH_ON,
        );
        assert_eq!(find(&r, "数字"), Some("半径"));
        assert_eq!(find(&r, "Tab"), Some("半径を固定"));
        assert_eq!(find(&r, "クリック"), Some("移動先"));
        assert_eq!(find(&r, "空の Enter・Esc"), Some("取り消し"));
        assert_eq!(find(&r, "Esc"), None, "Esc は取り消しにまとめる");
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
            BOTH_ON,
        );
        assert_eq!(keys(&a), vec!["クリック", "R", "Esc"]);
        assert_eq!(find(&a, "R"), Some("半径"));
        assert!(a.status.is_empty(), "図形を指す段階ではスナップは効かない");
    }

    #[test]
    fn values_can_be_clicked_only_with_a_base() {
        let with = build("円の半径を指定 [直径(D)]:", POINT, None, BOTH_ON);
        assert_eq!(keys(&with), vec!["数字", "クリック", "D", "Esc"]);
        assert_eq!(find(&with, "数字"), Some("円の半径"));
        assert!(
            !with.status.is_empty(),
            "クリックで指せるなら作図補助が効く"
        );

        let without = build("半径を指定:", Stage::default(), None, BOTH_ON);
        assert_eq!(keys(&without), vec!["数字", "Esc"]);
        assert!(without.status.is_empty());
    }

    #[test]
    fn text_and_choice() {
        let name = build(
            "グループ名を入力 <Enter で既定名>:",
            Stage::default(),
            None,
            Assists::default(),
        );
        assert_eq!(keys(&name), vec!["文字", "Enter", "Esc"]);
        assert_eq!(find(&name, "文字"), Some("グループ名を打つ"));
        assert_eq!(find(&name, "Enter"), Some("既定名"));

        let yn = build(
            "元のオブジェクトを消去しますか? [はい(Y)/いいえ(N)] <N>:",
            Stage::default(),
            None,
            BOTH_ON,
        );
        assert_eq!(keys(&yn), vec!["Y", "N", "Enter", "Esc"]);
        assert_eq!(find(&yn, "Enter"), Some("N（既定）"));
        assert!(yn.status.is_empty());

        let pset = build(
            "「幅」の値を入力 <R でリセット>:\n\u{3000}900",
            Stage::default(),
            None,
            Assists::default(),
        );
        assert_eq!(keys(&pset), vec!["文字", "R", "Enter", "Esc"]);
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
        let a = build("基点を指定:", POINT, None, shift);
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
            BOTH_ON,
        );
        assert_eq!(keys(&a), vec!["クリック", "x,y / @x,y", "", "Enter", "Esc"]);
        assert_eq!(find(&a, ""), Some("複写 2 個"));
    }
}
