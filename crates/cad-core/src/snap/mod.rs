//! オブジェクトスナップ（OSNAP）。
//!
//! カーソル近傍の図形から「端点」「中点」「中心」「交点」「垂線の足」「最近点」の
//! 候補を検出する。空間インデックス（[`index`]）で候補エンティティを絞り込み、
//! 候補生成（[`detect`]）で実際の点を計算する。

pub mod detect;
pub mod index;

pub use detect::{detect, detect_best, SnapQuery};
pub use index::SpatialIndex;

use crate::entity::EntityId;
use crate::geom::Point2;

/// スナップの種類。優先順位の高い順に並べる。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SnapKind {
    /// 端点。
    Endpoint,
    /// 中点。
    Midpoint,
    /// 中心。
    Center,
    /// 交点。
    Intersection,
    /// 垂線の足。
    Perpendicular,
    /// 最近点。
    Nearest,
}

impl SnapKind {
    /// 優先順位。小さいほど優先。
    ///
    /// 列挙子の宣言順がそのまま優先順位になっているため、`Ord` の導出結果と一致する。
    #[must_use]
    pub fn priority(self) -> u8 {
        self as u8
    }

    /// 表示名。
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Endpoint => "端点",
            Self::Midpoint => "中点",
            Self::Center => "中心",
            Self::Intersection => "交点",
            Self::Perpendicular => "垂線",
            Self::Nearest => "最近点",
        }
    }

    /// 全種類を優先順位順に。
    #[must_use]
    pub fn all() -> [SnapKind; 6] {
        [
            Self::Endpoint,
            Self::Midpoint,
            Self::Center,
            Self::Intersection,
            Self::Perpendicular,
            Self::Nearest,
        ]
    }
}

/// 検出したスナップ候補。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnapCandidate {
    /// スナップ先の点（モデル座標）。
    pub point: Point2,
    /// 種類。
    pub kind: SnapKind,
    /// 由来のエンティティ。交点は 2 つ絡むので代表 1 つでよい。
    pub entity: EntityId,
    /// カーソルからの距離。
    pub distance: f64,
}

/// どの種類を有効にするか。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnapModes {
    endpoint: bool,
    midpoint: bool,
    center: bool,
    intersection: bool,
    perpendicular: bool,
    nearest: bool,
}

impl SnapModes {
    /// 全種類を有効にする。
    #[must_use]
    pub fn all() -> Self {
        Self {
            endpoint: true,
            midpoint: true,
            center: true,
            intersection: true,
            perpendicular: true,
            nearest: true,
        }
    }

    /// 全種類を無効にする。
    #[must_use]
    pub fn none() -> Self {
        Self {
            endpoint: false,
            midpoint: false,
            center: false,
            intersection: false,
            perpendicular: false,
            nearest: false,
        }
    }

    /// 指定した種類が有効か。
    #[must_use]
    pub fn is_enabled(&self, kind: SnapKind) -> bool {
        match kind {
            SnapKind::Endpoint => self.endpoint,
            SnapKind::Midpoint => self.midpoint,
            SnapKind::Center => self.center,
            SnapKind::Intersection => self.intersection,
            SnapKind::Perpendicular => self.perpendicular,
            SnapKind::Nearest => self.nearest,
        }
    }

    /// 指定した種類の有効/無効を切り替える。
    pub fn set(&mut self, kind: SnapKind, on: bool) {
        match kind {
            SnapKind::Endpoint => self.endpoint = on,
            SnapKind::Midpoint => self.midpoint = on,
            SnapKind::Center => self.center = on,
            SnapKind::Intersection => self.intersection = on,
            SnapKind::Perpendicular => self.perpendicular = on,
            SnapKind::Nearest => self.nearest = on,
        }
    }
}

impl Default for SnapModes {
    fn default() -> Self {
        Self::all()
    }
}

/// テスト専用の決定的な疑似乱数生成器。
///
/// `rand` クレートに依存できないため、`index.rs` / `detect.rs` のランダムテストで
/// 共有して使う 32bit LCG（Numerical Recipes の定数）。
#[cfg(test)]
pub(crate) mod test_util {
    /// 32bit の線形合同法による疑似乱数生成器。
    pub struct Lcg(u32);

    impl Lcg {
        /// シード値から作る。`0` だと停留するため奇数に補正する。
        pub fn new(seed: u32) -> Self {
            Self(seed | 1)
        }

        /// 次の疑似乱数値 `[0, 2^32)` を返す。
        pub fn next_u32(&mut self) -> u32 {
            self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            self.0
        }

        /// `[lo, hi)` の範囲の `f64` を返す。
        ///
        /// `u32 -> f64` は常に無損失なので `cast_precision_loss` に触れない。
        pub fn next_f64(&mut self, lo: f64, hi: f64) -> f64 {
            let unit = f64::from(self.next_u32()) / f64::from(u32::MAX);
            lo + unit * (hi - lo)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priority_matches_declaration_order() {
        let kinds = SnapKind::all();
        for w in kinds.windows(2) {
            assert!(
                w[0].priority() < w[1].priority(),
                "{:?} は {:?} より優先されるべき",
                w[0],
                w[1]
            );
        }
    }

    #[test]
    fn priority_ordering_matches_spec() {
        assert!(SnapKind::Endpoint.priority() < SnapKind::Midpoint.priority());
        assert!(SnapKind::Midpoint.priority() < SnapKind::Center.priority());
        assert!(SnapKind::Center.priority() < SnapKind::Intersection.priority());
        assert!(SnapKind::Intersection.priority() < SnapKind::Perpendicular.priority());
        assert!(SnapKind::Perpendicular.priority() < SnapKind::Nearest.priority());
    }

    #[test]
    fn all_has_six_kinds_in_priority_order() {
        let kinds = SnapKind::all();
        assert_eq!(kinds.len(), 6);
        let mut sorted = kinds;
        sorted.sort_by_key(|k| k.priority());
        assert_eq!(kinds, sorted);
    }

    #[test]
    fn label_text_matches_spec() {
        assert_eq!(SnapKind::Endpoint.label(), "端点");
        assert_eq!(SnapKind::Midpoint.label(), "中点");
        assert_eq!(SnapKind::Center.label(), "中心");
        assert_eq!(SnapKind::Intersection.label(), "交点");
        assert_eq!(SnapKind::Perpendicular.label(), "垂線");
        assert_eq!(SnapKind::Nearest.label(), "最近点");
    }

    #[test]
    fn modes_all_enables_everything() {
        let m = SnapModes::all();
        for k in SnapKind::all() {
            assert!(m.is_enabled(k), "{k:?} は all() で有効なはず");
        }
    }

    #[test]
    fn modes_none_disables_everything() {
        let m = SnapModes::none();
        for k in SnapKind::all() {
            assert!(!m.is_enabled(k), "{k:?} は none() で無効なはず");
        }
    }

    #[test]
    fn modes_default_is_all() {
        assert_eq!(SnapModes::default(), SnapModes::all());
    }

    #[test]
    fn modes_set_toggles_individual_kind() {
        let mut m = SnapModes::all();
        m.set(SnapKind::Center, false);
        assert!(!m.is_enabled(SnapKind::Center));
        // 他は影響を受けない。
        assert!(m.is_enabled(SnapKind::Endpoint));
        assert!(m.is_enabled(SnapKind::Nearest));

        m.set(SnapKind::Center, true);
        assert!(m.is_enabled(SnapKind::Center));
    }
}
