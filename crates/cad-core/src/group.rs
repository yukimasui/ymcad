//! グループ（AutoCAD の GROUP）。
//!
//! 複数のエンティティに名前を付けてひとまとまりに扱う。
//! グループの一員をクリックすると全体が選択される（AutoCAD の既定）。
//!
//! # 所属をどちらが持つか
//!
//! 所属は **エンティティ側の [`Entity::group`](crate::Entity) が持つ**。
//! `GroupTable` は名前と ID の対応だけを持ち、メンバーの一覧は持たない。
//!
//! グループ側にメンバー一覧を持たせると、エンティティの削除・Undo のたびに
//! 両方を辻褄が合うように更新する必要があり、片方だけ直し損ねる事故が起きる。
//! エンティティ側だけが真実なら、メンバーは走査して求めればよく、
//! 削除や Undo で自動的に整合が保たれる。
//!
//! # DXF について
//!
//! DXF R12 に `GROUP` は無い（R13 以降の `OBJECTS` セクション）。
//! 保存するとグループ情報は失われ、書き出し時に警告が出る（ADR-0021）。

use std::collections::BTreeMap;

use crate::error::{CadError, Result};

/// グループの識別子。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GroupId(u32);

impl GroupId {
    /// 内部表現。
    #[must_use]
    pub const fn index(self) -> u32 {
        self.0
    }
}

/// グループ 1 つぶんの属性。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    /// グループ名。
    pub name: String,
}

impl Group {
    /// 名前を指定して作る。
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

/// グループの一覧。
///
/// [`LayerTable`](crate::LayerTable) と同じ作りにしている。
/// 変更系は `pub(crate)` に絞り、[`EditCtx`](crate::command::EditCtx) 経由でしか触れない。
#[derive(Clone, Debug, Default)]
pub struct GroupTable {
    groups: Vec<Option<Group>>,
    by_name: BTreeMap<String, GroupId>,
}

impl GroupTable {
    /// 空の表。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    // ---- 参照系 -----------------------------------------------------------

    /// ID からグループを引く。
    #[must_use]
    pub fn get(&self, id: GroupId) -> Option<&Group> {
        self.groups.get(id.index() as usize)?.as_ref()
    }

    /// 名前から ID を引く。
    #[must_use]
    pub fn by_name(&self, name: &str) -> Option<GroupId> {
        self.by_name.get(name).copied()
    }

    /// ID 昇順で走査する。
    pub fn iter(&self) -> impl Iterator<Item = (GroupId, &Group)> + '_ {
        self.groups.iter().enumerate().filter_map(|(i, g)| {
            let group = g.as_ref()?;
            let index = u32::try_from(i).ok()?;
            Some((GroupId(index), group))
        })
    }

    /// グループ数。
    #[must_use]
    pub fn len(&self) -> usize {
        self.groups.iter().filter(|g| g.is_some()).count()
    }

    /// グループが 1 つも無いか。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// まだ使われていないグループ名を作る（`グループ1`、`グループ2`, …）。
    ///
    /// AutoCAD の名前なしグループ（`*A1` など）に相当するが、
    /// 一覧で読めるように日本語の連番にしている。
    #[must_use]
    pub fn next_default_name(&self) -> String {
        let mut n = 1u32;
        loop {
            let name = format!("グループ{n}");
            if !self.by_name.contains_key(&name) {
                return name;
            }
            n += 1;
        }
    }

    // ---- 変更系（`pub(crate)` — EditCtx からのみ到達可能） ----------------

    /// グループを追加する。同名があればその ID を返す。
    pub(crate) fn insert(&mut self, group: Group) -> GroupId {
        if let Some(existing) = self.by_name.get(&group.name) {
            return *existing;
        }
        let index = u32::try_from(self.groups.len()).expect("グループ数が u32 を超えました");
        let id = GroupId(index);
        self.by_name.insert(group.name.clone(), id);
        self.groups.push(Some(group));
        id
    }

    /// グループを取り除く。
    ///
    /// # Errors
    ///
    /// 存在しない ID の場合 [`CadError::GroupNotFound`]。
    pub(crate) fn remove(&mut self, id: GroupId) -> Result<Group> {
        let slot = self
            .groups
            .get_mut(id.index() as usize)
            .ok_or(CadError::GroupNotFound)?;
        let group = slot.take().ok_or(CadError::GroupNotFound)?;
        self.by_name.remove(&group.name);
        Ok(group)
    }

    /// 取り除いたグループを **元の ID のまま** 戻す。Undo で使う。
    ///
    /// # Errors
    ///
    /// スロットが埋まっている場合 [`CadError::SlotOccupied`]。
    pub(crate) fn restore(&mut self, id: GroupId, group: Group) -> Result<()> {
        let index = id.index() as usize;
        if index >= self.groups.len() {
            self.groups.resize(index + 1, None);
        }
        if self.groups[index].is_some() {
            return Err(CadError::SlotOccupied);
        }
        self.by_name.insert(group.name.clone(), id);
        self.groups[index] = Some(group);
        Ok(())
    }

    /// グループ名を変更する。古い名前を返す。
    ///
    /// 衝突の検査は呼び出し側（コマンド）の責務。[`LayerTable`](crate::LayerTable) と同じ約束。
    pub(crate) fn rename(&mut self, id: GroupId, new_name: impl Into<String>) -> Option<String> {
        let new_name = new_name.into();
        let group = self.groups.get_mut(id.index() as usize)?.as_mut()?;
        let old = std::mem::replace(&mut group.name, new_name.clone());
        self.by_name.remove(&old);
        self.by_name.insert(new_name, id);
        Some(old)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_table_is_empty() {
        let t = GroupTable::new();
        assert!(t.is_empty());
        assert_eq!(t.len(), 0);
        assert!(t.by_name("なし").is_none());
    }

    #[test]
    fn insert_is_idempotent_by_name() {
        let mut t = GroupTable::new();
        let a = t.insert(Group::new("壁"));
        let b = t.insert(Group::new("壁"));
        assert_eq!(a, b, "同名グループを二重に作らないこと");
        assert_eq!(t.len(), 1);
        assert_eq!(t.by_name("壁"), Some(a));
    }

    #[test]
    fn remove_drops_the_name_index() {
        let mut t = GroupTable::new();
        let id = t.insert(Group::new("壁"));
        let removed = t.remove(id).unwrap();
        assert_eq!(removed.name, "壁");
        assert!(t.get(id).is_none());
        assert!(t.by_name("壁").is_none(), "名前の索引も消えること");
        assert_eq!(t.remove(id), Err(CadError::GroupNotFound));
    }

    /// Undo の要。取り除いたグループを元の ID のまま戻せること。
    #[test]
    fn restore_preserves_the_group_id() {
        let mut t = GroupTable::new();
        let a = t.insert(Group::new("A"));
        let b = t.insert(Group::new("B"));
        let removed = t.remove(a).unwrap();

        t.restore(a, removed).unwrap();
        assert_eq!(t.by_name("A"), Some(a), "同じ ID で戻ること");
        assert_eq!(t.by_name("B"), Some(b), "他のグループは無事");
        assert_eq!(t.len(), 2);
    }

    #[test]
    fn restore_into_occupied_slot_is_rejected() {
        let mut t = GroupTable::new();
        let id = t.insert(Group::new("A"));
        assert_eq!(t.restore(id, Group::new("B")), Err(CadError::SlotOccupied));
    }

    #[test]
    fn rename_updates_the_name_index() {
        let mut t = GroupTable::new();
        let id = t.insert(Group::new("旧"));
        let old = t.rename(id, "新").unwrap();
        assert_eq!(old, "旧");
        assert!(t.by_name("旧").is_none(), "古い名前は引けなくなる");
        assert_eq!(t.by_name("新"), Some(id));
        assert_eq!(t.get(id).unwrap().name, "新");
    }

    #[test]
    fn iteration_is_id_order() {
        let mut t = GroupTable::new();
        let a = t.insert(Group::new("A"));
        let b = t.insert(Group::new("B"));
        let ids: Vec<_> = t.iter().map(|(id, _)| id).collect();
        assert_eq!(ids, vec![a, b]);
    }

    /// 既定名は使われていないものを選ぶこと。
    #[test]
    fn default_name_skips_taken_ones() {
        let mut t = GroupTable::new();
        assert_eq!(t.next_default_name(), "グループ1");
        t.insert(Group::new("グループ1"));
        assert_eq!(t.next_default_name(), "グループ2");
        t.insert(Group::new("グループ2"));
        assert_eq!(t.next_default_name(), "グループ3");
    }
}
