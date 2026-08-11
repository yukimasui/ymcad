//! エンティティの識別子。

use std::fmt;

/// エンティティの識別子。世代つきのため、削除後に再利用されたスロットを誤って
/// 指すことがない。
///
/// `Vec` の添字をそのまま ID にすると、削除や Undo で他のエンティティを指すように
/// なってしまうため、世代を必ず持たせている。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EntityId {
    index: u32,
    generation: u32,
}

impl EntityId {
    /// 内部表現から組み立てる。ストアの実装専用。
    pub(crate) const fn new(index: u32, generation: u32) -> Self {
        Self { index, generation }
    }

    /// スロット番号。
    #[must_use]
    pub const fn index(self) -> u32 {
        self.index
    }

    /// 世代番号。
    #[must_use]
    pub const fn generation(self) -> u32 {
        self.generation
    }

    /// DXF のハンドル表記（16 進）。
    ///
    /// スロット番号は文書の生存期間中に再利用されないため、これで一意になる。
    #[must_use]
    pub fn to_dxf_handle(self) -> String {
        format!("{:X}", self.index + 1)
    }
}

impl fmt::Display for EntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}v{}", self.index, self.generation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_slot_different_generation_is_not_equal() {
        let a = EntityId::new(3, 0);
        let b = EntityId::new(3, 1);
        assert_ne!(a, b, "世代が違えば別のエンティティを指す");
    }

    #[test]
    fn dxf_handle_is_nonzero_hex() {
        // DXF のハンドル 0 は予約されているので 1 始まりにする。
        assert_eq!(EntityId::new(0, 0).to_dxf_handle(), "1");
        assert_eq!(EntityId::new(25, 7).to_dxf_handle(), "1A");
    }

    #[test]
    fn handle_ignores_generation() {
        // 削除 → Undo で世代が変わってもハンドルは変わらない。
        assert_eq!(
            EntityId::new(9, 0).to_dxf_handle(),
            EntityId::new(9, 5).to_dxf_handle()
        );
    }
}
