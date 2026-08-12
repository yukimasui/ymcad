//! DXF R12 の書き出し。
//!
//! # 座標の小数桁数について
//!
//! すべての座標・半径・角度は `format!("{v:.12}")`（小数点以下 12 桁固定）で書き出す。
//! 指示書の受け入れ基準は「1e-9 の精度で往復すること」だが、12 桁なら
//! 絶対誤差は高々 5e-13 程度に収まり、`tolerance::EPS_LEN`（1e-9）はもちろん
//! 要求される 1e-9 にも十分な余裕を持って収まる（実測は `dxf_roundtrip.rs` の
//! 大小の座標往復テストを参照）。固定桁数なので実装も単純で、`{}` のような
//! 可変桁数のフォーマットで「桁数が足りない場合がある」ことを心配する必要もない。

use std::collections::{HashMap, HashSet};
use std::path::Path;

/// 書き出しの結果。
///
/// `warnings` には、DXF R12 で表現できずに近似したものの説明が入る。
/// 呼び出し側はこれをユーザーへ見せること。黙って情報を落とさないための仕組み。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WriteReport {
    /// DXF 本文。
    pub text: String,
    /// 近似・欠落の警告。
    pub warnings: Vec<String>,
}

use super::{rad_to_deg, ACAD_VERSION};
use crate::document::Document;
use crate::entity::{Entity, EntityId, Geometry};
use crate::error::Result;
use crate::geom::Point2;
use crate::layer::{ColorSpec, LayerId};

/// 座標・半径・角度を書き出すときの小数桁数。モジュールドキュメント参照。
const COORD_DECIMALS: usize = 12;

/// `f64` を DXF の値行として書き出す。
fn fmt_f64(v: f64) -> String {
    format!("{v:.COORD_DECIMALS$}")
}

/// グループコード行・値行のペアを積み上げていく単純なバッファ。
struct Writer {
    buf: String,
}

impl Writer {
    fn new() -> Self {
        Self { buf: String::new() }
    }

    /// グループコードと文字列値の 1 ペアを書く。
    fn pair(&mut self, code: i32, value: &str) {
        self.buf.push_str(&code.to_string());
        self.buf.push('\n');
        self.buf.push_str(value);
        self.buf.push('\n');
    }

    /// グループコードと数値（整数）の 1 ペアを書く。
    fn pair_i32(&mut self, code: i32, value: i32) {
        self.pair(code, &value.to_string());
    }

    /// グループコードと `f64` 値の 1 ペアを書く。
    fn pair_f64(&mut self, code: i32, value: f64) {
        self.pair(code, &fmt_f64(value));
    }

    fn finish(self) -> String {
        self.buf
    }
}

/// レイヤ ID ごとに、DXF R12 で安全な一意なレイヤ名を割り当てる。
///
/// [`sanitize_layer_name`](super::sanitize_layer_name) は非可逆（大文字化・置換）なので、
/// 元の名前が違っても正規化後に衝突することがありうる（例: `"Wall"` と `"WALL"`）。
/// その場合は `_2`, `_3`, ... を付けて一意性を保つ。エンティティ側もこのマップを
/// 経由して同じ名前を参照するので、テーブルとエンティティの対応関係は必ず一致する。
fn sanitized_layer_names(doc: &Document) -> HashMap<LayerId, String> {
    let mut used: HashSet<String> = HashSet::new();
    let mut map = HashMap::new();
    for (id, layer) in doc.layers().iter() {
        let base = super::sanitize_layer_name(&layer.name);
        let mut candidate = base.clone();
        let mut suffix = 2u32;
        while used.contains(&candidate) {
            candidate = format!("{base}_{suffix}");
            suffix += 1;
        }
        used.insert(candidate.clone());
        map.insert(id, candidate);
    }
    map
}

fn layer_name_for(names: &HashMap<LayerId, String>, id: LayerId) -> &str {
    names.get(&id).map_or("0", String::as_str)
}

/// エンティティの色（`ByLayer` なら省略、明示指定なら group 62）を書く。
fn write_entity_color(w: &mut Writer, color: ColorSpec) {
    if let ColorSpec::Aci(c) = color {
        w.pair_i32(62, i32::from(c.0));
    }
}

fn write_header(w: &mut Writer, doc: &Document) {
    w.pair(0, "SECTION");
    w.pair(2, "HEADER");

    w.pair(9, "$ACADVER");
    w.pair(1, ACAD_VERSION);

    w.pair(9, "$INSBASE");
    w.pair_f64(10, 0.0);
    w.pair_f64(20, 0.0);
    w.pair_f64(30, 0.0);

    let bbox = doc.bbox();
    let (min, max) = if bbox.is_empty() {
        (Point2::ORIGIN, Point2::ORIGIN)
    } else {
        (bbox.min, bbox.max)
    };

    w.pair(9, "$EXTMIN");
    w.pair_f64(10, min.x);
    w.pair_f64(20, min.y);
    w.pair_f64(30, 0.0);

    w.pair(9, "$EXTMAX");
    w.pair_f64(10, max.x);
    w.pair_f64(20, max.y);
    w.pair_f64(30, 0.0);

    w.pair(0, "ENDSEC");
}

fn write_layer_table(w: &mut Writer, doc: &Document, names: &HashMap<LayerId, String>) {
    w.pair(0, "SECTION");
    w.pair(2, "TABLES");

    w.pair(0, "TABLE");
    w.pair(2, "LAYER");
    w.pair_i32(70, i32::try_from(doc.layers().len()).unwrap_or(i32::MAX));

    for (id, layer) in doc.layers().iter() {
        w.pair(0, "LAYER");
        w.pair(2, layer_name_for(names, id));
        // bit 4（値 4）= ロック中。凍結（bit 1 / 値 1）は本プロトタイプでは扱わない。
        let flags = if layer.locked { 4 } else { 0 };
        w.pair_i32(70, flags);
        // R12 の慣習: 色を負値にすると「レイヤ非表示」を表す。
        let color = i32::from(layer.color.0);
        w.pair_i32(62, if layer.visible { color } else { -color });
        w.pair(6, layer.linetype.dxf_name());
    }

    w.pair(0, "ENDTAB");
    w.pair(0, "ENDSEC");
}

/// 作図線を書き出すときの線分の長さを、図面範囲の何倍にするか。
///
/// 読み手が「実質的に無限の直線」と受け取れる程度に長く、かつ座標が
/// 桁あふれしない程度に抑える。
const XLINE_SPAN_FACTOR: f64 = 10.0;

/// 図面が空のときに使う作図線の長さ。
const XLINE_FALLBACK_SPAN: f64 = 1000.0;

/// 作図線を有限の線分へ落とすときの長さ（片側）。
///
/// 定数ではなく図面範囲から導く。極端に大きい定数を使うと、
/// 読み込んだ側で図面範囲が壊れてしまうため。
fn xline_span(doc: &Document) -> f64 {
    let b = doc.bbox();
    if b.is_empty() {
        return XLINE_FALLBACK_SPAN;
    }
    let diagonal = b.size().len();
    if diagonal > 0.0 {
        diagonal * XLINE_SPAN_FACTOR
    } else {
        XLINE_FALLBACK_SPAN
    }
}

fn write_entity(w: &mut Writer, id: EntityId, entity: &Entity, layer_name: &str, xline_span: f64) {
    match &entity.geom {
        Geometry::Line(l) => {
            w.pair(0, "LINE");
            w.pair(5, &id.to_dxf_handle());
            w.pair(8, layer_name);
            write_entity_color(w, entity.color);
            w.pair_f64(10, l.a.x);
            w.pair_f64(20, l.a.y);
            w.pair_f64(11, l.b.x);
            w.pair_f64(21, l.b.y);
        }
        // R12 に XLINE は存在しない（R13 以降）。十分長い LINE として書き出す。
        // 呼び出し側が警告を出すので、ここでは黙って近似してよい。
        Geometry::Xline(x) => {
            let a = x.point_at(-xline_span);
            let b = x.point_at(xline_span);
            w.pair(0, "LINE");
            w.pair(5, &id.to_dxf_handle());
            w.pair(8, layer_name);
            write_entity_color(w, entity.color);
            w.pair_f64(10, a.x);
            w.pair_f64(20, a.y);
            w.pair_f64(11, b.x);
            w.pair_f64(21, b.y);
        }
        Geometry::Circle(c) => {
            w.pair(0, "CIRCLE");
            w.pair(5, &id.to_dxf_handle());
            w.pair(8, layer_name);
            write_entity_color(w, entity.color);
            w.pair_f64(10, c.center.x);
            w.pair_f64(20, c.center.y);
            w.pair_f64(40, c.radius);
        }
        Geometry::Arc(a) => {
            w.pair(0, "ARC");
            w.pair(5, &id.to_dxf_handle());
            w.pair(8, layer_name);
            write_entity_color(w, entity.color);
            w.pair_f64(10, a.center.x);
            w.pair_f64(20, a.center.y);
            w.pair_f64(40, a.radius);
            // 角度の変換は rad_to_deg の 1 箇所だけを通す。
            w.pair_f64(50, rad_to_deg(a.start_angle));
            w.pair_f64(51, rad_to_deg(a.end_angle));
        }
        Geometry::Polyline(p) => {
            // R12 に LWPOLYLINE は無いので POLYLINE + VERTEX* + SEQEND で書く。
            w.pair(0, "POLYLINE");
            w.pair(5, &id.to_dxf_handle());
            w.pair(8, layer_name);
            write_entity_color(w, entity.color);
            w.pair(66, "1"); // 後続に VERTEX が続くことを示すフラグ。
            w.pair_i32(70, if p.closed { 1 } else { 0 });
            for v in &p.vertices {
                w.pair(0, "VERTEX");
                w.pair(8, layer_name);
                w.pair_f64(10, v.x);
                w.pair_f64(20, v.y);
            }
            w.pair(0, "SEQEND");
        }
    }
}

fn write_entities(
    w: &mut Writer,
    doc: &Document,
    names: &HashMap<LayerId, String>,
    warnings: &mut Vec<String>,
) {
    w.pair(0, "SECTION");
    w.pair(2, "ENTITIES");

    let span = xline_span(doc);
    let mut xline_count = 0usize;

    for (id, entity) in doc.entities().iter() {
        if matches!(entity.geom, Geometry::Xline(_)) {
            xline_count += 1;
        }
        let layer_name = layer_name_for(names, entity.layer);
        write_entity(w, id, entity, layer_name, span);
    }

    // 何本あっても警告は 1 行にまとめる。
    if xline_count > 0 {
        warnings.push(format!(
            "作図線 {xline_count} 本は DXF R12 に無いため、十分長い線分として書き出しました（読み戻すと線分になります）"
        ));
    }

    w.pair(0, "ENDSEC");
}

/// 図面を DXF R12（AC1009）として書き出す。
#[must_use]
pub fn write_to_string(doc: &Document) -> WriteReport {
    let names = sanitized_layer_names(doc);

    let mut w = Writer::new();
    let mut warnings = Vec::new();
    write_header(&mut w, doc);
    write_layer_table(&mut w, doc, &names);
    write_entities(&mut w, doc, &names, &mut warnings);
    w.pair(0, "EOF");

    WriteReport {
        text: w.finish(),
        warnings,
    }
}

/// 図面を DXF R12 としてファイルへ書き出す。
///
/// 書き込みは**アトミック**。失敗しても `path` は元の内容のまま残る
/// （[`crate::atomic_write`] を参照）。
///
/// # Errors
///
/// ファイルの書き込みに失敗した場合 [`CadError::Io`]。
pub fn write_to_file(doc: &Document, path: &Path) -> Result<Vec<String>> {
    let report = write_to_string(doc);
    crate::atomic_write::write_atomic(path, report.text.as_bytes())?;
    Ok(report.warnings)
}
