//! 座標の直接入力の解釈。
//!
//! AutoCAD 互換の 3 形式に対応する。
//!
//! | 書式 | 意味 |
//! |---|---|
//! | `100,50` | 絶対座標 |
//! | `@100,50` | 直前の点からの相対座標 |
//! | `@100<45` | 直前の点からの相対極座標（長さ 100、角度 45°） |
//!
//! 角度は **度** で入力し、内部でラジアンへ変換する。反時計回りが正。
//!
//! # 全角文字の正規化
//!
//! IME がかな入力モードのとき `@100,50` は `＠１００，５０` のように全角で入りうる。
//! そのままでは解釈できないため、パース前に ASCII 相当の全角文字を半角へ畳む
//! （[`ADR-0002`](../../../../docs/DECISIONS.md) の決定事項）。

use cad_core::geom::{Point2, Vec2};

/// 解釈した座標入力。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CoordInput {
    /// 絶対座標。
    Absolute(Point2),
    /// 直前の点からの相対座標。
    Relative(Vec2),
    /// 直前の点からの相対極座標。角度はラジアン。
    RelativePolar { length: f64, angle: f64 },
}

impl CoordInput {
    /// 直前の点を与えて実際の座標を決める。
    ///
    /// 相対指定なのに直前の点が無い場合は `None`。
    #[must_use]
    pub fn resolve(self, last: Option<Point2>) -> Option<Point2> {
        match self {
            Self::Absolute(p) => Some(p),
            Self::Relative(v) => Some(last? + v),
            Self::RelativePolar { length, angle } => Some(last? + Vec2::polar(angle, length)),
        }
    }

    /// 直前の点を必要とする形式か。
    #[must_use]
    pub fn needs_last_point(self) -> bool {
        !matches!(self, Self::Absolute(_))
    }
}

/// ASCII 相当の全角文字を半角へ畳む。
///
/// 数値と区切り記号だけが対象で、それ以外の文字はそのまま残す。
#[must_use]
pub fn normalize_ascii(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            // 全角英数字・記号は ASCII と 0xFEE0 ずれている。
            '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
            // 全角スペース
            '\u{3000}' => ' ',
            // 長音記号やダッシュ類をマイナスとして受け付ける（かな入力の取りこぼし対策）。
            // 全角ハイフン '－' (U+FF0D) は上の範囲で既に変換済み。
            'ー' | '−' | '—' | '–' => '-',
            // 読点を区切りとして受け付ける。
            '、' => ',',
            '。' => '.',
            other => other,
        })
        .collect()
}

/// 座標入力を解釈する。解釈できなければ `None`。
///
/// 前後の空白は無視する。全角文字は自動で正規化する。
#[must_use]
pub fn parse(input: &str) -> Option<CoordInput> {
    let normalized = normalize_ascii(input);
    let s = normalized.trim();
    if s.is_empty() {
        return None;
    }

    if let Some(rest) = s.strip_prefix('@') {
        let rest = rest.trim();
        if let Some((len_s, ang_s)) = rest.split_once('<') {
            let length = parse_number(len_s)?;
            let angle_deg = parse_number(ang_s)?;
            return Some(CoordInput::RelativePolar {
                length,
                angle: angle_deg.to_radians(),
            });
        }
        let (x, y) = split_pair(rest)?;
        return Some(CoordInput::Relative(Vec2::new(x, y)));
    }

    // 絶対極座標 (`100<45`) は指示書の対象外なので受け付けない。
    // `<` を含む入力を絶対座標として誤解釈しないよう、ここで弾く。
    if s.contains('<') {
        return None;
    }

    let (x, y) = split_pair(s)?;
    Some(CoordInput::Absolute(Point2::new(x, y)))
}

/// `x,y` 形式を数値の組に分解する。
fn split_pair(s: &str) -> Option<(f64, f64)> {
    let (x_s, y_s) = s.split_once(',')?;
    Some((parse_number(x_s)?, parse_number(y_s)?))
}

/// 数値 1 つを解釈する。前後の空白は無視し、有限値のみ受け付ける。
///
/// 半径や距離の入力にも使う。
#[must_use]
pub fn parse_number(s: &str) -> Option<f64> {
    let normalized = normalize_ascii(s);
    let t = normalized.trim();
    if t.is_empty() {
        return None;
    }
    // `inf` / `nan` を弾く。図面座標として意味を持たないため。
    let v: f64 = t.parse().ok()?;
    v.is_finite().then_some(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cad_core::geom::tolerance::eq_len;

    fn abs(s: &str) -> Point2 {
        match parse(s).unwrap() {
            CoordInput::Absolute(p) => p,
            other => panic!("絶対座標を期待したが {other:?}"),
        }
    }

    #[test]
    fn parses_absolute() {
        let p = abs("100,50");
        assert!(eq_len(p.x, 100.0) && eq_len(p.y, 50.0));
    }

    #[test]
    fn parses_absolute_with_signs_and_decimals() {
        let p = abs("-12.5, 0.25");
        assert!(eq_len(p.x, -12.5) && eq_len(p.y, 0.25));
    }

    #[test]
    fn parses_relative_cartesian() {
        let got = parse("@100,50").unwrap();
        assert_eq!(got, CoordInput::Relative(Vec2::new(100.0, 50.0)));
        assert!(got.needs_last_point());
    }

    #[test]
    fn parses_relative_polar_in_degrees() {
        let CoordInput::RelativePolar { length, angle } = parse("@100<45").unwrap() else {
            panic!("相対極座標を期待した");
        };
        assert!(eq_len(length, 100.0));
        assert!(eq_len(angle, std::f64::consts::FRAC_PI_4));
    }

    /// 相対極座標が実際に正しい点へ解決されること。
    #[test]
    fn relative_polar_resolves_to_expected_point() {
        let last = Point2::new(10.0, 20.0);
        let p = parse("@100<0").unwrap().resolve(Some(last)).unwrap();
        assert!(eq_len(p.x, 110.0) && eq_len(p.y, 20.0));

        let p = parse("@100<90").unwrap().resolve(Some(last)).unwrap();
        assert!(eq_len(p.x, 10.0), "x = {}", p.x);
        assert!(eq_len(p.y, 120.0), "y = {}", p.y);
    }

    #[test]
    fn relative_resolves_from_last_point() {
        let last = Point2::new(5.0, 5.0);
        let p = parse("@10,-3").unwrap().resolve(Some(last)).unwrap();
        assert!(eq_len(p.x, 15.0) && eq_len(p.y, 2.0));
    }

    /// 直前の点が無ければ相対指定は解決できないこと。
    #[test]
    fn relative_without_last_point_fails() {
        assert!(parse("@10,10").unwrap().resolve(None).is_none());
        assert!(parse("@10<10").unwrap().resolve(None).is_none());
        // 絶対座標は直前の点が無くても解決できる。
        assert!(parse("1,2").unwrap().resolve(None).is_some());
    }

    /// IME のかな入力で全角になっても解釈できること（ADR-0002）。
    #[test]
    fn parses_fullwidth_input() {
        let got = parse("＠１００，５０").unwrap();
        assert_eq!(got, CoordInput::Relative(Vec2::new(100.0, 50.0)));

        let p = abs("１００．５，－５０");
        assert!(eq_len(p.x, 100.5), "x = {}", p.x);
        assert!(eq_len(p.y, -50.0), "y = {}", p.y);

        let CoordInput::RelativePolar { length, angle } = parse("＠１００＜４５").unwrap()
        else {
            panic!("相対極座標を期待した");
        };
        assert!(eq_len(length, 100.0));
        assert!(eq_len(angle, std::f64::consts::FRAC_PI_4));
    }

    #[test]
    fn normalize_leaves_other_characters_alone() {
        assert_eq!(normalize_ascii("線分ＬＩＮＥ"), "線分LINE");
        assert_eq!(normalize_ascii("100,50"), "100,50");
    }

    #[test]
    fn whitespace_is_ignored() {
        let p = abs("  100 , 50  ");
        assert!(eq_len(p.x, 100.0) && eq_len(p.y, 50.0));
        let got = parse(" @ 10 , 20 ").unwrap();
        assert_eq!(got, CoordInput::Relative(Vec2::new(10.0, 20.0)));
    }

    #[test]
    fn rejects_malformed_input() {
        for bad in [
            "",
            "   ",
            "abc",
            "100",
            "100,",
            ",50",
            "100,50,25",
            "@",
            "@100",
            "@<45",
            "@100<",
            "LINE",
            "100;50",
        ] {
            assert!(parse(bad).is_none(), "{bad:?} は拒否されるべき");
        }
    }

    /// 絶対極座標は対象外。絶対座標として誤解釈しないこと。
    #[test]
    fn rejects_absolute_polar() {
        assert!(parse("100<45").is_none());
    }

    #[test]
    fn rejects_non_finite_numbers() {
        assert!(parse_number("inf").is_none());
        assert!(parse_number("-inf").is_none());
        assert!(parse_number("NaN").is_none());
        assert!(parse("inf,0").is_none());
    }

    #[test]
    fn parse_number_accepts_plain_values() {
        assert!(eq_len(parse_number("42").unwrap(), 42.0));
        assert!(eq_len(parse_number(" -3.5 ").unwrap(), -3.5));
        assert!(eq_len(parse_number("１２．５").unwrap(), 12.5));
        assert!(parse_number("").is_none());
        assert!(parse_number("x").is_none());
    }

    /// 大きな座標でも精度が落ちないこと。
    #[test]
    fn parses_large_coordinates() {
        let p = abs("1000000.000001,-1000000.000001");
        assert!(eq_len(p.x, 1_000_000.000_001), "x = {}", p.x);
        assert!(eq_len(p.y, -1_000_000.000_001), "y = {}", p.y);
    }
}
