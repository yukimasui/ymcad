//! モデル座標 ↔ 画像 px の変換。**この変換はここに 1 か所だけ**（設計原則 5 の精神）。
//!
//! 画像は Y が下向き、モデルは Y が上向きなので、変換で Y を反転する。
//! 逆変換（[`Fit::to_model`]）を対で持ち、往復がテストで一致することを守る。
//! SVG を書く側（`svg.rs`）は座標を必ず [`Fit::to_px`] / [`Fit::len_to_px`] 経由で出す。
//!
//! 座標はすべて `f64`（画像のラスタライザが内部で何を使うかはここの外の話）。

use cad_core::geom::tolerance::is_zero_len;
use cad_core::geom::{Aabb, Point2, Vec2};

/// 画像の一辺の最大 px。
pub const MAX_SIDE_PX: u32 = 4096;
/// 画像の一辺の最小 px。
pub const MIN_SIDE_PX: u32 = 16;

/// 画像上の位置 [px]。原点は左上、X は右へ、Y は下へ増える。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Px {
    /// 左端からの距離。
    pub x: f64,
    /// 上端からの距離。
    pub y: f64,
}

/// 表示するモデルの範囲と画像の大きさの対応。
///
/// 縦横比を保つ（円は円のまま）。指定した範囲は画像の中央に収まり、
/// 縦横比が違えば余った方向に範囲の外が見える（[`Self::view`] が実際に見える範囲）。
#[derive(Clone, Copy, Debug)]
pub struct Fit {
    /// 画像いっぱいに見えるモデルの範囲。
    view: Aabb,
    /// 1 モデル単位あたりの px。
    scale: f64,
    width: u32,
    height: u32,
}

impl Fit {
    /// `region` が（縦横比を保って）収まる大きさ `width` × `height` px の画像を作る。
    ///
    /// # Errors
    ///
    /// 大きさが [`MIN_SIDE_PX`]..=[`MAX_SIDE_PX`] の外、`region` が有限でない・幅か高さが 0。
    pub fn new(region: Aabb, width: u32, height: u32) -> Result<Self, String> {
        for (name, v) in [("width", width), ("height", height)] {
            if !(MIN_SIDE_PX..=MAX_SIDE_PX).contains(&v) {
                return Err(format!(
                    "{name} は {MIN_SIDE_PX} 以上 {MAX_SIDE_PX} 以下で指定してください（{v}）"
                ));
            }
        }
        let finite = [region.min.x, region.min.y, region.max.x, region.max.y]
            .iter()
            .all(|v| v.is_finite());
        if !finite || region.is_empty() {
            return Err("region が空か有限でありません".to_owned());
        }
        if is_zero_len(region.width()) || is_zero_len(region.height()) {
            return Err(format!(
                "region の幅と高さは 0 より大きくしてください（幅 {}、高さ {}）",
                region.width(),
                region.height()
            ));
        }
        let (w, h) = (f64::from(width), f64::from(height));
        let scale = (w / region.width()).min(h / region.height());
        if !scale.is_finite() || scale <= 0.0 {
            return Err("region が小さすぎるか大きすぎて、画像に収められません".to_owned());
        }
        let half = Vec2::new(w / scale / 2.0, h / scale / 2.0);
        let center = region.center();
        Ok(Self {
            view: Aabb::new(center - half, center + half),
            scale,
            width,
            height,
        })
    }

    /// 画像の幅 [px]。
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// 画像の高さ [px]。
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// 画像に見えているモデルの範囲。
    #[must_use]
    pub fn view(&self) -> Aabb {
        self.view
    }

    /// 1 モデル単位あたりの px。
    #[must_use]
    pub fn pixels_per_unit(&self) -> f64 {
        self.scale
    }

    /// モデル座標 → 画像上の位置。**Y を反転する**（モデルの上が画像の上）。
    #[must_use]
    pub fn to_px(&self, p: Point2) -> Px {
        Px {
            x: (p.x - self.view.min.x) * self.scale,
            y: (self.view.max.y - p.y) * self.scale,
        }
    }

    /// 画像上の位置 → モデル座標。[`Self::to_px`] の逆。
    #[must_use]
    pub fn to_model(&self, px: Px) -> Point2 {
        Point2::new(
            self.view.min.x + px.x / self.scale,
            self.view.max.y - px.y / self.scale,
        )
    }

    /// モデルの長さ → px。
    #[must_use]
    pub fn len_to_px(&self, len: f64) -> f64 {
        len * self.scale
    }

    /// px → モデルの長さ。[`Self::len_to_px`] の逆。
    #[must_use]
    pub fn px_to_len(&self, px: f64) -> f64 {
        px / self.scale
    }
}

#[cfg(test)]
mod tests {
    use cad_core::geom::tolerance::eq_len;

    use super::*;

    fn region(x0: f64, y0: f64, x1: f64, y1: f64) -> Aabb {
        Aabb::new(Point2::new(x0, y0), Point2::new(x1, y1))
    }

    #[test]
    fn model_to_px_and_back_agrees() {
        let fit = Fit::new(region(-5.0, 2.0, 15.0, 12.0), 800, 600).unwrap();
        for p in [
            Point2::new(-5.0, 2.0),
            Point2::new(15.0, 12.0),
            Point2::new(0.0, 0.0),
            Point2::new(3.25, -7.5),
            Point2::new(1.0e6, -1.0e6),
        ] {
            let back = fit.to_model(fit.to_px(p));
            assert!(
                eq_len(back.x, p.x) && eq_len(back.y, p.y),
                "{p:?} → {back:?}"
            );
        }
        for px in [
            Px { x: 0.0, y: 0.0 },
            Px { x: 800.0, y: 600.0 },
            Px {
                x: 123.5,
                y: 456.25,
            },
        ] {
            let back = fit.to_px(fit.to_model(px));
            assert!(
                eq_len(back.x, px.x) && eq_len(back.y, px.y),
                "{px:?} → {back:?}"
            );
        }
        assert!(eq_len(fit.px_to_len(fit.len_to_px(3.5)), 3.5));
    }

    /// 上が上のまま描かれる: モデルの Y が増えると画像の Y は減る。
    #[test]
    fn y_axis_is_flipped() {
        let fit = Fit::new(region(0.0, 0.0, 10.0, 10.0), 100, 100).unwrap();
        let low = fit.to_px(Point2::new(5.0, 1.0));
        let high = fit.to_px(Point2::new(5.0, 9.0));
        assert!(high.y < low.y, "{high:?} {low:?}");
        // 右は右のまま。
        let left = fit.to_px(Point2::new(1.0, 5.0));
        let right = fit.to_px(Point2::new(9.0, 5.0));
        assert!(left.x < right.x);
        // モデルの左上が画像の左上。
        let corner = fit.to_px(Point2::new(0.0, 10.0));
        assert!(eq_len(corner.x, 0.0) && eq_len(corner.y, 0.0), "{corner:?}");
    }

    /// 縦横比を保ち、指定の範囲は中央に収まる。
    #[test]
    fn aspect_ratio_is_kept_and_region_is_centered() {
        // 幅 10 × 高さ 10 の範囲を 200 × 100 の画像へ: 縦に合わせて 10 px/単位、横は余る。
        let fit = Fit::new(region(0.0, 0.0, 10.0, 10.0), 200, 100).unwrap();
        assert!(eq_len(fit.pixels_per_unit(), 10.0));
        let v = fit.view();
        assert!(eq_len(v.min.x, -5.0) && eq_len(v.max.x, 15.0), "{v:?}");
        assert!(eq_len(v.min.y, 0.0) && eq_len(v.max.y, 10.0), "{v:?}");
        // 指定の範囲の中心は画像の中心に来る。
        let c = fit.to_px(Point2::new(5.0, 5.0));
        assert!(eq_len(c.x, 100.0) && eq_len(c.y, 50.0), "{c:?}");
        // 横に合わせる場合も同じ。
        let fit = Fit::new(region(0.0, 0.0, 20.0, 5.0), 100, 100).unwrap();
        assert!(eq_len(fit.pixels_per_unit(), 5.0));
        assert!(eq_len(fit.view().height(), 20.0));
    }

    #[test]
    fn sizes_outside_the_limits_are_rejected() {
        let r = region(0.0, 0.0, 1.0, 1.0);
        assert!(Fit::new(r, MAX_SIDE_PX, MAX_SIDE_PX).is_ok());
        assert!(Fit::new(r, MIN_SIDE_PX, MIN_SIDE_PX).is_ok());
        assert!(Fit::new(r, MAX_SIDE_PX + 1, 100).is_err());
        assert!(Fit::new(r, 100, MAX_SIDE_PX + 1).is_err());
        assert!(Fit::new(r, MIN_SIDE_PX - 1, 100).is_err());
        assert!(Fit::new(r, 0, 100).is_err());
    }

    #[test]
    fn degenerate_or_non_finite_regions_are_rejected() {
        assert!(Fit::new(region(0.0, 0.0, 0.0, 10.0), 100, 100).is_err());
        assert!(Fit::new(region(0.0, 0.0, 10.0, 0.0), 100, 100).is_err());
        assert!(Fit::new(Aabb::EMPTY, 100, 100).is_err());
        assert!(Fit::new(Aabb::UNBOUNDED, 100, 100).is_err());
        let nan = Aabb {
            min: Point2::new(0.0, 0.0),
            max: Point2::new(f64::NAN, 1.0),
        };
        assert!(Fit::new(nan, 100, 100).is_err());
    }
}
