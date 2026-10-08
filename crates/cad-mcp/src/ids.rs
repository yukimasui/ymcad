//! 図形 ID の文字列表現 `d<図面の通し番号>e<index>g<generation>`。
//!
//! # なぜ図面の通し番号を入れるのか
//!
//! `EntityId` は図面の中でしか意味を持たない。図面を開き直すと ID は振り直され
//! （`.ymc` は ID を保存しない）、**前の図面の ID が新しい図面の別の図形と一致しうる**
//! （`docs/PROGRESS.md` の「既知の落とし穴」: `EntityStore` の ID 再利用）。
//! LLM は前の応答の ID を覚えていて使い回すので、図面を入れ替えるたびに増える通し番号を
//! ID に入れ、**別の図面の ID は呼び出しごと拒む**。
//!
//! `EntityId::new` は `cad-core` の外から呼べない（`pub(crate)`）ので、文字列から ID を
//! 組み立てるときは図面の ID を走査して `index()` / `generation()` で照合する。

use std::collections::HashMap;

use cad_core::{Document, EntityId};

/// ID を文字列にする。
#[must_use]
pub fn format_id(serial: u64, id: EntityId) -> String {
    format!("d{serial}e{}g{}", id.index(), id.generation())
}

/// 図面の名前（ID の頭の部分）。`d3` など。
#[must_use]
pub fn drawing_name(serial: u64) -> String {
    format!("d{serial}")
}

/// 文字列を `(図面の通し番号, index, generation)` に分ける。
///
/// 数字は 10 進の ASCII だけ（符号・空白・空を受け付けない）。
///
/// # Errors
///
/// 形が違う場合。
pub fn parse_id(s: &str) -> Result<(u64, u32, u32), String> {
    let malformed = || format!("ID {s:?} の形が違います（d<図面>e<番号>g<世代>、例: d1e0g0）");
    let rest = s.strip_prefix('d').ok_or_else(malformed)?;
    let (serial, rest) = rest.split_once('e').ok_or_else(malformed)?;
    let (index, generation) = rest.split_once('g').ok_or_else(malformed)?;
    let digits = |t: &str| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit());
    if !(digits(serial) && digits(index) && digits(generation)) {
        return Err(malformed());
    }
    Ok((
        serial.parse().map_err(|_| malformed())?,
        index.parse().map_err(|_| malformed())?,
        generation.parse().map_err(|_| malformed())?,
    ))
}

/// ID の文字列の列を、いまの図面の `EntityId` の列にする（順序は入力どおり）。
///
/// **1 つでも使えない ID があれば全体を拒む**（形が違う・別の図面・存在しない）。
/// 一部だけ処理すると、LLM が結果の欠けに気づかない。
///
/// # Errors
///
/// 使えない ID の一覧を説明した文字列。
pub fn resolve_ids(doc: &Document, serial: u64, ids: &[String]) -> Result<Vec<EntityId>, String> {
    let by_slot: HashMap<(u32, u32), EntityId> = doc
        .entities()
        .ids()
        .map(|id| ((id.index(), id.generation()), id))
        .collect();

    let mut out = Vec::with_capacity(ids.len());
    let mut problems = Vec::new();
    for s in ids {
        match parse_id(s) {
            Err(e) => problems.push(e),
            Ok((d, _, _)) if d != serial => problems.push(format!(
                "ID {s} は別の図面（{}）のものです。いまの図面は {} です（図面を開き直すと ID は変わります）",
                drawing_name(d),
                drawing_name(serial)
            )),
            Ok((_, index, generation)) => match by_slot.get(&(index, generation)) {
                Some(id) => out.push(*id),
                None => problems.push(format!("ID {s} の図形はありません（削除済みかもしれません）")),
            },
        }
    }
    if problems.is_empty() {
        Ok(out)
    } else {
        Err(problems.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cad_core::command::AddEntities;
    use cad_core::geom::{Line, Point2};
    use cad_core::{Entity, Geometry, LayerId};

    fn doc_with_lines(n: u32) -> Document {
        let mut doc = Document::new();
        for i in 0..n {
            let x = f64::from(i);
            doc.apply(Box::new(AddEntities::one(
                "LINE",
                Entity::new(
                    Geometry::Line(Line::new(Point2::new(x, 0.0), Point2::new(x, 1.0))),
                    LayerId::ZERO,
                ),
            )))
            .unwrap();
        }
        doc
    }

    #[test]
    fn format_and_parse_round_trip() {
        let doc = doc_with_lines(3);
        for id in doc.entities().ids() {
            let s = format_id(7, id);
            assert_eq!(parse_id(&s).unwrap(), (7, id.index(), id.generation()));
        }
        assert_eq!(format_id(7, doc.entities().ids().nth(2).unwrap()), "d7e2g0");
    }

    #[test]
    fn malformed_ids_are_rejected() {
        for bad in [
            "",
            "d",
            "d1",
            "d1e",
            "d1e0",
            "d1e0g",
            "1e0g0",
            "d1e0g0x",
            "d-1e0g0",
            "d1e+0g0",
            "d1e 0g0",
            "D1e0g0",
            "d1e99999999999g0",
        ] {
            assert!(parse_id(bad).is_err(), "{bad:?} を拒むこと");
        }
    }

    #[test]
    fn resolve_keeps_order() {
        let doc = doc_with_lines(3);
        let all: Vec<_> = doc.entities().ids().collect();
        let got = resolve_ids(&doc, 1, &["d1e2g0".into(), "d1e0g0".into()]).unwrap();
        assert_eq!(got, vec![all[2], all[0]]);
    }

    /// 別の図面の ID は、同じ番号の図形があっても拒む。
    #[test]
    fn ids_of_another_drawing_are_rejected() {
        let doc = doc_with_lines(1);
        let err = resolve_ids(&doc, 2, &["d1e0g0".into()]).unwrap_err();
        assert!(err.contains("別の図面"), "{err}");
    }

    /// 1 つでも使えない ID があれば全体を拒む。
    #[test]
    fn one_bad_id_rejects_the_whole_list() {
        let doc = doc_with_lines(2);
        let err = resolve_ids(&doc, 1, &["d1e0g0".into(), "d1e5g0".into()]).unwrap_err();
        assert!(err.contains("d1e5g0"), "{err}");
        assert!(
            resolve_ids(&doc, 1, &["d1e0g1".into()]).is_err(),
            "世代違い"
        );
    }
}
