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

use crate::component::{Definition, DefinitionId, ParamDecl, Slot, Value};
use crate::document::Document;
use crate::entity::{Entity, Geometry};
use crate::error::Result;
use crate::expr::{BinOp, Expr, Func1, Func2, ParamType, UnOp};
use crate::geom::Point2;
use crate::group::{Group, GroupId};
use crate::layer::{ColorSpec, Layer, LayerId, LineType};

use super::{
    color_tag, expr_tag, kind, layer_flags, linetype, option_tag, param_type, placement_flags,
    slot_tag, value_tag, FORMAT_VERSION, MAGIC,
};

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

    // コンポーネント定義。ID 昇順。インスタンスはこの並びへの添字で定義を指す。
    let definitions: Vec<_> = doc.definitions().iter().collect();
    let index_of = Indices {
        layers: &layers,
        groups: &groups,
        definitions: &definitions,
    };

    // エンティティ。スロット昇順（= 作成順 = 描画順）。
    // この順序が図面の見た目を決めるので、必ず `EntityStore::iter` の順で書く。
    w.count(doc.entities().len());
    for (_, entity) in doc.entities().iter() {
        write_entity(&mut w, entity, &index_of);
    }

    // 定義は**エンティティより後**。v1 のファイルはここから先が無いだけなので、
    // 後方互換の分岐が「最後まで読んだら終わり」で済む。
    w.count(definitions.len());
    for (_, def) in &definitions {
        w.string(&def.name);
        w.point(def.origin);
        w.count(def.entities.len());
        for entity in &def.entities {
            write_entity(&mut w, entity, &index_of);
        }

        // ---- 形式 v3 以降 ----
        // 追加は既存の並びの**後ろ**へ足す。こうすると古い版は
        // 「そこで終わり」として読める。
        w.count(def.params.len());
        for decl in &def.params {
            write_param(&mut w, decl);
        }
        w.count(def.bindings.len());
        for b in &def.bindings {
            w.count(b.entity);
            write_slot(&mut w, b.slot);
            write_expr(&mut w, &b.expr);
        }
    }

    w.finish()
}

/// パラメータの宣言を書く。
fn write_param(w: &mut Writer, decl: &ParamDecl) {
    w.string(&decl.name);
    match &decl.ty {
        ParamType::Number => w.u8(param_type::NUMBER),
        ParamType::Bool => w.u8(param_type::BOOL),
        ParamType::Choice(options) => {
            w.u8(param_type::CHOICE);
            w.count(options.len());
            for o in options {
                w.string(o);
            }
        }
    }
    // 範囲は Option。
    match decl.range {
        Some((lo, hi)) => {
            w.u8(option_tag::SOME);
            w.f64(lo);
            w.f64(hi);
        }
        None => w.u8(option_tag::NONE),
    }
    write_expr(w, &decl.default);
}

/// 束縛のスロットを書く。
fn write_slot(w: &mut Writer, slot: Slot) {
    match slot {
        Slot::LineAx => w.u8(slot_tag::LINE_AX),
        Slot::LineAy => w.u8(slot_tag::LINE_AY),
        Slot::LineBx => w.u8(slot_tag::LINE_BX),
        Slot::LineBy => w.u8(slot_tag::LINE_BY),
        Slot::CircleCx => w.u8(slot_tag::CIRCLE_CX),
        Slot::CircleCy => w.u8(slot_tag::CIRCLE_CY),
        Slot::CircleR => w.u8(slot_tag::CIRCLE_R),
        Slot::ArcCx => w.u8(slot_tag::ARC_CX),
        Slot::ArcCy => w.u8(slot_tag::ARC_CY),
        Slot::ArcR => w.u8(slot_tag::ARC_R),
        Slot::ArcStart => w.u8(slot_tag::ARC_START),
        Slot::ArcEnd => w.u8(slot_tag::ARC_END),
        Slot::XlineOx => w.u8(slot_tag::XLINE_OX),
        Slot::XlineOy => w.u8(slot_tag::XLINE_OY),
        Slot::XlineAngle => w.u8(slot_tag::XLINE_ANGLE),
        Slot::PolylineVx(i) => {
            w.u8(slot_tag::POLYLINE_VX);
            w.u32(i);
        }
        Slot::PolylineVy(i) => {
            w.u8(slot_tag::POLYLINE_VY);
            w.u32(i);
        }
        Slot::InstanceX => w.u8(slot_tag::INSTANCE_X),
        Slot::InstanceY => w.u8(slot_tag::INSTANCE_Y),
        Slot::InstanceRotation => w.u8(slot_tag::INSTANCE_ROTATION),
        Slot::InstanceScale => w.u8(slot_tag::INSTANCE_SCALE),
    }
}

/// パラメータの値を書く。
fn write_value(w: &mut Writer, v: &Value) {
    match v {
        Value::Number(n) => {
            w.u8(value_tag::NUMBER);
            w.f64(*n);
        }
        Value::Bool(b) => {
            w.u8(value_tag::BOOL);
            w.u8(u8::from(*b));
        }
        Value::Choice(c) => {
            w.u8(value_tag::CHOICE);
            w.string(c);
        }
    }
}

/// 式を**前置記法**で書く。
///
/// 子の個数はタグから決まるので、括弧や終端記号が要らない。
/// 読み込み側も同じ順で再帰するだけで木に戻る。
fn write_expr(w: &mut Writer, e: &Expr) {
    match e {
        Expr::Literal(v) => {
            w.u8(expr_tag::LITERAL);
            write_value(w, v);
        }
        Expr::Var(name) => {
            w.u8(expr_tag::VAR);
            w.string(name);
        }
        Expr::Unary(op, a) => {
            w.u8(expr_tag::UNARY);
            w.u8(unop_code(*op));
            write_expr(w, a);
        }
        Expr::Binary(op, a, b) => {
            w.u8(expr_tag::BINARY);
            w.u8(binop_code(*op));
            write_expr(w, a);
            write_expr(w, b);
        }
        Expr::If {
            cond,
            then,
            otherwise,
        } => {
            w.u8(expr_tag::IF);
            write_expr(w, cond);
            write_expr(w, then);
            write_expr(w, otherwise);
        }
        Expr::Call1(f, a) => {
            w.u8(expr_tag::CALL1);
            w.u8(func1_code(*f));
            write_expr(w, a);
        }
        Expr::Call2(f, a, b) => {
            w.u8(expr_tag::CALL2);
            w.u8(func2_code(*f));
            write_expr(w, a);
            write_expr(w, b);
        }
    }
}

/// 単項演算子の番号。**既存の値は変えない。**
fn unop_code(op: UnOp) -> u8 {
    match op {
        UnOp::Neg => 0,
        UnOp::Not => 1,
    }
}

/// 二項演算子の番号。**既存の値は変えない。**
fn binop_code(op: BinOp) -> u8 {
    match op {
        BinOp::Add => 0,
        BinOp::Sub => 1,
        BinOp::Mul => 2,
        BinOp::Div => 3,
        BinOp::Lt => 4,
        BinOp::Le => 5,
        BinOp::Gt => 6,
        BinOp::Ge => 7,
        BinOp::Eq => 8,
        BinOp::Ne => 9,
        BinOp::And => 10,
        BinOp::Or => 11,
    }
}

/// 1 引数の関数の番号。**既存の値は変えない。**
fn func1_code(f: Func1) -> u8 {
    match f {
        Func1::Sin => 0,
        Func1::Cos => 1,
        Func1::Tan => 2,
        Func1::Sqrt => 3,
        Func1::Abs => 4,
        Func1::Floor => 5,
        Func1::Ceil => 6,
        Func1::Round => 7,
        Func1::Deg => 8,
        Func1::Rad => 9,
    }
}

/// 2 引数の関数の番号。**既存の値は変えない。**
fn func2_code(f: Func2) -> u8 {
    match f {
        Func2::Min => 0,
        Func2::Max => 1,
        Func2::Atan2 => 2,
        Func2::Pow => 3,
    }
}

/// ID → ファイル内の添字を引くための対応表。
///
/// **ID 値ではなく添字で書く。** ID の再現に依存しないので、
/// 将来 ID の割り当て方が変わっても壊れない。
struct Indices<'a> {
    layers: &'a [(LayerId, &'a Layer)],
    groups: &'a [(GroupId, &'a Group)],
    definitions: &'a [(DefinitionId, &'a Definition)],
}

/// エンティティ 1 件を書く。
///
/// 図面直下の要素と定義の中身で**同じ形**を使う。分けると片方だけ直し忘れる。
fn write_entity(w: &mut Writer, entity: &Entity, idx: &Indices<'_>) {
    write_geometry(w, &entity.geom, idx);

    let layer_index = idx
        .layers
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
        .and_then(|g| idx.groups.iter().position(|(id, _)| *id == g))
    {
        Some(index) => {
            w.u8(option_tag::SOME);
            w.count(index);
        }
        None => w.u8(option_tag::NONE),
    }
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
fn write_geometry(w: &mut Writer, geom: &Geometry, idx: &Indices<'_>) {
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
        Geometry::Instance(i) => {
            w.u8(kind::INSTANCE);
            let def_index = idx
                .definitions
                .iter()
                .position(|(id, _)| *id == i.definition)
                .unwrap_or(0);
            w.count(def_index);
            w.point(i.placement.origin);
            w.f64(i.placement.rotation);
            w.f64(i.placement.scale);
            let mut flags = 0u8;
            if i.placement.flipped {
                flags |= placement_flags::FLIPPED;
            }
            w.u8(flags);
            // パラメータの個別上書き。名前の昇順（`BTreeMap` の走査順）で安定する。
            w.count(i.overrides.len());
            for (name, value) in &i.overrides {
                w.string(name);
                write_value(w, value);
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
