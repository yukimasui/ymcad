//! トレランス（許容誤差）の一元管理。
//!
//! # 規約
//!
//! **ソースコード中に `1e-9` のようなマジックナンバーを直接書くことを禁止する。**
//! 長さ・角度の比較は必ずこのモジュールの定数と関数を経由すること。
//! レビュー時に `grep -rn '1e-' crates/` で検査できる状態を保つため。
//!
//! # 比較方式
//!
//! 長さの比較は **絶対値の下限 + 相対誤差** のハイブリッドで行う。
//!
//! ```text
//! |a - b| <= max(EPS_LEN, EPS_REL * max(|a|, |b|))
//! ```
//!
//! 純粋な絶対値比較（`|a - b| <= EPS_LEN`）にしない理由:
//! 座標が 1e6 に達すると f64 の ULP は約 2e-10 になり、`EPS_LEN = 1e-9` は
//! わずか数 ULP しか許容しなくなる。つまり事実上の完全一致判定に化けてしまい、
//! 指示書が要求するズーム範囲 1e-6〜1e6 を支えられない。
//! 逆に純粋な相対比較にすると原点近傍（`a ≈ b ≈ 0`）で破綻するため、
//! `EPS_LEN` を絶対値の下限（フロア）として併用する。
//!
//! 角度は値域が `[0, 2π)` に収まり桁が暴れないため、絶対値比較のままでよい。

use std::f64::consts::{PI, TAU};

/// 長さ比較の絶対トレランス。原点近傍で使われる下限値。
pub const EPS_LEN: f64 = 1e-9;

/// 角度比較のトレランス [rad]。
pub const EPS_ANGLE: f64 = 1e-12;

/// 長さ比較の相対トレランス。座標の絶対値が大きい領域で効く。
pub const EPS_REL: f64 = 1e-12;

/// `a` と `b` を長さとして等しいとみなせるか。
///
/// 絶対値下限 [`EPS_LEN`] と相対誤差 [`EPS_REL`] のハイブリッド。
#[inline]
#[must_use]
pub fn eq_len(a: f64, b: f64) -> bool {
    let diff = (a - b).abs();
    // NaN が入ると全ての比較が false になり、そのまま false を返す（望ましい挙動）。
    diff <= len_tolerance(a, b)
}

/// `a` と `b` の比較に用いる実効トレランス。
#[inline]
#[must_use]
pub fn len_tolerance(a: f64, b: f64) -> f64 {
    let mag = a.abs().max(b.abs());
    EPS_LEN.max(EPS_REL * mag)
}

/// `a` を長さとしてゼロとみなせるか。
///
/// 比較対象が無いので相対誤差は使えない。純粋に [`EPS_LEN`] で判定する。
#[inline]
#[must_use]
pub fn is_zero_len(a: f64) -> bool {
    a.abs() <= EPS_LEN
}

/// `a < b` を長さとして判定する（トレランス内は「等しい」として false）。
#[inline]
#[must_use]
pub fn lt_len(a: f64, b: f64) -> bool {
    !eq_len(a, b) && a < b
}

/// `a > b` を長さとして判定する（トレランス内は「等しい」として false）。
#[inline]
#[must_use]
pub fn gt_len(a: f64, b: f64) -> bool {
    !eq_len(a, b) && a > b
}

/// 2 つの角度 [rad] を等しいとみなせるか。
///
/// `0` と `2π` のような一周ぶんの差は等しいと判定する。
///
/// 差が既に十分小さい場合は正規化を挟まずに判定する。
/// `wrap_signed` は内部で `± TAU` の加減算を行うため、`1e-12` 程度の微小な差を
/// 通すと丸め誤差が約 `9e-17` 混入し、ちょうど境界の値が誤って不一致になるため。
#[inline]
#[must_use]
pub fn eq_angle(a: f64, b: f64) -> bool {
    let d = a - b;
    d.abs() <= EPS_ANGLE || wrap_signed(d).abs() <= EPS_ANGLE
}

/// 角度 [rad] を `[0, 2π)` に正規化する。
#[inline]
#[must_use]
pub fn wrap_2pi(a: f64) -> f64 {
    let r = a % TAU;
    if r < 0.0 {
        r + TAU
    } else {
        r
    }
}

/// 角度 [rad] を `(-π, π]` に正規化する。
#[inline]
#[must_use]
pub fn wrap_signed(a: f64) -> f64 {
    let r = wrap_2pi(a);
    if r > PI {
        r - TAU
    } else {
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eq_len_exact_and_trivial() {
        assert!(eq_len(1.0, 1.0));
        assert!(eq_len(0.0, 0.0));
        assert!(!eq_len(1.0, 2.0));
    }

    /// 原点近傍では EPS_LEN が下限として効く（相対誤差はほぼゼロになる）。
    #[test]
    fn eq_len_boundary_near_origin() {
        // ちょうど境界は「等しい」に含む
        assert!(eq_len(0.0, EPS_LEN));
        // 境界のわずか内側 / 外側
        assert!(eq_len(0.0, EPS_LEN * 0.999));
        assert!(!eq_len(0.0, EPS_LEN * 1.001));
    }

    /// 大きな座標では相対誤差が効き、絶対値 EPS_LEN より緩くなる。
    /// この挙動こそがハイブリッド方式を採用した理由。
    #[test]
    fn eq_len_boundary_at_large_magnitude() {
        let mag = 1e6;
        // mag での実効トレランスは EPS_REL * mag = 1e-6
        assert!((len_tolerance(mag, mag) - 1e-6).abs() < 1e-18);

        // 絶対値 1e-9 しか違わない 2 値は、この領域では「等しい」
        assert!(eq_len(mag, mag + 1e-9));
        // 実効トレランスを超えれば区別される
        assert!(!eq_len(mag, mag + 1e-5));
    }

    /// 純粋な絶対値比較なら破綻する領域で、ハイブリッドが機能することの確認。
    #[test]
    fn eq_len_survives_full_zoom_range() {
        for exp in -6..=6 {
            let mag = 10f64.powi(exp);
            let tol = len_tolerance(mag, mag);
            // 実効トレランスは必ず 1 ULP より大きい = 完全一致判定に化けない
            let ulp = mag.abs() * f64::EPSILON;
            assert!(
                tol > ulp,
                "exp={exp}: tolerance {tol:e} must exceed 1 ULP {ulp:e}"
            );
        }
    }

    #[test]
    fn eq_len_rejects_nan() {
        assert!(!eq_len(f64::NAN, 1.0));
        assert!(!eq_len(1.0, f64::NAN));
        assert!(!eq_len(f64::NAN, f64::NAN));
    }

    #[test]
    fn is_zero_len_boundary() {
        assert!(is_zero_len(0.0));
        assert!(is_zero_len(EPS_LEN));
        assert!(is_zero_len(-EPS_LEN));
        assert!(!is_zero_len(EPS_LEN * 1.001));
    }

    #[test]
    fn lt_gt_treat_tolerance_as_equal() {
        assert!(!lt_len(0.0, EPS_LEN * 0.5));
        assert!(!gt_len(EPS_LEN * 0.5, 0.0));
        assert!(lt_len(0.0, 1.0));
        assert!(gt_len(1.0, 0.0));
    }

    #[test]
    fn wrap_2pi_range() {
        assert!((wrap_2pi(0.0) - 0.0).abs() < EPS_ANGLE);
        assert!((wrap_2pi(TAU) - 0.0).abs() < EPS_ANGLE);
        assert!((wrap_2pi(-PI) - PI).abs() < EPS_ANGLE);
        assert!((wrap_2pi(3.0 * TAU + 1.0) - 1.0).abs() < 1e-9);
        for a in [-100.0, -1.0, 0.0, 1.0, 100.0] {
            let w = wrap_2pi(a);
            assert!((0.0..TAU).contains(&w), "wrap_2pi({a}) = {w}");
        }
    }

    #[test]
    fn wrap_signed_range() {
        assert!((wrap_signed(PI) - PI).abs() < EPS_ANGLE);
        assert!((wrap_signed(-PI) - PI).abs() < EPS_ANGLE);
        assert!((wrap_signed(TAU) - 0.0).abs() < EPS_ANGLE);
        for a in [-100.0, -1.0, 0.0, 1.0, 100.0] {
            let w = wrap_signed(a);
            assert!(
                w > -PI - EPS_ANGLE && w <= PI + EPS_ANGLE,
                "wrap_signed({a}) = {w}"
            );
        }
    }

    #[test]
    fn eq_angle_wraps_around() {
        assert!(eq_angle(0.0, TAU));
        assert!(eq_angle(0.0, -TAU));
        assert!(eq_angle(PI, -PI));
        assert!(!eq_angle(0.0, PI));
    }

    #[test]
    fn eq_angle_boundary() {
        assert!(eq_angle(0.0, EPS_ANGLE));
        assert!(!eq_angle(0.0, EPS_ANGLE * 100.0));
    }
}
