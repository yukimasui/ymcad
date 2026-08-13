//! ymcad のコア。ジオメトリ、エンティティ、コマンド、ファイル入出力を担う。
//!
//! ファイル形式は 2 つある。[`native`] がネイティブ（`.ymc`、無損失）で、
//! [`dxf`] は交換専用（R12、非可逆）。
//!
//! # 不変条件
//!
//! 1. **UI 非依存。** egui / eframe / winit などに依存してはならない
//!    （将来 wasm ビューアを載せる余地を残すため。CI で機械的に検査している）。
//! 2. **座標はすべて `f64`。** `f32` はこのクレートに登場しない。
//!    画面座標への変換は `cad-app` の `viewport.rs` の責務。
//! 3. **エンティティを変更できるのは [`Command`] だけ。**
//!    詳細は [`command`] モジュールを参照。

#![forbid(unsafe_code)]

/// ファイルをアトミックに置き換える内部ヘルパ。
///
/// 書き出しモジュール（[`dxf`] / [`native`]）から使う。外部には出さない。
mod atomic_write;

pub mod command;
pub mod component;
pub mod document;
pub mod dxf;
pub mod entity;
pub mod error;
pub mod expr;
pub mod geom;
pub mod group;
pub mod layer;
pub mod native;
pub mod snap;

pub use command::{Command, EditCtx, UndoStack};
pub use component::{Definition, DefinitionId, DefinitionTable, Instance, Placement};
pub use document::Document;
pub use entity::{Entity, EntityId, EntityStore, Geometry};
pub use error::{CadError, Result};
pub use group::{Group, GroupId, GroupTable};
pub use layer::{AciColor, ColorSpec, Layer, LayerId, LayerTable};
