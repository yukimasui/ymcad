//! モデル空間 ↔ スクリーン空間の変換。
//!
//! # このモジュールの位置づけ
//!
//! **プログラム全体で `f64` → `f32` の縮小変換を行ってよいのはここだけ。**
//! 図面座標はすべて `f64`（`cad-core` は f32 を一切持たない）で、`f32` になるのは
//! egui の [`egui::Pos2`] へ渡す直前だけ。各所で個別にスケール計算をしないこと。
//! 順変換と逆変換は必ずペアで使う。
//!
//! # 変換の表現
//!
//! アフィン行列ではなく **相似変換（モデル空間の中心 + 一様スケール）** で保持する。
//!
//! - スカラ 3 個で済み、逆変換が閉じた形で厳密に求まる。行列式の逆数を取らないので
//!   極端なズームでも破綻しない。
//! - 画面上で一定に保ちたい量（スナップの拾い半径、線幅、破線ピッチ、円弧の分割精度、
//!   グリッド間隔）はすべてスカラ `scale` の関数として書ける。行列だと毎フレーム
//!   スケールを抽出し直すことになり、非直交成分が混ざると崩れる。
//! - 平行移動をスクリーン空間ではなく **モデル空間の点** `center` として持つことで、
//!   1e6 のような大きな値を f64 側に留められる。
//!
//! # 精度を守るための鉄則
//!
//! `model_to_screen` では **「f64 で引き算 → f64 で掛け算 → 最後に一度だけ f32 へ」**
//! の順を守る。`(p.x - center.x)` の結果は画面の見えている範囲、すなわち
//! `rect.width() / scale` で抑えられるので、f32 に落としても誤差は 1e-4 px 程度で見えない。
//! 逆に先に f32 化してから引き算すると、`center` が 1e6 のとき有効数字が
//! 引き算の前に 7 桁失われ、完全に破綻する。

use cad_core::geom::{Aabb, Point2, Vec2};

/// スケールの下限。これ以上引くと図面全体が 1px 未満になる。
const MIN_SCALE: f64 = 1e-9;
/// スケールの上限。
const MAX_SCALE: f64 = 1e9;

/// モデル空間とスクリーン空間の対応づけ。
#[derive(Clone, Copy, Debug)]
pub struct Viewport {
    /// `rect` の中心に表示されるモデル座標。
    center: Point2,
    /// モデル単位あたりのスクリーン points 数。常に正。
    scale: f64,
    /// 描画対象のスクリーン矩形。
    rect: egui::Rect,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            center: Point2::ORIGIN,
            scale: 1.0,
            rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1.0, 1.0)),
        }
    }
}

impl Viewport {
    /// 描画対象の矩形を更新する。毎フレーム `ui.max_rect()` などで呼ぶ。
    pub fn set_rect(&mut self, rect: egui::Rect) {
        self.rect = rect;
    }

    /// 現在のスクリーン矩形。
    #[must_use]
    pub fn rect(&self) -> egui::Rect {
        self.rect
    }

    /// モデル単位あたりのスクリーン points 数。
    #[must_use]
    pub fn scale(&self) -> f64 {
        self.scale
    }

    /// 画面中心のモデル座標。
    #[must_use]
    #[allow(dead_code, reason = "Phase 3 の作図・描画で使う")]
    pub fn center(&self) -> Point2 {
        self.center
    }

    // ---- 変換 -------------------------------------------------------------
    //
    // 以下の 2 つが f64 → f32 の縮小変換を行う唯一の場所。

    /// モデル座標 → スクリーン座標。
    ///
    /// モデル空間は +Y が上、スクリーン空間は +Y が下なので Y を反転する。
    #[inline]
    #[must_use]
    #[allow(
        clippy::cast_possible_truncation,
        reason = "f64→f32 の縮小はこの関数の役目。引き算と掛け算を f64 で終えた後に一度だけ行う"
    )]
    pub fn model_to_screen(&self, p: Point2) -> egui::Pos2 {
        let c = self.rect.center();
        egui::pos2(
            (f64::from(c.x) + (p.x - self.center.x) * self.scale) as f32,
            (f64::from(c.y) - (p.y - self.center.y) * self.scale) as f32,
        )
    }

    /// モデル空間のベクトル → スクリーン空間のベクトル（平行移動を含まない）。
    #[inline]
    #[must_use]
    #[allow(
        clippy::cast_possible_truncation,
        reason = "f64→f32 の縮小はこの関数の役目"
    )]
    #[allow(dead_code, reason = "Phase 3 の作図・描画で使う")]
    pub fn model_to_screen_vec(&self, v: Vec2) -> egui::Vec2 {
        egui::vec2((v.x * self.scale) as f32, (-v.y * self.scale) as f32)
    }

    /// スクリーン座標 → モデル座標。f32 → f64 の拡大なので情報は失われない。
    #[inline]
    #[must_use]
    pub fn screen_to_model(&self, s: egui::Pos2) -> Point2 {
        let c = self.rect.center();
        Point2::new(
            self.center.x + (f64::from(s.x) - f64::from(c.x)) / self.scale,
            self.center.y - (f64::from(s.y) - f64::from(c.y)) / self.scale,
        )
    }

    /// スクリーン空間のベクトル → モデル空間のベクトル。
    #[inline]
    #[must_use]
    pub fn screen_to_model_vec(&self, v: egui::Vec2) -> Vec2 {
        Vec2::new(f64::from(v.x) / self.scale, -f64::from(v.y) / self.scale)
    }

    /// モデル空間の長さ → スクリーン上の px 数。
    #[inline]
    #[must_use]
    #[allow(
        clippy::cast_possible_truncation,
        reason = "f64→f32 の縮小はこのモジュールの役目"
    )]
    pub fn model_len_to_px(&self, len: f64) -> f32 {
        (len * self.scale) as f32
    }

    /// スクリーン上の px 数 → モデル空間の長さ。
    ///
    /// スナップの拾い半径や円弧の分割精度など、「画面上で一定に見せたい量」を
    /// モデル空間へ持ち込むのに使う。
    #[inline]
    #[must_use]
    pub fn px_to_model_len(&self, px: f32) -> f64 {
        f64::from(px) / self.scale
    }

    // ---- ビュー操作 -------------------------------------------------------

    /// 現在見えているモデル空間の範囲。描画前のカリングに使う。
    #[must_use]
    pub fn visible_model_rect(&self) -> Aabb {
        Aabb::new(
            self.screen_to_model(self.rect.min),
            self.screen_to_model(self.rect.max),
        )
    }

    /// スクリーン上の移動量ぶんだけ図面をずらす。
    pub fn pan_px(&mut self, delta: egui::Vec2) {
        let d = self.screen_to_model_vec(delta);
        self.center -= d;
    }

    /// `anchor` のスクリーン位置にあるモデル座標を固定したままズームする。
    ///
    /// ホイールズームでカーソル直下の点が動かないことが受け入れ基準なので、
    /// 「ズーム前後で `anchor` のモデル座標が一致する」ように `center` を解いている。
    pub fn zoom_about(&mut self, anchor: egui::Pos2, factor: f64) {
        if !factor.is_finite() || factor <= 0.0 {
            return;
        }
        let before = self.screen_to_model(anchor);
        self.scale = (self.scale * factor).clamp(MIN_SCALE, MAX_SCALE);
        let after = self.screen_to_model(anchor);
        // アンカーのモデル座標のズレを打ち消すぶんだけ中心を動かす。
        self.center += before - after;
    }

    /// 指定した範囲が収まるように中心とスケールを設定する。
    ///
    /// `margin_frac` は上下左右に取る余白の割合（0.05 なら 5%）。
    pub fn zoom_to_fit(&mut self, bounds: Aabb, margin_frac: f64) {
        if bounds.is_empty() {
            return;
        }
        self.center = bounds.center();

        let w = f64::from(self.rect.width());
        let h = f64::from(self.rect.height());
        if w <= 0.0 || h <= 0.0 {
            return;
        }

        let size = bounds.size();
        let pad = (1.0 - margin_frac * 2.0).max(0.1);

        // 幅・高さが 0 の範囲（点、水平線、垂直線）でもゼロ除算しないよう、
        // 有効な軸のスケールだけを採用する。
        let sx = if size.x > 0.0 {
            Some(w * pad / size.x)
        } else {
            None
        };
        let sy = if size.y > 0.0 {
            Some(h * pad / size.y)
        } else {
            None
        };

        self.scale = match (sx, sy) {
            (Some(a), Some(b)) => a.min(b),
            (Some(a), None) => a,
            (None, Some(b)) => b,
            // 範囲が 1 点だけの場合は倍率を変えずに中心だけ合わせる。
            (None, None) => self.scale,
        }
        .clamp(MIN_SCALE, MAX_SCALE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vp(center: Point2, scale: f64) -> Viewport {
        let mut v = Viewport {
            center,
            scale,
            ..Default::default()
        };
        v.set_rect(egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(800.0, 600.0),
        ));
        v
    }

    /// ズーム倍率とオフセットを振っても、変換 → 逆変換で 1px 以内に戻ること。
    /// 指示書が要求する 1e-6〜1e6 の全域を確認する。
    #[test]
    fn roundtrip_across_full_zoom_range() {
        for exp in -6..=6 {
            let scale = 10f64.powi(exp);
            for center in [
                Point2::ORIGIN,
                Point2::new(1e6, -1e6),
                Point2::new(1e-6, 1e-6),
            ] {
                let v = vp(center, scale);
                // 画面内に収まるいくつかのモデル点で検査する。
                for (dx, dy) in [(0.0, 0.0), (100.0, -50.0), (-399.0, 299.0)] {
                    let p = Point2::new(center.x + dx / scale, center.y + dy / scale);
                    let back = v.screen_to_model(v.model_to_screen(p));
                    let err = back.dist(p);
                    let one_px = v.px_to_model_len(1.0);
                    assert!(
                        err <= one_px,
                        "scale=1e{exp} center=({}, {}): 誤差 {err:e} が 1px {one_px:e} を超えた",
                        center.x,
                        center.y
                    );
                }
            }
        }
    }

    /// ホイールズームでカーソル直下のモデル座標が動かないこと（Phase 2 の受け入れ基準）。
    #[test]
    fn zoom_about_keeps_anchor_fixed() {
        for exp in -6..=6 {
            let scale = 10f64.powi(exp);
            for anchor in [
                egui::pos2(0.0, 0.0),
                egui::pos2(400.0, 300.0),
                egui::pos2(773.0, 91.0),
            ] {
                for factor in [1.1, 0.9, 2.0, 0.5] {
                    let mut v = vp(Point2::new(1e3, -1e3), scale);
                    let before = v.screen_to_model(anchor);
                    v.zoom_about(anchor, factor);
                    let after = v.screen_to_model(anchor);
                    // ズレを px 換算して 1px 以内であること。
                    let err_px = v.model_len_to_px(before.dist(after));
                    assert!(
                        err_px <= 1.0,
                        "scale=1e{exp} factor={factor}: アンカーが {err_px} px ずれた"
                    );
                }
            }
        }
    }

    /// スケールは上下限で頭打ちになり、発散しないこと。
    #[test]
    fn zoom_is_clamped() {
        let mut v = vp(Point2::ORIGIN, 1.0);
        for _ in 0..2000 {
            v.zoom_about(egui::pos2(400.0, 300.0), 2.0);
        }
        assert!(v.scale() <= MAX_SCALE);
        for _ in 0..4000 {
            v.zoom_about(egui::pos2(400.0, 300.0), 0.5);
        }
        assert!(v.scale() >= MIN_SCALE);
    }

    /// 不正な倍率でビューポートを壊さないこと。
    #[test]
    fn zoom_ignores_invalid_factor() {
        let mut v = vp(Point2::ORIGIN, 1.0);
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            v.zoom_about(egui::pos2(400.0, 300.0), bad);
            assert_eq!(v.scale(), 1.0);
            assert!(v.center().x.is_finite() && v.center().y.is_finite());
        }
    }

    /// モデル空間の +Y は画面上では上方向（Y 反転）。
    #[test]
    fn y_axis_points_up_on_screen() {
        let v = vp(Point2::ORIGIN, 1.0);
        let origin = v.model_to_screen(Point2::ORIGIN);
        let up = v.model_to_screen(Point2::new(0.0, 10.0));
        assert!(up.y < origin.y, "モデルの +Y は画面上で上に来るべき");
        let right = v.model_to_screen(Point2::new(10.0, 0.0));
        assert!(right.x > origin.x);
    }

    /// パンした量だけ中心が動くこと。
    #[test]
    fn pan_moves_center_opposite_to_drag() {
        let mut v = vp(Point2::ORIGIN, 2.0);
        v.pan_px(egui::vec2(100.0, 0.0));
        // 図面を右へドラッグしたら、画面中心のモデル座標は左へ動く。
        assert!(v.center().x < 0.0);
        assert!((v.center().x - (-50.0)).abs() < 1e-9);
    }

    /// 範囲全体が画面に収まること。
    #[test]
    fn zoom_to_fit_contains_bounds() {
        let mut v = vp(Point2::ORIGIN, 1.0);
        let b = Aabb::new(Point2::new(-30.0, -10.0), Point2::new(70.0, 40.0));
        v.zoom_to_fit(b, 0.05);

        let visible = v.visible_model_rect();
        assert!(visible.contains(b.min), "左下が画面外");
        assert!(visible.contains(b.max), "右上が画面外");
        assert!(v.center().eq_tol(b.center()));
    }

    /// 幅または高さが 0 の範囲でもゼロ除算しないこと。
    #[test]
    fn zoom_to_fit_handles_degenerate_bounds() {
        let mut v = vp(Point2::ORIGIN, 1.0);

        // 水平線のみ
        v.zoom_to_fit(
            Aabb::new(Point2::new(0.0, 5.0), Point2::new(100.0, 5.0)),
            0.05,
        );
        assert!(v.scale().is_finite() && v.scale() > 0.0);

        // 1 点のみ
        let before = v.scale();
        v.zoom_to_fit(
            Aabb::new(Point2::new(3.0, 3.0), Point2::new(3.0, 3.0)),
            0.05,
        );
        assert_eq!(v.scale(), before, "点だけの範囲では倍率を変えない");
        assert!(v.center().eq_tol(Point2::new(3.0, 3.0)));

        // 空の範囲では何もしない
        let before = v.center();
        v.zoom_to_fit(Aabb::EMPTY, 0.05);
        assert!(v.center().eq_tol(before));
    }

    /// ホイールズームの往復で倍率がドリフトしないこと。
    ///
    /// Phase 1 の申し送り事項「scale を直接乗算しているとドリフトするのでは」への回答。
    /// 実測すると 1 往復あたりの誤差は数 ULP しかなく、1000 往復しても相対誤差は
    /// 1e-12 に届かない。整数ズームレベルから scale を導出する方式は不要と判断した。
    #[test]
    fn zoom_roundtrip_does_not_drift() {
        let mut v = vp(Point2::new(1e3, -1e3), 1.0);
        let anchor = egui::pos2(400.0, 300.0);
        let before = v.scale();

        // 1.1^50 ≈ 117 なので、50 段の出入りなら上下限のクランプに当たらない。
        // それを 200 回繰り返して計 20,000 回のズーム操作を行う。
        const DEPTH: usize = 50;
        const ROUNDS: usize = 200;
        for _ in 0..ROUNDS {
            for _ in 0..DEPTH {
                v.zoom_about(anchor, 1.1);
            }
            for _ in 0..DEPTH {
                v.zoom_about(anchor, 1.0 / 1.1);
            }
        }

        let rel = ((v.scale() - before) / before).abs();
        assert!(
            rel < 1e-9,
            "{} 回のズーム操作後、倍率の相対誤差 {rel:e} が大きすぎる (before={before}, after={})",
            DEPTH * ROUNDS * 2,
            v.scale()
        );
    }

    /// px ↔ モデル長の変換が往復すること。
    #[test]
    fn px_model_len_roundtrip() {
        let v = vp(Point2::ORIGIN, 250.0);
        let len = v.px_to_model_len(10.0);
        assert!((v.model_len_to_px(len) - 10.0).abs() < 1e-4);
    }
}
