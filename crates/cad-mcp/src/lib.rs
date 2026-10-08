//! ymcad の MCP サーバー。
//!
//! LLM（Claude Code などの MCP クライアント）が、標準入出力の JSON-RPC で
//! 図面ファイル（`.ymc` / `.dxf`）を開き、中身を調べ、描いて変え、保存できるようにする。
//! 設計判断は `docs/DECISIONS.md` の ADR-0046、使い方は `docs/MCP.md`。
//!
//! # 構成
//!
//! | モジュール | 役割 |
//! |---|---|
//! | [`protocol`] | JSON-RPC の封筒（要求・通知・エラー）と MCP の取り決め（`initialize`） |
//! | [`server`] | [`Server`]: 図面・図面の通し番号・root・開いたファイルの記録 |
//! | `tools` | 道具の一覧と実装（`tools/list` / `tools/call`）。図面を変える道具の約束は `tools/mutate.rs` |
//! | [`convert`] | 図形 ↔ JSON、度 ↔ ラジアン。**変換はここに 1 か所だけ** |
//! | [`ids`] | 図形 ID の文字列 `d<起動の印>-<図面>e<index>g<generation>` |
//! | [`render`] | 図面の SVG / PNG（モデル → 画像 px の変換は `render::fit` に 1 か所） |
//! | [`paths`] | root の配下だけを読み書きさせるパスの検査 |
//! | [`limits`] | 入力・ファイル・一覧の上限 |
//!
//! ライブラリは標準入出力から独立している（[`Server::handle`] は JSON を受けて JSON を返すだけ）。
//! 標準入出力のループはバイナリ `ymcad-mcp`（`src/main.rs`）にある。
//!
//! # 守ること
//!
//! - **図面を変える経路は `cad-core` の `Command` だけ**（設計原則 4）。このクレートは
//!   `Document::apply` / `undo` / `redo` しか呼ばない
//! - **ディスクに書くのは `save_drawing` だけ**。書き込みは `cad-core` のアトミックな書き出し
//! - **ネットワークもシェルも使わない**
//! - 座標は `f64`。角度は入出力とも**度**（内部はラジアン。変換は [`convert`] だけ）

#![forbid(unsafe_code)]

pub mod convert;
pub mod ids;
pub mod paths;
pub mod protocol;
pub mod render;
pub mod server;
mod tools;

#[cfg(test)]
mod test_util;

pub use protocol::{read_message, Incoming};
pub use server::Server;

/// 入力・ファイル・一覧の上限。
///
/// LLM は大きな値を平気で渡してくるので、メモリや出力の大きさが青天井にならないよう柵を置く。
pub mod limits {
    /// 1 行（= 1 メッセージ）の最大バイト数。超えた行は読み捨てて `-32700` を返す。
    pub const MAX_LINE_BYTES: usize = 4 * 1024 * 1024;

    /// 開けるファイルの最大バイト数。
    pub const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

    /// パスの最大バイト数。
    pub const MAX_PATH_BYTES: usize = 4096;

    /// `list_entities` の `limit` の既定値。
    pub const DEFAULT_LIST_LIMIT: usize = 100;

    /// `list_entities` の `limit` の上限。
    pub const MAX_LIST_LIMIT: usize = 1000;

    /// 1 回の呼び出しで指定できる図形 ID の数。
    pub const MAX_IDS_PER_CALL: usize = 1000;

    /// `undo` / `redo` の `steps` の上限。履歴の深さ（`UndoStack::DEFAULT_LIMIT`）と同じ。
    pub const MAX_HISTORY_STEPS: usize = cad_core::UndoStack::DEFAULT_LIMIT;

    /// 1 回の呼び出しで作る・変える図形の数（`add_entities` の `entities`・`modify_entities` の `changes`）。
    pub const MAX_SHAPES_PER_CALL: usize = 1000;

    /// 1 本のポリラインの頂点の数。
    pub const MAX_POLYLINE_VERTICES: usize = 10_000;

    /// 図面に置ける図形の数。複製の道具が倍々に増やしても、メモリと保存の時間が青天井にならないように。
    pub const MAX_DRAWING_ENTITIES: usize = 1_000_000;

    /// 座標・長さの絶対値の上限（図面単位）。`1e300` のような値は有限でも、描画・スナップ・
    /// トレランスの計算が成り立たない。10 億（mm なら 1000 km）あれば図面には足りる。
    pub const MAX_COORDINATE: f64 = 1e9;

    /// 数値に書ける式の文字列の長さ（バイト）。
    pub const MAX_EXPR_BYTES: usize = 1024;

    /// レイヤ名の長さ（文字数）。
    pub const MAX_LAYER_NAME_CHARS: usize = 255;

    /// 道具の結果（JSON の text）の最大バイト数。超えたら `isError` で絞り込みを促す。
    ///
    /// 入力を 1 行 4 MiB で止めるのと対にする。クライアントの側にも出力の上限がある
    /// （Claude Code は既定で MCP の出力をおよそ 25,000 トークンで打ち切る）ので、それより
    /// 少し大きいくらいに置き、超えたら一覧の `limit` や ID の数を減らしてもらう。
    pub const MAX_RESULT_BYTES: usize = 256 * 1024;
}
