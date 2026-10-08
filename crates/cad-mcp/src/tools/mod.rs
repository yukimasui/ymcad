//! 道具（MCP の tools）の一覧と呼び出し。
//!
//! # 約束
//!
//! - 道具は [`TOOLS`] の表に 1 行ずつ。`tools/list` も `tools/call` もこの表だけを見る
//! - 引数の形の誤り・図面の状態による失敗は **`isError: true` と日本語の説明**で返す
//!   （LLM が読んで直せるように。JSON-RPC のエラーは「知らない道具」「引数がオブジェクトでない」だけ）
//! - **知らない引数は動かす前に拒む**（`discard_change` のような綴り違いを黙って無視しない）
//! - 成功の結果は `structuredContent`（オブジェクト）と、同じ JSON の text の両方で返す
//! - 道具の処理は `catch_unwind` で包む。panic してもサーバーは止まらず、`isError` を返す
//! - 道具の説明は日本語、フィールド名は英語

mod args;
mod file;
mod history;
mod query;

use std::panic::{catch_unwind, AssertUnwindSafe};

use serde_json::{json, Map, Value};

use crate::protocol::{error_code, RpcError};
use crate::server::Server;
use args::Args;

/// 道具の結果。成功は `structuredContent` にするオブジェクト、失敗は日本語の説明。
type ToolResult = Result<Value, String>;

/// 道具 1 つ。
struct Tool {
    name: &'static str,
    title: &'static str,
    description: &'static str,
    /// `properties` と `required`（[`object_schema`] で包む前）。
    schema: fn() -> (Value, &'static [&'static str]),
    /// 図面もファイルも変えない。
    read_only: bool,
    /// 取り消せない変更をしうる（未保存の変更を捨てる・ファイルを上書きする）。
    destructive: bool,
    /// 同じ引数で繰り返しても結果が変わらない。
    idempotent: bool,
    run: fn(&mut Server, &Args) -> ToolResult,
}

/// 道具の表。
const TOOLS: &[Tool] = &[
    file::NEW_DRAWING,
    file::OPEN_DRAWING,
    file::SAVE_DRAWING,
    file::DRAWING_INFO,
    query::LIST_ENTITIES,
    query::GET_ENTITIES,
    query::LIST_LAYERS,
    query::LIST_COMPONENTS,
    history::UNDO,
    history::REDO,
];

/// 引数の JSON Schema（object）を組み立てる。**知らない引数は受け付けない**。
fn object_schema(properties: Value, required: &[&str]) -> Value {
    let mut schema = json!({
        "type": "object",
        "properties": properties,
        "additionalProperties": false,
    });
    if !required.is_empty() {
        schema["required"] = json!(required);
    }
    schema
}

/// `tools/list` の中身。
pub(crate) fn definitions() -> Vec<Value> {
    TOOLS
        .iter()
        .map(|t| {
            let (props, required) = (t.schema)();
            json!({
                "name": t.name,
                "title": t.title,
                "description": t.description,
                "inputSchema": object_schema(props, required),
                "annotations": {
                    "title": t.title,
                    "readOnlyHint": t.read_only,
                    "destructiveHint": t.destructive,
                    "idempotentHint": t.idempotent,
                    "openWorldHint": false,
                },
            })
        })
        .collect()
}

/// `tools/call`。
pub(crate) fn call(server: &mut Server, params: &Map<String, Value>) -> Result<Value, RpcError> {
    let name = params.get("name").and_then(Value::as_str).ok_or_else(|| {
        (
            error_code::INVALID_PARAMS,
            "name（道具の名前）がありません".to_owned(),
        )
    })?;
    let tool = TOOLS.iter().find(|t| t.name == name).ok_or_else(|| {
        (
            error_code::INVALID_PARAMS,
            format!("Unknown tool: {name}（知らない道具です。tools/list で一覧を見てください）"),
        )
    })?;
    let arguments = match params.get("arguments") {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(m)) => m.clone(),
        Some(_) => {
            return Err((
                error_code::INVALID_PARAMS,
                "arguments はオブジェクトでなければなりません".to_owned(),
            ))
        }
    };

    Ok(run_tool(server, tool, arguments))
}

/// 道具を 1 つ動かし、`tools/call` の結果（`content` / `structuredContent` / `isError`）を作る。
fn run_tool(server: &mut Server, tool: &Tool, arguments: Map<String, Value>) -> Value {
    let (props, _) = (tool.schema)();
    let known: Vec<&str> = props
        .as_object()
        .map(|m| m.keys().map(String::as_str).collect())
        .unwrap_or_default();
    let outcome = match Args::new(arguments, &known) {
        Err(msg) => Err(msg),
        Ok(args) => catch_unwind(AssertUnwindSafe(|| (tool.run)(server, &args)))
            .unwrap_or_else(|payload| {
                let detail = payload
                    .downcast_ref::<&str>()
                    .map(|s| (*s).to_owned())
                    .or_else(|| payload.downcast_ref::<String>().cloned())
                    .unwrap_or_default();
                Err(format!(
                    "内部エラーが起きました（{}）。drawing_info で図面の状態を確かめてください。{detail}",
                    tool.name
                ))
            }),
    };

    match outcome {
        Ok(structured) => json!({
            "content": [{ "type": "text", "text": structured.to_string() }],
            "structuredContent": structured,
            "isError": false,
        }),
        Err(message) => json!({
            "content": [{ "type": "text", "text": message }],
            "isError": true,
        }),
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    //! 道具のテストの足場。`initialize` 済みのサーバーに道具を呼ぶ。

    use serde_json::{json, Value};

    use crate::server::Server;
    use crate::test_util::TempDir;

    /// `initialize` 済みのサーバー。
    pub fn server(dir: &TempDir) -> Server {
        let mut s = Server::new(&[dir.path().to_path_buf()]).unwrap();
        s.handle(json!({"jsonrpc": "2.0", "id": 0, "method": "initialize",
            "params": {"protocolVersion": "2025-11-25", "capabilities": {}}}))
            .unwrap();
        s
    }

    /// 道具を呼び、`result` を返す（JSON-RPC のエラーなら panic）。
    pub fn call_raw(s: &mut Server, name: &str, args: Value) -> Value {
        let r = s
            .handle(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": {"name": name, "arguments": args}}))
            .unwrap();
        assert!(r.get("error").is_none(), "JSON-RPC のエラー: {r}");
        r["result"].clone()
    }

    /// 成功するはずの呼び出し。`structuredContent` を返す。
    pub fn ok(s: &mut Server, name: &str, args: Value) -> Value {
        let r = call_raw(s, name, args.clone());
        assert_eq!(r["isError"], false, "{name} {args} が失敗: {r}");
        let text: Value = serde_json::from_str(r["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(
            text, r["structuredContent"],
            "text と structuredContent が同じ JSON"
        );
        r["structuredContent"].clone()
    }

    /// 失敗するはずの呼び出し。説明の文を返す。
    pub fn err(s: &mut Server, name: &str, args: Value) -> String {
        let r = call_raw(s, name, args.clone());
        assert_eq!(r["isError"], true, "{name} {args} が成功してしまった: {r}");
        assert!(r.get("structuredContent").is_none());
        r["content"][0]["text"].as_str().unwrap().to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{call_raw, err, server};
    use super::*;
    use crate::test_util::TempDir;

    #[test]
    fn unknown_arguments_are_rejected_before_running() {
        let dir = TempDir::new("tools-unknown-arg");
        let mut s = server(&dir);
        cad_core_add_line(&mut s);
        let msg = err(&mut s, "new_drawing", json!({"discard_change": true}));
        assert!(msg.contains("discard_change"), "{msg}");
        assert!(s.doc.is_dirty(), "図面は捨てられていない");
        assert_eq!(s.serial, 1);
    }

    /// 道具の中の panic はサーバーを止めず、`isError` になる。
    #[test]
    fn panics_in_tools_become_tool_errors() {
        let dir = TempDir::new("tools-panic");
        let mut s = server(&dir);
        let tool = Tool {
            name: "boom",
            title: "",
            description: "",
            schema: || (json!({}), &[]),
            read_only: true,
            destructive: false,
            idempotent: true,
            run: |_, _| panic!("わざと"),
        };
        // 既定のフックは stderr に panic の文を出すだけなので、テストの出力を汚さないよう黙らせる。
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let r = run_tool(&mut s, &tool, Map::new());
        std::panic::set_hook(hook);
        assert_eq!(r["isError"], true, "{r}");
        let text = r["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("内部エラー") && text.contains("わざと"),
            "{text}"
        );
        // サーバーは続けて使える。
        let r = call_raw(&mut s, "drawing_info", json!({}));
        assert_eq!(r["isError"], false);
    }

    pub(super) fn cad_core_add_line(s: &mut Server) {
        use cad_core::command::AddEntities;
        use cad_core::geom::{Line, Point2};
        use cad_core::{Entity, Geometry, LayerId};
        s.doc
            .apply(Box::new(AddEntities::one(
                "LINE",
                Entity::new(
                    Geometry::Line(Line::new(Point2::new(0.0, 0.0), Point2::new(1.0, 1.0))),
                    LayerId::ZERO,
                ),
            )))
            .unwrap();
    }
}
