//! 図面の描画（SVG / PNG）。道具 `render`（`tools/render.rs`）の下請け。
//!
//! | モジュール | 役割 |
//! |---|---|
//! | [`fit`] | モデル座標 ↔ 画像 px の変換（Y の反転。**ここに 1 か所だけ**）と画像の大きさの上限 |
//! | [`svg`] | 図面を SVG の文字列にする（自前。依存ゼロ） |
//! | [`raster`] | SVG → PNG（resvg。`f32` はこのクレートのソースに出てこない） |
//! | [`base64`] | PNG を MCP の画像として返すための符号化 |
//!
//! 図面は読むだけ（`&Document`）。派生データを `Document` に置かない。

pub mod base64;
pub mod fit;
pub mod raster;
pub mod svg;

use cad_core::geom::tolerance::is_zero_len;
use cad_core::geom::{Aabb, Point2};
use cad_core::Document;

pub use fit::Fit;
pub use svg::{Background, Stats};

/// 返す形式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// PNG だけ。
    Png,
    /// SVG だけ。
    Svg,
    /// 両方。
    Both,
}

impl Format {
    /// 名前（道具の引数 `format`）から。
    ///
    /// # Errors
    ///
    /// `png` / `svg` / `both` でないとき。
    pub fn parse(name: &str) -> Result<Self, String> {
        match name {
            "png" => Ok(Self::Png),
            "svg" => Ok(Self::Svg),
            "both" => Ok(Self::Both),
            other => Err(format!("format {other} は使えません（png / svg / both）")),
        }
    }

    fn wants_png(self) -> bool {
        matches!(self, Self::Png | Self::Both)
    }

    fn wants_svg(self) -> bool {
        matches!(self, Self::Svg | Self::Both)
    }
}

/// 描画の指定。
#[derive(Clone, Copy, Debug)]
pub struct Request {
    /// 返す形式。
    pub format: Format,
    /// 画像の幅 [px]。
    pub width: u32,
    /// 画像の高さ [px]。
    pub height: u32,
    /// 見せる範囲（モデル座標）。省略すると図面の範囲に余白を付ける。
    pub region: Option<Aabb>,
    /// 背景。
    pub background: Background,
}

/// 描画の結果。
#[derive(Debug)]
pub struct Output {
    /// モデルと画像の対応（表示範囲・px/単位）。
    pub fit: Fit,
    /// 描いた数・描かなかった数。
    pub stats: Stats,
    /// 見せる範囲を図面から決めたか（`region` の指定が無かったか）。
    pub auto_region: bool,
    /// SVG（要求したとき）。
    pub svg: Option<String>,
    /// PNG（要求したとき）。
    pub png: Option<Vec<u8>>,
}

/// 図面の範囲に余白を付ける割合（長辺に対して、四方）。
const MARGIN_RATIO: f64 = 0.05;

/// 図面が空のとき（何も描けないとき）に見せる範囲の一辺の半分。
const EMPTY_HALF_SPAN: f64 = 10.0;

/// 見せる範囲の既定: 表示中の図形（インスタンスの中身を含む）の範囲に余白を付けたもの。
///
/// 作図線は含めない（無限に伸びるので。表示範囲で切って描く）。非表示のレイヤは含めない。
/// 描くものが無ければ原点まわりの一定の範囲。
#[must_use]
pub fn default_region(doc: &Document) -> Aabb {
    let defs = doc.definitions();
    let extent = doc
        .entities()
        .iter()
        .filter(|(_, e)| doc.layers().is_entity_visible(e) && e.geom.is_bounded(defs))
        .map(|(_, e)| e.bbox(defs))
        .filter(|b| !b.is_empty())
        .fold(Aabb::EMPTY, Aabb::union);
    let finite = [extent.min.x, extent.min.y, extent.max.x, extent.max.y]
        .iter()
        .all(|v| v.is_finite());
    if extent.is_empty() || !finite {
        return Aabb::new(
            Point2::new(-EMPTY_HALF_SPAN, -EMPTY_HALF_SPAN),
            Point2::new(EMPTY_HALF_SPAN, EMPTY_HALF_SPAN),
        );
    }
    // 点や水平線だけの図面は幅か高さが 0 になる。長辺も 0 なら 1 単位だけ余白を付ける。
    let span = extent.width().max(extent.height());
    let pad = if is_zero_len(span) {
        1.0
    } else {
        span * MARGIN_RATIO
    };
    extent.expanded(pad)
}

/// 図面を描く。
///
/// # Errors
///
/// 大きさ・範囲の不正、SVG が大きすぎる、PNG にできない場合（日本語の説明）。
pub fn render(doc: &Document, serial: u64, req: &Request) -> Result<Output, String> {
    let auto_region = req.region.is_none();
    let region = req.region.unwrap_or_else(|| default_region(doc));
    let fit = Fit::new(region, req.width, req.height)?;
    let (svg_text, stats) = svg::draw(doc, serial, &fit, req.background)?;
    let png = if req.format.wants_png() {
        Some(raster::svg_to_png(&svg_text)?)
    } else {
        None
    };
    Ok(Output {
        fit,
        stats,
        auto_region,
        svg: req.format.wants_svg().then_some(svg_text),
        png,
    })
}

#[cfg(test)]
mod tests {
    use cad_core::command::AddEntities;
    use cad_core::geom::tolerance::eq_len;
    use cad_core::geom::{Circle, Line};
    use cad_core::{Entity, Geometry, LayerId};

    use super::*;

    const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

    fn drawing() -> Document {
        let mut doc = Document::new();
        doc.apply(Box::new(AddEntities::many(
            "ADD",
            vec![
                Entity::new(
                    Geometry::Line(Line::new(Point2::new(-20.0, 0.0), Point2::new(30.0, 10.0))),
                    LayerId::ZERO,
                ),
                Entity::new(
                    Geometry::Circle(Circle::new(Point2::new(5.0, 5.0), 8.0)),
                    LayerId::ZERO,
                ),
            ],
        )))
        .unwrap();
        doc
    }

    fn request(format: Format, width: u32, height: u32) -> Request {
        Request {
            format,
            width,
            height,
            region: None,
            background: Background::Dark,
        }
    }

    /// PNG の IHDR（シグネチャの直後）から幅と高さ。
    fn ihdr_size(png: &[u8]) -> (u32, u32) {
        assert_eq!(png[..8], PNG_SIGNATURE);
        assert_eq!(&png[12..16], b"IHDR");
        let be = |b: &[u8]| u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
        (be(&png[16..20]), be(&png[20..24]))
    }

    #[test]
    fn png_has_the_requested_size() {
        let doc = drawing();
        for (w, h) in [(1024, 768), (16, 16), (333, 77), (4096, 64)] {
            let out = render(&doc, 1, &request(Format::Png, w, h)).unwrap();
            let png = out.png.expect("PNG を要求した");
            assert_eq!(ihdr_size(&png), (w, h));
            assert!(out.svg.is_none());
        }
    }

    #[test]
    fn largest_png_is_allowed() {
        let out = render(&drawing(), 1, &request(Format::Png, 4096, 4096)).unwrap();
        assert_eq!(ihdr_size(&out.png.unwrap()), (4096, 4096));
    }

    #[test]
    fn oversized_images_are_refused() {
        let doc = drawing();
        for (w, h) in [
            (4097, 100),
            (100, 4097),
            (100_000, 100_000),
            (0, 100),
            (15, 100),
        ] {
            let e = render(&doc, 1, &request(Format::Png, w, h)).unwrap_err();
            assert!(e.contains("4096") || e.contains("16"), "{e}");
        }
    }

    #[test]
    fn format_selects_what_is_returned() {
        let doc = drawing();
        let svg = render(&doc, 1, &request(Format::Svg, 200, 100)).unwrap();
        assert!(svg.png.is_none() && svg.svg.is_some());
        let both = render(&doc, 1, &request(Format::Both, 200, 100)).unwrap();
        assert!(both.png.is_some() && both.svg.is_some());
        assert!(Format::parse("jpeg").is_err());
    }

    /// PNG は SVG をそのままラスタにしたもの: 背景が塗られ、図形の画素がある。
    #[test]
    fn png_pixels_show_the_drawing_on_the_background() {
        let doc = drawing();
        let out = render(&doc, 1, &request(Format::Both, 200, 100)).unwrap();
        let pm = raster::svg_to_pixmap(&out.svg.unwrap());
        let corner = pm.pixel(0, 0).unwrap();
        assert_eq!((corner.red(), corner.green(), corner.blue()), (10, 10, 10));
        let inked = pm
            .pixels()
            .iter()
            .filter(|c| (c.red(), c.green(), c.blue()) != (10, 10, 10))
            .count();
        assert!(inked > 100, "図形の画素が少ない: {inked}");
    }

    #[test]
    fn default_region_covers_the_drawing_with_a_margin() {
        let doc = drawing();
        let region = default_region(&doc);
        let extent = doc.bbox();
        assert!(region.contains_aabb(&extent));
        assert!(region.width() > extent.width() && region.height() > extent.height());
    }

    #[test]
    fn empty_drawing_gets_a_fixed_default_view() {
        let region = default_region(&Document::new());
        assert!(region.width() > 0.0 && region.height() > 0.0);
        let out = render(&Document::new(), 1, &request(Format::Png, 100, 100)).unwrap();
        assert_eq!(out.stats, Stats::default());
        assert!(out.auto_region);
    }

    /// 点や水平線だけでも範囲が潰れない。
    #[test]
    fn flat_drawing_still_renders() {
        let mut doc = Document::new();
        doc.apply(Box::new(AddEntities::one(
            "ADD",
            Entity::new(
                Geometry::Line(Line::new(Point2::new(0.0, 3.0), Point2::new(10.0, 3.0))),
                LayerId::ZERO,
            ),
        )))
        .unwrap();
        let out = render(&doc, 1, &request(Format::Png, 200, 100)).unwrap();
        assert_eq!(ihdr_size(&out.png.unwrap()), (200, 100));
    }

    #[test]
    fn explicit_region_is_used_as_given() {
        let doc = drawing();
        let region = Aabb::new(Point2::new(0.0, 0.0), Point2::new(4.0, 2.0));
        let out = render(
            &doc,
            1,
            &Request {
                region: Some(region),
                ..request(Format::Svg, 400, 200)
            },
        )
        .unwrap();
        assert!(!out.auto_region);
        // 4 × 2 の範囲を 400 × 200 に: 100 px / 単位で、見える範囲は指定の範囲と一致する。
        assert!(eq_len(out.fit.pixels_per_unit(), 100.0));
        assert!(out.fit.view().contains_aabb(&region) && region.contains_aabb(&out.fit.view()));
    }
}
