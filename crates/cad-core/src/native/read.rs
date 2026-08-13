//! ネイティブ形式（`.ymc`）の読み込み。
//!
//! # 壊れた入力について
//!
//! **`panic!` / `unwrap` / スライスの直接添字を使わない。**
//! ファイルは外から来るので、途中で切れていても未知の値が入っていても
//! エラーとして返すこと。バイナリは目で見て気づけないぶん、ここが緩いと
//! クラッシュの原因になる。
//!
//! # 組み立て方
//!
//! `Document` のフィールドは private で、変更経路は
//! [`Command`](crate::Command) だけ（`cad-core` 全体の不変条件）。
//! **読み込みも例外にしない。** `dxf/read.rs` の `build_document` と同じ流儀で
//! `AddLayer` / `SetLayerProperties` / `AddEntities` / `CreateGroup` を順に適用し、
//! 最後に [`Document::mark_saved`] で Undo 履歴を捨てる。

use std::path::Path;

use std::collections::BTreeMap;

use crate::command::{
    AddEntities, AddLayer, CreateGroup, DefineComponent, SetDefinitionContents,
    SetDefinitionParams, SetLayerProperties,
};
use crate::component::{Binding, DefinitionId, Instance, ParamDecl, Placement, Slot, Value};
use crate::document::Document;
use crate::entity::{Entity, EntityId, Geometry};
use crate::error::{CadError, Result};
use crate::expr::{BinOp, Expr, Func1, Func2, ParamType, UnOp};
use crate::geom::tolerance::eq_len;
use crate::geom::{Arc, Circle, Line, Point2, Polyline, Vec2, Xline};
use crate::layer::{AciColor, ColorSpec, LayerId, LineType};

use super::{
    color_tag, expr_tag, kind, layer_flags, linetype, option_tag, param_type, placement_flags,
    slot_tag, value_tag, FORMAT_VERSION, MAGIC, VERSION_WITH_COMPONENTS, VERSION_WITH_PARAMS,
};

/// 読み取ったレイヤ 1 件。
struct LayerRecord {
    name: String,
    color: AciColor,
    visible: bool,
    locked: bool,
    linetype: LineType,
}

/// 読み取ったエンティティ 1 件。レイヤ・グループ・定義はファイル内の添字のまま持つ。
struct EntityRecord {
    geom: GeomRecord,
    layer_index: usize,
    color: ColorSpec,
    group_index: Option<usize>,
}

/// 読み取った図形。
///
/// インスタンスは**定義をファイル内の添字で持つ**（まだ `DefinitionId` に直せない）。
/// 定義セクションはエンティティより後ろにあり、しかも前方参照できるので、
/// 全部読み終わってから ID へ解決する 2 パス構成にする。
enum GeomRecord {
    /// インスタンス以外。そのまま使える。
    Plain(Geometry),
    /// インスタンス。定義の添字と配置・上書き。
    Instance {
        definition_index: usize,
        placement: Placement,
        overrides: BTreeMap<String, Value>,
    },
}

/// 読み取った定義 1 件。
struct DefinitionRecord {
    name: String,
    origin: Point2,
    entities: Vec<EntityRecord>,
    /// 形式 v3 以降。古い版では空。
    params: Vec<ParamDecl>,
    /// 形式 v3 以降。古い版では空。
    bindings: Vec<Binding>,
}

/// バイト列からネイティブ形式の図面を読む。
///
/// # Errors
///
/// 識別子が違う、バージョンが新しすぎる、途中で尽きている、未知のタグがある等の場合
/// [`CadError::Format`]。図面として組み立てられない場合はその由のエラー。
pub fn read_from_bytes(bytes: &[u8]) -> Result<Document> {
    let mut r = Reader::new(bytes);

    let magic = r.take(MAGIC.len())?;
    if magic != MAGIC.as_slice() {
        return Err(r.error_at(0, "ymcad の図面ファイルではありません"));
    }

    let version = r.u32()?;
    if version > FORMAT_VERSION {
        return Err(r.error(format!(
            "新しいバージョンの ymcad で作られた図面です（形式 {version}、\
             このプログラムが読めるのは {FORMAT_VERSION} まで）"
        )));
    }

    let layer_count = r.count()?;
    let mut layers = Vec::with_capacity(layer_count.min(r.remaining()));
    for _ in 0..layer_count {
        layers.push(read_layer(&mut r)?);
    }

    let group_count = r.count()?;
    let mut groups = Vec::with_capacity(group_count.min(r.remaining()));
    for _ in 0..group_count {
        groups.push(r.string()?);
    }

    let entity_count = r.count()?;
    let mut entities = Vec::with_capacity(entity_count.min(r.remaining()));
    for _ in 0..entity_count {
        entities.push(read_entity(&mut r, layers.len(), groups.len())?);
    }

    // 形式 v1 にはコンポーネント定義のセクションが無い。**そこで終わり**。
    // 前半の表現は v1 と v2 で変わっていないので、分岐はこれだけで済む。
    let mut definitions: Vec<DefinitionRecord> = Vec::new();
    if version >= VERSION_WITH_COMPONENTS {
        let def_count = r.count()?;
        definitions.reserve(def_count.min(r.remaining()));
        for _ in 0..def_count {
            let name = r.string()?;
            let origin = r.point()?;
            let n = r.count()?;
            let mut inner = Vec::with_capacity(n.min(r.remaining()));
            for _ in 0..n {
                inner.push(read_entity(&mut r, layers.len(), groups.len())?);
            }
            // ---- 形式 v3 以降 ----
            // 追加は既存の並びの後ろにあるので、古い版は「そこで終わり」になる。
            let mut params = Vec::new();
            let mut bindings = Vec::new();
            if version >= VERSION_WITH_PARAMS {
                let np = r.count()?;
                params.reserve(np.min(r.remaining()));
                for _ in 0..np {
                    params.push(read_param(&mut r)?);
                }
                let nb = r.count()?;
                bindings.reserve(nb.min(r.remaining()));
                for _ in 0..nb {
                    let entity = r.count()?;
                    let slot = read_slot(&mut r)?;
                    let expr = read_expr(&mut r, 0)?;
                    bindings.push(Binding::new(entity, slot, expr));
                }
            }

            definitions.push(DefinitionRecord {
                name,
                origin,
                entities: inner,
                params,
                bindings,
            });
        }
    }

    // 定義の添字が範囲内であることを、組み立てる前にまとめて検査する。
    // 前方参照があるので、読みながらでは検査できない。
    let def_count = definitions.len();
    for rec in entities
        .iter()
        .chain(definitions.iter().flat_map(|d| d.entities.iter()))
    {
        if let GeomRecord::Instance {
            definition_index, ..
        } = &rec.geom
        {
            if *definition_index >= def_count {
                return Err(r.error(format!(
                    "コンポーネント定義の参照が範囲外です: {definition_index}（定義数 {def_count}）"
                )));
            }
        }
    }

    // **末尾でぴったり尽きること。**
    //
    // これが無いと、書き手が足したセクションを読み手が読み飛ばしても
    // 静かに通ってしまう。実際 2026-08-13 に、形式 v3 のパラメータを
    // 書いているのに読んでいない状態を、この検査が無いせいで
    // ラウンドトリップテストが見逃した（`tools/validate_ymc.py` は
    // 最初からこれを見ていた）。
    if r.remaining() != 0 {
        return Err(r.error(format!(
            "{} バイトが読み残されています（形式の解釈がずれています）",
            r.remaining()
        )));
    }

    build_document(layers, groups, definitions, entities)
}

/// ネイティブ形式のファイルを読む。
///
/// # Errors
///
/// ファイルが読めない場合 [`CadError::Io`]、内容が形式として成立しない場合
/// [`CadError::Format`]。
pub fn read_from_file(path: &Path) -> Result<Document> {
    let bytes = std::fs::read(path).map_err(|e| CadError::Io(e.to_string()))?;
    let mut doc = read_from_bytes(&bytes)?;
    doc.mark_saved(Some(path.to_path_buf()));
    Ok(doc)
}

fn read_layer(r: &mut Reader<'_>) -> Result<LayerRecord> {
    let name = r.string()?;
    let color = AciColor(r.u8()?);
    let flags = r.u8()?;
    let lt = r.u8()?;
    Ok(LayerRecord {
        name,
        color,
        visible: flags & layer_flags::VISIBLE != 0,
        locked: flags & layer_flags::LOCKED != 0,
        linetype: match lt {
            linetype::CONTINUOUS => LineType::Continuous,
            linetype::DASHED => LineType::Dashed,
            linetype::CENTER => LineType::Center,
            linetype::HIDDEN => LineType::Hidden,
            other => return Err(r.error(format!("未知の線種です: {other}"))),
        },
    })
}

fn read_entity(r: &mut Reader<'_>, layer_count: usize, group_count: usize) -> Result<EntityRecord> {
    let geom = read_geometry(r)?;

    let layer_index = r.count()?;
    if layer_index >= layer_count {
        return Err(r.error(format!(
            "レイヤの参照が範囲外です: {layer_index}（レイヤ数 {layer_count}）"
        )));
    }

    let color = match r.u8()? {
        color_tag::BY_LAYER => ColorSpec::ByLayer,
        color_tag::ACI => ColorSpec::Aci(AciColor(r.u8()?)),
        other => return Err(r.error(format!("未知の色指定です: {other}"))),
    };

    let group_index = match r.u8()? {
        option_tag::NONE => None,
        option_tag::SOME => {
            let index = r.count()?;
            if index >= group_count {
                return Err(r.error(format!(
                    "グループの参照が範囲外です: {index}（グループ数 {group_count}）"
                )));
            }
            Some(index)
        }
        other => return Err(r.error(format!("未知のグループ指定です: {other}"))),
    };

    Ok(EntityRecord {
        geom,
        layer_index,
        color,
        group_index,
    })
}

fn read_geometry(r: &mut Reader<'_>) -> Result<GeomRecord> {
    let tag = r.u8()?;
    match tag {
        kind::LINE => Ok(GeomRecord::Plain(Geometry::Line(Line::new(
            r.point()?,
            r.point()?,
        )))),
        kind::CIRCLE => Ok(GeomRecord::Plain(Geometry::Circle(Circle::new(
            r.point()?,
            r.f64()?,
        )))),
        kind::ARC => Ok(GeomRecord::Plain(Geometry::Arc(Arc::new(
            r.point()?,
            r.f64()?,
            r.f64()?,
            r.f64()?,
        )))),
        kind::XLINE => {
            let origin = r.point()?;
            let direction = Vec2::new(r.f64()?, r.f64()?);
            // ここで `Xline::new` を通してはいけない。中で `normalized()` が
            // 除算をやり直すため、**既に正規化済みの値が 1 ULP ずれる**
            // （例: 0.7071067811865475 → 0.7071067811865476）。
            // 往復が非可逆になり、この形式の存在理由に反する。
            //
            // 値はそのまま使い、代わりに「単位ベクトルであること」を検査して
            // 不変条件を守る。`Xline::new` は非ゼロなら何でも受け取って正規化してしまうので、
            // 壊れたファイルに対する検査としてはこちらのほうが強い。
            if !eq_len(direction.len(), 1.0) {
                return Err(r.error(format!(
                    "作図線の方向が単位ベクトルではありません（長さ {}）",
                    direction.len()
                )));
            }
            Ok(GeomRecord::Plain(Geometry::Xline(Xline {
                origin,
                direction,
            })))
        }
        kind::POLYLINE => {
            let closed = r.u8()? != 0;
            let count = r.count()?;
            let mut vertices = Vec::with_capacity(count.min(r.remaining()));
            for _ in 0..count {
                vertices.push(r.point()?);
            }
            Ok(GeomRecord::Plain(Geometry::Polyline(Polyline::new(
                vertices, closed,
            ))))
        }
        kind::INSTANCE => {
            let definition_index = r.count()?;
            let origin = r.point()?;
            let rotation = r.f64()?;
            let scale = r.f64()?;
            let flags = r.u8()?;
            let unknown = flags & !placement_flags::FLIPPED;
            if unknown != 0 {
                return Err(r.error(format!(
                    "配置に未定義のフラグビットがあります: {unknown:#04x}"
                )));
            }
            // ファイルの中身を信用せず、`Placement` の不変条件（倍率は正の有限値）を
            // コンストラクタに検査させる。手で壊されたファイルでも不変条件を破らない。
            let placement = Placement::new(
                origin,
                rotation,
                scale,
                flags & placement_flags::FLIPPED != 0,
            )
            .map_err(|e| r.error(format!("配置が妥当ではありません: {e}")))?;

            let n = r.count()?;
            let mut overrides = BTreeMap::new();
            for _ in 0..n {
                let name = r.string()?;
                let value = match r.u8()? {
                    value_tag::NUMBER => Value::Number(r.f64()?),
                    value_tag::BOOL => Value::Bool(r.u8()? != 0),
                    value_tag::CHOICE => Value::Choice(r.string()?),
                    other => return Err(r.error(format!("未知のパラメータ値の種別です: {other}"))),
                };
                overrides.insert(name, value);
            }

            Ok(GeomRecord::Instance {
                definition_index,
                placement,
                overrides,
            })
        }
        other => Err(r.error(format!("未知の図形種別です: {other}"))),
    }
}

/// 読み取った内容から、**コマンド経由でのみ** [`Document`] を組み立てる。
fn build_document(
    layers: Vec<LayerRecord>,
    groups: Vec<String>,
    definitions: Vec<DefinitionRecord>,
    entities: Vec<EntityRecord>,
) -> Result<Document> {
    let mut doc = Document::new();

    // ---- レイヤ ----------------------------------------------------------
    // ファイル内の添字 → 実際の `LayerId` の対応表。
    let mut layer_ids: Vec<LayerId> = Vec::with_capacity(layers.len());
    for rec in &layers {
        // レイヤ `"0"` は `Document::new` が既に持っているので、属性だけ当てる。
        let id = if rec.name == "0" {
            LayerId::ZERO
        } else {
            doc.apply(Box::new(AddLayer::new(rec.name.clone(), rec.color)))?;
            doc.layers()
                .by_name(&rec.name)
                .ok_or(CadError::LayerNotFound)?
        };
        doc.apply(Box::new(
            SetLayerProperties::new(id)
                .color(rec.color)
                .visible(rec.visible)
                .locked(rec.locked)
                .linetype(rec.linetype),
        ))?;
        layer_ids.push(id);
    }

    // ---- コンポーネント定義（2 パス） ------------------------------------
    //
    // 定義の中身は自分より後ろの定義を参照できる（前方参照）。
    // そのため **まず空の定義を全部作って ID を確定させ**、そのあと中身を入れる。
    // 1 パスでやると、まだ存在しない定義への参照を解決できない。
    let mut def_ids: Vec<DefinitionId> = Vec::with_capacity(definitions.len());
    for rec in &definitions {
        doc.apply(Box::new(DefineComponent::new(
            "YMC_LOAD",
            rec.name.clone(),
            rec.origin,
            Vec::new(),
        )))?;
        // `Document::apply` はコマンドを消費するので `created()` を読めない。
        // `AddLayer` と同じく名前で引く。
        let id = doc
            .definitions()
            .by_name(&rec.name)
            .ok_or(CadError::DefinitionNotFound)?;
        def_ids.push(id);
    }
    for (rec, id) in definitions.iter().zip(def_ids.iter()) {
        // **パラメータを先に入れる。** 束縛はパラメータを参照するので、
        // 順序を逆にすると `SetDefinitionContents` の検証に落ちる。
        if !rec.params.is_empty() {
            doc.apply(Box::new(SetDefinitionParams::new(
                "YMC_LOAD",
                *id,
                rec.params.clone(),
            )))?;
        }
        let contents = build_entities(&rec.entities, &layer_ids, &def_ids)?;
        doc.apply(Box::new(SetDefinitionContents::with_bindings(
            "YMC_LOAD",
            *id,
            rec.origin,
            contents,
            rec.bindings.clone(),
        )))?;
    }

    // ---- エンティティ ----------------------------------------------------
    let group_of: Vec<Option<usize>> = entities.iter().map(|e| e.group_index).collect();
    let built = build_entities(&entities, &layer_ids, &def_ids)?;

    if !built.is_empty() {
        doc.apply(Box::new(AddEntities::many("YMC_LOAD", built)))?;
    }

    // ---- グループ --------------------------------------------------------
    // `AddEntities` は作った ID を返さない（`transform.rs` の各コマンドは返す）。
    // まっさらな `Document` にファイル順で 1 コマンド挿入した直後なので、
    // `ids()` の並び（スロット昇順 = 挿入順）の i 番目がファイルの i 番目に対応する。
    let ids: Vec<EntityId> = doc.entities().ids().collect();
    for (index, name) in groups.iter().enumerate() {
        let members: Vec<EntityId> = group_of
            .iter()
            .enumerate()
            .filter(|(_, g)| **g == Some(index))
            .filter_map(|(i, _)| ids.get(i).copied())
            .collect();
        // メンバーのいないグループは `CreateGroup` が空の対象を拒否するので飛ばす。
        // 名前だけのグループを復元できないのは実害が無い（所属はエンティティ側が真実 /
        // ADR-0022 なので、メンバーが居ないグループは選択にも描画にも現れない）。
        if members.is_empty() {
            continue;
        }
        doc.apply(Box::new(CreateGroup::new(
            "YMC_LOAD",
            name.clone(),
            members,
        )))?;
    }

    // 開いた直後のファイルは Undo 履歴を持たず、未保存扱いにもしない。
    // `clear_history` を忘れると、読み込みに使ったコマンドが Undo できてしまう。
    doc.clear_history();
    doc.mark_saved(None);

    Ok(doc)
}

/// パラメータの宣言を読む。
fn read_param(r: &mut Reader<'_>) -> Result<ParamDecl> {
    let name = r.string()?;
    let ty = match r.u8()? {
        param_type::NUMBER => ParamType::Number,
        param_type::BOOL => ParamType::Bool,
        param_type::CHOICE => {
            let n = r.count()?;
            let mut options = Vec::with_capacity(n.min(r.remaining()));
            for _ in 0..n {
                options.push(r.string()?);
            }
            ParamType::Choice(options)
        }
        other => return Err(r.error(format!("未知のパラメータ型です: {other}"))),
    };
    let range = match r.u8()? {
        option_tag::NONE => None,
        option_tag::SOME => Some((r.f64()?, r.f64()?)),
        other => return Err(r.error(format!("未知の範囲指定です: {other}"))),
    };
    let default = read_expr(r, 0)?;
    Ok(ParamDecl {
        name,
        ty,
        default,
        range,
    })
}

/// 束縛のスロットを読む。
fn read_slot(r: &mut Reader<'_>) -> Result<Slot> {
    Ok(match r.u8()? {
        slot_tag::LINE_AX => Slot::LineAx,
        slot_tag::LINE_AY => Slot::LineAy,
        slot_tag::LINE_BX => Slot::LineBx,
        slot_tag::LINE_BY => Slot::LineBy,
        slot_tag::CIRCLE_CX => Slot::CircleCx,
        slot_tag::CIRCLE_CY => Slot::CircleCy,
        slot_tag::CIRCLE_R => Slot::CircleR,
        slot_tag::ARC_CX => Slot::ArcCx,
        slot_tag::ARC_CY => Slot::ArcCy,
        slot_tag::ARC_R => Slot::ArcR,
        slot_tag::ARC_START => Slot::ArcStart,
        slot_tag::ARC_END => Slot::ArcEnd,
        slot_tag::XLINE_OX => Slot::XlineOx,
        slot_tag::XLINE_OY => Slot::XlineOy,
        slot_tag::XLINE_ANGLE => Slot::XlineAngle,
        slot_tag::POLYLINE_VX => Slot::PolylineVx(r.u32()?),
        slot_tag::POLYLINE_VY => Slot::PolylineVy(r.u32()?),
        slot_tag::INSTANCE_X => Slot::InstanceX,
        slot_tag::INSTANCE_Y => Slot::InstanceY,
        slot_tag::INSTANCE_ROTATION => Slot::InstanceRotation,
        slot_tag::INSTANCE_SCALE => Slot::InstanceScale,
        other => return Err(r.error(format!("未知の束縛スロットです: {other}"))),
    })
}

/// パラメータの値を読む。
fn read_value(r: &mut Reader<'_>) -> Result<Value> {
    Ok(match r.u8()? {
        value_tag::NUMBER => Value::Number(r.f64()?),
        value_tag::BOOL => Value::Bool(r.u8()? != 0),
        value_tag::CHOICE => Value::Choice(r.string()?),
        other => return Err(r.error(format!("未知のパラメータ値の種別です: {other}"))),
    })
}

/// 式の深さの上限。
///
/// 前置記法なので、壊れたファイルは深い入れ子として読めてしまう。
/// **再帰でスタックを溢れさせないための柵。**
const MAX_EXPR_DEPTH: usize = 64;

/// 式を前置記法から読む。
fn read_expr(r: &mut Reader<'_>, depth: usize) -> Result<Expr> {
    if depth >= MAX_EXPR_DEPTH {
        return Err(r.error("式の入れ子が深すぎます".to_owned()));
    }
    let d = depth + 1;
    Ok(match r.u8()? {
        expr_tag::LITERAL => Expr::Literal(read_value(r)?),
        expr_tag::VAR => Expr::Var(r.string()?),
        expr_tag::UNARY => {
            let op = match r.u8()? {
                0 => UnOp::Neg,
                1 => UnOp::Not,
                other => return Err(r.error(format!("未知の単項演算子です: {other}"))),
            };
            Expr::Unary(op, Box::new(read_expr(r, d)?))
        }
        expr_tag::BINARY => {
            let op = read_binop(r)?;
            let a = read_expr(r, d)?;
            let b = read_expr(r, d)?;
            Expr::Binary(op, Box::new(a), Box::new(b))
        }
        expr_tag::IF => {
            let cond = read_expr(r, d)?;
            let then = read_expr(r, d)?;
            let otherwise = read_expr(r, d)?;
            Expr::If {
                cond: Box::new(cond),
                then: Box::new(then),
                otherwise: Box::new(otherwise),
            }
        }
        expr_tag::CALL1 => {
            let f = match r.u8()? {
                0 => Func1::Sin,
                1 => Func1::Cos,
                2 => Func1::Tan,
                3 => Func1::Sqrt,
                4 => Func1::Abs,
                5 => Func1::Floor,
                6 => Func1::Ceil,
                7 => Func1::Round,
                8 => Func1::Deg,
                9 => Func1::Rad,
                other => return Err(r.error(format!("未知の関数です: {other}"))),
            };
            Expr::Call1(f, Box::new(read_expr(r, d)?))
        }
        expr_tag::CALL2 => {
            let f = match r.u8()? {
                0 => Func2::Min,
                1 => Func2::Max,
                2 => Func2::Atan2,
                3 => Func2::Pow,
                other => return Err(r.error(format!("未知の関数です: {other}"))),
            };
            let a = read_expr(r, d)?;
            let b = read_expr(r, d)?;
            Expr::Call2(f, Box::new(a), Box::new(b))
        }
        other => return Err(r.error(format!("未知の式の種別です: {other}"))),
    })
}

fn read_binop(r: &mut Reader<'_>) -> Result<BinOp> {
    Ok(match r.u8()? {
        0 => BinOp::Add,
        1 => BinOp::Sub,
        2 => BinOp::Mul,
        3 => BinOp::Div,
        4 => BinOp::Lt,
        5 => BinOp::Le,
        6 => BinOp::Gt,
        7 => BinOp::Ge,
        8 => BinOp::Eq,
        9 => BinOp::Ne,
        10 => BinOp::And,
        11 => BinOp::Or,
        other => return Err(r.error(format!("未知の二項演算子です: {other}"))),
    })
}

/// 読み取ったレコード列を [`Entity`] へ直す。
///
/// 図面直下の要素と定義の中身で同じ処理を使う。
/// 添字はすべて `read_from_bytes` で範囲を検査済み。
fn build_entities(
    records: &[EntityRecord],
    layer_ids: &[LayerId],
    def_ids: &[DefinitionId],
) -> Result<Vec<Entity>> {
    records
        .iter()
        .map(|rec| {
            let layer = layer_ids
                .get(rec.layer_index)
                .copied()
                .unwrap_or(LayerId::ZERO);
            let geom = match &rec.geom {
                GeomRecord::Plain(g) => g.clone(),
                GeomRecord::Instance {
                    definition_index,
                    placement,
                    overrides,
                } => {
                    // 添字は `read_from_bytes` で範囲を検査済み。
                    // それでも黙って別の定義を指さないよう、ここでもエラーにする。
                    let definition = def_ids
                        .get(*definition_index)
                        .copied()
                        .ok_or(CadError::DefinitionNotFound)?;
                    Geometry::Instance(Instance {
                        definition,
                        placement: *placement,
                        overrides: overrides.clone(),
                    })
                }
            };
            let mut e = Entity::new(geom, layer);
            e.color = rec.color;
            Ok(e)
        })
        .collect()
}

/// バイト列を前から読み進める小さなヘルパ。
///
/// 位置の管理とエンディアンを 1 箇所に閉じ込め、
/// **範囲外アクセスを必ずエラーにする**ためにある。
struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    /// 残りバイト数。`Vec::with_capacity` の上限に使い、
    /// 壊れた件数で巨大な確保をしないようにする。
    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.pos)
    }

    /// 現在位置でのエラー。
    fn error(&self, message: String) -> CadError {
        self.error_at(self.pos, &message)
    }

    /// 位置を指定したエラー。
    fn error_at(&self, offset: usize, message: &str) -> CadError {
        CadError::Format {
            offset,
            message: message.to_owned(),
        }
    }

    /// `n` バイト取り出して進める。足りなければエラー。
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or_else(|| self.error_at(self.pos, "長さの指定が大きすぎます"))?;
        let slice = self.bytes.get(self.pos..end).ok_or_else(|| {
            self.error_at(
                self.pos,
                &format!(
                    "ファイルが途中で終わっています（{n} バイト必要、残り {} バイト）",
                    self.remaining()
                ),
            )
        })?;
        self.pos = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        // `take` が 4 バイトを保証しているので、変換は必ず成功する。
        let arr: [u8; 4] = b
            .try_into()
            .map_err(|_| self.error_at(self.pos, "内部エラー: 4 バイトの取り出しに失敗しました"))?;
        Ok(u32::from_le_bytes(arr))
    }

    /// 件数・添字を読む。
    fn count(&mut self) -> Result<usize> {
        Ok(self.u32()? as usize)
    }

    fn f64(&mut self) -> Result<f64> {
        let b = self.take(8)?;
        let arr: [u8; 8] = b
            .try_into()
            .map_err(|_| self.error_at(self.pos, "内部エラー: 8 バイトの取り出しに失敗しました"))?;
        Ok(f64::from_le_bytes(arr))
    }

    fn point(&mut self) -> Result<Point2> {
        Ok(Point2::new(self.f64()?, self.f64()?))
    }

    /// 「長さ + UTF-8」の文字列を読む。
    fn string(&mut self) -> Result<String> {
        let len = self.count()?;
        let start = self.pos;
        let bytes = self.take(len)?;
        String::from_utf8(bytes.to_vec())
            .map_err(|_| self.error_at(start, "文字列が UTF-8 として解釈できません"))
    }
}
