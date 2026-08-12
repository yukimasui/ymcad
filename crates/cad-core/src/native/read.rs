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

use crate::command::{AddEntities, AddLayer, CreateGroup, SetLayerProperties};
use crate::document::Document;
use crate::entity::{Entity, EntityId, Geometry};
use crate::error::{CadError, Result};
use crate::geom::tolerance::eq_len;
use crate::geom::{Arc, Circle, Line, Point2, Polyline, Vec2, Xline};
use crate::layer::{AciColor, ColorSpec, LayerId, LineType};

use super::{color_tag, kind, layer_flags, linetype, option_tag, FORMAT_VERSION, MAGIC};

/// 読み取ったレイヤ 1 件。
struct LayerRecord {
    name: String,
    color: AciColor,
    visible: bool,
    locked: bool,
    linetype: LineType,
}

/// 読み取ったエンティティ 1 件。レイヤ・グループはファイル内の添字のまま持つ。
struct EntityRecord {
    geom: Geometry,
    layer_index: usize,
    color: ColorSpec,
    group_index: Option<usize>,
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

    build_document(layers, groups, entities)
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

fn read_geometry(r: &mut Reader<'_>) -> Result<Geometry> {
    let tag = r.u8()?;
    match tag {
        kind::LINE => Ok(Geometry::Line(Line::new(r.point()?, r.point()?))),
        kind::CIRCLE => Ok(Geometry::Circle(Circle::new(r.point()?, r.f64()?))),
        kind::ARC => Ok(Geometry::Arc(Arc::new(
            r.point()?,
            r.f64()?,
            r.f64()?,
            r.f64()?,
        ))),
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
            Ok(Geometry::Xline(Xline { origin, direction }))
        }
        kind::POLYLINE => {
            let closed = r.u8()? != 0;
            let count = r.count()?;
            let mut vertices = Vec::with_capacity(count.min(r.remaining()));
            for _ in 0..count {
                vertices.push(r.point()?);
            }
            Ok(Geometry::Polyline(Polyline::new(vertices, closed)))
        }
        other => Err(r.error(format!("未知の図形種別です: {other}"))),
    }
}

/// 読み取った内容から、**コマンド経由でのみ** [`Document`] を組み立てる。
fn build_document(
    layers: Vec<LayerRecord>,
    groups: Vec<String>,
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

    // ---- エンティティ ----------------------------------------------------
    let group_of: Vec<Option<usize>> = entities.iter().map(|e| e.group_index).collect();
    let built: Vec<Entity> = entities
        .into_iter()
        .map(|rec| {
            // 添字は `read_entity` で範囲を検査済み。
            let layer = layer_ids
                .get(rec.layer_index)
                .copied()
                .unwrap_or(LayerId::ZERO);
            let mut e = Entity::new(rec.geom, layer);
            e.color = rec.color;
            e
        })
        .collect();

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
