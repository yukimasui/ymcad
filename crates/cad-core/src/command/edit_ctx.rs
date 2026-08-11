//! エンティティとレイヤを変更できる唯一のハンドル。

use crate::entity::{Entity, EntityId, EntityStore};
use crate::error::{CadError, Result};
use crate::layer::{Layer, LayerId, LayerTable};

/// 構築を封じるためのゼロサイズ型。
///
/// このモジュールの外では **名前を書けない**ので、たとえ他のフィールドが公開されていても
/// 構造体リテラルで [`EditCtx`] を組み立てることはできない。
#[derive(Debug)]
struct Seal;

/// エンティティ・レイヤへの `&mut` を与える唯一の経路。
///
/// # なぜこれが必要か
///
/// 「Undo は後から付ける」を防ぐため、**エンティティを変更する経路をコマンド以外に
/// 作らない**というのが本プロジェクトの中核不変条件。これを規約ではなく
/// **型で強制**しているのがこの構造体。
///
/// 1. [`EntityStore`] / [`LayerTable`] の変更系メソッドはすべて `pub(crate)`。
///    `cad-app` は別クレートなので、Rust のモジュール可視性により
///    **名前を呼ぶことすらできない**（lint ではなくコンパイルエラー）。
/// 2. `EditCtx` のフィールドは private、[`EditCtx::new`] は `pub(crate)`、
///    さらに [`Seal`] を持つため構造体リテラルでも作れない。
/// 3. [`Command`](super::Command) は `&mut Document` ではなく `&mut EditCtx` を受け取る。
///    コマンドから `Document::apply` を再帰的に呼べないので、
///    Undo エントリの入れ子や再入が原理的に起こらない。
///
/// 複合的な編集は `Document` を渡すのではなく
/// [`MacroCommand`](super::MacroCommand) で表現する。
#[derive(Debug)]
pub struct EditCtx<'a> {
    entities: &'a mut EntityStore,
    layers: &'a mut LayerTable,
    _seal: Seal,
}

impl<'a> EditCtx<'a> {
    /// [`Document`](crate::Document) だけが呼ぶ。
    pub(crate) fn new(entities: &'a mut EntityStore, layers: &'a mut LayerTable) -> Self {
        Self {
            entities,
            layers,
            _seal: Seal,
        }
    }

    // ---- 参照 -------------------------------------------------------------

    /// エンティティの読み取り。
    #[must_use]
    pub fn entities(&self) -> &EntityStore {
        self.entities
    }

    /// レイヤの読み取り。
    #[must_use]
    pub fn layers(&self) -> &LayerTable {
        self.layers
    }

    // ---- エンティティの変更 -----------------------------------------------

    /// エンティティを追加し、割り当てられた ID を返す。
    pub fn add_entity(&mut self, entity: Entity) -> EntityId {
        self.entities.insert(entity)
    }

    /// **元の ID のまま**エンティティを戻す。削除の Undo で使う。
    ///
    /// # Errors
    ///
    /// スロットが埋まっている場合 [`CadError::SlotOccupied`]。
    pub fn restore_entity(&mut self, id: EntityId, entity: Entity) -> Result<()> {
        self.entities.restore(id, entity)
    }

    /// エンティティを取り除き、中身を返す。Undo で戻せるよう必ず受け取ること。
    ///
    /// # Errors
    ///
    /// 存在しない ID の場合 [`CadError::EntityNotFound`]。
    pub fn remove_entity(&mut self, id: EntityId) -> Result<Entity> {
        self.entities.remove(id)
    }

    /// エンティティを変更するための可変参照。
    ///
    /// # Errors
    ///
    /// 存在しない ID の場合 [`CadError::EntityNotFound`]。
    pub fn entity_mut(&mut self, id: EntityId) -> Result<&mut Entity> {
        self.entities.get_mut(id).ok_or(CadError::EntityNotFound)
    }

    // ---- レイヤの変更 -----------------------------------------------------

    /// レイヤを追加する（同名があればその ID を返す）。
    pub fn add_layer(&mut self, layer: Layer) -> LayerId {
        self.layers.insert(layer)
    }

    /// レイヤを変更するための可変参照。
    ///
    /// # Errors
    ///
    /// 存在しない ID の場合 [`CadError::LayerNotFound`]。
    pub fn layer_mut(&mut self, id: LayerId) -> Result<&mut Layer> {
        self.layers.get_mut(id).ok_or(CadError::LayerNotFound)
    }

    /// 現在レイヤを切り替える。
    pub fn set_current_layer(&mut self, id: LayerId) {
        self.layers.set_current(id);
    }
}
