//! テストの共通の補助。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// テスト用の一時ディレクトリ。`Drop` で中身ごと消す。
///
/// 最後に `remove_dir_all` を書く形だと、途中の `assert!` で落ちたときに残る（PR #42 のレビュー）。
/// 並列に走るテスト同士で重ならないよう、名前にプロセス ID と通し番号を入れる。
pub struct TempDir(PathBuf);

impl TempDir {
    /// `tag` を名前に含む空のディレクトリを作る。
    pub fn new(tag: &str) -> Self {
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("ymcad_{tag}_{}_{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("テスト用ディレクトリを作れない");
        Self(dir)
    }

    /// ディレクトリのパス。
    pub fn path(&self) -> &Path {
        &self.0
    }

    /// ディレクトリの下のパス。
    pub fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 決定的な疑似乱数生成器（32bit LCG、Numerical Recipes の定数）。
///
/// `cad_core::snap::test_util::Lcg` と同じもの。あちらは `cad-core` のテスト専用で
/// 外から使えず、`rand` にも依存できないので写した。
pub struct Lcg(u32);

impl Lcg {
    /// シード値から作る。`0` だと停留するため奇数に補正する。
    pub fn new(seed: u32) -> Self {
        Self(seed | 1)
    }

    /// 次の疑似乱数値 `[0, 2^32)`。
    pub fn next_u32(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        self.0
    }

    /// `[lo, hi)` の `f64`。`u32 -> f64` は無損失なので `cast_precision_loss` に触れない。
    pub fn next_f64(&mut self, lo: f64, hi: f64) -> f64 {
        let unit = f64::from(self.next_u32()) / f64::from(u32::MAX);
        lo + unit * (hi - lo)
    }

    /// `[0, n)` の整数。
    pub fn below(&mut self, n: u32) -> u32 {
        self.next_u32() % n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 落ちたテストでも消えるように、`Drop` で消える。
    #[test]
    fn temp_dir_is_removed_on_drop() {
        let path = {
            let dir = TempDir::new("guard");
            std::fs::write(dir.join("a.txt"), "x").expect("書ける");
            assert!(dir.path().is_dir());
            dir.path().to_path_buf()
        };
        assert!(!path.exists(), "drop で消える");
    }

    /// パニックで巻き戻っても消える。
    #[test]
    fn temp_dir_is_removed_when_a_test_panics() {
        let path = std::sync::Mutex::new(None);
        let result = std::panic::catch_unwind(|| {
            let dir = TempDir::new("guard_panic");
            *path.lock().expect("lock") = Some(dir.path().to_path_buf());
            panic!("テストが落ちた");
        });
        assert!(result.is_err());
        let path = path.lock().expect("lock").clone().expect("パスを記録した");
        assert!(!path.exists(), "パニックでも消える");
    }
}
