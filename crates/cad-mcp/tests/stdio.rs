//! バイナリ `ymcad-mcp` を実際に起動し、標準入出力で話す結合テスト。
//!
//! root は `CARGO_TARGET_TMPDIR` の下にテストごとのディレクトリを作る（tempfile の依存を足さない）。

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{json, Value};

/// テストごとの空のディレクトリ。
fn work_dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("stdio-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

struct Mcp {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
}

impl Mcp {
    fn start(args: &[&str]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_ymcad-mcp"))
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("ymcad-mcp を起動できません");
        let stdin = child.stdin.take();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            stdin,
            stdout,
        }
    }

    fn send_raw(&mut self, line: &str) {
        let stdin = self.stdin.as_mut().unwrap();
        stdin.write_all(line.as_bytes()).unwrap();
        stdin.write_all(b"\n").unwrap();
        stdin.flush().unwrap();
    }

    fn send(&mut self, msg: &Value) {
        self.send_raw(&msg.to_string());
    }

    /// 返事を 1 行読む。**1 行が丸ごと 1 つの JSON であること**も確かめる。
    fn recv(&mut self) -> Value {
        let mut line = String::new();
        let n = self.stdout.read_line(&mut line).unwrap();
        assert!(n > 0, "stdout が閉じた");
        assert!(line.ends_with('\n'));
        serde_json::from_str(line.trim_end())
            .unwrap_or_else(|e| panic!("stdout にプロトコル以外が出た: {line:?} ({e})"))
    }

    fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        let r = self.recv();
        assert_eq!(r["id"], id, "{r}");
        r
    }

    fn call(&mut self, id: u64, name: &str, args: Value) -> Value {
        self.request(id, "tools/call", json!({"name": name, "arguments": args}))["result"].clone()
    }

    fn initialize(&mut self) {
        let r = self.request(
            0,
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {},
                   "clientInfo": {"name": "stdio-test", "version": "0"}}),
        );
        assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
        self.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    }

    /// stdin を閉じ、終了を待つ。stdout の残りと stderr を返す。
    fn finish(mut self) -> (std::process::ExitStatus, String, String) {
        drop(self.stdin.take());
        let status = self.child.wait().unwrap();
        let mut rest = String::new();
        self.stdout.read_to_string(&mut rest).unwrap();
        let mut err = String::new();
        self.child
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut err)
            .unwrap();
        (status, rest, err)
    }
}

#[test]
fn session_over_stdio_saves_a_drawing() {
    let dir = work_dir("session");
    let mut mcp = Mcp::start(&["--root", dir.to_str().unwrap()]);
    mcp.initialize();

    let tools = mcp.request(1, "tools/list", json!({}));
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"save_drawing"), "{names:?}");

    let r = mcp.call(2, "save_drawing", json!({"path": "empty"}));
    assert_eq!(r["isError"], false, "{r}");
    let saved = dir.join("empty.ymc");
    let bytes = std::fs::read(&saved).unwrap();
    assert!(
        bytes.starts_with(b"YMCAD\x1a\0\0"),
        "ネイティブ形式で書かれている"
    );

    let r = mcp.call(3, "open_drawing", json!({"path": "empty.ymc"}));
    let name = r["structuredContent"]["drawing"].as_str().unwrap();
    assert!(
        name.starts_with('d') && name.ends_with("-2"),
        "2 枚目の図面: {name}"
    );
    let r = mcp.call(4, "drawing_info", json!({}));
    assert_eq!(r["structuredContent"]["format"], "ymc");
    assert_eq!(r["structuredContent"]["dirty"], false);

    // root の外は拒む（isError）。サーバーは続けて動く。
    let r = mcp.call(5, "save_drawing", json!({"path": "../escape.ymc"}));
    assert_eq!(r["isError"], true);
    assert!(!dir.parent().unwrap().join("escape.ymc").exists());
    let r = mcp.request(6, "ping", json!({}));
    assert_eq!(r["result"], json!({}));

    let (status, rest, err) = mcp.finish();
    assert!(
        status.success(),
        "stdin が尽きたら正常終了: {status:?}\n{err}"
    );
    assert_eq!(rest, "", "返事の無いものは stdout に出ない");
    assert!(err.contains("起動しました"), "ログは stderr: {err}");
}

#[test]
fn bad_lines_get_errors_and_the_loop_continues() {
    let dir = work_dir("bad-lines");
    let mut mcp = Mcp::start(&["--root", dir.to_str().unwrap()]);

    mcp.send_raw("{not json");
    let r = mcp.recv();
    assert_eq!(r["error"]["code"], -32700);
    assert_eq!(r["id"], Value::Null);

    // 上限を超える 1 行は読み捨てて -32700。
    let long = format!("\"{}\"", "x".repeat(cad_mcp::limits::MAX_LINE_BYTES + 10));
    mcp.send_raw(&long);
    let r = mcp.recv();
    assert_eq!(r["error"]["code"], -32700);

    // 空行と通知には返事をしない。次の要求の返事がすぐ来ること。
    mcp.send_raw("");
    mcp.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    let r = mcp.request(7, "tools/list", json!({}));
    assert_eq!(r["error"]["code"], -32600, "initialize の前の tools/list");
    mcp.initialize();
    let r = mcp.request(8, "nope", json!({}));
    assert_eq!(r["error"]["code"], -32601);

    let (status, rest, _) = mcp.finish();
    assert!(status.success());
    assert_eq!(rest, "");
}

#[test]
fn default_root_is_the_current_directory() {
    let dir = work_dir("cwd");
    let mut child = Command::new(env!("CARGO_BIN_EXE_ymcad-mcp"))
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    assert!(out.stdout.is_empty());
    let err = String::from_utf8(out.stderr).unwrap();
    assert!(err.contains("カレントディレクトリ"), "{err}");
}

#[test]
fn missing_root_is_an_error() {
    let dir = work_dir("missing-root");
    let out = Command::new(env!("CARGO_BIN_EXE_ymcad-mcp"))
        .args(["--root", dir.join("nope").to_str().unwrap()])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(out.stdout.is_empty(), "エラーも stdout には出さない");

    let out = Command::new(env!("CARGO_BIN_EXE_ymcad-mcp"))
        .args(["--bogus"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(out.stdout.is_empty());
}
