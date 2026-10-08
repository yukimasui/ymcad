//! ホバーで強調する選択プレビュー（Issue #34 段階 1、ADR-0042）。
//!
//! カーソルを乗せた図形のうち、**クリックしたら拾われるもの**を強調する。
//! 何が拾われるかは `Session::click_target` が決め、クリック（`Session::handle_click`）も
//! 同じ関数と同じ [`PickIndex`] を通るので、強調とクリックの結果は構造上一致する。
//!
//! # 空間インデックス
//!
//! ピック用の [`SpatialIndex`] を `Document::revision()` をキーにここで持つ（ADR-0011）。
//! スナップ（`snap::SnapState`）の索引は共有しない。スナップは点の入力待ちでしか更新されず、
//! ホバーは待機中にも要るので、片方の都合で作り直しの時期が決まると取り違えが起きる。
//! 1 万図形でも作り直しはミリ秒の桁なので、2 つ持っても困らない。
//!
//! # 結果プレビュー（段階 2、ADR-0043）
//!
//! 図形を指す段階では、強調する図形をクリックしたら図面がどう変わるか（TRIM で消える部分、
//! EXTEND で伸びる部分）も求める（`Session::entity_preview`）。境界の列のキャッシュ
//! （[`Boundaries`]）もここに持つので、図面を入れ替えたとき（`Hover::new()` で作り直す）に
//! 索引と一緒に捨てられる。

use cad_core::geom::{Aabb, Point2};
use cad_core::snap::SpatialIndex;
use cad_core::{Document, EntityId};

use crate::selection::{self, Picker};
use crate::session::{ClickTarget, PickStage, Session};
use crate::tools::entity_preview::Boundaries;
use crate::tools::EntityPreview;

/// 空間インデックスで候補を絞って拾う。
///
/// 採点は [`selection::pick_among`] なので、全走査（[`selection::pick_at`]）と同じ結果になる。
#[derive(Debug, Default)]
pub struct PickIndex {
    index: SpatialIndex,
    /// インデックスを作ったときの図面の版番号。
    revision: u64,
    /// インデックスがまだ一度も作られていないか。
    valid: bool,
}

impl PickIndex {
    /// 版が変わっていれば作り直す。
    fn refresh(&mut self, doc: &Document) {
        if self.valid && self.revision == doc.revision() {
            return;
        }
        self.index = SpatialIndex::build(doc);
        self.revision = doc.revision();
        self.valid = true;
    }

    /// `pos` の周りで拾われうる候補。
    ///
    /// 拾い半径の 2 倍の正方形で引く。距離の計算の丸めで、境界ぎりぎりの図形を
    /// 落とさないための余裕（候補が少し増えるだけで、結果は `pick_among` が決める）。
    fn candidates(&mut self, doc: &Document, pos: Point2, tolerance: f64) -> Vec<EntityId> {
        self.refresh(doc);
        let area = Aabb::new(pos, pos).expanded(tolerance * 2.0);
        self.index.query(area)
    }
}

impl Picker for PickIndex {
    fn pick(&mut self, doc: &Document, pos: Point2, tolerance: f64) -> Option<EntityId> {
        let candidates = self.candidates(doc, pos, tolerance);
        selection::pick_among(doc, candidates, pos, tolerance)
    }
}

/// 前フレームの結果を使い回すための鍵。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Key {
    revision: u64,
    /// 位置と半径は `to_bits` で比べる（同じ値なら同じ結果、を厳密に）。
    x: u64,
    y: u64,
    tolerance: u64,
    stage: PickStage,
}

/// ホバーの強調の状態。
#[derive(Debug, Default)]
pub struct Hover {
    picker: PickIndex,
    /// 最後に計算した鍵と結果。
    last: Option<(Key, ClickTarget)>,
    /// このフレームで強調するか（カーソルがキャンバスの外・矩形選択中などは偽）。
    shown: bool,
    /// 結果プレビュー（TRIM / EXTEND）の境界の列のキャッシュ。
    boundaries: Boundaries,
    /// このフレームの結果プレビュー。
    preview: Option<EntityPreview>,
    /// 計算し直した回数（使い回しのテスト用）。
    #[cfg(test)]
    computed: usize,
}

impl Hover {
    /// 空の状態。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// クリックで使う拾い手。ホバーと同じ索引を使う。
    pub fn picker(&mut self) -> &mut PickIndex {
        &mut self.picker
    }

    /// このフレームの強調を決める。
    ///
    /// `at` はクリックと同じカーソル位置（`CadApp::cursor_model`）。強調しない場面
    /// （キャンバスの外、矩形選択のドラッグ中）では `None` を渡す。点や値の入力待ちでは
    /// `click_target` が点を返すので、何も強調されない。
    ///
    /// 図面・位置・半径・段階が前フレームと同じなら計算し直さない。
    pub fn update(
        &mut self,
        session: &Session,
        doc: &Document,
        at: Option<Point2>,
        tolerance: f64,
    ) {
        let Some(pos) = at else {
            self.shown = false;
            self.preview = None;
            return;
        };
        self.shown = true;
        let key = Key {
            revision: doc.revision(),
            x: pos.x.to_bits(),
            y: pos.y.to_bits(),
            tolerance: tolerance.to_bits(),
            stage: session.pick_stage(),
        };
        if !self.last.as_ref().is_some_and(|(k, _)| *k == key) {
            let target = session.click_target(pos, tolerance, doc, &mut self.picker);
            self.last = Some((key, target));
            #[cfg(test)]
            {
                self.computed += 1;
            }
        }
        // 結果プレビューは毎回ツールに聞く。強調する図形が同じでも、実行中のツールが
        // 替わる（TRIM → EXTEND）と結果が変わるので、上の鍵では使い回せない。
        // 重いのは境界の列の複製で、そちらは `Boundaries` が（版番号, 対象）で使い回す。
        self.preview = self
            .last
            .as_ref()
            .and_then(|(_, target)| session.entity_preview(target, doc, &mut self.boundaries));
    }

    /// クリックしたら図面がどう変わるか（TRIM で消える部分・EXTEND で伸びる部分）。
    ///
    /// 強調している図形（[`Self::highlighted`]）をクリックした結果。計算できない
    /// （線分以外・交点が無い等）・強調しない場面では `None`。
    #[must_use]
    pub fn entity_preview(&self) -> Option<&EntityPreview> {
        self.preview.as_ref().filter(|_| self.shown)
    }

    /// 境界の列のキャッシュ（テスト用）。
    #[cfg(test)]
    pub(crate) fn boundaries(&self) -> &Boundaries {
        &self.boundaries
    }

    /// 強調する図形。
    #[must_use]
    pub fn highlighted(&self) -> &[EntityId] {
        match (&self.last, self.shown) {
            (Some((_, target)), true) => target.highlighted(),
            _ => &[],
        }
    }
}

#[cfg(test)]
mod tests;
