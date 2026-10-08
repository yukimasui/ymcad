//! 道具（MCP の tools）の一覧と呼び出し。
//!
//! # 約束
//!
//! - 道具は [`TOOLS`] の表に 1 行ずつ。`tools/list` も `tools/call` もこの表だけを見る
//! - 引数の形の誤り・図面の状態による失敗は **`isError: true` と日本語の説明**で返す
//!   （LLM が読んで直せるように。JSON-RPC のエラーは「知らない道具」「引数がオブジェクトでない」だけ）
//! - **知らない引数は動かす前に拒む**（`discard_change` のような綴り違いを黙って無視しない）
//! - 成功の結果は `structuredContent`（オブジェクト）と、同じ JSON の text の両方で返す。
//!   大きさに上限（[`MAX_RESULT_BYTES`]）を置く（[`bound_result`]）
//! - 道具の処理は `catch_unwind` で包む。panic してもサーバーは止まらず、`isError` を返す。
//!   図面を変える道具（`read_only: false`）が panic したら図面に「壊れた」印を付け、以後の保存を拒む
//!   （`Server::poisoned`。新規・開くで外す）
//! - 道具の説明は日本語、フィールド名は英語

mod args;
mod components;
mod draw;
mod file;
mod history;
mod layers;
mod mutate;
mod query;
mod render;
mod transform;

use std::panic::{catch_unwind, AssertUnwindSafe};

use serde_json::{json, Map, Value};

use crate::limits::MAX_RESULT_BYTES;
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
    // 段階 1b: 作図・変更・変形・レイヤ（図面を変える。1 回の呼び出しが undo 1 回ぶん）
    draw::ADD_ENTITIES,
    draw::MODIFY_ENTITIES,
    draw::DELETE_ENTITIES,
    transform::MOVE_ENTITIES,
    transform::ROTATE_ENTITIES,
    transform::SCALE_ENTITIES,
    transform::MIRROR_ENTITIES,
    transform::SET_ENTITY_LAYER,
    layers::ADD_LAYER,
    layers::UPDATE_LAYER,
    layers::DELETE_LAYER,
    // 段階 1d: コンポーネント（図面を変える。1 回の呼び出しが undo 1 回ぶん）
    components::DEFINE_COMPONENT,
    components::SET_COMPONENT_PARAMS,
    components::BIND,
    components::INSERT_COMPONENT,
    components::SET_INSTANCE_PARAMS,
    history::UNDO,
    history::REDO,
    render::RENDER,
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
    server.attachments.clear();
    let outcome = match Args::new(arguments, &known) {
        Err(msg) => Err(msg),
        Ok(args) => catch_unwind(AssertUnwindSafe(|| (tool.run)(server, &args)))
            .unwrap_or_else(|payload| {
                let detail = payload
                    .downcast_ref::<&str>()
                    .map(|s| (*s).to_owned())
                    .or_else(|| payload.downcast_ref::<String>().cloned())
                    .unwrap_or_default();
                if tool.read_only {
                    return Err(format!(
                        "内部エラーが起きました（{}）。図面は変わっていません。{detail}",
                        tool.name
                    ));
                }
                // 図面を変える途中で止まったかもしれない。書きかけの図面を保存させない。
                server.poisoned = Some(tool.name);
                Err(format!(
                    "内部エラーが起きました（{}）。図面が書きかけの状態かもしれないので、この図面の保存は\
                     できなくなりました。open_drawing で開き直すか new_drawing で新規にしてください。{detail}",
                    tool.name
                ))
            }),
    };

    match outcome.and_then(|structured| bound_result(tool, structured)) {
        Ok((structured, text)) => {
            // 先頭は structuredContent と同じ JSON の text。画像などの追加のブロックはその後ろ。
            let mut content = vec![json!({ "type": "text", "text": text })];
            content.append(&mut server.attachments);
            json!({
                "content": content,
                "structuredContent": structured,
                "isError": false,
            })
        }
        Err(message) => {
            // 失敗した道具が途中まで積んだ画像などは返さない（次の呼び出しへも持ち越さない）。
            server.attachments.clear();
            json!({
                "content": [{ "type": "text", "text": message }],
                "isError": true,
            })
        }
    }
}

/// 結果の大きさを上限（[`MAX_RESULT_BYTES`]）に収める。結果と、その JSON の文字列を返す。
///
/// 読むだけの道具なら `isError` にして絞り込みを促す（何も変わっていないので呼び直せばよい）。
/// **図面を変える道具は失敗にしない**（変更はもう済んでいるので、失敗と返すと LLM が同じ変更を
/// やり直しかねない）。結果を省いた印だけを返す。
fn bound_result(tool: &Tool, structured: Value) -> Result<(Value, String), String> {
    let text = structured.to_string();
    if text.len() <= MAX_RESULT_BYTES {
        return Ok((structured, text));
    }
    if tool.read_only {
        return Err(format!(
            "結果が大きすぎます（{} バイト、上限 {MAX_RESULT_BYTES} バイト）。\
             limit を小さくする・ID の数を減らす・layer / type / bbox で絞り込むなどして呼び直してください。",
            text.len()
        ));
    }
    let short = json!({
        "result_omitted": true,
        "result_bytes": text.len(),
        "note": "図面への変更は済んでいます。結果が大きすぎるので省きました。drawing_info・list_entities で確かめてください",
    });
    let text = short.to_string();
    Ok((short, text))
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

    /// いまの図面の、世代 0 の図形 ID（テストで ID を書くため）。
    pub fn eid(s: &Server, index: u32) -> String {
        format!("{}e{index}g0", s.tag().name())
    }

    /// 図面を変えるはずの呼び出し。段階 1b の約束を確かめて `structuredContent` を返す:
    ///
    /// - 成功し、履歴がちょうど 1 つ増える（`MacroCommand` で束ねていること）
    /// - `undo` 1 回で、適用前と `.ymc` のバイト列まで一致する。`redo` で適用後に戻る
    ///
    /// **作った図形の ID は `redo` で振り直される**（`AddEntities` は適用のたびに新しいスロットに入れる）。
    /// 返り値の新しい ID を後で使うテストは [`ok`] を使う。
    pub fn mutate(s: &mut Server, name: &str, args: Value) -> Value {
        use cad_core::native::write::write_to_bytes;
        let before = write_to_bytes(&s.doc);
        let history = s.doc.history().len();
        let revision = s.doc.revision();
        let r = ok(s, name, args.clone());
        assert_eq!(
            s.doc.history().len(),
            history + 1,
            "{name} {args}: 履歴がちょうど 1 つ増える"
        );
        assert!(s.doc.revision() > revision);
        assert!(s.doc.is_dirty());
        let after = write_to_bytes(&s.doc);
        s.doc.undo().unwrap();
        assert_eq!(
            write_to_bytes(&s.doc),
            before,
            "{name} {args}: undo 1 回で適用前に戻る"
        );
        s.doc.redo().unwrap();
        assert_eq!(write_to_bytes(&s.doc), after, "{name}: redo で適用後に戻る");
        r
    }

    /// 図面を変える道具の失敗。履歴も版番号も図面のバイト列も変わらないことを確かめ、説明の文を返す。
    pub fn rejected(s: &mut Server, name: &str, args: Value) -> String {
        use cad_core::native::write::write_to_bytes;
        let before = write_to_bytes(&s.doc);
        let history = s.doc.history().len();
        let revision = s.doc.revision();
        let dirty = s.doc.is_dirty();
        let msg = err(s, name, args.clone());
        assert_eq!(s.doc.history().len(), history, "{name} {args}: 履歴は不変");
        assert_eq!(s.doc.revision(), revision, "{name} {args}: 版番号は不変");
        assert_eq!(s.doc.is_dirty(), dirty);
        assert_eq!(write_to_bytes(&s.doc), before, "{name} {args}: 図面は不変");
        msg
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
    use super::test_support::{call_raw, err, ok, server};
    use super::*;
    use crate::test_util::TempDir;

    #[test]
    fn unknown_arguments_are_rejected_before_running() {
        let dir = TempDir::new("tools-unknown-arg");
        let mut s = server(&dir);
        cad_core_add_line(&mut s);
        let msg = err(&mut s, "new_drawing", json!({"discard_change": true}));
        assert!(
            msg.contains("知らない引数") && msg.contains("discard_change"),
            "{msg}"
        );
        assert!(s.doc.is_dirty(), "図面は捨てられていない");
        assert_eq!(s.serial, 1);
        // 引数を取らない道具も同じ。
        let msg = err(&mut s, "drawing_info", json!({"verbose": true}));
        assert!(msg.contains("引数を取りません"), "{msg}");
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
        // サーバーは続けて使える。読むだけの道具の panic では保存を止めない。
        let r = call_raw(&mut s, "drawing_info", json!({}));
        assert_eq!(r["isError"], false);
        assert_eq!(r["structuredContent"]["poisoned"], false);
        assert!(s.poisoned.is_none());
    }

    /// 図面を変える道具が panic したら、書きかけかもしれない図面の保存を拒む。
    /// 新規・開くで印が外れる（PR #84 レビューの非ブロッキング 2）。
    #[test]
    fn panics_in_editing_tools_block_saving() {
        let dir = TempDir::new("tools-poison");
        let mut s = server(&dir);
        cad_core_add_line(&mut s);
        ok(&mut s, "save_drawing", json!({"path": "a.ymc"}));
        let before = std::fs::read(dir.path().join("a.ymc")).unwrap();
        let tool = Tool {
            name: "half_edit",
            title: "",
            description: "",
            schema: || (json!({}), &[]),
            read_only: false,
            destructive: false,
            idempotent: false,
            run: |s, _| {
                cad_core_add_line(s);
                panic!("書きかけ")
            },
        };
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let r = run_tool(&mut s, &tool, Map::new());
        std::panic::set_hook(hook);
        assert_eq!(r["isError"], true, "{r}");
        assert!(r["content"][0]["text"].as_str().unwrap().contains("保存"));

        let info = ok(&mut s, "drawing_info", json!({}));
        assert_eq!(info["poisoned"], true);
        assert_eq!(info["poisoned_by"], "half_edit");
        for args in [
            json!({}),
            json!({"path": "b.ymc"}),
            json!({"path": "a.ymc", "overwrite": true}),
        ] {
            let msg = err(&mut s, "save_drawing", args);
            assert!(msg.contains("open_drawing"), "{msg}");
        }
        assert_eq!(
            std::fs::read(dir.path().join("a.ymc")).unwrap(),
            before,
            "上書きしていない"
        );
        assert!(!dir.path().join("b.ymc").exists());

        // 開き直すと印が外れ、保存できる。
        ok(
            &mut s,
            "open_drawing",
            json!({"path": "a.ymc", "discard_changes": true}),
        );
        assert_eq!(ok(&mut s, "drawing_info", json!({}))["poisoned"], false);
        ok(&mut s, "save_drawing", json!({}));
        // 新規でも外れる。
        s.poisoned = Some("x");
        ok(&mut s, "new_drawing", json!({}));
        assert!(s.poisoned.is_none());
    }

    /// 結果の大きさの上限。読むだけの道具は isError、図面を変える道具は結果を省いて成功のまま
    /// （PR #84 レビューの非ブロッキング 7）。
    #[test]
    fn results_are_bounded() {
        let dir = TempDir::new("tools-bounded");
        let mut s = server(&dir);
        let mut tool = Tool {
            name: "huge",
            title: "",
            description: "",
            schema: || (json!({}), &[]),
            read_only: true,
            destructive: false,
            idempotent: true,
            run: |_, _| Ok(json!({ "blob": "x".repeat(MAX_RESULT_BYTES) })),
        };
        let r = run_tool(&mut s, &tool, Map::new());
        assert_eq!(r["isError"], true, "{}", &r.to_string()[..200]);
        assert!(r["content"][0]["text"].as_str().unwrap().contains("絞り込"));

        tool.read_only = false;
        let r = run_tool(&mut s, &tool, Map::new());
        assert_eq!(r["isError"], false);
        assert_eq!(r["structuredContent"]["result_omitted"], true);
        assert!(r.to_string().len() < MAX_RESULT_BYTES);

        // 本物の道具でも: 頂点の多いポリラインを get_entities で取ると大きすぎる。
        use cad_core::command::AddEntities;
        use cad_core::geom::{Point2, Polyline};
        use cad_core::{Entity, Geometry, LayerId};
        let vertices: Vec<Point2> = (0..crate::limits::MAX_POLYLINE_VERTICES)
            .map(|i| Point2::new(f64::from(u32::try_from(i).unwrap()) * 1.234_567, 9.876_543))
            .collect();
        s.doc
            .apply(Box::new(AddEntities::one(
                "PLINE",
                Entity::new(
                    Geometry::Polyline(Polyline::new(vertices, false)),
                    LayerId::ZERO,
                ),
            )))
            .unwrap();
        let id = super::test_support::eid(&s, 0);
        let msg = err(&mut s, "get_entities", json!({ "ids": [id] }));
        assert!(msg.contains("大きすぎ"), "{msg}");
        ok(&mut s, "list_entities", json!({}));
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
