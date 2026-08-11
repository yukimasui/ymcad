//! Undo / Redo スタック。

use std::collections::VecDeque;

use super::Command;

/// 取り消し履歴。
///
/// 上限を超えた分は古い方から捨てる。
#[derive(Debug)]
pub struct UndoStack {
    done: VecDeque<Box<dyn Command>>,
    undone: Vec<Box<dyn Command>>,
    limit: usize,
}

impl Default for UndoStack {
    fn default() -> Self {
        Self::new(Self::DEFAULT_LIMIT)
    }
}

impl UndoStack {
    /// 既定の履歴数。指示書の要求（最低 100）に対して余裕を持たせている。
    pub const DEFAULT_LIMIT: usize = 256;

    /// 上限を指定して作る。0 を渡した場合は 1 に切り上げる。
    #[must_use]
    pub fn new(limit: usize) -> Self {
        Self {
            done: VecDeque::new(),
            undone: Vec::new(),
            limit: limit.max(1),
        }
    }

    /// Undo できるか。
    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.done.is_empty()
    }

    /// Redo できるか。
    #[must_use]
    pub fn can_redo(&self) -> bool {
        !self.undone.is_empty()
    }

    /// 次に Undo される操作の名前。
    #[must_use]
    pub fn undo_name(&self) -> Option<&'static str> {
        self.done.back().map(|c| c.name())
    }

    /// 次に Redo される操作の名前。
    #[must_use]
    pub fn redo_name(&self) -> Option<&'static str> {
        self.undone.last().map(|c| c.name())
    }

    /// 保持している Undo 可能な操作数。
    #[must_use]
    pub fn len(&self) -> usize {
        self.done.len()
    }

    /// 履歴が空か。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.done.is_empty()
    }

    /// 履歴の上限。
    #[must_use]
    pub fn limit(&self) -> usize {
        self.limit
    }

    // ---- `Document` からのみ使う ----------------------------------------

    /// 適用済みコマンドを積む。Redo 列は捨てる。
    pub(crate) fn push(&mut self, command: Box<dyn Command>) {
        self.undone.clear();
        self.done.push_back(command);
        while self.done.len() > self.limit {
            self.done.pop_front();
        }
    }

    /// Undo 対象を取り出す。
    pub(crate) fn pop_for_undo(&mut self) -> Option<Box<dyn Command>> {
        self.done.pop_back()
    }

    /// Undo に成功したコマンドを Redo 列へ移す。
    pub(crate) fn push_undone(&mut self, command: Box<dyn Command>) {
        self.undone.push(command);
    }

    /// Redo 対象を取り出す。
    pub(crate) fn pop_for_redo(&mut self) -> Option<Box<dyn Command>> {
        self.undone.pop()
    }

    /// Undo に失敗したコマンドを元の位置へ戻す。
    pub(crate) fn push_back_done(&mut self, command: Box<dyn Command>) {
        self.done.push_back(command);
    }

    /// 履歴をすべて捨てる。
    pub(crate) fn clear(&mut self) {
        self.done.clear();
        self.undone.clear();
    }
}
