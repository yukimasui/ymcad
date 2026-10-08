//! プロパティパネルの数値編集（Issue #31 段階 2）を決める純粋な関数。
//!
//! egui に依存しない。「形 × 項目 × 値 → 新しい形、または理由つきのエラー」を決める。
//! 描くのと確定の時機を決めるのは `properties_panel.rs`。
//!
//! # 編集した 1 項目だけを差し替える
//!
//! 新しい形は元の形を複製し、編集した項目に関わる値だけを書き換えて作る。ほかの値は
//! 元の `f64` のまま残す。表示用に度へ直した角度を、ラジアンへ戻して書き込むことは
//! しない（往復で値がずれて、触っていない図形の値が動くため）。
//!
//! # 不正な値は確定しない
//!
//! 非有限の値・0 以下の半径や倍率・長さ 0 などは、理由の文を返して止める
//! （設計原則 6: `NaN` は作らせずエラーで止める）。黙って直さない（ADR-0040 の
//! 「採らなかった案」と同じ考え）。最後の砦として [`Geometry::validate`] も通す。

use cad_core::geom::tolerance::{eq_angle, is_zero_len, wrap_2pi};
use cad_core::geom::Vec2;
use cad_core::{CadError, Geometry};

use crate::cmdline::coord::normalize_ascii;
use crate::properties::fmt_num;

/// 数値で編集できる項目。種類ごとの項目の並びは `properties::items` が決める。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Field {
    /// 線分の始点 X。
    LineStartX,
    /// 線分の始点 Y。
    LineStartY,
    /// 線分の終点 X。
    LineEndX,
    /// 線分の終点 Y。
    LineEndY,
    /// 線分の長さ（始点と向きを保ち、終点を動かす）。
    LineLength,
    /// 線分の角度 [度]（始点と長さを保ち、終点を動かす）。
    LineAngle,
    /// 円・円弧の中心 X。
    CenterX,
    /// 円・円弧の中心 Y。
    CenterY,
    /// 円・円弧の半径。
    Radius,
    /// 円弧の開始角 [度]。
    ArcStart,
    /// 円弧の終了角 [度]。
    ArcEnd,
    /// 作図線の通過点 X。
    XlineX,
    /// 作図線の通過点 Y。
    XlineY,
    /// 作図線の角度 [度]。
    XlineAngle,
    /// インスタンスの基点 X。
    BaseX,
    /// インスタンスの基点 Y。
    BaseY,
    /// インスタンスの回転 [度]。
    Rotation,
    /// インスタンスの倍率。
    Scale,
}

impl Field {
    /// 角度（度で表示・入力する）か。
    #[must_use]
    pub fn is_angle(self) -> bool {
        matches!(
            self,
            Self::LineAngle | Self::ArcStart | Self::ArcEnd | Self::XlineAngle | Self::Rotation
        )
    }
}

/// はい・いいえで切り替える項目。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Toggle {
    /// ポリラインの閉じ（頂点 3 以上のときだけ編集できる）。
    Closed,
    /// インスタンスの反転。
    Flipped,
}

/// 入力が数値として読めないときの理由。
pub const NOT_A_NUMBER: &str = "数値として読めません（数値だけを入力してください。式は使えません）";
/// 非有限（`nan`・`inf`・桁あふれ）の値の理由。
pub const NOT_FINITE: &str = "有限の数値を入力してください";
/// 0 以下の長さの理由。
pub const LENGTH_NOT_POSITIVE: &str = "長さは 0 より大きい値にしてください";
/// 0 以下の半径の理由。
pub const RADIUS_NOT_POSITIVE: &str = "半径は 0 より大きい値にしてください";
/// 0 以下の倍率の理由。
pub const SCALE_NOT_POSITIVE: &str = "倍率は 0 より大きい値にしてください";
/// 円弧の開始角と終了角を同じにしようとしたときの理由（Issue #31 のユーザー判断 7）。
pub const ZERO_SWEEP: &str = "掃引が 0° になります（開始角と終了角が同じです）";
/// 頂点が 2 つのポリラインを閉じようとしたときの理由。
pub const CLOSE_NEEDS_THREE: &str = "閉じるには頂点が 3 つ以上要ります";
/// 図形の種類に無い項目を指したとき（呼び出し側の誤り。画面からは起きない）。
pub const NO_SUCH_FIELD: &str = "この図形にはその項目がありません";

/// 角度 [rad] を、表示と同じ `[0, 360)` の度に直す。丸めて `360.0000` に見える値は 0 にする
/// （段階 1 の表示と、編集欄の値の両方がこれを使う）。
#[must_use]
pub fn display_deg(rad: f64) -> f64 {
    let deg = wrap_2pi(rad).to_degrees();
    if fmt_num(deg) == "360.0000" {
        0.0
    } else {
        deg
    }
}

/// 項目の、いま表示している値（角度は度）。図形の種類に無い項目なら `None`。
#[must_use]
pub fn display_value(geom: &Geometry, field: Field) -> Option<f64> {
    Some(match (geom, field) {
        (Geometry::Line(l), Field::LineStartX) => l.a.x,
        (Geometry::Line(l), Field::LineStartY) => l.a.y,
        (Geometry::Line(l), Field::LineEndX) => l.b.x,
        (Geometry::Line(l), Field::LineEndY) => l.b.y,
        (Geometry::Line(l), Field::LineLength) => l.length(),
        (Geometry::Line(l), Field::LineAngle) => display_deg(l.vector().angle()),
        (Geometry::Circle(c), Field::CenterX) => c.center.x,
        (Geometry::Circle(c), Field::CenterY) => c.center.y,
        (Geometry::Circle(c), Field::Radius) => c.radius,
        (Geometry::Arc(a), Field::CenterX) => a.center.x,
        (Geometry::Arc(a), Field::CenterY) => a.center.y,
        (Geometry::Arc(a), Field::Radius) => a.radius,
        (Geometry::Arc(a), Field::ArcStart) => display_deg(a.start_angle),
        (Geometry::Arc(a), Field::ArcEnd) => display_deg(a.end_angle),
        (Geometry::Xline(x), Field::XlineX) => x.origin.x,
        (Geometry::Xline(x), Field::XlineY) => x.origin.y,
        (Geometry::Xline(x), Field::XlineAngle) => display_deg(x.angle()),
        (Geometry::Instance(i), Field::BaseX) => i.placement.origin.x,
        (Geometry::Instance(i), Field::BaseY) => i.placement.origin.y,
        (Geometry::Instance(i), Field::Rotation) => display_deg(i.placement.rotation),
        (Geometry::Instance(i), Field::Scale) => i.placement.scale,
        _ => return None,
    })
}

/// 入力欄の文字列を数値にする。
///
/// 全角数字・全角の記号・`−`（U+2212）などは半角へ畳む（`coord::normalize_ascii`、
/// ADR-0002 と同じ約束）。前後の空白と、末尾の `°` は無視する。Rust の `f64` の読み取りは
/// `nan`・`inf`・`infinity` を受け付け、`1e400` は無限大になるので、有限でなければ拒む。
///
/// # Errors
///
/// 読めなければ [`NOT_A_NUMBER`]、有限でなければ [`NOT_FINITE`]。
pub fn parse_number(text: &str) -> Result<f64, &'static str> {
    let normalized = normalize_ascii(text);
    let s = normalized.trim();
    let s = s.strip_suffix('°').unwrap_or(s).trim_end();
    let v: f64 = s.parse().map_err(|_| NOT_A_NUMBER)?;
    if v.is_finite() {
        Ok(v)
    } else {
        Err(NOT_FINITE)
    }
}

/// 2 つの値が、画面の表示（小数点以下 4 桁）で同じに見えるか。
///
/// 表示は丸めた値なので、欄をクリックしてそのまま Enter を押すと、丸めた値が入力される。
/// それを「変更」として確定すると、触っただけの図形が丸めの分だけ動く。
#[must_use]
pub fn same_on_screen(a: f64, b: f64) -> bool {
    fmt_num(a) == fmt_num(b)
}

/// 2 つの角度 [rad] が、画面の表示（`[0, 360)` の度、4 桁）で同じに見えるか。
fn same_angle_on_screen(a: f64, b: f64) -> bool {
    eq_angle(a, b) || same_on_screen(display_deg(a), display_deg(b))
}

/// `value` が 0 より大きい長さか（トレランスで 0 とみなせる値も拒む）。
fn positive(value: f64, reason: &'static str) -> Result<(), &'static str> {
    if value <= 0.0 || is_zero_len(value) {
        Err(reason)
    } else {
        Ok(())
    }
}

/// 形の検証のエラーを、画面に出す理由の文にする。
fn reason(e: &CadError) -> String {
    match e {
        CadError::DegenerateGeometry(m) | CadError::NotEditable(m) => (*m).to_owned(),
        other => other.to_string(),
    }
}

/// 円弧の片方の角を `value` [度] にする。もう片方と同じに見える値は、元が 1 周の円弧なら
/// 1 周のまま（両方を同じ値にする）、そうでなければ掃引 0 として拒む（判断 7）。
///
/// `Arc::sweep` は開始角と終了角の一致を 1 周と約束しているので、拒まないと、利用者が
/// 「短い円弧の始点を終点へ寄せた」つもりで 1 周の円になる（ADR-0040 決定 4）。
fn set_arc_angle(
    angle: &mut f64,
    other: f64,
    was_full: bool,
    value: f64,
) -> Result<(), &'static str> {
    let rad = value.to_radians();
    if same_angle_on_screen(rad, other) {
        if !was_full {
            return Err(ZERO_SWEEP);
        }
        *angle = other;
    } else {
        *angle = rad;
    }
    Ok(())
}

/// 項目 `field` を `value`（角度は度）にした新しい形。ほかの値は元のまま。
///
/// # Errors
///
/// 値が不正なら、画面に出す理由の文。
pub fn with_number(geom: &Geometry, field: Field, value: f64) -> Result<Geometry, String> {
    if !value.is_finite() {
        return Err(NOT_FINITE.to_owned());
    }
    let mut g = geom.clone();
    match (&mut g, field) {
        (Geometry::Line(l), Field::LineStartX) => l.a.x = value,
        (Geometry::Line(l), Field::LineStartY) => l.a.y = value,
        (Geometry::Line(l), Field::LineEndX) => l.b.x = value,
        (Geometry::Line(l), Field::LineEndY) => l.b.y = value,
        (Geometry::Line(l), Field::LineLength) => {
            positive(value, LENGTH_NOT_POSITIVE)?;
            // 元の線分は長さを持つ（`Geometry::validate` を通った図形だけが図面にある）。
            let dir = l.dir().ok_or(LENGTH_NOT_POSITIVE)?;
            l.b = l.a + dir * value;
        }
        (Geometry::Line(l), Field::LineAngle) => {
            let len = l.length();
            l.b = l.a + Vec2::polar(value.to_radians(), len);
        }
        (Geometry::Circle(c), Field::CenterX) => c.center.x = value,
        (Geometry::Circle(c), Field::CenterY) => c.center.y = value,
        (Geometry::Circle(c), Field::Radius) => {
            positive(value, RADIUS_NOT_POSITIVE)?;
            c.radius = value;
        }
        (Geometry::Arc(a), Field::CenterX) => a.center.x = value,
        (Geometry::Arc(a), Field::CenterY) => a.center.y = value,
        (Geometry::Arc(a), Field::Radius) => {
            positive(value, RADIUS_NOT_POSITIVE)?;
            a.radius = value;
        }
        (Geometry::Arc(a), Field::ArcStart) => {
            let was_full = eq_angle(a.start_angle, a.end_angle);
            set_arc_angle(&mut a.start_angle, a.end_angle, was_full, value)?;
        }
        (Geometry::Arc(a), Field::ArcEnd) => {
            let was_full = eq_angle(a.start_angle, a.end_angle);
            set_arc_angle(&mut a.end_angle, a.start_angle, was_full, value)?;
        }
        (Geometry::Xline(x), Field::XlineX) => x.origin.x = value,
        (Geometry::Xline(x), Field::XlineY) => x.origin.y = value,
        (Geometry::Xline(x), Field::XlineAngle) => {
            x.direction = Vec2::from_angle(value.to_radians())
        }
        (Geometry::Instance(i), Field::BaseX) => i.placement.origin.x = value,
        (Geometry::Instance(i), Field::BaseY) => i.placement.origin.y = value,
        (Geometry::Instance(i), Field::Rotation) => i.placement.rotation = value.to_radians(),
        (Geometry::Instance(i), Field::Scale) => {
            positive(value, SCALE_NOT_POSITIVE)?;
            i.placement.scale = value;
        }
        _ => return Err(NO_SUCH_FIELD.to_owned()),
    }
    // 最後の砦。上で拾い切れない形（端点を重ねた線分、桁あふれの座標など）を止める。
    g.validate().map_err(|e| reason(&e))?;
    Ok(g)
}

/// 項目を `value` にしたときに確定すべき新しい形。
///
/// - 表示（4 桁）で今の値と同じに見えるなら `Ok(None)`（確定しない。触っただけ・Enter を
///   押しただけで図形が丸めの分だけ動いたり、履歴が増えたりしない）。角度は 360° 違いも
///   同じ値（`[0, 360)` に巻いた表示の値で比べる）
/// - 結果が元の形と同じでも `Ok(None)`（`ReplaceGeometries` は同じ形でも履歴に積むので、
///   ここで止める。ADR-0040 の補足）
///
/// # Errors
///
/// 値が不正なら、画面に出す理由の文。
pub fn edit_number(geom: &Geometry, field: Field, value: f64) -> Result<Option<Geometry>, String> {
    let current = display_value(geom, field).ok_or_else(|| NO_SUCH_FIELD.to_owned())?;
    if !value.is_finite() {
        return Err(NOT_FINITE.to_owned());
    }
    // 角度は 360° 違いを同じ値と見る。`current` は `[0, 360)` に巻いた表示の値なので、入力も
    // 同じ巻き方にしてから比べる（0° の線分に `360` を打って履歴が増え、終点が誤差だけ
    // 動くのを防ぐ）。巻くのは比べるときだけで、確定する値は打たれたまま `with_number` へ渡す。
    let compared = if field.is_angle() {
        display_deg(value.to_radians())
    } else {
        value
    };
    if same_on_screen(compared, current) {
        return Ok(None);
    }
    let new = with_number(geom, field, value)?;
    Ok((new != *geom).then_some(new))
}

/// はい・いいえの項目を `value` にしたときに確定すべき新しい形。変わらなければ `Ok(None)`。
///
/// # Errors
///
/// 値が不正なら、画面に出す理由の文。
pub fn edit_toggle(
    geom: &Geometry,
    toggle: Toggle,
    value: bool,
) -> Result<Option<Geometry>, String> {
    let mut g = geom.clone();
    match (&mut g, toggle) {
        (Geometry::Polyline(p), Toggle::Closed) => {
            if value && p.vertices.len() < 3 {
                return Err(CLOSE_NEEDS_THREE.to_owned());
            }
            p.closed = value;
        }
        (Geometry::Instance(i), Toggle::Flipped) => i.placement.flipped = value,
        _ => return Err(NO_SUCH_FIELD.to_owned()),
    }
    g.validate().map_err(|e| reason(&e))?;
    Ok((g != *geom).then_some(g))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cad_core::command::{DefineComponent, InsertInstance};
    use cad_core::component::Placement;
    use cad_core::geom::tolerance::eq_len;
    use cad_core::geom::{Arc, Circle, Line, Point2, Polyline, Xline};
    use cad_core::{Document, Entity, LayerId};
    use std::f64::consts::{FRAC_PI_2, PI, TAU};

    fn line() -> Geometry {
        Geometry::Line(Line::new(Point2::new(1.0, 2.0), Point2::new(31.0, 42.0)))
    }

    fn arc(start: f64, end: f64) -> Geometry {
        Geometry::Arc(Arc::new(Point2::new(5.0, 6.0), 10.0, start, end))
    }

    /// 基点 (3, 4)・回転 0.3 rad・倍率 2 のインスタンス（定義は図面を作って足す）。
    fn instance() -> Geometry {
        instance_rotated(0.3)
    }

    /// 回転が `rotation` [rad] のインスタンス（ほかは [`instance`] と同じ）。
    fn instance_rotated(rotation: f64) -> Geometry {
        let mut doc = Document::new();
        doc.apply(Box::new(DefineComponent::new(
            "COMPONENT",
            "窓",
            Point2::ORIGIN,
            vec![Entity::new(
                Geometry::Line(Line::new(Point2::ORIGIN, Point2::new(1.0, 0.0))),
                LayerId::ZERO,
            )],
        )))
        .expect("定義");
        let def = doc.definitions().by_name("窓").expect("定義がある");
        let placement = Placement::new(Point2::new(3.0, 4.0), rotation, 2.0, false).expect("配置");
        doc.apply(Box::new(InsertInstance::new(
            "INSERT",
            def,
            placement,
            LayerId::ZERO,
        )))
        .expect("配置");
        let id = doc.entities().ids().last().expect("インスタンス");
        doc.entities().get(id).expect("ある").geom.clone()
    }

    fn edited(geom: &Geometry, field: Field, value: f64) -> Geometry {
        edit_number(geom, field, value)
            .expect("通る値")
            .expect("形が変わる")
    }

    // ---- 1 項目だけが変わる ----------------------------------------------------

    /// 線分の座標を 1 つ変えると、その座標だけが変わる（ほかの 3 つはビット単位で元のまま）。
    #[test]
    fn line_coordinates_change_only_the_edited_value() {
        let Geometry::Line(orig) = line() else {
            unreachable!()
        };
        let cases = [
            (Field::LineStartX, Line::new(Point2::new(7.5, 2.0), orig.b)),
            (Field::LineStartY, Line::new(Point2::new(1.0, 7.5), orig.b)),
            (Field::LineEndX, Line::new(orig.a, Point2::new(7.5, 42.0))),
            (Field::LineEndY, Line::new(orig.a, Point2::new(31.0, 7.5))),
        ];
        for (field, want) in cases {
            assert_eq!(
                edited(&line(), field, 7.5),
                Geometry::Line(want),
                "{field:?}"
            );
        }
    }

    /// 長さは始点と向きを保って終点を動かし、角度は始点と長さを保って終点を回す。
    #[test]
    fn line_length_and_angle_keep_the_start_point() {
        let Geometry::Line(l) = edited(&line(), Field::LineLength, 100.0) else {
            unreachable!()
        };
        assert_eq!(l.a, Point2::new(1.0, 2.0), "始点はそのまま");
        assert!(eq_len(l.length(), 100.0));
        assert!(
            eq_len(l.b.x, 1.0 + 60.0) && eq_len(l.b.y, 2.0 + 80.0),
            "向きは 3:4 のまま"
        );

        let Geometry::Line(l) = edited(&line(), Field::LineAngle, 90.0) else {
            unreachable!()
        };
        assert_eq!(l.a, Point2::new(1.0, 2.0), "始点はそのまま");
        assert!(eq_len(l.length(), 50.0), "長さはそのまま");
        assert!(eq_len(l.b.x, 1.0) && eq_len(l.b.y, 52.0), "真上を向く");
    }

    #[test]
    fn circle_center_and_radius() {
        let c = Geometry::Circle(Circle::new(Point2::new(1.0, 2.0), 3.0));
        assert_eq!(
            edited(&c, Field::CenterX, -4.0),
            Geometry::Circle(Circle::new(Point2::new(-4.0, 2.0), 3.0))
        );
        assert_eq!(
            edited(&c, Field::CenterY, -4.0),
            Geometry::Circle(Circle::new(Point2::new(1.0, -4.0), 3.0))
        );
        assert_eq!(
            edited(&c, Field::Radius, 8.0),
            Geometry::Circle(Circle::new(Point2::new(1.0, 2.0), 8.0))
        );
    }

    /// 円弧の中心や半径を変えても、角度（ラジアンの `f64`）は度との往復を通らず元のまま。
    #[test]
    fn arc_angles_are_not_round_tripped_through_degrees() {
        // 度にしてラジアンへ戻すと同じ値に戻らない角度。
        let start: f64 = 0.085_042;
        let end: f64 = 2.291_324;
        assert_ne!(
            start.to_degrees().to_radians(),
            start,
            "前提: 往復でずれる値"
        );
        assert_ne!(end.to_degrees().to_radians(), end, "前提: 往復でずれる値");
        for field in [Field::CenterX, Field::CenterY, Field::Radius] {
            let Geometry::Arc(a) = edited(&arc(start, end), field, 20.0) else {
                unreachable!()
            };
            assert_eq!(a.start_angle.to_bits(), start.to_bits(), "{field:?}");
            assert_eq!(a.end_angle.to_bits(), end.to_bits(), "{field:?}");
        }
        // 開始角を変えても、終了角は元のまま。
        let Geometry::Arc(a) = edited(&arc(start, end), Field::ArcStart, 90.0) else {
            unreachable!()
        };
        assert_eq!(a.start_angle, 90f64.to_radians());
        assert_eq!(a.end_angle.to_bits(), end.to_bits());
        let Geometry::Arc(a) = edited(&arc(start, end), Field::ArcEnd, 270.0) else {
            unreachable!()
        };
        assert_eq!(a.start_angle.to_bits(), start.to_bits());
        assert_eq!(a.end_angle, 270f64.to_radians());
        assert_eq!((a.center, a.radius), (Point2::new(5.0, 6.0), 10.0));
    }

    /// インスタンスの回転・倍率・基点を変えても、ほかの配置の値は元のまま。
    #[test]
    fn instance_placement_changes_one_value() {
        let Geometry::Instance(orig) = instance() else {
            unreachable!()
        };
        let p = |g: Geometry| match g {
            Geometry::Instance(i) => i.placement,
            _ => unreachable!(),
        };
        let got = p(edited(&instance(), Field::BaseX, 9.0));
        assert_eq!((got.origin.x, got.origin.y), (9.0, 4.0));
        assert_eq!(got.rotation.to_bits(), orig.placement.rotation.to_bits());
        let got = p(edited(&instance(), Field::BaseY, 9.0));
        assert_eq!((got.origin.x, got.origin.y), (3.0, 9.0));
        let got = p(edited(&instance(), Field::Rotation, 90.0));
        assert_eq!(got.rotation, FRAC_PI_2);
        assert_eq!((got.origin, got.scale), (orig.placement.origin, 2.0));
        let got = p(edited(&instance(), Field::Scale, 0.5));
        assert_eq!(got.scale, 0.5);
        assert_eq!(got.rotation.to_bits(), orig.placement.rotation.to_bits());
    }

    #[test]
    fn xline_through_point_and_angle() {
        let x = Geometry::Xline(Xline::at_angle(Point2::new(1.0, 2.0), 0.4));
        let Geometry::Xline(got) = edited(&x, Field::XlineX, 5.0) else {
            unreachable!()
        };
        assert_eq!(got.origin, Point2::new(5.0, 2.0));
        assert_eq!(got.direction, Vec2::from_angle(0.4), "向きは元のまま");
        let Geometry::Xline(got) = edited(&x, Field::XlineAngle, 90.0) else {
            unreachable!()
        };
        assert_eq!(got.origin, Point2::new(1.0, 2.0));
        assert!(eq_len(got.direction.x, 0.0) && eq_len(got.direction.y, 1.0));
    }

    /// 種類に無い項目は拒む（画面からは起きないが、取り違えで別の値を書き換えない）。
    #[test]
    fn a_field_of_another_kind_is_refused() {
        assert_eq!(
            edit_number(&line(), Field::Radius, 3.0),
            Err(NO_SUCH_FIELD.to_owned())
        );
        assert_eq!(
            edit_toggle(&line(), Toggle::Closed, true),
            Err(NO_SUCH_FIELD.to_owned())
        );
    }

    // ---- 度とラジアン -----------------------------------------------------------

    /// 表示する角度は `[0, 360)` の度。負の向きは 360 を足し、丸めて 360 に見える値は 0。
    #[test]
    fn angles_are_shown_in_degrees_between_0_and_360() {
        let horizontal_left = Geometry::Line(Line::new(Point2::ORIGIN, Point2::new(-1.0, 0.0)));
        assert_eq!(
            display_value(&horizontal_left, Field::LineAngle),
            Some(180.0)
        );
        let down = Geometry::Line(Line::new(Point2::ORIGIN, Point2::new(0.0, -1.0)));
        assert_eq!(display_value(&down, Field::LineAngle), Some(270.0));
        assert_eq!(display_deg(-f64::MIN_POSITIVE), 0.0);
        assert_eq!(display_deg(TAU), 0.0);
        assert_eq!(display_value(&arc(0.0, TAU), Field::ArcEnd), Some(0.0));
        assert_eq!(display_value(&arc(PI, 0.0), Field::ArcStart), Some(180.0));
    }

    // ---- 入力の読み取り ---------------------------------------------------------

    #[test]
    fn parse_accepts_full_width_digits_and_unicode_minus() {
        assert_eq!(parse_number("12.5"), Ok(12.5));
        assert_eq!(parse_number(" -3 "), Ok(-3.0));
        assert_eq!(parse_number("１２．５"), Ok(12.5), "全角数字と全角の小数点");
        assert_eq!(parse_number("−７"), Ok(-7.0), "U+2212 のマイナス");
        assert_eq!(parse_number("－７"), Ok(-7.0), "全角ハイフン");
        assert_eq!(
            parse_number("\u{3000}４５°"),
            Ok(45.0),
            "全角スペースと度の記号"
        );
        assert_eq!(parse_number("1e3"), Ok(1000.0));
    }

    /// `nan`・`inf` と桁あふれは拒む。数値でない文字列も拒む。
    #[test]
    fn parse_refuses_non_finite_and_garbage() {
        for text in ["nan", "NaN", "inf", "-inf", "infinity", "1e400", "ｉｎｆ"] {
            assert_eq!(parse_number(text), Err(NOT_FINITE), "{text}");
        }
        for text in ["", " ", "abc", "1,2", "１２あ", "--1"] {
            assert_eq!(parse_number(text), Err(NOT_A_NUMBER), "{text}");
        }
    }

    // ---- 不正な値 ---------------------------------------------------------------

    #[test]
    fn non_finite_values_are_refused() {
        for v in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(
                edit_number(&line(), Field::LineStartX, v),
                Err(NOT_FINITE.to_owned())
            );
        }
        // 有限でも、結果の座標があふれるなら形の検証で止まる。
        let far = Geometry::Line(Line::new(Point2::new(f64::MAX, 0.0), Point2::new(0.0, 0.0)));
        assert!(edit_number(&far, Field::LineLength, f64::MAX).is_err());
    }

    #[test]
    fn zero_or_negative_radius_scale_and_length_are_refused() {
        let c = Geometry::Circle(Circle::new(Point2::ORIGIN, 3.0));
        for v in [0.0, -1.0, -0.0] {
            assert_eq!(
                edit_number(&c, Field::Radius, v),
                Err(RADIUS_NOT_POSITIVE.to_owned())
            );
            assert_eq!(
                edit_number(&arc(0.0, 1.0), Field::Radius, v),
                Err(RADIUS_NOT_POSITIVE.to_owned())
            );
            assert_eq!(
                edit_number(&instance(), Field::Scale, v),
                Err(SCALE_NOT_POSITIVE.to_owned())
            );
            assert_eq!(
                edit_number(&line(), Field::LineLength, v),
                Err(LENGTH_NOT_POSITIVE.to_owned())
            );
        }
    }

    /// 端点を重ねて長さ 0 にする入力は、形の検証（最後の砦）で理由つきで止まる。
    #[test]
    fn moving_an_endpoint_onto_the_other_is_refused() {
        let l = Geometry::Line(Line::new(Point2::new(5.0, 0.0), Point2::new(0.0, 0.0)));
        assert_eq!(
            edit_number(&l, Field::LineStartX, 0.0),
            Err("線分の長さが 0 です".to_owned())
        );
    }

    #[test]
    fn a_two_vertex_polyline_cannot_be_closed() {
        let open = Polyline::new(vec![Point2::ORIGIN, Point2::new(1.0, 0.0)], false);
        assert_eq!(
            edit_toggle(&Geometry::Polyline(open), Toggle::Closed, true),
            Err(CLOSE_NEEDS_THREE.to_owned())
        );
        let three = Polyline::new(
            vec![Point2::ORIGIN, Point2::new(1.0, 0.0), Point2::new(1.0, 1.0)],
            false,
        );
        let got = edit_toggle(&Geometry::Polyline(three.clone()), Toggle::Closed, true)
            .expect("閉じられる")
            .expect("変わる");
        assert_eq!(
            got,
            Geometry::Polyline(Polyline {
                closed: true,
                ..three.clone()
            })
        );
        assert_eq!(
            edit_toggle(&Geometry::Polyline(three), Toggle::Closed, false),
            Ok(None),
            "変わらなければ確定しない"
        );
    }

    #[test]
    fn flipping_an_instance_changes_only_the_flag() {
        let Some(Geometry::Instance(i)) =
            edit_toggle(&instance(), Toggle::Flipped, true).expect("通る")
        else {
            panic!("変わる")
        };
        assert!(i.placement.flipped);
        assert_eq!(
            (i.placement.origin, i.placement.scale),
            (Point2::new(3.0, 4.0), 2.0)
        );
    }

    // ---- 円弧の開始角 = 終了角（判断 7） ---------------------------------------

    /// 1 周でない円弧の開始角を終了角と同じにする入力は「掃引が 0°」で拒む（表示で同じに
    /// 見える値も）。終了角を開始角と同じにするのも同じ。
    #[test]
    fn making_a_partial_arc_zero_sweep_is_refused() {
        let a = arc(FRAC_PI_2, PI);
        for v in [180.0, 180.00001, 540.0, -180.0] {
            assert_eq!(
                edit_number(&a, Field::ArcStart, v),
                Err(ZERO_SWEEP.to_owned()),
                "{v}"
            );
        }
        assert_eq!(
            edit_number(&a, Field::ArcEnd, 90.0),
            Err(ZERO_SWEEP.to_owned())
        );
        // 少しでも違えば通る。
        assert!(edit_number(&a, Field::ArcStart, 179.9)
            .expect("通る")
            .is_some());
    }

    /// 元が 1 周の円弧なら、開始角と終了角が同じでも 1 周のまま受け付ける（両方を同じ値にする）。
    #[test]
    fn a_full_arc_stays_full() {
        let full = arc(0.5, 0.5);
        let shown = display_deg(0.5);
        // 表示と同じ値なら確定しない。
        assert_eq!(edit_number(&full, Field::ArcStart, shown), Ok(None));
        // DXF の 0°→360° の円弧（終了角が 2π）。開始角に 360 を入れても、0° と 360° は同じ角度
        // なので確定しない（Issue #78 の 6。以前は 1 周のまま表現だけが変わる確定になった）。
        let dxf = arc(0.0, TAU);
        assert_eq!(edit_number(&dxf, Field::ArcStart, 360.0), Ok(None));
        // `with_number` を直接呼べば、1 周のまま両方が同じ値になる。
        let Geometry::Arc(a) = with_number(&dxf, Field::ArcStart, 360.0).expect("通る") else {
            unreachable!()
        };
        assert_eq!(a.start_angle, a.end_angle, "両方が同じ値");
        assert_eq!(a.sweep(), TAU, "1 周のまま");
        // 別の値にすれば部分的な円弧になる。
        let Geometry::Arc(a) = edited(&full, Field::ArcStart, 90.0) else {
            unreachable!()
        };
        assert!(a.sweep() < TAU);
    }

    // ---- 角度は 360° 違いを同じ値と見る（Issue #78 の 6） -----------------------

    /// 0° の線分に `360` / `-360` / `720` を打っても確定しない（終点が誤差だけ動いて履歴が増えない）。
    #[test]
    fn a_full_turn_on_a_zero_degree_line_is_not_a_change() {
        let l = Geometry::Line(Line::new(Point2::new(1.0, 2.0), Point2::new(11.0, 2.0)));
        assert_eq!(display_value(&l, Field::LineAngle), Some(0.0), "前提");
        for v in [0.0, 360.0, -360.0, 720.0] {
            assert_eq!(edit_number(&l, Field::LineAngle, v), Ok(None), "{v}");
        }
        // 0° 以外が同じ向きの別の書き方でも同じ。90° の線分に 450・-270。
        let up = Geometry::Line(Line::new(Point2::ORIGIN, Point2::new(0.0, 5.0)));
        for v in [90.0, 450.0, -270.0] {
            assert_eq!(edit_number(&up, Field::LineAngle, v), Ok(None), "{v}");
        }
        // 角度が違えば通る。
        assert!(edit_number(&l, Field::LineAngle, 359.0)
            .expect("通る")
            .is_some());
    }

    /// 回転 270° のインスタンスに `-90` を打っても確定しない。作図線・円弧の角も同じ。
    #[test]
    fn minus_ninety_is_two_hundred_seventy_degrees() {
        let i = instance_rotated(3.0 * FRAC_PI_2);
        assert_eq!(display_value(&i, Field::Rotation), Some(270.0), "前提");
        for v in [270.0, -90.0, 630.0] {
            assert_eq!(edit_number(&i, Field::Rotation, v), Ok(None), "{v}");
        }
        let x =
            Geometry::Xline(Xline::new(Point2::ORIGIN, Vec2::new(0.0, -1.0)).expect("方向がある"));
        assert_eq!(edit_number(&x, Field::XlineAngle, -90.0), Ok(None));
        let a = arc(3.0 * FRAC_PI_2, 0.5);
        assert_eq!(edit_number(&a, Field::ArcStart, -90.0), Ok(None));
    }

    /// 丸めの境目。`359.99995` は表示で `360.0000` = 0° に見えるので、0° と同じ値として確定しない。
    /// 逆に、表示で違って見える `359.9999` は確定する。
    #[test]
    fn rounding_boundaries_of_the_wrapped_angle() {
        let l = Geometry::Line(Line::new(Point2::ORIGIN, Point2::new(10.0, 0.0)));
        for v in [359.999_95, 359.999_99, -0.000_04, 360.000_04] {
            assert_eq!(edit_number(&l, Field::LineAngle, v), Ok(None), "{v}");
        }
        for v in [359.9999, 0.0001, -0.0001] {
            assert!(
                edit_number(&l, Field::LineAngle, v)
                    .expect("通る")
                    .is_some(),
                "{v}"
            );
        }
        // 長さなど、角度でない項目は巻かない（360 は 360）。
        assert!(edit_number(&l, Field::LineLength, 360.0)
            .expect("通る")
            .is_some());
    }

    /// 360° 違いを同じと見るのは確定するかどうかだけ。確定するときの値は打たれたまま
    /// （`-90` を打てば `270°` として書き込まれ、表示は `[0, 360)`）。
    #[test]
    fn a_changed_angle_is_written_as_typed() {
        let i = instance_rotated(0.3);
        let Geometry::Instance(got) = edited(&i, Field::Rotation, -90.0) else {
            unreachable!()
        };
        assert!(eq_angle(got.placement.rotation, (-90f64).to_radians()));
        assert_eq!(
            display_value(&Geometry::Instance(got), Field::Rotation),
            Some(270.0)
        );
    }

    // ---- 表示と同じなら確定しない ----------------------------------------------

    /// 欄に出ている丸めた値をそのまま確定しても、図形は動かない（確定しない）。
    #[test]
    fn the_rounded_value_on_screen_is_not_a_change() {
        let l = Geometry::Line(Line::new(
            Point2::new(1.234_567_89, 2.0),
            Point2::new(10.0, 2.0),
        ));
        let shown: f64 = fmt_num(1.234_567_89).parse().expect("表示は数値");
        assert_eq!(shown, 1.2346, "前提: 表示は 4 桁に丸めている");
        assert_eq!(edit_number(&l, Field::LineStartX, shown), Ok(None));
        // 角度も同じ（表示は度）。
        let a = arc(0.123_456_789, 2.0);
        let shown: f64 = fmt_num(display_deg(0.123_456_789))
            .parse()
            .expect("表示は数値");
        assert_eq!(edit_number(&a, Field::ArcStart, shown), Ok(None));
        // 表示で違って見える値は確定する。
        assert!(edit_number(&l, Field::LineStartX, 1.2347)
            .expect("通る")
            .is_some());
    }

    /// 同じに見えるかは表示の 4 桁で決める。`-0` と `0` も同じ。
    #[test]
    fn same_on_screen_compares_the_four_digit_display() {
        assert!(same_on_screen(1.000_04, 1.0));
        assert!(!same_on_screen(1.000_06, 1.0));
        assert!(same_on_screen(-0.0, 0.0));
        assert!(same_on_screen(-0.000_01, 0.0));
    }
}
