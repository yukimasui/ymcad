//! DXF R12 の読み込み。
//!
//! # 方針
//!
//! - 知らないセクション・知らないエンティティ種別は **エラーにせず読み飛ばす**。
//!   現実の DXF ファイルには本プロトタイプが扱わない要素（`TEXT`, `DIMENSION`, …）が
//!   大量に含まれるため。読まなかった種類と件数は警告で返す（[`ReadReport`]）。
//! - `LINE` / `CIRCLE` / `ARC` / `POLYLINE`+`VERTEX`+`SEQEND` / `LWPOLYLINE` / `INSERT` は理解する。
//!   ポリラインは 2 形式ともポリラインとして読み込む（自分では `POLYLINE` 形式でしか
//!   書き出さないが、他アプリが吐いた `LWPOLYLINE` も受け付ける）。
//! - `BLOCKS` セクションのブロックはコンポーネント定義に、`INSERT` はインスタンスにする
//!   （Issue #109、ADR-0050）。組み立ては [`build`] を参照。
//! - 押し出し方向（210/220/230）が `(0, 0, -1)` の図形は OCS の X が反転しているので、
//!   ワールドへ移すときに Y 軸で鏡像にする。Z 軸から傾いた押し出しは 2D に落とせないので
//!   読まずに警告する。`LINE` の座標はワールド座標（WCS）なので押し出し方向に関係しない。
//! - `LAYER` テーブルを読み、レイヤの色・表示・ロック・線種を復元する。
//!   テーブルに無いレイヤ名をエンティティが参照している場合は、その場で作る。
//! - 数値としてパースできない値やファイルの途中切れは [`CadError::Parse`] として
//!   はっきり報告する。

mod build;
mod xform;

use std::collections::BTreeMap;
use std::path::Path;

use super::deg_to_rad;
use crate::document::Document;
use crate::entity::Geometry;
use crate::error::{CadError, Result};
use crate::geom::tolerance::is_zero_len;
use crate::geom::{Arc, Circle, Line, Point2, Polyline};
use crate::layer::{AciColor, ColorSpec, LineType};

/// 読み込みの結果。
///
/// `warnings` には、ymcad で表せずに近似したもの・読まなかったものの説明が入る。
/// 呼び出し側はこれをユーザーへ見せること。書き出しの [`super::write::WriteReport`] と同じ約束で、
/// 黙って情報を落とさないための仕組み（ADR-0021 / ADR-0050）。
#[derive(Debug)]
pub struct ReadReport {
    /// 組み立てた図面。Undo 履歴は空で、未保存の変更も無い。
    pub document: Document,
    /// 近似・欠落の警告。
    pub warnings: Vec<String>,
}

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

    /// 次のペアの値だけを覗き見る（消費しない）。
    fn peek_value(&self) -> Option<&'a str> {
        self.lines.get(self.pos + 1).copied()
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

// ---- 色・押し出し方向 -------------------------------------------------------

/// エンティティの色（62）を読む。
///
/// - `256` は BYLAYER（`ColorSpec::ByLayer`）
/// - `0` は BYBLOCK。`ColorSpec` に BYBLOCK が無いので BYLAYER に寄せ、件数を数えて警告する
///   （変種を足すのはエンティティモデルの変更になる。ADR-0050）
/// - `1..=255` は ACI。負の値（エンティティでは本来使わない）は絶対値を取る
/// - それ以外（`257` = BYOBJECT など）は BYLAYER
fn color_from_pair(p: Pair<'_>, tally: &mut Tally) -> Result<ColorSpec> {
    let raw = parse_i32_pair(p)?.unsigned_abs();
    Ok(match raw {
        0 => {
            tally.byblock += 1;
            ColorSpec::ByLayer
        }
        1..=255 => ColorSpec::Aci(AciColor(u8::try_from(raw).unwrap_or(u8::MAX))),
        _ => ColorSpec::ByLayer,
    })
}

/// 押し出し方向（210/220/230）から決まる、OCS → ワールドの扱い。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ocs {
    /// `(0, 0, 1)`。OCS がそのままワールド。
    Identity,
    /// `(0, 0, -1)`。arbitrary axis algorithm で OCS の X 軸が `(-1, 0, 0)`、
    /// Y 軸が `(0, 1, 0)` になるので、ワールドでは Y 軸に関する鏡像。
    MirrorX,
    /// Z 軸から傾いている。2D に落とせない。
    Oblique,
}

impl Ocs {
    fn from_extrusion(e: [f64; 3]) -> Self {
        let len = (e[0] * e[0] + e[1] * e[1] + e[2] * e[2]).sqrt();
        if !len.is_finite() || is_zero_len(len) {
            // 零ベクトルは不正。既定の向きとして扱う。
            return Self::Identity;
        }
        if is_zero_len(e[0] / len) && is_zero_len(e[1] / len) {
            if e[2] > 0.0 {
                Self::Identity
            } else {
                Self::MirrorX
            }
        } else {
            Self::Oblique
        }
    }
}

/// 押し出し方向 Z<0 のときにかける鏡像の軸（原点を通る垂直線）。
const Y_AXIS: Line = Line {
    a: Point2 { x: 0.0, y: 0.0 },
    b: Point2 { x: 0.0, y: 1.0 },
};

/// OCS の図形をワールドへ移す。傾いた押し出しなら数えて `None`。
///
/// 鏡像に `Geometry::mirrored` を使うのは、円弧の開始角・終了角の入れ替え
/// （ADR-0020）を再実装しないため。
fn ocs_to_world(geom: Geometry, ocs: Ocs, tally: &mut Tally) -> Option<Geometry> {
    match ocs {
        Ocs::Identity => Some(geom),
        Ocs::MirrorX => Some(geom.mirrored(&Y_AXIS)),
        Ocs::Oblique => {
            tally.oblique += 1;
            None
        }
    }
}

// ---- 中間表現 ---------------------------------------------------------------

/// 読んだ図形 1 つ。レイヤは名前のまま持ち、`Document` の組み立てで解決する。
struct RawEntity {
    kind: RawKind,
    layer: String,
    color: ColorSpec,
}

/// 図形の中身。`INSERT` は参照先のブロックが揃ってから解決するので別に持つ。
enum RawKind {
    /// 線分・円・円弧・ポリライン（ワールド座標、またはブロック座標）。
    Geom(Geometry),
    /// ブロック参照。
    Insert(InsertRecord),
}

/// `INSERT` 1 件ぶんの中間表現。
struct InsertRecord {
    /// 参照するブロック名（2）。
    block: String,
    /// 挿入点（10/20、OCS）。
    insert: Point2,
    /// X / Y 倍率（41/42、既定 1）。43（Z）は 2D なので使わない。
    sx: f64,
    sy: f64,
    /// 回転角 [rad]（50 は度）。
    rotation: f64,
    /// 押し出し方向が Z<0。
    mirror_ocs: bool,
    /// MINSERT の列数（70）・行数（71）。0 以下は 1 とみなす。
    cols: u32,
    rows: u32,
    /// MINSERT の列の間隔（44）・行の間隔（45）。
    col_spacing: f64,
    row_spacing: f64,
}

/// `BLOCK` … `ENDBLK` 1 つぶんの中間表現。
struct BlockRecord {
    /// ブロック名（2）。
    name: String,
    /// フラグ（70）。1 = 匿名、4 = 外部参照、8 = 外部参照のオーバーレイ。
    flags: i32,
    /// 基点（10/20）。
    base: Point2,
    /// 中身。
    entities: Vec<RawEntity>,
    /// 中身を読むときに数えたもの。定義にしたブロックのぶんだけ警告に足す。
    tally: Tally,
}

/// 読むときに数えるもの。ブロックごとと `ENTITIES` ごとに持ち、使ったぶんだけ警告に足す
/// （寸法の匿名ブロック `*D…` のように定義にしないブロックの中身まで数えないため）。
#[derive(Default)]
struct Tally {
    /// 読まなかった種類と件数。
    unread: BTreeMap<String, usize>,
    /// BYBLOCK を BYLAYER に寄せた件数。
    byblock: usize,
    /// 押し出し方向が傾いていて読まなかった件数。
    oblique: usize,
}

impl Tally {
    fn skip(&mut self, kind: &str) {
        *self.unread.entry(kind.to_owned()).or_default() += 1;
    }

    fn absorb(&mut self, other: &Self) {
        for (k, n) in &other.unread {
            *self.unread.entry(k.clone()).or_default() += n;
        }
        self.byblock += other.byblock;
        self.oblique += other.oblique;
    }
}

// ---- エンティティ（ENTITIES と BLOCKS で共通） --------------------------------

/// 図形に共通する属性（レイヤ・色・押し出し方向）。
struct CommonAttrs {
    layer: String,
    color: ColorSpec,
    extrusion: [f64; 3],
}

impl CommonAttrs {
    fn new() -> Self {
        Self {
            layer: "0".to_string(),
            color: ColorSpec::ByLayer,
            extrusion: [0.0, 0.0, 1.0],
        }
    }

    /// 共通の属性なら取り込んで `true`。
    fn take(&mut self, p: Pair<'_>, tally: &mut Tally) -> Result<bool> {
        match p.code {
            8 => self.layer = p.value.to_string(),
            62 => self.color = color_from_pair(p, tally)?,
            210 => self.extrusion[0] = parse_f64_pair(p)?,
            220 => self.extrusion[1] = parse_f64_pair(p)?,
            230 => self.extrusion[2] = parse_f64_pair(p)?,
            _ => return Ok(false),
        }
        Ok(true)
    }

    fn ocs(&self) -> Ocs {
        Ocs::from_extrusion(self.extrusion)
    }

    fn into_raw(self, kind: RawKind) -> RawEntity {
        RawEntity {
            kind,
            layer: self.layer,
            color: self.color,
        }
    }

    fn into_geom(self, geom: Geometry) -> RawEntity {
        self.into_raw(RawKind::Geom(geom))
    }
}

/// `LINE`。座標はワールド座標（WCS）なので、押し出し方向は形に関係しない。
fn parse_line(reader: &mut Reader<'_>, tally: &mut Tally) -> Result<Option<RawEntity>> {
    let mut common = CommonAttrs::new();
    let (mut ax, mut ay, mut bx, mut by) = (0.0, 0.0, 0.0, 0.0);
    for p in reader.read_attrs()? {
        if common.take(p, tally)? {
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
    let geom = Geometry::Line(Line::new(Point2::new(ax, ay), Point2::new(bx, by)));
    Ok(Some(common.into_geom(geom)))
}

fn parse_circle(reader: &mut Reader<'_>, tally: &mut Tally) -> Result<Option<RawEntity>> {
    let mut common = CommonAttrs::new();
    let (mut cx, mut cy, mut radius) = (0.0, 0.0, 0.0);
    for p in reader.read_attrs()? {
        if common.take(p, tally)? {
            continue;
        }
        match p.code {
            10 => cx = parse_f64_pair(p)?,
            20 => cy = parse_f64_pair(p)?,
            40 => radius = parse_f64_pair(p)?,
            _ => {}
        }
    }
    let geom = Geometry::Circle(Circle::new(Point2::new(cx, cy), radius));
    let ocs = common.ocs();
    Ok(ocs_to_world(geom, ocs, tally).map(|g| common.into_geom(g)))
}

fn parse_arc(reader: &mut Reader<'_>, tally: &mut Tally) -> Result<Option<RawEntity>> {
    let mut common = CommonAttrs::new();
    let (mut cx, mut cy, mut radius, mut start_deg, mut end_deg) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for p in reader.read_attrs()? {
        if common.take(p, tally)? {
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
    let geom = Geometry::Arc(Arc::new(
        Point2::new(cx, cy),
        radius,
        deg_to_rad(start_deg),
        deg_to_rad(end_deg),
    ));
    let ocs = common.ocs();
    Ok(ocs_to_world(geom, ocs, tally).map(|g| common.into_geom(g)))
}

/// `POLYLINE` のフラグ（70）のうち、頂点がワールド座標になるもの
/// （8 = 3D ポリライン、16 = 3D メッシュ、64 = ポリフェースメッシュ）。
/// それ以外（2D ポリライン）の頂点は OCS。
const POLYLINE_WCS_FLAGS: i32 = 8 | 16 | 64;

/// `POLYLINE` + `VERTEX`* + `SEQEND`（R12 の伝統的な形式）を読む。
fn parse_polyline(reader: &mut Reader<'_>, tally: &mut Tally) -> Result<Option<RawEntity>> {
    let mut common = CommonAttrs::new();
    let mut flags = 0;
    for p in reader.read_attrs()? {
        if common.take(p, tally)? {
            continue;
        }
        if p.code == 70 {
            flags = parse_i32_pair(p)?;
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
                let (mut vx, mut vy) = (0.0, 0.0);
                for vp in reader.read_attrs()? {
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

    let geom = Geometry::Polyline(Polyline::new(vertices, flags & 1 != 0));
    let ocs = if flags & POLYLINE_WCS_FLAGS != 0 {
        Ocs::Identity
    } else {
        common.ocs()
    };
    Ok(ocs_to_world(geom, ocs, tally).map(|g| common.into_geom(g)))
}

/// `LWPOLYLINE`（R12 には無いが、新しいアプリが吐いたファイル用に読めるようにする）を読む。
///
/// 頂点は `VERTEX` サブエンティティではなく、10/20 の組が並ぶだけの単純な属性列として
/// 埋め込まれている。DXF の慣習どおり 10 の直後に対応する 20 が来る前提で読む。
fn parse_lwpolyline(reader: &mut Reader<'_>, tally: &mut Tally) -> Result<Option<RawEntity>> {
    let mut common = CommonAttrs::new();
    let mut closed = false;
    let mut vertices = Vec::new();
    let mut pending_x: Option<f64> = None;

    for p in reader.read_attrs()? {
        if common.take(p, tally)? {
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

    let geom = Geometry::Polyline(Polyline::new(vertices, closed));
    let ocs = common.ocs();
    Ok(ocs_to_world(geom, ocs, tally).map(|g| common.into_geom(g)))
}

/// `INSERT`（と MINSERT）を読む。
///
/// 66 = 1 なら後ろに `ATTRIB` … `SEQEND` が続くので、ここでまとめて読み飛ばす
/// （属性の中身は文字なので非スコープ。件数は「読まなかった種類」に数える）。
fn parse_insert(reader: &mut Reader<'_>, tally: &mut Tally) -> Result<Option<RawEntity>> {
    let mut common = CommonAttrs::new();
    let mut block = String::new();
    let (mut ix, mut iy) = (0.0, 0.0);
    let (mut sx, mut sy, mut rot_deg) = (1.0, 1.0, 0.0);
    let (mut cols, mut rows) = (1, 1);
    let (mut col_spacing, mut row_spacing) = (0.0, 0.0);
    let mut attribs_follow = false;
    for p in reader.read_attrs()? {
        if common.take(p, tally)? {
            continue;
        }
        match p.code {
            2 => block = p.value.to_string(),
            10 => ix = parse_f64_pair(p)?,
            20 => iy = parse_f64_pair(p)?,
            41 => sx = parse_f64_pair(p)?,
            42 => sy = parse_f64_pair(p)?,
            50 => rot_deg = parse_f64_pair(p)?,
            70 => cols = parse_i32_pair(p)?,
            71 => rows = parse_i32_pair(p)?,
            44 => col_spacing = parse_f64_pair(p)?,
            45 => row_spacing = parse_f64_pair(p)?,
            66 => attribs_follow = parse_i32_pair(p)? == 1,
            _ => {}
        }
    }

    if attribs_follow {
        skip_attribs(reader, tally)?;
    }

    let mirror_ocs = match common.ocs() {
        Ocs::Identity => false,
        Ocs::MirrorX => true,
        Ocs::Oblique => {
            tally.oblique += 1;
            return Ok(None);
        }
    };
    let count = |n: i32| u32::try_from(n).unwrap_or(0).max(1);
    let record = InsertRecord {
        block,
        insert: Point2::new(ix, iy),
        sx,
        sy,
        rotation: deg_to_rad(rot_deg),
        mirror_ocs,
        cols: count(cols),
        rows: count(rows),
        col_spacing,
        row_spacing,
    };
    Ok(Some(common.into_raw(RawKind::Insert(record))))
}

/// `INSERT` の後ろの `ATTRIB`* `SEQEND` を読み飛ばす。
///
/// `SEQEND` が無いまま次の図形が来た壊れたファイルでは、そこで止める（次の図形を食べない）。
fn skip_attribs(reader: &mut Reader<'_>, tally: &mut Tally) -> Result<()> {
    while reader.peek_code() == Some(0) {
        match reader.peek_value() {
            Some("ATTRIB") => {
                reader.require_pair()?;
                let _ = reader.read_attrs()?;
                tally.skip("ATTRIB");
            }
            Some("SEQEND") => {
                reader.require_pair()?;
                let _ = reader.read_attrs()?;
                break;
            }
            _ => break,
        }
    }
    Ok(())
}

/// 図形 1 つを読む（直前に `0` / `kind` のペアを読んだところから）。
///
/// `ENTITIES` とブロックの中身で**同じ関数**を使う。読めない種類は数えて読み飛ばす。
fn parse_entity(
    reader: &mut Reader<'_>,
    kind: &str,
    tally: &mut Tally,
) -> Result<Option<RawEntity>> {
    match kind {
        "LINE" => parse_line(reader, tally),
        "CIRCLE" => parse_circle(reader, tally),
        "ARC" => parse_arc(reader, tally),
        "POLYLINE" => parse_polyline(reader, tally),
        "LWPOLYLINE" => parse_lwpolyline(reader, tally),
        "INSERT" => parse_insert(reader, tally),
        // 対になる相手の無い SEQEND は中身が無いので数えない。
        "SEQEND" => {
            let _ = reader.read_attrs()?;
            Ok(None)
        }
        _ => {
            // 知らないエンティティ種別は読み飛ばし、種類ごとに数える。
            let _ = reader.read_attrs()?;
            tally.skip(kind);
            Ok(None)
        }
    }
}

fn parse_entities(
    reader: &mut Reader<'_>,
    out: &mut Vec<RawEntity>,
    tally: &mut Tally,
) -> Result<()> {
    loop {
        let p = reader.require_pair()?;
        if p.code != 0 {
            continue;
        }
        if p.value == "ENDSEC" {
            return Ok(());
        }
        if let Some(e) = parse_entity(reader, p.value, tally)? {
            out.push(e);
        }
    }
}

// ---- BLOCKS セクション --------------------------------------------------------

/// `BLOCKS` セクションを読む。
fn parse_blocks(reader: &mut Reader<'_>, out: &mut Vec<BlockRecord>) -> Result<()> {
    loop {
        let p = reader.require_pair()?;
        if p.code != 0 {
            continue;
        }
        match p.value {
            "ENDSEC" => return Ok(()),
            "BLOCK" => {
                let (block, section_ended) = parse_block(reader)?;
                out.push(block);
                if section_ended {
                    return Ok(());
                }
            }
            _ => {
                let _ = reader.read_attrs()?;
            }
        }
    }
}

/// `BLOCK` の見出しから `ENDBLK` までを読む。
///
/// `ENDBLK` が無いまま `ENDSEC` に着いた壊れたファイルでは、そこまでを中身とし、
/// セクションも終わったことを `true` で返す。
fn parse_block(reader: &mut Reader<'_>) -> Result<(BlockRecord, bool)> {
    let mut name = None;
    let mut alt_name = None;
    let mut flags = 0;
    let (mut bx, mut by) = (0.0, 0.0);
    for p in reader.read_attrs()? {
        match p.code {
            2 => name = Some(p.value.to_string()),
            3 => alt_name = Some(p.value.to_string()),
            70 => flags = parse_i32_pair(p)?,
            10 => bx = parse_f64_pair(p)?,
            20 => by = parse_f64_pair(p)?,
            _ => {}
        }
    }
    let mut block = BlockRecord {
        name: name.or(alt_name).unwrap_or_default(),
        flags,
        base: Point2::new(bx, by),
        entities: Vec::new(),
        tally: Tally::default(),
    };

    loop {
        let p = reader.require_pair()?;
        if p.code != 0 {
            continue;
        }
        match p.value {
            "ENDBLK" => {
                let _ = reader.read_attrs()?;
                return Ok((block, false));
            }
            "ENDSEC" => return Ok((block, true)),
            kind => {
                if let Some(e) = parse_entity(reader, kind, &mut block.tally)? {
                    block.entities.push(e);
                }
            }
        }
    }
}

// ---- 公開 API ---------------------------------------------------------------

/// DXF テキストを読み、新しい [`Document`] を組み立てて返す。
///
/// # Errors
///
/// 行数が奇数、`EOF` が無い、数値としてパースできない値がある等、
/// 構文として成立しない入力の場合 [`CadError::Parse`]。
pub fn read_from_str(text: &str) -> Result<ReadReport> {
    let mut reader = Reader::new(text)?;
    let mut layer_records = Vec::new();
    let mut entity_records = Vec::new();
    let mut entity_tally = Tally::default();
    let mut block_records = Vec::new();
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
                    "BLOCKS" => parse_blocks(&mut reader, &mut block_records)?,
                    "ENTITIES" => {
                        parse_entities(&mut reader, &mut entity_records, &mut entity_tally)?;
                    }
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

    build::build_document(layer_records, entity_records, entity_tally, block_records)
}

/// DXF ファイルを読み、新しい [`Document`] を組み立てて返す。
///
/// # Errors
///
/// ファイルが読めない場合 [`CadError::Io`]、内容が構文として成立しない場合
/// [`CadError::Parse`]。
pub fn read_from_file(path: &Path) -> Result<ReadReport> {
    let text = std::fs::read_to_string(path).map_err(|e| CadError::Io(e.to_string()))?;
    let mut report = read_from_str(&text)?;
    report.document.mark_saved(Some(path.to_path_buf()));
    Ok(report)
}
