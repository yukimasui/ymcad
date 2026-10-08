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

/// 返す PNG の最大バイト数（生のバイト列）: 3.5 MiB。
///
/// 画像は base64 にして返す（4/3 倍）。Claude の API は画像 1 枚が約 5 MB までで、超えた画像が会話の
/// 履歴に入ると、その後の要求が失敗しうる。3.5 MiB の base64 は約 4.9 MB で、これに収まる。
/// 既定の 1024 × 768 なら、密な図面でも十分に収まる（線分 2 万本で約 0.3 MB）。
pub const MAX_PNG_BYTES: usize = 3 * 1024 * 1024 + 512 * 1024;

/// 返す SVG（`format` が `svg` / `both`）の最大バイト数: 256 KiB（図形 2000 個ほど）。
///
/// SVG は LLM が本文を読む形式で、数字が多く 3 文字ほどで 1 トークンになる。256 KiB で約 8 万トークン。
/// Claude Code の MCP の出力の既定の上限（25,000 トークン = 約 80 KB）には収まらないが、それは
/// クライアントの設定（上げられる）で、ここは「人が読める範囲」ではなく「読ませようとしても無駄な大きさ」の線。
/// 8 MiB（以前の上限）は数百万トークンになり、文脈を食いつぶす。密な図面は PNG で見せ、
/// SVG は `region` で絞って要素を辿るために使わせる。
pub const MAX_RETURNED_SVG_BYTES: usize = 256 * 1024;

/// PNG だけを返すとき、ラスタにする途中の SVG の最大バイト数: 8 MiB。
///
/// この SVG は読まれず捨てられる（返るのは PNG）ので、返す SVG よりずっと大きくてよい。
/// 上限はラスタライザの時間とメモリのため。
pub const MAX_RASTER_SVG_BYTES: usize = 8 * 1024 * 1024;

// 返す SVG の上限は、途中の SVG の上限より小さい（PNG なら描ける図面を巻き添えで拒まない）。
const _: () = assert!(MAX_RETURNED_SVG_BYTES < MAX_RASTER_SVG_BYTES);

/// 大きさの上限の一式（本番は [`Limits::DEFAULT`]。テストが小さい値で境界を確かめる）。
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// 返す PNG の最大バイト数。
    pub png_bytes: usize,
    /// 返す SVG の最大バイト数。
    pub returned_svg_bytes: usize,
    /// PNG だけを返すときの、途中の SVG の最大バイト数。
    pub raster_svg_bytes: usize,
}

impl Limits {
    /// 本番の上限。
    pub const DEFAULT: Self = Self {
        png_bytes: MAX_PNG_BYTES,
        returned_svg_bytes: MAX_RETURNED_SVG_BYTES,
        raster_svg_bytes: MAX_RASTER_SVG_BYTES,
    };
}

/// バイト数を `1 MiB` / `3.5 MiB` の形にする（エラーの文言用）。
#[must_use]
pub fn format_bytes(bytes: usize) -> String {
    const KIB: usize = 1024;
    const MIB: usize = 1024 * 1024;
    if bytes >= MIB {
        // 小数 1 桁（10 倍して整数で割る。`as` による浮動小数点への変換を避ける）。
        let tenths = bytes * 10 / MIB;
        if tenths % 10 == 0 {
            format!("{} MiB", tenths / 10)
        } else {
            format!("{}.{} MiB", tenths / 10, tenths % 10)
        }
    } else {
        format!("{} KiB", bytes.div_ceil(KIB))
    }
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
/// 大きさ・範囲の不正、SVG が大きすぎる（[`MAX_RETURNED_SVG_BYTES`] / [`MAX_RASTER_SVG_BYTES`]）、
/// PNG にできない、PNG が大きすぎる（[`MAX_PNG_BYTES`]）場合（日本語の説明。図面は変わらない）。
pub fn render(
    doc: &Document,
    tag: crate::ids::DrawingTag,
    req: &Request,
) -> Result<Output, String> {
    render_within(doc, tag, req, &Limits::DEFAULT)
}

/// [`render`] を、指定の上限で。
///
/// # Errors
///
/// [`render`] と同じ。
pub fn render_within(
    doc: &Document,
    tag: crate::ids::DrawingTag,
    req: &Request,
    limits: &Limits,
) -> Result<Output, String> {
    let auto_region = req.region.is_none();
    let region = req.region.unwrap_or_else(|| default_region(doc));
    let fit = Fit::new(region, req.width, req.height)?;
    // SVG を返すなら返す上限、PNG だけなら途中の SVG の上限。
    let svg_limit = if req.format.wants_svg() {
        limits.returned_svg_bytes
    } else {
        limits.raster_svg_bytes
    };
    let (svg_text, stats) = svg::draw(doc, tag, &fit, req.background, svg_limit)?;
    let png = if req.format.wants_png() {
        let png = raster::svg_to_png(&svg_text)?;
        if png.len() > limits.png_bytes {
            return Err(format!(
                "PNG が {}（上限 {}）になりました。width / height を小さくするか、region で範囲を絞ってください",
                format_bytes(png.len()),
                format_bytes(limits.png_bytes)
            ));
        }
        Some(png)
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

    /// テストの図面の印。
    const TAG: crate::ids::DrawingTag = crate::ids::DrawingTag {
        session: 0,
        serial: 1,
    };

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
            let out = render(&doc, TAG, &request(Format::Png, w, h)).unwrap();
            let png = out.png.expect("PNG を要求した");
            assert_eq!(ihdr_size(&png), (w, h));
            assert!(out.svg.is_none());
        }
    }

    #[test]
    fn largest_png_is_allowed() {
        let out = render(&drawing(), TAG, &request(Format::Png, 4096, 4096)).unwrap();
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
            let e = render(&doc, TAG, &request(Format::Png, w, h)).unwrap_err();
            assert!(e.contains("4096") || e.contains("16"), "{e}");
        }
    }

    #[test]
    fn format_selects_what_is_returned() {
        let doc = drawing();
        let svg = render(&doc, TAG, &request(Format::Svg, 200, 100)).unwrap();
        assert!(svg.png.is_none() && svg.svg.is_some());
        let both = render(&doc, TAG, &request(Format::Both, 200, 100)).unwrap();
        assert!(both.png.is_some() && both.svg.is_some());
        assert!(Format::parse("jpeg").is_err());
    }

    /// PNG は SVG をそのままラスタにしたもの: 背景が塗られ、図形の画素がある。
    #[test]
    fn png_pixels_show_the_drawing_on_the_background() {
        let doc = drawing();
        let out = render(&doc, TAG, &request(Format::Both, 200, 100)).unwrap();
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

    /// PNG が上限を超える密な図面の線分の数（デバッグビルドでも数秒で描ける範囲）。
    const DENSE_LINES: usize = 400;

    /// 線分を `n` 本（細かく振れる斜線）足した図面。
    fn dense_drawing(n: usize) -> Document {
        let mut doc = Document::new();
        #[allow(
            clippy::cast_precision_loss,
            reason = "テストの座標（n は数万以下）を小数にするだけ"
        )]
        let lines: Vec<Entity> = (0..n)
            .map(|i| {
                let t = i as f64;
                Entity::new(
                    Geometry::Line(Line::new(
                        Point2::new(t * 0.37 % 1000.0, t * 0.91 % 1000.0),
                        Point2::new(1000.0 - t * 0.53 % 1000.0, t * 0.29 % 1000.0),
                    )),
                    LayerId::ZERO,
                )
            })
            .collect();
        doc.apply(Box::new(AddEntities::many("ADD", lines)))
            .unwrap();
        doc
    }

    /// base64 にしても Claude の API の画像の上限（5 MB）に収まる。
    #[test]
    fn png_limit_fits_the_api_image_limit_after_base64() {
        assert!(MAX_PNG_BYTES.div_ceil(3) * 4 < 5_000_000);
        assert_eq!(format_bytes(MAX_PNG_BYTES), "3.5 MiB");
        assert_eq!(format_bytes(MAX_RETURNED_SVG_BYTES), "256 KiB");
        assert_eq!(format_bytes(MAX_RASTER_SVG_BYTES), "8 MiB");
    }

    /// PNG の上限: ちょうどなら通り、1 バイトでも超えれば拒む。拒んだ後も図面は変わらない。
    #[test]
    fn png_at_the_limit_passes_and_one_byte_over_is_refused() {
        let doc = dense_drawing(300);
        let req = request(Format::Png, 400, 300);
        let size = render(&doc, TAG, &req).unwrap().png.unwrap().len();
        let revision = doc.revision();
        let at = Limits {
            png_bytes: size,
            ..Limits::DEFAULT
        };
        assert!(render_within(&doc, TAG, &req, &at).is_ok());
        let over = Limits {
            png_bytes: size - 1,
            ..Limits::DEFAULT
        };
        let e = render_within(&doc, TAG, &req, &over).unwrap_err();
        assert!(
            e.contains("PNG")
                && e.contains("上限")
                && e.contains("width / height")
                && e.contains("region"),
            "{e}"
        );
        assert_eq!(doc.revision(), revision);
    }

    /// 本番の上限で、reviewer が見つけた場合（線分が密な図面を 4096 × 4096 にした場合。レビューの実測では線分 2 万本で PNG 約 19 MB）が拒まれる。
    /// 同じ図面は 1024 × 768 なら通る。
    #[test]
    fn dense_drawing_at_max_size_is_refused_but_default_size_passes() {
        let doc = dense_drawing(DENSE_LINES);
        let e = render(&doc, TAG, &request(Format::Png, 4096, 4096)).unwrap_err();
        assert!(e.contains("PNG") && e.contains("3.5 MiB"), "{e}");
        let ok = render(&doc, TAG, &request(Format::Png, 1024, 768)).unwrap();
        assert!(ok.png.unwrap().len() <= MAX_PNG_BYTES);
    }

    /// 返す SVG の上限: ちょうどなら通り、1 バイト超えれば拒む。
    /// PNG だけなら、同じ図面でも途中の SVG の上限（別の値）で通る。
    #[test]
    fn returned_svg_limit_is_tighter_than_the_one_for_rasterizing() {
        let doc = dense_drawing(300);
        let svg_req = request(Format::Svg, 400, 300);
        let size = render(&doc, TAG, &svg_req).unwrap().svg.unwrap().len();
        let tight = |bytes| Limits {
            returned_svg_bytes: bytes,
            ..Limits::DEFAULT
        };
        assert!(render_within(&doc, TAG, &svg_req, &tight(size)).is_ok());
        let e = render_within(&doc, TAG, &svg_req, &tight(size - 1)).unwrap_err();
        assert!(
            e.contains("SVG") && e.contains("上限") && e.contains("region"),
            "{e}"
        );
        // both も同じ（SVG を返すので）。
        let both = request(Format::Both, 400, 300);
        assert!(render_within(&doc, TAG, &both, &tight(size - 1)).is_err());
        // PNG だけなら、返す SVG の上限は効かない。
        let png = request(Format::Png, 400, 300);
        assert!(render_within(&doc, TAG, &png, &tight(size - 1)).is_ok());
        // 途中の SVG の上限は、PNG だけのときに効く。
        let raster = Limits {
            raster_svg_bytes: size - 1,
            ..Limits::DEFAULT
        };
        assert!(render_within(&doc, TAG, &png, &raster).is_err());
        assert!(render_within(&doc, TAG, &svg_req, &raster).is_ok());
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
        let out = render(&Document::new(), TAG, &request(Format::Png, 100, 100)).unwrap();
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
        let out = render(&doc, TAG, &request(Format::Png, 200, 100)).unwrap();
        assert_eq!(ihdr_size(&out.png.unwrap()), (200, 100));
    }

    #[test]
    fn explicit_region_is_used_as_given() {
        let doc = drawing();
        let region = Aabb::new(Point2::new(0.0, 0.0), Point2::new(4.0, 2.0));
        let out = render(
            &doc,
            TAG,
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
