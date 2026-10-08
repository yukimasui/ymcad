//! サーバーの状態。

use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use cad_core::Document;
use serde_json::Value;

use crate::paths::{Format, Roots};

/// MCP サーバー。図面を 1 枚だけ持つ。
///
/// 標準入出力からは独立していて、[`Self::handle`] が JSON を受けて JSON を返すだけ。
#[derive(Debug)]
pub struct Server {
    /// 読み書きを許すディレクトリ。
    pub(crate) roots: Roots,
    /// いまの図面。**変えるのは `Document::apply` / `undo` / `redo` だけ**（設計原則 4）。
    pub(crate) doc: Document,
    /// 図面の通し番号。新規・開くたびに増える。図形 ID の頭に入る（[`crate::ids`]）。
    pub(crate) serial: u64,
    /// 図面を読み書きしたファイル。新規なら `None`。
    pub(crate) file: Option<OpenedFile>,
    /// `initialize` で取り決めた版。取り決める前は `None`。
    pub(crate) protocol_version: Option<&'static str>,
    /// `notifications/initialized` を受け取ったか（記録だけ。動作は変えない）。
    pub(crate) initialized_notified: bool,
}

/// 図面を読み書きしたファイル。
#[derive(Debug, Clone)]
pub(crate) struct OpenedFile {
    /// 実体のパス（`canonicalize` 済み）。
    pub path: PathBuf,
    /// 形式（拡張子で決まる）。
    pub format: Format,
    /// 読み書きした時点の更新時刻と大きさ。他のプロセスが書き換えたかの判定に使う。
    pub stamp: Option<FileStamp>,
}

/// ファイルの更新時刻と大きさ。
///
/// 中身のハッシュは取らない（依存を足さず、大きなファイルを毎回読み直さないため）。
/// 同じ大きさで更新時刻の分解能の内に書き換えられると見逃す（ADR-0046）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FileStamp {
    modified: Option<SystemTime>,
    len: u64,
}

impl FileStamp {
    pub(crate) fn from_metadata(meta: &Metadata) -> Self {
        Self {
            modified: meta.modified().ok(),
            len: meta.len(),
        }
    }

    /// いまのファイルの記録。ファイルが無ければ `None`。
    pub(crate) fn of(path: &Path) -> Option<Self> {
        std::fs::metadata(path)
            .ok()
            .map(|m| Self::from_metadata(&m))
    }
}

impl Server {
    /// `roots` の配下だけを読み書きするサーバーを作る。図面は空の新規図面。
    ///
    /// # Errors
    ///
    /// root が 1 つも無い・ディレクトリとして解決できない場合。
    pub fn new(roots: &[PathBuf]) -> Result<Self, String> {
        Ok(Self {
            roots: Roots::new(roots)?,
            doc: Document::new(),
            serial: 1,
            file: None,
            protocol_version: None,
            initialized_notified: false,
        })
    }

    /// 読み書きを許すディレクトリ（`canonicalize` 済み）。
    #[must_use]
    pub fn roots(&self) -> &[PathBuf] {
        self.roots.dirs()
    }

    /// `initialize` で取り決めた版。
    #[must_use]
    pub fn protocol_version(&self) -> Option<&'static str> {
        self.protocol_version
    }

    /// 1 メッセージを処理する。返事が要らない（通知など）なら `None`。
    pub fn handle(&mut self, msg: Value) -> Option<Value> {
        crate::protocol::handle(self, msg)
    }

    /// 1 行の JSON を処理し、返事の 1 行（改行なし）を返す。空行は無視する。
    ///
    /// JSON として読めなければ `-32700` を返す。
    pub fn handle_line(&mut self, line: &str) -> Option<String> {
        let line = line.trim();
        if line.is_empty() {
            return None;
        }
        let reply = match serde_json::from_str::<Value>(line) {
            Ok(msg) => self.handle(msg)?,
            Err(e) => crate::protocol::error_response(
                Value::Null,
                crate::protocol::error_code::PARSE_ERROR,
                format!("JSON として読めません: {e}"),
            ),
        };
        // `serde_json::to_string` は文字列中の改行をエスケープするので、返事は必ず 1 行になる。
        Some(reply.to_string())
    }

    /// 図面を入れ替える（新規・開く）。通し番号を進める。
    pub(crate) fn replace_document(&mut self, doc: Document, file: Option<OpenedFile>) {
        self.doc = doc;
        self.file = file;
        self.serial += 1;
    }
}
