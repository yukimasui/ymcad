//! JSON-RPC 2.0 の封筒と MCP の取り決め。
//!
//! # 扱うもの
//!
//! | メソッド | 種類 | 返事 |
//! |---|---|---|
//! | `initialize` | 要求 | 版・能力・サーバー名 |
//! | `notifications/initialized` | 通知 | しない |
//! | `ping` | 要求 | `{}` |
//! | `tools/list` | 要求 | 道具の一覧 |
//! | `tools/call` | 要求 | 道具の結果（失敗は `isError: true`） |
//!
//! - **通知（`id` の無いメッセージ）には返事をしない**（知らない通知も黙って捨てる）
//! - **一括要求（配列）は扱わない**。`-32600` を 1 つ返す
//! - `initialize` の前に `tools/*` が来たら `-32600` で断る。版の取り決めの前に
//!   道具を動かすと、別の版の意味で処理してしまうため
//! - 知らないメソッドは `-32601`（`server/discover` もここ。下の「版」を参照）
//!
//! # 版
//!
//! `initialize` で取り決める版（2025-11-25 まで）に対応する。要求された版が
//! [`SUPPORTED_PROTOCOL_VERSIONS`] にあればそれを、無ければこちらの最新を返す。
//!
//! 仕様の最新（2026-07-28）は `initialize` を廃して要求ごとに版を載せる形に変わった。
//! 両方に対応するクライアントは、まず `server/discover` を送り、知らないエラーが返ったら
//! `initialize` へ戻る決まりなので、`server/discover` に `-32601` を返せばつながる。
//! 新しい形への対応は別の段階で行う（ADR-0046）。

use std::io::{self, BufRead};

use serde_json::{json, Map, Value};

use crate::server::Server;

/// 対応する版。**先頭が最新**。
pub const SUPPORTED_PROTOCOL_VERSIONS: [&str; 4] =
    ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// JSON-RPC のエラーコード。
pub mod error_code {
    /// JSON として読めない。
    pub const PARSE_ERROR: i64 = -32700;
    /// 要求の形が違う。
    pub const INVALID_REQUEST: i64 = -32600;
    /// 知らないメソッド。
    pub const METHOD_NOT_FOUND: i64 = -32601;
    /// 引数の形が違う（知らない道具を含む）。
    pub const INVALID_PARAMS: i64 = -32602;
}

/// 要求の失敗（JSON-RPC のエラー）。
pub(crate) type RpcError = (i64, String);

/// エラーの返事を作る。
#[must_use]
pub fn error_response(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message.into() },
    })
}

fn result_response(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

/// 届いた 1 メッセージを処理する。返事が要らなければ `None`。
pub(crate) fn handle(server: &mut Server, msg: Value) -> Option<Value> {
    use error_code::{INVALID_PARAMS, INVALID_REQUEST};

    let obj = match msg {
        Value::Object(o) => o,
        Value::Array(_) => {
            return Some(error_response(
                Value::Null,
                INVALID_REQUEST,
                "一括要求（配列）は扱いません。1 行に 1 つの要求を送ってください",
            ))
        }
        _ => {
            return Some(error_response(
                Value::Null,
                INVALID_REQUEST,
                "要求は JSON のオブジェクトでなければなりません",
            ))
        }
    };

    let id = match obj.get("id") {
        None => None,
        Some(v @ (Value::String(_) | Value::Number(_))) => Some(v.clone()),
        Some(_) => {
            return Some(error_response(
                Value::Null,
                INVALID_REQUEST,
                "id は文字列か数値でなければなりません",
            ))
        }
    };
    let reply_id = id.clone().unwrap_or(Value::Null);

    if obj.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Some(error_response(
            reply_id,
            INVALID_REQUEST,
            "jsonrpc は \"2.0\" でなければなりません",
        ));
    }

    let method = match obj.get("method") {
        Some(Value::String(m)) => m.as_str(),
        Some(_) => {
            return Some(error_response(
                reply_id,
                INVALID_REQUEST,
                "method は文字列でなければなりません",
            ))
        }
        None => {
            // クライアントからの返事。こちらは要求を送らないので、来ても捨てる。
            if id.is_some() && (obj.contains_key("result") || obj.contains_key("error")) {
                return None;
            }
            return Some(error_response(
                reply_id,
                INVALID_REQUEST,
                "method がありません",
            ));
        }
    };

    let params = match obj.get("params") {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(m)) => m.clone(),
        // 通知の形の誤りには返事をしない（通知には返事をしない約束が先）。
        Some(Value::Array(_)) => {
            id.as_ref()?;
            return Some(error_response(
                reply_id,
                INVALID_PARAMS,
                "params はオブジェクトでなければなりません",
            ));
        }
        Some(_) => {
            id.as_ref()?;
            return Some(error_response(
                reply_id,
                INVALID_REQUEST,
                "params はオブジェクトでなければなりません",
            ));
        }
    };

    let Some(id) = id else {
        notification(server, method);
        return None;
    };

    Some(match request(server, method, &params) {
        Ok(result) => result_response(id, result),
        Err((code, message)) => error_response(id, code, message),
    })
}

/// 通知。返事はしない。
fn notification(server: &mut Server, method: &str) {
    if method == "notifications/initialized" {
        server.initialized_notified = true;
    }
    // notifications/cancelled など: 要求は 1 つずつ同期で処理するので、取り消す相手がいない。
}

fn request(
    server: &mut Server,
    method: &str,
    params: &Map<String, Value>,
) -> Result<Value, RpcError> {
    use error_code::{INVALID_REQUEST, METHOD_NOT_FOUND};

    match method {
        "initialize" => initialize(server, params),
        "ping" => Ok(json!({})),
        "tools/list" | "tools/call" if server.protocol_version.is_none() => Err((
            INVALID_REQUEST,
            format!("{method} は initialize の後でなければ使えません"),
        )),
        "tools/list" => tools_list(params),
        "tools/call" => crate::tools::call(server, params),
        _ => Err((METHOD_NOT_FOUND, format!("未対応のメソッドです: {method}"))),
    }
}

fn initialize(server: &mut Server, params: &Map<String, Value>) -> Result<Value, RpcError> {
    use error_code::{INVALID_PARAMS, INVALID_REQUEST};

    if server.protocol_version.is_some() {
        return Err((INVALID_REQUEST, "initialize は済んでいます".to_owned()));
    }
    let requested = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            (
                INVALID_PARAMS,
                "protocolVersion（文字列）がありません".to_owned(),
            )
        })?;
    let chosen = SUPPORTED_PROTOCOL_VERSIONS
        .iter()
        .copied()
        .find(|v| *v == requested)
        .unwrap_or(SUPPORTED_PROTOCOL_VERSIONS[0]);
    server.protocol_version = Some(chosen);

    Ok(json!({
        "protocolVersion": chosen,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": {
            "name": "ymcad-mcp",
            "title": "ymcad",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "instructions": INSTRUCTIONS,
    }))
}

/// クライアントへ渡す使い方の要約（LLM が読む）。
const INSTRUCTIONS: &str = "ymcad（2D CAD）の図面ファイル（.ymc / .dxf）を開いて調べ、保存するサーバーです。\
角度は度、座標は f64 の数値（点は {\"x\":..,\"y\":..}）。\
図形 ID は d<起動の印>-<図面>e<番号>g<世代> の文字列で、図面を開き直す・サーバーをつなぎ直すと変わります（古い ID は拒まれます）。\
読み書きできるのは起動時に指定した root の配下の .ymc / .dxf だけです。\
.ymc が保存形式（無損失）、.dxf は交換用（R12・非可逆。保存すると警告が出ます）。\
未保存の変更があるときの new_drawing / open_drawing は discard_changes: true、\
開いたファイル以外の既存ファイルや他で書き換えられたファイルへの保存は overwrite: true が必要です。";

fn tools_list(params: &Map<String, Value>) -> Result<Value, RpcError> {
    if params.contains_key("cursor") {
        return Err((
            error_code::INVALID_PARAMS,
            "cursor は使えません（道具の一覧は 1 ページです）".to_owned(),
        ));
    }
    Ok(json!({ "tools": crate::tools::definitions() }))
}

/// 1 行ぶんの入力。
#[derive(Debug, PartialEq, Eq)]
pub enum Incoming {
    /// 1 行（改行は除いてある）。
    Message(String),
    /// 上限を超えた行（読み捨て済み）。
    TooLong,
    /// UTF-8 として読めない行。
    InvalidUtf8,
}

/// 改行までの 1 行を読む。入力が尽きたら `None`。
///
/// **`max` バイトを超えた行は貯めずに読み捨てる**（`BufRead::read_line` は上限なく
/// メモリを取るので使わない）。末尾の `\r` は外す。最後の行に改行が無くても 1 行として返す。
///
/// # Errors
///
/// 読み込みの失敗。
pub fn read_message<R: BufRead>(r: &mut R, max: usize) -> io::Result<Option<Incoming>> {
    let mut buf = Vec::new();
    let mut too_long = false;
    let mut saw_any = false;
    loop {
        let available = match r.fill_buf() {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if available.is_empty() {
            if !saw_any {
                return Ok(None);
            }
            break;
        }
        saw_any = true;
        let (chunk, used, done) = match available.iter().position(|b| *b == b'\n') {
            Some(i) => (&available[..i], i + 1, true),
            None => (available, available.len(), false),
        };
        if !too_long {
            if buf.len() + chunk.len() > max {
                too_long = true;
                buf = Vec::new();
            } else {
                buf.extend_from_slice(chunk);
            }
        }
        r.consume(used);
        if done {
            break;
        }
    }
    if too_long {
        return Ok(Some(Incoming::TooLong));
    }
    if buf.last() == Some(&b'\r') {
        buf.pop();
    }
    Ok(Some(match String::from_utf8(buf) {
        Ok(s) => Incoming::Message(s),
        Err(_) => Incoming::InvalidUtf8,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;
    use std::io::Cursor;

    fn server(dir: &TempDir) -> Server {
        Server::new(&[dir.path().to_path_buf()]).unwrap()
    }

    fn initialized(dir: &TempDir) -> Server {
        let mut s = server(dir);
        let r = s
            .handle(json!({"jsonrpc": "2.0", "id": 0, "method": "initialize",
                "params": {"protocolVersion": "2025-11-25", "capabilities": {},
                           "clientInfo": {"name": "test", "version": "0"}}}))
            .unwrap();
        assert!(r.get("result").is_some(), "{r}");
        assert!(s
            .handle(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .is_none());
        s
    }

    fn code(r: &Value) -> i64 {
        r["error"]["code"]
            .as_i64()
            .unwrap_or_else(|| panic!("エラーではない: {r}"))
    }

    #[test]
    fn initialize_echoes_a_supported_version() {
        let dir = TempDir::new("proto-init");
        for v in SUPPORTED_PROTOCOL_VERSIONS {
            let mut s = server(&dir);
            let r = s
                .handle(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                    "params": {"protocolVersion": v, "capabilities": {}}}))
                .unwrap();
            assert_eq!(r["result"]["protocolVersion"], v);
            assert_eq!(r["id"], 1);
            assert_eq!(r["result"]["capabilities"]["tools"]["listChanged"], false);
            assert_eq!(r["result"]["serverInfo"]["name"], "ymcad-mcp");
        }
    }

    #[test]
    fn initialize_answers_the_latest_for_unknown_versions() {
        let dir = TempDir::new("proto-init-unknown");
        let mut s = server(&dir);
        let r = s
            .handle(json!({"jsonrpc": "2.0", "id": "a", "method": "initialize",
                "params": {"protocolVersion": "1900-01-01"}}))
            .unwrap();
        assert_eq!(
            r["result"]["protocolVersion"],
            SUPPORTED_PROTOCOL_VERSIONS[0]
        );
        assert_eq!(r["id"], "a", "文字列の id もそのまま返す");
    }

    #[test]
    fn initialize_needs_a_version_and_only_once() {
        let dir = TempDir::new("proto-init-twice");
        let mut s = server(&dir);
        let r = s
            .handle(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}))
            .unwrap();
        assert_eq!(code(&r), error_code::INVALID_PARAMS);
        let mut s = initialized(&dir);
        let r = s
            .handle(json!({"jsonrpc": "2.0", "id": 2, "method": "initialize",
                "params": {"protocolVersion": "2025-11-25"}}))
            .unwrap();
        assert_eq!(code(&r), error_code::INVALID_REQUEST);
    }

    #[test]
    fn tools_are_refused_before_initialize() {
        let dir = TempDir::new("proto-preinit");
        let mut s = server(&dir);
        let r = s
            .handle(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}))
            .unwrap();
        assert_eq!(code(&r), error_code::INVALID_REQUEST);
        // ping は前でもよい。
        let r = s
            .handle(json!({"jsonrpc": "2.0", "id": 2, "method": "ping"}))
            .unwrap();
        assert_eq!(r["result"], json!({}));
    }

    /// 2026-07-28 の形のクライアントが最初に送る `server/discover` は、知らないメソッドとして断る
    /// （クライアントはこれを見て `initialize` へ戻る）。
    #[test]
    fn server_discover_is_method_not_found() {
        let dir = TempDir::new("proto-discover");
        let mut s = server(&dir);
        let r = s
            .handle(
                json!({"jsonrpc": "2.0", "id": 1, "method": "server/discover",
                "params": {"_meta": {"io.modelcontextprotocol/protocolVersion": "2026-07-28"}}}),
            )
            .unwrap();
        assert_eq!(code(&r), error_code::METHOD_NOT_FOUND);
    }

    #[test]
    fn notifications_get_no_reply() {
        let dir = TempDir::new("proto-notify");
        let mut s = initialized(&dir);
        for msg in [
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 1}}),
            json!({"jsonrpc": "2.0", "method": "notifications/unknown"}),
            json!({"jsonrpc": "2.0", "method": "tools/call", "params": {"name": "new_drawing"}}),
            json!({"jsonrpc": "2.0", "method": "ping", "params": [1]}),
            // クライアントからの返事（こちらは要求を送らない）も捨てる。
            json!({"jsonrpc": "2.0", "id": 9, "result": {}}),
        ] {
            assert_eq!(s.handle(msg.clone()), None, "{msg} に返事をしないこと");
        }
    }

    #[test]
    fn json_rpc_errors_have_the_right_codes() {
        let dir = TempDir::new("proto-errors");
        let mut s = initialized(&dir);
        let cases = [
            (
                json!([{"jsonrpc": "2.0", "id": 1, "method": "ping"}]),
                error_code::INVALID_REQUEST,
            ),
            (json!("ping"), error_code::INVALID_REQUEST),
            (
                json!({"jsonrpc": "1.0", "id": 1, "method": "ping"}),
                error_code::INVALID_REQUEST,
            ),
            (
                json!({"id": 1, "method": "ping"}),
                error_code::INVALID_REQUEST,
            ),
            (
                json!({"jsonrpc": "2.0", "id": {}, "method": "ping"}),
                error_code::INVALID_REQUEST,
            ),
            (
                json!({"jsonrpc": "2.0", "id": 1}),
                error_code::INVALID_REQUEST,
            ),
            (
                json!({"jsonrpc": "2.0", "id": 1, "method": 5}),
                error_code::INVALID_REQUEST,
            ),
            (
                json!({"jsonrpc": "2.0", "id": 1, "method": "nope"}),
                error_code::METHOD_NOT_FOUND,
            ),
            (
                json!({"jsonrpc": "2.0", "id": 1, "method": "ping", "params": [1]}),
                error_code::INVALID_PARAMS,
            ),
            (
                json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {}}),
                error_code::INVALID_PARAMS,
            ),
            (
                json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "nope"}}),
                error_code::INVALID_PARAMS,
            ),
            (
                json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                    "params": {"name": "drawing_info", "arguments": [1]}}),
                error_code::INVALID_PARAMS,
            ),
            (
                json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {"cursor": "x"}}),
                error_code::INVALID_PARAMS,
            ),
        ];
        for (msg, want) in cases {
            let r = s
                .handle(msg.clone())
                .unwrap_or_else(|| panic!("{msg} に返事が無い"));
            assert_eq!(code(&r), want, "{msg} → {r}");
            assert_eq!(r["jsonrpc"], "2.0");
        }
    }

    #[test]
    fn unparsable_lines_are_parse_errors() {
        let dir = TempDir::new("proto-parse");
        let mut s = initialized(&dir);
        let r = s.handle_line("{\"jsonrpc\": \"2.0\", ").unwrap();
        let r: Value = serde_json::from_str(&r).unwrap();
        assert_eq!(code(&r), error_code::PARSE_ERROR);
        assert_eq!(r["id"], Value::Null);
        assert_eq!(s.handle_line("   "), None, "空行は無視する");
        let r = s
            .handle_line("{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"ping\"}")
            .unwrap();
        assert!(!r.contains('\n'), "返事は 1 行");
    }

    /// tools/list の全道具: 名前が一意で仕様の文字種、inputSchema が object、
    /// required の各キーが properties にある。
    #[test]
    fn tool_definitions_are_well_formed() {
        let dir = TempDir::new("proto-list");
        let mut s = initialized(&dir);
        let r = s
            .handle(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}))
            .unwrap();
        let tools = r["result"]["tools"].as_array().unwrap();
        assert!(tools.len() >= 10);
        let mut names = std::collections::HashSet::new();
        for t in tools {
            let name = t["name"].as_str().unwrap();
            assert!(names.insert(name), "道具の名前 {name} が重複");
            assert!(
                !name.is_empty()
                    && name.len() <= 128
                    && name
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_-.".contains(&b)),
                "{name}"
            );
            assert!(t["description"].as_str().is_some_and(|d| !d.is_empty()));
            let schema = &t["inputSchema"];
            assert_eq!(schema["type"], "object", "{name}");
            let props = schema["properties"].as_object().unwrap();
            for req in schema["required"].as_array().map_or(&[][..], Vec::as_slice) {
                let key = req.as_str().unwrap();
                assert!(
                    props.contains_key(key),
                    "{name}: required の {key} が properties に無い"
                );
            }
            assert_eq!(schema["additionalProperties"], false, "{name}");
            let ann = &t["annotations"];
            assert!(ann["readOnlyHint"].is_boolean(), "{name}");
            assert!(ann["destructiveHint"].is_boolean(), "{name}");
        }
        for want in [
            "new_drawing",
            "open_drawing",
            "save_drawing",
            "drawing_info",
            "list_entities",
            "get_entities",
            "list_layers",
            "list_components",
            "undo",
            "redo",
        ] {
            assert!(names.contains(want), "{want} が一覧に無い");
        }
    }

    #[test]
    fn read_message_splits_lines_and_bounds_them() {
        let mut input = Cursor::new(b"abc\r\n\nxxxxxxxxxx\nok\n\xff\xfe\nlast".to_vec());
        let mut next = || read_message(&mut input, 5).unwrap();
        assert_eq!(next(), Some(Incoming::Message("abc".into())));
        assert_eq!(next(), Some(Incoming::Message(String::new())));
        assert_eq!(next(), Some(Incoming::TooLong));
        assert_eq!(
            next(),
            Some(Incoming::Message("ok".into())),
            "長い行の後も続けて読める"
        );
        assert_eq!(next(), Some(Incoming::InvalidUtf8));
        assert_eq!(next(), Some(Incoming::Message("last".into())));
        assert_eq!(next(), None);
    }

    /// バッファの境目をまたぐ長い行も、上限を超えた時点で貯めるのをやめる。
    #[test]
    fn read_message_handles_lines_longer_than_the_buffer() {
        let line = "y".repeat(100);
        let data = format!("{line}\nz\n");
        let mut input = io::BufReader::with_capacity(8, Cursor::new(data.into_bytes()));
        assert_eq!(
            read_message(&mut input, 50).unwrap(),
            Some(Incoming::TooLong)
        );
        assert_eq!(
            read_message(&mut input, 50).unwrap(),
            Some(Incoming::Message("z".into()))
        );
        let mut input =
            io::BufReader::with_capacity(8, Cursor::new(format!("{line}\n").into_bytes()));
        assert_eq!(
            read_message(&mut input, 100).unwrap(),
            Some(Incoming::Message(line))
        );
    }
}
