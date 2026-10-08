//! root の配下だけを読み書きさせるパスの検査。
//!
//! LLM はパスを自由に組み立てるので、サーバーの側で読み書きできる範囲を閉じる。
//!
//! # 規則（読み込み・書き込みで共通）
//!
//! 1. 空・NUL を含む・長すぎるパスは拒む
//! 2. **`..` を含むパスは拒む**（解決した先が root の配下でも。意図が読めないため）
//! 3. 相対パスは**最初の root** から解決する（サーバーのカレントディレクトリではない）
//! 4. 拡張子は `.ymc` / `.dxf` だけ（大文字小文字は問わない）。**拡張子が無ければ `.ymc` を付ける**
//! 5. 既存のファイルが**シンボリックリンクなら拒む**（指す先が root の配下でも）
//! 6. `canonicalize` した実体が**どれかの root の配下**であること
//!    （途中のディレクトリが外を指すシンボリックリンクならここで落ちる）
//!
//! 書き込みでは、ファイルがまだ無くてよい。親ディレクトリを `canonicalize` して root の配下か
//! 確かめ、そこへファイル名を足したものを保存先にする（ディレクトリは作らない）。
//!
//! 検査と実際の読み書きの間に他のプロセスがファイルを差し替える競合（TOCTOU）は防げない。
//! 守るのは「LLM が組み立てたパスで root の外に触れない」ことで、ローカルの他プロセスへの
//! 防御ではない（ADR-0046）。

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use crate::limits::MAX_PATH_BYTES;

/// ネイティブ形式の拡張子。
pub const NATIVE_EXTENSION: &str = cad_core::native::EXTENSION;

/// 交換用形式の拡張子。
pub const DXF_EXTENSION: &str = "dxf";

/// ファイル形式。**拡張子だけで決める**（設計原則 9）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// `.ymc`（ネイティブ・無損失）。
    Native,
    /// `.dxf`（R12・交換用・非可逆）。
    Dxf,
}

impl Format {
    /// パスの拡張子から形式を決める。`.ymc` / `.dxf` 以外（拡張子なしを含む）は `None`。
    ///
    /// `cad-app` の `file_ops::is_dxf` と同じく拡張子だけを見る（大文字小文字は無視）。
    /// アプリは `.dxf` 以外をすべてネイティブとして開くが、ここでは知らない拡張子を拒む
    /// （LLM が別の種類のファイルを読ませたり、上書きしたりしないように）。
    #[must_use]
    pub fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?;
        if ext.eq_ignore_ascii_case(NATIVE_EXTENSION) {
            Some(Self::Native)
        } else if ext.eq_ignore_ascii_case(DXF_EXTENSION) {
            Some(Self::Dxf)
        } else {
            None
        }
    }

    /// JSON に出す名前。
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Native => "ymc",
            Self::Dxf => "dxf",
        }
    }
}

/// 読み書きを許すディレクトリの一覧（`canonicalize` 済み）。
#[derive(Clone, Debug)]
pub struct Roots(Vec<PathBuf>);

impl Roots {
    /// ディレクトリの一覧から作る。各ディレクトリは `canonicalize` する。
    ///
    /// # Errors
    ///
    /// 一覧が空、ディレクトリが無い・ディレクトリでない場合。
    pub fn new(dirs: &[PathBuf]) -> Result<Self, String> {
        if dirs.is_empty() {
            return Err("root が 1 つもありません".to_owned());
        }
        let mut out = Vec::with_capacity(dirs.len());
        for d in dirs {
            let c = d
                .canonicalize()
                .map_err(|e| format!("root {} を解決できません: {e}", d.display()))?;
            if !c.is_dir() {
                return Err(format!("root {} はディレクトリではありません", d.display()));
            }
            if !out.contains(&c) {
                out.push(c);
            }
        }
        Ok(Self(out))
    }

    /// root の一覧。
    #[must_use]
    pub fn dirs(&self) -> &[PathBuf] {
        &self.0
    }

    fn contains(&self, canonical: &Path) -> bool {
        self.0.iter().any(|r| canonical.starts_with(r))
    }

    fn outside_message(&self, raw: &str) -> String {
        let roots: Vec<String> = self.0.iter().map(|r| r.display().to_string()).collect();
        format!(
            "{raw} は読み書きを許したディレクトリ（root: {}）の外です",
            roots.join(", ")
        )
    }

    /// 読み込むファイルのパスを検査し、実体のパス（`canonicalize` 済み）と形式を返す。
    ///
    /// # Errors
    ///
    /// モジュールの規則に反する場合・ファイルが無い場合。
    pub fn resolve_for_read(&self, raw: &str) -> Result<(PathBuf, Format), String> {
        let (path, format) = self.lexical(raw)?;
        let meta = std::fs::symlink_metadata(&path)
            .map_err(|e| format!("{} を開けません: {e}", path.display()))?;
        if meta.file_type().is_symlink() {
            return Err(format!(
                "{} はシンボリックリンクなので開きません（実体のパスを指定してください）",
                path.display()
            ));
        }
        if !meta.is_file() {
            return Err(format!("{} は通常のファイルではありません", path.display()));
        }
        let canonical = path
            .canonicalize()
            .map_err(|e| format!("{} を解決できません: {e}", path.display()))?;
        if !self.contains(&canonical) {
            return Err(self.outside_message(raw));
        }
        Ok((canonical, format))
    }

    /// 書き込むファイルのパスを検査し、保存先（親を `canonicalize` 済み）と形式を返す。
    ///
    /// 保存先がまだ無くてもよい。あるなら通常のファイルでなければならない。
    /// **上書きしてよいかはここでは決めない**（呼び出し側が `overwrite` を見る）。
    ///
    /// # Errors
    ///
    /// モジュールの規則に反する場合・親ディレクトリが無い場合。
    pub fn resolve_for_write(&self, raw: &str) -> Result<(PathBuf, Format), String> {
        let (path, format) = self.lexical(raw)?;
        Ok((self.check_write_target(&path, raw)?, format))
    }

    /// 前に保存・読み込みしたパス（`save_drawing` で `path` を省いたとき）を、書き込む前に検査し直す。
    ///
    /// 読み込んだ後にファイルがシンボリックリンクへ差し替えられた、などを拾う。
    ///
    /// # Errors
    ///
    /// [`Self::resolve_for_write`] と同じ。
    pub fn recheck_for_write(&self, path: &Path) -> Result<(PathBuf, Format), String> {
        let raw = path.display().to_string();
        let format = Format::from_path(path).ok_or_else(|| {
            format!("{raw} の拡張子は扱えません（.{NATIVE_EXTENSION} か .{DXF_EXTENSION} だけ）")
        })?;
        Ok((self.check_write_target(path, &raw)?, format))
    }

    fn check_write_target(&self, path: &Path, raw: &str) -> Result<PathBuf, String> {
        let name = path
            .file_name()
            .ok_or_else(|| format!("{raw} にファイル名がありません"))?;
        let parent = path
            .parent()
            .ok_or_else(|| format!("{raw} の親ディレクトリがありません"))?;
        let parent = parent.canonicalize().map_err(|e| {
            format!(
                "保存先のディレクトリ {} がありません（ディレクトリは作りません）: {e}",
                parent.display()
            )
        })?;
        if !self.contains(&parent) {
            return Err(self.outside_message(raw));
        }
        let target = parent.join(name);
        match std::fs::symlink_metadata(&target) {
            Ok(meta) if meta.file_type().is_symlink() => Err(format!(
                "{} はシンボリックリンクなので上書きしません",
                target.display()
            )),
            Ok(meta) if !meta.is_file() => Err(format!(
                "{} は通常のファイルではありません",
                target.display()
            )),
            _ => Ok(target),
        }
    }

    /// ファイルシステムを見ない検査（規則 1〜4）。絶対パスにして返す。
    fn lexical(&self, raw: &str) -> Result<(PathBuf, Format), String> {
        if raw.is_empty() {
            return Err("path が空です".to_owned());
        }
        if raw.len() > MAX_PATH_BYTES {
            return Err(format!("path が長すぎます（上限 {MAX_PATH_BYTES} バイト）"));
        }
        if raw.contains('\0') {
            return Err("path に NUL 文字が含まれています".to_owned());
        }
        let given = Path::new(raw);
        if given
            .components()
            .any(|c| matches!(c, Component::ParentDir))
        {
            return Err(format!(
                "{raw} に .. が含まれています（.. を使わないパスで指定してください）"
            ));
        }
        let mut path = if given.is_absolute() {
            given.to_path_buf()
        } else {
            self.0[0].join(given)
        };
        if raw.ends_with('/') || path.file_name().is_none() {
            return Err(format!("{raw} にファイル名がありません"));
        }
        if path.extension().is_none() {
            // `with_extension` は `.hidden` のような名前で拡張子の位置を誤るので、名前に足す。
            let mut name = OsString::from(path.file_name().unwrap_or_default());
            name.push(".");
            name.push(NATIVE_EXTENSION);
            path.set_file_name(name);
        }
        let format = Format::from_path(&path).ok_or_else(|| {
            format!("{raw} の拡張子は扱えません（.{NATIVE_EXTENSION} か .{DXF_EXTENSION} だけ）")
        })?;
        Ok((path, format))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    fn roots(dir: &TempDir) -> Roots {
        Roots::new(&[dir.path().to_path_buf()]).unwrap()
    }

    #[test]
    fn format_is_decided_by_extension_only() {
        assert_eq!(Format::from_path(Path::new("a.ymc")), Some(Format::Native));
        assert_eq!(Format::from_path(Path::new("A.YMC")), Some(Format::Native));
        assert_eq!(Format::from_path(Path::new("a.DXF")), Some(Format::Dxf));
        assert_eq!(Format::from_path(Path::new("a.txt")), None);
        assert_eq!(Format::from_path(Path::new("a")), None);
    }

    #[test]
    fn relative_paths_resolve_from_the_first_root() {
        let dir = TempDir::new("paths-rel");
        let r = roots(&dir);
        let (p, f) = r.resolve_for_write("a.dxf").unwrap();
        assert_eq!(p, dir.canonical().join("a.dxf"));
        assert_eq!(f, Format::Dxf);
    }

    #[test]
    fn missing_extension_becomes_ymc() {
        let dir = TempDir::new("paths-ext");
        let r = roots(&dir);
        let (p, f) = r.resolve_for_write("drawing").unwrap();
        assert_eq!(p, dir.canonical().join("drawing.ymc"));
        assert_eq!(f, Format::Native);
    }

    #[test]
    fn other_extensions_are_rejected() {
        let dir = TempDir::new("paths-badext");
        let r = roots(&dir);
        for bad in ["a.txt", "a.ymc.bak", "a.dwg", ""] {
            assert!(r.resolve_for_write(bad).is_err(), "{bad:?} を拒むこと");
        }
        std::fs::write(dir.path().join("a.txt"), b"x").unwrap();
        assert!(r.resolve_for_read("a.txt").is_err());
    }

    #[test]
    fn parent_dir_components_are_rejected_even_inside_the_root() {
        let dir = TempDir::new("paths-dotdot");
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let r = roots(&dir);
        let err = r.resolve_for_write("sub/../a.ymc").unwrap_err();
        assert!(err.contains(".."), "{err}");
        assert!(r.resolve_for_write("../a.ymc").is_err());
    }

    #[test]
    fn absolute_paths_outside_the_roots_are_rejected() {
        let inside = TempDir::new("paths-in");
        let outside = TempDir::new("paths-out");
        let r = roots(&inside);
        let target = outside.path().join("a.ymc");
        let err = r.resolve_for_write(target.to_str().unwrap()).unwrap_err();
        assert!(err.contains("外"), "{err}");
        std::fs::write(&target, b"x").unwrap();
        assert!(r.resolve_for_read(target.to_str().unwrap()).is_err());
    }

    #[test]
    fn any_of_several_roots_is_allowed() {
        let a = TempDir::new("paths-a");
        let b = TempDir::new("paths-b");
        let r = Roots::new(&[a.path().to_path_buf(), b.path().to_path_buf()]).unwrap();
        let in_b = b.path().join("x.ymc");
        assert!(r.resolve_for_write(in_b.to_str().unwrap()).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_directory_pointing_outside_is_rejected() {
        let inside = TempDir::new("paths-link-in");
        let outside = TempDir::new("paths-link-out");
        std::os::unix::fs::symlink(outside.path(), inside.path().join("escape")).unwrap();
        std::fs::write(outside.path().join("a.ymc"), b"x").unwrap();
        let r = roots(&inside);
        assert!(r.resolve_for_write("escape/b.ymc").is_err());
        assert!(r.resolve_for_read("escape/a.ymc").is_err());
    }

    /// 既存のファイルがシンボリックリンクなら、指す先が root の配下でも拒む。
    #[cfg(unix)]
    #[test]
    fn symlinked_files_are_rejected_even_inside() {
        let dir = TempDir::new("paths-link-file");
        std::fs::write(dir.path().join("real.ymc"), b"x").unwrap();
        std::os::unix::fs::symlink(dir.path().join("real.ymc"), dir.path().join("link.ymc"))
            .unwrap();
        let r = roots(&dir);
        // 「通常のファイルではない」で落ちても拒めるが、理由が伝わるようリンクだと言うこと。
        let link = "シンボリックリンク";
        assert!(r.resolve_for_read("link.ymc").unwrap_err().contains(link));
        assert!(r.resolve_for_write("link.ymc").unwrap_err().contains(link));
        // 指す先の無いリンクも同じ。
        std::os::unix::fs::symlink(dir.path().join("none.ymc"), dir.path().join("dangling.ymc"))
            .unwrap();
        assert!(r
            .resolve_for_write("dangling.ymc")
            .unwrap_err()
            .contains(link));
        assert!(r.resolve_for_read("real.ymc").is_ok());
    }

    #[test]
    fn directories_are_not_files() {
        let dir = TempDir::new("paths-dir");
        std::fs::create_dir(dir.path().join("d.ymc")).unwrap();
        let r = roots(&dir);
        assert!(r.resolve_for_read("d.ymc").is_err());
        assert!(r.resolve_for_write("d.ymc").is_err());
    }

    #[test]
    fn missing_parent_directory_is_not_created() {
        let dir = TempDir::new("paths-noparent");
        let r = roots(&dir);
        assert!(r.resolve_for_write("no/such/a.ymc").is_err());
        assert!(!dir.path().join("no").exists());
    }

    #[test]
    fn roots_must_exist() {
        assert!(Roots::new(&[]).is_err());
        let dir = TempDir::new("paths-root");
        assert!(Roots::new(&[dir.path().join("missing")]).is_err());
    }
}
