//! 式の評価。
//!
//! # 失敗しうるのは型と定義域だけ
//!
//! 構文の誤りは [`parse`](super::parse) で除かれているので、ここで起きうるのは
//!
//! - 型の取り違え（`真 + 1`）
//! - 未定義のパラメータ参照
//! - 定義域の外（`sqrt(-1)`、0 除算）
//!
//! の 3 つだけ。**すべてコマンドの `execute` で先に潰す**ので、
//! `Document` に入った式の評価はこれらを返さない
//! （それでも `Result` を返すのは、コマンド側が検査に使うため）。
//!
//! # `NaN` と無限大を作らせない
//!
//! 0 除算や `sqrt(-1)` を黙って `NaN` にすると、その座標が図形へ流れて
//! **画面から図形が消え、原因が分からなくなる**。エラーとして止める。
//!
//! # 角度は度
//!
//! `sin` / `cos` / `tan` の引数と `atan2` の返り値は**度**。
//! ラジアンが要るところは `rad()` で明示的に変換する（モジュールドキュメント参照）。

use std::collections::BTreeMap;
use std::fmt;

use super::{BinOp, Expr, Func1, Func2, UnOp, Value};
use crate::geom::tolerance::is_zero_len;

/// パラメータ名 → 値の対応。
pub type Env = BTreeMap<String, Value>;

/// 評価の失敗。
#[derive(Clone, Debug, PartialEq)]
pub enum EvalError {
    /// 未定義のパラメータを参照した。
    UnknownVar(String),
    /// 型が合わない。
    TypeMismatch {
        /// どこで起きたか（演算子名など）。
        context: String,
        /// 期待した型。
        expected: &'static str,
        /// 実際の型。
        found: &'static str,
    },
    /// 定義域の外（0 除算、負の平方根など）。
    OutOfDomain(String),
}

impl fmt::Display for EvalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownVar(name) => write!(f, "パラメータ「{name}」がありません"),
            Self::TypeMismatch {
                context,
                expected,
                found,
            } => write!(f, "{context} には{expected}が必要です（{found}が来ました）"),
            Self::OutOfDomain(msg) => write!(f, "{msg}"),
        }
    }
}

/// 式を評価する。
///
/// # Errors
///
/// 未定義のパラメータ参照、型の不一致、定義域の外の場合 [`EvalError`]。
pub fn eval(expr: &Expr, env: &Env) -> Result<Value, EvalError> {
    match expr {
        Expr::Literal(v) => Ok(v.clone()),
        Expr::Var(name) => env
            .get(name)
            .cloned()
            .ok_or_else(|| EvalError::UnknownVar(name.clone())),
        Expr::Unary(op, e) => eval_unary(*op, &eval(e, env)?),
        Expr::Binary(op, a, b) => eval_binary(*op, &eval(a, env)?, &eval(b, env)?),
        Expr::If {
            cond,
            then,
            otherwise,
        } => {
            let c = need_bool(&eval(cond, env)?, "if の条件")?;
            // **選ばれなかった側は評価しない。** `if 幅 > 0 then 1/幅 else 0` が
            // 幅 = 0 でエラーにならないようにするため。
            if c {
                eval(then, env)
            } else {
                eval(otherwise, env)
            }
        }
        Expr::Call1(f, a) => eval_call1(*f, need_number(&eval(a, env)?, name1(*f))?),
        Expr::Call2(f, a, b) => eval_call2(
            *f,
            need_number(&eval(a, env)?, name2(*f))?,
            need_number(&eval(b, env)?, name2(*f))?,
        ),
    }
}

fn eval_unary(op: UnOp, v: &Value) -> Result<Value, EvalError> {
    match op {
        UnOp::Neg => Ok(Value::Number(-need_number(v, "符号反転 -")?)),
        UnOp::Not => Ok(Value::Bool(!need_bool(v, "否定 !")?)),
    }
}

fn eval_binary(op: BinOp, a: &Value, b: &Value) -> Result<Value, EvalError> {
    match op {
        // ---- 算術 ----
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div => {
            let (x, y) = (need_number(a, op.symbol())?, need_number(b, op.symbol())?);
            let r = match op {
                BinOp::Add => x + y,
                BinOp::Sub => x - y,
                BinOp::Mul => x * y,
                BinOp::Div => {
                    // **0 除算を黙って無限大にしない。**
                    // トレランスは `geom/tolerance.rs` に一元管理（数値を直書きしない）。
                    if is_zero_len(y) {
                        return Err(EvalError::OutOfDomain("0 で割れません".to_owned()));
                    }
                    x / y
                }
                _ => unreachable!("算術の分岐は上の 4 つだけ"),
            };
            finite(r, op.symbol())
        }

        // ---- 大小比較（数値のみ）----
        BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
            let (x, y) = (need_number(a, op.symbol())?, need_number(b, op.symbol())?);
            Ok(Value::Bool(match op {
                BinOp::Lt => x < y,
                BinOp::Le => x <= y,
                BinOp::Gt => x > y,
                BinOp::Ge => x >= y,
                _ => unreachable!("比較の分岐は上の 4 つだけ"),
            }))
        }

        // ---- 等値（同じ型どうしのみ）----
        //
        // 型が違う場合を `false` にすると、書き間違いが静かに通ってしまう。
        BinOp::Eq | BinOp::Ne => {
            if std::mem::discriminant(a) != std::mem::discriminant(b) {
                return Err(EvalError::TypeMismatch {
                    context: format!("{} の両辺", op.symbol()),
                    expected: "同じ種類の値",
                    found: b.type_name(),
                });
            }
            let same = a == b;
            Ok(Value::Bool(if op == BinOp::Eq { same } else { !same }))
        }

        // ---- 論理 ----
        BinOp::And => Ok(Value::Bool(need_bool(a, "&&")? && need_bool(b, "&&")?)),
        BinOp::Or => Ok(Value::Bool(need_bool(a, "||")? || need_bool(b, "||")?)),
    }
}

fn eval_call1(f: Func1, x: f64) -> Result<Value, EvalError> {
    let name = name1(f);
    let r = match f {
        // 三角関数の引数は度。
        Func1::Sin => x.to_radians().sin(),
        Func1::Cos => x.to_radians().cos(),
        Func1::Tan => {
            // **`is_finite` では捕まらない。**
            // 90 度をラジアンにしても厳密に π/2 にはならないので、
            // `tan(90)` は無限大ではなく 1.6e16 という巨大な有限値になる。
            // それが座標へ流れると図形が事実上消える。
            // tan が未定義なのは cos が 0 のところなので、そちらで判定する。
            let r = x.to_radians();
            if is_zero_len(r.cos()) {
                return Err(EvalError::OutOfDomain(format!(
                    "tan({x}) は定義できません（90 度の奇数倍）"
                )));
            }
            r.tan()
        }
        Func1::Sqrt => {
            if x < 0.0 {
                return Err(EvalError::OutOfDomain(format!(
                    "sqrt に負の値は渡せません: {x}"
                )));
            }
            x.sqrt()
        }
        Func1::Abs => x.abs(),
        Func1::Floor => x.floor(),
        Func1::Ceil => x.ceil(),
        Func1::Round => x.round(),
        Func1::Deg => x.to_degrees(),
        Func1::Rad => x.to_radians(),
    };
    finite(r, name)
}

fn eval_call2(f: Func2, x: f64, y: f64) -> Result<Value, EvalError> {
    let name = name2(f);
    let r = match f {
        Func2::Min => x.min(y),
        Func2::Max => x.max(y),
        // 返り値は度。
        Func2::Atan2 => {
            if is_zero_len(x) && is_zero_len(y) {
                return Err(EvalError::OutOfDomain(
                    "atan2(0, 0) は定義できません".to_owned(),
                ));
            }
            x.atan2(y).to_degrees()
        }
        Func2::Pow => x.powf(y),
    };
    finite(r, name)
}

/// 結果が有限でなければエラーにする。
///
/// **`NaN` や無限大を図形へ流さない。** 流すと座標が壊れて図形が画面から消え、
/// 原因が分からなくなる。
fn finite(v: f64, context: &str) -> Result<Value, EvalError> {
    if v.is_finite() {
        Ok(Value::Number(v))
    } else {
        Err(EvalError::OutOfDomain(format!(
            "{context} の結果が数値になりません"
        )))
    }
}

fn need_number(v: &Value, context: &str) -> Result<f64, EvalError> {
    v.as_number().ok_or_else(|| EvalError::TypeMismatch {
        context: context.to_owned(),
        expected: "数値",
        found: v.type_name(),
    })
}

fn need_bool(v: &Value, context: &str) -> Result<bool, EvalError> {
    v.as_bool().ok_or_else(|| EvalError::TypeMismatch {
        context: context.to_owned(),
        expected: "真偽",
        found: v.type_name(),
    })
}

fn name1(f: Func1) -> &'static str {
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

fn name2(f: Func2) -> &'static str {
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
    use crate::expr::parse;
    use crate::geom::tolerance::eq_len;

    fn env(pairs: &[(&str, Value)]) -> Env {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect()
    }

    fn num(src: &str, e: &Env) -> f64 {
        match eval(&parse(src).expect("解析できるはず"), e) {
            Ok(Value::Number(n)) => n,
            other => panic!("{src} が数値にならない: {other:?}"),
        }
    }

    fn boolean(src: &str, e: &Env) -> bool {
        match eval(&parse(src).expect("解析できるはず"), e) {
            Ok(Value::Bool(b)) => b,
            other => panic!("{src} が真偽にならない: {other:?}"),
        }
    }

    fn fails(src: &str, e: &Env) -> EvalError {
        eval(&parse(src).expect("解析できるはず"), e).expect_err("失敗するはず")
    }

    // ---- 算術 -------------------------------------------------------------

    #[test]
    fn evaluates_arithmetic_with_precedence() {
        let e = Env::new();
        assert!(eq_len(num("1 + 2 * 3", &e), 7.0));
        assert!(eq_len(num("(1 + 2) * 3", &e), 9.0));
        assert!(eq_len(num("10 - 3 - 2", &e), 5.0), "左結合");
        assert!(eq_len(num("-2 * 3", &e), -6.0));
    }

    #[test]
    fn evaluates_variables() {
        let e = env(&[
            ("幅", Value::Number(900.0)),
            ("高さ", Value::Number(2000.0)),
        ]);
        assert!(eq_len(num("幅", &e), 900.0));
        assert!(eq_len(num("幅 / 2", &e), 450.0));
        assert!(eq_len(num("高さ * 2 + 10", &e), 4010.0), "式が合成できる");
    }

    /// **これがダイナミックブロックの「アクション」で書けなかったもの。**
    #[test]
    fn composes_expressions_over_several_parameters() {
        let e = env(&[("幅", Value::Number(100.0)), ("高さ", Value::Number(50.0))]);
        assert!(eq_len(num("高さ * 2 + 10", &e), 110.0));
        assert!(eq_len(num("min(幅, 高さ) / 2", &e), 25.0));
        assert!(eq_len(
            num("if 幅 > 高さ then 幅 - 高さ else 高さ - 幅", &e),
            50.0
        ));
    }

    // ---- 比較・論理 -------------------------------------------------------

    #[test]
    fn evaluates_comparisons_and_logic() {
        let e = env(&[("幅", Value::Number(900.0))]);
        assert!(boolean("幅 > 500", &e));
        assert!(!boolean("幅 < 500", &e));
        assert!(boolean("幅 >= 900 && 幅 <= 900", &e));
        assert!(boolean("幅 < 0 || 幅 > 0", &e));
        assert!(boolean("!(幅 < 0)", &e));
    }

    #[test]
    fn equality_works_for_each_type() {
        let e = env(&[
            ("種別", Value::Choice("引違い".to_owned())),
            ("開く", Value::Bool(true)),
        ]);
        assert!(boolean("種別 == '引違い'", &e));
        assert!(boolean("種別 != '開き'", &e));
        assert!(boolean("開く == 真", &e));
        assert!(boolean("1 == 1", &e));
    }

    /// **型の違う等値比較は `false` ではなくエラーにすること。**
    ///
    /// `false` にすると書き間違いが静かに通ってしまう。
    #[test]
    fn comparing_different_types_is_an_error_not_false() {
        let e = env(&[("開く", Value::Bool(true))]);
        assert!(matches!(
            fails("開く == 1", &e),
            EvalError::TypeMismatch { .. }
        ));
    }

    // ---- 条件式 -----------------------------------------------------------

    #[test]
    fn evaluates_if_expressions() {
        let e = env(&[("幅", Value::Number(900.0))]);
        assert!(eq_len(num("if 幅 > 500 then 1 else 2", &e), 1.0));
        assert!(eq_len(num("if 幅 > 5000 then 1 else 2", &e), 2.0));
    }

    /// **選ばれなかった側は評価しないこと。**
    ///
    /// `if 幅 > 0 then 1/幅 else 0` が 幅 = 0 でエラーにならないために必要。
    #[test]
    fn the_unselected_branch_is_not_evaluated() {
        let e = env(&[("幅", Value::Number(0.0))]);
        assert!(eq_len(num("if 幅 > 0 then 1 / 幅 else 0", &e), 0.0));
    }

    #[test]
    fn if_condition_must_be_boolean() {
        let e = env(&[("幅", Value::Number(1.0))]);
        assert!(matches!(
            fails("if 幅 then 1 else 2", &e),
            EvalError::TypeMismatch { .. }
        ));
    }

    // ---- 関数 -------------------------------------------------------------

    #[test]
    fn evaluates_builtin_functions() {
        let e = Env::new();
        assert!(eq_len(num("sqrt(9)", &e), 3.0));
        assert!(eq_len(num("abs(-3)", &e), 3.0));
        assert!(eq_len(num("min(1, 2)", &e), 1.0));
        assert!(eq_len(num("max(1, 2)", &e), 2.0));
        assert!(eq_len(num("floor(1.7)", &e), 1.0));
        assert!(eq_len(num("ceil(1.2)", &e), 2.0));
        assert!(eq_len(num("round(1.5)", &e), 2.0));
        assert!(eq_len(num("pow(2, 10)", &e), 1024.0));
    }

    /// **三角関数の引数は度。** ラジアンで書くと値が変わるので固定する。
    #[test]
    fn trigonometric_functions_take_degrees() {
        let e = Env::new();
        assert!(eq_len(num("sin(90)", &e), 1.0), "sin(90 度) = 1");
        assert!(eq_len(num("cos(0)", &e), 1.0));
        assert!(eq_len(num("tan(45)", &e), 1.0));
        assert!(eq_len(num("atan2(1, 1)", &e), 45.0), "返り値も度");
    }

    #[test]
    fn degree_and_radian_conversion_round_trips() {
        let e = Env::new();
        assert!(eq_len(num("deg(rad(30))", &e), 30.0));
        assert!(eq_len(num("rad(180)", &e), std::f64::consts::PI));
    }

    // ---- 定義域と NaN -----------------------------------------------------

    /// **0 除算を黙って無限大にしないこと。**
    #[test]
    fn division_by_zero_is_an_error() {
        let e = Env::new();
        assert!(matches!(fails("1 / 0", &e), EvalError::OutOfDomain(_)));
        // トレランス内のゼロも同じ扱い。
        assert!(matches!(fails("1 / 0.0", &e), EvalError::OutOfDomain(_)));
    }

    #[test]
    fn negative_square_root_is_an_error() {
        let e = Env::new();
        let err = fails("sqrt(-1)", &e);
        assert!(matches!(err, EvalError::OutOfDomain(_)), "{err}");
    }

    /// **`tan(90)` を止めること。**
    ///
    /// 90 度をラジアンにしても厳密に π/2 にならないので、結果は無限大ではなく
    /// 1.6e16 という巨大な有限値になる。`is_finite` では捕まらない。
    /// これが座標へ流れると図形が事実上消える。
    #[test]
    fn tangent_at_ninety_degrees_is_an_error() {
        let e = Env::new();
        assert!(matches!(fails("tan(90)", &e), EvalError::OutOfDomain(_)));
        assert!(matches!(fails("tan(270)", &e), EvalError::OutOfDomain(_)));
        assert!(matches!(fails("tan(-90)", &e), EvalError::OutOfDomain(_)));
        // 近いだけなら通る（極端に大きくはなるが有限）。
        assert!(num("tan(89.9)", &e) > 500.0);
    }

    #[test]
    fn atan2_of_the_origin_is_an_error() {
        let e = Env::new();
        assert!(matches!(
            fails("atan2(0, 0)", &e),
            EvalError::OutOfDomain(_)
        ));
    }

    /// **無限大になる計算を止めること。** 座標へ流すと図形が消える。
    #[test]
    fn overflowing_results_are_rejected() {
        let e = Env::new();
        assert!(matches!(
            fails("pow(10, 400)", &e),
            EvalError::OutOfDomain(_)
        ));
    }

    // ---- 型と未定義 -------------------------------------------------------

    #[test]
    fn unknown_variables_are_reported_by_name() {
        let e = Env::new();
        match fails("ない名前 + 1", &e) {
            EvalError::UnknownVar(name) => assert_eq!(name, "ない名前"),
            other => panic!("未定義エラーのはず: {other:?}"),
        }
    }

    #[test]
    fn arithmetic_on_booleans_is_a_type_error() {
        let e = env(&[("開く", Value::Bool(true))]);
        assert!(matches!(
            fails("開く + 1", &e),
            EvalError::TypeMismatch { .. }
        ));
    }

    #[test]
    fn logic_on_numbers_is_a_type_error() {
        let e = env(&[("幅", Value::Number(1.0))]);
        assert!(matches!(
            fails("幅 && 真", &e),
            EvalError::TypeMismatch { .. }
        ));
    }

    #[test]
    fn comparing_choices_with_less_than_is_a_type_error() {
        let e = env(&[("種別", Value::Choice("開き".to_owned()))]);
        assert!(matches!(
            fails("種別 < '引違い'", &e),
            EvalError::TypeMismatch { .. }
        ));
    }

    /// エラーメッセージが読めること（何が来て何が要るか）。
    #[test]
    fn type_errors_say_what_was_expected_and_found() {
        let e = env(&[("開く", Value::Bool(true))]);
        let msg = fails("開く + 1", &e).to_string();
        assert!(msg.contains("数値"), "{msg}");
        assert!(msg.contains("真偽"), "{msg}");
    }
}
