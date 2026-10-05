//! 直交モード（F8）と極トラッキング（F10）。Issue #29、ADR-0038。
//!
//! どちらも「基準点からの向き」でカーソル点を拘束する作図補助。
//! 拘束はカーソル点を 1 か所で決める経路（`CadApp::canvas` のラバーバンドと
//! `CadApp::place_point` のクリック）に載せ、両方が [`track_cursor`] を通す。
//! 経路ごとに計算を書くと、見えている線の先とクリックで入る点がずれる（ADR-0036 決定 4 と同じ）。
//!
//! # 優先順位（ADR-0038）
//!
//! 寸法入力の固定 > スナップ吸着 > 直交 > 極。
//!
//! - **寸法入力の固定**は直交・極の後段（`Session::constrain` / `rubber_band`）でかかる。
//!   角度を固定しているときは向きが決まっているので、直交・極はかけない
//! - **スナップに吸着している**ときは吸着点をそのまま使う（AutoCAD と同じ）。
//!   交点や端点に吸い付かないと、直交を入れたまま既存の図形へつなげなくなる
//! - **直交と極は排他にしない。** 両方オンなら直交が勝ち、極の補助線は出さない
//!
//! 状態（オン/オフ）は `cad-app` 側の UI 状態なので `Document` には入れず、保存もしない。
//!
//! 前半は egui に依存しない純粋関数（単体テストで固定）、後半がキーとステータスバー。

use cad_core::geom::Point2;

use crate::cmdline::dimension;
use crate::session::Session;
use crate::viewport::Viewport;

/// 極トラッキングの角度の刻み [度]。AutoCAD の既定の 1 つ。今回は設定で変えない（ADR-0038）。
pub const POLAR_STEP_DEG: f64 = 15.0;

/// 極トラッキングが吸い付く距離 [px]。カーソルから刻み角度の半直線までの画面上の距離。
///
/// スナップの吸着半径（`snap.rs` の `ACQUIRE_RADIUS_PX`）と揃える。
/// 角度の差（度）で決めると、基準点から遠いほど画面上で大きく吸い寄せられて、
/// 狙った位置から指が離れたように感じる。
pub const POLAR_SNAP_PX: f32 = 10.0;

/// 作図補助の種類。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// 直交モード（F8）。
    Ortho,
    /// 極トラッキング（F10）。
    Polar,
}

impl Mode {
    /// 両方。ステータスバーとキーの処理はこの順。
    pub const ALL: [Self; 2] = [Self::Ortho, Self::Polar];

    /// 切り替えのキー（AutoCAD と同じ）。
    #[must_use]
    pub fn key(self) -> egui::Key {
        match self {
            Self::Ortho => egui::Key::F8,
            Self::Polar => egui::Key::F10,
        }
    }

    /// ステータスバーの表示（オンのとき）。オフは小文字にする（`OSNAP` / `DYN` と同じ）。
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Ortho => "ORTHO",
            Self::Polar => "POLAR",
        }
    }

    /// 履歴に出す名前。
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Ortho => "直交モード",
            Self::Polar => "極トラッキング",
        }
    }
}

/// 直交モードと極トラッキングのオン/オフ。
///
/// 2 つは**独立したトグル**。片方を切り替えても他方は変えない
/// （AutoCAD の「F8 で F10 が勝手に切れる」を採らない。ADR-0038）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Drafting {
    ortho: bool,
    polar: bool,
}

impl Drafting {
    /// 初期状態（どちらもオフ）。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 指定の補助がオンか。
    #[must_use]
    pub fn is_on(self, mode: Mode) -> bool {
        match mode {
            Mode::Ortho => self.ortho,
            Mode::Polar => self.polar,
        }
    }

    /// 極トラッキングがオンでも、直交が優先されて効いていないか。
    #[must_use]
    pub fn polar_overridden(self) -> bool {
        self.ortho && self.polar
    }

    /// 指定の補助だけを切り替え、履歴に出す文を返す。
    pub fn toggle(&mut self, mode: Mode) -> String {
        let flag = match mode {
            Mode::Ortho => &mut self.ortho,
            Mode::Polar => &mut self.polar,
        };
        *flag = !*flag;
        let state = if *flag { "ON" } else { "OFF" };
        let note = if self.polar_overridden() {
            "（両方オンの間は直交が優先されます）"
        } else {
            ""
        };
        format!("{}: {state}{note}", mode.name())
    }

    /// カーソル点を拘束する。egui に依存しない拘束の本体。
    ///
    /// - `base` … 基準点（[`Session::tracking_base`]）。無ければ拘束しない
    /// - `snapped` … スナップに吸着しているか。していれば拘束しない（スナップが優先）
    /// - `angle_locked` … 寸法入力で角度を固定しているか。していれば拘束しない（固定が優先）
    /// - `polar_tolerance` … 極が吸い付く距離（モデル座標）
    #[must_use]
    pub fn track(self, input: TrackInput) -> Tracked {
        let free = Tracked {
            point: input.cursor,
            polar: None,
        };
        let Some(base) = input.base else {
            return free;
        };
        if input.snapped || input.angle_locked {
            return free;
        }
        if self.ortho {
            return Tracked {
                point: ortho(base, input.cursor),
                polar: None,
            };
        }
        if self.polar {
            if let Some(hit) = polar(base, input.cursor, POLAR_STEP_DEG, input.polar_tolerance) {
                return Tracked {
                    point: hit.point,
                    polar: Some(hit),
                };
            }
        }
        free
    }
}

/// [`Drafting::track`] の入力。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackInput {
    /// 基準点。
    pub base: Option<Point2>,
    /// カーソル（スナップ後。吸着していればその点）。
    pub cursor: Point2,
    /// スナップに吸着しているか。
    pub snapped: bool,
    /// 寸法入力で角度を固定しているか。
    pub angle_locked: bool,
    /// 極が吸い付く距離（モデル座標）。
    pub polar_tolerance: f64,
}

/// 拘束の結果。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tracked {
    /// 拘束後の点。
    pub point: Point2,
    /// 極トラッキングで吸い付いたときだけ、その角度（補助線と角度の表示に使う）。
    pub polar: Option<PolarHit>,
}

/// 極トラッキングで刻み角度に吸い付いた結果。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PolarHit {
    /// 基準点（補助線の起点）。
    pub base: Point2,
    /// 吸い付いた点（カーソルを半直線へ正射影した点）。
    pub point: Point2,
    /// 半直線の角度 [度]。+X から反時計回り、0 以上 360 未満。
    pub angle_deg: f64,
}

impl PolarHit {
    /// カーソル近くに出す角度の表示（`45°`）。
    #[must_use]
    pub fn label(self) -> String {
        format!("{:.0}°", self.angle_deg)
    }
}

/// 直交の射影。基準点を通る水平線と垂直線のうち、カーソルに近い方へ射影する。
///
/// 水平・垂直の差がちょうど同じ（45° の方向）なら**水平**にする。
/// 境目で毎フレーム入れ替わらないよう、どちらかに決め打ちしておく。
/// カーソルが基準点と同じなら基準点を返す（向きが無いが、点としては決まる）。
#[must_use]
pub fn ortho(base: Point2, cursor: Point2) -> Point2 {
    let d = cursor - base;
    if d.x.abs() >= d.y.abs() {
        Point2::new(cursor.x, base.y)
    } else {
        Point2::new(base.x, cursor.y)
    }
}

/// 角度 [度] を 0 以上 360 未満にそろえる。`-0.0` は `0.0` にする（表示が `-0°` にならないように）。
#[must_use]
pub fn normalize_deg(angle_deg: f64) -> f64 {
    let a = angle_deg.rem_euclid(360.0) + 0.0;
    // rem_euclid は -0.0 付近で 360.0 を返しうる。
    if a >= 360.0 {
        0.0
    } else {
        a
    }
}

/// 極トラッキング。カーソルの向きに最も近い `step_deg` の倍数の半直線を選び、
/// カーソルからその半直線までの距離が `tolerance` 以内なら、カーソルを半直線へ正射影した点を返す。
///
/// 遠ければ `None`（カーソルのまま）。カーソルが基準点と同じで向きが無いときも `None`。
/// 刻みは 0 より大きく 180 未満を想定する（最も近い倍数との差が 90° 未満になり、
/// 射影が基準点の反対側へ出ない）。
#[must_use]
pub fn polar(base: Point2, cursor: Point2, step_deg: f64, tolerance: f64) -> Option<PolarHit> {
    let d = cursor - base;
    let dir = d.normalized()?;
    let angle = dir.angle().to_degrees();
    let snapped = (angle / step_deg).round() * step_deg;
    let diff = (angle - snapped).to_radians();
    let len = d.len();
    // 半直線までの距離と、半直線に沿った長さ。
    let off = len * diff.sin().abs();
    if off > tolerance {
        return None;
    }
    let angle_deg = normalize_deg(snapped);
    // 90° の倍数はちょうどの単位ベクトルで作る（寸法入力と同じ `point_at`）。
    let point = dimension::point_at(base, len * diff.cos(), angle_deg).ok()?;
    Some(PolarHit {
        base,
        point,
        angle_deg,
    })
}

/// キャンバスのカーソル（スナップ前のモデル座標）から、ラバーバンドとクリックに使う点を決める。
///
/// `snapped` はスナップの吸着点。吸着していればそれを返す。
/// ラバーバンド（`canvas`）とクリック（`place_point`）の両方がここを通る。
/// この後に寸法入力の固定（`Session::rubber_band` / `constrain`）がかかる。
#[must_use]
pub fn track_cursor(
    drafting: Drafting,
    session: &Session,
    viewport: &Viewport,
    snapped: Option<Point2>,
    raw: Point2,
) -> Tracked {
    let angle_locked = session.dimension_base().is_some()
        && session
            .cmdline
            .dimension_locks()
            .is_some_and(|l| l.angle_deg.is_some());
    drafting.track(TrackInput {
        base: session.tracking_base(),
        cursor: snapped.unwrap_or(raw),
        snapped: snapped.is_some(),
        angle_locked,
        polar_tolerance: viewport.px_to_model_len(POLAR_SNAP_PX),
    })
}

// ---- キーとステータスバー（egui） ------------------------------------------

/// このフレームで押された切り替えキー（F8 / F10）を取る。
///
/// F3 / F12 と同じく `TextEdit` はファンクションキーを消費しないので、キャンバスで拾ってよい。
pub fn take_key_toggles(ui: &egui::Ui) -> Vec<Mode> {
    Mode::ALL
        .into_iter()
        .filter(|m| ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, m.key())))
        .collect()
}

/// オンの色（`OSNAP` / `DYN` と同じ）。
const ON_COLOR: egui::Color32 = egui::Color32::from_rgb(0xc6, 0xff, 0x00);
/// オンだが直交に負けて効いていない極の色。オンとオフの中間に見せる。
const OVERRIDDEN_COLOR: egui::Color32 = egui::Color32::from_rgb(0x8a, 0x9a, 0x5b);

/// ステータスバーに `ORTHO` / `POLAR` を描く（`OSNAP` / `DYN` と同じ見せ方）。
/// クリックされた補助を返す。区切り線は各項目の後ろに描く。
pub fn status_toggles(ui: &mut egui::Ui, drafting: Drafting) -> Option<Mode> {
    let mut clicked = None;
    for mode in Mode::ALL {
        let on = drafting.is_on(mode);
        let overridden = mode == Mode::Polar && drafting.polar_overridden();
        let text = if on {
            let color = if overridden {
                OVERRIDDEN_COLOR
            } else {
                ON_COLOR
            };
            egui::RichText::new(mode.label()).monospace().color(color)
        } else {
            egui::RichText::new(mode.label().to_lowercase())
                .monospace()
                .color(ui.visuals().weak_text_color())
        };
        let mut tooltip = match mode {
            Mode::Ortho => "直交モード（水平・垂直に拘束）の ON/OFF  F8".to_owned(),
            Mode::Polar => {
                format!("極トラッキング（{POLAR_STEP_DEG:.0}° 刻みに吸い付く）の ON/OFF  F10")
            }
        };
        if overridden {
            tooltip.push_str("\n直交モードがオンの間は直交が優先されます");
        }
        // クリックで切り替える部品なので、選択を切って指のカーソルにする。
        let response = ui
            .add(
                egui::Label::new(text)
                    .selectable(false)
                    .sense(egui::Sense::click()),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(tooltip);
        if response.clicked() {
            clicked = Some(mode);
        }
        ui.separator();
    }
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;
    use cad_core::geom::tolerance::eq_len;

    fn assert_point(actual: Point2, expected: Point2) {
        assert!(
            eq_len(actual.x, expected.x) && eq_len(actual.y, expected.y),
            "{actual:?} != {expected:?}"
        );
    }

    fn at_deg(base: Point2, deg: f64, len: f64) -> Point2 {
        base + cad_core::geom::Vec2::from_angle(deg.to_radians()) * len
    }

    const BASE: Point2 = Point2 { x: 10.0, y: 20.0 };

    // ---- 直交 ----

    /// 4 象限とも、水平・垂直のうちカーソルに近い方へ射影する。
    #[test]
    fn ortho_projects_to_the_nearer_axis_in_all_quadrants() {
        for (dx, dy, expected) in [
            (30.0, 10.0, Point2::new(40.0, 20.0)),   // 第 1 象限・水平寄り
            (10.0, 30.0, Point2::new(10.0, 50.0)),   // 第 1 象限・垂直寄り
            (-30.0, 10.0, Point2::new(-20.0, 20.0)), // 第 2 象限・水平寄り
            (-10.0, 30.0, Point2::new(10.0, 50.0)),  // 第 2 象限・垂直寄り
            (-30.0, -10.0, Point2::new(-20.0, 20.0)),
            (-10.0, -30.0, Point2::new(10.0, -10.0)),
            (30.0, -10.0, Point2::new(40.0, 20.0)),
            (10.0, -30.0, Point2::new(10.0, -10.0)),
        ] {
            let c = Point2::new(BASE.x + dx, BASE.y + dy);
            assert_eq!(ortho(BASE, c), expected, "({dx}, {dy})");
        }
    }

    /// ちょうど 45° の方向（水平・垂直の差が同じ）は水平に決める。4 象限とも。
    #[test]
    fn ortho_at_exactly_45_degrees_picks_horizontal() {
        for (dx, dy) in [(25.0, 25.0), (-25.0, 25.0), (-25.0, -25.0), (25.0, -25.0)] {
            let c = Point2::new(BASE.x + dx, BASE.y + dy);
            assert_eq!(ortho(BASE, c), Point2::new(c.x, BASE.y), "({dx}, {dy})");
        }
    }

    /// カーソルが基準点と同じなら基準点。NaN にならない。
    #[test]
    fn ortho_at_the_base_returns_the_base() {
        assert_eq!(ortho(BASE, BASE), BASE);
    }

    // ---- 極 ----

    /// 刻みの倍数から少しずれた向きは、その倍数の半直線へ射影される。
    #[test]
    fn polar_snaps_to_the_nearest_multiple_within_the_tolerance() {
        // 100 離れた 44° の点は 45° の半直線から 100·sin1° ≈ 1.75 の距離。
        let c = at_deg(BASE, 44.0, 100.0);
        let hit = polar(BASE, c, 15.0, 2.0).expect("吸い付く");
        assert_eq!(hit.angle_deg, 45.0);
        assert_eq!(hit.base, BASE);
        assert_point(
            hit.point,
            at_deg(BASE, 45.0, 100.0 * 1.0_f64.to_radians().cos()),
        );
        assert_eq!(hit.label(), "45°");
    }

    /// 閾値の内外。距離がちょうど閾値なら吸い付き、わずかに超えたら吸い付かない。
    #[test]
    fn polar_respects_the_tolerance_boundary() {
        let c = at_deg(BASE, 40.0, 100.0); // 45° から 5°。半直線までの距離 ≈ 8.716
        let off = 100.0 * 5.0_f64.to_radians().sin();
        assert!(
            polar(BASE, c, 15.0, off * 1.001).is_some(),
            "閾値のすぐ内側"
        );
        assert!(
            polar(BASE, c, 15.0, off * 0.999).is_none(),
            "閾値のすぐ外側"
        );
        assert!(polar(BASE, c, 15.0, 1.0).is_none(), "遠いとカーソルのまま");
    }

    /// 刻みのちょうど中間（7.5° ずれ）でも閾値が大きければどちらかに決まり、点は半直線上。
    #[test]
    fn polar_point_lies_on_the_ray() {
        for deg in [3.0, 17.0, 88.0, 101.0, 179.0, 200.0, 271.0, 359.0] {
            let c = at_deg(BASE, deg, 50.0);
            let hit = polar(BASE, c, 15.0, 100.0).expect("閾値が大きいので必ず吸い付く");
            let v = hit.point - BASE;
            let ray = cad_core::geom::Vec2::from_angle(hit.angle_deg.to_radians());
            assert!(eq_len(v.cross(ray), 0.0), "{deg}°: 半直線上");
            assert!(v.dot(ray) > 0.0, "{deg}°: 基準点の反対側へ出ない");
            assert!(
                (hit.angle_deg / 15.0).fract() == 0.0,
                "{deg}°: 刻みの倍数 {}",
                hit.angle_deg
            );
        }
    }

    /// 負の角度（+X より下）は 0 以上 360 未満で表す。-45° は 315°。
    #[test]
    fn polar_negative_angles_are_normalized() {
        let c = at_deg(BASE, -44.0, 100.0);
        let hit = polar(BASE, c, 15.0, 5.0).expect("吸い付く");
        assert_eq!(hit.angle_deg, 315.0);
        assert_eq!(hit.label(), "315°");
    }

    /// 360° 付近（359° と 1°）は 0° に吸い付き、`360°` や `-0°` にならない。
    #[test]
    fn polar_near_360_wraps_to_zero() {
        for deg in [359.0, 1.0, -0.5] {
            let c = at_deg(BASE, deg, 100.0);
            let hit = polar(BASE, c, 15.0, 5.0).expect("吸い付く");
            assert_eq!(hit.angle_deg, 0.0, "{deg}°");
            assert_eq!(hit.label(), "0°", "{deg}°");
            // 0° はちょうど水平（寸法入力の `point_at` と同じく 90° の倍数はちょうど）。
            assert_eq!(hit.point.y, BASE.y, "{deg}°");
        }
    }

    /// カーソルが基準点と同じなら向きが無いので吸い付かない。
    #[test]
    fn polar_at_the_base_does_nothing() {
        assert_eq!(polar(BASE, BASE, 15.0, 100.0), None);
    }

    #[test]
    fn normalize_deg_maps_into_0_to_360() {
        assert_eq!(normalize_deg(-45.0), 315.0);
        assert_eq!(normalize_deg(360.0), 0.0);
        assert_eq!(normalize_deg(720.0 + 15.0), 15.0);
        assert!(normalize_deg(-0.0).is_sign_positive(), "-0.0 を返さない");
    }

    // ---- 優先順位と状態 ----

    fn input(cursor: Point2) -> TrackInput {
        TrackInput {
            base: Some(BASE),
            cursor,
            snapped: false,
            angle_locked: false,
            polar_tolerance: 20.0,
        }
    }

    fn with(ortho: bool, polar: bool) -> Drafting {
        Drafting { ortho, polar }
    }

    /// 両方オンなら直交が勝ち、極の補助線（`polar`）は出さない。
    #[test]
    fn ortho_wins_over_polar() {
        let c = at_deg(BASE, 40.0, 100.0);
        let t = with(true, true).track(input(c));
        assert_eq!(t.point, ortho(BASE, c));
        assert_eq!(t.polar, None);

        let t = with(false, true).track(input(c));
        assert_eq!(
            t.polar.map(|h| h.angle_deg),
            Some(45.0),
            "極だけなら吸い付く"
        );
    }

    /// スナップに吸着しているとき・角度を固定しているときは拘束しない。
    #[test]
    fn snap_and_angle_lock_win_over_ortho_and_polar() {
        let c = at_deg(BASE, 40.0, 100.0);
        for d in [with(true, false), with(false, true), with(true, true)] {
            let snapped = TrackInput {
                snapped: true,
                ..input(c)
            };
            assert_eq!(
                d.track(snapped),
                Tracked {
                    point: c,
                    polar: None
                },
                "{d:?}"
            );
            let locked = TrackInput {
                angle_locked: true,
                ..input(c)
            };
            assert_eq!(
                d.track(locked),
                Tracked {
                    point: c,
                    polar: None
                },
                "{d:?}"
            );
        }
    }

    /// 基準点が無ければ（LINE の 1 点目など）拘束しない。
    #[test]
    fn nothing_happens_without_a_base() {
        let c = at_deg(BASE, 40.0, 100.0);
        let t = with(true, true).track(TrackInput {
            base: None,
            ..input(c)
        });
        assert_eq!(
            t,
            Tracked {
                point: c,
                polar: None
            }
        );
    }

    /// 片方を切り替えても他方は変わらない（排他にしない）。
    #[test]
    fn toggles_are_independent() {
        let mut d = Drafting::new();
        assert_eq!(d, with(false, false), "起動時はどちらもオフ");
        assert_eq!(d.toggle(Mode::Polar), "極トラッキング: ON");
        let msg = d.toggle(Mode::Ortho);
        assert!(msg.starts_with("直交モード: ON"), "{msg}");
        assert!(
            msg.contains("直交が優先"),
            "両方オンになったら案内する: {msg}"
        );
        assert!(d.polar_overridden());
        assert_eq!(d.toggle(Mode::Ortho), "直交モード: OFF");
        assert!(d.is_on(Mode::Polar), "F8 を切っても POLAR は残る");
        d.toggle(Mode::Ortho);
        d.toggle(Mode::Polar);
        assert!(d.is_on(Mode::Ortho), "F10 を切っても ORTHO は残る");
        assert!(!d.is_on(Mode::Polar));
    }
}
