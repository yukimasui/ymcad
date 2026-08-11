//! オブジェクトスナップの UI 側。ヒステリシスと空間インデックスの管理。
//!
//! 検出そのものは [`cad_core::snap`] にある。ここが持つのは
//! 「一度吸い付いたら簡単には離さない」という **操作感を決める部分**。
//!
//! # ヒステリシスがなぜ必要か
//!
//! 単一の半径で「近ければ吸着」とすると、カーソルが境界上にあるとき
//! 吸着と解除が毎フレーム入れ替わり、マーカーが激しくちらつく。
//! さらに、狙った点に吸い付いた後に少し手が動いただけで外れるため、
//! 「掴んだ感じ」がまったくしない。
//!
//! そこで取得と解放で別の半径を使う。
//!
//! - 未吸着のとき: カーソルから [`ACQUIRE_RADIUS_PX`] 以内に候補があれば吸着する
//! - 吸着中のとき: **吸着点から** [`RELEASE_RADIUS_PX`] を超えて離れるまで保持する
//!
//! 判定の基準がカーソルではなく **吸着点** である点が肝。カーソル基準だと、
//! 吸着した瞬間に基準が動いてしまい履歴が効かない。

use cad_core::geom::Point2;
use cad_core::snap::{detect_best, SnapCandidate, SnapKind, SnapModes, SnapQuery, SpatialIndex};
use cad_core::Document;

/// 未吸着から吸着へ移る半径 [px]。
const ACQUIRE_RADIUS_PX: f32 = 10.0;
/// 吸着を解除する半径 [px]。取得半径より必ず大きくする。
const RELEASE_RADIUS_PX: f32 = 16.0;

/// スナップの状態。
///
/// 空間インデックスは **`Document` の中ではなくここ（派生キャッシュ）** に持ち、
/// `Document::revision()` をキーに再構築する。こうすることで
///
/// - すべてのコマンドがインデックスの更新を意識しなくてよい
/// - Undo / Redo でも revision が進むので、巻き戻しで自動的に無効化される
#[derive(Debug)]
pub struct SnapState {
    enabled: bool,
    modes: SnapModes,
    index: SpatialIndex,
    /// インデックスを作ったときの図面の版番号。
    index_revision: u64,
    /// インデックスがまだ一度も作られていないか。
    index_valid: bool,
    /// 現在吸着している候補。
    held: Option<SnapCandidate>,
}

impl Default for SnapState {
    fn default() -> Self {
        Self::new()
    }
}

impl SnapState {
    /// 既定では全種類が有効。
    #[must_use]
    pub fn new() -> Self {
        Self {
            enabled: true,
            modes: SnapModes::all(),
            index: SpatialIndex::default(),
            index_revision: 0,
            index_valid: false,
            held: None,
        }
    }

    /// OSNAP が有効か。
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// `F3` による ON/OFF 切り替え。切り替えたら吸着状態は捨てる。
    pub fn toggle(&mut self) {
        self.enabled = !self.enabled;
        self.held = None;
    }

    /// 有効なスナップ種別。
    #[must_use]
    #[allow(dead_code, reason = "Phase 5 のスナップ設定 UI で使う")]
    pub fn modes(&self) -> SnapModes {
        self.modes
    }

    /// スナップ種別を切り替える。
    #[allow(dead_code, reason = "Phase 5 のスナップ設定 UI で使う")]
    pub fn set_mode(&mut self, kind: SnapKind, on: bool) {
        self.modes.set(kind, on);
        self.held = None;
    }

    /// 現在吸着している候補。
    #[must_use]
    pub fn held(&self) -> Option<SnapCandidate> {
        self.held
    }

    /// 吸着を捨てる（コマンドの確定時など）。
    pub fn release(&mut self) {
        self.held = None;
    }

    /// インデックスを必要なら作り直す。
    fn refresh_index(&mut self, doc: &Document) {
        if self.index_valid && self.index_revision == doc.revision() {
            return;
        }
        self.index = SpatialIndex::build(doc);
        self.index_revision = doc.revision();
        self.index_valid = true;
        // 図面が変わったら、掴んでいた点はもう当てにならない。
        self.held = None;
    }

    /// カーソル位置に対するスナップ結果を更新して返す。
    ///
    /// - `cursor` … カーソルのモデル座標
    /// - `acquire_radius` / `release_radius` … モデル空間での半径
    ///   （`Viewport::px_to_model_len` で換算して渡すこと）
    /// - `from` … 垂線スナップの基準点（直前に確定した点）
    pub fn update(
        &mut self,
        doc: &Document,
        cursor: Point2,
        acquire_radius: f64,
        release_radius: f64,
        from: Option<Point2>,
    ) -> Option<SnapCandidate> {
        if !self.enabled {
            self.held = None;
            return None;
        }

        self.refresh_index(doc);

        // 吸着中なら、まず「まだ保持できるか」を吸着点からの距離で判定する。
        if let Some(held) = self.held {
            if held.point.dist(cursor) <= release_radius {
                return Some(held);
            }
        }

        // 取得は狭い半径で行う。
        let query = SnapQuery {
            cursor,
            radius: acquire_radius,
            modes: self.modes,
            from,
        };
        self.held = detect_best(doc, &self.index, &query);
        self.held
    }

    /// 画面上の px 半径をモデル空間へ換算して [`Self::update`] を呼ぶ。
    pub fn update_px(
        &mut self,
        doc: &Document,
        cursor: Point2,
        vp: &crate::viewport::Viewport,
        from: Option<Point2>,
    ) -> Option<SnapCandidate> {
        let acquire = vp.px_to_model_len(ACQUIRE_RADIUS_PX);
        let release = vp.px_to_model_len(RELEASE_RADIUS_PX);
        self.update(doc, cursor, acquire, release, from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cad_core::command::AddEntities;
    use cad_core::geom::Line;
    use cad_core::{Entity, Geometry, LayerId};

    fn doc_with_line() -> Document {
        let mut d = Document::new();
        d.apply(Box::new(AddEntities::one(
            "LINE",
            Entity::new(
                Geometry::Line(Line::new(Point2::ORIGIN, Point2::new(100.0, 0.0))),
                LayerId::ZERO,
            ),
        )))
        .unwrap();
        d
    }

    /// 取得半径は解放半径より小さいこと。逆だとヒステリシスにならない。
    #[test]
    fn acquire_radius_is_smaller_than_release() {
        const { assert!(ACQUIRE_RADIUS_PX < RELEASE_RADIUS_PX) };
    }

    #[test]
    fn disabled_snap_returns_nothing() {
        let doc = doc_with_line();
        let mut s = SnapState::new();
        s.toggle();
        assert!(!s.is_enabled());
        assert!(s
            .update(&doc, Point2::new(0.5, 0.5), 5.0, 8.0, None)
            .is_none());
    }

    #[test]
    fn f3_toggles_back_and_forth() {
        let mut s = SnapState::new();
        assert!(s.is_enabled());
        s.toggle();
        assert!(!s.is_enabled());
        s.toggle();
        assert!(s.is_enabled());
    }

    /// 端点の近くで端点に吸着すること。
    #[test]
    fn acquires_endpoint_near_cursor() {
        let doc = doc_with_line();
        let mut s = SnapState::new();
        let got = s
            .update(&doc, Point2::new(1.0, 1.0), 5.0, 8.0, None)
            .expect("端点に吸着するはず");
        assert_eq!(got.kind, SnapKind::Endpoint);
        assert!(got.point.eq_tol(Point2::ORIGIN));
    }

    /// **ヒステリシスの本体。** 取得半径の外へ出ても、解放半径の内側なら保持し続けること。
    #[test]
    fn holds_snap_between_acquire_and_release_radius() {
        let doc = doc_with_line();
        let mut s = SnapState::new();

        // 取得半径 5、解放半径 8 で原点の端点を掴む。
        let held = s
            .update(&doc, Point2::new(1.0, 0.0), 5.0, 8.0, None)
            .unwrap();
        assert!(held.point.eq_tol(Point2::ORIGIN));

        // 取得半径より外だが解放半径の内側 → 保持し続ける。
        let still = s
            .update(&doc, Point2::new(7.0, 0.0), 5.0, 8.0, None)
            .expect("解放半径の内側なので保持されるはず");
        assert!(
            still.point.eq_tol(Point2::ORIGIN),
            "同じ点を掴んだままのはず"
        );
    }

    /// 解放半径を超えたら離すこと。
    #[test]
    fn releases_beyond_release_radius() {
        let doc = doc_with_line();
        let mut s = SnapState::new();
        s.update(&doc, Point2::new(1.0, 0.0), 5.0, 8.0, None)
            .unwrap();

        // 解放半径の外。線上なので「最近点」には吸くが、端点ではなくなる。
        let got = s.update(&doc, Point2::new(50.0, 0.0), 5.0, 8.0, None);
        if let Some(c) = got {
            assert!(
                !c.point.eq_tol(Point2::ORIGIN),
                "遠く離れたら端点は離すはず"
            );
        }
    }

    /// 図面が変わったら掴んでいた点を捨て、インデックスを作り直すこと。
    #[test]
    fn document_change_invalidates_held_snap() {
        let mut doc = doc_with_line();
        let mut s = SnapState::new();
        s.update(&doc, Point2::new(1.0, 0.0), 5.0, 8.0, None)
            .unwrap();
        assert!(s.held().is_some());

        doc.undo().unwrap(); // 線を消す
        let got = s.update(&doc, Point2::new(1.0, 0.0), 5.0, 8.0, None);
        assert!(got.is_none(), "図形が消えたら吸着も消えるはず");
    }

    /// インデックスは版番号が変わらない限り作り直さないこと。
    #[test]
    fn index_is_reused_while_revision_is_unchanged() {
        let doc = doc_with_line();
        let mut s = SnapState::new();
        s.update(&doc, Point2::new(1.0, 0.0), 5.0, 8.0, None);
        let rev = s.index_revision;

        s.update(&doc, Point2::new(2.0, 0.0), 5.0, 8.0, None);
        assert_eq!(s.index_revision, rev, "版が同じなら再構築しない");
        assert_eq!(rev, doc.revision());
    }

    #[test]
    fn release_clears_held() {
        let doc = doc_with_line();
        let mut s = SnapState::new();
        s.update(&doc, Point2::new(1.0, 0.0), 5.0, 8.0, None);
        assert!(s.held().is_some());
        s.release();
        assert!(s.held().is_none());
    }
}
