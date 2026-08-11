//! レイヤ（画層）。
//!
//! Phase 1 では色・表示/非表示・ロックまで。線種は Phase 5 で追加する。

use std::collections::BTreeMap;

use crate::entity::Entity;

/// レイヤの識別子。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LayerId(u32);

impl LayerId {
    /// DXF で必ず存在する既定レイヤ `"0"`。削除できない。
    pub const ZERO: Self = Self(0);

    /// 内部表現。
    #[must_use]
    pub const fn index(self) -> u32 {
        self.0
    }
}

/// AutoCAD Color Index。DXF のグループコード 62 に対応する。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AciColor(pub u8);

impl AciColor {
    /// 既定色（白 / 黒 — 背景に応じて反転する慣習）。
    pub const WHITE: Self = Self(7);
    /// 赤。
    pub const RED: Self = Self(1);

    /// RGB 値。ACI の標準 7 色のみ厳密に、それ以外は簡易的に割り当てる。
    ///
    /// `cad-core` は UI に依存できないので、色は素の `(u8, u8, u8)` で返す。
    #[must_use]
    pub fn rgb(self) -> (u8, u8, u8) {
        match self.0 {
            1 => (255, 0, 0),
            2 => (255, 255, 0),
            3 => (0, 255, 0),
            4 => (0, 255, 255),
            5 => (0, 0, 255),
            6 => (255, 0, 255),
            7 => (255, 255, 255),
            8 => (128, 128, 128),
            9 => (192, 192, 192),
            // 10 以降は未実装。視認できる灰色を返しておく。
            _ => (160, 160, 160),
        }
    }
}

/// エンティティの色指定。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ColorSpec {
    /// レイヤの色に従う。
    #[default]
    ByLayer,
    /// 個別指定。
    Aci(AciColor),
}

/// レイヤ 1 つぶんの属性。
#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    /// レイヤ名。DXF R12 では大文字・空白なしが安全。
    pub name: String,
    /// レイヤの色。
    pub color: AciColor,
    /// 表示するか。非表示のレイヤは描画からも選択からも除外する。
    pub visible: bool,
    /// ロックされているか。ロック中は選択も編集もできない。
    pub locked: bool,
}

impl Layer {
    /// 既定の属性で作る。
    #[must_use]
    pub fn new(name: impl Into<String>, color: AciColor) -> Self {
        Self {
            name: name.into(),
            color,
            visible: true,
            locked: false,
        }
    }

    /// 選択・編集の対象になりうるか。
    #[must_use]
    pub fn is_editable(&self) -> bool {
        self.visible && !self.locked
    }
}

/// レイヤの一覧と現在レイヤ。
///
/// [`EntityStore`](crate::entity::EntityStore) と同様、変更系は `pub(crate)` に絞り
/// [`EditCtx`](crate::command::EditCtx) 経由でしか触れないようにしている。
#[derive(Clone, Debug)]
pub struct LayerTable {
    layers: Vec<Option<Layer>>,
    by_name: BTreeMap<String, LayerId>,
    current: LayerId,
}

impl Default for LayerTable {
    fn default() -> Self {
        Self::new()
    }
}

impl LayerTable {
    /// レイヤ `"0"` だけを持つ表を作る。
    #[must_use]
    pub fn new() -> Self {
        let zero = Layer::new("0", AciColor::WHITE);
        let mut by_name = BTreeMap::new();
        by_name.insert(zero.name.clone(), LayerId::ZERO);
        Self {
            layers: vec![Some(zero)],
            by_name,
            current: LayerId::ZERO,
        }
    }

    // ---- 参照系 -----------------------------------------------------------

    /// ID からレイヤを引く。
    #[must_use]
    pub fn get(&self, id: LayerId) -> Option<&Layer> {
        self.layers.get(id.index() as usize)?.as_ref()
    }

    /// 名前から ID を引く。
    #[must_use]
    pub fn by_name(&self, name: &str) -> Option<LayerId> {
        self.by_name.get(name).copied()
    }

    /// 現在レイヤ。新規に作図した要素はここへ入る。
    #[must_use]
    pub fn current(&self) -> LayerId {
        self.current
    }

    /// ID 昇順で走査する。
    pub fn iter(&self) -> impl Iterator<Item = (LayerId, &Layer)> + '_ {
        self.layers.iter().enumerate().filter_map(|(i, l)| {
            let layer = l.as_ref()?;
            let index = u32::try_from(i).ok()?;
            Some((LayerId(index), layer))
        })
    }

    /// レイヤ数。
    #[must_use]
    pub fn len(&self) -> usize {
        self.layers.iter().filter(|l| l.is_some()).count()
    }

    /// 常に `"0"` があるので空にはならない。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        false
    }

    /// エンティティの実効的な色を解決する。
    ///
    /// レイヤが見つからない場合は既定色を返す（描画を止めないため）。
    #[must_use]
    pub fn resolve_color(&self, entity: &Entity) -> AciColor {
        match entity.color {
            ColorSpec::Aci(c) => c,
            ColorSpec::ByLayer => self.get(entity.layer).map_or(AciColor::WHITE, |l| l.color),
        }
    }

    /// エンティティが描画対象か（所属レイヤが表示中か）。
    #[must_use]
    pub fn is_entity_visible(&self, entity: &Entity) -> bool {
        self.get(entity.layer).is_some_and(|l| l.visible)
    }

    /// エンティティが選択・編集の対象か。
    #[must_use]
    pub fn is_entity_editable(&self, entity: &Entity) -> bool {
        self.get(entity.layer).is_some_and(Layer::is_editable)
    }

    // ---- 変更系（`pub(crate)` — EditCtx からのみ到達可能） ----------------

    /// レイヤを追加する。同名があればその ID を返す。
    pub(crate) fn insert(&mut self, layer: Layer) -> LayerId {
        if let Some(existing) = self.by_name.get(&layer.name) {
            return *existing;
        }
        let index = u32::try_from(self.layers.len()).expect("レイヤ数が u32 を超えました");
        let id = LayerId(index);
        self.by_name.insert(layer.name.clone(), id);
        self.layers.push(Some(layer));
        id
    }

    /// 変更のために可変参照を取る。
    pub(crate) fn get_mut(&mut self, id: LayerId) -> Option<&mut Layer> {
        self.layers.get_mut(id.index() as usize)?.as_mut()
    }

    /// 現在レイヤを切り替える。存在しない ID は無視する。
    pub(crate) fn set_current(&mut self, id: LayerId) {
        if self.get(id).is_some() {
            self.current = id;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::{Entity, Geometry};
    use crate::geom::{Line, Point2};

    fn entity_on(layer: LayerId) -> Entity {
        Entity::new(
            Geometry::Line(Line::new(Point2::ORIGIN, Point2::new(1.0, 0.0))),
            layer,
        )
    }

    #[test]
    fn new_table_has_layer_zero_as_current() {
        let t = LayerTable::new();
        assert_eq!(t.current(), LayerId::ZERO);
        assert_eq!(t.get(LayerId::ZERO).unwrap().name, "0");
        assert_eq!(t.by_name("0"), Some(LayerId::ZERO));
        assert_eq!(t.len(), 1);
    }

    #[test]
    fn insert_is_idempotent_by_name() {
        let mut t = LayerTable::new();
        let a = t.insert(Layer::new("WALL", AciColor::RED));
        let b = t.insert(Layer::new("WALL", AciColor::WHITE));
        assert_eq!(a, b, "同名レイヤを二重に作らないこと");
        assert_eq!(t.len(), 2);
    }

    #[test]
    fn color_falls_back_to_layer() {
        let mut t = LayerTable::new();
        let wall = t.insert(Layer::new("WALL", AciColor::RED));
        let e = entity_on(wall);
        assert_eq!(t.resolve_color(&e), AciColor::RED);
    }

    #[test]
    fn explicit_color_overrides_layer() {
        let mut t = LayerTable::new();
        let wall = t.insert(Layer::new("WALL", AciColor::RED));
        let mut e = entity_on(wall);
        e.color = ColorSpec::Aci(AciColor(3));
        assert_eq!(t.resolve_color(&e), AciColor(3));
    }

    #[test]
    fn hidden_layer_hides_and_disables_entity() {
        let mut t = LayerTable::new();
        let id = t.insert(Layer::new("HIDDEN", AciColor::WHITE));
        t.get_mut(id).unwrap().visible = false;

        let e = entity_on(id);
        assert!(!t.is_entity_visible(&e));
        assert!(!t.is_entity_editable(&e));
    }

    #[test]
    fn locked_layer_is_visible_but_not_editable() {
        let mut t = LayerTable::new();
        let id = t.insert(Layer::new("LOCKED", AciColor::WHITE));
        t.get_mut(id).unwrap().locked = true;

        let e = entity_on(id);
        assert!(t.is_entity_visible(&e));
        assert!(!t.is_entity_editable(&e));
    }

    #[test]
    fn set_current_ignores_unknown_layer() {
        let mut t = LayerTable::new();
        t.set_current(LayerId(999));
        assert_eq!(t.current(), LayerId::ZERO);
    }

    #[test]
    fn iteration_is_id_order() {
        let mut t = LayerTable::new();
        let a = t.insert(Layer::new("A", AciColor::WHITE));
        let b = t.insert(Layer::new("B", AciColor::WHITE));
        let ids: Vec<_> = t.iter().map(|(id, _)| id).collect();
        assert_eq!(ids, vec![LayerId::ZERO, a, b]);
    }

    #[test]
    fn aci_standard_colors() {
        assert_eq!(AciColor::RED.rgb(), (255, 0, 0));
        assert_eq!(AciColor::WHITE.rgb(), (255, 255, 255));
    }
}
