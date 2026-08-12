//! コンポーネントインスタンスの解決結果のキャッシュ。
//!
//! # なぜ `Document` に入れないか
//!
//! インスタンスをワールド座標の図形列へ展開するのは、パラメータの評価を含むので重い。
//! 描画・ピック・矩形選択で毎フレーム何度も必要になるが、
//! **これは派生データなので `Document` には入れない**（ADR-0011）。
//!
//! `Document::revision()` をキーに再構築する。`snap.rs` の [`SpatialIndex`] と
//! まったく同じ仕組みで、
//!
//! - すべてのコマンドがキャッシュの更新を意識しなくてよい
//! - Undo / Redo でも revision が進むので、巻き戻しで自動的に無効化される
//! - **定義を編集すると revision が進むので、全インスタンスが自動的に追従する**
//!
//! 最後の点が「定義を編集すると全インスタンスが変わる」の実装そのもの。
//! インスタンス側を書き換える処理はどこにも無い。
//!
//! [`SpatialIndex`]: cad_core::snap::index::SpatialIndex

use std::collections::HashMap;

use cad_core::{Document, EntityId, Geometry};

/// 解決済みインスタンスのキャッシュ。
#[derive(Debug, Default)]
pub struct ResolvedInstances {
    /// インスタンスの `EntityId` → ワールド座標の図形列。
    ///
    /// インスタンス以外のエンティティは入れない（そのまま使えるので不要）。
    map: HashMap<EntityId, Vec<Geometry>>,
    /// キャッシュを作ったときの図面の版番号。
    revision: u64,
    /// まだ一度も作られていないか。
    valid: bool,
}

impl ResolvedInstances {
    /// 空のキャッシュ。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// `id` の解決結果を返す。
    ///
    /// インスタンスでないエンティティ、存在しない ID では `None`。
    ///
    /// **[`Self::refresh`] を先に呼ぶこと。** ここでは版を確認しない。
    /// 描画ループの中で `&mut self` を取ると、`Document` の借用と衝突して
    /// 毎フレーム複製する羽目になる。更新と読み取りを分けてある。
    #[must_use]
    pub fn get(&self, id: EntityId) -> Option<&[Geometry]> {
        self.map.get(&id).map(Vec::as_slice)
    }

    /// 版が変わっていれば作り直す。
    ///
    /// 描画・ピック・選択の前に 1 回呼ぶ。何度呼んでも安い（版の比較だけ）。
    pub fn refresh(&mut self, doc: &Document) {
        if self.valid && self.revision == doc.revision() {
            return;
        }
        self.map.clear();
        for (id, entity) in doc.entities().iter() {
            if let Geometry::Instance(i) = &entity.geom {
                self.map
                    .insert(id, cad_core::component::resolve(i, doc.definitions()));
            }
        }
        self.revision = doc.revision();
        self.valid = true;
    }

    /// キャッシュしているインスタンスの数。
    #[cfg(test)]
    fn len(&self) -> usize {
        self.map.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cad_core::command::{AddEntities, DefineComponent, InsertInstance};
    use cad_core::component::Placement;
    use cad_core::geom::{Line, Point2};
    use cad_core::{Entity, LayerId};

    fn line(x: f64) -> Entity {
        Entity::new(
            Geometry::Line(Line::new(Point2::new(x, 0.0), Point2::new(x + 1.0, 0.0))),
            LayerId::ZERO,
        )
    }

    /// 線分 1 本を含む定義と、その配置を 1 つ持つ図面。
    fn doc_with_one_instance() -> (Document, EntityId) {
        let mut doc = Document::new();
        doc.apply(Box::new(DefineComponent::new(
            "COMPONENT",
            "部品",
            Point2::ORIGIN,
            vec![line(0.0)],
        )))
        .expect("定義を作れるはず");
        let def = doc.definitions().by_name("部品").expect("作ったはず");
        doc.apply(Box::new(InsertInstance::new(
            "INSERT",
            def,
            Placement::at(Point2::new(10.0, 0.0)),
            LayerId::ZERO,
        )))
        .expect("配置できるはず");
        let id = doc.entities().ids().next().expect("1 件あるはず");
        (doc, id)
    }

    #[test]
    fn instance_resolves_to_its_contents() {
        let (doc, id) = doc_with_one_instance();
        let mut cache = ResolvedInstances::new();

        cache.refresh(&doc);
        let resolved = cache.get(id).expect("インスタンスのはず");
        assert_eq!(resolved.len(), 1, "中身は線分 1 本");
        let Geometry::Line(l) = &resolved[0] else {
            panic!("線分のはず: {:?}", resolved[0]);
        };
        // 基点 (0,0) の定義を (10,0) へ置いたので、線分も 10 ずれる。
        assert!((l.a.x - 10.0).abs() < 1e-9, "x = {}", l.a.x);
    }

    /// **版が同じ間は作り直さないこと。**
    ///
    /// `snap.rs` の `index_is_reused_while_revision_is_unchanged` と同型。
    #[test]
    fn cache_is_reused_while_revision_is_unchanged() {
        let (doc, _) = doc_with_one_instance();
        let mut cache = ResolvedInstances::new();

        cache.refresh(&doc);
        let rev = cache.revision;
        cache.refresh(&doc);
        assert_eq!(cache.revision, rev, "版が同じなら再構築しない");
        assert_eq!(rev, doc.revision());
    }

    /// **定義を編集すると全インスタンスが追従すること。**
    ///
    /// インスタンス側を書き換える処理はどこにも無く、
    /// 版が進んでキャッシュが捨てられることだけで実現している。
    #[test]
    fn editing_the_definition_updates_the_instance() {
        let (mut doc, id) = doc_with_one_instance();
        let mut cache = ResolvedInstances::new();
        cache.refresh(&doc);
        assert_eq!(cache.get(id).expect("あるはず").len(), 1);

        // 定義の中身を線分 2 本に差し替える。
        let def = doc.definitions().by_name("部品").expect("あるはず");
        doc.apply(Box::new(cad_core::command::SetDefinitionContents::new(
            "EDITCOMP",
            def,
            Point2::ORIGIN,
            vec![line(0.0), line(5.0)],
        )))
        .expect("差し替えられるはず");

        cache.refresh(&doc);
        assert_eq!(
            cache.get(id).expect("あるはず").len(),
            2,
            "インスタンスに触っていないのに中身が増える"
        );
    }

    /// Undo でキャッシュが無効化されること。
    #[test]
    fn undo_invalidates_the_cache() {
        let (mut doc, _) = doc_with_one_instance();
        let mut cache = ResolvedInstances::new();
        assert_eq!(cache.len(), 0, "まだ作られていない");
        cache.refresh(&doc);
        assert_eq!(cache.len(), 1);

        doc.undo().expect("配置を取り消せるはず");
        cache.refresh(&doc);
        assert_eq!(cache.len(), 0, "配置が消えたらキャッシュからも消える");
    }

    /// インスタンス以外はキャッシュに入らないこと。
    ///
    /// 展開が要らないものを持つと、無駄に複製したうえ版ごとに作り直すことになる。
    #[test]
    fn plain_geometry_is_not_cached() {
        let mut doc = Document::new();
        doc.apply(Box::new(AddEntities::one("LINE", line(0.0))))
            .expect("追加できるはず");
        let id = doc.entities().ids().next().expect("あるはず");

        let mut cache = ResolvedInstances::new();
        cache.refresh(&doc);
        assert_eq!(cache.len(), 0, "線分はキャッシュに入らない");
        assert!(cache.get(id).is_none());
    }
}
