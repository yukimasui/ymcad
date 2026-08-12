//! ファイルをアトミックに置き換える。
//!
//! # なぜ必要か
//!
//! `std::fs::write` は **既存ファイルを切り詰めてから書く**。書き込みの途中で
//! クラッシュ・電源断・ディスク満杯が起きると、
//! **保存前の正常なファイルまで失われる**。
//! 図面は失うと作り直しになるので、これは受け入れられない。
//!
//! # 手順と、順序を守る理由
//!
//! 1. **保存先と同じディレクトリに**一時ファイルを作る
//! 2. 書いて `sync_all()` で実際にディスクへ届かせる
//! 3. `rename` で置き換える
//!
//! **一時ファイルを `std::env::temp_dir()` に作ってはいけない。**
//! 別のファイルシステムだと `rename` が「コピー + 削除」に退化してアトミック性を失う。
//!
//! **`sync_all()` を省いてもいけない。** `rename` 自体はアトミックだが、
//! 中身がまだページキャッシュにあるだけの状態で電源断が起きると、
//! **中身が空のファイルが正しい名前で残る**。名前だけ正しくて中身が無いのは、
//! 書き換えに失敗するより悪い。
//!
//! この 3 手順が守るのは「**`path` は常に、書き終わった状態か元の状態のどちらか**」
//! という性質だけ。書きかけの中間状態が `path` から見えることはない。

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::{CadError, Result};

/// 一時ファイル名の衝突を避けるための連番。
///
/// プロセス ID だけでは、同じプロセスから続けて保存したときに衝突する。
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// 一時ファイル名の候補を作り直す回数。
///
/// クラッシュで取り残された一時ファイルとプロセス ID が偶然一致した場合に備える。
/// これを設けないと、隠しファイルを手で消すまで保存が一切できなくなる。
const NAME_ATTEMPTS: u32 = 32;

/// バイト列を `path` へアトミックに書く。
///
/// 成功すれば `path` は `bytes` の内容になり、失敗すれば **`path` は元のまま**。
/// どちらの場合も一時ファイルは残らない。
///
/// # Errors
///
/// 一時ファイルが作れない、書けない、`rename` できない場合 [`CadError::Io`]。
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let (mut file, tmp) = create_temp(path)?;

    // ここから先の失敗は、一時ファイルを片付けてから返す。
    if let Err(e) = write_and_sync(&mut file, bytes) {
        discard(file, &tmp);
        return Err(e);
    }
    // rename の前に閉じる。開いたまま rename しても Unix では動くが、
    // 閉じてからのほうが意図が明確で、他のプラットフォームでも安全。
    drop(file);

    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(io_error(e));
    }
    Ok(())
}

/// 書いてディスクへ届かせる。
fn write_and_sync(file: &mut File, bytes: &[u8]) -> Result<()> {
    file.write_all(bytes).map_err(io_error)?;
    file.sync_all().map_err(io_error)
}

/// 失敗した経路で一時ファイルを捨てる。
fn discard(file: File, tmp: &Path) {
    drop(file);
    // 消せなくても報告するのは元の失敗のほうなので、ここでは握る。
    let _ = fs::remove_file(tmp);
}

/// 保存先と同じディレクトリに、まだ存在しない一時ファイルを作る。
fn create_temp(path: &Path) -> Result<(File, PathBuf)> {
    let dir = parent_dir(path);
    let name = path
        .file_name()
        .ok_or_else(|| CadError::Io("保存先がファイル名で終わっていません".to_owned()))?
        .to_string_lossy()
        .into_owned();

    let mut last = None;
    for _ in 0..NAME_ATTEMPTS {
        let tmp = dir.join(temp_name(&name));
        // create_new なので、既にあるファイルを黙って上書きすることはない。
        match OpenOptions::new().write(true).create_new(true).open(&tmp) {
            Ok(file) => return Ok((file, tmp)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // 取り残された一時ファイルとぶつかった。別の名前で試す。
                last = Some(e);
            }
            Err(e) => return Err(io_error(e)),
        }
    }
    Err(io_error(last.unwrap_or_else(|| {
        std::io::Error::other("一時ファイル名を決められませんでした")
    })))
}

/// 一時ファイル名。
///
/// 元のファイル名を含めておくと、万一取り残されたときに何の一時ファイルか分かる。
/// 先頭の `.` は Unix で隠しファイルになり、ファイル一覧を汚さない。
fn temp_name(original: &str) -> String {
    format!(
        ".{original}.tmp-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

/// 保存先のディレクトリ。
///
/// `Path::parent` は `"a.ymc"` のような裸のファイル名に対して**空のパス**を返すので、
/// そのまま `join` すると相対パスの意味が変わる。カレントディレクトリへ寄せる。
fn parent_dir(path: &Path) -> PathBuf {
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

/// `std::io::Error` を [`CadError`] へ包む。
fn io_error(e: std::io::Error) -> CadError {
    CadError::Io(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Path::parent` は裸のファイル名に対して**空のパス**を返す。
    /// そのまま `join` すると一時ファイルの位置が変わってしまうので、
    /// カレントディレクトリへ寄せていること。
    #[test]
    fn bare_file_name_resolves_to_the_current_directory() {
        assert_eq!(parent_dir(Path::new("drawing.ymc")), PathBuf::from("."));
    }

    #[test]
    fn parent_directory_is_kept_when_present() {
        assert_eq!(
            parent_dir(Path::new("/tmp/somewhere/drawing.ymc")),
            PathBuf::from("/tmp/somewhere")
        );
        assert_eq!(
            parent_dir(Path::new("sub/drawing.ymc")),
            PathBuf::from("sub")
        );
    }

    /// 一時ファイル名は元の名前を含み、隠しファイルになり、毎回変わること。
    #[test]
    fn temp_names_are_unique_and_derived_from_the_original() {
        let a = temp_name("drawing.ymc");
        let b = temp_name("drawing.ymc");
        assert_ne!(a, b, "続けて保存しても衝突しないこと");
        for name in [&a, &b] {
            assert!(name.starts_with('.'), "隠しファイルになること: {name}");
            assert!(name.contains("drawing.ymc"), "元の名前を含むこと: {name}");
            assert!(name.contains(".tmp-"), "一時ファイルと分かること: {name}");
        }
    }

    /// ファイル名で終わらないパスは、一時ファイルを作る前に断ること。
    #[test]
    fn a_path_without_a_file_name_is_rejected() {
        let err = create_temp(Path::new("/")).unwrap_err();
        assert!(matches!(err, CadError::Io(_)), "{err:?}");
    }
}
