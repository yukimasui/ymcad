//! 式の構文解析。
//!
//! 優先順位登攀法（precedence climbing）。演算子の優先順位は
//! [`BinOp::precedence`] が持つので、ここには階層ごとの関数を書かない。
//!
//! # 文法
//!
//! ```text
//! expr    := if | binary
//! if      := "if" expr "then" expr "else" expr
//! binary  := unary (op unary)*
//! unary   := ("-" | "!") unary | primary
//! primary := number | string | "真" | "偽" | ident | call | "(" expr ")"
//! call    := ident "(" expr ("," expr)* ")"
//! ```
//!
//! **関数の引数の個数は構文解析で確定する。** 実行時に数えるより誤りが早く出る。

use std::fmt;

use super::lexer::{lex, Token};
use super::{BinOp, Expr, Func1, Func2, UnOp, Value};

/// 構文解析の失敗。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    /// 人間向けの説明。
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<super::LexError> for ParseError {
    fn from(e: super::LexError) -> Self {
        Self {
            message: e.to_string(),
        }
    }
}

/// 真を表すキーワード。日本語と英語の両方を受ける。
const TRUE_WORDS: &[&str] = &["真", "true"];
/// 偽を表すキーワード。
const FALSE_WORDS: &[&str] = &["偽", "false"];

/// 式の文字列を [`Expr`] へ解析する。
///
/// # Errors
///
/// 字句解析に失敗した場合、構文として成立しない場合 [`ParseError`]。
pub fn parse(input: &str) -> Result<Expr, ParseError> {
    let tokens = lex(input)?;
    if tokens.is_empty() {
        return Err(error("式が空です"));
    }
    let mut p = Parser { tokens, pos: 0 };
    let e = p.expr(0)?;
    if p.pos < p.tokens.len() {
        return Err(error(format!(
            "式の後ろに余分なものがあります: {}",
            p.tokens[p.pos]
        )));
    }
    Ok(e)
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn next(&mut self) -> Option<Token> {
        let t = self.tokens.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    /// 次が指定の字句なら読み進める。
    fn eat(&mut self, want: &Token) -> bool {
        if self.peek() == Some(want) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    /// 次が指定のキーワードなら読み進める。
    fn eat_keyword(&mut self, word: &str) -> bool {
        if matches!(self.peek(), Some(Token::Ident(s)) if s == word) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, want: &Token) -> Result<(), ParseError> {
        if self.eat(want) {
            Ok(())
        } else {
            Err(error(format!(
                "{want} が必要です（見つかったのは {}）",
                self.describe_current()
            )))
        }
    }

    fn describe_current(&self) -> String {
        self.peek()
            .map_or_else(|| "式の終わり".to_owned(), ToString::to_string)
    }

    /// `min_prec` 以上の優先順位の演算子だけを取り込む。
    fn expr(&mut self, min_prec: u8) -> Result<Expr, ParseError> {
        if self.eat_keyword("if") {
            return self.if_expr();
        }

        let mut left = self.unary()?;

        while let Some(op) = self.peek().and_then(binop_of) {
            let prec = op.precedence();
            if prec < min_prec {
                break;
            }
            self.pos += 1;
            // 左結合なので、右辺は 1 段強い優先順位で読む。
            let right = self.expr(prec + 1)?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }

        Ok(left)
    }

    fn if_expr(&mut self) -> Result<Expr, ParseError> {
        let cond = self.expr(0)?;
        if !self.eat_keyword("then") {
            return Err(error(format!(
                "if には then が必要です（見つかったのは {}）",
                self.describe_current()
            )));
        }
        let then = self.expr(0)?;
        if !self.eat_keyword("else") {
            return Err(error(format!(
                "if には else が必要です（見つかったのは {}）",
                self.describe_current()
            )));
        }
        let otherwise = self.expr(0)?;
        Ok(Expr::If {
            cond: Box::new(cond),
            then: Box::new(then),
            otherwise: Box::new(otherwise),
        })
    }

    fn unary(&mut self) -> Result<Expr, ParseError> {
        if self.eat(&Token::Minus) {
            return Ok(Expr::Unary(UnOp::Neg, Box::new(self.unary()?)));
        }
        if self.eat(&Token::Bang) {
            return Ok(Expr::Unary(UnOp::Not, Box::new(self.unary()?)));
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<Expr, ParseError> {
        let Some(token) = self.next() else {
            return Err(error("式が途中で終わっています"));
        };

        match token {
            Token::Number(n) => Ok(Expr::number(n)),
            Token::Str(s) => Ok(Expr::Literal(Value::Choice(s))),
            Token::LParen => {
                let e = self.expr(0)?;
                self.expect(&Token::RParen)?;
                Ok(e)
            }
            Token::Ident(name) => self.ident_or_call(name),
            other => Err(error(format!("式の先頭に置けません: {other}"))),
        }
    }

    fn ident_or_call(&mut self, name: String) -> Result<Expr, ParseError> {
        // キーワードは変数にならない。
        if TRUE_WORDS.contains(&name.as_str()) {
            return Ok(Expr::Literal(Value::Bool(true)));
        }
        if FALSE_WORDS.contains(&name.as_str()) {
            return Ok(Expr::Literal(Value::Bool(false)));
        }
        if matches!(name.as_str(), "then" | "else") {
            return Err(error(format!("{name} だけでは式になりません")));
        }

        // 関数呼び出しでなければ変数。
        if !self.eat(&Token::LParen) {
            if func1_of(&name).is_some() || func2_of(&name).is_some() {
                return Err(error(format!("{name} は関数です。{name}(…) と書きます")));
            }
            return Ok(Expr::Var(name));
        }

        // ---- 関数呼び出し ----
        let mut args = Vec::new();
        if !self.eat(&Token::RParen) {
            loop {
                args.push(self.expr(0)?);
                if self.eat(&Token::Comma) {
                    continue;
                }
                self.expect(&Token::RParen)?;
                break;
            }
        }

        if let Some(f) = func1_of(&name) {
            let [a] = take_args(&name, args, 1)?
                .try_into()
                .map_err(|_| error(format!("内部エラー: {name} の引数の個数が合いません")))?;
            return Ok(Expr::Call1(f, Box::new(a)));
        }
        if let Some(f) = func2_of(&name) {
            let [a, b] = take_args(&name, args, 2)?
                .try_into()
                .map_err(|_| error(format!("内部エラー: {name} の引数の個数が合いません")))?;
            return Ok(Expr::Call2(f, Box::new(a), Box::new(b)));
        }

        Err(error(format!(
            "そんな関数はありません: {name}（使えるのは {}）",
            known_function_names()
        )))
    }
}

/// 引数の個数を検査する。
fn take_args(name: &str, args: Vec<Expr>, want: usize) -> Result<Vec<Expr>, ParseError> {
    if args.len() == want {
        Ok(args)
    } else {
        Err(error(format!(
            "{name} は引数が {want} 個です（{} 個ありました）",
            args.len()
        )))
    }
}

fn known_function_names() -> String {
    let mut names: Vec<&str> = FUNC1_NAMES
        .iter()
        .map(|(n, _)| *n)
        .chain(FUNC2_NAMES.iter().map(|(n, _)| *n))
        .collect();
    names.sort_unstable();
    names.join(" / ")
}

/// 1 引数の組み込み関数の名前。
const FUNC1_NAMES: &[(&str, Func1)] = &[
    ("sin", Func1::Sin),
    ("cos", Func1::Cos),
    ("tan", Func1::Tan),
    ("sqrt", Func1::Sqrt),
    ("abs", Func1::Abs),
    ("floor", Func1::Floor),
    ("ceil", Func1::Ceil),
    ("round", Func1::Round),
    ("deg", Func1::Deg),
    ("rad", Func1::Rad),
];

/// 2 引数の組み込み関数の名前。
const FUNC2_NAMES: &[(&str, Func2)] = &[
    ("min", Func2::Min),
    ("max", Func2::Max),
    ("atan2", Func2::Atan2),
    ("pow", Func2::Pow),
];

fn func1_of(name: &str) -> Option<Func1> {
    FUNC1_NAMES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, f)| *f)
}

fn func2_of(name: &str) -> Option<Func2> {
    FUNC2_NAMES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, f)| *f)
}

fn binop_of(token: &Token) -> Option<BinOp> {
    Some(match token {
        Token::Plus => BinOp::Add,
        Token::Minus => BinOp::Sub,
        Token::Star => BinOp::Mul,
        Token::Slash => BinOp::Div,
        Token::Lt => BinOp::Lt,
        Token::Le => BinOp::Le,
        Token::Gt => BinOp::Gt,
        Token::Ge => BinOp::Ge,
        Token::EqEq => BinOp::Eq,
        Token::Ne => BinOp::Ne,
        Token::AndAnd => BinOp::And,
        Token::OrOr => BinOp::Or,
        _ => return None,
    })
}

fn error(message: impl Into<String>) -> ParseError {
    ParseError {
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Expr {
        parse(s).unwrap_or_else(|e| panic!("{s} の解析に失敗: {e}"))
    }

    fn v(name: &str) -> Expr {
        Expr::Var(name.to_owned())
    }

    fn bin(op: BinOp, a: Expr, b: Expr) -> Expr {
        Expr::Binary(op, Box::new(a), Box::new(b))
    }

    #[test]
    fn parses_a_number() {
        assert_eq!(p("42"), Expr::number(42.0));
    }

    #[test]
    fn parses_a_variable() {
        assert_eq!(p("幅"), v("幅"));
    }

    /// **乗除が加減より強く結びつくこと。**
    #[test]
    fn multiplication_binds_tighter_than_addition() {
        assert_eq!(
            p("1 + 2 * 3"),
            bin(
                BinOp::Add,
                Expr::number(1.0),
                bin(BinOp::Mul, Expr::number(2.0), Expr::number(3.0))
            )
        );
    }

    /// **同じ優先順位は左結合であること。**
    ///
    /// 右結合になると `10 - 3 - 2` が 9 になってしまう（正しくは 5）。
    #[test]
    fn same_precedence_is_left_associative() {
        assert_eq!(
            p("10 - 3 - 2"),
            bin(
                BinOp::Sub,
                bin(BinOp::Sub, Expr::number(10.0), Expr::number(3.0)),
                Expr::number(2.0)
            )
        );
    }

    #[test]
    fn parentheses_override_precedence() {
        assert_eq!(
            p("(1 + 2) * 3"),
            bin(
                BinOp::Mul,
                bin(BinOp::Add, Expr::number(1.0), Expr::number(2.0)),
                Expr::number(3.0)
            )
        );
    }

    /// 比較・論理の優先順位（`a < b && c < d` が意図どおり括られること）。
    #[test]
    fn comparison_binds_tighter_than_logic() {
        assert_eq!(
            p("1 < 2 && 3 < 4"),
            bin(
                BinOp::And,
                bin(BinOp::Lt, Expr::number(1.0), Expr::number(2.0)),
                bin(BinOp::Lt, Expr::number(3.0), Expr::number(4.0))
            )
        );
    }

    #[test]
    fn parses_unary_operators() {
        assert_eq!(p("-幅"), Expr::Unary(UnOp::Neg, Box::new(v("幅"))));
        assert_eq!(
            p("!開いている"),
            Expr::Unary(UnOp::Not, Box::new(v("開いている")))
        );
        // 単項は繰り返せる。
        assert_eq!(
            p("--1"),
            Expr::Unary(
                UnOp::Neg,
                Box::new(Expr::Unary(UnOp::Neg, Box::new(Expr::number(1.0))))
            )
        );
    }

    /// **単項マイナスは乗算より強いこと。** `-2 * 3` は `(-2) * 3`。
    #[test]
    fn unary_minus_binds_tighter_than_multiplication() {
        assert_eq!(
            p("-2 * 3"),
            bin(
                BinOp::Mul,
                Expr::Unary(UnOp::Neg, Box::new(Expr::number(2.0))),
                Expr::number(3.0)
            )
        );
    }

    #[test]
    fn parses_booleans_in_japanese_and_english() {
        assert_eq!(p("真"), Expr::Literal(Value::Bool(true)));
        assert_eq!(p("偽"), Expr::Literal(Value::Bool(false)));
        assert_eq!(p("true"), Expr::Literal(Value::Bool(true)));
        assert_eq!(p("false"), Expr::Literal(Value::Bool(false)));
    }

    #[test]
    fn parses_choice_literals() {
        assert_eq!(
            p("'引違い'"),
            Expr::Literal(Value::Choice("引違い".to_owned()))
        );
    }

    #[test]
    fn parses_if_expressions() {
        assert_eq!(
            p("if 幅 > 0 then 1 else 2"),
            Expr::If {
                cond: Box::new(bin(BinOp::Gt, v("幅"), Expr::number(0.0))),
                then: Box::new(Expr::number(1.0)),
                otherwise: Box::new(Expr::number(2.0)),
            }
        );
    }

    #[test]
    fn parses_nested_if() {
        let e = p("if a then if b then 1 else 2 else 3");
        let Expr::If { then, .. } = e else {
            panic!("if のはず")
        };
        assert!(matches!(*then, Expr::If { .. }), "then 側が入れ子の if");
    }

    #[test]
    fn parses_function_calls() {
        assert_eq!(
            p("sqrt(4)"),
            Expr::Call1(Func1::Sqrt, Box::new(Expr::number(4.0)))
        );
        assert_eq!(
            p("min(1, 2)"),
            Expr::Call2(
                Func2::Min,
                Box::new(Expr::number(1.0)),
                Box::new(Expr::number(2.0))
            )
        );
        // 引数の中でも式が使える。
        assert_eq!(
            p("max(幅 * 2, 10)"),
            Expr::Call2(
                Func2::Max,
                Box::new(bin(BinOp::Mul, v("幅"), Expr::number(2.0))),
                Box::new(Expr::number(10.0))
            )
        );
    }

    // ---- 誤りの案内 -------------------------------------------------------

    #[test]
    fn wrong_argument_count_is_reported() {
        let e = parse("sqrt(1, 2)").unwrap_err();
        assert!(e.message.contains("引数が 1 個"), "{e}");
        let e = parse("min(1)").unwrap_err();
        assert!(e.message.contains("引数が 2 個"), "{e}");
    }

    #[test]
    fn unknown_functions_list_the_known_ones() {
        let e = parse("hypot(1, 2)").unwrap_err();
        assert!(e.message.contains("そんな関数はありません"), "{e}");
        assert!(e.message.contains("sqrt"), "使える関数を案内する: {e}");
    }

    /// 関数名を括弧なしで書いたら案内すること。
    #[test]
    fn a_bare_function_name_is_guided() {
        let e = parse("sqrt").unwrap_err();
        assert!(e.message.contains("sqrt(…)"), "{e}");
    }

    #[test]
    fn unbalanced_parentheses_are_reported() {
        assert!(parse("(1 + 2").is_err());
        assert!(parse("1 + 2)").is_err());
    }

    #[test]
    fn if_without_then_or_else_is_reported() {
        assert!(parse("if a then 1").unwrap_err().message.contains("else"));
        assert!(parse("if a 1 else 2").unwrap_err().message.contains("then"));
    }

    #[test]
    fn empty_input_is_an_error() {
        assert!(parse("").is_err());
        assert!(parse("   ").is_err());
    }

    #[test]
    fn trailing_tokens_are_reported() {
        let e = parse("1 2").unwrap_err();
        assert!(e.message.contains("余分"), "{e}");
    }

    #[test]
    fn a_dangling_operator_is_reported() {
        assert!(parse("1 +").is_err());
        assert!(parse("* 2").is_err());
    }

    /// 字句解析の誤りも構文解析の誤りとして返ること（位置つき）。
    #[test]
    fn lex_errors_are_propagated() {
        let e = parse("1 @ 2").unwrap_err();
        assert!(e.message.contains("使えない文字"), "{e}");
    }
}
