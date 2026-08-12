//! 世代つきアリーナによるエンティティストア。
//!
//! # なぜ `slotmap` を使わないか
//!
//! `slotmap` には **「呼び出し側が指定したキーで再挿入する API」が無い**。
//! `remove(k)` した後に挿入すると必ず別のキーになる。
//! CAD では `ERASE` → `UNDO` が日常操作なので、Undo でエンティティを戻したときに
//! ID が変わると、Undo スタックに積まれた他のコマンド（「エンティティ k を移動」など）の
//! 参照が一斉に壊れる。
//!
//! そのため [`EntityStore::restore`] で **元の `EntityId` のまま**書き戻せる自前実装にした。
//! 併せて `slotmap` の `iter()` は順序が未定義だが、こちらは常にスロット昇順
//! （= 作成順 = CAD の描画順）で走査できる。
//!
//! # スロット割り当ての方針
//!
//! **スロット番号は文書の生存期間中に再利用しない（単調増加）。**
//! 空いたスロットを使い回すと、まだ Undo スタックに残っているコマンドが参照している
//! スロットを別のエンティティが占有し、[`EntityStore::restore`] が失敗しうる。
//! 2D 作図のセッションで消費するメモリは無視できるため、単調増加で問題ない。
//! スロットの回収は Undo 履歴を捨てるとき（新規作成・ファイル読み込み）にのみ行う。

use crate::entity::{Entity, EntityId};
use crate::error::{CadError, Result};
use crate::geom::Aabb;

/// アリーナのスロット。
///
/// 空きスロットが世代番号を持つ必要はない。スロットを新規挿入で再利用しないため、
/// 空きスロットへ入りうるのは [`EntityStore::restore`] で指定された ID だけであり、
/// そのときの世代は呼び出し側の `EntityId` が持っているから。
#[derive(Debug, Clone)]
enum Slot {
    /// 空き。
    Vacant,
    /// 使用中。
    Occupied { generation: u32, entity: Entity },
}

/// エンティティの集合。
///
/// **書き込み系のメソッドはすべて `pub(crate)`。** `cad-app` は別クレートなので
/// Rust のモジュール可視性により名前を呼ぶことすらできず、
/// [`crate::command::EditCtx`] 経由でしか変更できない。
#[derive(Debug, Default, Clone)]
pub struct EntityStore {
    slots: Vec<Slot>,
    alive: usize,
}

impl EntityStore {
    /// 空のストア。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    // ---- 参照系（公開） ---------------------------------------------------

    /// 生存しているエンティティ数。
    #[must_use]
    pub fn len(&self) -> usize {
        self.alive
    }

    /// エンティティが 1 つも無いか。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.alive == 0
    }

    /// ID に対応するエンティティ。世代が一致しなければ `None`。
    #[must_use]
    pub fn get(&self, id: EntityId) -> Option<&Entity> {
        match self.slots.get(id.index() as usize)? {
            Slot::Occupied { generation, entity } if *generation == id.generation() => Some(entity),
            _ => None,
        }
    }

    /// ID が生存しているか。
    #[must_use]
    pub fn contains(&self, id: EntityId) -> bool {
        self.get(id).is_some()
    }

    /// 生存しているエンティティをスロット昇順（= 作成順 = 描画順）で走査する。
    pub fn iter(&self) -> impl Iterator<Item = (EntityId, &Entity)> + '_ {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(i, slot)| match slot {
                Slot::Occupied { generation, entity } => {
                    let index = u32::try_from(i).ok()?;
                    Some((EntityId::new(index, *generation), entity))
                }
                Slot::Vacant => None,
            })
    }

    /// 生存している ID を作成順に走査する。
    pub fn ids(&self) -> impl Iterator<Item = EntityId> + '_ {
        self.iter().map(|(id, _)| id)
    }

    /// 全エンティティを含む境界ボックス。空なら [`Aabb::EMPTY`]。
    ///
    /// **無限に伸びる図形（作図線）は含めない。** 含めると図面範囲が無限になり、
    /// ZOOM EXTENTS が意味を失う。AutoCAD も作図線を図面範囲に含めない。
    #[must_use]
    pub fn bbox(&self) -> Aabb {
        self.iter()
            .filter(|(_, e)| e.geom.is_bounded())
            .fold(Aabb::EMPTY, |acc, (_, e)| acc.union(e.bbox()))
    }

    // ---- 変更系（`pub(crate)` — EditCtx からのみ到達可能） ----------------

    /// 新しいエンティティを追加する。
    pub(crate) fn insert(&mut self, entity: Entity) -> EntityId {
        // スロットは再利用しない（モジュールドキュメントの方針）。常に末尾へ足す。
        let index =
            u32::try_from(self.slots.len()).expect("エンティティ数が u32 の範囲を超えました");
        self.slots.push(Slot::Occupied {
            generation: 0,
            entity,
        });
        self.alive += 1;
        EntityId::new(index, 0)
    }

    /// 変更のために可変参照を取る。
    pub(crate) fn get_mut(&mut self, id: EntityId) -> Option<&mut Entity> {
        match self.slots.get_mut(id.index() as usize)? {
            Slot::Occupied { generation, entity } if *generation == id.generation() => Some(entity),
            _ => None,
        }
    }

    /// エンティティを取り除く。取り除いた中身を返す（Undo で書き戻すため）。
    pub(crate) fn remove(&mut self, id: EntityId) -> Result<Entity> {
        let slot = self
            .slots
            .get_mut(id.index() as usize)
            .ok_or(CadError::EntityNotFound)?;

        match slot {
            Slot::Occupied { generation, .. } if *generation == id.generation() => {
                // 空きにした時点で、この ID を含むあらゆる参照が「死んだ」と判定される。
                let old = std::mem::replace(slot, Slot::Vacant);
                self.alive -= 1;
                match old {
                    Slot::Occupied { entity, .. } => Ok(entity),
                    Slot::Vacant => unreachable!("直前に Occupied と確認済み"),
                }
            }
            _ => Err(CadError::EntityNotFound),
        }
    }

    /// 削除したエンティティを **元の `EntityId` のまま** 書き戻す。
    ///
    /// Undo の要。これがあるおかげで「削除 → Undo」が恒等操作になり、
    /// Undo スタックに残る他コマンドの ID 参照が壊れない。
    ///
    /// # Errors
    ///
    /// スロットが既に使用中の場合 [`CadError::SlotOccupied`]。
    /// スロット番号を再利用しない方針を守っている限り発生しない。
    pub(crate) fn restore(&mut self, id: EntityId, entity: Entity) -> Result<()> {
        let index = id.index() as usize;
        // 削除済みスロットは Vacant として残っているはずだが、
        // 履歴を捨てた後などスロット自体が消えている場合に備えて伸長する。
        if index >= self.slots.len() {
            self.slots.resize(index + 1, Slot::Vacant);
        }
        let slot = &mut self.slots[index];
        match slot {
            Slot::Occupied { .. } => Err(CadError::SlotOccupied),
            Slot::Vacant => {
                *slot = Slot::Occupied {
                    generation: id.generation(),
                    entity,
                };
                self.alive += 1;
                Ok(())
            }
        }
    }

    /// 空きスロットを回収する。**Undo 履歴を捨てるときにだけ**呼ぶこと。
    ///
    /// 履歴が残っている状態で呼ぶと、まだ参照されているスロットが詰められて
    /// [`Self::restore`] が別のエンティティを上書きしうる。
    pub(crate) fn shrink_after_history_cleared(&mut self) {
        while matches!(self.slots.last(), Some(Slot::Vacant)) {
            self.slots.pop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::Geometry;
    use crate::geom::{Line, Point2};
    use crate::layer::LayerId;

    fn line_entity(x: f64) -> Entity {
        Entity::new(
            Geometry::Line(Line::new(Point2::new(x, 0.0), Point2::new(x, 1.0))),
            LayerId::ZERO,
        )
    }

    #[test]
    fn insert_and_get() {
        let mut s = EntityStore::new();
        let id = s.insert(line_entity(1.0));
        assert_eq!(s.len(), 1);
        assert!(s.contains(id));
        assert!(s.get(id).is_some());
    }

    #[test]
    fn removed_id_becomes_dead() {
        let mut s = EntityStore::new();
        let id = s.insert(line_entity(1.0));
        s.remove(id).unwrap();
        assert!(!s.contains(id));
        assert_eq!(s.len(), 0);
        assert_eq!(s.remove(id), Err(CadError::EntityNotFound));
    }

    /// Undo の中核。削除して戻したとき ID が完全に一致すること。
    #[test]
    fn restore_preserves_entity_id() {
        let mut s = EntityStore::new();
        let id = s.insert(line_entity(1.0));
        let e = s.remove(id).unwrap();
        s.restore(id, e).unwrap();

        assert!(s.contains(id), "Undo 後も同じ EntityId で参照できること");
        assert_eq!(s.len(), 1);
    }

    /// 削除 → Undo を挟んでも、他のエンティティの ID が壊れないこと。
    #[test]
    fn restore_does_not_disturb_other_ids() {
        let mut s = EntityStore::new();
        let a = s.insert(line_entity(1.0));
        let b = s.insert(line_entity(2.0));
        let c = s.insert(line_entity(3.0));

        let removed = s.remove(b).unwrap();
        assert!(s.contains(a) && s.contains(c));
        s.restore(b, removed).unwrap();

        assert!(s.contains(a) && s.contains(b) && s.contains(c));
        assert_eq!(s.len(), 3);
    }

    /// スロットを再利用しないこと。再利用すると restore が失敗しうる。
    #[test]
    fn slots_are_not_reused() {
        let mut s = EntityStore::new();
        let a = s.insert(line_entity(1.0));
        let removed = s.remove(a).unwrap();
        let b = s.insert(line_entity(2.0));

        assert_ne!(
            a.index(),
            b.index(),
            "削除したスロットを新規挿入で再利用してはいけない"
        );
        // だからこそ元のスロットへ戻せる。
        s.restore(a, removed).unwrap();
        assert!(s.contains(a) && s.contains(b));
    }

    #[test]
    fn restore_into_occupied_slot_is_rejected() {
        let mut s = EntityStore::new();
        let id = s.insert(line_entity(1.0));
        assert_eq!(s.restore(id, line_entity(9.0)), Err(CadError::SlotOccupied));
    }

    /// 削除後に同じスロットへ新しい世代で入ると、古い ID は死んだままであること。
    #[test]
    fn stale_id_stays_dead_after_restore_with_new_generation() {
        let mut s = EntityStore::new();
        let old = s.insert(line_entity(1.0));
        s.remove(old).unwrap();

        // 世代を進めた ID で書き戻す（remove が世代を +1 している）。
        let fresh = EntityId::new(old.index(), old.generation() + 1);
        s.restore(fresh, line_entity(2.0)).unwrap();

        assert!(s.contains(fresh));
        assert!(!s.contains(old), "古い世代の ID は死んだままであること");
    }

    #[test]
    fn iteration_is_creation_order() {
        let mut s = EntityStore::new();
        let ids: Vec<_> = (0..5)
            .map(|i| s.insert(line_entity(f64::from(i))))
            .collect();
        s.remove(ids[2]).unwrap();

        let seen: Vec<_> = s.ids().collect();
        assert_eq!(seen, vec![ids[0], ids[1], ids[3], ids[4]]);
    }

    #[test]
    fn bbox_of_empty_store_is_empty() {
        assert!(EntityStore::new().bbox().is_empty());
    }

    #[test]
    fn bbox_covers_all_entities() {
        let mut s = EntityStore::new();
        s.insert(line_entity(0.0));
        s.insert(line_entity(10.0));
        let b = s.bbox();
        assert!(b.contains(Point2::new(0.0, 0.0)));
        assert!(b.contains(Point2::new(10.0, 1.0)));
    }

    #[test]
    fn get_mut_respects_generation() {
        let mut s = EntityStore::new();
        let id = s.insert(line_entity(1.0));
        assert!(s.get_mut(id).is_some());

        let stale = EntityId::new(id.index(), id.generation() + 1);
        assert!(s.get_mut(stale).is_none());
    }

    /// 作図線は図面範囲に含めないこと。
    ///
    /// 含めると `Document::bbox()` が無限になり ZOOM EXTENTS が意味を失う。
    #[test]
    fn bbox_ignores_unbounded_geometry() {
        use crate::geom::{Vec2, Xline};

        let mut s = EntityStore::new();
        s.insert(line_entity(0.0));
        s.insert(line_entity(10.0));
        let bounded = s.bbox();

        s.insert(Entity::new(
            Geometry::Xline(Xline::new(Point2::ORIGIN, Vec2::new(1.0, 1.0)).unwrap()),
            LayerId::ZERO,
        ));

        assert_eq!(s.bbox(), bounded, "作図線を足しても図面範囲は変わらない");
        assert!(!s.bbox().is_unbounded());
    }

    /// 作図線しか無い図面の範囲は空であること。
    #[test]
    fn bbox_of_only_unbounded_geometry_is_empty() {
        use crate::geom::{Vec2, Xline};

        let mut s = EntityStore::new();
        s.insert(Entity::new(
            Geometry::Xline(Xline::new(Point2::ORIGIN, Vec2::X).unwrap()),
            LayerId::ZERO,
        ));
        assert!(s.bbox().is_empty(), "{:?}", s.bbox());
    }
}
