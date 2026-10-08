//! 図面を SVG の文字列にする（自前。依存ゼロ）。
//!
//! # 約束
//!
//! - 座標は [`Fit`] を通して画像 px にしてから書く（Y の反転もそこ）。SVG の座標系は画像の px そのもの
//!   （`viewBox="0 0 幅 高さ"`）なので、PNG にするときも拡大縮小しない
//! - 非表示のレイヤは描かない。表示範囲に入らない図形は書かない（件数は [`Stats`] に残す）
//! - 色は ACI（`AciColor::rgb`）。**背景が明るいときだけ、白（ACI 7）を黒にする**（ACI の慣習。
//!   白い背景に白い線を引くと消える）
//! - 破線は線種の `dash_pattern_px`（画像 px で与えられる）をそのまま `stroke-dasharray` にする
//! - 文字は描かない（図面に文字の図形は無い）
//! - 無限に伸びる作図線と、表示範囲を横切る線分・ポリラインは、表示範囲で切ってから書く
//!   （巨大な座標をラスタライザへ渡さないため。アプリの描画と同じ）
//! - インスタンスは `component::resolve` で図形に展開し、**インスタンス自身のレイヤ・色・線種**で描く
//!   （アプリの描画と同じ。定義の中の図形が持つレイヤは使わない）
//! - 円・円弧は `<circle>` / `A` コマンドで書く。ただし半径が [`MAX_NATIVE_ARC_RADIUS_PX`] を超えるとき
//!   （拡大して大きな円の一部だけを見るとき）は折れ線にして切る。半径が桁違いに大きい円弧を
//!   ラスタライザの精度に任せると、見えている部分が数 px ずれる

use std::f64::consts::{PI, TAU};
use std::fmt::Write as _;

use cad_core::component;
use cad_core::component::DefinitionTable;
use cad_core::geom::tolerance::{eq_angle, wrap_2pi, wrap_signed};
use cad_core::geom::{Aabb, Arc, Circle, Line, Point2};
use cad_core::{AciColor, Document, Entity, Geometry};

use super::fit::Fit;
use crate::ids::format_id;

/// 線幅 [px]。
const STROKE_PX: f64 = 1.5;

/// 折れ線で近似するときの許容誤差 [px]（アプリの描画と同じ）。
const TESSELLATION_SAGITTA_PX: f64 = 0.3;

/// これより半径が大きい円・円弧は、`<circle>` / `A` ではなく折れ線にする [px]。
pub const MAX_NATIVE_ARC_RADIUS_PX: f64 = 1.0e5;

/// 巨大な円弧の角度の窓を広げる割合（窓の両端に、窓の幅のこの割合ずつ）。
const WINDOW_PAD_RATIO: f64 = 0.05;

/// 出力する SVG の最大バイト数。超えたら範囲を絞らせる。
pub const MAX_SVG_BYTES: usize = 8 * 1024 * 1024;

/// 背景。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Background {
    /// アプリと同じ暗い背景（egui の `extreme_bg_color` = 灰 10）。
    Dark,
    /// 白い背景。
    Light,
}

impl Background {
    /// 名前（道具の引数 `background`）から。
    ///
    /// # Errors
    ///
    /// `dark` / `light` でないとき。
    pub fn parse(name: &str) -> Result<Self, String> {
        match name {
            "dark" => Ok(Self::Dark),
            "light" => Ok(Self::Light),
            other => Err(format!("background {other} は使えません（dark / light）")),
        }
    }

    /// 道具の引数に使う名前。
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }

    fn fill(self) -> &'static str {
        match self {
            Self::Dark => "#0a0a0a",
            Self::Light => "#ffffff",
        }
    }

    /// 図形の色。
    fn stroke(self, color: AciColor) -> String {
        let (r, g, b) = if self == Self::Light && color == AciColor::WHITE {
            (0, 0, 0)
        } else {
            color.rgb()
        };
        format!("#{r:02x}{g:02x}{b:02x}")
    }
}

/// 描いたもの・描かなかったものの数（図面の図形 = トップレベルの図形の単位）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// 描いた図形の数。
    pub drawn: usize,
    /// 表示範囲に入らなくて描かなかった図形の数。
    pub outside_view: usize,
    /// 非表示のレイヤにあって描かなかった図形の数。
    pub hidden: usize,
}

/// 図面を描く SVG と、描いた数。
///
/// `serial` は図面の通し番号（各要素に `data-id`（図形 ID）を付けるため）。
///
/// # Errors
///
/// 出力が [`MAX_SVG_BYTES`] を超えたとき（範囲を絞らせる）。
pub fn draw(
    doc: &Document,
    serial: u64,
    fit: &Fit,
    background: Background,
) -> Result<(String, Stats), String> {
    let (w, h) = (fit.width(), fit.height());
    let mut out = String::new();
    // 書式文字列への書き込みは String に対しては失敗しない。
    let _ = write!(
        out,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\">\n\
<rect width=\"{w}\" height=\"{h}\" fill=\"{}\"/>\n\
<g fill=\"none\" stroke-width=\"{STROKE_PX}\" stroke-linejoin=\"round\">\n",
        background.fill()
    );

    let defs = doc.definitions();
    // 線幅ぶんだけ広げて切る・省く（境界上の図形が消えないように）。
    let cull = fit.view().expanded(fit.px_to_len(STROKE_PX));
    let mut stats = Stats::default();
    for (id, entity) in doc.entities().iter() {
        if !doc.layers().is_entity_visible(entity) {
            stats.hidden += 1;
            continue;
        }
        if !cull.intersects(&entity.bbox(defs)) {
            stats.outside_view += 1;
            continue;
        }
        let style = Style::of(doc, entity, background);
        let before = out.len();
        let mut ctx = Ctx {
            out: &mut out,
            fit,
            cull,
            style: &style,
            data_id: format_id(serial, id),
            defs,
        };
        ctx.geometry(&entity.geom);
        // 作図線やポリラインは、範囲の中に何も書かないことがある。書いたものだけを「描いた」と数える。
        if out.len() > before {
            stats.drawn += 1;
        } else {
            stats.outside_view += 1;
        }
        if out.len() > MAX_SVG_BYTES {
            return Err(format!(
                "SVG が {} MiB を超えました。region で範囲を絞るか、width / height を小さくしてください",
                MAX_SVG_BYTES / (1024 * 1024)
            ));
        }
    }
    out.push_str("</g>\n</svg>\n");
    Ok((out, stats))
}

/// 図形の見た目。
struct Style {
    color: String,
    dashes: &'static [f64],
}

impl Style {
    fn of(doc: &Document, entity: &Entity, background: Background) -> Self {
        let layers = doc.layers();
        Self {
            color: background.stroke(layers.resolve_color(entity)),
            dashes: layers.resolve_linetype(entity).dash_pattern_px(),
        }
    }

    /// 要素の属性（先頭に空白つき）。
    fn attrs(&self, data_id: &str) -> String {
        let mut s = format!(" stroke=\"{}\"", self.color);
        if !self.dashes.is_empty() {
            let list: Vec<String> = self.dashes.iter().map(|d| num(*d)).collect();
            let _ = write!(s, " stroke-dasharray=\"{}\"", list.join(" "));
        }
        let _ = write!(s, " data-id=\"{data_id}\"");
        s
    }
}

/// 1 つの図形を書くための持ち物。
struct Ctx<'a> {
    out: &'a mut String,
    fit: &'a Fit,
    /// 表示範囲を線幅ぶん広げたもの（モデル座標）。
    cull: Aabb,
    style: &'a Style,
    data_id: String,
    defs: &'a DefinitionTable,
}

impl Ctx<'_> {
    fn geometry(&mut self, g: &Geometry) {
        match g {
            Geometry::Line(l) => {
                if let Some(seg) = l.clip_to(self.cull) {
                    self.line(&seg);
                }
            }
            Geometry::Circle(c) => self.circle(c),
            Geometry::Arc(a) => self.arc(a),
            Geometry::Xline(x) => {
                if let Some(seg) = x.clip_to(self.cull) {
                    self.line(&seg);
                }
            }
            Geometry::Polyline(p) => {
                let points: Vec<Point2> = p.vertices.clone();
                self.path_through(&points, p.closed);
            }
            Geometry::Instance(i) => {
                for inner in component::resolve(i, self.defs) {
                    // 展開した図形ごとにも、表示範囲との交わりを見る（インスタンス全体の範囲は広い）。
                    if self.cull.intersects(&inner.bbox(self.defs)) {
                        self.geometry(&inner);
                    }
                }
            }
        }
    }

    fn line(&mut self, l: &Line) {
        let (a, b) = (self.fit.to_px(l.a), self.fit.to_px(l.b));
        let _ = writeln!(
            self.out,
            "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\"{}/>",
            num(a.x),
            num(a.y),
            num(b.x),
            num(b.y),
            self.style.attrs(&self.data_id)
        );
    }

    fn circle(&mut self, c: &Circle) {
        if !self.cull.intersects(&c.bbox()) {
            return;
        }
        if self.fit.len_to_px(c.radius) > MAX_NATIVE_ARC_RADIUS_PX {
            self.huge_arc(&Arc::new(c.center, c.radius, 0.0, TAU));
            return;
        }
        let o = self.fit.to_px(c.center);
        let _ = writeln!(
            self.out,
            "<circle cx=\"{}\" cy=\"{}\" r=\"{}\"{}/>",
            num(o.x),
            num(o.y),
            num(self.fit.len_to_px(c.radius)),
            self.style.attrs(&self.data_id)
        );
    }

    fn arc(&mut self, a: &Arc) {
        if !self.cull.intersects(&a.bbox()) {
            return;
        }
        let sweep = a.sweep();
        // 1 周の円弧は、始点と終点が重なって SVG の `A` では描けない。円として書く。
        if eq_angle(sweep, TAU) {
            self.circle(&Circle::new(a.center, a.radius));
            return;
        }
        if self.fit.len_to_px(a.radius) > MAX_NATIVE_ARC_RADIUS_PX {
            self.huge_arc(a);
            return;
        }
        let (s, e) = (
            self.fit.to_px(a.start_point()),
            self.fit.to_px(a.end_point()),
        );
        let r = num(self.fit.len_to_px(a.radius));
        // モデルの反時計回りは、画像でも（Y を反転しているので）反時計回りに見える。
        // SVG の sweep-flag は 1 が「角度が増える向き」= 画像（Y が下向き）では時計回りなので、0 になる。
        let large = i32::from(sweep > PI);
        let _ = writeln!(
            self.out,
            "<path d=\"M{} {}A{r} {r} 0 {large} 0 {} {}\"{}/>",
            num(s.x),
            num(s.y),
            num(e.x),
            num(e.y),
            self.style.attrs(&self.data_id)
        );
    }

    /// 半径が桁違いに大きい円・円弧。**表示範囲に見える角度の窓だけ**を折れ線にして書く。
    ///
    /// 全周を折れ線にすると、分割数の上限（`cad-core` の `tessellate`）で 1 辺が長くなり、
    /// 見えている部分が円から大きくずれる（半径 1e7 で数単位）。窓だけなら分割は少なくて済み、
    /// 許容誤差（[`TESSELLATION_SAGITTA_PX`]）を守れる。
    fn huge_arc(&mut self, a: &Arc) {
        let Some((window_start, window_sweep)) = angular_window(a.center, self.cull) else {
            return;
        };
        let sagitta = self.fit.px_to_len(TESSELLATION_SAGITTA_PX);
        // 窓を円弧の始点からの角度に直し、円弧の範囲 [0, sweep] と重なる区間を取る。
        // 窓が 0 をまたぐ（2π 戻した窓も見る）ので、2 通り試す。
        let offset = wrap_2pi(window_start - a.start_angle);
        let sweep = a.sweep();
        for shift in [0.0, -TAU] {
            let lo = (offset + shift).max(0.0);
            let hi = (offset + shift + window_sweep).min(sweep);
            // 掃引がほぼ 0 の円弧は `Arc::sweep` が 1 周と読むので、入れない。
            if !eq_angle(hi - lo, 0.0) && hi > lo {
                let piece = Arc::new(a.center, a.radius, a.start_angle + lo, a.start_angle + hi);
                self.path_through(&piece.tessellate(sagitta), false);
            }
        }
    }

    /// 点列を結ぶ折れ線。1 本ごとに表示範囲で切り、つながっている区間は 1 つの `<path>` にする。
    fn path_through(&mut self, points: &[Point2], closed: bool) {
        let n = points.len();
        let count = match n {
            0 | 1 => return,
            _ if closed => n,
            _ => n - 1,
        };
        let mut d = String::new();
        let mut pen: Option<Point2> = None;
        let mut all_inside = true;
        for i in 0..count {
            let seg = Line::new(points[i], points[(i + 1) % n]);
            let Some(clipped) = seg.clip_to(self.cull) else {
                all_inside = false;
                pen = None;
                continue;
            };
            let continues = pen.is_some_and(|p| p.eq_tol(clipped.a));
            if !continues {
                let a = self.fit.to_px(clipped.a);
                let _ = write!(d, "M{} {}", num(a.x), num(a.y));
            }
            let b = self.fit.to_px(clipped.b);
            let _ = write!(d, "L{} {}", num(b.x), num(b.y));
            // 端が切られていたら、ここでペンを上げる。
            all_inside &= clipped.a.eq_tol(seg.a) && clipped.b.eq_tol(seg.b);
            pen = clipped.b.eq_tol(seg.b).then_some(clipped.b);
        }
        if d.is_empty() {
            return;
        }
        if closed && all_inside {
            d.push('Z');
        }
        let _ = writeln!(
            self.out,
            "<path d=\"{d}\"{}/>",
            self.style.attrs(&self.data_id)
        );
    }
}

/// 中心 `center` から矩形 `rect` が見える角度の窓 `(開始角, 掃引角)` [rad]。
///
/// 中心が矩形の中や縁にあるとき、窓が半周以上になるとき（= 巨大な円が矩形を横切ることがありえないとき）は `None`。
/// 窓は少し広げてある（端の折れ線の頂点が矩形の中へ入るように）。
fn angular_window(center: Point2, rect: Aabb) -> Option<(f64, f64)> {
    let toward = |p: Point2| (p.y - center.y).atan2(p.x - center.x);
    let reference = toward(rect.center());
    let corners = [
        rect.min,
        Point2::new(rect.max.x, rect.min.y),
        rect.max,
        Point2::new(rect.min.x, rect.max.y),
    ];
    let (lo, hi) = corners
        .iter()
        .map(|c| wrap_signed(toward(*c) - reference))
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), d| {
            (lo.min(d), hi.max(d))
        });
    let span = hi - lo;
    if !span.is_finite() || span >= PI {
        return None;
    }
    let pad = span * WINDOW_PAD_RATIO;
    Some((reference + lo - pad, span + 2.0 * pad))
}

/// 数値を SVG に書く形にする（小数 3 桁まで、末尾の 0 は落とす）。
fn num(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" || s.is_empty() {
        "0".to_owned()
    } else {
        s.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use cad_core::command::{
        AddEntities, AddLayer, DefineComponent, InsertInstance, SetLayerProperties,
    };
    use cad_core::component::Placement;
    use cad_core::geom::{Polyline, Xline};
    use cad_core::layer::LineType;
    use cad_core::{ColorSpec, LayerId};

    use super::super::raster;
    use super::*;

    fn region(x0: f64, y0: f64, x1: f64, y1: f64) -> Aabb {
        Aabb::new(Point2::new(x0, y0), Point2::new(x1, y1))
    }

    /// 0..10 の範囲を 100 × 100 px にする（10 px / 単位。Y は 10 - y を 10 倍）。
    fn fit10() -> Fit {
        Fit::new(region(0.0, 0.0, 10.0, 10.0), 100, 100).unwrap()
    }

    fn doc_with(entities: Vec<Entity>) -> Document {
        let mut doc = Document::new();
        doc.apply(Box::new(AddEntities::many("ADD", entities)))
            .unwrap();
        doc
    }

    fn add_layer(doc: &mut Document, name: &str, color: u8) -> LayerId {
        doc.apply(Box::new(AddLayer::new(name.to_owned(), AciColor(color))))
            .unwrap();
        doc.layers().by_name(name).unwrap()
    }

    fn on_zero(g: Geometry) -> Entity {
        Entity::new(g, LayerId::ZERO)
    }

    fn line(x0: f64, y0: f64, x1: f64, y1: f64) -> Geometry {
        Geometry::Line(Line::new(Point2::new(x0, y0), Point2::new(x1, y1)))
    }

    fn arc(cx: f64, cy: f64, r: f64, start_deg: f64, end_deg: f64) -> Geometry {
        Geometry::Arc(Arc::new(
            Point2::new(cx, cy),
            r,
            start_deg.to_radians(),
            end_deg.to_radians(),
        ))
    }

    fn svg_of(doc: &Document, fit: &Fit, bg: Background) -> (String, Stats) {
        let (svg, stats) = draw(doc, 1, fit, bg).unwrap();
        raster::parse(&svg).unwrap_or_else(|e| panic!("SVG を読めない: {e}\n{svg}"));
        (svg, stats)
    }

    /// 図形の要素の行（背景の rect と外側の g を除く）。
    fn elements(svg: &str) -> Vec<&str> {
        svg.lines()
            .filter(|l| {
                l.starts_with("<line") || l.starts_with("<circle") || l.starts_with("<path")
            })
            .collect()
    }

    /// 要素の属性 `name="..."` の値。
    fn attr<'a>(element: &'a str, name: &str) -> &'a str {
        let key = format!(" {name}=\"");
        let start = element
            .find(&key)
            .unwrap_or_else(|| panic!("{name} が無い: {element}"))
            + key.len();
        let len = element[start..].find('"').unwrap();
        &element[start..start + len]
    }

    /// 画素 (x, y) の周り 1 px 以内に、背景と違う色があるか。
    fn inked(pm: &resvg::tiny_skia::Pixmap, x: i64, y: i64, bg: (u8, u8, u8)) -> bool {
        for dy in -1..=1 {
            for dx in -1..=1 {
                let (px, py) = (x + dx, y + dy);
                let (Ok(px), Ok(py)) = (u32::try_from(px), u32::try_from(py)) else {
                    continue;
                };
                if let Some(c) = pm.pixel(px, py) {
                    if (c.red(), c.green(), c.blue()) != bg {
                        return true;
                    }
                }
            }
        }
        false
    }

    const DARK_BG: (u8, u8, u8) = (10, 10, 10);

    #[allow(
        clippy::cast_possible_truncation,
        reason = "テストの画素位置（0..100 の範囲）を丸めるだけ"
    )]
    fn to_px_index(v: f64) -> i64 {
        v.round() as i64
    }

    #[test]
    fn line_is_written_in_image_pixels_with_y_flipped() {
        let doc = doc_with(vec![on_zero(line(0.0, 0.0, 10.0, 4.0))]);
        let (svg, stats) = svg_of(&doc, &fit10(), Background::Dark);
        let els = elements(&svg);
        assert_eq!(els.len(), 1, "{svg}");
        let l = els[0];
        // モデルの (0, 0) は画像の左下、(10, 4) は右端で下から 40 px。
        assert_eq!(attr(l, "x1"), "0");
        assert_eq!(attr(l, "y1"), "100");
        assert_eq!(attr(l, "x2"), "100");
        assert_eq!(attr(l, "y2"), "60");
        assert_eq!(
            stats,
            Stats {
                drawn: 1,
                outside_view: 0,
                hidden: 0
            }
        );
    }

    #[test]
    fn svg_has_the_requested_size_and_a_background() {
        let doc = Document::new();
        let fit = Fit::new(region(0.0, 0.0, 10.0, 10.0), 320, 200).unwrap();
        let (svg, _) = svg_of(&doc, &fit, Background::Light);
        assert!(
            svg.contains("width=\"320\" height=\"200\" viewBox=\"0 0 320 200\""),
            "{svg}"
        );
        assert!(svg.contains("fill=\"#ffffff\""), "{svg}");
        let (svg, _) = svg_of(&doc, &fit, Background::Dark);
        assert!(svg.contains("fill=\"#0a0a0a\""), "{svg}");
        assert!(elements(&svg).is_empty());
    }

    #[test]
    fn circle_is_a_circle_element() {
        let doc = doc_with(vec![on_zero(Geometry::Circle(Circle::new(
            Point2::new(5.0, 5.0),
            2.0,
        )))]);
        let (svg, _) = svg_of(&doc, &fit10(), Background::Dark);
        let els = elements(&svg);
        assert_eq!(els.len(), 1);
        assert!(els[0].starts_with("<circle"), "{}", els[0]);
        assert_eq!(attr(els[0], "cx"), "50");
        assert_eq!(attr(els[0], "cy"), "50");
        assert_eq!(attr(els[0], "r"), "20");
    }

    #[test]
    fn arc_uses_the_a_command_with_the_ccw_sweep_flag() {
        // 中心 (5, 5)・半径 3 の 0°..90°。始点 (8, 5) → 画像 (80, 50)、終点 (5, 8) → (50, 20)。
        let doc = doc_with(vec![on_zero(arc(5.0, 5.0, 3.0, 0.0, 90.0))]);
        let (svg, _) = svg_of(&doc, &fit10(), Background::Dark);
        let d = attr(elements(&svg)[0], "d");
        assert_eq!(d, "M80 50A30 30 0 0 0 50 20", "{svg}");

        // 270° の弧は large-arc-flag が 1。終点 (5, 2) → (50, 80)。
        let doc = doc_with(vec![on_zero(arc(5.0, 5.0, 3.0, 0.0, 270.0))]);
        let (svg, _) = svg_of(&doc, &fit10(), Background::Dark);
        let d = attr(elements(&svg)[0], "d");
        assert_eq!(d, "M80 50A30 30 0 1 0 50 80", "{svg}");
    }

    /// SVG のフラグの向きを、式ではなく画素で確かめる（Y の反転で sweep の向きが変わる点）。
    #[test]
    fn arc_is_drawn_on_the_correct_side() {
        let c = 5.0;
        let at = |deg: f64, r: f64| -> (i64, i64) {
            let (s, co) = deg.to_radians().sin_cos();
            // fit10: x = 10 * x、y = 10 * (10 - y)
            (
                to_px_index(10.0 * (c + r * co)),
                to_px_index(10.0 * (10.0 - (c + r * s))),
            )
        };
        // 90° の弧: 45° の位置は描かれ、反対側の 225° は描かれない。
        let doc = doc_with(vec![on_zero(arc(c, c, 3.0, 0.0, 90.0))]);
        let (svg, _) = svg_of(&doc, &fit10(), Background::Dark);
        let pm = raster::svg_to_pixmap(&svg);
        let (x, y) = at(45.0, 3.0);
        assert!(inked(&pm, x, y, DARK_BG), "45° が描かれていない");
        let (x, y) = at(225.0, 3.0);
        assert!(
            !inked(&pm, x, y, DARK_BG),
            "225° に描かれている（向きが逆）"
        );

        // 270° の弧（0°→270°）: 135° は描かれ、欠けた 315° は描かれない。
        let doc = doc_with(vec![on_zero(arc(c, c, 3.0, 0.0, 270.0))]);
        let (svg, _) = svg_of(&doc, &fit10(), Background::Dark);
        let pm = raster::svg_to_pixmap(&svg);
        for deg in [45.0, 135.0, 225.0] {
            let (x, y) = at(deg, 3.0);
            assert!(inked(&pm, x, y, DARK_BG), "{deg}° が描かれていない");
        }
        let (x, y) = at(315.0, 3.0);
        assert!(
            !inked(&pm, x, y, DARK_BG),
            "315° に描かれている（large-arc の向きが逆）"
        );

        // 開始角が終了角より大きい（0 をまたぐ）弧: 300°→60° は 120° の弧で 0° を通る。
        let doc = doc_with(vec![on_zero(arc(c, c, 3.0, 300.0, 60.0))]);
        let (svg, _) = svg_of(&doc, &fit10(), Background::Dark);
        let pm = raster::svg_to_pixmap(&svg);
        let (x, y) = at(0.0, 3.0);
        assert!(inked(&pm, x, y, DARK_BG), "0° が描かれていない");
        let (x, y) = at(180.0, 3.0);
        assert!(!inked(&pm, x, y, DARK_BG), "180° に描かれている");
    }

    /// 1 周の円弧（開始角 = 終了角）は円として書く。`A` では始点と終点が重なって何も描かれない。
    #[test]
    fn full_turn_arc_is_drawn_as_a_circle() {
        for (s, e) in [(0.0, 360.0), (30.0, 30.0), (0.0, 0.0)] {
            let doc = doc_with(vec![on_zero(arc(5.0, 5.0, 3.0, s, e))]);
            let (svg, _) = svg_of(&doc, &fit10(), Background::Dark);
            let els = elements(&svg);
            assert_eq!(els.len(), 1, "{s}..{e}");
            assert!(els[0].starts_with("<circle"), "{s}..{e}: {}", els[0]);
            assert_eq!(attr(els[0], "r"), "30");
            let pm = raster::svg_to_pixmap(&svg);
            assert!(
                inked(&pm, 80, 50, DARK_BG),
                "{s}..{e}: 円周が描かれていない"
            );
        }
    }

    #[test]
    fn xline_is_clipped_to_the_view() {
        // 原点が桁違いに遠くても、書く座標は画像の近くに収まる。
        let far = Xline::at_angle(Point2::new(1.0e9, 1.0e9), 45.0_f64.to_radians());
        let horizontal = Xline::horizontal(Point2::new(-3.0e8, 5.0));
        let doc = doc_with(vec![
            on_zero(Geometry::Xline(far)),
            on_zero(Geometry::Xline(horizontal)),
        ]);
        let (svg, stats) = svg_of(&doc, &fit10(), Background::Dark);
        let els = elements(&svg);
        assert_eq!(els.len(), 2, "{svg}");
        for e in &els {
            for key in ["x1", "y1", "x2", "y2"] {
                let v: f64 = attr(e, key).parse().unwrap();
                assert!(
                    (-5.0..=105.0).contains(&v),
                    "{key}={v} が画像の外へ伸びている: {e}"
                );
            }
        }
        // 水平線は画像の端から端まで（線幅ぶんはみ出す）。
        let h = els[1];
        assert_eq!(attr(h, "y1"), "50");
        assert_eq!(attr(h, "y2"), "50");
        assert!(attr(h, "x1").parse::<f64>().unwrap() <= 0.0);
        assert!(attr(h, "x2").parse::<f64>().unwrap() >= 100.0);
        assert_eq!(stats.drawn, 2);
    }

    /// 範囲を外れる作図線は書かず、「描いた」とも数えない。
    #[test]
    fn xline_missing_the_view_is_not_counted_as_drawn() {
        let off = Xline::horizontal(Point2::new(0.0, 500.0));
        let doc = doc_with(vec![on_zero(Geometry::Xline(off))]);
        let (svg, stats) = svg_of(&doc, &fit10(), Background::Dark);
        assert!(elements(&svg).is_empty(), "{svg}");
        assert_eq!(stats.drawn, 0);
        assert_eq!(stats.outside_view, 1);
    }

    #[test]
    fn polyline_closed_or_open() {
        let tri = |closed| {
            Geometry::Polyline(Polyline::new(
                vec![
                    Point2::new(1.0, 1.0),
                    Point2::new(9.0, 1.0),
                    Point2::new(5.0, 8.0),
                ],
                closed,
            ))
        };
        let doc = doc_with(vec![on_zero(tri(true))]);
        let (svg, _) = svg_of(&doc, &fit10(), Background::Dark);
        let els = elements(&svg);
        assert_eq!(els.len(), 1);
        // 頂点 3 つ + 閉じる辺。閉じる印 Z がある。
        assert_eq!(attr(els[0], "d"), "M10 90L90 90L50 20L10 90Z");

        let doc = doc_with(vec![on_zero(tri(false))]);
        let (svg, _) = svg_of(&doc, &fit10(), Background::Dark);
        assert_eq!(attr(elements(&svg)[0], "d"), "M10 90L90 90L50 20");
    }

    #[test]
    fn polyline_crossing_the_view_edge_is_clipped() {
        let doc = doc_with(vec![on_zero(Geometry::Polyline(Polyline::new(
            vec![
                Point2::new(2.0, 2.0),
                Point2::new(1.0e9, 5.0),
                Point2::new(1.0e9, 8.0),
                Point2::new(2.0, 8.0),
            ],
            true,
        )))]);
        let (svg, _) = svg_of(&doc, &fit10(), Background::Dark);
        let d = elements(&svg)
            .iter()
            .map(|e| attr(e, "d"))
            .collect::<Vec<_>>()
            .join(" ");
        for n in d
            .split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
            .filter(|t| !t.is_empty())
        {
            let v: f64 = n.parse().unwrap();
            assert!((-5.0..=105.0).contains(&v), "{v} が画像の外: {d}");
        }
        assert!(!d.contains('Z'), "切った多角形を閉じてはいけない: {d}");
    }

    #[test]
    fn hidden_layers_are_not_drawn() {
        let mut doc = Document::new();
        let hidden = add_layer(&mut doc, "HIDE", 1);
        doc.apply(Box::new(SetLayerProperties::new(hidden).visible(false)))
            .unwrap();
        doc.apply(Box::new(AddEntities::many(
            "ADD",
            vec![
                Entity::new(line(0.0, 0.0, 5.0, 5.0), hidden),
                on_zero(line(0.0, 10.0, 5.0, 5.0)),
            ],
        )))
        .unwrap();
        let (svg, stats) = svg_of(&doc, &fit10(), Background::Dark);
        assert_eq!(elements(&svg).len(), 1, "{svg}");
        assert_eq!(stats.hidden, 1);
        assert_eq!(stats.drawn, 1);
    }

    #[test]
    fn entities_outside_the_view_are_left_out() {
        let doc = doc_with(vec![
            on_zero(line(1.0, 1.0, 2.0, 2.0)),
            on_zero(line(100.0, 100.0, 200.0, 200.0)),
            on_zero(Geometry::Circle(Circle::new(Point2::new(-50.0, 0.0), 3.0))),
        ]);
        let (svg, stats) = svg_of(&doc, &fit10(), Background::Dark);
        assert_eq!(elements(&svg).len(), 1);
        assert_eq!(stats.drawn, 1);
        assert_eq!(stats.outside_view, 2);
    }

    #[test]
    fn linetype_becomes_a_dash_array_in_pixels() {
        let mut doc = Document::new();
        let dashed = add_layer(&mut doc, "D", 3);
        let center = add_layer(&mut doc, "C", 3);
        doc.apply(Box::new(
            SetLayerProperties::new(dashed).linetype(LineType::Dashed),
        ))
        .unwrap();
        doc.apply(Box::new(
            SetLayerProperties::new(center).linetype(LineType::Center),
        ))
        .unwrap();
        doc.apply(Box::new(AddEntities::many(
            "ADD",
            vec![
                Entity::new(line(0.0, 1.0, 9.0, 1.0), dashed),
                Entity::new(line(0.0, 2.0, 9.0, 2.0), center),
                on_zero(line(0.0, 3.0, 9.0, 3.0)),
            ],
        )))
        .unwrap();
        let (svg, _) = svg_of(&doc, &fit10(), Background::Dark);
        let els = elements(&svg);
        assert_eq!(els.len(), 3);
        assert_eq!(attr(els[0], "stroke-dasharray"), "6 4");
        assert_eq!(attr(els[1], "stroke-dasharray"), "12 3 3 3");
        assert!(
            !els[2].contains("stroke-dasharray"),
            "実線に破線: {}",
            els[2]
        );
    }

    #[test]
    fn colors_follow_aci_and_flip_white_on_light() {
        let mut doc = Document::new();
        let red = add_layer(&mut doc, "RED", 1);
        let mut own = Entity::new(line(0.0, 3.0, 9.0, 3.0), red);
        own.color = ColorSpec::Aci(AciColor(5));
        doc.apply(Box::new(AddEntities::many(
            "ADD",
            vec![
                Entity::new(line(0.0, 1.0, 9.0, 1.0), red),
                on_zero(line(0.0, 2.0, 9.0, 2.0)),
                own,
            ],
        )))
        .unwrap();
        let (dark, _) = svg_of(&doc, &fit10(), Background::Dark);
        let strokes: Vec<&str> = elements(&dark).iter().map(|e| attr(e, "stroke")).collect();
        // レイヤの色（赤）、既定色（白）、図形自身の色（青）。
        assert_eq!(strokes, ["#ff0000", "#ffffff", "#0000ff"]);
        let (light, _) = svg_of(&doc, &fit10(), Background::Light);
        let strokes: Vec<&str> = elements(&light).iter().map(|e| attr(e, "stroke")).collect();
        assert_eq!(strokes, ["#ff0000", "#000000", "#0000ff"]);
    }

    fn doc_with_instance(instance_layer_visible: bool) -> (Document, LayerId) {
        let mut doc = Document::new();
        let shown = add_layer(&mut doc, "INST", 2);
        doc.apply(Box::new(DefineComponent::new(
            "DEF",
            "部品",
            Point2::ORIGIN,
            vec![
                on_zero(line(0.0, 0.0, 2.0, 0.0)),
                on_zero(Geometry::Circle(Circle::new(Point2::new(1.0, 1.0), 0.5))),
            ],
        )))
        .unwrap();
        let def = doc.definitions().by_name("部品").unwrap();
        doc.apply(Box::new(InsertInstance::new(
            "INSERT",
            def,
            Placement::at(Point2::new(3.0, 4.0)),
            shown,
        )))
        .unwrap();
        if !instance_layer_visible {
            doc.apply(Box::new(SetLayerProperties::new(shown).visible(false)))
                .unwrap();
        }
        (doc, shown)
    }

    /// インスタンスは中身に展開し、インスタンス自身のレイヤの色で描く（アプリと同じ）。
    #[test]
    fn instances_are_expanded_with_the_instance_attributes() {
        let (doc, _) = doc_with_instance(true);
        let (svg, stats) = svg_of(&doc, &fit10(), Background::Dark);
        let els = elements(&svg);
        assert_eq!(els.len(), 2, "{svg}");
        assert!(els[0].starts_with("<line") && els[1].starts_with("<circle"));
        // 配置 (3, 4): 線 (3,4)-(5,4) → 画像 (30,60)-(50,60)。円の中心 (4, 5) → (40, 50)。
        assert_eq!(attr(els[0], "x1"), "30");
        assert_eq!(attr(els[0], "y1"), "60");
        assert_eq!(attr(els[0], "x2"), "50");
        assert_eq!(attr(els[1], "cx"), "40");
        assert_eq!(attr(els[1], "cy"), "50");
        // インスタンスのレイヤは黄（ACI 2）。中身が載っているレイヤ 0（白）ではない。
        assert_eq!(attr(els[0], "stroke"), "#ffff00");
        assert_eq!(attr(els[1], "stroke"), "#ffff00");
        assert_eq!(attr(els[0], "data-id"), attr(els[1], "data-id"));
        assert_eq!(stats.drawn, 1, "インスタンス 1 つ");
    }

    #[test]
    fn instance_on_a_hidden_layer_is_not_drawn() {
        let (doc, _) = doc_with_instance(false);
        let (svg, stats) = svg_of(&doc, &fit10(), Background::Dark);
        assert!(elements(&svg).is_empty(), "{svg}");
        assert_eq!(stats.hidden, 1);
    }

    /// 全種類を 1 枚に描いて、ラスタライザで読めること。
    #[test]
    fn mixed_drawing_parses() {
        let (mut doc, _) = doc_with_instance(true);
        doc.apply(Box::new(AddEntities::many(
            "ADD",
            vec![
                on_zero(line(0.0, 0.0, 10.0, 10.0)),
                on_zero(arc(5.0, 5.0, 4.0, 10.0, 200.0)),
                on_zero(Geometry::Xline(Xline::at_angle(Point2::new(2.0, 2.0), 0.3))),
                on_zero(Geometry::Polyline(Polyline::new(
                    vec![
                        Point2::new(1.0, 8.0),
                        Point2::new(3.0, 9.0),
                        Point2::new(2.0, 7.0),
                    ],
                    true,
                ))),
            ],
        )))
        .unwrap();
        let (svg, stats) = svg_of(&doc, &fit10(), Background::Dark);
        assert_eq!(stats.drawn, 5);
        assert!(elements(&svg).len() >= 6);
        let pm = raster::svg_to_pixmap(&svg);
        assert_eq!((pm.width(), pm.height()), (100, 100));
    }

    /// 半径が桁違いに大きい円は、見えている部分の位置が合う。
    #[test]
    fn huge_circle_stays_accurate_where_it_crosses_the_view() {
        // 中心 (5 - 1e7, 5)・半径 1e7 の円は (5, 5) を通り、ここでの接線は垂直。
        let r = 1.0e7;
        let doc = doc_with(vec![on_zero(Geometry::Circle(Circle::new(
            Point2::new(5.0 - r, 5.0),
            r,
        )))]);
        let (svg, stats) = svg_of(&doc, &fit10(), Background::Dark);
        assert_eq!(stats.drawn, 1);
        assert!(
            !svg.contains("<circle"),
            "巨大な円を <circle> にしてはいけない"
        );
        assert!(
            svg.len() < 20_000,
            "窓だけを書くはずが {} バイト",
            svg.len()
        );
        let pm = raster::svg_to_pixmap(&svg);
        // 画像の縦の中ほどずっと x = 50 の列に線がある（全周を粗い折れ線にすると 10 単位以上ずれる）。
        for y in (10..90).step_by(10) {
            assert!(inked(&pm, 50, y, DARK_BG), "y={y} で x=50 に線が無い");
        }
        assert!(!inked(&pm, 20, 50, DARK_BG) && !inked(&pm, 80, 50, DARK_BG));
    }

    /// 巨大な円弧でも、円弧の範囲の外は描かない（見える窓と円弧の重なりだけを書く）。
    #[test]
    fn huge_arc_respects_its_own_range() {
        let r = 1.0e7;
        // 中心 (5 - 1e7, 5)。0° 付近で (5, 5) を通り、1e-3° は約 174 単位（画像の外まで）。
        let center = Point2::new(5.0 - r, 5.0);
        let huge = |start_deg: f64, end_deg: f64| {
            let g = Geometry::Arc(Arc::new(
                center,
                r,
                start_deg.to_radians(),
                end_deg.to_radians(),
            ));
            let (svg, stats) = svg_of(&doc_with(vec![on_zero(g)]), &fit10(), Background::Dark);
            (raster::svg_to_pixmap(&svg), stats)
        };
        // 画像の上半分（モデルの y が 5 より上）は px y = 10..40、下半分は 60..90。
        let upper = |pm: &resvg::tiny_skia::Pixmap| (1..5).all(|k| inked(pm, 50, 10 * k, DARK_BG));
        let lower = |pm: &resvg::tiny_skia::Pixmap| (6..10).all(|k| inked(pm, 50, 10 * k, DARK_BG));
        let none = |pm: &resvg::tiny_skia::Pixmap, rows: std::ops::Range<i64>| {
            rows.map(|k| 10 * k).all(|y| !inked(pm, 50, y, DARK_BG))
        };

        // (5, 5) を通って上下に伸びる弧。
        let (pm, stats) = huge(-0.001, 0.001);
        assert_eq!(stats.drawn, 1);
        assert!(upper(&pm) && lower(&pm));
        // 0 をまたぐ書き方（359.999° → 0.001°）でも同じ。
        let (pm, _) = huge(359.999, 0.001);
        assert!(upper(&pm) && lower(&pm));
        // (5, 5) で終わる弧: 上半分には描かない。
        let (pm, _) = huge(-0.001, 0.0);
        assert!(lower(&pm) && none(&pm, 1..5), "終点を越えて描いている");
        // (5, 5) から始まる弧: 下半分には描かない。
        let (pm, _) = huge(0.0, 0.001);
        assert!(upper(&pm) && none(&pm, 6..10), "始点より前に描いている");

        // 範囲の外の弧は、書かない。
        let away = Geometry::Arc(Arc::new(
            center,
            r,
            180.0_f64.to_radians(),
            270.0_f64.to_radians(),
        ));
        let (svg, stats) = svg_of(&doc_with(vec![on_zero(away)]), &fit10(), Background::Dark);
        assert_eq!(stats.drawn, 0, "{svg}");
    }

    #[test]
    fn numbers_are_short_and_never_negative_zero() {
        assert_eq!(num(0.0), "0");
        assert_eq!(num(-0.0), "0");
        assert_eq!(num(-0.0001), "0");
        assert_eq!(num(12.5), "12.5");
        assert_eq!(num(3.0), "3");
        assert_eq!(num(1.23456), "1.235");
        assert_eq!(num(-7.25), "-7.25");
        assert_eq!(num(100.0), "100");
    }
}
