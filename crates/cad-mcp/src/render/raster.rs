//! SVG → PNG（resvg）。
//!
//! # `f32` について
//!
//! resvg / tiny-skia の API は `f32` を使うが、**このクレートのソースには `f32` を書かない**
//! （CI が禁止している）。SVG は画像の px をそのまま座標にしてあり（`viewBox="0 0 幅 高さ"`）、
//! 変換は恒等（`Transform::identity()`）、画像の大きさは `u32` で渡せる。
//!
//! ただし**図面の座標が `f32` に落ちる場所が無いわけではない**: モデル座標を [`super::Fit`] で px
//! （`f64`）にして SVG の文字列へ小数 3 桁で書き、usvg がその文字列を読むときに `f32` にする
//! （設計原則 1 の `f32` の出口が、`viewport.rs` のほかにここにもある。ADR-0046 決定 9）。
//! 値は表示範囲で切ってあるので数千 px（円・円弧は 1e5 px）までに収まり、`f32` の精度（1e5 px で約 0.008 px）で足りる。

use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg;

/// SVG の文字列を PNG にする。画像の大きさは SVG の `width` × `height`（px）。
///
/// # Errors
///
/// SVG として読めない・大きさが 0・画像の領域を確保できない・PNG に符号化できない場合。
pub fn svg_to_png(svg: &str) -> Result<Vec<u8>, String> {
    let tree = usvg::Tree::from_str(svg, &usvg::Options::default())
        .map_err(|e| format!("SVG を読めません: {e}"))?;
    let size = tree.size().to_int_size();
    let mut pixmap = Pixmap::new(size.width(), size.height())
        .ok_or_else(|| "画像の大きさが 0 か大きすぎます".to_owned())?;
    resvg::render(&tree, Transform::identity(), &mut pixmap.as_mut());
    pixmap
        .encode_png()
        .map_err(|e| format!("PNG に符号化できません: {e}"))
}

/// SVG をラスタライザで読めるか（テスト用。Rust 側の別経路の確認）。
#[cfg(test)]
pub fn parse(svg: &str) -> Result<usvg::Tree, String> {
    usvg::Tree::from_str(svg, &usvg::Options::default()).map_err(|e| e.to_string())
}

/// SVG を描いた画素（RGBA、事前乗算済み）と大きさ（テスト用）。
#[cfg(test)]
pub fn svg_to_pixmap(svg: &str) -> Pixmap {
    let tree = parse(svg).unwrap();
    let size = tree.size().to_int_size();
    let mut pixmap = Pixmap::new(size.width(), size.height()).unwrap();
    resvg::render(&tree, Transform::identity(), &mut pixmap.as_mut());
    pixmap
}
