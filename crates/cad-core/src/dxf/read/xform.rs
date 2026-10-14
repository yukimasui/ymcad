//! `INSERT` の変換を 2 次元のアフィン行列で扱う。
//!
//! # なぜ一度行列にするのか
//!
//! DXF の `INSERT` は「ブロック基点を原点へ → (sx, sy) 倍 → 回転 → 挿入点へ →
//! OCS からワールドへ」の順で図形を移す。ymcad の [`Placement`] は
//! 「基点を原点へ → X 軸での鏡像 → 一様倍率 → 回転 → 平行移動」の順
//! （[`crate::component::place`]）。**順序も持ち方も違う**ので、項目どうしを
//! 直接対応づけると、負の倍率・押し出し方向 Z<0・回転の組み合わせで取りこぼす。
//!
//! そこで DXF 側を一度 2D のアフィン行列にまとめ、線形部分が相似変換
//! （直交 × 一様倍率。反転を含む）かを調べ、相似なら `Placement` に分解する。
//! 相似でない（縦横の倍率が違う）ときは `Placement` で表せないので、
//! 呼び出し側が中身を行列で変換した通常の図形へ展開する（ADR-0050）。

use crate::component::{place, Placement};
use crate::entity::Geometry;
use crate::geom::tolerance::{eq_angle, eq_len, is_zero_len, wrap_2pi};
use crate::geom::{Point2, Polyline, Vec2, Xline};

/// 円・円弧を楕円へ写すときのポリライン近似の細かさ（半径に対するサジッタの比）。
///
/// 近似はブロック座標の円で `半径 × この比` 以下のサジッタになるよう分割してから
/// 頂点を行列で写す。写した後の誤差は高々「行列の最大の伸び × サジッタ」で、
/// 楕円の長半径も同じ伸びで大きくなるので、**写した後も長半径に対して同じ比**に収まる。
/// 1/1000 は 1 周でおよそ 70 分割になり、直径 2000 px で表示しても誤差が 1 px 程度。
/// 倍率によらず分割数が決まるので、巨大な倍率で頂点数が暴れることもない。
const ELLIPSE_SAGITTA_RATIO: f64 = 1.0 / 1000.0;

/// 2 次元のアフィン変換 `p' = (a·x + c·y + tx, b·x + d·y + ty)`。
///
/// 列ベクトル `(a, b)` が X 軸の、`(c, d)` が Y 軸の行き先。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Affine {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    tx: f64,
    ty: f64,
}

/// 行列の線形部分の分類。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Shape {
    /// 相似変換。`scale` は正、`rotation` は `[0, 2π)`。
    Similar {
        scale: f64,
        rotation: f64,
        flipped: bool,
    },
    /// 縦横の倍率が違う、または軸が直交しない。`Placement` では表せない。
    NonUniform,
    /// 図形が線や点に潰れる（倍率 0 など）、または非有限。
    Degenerate,
}

impl Affine {
    /// 恒等変換。
    pub(super) const IDENTITY: Self = Self {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        tx: 0.0,
        ty: 0.0,
    };

    /// 平行移動。
    pub(super) fn translation(v: Vec2) -> Self {
        Self {
            tx: v.x,
            ty: v.y,
            ..Self::IDENTITY
        }
    }

    /// 原点まわりの回転 [rad]。反時計回り。
    pub(super) fn rotation(rad: f64) -> Self {
        let (s, c) = rad.sin_cos();
        Self {
            a: c,
            b: s,
            c: -s,
            d: c,
            tx: 0.0,
            ty: 0.0,
        }
    }

    /// 原点まわりの縦横別の倍率。
    pub(super) fn scale(sx: f64, sy: f64) -> Self {
        Self {
            a: sx,
            d: sy,
            ..Self::IDENTITY
        }
    }

    /// X 座標の符号を反転する（Y 軸での鏡像）。
    ///
    /// 押し出し方向が `(0, 0, -1)` の OCS をワールドへ移す変換がこれになる
    /// （DXF の arbitrary axis algorithm で OCS の X 軸が `(-1, 0, 0)`、Y 軸が `(0, 1, 0)`）。
    pub(super) fn mirror_x() -> Self {
        Self::scale(-1.0, 1.0)
    }

    /// 点を写す。
    pub(super) fn apply(self, p: Point2) -> Point2 {
        Point2::new(
            self.a * p.x + self.c * p.y + self.tx,
            self.b * p.x + self.d * p.y + self.ty,
        )
    }

    /// ベクトル（向きと長さ）を写す。平行移動は効かない。
    pub(super) fn apply_vec(self, v: Vec2) -> Vec2 {
        Vec2::new(self.a * v.x + self.c * v.y, self.b * v.x + self.d * v.y)
    }

    /// 線形部分を分類し、相似なら倍率・回転・反転へ分解する。
    ///
    /// 相似の判定は「2 本の軸の長さの比が 1」かつ「軸どうしが直交」。どちらも
    /// 比（無次元）で見るので、図面の大きさや倍率の桁によらない。
    ///
    /// 分解は [`crate::component::place`] の順序（鏡像 → 倍率 → 回転）に合わせる。
    /// 線形部分は `s·R(θ)`（非反転）か `s·R(θ)·diag(1, -1)`（反転）で、
    /// **どちらでも X 軸の行き先は `s·(cos θ, sin θ)`** なので、
    /// 倍率と回転は X 軸の列だけから読める。反転は行列式の符号で決まる。
    pub(super) fn shape(self) -> Shape {
        let all_finite = [self.a, self.b, self.c, self.d, self.tx, self.ty]
            .iter()
            .all(|v| v.is_finite());
        if !all_finite {
            return Shape::Degenerate;
        }
        let n1 = self.a.hypot(self.b);
        let n2 = self.c.hypot(self.d);
        if is_zero_len(n1) || is_zero_len(n2) {
            return Shape::Degenerate;
        }
        let det = self.a * self.d - self.b * self.c;
        // 軸が平行（行列式 ≈ 0）だと面積が潰れる。軸の長さで割って無次元にしてから見る。
        if is_zero_len(det / (n1 * n2)) {
            return Shape::Degenerate;
        }
        let cos_between = (self.a * self.c + self.b * self.d) / (n1 * n2);
        if !eq_len(n2 / n1, 1.0) || !is_zero_len(cos_between) {
            return Shape::NonUniform;
        }
        Shape::Similar {
            scale: n1,
            rotation: normalize_angle(self.b.atan2(self.a)),
            flipped: det < 0.0,
        }
    }

    /// 相似なら、基点 `base` の定義をこの変換で置く [`Placement`] を返す。
    ///
    /// `Placement` の基点は「定義の基点の行き先」なので `self.apply(base)`。
    pub(super) fn placement(self, base: Point2) -> Option<Placement> {
        match self.shape() {
            Shape::Similar {
                scale,
                rotation,
                flipped,
            } => Placement::new(self.apply(base), rotation, scale, flipped).ok(),
            Shape::NonUniform | Shape::Degenerate => None,
        }
    }
}

/// `self * rhs` は「`rhs` を先にかけ、次に `self`」（数学の行列の積と同じ順）。
impl std::ops::Mul for Affine {
    type Output = Self;

    fn mul(self, r: Self) -> Self {
        Self {
            a: self.a * r.a + self.c * r.b,
            b: self.b * r.a + self.d * r.b,
            c: self.a * r.c + self.c * r.d,
            d: self.b * r.c + self.d * r.d,
            tx: self.a * r.tx + self.c * r.ty + self.tx,
            ty: self.b * r.tx + self.d * r.ty + self.ty,
        }
    }
}

/// 角度を `[0, 2π)` に寄せる。トレランス内で 0（または 2π）なら 0 ちょうどにする。
fn normalize_angle(rad: f64) -> f64 {
    let r = wrap_2pi(rad);
    if eq_angle(r, 0.0) {
        0.0
    } else {
        r
    }
}

/// `INSERT` 1 個（MINSERT なら 1 マス）の変換。ブロック座標 → INSERT の親の座標。
///
/// DXF の順序: ブロック基点を原点へ → (sx, sy) 倍 → 回転 → 挿入点へ → OCS からワールドへ。
/// MINSERT の列・行の間隔は**回転した（倍率はかけない）座標系**で並ぶので、
/// `offset` を回してから挿入点に足す。
pub(super) struct InsertXform {
    /// ブロックの基点。
    pub base: Point2,
    /// 挿入点（OCS）。
    pub insert: Point2,
    /// X 倍率（41）。
    pub sx: f64,
    /// Y 倍率（42）。
    pub sy: f64,
    /// 回転角 [rad]（50 を変換したもの）。
    pub rotation: f64,
    /// 押し出し方向が Z<0（OCS の X が反転する）。
    pub mirror_ocs: bool,
}

impl InsertXform {
    /// MINSERT のマス目のずれ（列方向・行方向の距離。回転前）を足した変換。
    pub(super) fn matrix(&self, offset: Vec2) -> Affine {
        let ocs = if self.mirror_ocs {
            Affine::mirror_x()
        } else {
            Affine::IDENTITY
        };
        let at = self.insert + offset.rotated(self.rotation);
        ocs * Affine::translation(at - Point2::ORIGIN)
            * Affine::rotation(self.rotation)
            * Affine::scale(self.sx, self.sy)
            * Affine::translation(Point2::ORIGIN - self.base)
    }
}

/// 図形を行列で写す。
///
/// - 相似なら [`crate::component::place`] を通す。円は円、円弧は円弧のまま
///   （反転での開始角・終了角の入れ替えも `place` の中の `Geometry::mirrored` に任せる。ADR-0020）
/// - 相似でなければ線分・ポリラインは頂点を写し、円・円弧は楕円になるので
///   ポリラインで近似する（[`ELLIPSE_SAGITTA_RATIO`]）
/// - 潰れる変換なら `None`
pub(super) fn transform_geometry(geom: &Geometry, m: Affine) -> Option<Geometry> {
    match m.shape() {
        Shape::Similar { .. } => {
            let p = m.placement(Point2::ORIGIN)?;
            Some(place(geom, Point2::ORIGIN, p))
        }
        Shape::NonUniform => affine_approx(geom, m),
        Shape::Degenerate => None,
    }
}

/// 相似でない行列で写す。円・円弧はポリラインになる。
fn affine_approx(geom: &Geometry, m: Affine) -> Option<Geometry> {
    let map = |pts: &[Point2]| pts.iter().map(|&p| m.apply(p)).collect::<Vec<_>>();
    match geom {
        Geometry::Line(l) => Some(Geometry::Line(crate::geom::Line::new(
            m.apply(l.a),
            m.apply(l.b),
        ))),
        Geometry::Polyline(p) => Some(Geometry::Polyline(Polyline::new(
            map(&p.vertices),
            p.closed,
        ))),
        Geometry::Circle(c) => {
            let pts = c.tessellate(c.radius * ELLIPSE_SAGITTA_RATIO);
            (pts.len() >= 3).then(|| Geometry::Polyline(Polyline::new(map(&pts), true)))
        }
        Geometry::Arc(a) => {
            let pts = a.tessellate(a.radius * ELLIPSE_SAGITTA_RATIO);
            (pts.len() >= 2).then(|| Geometry::Polyline(Polyline::new(map(&pts), false)))
        }
        Geometry::Xline(x) => {
            Xline::new(m.apply(x.origin), m.apply_vec(x.direction)).map(Geometry::Xline)
        }
        // DXF から読んだ生の図形にインスタンスは無い（INSERT は別に扱う）。
        Geometry::Instance(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::tolerance::eq_angle;
    use crate::geom::{Arc, Circle, Line};
    use std::f64::consts::{FRAC_PI_2, PI};

    fn p(x: f64, y: f64) -> Point2 {
        Point2::new(x, y)
    }

    fn insert(sx: f64, sy: f64, deg: f64, mirror: bool) -> InsertXform {
        InsertXform {
            base: p(1.0, 2.0),
            insert: p(10.0, 20.0),
            sx,
            sy,
            rotation: crate::dxf::deg_to_rad(deg),
            mirror_ocs: mirror,
        }
    }

    /// 行列で写した点と、分解した `Placement` で `place` した点が一致すること。
    /// 分解の順序（鏡像 → 倍率 → 回転）を取り違えると、このどれかで外れる。
    #[test]
    fn similarity_decomposition_matches_the_matrix() {
        let cases = [
            (2.0, 2.0, 30.0, false),
            (2.0, -2.0, 30.0, false),
            (-2.0, 2.0, 30.0, false),
            (-2.0, -2.0, 30.0, false),
            (1.5, 1.5, 200.0, true),
            (-1.5, 1.5, 75.0, true),
            (0.5, -0.5, -45.0, true),
        ];
        let probe = Geometry::Line(Line::new(p(3.0, 2.0), p(1.0, 5.0)));
        for (sx, sy, deg, mirror) in cases {
            let x = insert(sx, sy, deg, mirror);
            let m = x.matrix(Vec2::ZERO);
            let pl = m.placement(x.base).expect("相似のはず");
            let Geometry::Line(placed) = place(&probe, x.base, pl) else {
                panic!("線分のはず")
            };
            assert!(
                placed.a.eq_tol(m.apply(p(3.0, 2.0))),
                "{sx} {sy} {deg} {mirror}"
            );
            assert!(
                placed.b.eq_tol(m.apply(p(1.0, 5.0))),
                "{sx} {sy} {deg} {mirror}"
            );
        }
    }

    #[test]
    fn negative_y_scale_is_a_flip_without_extra_rotation() {
        let m = insert(2.0, -2.0, 30.0, false).matrix(Vec2::ZERO);
        let Shape::Similar {
            scale,
            rotation,
            flipped,
        } = m.shape()
        else {
            panic!("相似のはず")
        };
        assert!(eq_len(scale, 2.0));
        assert!(eq_angle(rotation, crate::dxf::deg_to_rad(30.0)));
        assert!(flipped);
    }

    #[test]
    fn both_negative_is_a_half_turn_not_a_flip() {
        let m = insert(-1.0, -1.0, 0.0, false).matrix(Vec2::ZERO);
        let Shape::Similar {
            rotation, flipped, ..
        } = m.shape()
        else {
            panic!("相似のはず")
        };
        assert!(eq_angle(rotation, PI));
        assert!(!flipped);
    }

    #[test]
    fn non_uniform_and_zero_scale_are_classified() {
        assert_eq!(
            insert(2.0, 1.0, 0.0, false).matrix(Vec2::ZERO).shape(),
            Shape::NonUniform
        );
        assert_eq!(
            insert(0.0, 1.0, 0.0, false).matrix(Vec2::ZERO).shape(),
            Shape::Degenerate
        );
    }

    #[test]
    fn composition_applies_right_operand_first() {
        let m = Affine::translation(Vec2::new(5.0, 0.0)) * Affine::rotation(FRAC_PI_2);
        assert!(m.apply(p(1.0, 0.0)).eq_tol(p(5.0, 1.0)));
    }

    /// 縦横別倍率で円を写すと楕円のポリラインになり、頂点は楕円の式を満たす。
    #[test]
    fn circle_under_non_uniform_scale_becomes_an_ellipse_polyline() {
        let m = Affine::scale(2.0, 1.0);
        let g = transform_geometry(&Geometry::Circle(Circle::new(Point2::ORIGIN, 1.0)), m)
            .expect("写せる");
        let Geometry::Polyline(pl) = g else {
            panic!("ポリラインのはず")
        };
        assert!(pl.closed);
        assert!(pl.vertices.len() >= 32, "{}", pl.vertices.len());
        for v in &pl.vertices {
            let on = (v.x / 2.0).powi(2) + v.y.powi(2);
            assert!(eq_len(on, 1.0), "{v:?}");
        }
    }

    #[test]
    fn arc_under_non_uniform_scale_keeps_its_end_points() {
        let a = Arc::new(Point2::ORIGIN, 1.0, 0.0, FRAC_PI_2);
        let g = transform_geometry(&Geometry::Arc(a), Affine::scale(3.0, 1.0)).expect("写せる");
        let Geometry::Polyline(pl) = g else {
            panic!("ポリラインのはず")
        };
        assert!(!pl.closed);
        assert!(pl.vertices[0].eq_tol(p(3.0, 0.0)));
        assert!(pl.vertices.last().unwrap().eq_tol(p(0.0, 1.0)));
    }

    #[test]
    fn arc_under_mirror_swaps_its_angles() {
        let a = Arc::new(Point2::ORIGIN, 1.0, 0.0, FRAC_PI_2);
        let g = transform_geometry(&Geometry::Arc(a), Affine::mirror_x()).expect("写せる");
        let Geometry::Arc(m) = g else {
            panic!("円弧のまま")
        };
        // (1,0)→(-1,0)、(0,1)→(0,1)。反時計回りに 90° から 180°。
        assert!(m.start_point().eq_tol(p(0.0, 1.0)));
        assert!(m.end_point().eq_tol(p(-1.0, 0.0)));
    }
}
