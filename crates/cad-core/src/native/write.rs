//! ネイティブ形式（`.ymc`）の書き出し。
//!
//! # DXF ライタとの違い
//!
//! **警告（`WriteReport`）を持たない。** この形式は非可逆な近似をしないので、
//! 伝えるべき警告が存在しないことが仕様。`dxf::write` との非対称はそのまま出す。
//!
//! `f64` は `to_le_bytes` でビット単位に書く。テキスト形式の桁数丸めが無いので、
//! 座標は完全に往復する。

use std::path::Path;

use crate::document::Document;
use crate::entity::Geometry;
use crate::error::Result;
use crate::geom::Point2;
use crate::layer::{ColorSpec, Layer, LineType};

use super::{color_tag, kind, layer_flags, linetype, option_tag, FORMAT_VERSION, MAGIC};

/// 図面をネイティブ形式のバイト列にする。
#[must_use]
pub fn write_to_bytes(doc: &Document) -> Vec<u8> {
    let mut w = Writer::new();

    w.raw(MAGIC);
    w.u32(FORMAT_VERSION);

    // レイヤ。ID 昇順（= `LayerTable::iter` の順）。
    // エンティティはこの並びへの添字でレイヤを指す。
    let layers: Vec<_> = doc.layers().iter().collect();
    w.count(layers.len());
    for (_, layer) in &layers {
        write_layer(&mut w, layer);
    }

    // グループ。ID 昇順。同じく添字で参照される。
    let groups: Vec<_> = doc.groups().iter().collect();
    w.count(groups.len());
    for (_, group) in &groups {
        w.string(&group.name);
    }

    // エンティティ。スロット昇順（= 作成順 = 描画順）。
    // この順序が図面の見た目を決めるので、必ず `EntityStore::iter` の順で書く。
    w.count(doc.entities().len());
    for (_, entity) in doc.entities().iter() {
        write_geometry(&mut w, &entity.geom);

        // レイヤ・グループは ID 値ではなくファイル内の添字で書く。
        // ID の再現に依存しないので、将来 ID の割り当て方が変わっても壊れない。
        let layer_index = layers
            .iter()
            .position(|(id, _)| *id == entity.layer)
            .unwrap_or(0);
        w.count(layer_index);

        match entity.color {
            ColorSpec::ByLayer => w.u8(color_tag::BY_LAYER),
            ColorSpec::Aci(c) => {
                w.u8(color_tag::ACI);
                w.u8(c.0);
            }
        }

        match entity
            .group
            .and_then(|g| groups.iter().position(|(id, _)| *id == g))
        {
            Some(index) => {
                w.u8(option_tag::SOME);
                w.count(index);
            }
            None => w.u8(option_tag::NONE),
        }
    }

    w.finish()
}

/// 図面をネイティブ形式でファイルへ書き出す。
///
/// 書き込みは**アトミック**。失敗しても `path` は元の内容のまま残る。
///
/// # Errors
///
/// ファイルの書き込みに失敗した場合 [`crate::CadError::Io`]。
pub fn write_to_file(doc: &Document, path: &Path) -> Result<()> {
    crate::atomic_write::write_atomic(path, &write_to_bytes(doc))
}

fn write_layer(w: &mut Writer, layer: &Layer) {
    w.string(&layer.name);
    w.u8(layer.color.0);

    let mut flags = 0u8;
    if layer.visible {
        flags |= layer_flags::VISIBLE;
    }
    if layer.locked {
        flags |= layer_flags::LOCKED;
    }
    w.u8(flags);

    w.u8(match layer.linetype {
        LineType::Continuous => linetype::CONTINUOUS,
        LineType::Dashed => linetype::DASHED,
        LineType::Center => linetype::CENTER,
        LineType::Hidden => linetype::HIDDEN,
    });
}

/// 図形を「種別タグ + ペイロード」の形で書く。
///
/// **角度はラジアンのまま書く。** DXF ライタの度変換を通さないので、
/// π/180 の往復による誤差が入らない。
fn write_geometry(w: &mut Writer, geom: &Geometry) {
    match geom {
        Geometry::Line(l) => {
            w.u8(kind::LINE);
            w.point(l.a);
            w.point(l.b);
        }
        Geometry::Circle(c) => {
            w.u8(kind::CIRCLE);
            w.point(c.center);
            w.f64(c.radius);
        }
        Geometry::Arc(a) => {
            w.u8(kind::ARC);
            w.point(a.center);
            w.f64(a.radius);
            w.f64(a.start_angle);
            w.f64(a.end_angle);
        }
        Geometry::Xline(x) => {
            w.u8(kind::XLINE);
            w.point(x.origin);
            w.f64(x.direction.x);
            w.f64(x.direction.y);
        }
        Geometry::Polyline(p) => {
            w.u8(kind::POLYLINE);
            w.u8(u8::from(p.closed));
            w.count(p.vertices.len());
            for v in &p.vertices {
                w.point(*v);
            }
        }
    }
}

/// バイト列を組み立てる小さなヘルパ。
///
/// すべてリトルエンディアン固定幅。エンディアンを 1 箇所に閉じ込めるためにある。
struct Writer {
    bytes: Vec<u8>,
}

impl Writer {
    fn new() -> Self {
        Self { bytes: Vec::new() }
    }

    fn raw(&mut self, b: &[u8]) {
        self.bytes.extend_from_slice(b);
    }

    fn u8(&mut self, v: u8) {
        self.bytes.push(v);
    }

    fn u32(&mut self, v: u32) {
        self.bytes.extend_from_slice(&v.to_le_bytes());
    }

    /// 件数を書く。
    ///
    /// `usize` から `u32` への切り詰めを 1 箇所に閉じ込める。
    /// エンティティ数は `EntityStore` の側で既に `u32` に収まることが保証されている
    /// （`insert` が `u32::try_from` で expect している）ので、飽和させて足りる。
    fn count(&mut self, v: usize) {
        self.u32(u32::try_from(v).unwrap_or(u32::MAX));
    }

    fn f64(&mut self, v: f64) {
        self.bytes.extend_from_slice(&v.to_le_bytes());
    }

    fn point(&mut self, p: Point2) {
        self.f64(p.x);
        self.f64(p.y);
    }

    /// 文字列を「長さ + UTF-8 バイト列」で書く。
    ///
    /// **サニタイズしない。** DXF R12 向けの大文字化・空白除去を通さないので、
    /// 日本語のレイヤ名・グループ名がそのまま保たれる。
    fn string(&mut self, s: &str) {
        self.count(s.len());
        self.raw(s.as_bytes());
    }

    fn finish(self) -> Vec<u8> {
        self.bytes
    }
}
