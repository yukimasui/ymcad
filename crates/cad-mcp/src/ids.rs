//! 図形 ID の文字列表現 `d<起動の印>-<図面の通し番号>e<index>g<generation>`（例 `d3fa9c1-2e5g0`）。
//!
//! # なぜ図面の通し番号を入れるのか
//!
//! `EntityId` は図面の中でしか意味を持たない。図面を開き直すと ID は振り直され
//! （`.ymc` は ID を保存しない）、**前の図面の ID が新しい図面の別の図形と一致しうる**
//! （`docs/PROGRESS.md` の「既知の落とし穴」: `EntityStore` の ID 再利用）。
//! LLM は前の応答の ID を覚えていて使い回すので、図面を入れ替えるたびに増える通し番号を
//! ID に入れ、**別の図面の ID は呼び出しごと拒む**。
//!
//! # なぜ起動の印を入れるのか
//!
//! 通し番号はプロセスごとに 1 から始まる。MCP クライアントがサーバーをつなぎ直すと
//! （再接続・異常終了からの再起動）、LLM の文脈には前のプロセスの ID が残っていて、
//! 新しいプロセスの同じ番号の図面の別の図形と一致しうる。そこで起動ごとに違う値
//! （[`new_session`]: 時刻・PID・標準ライブラリの乱数の種から作る 24 ビット）を ID に入れる。
//! 乱数の依存は足さない。
//!
//! `EntityId::new` は `cad-core` の外から呼べない（`pub(crate)`）ので、文字列から ID を
//! 組み立てるときは図面の ID を走査して `index()` / `generation()` で照合する。

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, Hasher};

use cad_core::{Document, EntityId};

/// 起動の印のビット数（16 進 6 桁）。
const SESSION_MASK: u32 = 0x00ff_ffff;

/// 図面を見分ける印。起動の印と、その起動の中での図面の通し番号。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DrawingTag {
    /// 起動の印（24 ビット）。
    pub session: u32,
    /// 図面の通し番号。新規・開くたびに増える。
    pub serial: u64,
}

impl DrawingTag {
    /// 図面の名前（ID の頭の部分）。`d3fa9c1-2` など。
    #[must_use]
    pub fn name(self) -> String {
        format!("d{:06x}-{}", self.session & SESSION_MASK, self.serial)
    }
}

/// 起動ごとに違う印を作る。
///
/// 時刻（ナノ秒）と PID を、標準ライブラリのハッシュ（`RandomState` は種をプロセスごとに
/// OS の乱数から取る）で混ぜる。暗号的な強さは要らない。つなぎ直した前後で一致しなければよい
/// （一致する確率は 2^-24）。
#[must_use]
pub fn new_session() -> u32 {
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    h.write_u128(nanos);
    h.write_u32(std::process::id());
    // 上位と下位を畳んでから 24 ビットに切る（切り捨ての `as` を使わない）。
    let v = h.finish();
    let folded = (v >> 32) ^ (v & 0xffff_ffff);
    u32::try_from(folded & u64::from(SESSION_MASK)).unwrap_or(0)
}

/// ID を文字列にする。
#[must_use]
pub fn format_id(tag: DrawingTag, id: EntityId) -> String {
    format!("{}e{}g{}", tag.name(), id.index(), id.generation())
}

/// 文字列を `(図面の印, index, generation)` に分ける。
///
/// 起動の印は小文字の 16 進 1〜8 桁、ほかの数字は 10 進の ASCII だけ（符号・空白・空を受け付けない）。
/// 16 進には `e` が含まれうるので、`g` と `e` は**後ろから**切る（index と generation は数字だけ）。
///
/// # Errors
///
/// 形が違う場合。
pub fn parse_id(s: &str) -> Result<(DrawingTag, u32, u32), String> {
    let malformed = || {
        format!(
            "ID {s:?} の形が違います（d<起動の印>-<図面>e<番号>g<世代>、例: d3fa9c1-2e5g0。\
             list_entities などが返した ID をそのまま使ってください）"
        )
    };
    let rest = s.strip_prefix('d').ok_or_else(malformed)?;
    let (rest, generation) = rest.rsplit_once('g').ok_or_else(malformed)?;
    let (drawing, index) = rest.rsplit_once('e').ok_or_else(malformed)?;
    let (session, serial) = drawing.split_once('-').ok_or_else(malformed)?;
    let digits = |t: &str| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit());
    let hex = |t: &str| {
        (1..=8).contains(&t.len())
            && t.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    };
    if !(hex(session) && digits(serial) && digits(index) && digits(generation)) {
        return Err(malformed());
    }
    let tag = DrawingTag {
        session: u32::from_str_radix(session, 16).map_err(|_| malformed())?,
        serial: serial.parse().map_err(|_| malformed())?,
    };
    Ok((
        tag,
        index.parse().map_err(|_| malformed())?,
        generation.parse().map_err(|_| malformed())?,
    ))
}

/// ID の文字列の列を、いまの図面の `EntityId` の列にする（順序は入力どおり）。
///
/// **1 つでも使えない ID があれば全体を拒む**（形が違う・別の起動・別の図面・存在しない・重複）。
/// 一部だけ処理すると、LLM が結果の欠けに気づかない。同じ ID を 2 回書くと、移動が 2 回効く
/// などの事故になるので拒む。
///
/// # Errors
///
/// 使えない ID の一覧を説明した文字列。
pub fn resolve_ids(
    doc: &Document,
    tag: DrawingTag,
    ids: &[String],
) -> Result<Vec<EntityId>, String> {
    let by_slot: HashMap<(u32, u32), EntityId> = doc
        .entities()
        .ids()
        .map(|id| ((id.index(), id.generation()), id))
        .collect();

    let mut out = Vec::with_capacity(ids.len());
    let mut seen = HashSet::new();
    let mut problems = Vec::new();
    for s in ids {
        match parse_id(s) {
            Err(e) => problems.push(e),
            Ok((t, _, _)) if t.session != tag.session => problems.push(format!(
                "ID {s} はこのサーバーの起動より前（つなぎ直す前）のものです。いまの図面は {} です\
                 （list_entities で ID を取り直してください）",
                tag.name()
            )),
            Ok((t, _, _)) if t.serial != tag.serial => problems.push(format!(
                "ID {s} は別の図面（{}）のものです。いまの図面は {} です（図面を開き直すと ID は変わります）",
                t.name(),
                tag.name()
            )),
            Ok((_, index, generation)) => match by_slot.get(&(index, generation)) {
                Some(id) if !seen.insert(*id) => {
                    problems.push(format!("ID {s} が 2 回指定されています"));
                }
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

    /// テストの図面の印。起動の印に `e` を含めて、後ろから切る解析を試す。
    const TAG: DrawingTag = DrawingTag {
        session: 0x00e1_e2e3,
        serial: 7,
    };

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
            let s = format_id(TAG, id);
            assert_eq!(parse_id(&s).unwrap(), (TAG, id.index(), id.generation()));
        }
        assert_eq!(
            format_id(TAG, doc.entities().ids().nth(2).unwrap()),
            "de1e2e3-7e2g0"
        );
        let small = DrawingTag {
            session: 0xa,
            serial: 1,
        };
        assert_eq!(small.name(), "d00000a-1", "起動の印は 6 桁に揃える");
        assert_eq!(parse_id("d00000a-1e0g0").unwrap().0, small);
    }

    #[test]
    fn malformed_ids_are_rejected() {
        for bad in [
            "",
            "d",
            "d1",
            "d1e0g0",
            "da-1e",
            "da-1e0",
            "da-1e0g",
            "a-1e0g0",
            "da-1e0g0x",
            "d-1e0g0",
            "dA-1e0g0",
            "dx-1e0g0",
            "da--1e0g0",
            "da-+1e0g0",
            "da-1e 0g0",
            "D1-1e0g0",
            "d123456789-1e0g0",
            "da-1e99999999999g0",
        ] {
            assert!(parse_id(bad).is_err(), "{bad:?} を拒むこと");
        }
    }

    #[test]
    fn sessions_differ_between_calls() {
        // 時刻と乱数の種が違うので、続けて作っても（ほぼ確実に）違う。24 ビットに収まる。
        let a: Vec<u32> = (0..8).map(|_| new_session()).collect();
        assert!(a.iter().all(|s| *s <= SESSION_MASK));
        assert!(a.windows(2).any(|w| w[0] != w[1]), "{a:?}");
    }

    #[test]
    fn resolve_keeps_order() {
        let doc = doc_with_lines(3);
        let all: Vec<_> = doc.entities().ids().collect();
        let got =
            resolve_ids(&doc, TAG, &[format_id(TAG, all[2]), format_id(TAG, all[0])]).unwrap();
        assert_eq!(got, vec![all[2], all[0]]);
    }

    /// 別の図面の ID は、同じ番号の図形があっても拒む。
    #[test]
    fn ids_of_another_drawing_are_rejected() {
        let doc = doc_with_lines(1);
        let id = doc.entities().ids().next().unwrap();
        let older = DrawingTag {
            serial: TAG.serial - 1,
            ..TAG
        };
        let err = resolve_ids(&doc, TAG, &[format_id(older, id)]).unwrap_err();
        assert!(err.contains("別の図面"), "{err}");
    }

    /// 別の起動（つなぎ直す前のプロセス）の ID は、図面の番号が同じでも拒む。
    #[test]
    fn ids_of_another_session_are_rejected() {
        let doc = doc_with_lines(1);
        let id = doc.entities().ids().next().unwrap();
        let before = DrawingTag {
            session: TAG.session ^ 1,
            ..TAG
        };
        let err = resolve_ids(&doc, TAG, &[format_id(before, id)]).unwrap_err();
        assert!(err.contains("つなぎ直す前"), "{err}");
    }

    /// 1 つでも使えない ID があれば全体を拒む。同じ ID の 2 回目も拒む。
    #[test]
    fn one_bad_id_rejects_the_whole_list() {
        let doc = doc_with_lines(2);
        let first = format_id(TAG, doc.entities().ids().next().unwrap());
        let missing = format!("{}e5g0", TAG.name());
        let err = resolve_ids(&doc, TAG, &[first.clone(), missing.clone()]).unwrap_err();
        assert!(err.contains(&missing), "{err}");
        let other_gen = format!("{}e0g1", TAG.name());
        assert!(resolve_ids(&doc, TAG, &[other_gen]).is_err(), "世代違い");
        let err = resolve_ids(&doc, TAG, &[first.clone(), first]).unwrap_err();
        assert!(err.contains("2 回"), "{err}");
    }
}
