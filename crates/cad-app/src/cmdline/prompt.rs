//! プロンプトの文字列を読む（Issue #27 段階 2）。
//!
//! 各ツールのプロンプトは書式がそろっていない（`[閉じる(C)]`・`[X / Y]`・`[はい(Y)/いいえ(N)] <N>`・
//! `<Enter で終了>`・`<R でリセット>`・`(Enter で削除)`・`(空の Enter・Esc で取り消し)`・
//! `（番号で選べます）` など）。ツールを書き換えずに「今できること」を組み立てるため、
//! ここで 1 か所で読む。**どんな文字列でも失敗しない（全域関数）**。読めない部分は捨てずに
//! 「添え書き」として残す。
//!
//! 段階 C（オプションをクリックで選ぶ、#20）でも、オプションの一覧の土台として使える。
//!
//! **egui に依存しない純粋関数だけを置く。** 全ツールの全段階のプロンプトを読めることは
//! テスト（ソースの走査と、実際にツールを動かすもの）で確かめる。

/// プロンプトが求めているもの。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ask {
    /// 何もしていない（`コマンド:`）。
    Command,
    /// 点（`…点を指定`・`…の角を指定`・`…する側を指定`）。
    Point,
    /// 値（`半径を指定`・`尺度を指定` など）。中身は値の名前。
    Value(String),
    /// 図形の選択（`…を選択`）。
    Select,
    /// 図形をクリックで指す（`…をクリック`）。
    Click,
    /// 文字を打つ（`…を入力`・`…名:`）。中身は打つものの名前。
    Text(String),
    /// オプションから選ぶだけ（`[はい(Y)/いいえ(N)]`・`[X / Y]`・`ZOOM オプションを指定`）。
    Choice,
    /// どれとも読めない。
    Unknown,
}

/// オプション 1 つ（`閉じる(C)` → キー `C`・名前 `閉じる`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptOption {
    /// 打つキー（`C`・`2P`）。
    pub key: String,
    /// 名前（`閉じる`）。`[X / Y]` のように名前の無いものは空。
    pub label: String,
}

/// `A で B` の形の添え書き（`<Enter で終了>` → キー `Enter`・意味 `終了`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyNote {
    /// 押すもの（`Enter`・`R`・`空の Enter・Esc`）。
    pub key: String,
    /// 何が起きるか（`終了`・`リセット`・`取り消し`）。
    pub meaning: String,
}

/// 読んだプロンプト。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prompt {
    /// 指示の本文（括弧と末尾の `:` を除いた 1 行目。`線分の次の点を指定`）。
    pub head: String,
    /// 求めているもの。
    pub ask: Ask,
    /// `[...]` のオプション。
    pub options: Vec<PromptOption>,
    /// `<Enter で終了>`・`<R でリセット>`・`(Enter で削除)` のような、キーと意味の添え書き。
    pub keys: Vec<KeyNote>,
    /// `<N>` のような既定の答え（空 Enter で選ばれる）。
    pub default: Option<String>,
    /// それ以外の添え書き（`(複写 2 個)`・`<コピー>`・`（番号で選べます）`）。
    pub notes: Vec<String>,
}

/// プロンプトを読む。どんな文字列でも失敗しない。
///
/// 2 行目以降（一覧や例）は読まない。1 行目に指示と括弧がまとまっているため。
#[must_use]
pub fn parse(prompt: &str) -> Prompt {
    let first = prompt.lines().next().unwrap_or_default().trim();
    let first = first
        .strip_suffix(':')
        .or_else(|| first.strip_suffix('：'))
        .unwrap_or(first)
        .trim_end();

    let mut head = String::new();
    let mut options = Vec::new();
    let mut keys = Vec::new();
    let mut default = None;
    let mut notes = Vec::new();

    let mut rest = first;
    while let Some((before, open, inner, after)) = next_group(rest) {
        head.push_str(before);
        match open {
            '[' => options.extend(parse_options(inner)),
            _ => match parse_key_note(inner) {
                Some(k) => keys.push(k),
                None if open == '<' && is_key(inner.trim()) => {
                    default = Some(inner.trim().to_owned());
                }
                None => notes.push(inner.trim().to_owned()),
            },
        }
        rest = after;
    }
    head.push_str(rest);
    let head = head.split_whitespace().collect::<Vec<_>>().join(" ");
    let ask = classify(&head, &options);
    Prompt {
        head,
        ask,
        options,
        keys,
        default,
        notes,
    }
}

/// 次の括弧のまとまり（`[...]`・`<...>`・`(...)`・`（...）`）を探し、
/// (前, 開き括弧, 中身, 後ろ) を返す。閉じていなければ `None`（残りは本文のまま）。
///
/// 括弧は入れ子になりうる（`[閉じる(C)]`）ので、同じ種類の括弧の深さを数える。
fn next_group(s: &str) -> Option<(&str, char, &str, &str)> {
    let (start, open) = s
        .char_indices()
        .find(|(_, c)| matches!(c, '[' | '<' | '(' | '（'))?;
    let close = match open {
        '[' => ']',
        '<' => '>',
        '(' => ')',
        _ => '）',
    };
    let inner_start = start + open.len_utf8();
    let mut depth = 1;
    for (i, c) in s[inner_start..].char_indices() {
        if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                let end = inner_start + i;
                return Some((
                    &s[..start],
                    open,
                    &s[inner_start..end],
                    &s[end + close.len_utf8()..],
                ));
            }
        }
    }
    None
}

/// `[閉じる(C)]`・`[水平(H)/垂直(V)]`・`[X / Y]`・`[はい(Y)/いいえ(N)]` の中身を読む。
fn parse_options(inner: &str) -> Vec<PromptOption> {
    inner
        .split('/')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|item| match next_group(item) {
            Some((label, '(' | '（', key, _)) if is_key(key.trim()) => PromptOption {
                key: key.trim().to_owned(),
                label: label.trim().to_owned(),
            },
            _ if is_key(item) => PromptOption {
                key: item.to_owned(),
                label: String::new(),
            },
            _ => PromptOption {
                key: String::new(),
                label: item.to_owned(),
            },
        })
        .collect()
}

/// `Enter で終了`・`R でリセット`・`空の Enter・Esc で取り消し` を読む。
/// 押すものが `Enter`・`Esc` を含むか、キー（`R`）のときだけ。
fn parse_key_note(inner: &str) -> Option<KeyNote> {
    let (key, meaning) = inner.split_once(" で")?;
    let key = key.trim();
    let meaning = meaning.trim();
    let is_press = key.contains("Enter") || key.contains("Esc") || is_key(key);
    (is_press && !meaning.is_empty()).then(|| KeyNote {
        key: key.to_owned(),
        meaning: meaning.to_owned(),
    })
}

/// オプションのキーとして読める文字列か（半角の英大文字と数字、1〜3 文字）。
fn is_key(s: &str) -> bool {
    (1..=3).contains(&s.len())
        && s.chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        && s.chars().any(|c| c.is_ascii_uppercase())
}

/// 本文とオプションから、求めているものを決める。
fn classify(head: &str, options: &[PromptOption]) -> Ask {
    if head == "コマンド" {
        return Ask::Command;
    }
    if head.contains("を選択") {
        return Ask::Select;
    }
    if head.contains("をクリック") {
        return Ask::Click;
    }
    if let Some(subject) = head.strip_suffix("を入力") {
        return Ask::Text(unquote(subject));
    }
    if head.ends_with('名') {
        return Ask::Text(unquote(head));
    }
    if let Some(subject) = head.strip_suffix("を指定") {
        let subject = subject.trim();
        if subject.ends_with("オプション") {
            return Ask::Choice;
        }
        if subject.contains('点') || subject.ends_with('角') || subject.ends_with('側') {
            return Ask::Point;
        }
        return Ask::Value(unquote(subject));
    }
    let asks_which = head.contains('?') || head.contains('？') || head.contains("どちら");
    if asks_which && !options.is_empty() {
        return Ask::Choice;
    }
    Ask::Unknown
}

/// 打つものの名前から、前置き（`束縛する`）や引用（`「幅」の`）はそのまま、前後の空白だけ落とす。
fn unquote(s: &str) -> String {
    s.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opt(key: &str, label: &str) -> PromptOption {
        PromptOption {
            key: key.to_owned(),
            label: label.to_owned(),
        }
    }

    fn key(key: &str, meaning: &str) -> KeyNote {
        KeyNote {
            key: key.to_owned(),
            meaning: meaning.to_owned(),
        }
    }

    #[test]
    fn line_next_point_with_close_and_enter() {
        let p = parse("線分の次の点を指定 [閉じる(C)] <Enter で終了>:");
        assert_eq!(p.head, "線分の次の点を指定");
        assert_eq!(p.ask, Ask::Point);
        assert_eq!(p.options, vec![opt("C", "閉じる")]);
        assert_eq!(p.keys, vec![key("Enter", "終了")]);
        assert_eq!(p.default, None);
        assert!(p.notes.is_empty());
    }

    #[test]
    fn several_options_separated_by_slashes() {
        let p = parse("点を指定 [水平(H)/垂直(V)/角度(A)/オフセット(O)]:");
        assert_eq!(p.ask, Ask::Point);
        assert_eq!(
            p.options,
            vec![
                opt("H", "水平"),
                opt("V", "垂直"),
                opt("A", "角度"),
                opt("O", "オフセット")
            ]
        );
    }

    #[test]
    fn two_letter_keys() {
        let p = parse("円の中心点を指定 [2点(2P)]:");
        assert_eq!(p.options, vec![opt("2P", "2点")]);
        assert_eq!(p.ask, Ask::Point);
    }

    #[test]
    fn yes_no_with_a_default() {
        let p = parse("元のオブジェクトを消去しますか? [はい(Y)/いいえ(N)] <N>:");
        assert_eq!(p.ask, Ask::Choice);
        assert_eq!(p.options, vec![opt("Y", "はい"), opt("N", "いいえ")]);
        assert_eq!(p.default.as_deref(), Some("N"));
    }

    #[test]
    fn keys_without_labels() {
        let p = parse("「線分 1 の始点」のどちらを動かすか [X / Y]:");
        assert_eq!(p.ask, Ask::Choice);
        assert_eq!(p.options, vec![opt("X", ""), opt("Y", "")]);
    }

    #[test]
    fn ascii_and_full_width_parentheses() {
        let p = parse("オブジェクトを選択 (Enter で削除):");
        assert_eq!(p.ask, Ask::Select);
        assert_eq!(p.keys, vec![key("Enter", "削除")]);

        let p = parse("パラメータで動かしたい点をクリック（編集中でなければ名前を入力）:");
        assert_eq!(p.ask, Ask::Click);
        assert_eq!(p.notes, vec!["編集中でなければ名前を入力".to_owned()]);
    }

    #[test]
    fn grip_prompt() {
        let p = parse("** 端点 ** 点を指定 (空の Enter・Esc で取り消し):");
        assert_eq!(p.ask, Ask::Point);
        assert_eq!(p.keys, vec![key("空の Enter・Esc", "取り消し")]);
    }

    #[test]
    fn values_and_their_names() {
        assert_eq!(
            parse("円の半径を指定 [直径(D)]:").ask,
            Ask::Value("円の半径".to_owned())
        );
        assert_eq!(
            parse("回転角度を指定 <コピー>:").ask,
            Ask::Value("回転角度".to_owned())
        );
        assert_eq!(parse("回転角度を指定 <コピー>:").notes, vec!["コピー"]);
        assert_eq!(
            parse("倍率を指定 <Enter で 1>:").keys,
            vec![key("Enter", "1")]
        );
    }

    #[test]
    fn points_by_their_names() {
        for p in [
            "矩形の 1 つ目の角を指定:",
            "矩形のもう一方の角を指定:",
            "オフセットする側を指定:",
            "対称軸の 2 点目を指定:",
            "基点を指定:",
            "目的点を指定 <Enter で終了> (複写 3 個):",
        ] {
            assert_eq!(parse(p).ask, Ask::Point, "{p}");
        }
        assert_eq!(
            parse("目的点を指定 <Enter で終了> (複写 3 個):").notes,
            vec!["複写 3 個"]
        );
    }

    #[test]
    fn text_prompts() {
        assert_eq!(
            parse("グループ名を入力 <Enter で既定名>:").ask,
            Ask::Text("グループ名".to_owned())
        );
        assert_eq!(
            parse("差し替えるコンポーネント名:").ask,
            Ask::Text("差し替えるコンポーネント名".to_owned())
        );
        let p = parse("「幅」の値を入力 <R でリセット>:\n\u{3000}900");
        assert_eq!(p.ask, Ask::Text("「幅」の値".to_owned()));
        assert_eq!(p.keys, vec![key("R", "リセット")]);
    }

    #[test]
    fn zoom_options_are_a_choice() {
        let p = parse("ZOOM オプションを指定 [全体(A)/範囲(E)]:");
        assert_eq!(p.ask, Ask::Choice);
        assert_eq!(p.options, vec![opt("A", "全体"), opt("E", "範囲")]);
    }

    #[test]
    fn idle() {
        assert_eq!(parse("コマンド:").ask, Ask::Command);
    }

    /// どんな文字列でも落ちない。閉じていない括弧は本文に残る。
    #[test]
    fn total_on_odd_input() {
        for s in [
            "",
            ":",
            "[",
            "<",
            "(",
            "（",
            "]",
            "[[[",
            "a [b(c] d",
            "<Enter で>",
            "[(C)]",
            "点を指定 [閉じる(C)",
            "\n\n",
            "（）",
            "<>",
            "[/]",
        ] {
            let _ = parse(s);
        }
        assert_eq!(parse("点を指定 [閉じる(C)").head, "点を指定 [閉じる(C)");
        assert_eq!(parse("何か").ask, Ask::Unknown);
    }
}
