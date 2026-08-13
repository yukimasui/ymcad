//! コンポーネント定義のインプレース編集セッション。
//!
//! # どこまでが「編集中の要素」か
//!
//! 編集に入ると定義の中身が実エンティティとして図面へ置かれる。
//! そのあとユーザーは既存のツールで自由に編集するので、
//! **どの要素が定義に戻るのか**を追い続ける必要がある。
//!
//! 追い方は 2 つの規則だけ。
//!
//! 1. **入ったときに置かれた要素**（消されたものは除く）
//! 2. **入ったあとに作られた要素**
//!
//! 2 が判定できるのは、`EntityStore` が
//! **スロット番号を文書の生存期間中に再利用しない**（単調増加）ため
//! （`entity/store.rs` のモジュールドキュメント）。
//! 入った時点の最大スロット番号を覚えておけば、それより大きいものは
//! すべて「あとから作られた」と分かる。追跡用の印を付ける必要がない。
//!
//! **編集中に描いたものはすべて定義に入る。** AutoCAD の `BEDIT` と同じで、
//! 「編集中の図面」に描いているのだから当然という考え方。
//!
//! # 束縛を保つ
//!
//! 束縛は「中身への添字」で座標を指すので、編集で順序が変わると指す先がずれる。
//! 入ったときに「置いた要素 → 元の定義での添字」を記録しておき、
//! 出るときにそれを渡す（`ExitDefinitionEdit`）。
//! **定義の形は変えずに、編集セッションの間だけ対応を持つ。**

use cad_core::component::{DefinitionId, Placement};
use cad_core::{Document, EntityId};

/// 編集セッション。
#[derive(Debug, Clone)]
pub struct EditSession {
    /// 編集している定義。
    definition: DefinitionId,
    /// 入口になったインスタンスの配置。定義座標へ戻すのに使う。
    placement: Placement,
    /// 入ったときに置かれた要素と、その元の定義での添字。
    ///
    /// **並びは定義の中身の順**。
    entered: Vec<EntityId>,
    /// 入った時点で使われていた最大のスロット番号。
    ///
    /// これより大きい番号の要素は「あとから作られた」と判定できる
    /// （スロット番号は再利用されない）。
    watermark: u32,
}

impl EditSession {
    /// 編集を始める。`entered` は置かれた要素（定義の中身と同じ順）。
    #[must_use]
    pub fn new(
        doc: &Document,
        definition: DefinitionId,
        placement: Placement,
        entered: Vec<EntityId>,
    ) -> Self {
        // 置かれた要素も含めた時点での最大スロット番号。
        let watermark = doc.entities().ids().map(EntityId::index).max().unwrap_or(0);
        Self {
            definition,
            placement,
            entered,
            watermark,
        }
    }

    /// 編集している定義。
    #[must_use]
    pub fn definition(&self) -> DefinitionId {
        self.definition
    }

    /// 入口になったインスタンスの配置。
    #[must_use]
    pub fn placement(&self) -> Placement {
        self.placement
    }

    /// いま定義に入る要素と、それぞれの「元の定義での添字」。
    ///
    /// 返り値は `ExitDefinitionEdit` へそのまま渡せる形。
    ///
    /// - 入ったときの要素は、**消されていなければ**元の添字つきで入る
    /// - あとから作られた要素は添字なし（`None`）で入る
    /// - 並びは「元の要素 → 新しい要素」の順。元の並びは保たれる
    #[must_use]
    pub fn members(&self, doc: &Document) -> (Vec<EntityId>, Vec<Option<usize>>) {
        let mut members = Vec::new();
        let mut origins = Vec::new();

        // 1. 入ったときの要素のうち、まだ生きているもの。
        for (index, id) in self.entered.iter().enumerate() {
            if doc.entities().contains(*id) {
                members.push(*id);
                origins.push(Some(index));
            }
        }

        // 2. あとから作られたもの。スロット番号が水位より大きいかで判定する。
        for id in doc.entities().ids() {
            if id.index() > self.watermark {
                members.push(id);
                origins.push(None);
            }
        }

        (members, origins)
    }

    /// この要素が編集中の集合に入っているか。
    ///
    /// 描画で「編集外を淡くする」のに使う。
    #[must_use]
    pub fn contains(&self, id: EntityId) -> bool {
        id.index() > self.watermark || self.entered.contains(&id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cad_core::command::{AddEntities, DefineComponent, DeleteEntities, InsertInstance};
    use cad_core::geom::{Line, Point2};
    use cad_core::{Entity, Geometry, LayerId};

    fn line(x: f64) -> Entity {
        Entity::new(
            Geometry::Line(Line::new(Point2::new(x, 0.0), Point2::new(x + 1.0, 0.0))),
            LayerId::ZERO,
        )
    }

    /// 線分 2 本の定義を持ち、その中身が図面に置かれた状態を作る。
    fn doc_in_edit() -> (Document, EditSession) {
        let mut doc = Document::new();
        doc.apply(Box::new(DefineComponent::new(
            "COMPONENT",
            "窓",
            Point2::ORIGIN,
            vec![line(0.0), line(5.0)],
        )))
        .expect("定義");
        let def = doc.definitions().by_name("窓").expect("あるはず");
        doc.apply(Box::new(InsertInstance::new(
            "INSERT",
            def,
            Placement::at(Point2::ORIGIN),
            LayerId::ZERO,
        )))
        .expect("配置");

        // 中身を図面へ置く（`EnterDefinitionEdit` と同じことを手で行う）。
        let inst = doc.entities().ids().next().expect("あるはず");
        // `apply` はコマンドを消費するので、置かれた ID は差分から求める。
        let before: Vec<EntityId> = doc.entities().ids().collect();
        doc.apply(Box::new(cad_core::command::EnterDefinitionEdit::new(
            "EDITCOMP", inst,
        )))
        .expect("編集に入る");
        let entered: Vec<EntityId> = doc
            .entities()
            .ids()
            .filter(|id| !before.contains(id))
            .collect();
        assert_eq!(entered.len(), 2);

        let session = EditSession::new(&doc, def, Placement::at(Point2::ORIGIN), entered);
        (doc, session)
    }

    #[test]
    fn members_start_as_the_entered_entities() {
        let (doc, session) = doc_in_edit();
        let (members, origins) = session.members(&doc);
        assert_eq!(members.len(), 2);
        assert_eq!(origins, vec![Some(0), Some(1)], "元の添字が付く");
    }

    /// **あとから描いたものが入ること。**
    ///
    /// スロット番号が再利用されないので、印を付けなくても判定できる。
    #[test]
    fn newly_drawn_entities_join_the_set() {
        let (mut doc, session) = doc_in_edit();
        doc.apply(Box::new(AddEntities::one("LINE", line(50.0))))
            .expect("追加");

        let (members, origins) = session.members(&doc);
        assert_eq!(members.len(), 3);
        assert_eq!(
            origins,
            vec![Some(0), Some(1), None],
            "新しいものは添字なし"
        );
    }

    /// **消された要素が外れること。**
    #[test]
    fn deleted_entities_leave_the_set() {
        let (mut doc, session) = doc_in_edit();
        let first = session.entered[0];
        doc.apply(Box::new(DeleteEntities::new("ERASE", vec![first])))
            .expect("削除");

        let (members, origins) = session.members(&doc);
        assert_eq!(members.len(), 1);
        assert_eq!(origins, vec![Some(1)], "残った側の元の添字");
    }

    /// **Undo で戻った要素は再び入ること。**
    ///
    /// 復元は元の ID のまま行われる（世代つきアリーナ）ので、
    /// 判定が自動的に元へ戻る。追跡用の印を持たない利点。
    #[test]
    fn undoing_a_deletion_brings_the_entity_back_into_the_set() {
        let (mut doc, session) = doc_in_edit();
        let first = session.entered[0];
        doc.apply(Box::new(DeleteEntities::new("ERASE", vec![first])))
            .expect("削除");
        assert_eq!(session.members(&doc).0.len(), 1);

        doc.undo().expect("戻す");
        let (members, origins) = session.members(&doc);
        assert_eq!(members.len(), 2, "戻った要素が再び入る");
        assert_eq!(origins, vec![Some(0), Some(1)]);
    }

    #[test]
    fn contains_reports_membership() {
        let (mut doc, session) = doc_in_edit();
        assert!(session.contains(session.entered[0]));

        doc.apply(Box::new(AddEntities::one("LINE", line(50.0))))
            .expect("追加");
        let fresh = doc.entities().ids().last().expect("あるはず");
        assert!(session.contains(fresh), "あとから描いたものも編集中");
    }

    /// 編集の外にある要素は入らないこと。
    #[test]
    fn entities_outside_the_edit_are_not_members() {
        let mut doc = Document::new();
        // 先に外側の図形を描いておく。
        doc.apply(Box::new(AddEntities::one("LINE", line(-100.0))))
            .expect("外側");
        let outside = doc.entities().ids().next().expect("あるはず");

        doc.apply(Box::new(DefineComponent::new(
            "COMPONENT",
            "窓",
            Point2::ORIGIN,
            vec![line(0.0)],
        )))
        .expect("定義");
        let def = doc.definitions().by_name("窓").expect("あるはず");
        let before: Vec<EntityId> = doc.entities().ids().collect();
        doc.apply(Box::new(AddEntities::one("LINE", line(0.0))))
            .expect("中身を置いたことにする");
        let entered: Vec<EntityId> = doc
            .entities()
            .ids()
            .filter(|id| !before.contains(id))
            .collect();

        let session = EditSession::new(&doc, def, Placement::at(Point2::ORIGIN), entered);
        assert!(!session.contains(outside), "外側の図形は編集中ではない");
        let (members, _) = session.members(&doc);
        assert!(!members.contains(&outside));
    }
}
