//! `ymcad-mcp`: 標準入出力で話す ymcad の MCP サーバー。
//!
//! ```text
//! ymcad-mcp [--root <dir>]...
//! ```
//!
//! - 1 行 = 1 メッセージ（JSON-RPC）。**stdout にはプロトコルのメッセージだけを書く**。
//!   案内やログはすべて stderr
//! - stdin が尽きたら正常終了する（クライアントが閉じたとき）
//! - `--root` を省くとカレントディレクトリを root にし、そのことを stderr に出す
//!
//! 使い方は `docs/MCP.md`。

#![forbid(unsafe_code)]

use std::io::{self, BufWriter, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use cad_mcp::limits::MAX_LINE_BYTES;
use cad_mcp::protocol::{error_code, error_response};
use cad_mcp::{read_message, Incoming, Server};
use serde_json::Value;

const USAGE: &str = "使い方: ymcad-mcp [--root <ディレクトリ>]...
  標準入出力で MCP（JSON-RPC、1 行 1 メッセージ）を話すサーバーです。
  --root <dir>  読み書きを許すディレクトリ（複数指定可。省略するとカレントディレクトリ）
  --help        この説明
  --version     版";

fn main() -> ExitCode {
    let roots = match parse_args(std::env::args().skip(1)) {
        Ok(Some(r)) => r,
        Ok(None) => return ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("ymcad-mcp: {msg}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let roots = if roots.is_empty() {
        match std::env::current_dir() {
            Ok(cwd) => {
                eprintln!(
                    "ymcad-mcp: --root が無いので、カレントディレクトリ {} の配下だけを読み書きします",
                    cwd.display()
                );
                vec![cwd]
            }
            Err(e) => {
                eprintln!("ymcad-mcp: カレントディレクトリが分かりません: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        roots
    };

    let mut server = match Server::new(&roots) {
        Ok(s) => s,
        Err(msg) => {
            eprintln!("ymcad-mcp: {msg}");
            return ExitCode::from(2);
        }
    };
    let shown: Vec<String> = server
        .roots()
        .iter()
        .map(|r| r.display().to_string())
        .collect();
    eprintln!(
        "ymcad-mcp {}: 起動しました（root: {}）",
        env!("CARGO_PKG_VERSION"),
        shown.join(", ")
    );

    match serve(&mut server) {
        Ok(()) => {
            eprintln!("ymcad-mcp: 入力が閉じたので終了します");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("ymcad-mcp: 入出力のエラーで終了します: {e}");
            ExitCode::FAILURE
        }
    }
}

/// 引数を読む。`--help` / `--version` なら表示して `None`。
fn parse_args(args: impl Iterator<Item = String>) -> Result<Option<Vec<PathBuf>>, String> {
    let mut roots = Vec::new();
    let mut args = args.peekable();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--help" | "-h" => {
                eprintln!("{USAGE}");
                return Ok(None);
            }
            "--version" | "-V" => {
                eprintln!("ymcad-mcp {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "--root" => {
                let dir = args
                    .next()
                    .ok_or_else(|| "--root の後にディレクトリがありません".to_owned())?;
                roots.push(PathBuf::from(dir));
            }
            other => match other.strip_prefix("--root=") {
                Some(dir) if !dir.is_empty() => roots.push(PathBuf::from(dir)),
                _ => return Err(format!("知らない引数です: {other}")),
            },
        }
    }
    Ok(Some(roots))
}

/// 標準入出力のループ。stdin が尽きたら `Ok`。
fn serve(server: &mut Server) -> io::Result<()> {
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut output = BufWriter::new(io::stdout().lock());
    while let Some(incoming) = read_message(&mut input, MAX_LINE_BYTES)? {
        let reply = match incoming {
            Incoming::Message(line) => server.handle_line(&line),
            Incoming::TooLong => Some(parse_error(&format!(
                "1 行が長すぎます（上限 {MAX_LINE_BYTES} バイト）。読み捨てました"
            ))),
            Incoming::InvalidUtf8 => Some(parse_error("UTF-8 として読めない行です")),
        };
        if let Some(reply) = reply {
            output.write_all(reply.as_bytes())?;
            output.write_all(b"\n")?;
            output.flush()?;
        }
    }
    Ok(())
}

fn parse_error(message: &str) -> String {
    error_response(Value::Null, error_code::PARSE_ERROR, message).to_string()
}
