//! 図形要素の定義。

use crate::geom::{Aabb, Arc, Circle, Line, Point2, Polyline, Vec2, Xline};
use crate::group::GroupId;
use crate::layer::{ColorSpec, LayerId};

/// 図形の実体。
#[derive(Clone, Debug, PartialEq)]
pub enum Geometry {
    /// 線分。
    Line(Line),
    /// 円。
    Circle(Circle),
    /// 円弧。
    Arc(Arc),
    /// 無限に伸びる作図線。
    Xline(Xline),
    /// ポリライン（連結した線分列）。
    Polyline(Polyline),
}

impl Geometry {
    /// 境界ボックス。
    #[must_use]
    pub fn bbox(&self) -> Aabb {
        match self {
            Self::Line(l) => l.bbox(),
            Self::Circle(c) => c.bbox(),
            Self::Arc(a) => a.bbox(),
            // 無限に伸びるので全平面を返す。図面範囲（ZOOM EXTENTS）からは
            // `EntityStore::bbox` が `is_bounded` を見て除外する。
            Self::Xline(_) => Aabb::UNBOUNDED,
            Self::Polyline(p) => p.bbox(),
        }
    }

    /// 点との最短距離。ピックやスナップの判定に使う。
    #[must_use]
    pub fn dist_to(&self, p: Point2) -> f64 {
        match self {
            Self::Line(l) => l.dist_to(p),
            Self::Circle(c) => c.dist_to(p),
            Self::Arc(a) => a.dist_to(p),
            Self::Xline(x) => x.dist_to(p),
            Self::Polyline(pl) => pl.dist_to(p),
        }
    }

    /// 有界な図形か。作図線だけが `false`。
    ///
    /// 図面範囲（ZOOM EXTENTS）の計算から無限図形を外すために使う。
    /// AutoCAD も作図線を図面範囲に含めない。
    #[must_use]
    pub fn is_bounded(&self) -> bool {
        !matches!(self, Self::Xline(_))
    }

    /// コマンド名などに使う種別名。
    #[must_use]
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Line(_) => "LINE",
            Self::Circle(_) => "CIRCLE",
            Self::Arc(_) => "ARC",
            Self::Xline(_) => "XLINE",
            // DXF R12 での LWPOLYLINE 相当の名前（Phase 6 の DXF 出力で使う）。
            Self::Polyline(_) => "LWPOLYLINE",
        }
    }

    /// 平行移動した複製を作る。
    #[must_use]
    pub fn translated(&self, v: Vec2) -> Self {
        match self {
            Self::Line(l) => Self::Line(l.translated(v)),
            Self::Circle(c) => Self::Circle(c.translated(v)),
            Self::Arc(a) => Self::Arc(a.translated(v)),
            Self::Xline(x) => Self::Xline(x.translated(v)),
            Self::Polyline(p) => Self::Polyline(p.translated(v)),
        }
    }

    /// `regions` のいずれかに入っている定義点だけを `delta` ぶん動かす。
    ///
    /// `regions` が空のときは図形全体を平行移動する（MOVE と同じ）。
    /// AutoCAD は交差窓を複数回重ねられるので、範囲は 1 つではなくスライスで受ける。
    #[must_use]
    pub fn stretched(&self, regions: &[Aabb], delta: Vec2) -> Self {
        if regions.is_empty() {
            return self.translated(delta);
        }
        let inside = |p: Point2| regions.iter().any(|r| r.contains(p));
        let move_if_inside = |p: Point2| if inside(p) { p + delta } else { p };
        match self {
            Self::Line(l) => Self::Line(Line::new(move_if_inside(l.a), move_if_inside(l.b))),
            // 中心が範囲内のときだけ図形全体を動かす。半径・角度は不変。
            Self::Circle(c) => {
                if inside(c.center) {
                    Self::Circle(c.translated(delta))
                } else {
                    Self::Circle(*c)
                }
            }
            Self::Arc(a) => {
                if inside(a.center) {
                    Self::Arc(a.translated(delta))
                } else {
                    Self::Arc(*a)
                }
            }
            // 無限直線に「範囲内の定義点」という概念が無いので、円や円弧と同じく
            // 通過点が範囲に入っているときだけ全体を動かす。
            Self::Xline(x) => {
                if inside(x.origin) {
                    Self::Xline(x.translated(delta))
                } else {
                    Self::Xline(*x)
                }
            }
            Self::Polyline(p) => {
                let vertices = p.vertices.iter().copied().map(move_if_inside).collect();
                Self::Polyline(Polyline::new(vertices, p.closed))
            }
        }
    }

    /// `center` を中心に `angle`（ラジアン、反時計回り）だけ回転する。
    #[must_use]
    pub fn rotated(&self, center: Point2, angle: f64) -> Self {
        let rot = |p: Point2| center + (p - center).rotated(angle);
        match self {
            Self::Line(l) => Self::Line(Line::new(rot(l.a), rot(l.b))),
            // 中心のみ回転する。半径は不変。
            Self::Circle(c) => Self::Circle(Circle::new(rot(c.center), c.radius)),
            // 中心を回転し、開始角・終了角の両方に angle を加える。
            Self::Arc(a) => Self::Arc(Arc::new(
                rot(a.center),
                a.radius,
                a.start_angle + angle,
                a.end_angle + angle,
            )),
            Self::Xline(x) => Self::Xline(x.rotated(center, angle)),
            Self::Polyline(p) => {
                let vertices = p.vertices.iter().copied().map(rot).collect();
                Self::Polyline(Polyline::new(vertices, p.closed))
            }
        }
    }

    /// `center` を中心に `factor` 倍に拡大縮小する。
    ///
    /// `factor` が `0` または有限値でない（NaN・無限大）場合は安全策として `self` を
    /// そのまま返す。`factor` の妥当性検証は呼び出し側の責務であり、ここでは
    /// 縮退したジオメトリ（半径 0 の円など）を作らないための最終防衛線に過ぎない。
    #[must_use]
    pub fn scaled(&self, center: Point2, factor: f64) -> Self {
        if !factor.is_finite() || factor == 0.0 {
            return self.clone();
        }
        let scale = |p: Point2| center + (p - center) * factor;
        match self {
            Self::Line(l) => Self::Line(Line::new(scale(l.a), scale(l.b))),
            // 中心が動き、半径も factor 倍になる。
            Self::Circle(c) => Self::Circle(Circle::new(scale(c.center), c.radius * factor)),
            // 中心が動き、半径も factor 倍になる。角度は不変。
            Self::Arc(a) => Self::Arc(Arc::new(
                scale(a.center),
                a.radius * factor,
                a.start_angle,
                a.end_angle,
            )),
            Self::Xline(x) => Self::Xline(x.scaled(center, factor)),
            Self::Polyline(p) => {
                let vertices = p.vertices.iter().copied().map(scale).collect();
                Self::Polyline(Polyline::new(vertices, p.closed))
            }
        }
    }

    /// `axis` を鏡像軸として反転する。
    ///
    /// `axis` が退化している（長さ 0）場合は反転先が定まらないため `self` を
    /// そのまま返す（NaN を作らない）。
    ///
    /// `Arc` は反射によって掃引の向き（CCW）が逆転するため、単に端点を反射する
    /// だけでは足りない。`start_angle` と `end_angle` を入れ替えたうえで反射する
    /// ことで、「`start_angle` から `end_angle` へ CCW」という不変条件を保つ
    /// （入れ替えを忘れると、鏡像の弧ではなく円の残り部分＝補角の弧になる）。
    #[must_use]
    pub fn mirrored(&self, axis: &Line) -> Self {
        if axis.is_degenerate() {
            return self.clone();
        }
        match self {
            Self::Line(l) => Self::Line(Line::new(
                reflect_point(axis, l.a),
                reflect_point(axis, l.b),
            )),
            // 中心のみ反射する。半径は不変。
            Self::Circle(c) => Self::Circle(Circle::new(reflect_point(axis, c.center), c.radius)),
            Self::Arc(a) => {
                // 非退化なので axis.dir() は必ず Some。
                let axis_angle = axis.dir().expect("非退化な axis のはず").angle();
                let new_center = reflect_point(axis, a.center);
                // 反射で向きが逆転するため start/end を入れ替えて反射する。
                let new_start = 2.0 * axis_angle - a.end_angle;
                let new_end = 2.0 * axis_angle - a.start_angle;
                Self::Arc(Arc::new(new_center, a.radius, new_start, new_end))
            }
            Self::Xline(x) => Self::Xline(x.mirrored(axis)),
            Self::Polyline(p) => {
                let vertices = p
                    .vertices
                    .iter()
                    .copied()
                    .map(|v| reflect_point(axis, v))
                    .collect();
                Self::Polyline(Polyline::new(vertices, p.closed))
            }
        }
    }
}

/// 点 `p` を `axis`（非退化前提）に関して反射する。
///
/// `axis.closest_param` は無限直線への射影なので、線分の外側でも正しく使える。
fn reflect_point(axis: &Line, p: Point2) -> Point2 {
    let foot = axis.point_at(axis.closest_param(p));
    foot + (foot - p)
}

/// 図面を構成する 1 要素。
#[derive(Clone, Debug, PartialEq)]
pub struct Entity {
    /// 図形。
    pub geom: Geometry,
    /// 所属レイヤ。
    pub layer: LayerId,
    /// 色。既定はレイヤの色に従う。
    pub color: ColorSpec,
    /// 所属グループ。属していなければ `None`。
    ///
    /// 所属は **エンティティ側だけが持つ**。グループ側にメンバー一覧を持たせると
    /// 削除や Undo のたびに両方を更新する必要があり、片方だけ直し損ねる事故が起きる。
    pub group: Option<GroupId>,
}

impl Entity {
    /// レイヤの色に従う要素を作る。
    #[must_use]
    pub fn new(geom: Geometry, layer: LayerId) -> Self {
        Self {
            geom,
            layer,
            color: ColorSpec::ByLayer,
            group: None,
        }
    }

    /// 境界ボックス。
    #[must_use]
    pub fn bbox(&self) -> Aabb {
        self.geom.bbox()
    }

    /// 平行移動した複製を作る（レイヤ・色は変わらない）。
    #[must_use]
    pub fn translated(&self, v: Vec2) -> Self {
        Self {
            geom: self.geom.translated(v),
            layer: self.layer,
            color: self.color,
            group: self.group,
        }
    }

    /// `regions` に入っている定義点だけを動かした複製を作る（レイヤ・色は変わらない）。
    #[must_use]
    pub fn stretched(&self, regions: &[Aabb], delta: Vec2) -> Self {
        Self {
            geom: self.geom.stretched(regions, delta),
            layer: self.layer,
            color: self.color,
            group: self.group,
        }
    }

    /// `center` を中心に回転した複製を作る（レイヤ・色は変わらない）。
    #[must_use]
    pub fn rotated(&self, center: Point2, angle: f64) -> Self {
        Self {
            geom: self.geom.rotated(center, angle),
            layer: self.layer,
            color: self.color,
            group: self.group,
        }
    }

    /// `center` を中心に拡大縮小した複製を作る（レイヤ・色は変わらない）。
    #[must_use]
    pub fn scaled(&self, center: Point2, factor: f64) -> Self {
        Self {
            geom: self.geom.scaled(center, factor),
            layer: self.layer,
            color: self.color,
            group: self.group,
        }
    }

    /// `axis` を鏡像軸として反転した複製を作る（レイヤ・色は変わらない）。
    #[must_use]
    pub fn mirrored(&self, axis: &Line) -> Self {
        Self {
            geom: self.geom.mirrored(axis),
            layer: self.layer,
            color: self.color,
            group: self.group,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::tolerance::{eq_angle, eq_len};
    use crate::geom::{Point2 as P, Vec2 as V};
    use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, TAU};

    fn line_geom(a: (f64, f64), b: (f64, f64)) -> Geometry {
        Geometry::Line(Line::new(P::new(a.0, a.1), P::new(b.0, b.1)))
    }

    fn line_at(g: &Geometry) -> Line {
        match g {
            Geometry::Line(l) => *l,
            other => panic!("Line のはず: {other:?}"),
        }
    }

    fn circle_at(g: &Geometry) -> Circle {
        match g {
            Geometry::Circle(c) => *c,
            other => panic!("Circle のはず: {other:?}"),
        }
    }

    fn arc_at(g: &Geometry) -> Arc {
        match g {
            Geometry::Arc(a) => *a,
            other => panic!("Arc のはず: {other:?}"),
        }
    }

    fn polyline_at(g: &Geometry) -> &Polyline {
        match g {
            Geometry::Polyline(p) => p,
            other => panic!("Polyline のはず: {other:?}"),
        }
    }

    #[test]
    fn geometry_bbox_dispatches_to_polyline() {
        let g = Geometry::Polyline(Polyline::rectangle(P::new(0.0, 0.0), P::new(2.0, 3.0)));
        let b = g.bbox();
        assert_eq!(b.min, P::new(0.0, 0.0));
        assert_eq!(b.max, P::new(2.0, 3.0));
    }

    #[test]
    fn geometry_dist_to_dispatches_to_polyline() {
        let g = Geometry::Polyline(Polyline::new(
            vec![P::new(0.0, 0.0), P::new(10.0, 0.0)],
            false,
        ));
        assert!(eq_len(g.dist_to(P::new(5.0, 3.0)), 3.0));
    }

    #[test]
    fn geometry_type_name_polyline_is_lwpolyline() {
        let g = Geometry::Polyline(Polyline::new(vec![], false));
        assert_eq!(g.type_name(), "LWPOLYLINE");
    }

    #[test]
    fn geometry_type_name_other_variants() {
        assert_eq!(line_geom((0.0, 0.0), (1.0, 1.0)).type_name(), "LINE");
        assert_eq!(
            Geometry::Circle(Circle::new(P::ORIGIN, 1.0)).type_name(),
            "CIRCLE"
        );
        assert_eq!(
            Geometry::Arc(Arc::new(P::ORIGIN, 1.0, 0.0, 1.0)).type_name(),
            "ARC"
        );
    }

    #[test]
    fn geometry_translated_line() {
        let g = line_geom((0.0, 0.0), (1.0, 1.0));
        let moved = g.translated(V::new(5.0, 5.0));
        assert_eq!(moved, line_geom((5.0, 5.0), (6.0, 6.0)));
    }

    #[test]
    fn entity_translated_keeps_layer_and_color() {
        let e = Entity::new(line_geom((0.0, 0.0), (1.0, 0.0)), LayerId::ZERO);
        let moved = e.translated(V::new(3.0, 4.0));
        assert_eq!(moved.layer, e.layer);
        assert_eq!(moved.color, e.color);
        assert_eq!(moved.geom, line_geom((3.0, 4.0), (4.0, 4.0)));
    }

    // ---- stretched --------------------------------------------------------

    /// `(0,0)-(10,10)` の範囲。
    fn region() -> Aabb {
        Aabb::new(P::new(0.0, 0.0), P::new(10.0, 10.0))
    }

    /// `region()` とは重ならない範囲。
    fn other_region() -> Aabb {
        Aabb::new(P::new(100.0, 100.0), P::new(110.0, 110.0))
    }

    #[test]
    fn geometry_stretched_empty_regions_equals_translated_line() {
        let g = line_geom((1.0, 1.0), (2.0, 2.0));
        let delta = V::new(5.0, -3.0);
        assert_eq!(g.stretched(&[], delta), g.translated(delta));
    }

    #[test]
    fn geometry_stretched_empty_regions_equals_translated_circle() {
        let g = Geometry::Circle(Circle::new(P::new(1.0, 1.0), 3.0));
        let delta = V::new(5.0, -3.0);
        assert_eq!(g.stretched(&[], delta), g.translated(delta));
    }

    #[test]
    fn geometry_stretched_empty_regions_equals_translated_arc() {
        let g = Geometry::Arc(Arc::new(P::new(1.0, 1.0), 3.0, 0.0, 1.0));
        let delta = V::new(5.0, -3.0);
        assert_eq!(g.stretched(&[], delta), g.translated(delta));
    }

    #[test]
    fn geometry_stretched_empty_regions_equals_translated_polyline() {
        let g = Geometry::Polyline(Polyline::rectangle(P::new(0.0, 0.0), P::new(2.0, 3.0)));
        let delta = V::new(5.0, -3.0);
        assert_eq!(g.stretched(&[], delta), g.translated(delta));
    }

    #[test]
    fn geometry_stretched_line_only_a_inside_moves_a_only() {
        // a は region の内側、b はその外側。
        let g = line_geom((1.0, 1.0), (50.0, 50.0));
        let moved = g.stretched(&[region()], V::new(5.0, 5.0));
        assert_eq!(moved, line_geom((6.0, 6.0), (50.0, 50.0)));
    }

    #[test]
    fn geometry_stretched_line_only_b_inside_moves_b_only() {
        let g = line_geom((50.0, 50.0), (1.0, 1.0));
        let moved = g.stretched(&[region()], V::new(5.0, 5.0));
        assert_eq!(moved, line_geom((50.0, 50.0), (6.0, 6.0)));
    }

    #[test]
    fn geometry_stretched_line_both_inside_equals_translated() {
        let g = line_geom((1.0, 1.0), (2.0, 2.0));
        let delta = V::new(5.0, 5.0);
        assert_eq!(g.stretched(&[region()], delta), g.translated(delta));
    }

    #[test]
    fn geometry_stretched_line_neither_inside_is_unchanged() {
        let g = line_geom((50.0, 50.0), (60.0, 60.0));
        let moved = g.stretched(&[region()], V::new(5.0, 5.0));
        assert_eq!(moved, g);
    }

    #[test]
    fn geometry_stretched_polyline_moves_only_vertices_inside_region() {
        // 4 頂点のうち先頭 2 つだけが region の内側。
        let g = Geometry::Polyline(Polyline::new(
            vec![
                P::new(1.0, 1.0),
                P::new(2.0, 2.0),
                P::new(50.0, 50.0),
                P::new(60.0, 60.0),
            ],
            false,
        ));
        let moved = g.stretched(&[region()], V::new(3.0, 3.0));
        let expected = Geometry::Polyline(Polyline::new(
            vec![
                P::new(4.0, 4.0),
                P::new(5.0, 5.0),
                P::new(50.0, 50.0),
                P::new(60.0, 60.0),
            ],
            false,
        ));
        assert_eq!(moved, expected);
    }

    #[test]
    fn geometry_stretched_polyline_preserves_closed_flag_and_vertex_count() {
        let g = Geometry::Polyline(Polyline::rectangle(P::new(0.0, 0.0), P::new(2.0, 3.0)));
        let moved = g.stretched(&[region()], V::new(1.0, 1.0));
        match (&g, &moved) {
            (Geometry::Polyline(orig), Geometry::Polyline(m)) => {
                assert_eq!(m.closed, orig.closed);
                assert_eq!(m.vertex_count(), orig.vertex_count());
            }
            _ => panic!("Polyline のはず"),
        }
    }

    #[test]
    fn geometry_stretched_circle_center_inside_moves_whole_circle_radius_unchanged() {
        let g = Geometry::Circle(Circle::new(P::new(5.0, 5.0), 3.0));
        let moved = g.stretched(&[region()], V::new(2.0, 2.0));
        assert_eq!(moved, Geometry::Circle(Circle::new(P::new(7.0, 7.0), 3.0)));
    }

    #[test]
    fn geometry_stretched_circle_center_outside_is_unchanged_even_when_circumference_crosses_region(
    ) {
        // 中心は region の外だが、半径が大きいため円周は region と交差する。
        let g = Geometry::Circle(Circle::new(P::new(15.0, 5.0), 8.0));
        let moved = g.stretched(&[region()], V::new(2.0, 2.0));
        assert_eq!(moved, g);
    }

    #[test]
    fn geometry_stretched_arc_center_inside_moves_radius_and_angles_unchanged() {
        let g = Geometry::Arc(Arc::new(P::new(5.0, 5.0), 3.0, 0.1, 1.5));
        let moved = g.stretched(&[region()], V::new(2.0, 2.0));
        assert_eq!(
            moved,
            Geometry::Arc(Arc::new(P::new(7.0, 7.0), 3.0, 0.1, 1.5))
        );
    }

    #[test]
    fn geometry_stretched_arc_center_outside_is_unchanged() {
        let g = Geometry::Arc(Arc::new(P::new(50.0, 50.0), 3.0, 0.1, 1.5));
        let moved = g.stretched(&[region()], V::new(2.0, 2.0));
        assert_eq!(moved, g);
    }

    #[test]
    fn geometry_stretched_multiple_regions_point_in_either_moves() {
        // a は region() の内側、b は other_region() の内側。両方とも動くはず。
        let g = line_geom((1.0, 1.0), (105.0, 105.0));
        let moved = g.stretched(&[region(), other_region()], V::new(1.0, 1.0));
        assert_eq!(moved, line_geom((2.0, 2.0), (106.0, 106.0)));
    }

    #[test]
    fn geometry_stretched_multiple_overlapping_regions_moves_point_exactly_once() {
        // 2 つの重なり合う範囲の両方に入る点でも、delta が 2 重にはかからない。
        let overlap_a = Aabb::new(P::new(0.0, 0.0), P::new(10.0, 10.0));
        let overlap_b = Aabb::new(P::new(5.0, 5.0), P::new(15.0, 15.0));
        let g = line_geom((7.0, 7.0), (100.0, 100.0));
        let moved = g.stretched(&[overlap_a, overlap_b], V::new(3.0, 3.0));
        assert_eq!(moved, line_geom((10.0, 10.0), (100.0, 100.0)));
    }

    #[test]
    fn geometry_stretched_large_coordinates() {
        let big_region = Aabb::new(P::new(0.0, 0.0), P::new(2.0e6, 2.0e6));
        let g = line_geom((1.0e6, 1.0e6), (5.0e6, 5.0e6));
        let moved = g.stretched(&[big_region], V::new(1.0e6, 0.0));
        assert_eq!(moved, line_geom((2.0e6, 1.0e6), (5.0e6, 5.0e6)));
    }

    #[test]
    fn entity_stretched_keeps_layer_and_color() {
        let e = Entity::new(line_geom((1.0, 1.0), (50.0, 50.0)), LayerId::ZERO);
        let moved = e.stretched(&[region()], V::new(2.0, 2.0));
        assert_eq!(moved.layer, e.layer);
        assert_eq!(moved.color, e.color);
        assert_eq!(moved.geom, line_geom((3.0, 3.0), (50.0, 50.0)));
    }

    // ---- rotated ------------------------------------------------------------

    #[test]
    fn geometry_rotated_line_quarter_turn() {
        let g = line_geom((1.0, 0.0), (1.0, 1.0));
        let rotated = g.rotated(P::ORIGIN, FRAC_PI_2);
        let l = line_at(&rotated);
        assert!(l.a.eq_tol(P::new(0.0, 1.0)));
        assert!(l.b.eq_tol(P::new(-1.0, 1.0)));
    }

    #[test]
    fn geometry_rotated_by_zero_is_identity() {
        let g = line_geom((2.0, 3.0), (5.0, 7.0));
        let rotated = g.rotated(P::new(1.0, 1.0), 0.0);
        let l = line_at(&rotated);
        assert!(l.a.eq_tol(P::new(2.0, 3.0)));
        assert!(l.b.eq_tol(P::new(5.0, 7.0)));
    }

    #[test]
    fn geometry_rotated_by_tau_is_identity() {
        let g = line_geom((2.0, 3.0), (5.0, 7.0));
        let rotated = g.rotated(P::new(1.0, 1.0), TAU);
        let l = line_at(&rotated);
        assert!(l.a.eq_tol(P::new(2.0, 3.0)));
        assert!(l.b.eq_tol(P::new(5.0, 7.0)));
    }

    #[test]
    fn geometry_rotated_circle_center_moves_radius_unchanged() {
        let g = Geometry::Circle(Circle::new(P::new(1.0, 0.0), 5.0));
        let rotated = g.rotated(P::ORIGIN, FRAC_PI_2);
        let c = circle_at(&rotated);
        assert!(c.center.eq_tol(P::new(0.0, 1.0)));
        assert!(eq_len(c.radius, 5.0));
    }

    #[test]
    fn geometry_rotated_arc_shifts_both_angles() {
        let g = Geometry::Arc(Arc::new(P::new(1.0, 0.0), 2.0, 0.2, 1.0));
        let rotated = g.rotated(P::ORIGIN, FRAC_PI_4);
        let a = arc_at(&rotated);
        let expected_center = P::ORIGIN + V::new(1.0, 0.0).rotated(FRAC_PI_4);
        assert!(a.center.eq_tol(expected_center));
        assert!(eq_len(a.radius, 2.0));
        assert!(eq_angle(a.start_angle, 0.2 + FRAC_PI_4));
        assert!(eq_angle(a.end_angle, 1.0 + FRAC_PI_4));
    }

    #[test]
    fn geometry_rotated_polyline_preserves_closed_and_vertex_count() {
        let g = Geometry::Polyline(Polyline::rectangle(P::new(0.0, 0.0), P::new(2.0, 3.0)));
        let rotated = g.rotated(P::new(1.0, 1.0), FRAC_PI_2);
        let p = polyline_at(&rotated);
        assert!(p.closed);
        assert_eq!(p.vertex_count(), 4);
    }

    // ---- scaled ---------------------------------------------------------------

    #[test]
    fn geometry_scaled_line() {
        let g = line_geom((2.0, 0.0), (2.0, 1.0));
        let scaled = g.scaled(P::ORIGIN, 3.0);
        let l = line_at(&scaled);
        assert!(l.a.eq_tol(P::new(6.0, 0.0)));
        assert!(l.b.eq_tol(P::new(6.0, 3.0)));
    }

    #[test]
    fn geometry_scaled_by_one_is_identity() {
        let g = line_geom((2.0, 3.0), (5.0, 7.0));
        let scaled = g.scaled(P::new(1.0, 1.0), 1.0);
        let l = line_at(&scaled);
        assert!(l.a.eq_tol(P::new(2.0, 3.0)));
        assert!(l.b.eq_tol(P::new(5.0, 7.0)));
    }

    #[test]
    fn geometry_scaled_zero_factor_returns_self() {
        let g = line_geom((2.0, 3.0), (5.0, 7.0));
        assert_eq!(g.scaled(P::new(5.0, 5.0), 0.0), g);
    }

    #[test]
    fn geometry_scaled_nan_factor_returns_self() {
        let g = line_geom((2.0, 3.0), (5.0, 7.0));
        assert_eq!(g.scaled(P::new(5.0, 5.0), f64::NAN), g);
    }

    #[test]
    fn geometry_scaled_infinite_factor_returns_self() {
        let g = line_geom((2.0, 3.0), (5.0, 7.0));
        assert_eq!(g.scaled(P::new(5.0, 5.0), f64::INFINITY), g);
    }

    #[test]
    fn geometry_scaled_circle_radius_scales() {
        let g = Geometry::Circle(Circle::new(P::new(2.0, 0.0), 4.0));
        let scaled = g.scaled(P::ORIGIN, 2.0);
        let c = circle_at(&scaled);
        assert!(c.center.eq_tol(P::new(4.0, 0.0)));
        assert!(eq_len(c.radius, 8.0));
    }

    #[test]
    fn geometry_scaled_arc_radius_scales_angles_unchanged() {
        let g = Geometry::Arc(Arc::new(P::new(2.0, 0.0), 4.0, 0.3, 1.2));
        let scaled = g.scaled(P::ORIGIN, 2.0);
        let a = arc_at(&scaled);
        assert!(a.center.eq_tol(P::new(4.0, 0.0)));
        assert!(eq_len(a.radius, 8.0));
        assert!(eq_angle(a.start_angle, 0.3));
        assert!(eq_angle(a.end_angle, 1.2));
    }

    #[test]
    fn geometry_scaled_polyline_preserves_closed_and_vertex_count() {
        let g = Geometry::Polyline(Polyline::rectangle(P::new(0.0, 0.0), P::new(2.0, 3.0)));
        let scaled = g.scaled(P::new(1.0, 1.0), 2.0);
        let p = polyline_at(&scaled);
        assert!(p.closed);
        assert_eq!(p.vertex_count(), 4);
    }

    // ---- mirrored ---------------------------------------------------------------

    fn x_axis() -> Line {
        Line::new(P::ORIGIN, P::new(1.0, 0.0))
    }

    #[test]
    fn geometry_mirrored_line_across_x_axis() {
        let g = line_geom((2.0, 3.0), (5.0, -1.0));
        let mirrored = g.mirrored(&x_axis());
        let l = line_at(&mirrored);
        assert!(l.a.eq_tol(P::new(2.0, -3.0)));
        assert!(l.b.eq_tol(P::new(5.0, 1.0)));
    }

    #[test]
    fn geometry_mirrored_twice_is_identity() {
        let axis = Line::new(P::new(0.0, 1.0), P::new(1.0, 1.0));
        let g = line_geom((2.0, 3.0), (5.0, -1.0));
        let twice = g.mirrored(&axis).mirrored(&axis);
        let l = line_at(&twice);
        assert!(l.a.eq_tol(P::new(2.0, 3.0)));
        assert!(l.b.eq_tol(P::new(5.0, -1.0)));
    }

    #[test]
    fn geometry_mirrored_degenerate_axis_returns_self() {
        let axis = Line::new(P::new(3.0, 3.0), P::new(3.0, 3.0));
        let g = line_geom((2.0, 3.0), (5.0, -1.0));
        assert_eq!(g.mirrored(&axis), g);
    }

    #[test]
    fn geometry_mirrored_circle_center_only_radius_unchanged() {
        let g = Geometry::Circle(Circle::new(P::new(2.0, 3.0), 5.0));
        let mirrored = g.mirrored(&x_axis());
        let c = circle_at(&mirrored);
        assert!(c.center.eq_tol(P::new(2.0, -3.0)));
        assert!(eq_len(c.radius, 5.0));
    }

    /// 鏡像で掃引方向が反転する落とし穴を明示的な角度でピン留めするテスト。
    ///
    /// 第一象限の四半円（0 〜 π/2, CCW）を X 軸で反転すると、
    /// 第四象限の四半円（-π/2 〜 0, CCW）になるはず。
    /// start/end を入れ替えずに単純に反射しただけだと、円の残り 3/4
    /// （補角の弧）になってしまう。
    #[test]
    fn geometry_mirrored_arc_swaps_start_and_end() {
        let g = Geometry::Arc(Arc::new(P::ORIGIN, 1.0, 0.0, FRAC_PI_2));
        let mirrored = g.mirrored(&x_axis());
        let a = arc_at(&mirrored);
        assert!(a.center.eq_tol(P::ORIGIN));
        assert!(eq_len(a.radius, 1.0));
        assert!(eq_angle(a.start_angle, -FRAC_PI_2));
        assert!(eq_angle(a.end_angle, 0.0));
    }

    #[test]
    fn geometry_mirrored_arc_twice_is_identity() {
        let axis = Line::new(P::new(0.0, 2.0), P::new(3.0, 2.0));
        let g = Geometry::Arc(Arc::new(P::new(1.0, 1.0), 3.0, 0.4, 2.1));
        let twice = g.mirrored(&axis).mirrored(&axis);
        let a = arc_at(&twice);
        assert!(a.center.eq_tol(P::new(1.0, 1.0)));
        assert!(eq_len(a.radius, 3.0));
        assert!(eq_angle(a.start_angle, 0.4));
        assert!(eq_angle(a.end_angle, 2.1));
    }

    #[test]
    fn geometry_mirrored_polyline_preserves_closed_and_vertex_count() {
        let g = Geometry::Polyline(Polyline::rectangle(P::new(0.0, 0.0), P::new(2.0, 3.0)));
        let mirrored = g.mirrored(&x_axis());
        let p = polyline_at(&mirrored);
        assert!(p.closed);
        assert_eq!(p.vertex_count(), 4);
    }

    // ---- Entity: rotated / scaled / mirrored -----------------------------------

    #[test]
    fn entity_rotated_keeps_layer_and_color() {
        let e = Entity::new(line_geom((1.0, 0.0), (1.0, 1.0)), LayerId::ZERO);
        let rotated = e.rotated(P::ORIGIN, FRAC_PI_2);
        assert_eq!(rotated.layer, e.layer);
        assert_eq!(rotated.color, e.color);
        let l = line_at(&rotated.geom);
        assert!(l.a.eq_tol(P::new(0.0, 1.0)));
    }

    #[test]
    fn entity_scaled_keeps_layer_and_color() {
        let e = Entity::new(line_geom((2.0, 0.0), (2.0, 1.0)), LayerId::ZERO);
        let scaled = e.scaled(P::ORIGIN, 3.0);
        assert_eq!(scaled.layer, e.layer);
        assert_eq!(scaled.color, e.color);
        let l = line_at(&scaled.geom);
        assert!(l.a.eq_tol(P::new(6.0, 0.0)));
    }

    #[test]
    fn entity_mirrored_keeps_layer_and_color() {
        let e = Entity::new(line_geom((2.0, 3.0), (5.0, -1.0)), LayerId::ZERO);
        let mirrored = e.mirrored(&x_axis());
        assert_eq!(mirrored.layer, e.layer);
        assert_eq!(mirrored.color, e.color);
        let l = line_at(&mirrored.geom);
        assert!(l.a.eq_tol(P::new(2.0, -3.0)));
    }
}
