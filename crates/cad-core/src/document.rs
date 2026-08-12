//! 図面ドキュメント。
//!
//! エンティティ・レイヤ・Undo 履歴を束ねる。

use std::path::{Path, PathBuf};

use crate::command::{Command, EditCtx, UndoStack};
use crate::entity::EntityStore;
use crate::error::Result;
use crate::geom::Aabb;
use crate::group::GroupTable;
use crate::layer::LayerTable;

/// 1 つの図面。
///
/// # 変更経路は 3 つだけ
///
/// [`apply`](Self::apply) / [`undo`](Self::undo) / [`redo`](Self::redo) 以外に
/// エンティティを変更する手段を **意図的に用意していない**。
/// `entities_mut` や `DerefMut` を生やすと、その瞬間に Undo の正しさが失われる。
#[derive(Debug)]
pub struct Document {
    entities: EntityStore,
    layers: LayerTable,
    groups: GroupTable,
    history: UndoStack,
    revision: u64,
    dirty: bool,
    path: Option<PathBuf>,
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

impl Document {
    /// レイヤ `"0"` だけを持つ空の図面。
    #[must_use]
    pub fn new() -> Self {
        Self {
            entities: EntityStore::new(),
            layers: LayerTable::new(),
            groups: GroupTable::new(),
            history: UndoStack::default(),
            revision: 0,
            dirty: false,
            path: None,
        }
    }

    // ---- 参照 -------------------------------------------------------------

    /// エンティティ。
    #[must_use]
    pub fn entities(&self) -> &EntityStore {
        &self.entities
    }

    /// レイヤ。
    #[must_use]
    pub fn layers(&self) -> &LayerTable {
        &self.layers
    }

    /// グループ。
    #[must_use]
    pub fn groups(&self) -> &GroupTable {
        &self.groups
    }

    /// Undo 履歴。
    #[must_use]
    pub fn history(&self) -> &UndoStack {
        &self.history
    }

    /// 図面全体の境界ボックス。ZOOM EXTENTS で使う。
    #[must_use]
    pub fn bbox(&self) -> Aabb {
        self.entities.bbox()
    }

    /// 変更のたびに増える版番号。
    ///
    /// `cad-app` 側の空間インデックスや描画キャッシュはこの値をキーにして
    /// 無効化を判断する。Undo/Redo でも増えるので、巻き戻しでもキャッシュが正しく捨てられる。
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// 保存されていない変更があるか。
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// 関連づけられたファイルパス。
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    // ---- 変更 -------------------------------------------------------------

    /// コマンドを適用し、成功したら履歴に積む。
    ///
    /// 失敗した場合は履歴に積まず、版番号も進めない。
    ///
    /// # Errors
    ///
    /// コマンドの `execute` が失敗した場合。
    pub fn apply(&mut self, mut command: Box<dyn Command>) -> Result<()> {
        {
            let mut ctx = EditCtx::new(&mut self.entities, &mut self.layers, &mut self.groups);
            command.execute(&mut ctx)?;
        }
        self.history.push(command);
        self.mark_changed();
        Ok(())
    }

    /// 直前の操作を取り消す。取り消した操作の名前を返す。
    ///
    /// # Errors
    ///
    /// コマンドの `undo` が失敗した場合。このときコマンドは履歴に残したままにする。
    pub fn undo(&mut self) -> Result<Option<&'static str>> {
        let Some(mut command) = self.history.pop_for_undo() else {
            return Ok(None);
        };
        let name = command.name();

        let result = {
            let mut ctx = EditCtx::new(&mut self.entities, &mut self.layers, &mut self.groups);
            command.undo(&mut ctx)
        };

        match result {
            Ok(()) => {
                self.history.push_undone(command);
                self.mark_changed();
                Ok(Some(name))
            }
            Err(e) => {
                // 取り消せなかったものは Undo 列へ戻す。
                self.history.push_back_done(command);
                Err(e)
            }
        }
    }

    /// 取り消した操作をやり直す。やり直した操作の名前を返す。
    ///
    /// # Errors
    ///
    /// コマンドの `execute` が失敗した場合。
    pub fn redo(&mut self) -> Result<Option<&'static str>> {
        let Some(mut command) = self.history.pop_for_redo() else {
            return Ok(None);
        };
        let name = command.name();

        let result = {
            let mut ctx = EditCtx::new(&mut self.entities, &mut self.layers, &mut self.groups);
            command.execute(&mut ctx)
        };

        match result {
            Ok(()) => {
                self.history.push_back_done(command);
                self.mark_changed();
                Ok(Some(name))
            }
            Err(e) => Err(e),
        }
    }

    /// Undo 履歴を捨てる。
    ///
    /// 履歴が無くなって初めて、削除済みスロットを安全に回収できる。
    pub fn clear_history(&mut self) {
        self.history.clear();
        self.entities.shrink_after_history_cleared();
    }

    /// 保存済みとして印をつける。
    pub fn mark_saved(&mut self, path: Option<PathBuf>) {
        self.dirty = false;
        if path.is_some() {
            self.path = path;
        }
    }

    fn mark_changed(&mut self) {
        self.revision += 1;
        self.dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{AddEntities, DeleteEntities, MacroCommand};
    use crate::entity::{Entity, Geometry};
    use crate::error::CadError;
    use crate::geom::{Line, Point2};
    use crate::layer::LayerId;

    fn line(x: f64) -> Entity {
        Entity::new(
            Geometry::Line(Line::new(Point2::new(x, 0.0), Point2::new(x, 1.0))),
            LayerId::ZERO,
        )
    }

    fn add(doc: &mut Document, x: f64) {
        doc.apply(Box::new(AddEntities::one("LINE", line(x))))
            .unwrap();
    }

    #[test]
    fn new_document_is_clean_and_empty() {
        let d = Document::new();
        assert!(d.entities().is_empty());
        assert!(!d.is_dirty());
        assert_eq!(d.revision(), 0);
        assert_eq!(d.layers().current(), LayerId::ZERO);
    }

    #[test]
    fn apply_marks_dirty_and_bumps_revision() {
        let mut d = Document::new();
        add(&mut d, 0.0);
        assert_eq!(d.entities().len(), 1);
        assert!(d.is_dirty());
        assert_eq!(d.revision(), 1);
    }

    #[test]
    fn undo_and_redo_restore_entity_count() {
        let mut d = Document::new();
        add(&mut d, 0.0);
        add(&mut d, 1.0);
        assert_eq!(d.entities().len(), 2);

        assert_eq!(d.undo().unwrap(), Some("LINE"));
        assert_eq!(d.entities().len(), 1);

        assert_eq!(d.redo().unwrap(), Some("LINE"));
        assert_eq!(d.entities().len(), 2);
    }

    /// Undo/Redo でも版番号が進むこと（キャッシュ無効化が効くために必要）。
    #[test]
    fn undo_redo_bump_revision() {
        let mut d = Document::new();
        add(&mut d, 0.0);
        let r1 = d.revision();
        d.undo().unwrap();
        assert!(d.revision() > r1);
        let r2 = d.revision();
        d.redo().unwrap();
        assert!(d.revision() > r2);
    }

    /// 削除 → Undo でエンティティ ID が完全に一致すること。これが崩れると
    /// Undo スタックに残る他コマンドの参照が壊れる。
    #[test]
    fn delete_undo_preserves_entity_ids() {
        let mut d = Document::new();
        add(&mut d, 0.0);
        add(&mut d, 1.0);
        let before: Vec<_> = d.entities().ids().collect();

        d.apply(Box::new(DeleteEntities::new("ERASE", before.clone())))
            .unwrap();
        assert!(d.entities().is_empty());

        d.undo().unwrap();
        let after: Vec<_> = d.entities().ids().collect();
        assert_eq!(before, after, "Undo 後も同じ EntityId であること");
    }

    /// 「削除 → Undo → 別の操作 → Undo」でも参照が壊れないこと。
    #[test]
    fn undo_chain_keeps_references_valid() {
        let mut d = Document::new();
        add(&mut d, 0.0);
        let target = d.entities().ids().next().unwrap();

        d.apply(Box::new(DeleteEntities::new("ERASE", vec![target])))
            .unwrap();
        d.undo().unwrap();

        // 元の ID がまだ生きているので、同じ ID を指すコマンドが通る。
        d.apply(Box::new(DeleteEntities::new("ERASE", vec![target])))
            .unwrap();
        assert!(d.entities().is_empty());
        d.undo().unwrap();
        assert!(d.entities().contains(target));
    }

    #[test]
    fn undo_redo_on_empty_history_is_noop() {
        let mut d = Document::new();
        assert_eq!(d.undo().unwrap(), None);
        assert_eq!(d.redo().unwrap(), None);
        assert_eq!(d.revision(), 0);
    }

    /// 新しい操作を適用したら Redo 列は捨てられること。
    #[test]
    fn apply_clears_redo_branch() {
        let mut d = Document::new();
        add(&mut d, 0.0);
        d.undo().unwrap();
        assert!(d.history().can_redo());

        add(&mut d, 5.0);
        assert!(!d.history().can_redo());
    }

    /// 失敗したコマンドは履歴に積まれず、文書も変わらないこと。
    #[test]
    fn failed_command_is_not_recorded() {
        let mut d = Document::new();
        add(&mut d, 0.0);
        let id = d.entities().ids().next().unwrap();
        d.apply(Box::new(DeleteEntities::new("ERASE", vec![id])))
            .unwrap();

        let revision = d.revision();
        let depth = d.history().len();

        // 既に消えている ID を消そうとする。
        let err = d
            .apply(Box::new(DeleteEntities::new("ERASE", vec![id])))
            .unwrap_err();
        assert_eq!(err, CadError::EntityNotFound);
        assert_eq!(d.revision(), revision, "失敗時に版番号を進めない");
        assert_eq!(d.history().len(), depth, "失敗時に履歴へ積まない");
    }

    /// 一部が失敗する削除は、成功した分も巻き戻して原状復帰すること。
    #[test]
    fn partially_failing_delete_rolls_back() {
        let mut d = Document::new();
        add(&mut d, 0.0);
        add(&mut d, 1.0);
        let ids: Vec<_> = d.entities().ids().collect();

        // 1 つ目は生きているが 2 つ目は存在しない ID を混ぜる。
        let dead = {
            let mut d2 = Document::new();
            d2.apply(Box::new(AddEntities::one("LINE", line(0.0))))
                .unwrap();
            let dead_id = d2.entities().ids().next().unwrap();
            d2.apply(Box::new(DeleteEntities::new("ERASE", vec![dead_id])))
                .unwrap();
            dead_id
        };

        let err = d
            .apply(Box::new(DeleteEntities::new("ERASE", vec![ids[0], dead])))
            .unwrap_err();
        assert_eq!(err, CadError::EntityNotFound);
        assert_eq!(
            d.entities().len(),
            2,
            "失敗した削除で要素が減っていないこと"
        );
    }

    #[test]
    fn macro_command_undoes_in_reverse() {
        let mut d = Document::new();
        d.apply(Box::new(MacroCommand::new(
            "RECTANGLE",
            vec![
                Box::new(AddEntities::one("LINE", line(0.0))),
                Box::new(AddEntities::one("LINE", line(1.0))),
            ],
        )))
        .unwrap();
        assert_eq!(d.entities().len(), 2);

        assert_eq!(d.undo().unwrap(), Some("RECTANGLE"));
        assert!(d.entities().is_empty(), "まとめて 1 回で取り消せること");

        d.redo().unwrap();
        assert_eq!(d.entities().len(), 2);
    }

    /// 履歴の上限を超えても壊れないこと。指示書の要求は最低 100。
    #[test]
    fn history_depth_is_at_least_100() {
        const { assert!(UndoStack::DEFAULT_LIMIT >= 100) };

        let mut d = Document::new();
        for i in 0..(UndoStack::DEFAULT_LIMIT + 10) {
            add(&mut d, f64::from(u16::try_from(i).unwrap()));
        }
        assert_eq!(d.history().len(), UndoStack::DEFAULT_LIMIT);

        // 上限ぶんは確実に巻き戻せる。
        for _ in 0..UndoStack::DEFAULT_LIMIT {
            assert!(d.undo().unwrap().is_some());
        }
        assert_eq!(d.undo().unwrap(), None);
    }

    #[test]
    fn mark_saved_clears_dirty() {
        let mut d = Document::new();
        add(&mut d, 0.0);
        assert!(d.is_dirty());
        d.mark_saved(Some(PathBuf::from("/tmp/a.dxf")));
        assert!(!d.is_dirty());
        assert_eq!(d.path().unwrap().to_str().unwrap(), "/tmp/a.dxf");
    }

    #[test]
    fn bbox_tracks_entities() {
        let mut d = Document::new();
        assert!(d.bbox().is_empty());
        add(&mut d, 0.0);
        add(&mut d, 10.0);
        let b = d.bbox();
        assert!(b.contains(Point2::new(0.0, 0.0)));
        assert!(b.contains(Point2::new(10.0, 1.0)));
    }
}
