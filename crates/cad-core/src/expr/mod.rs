//! パラメータの式。
//!
//! # なぜ式なのか
//!
//! AutoCAD のダイナミックブロックは、ストレッチ・配列・反転といった「アクション」を
//! **GUI で貼り付けて**作る。これは実質マクロ記録によるプログラミングで、
//!
//! - 作るのが難しく、デバッグがもっと難しい
//! - 読めない（何をするブロックか、開いて突いてみるまで分からない）
//! - **合成できない**（`幅 = 高さ × 2 + 10` が書けない）
//!
//! テキストの式なら、読めて・合成できて・`git diff` で意味が読めて・テストできる。
//! ノードベースのビジュアルエディタは現代的に見えるが、
//! **GUI でプログラミングさせる点でアクションと同じ罠**なので採らない
//! （`docs/DECISIONS.md` の ADR-0028）。
//!
//! # 妥当性はコマンド境界で保証する
//!
//! **`Document` に入っている式は常に妥当。** 字句解析・構文解析・型検査・
//! パラメータ間の循環検出は、すべて [`Command`](crate::Command) の `execute` で行う。
//! [`Definition`](crate::Definition) は**解析済みの [`Expr`] 木**を持ち、文字列は持たない。
//!
//! そのおかげで評価が「値の種類の取り違え」で失敗しなくなり、
//! 図形の解決が実行時エラーを持ち回らずに済む。
//! [`Xline::new`](crate::geom::Xline::new) が零ベクトルを弾くのと同じ流儀。
//!
//! # 角度は度で書く
//!
//! 式の中の角度は**度**。ラジアンが要るところは `rad()` で明示的に変換する。
//! 座標入力の `@100<45` と同じ約束で、`docs/PROGRESS.md` の「既知の落とし穴」に
//! 挙がっている π/180 のずれを、変換を明示させることで避ける。
//!
//! # 識別子に日本語を許す
//!
//! レイヤ名・グループ名・コンポーネント名が日本語なのと揃える。
//! `幅` や `扉の向き` がそのまま書ける。

pub mod eval;
pub mod lexer;
pub mod parser;

pub use eval::{eval, Env, EvalError};
pub use lexer::{lex, LexError, Token};
pub use parser::{parse, ParseError};

use std::fmt;

/// パラメータの型。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ParamType {
    /// 数値。
    Number,
    /// 真偽。
    Bool,
    /// 選択肢。候補は宣言順に持つ（パネルの表示順になる）。
    Choice(Vec<String>),
}

impl ParamType {
    /// 表示用の名前。
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::Number => "数値",
            Self::Bool => "真偽",
            Self::Choice(_) => "選択",
        }
    }

    /// この型が `value` を受け入れるか。
    #[must_use]
    pub fn accepts(&self, value: &Value) -> bool {
        match (self, value) {
            (Self::Number, Value::Number(_)) | (Self::Bool, Value::Bool(_)) => true,
            // 選択肢は候補に含まれていなければならない。
            (Self::Choice(options), Value::Choice(c)) => options.iter().any(|o| o == c),
            _ => false,
        }
    }
}

/// 式の評価結果。
///
/// 永続化される値でもある（インスタンスの個別上書き）。
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// 数値。
    Number(f64),
    /// 真偽。
    Bool(bool),
    /// 選択肢。
    Choice(String),
}

impl Value {
    /// 値の型の名前。エラーメッセージ用。
    #[must_use]
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Number(_) => "数値",
            Self::Bool(_) => "真偽",
            Self::Choice(_) => "選択",
        }
    }

    /// 数値として取り出す。
    #[must_use]
    pub fn as_number(&self) -> Option<f64> {
        match self {
            Self::Number(n) => Some(*n),
            _ => None,
        }
    }

    /// 真偽として取り出す。
    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Number(n) => write!(f, "{n}"),
            Self::Bool(true) => write!(f, "真"),
            Self::Bool(false) => write!(f, "偽"),
            Self::Choice(c) => write!(f, "'{c}'"),
        }
    }
}

/// 二項演算子。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    /// `+`
    Add,
    /// `-`
    Sub,
    /// `*`
    Mul,
    /// `/`
    Div,
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
    /// `==`
    Eq,
    /// `!=`
    Ne,
    /// `&&`
    And,
    /// `||`
    Or,
}

impl BinOp {
    /// 結合の強さ。大きいほど強く結びつく。
    #[must_use]
    pub fn precedence(self) -> u8 {
        match self {
            Self::Or => 1,
            Self::And => 2,
            Self::Eq | Self::Ne => 3,
            Self::Lt | Self::Le | Self::Gt | Self::Ge => 4,
            Self::Add | Self::Sub => 5,
            Self::Mul | Self::Div => 6,
        }
    }

    /// 表示用の記号。
    #[must_use]
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Sub => "-",
            Self::Mul => "*",
            Self::Div => "/",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
            Self::Eq => "==",
            Self::Ne => "!=",
            Self::And => "&&",
            Self::Or => "||",
        }
    }
}

/// 単項演算子。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    /// `-`（符号反転）
    Neg,
    /// `!`（否定）
    Not,
}

/// 組み込み関数。
///
/// **引数の個数は型で表す。** 実行時に個数を数えるより、
/// 構文解析の時点で確定するほうが誤りが早く出る。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Func1 {
    /// 正弦。引数は**度**。
    Sin,
    /// 余弦。引数は**度**。
    Cos,
    /// 正接。引数は**度**。
    Tan,
    /// 平方根。負の値はエラー。
    Sqrt,
    /// 絶対値。
    Abs,
    /// 切り捨て。
    Floor,
    /// 切り上げ。
    Ceil,
    /// 四捨五入。
    Round,
    /// ラジアン → 度。
    Deg,
    /// 度 → ラジアン。
    Rad,
}

/// 引数を 2 つ取る組み込み関数。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Func2 {
    /// 小さいほう。
    Min,
    /// 大きいほう。
    Max,
    /// `atan2(y, x)`。返り値は**度**。
    Atan2,
    /// 累乗。
    Pow,
}

/// 式の木。
///
/// **文字列ではなくこの木を永続化する。** 保存のたびに再解析しないで済み、
/// `Document` に妥当でない式が入り込む隙も無くなる。
#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    /// 定数。
    Literal(Value),
    /// パラメータの参照。
    Var(String),
    /// 単項演算。
    Unary(UnOp, Box<Expr>),
    /// 二項演算。
    Binary(BinOp, Box<Expr>, Box<Expr>),
    /// 条件式 `if c then a else b`。
    If {
        /// 条件。`Bool` でなければならない。
        cond: Box<Expr>,
        /// 真のときの値。
        then: Box<Expr>,
        /// 偽のときの値。
        otherwise: Box<Expr>,
    },
    /// 1 引数の組み込み関数。
    Call1(Func1, Box<Expr>),
    /// 2 引数の組み込み関数。
    Call2(Func2, Box<Expr>, Box<Expr>),
}

impl Expr {
    /// 数値の定数。
    #[must_use]
    pub fn number(v: f64) -> Self {
        Self::Literal(Value::Number(v))
    }

    /// この式が参照しているパラメータ名を集める（重複あり）。
    ///
    /// **循環検出に使う。** パラメータの既定値が互いを参照していると、
    /// 評価が無限再帰する。
    #[must_use]
    pub fn referenced_vars(&self) -> Vec<&str> {
        let mut out = Vec::new();
        self.collect_vars(&mut out);
        out
    }

    fn collect_vars<'a>(&'a self, out: &mut Vec<&'a str>) {
        match self {
            Self::Literal(_) => {}
            Self::Var(name) => out.push(name),
            Self::Unary(_, e) | Self::Call1(_, e) => e.collect_vars(out),
            Self::Binary(_, a, b) | Self::Call2(_, a, b) => {
                a.collect_vars(out);
                b.collect_vars(out);
            }
            Self::If {
                cond,
                then,
                otherwise,
            } => {
                cond.collect_vars(out);
                then.collect_vars(out);
                otherwise.collect_vars(out);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 式を文字列へ戻す
// ---------------------------------------------------------------------------

/// 式を**もう一度読める形**で書き出す。
///
/// # なぜ必要か
///
/// パネルで「この座標は何で動いているか」を見せるのに要る。
/// 木のまま持っている（[`crate::expr`] のモジュールドキュメント）ので、
/// 表示のたびに元の文字列へ戻す手段が要る。
///
/// **`parse(&expr.to_string())` が同じ木を返すこと**をテストで固定している。
/// 括弧の付け方を間違えると意味が変わるので、往復で確かめるのがいちばん確実。
impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.write(f, 0)
    }
}

impl Expr {
    /// `min_prec` より弱い演算子なら括弧を付けて書く。
    fn write(&self, f: &mut fmt::Formatter<'_>, min_prec: u8) -> fmt::Result {
        match self {
            Self::Literal(v) => write!(f, "{v}"),
            Self::Var(name) => f.write_str(name),
            Self::Unary(op, e) => {
                let symbol = match op {
                    UnOp::Neg => "-",
                    UnOp::Not => "!",
                };
                f.write_str(symbol)?;
                // 単項はどの二項より強いので、中身は最強の優先順位で書く。
                e.write(f, u8::MAX)
            }
            Self::Binary(op, a, b) => {
                let prec = op.precedence();
                let needs = prec < min_prec;
                if needs {
                    f.write_str("(")?;
                }
                a.write(f, prec)?;
                write!(f, " {} ", op.symbol())?;
                // **右辺は 1 段強い優先順位で書く。**
                // 左結合なので、`a - (b - c)` の括弧を落とすと意味が変わる。
                b.write(f, prec + 1)?;
                if needs {
                    f.write_str(")")?;
                }
                Ok(())
            }
            Self::If {
                cond,
                then,
                otherwise,
            } => {
                // `if` は最も弱いので、何かの引数になるときは常に括弧を付ける。
                let needs = min_prec > 0;
                if needs {
                    f.write_str("(")?;
                }
                f.write_str("if ")?;
                cond.write(f, 0)?;
                f.write_str(" then ")?;
                then.write(f, 0)?;
                f.write_str(" else ")?;
                otherwise.write(f, 0)?;
                if needs {
                    f.write_str(")")?;
                }
                Ok(())
            }
            Self::Call1(func, a) => {
                write!(f, "{}(", func1_name(*func))?;
                a.write(f, 0)?;
                f.write_str(")")
            }
            Self::Call2(func, a, b) => {
                write!(f, "{}(", func2_name(*func))?;
                a.write(f, 0)?;
                f.write_str(", ")?;
                b.write(f, 0)?;
                f.write_str(")")
            }
        }
    }
}

/// 1 引数の関数の名前。**構文解析が受け付ける綴りと一致させること。**
fn func1_name(f: Func1) -> &'static str {
    match f {
        Func1::Sin => "sin",
        Func1::Cos => "cos",
        Func1::Tan => "tan",
        Func1::Sqrt => "sqrt",
        Func1::Abs => "abs",
        Func1::Floor => "floor",
        Func1::Ceil => "ceil",
        Func1::Round => "round",
        Func1::Deg => "deg",
        Func1::Rad => "rad",
    }
}

/// 2 引数の関数の名前。
fn func2_name(f: Func2) -> &'static str {
    match f {
        Func2::Min => "min",
        Func2::Max => "max",
        Func2::Atan2 => "atan2",
        Func2::Pow => "pow",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn param_type_accepts_matching_values() {
        assert!(ParamType::Number.accepts(&Value::Number(1.0)));
        assert!(!ParamType::Number.accepts(&Value::Bool(true)));
        assert!(ParamType::Bool.accepts(&Value::Bool(false)));
        assert!(!ParamType::Bool.accepts(&Value::Number(0.0)));
    }

    /// **選択肢は候補に無い値を拒否すること。**
    ///
    /// 型が合っていても候補外なら不正。ここを緩めると、
    /// パネルに出せない値がファイルに入る。
    #[test]
    fn choice_rejects_values_outside_its_options() {
        let ty = ParamType::Choice(vec!["引違い".to_owned(), "開き".to_owned()]);
        assert!(ty.accepts(&Value::Choice("引違い".to_owned())));
        assert!(!ty.accepts(&Value::Choice("FIX".to_owned())));
        assert!(!ty.accepts(&Value::Number(1.0)));
    }

    #[test]
    fn precedence_orders_operators_conventionally() {
        assert!(BinOp::Mul.precedence() > BinOp::Add.precedence());
        assert!(BinOp::Add.precedence() > BinOp::Lt.precedence());
        assert!(BinOp::Lt.precedence() > BinOp::Eq.precedence());
        assert!(BinOp::Eq.precedence() > BinOp::And.precedence());
        assert!(BinOp::And.precedence() > BinOp::Or.precedence());
    }

    #[test]
    fn referenced_vars_walks_the_whole_tree() {
        // if 幅 > 0 then 高さ else min(奥行, 1)
        let e = Expr::If {
            cond: Box::new(Expr::Binary(
                BinOp::Gt,
                Box::new(Expr::Var("幅".to_owned())),
                Box::new(Expr::number(0.0)),
            )),
            then: Box::new(Expr::Var("高さ".to_owned())),
            otherwise: Box::new(Expr::Call2(
                Func2::Min,
                Box::new(Expr::Var("奥行".to_owned())),
                Box::new(Expr::number(1.0)),
            )),
        };
        let mut vars = e.referenced_vars();
        vars.sort_unstable();
        assert_eq!(vars, vec!["奥行", "幅", "高さ"]);
    }

    #[test]
    fn referenced_vars_is_empty_for_constants() {
        assert!(Expr::number(1.0).referenced_vars().is_empty());
    }

    // ---- 文字列へ戻す -----------------------------------------------------

    /// **書き出して読み直すと同じ木になること。**
    ///
    /// 括弧の付け方を間違えると意味が変わる。往復で確かめるのがいちばん確実。
    #[test]
    fn formatting_round_trips_through_the_parser() {
        let sources = [
            "1",
            "幅",
            "真",
            "'引違い'",
            "1 + 2 * 3",
            "(1 + 2) * 3",
            "10 - 3 - 2",
            "10 - (3 - 2)",
            "1 / (2 / 3)",
            "-幅",
            "-(幅 + 1)",
            "!開いている",
            "!(a && b)",
            "a && b || c",
            "a && (b || c)",
            "幅 > 500 && 高さ < 200",
            "if 幅 > 0 then 幅 / 2 else 0",
            "min(幅, 高さ) + 1",
            "max(幅 / 4, 10)",
            "sqrt(pow(a, 2) + pow(b, 2))",
            "atan2(1, 1) + deg(rad(30))",
            "if a then if b then 1 else 2 else 3",
            "1 + if a then 2 else 3",
        ];

        for src in sources {
            let original = parse(src).unwrap_or_else(|e| panic!("{src} の解析に失敗: {e}"));
            let text = original.to_string();
            let again =
                parse(&text).unwrap_or_else(|e| panic!("{src} → {text} を読み直せない: {e}"));
            assert_eq!(original, again, "{src} → {text} で木が変わった");
        }
    }

    /// **左結合を壊さないこと。**
    ///
    /// `10 - (3 - 2)` の括弧を落とすと 9 が 5 になる。
    #[test]
    fn right_operands_keep_their_parentheses() {
        let e = parse("10 - (3 - 2)").expect("解析");
        assert_eq!(e.to_string(), "10 - (3 - 2)");

        // 左側は括弧が要らない。
        let e = parse("(10 - 3) - 2").expect("解析");
        assert_eq!(e.to_string(), "10 - 3 - 2");
    }

    /// 要らない括弧は付けないこと（読みやすさ）。
    #[test]
    fn unnecessary_parentheses_are_dropped() {
        assert_eq!(parse("(1) + (2)").expect("解析").to_string(), "1 + 2");
        assert_eq!(parse("1 + (2 * 3)").expect("解析").to_string(), "1 + 2 * 3");
        assert_eq!(parse("((幅))").expect("解析").to_string(), "幅");
    }

    /// 弱い演算子が強い演算子の中に来たら括弧を付けること。
    #[test]
    fn weaker_operators_are_parenthesised_inside_stronger_ones() {
        assert_eq!(
            parse("(1 + 2) * 3").expect("解析").to_string(),
            "(1 + 2) * 3"
        );
        assert_eq!(
            parse("(a || b) && c").expect("解析").to_string(),
            "(a || b) && c"
        );
    }

    /// `if` は引数になるとき括弧が要ること。
    #[test]
    fn conditionals_are_parenthesised_where_needed() {
        let e = parse("1 + if a then 2 else 3").expect("解析");
        assert!(e.to_string().contains('('), "括弧が付く: {e}");
        assert_eq!(parse(&e.to_string()).expect("読み直せる"), e);

        // いちばん外側なら括弧は要らない。
        let e = parse("if a then 1 else 2").expect("解析");
        assert_eq!(e.to_string(), "if a then 1 else 2");
    }

    /// 単項は中身に括弧が要ること。
    #[test]
    fn unary_operands_are_parenthesised() {
        let e = parse("-(1 + 2)").expect("解析");
        assert_eq!(e.to_string(), "-(1 + 2)");
        assert_eq!(parse(&e.to_string()).expect("読み直せる"), e);
    }

    /// **書き出した関数名を構文解析が受け付けること。**
    #[test]
    fn every_function_name_round_trips() {
        let ones = [
            "sin", "cos", "tan", "sqrt", "abs", "floor", "ceil", "round", "deg", "rad",
        ];
        for name in ones {
            let src = format!("{name}(1)");
            let e = parse(&src).unwrap_or_else(|err| panic!("{src}: {err}"));
            assert_eq!(e.to_string(), src);
        }
        for name in ["min", "max", "atan2", "pow"] {
            let src = format!("{name}(1, 2)");
            let e = parse(&src).unwrap_or_else(|err| panic!("{src}: {err}"));
            assert_eq!(e.to_string(), src);
        }
    }

    #[test]
    fn value_display_is_readable() {
        assert_eq!(Value::Number(1.5).to_string(), "1.5");
        assert_eq!(Value::Bool(true).to_string(), "真");
        assert_eq!(Value::Choice("開き".to_owned()).to_string(), "'開き'");
    }
}
