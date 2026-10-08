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
//! - 出力の大きさの上限は [`draw`] の引数 `max_bytes`。**書きながら見て**、超えた時点で打ち切る
//!   （頂点が数百万のポリライン 1 本でも、上限を大きく超えるぶんは書かない）
//! - 円・円弧は、表示範囲が円の内側に丸ごと入って線が 1 本も見えないときは書かず、「描いた」にも数えない

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
/// `tag` は図面の印（各要素に `data-id`（図形 ID）を付けるため）。
///
/// `max_bytes` は SVG の文字列の大きさの上限。書きながら見て、超えたら打ち切って拒む。
///
/// # Errors
///
/// 出力が `max_bytes` を超えたとき（範囲を絞らせる）。
pub fn draw(
    doc: &Document,
    tag: crate::ids::DrawingTag,
    fit: &Fit,
    background: Background,
    max_bytes: usize,
) -> Result<(String, Stats), String> {
    let mut out = String::new();
    let stats = write_svg(&mut out, doc, tag, fit, background, max_bytes)?;
    Ok((out, stats))
}

/// [`draw`] の本体。`out` に書く（失敗したときは書きかけが残る。テストが打ち切りの位置を見る）。
fn write_svg(
    out: &mut String,
    doc: &Document,
    tag: crate::ids::DrawingTag,
    fit: &Fit,
    background: Background,
    max_bytes: usize,
) -> Result<Stats, String> {
    let (w, h) = (fit.width(), fit.height());
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
            out,
            fit,
            cull,
            style: &style,
            data_id: format_id(tag, id),
            defs,
            max_bytes,
        };
        ctx.geometry(&entity.geom);
        // 作図線やポリラインは、範囲の中に何も書かないことがある。書いたものだけを「描いた」と数える。
        if out.len() > before {
            stats.drawn += 1;
        } else {
            stats.outside_view += 1;
        }
        if out.len() > max_bytes {
            return Err(too_large(max_bytes));
        }
    }
    out.push_str("</g>\n</svg>\n");
    // 終わりのタグを足したぶんも含めて、上限を守る（ちょうど上限の大きさは通す）。
    if out.len() > max_bytes {
        return Err(too_large(max_bytes));
    }
    Ok(stats)
}

/// 上限を超えたときの説明。SVG の大きさは画像の大きさにほとんど依らない（桁が数文字変わるだけ）ので、
/// 促すのは `region` で絞ることだけ。
fn too_large(max_bytes: usize) -> String {
    format!(
        "SVG が上限 {} を超えました。region で範囲を絞ってください（図形の数が多すぎます）",
        crate::render::format_bytes(max_bytes)
    )
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
    /// `out` の大きさの上限。
    max_bytes: usize,
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
            Geometry::Polyline(p) => self.path_through(&p.vertices, p.closed),
            Geometry::Instance(i) => {
                for inner in component::resolve(i, self.defs) {
                    if self.is_full() {
                        return;
                    }
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

    /// 上限に達したか（以降は何も書かない）。
    fn is_full(&self) -> bool {
        self.out.len() > self.max_bytes
    }

    fn circle(&mut self, c: &Circle) {
        if !self.cull.intersects(&c.bbox()) || !ring_touches(c.center, c.radius, self.cull) {
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
        if !self.cull.intersects(&a.bbox()) || !ring_touches(a.center, a.radius, self.cull) {
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
    ///
    /// `out` へ直接書き、上限（`max_bytes`）を超えたらその場で打ち切る（頂点が数百万あっても、
    /// 上限を超えたぶんは書かない）。
    fn path_through(&mut self, points: &[Point2], closed: bool) {
        let n = points.len();
        let count = match n {
            0 | 1 => return,
            _ if closed => n,
            _ => n - 1,
        };
        let start = self.out.len();
        self.out.push_str("<path d=\"");
        let body = self.out.len();
        let mut pen: Option<Point2> = None;
        let mut all_inside = true;
        for i in 0..count {
            if self.is_full() {
                return;
            }
            let seg = Line::new(points[i], points[(i + 1) % n]);
            let Some(clipped) = seg.clip_to(self.cull) else {
                all_inside = false;
                pen = None;
                continue;
            };
            let continues = pen.is_some_and(|p| p.eq_tol(clipped.a));
            if !continues {
                let a = self.fit.to_px(clipped.a);
                let _ = write!(self.out, "M{} {}", num(a.x), num(a.y));
            }
            let b = self.fit.to_px(clipped.b);
            let _ = write!(self.out, "L{} {}", num(b.x), num(b.y));
            // 端が切られていたら、ここでペンを上げる。
            all_inside &= clipped.a.eq_tol(seg.a) && clipped.b.eq_tol(seg.b);
            pen = clipped.b.eq_tol(seg.b).then_some(clipped.b);
        }
        if self.out.len() == body {
            // 範囲の中に何も書かなかった。
            self.out.truncate(start);
            return;
        }
        if closed && all_inside {
            self.out.push('Z');
        }
        let _ = writeln!(self.out, "\"{}/>", self.style.attrs(&self.data_id));
    }
}

/// 半径 `radius` の円周が、矩形 `rect` と交わりうるか（矩形が円の内側に丸ごと入るときと、
/// 円が矩形から離れているときは `false`）。円周上の点までの距離は、矩形の最も近い点から
/// 最も遠い角までの範囲を取る。
///
/// 円弧にも使う（弧の範囲までは見ない。円周が通らなければ弧も通らない、という十分条件）。
fn ring_touches(center: Point2, radius: f64, rect: Aabb) -> bool {
    let nearest = Point2::new(
        center.x.clamp(rect.min.x, rect.max.x),
        center.y.clamp(rect.min.y, rect.max.y),
    );
    let far_x = (center.x - rect.min.x)
        .abs()
        .max((center.x - rect.max.x).abs());
    let far_y = (center.y - rect.min.y)
        .abs()
        .max((center.y - rect.max.y).abs());
    let (near, far) = (center.dist(nearest), far_x.hypot(far_y));
    near <= radius && radius <= far
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

    /// テストの図面の印。
    const TAG: crate::ids::DrawingTag = crate::ids::DrawingTag {
        session: 0,
        serial: 1,
    };

    /// テストで使う、十分に大きい上限。
    const TEST_MAX_BYTES: usize = 64 * 1024 * 1024;

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
        let (svg, stats) = draw(doc, TAG, fit, bg, TEST_MAX_BYTES).unwrap();
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

    /// 書いた結果の大きさ（上限なしで書く）。
    fn written_len(doc: &Document, fit: &Fit) -> usize {
        draw(doc, TAG, fit, Background::Dark, TEST_MAX_BYTES)
            .unwrap()
            .0
            .len()
    }

    /// 上限: ちょうどなら通り、1 バイト超えれば拒む。拒んでも図面は変わらない。
    #[test]
    fn size_limit_is_exact_and_explains_what_to_do() {
        let doc = doc_with(
            (0..50)
                .map(|i| on_zero(line(0.0, f64::from(i) * 0.1, 9.0, 3.0)))
                .collect(),
        );
        let fit = fit10();
        let size = written_len(&doc, &fit);
        let revision = doc.revision();
        assert!(draw(&doc, TAG, &fit, Background::Dark, size).is_ok());
        let e = draw(&doc, TAG, &fit, Background::Dark, size - 1).unwrap_err();
        assert!(
            e.contains("SVG") && e.contains("上限") && e.contains("region"),
            "{e}"
        );
        assert!(
            !e.contains("width"),
            "SVG の大きさは width / height に依らない: {e}"
        );
        assert_eq!(doc.revision(), revision);
    }

    /// 頂点が数百万のポリライン 1 本でも、上限を超えたところで書くのをやめる
    /// （書き終えてから数えるのではない）。
    #[test]
    fn huge_polyline_stops_being_written_at_the_limit() {
        // 画像の中で上下に振れる 300 万頂点。
        let vertices: Vec<Point2> = (0..3_000_000_u32)
            .map(|i| Point2::new(f64::from(i % 10), f64::from(i % 7) + 1.0))
            .collect();
        let doc = doc_with(vec![on_zero(Geometry::Polyline(Polyline::new(
            vertices, false,
        )))]);
        let limit = 100_000;
        let mut out = String::new();
        let e = write_svg(&mut out, &doc, TAG, &fit10(), Background::Dark, limit).unwrap_err();
        assert!(e.contains("上限") && e.contains("region"), "{e}");
        // 上限を超えた時点で止まる: 数百万頂点ぶん（数十 MB）を書いていない。
        assert!(out.len() < limit + 200, "{} バイトまで書いた", out.len());
    }

    /// 展開すると大量になるインスタンスも、上限で止まる。
    #[test]
    fn many_geometries_stop_at_the_limit() {
        let doc = doc_with(
            (0..5000)
                .map(|_| on_zero(line(0.0, 1.0, 9.0, 3.0)))
                .collect(),
        );
        let mut out = String::new();
        let e = write_svg(&mut out, &doc, TAG, &fit10(), Background::Dark, 10_000).unwrap_err();
        assert!(e.contains("上限"), "{e}");
        assert!(out.len() < 10_000 + 300, "{} バイト", out.len());
    }

    /// 中身の多いインスタンスを多数置いた図面は、**インスタンスを展開する途中でも**上限で止まる
    /// （図形ごとの検査だけだと、上限を超えるのはインスタンスを 1 つ書き終えてから。中身 2000 本なら数百 KB 書き過ぎる）。
    #[test]
    fn instances_stop_being_expanded_at_the_limit() {
        let mut doc = Document::new();
        let contents: Vec<Entity> = (0..2000_u32)
            .map(|i| on_zero(line(0.0, f64::from(i % 9) + 0.5, 9.0, 3.0)))
            .collect();
        doc.apply(Box::new(DefineComponent::new(
            "DEF",
            "密",
            Point2::ORIGIN,
            contents,
        )))
        .unwrap();
        let def = doc.definitions().by_name("密").unwrap();
        for _ in 0..20 {
            doc.apply(Box::new(InsertInstance::new(
                "INSERT",
                def,
                Placement::at(Point2::ORIGIN),
                LayerId::ZERO,
            )))
            .unwrap();
        }
        // インスタンス 1 個ぶん（中身 2000 本）の SVG の大きさ。
        let one = {
            let mut single = String::new();
            let _ = write_svg(
                &mut single,
                &doc,
                TAG,
                &fit10(),
                Background::Dark,
                usize::MAX,
            );
            single.len() / 20
        };
        let limit = 10_000;
        assert!(
            one > limit,
            "インスタンス 1 個が上限より大きいこと（{one}）"
        );
        let mut out = String::new();
        let e = write_svg(&mut out, &doc, TAG, &fit10(), Background::Dark, limit).unwrap_err();
        assert!(e.contains("上限") && e.contains("region"), "{e}");
        // 上限 + 要素 1 つぶんで止まる（インスタンス 1 個ぶんも書き過ぎない）。
        assert!(
            out.len() < limit + 300,
            "{} バイトまで書いた（インスタンス 1 個は {one} バイト）",
            out.len()
        );
    }

    /// 表示範囲が円の内側に丸ごと入るとき、円周は 1 本も見えない: 書かず、「描いた」にも数えない。
    #[test]
    fn circle_around_the_view_is_not_drawn_or_counted() {
        let around = |r: f64| Geometry::Circle(Circle::new(Point2::new(5.0, 5.0), r));
        // 範囲（0..10、線幅ぶん広げて 0.15）の角までの距離は約 7.28。
        for (r, drawn) in [(1.0, 1), (7.0, 1), (7.5, 0), (1.0e3, 0), (5.0e3, 0)] {
            let (svg, stats) = svg_of(
                &doc_with(vec![on_zero(around(r))]),
                &fit10(),
                Background::Dark,
            );
            assert_eq!(stats.drawn, drawn, "r={r}\n{svg}");
            assert_eq!(stats.outside_view, 1 - drawn, "r={r}");
            assert_eq!(elements(&svg).len(), drawn, "r={r}");
        }
        // 1e5 px（= 1e4 単位）を超える巨大な円（折れ線にする側）も数えない。
        let (_, stats) = svg_of(
            &doc_with(vec![on_zero(around(2.0e4))]),
            &fit10(),
            Background::Dark,
        );
        assert_eq!((stats.drawn, stats.outside_view), (0, 1));
        // 円周が範囲を横切る位置に中心がある巨大な円は、これまでどおり描く。
        let far = Geometry::Circle(Circle::new(Point2::new(5.0 - 2.0e4, 5.0), 2.0e4));
        let (_, stats) = svg_of(&doc_with(vec![on_zero(far)]), &fit10(), Background::Dark);
        assert_eq!(stats.drawn, 1);
    }

    #[test]
    fn arc_around_the_view_is_not_counted() {
        let (svg, stats) = svg_of(
            &doc_with(vec![on_zero(arc(5.0, 5.0, 500.0, 0.0, 90.0))]),
            &fit10(),
            Background::Dark,
        );
        assert_eq!((stats.drawn, stats.outside_view), (0, 1), "{svg}");
        assert!(elements(&svg).is_empty());
    }

    #[test]
    fn ring_touches_covers_inside_crossing_and_outside() {
        let rect = region(0.0, 0.0, 10.0, 10.0);
        let c = Point2::new(5.0, 5.0);
        assert!(ring_touches(c, 3.0, rect)); // 内側の円（範囲に収まる）
        assert!(ring_touches(c, 6.0, rect)); // 辺を横切る
        assert!(!ring_touches(c, 8.0, rect)); // 範囲を囲む
        let outside = Point2::new(30.0, 5.0);
        assert!(!ring_touches(outside, 10.0, rect)); // 離れている
        assert!(ring_touches(outside, 25.0, rect)); // 範囲を横切る
        assert!(!ring_touches(outside, 40.0, rect)); // 範囲を囲む
    }

    /// 白い背景で黒に寄せるのは白（ACI 7）だけにしてある。ほかの ACI に「白に近い色」が無いことを固定する
    /// （`AciColor::rgb` の対応表が広がって白に近い色が増えたら、ここが落ちて見直しを促す。
    /// Issue #90 の 5: ACI 255 は今の対応表では灰 160 で、白ではない）。
    #[test]
    fn only_aci_7_is_near_white_in_the_palette() {
        let luma = |c: AciColor| {
            let (r, g, b) = c.rgb();
            0.2126 * f64::from(r) + 0.7152 * f64::from(g) + 0.0722 * f64::from(b)
        };
        // 黄（ACI 2、輝度 約 237）は色そのものが意味を持つので、白に寄せる側に含めない。
        let near_white: Vec<u8> = (0..=255_u8)
            .filter(|n| luma(AciColor(*n)) > 240.0)
            .collect();
        assert_eq!(near_white, [7]);
        assert_eq!(Background::Light.stroke(AciColor(255)), "#a0a0a0");
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
