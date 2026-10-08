//! 履歴の道具: `undo` / `redo`。
//!
//! `Document::undo` / `redo` をそのまま呼ぶ（設計原則 4。図面を変える経路を増やさない）。
//! MCP の道具が図面を変えるときは 1 回の呼び出しで `apply` 1 回にするので、
//! `steps: 1` が「直前の道具の呼び出し 1 回ぶん」に当たる（1b 以降の約束）。

use serde_json::json;

use super::{Args, Tool, ToolResult};
use crate::limits::MAX_HISTORY_STEPS;
use crate::server::Server;

pub(super) const UNDO: Tool = Tool {
    name: "undo",
    title: "取り消し",
    description: "直前の操作を steps 回取り消す（既定 1）。取り消せる操作が尽きたらそこで止まる（エラーにはしない）。\
取り消した操作の名前の一覧を返す。ファイルには何も書かない。図面を開き直した直後は履歴が空。",
    schema: || (steps_schema("取り消す回数"), &[]),
    read_only: false,
    destructive: false,
    idempotent: false,
    run: undo,
};

pub(super) const REDO: Tool = Tool {
    name: "redo",
    title: "やり直し",
    description: "取り消した操作を steps 回やり直す（既定 1）。やり直せる操作が尽きたらそこで止まる（エラーにはしない）。\
取り消した後に別の操作をすると、やり直しの履歴は消える。",
    schema: || (steps_schema("やり直す回数"), &[]),
    read_only: false,
    destructive: false,
    idempotent: false,
    run: redo,
};

fn steps_schema(what: &str) -> serde_json::Value {
    json!({
        "steps": {
            "type": "integer", "minimum": 1, "maximum": MAX_HISTORY_STEPS,
            "description": format!("{what}（既定 1）"),
        },
    })
}

#[derive(Clone, Copy)]
enum Direction {
    Undo,
    Redo,
}

fn undo(s: &mut Server, a: &Args) -> ToolResult {
    step(s, a, Direction::Undo)
}

fn redo(s: &mut Server, a: &Args) -> ToolResult {
    step(s, a, Direction::Redo)
}

fn step(s: &mut Server, a: &Args, dir: Direction) -> ToolResult {
    let steps = a.usize_in("steps", 1, 1, MAX_HISTORY_STEPS)?;
    let (verb, key) = match dir {
        Direction::Undo => ("取り消し", "undone"),
        Direction::Redo => ("やり直し", "redone"),
    };
    let mut names = Vec::new();
    for _ in 0..steps {
        let r = match dir {
            Direction::Undo => s.doc.undo(),
            Direction::Redo => s.doc.redo(),
        };
        match r {
            Ok(Some(name)) => names.push(name),
            Ok(None) => break,
            Err(e) => {
                return Err(format!(
                    "{} 回目の{verb}に失敗しました（それまでの {} 回は済んでいます: {}）: {e}",
                    names.len() + 1,
                    names.len(),
                    names.join(", ")
                ))
            }
        }
    }
    let history = s.doc.history();
    Ok(json!({
        "drawing": s.tag().name(),
        key: names,
        "count": names.len(),
        "can_undo": history.can_undo(),
        "can_redo": history.can_redo(),
        "dirty": s.doc.is_dirty(),
    }))
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{err, ok, server};
    use super::super::tests::cad_core_add_line;
    use super::*;
    use crate::test_util::TempDir;
    use cad_core::native::write::write_to_bytes;

    #[test]
    fn undo_and_redo_walk_the_history() {
        let dir = TempDir::new("history-walk");
        let mut s = server(&dir);
        let empty = write_to_bytes(&s.doc);
        cad_core_add_line(&mut s);
        let one = write_to_bytes(&s.doc);
        cad_core_add_line(&mut s);
        let two = write_to_bytes(&s.doc);

        let r = ok(&mut s, "undo", json!({}));
        assert_eq!(r["undone"], json!(["LINE"]));
        assert_eq!(write_to_bytes(&s.doc), one, "1 回ぶん戻る");

        let r = ok(&mut s, "undo", json!({"steps": 5}));
        assert_eq!(r["count"], 1, "尽きたら止まる");
        assert_eq!(r["can_undo"], false);
        assert_eq!(write_to_bytes(&s.doc), empty);

        let r = ok(&mut s, "redo", json!({"steps": 2}));
        assert_eq!(r["redone"], json!(["LINE", "LINE"]));
        assert_eq!(r["can_redo"], false);
        assert_eq!(write_to_bytes(&s.doc), two);

        let r = ok(&mut s, "redo", json!({}));
        assert_eq!(r["count"], 0);
    }

    #[test]
    fn steps_are_bounded() {
        let dir = TempDir::new("history-bounds");
        let mut s = server(&dir);
        err(&mut s, "undo", json!({"steps": 0}));
        err(&mut s, "redo", json!({"steps": MAX_HISTORY_STEPS + 1}));
        err(&mut s, "undo", json!({"steps": "1"}));
    }

    /// 開き直した図面の履歴は空（前の図面の操作を取り消さない）。
    #[test]
    fn history_does_not_cross_drawings() {
        let dir = TempDir::new("history-cross");
        let mut s = server(&dir);
        cad_core_add_line(&mut s);
        ok(&mut s, "save_drawing", json!({"path": "a.ymc"}));
        ok(&mut s, "open_drawing", json!({"path": "a.ymc"}));
        let r = ok(&mut s, "undo", json!({}));
        assert_eq!(r["count"], 0);
        assert_eq!(s.doc.entities().len(), 1);
    }
}
