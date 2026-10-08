//! テスト用の一時ディレクトリ（依存を足さないための最小限の自作）。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

static SEQUENCE: AtomicU32 = AtomicU32::new(0);

/// 作った一時ディレクトリ。drop で中身ごと消す。
#[derive(Debug)]
pub struct TempDir(PathBuf);

impl TempDir {
    /// `label` を名前に含む空のディレクトリを作る。
    pub fn new(label: &str) -> Self {
        let n = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("ymcad-mcp-test-{label}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("一時ディレクトリを作れません");
        Self(path)
    }

    /// パス（作ったときのまま）。
    pub fn path(&self) -> &Path {
        &self.0
    }

    /// `canonicalize` したパス（`/tmp` がシンボリックリンクの環境でも比べられるように）。
    pub fn canonical(&self) -> PathBuf {
        self.0
            .canonicalize()
            .expect("一時ディレクトリを解決できません")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
