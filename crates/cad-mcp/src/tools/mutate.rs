//! 図面を変える道具の共通部品。
//!
//! # 約束（段階 1b の道具すべて）
//!
//! - **1 回の呼び出しで `Document::apply` を 1 回**（コマンドが複数なら `MacroCommand` に束ねる）。
//!   成功すれば履歴が 1 つ増え、`undo` 1 回で呼び出し 1 回ぶんが戻る。失敗すれば履歴も版番号も変わらない
//! - **検査は適用の前にすべて済ませる**。作る・変える形は `Geometry::validate`（NaN・無限大・退化）と
//!   [`check_extent`](crate::convert::check_extent)（大きさの上限）に通す。変形は結果の形を
//!   `translated` / `rotated` / `scaled` / `mirrored` で試しに求めてから確かめる
//! - **非表示・ロック中のレイヤの図形を対象にしたら、呼び出しごと拒む**（一部だけ処理しない）。
//!   アプリでは選べない図形なので、LLM にだけ触らせない
//! - 新しい図形の ID は「適用前のスロット番号の最大値より大きいもの」で拾う。
//!   `EntityStore` はスロット番号を再利用しない（`entity/store.rs`）ので、適用で増えた図形だけが当たる

use cad_core::command::MacroCommand;
use cad_core::{Command, Document, EntityId, LayerId};

use crate::ids::{format_id, resolve_ids};
use crate::limits::MAX_DRAWING_ENTITIES;
use crate::server::Server;

/// コマンドを 1 回の `apply` で適用する。複数なら `name` の `MacroCommand` に束ねる。
///
/// # Errors
///
/// コマンドが空・適用に失敗した場合（図面は変わっていない）。
pub(super) fn apply(
    s: &mut Server,
    name: &'static str,
    mut commands: Vec<Box<dyn Command>>,
) -> Result<(), String> {
    let command: Box<dyn Command> = match commands.len() {
        0 => return Err("変更する項目がありません".to_owned()),
        1 => commands.pop().expect("1 個ある"),
        _ => Box::new(MacroCommand::new(name, commands)),
    };
    s.doc
        .apply(command)
        .map_err(|e| format!("図面に適用できませんでした（図面は変わっていません）: {e}"))
}

/// ID の文字列を、**編集できる**図形の ID にする（順序は入力どおり）。
///
/// [`resolve_ids`] の検査（形・起動・図面・存在・重複）に加えて、非表示・ロック中のレイヤの
/// 図形があれば全体を拒む。
///
/// # Errors
///
/// 使えない ID・編集できない図形の一覧を説明した文字列。
pub(super) fn editable_targets(s: &Server, ids: &[String]) -> Result<Vec<EntityId>, String> {
    let resolved = resolve_ids(&s.doc, s.tag(), ids)?;
    let mut problems = Vec::new();
    for (raw, id) in ids.iter().zip(&resolved) {
        let Some(e) = s.doc.entities().get(*id) else {
            continue;
        };
        if let Some(why) = layer_problem(&s.doc, e.layer) {
            problems.push(format!("ID {raw} は{why}の図形です"));
        }
    }
    if problems.is_empty() {
        Ok(resolved)
    } else {
        Err(format!(
            "{}\n非表示・ロック中のレイヤの図形は変えられません（呼び出し全体を取りやめました）。\
             変えるなら update_layer で表示する・ロックを外してから呼び直してください。",
            problems.join("\n")
        ))
    }
}

/// レイヤが編集できない理由（「レイヤ 壁（ロック中）」など）。編集できれば `None`。
pub(super) fn layer_problem(doc: &Document, layer: LayerId) -> Option<String> {
    let Some(l) = doc.layers().get(layer) else {
        return Some("存在しないレイヤ".to_owned());
    };
    match (l.visible, l.locked) {
        (true, false) => None,
        (false, false) => Some(format!("レイヤ {}（非表示）", l.name)),
        (true, true) => Some(format!("レイヤ {}（ロック中）", l.name)),
        (false, true) => Some(format!("レイヤ {}（非表示・ロック中）", l.name)),
    }
}

/// 図形を置く・移す先のレイヤ。名前が無ければ現在レイヤ。非表示・ロック中なら拒む。
///
/// # Errors
///
/// レイヤが無い・編集できない場合。
pub(super) fn destination_layer(s: &Server, name: Option<&str>) -> Result<LayerId, String> {
    let layers = s.doc.layers();
    let id = match name {
        None => layers.current(),
        Some(n) => layers.by_name(n).ok_or_else(|| {
            format!("レイヤ {n} はありません（list_layers で一覧を見られます。作るなら add_layer）")
        })?,
    };
    if let Some(why) = layer_problem(&s.doc, id) {
        return Err(format!(
            "{why}には図形を置けません。update_layer で表示する・ロックを外してから呼び直してください。"
        ));
    }
    Ok(id)
}

/// 図形を `adding` 個足しても上限（[`MAX_DRAWING_ENTITIES`]）に収まるか。
///
/// # Errors
///
/// 収まらない場合。
pub(super) fn ensure_capacity(doc: &Document, adding: usize) -> Result<(), String> {
    let now = doc.entities().len();
    if now.saturating_add(adding) > MAX_DRAWING_ENTITIES {
        return Err(format!(
            "図形が多すぎます（いま {now} 個に {adding} 個を足すと、上限 {MAX_DRAWING_ENTITIES} 個を超えます）"
        ));
    }
    Ok(())
}

/// いまの図面のスロット番号の最大値（図形が無ければ `None`）。適用の前に取っておく。
pub(super) fn max_slot(doc: &Document) -> Option<u32> {
    doc.entities().ids().map(EntityId::index).max()
}

/// `before`（[`max_slot`] の値）より後に作られた図形の ID。作られた順（スロット番号の順）。
pub(super) fn created_since(doc: &Document, before: Option<u32>) -> Vec<EntityId> {
    let mut ids: Vec<EntityId> = doc
        .entities()
        .ids()
        .filter(|id| before.is_none_or(|b| id.index() > b))
        .collect();
    ids.sort_by_key(|id| id.index());
    ids
}

/// ID の文字列の列。
pub(super) fn id_strings(s: &Server, ids: &[EntityId]) -> Vec<String> {
    let tag = s.tag();
    ids.iter().map(|id| format_id(tag, *id)).collect()
}
