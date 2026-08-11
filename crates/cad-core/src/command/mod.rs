//! コマンドと Undo/Redo。
//!
//! **エンティティを変更できるのはこのモジュールの [`Command`] だけ。**
//! 詳しい理由と型による強制のしかたは [`EditCtx`] を参照。

pub mod basic;
pub mod edit_ctx;
pub mod layer_ops;
pub mod stack;
pub mod transform;

pub use basic::{AddEntities, DeleteEntities};
pub use edit_ctx::EditCtx;
pub use layer_ops::{
    AddLayer, DeleteLayer, MoveEntitiesToLayer, RenameLayer, SetCurrentLayer, SetLayerProperties,
};
pub use stack::UndoStack;
pub use transform::{CopyEntities, MoveEntities};

use crate::error::Result;

/// 取り消し可能な 1 操作。
///
/// # 実装時の契約
///
/// - `undo` は `execute` の直前の状態を **`EntityId` まで含めて** 完全に復元すること。
///   削除した要素は [`EditCtx::restore_entity`] で元の ID のまま戻す。
/// - `execute` は全部成功するか、何も変えずに失敗するかのどちらかであること
///   （途中まで適用して `Err` を返さない）。失敗したコマンドは履歴に積まれない。
/// - `execute` → `undo` → `execute` を繰り返しても同じ結果になること。
pub trait Command: std::fmt::Debug {
    /// 操作を適用する。
    ///
    /// # Errors
    ///
    /// 適用できない場合。このとき文書は変更前のままでなければならない。
    fn execute(&mut self, ctx: &mut EditCtx<'_>) -> Result<()>;

    /// 操作を取り消す。
    ///
    /// # Errors
    ///
    /// 取り消しに失敗した場合。通常は発生しない。
    fn undo(&mut self, ctx: &mut EditCtx<'_>) -> Result<()>;

    /// コマンドラインや Undo 表示に出す名前（`"LINE"`, `"ERASE"` など）。
    fn name(&self) -> &'static str;
}

/// 複数のコマンドを 1 つの Undo 単位にまとめる。
///
/// コマンドに `&mut Document` を渡さずに複合操作を表現するための手段。
#[derive(Debug)]
pub struct MacroCommand {
    name: &'static str,
    commands: Vec<Box<dyn Command>>,
}

impl MacroCommand {
    /// 名前と子コマンドから作る。
    #[must_use]
    pub fn new(name: &'static str, commands: Vec<Box<dyn Command>>) -> Self {
        Self { name, commands }
    }
}

impl Command for MacroCommand {
    fn execute(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        for (i, c) in self.commands.iter_mut().enumerate() {
            if let Err(e) = c.execute(ctx) {
                // 途中で失敗したら、成功した分を逆順に巻き戻して原状復帰する。
                for done in self.commands[..i].iter_mut().rev() {
                    let _ = done.undo(ctx);
                }
                return Err(e);
            }
        }
        Ok(())
    }

    fn undo(&mut self, ctx: &mut EditCtx<'_>) -> Result<()> {
        // 適用と逆順に戻す。
        for c in self.commands.iter_mut().rev() {
            c.undo(ctx)?;
        }
        Ok(())
    }

    fn name(&self) -> &'static str {
        self.name
    }
}
