//! 図形を指す段階の結果プレビュー（Issue #34 段階 2、ADR-0043）。
//!
//! TRIM / EXTEND で、クリックしたら**消える部分**・**伸びる部分**をクリック前に見せる。
//! 結果は `cad-core` の `trim_line` / `extend_line` を副作用なしで呼んで求める。
//! 実行（`TrimEntity` / `ExtendEntity`）と同じ関数・同じ境界を使うので、プレビューと
//! クリックの結果は一致する。
//!
//! # 境界の条件
//!
//! 実行側の境界（`cad-core` の `cutters_except`、非公開）は「表示中のレイヤにある、対象以外の
//! すべての図形。インスタンスは展開しない」。[`boundaries_except`] は同じ条件を `cad-app` 側で
//! 組み立てる。条件が同じことはテスト（プレビューと実行結果の照合）で固定している。
//! インスタンスが境界にならないのは既存の問題（#60）で、直すときは両方を変える。
//!
//! # キャッシュ
//!
//! 境界の列は図面の図形をすべて複製するので、[`Boundaries`] が `(版番号, 対象の ID)` を鍵に
//! 持ち回す。版番号は図面をまたいで比べられない（#65）ので、図面を入れ替えたら
//! 持ち主（`hover::Hover`）ごと捨てる。

use cad_core::geom::Line;
use cad_core::{Document, EntityId, Geometry};

use super::ToolCtx;

/// クリックしたら図面がどう変わるか（図形を指す段階の結果プレビュー）。
///
/// `Document` には入らない派生データ。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EntityPreview {
    /// 消える部分（TRIM）。赤の破線で描く。
    pub removed: Vec<Geometry>,
    /// 増える部分（EXTEND の伸びる部分）。ラバーバンドと同じ琥珀色の破線で描く。
    pub added: Vec<Geometry>,
}

/// [`Tool::entity_preview`](super::Tool::entity_preview) に渡す文脈。
///
/// 図面の読み取り（[`ToolCtx`]）と、境界の列のキャッシュを持つ。
pub struct PreviewCtx<'a> {
    /// 図面と選択など。ツールの `step` に渡すものと同じ。
    pub tool: ToolCtx<'a>,
    boundaries: &'a mut Boundaries,
}

impl<'a> PreviewCtx<'a> {
    /// 文脈を作る。
    pub fn new(tool: ToolCtx<'a>, boundaries: &'a mut Boundaries) -> Self {
        Self { tool, boundaries }
    }

    /// TRIM / EXTEND の境界（[`boundaries_except`]）。前と同じ図面・同じ対象なら作り直さない。
    pub fn boundaries_except(&mut self, target: EntityId) -> &[Geometry] {
        self.boundaries.except(self.tool.doc, target)
    }
}

/// TRIM / EXTEND の境界: 表示中のレイヤにある、`exclude` 以外のすべての図形。
///
/// `cad-core` の `TrimEntity` / `ExtendEntity` が使う `cutters_except` と同じ条件にする
/// （ロックされたレイヤの図形は境界になる。インスタンスは展開せずそのまま入れる）。
#[must_use]
pub fn boundaries_except(doc: &Document, exclude: EntityId) -> Vec<Geometry> {
    doc.entities()
        .iter()
        .filter(|(id, e)| *id != exclude && doc.layers().is_entity_visible(e))
        .map(|(_, e)| e.geom.clone())
        .collect()
}

/// 境界の列のキャッシュ。鍵は `(版番号, 対象の ID)`。
///
/// 版番号は同じ図面の中でしか比べられないので、図面を入れ替えたら捨てること（#65）。
#[derive(Debug, Default)]
pub struct Boundaries {
    key: Option<(u64, EntityId)>,
    geoms: Vec<Geometry>,
    /// 作り直した回数（使い回しのテスト用）。
    #[cfg(test)]
    pub(crate) built: usize,
}

impl Boundaries {
    /// `target` 以外の境界。版番号と対象が前と同じなら作り直さない。
    pub fn except(&mut self, doc: &Document, target: EntityId) -> &[Geometry] {
        let key = (doc.revision(), target);
        if self.key != Some(key) {
            self.geoms = boundaries_except(doc, target);
            self.key = Some(key);
            #[cfg(test)]
            {
                self.built += 1;
            }
        }
        &self.geoms
    }
}

/// TRIM で消える部分。`keep` は `trim_line` の返り値（残る線分の列）。
///
/// - 0 本 … 線分全体が消える
/// - 1 本 … 残る側の反対側。`trim_line` は始点側を `(target.a, 交点)`、終点側を
///   `(交点, target.b)` で作るので、残る線分の始点が対象の始点と同じなら始点側が残る
/// - 2 本 … 2 本の間
///
/// それ以外（3 本以上）は `trim_line` が返さない形なので `None`。
#[must_use]
pub fn trimmed_away(target: &Line, keep: &[Line]) -> Option<Line> {
    match keep {
        [] => Some(*target),
        // 始点が同じ値か（複製した値なので厳密に比べる）。
        [only] if only.a == target.a => Some(Line::new(only.b, target.b)),
        [only] => Some(Line::new(target.a, only.a)),
        [first, second] => Some(Line::new(first.b, second.a)),
        _ => None,
    }
}

/// EXTEND で伸びる部分: 元の端から新しい端まで。`extended` は `extend_line` の返り値。
///
/// `extend_line` は終点側へ伸ばすとき `(target.a, 新しい端)`、始点側へ伸ばすとき
/// `(新しい端, target.b)` を返すので、始点が元のままなら終点側が伸びた。
#[must_use]
pub fn extended_part(target: &Line, extended: &Line) -> Line {
    if extended.a == target.a {
        Line::new(target.b, extended.b)
    } else {
        Line::new(target.a, extended.a)
    }
}

/// 図面の `id` が線分ならその形。
#[must_use]
pub fn line_of(doc: &Document, id: EntityId) -> Option<Line> {
    match doc.entities().get(id).map(|e| &e.geom) {
        Some(Geometry::Line(l)) => Some(*l),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
