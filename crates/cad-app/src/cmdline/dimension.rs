//! 寸法入力（Issue #20 段階 B）の計算。egui に依存しない純粋関数だけを置く。
//!
//! 点の指定待ちで基準点（`base`）があるとき、次の点を「base からの長さと角度」で
//! 指定できるようにする。どのツールが参加するかは `Tool::dimension_base` で
//! ツール自身が宣言する（ROTATE の数値は角度、SCALE の数値は倍率なので、
//! 一律には効かせられない。ADR-0036）。
//!
//! - **角度は度**、+X 方向から**反時計回り**、**絶対角**（直前の線分からの相対角ではない）
//! - 長さは 0 より大きい有限の数だけ受け付ける。`NaN` や無限大は座標へ流さず
//!   [`DimError`] で止める（座標に入ると図形が消えて原因が追えない）
//!
//! ここにある関数は、ラバーバンド・クリック・`Enter` の 3 経路から同じように使う。
//! 経路ごとに計算を書くと、見えている線と実際に入る点がずれる。

use cad_core::geom::tolerance::is_zero_len;
use cad_core::geom::{Point2, Vec2};

use super::coord;

/// 寸法入力の欄。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Field {
    /// 長さ。数字を打ち始めるとまずここに入る。
    #[default]
    Length,
    /// 角度 [度]。
    Angle,
}

impl Field {
    /// Tab で移る先。2 欄を巡回する。
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::Length => Self::Angle,
            Self::Angle => Self::Length,
        }
    }
}

/// 入力欄のバッファの分類。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BufferKind {
    /// 空（空白だけを含む）。
    Empty,
    /// 数値 1 つとして読める。
    Number(f64),
    /// 数値を打っている途中（`-` や `.` だけ）。欄の表示は続けるが、値としては使わない。
    ///
    /// これを「それ以外」に入れると、`-45` や `.5` を打ち始めた瞬間に欄が消えて
    /// 通常の入力欄に戻り、次の 1 文字でまた欄が出る。入力欄がそのたびに跳ねる。
    Incomplete,
    /// 座標（`,` `@` `<`）やオプション（英字）など、数値以外。
    /// 欄の表示をやめ、従来どおり座標・オプションとして解釈する。
    Other,
}

impl BufferKind {
    /// 寸法入力の欄を出し続けてよい中身か。
    #[must_use]
    pub fn keeps_fields(self) -> bool {
        !matches!(self, Self::Other)
    }
}

/// バッファを分類する。全角の数字・記号は半角として扱う（ADR-0002）。
///
/// **変換中のバッファに対して呼ばないこと。** 未確定文字列が入っている（ADR-0002）。
#[must_use]
pub fn classify(buffer: &str) -> BufferKind {
    let normalized = coord::normalize_ascii(buffer);
    let t = normalized.trim();
    if t.is_empty() {
        return BufferKind::Empty;
    }
    // 数値に使う文字だけでできているかを先に見る。`1e3` のような指数表記は
    // 英字を含むので「それ以外」に回す（オプションの英字と見分けられないため）。
    if !t
        .chars()
        .all(|c| c.is_ascii_digit() || matches!(c, '.' | '+' | '-'))
    {
        return BufferKind::Other;
    }
    coord::parse_number(t).map_or(BufferKind::Incomplete, BufferKind::Number)
}

/// 長さと角度の値。固定した値（錠前）にも、`Enter` で確定する値にも使う。
///
/// `None` の欄は「カーソルから決める」を意味する。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DimValues {
    /// 長さ。
    pub length: Option<f64>,
    /// 角度 [度]。
    pub angle_deg: Option<f64>,
}

impl DimValues {
    /// どちらの欄にも値が無いか。
    #[must_use]
    pub fn is_empty(self) -> bool {
        self.length.is_none() && self.angle_deg.is_none()
    }

    /// 指定の欄の値。
    #[must_use]
    pub fn get(self, field: Field) -> Option<f64> {
        match field {
            Field::Length => self.length,
            Field::Angle => self.angle_deg,
        }
    }

    /// 指定の欄に値を入れたもの。
    #[must_use]
    pub fn with(mut self, field: Field, value: f64) -> Self {
        match field {
            Field::Length => self.length = Some(value),
            Field::Angle => self.angle_deg = Some(value),
        }
        self
    }
}

/// 寸法入力で点を決められなかった理由。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DimError {
    /// カーソルの位置が無いか基点と同じで、向きが決まらない。
    NoDirection,
    /// 長さが 0 以下、または有限でない。
    BadLength,
    /// 角度が有限でない。
    BadAngle,
    /// 角度だけが決まっていて、カーソルがその向きの反対側（または真横）にある。
    BehindAngle,
}

impl DimError {
    /// コマンドラインに出す文。
    #[must_use]
    pub fn message(self) -> &'static str {
        match self {
            Self::NoDirection => {
                "向きが決まりません。カーソルを作図領域の上で基点から離してください"
            }
            Self::BadLength => "長さは 0 より大きい有限の数で指定してください",
            Self::BadAngle => "角度は有限の数（度）で指定してください",
            Self::BehindAngle => {
                "カーソルが指定の角度の反対側にあるため長さが決まりません。長さも指定してください"
            }
        }
    }
}

/// 長さとして使える値か。
fn check_length(length: f64) -> Result<f64, DimError> {
    if length.is_finite() && length > 0.0 && !is_zero_len(length) {
        Ok(length)
    } else {
        Err(DimError::BadLength)
    }
}

/// 角度として使える値か。
fn check_angle(angle_deg: f64) -> Result<f64, DimError> {
    if angle_deg.is_finite() {
        Ok(angle_deg)
    } else {
        Err(DimError::BadAngle)
    }
}

/// 欄を固定（錠前）してよい値か。長さは正、角度は有限であること。
///
/// # Errors
///
/// 長さが 0 以下・有限でない、角度が有限でないとき。
pub fn check_lock(field: Field, value: f64) -> Result<f64, DimError> {
    match field {
        Field::Length => check_length(value),
        Field::Angle => check_angle(value),
    }
}

/// 角度 [度] の向きの単位ベクトル。
///
/// 90° の倍数はちょうどの値にする。`cos(90°)` は浮動小数点では 0 にならず
/// （約 6e-17）、真上に 100 引いたつもりの線の終点 X が基点からわずかにずれる。
fn unit(angle_deg: f64) -> Vec2 {
    let quarter = angle_deg / 90.0;
    if quarter.fract() == 0.0 {
        // ちょうど 0, 1, 2, 3 のどれかになる。
        let q = quarter.rem_euclid(4.0);
        if q < 0.5 {
            Vec2::X
        } else if q < 1.5 {
            Vec2::Y
        } else if q < 2.5 {
            -Vec2::X
        } else {
            -Vec2::Y
        }
    } else {
        Vec2::from_angle(angle_deg.to_radians())
    }
}

/// 長さと角度から点を作る。
///
/// # Errors
///
/// 長さが 0 以下・有限でない、角度が有限でない、結果が有限でないとき。
pub fn point_at(base: Point2, length: f64, angle_deg: f64) -> Result<Point2, DimError> {
    let length = check_length(length)?;
    let angle_deg = check_angle(angle_deg)?;
    let p = base + unit(angle_deg) * length;
    // 巨大な長さで桁あふれすると無限大になる。座標へ流さない。
    if p.x.is_finite() && p.y.is_finite() {
        Ok(p)
    } else {
        Err(DimError::BadLength)
    }
}

/// 基点からカーソルへ向かう単位ベクトル。カーソルが無いか基点と同じなら `None`。
fn direction(base: Point2, cursor: Option<Point2>) -> Option<Vec2> {
    (cursor? - base).normalized()
}

/// 寸法の値（欠けている欄はカーソルから決める）から点を作る。
///
/// - 長さ・角度とも有り … その点
/// - 長さだけ … カーソルの向きにその長さ（直接距離入力と同じ）
/// - 角度だけ … カーソルをその角度の半直線へ正射影した点
/// - どちらも無し … カーソルの位置
///
/// # Errors
///
/// [`DimError`] の各場合。向きや長さが決まらないときに勝手な点を作らない。
pub fn resolve(base: Point2, cursor: Option<Point2>, v: DimValues) -> Result<Point2, DimError> {
    match (v.length, v.angle_deg) {
        (Some(length), Some(angle)) => point_at(base, length, angle),
        (Some(length), None) => {
            let length = check_length(length)?;
            let dir = direction(base, cursor).ok_or(DimError::NoDirection)?;
            point_at(base, length, dir.angle().to_degrees())
        }
        (None, Some(angle)) => {
            let angle = check_angle(angle)?;
            let c = cursor.ok_or(DimError::NoDirection)?;
            let along = (c - base).dot(unit(angle));
            if along > 0.0 && !is_zero_len(along) {
                point_at(base, along, angle)
            } else {
                Err(DimError::BehindAngle)
            }
        }
        (None, None) => cursor.ok_or(DimError::NoDirection),
    }
}

/// 直接距離入力。カーソルの向きに `length` だけ進んだ点。
///
/// # Errors
///
/// 長さが 0 以下・有限でない、カーソルが無いか基点と同じとき。
pub fn direct_distance(
    base: Point2,
    cursor: Option<Point2>,
    length: f64,
) -> Result<Point2, DimError> {
    resolve(
        base,
        cursor,
        DimValues {
            length: Some(length),
            angle_deg: None,
        },
    )
}

/// 固定した値でカーソルを拘束する。ラバーバンドとクリックに使う。
///
/// 決まらないとき（カーソルが基点と同じ、角度の反対側にある）は
/// カーソルをそのまま返す。ラバーバンドを消すより、動いている方が状況が分かる。
/// その位置でクリックしても、ツール側が「同じ点」として断るか、
/// 固定とは関係なくその点が入るだけで、`NaN` は生まれない。
#[must_use]
pub fn constrain(base: Point2, cursor: Point2, locks: DimValues) -> Point2 {
    resolve(base, Some(cursor), locks).unwrap_or(cursor)
}

/// 欄にライブ表示する値（基点 → カーソル）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Live {
    /// 長さ。
    pub length: f64,
    /// 角度 [度]（0 以上 360 未満）。長さが 0 のときは向きが無いので `None`。
    pub angle_deg: Option<f64>,
}

/// 基点からカーソルまでの長さと角度。
#[must_use]
pub fn live(base: Point2, cursor: Point2) -> Live {
    let d = cursor - base;
    Live {
        length: d.len(),
        angle_deg: d.normalized().map(|u| {
            let deg = u.angle().to_degrees().rem_euclid(360.0);
            // rem_euclid は -0.0 付近で 360.0 を返しうる。表示が 360.00 にならないように。
            if deg >= 360.0 {
                0.0
            } else {
                deg
            }
        }),
    }
}

/// 長さの表示（小数 4 桁。ステータスバーの座標と揃える）。
#[must_use]
pub fn format_length(length: f64) -> String {
    format!("{length:.4}")
}

/// 角度の表示（小数 2 桁）。
#[must_use]
pub fn format_angle(angle_deg: f64) -> String {
    format!("{angle_deg:.2}°")
}

/// 寸法入力の欄の状態（入力中の欄と固定した値）。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DimState {
    /// いま入力中の欄。
    pub field: Field,
    /// 固定した値（錠前）。
    pub locks: DimValues,
}

/// `Tab` を押したときの結果。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TabOutcome {
    /// 欄を移った。`consumed` が真ならバッファの値で固定したので、バッファを空にする。
    Moved { consumed: bool },
    /// 欄を出していない（数値以外を打っている）。`Tab` は何もしない。
    Ignored,
    /// 打っている値では固定できない（長さが 0 以下など）。欄もバッファもそのまま。
    Rejected(DimError),
}

impl DimState {
    /// すべての固定を外し、長さの欄へ戻る。
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// `Tab`。バッファが数値なら今の欄をその値で固定し、次の欄へ移る。
    ///
    /// 空なら固定せずに移るだけ（既に固定してある値はそのまま）。
    /// 打ちかけ（`-` だけなど）は値にならないので何もしない。
    pub fn tab(&mut self, buffer: BufferKind) -> TabOutcome {
        let consumed = match buffer {
            BufferKind::Other => return TabOutcome::Ignored,
            BufferKind::Incomplete => return TabOutcome::Rejected(self.incomplete_error()),
            BufferKind::Empty => false,
            BufferKind::Number(v) => match check_lock(self.field, v) {
                Ok(v) => {
                    self.locks = self.locks.with(self.field, v);
                    true
                }
                Err(e) => return TabOutcome::Rejected(e),
            },
        };
        self.field = self.field.next();
        TabOutcome::Moved { consumed }
    }

    fn incomplete_error(self) -> DimError {
        match self.field {
            Field::Length => DimError::BadLength,
            Field::Angle => DimError::BadAngle,
        }
    }

    /// `Esc`。固定か入力があれば全部解除して `true`（中断はしない）。
    /// 何も無ければ `false`（呼び出し側が従来どおり中断する）。
    pub fn escape(&mut self, buffer: BufferKind) -> bool {
        let has_input = !matches!(buffer, BufferKind::Empty);
        if self.locks.is_empty() && !has_input {
            return false;
        }
        self.reset();
        true
    }

    /// `Enter` で確定する値。固定値に、入力中の欄の値を重ねる。
    ///
    /// 固定も入力も無ければ `None`（呼び出し側は従来どおり空 Enter として扱う）。
    /// 数値以外を打っているときも `None`（座標・オプションとして扱う）。
    #[must_use]
    pub fn enter_values(self, buffer: BufferKind) -> Option<DimValues> {
        match buffer {
            BufferKind::Other => None,
            BufferKind::Number(v) => Some(self.locks.with(self.field, v)),
            // 打ちかけは値にならない。固定があればそれで確定し、無ければ
            // 数値として解釈できない文字列として従来の経路に任せる。
            BufferKind::Incomplete | BufferKind::Empty => {
                (!self.locks.is_empty()).then_some(self.locks)
            }
        }
    }

    /// 欄が出ているときに、ラバーバンドとクリックへ効かせる固定値。
    /// 数値以外を打っている間は欄を出していないので、効かせない（見えない拘束を作らない）。
    #[must_use]
    pub fn effective_locks(self, buffer: BufferKind) -> Option<DimValues> {
        (buffer.keeps_fields() && !self.locks.is_empty()).then_some(self.locks)
    }
}

/// 履歴に残す寸法入力の表示（`長さ 100.0000 角度 90.00°`）。カーソルから決めた欄は書かない。
#[must_use]
pub fn describe(v: DimValues) -> String {
    let mut parts = Vec::new();
    if let Some(l) = v.length {
        parts.push(format!("長さ {}", format_length(l)));
    }
    if let Some(a) = v.angle_deg {
        parts.push(format!("角度 {}", format_angle(a)));
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use cad_core::geom::tolerance::eq_len;

    fn p(x: f64, y: f64) -> Point2 {
        Point2::new(x, y)
    }

    fn assert_point(actual: Point2, expected: Point2) {
        assert!(
            eq_len(actual.x, expected.x) && eq_len(actual.y, expected.y),
            "{actual:?} != {expected:?}"
        );
    }

    const BASE: Point2 = Point2 { x: 10.0, y: 20.0 };

    // ---- 長さ・角度から点 ----------------------------------------------------

    #[test]
    fn axis_angles_are_exact() {
        // 90° の倍数はちょうどの値になる（cos(90°) の誤差を持ち込まない）。
        assert_eq!(point_at(BASE, 100.0, 0.0), Ok(p(110.0, 20.0)));
        assert_eq!(point_at(BASE, 100.0, 90.0), Ok(p(10.0, 120.0)));
        assert_eq!(point_at(BASE, 100.0, 180.0), Ok(p(-90.0, 20.0)));
        assert_eq!(point_at(BASE, 100.0, 270.0), Ok(p(10.0, -80.0)));
    }

    #[test]
    fn every_quadrant_is_counter_clockwise_from_plus_x() {
        let r = 2.0_f64.sqrt();
        let cases = [
            (45.0, p(1.0, 1.0)),
            (135.0, p(-1.0, 1.0)),
            (225.0, p(-1.0, -1.0)),
            (315.0, p(1.0, -1.0)),
        ];
        for (deg, dir) in cases {
            let got = point_at(BASE, r, deg).unwrap();
            assert_point(got, p(BASE.x + dir.x, BASE.y + dir.y));
        }
    }

    #[test]
    fn negative_and_over_360_angles_wrap() {
        assert_eq!(point_at(BASE, 100.0, -90.0), Ok(p(10.0, -80.0)));
        assert_eq!(point_at(BASE, 100.0, 450.0), Ok(p(10.0, 120.0)));
        assert_eq!(point_at(BASE, 100.0, -360.0), Ok(p(110.0, 20.0)));
        assert_point(
            point_at(BASE, 100.0, 405.0).unwrap(),
            point_at(BASE, 100.0, 45.0).unwrap(),
        );
        assert_point(
            point_at(BASE, 100.0, -45.0).unwrap(),
            point_at(BASE, 100.0, 315.0).unwrap(),
        );
    }

    #[test]
    fn bad_lengths_and_angles_are_rejected() {
        for len in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(point_at(BASE, len, 0.0), Err(DimError::BadLength), "{len}");
            assert_eq!(
                direct_distance(BASE, Some(p(50.0, 20.0)), len),
                Err(DimError::BadLength),
                "{len}"
            );
        }
        for ang in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(point_at(BASE, 1.0, ang), Err(DimError::BadAngle), "{ang}");
        }
        // 桁あふれで無限大になる長さも座標へ流さない。
        assert_eq!(
            point_at(p(f64::MAX, 0.0), f64::MAX, 0.0),
            Err(DimError::BadLength)
        );
    }

    // ---- 直接距離入力 --------------------------------------------------------

    #[test]
    fn direct_distance_follows_the_cursor_direction() {
        // カーソルの距離は関係なく、向きだけを使う。
        let got = direct_distance(BASE, Some(p(13.0, 24.0)), 50.0).unwrap();
        assert_point(got, p(40.0, 60.0));
        let got = direct_distance(BASE, Some(p(-1000.0, 20.0)), 5.0).unwrap();
        assert_point(got, p(5.0, 20.0));
    }

    #[test]
    fn direct_distance_needs_a_direction() {
        assert_eq!(
            direct_distance(BASE, Some(BASE), 50.0),
            Err(DimError::NoDirection),
            "カーソルが基点と同じ"
        );
        assert_eq!(
            direct_distance(BASE, None, 50.0),
            Err(DimError::NoDirection),
            "カーソルが無い"
        );
    }

    // ---- 固定による拘束 -------------------------------------------------------

    fn locks(length: Option<f64>, angle_deg: Option<f64>) -> DimValues {
        DimValues { length, angle_deg }
    }

    #[test]
    fn length_lock_keeps_the_direction_free() {
        let got = constrain(BASE, p(10.0, 25.0), locks(Some(100.0), None));
        assert_point(got, p(10.0, 120.0));
        let got = constrain(BASE, p(13.0, 24.0), locks(Some(10.0), None));
        assert_point(got, p(16.0, 28.0));
    }

    #[test]
    fn angle_lock_projects_onto_the_ray() {
        // 0° に固定 → Y を捨てて X だけが効く。
        let got = constrain(BASE, p(60.0, 70.0), locks(None, Some(0.0)));
        assert_point(got, p(60.0, 20.0));
        // 45° に固定 → (10,0) 方向の成分のうち 45° 方向の分。
        let got = constrain(BASE, p(20.0, 20.0), locks(None, Some(45.0)));
        assert_point(got, p(15.0, 25.0));
    }

    #[test]
    fn angle_lock_behind_the_ray_does_not_invent_a_point() {
        // 半直線の反対側。Enter では止め、ラバーバンドはカーソルのまま。
        let c = p(0.0, 20.0);
        assert_eq!(
            resolve(BASE, Some(c), locks(None, Some(0.0))),
            Err(DimError::BehindAngle)
        );
        assert_eq!(constrain(BASE, c, locks(None, Some(0.0))), c);
        // 真横（射影の長さ 0）も同じ。
        assert_eq!(
            resolve(BASE, Some(p(10.0, 50.0)), locks(None, Some(0.0))),
            Err(DimError::BehindAngle)
        );
    }

    #[test]
    fn both_locks_fix_the_point_regardless_of_the_cursor() {
        for c in [p(-500.0, 3.0), BASE, p(1.0, 1.0)] {
            assert_eq!(
                constrain(BASE, c, locks(Some(100.0), Some(90.0))),
                p(10.0, 120.0)
            );
        }
        assert_eq!(
            resolve(BASE, None, locks(Some(100.0), Some(90.0))),
            Ok(p(10.0, 120.0)),
            "両方決まっていればカーソルが無くてもよい"
        );
    }

    #[test]
    fn cursor_on_the_base_leaves_the_cursor_alone() {
        assert_eq!(constrain(BASE, BASE, locks(Some(100.0), None)), BASE);
        assert_eq!(constrain(BASE, BASE, locks(None, Some(30.0))), BASE);
        assert_eq!(
            resolve(BASE, Some(BASE), locks(Some(100.0), None)),
            Err(DimError::NoDirection)
        );
    }

    #[test]
    fn no_locks_leave_the_cursor_alone() {
        let c = p(33.0, 44.0);
        assert_eq!(constrain(BASE, c, DimValues::default()), c);
    }

    /// 拘束した点にもう一度拘束をかけても動かない（ラバーバンドの点と
    /// その点でクリックした結果が一致する前提）。
    #[test]
    fn constrain_is_idempotent() {
        for l in [
            locks(Some(40.0), None),
            locks(None, Some(30.0)),
            locks(Some(40.0), Some(30.0)),
        ] {
            let once = constrain(BASE, p(70.0, 90.0), l);
            let twice = constrain(BASE, once, l);
            assert_point(twice, once);
        }
    }

    // ---- ライブ表示 -----------------------------------------------------------

    #[test]
    fn live_values_are_absolute_ccw_degrees() {
        let v = live(BASE, p(10.0, 120.0));
        assert!(eq_len(v.length, 100.0));
        assert!(eq_len(v.angle_deg.unwrap(), 90.0));
        let v = live(BASE, p(10.0, -80.0));
        assert!(eq_len(v.angle_deg.unwrap(), 270.0), "負ではなく 0〜360");
        let v = live(BASE, p(110.0, 20.0));
        assert!(eq_len(v.angle_deg.unwrap(), 0.0));
        assert_eq!(live(BASE, BASE).angle_deg, None, "長さ 0 では向きが無い");
    }

    #[test]
    fn formats_follow_the_status_bar() {
        assert_eq!(format_length(100.0), "100.0000");
        assert_eq!(format_angle(90.0), "90.00°");
        assert_eq!(
            describe(locks(Some(100.0), Some(90.0))),
            "長さ 100.0000 角度 90.00°"
        );
        assert_eq!(describe(locks(Some(5.0), None)), "長さ 5.0000");
    }

    // ---- バッファの分類 ---------------------------------------------------------

    #[test]
    fn buffer_classification() {
        assert_eq!(classify(""), BufferKind::Empty);
        assert_eq!(classify("  "), BufferKind::Empty);
        assert_eq!(classify("100"), BufferKind::Number(100.0));
        assert_eq!(classify(" 12.5 "), BufferKind::Number(12.5));
        assert_eq!(classify("-45"), BufferKind::Number(-45.0));
        assert_eq!(classify("１００"), BufferKind::Number(100.0), "全角");
        assert_eq!(classify("-"), BufferKind::Incomplete);
        assert_eq!(classify("."), BufferKind::Incomplete);
        assert_eq!(classify("100,50"), BufferKind::Other);
        assert_eq!(classify("@10<45"), BufferKind::Other);
        assert_eq!(classify("@"), BufferKind::Other);
        assert_eq!(classify("C"), BufferKind::Other);
        assert_eq!(classify("1e3"), BufferKind::Other, "指数表記は英字扱い");
        assert_eq!(classify("nan"), BufferKind::Other);
        assert_eq!(classify("inf"), BufferKind::Other);
    }

    // ---- 欄の巡回・固定・解除 -----------------------------------------------------

    #[test]
    fn tab_cycles_between_the_two_fields() {
        assert_eq!(Field::Length.next(), Field::Angle);
        assert_eq!(Field::Angle.next(), Field::Length);

        let mut s = DimState::default();
        assert_eq!(s.field, Field::Length, "最初は長さ");
        assert_eq!(
            s.tab(BufferKind::Empty),
            TabOutcome::Moved { consumed: false }
        );
        assert_eq!(s.field, Field::Angle);
        assert!(s.locks.is_empty(), "空の Tab は固定しない");
        s.tab(BufferKind::Empty);
        assert_eq!(s.field, Field::Length, "巡回する");
    }

    #[test]
    fn tab_locks_the_current_field_with_the_typed_number() {
        let mut s = DimState::default();
        assert_eq!(
            s.tab(BufferKind::Number(100.0)),
            TabOutcome::Moved { consumed: true }
        );
        assert_eq!(s.locks, locks(Some(100.0), None));
        assert_eq!(s.field, Field::Angle);
        s.tab(BufferKind::Number(-30.0));
        assert_eq!(s.locks, locks(Some(100.0), Some(-30.0)), "負の角度は可");
        // 固定済みの欄へ戻って打ち直すと上書き。
        s.tab(BufferKind::Number(5.0));
        assert_eq!(s.locks, locks(Some(5.0), Some(-30.0)));
        // 空の Tab では固定は残る。
        s.tab(BufferKind::Empty);
        s.tab(BufferKind::Empty);
        assert_eq!(s.locks, locks(Some(5.0), Some(-30.0)));
    }

    #[test]
    fn tab_rejects_bad_lengths_and_ignores_non_numbers() {
        let mut s = DimState::default();
        for bad in [0.0, -5.0] {
            assert_eq!(
                s.tab(BufferKind::Number(bad)),
                TabOutcome::Rejected(DimError::BadLength)
            );
            assert_eq!(s, DimState::default(), "欄も固定も変わらない");
        }
        assert_eq!(
            s.tab(BufferKind::Incomplete),
            TabOutcome::Rejected(DimError::BadLength)
        );
        assert_eq!(s.tab(BufferKind::Other), TabOutcome::Ignored);
        assert_eq!(s, DimState::default());
    }

    #[test]
    fn escape_clears_locks_and_input_before_cancelling() {
        let mut s = DimState::default();
        assert!(!s.escape(BufferKind::Empty), "何も無ければ中断に回す");

        s.tab(BufferKind::Number(100.0));
        assert!(s.escape(BufferKind::Empty), "固定があれば解除");
        assert_eq!(s, DimState::default());
        assert!(!s.escape(BufferKind::Empty), "2 回目で中断");

        assert!(s.escape(BufferKind::Number(3.0)), "入力だけでも解除");
        assert!(s.escape(BufferKind::Other), "数値以外の入力でも解除");
    }

    #[test]
    fn enter_combines_locks_with_the_typed_value() {
        let mut s = DimState::default();
        assert_eq!(
            s.enter_values(BufferKind::Empty),
            None,
            "空 Enter は従来どおり"
        );
        assert_eq!(
            s.enter_values(BufferKind::Number(100.0)),
            Some(locks(Some(100.0), None)),
            "長さだけ = 直接距離入力"
        );
        assert_eq!(s.enter_values(BufferKind::Other), None);

        s.tab(BufferKind::Number(100.0));
        assert_eq!(
            s.enter_values(BufferKind::Number(90.0)),
            Some(locks(Some(100.0), Some(90.0)))
        );
        assert_eq!(
            s.enter_values(BufferKind::Empty),
            Some(locks(Some(100.0), None)),
            "入力が無ければ固定値だけ（角度はカーソルから）"
        );
        assert_eq!(s.enter_values(BufferKind::Other), None, "座標を打てば座標");
    }

    #[test]
    fn locks_take_effect_only_while_the_fields_are_shown() {
        let mut s = DimState::default();
        assert_eq!(s.effective_locks(BufferKind::Empty), None, "固定が無い");
        s.tab(BufferKind::Number(100.0));
        assert_eq!(
            s.effective_locks(BufferKind::Empty),
            Some(locks(Some(100.0), None))
        );
        assert_eq!(
            s.effective_locks(BufferKind::Number(1.0)),
            Some(locks(Some(100.0), None))
        );
        assert_eq!(s.effective_locks(BufferKind::Other), None);
    }
}
