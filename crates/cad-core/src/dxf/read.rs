//! DXF R12 の読み込み。
//!
//! # 方針
//!
//! - 知らないセクション・知らないエンティティ種別は **エラーにせず読み飛ばす**。
//!   現実の DXF ファイルには本プロトタイプが書き出さない要素（`BLOCKS`, `TEXT`, …）が
//!   大量に含まれるため。
//! - `LINE` / `CIRCLE` / `ARC` / `POLYLINE`+`VERTEX`+`SEQEND` / `LWPOLYLINE` は理解する。
//!   後者 2 つはどちらもポリラインとして読み込む（自分では `POLYLINE` 形式でしか
//!   書き出さないが、他アプリが吐いた `LWPOLYLINE` も受け付ける）。
//! - `LAYER` テーブルを読み、レイヤの色・表示・ロック・線種を復元する。
//!   テーブルに無いレイヤ名をエンティティが参照している場合は、その場で作る。
//! - 数値としてパースできない値やファイルの途中切れは [`CadError::Parse`] として
//!   はっきり報告する。

use std::collections::HashMap;
use std::path::Path;

use super::deg_to_rad;
use crate::command::{AddEntities, AddLayer, SetLayerProperties};
use crate::document::Document;
use crate::entity::{Entity, Geometry};
use crate::error::{CadError, Result};
use crate::geom::{Arc, Circle, Line, Point2, Polyline};
use crate::layer::{AciColor, ColorSpec, LayerId, LineType};

/// 1 組のグループコード・値。デバッグ用にソース上の行番号（1 始まり、値行の行番号）も持つ。
#[derive(Debug, Clone, Copy)]
struct Pair<'a> {
    code: i32,
    value: &'a str,
    /// エラーメッセージ用の、値行の行番号（1 始まり）。
    line: usize,
}

/// グループコード行・値行のペア列を先頭から読み進めるだけの単純なカーソル。
struct Reader<'a> {
    lines: Vec<&'a str>,
    pos: usize,
}

impl<'a> Reader<'a> {
    /// テキストを行に分割してカーソルを作る。
    ///
    /// `str::lines()` は `\n` と `\r\n` の両方を行区切りとして扱うので、
    /// 改行コードの違いはここで自然に吸収される。
    fn new(text: &'a str) -> Result<Self> {
        let mut lines: Vec<&str> = text.lines().map(str::trim).collect();
        while matches!(lines.last(), Some(l) if l.is_empty()) {
            lines.pop();
        }
        if lines.len() % 2 != 0 {
            return Err(CadError::Parse {
                line: lines.len(),
                message: "行数が偶数ではありません（グループコード行と値行が対になっていません）"
                    .to_string(),
            });
        }
        Ok(Self { lines, pos: 0 })
    }

    /// 次のペアを読んで消費する。ファイル末尾なら `Ok(None)`。
    fn next_pair(&mut self) -> Result<Option<Pair<'a>>> {
        if self.pos >= self.lines.len() {
            return Ok(None);
        }
        let code_str = self.lines[self.pos];
        let value = self.lines[self.pos + 1];
        let code_line = self.pos + 1;
        self.pos += 2;
        let code = code_str.parse::<i32>().map_err(|_| CadError::Parse {
            line: code_line,
            message: format!("グループコードが数値ではありません: {code_str:?}"),
        })?;
        Ok(Some(Pair {
            code,
            value,
            line: code_line + 1,
        }))
    }

    /// 次のペアを要求する。無ければファイルが途中で切れているとみなしエラーにする。
    fn require_pair(&mut self) -> Result<Pair<'a>> {
        self.next_pair()?.ok_or_else(|| CadError::Parse {
            line: self.lines.len(),
            message: "ファイルが途中で終了しています（EOF が見つかりません）".to_string(),
        })
    }

    /// 次のペアのグループコードだけを覗き見る（消費しない）。パースできなければ `None`。
    fn peek_code(&self) -> Option<i32> {
        self.lines.get(self.pos)?.parse::<i32>().ok()
    }

    /// 次の「グループコード 0」の手前まで（= 1 レコード分の属性）を読み進める。
    fn read_attrs(&mut self) -> Result<Vec<Pair<'a>>> {
        let mut out = Vec::new();
        while self.pos < self.lines.len() && self.peek_code() != Some(0) {
            out.push(self.require_pair()?);
        }
        Ok(out)
    }
}

fn parse_f64_pair(p: Pair<'_>) -> Result<f64> {
    p.value.parse::<f64>().map_err(|_| CadError::Parse {
        line: p.line,
        message: format!(
            "数値として解釈できません（グループコード {}）: {:?}",
            p.code, p.value
        ),
    })
}

fn parse_i32_pair(p: Pair<'_>) -> Result<i32> {
    p.value.parse::<i32>().map_err(|_| CadError::Parse {
        line: p.line,
        message: format!(
            "整数として解釈できません（グループコード {}）: {:?}",
            p.code, p.value
        ),
    })
}

fn color_from_pair(p: Pair<'_>) -> Result<ColorSpec> {
    let raw = parse_i32_pair(p)?;
    let byte = u8::try_from(raw.unsigned_abs()).unwrap_or(u8::MAX);
    Ok(ColorSpec::Aci(AciColor(byte)))
}

fn linetype_from_dxf_name(name: &str) -> LineType {
    match name.trim().to_ascii_uppercase().as_str() {
        "DASHED" => LineType::Dashed,
        "CENTER" => LineType::Center,
        "HIDDEN" => LineType::Hidden,
        _ => LineType::Continuous,
    }
}

/// `LAYER` テーブルの 1 レコードぶんの中間表現。
struct LayerRecord {
    name: String,
    color: AciColor,
    visible: bool,
    locked: bool,
    linetype: LineType,
}

/// エンティティ 1 つぶんの中間表現。レイヤは名前のまま持ち、`Document` 構築時に解決する。
struct EntityRecord {
    geom: Geometry,
    layer: String,
    color: ColorSpec,
}

// ---- セクション読み飛ばし --------------------------------------------------

/// セクションの中身を理解せず、対応する `ENDSEC` まで読み飛ばす。
fn skip_section(reader: &mut Reader<'_>) -> Result<()> {
    loop {
        let p = reader.require_pair()?;
        if p.code == 0 && p.value == "ENDSEC" {
            return Ok(());
        }
    }
}

/// テーブルの中身を理解せず、対応する `ENDTAB` まで読み飛ばす。
fn skip_table(reader: &mut Reader<'_>) -> Result<()> {
    loop {
        let p = reader.require_pair()?;
        if p.code == 0 && p.value == "ENDTAB" {
            return Ok(());
        }
    }
}

// ---- TABLES セクション ------------------------------------------------------

fn build_layer_record(attrs: &[Pair<'_>]) -> Result<Option<LayerRecord>> {
    let mut name = None;
    let mut color = AciColor::WHITE;
    let mut visible = true;
    let mut locked = false;
    let mut linetype = LineType::Continuous;

    for &p in attrs {
        match p.code {
            2 => name = Some(p.value.to_string()),
            62 => {
                let raw = parse_i32_pair(p)?;
                visible = raw >= 0;
                color = AciColor(u8::try_from(raw.unsigned_abs()).unwrap_or(AciColor::WHITE.0));
            }
            70 => {
                let flags = parse_i32_pair(p)?;
                locked = flags & 4 != 0;
            }
            6 => linetype = linetype_from_dxf_name(p.value),
            _ => {}
        }
    }

    Ok(name.map(|name| LayerRecord {
        name,
        color,
        visible,
        locked,
        linetype,
    }))
}

fn parse_layer_table(reader: &mut Reader<'_>, out: &mut Vec<LayerRecord>) -> Result<()> {
    loop {
        let p = reader.require_pair()?;
        if p.code != 0 {
            continue;
        }
        match p.value {
            "ENDTAB" => return Ok(()),
            "LAYER" => {
                let attrs = reader.read_attrs()?;
                if let Some(rec) = build_layer_record(&attrs)? {
                    out.push(rec);
                }
            }
            _ => {
                let _ = reader.read_attrs()?;
            }
        }
    }
}

fn parse_tables(reader: &mut Reader<'_>, out: &mut Vec<LayerRecord>) -> Result<()> {
    loop {
        let p = reader.require_pair()?;
        if p.code != 0 {
            continue;
        }
        match p.value {
            "ENDSEC" => return Ok(()),
            "TABLE" => {
                let name_pair = reader.require_pair()?;
                if name_pair.code == 2 && name_pair.value == "LAYER" {
                    parse_layer_table(reader, out)?;
                } else {
                    skip_table(reader)?;
                }
            }
            _ => {}
        }
    }
}

// ---- ENTITIES セクション -----------------------------------------------------

/// `LINE` / `CIRCLE` / `ARC` に共通する、単純な属性ペアの集合を読む。
struct CommonAttrs {
    layer: String,
    color: ColorSpec,
}

fn take_common(p: Pair<'_>, common: &mut CommonAttrs) -> Result<bool> {
    match p.code {
        8 => {
            common.layer = p.value.to_string();
            Ok(true)
        }
        62 => {
            common.color = color_from_pair(p)?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

fn parse_line(reader: &mut Reader<'_>) -> Result<EntityRecord> {
    let attrs = reader.read_attrs()?;
    let mut common = CommonAttrs {
        layer: "0".to_string(),
        color: ColorSpec::ByLayer,
    };
    let (mut ax, mut ay, mut bx, mut by) = (0.0, 0.0, 0.0, 0.0);
    for p in attrs {
        if take_common(p, &mut common)? {
            continue;
        }
        match p.code {
            10 => ax = parse_f64_pair(p)?,
            20 => ay = parse_f64_pair(p)?,
            11 => bx = parse_f64_pair(p)?,
            21 => by = parse_f64_pair(p)?,
            _ => {}
        }
    }
    Ok(EntityRecord {
        geom: Geometry::Line(Line::new(Point2::new(ax, ay), Point2::new(bx, by))),
        layer: common.layer,
        color: common.color,
    })
}

fn parse_circle(reader: &mut Reader<'_>) -> Result<EntityRecord> {
    let attrs = reader.read_attrs()?;
    let mut common = CommonAttrs {
        layer: "0".to_string(),
        color: ColorSpec::ByLayer,
    };
    let (mut cx, mut cy, mut radius) = (0.0, 0.0, 0.0);
    for p in attrs {
        if take_common(p, &mut common)? {
            continue;
        }
        match p.code {
            10 => cx = parse_f64_pair(p)?,
            20 => cy = parse_f64_pair(p)?,
            40 => radius = parse_f64_pair(p)?,
            _ => {}
        }
    }
    Ok(EntityRecord {
        geom: Geometry::Circle(Circle::new(Point2::new(cx, cy), radius)),
        layer: common.layer,
        color: common.color,
    })
}

fn parse_arc(reader: &mut Reader<'_>) -> Result<EntityRecord> {
    let attrs = reader.read_attrs()?;
    let mut common = CommonAttrs {
        layer: "0".to_string(),
        color: ColorSpec::ByLayer,
    };
    let (mut cx, mut cy, mut radius, mut start_deg, mut end_deg) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for p in attrs {
        if take_common(p, &mut common)? {
            continue;
        }
        match p.code {
            10 => cx = parse_f64_pair(p)?,
            20 => cy = parse_f64_pair(p)?,
            40 => radius = parse_f64_pair(p)?,
            // 角度の変換は deg_to_rad の 1 箇所だけを通す。
            50 => start_deg = parse_f64_pair(p)?,
            51 => end_deg = parse_f64_pair(p)?,
            _ => {}
        }
    }
    Ok(EntityRecord {
        geom: Geometry::Arc(Arc::new(
            Point2::new(cx, cy),
            radius,
            deg_to_rad(start_deg),
            deg_to_rad(end_deg),
        )),
        layer: common.layer,
        color: common.color,
    })
}

/// `POLYLINE` + `VERTEX`* + `SEQEND`（R12 の伝統的な形式）を読む。
fn parse_polyline(reader: &mut Reader<'_>) -> Result<EntityRecord> {
    let attrs = reader.read_attrs()?;
    let mut common = CommonAttrs {
        layer: "0".to_string(),
        color: ColorSpec::ByLayer,
    };
    let mut closed = false;
    for p in attrs {
        if take_common(p, &mut common)? {
            continue;
        }
        if p.code == 70 {
            closed = parse_i32_pair(p)? & 1 != 0;
        }
    }

    let mut vertices = Vec::new();
    loop {
        let p = reader.require_pair()?;
        if p.code != 0 {
            // read_attrs() が既に 0 の手前まで読んでいるので通常は通らないが、念のため。
            continue;
        }
        match p.value {
            "VERTEX" => {
                let vattrs = reader.read_attrs()?;
                let (mut vx, mut vy) = (0.0, 0.0);
                for vp in vattrs {
                    match vp.code {
                        10 => vx = parse_f64_pair(vp)?,
                        20 => vy = parse_f64_pair(vp)?,
                        _ => {}
                    }
                }
                vertices.push(Point2::new(vx, vy));
            }
            "SEQEND" => {
                let _ = reader.read_attrs()?;
                break;
            }
            _ => {
                // 未知の内容が紛れ込んでいても諦めずに読み飛ばす。
                let _ = reader.read_attrs()?;
            }
        }
    }

    Ok(EntityRecord {
        geom: Geometry::Polyline(Polyline::new(vertices, closed)),
        layer: common.layer,
        color: common.color,
    })
}

/// `LWPOLYLINE`（R12 には無いが、新しいアプリが吐いたファイル用に読めるようにする）を読む。
///
/// 頂点は `VERTEX` サブエンティティではなく、10/20 の組が並ぶだけの単純な属性列として
/// 埋め込まれている。DXF の慣習どおり 10 の直後に対応する 20 が来る前提で読む。
fn parse_lwpolyline(reader: &mut Reader<'_>) -> Result<EntityRecord> {
    let attrs = reader.read_attrs()?;
    let mut common = CommonAttrs {
        layer: "0".to_string(),
        color: ColorSpec::ByLayer,
    };
    let mut closed = false;
    let mut vertices = Vec::new();
    let mut pending_x: Option<f64> = None;

    for p in attrs {
        if take_common(p, &mut common)? {
            continue;
        }
        match p.code {
            70 => closed = parse_i32_pair(p)? & 1 != 0,
            10 => pending_x = Some(parse_f64_pair(p)?),
            20 => {
                let y = parse_f64_pair(p)?;
                let x = pending_x.take().unwrap_or(0.0);
                vertices.push(Point2::new(x, y));
            }
            _ => {}
        }
    }

    Ok(EntityRecord {
        geom: Geometry::Polyline(Polyline::new(vertices, closed)),
        layer: common.layer,
        color: common.color,
    })
}

fn parse_entities(reader: &mut Reader<'_>, out: &mut Vec<EntityRecord>) -> Result<()> {
    loop {
        let p = reader.require_pair()?;
        if p.code != 0 {
            continue;
        }
        match p.value {
            "ENDSEC" => return Ok(()),
            "LINE" => out.push(parse_line(reader)?),
            "CIRCLE" => out.push(parse_circle(reader)?),
            "ARC" => out.push(parse_arc(reader)?),
            "POLYLINE" => out.push(parse_polyline(reader)?),
            "LWPOLYLINE" => out.push(parse_lwpolyline(reader)?),
            _ => {
                // 知らないエンティティ種別は読み飛ばす。
                let _ = reader.read_attrs()?;
            }
        }
    }
}

// ---- Document の組み立て -----------------------------------------------------

/// レイヤレコードの属性（色・表示・ロック・線種）を対象レイヤへ適用する。
fn apply_layer_props(doc: &mut Document, id: LayerId, rec: &LayerRecord) -> Result<()> {
    doc.apply(Box::new(
        SetLayerProperties::new(id)
            .color(rec.color)
            .visible(rec.visible)
            .locked(rec.locked)
            .linetype(rec.linetype),
    ))
}

/// パース結果から実際の [`Document`] を、コマンド経由でのみ組み立てる。
///
/// `Document` のフィールドは private で、変更経路は [`Command`](crate::Command) しかない
/// （`cad-core` 全体の不変条件）。読み込みも例外ではなく、`AddLayer` /
/// `SetLayerProperties` / `AddEntities` を順に適用してから [`Document::clear_history`] で
/// Undo 履歴を消し、まっさらな「開いた直後」の状態に整える。
fn build_document(
    layer_records: Vec<LayerRecord>,
    entity_records: Vec<EntityRecord>,
) -> Result<Document> {
    let mut doc = Document::new();
    let mut name_to_id: HashMap<String, LayerId> = HashMap::new();
    name_to_id.insert("0".to_string(), LayerId::ZERO);

    for rec in &layer_records {
        if rec.name == "0" {
            apply_layer_props(&mut doc, LayerId::ZERO, rec)?;
            continue;
        }
        doc.apply(Box::new(AddLayer::new(rec.name.clone(), rec.color)))?;
        let id = doc
            .layers()
            .by_name(&rec.name)
            .ok_or(CadError::LayerNotFound)?;
        apply_layer_props(&mut doc, id, rec)?;
        name_to_id.insert(rec.name.clone(), id);
    }

    let mut entities = Vec::with_capacity(entity_records.len());
    for rec in entity_records {
        let layer_id = if let Some(&id) = name_to_id.get(&rec.layer) {
            id
        } else {
            // テーブルに無いレイヤをエンティティが参照している場合は、その場で作る。
            doc.apply(Box::new(AddLayer::new(rec.layer.clone(), AciColor::WHITE)))?;
            let id = doc
                .layers()
                .by_name(&rec.layer)
                .ok_or(CadError::LayerNotFound)?;
            name_to_id.insert(rec.layer.clone(), id);
            id
        };
        let mut entity = Entity::new(rec.geom, layer_id);
        entity.color = rec.color;
        entities.push(entity);
    }

    if !entities.is_empty() {
        doc.apply(Box::new(AddEntities::many("DXF_IMPORT", entities)))?;
    }

    // 開いた直後のファイルは Undo 履歴を持たず、未保存扱いにもしない。
    doc.clear_history();
    doc.mark_saved(None);

    Ok(doc)
}

/// DXF テキストを読み、新しい [`Document`] を組み立てて返す。
///
/// # Errors
///
/// 行数が奇数、`EOF` が無い、数値としてパースできない値がある等、
/// 構文として成立しない入力の場合 [`CadError::Parse`]。
pub fn read_from_str(text: &str) -> Result<Document> {
    let mut reader = Reader::new(text)?;
    let mut layer_records = Vec::new();
    let mut entity_records = Vec::new();
    let mut saw_eof = false;

    while let Some(p) = reader.next_pair()? {
        if p.code != 0 {
            continue;
        }
        match p.value {
            "EOF" => {
                saw_eof = true;
                break;
            }
            "SECTION" => {
                let name_pair = reader.require_pair()?;
                match name_pair.value {
                    "TABLES" => parse_tables(&mut reader, &mut layer_records)?,
                    "ENTITIES" => parse_entities(&mut reader, &mut entity_records)?,
                    _ => skip_section(&mut reader)?,
                }
            }
            _ => {}
        }
    }

    if !saw_eof {
        return Err(CadError::Parse {
            line: reader.lines.len(),
            message: "EOF が見つかりません".to_string(),
        });
    }

    build_document(layer_records, entity_records)
}

/// DXF ファイルを読み、新しい [`Document`] を組み立てて返す。
///
/// # Errors
///
/// ファイルが読めない場合 [`CadError::Io`]、内容が構文として成立しない場合
/// [`CadError::Parse`]。
pub fn read_from_file(path: &Path) -> Result<Document> {
    let text = std::fs::read_to_string(path).map_err(|e| CadError::Io(e.to_string()))?;
    let mut doc = read_from_str(&text)?;
    doc.mark_saved(Some(path.to_path_buf()));
    Ok(doc)
}
