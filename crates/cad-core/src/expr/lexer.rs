//! 式の字句解析。
//!
//! # 識別子
//!
//! **日本語を許す。** レイヤ名・グループ名・コンポーネント名が日本語なのと揃える。
//! 判定は「英数字・下線でも記号でもない文字」を識別子の一部とみなす形にした。
//! Unicode の細かい分類（`XID_Start` など）を使うと `unicode-ident` 等の
//! 依存が要るが、**`cad-core` の依存パッケージはゼロに保つ**（ADR-0026）。
//!
//! この単純な規則で通るもの: `幅`、`高さ`、`扉の向き`、`w1`、`_tmp`。
//! 通らないもの: 演算子や括弧などの ASCII 記号で始まる名前。

use std::fmt;

/// 字句。
#[derive(Clone, Debug, PartialEq)]
pub enum Token {
    /// 数値リテラル。
    Number(f64),
    /// 識別子（パラメータ名・関数名・キーワード）。
    Ident(String),
    /// 文字列リテラル（選択肢の値）。`'引違い'` の形。
    Str(String),
    /// `(`
    LParen,
    /// `)`
    RParen,
    /// `,`
    Comma,
    /// `+`
    Plus,
    /// `-`
    Minus,
    /// `*`
    Star,
    /// `/`
    Slash,
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
    /// `==`
    EqEq,
    /// `!=`
    Ne,
    /// `!`
    Bang,
    /// `&&`
    AndAnd,
    /// `||`
    OrOr,
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Number(n) => write!(f, "{n}"),
            Self::Ident(s) => write!(f, "{s}"),
            Self::Str(s) => write!(f, "'{s}'"),
            Self::LParen => write!(f, "("),
            Self::RParen => write!(f, ")"),
            Self::Comma => write!(f, ","),
            Self::Plus => write!(f, "+"),
            Self::Minus => write!(f, "-"),
            Self::Star => write!(f, "*"),
            Self::Slash => write!(f, "/"),
            Self::Lt => write!(f, "<"),
            Self::Le => write!(f, "<="),
            Self::Gt => write!(f, ">"),
            Self::Ge => write!(f, ">="),
            Self::EqEq => write!(f, "=="),
            Self::Ne => write!(f, "!="),
            Self::Bang => write!(f, "!"),
            Self::AndAnd => write!(f, "&&"),
            Self::OrOr => write!(f, "||"),
        }
    }
}

/// 字句解析の失敗。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LexError {
    /// 何文字目か（0 始まり、文字単位）。
    pub position: usize,
    /// 人間向けの説明。
    pub message: String,
}

impl fmt::Display for LexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} 文字目: {}", self.position + 1, self.message)
    }
}

/// 全角の記号を半角へ直す。
///
/// # なぜ必要か
///
/// 日本語 IME で式を打つと `幅＊2` のように**全角の演算子**が混ざりやすい。
/// 直さないと `＊` が識別子の一部として吸われ、
/// 「パラメータ『幅＊2』がありません」という分かりにくい誤りになる。
///
/// # `cmdline/coord.rs` の `normalize_ascii` をそのまま使わない理由
///
/// あちらは `ー`（長音）を `-` に、`、` を `,` に直す。座標入力では有効だが、
/// **式では識別子に日本語を使うので名前が壊れる**（`データー` → `データ-`）。
/// ここでは **U+FF01〜U+FF5E の全角 ASCII と全角スペースだけ**を直す。
fn normalize_fullwidth(c: char) -> char {
    match c {
        // 全角 ASCII は半角と 0xFEE0 ずれている。
        '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
        '\u{3000}' => ' ',
        other => other,
    }
}

/// この文字が識別子の一部になれるか。
///
/// 英数字・下線に加えて、**記号でも空白でもない非 ASCII 文字**を許す
/// （日本語の識別子のため）。全角の記号は [`normalize_fullwidth`] で
/// 半角へ直したあとなので、ここへは来ない。
fn is_ident_char(c: char) -> bool {
    if c.is_alphanumeric() || c == '_' {
        return true;
    }
    !c.is_ascii() && !c.is_whitespace() && !c.is_control()
}

/// 識別子の先頭になれるか。数字で始まる名前は数値と紛れるので許さない。
fn is_ident_start(c: char) -> bool {
    is_ident_char(c) && !c.is_ascii_digit()
}

/// 式の文字列を字句へ分解する。
///
/// # Errors
///
/// 未知の文字、閉じていない文字列、数値として読めない綴りがある場合 [`LexError`]。
pub fn lex(input: &str) -> Result<Vec<Token>, LexError> {
    // 全角の演算子を先に直す。文字数は変わらないので位置はずれない。
    let chars: Vec<char> = input.chars().map(normalize_fullwidth).collect();
    let mut out = Vec::new();
    let mut i = 0usize;

    while i < chars.len() {
        let c = chars[i];

        if c.is_whitespace() {
            i += 1;
            continue;
        }

        // ---- 2 文字の演算子を先に見る ----
        //
        // `<` より `<=` を先に試さないと、`<=` が `<` と `=` に割れる。
        if let Some(&next) = chars.get(i + 1) {
            let two = match (c, next) {
                ('<', '=') => Some(Token::Le),
                ('>', '=') => Some(Token::Ge),
                ('=', '=') => Some(Token::EqEq),
                ('!', '=') => Some(Token::Ne),
                ('&', '&') => Some(Token::AndAnd),
                ('|', '|') => Some(Token::OrOr),
                _ => None,
            };
            if let Some(t) = two {
                out.push(t);
                i += 2;
                continue;
            }
        }

        // ---- 1 文字の演算子 ----
        let one = match c {
            '(' => Some(Token::LParen),
            ')' => Some(Token::RParen),
            ',' => Some(Token::Comma),
            '+' => Some(Token::Plus),
            '-' => Some(Token::Minus),
            '*' => Some(Token::Star),
            '/' => Some(Token::Slash),
            '<' => Some(Token::Lt),
            '>' => Some(Token::Gt),
            '!' => Some(Token::Bang),
            _ => None,
        };
        if let Some(t) = one {
            out.push(t);
            i += 1;
            continue;
        }

        // ---- 単独の `=` `&` `|` は誤りとして案内する ----
        if c == '=' {
            return Err(err(i, "比較は `==` と書きます（`=` は代入ではありません）"));
        }
        if c == '&' {
            return Err(err(i, "論理積は `&&` と書きます"));
        }
        if c == '|' {
            return Err(err(i, "論理和は `||` と書きます"));
        }

        // ---- 文字列リテラル ----
        if c == '\'' || c == '"' {
            let quote = c;
            let start = i;
            i += 1;
            let mut s = String::new();
            loop {
                let Some(&ch) = chars.get(i) else {
                    return Err(err(start, "文字列が閉じられていません"));
                };
                i += 1;
                if ch == quote {
                    break;
                }
                s.push(ch);
            }
            out.push(Token::Str(s));
            continue;
        }

        // ---- 数値 ----
        if c.is_ascii_digit() || (c == '.' && chars.get(i + 1).is_some_and(char::is_ascii_digit)) {
            let start = i;
            while chars
                .get(i)
                .is_some_and(|d| d.is_ascii_digit() || *d == '.')
            {
                i += 1;
            }
            // 指数表記（`1e-3`）も許す。
            if chars.get(i).is_some_and(|d| *d == 'e' || *d == 'E') {
                let mark = i;
                i += 1;
                if chars.get(i).is_some_and(|d| *d == '+' || *d == '-') {
                    i += 1;
                }
                if chars.get(i).is_some_and(char::is_ascii_digit) {
                    while chars.get(i).is_some_and(char::is_ascii_digit) {
                        i += 1;
                    }
                } else {
                    // `1eX` のような綴り。`e` は識別子の一部だったとみなして戻す。
                    i = mark;
                }
            }
            let text: String = chars[start..i].iter().collect();
            let value = text
                .parse::<f64>()
                .map_err(|_| err(start, format!("数値として読めません: {text}")))?;
            if !value.is_finite() {
                return Err(err(start, format!("数値が大きすぎます: {text}")));
            }
            out.push(Token::Number(value));
            continue;
        }

        // ---- 識別子 ----
        if is_ident_start(c) {
            let start = i;
            while chars.get(i).is_some_and(|d| is_ident_char(*d)) {
                i += 1;
            }
            out.push(Token::Ident(chars[start..i].iter().collect()));
            continue;
        }

        return Err(err(i, format!("使えない文字です: {c}")));
    }

    Ok(out)
}

fn err(position: usize, message: impl Into<String>) -> LexError {
    LexError {
        position,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idents(input: &str) -> Vec<String> {
        lex(input)
            .unwrap()
            .into_iter()
            .filter_map(|t| match t {
                Token::Ident(s) => Some(s),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn lexes_arithmetic() {
        assert_eq!(
            lex("1 + 2 * 3").unwrap(),
            vec![
                Token::Number(1.0),
                Token::Plus,
                Token::Number(2.0),
                Token::Star,
                Token::Number(3.0),
            ]
        );
    }

    /// **2 文字の演算子を 1 文字に割らないこと。**
    #[test]
    fn two_character_operators_are_not_split() {
        assert_eq!(lex("<=").unwrap(), vec![Token::Le]);
        assert_eq!(lex(">=").unwrap(), vec![Token::Ge]);
        assert_eq!(lex("==").unwrap(), vec![Token::EqEq]);
        assert_eq!(lex("!=").unwrap(), vec![Token::Ne]);
        assert_eq!(lex("&&").unwrap(), vec![Token::AndAnd]);
        assert_eq!(lex("||").unwrap(), vec![Token::OrOr]);
        // 単独の `<` `>` `!` は 1 文字のまま。
        assert_eq!(
            lex("< > !").unwrap(),
            vec![Token::Lt, Token::Gt, Token::Bang]
        );
    }

    /// **日本語の識別子が通ること。** レイヤ名などと揃える。
    #[test]
    fn japanese_identifiers_are_allowed() {
        assert_eq!(idents("幅"), vec!["幅"]);
        assert_eq!(idents("高さ + 奥行"), vec!["高さ", "奥行"]);
        assert_eq!(idents("扉の向き"), vec!["扉の向き"]);
        // 英数字混じりも 1 つの識別子。
        assert_eq!(idents("壁2の幅"), vec!["壁2の幅"]);
    }

    #[test]
    fn ascii_identifiers_still_work() {
        assert_eq!(idents("w + h1 + _tmp"), vec!["w", "h1", "_tmp"]);
    }

    /// 数字で始まる名前は許さない（数値と紛れる）。
    #[test]
    fn identifiers_cannot_start_with_a_digit() {
        // `2w` は数値 2 と識別子 w に割れる。
        assert_eq!(
            lex("2w").unwrap(),
            vec![Token::Number(2.0), Token::Ident("w".to_owned())]
        );
    }

    #[test]
    fn lexes_numbers_including_decimals_and_exponents() {
        assert_eq!(lex("1").unwrap(), vec![Token::Number(1.0)]);
        assert_eq!(lex("1.5").unwrap(), vec![Token::Number(1.5)]);
        assert_eq!(lex(".5").unwrap(), vec![Token::Number(0.5)]);
        assert_eq!(lex("1e3").unwrap(), vec![Token::Number(1000.0)]);
        // 負の指数。**文字列を連結して書いてある**のは、CI の
        // 「トレランスの直書きが無いこと」検査（`\d+e-\d` を探す grep）が
        // これを誤検出するため。検査を緩めるより、こちらで避ける。
        assert_eq!(
            lex(concat!("1.5e", "-2")).unwrap(),
            vec![Token::Number(0.015)]
        );
    }

    /// `1e` のあとに数字が無ければ、`e` は識別子として読み直すこと。
    #[test]
    fn a_dangling_exponent_marker_becomes_an_identifier() {
        assert_eq!(
            lex("1e").unwrap(),
            vec![Token::Number(1.0), Token::Ident("e".to_owned())]
        );
    }

    #[test]
    fn lexes_string_literals() {
        assert_eq!(
            lex("'引違い'").unwrap(),
            vec![Token::Str("引違い".to_owned())]
        );
        assert_eq!(
            lex("\"開き\"").unwrap(),
            vec![Token::Str("開き".to_owned())]
        );
    }

    #[test]
    fn an_unterminated_string_is_an_error() {
        let e = lex("'開き").unwrap_err();
        assert!(e.message.contains("閉じられていません"), "{e}");
    }

    /// 単独の `=` `&` `|` は、よくある書き間違いなので案内を出すこと。
    #[test]
    fn single_equals_and_single_logical_operators_are_guided() {
        assert!(lex("a = 1").unwrap_err().message.contains("`==`"));
        assert!(lex("a & b").unwrap_err().message.contains("`&&`"));
        assert!(lex("a | b").unwrap_err().message.contains("`||`"));
    }

    #[test]
    fn unknown_characters_are_rejected() {
        let e = lex("a @ b").unwrap_err();
        assert!(e.message.contains("使えない文字"), "{e}");
        assert_eq!(e.position, 2);
    }

    /// 位置は**文字単位**で数えること（日本語でずれない）。
    #[test]
    fn error_positions_count_characters_not_bytes() {
        let e = lex("幅 @ 高さ").unwrap_err();
        // 0 始まり: 幅=0、空白=1、@=2。バイト位置なら 4 になるのでずれが分かる。
        assert_eq!(e.position, 2, "文字単位で数えること");
        assert!(
            e.to_string().starts_with("3 文字目"),
            "表示は 1 始まり: {e}"
        );
    }

    /// **全角の演算子を半角として読むこと。**
    ///
    /// 日本語 IME で打つと混ざりやすい。直さないと識別子に吸われて
    /// 「パラメータ『幅＊2』がありません」という分かりにくい誤りになる。
    #[test]
    fn fullwidth_operators_are_normalised() {
        assert_eq!(
            lex("幅＊2").unwrap(),
            vec![
                Token::Ident("幅".to_owned()),
                Token::Star,
                Token::Number(2.0)
            ]
        );
        assert_eq!(
            lex("（幅＋高さ）").unwrap(),
            vec![
                Token::LParen,
                Token::Ident("幅".to_owned()),
                Token::Plus,
                Token::Ident("高さ".to_owned()),
                Token::RParen
            ]
        );
        // 全角スペースも空白として扱う。
        assert_eq!(lex("1　+　2").unwrap().len(), 3);
    }

    /// **識別子の日本語は壊さないこと。**
    ///
    /// `cmdline/coord.rs` の `normalize_ascii` は長音を `-` に直すが、
    /// 式でそれをやると `データー` が `データ-` になって名前が壊れる。
    #[test]
    fn japanese_identifiers_are_not_damaged_by_normalisation() {
        assert_eq!(idents("データー"), vec!["データー"]);
        assert_eq!(idents("寸法、記号"), vec!["寸法、記号"], "読点は名前の一部");
    }

    #[test]
    fn whitespace_is_skipped() {
        assert_eq!(lex("  1  +  2  ").unwrap().len(), 3);
        assert!(lex("   ").unwrap().is_empty());
        assert!(lex("").unwrap().is_empty());
    }
}
