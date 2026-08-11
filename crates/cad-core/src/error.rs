//! エラー型。

use std::fmt;

/// `cad-core` の共通 Result 型。
pub type Result<T, E = CadError> = std::result::Result<T, E>;

/// `cad-core` が返すエラー。
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CadError {
    /// 指定された ID のエンティティが存在しない（削除済み、または世代違い）。
    EntityNotFound,

    /// 指定された ID のレイヤが存在しない。
    LayerNotFound,

    /// 復元しようとしたスロットが既に埋まっている。
    ///
    /// Undo でエンティティを元の ID のまま戻せなかったことを意味し、
    /// 発生した時点でアリーナのスロット割り当て方針にバグがある。
    SlotOccupied,

    /// ジオメトリとして成立しない入力（長さ 0 の線分、半径 0 の円、共線な 3 点など）。
    DegenerateGeometry(&'static str),

    /// 操作が許可されていない（ロックされたレイヤ上のエンティティなど）。
    NotEditable(&'static str),
}

impl fmt::Display for CadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EntityNotFound => write!(f, "エンティティが見つかりません"),
            Self::LayerNotFound => write!(f, "レイヤが見つかりません"),
            Self::SlotOccupied => write!(f, "復元先のスロットが既に使用されています"),
            Self::DegenerateGeometry(what) => {
                write!(f, "ジオメトリとして成立しません: {what}")
            }
            Self::NotEditable(why) => write!(f, "編集できません: {why}"),
        }
    }
}

impl std::error::Error for CadError {}
