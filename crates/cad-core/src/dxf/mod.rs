//! DXF R12（AC1009）の読み書き。
//!
//! # スコープ
//!
//! 本プロトタイプが対応するのは DXF R12 のみ。R12 は行指向の単純なテキスト形式
//! （グループコード行 → 値行の繰り返し）で、`LWPOLYLINE` のような新しいエンティティが
//! 存在しないなど制約が多い一方、依存クレート無しで手書きするのに向いている。
//!
//! 読み書きの詳細は [`read`] / [`write`] を参照。

pub mod read;
pub mod write;

/// 本プロトタイプが書き出す DXF のバージョン文字列（`$ACADVER` の値）。
pub const ACAD_VERSION: &str = "AC1009";

/// ラジアン → 度。DXF の角度（`ARC` のグループコード 50/51 など）はすべて度で表現される。
///
/// **ラジアン⇔度の変換はこの関数と [`deg_to_rad`] の 2 箇所だけに閉じ込める。**
/// 変換式をあちこちに書くと `π/180` の掛け忘れ・掛けすぎが紛れ込みやすく、
/// しかも小さな角度のテストでは誤差が誤魔化されて発覚しにくい。
#[must_use]
pub fn rad_to_deg(rad: f64) -> f64 {
    rad * 180.0 / std::f64::consts::PI
}

/// 度 → ラジアン。[`rad_to_deg`] の逆変換。
#[must_use]
pub fn deg_to_rad(deg: f64) -> f64 {
    deg * std::f64::consts::PI / 180.0
}

/// R12 で安全なレイヤ名へ正規化する。
///
/// - 英字は大文字化する（ASCII のみ。日本語などケースの無い文字はそのまま）。
/// - 空白と DXF で問題になりやすい記号（`< > / \ " : ; ? * | , = '`）は `_` に置き換える。
/// - 結果が空文字列になる場合は `"LAYER"` を返す（DXF のレイヤ名は空にできないため）。
#[must_use]
pub fn sanitize_layer_name(name: &str) -> String {
    /// DXF のレイヤ名として使うと問題になりやすい記号。
    const INVALID_CHARS: &[char] = &[
        '<', '>', '/', '\\', '"', ':', ';', '?', '*', '|', ',', '=', '\'',
    ];

    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_whitespace() || INVALID_CHARS.contains(&c) {
            out.push('_');
        } else {
            out.push(c.to_ascii_uppercase());
        }
    }
    if out.is_empty() {
        out.push_str("LAYER");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::tolerance::{eq_angle, eq_len};
    use std::f64::consts::{FRAC_PI_2, PI};

    #[test]
    fn rad_to_deg_known_values() {
        assert!(eq_len(rad_to_deg(0.0), 0.0));
        assert!(eq_len(rad_to_deg(FRAC_PI_2), 90.0));
        assert!(eq_len(rad_to_deg(PI), 180.0));
    }

    #[test]
    fn deg_to_rad_known_values() {
        assert!(eq_angle(deg_to_rad(0.0), 0.0));
        assert!(eq_angle(deg_to_rad(90.0), FRAC_PI_2));
        assert!(eq_angle(deg_to_rad(180.0), PI));
    }

    /// 変換の往復が誤差なく戻ること。指示書が名指しする「見えにくい π/180 の
    /// 掛け忘れ・掛けすぎ」バグを直接検出するためのテスト。
    #[test]
    fn rad_deg_roundtrip() {
        for deg in [0.0, 30.0, 45.0, 90.0, 123.456, 270.0, 359.999, -45.0] {
            let back = rad_to_deg(deg_to_rad(deg));
            assert!(eq_len(back, deg), "deg={deg} back={back}");
        }
    }

    /// 90 度が本当に `PI / 2` ラジアンになること（指示書の明示的な受け入れ基準）。
    #[test]
    fn ninety_degrees_is_frac_pi_2() {
        assert!(eq_angle(deg_to_rad(90.0), FRAC_PI_2));
    }

    #[test]
    fn sanitize_lower_case_is_uppercased() {
        assert_eq!(sanitize_layer_name("wall"), "WALL");
        assert_eq!(sanitize_layer_name("Wall"), "WALL");
    }

    #[test]
    fn sanitize_spaces_become_underscore() {
        assert_eq!(sanitize_layer_name("my layer"), "MY_LAYER");
        assert_eq!(sanitize_layer_name("a  b"), "A__B");
    }

    #[test]
    fn sanitize_invalid_chars_become_underscore() {
        assert_eq!(sanitize_layer_name("a/b\\c:d"), "A_B_C_D");
        assert_eq!(sanitize_layer_name("<x>"), "_X_");
    }

    #[test]
    fn sanitize_empty_name_falls_back() {
        assert_eq!(sanitize_layer_name(""), "LAYER");
    }

    #[test]
    fn sanitize_already_valid_name_is_unchanged_except_case() {
        assert_eq!(sanitize_layer_name("WALL_01"), "WALL_01");
    }

    #[test]
    fn acad_version_is_r12() {
        assert_eq!(ACAD_VERSION, "AC1009");
    }
}
