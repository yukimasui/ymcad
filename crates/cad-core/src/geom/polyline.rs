//! ポリライン（連結した線分列）。

use super::aabb::Aabb;
use super::line::Line;
use super::point::{Point2, Vec2};

/// 連結した線分列。`closed` が真なら最終頂点と先頭頂点を結ぶ。
#[derive(Clone, Debug, PartialEq)]
pub struct Polyline {
    /// 頂点列。
    pub vertices: Vec<Point2>,
    /// 閉じているか（真なら最終頂点と先頭頂点を結ぶ）。
    pub closed: bool,
}

impl Polyline {
    /// 頂点列と開閉フラグからポリラインを作る。
    #[inline]
    #[must_use]
    pub fn new(vertices: Vec<Point2>, closed: bool) -> Self {
        Self { vertices, closed }
    }

    /// 対角 2 点から軸平行の矩形を作る（RECTANGLE コマンド用）。closed = true。
    #[must_use]
    pub fn rectangle(a: Point2, b: Point2) -> Self {
        let vertices = vec![a, Point2::new(b.x, a.y), b, Point2::new(a.x, b.y)];
        Self::new(vertices, true)
    }

    /// 構成する線分。`closed` なら閉じる分（最終頂点→先頭頂点）も含む。
    ///
    /// 頂点が 0 または 1 のときは 1 本も返さない（`closed` でも自己ループは作らない）。
    pub fn segments(&self) -> impl Iterator<Item = Line> + '_ {
        let n = self.vertices.len();
        let count = if n < 2 {
            0
        } else if self.closed {
            n
        } else {
            n - 1
        };
        (0..count).map(move |i| {
            let a = self.vertices[i];
            let b = self.vertices[(i + 1) % n];
            Line::new(a, b)
        })
    }

    /// 頂点数。
    #[inline]
    #[must_use]
    pub fn vertex_count(&self) -> usize {
        self.vertices.len()
    }

    /// ポリラインを包む境界ボックス。頂点が無ければ [`Aabb::EMPTY`]。
    #[must_use]
    pub fn bbox(&self) -> Aabb {
        Aabb::from_points(self.vertices.iter().copied())
    }

    /// 点 `p` から線分列への最短距離。
    ///
    /// 頂点が無ければ `f64::INFINITY`、頂点が 1 つだけならその頂点までの距離を返す。
    #[must_use]
    pub fn dist_to(&self, p: Point2) -> f64 {
        match self.vertices.len() {
            0 => f64::INFINITY,
            1 => p.dist(self.vertices[0]),
            _ => self
                .segments()
                .map(|s| s.dist_to(p))
                .fold(f64::INFINITY, f64::min),
        }
    }

    /// 全長（構成する線分の長さの総和）。
    #[must_use]
    pub fn length(&self) -> f64 {
        self.segments().map(|s| s.length()).sum()
    }

    /// 線分を 1 本も持たない（頂点 0 or 1、または全頂点が同一点）か。
    #[must_use]
    pub fn is_degenerate(&self) -> bool {
        // 頂点が 0 or 1 なら segments() は空になり、空イテレータの all() は true。
        // 全頂点が同一点なら、生成される線分がすべて退化線分になる。
        self.segments().all(|s| s.is_degenerate())
    }

    /// 平行移動した複製を作る。
    #[must_use]
    pub fn translated(&self, v: Vec2) -> Self {
        Self {
            vertices: self.vertices.iter().map(|p| *p + v).collect(),
            closed: self.closed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::tolerance::{eq_len, is_zero_len, EPS_LEN};

    #[test]
    fn rectangle_has_4_corners_and_is_closed() {
        let r = Polyline::rectangle(Point2::new(0.0, 0.0), Point2::new(10.0, 5.0));
        assert_eq!(r.vertex_count(), 4);
        assert!(r.closed);
        assert_eq!(r.vertices[0], Point2::new(0.0, 0.0));
        assert_eq!(r.vertices[1], Point2::new(10.0, 0.0));
        assert_eq!(r.vertices[2], Point2::new(10.0, 5.0));
        assert_eq!(r.vertices[3], Point2::new(0.0, 5.0));
    }

    #[test]
    fn rectangle_yields_4_segments() {
        let r = Polyline::rectangle(Point2::new(0.0, 0.0), Point2::new(10.0, 5.0));
        let segs: Vec<Line> = r.segments().collect();
        assert_eq!(segs.len(), 4);
        // 最後の線分が最終頂点から先頭頂点へ戻ること。
        assert_eq!(segs[3].a, Point2::new(0.0, 5.0));
        assert_eq!(segs[3].b, Point2::new(0.0, 0.0));
    }

    #[test]
    fn rectangle_normalizes_diagonal_direction() {
        // 対角の指定順が逆でも同じ矩形になること。
        let r = Polyline::rectangle(Point2::new(10.0, 5.0), Point2::new(0.0, 0.0));
        assert_eq!(r.bbox().min, Point2::new(0.0, 0.0));
        assert_eq!(r.bbox().max, Point2::new(10.0, 5.0));
    }

    #[test]
    fn segments_count_open_vs_closed() {
        let verts = vec![
            Point2::new(0.0, 0.0),
            Point2::new(1.0, 0.0),
            Point2::new(1.0, 1.0),
        ];
        let open = Polyline::new(verts.clone(), false);
        assert_eq!(open.segments().count(), 2);

        let closed = Polyline::new(verts, true);
        assert_eq!(closed.segments().count(), 3);
    }

    #[test]
    fn segments_zero_vertices_is_empty() {
        let p = Polyline::new(vec![], false);
        assert_eq!(p.segments().count(), 0);
        let p_closed = Polyline::new(vec![], true);
        assert_eq!(p_closed.segments().count(), 0);
    }

    #[test]
    fn segments_one_vertex_is_empty_even_when_closed() {
        let p = Polyline::new(vec![Point2::new(1.0, 1.0)], true);
        assert_eq!(p.segments().count(), 0);
        let p_open = Polyline::new(vec![Point2::new(1.0, 1.0)], false);
        assert_eq!(p_open.segments().count(), 0);
    }

    #[test]
    fn segments_all_identical_vertices_are_degenerate_lines() {
        let v = Point2::new(3.0, 3.0);
        let p = Polyline::new(vec![v, v, v], true);
        let segs: Vec<Line> = p.segments().collect();
        assert_eq!(segs.len(), 3);
        assert!(segs.iter().all(Line::is_degenerate));
    }

    #[test]
    fn dist_to_point_on_polyline() {
        let p = Polyline::new(vec![Point2::new(0.0, 0.0), Point2::new(10.0, 0.0)], false);
        assert!(is_zero_len(p.dist_to(Point2::new(5.0, 0.0))));
    }

    #[test]
    fn dist_to_point_off_to_one_side() {
        let p = Polyline::new(vec![Point2::new(0.0, 0.0), Point2::new(10.0, 0.0)], false);
        assert!(eq_len(p.dist_to(Point2::new(5.0, 3.0)), 3.0));
    }

    #[test]
    fn dist_to_near_a_vertex() {
        // 折れ曲がりの頂点付近では、頂点までの距離が最短になる。
        let p = Polyline::new(
            vec![
                Point2::new(0.0, 0.0),
                Point2::new(10.0, 0.0),
                Point2::new(10.0, 10.0),
            ],
            false,
        );
        let d = p.dist_to(Point2::new(11.0, -1.0));
        assert!(eq_len(
            d,
            Point2::new(11.0, -1.0).dist(Point2::new(10.0, 0.0))
        ));
    }

    #[test]
    fn dist_to_zero_vertices_is_infinite() {
        let p = Polyline::new(vec![], false);
        assert_eq!(p.dist_to(Point2::new(1.0, 1.0)), f64::INFINITY);
    }

    #[test]
    fn dist_to_one_vertex_is_distance_to_it() {
        let p = Polyline::new(vec![Point2::new(2.0, 3.0)], false);
        assert!(eq_len(p.dist_to(Point2::new(5.0, 7.0)), 5.0));
    }

    #[test]
    fn bbox_open_matches_extents() {
        let p = Polyline::new(
            vec![
                Point2::new(0.0, 0.0),
                Point2::new(5.0, -2.0),
                Point2::new(-1.0, 3.0),
            ],
            false,
        );
        let b = p.bbox();
        assert_eq!(b.min, Point2::new(-1.0, -2.0));
        assert_eq!(b.max, Point2::new(5.0, 3.0));
    }

    #[test]
    fn bbox_closed_same_as_open_extents() {
        let verts = vec![
            Point2::new(0.0, 0.0),
            Point2::new(5.0, -2.0),
            Point2::new(-1.0, 3.0),
        ];
        let open = Polyline::new(verts.clone(), false);
        let closed = Polyline::new(verts, true);
        assert_eq!(open.bbox(), closed.bbox());
    }

    #[test]
    fn bbox_empty_polyline_is_empty() {
        let p = Polyline::new(vec![], false);
        assert!(p.bbox().is_empty());
    }

    #[test]
    fn vertex_count_matches_len() {
        let p = Polyline::new(vec![Point2::new(0.0, 0.0), Point2::new(1.0, 1.0)], false);
        assert_eq!(p.vertex_count(), 2);
    }

    #[test]
    fn length_open_and_closed() {
        let verts = vec![
            Point2::new(0.0, 0.0),
            Point2::new(3.0, 0.0),
            Point2::new(3.0, 4.0),
        ];
        let open = Polyline::new(verts.clone(), false);
        assert!(eq_len(open.length(), 7.0));

        let closed = Polyline::new(verts, true);
        // 閉じる分（(3,4) -> (0,0) の長さ 5）が加わる。
        assert!(eq_len(closed.length(), 12.0));
    }

    #[test]
    fn is_degenerate_zero_or_one_vertex() {
        assert!(Polyline::new(vec![], false).is_degenerate());
        assert!(Polyline::new(vec![Point2::new(1.0, 1.0)], true).is_degenerate());
    }

    #[test]
    fn is_degenerate_all_identical_vertices() {
        let v = Point2::new(2.0, 2.0);
        assert!(Polyline::new(vec![v, v, v], false).is_degenerate());
        assert!(Polyline::new(vec![v, v], true).is_degenerate());
    }

    #[test]
    fn is_degenerate_false_for_real_polyline() {
        let p = Polyline::new(vec![Point2::new(0.0, 0.0), Point2::new(1.0, 0.0)], false);
        assert!(!p.is_degenerate());
    }

    #[test]
    fn translated_moves_all_vertices() {
        let p = Polyline::new(vec![Point2::new(0.0, 0.0), Point2::new(1.0, 1.0)], true);
        let moved = p.translated(Vec2::new(10.0, -5.0));
        assert_eq!(moved.vertices[0], Point2::new(10.0, -5.0));
        assert_eq!(moved.vertices[1], Point2::new(11.0, -4.0));
        assert!(moved.closed);
    }

    #[test]
    fn translated_by_zero_is_unchanged() {
        let p = Polyline::rectangle(Point2::new(0.0, 0.0), Point2::new(2.0, 2.0));
        let moved = p.translated(Vec2::ZERO);
        assert_eq!(moved, p);
    }

    #[test]
    fn large_coordinate_polyline_length_and_dist() {
        let p = Polyline::new(vec![Point2::new(0.0, 0.0), Point2::new(1e6, 0.0)], false);
        assert!(eq_len(p.length(), 1e6));
        assert!(is_zero_len(p.dist_to(Point2::new(5e5, 0.0))));
    }

    #[test]
    fn small_magnitude_polyline_is_not_degenerate() {
        let p = Polyline::new(
            vec![Point2::new(0.0, 0.0), Point2::new(EPS_LEN * 1000.0, 0.0)],
            false,
        );
        assert!(!p.is_degenerate());
    }
}
