//! 描画の道具: `render`（図面を PNG / SVG にして返す）。読むだけ。
//!
//! 描き方は [`crate::render`]。ここは引数の取り出しと、結果（`structuredContent` と
//! 画像・SVG の `content` ブロック）の組み立て。

use serde_json::{json, Value};

use super::{Args, Tool, ToolResult};
use crate::convert::{aabb_from_json, aabb_to_json};
use crate::render::fit::{MAX_SIDE_PX, MIN_SIDE_PX};
use crate::render::{self, base64, Background, Format, Request};
use crate::server::Server;

const DEFAULT_WIDTH_PX: usize = 1024;
const DEFAULT_HEIGHT_PX: usize = 768;

pub(super) const RENDER: Tool = Tool {
    name: "render",
    title: "図面を画像にする",
    description: "図面を PNG（image ブロック）か SVG（text ブロック）にして返す。結果の text（JSON）に、見えている範囲 view（モデル座標）と \
1 モデル単位あたりの px（pixels_per_unit）が入る。画像の位置 (px, py)（左上が原点、下向きが +）のモデル座標は \
x = view.min.x + px / pixels_per_unit、y = view.max.y - py / pixels_per_unit（Y は上向きに描く）。\
region を省くと、表示中の図形（インスタンスの中身を含む。作図線は除く）の範囲に余白を付けて収める。region を指定すると、その範囲が縦横比を保って画像の中央に収まる。\
非表示のレイヤは描かない。色はレイヤ・図形の色（ACI）、破線は線種のとおり。light の背景では白（ACI 7）を黒で描く。文字は描かない。\
SVG の各要素の data-id は図形 ID。図面を変えない。\
返す大きさに上限がある（PNG は 3.5 MiB、SVG は 256 KiB）。超えたらエラーになるので、width / height を下げるか region で範囲を絞る（SVG は region だけが効く。図形が多い図面は PNG で見る）。",
    schema: || {
        (
            json!({
                "format": {
                    "type": "string", "enum": ["png", "svg", "both"],
                    "description": "返す形式（既定 png）",
                },
                "width": {
                    "type": "integer", "minimum": MIN_SIDE_PX, "maximum": MAX_SIDE_PX,
                    "description": format!("画像の幅 px（既定 {DEFAULT_WIDTH_PX}）"),
                },
                "height": {
                    "type": "integer", "minimum": MIN_SIDE_PX, "maximum": MAX_SIDE_PX,
                    "description": format!("画像の高さ px（既定 {DEFAULT_HEIGHT_PX}）"),
                },
                "region": {
                    "type": "object",
                    "description": "見せるモデルの範囲。{\"min\": [x, y], \"max\": [x, y]}（点は {\"x\":..,\"y\":..} でもよい）。幅と高さは 0 より大きく",
                    "properties": { "min": point_schema(), "max": point_schema() },
                    "required": ["min", "max"],
                },
                "background": {
                    "type": "string", "enum": ["dark", "light"],
                    "description": "背景（既定 dark = アプリと同じ暗い背景）",
                },
            }),
            &[],
        )
    },
    read_only: true,
    destructive: false,
    idempotent: true,
    run: render_drawing,
};

fn point_schema() -> Value {
    json!({
        "description": "[x, y] か {\"x\":..,\"y\":..}",
        "oneOf": [
            { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2 },
            {
                "type": "object",
                "properties": { "x": { "type": "number" }, "y": { "type": "number" } },
                "required": ["x", "y"],
            },
        ],
    })
}

fn render_drawing(s: &mut Server, a: &Args) -> ToolResult {
    let format = Format::parse(a.opt_str("format")?.unwrap_or("png"))?;
    let background = Background::parse(a.opt_str("background")?.unwrap_or("dark"))?;
    let side = |key: &str, default: usize| -> Result<u32, String> {
        let n = a.usize_in(key, default, MIN_SIDE_PX as usize, MAX_SIDE_PX as usize)?;
        u32::try_from(n).map_err(|_| format!("{key} が大きすぎます"))
    };
    let (width, height) = (
        side("width", DEFAULT_WIDTH_PX)?,
        side("height", DEFAULT_HEIGHT_PX)?,
    );
    let region = a
        .value("region")
        .map(|v| aabb_from_json(v, "region"))
        .transpose()?;

    let out = render::render(
        &s.doc,
        s.tag(),
        &Request {
            format,
            width,
            height,
            region,
            background,
        },
    )?;

    // 失敗しないところまで来てから追加のブロックを積む（失敗した呼び出しに画像を残さない）。
    let mut structured = json!({
        "drawing": s.tag().name(),
        "format": match format {
            Format::Png => "png",
            Format::Svg => "svg",
            Format::Both => "both",
        },
        "width": out.fit.width(),
        "height": out.fit.height(),
        "view": aabb_to_json(out.fit.view()),
        "pixels_per_unit": out.fit.pixels_per_unit(),
        "region_source": if out.auto_region { "drawing_extent" } else { "given" },
        "background": background.name(),
        "y_axis": "up",
        "entities_drawn": out.stats.drawn,
        "entities_outside_view": out.stats.outside_view,
        "entities_on_hidden_layers": out.stats.hidden,
    });
    if let Some(png) = &out.png {
        structured["png_bytes"] = json!(png.len());
        s.attachments.push(json!({
            "type": "image",
            "mimeType": "image/png",
            "data": base64::encode(png),
        }));
    }
    if let Some(svg) = out.svg {
        structured["svg_bytes"] = json!(svg.len());
        s.attachments.push(json!({ "type": "text", "text": svg }));
    }
    Ok(structured)
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{call_raw, eid, err, ok, server};
    use super::super::tests::cad_core_add_line;
    use super::*;
    use crate::test_util::TempDir;

    fn content_types(r: &Value) -> Vec<&str> {
        r["content"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["type"].as_str().unwrap())
            .collect()
    }

    #[test]
    fn png_is_returned_as_an_image_block_after_the_json_text() {
        let dir = TempDir::new("render-png");
        let mut s = server(&dir);
        cad_core_add_line(&mut s);
        let r = call_raw(&mut s, "render", json!({"width": 320, "height": 200}));
        assert_eq!(r["isError"], false, "{r}");
        assert_eq!(content_types(&r), ["text", "image"]);
        let image = &r["content"][1];
        assert_eq!(image["mimeType"], "image/png");
        let data = image["data"].as_str().unwrap();
        // PNG のシグネチャの base64。
        assert!(data.starts_with("iVBORw0KGgo"), "{}", &data[..20]);
        let bytes = r["structuredContent"]["png_bytes"].as_u64().unwrap();
        assert_eq!(data.len() as u64, bytes.div_ceil(3) * 4);
        // text は structuredContent と同じ JSON。小数も最後のビットまで読み戻せる
        // （serde_json の float_roundtrip。ADR-0046 決定 16）ので、座標を含む view ごと比べる。
        let text: Value = serde_json::from_str(r["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(text, r["structuredContent"]);
        assert_eq!(
            r["content"][0]["text"].as_str().unwrap(),
            r["structuredContent"].to_string()
        );
        let sc = &r["structuredContent"];
        assert_eq!(
            (sc["width"].as_u64(), sc["height"].as_u64()),
            (Some(320), Some(200))
        );
        assert!(sc["pixels_per_unit"].as_f64().unwrap() > 0.0);
        assert_eq!(sc["region_source"], "drawing_extent");
        assert_eq!(sc["entities_drawn"], 1);
        // 画像はサイズが大きいので、structuredContent には載せない。
        assert!(!sc.to_string().contains("iVBOR"));
    }

    #[test]
    fn svg_is_returned_as_text_and_both_returns_both() {
        let dir = TempDir::new("render-svg");
        let mut s = server(&dir);
        cad_core_add_line(&mut s);
        let r = call_raw(&mut s, "render", json!({"format": "svg"}));
        assert_eq!(content_types(&r), ["text", "text"]);
        let svg = r["content"][1]["text"].as_str().unwrap();
        assert!(
            svg.starts_with("<svg ") && svg.contains(&format!("data-id=\"{}\"", eid(&s, 0))),
            "{svg}"
        );
        assert_eq!(
            r["structuredContent"]["svg_bytes"].as_u64().unwrap(),
            svg.len() as u64
        );

        let r = call_raw(&mut s, "render", json!({"format": "both"}));
        assert_eq!(content_types(&r), ["text", "image", "text"]);
    }

    /// 前の呼び出しの画像が、次の（画像のない・失敗した）結果に残らない。
    #[test]
    fn attachments_do_not_leak_between_calls() {
        let dir = TempDir::new("render-leak");
        let mut s = server(&dir);
        let r = call_raw(&mut s, "render", json!({}));
        assert_eq!(content_types(&r), ["text", "image"]);
        let r = call_raw(&mut s, "drawing_info", json!({}));
        assert_eq!(content_types(&r), ["text"]);
        let r = call_raw(&mut s, "render", json!({"width": 99999}));
        assert_eq!(r["isError"], true);
        assert_eq!(content_types(&r), ["text"]);
    }

    #[test]
    fn region_and_background_are_applied() {
        let dir = TempDir::new("render-region");
        let mut s = server(&dir);
        cad_core_add_line(&mut s);
        let r = ok(
            &mut s,
            "render",
            json!({"format": "svg", "width": 400, "height": 200, "background": "light",
                   "region": {"min": [0, 0], "max": {"x": 4, "y": 2}}}),
        );
        assert_eq!(r["region_source"], "given");
        assert_eq!(r["background"], "light");
        assert_eq!(r["pixels_per_unit"].as_f64().unwrap(), 100.0);
        assert_eq!(r["view"]["min"], json!({"x": 0.0, "y": 0.0}));
        assert_eq!(r["view"]["max"], json!({"x": 4.0, "y": 2.0}));
        assert_eq!(r["y_axis"], "up");
    }

    #[test]
    fn bad_arguments_are_explained() {
        let dir = TempDir::new("render-errors");
        let mut s = server(&dir);
        for (args, needle) in [
            (json!({"width": 4097}), "width"),
            (json!({"height": 4097}), "height"),
            (json!({"width": 15}), "width"),
            (json!({"width": 1.5}), "width"),
            (json!({"width": -1}), "width"),
            (json!({"format": "jpeg"}), "png / svg / both"),
            (json!({"background": "red"}), "dark / light"),
            (json!({"region": {"min": [0, 0]}}), "max"),
            (
                json!({"region": {"min": [0, 0], "max": [0, 5]}}),
                "0 より大きく",
            ),
            (
                json!({"region": {"min": [0, 0], "max": [1, 1], "z": 1}}),
                "region",
            ),
            (json!({"regoin": {}}), "regoin"),
        ] {
            let msg = err(&mut s, "render", args.clone());
            assert!(msg.contains(needle), "{args}: {msg}");
        }
    }

    /// 返す SVG が上限（256 KiB）を超える図面は `isError` で、`region` を促す。画像・SVG は残らず、図面も変わらない。
    /// 同じ図面でも PNG だけなら通る（途中の SVG の上限は別）。
    #[test]
    fn oversized_svg_is_an_error_that_suggests_region() {
        use cad_core::command::AddEntities;
        use cad_core::geom::{Line, Point2};
        use cad_core::{Entity, Geometry, LayerId};

        let dir = TempDir::new("render-svg-limit");
        let mut s = server(&dir);
        let lines: Vec<Entity> = (0..4000)
            .map(|i| {
                let y = f64::from(i) * 0.01;
                Entity::new(
                    Geometry::Line(Line::new(Point2::new(0.0, y), Point2::new(100.0, y + 1.0))),
                    LayerId::ZERO,
                )
            })
            .collect();
        s.doc
            .apply(Box::new(AddEntities::many("ADD", lines)))
            .unwrap();
        let revision = s.doc.revision();
        let r = call_raw(&mut s, "render", json!({"format": "svg"}));
        assert_eq!(r["isError"], true, "{r}");
        assert_eq!(content_types(&r), ["text"]);
        let msg = r["content"][0]["text"].as_str().unwrap();
        assert!(msg.contains("256 KiB") && msg.contains("region"), "{msg}");
        assert_eq!(s.doc.revision(), revision);
        let r = call_raw(&mut s, "render", json!({"format": "both"}));
        assert_eq!(r["isError"], true, "{r}");
        let r = call_raw(&mut s, "render", json!({"format": "png"}));
        assert_eq!(r["isError"], false, "{r}");
        assert_eq!(content_types(&r), ["text", "image"]);
    }

    #[test]
    fn render_does_not_change_the_drawing() {
        let dir = TempDir::new("render-readonly");
        let mut s = server(&dir);
        cad_core_add_line(&mut s);
        let before = s.doc.revision();
        let r = call_raw(&mut s, "render", json!({"format": "both"}));
        assert_eq!(r["isError"], false);
        assert_eq!(s.doc.revision(), before);
        assert!(s.doc.is_dirty());
    }

    #[test]
    fn render_is_declared_read_only() {
        let defs = crate::tools::definitions();
        let t = defs.iter().find(|d| d["name"] == "render").unwrap();
        assert_eq!(t["annotations"]["readOnlyHint"], true);
        assert_eq!(t["annotations"]["destructiveHint"], false);
        assert_eq!(t["inputSchema"]["properties"]["width"]["maximum"], 4096);
    }
}
