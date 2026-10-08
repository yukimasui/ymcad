//! ファイルの道具: `new_drawing` / `open_drawing` / `save_drawing` / `drawing_info`。
//!
//! # 未保存の変更と上書きの確認
//!
//! アプリは未保存確認のモーダルで人に聞く（ADR-0015）。MCP では人に聞けないので、
//! **明示のフラグを付けて呼び直させる**。
//!
//! | 場面 | 要るフラグ |
//! |---|---|
//! | 未保存の変更がある図面を新規・開くで捨てる | `discard_changes: true` |
//! | 開いている図面のファイル以外の既存ファイルへ保存する | `overwrite: true` |
//! | 開いた後に他のプロセスが書き換えたファイルへ保存する（更新時刻か大きさが違う） | `overwrite: true` |
//!
//! 保存は `cad-core` のアトミックな書き出し（ADR-0027）。形式は拡張子だけで決める（設計原則 9）。

use std::fs::File;
use std::io::Read;
use std::path::Path;

use cad_core::{dxf, native, Document};
use serde_json::json;

use super::{Args, Tool, ToolResult};
use crate::convert::aabb_to_json;
use crate::limits::MAX_FILE_BYTES;
use crate::paths::Format;
use crate::server::{FileStamp, OpenedFile, Server};

const DISCARD_CHANGES: &str =
    "未保存の変更があっても捨ててよいなら true。省略すると、未保存の変更があるときは何もせずエラーを返す";

pub(super) const NEW_DRAWING: Tool = Tool {
    name: "new_drawing",
    title: "新規図面",
    description: "空の新規図面にする（レイヤ 0 だけ）。図面の通し番号が進み、前の図面の図形 ID は使えなくなる。\
いまの図面に未保存の変更があるときは discard_changes: true が要る。ファイルには何も書かない。",
    schema: || {
        (
            json!({ "discard_changes": { "type": "boolean", "description": DISCARD_CHANGES } }),
            &[],
        )
    },
    read_only: false,
    destructive: true,
    idempotent: false,
    run: new_drawing,
};

pub(super) const OPEN_DRAWING: Tool = Tool {
    name: "open_drawing",
    title: "図面を開く",
    description: "root の配下の .ymc（ネイティブ）か .dxf（R12・交換用）を開く。形式は拡張子だけで決まり、\
拡張子が無ければ .ymc を付ける。相対パスは最初の root から解決する。.. を含むパス・root の外・シンボリックリンクは拒む。\
図面の通し番号が進み、前の図面の図形 ID は使えなくなる。いまの図面に未保存の変更があるときは discard_changes: true が要る。",
    schema: || {
        (
            json!({
                "path": { "type": "string", "description": "開くファイルのパス（.ymc / .dxf）" },
                "discard_changes": { "type": "boolean", "description": DISCARD_CHANGES },
            }),
            &["path"],
        )
    },
    read_only: false,
    destructive: true,
    idempotent: false,
    run: open_drawing,
};

pub(super) const SAVE_DRAWING: Tool = Tool {
    name: "save_drawing",
    title: "図面を保存",
    description: "図面を保存する（アトミックに書き換える）。path を省くと開いた・前に保存した .ymc ファイルへ上書き保存する\
（開いたのが .dxf なら path を省けない。.ymc のパスか、DXF へ書くなら .dxf のパスを明示する）。\
形式は拡張子だけで決まる: .ymc（無損失・保存形式）/ .dxf（R12・交換用。作図線・グループ・パラメータなどが失われ、警告が返る）。\
拡張子が無ければ .ymc を付ける。開いている図面のファイル以外の既存ファイルや、開いた後に他のプログラムが書き換えたファイルへ\
保存するには overwrite: true が要る。保存先は root の配下だけ（ディレクトリは作らない）。",
    schema: || {
        (
            json!({
                "path": { "type": "string", "description": "保存先（.ymc / .dxf）。省略すると開いたファイルへ上書き保存" },
                "overwrite": { "type": "boolean", "description": "既存のファイルを上書きしてよいなら true（開いている図面のファイルで、他から書き換えられていなければ不要）" },
            }),
            &[],
        )
    },
    read_only: false,
    destructive: true,
    idempotent: true,
    run: save_drawing,
};

pub(super) const DRAWING_INFO: Tool = Tool {
    name: "drawing_info",
    title: "図面の情報",
    description: "いまの図面の情報: 図面名（ID の頭の d<起動の印>-<番号>）、ファイルのパスと形式、未保存の変更の有無、図形・レイヤ・コンポーネントの数、\
図面範囲（作図線を除く。度・f64）、取り消し・やり直しができるか、開いた後にファイルが他から書き換えられたか、読み書きできる root、\
内部エラーで図面が書きかけかもしれないか（poisoned。true なら保存できないので開き直す）。",
    schema: || (json!({}), &[]),
    read_only: true,
    destructive: false,
    idempotent: true,
    run: drawing_info,
};

/// 未保存の変更を捨ててよいか確かめる。
fn ensure_can_discard(s: &Server, discard: bool, action: &str) -> Result<(), String> {
    if s.doc.is_dirty() && !discard {
        return Err(format!(
            "いまの図面（{}）に未保存の変更があります。{action}と変更は失われます。\
             残すなら先に save_drawing で保存し、捨ててよければ discard_changes: true を付けて呼び直してください。",
            s.tag().name()
        ));
    }
    Ok(())
}

fn new_drawing(s: &mut Server, a: &Args) -> ToolResult {
    let discard = a.bool_or("discard_changes", false)?;
    ensure_can_discard(s, discard, "新規図面にする")?;
    let discarded = s.doc.is_dirty();
    s.replace_document(Document::new(), None);
    Ok(json!({
        "drawing": s.tag().name(),
        "discarded_changes": discarded,
    }))
}

fn open_drawing(s: &mut Server, a: &Args) -> ToolResult {
    let raw = a.req_str("path")?;
    let discard = a.bool_or("discard_changes", false)?;
    ensure_can_discard(s, discard, "別の図面を開く")?;
    let (path, format) = s.roots.resolve_for_read(raw)?;
    let (doc, stamp) = read_document(&path, format)?;

    let mut warnings = Vec::new();
    if format == Format::Dxf {
        warnings.push(
            "DXF R12 は交換用の形式です。ymcad の図面として残すなら .ymc で保存してください（.dxf への保存は非可逆）"
                .to_owned(),
        );
    }
    let discarded = s.doc.is_dirty();
    s.replace_document(
        doc,
        Some(OpenedFile {
            path: path.clone(),
            format,
            stamp: Some(stamp),
        }),
    );
    Ok(json!({
        "drawing": s.tag().name(),
        "path": path.display().to_string(),
        "format": format.name(),
        "entity_count": s.doc.entities().len(),
        "layer_count": s.doc.layers().len(),
        "component_count": s.doc.definitions().len(),
        "discarded_changes": discarded,
        "warnings": warnings,
    }))
}

/// ファイルを読んで図面にする。大きさの上限を超えるファイルは読まない。
///
/// 更新時刻と大きさは**読む前に**開いたファイルから取る。読んでいる間に書き換えられたら、
/// 記録が古いほうへずれるので、次の保存で上書きの確認が出る（安全な側）。
fn read_document(path: &Path, format: Format) -> Result<(Document, FileStamp), String> {
    let cannot = |e: &dyn std::fmt::Display| format!("{} を読めません: {e}", path.display());
    let file = File::open(path).map_err(|e| cannot(&e))?;
    let meta = file.metadata().map_err(|e| cannot(&e))?;
    let too_large = || {
        format!(
            "{} は大きすぎます（上限 {} MiB）",
            path.display(),
            MAX_FILE_BYTES / (1024 * 1024)
        )
    };
    if meta.len() > MAX_FILE_BYTES {
        return Err(too_large());
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| cannot(&e))?;
    if u64::try_from(bytes.len()).map_or(true, |n| n > MAX_FILE_BYTES) {
        return Err(too_large());
    }

    let loaded = match format {
        Format::Native => native::read::read_from_bytes(&bytes),
        Format::Dxf => {
            let text = String::from_utf8(bytes)
                .map_err(|_| format!("{} は UTF-8 のテキストではありません", path.display()))?;
            dxf::read::read_from_str(&text)
        }
    };
    let mut doc = loaded.map_err(|e| format!("{} を図面として読めません: {e}", path.display()))?;
    doc.mark_saved(Some(path.to_path_buf()));
    Ok((doc, FileStamp::from_metadata(&meta)))
}

fn save_drawing(s: &mut Server, a: &Args) -> ToolResult {
    let raw = a.opt_str("path")?;
    let overwrite = a.bool_or("overwrite", false)?;

    if let Some(tool) = s.poisoned {
        return Err(format!(
            "図面を変える道具（{tool}）の途中で内部エラーが起きたため、図面が書きかけの状態かもしれません。\
             壊れた図面でファイルを上書きしないよう、この図面は保存できません。\
             open_drawing で開き直すか new_drawing で新規にしてください。"
        ));
    }

    let (target, format) = match (raw, &s.file) {
        (Some(raw), _) => s.roots.resolve_for_write(raw)?,
        // 設計原則 9: DXF は交換用。開いた .dxf へ黙って上書きすると、作図線・グループ・パラメータなどが
        // 確認なしに失われる（警告は保存の後にしか返らず、LLM は読み流しやすい）。path を省いた保存は
        // .ymc へ限り、DXF へ書くなら path に .dxf を明示させる。
        (None, Some(f)) if f.format == Format::Dxf => {
            return Err(format!(
                "開いた（前に保存した）ファイル {} は DXF（交換用・非可逆）です。path を省いた保存は .ymc のときだけ行います。\
                 ymcad の図面として残すなら path に .ymc のパスを指定してください。\
                 DXF へ書き出すなら path に .dxf のパスを明示してください（作図線・グループ・パラメータなどが失われます）。",
                f.path.display()
            ))
        }
        (None, Some(f)) => s.roots.recheck_for_write(&f.path)?,
        (None, None) => {
            return Err(
                "保存先がありません（新規図面です）。path に .ymc のパスを指定してください"
                    .to_owned(),
            )
        }
    };

    let existed = std::fs::symlink_metadata(&target).is_ok();
    if existed && !overwrite {
        match s.file.as_ref().filter(|f| f.path == target) {
            None => {
                return Err(format!(
                    "{} は既にあります（いま開いている図面のファイルではありません）。\
                     上書きしてよければ overwrite: true を付けて呼び直してください。",
                    target.display()
                ))
            }
            Some(f) if f.stamp != FileStamp::of(&target) => {
                return Err(format!(
                    "{} は開いた後に他のプログラムが書き換えた可能性があります（更新時刻か大きさが変わっています）。\
                     上書きしてよければ overwrite: true を付けて呼び直してください。",
                    target.display()
                ))
            }
            Some(_) => {}
        }
    }

    let written = match format {
        Format::Native => native::write::write_to_file(&s.doc, &target).map(|()| Vec::new()),
        Format::Dxf => dxf::write::write_to_file(&s.doc, &target),
    };
    let writer_warnings = written.map_err(|e| format!("保存に失敗しました: {e}"))?;

    let mut warnings = Vec::new();
    if format == Format::Dxf {
        warnings.push(
            "DXF R12 は交換用の形式で非可逆です（作図線・グループ・線種・パラメータ・日本語のレイヤ名などが変わります）。\
             ymcad の図面として残すなら .ymc でも保存してください"
                .to_owned(),
        );
    }
    warnings.extend(writer_warnings);

    s.doc.mark_saved(Some(target.clone()));
    s.file = Some(OpenedFile {
        path: target.clone(),
        format,
        stamp: FileStamp::of(&target),
    });
    Ok(json!({
        "drawing": s.tag().name(),
        "path": target.display().to_string(),
        "format": format.name(),
        "overwrote": existed,
        "entity_count": s.doc.entities().len(),
        "warnings": warnings,
    }))
}

fn drawing_info(s: &mut Server, _: &Args) -> ToolResult {
    let doc = &s.doc;
    let history = doc.history();
    let file_changed = s.file.as_ref().map(|f| f.stamp != FileStamp::of(&f.path));
    Ok(json!({
        "drawing": s.tag().name(),
        "path": s.file.as_ref().map(|f| f.path.display().to_string()),
        "format": s.file.as_ref().map(|f| f.format.name()),
        "dirty": doc.is_dirty(),
        "entity_count": doc.entities().len(),
        "layer_count": doc.layers().len(),
        "current_layer": doc.layers().get(doc.layers().current()).map(|l| l.name.as_str()),
        "group_count": doc.groups().len(),
        "component_count": doc.definitions().len(),
        "bbox": aabb_to_json(doc.bbox()),
        "can_undo": history.can_undo(),
        "can_redo": history.can_redo(),
        "undo_name": history.undo_name(),
        "redo_name": history.redo_name(),
        "file_changed_on_disk": file_changed,
        "poisoned": s.poisoned.is_some(),
        "poisoned_by": s.poisoned,
        "roots": s.roots.dirs().iter().map(|r| r.display().to_string()).collect::<Vec<_>>(),
        "angle_unit": "degree",
    }))
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{err, ok, server};
    use super::super::tests::cad_core_add_line;
    use super::*;
    use crate::test_util::TempDir;
    use cad_core::command::AddEntities;
    use cad_core::geom::{Point2, Xline};
    use cad_core::{Entity, Geometry, LayerId};
    use serde_json::Value;

    /// 線分 1 本の図面を `name` に書いておく。
    fn write_sample(dir: &TempDir, name: &str) -> std::path::PathBuf {
        let mut doc = Document::new();
        doc.apply(Box::new(AddEntities::one(
            "LINE",
            Entity::new(
                Geometry::Line(cad_core::geom::Line::new(
                    Point2::new(0.0, 0.0),
                    Point2::new(10.0, 5.0),
                )),
                LayerId::ZERO,
            ),
        )))
        .unwrap();
        let path = dir.path().join(name);
        if Format::from_path(&path) == Some(Format::Dxf) {
            dxf::write::write_to_file(&doc, &path).unwrap();
        } else {
            native::write::write_to_file(&doc, &path).unwrap();
        }
        path
    }

    #[test]
    fn new_drawing_advances_the_serial() {
        let dir = TempDir::new("file-new");
        let mut s = server(&dir);
        let first = s.tag().name();
        let r = ok(&mut s, "new_drawing", json!({}));
        assert_eq!(r["drawing"], s.tag().name());
        assert_eq!(s.serial, 2);
        assert_ne!(r["drawing"], first.as_str());
        assert_eq!(r["discarded_changes"], false);
    }

    #[test]
    fn new_and_open_refuse_to_drop_unsaved_changes() {
        let dir = TempDir::new("file-dirty");
        write_sample(&dir, "a.ymc");
        let mut s = server(&dir);
        cad_core_add_line(&mut s);

        let msg = err(&mut s, "new_drawing", json!({}));
        assert!(msg.contains("discard_changes"), "{msg}");
        let msg = err(&mut s, "open_drawing", json!({"path": "a.ymc"}));
        assert!(msg.contains("discard_changes"), "{msg}");
        assert_eq!(s.doc.entities().len(), 1, "図面はそのまま");
        assert_eq!(s.serial, 1);

        let r = ok(
            &mut s,
            "open_drawing",
            json!({"path": "a.ymc", "discard_changes": true}),
        );
        assert_eq!(r["discarded_changes"], true);
        assert_eq!(s.serial, 2);
        assert_eq!(r["drawing"], s.tag().name());
    }

    #[test]
    fn open_reads_ymc_and_dxf() {
        let dir = TempDir::new("file-open");
        write_sample(&dir, "a.ymc");
        write_sample(&dir, "b.DXF");
        let mut s = server(&dir);

        let r = ok(&mut s, "open_drawing", json!({"path": "a"}));
        assert_eq!(r["format"], "ymc");
        assert_eq!(r["entity_count"], 1);
        assert_eq!(r["warnings"], json!([]));
        assert_eq!(
            r["path"],
            dir.canonical().join("a.ymc").display().to_string(),
            "拡張子なしは .ymc"
        );
        assert!(!s.doc.is_dirty());

        let r = ok(&mut s, "open_drawing", json!({"path": "b.DXF"}));
        assert_eq!(r["format"], "dxf");
        assert_eq!(r["entity_count"], 1);
        assert!(r["warnings"][0].as_str().unwrap().contains("交換用"));
    }

    #[test]
    fn open_rejects_paths_outside_the_rules() {
        let dir = TempDir::new("file-open-bad");
        let outside = TempDir::new("file-open-outside");
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        write_sample(&dir, "a.ymc");
        write_sample(&outside, "x.ymc");
        std::fs::write(dir.path().join("note.txt"), b"x").unwrap();
        let mut s = server(&dir);

        for bad in [
            json!("sub/../a.ymc"),
            json!(outside.path().join("x.ymc").display().to_string()),
            json!("note.txt"),
            json!("missing.ymc"),
        ] {
            err(&mut s, "open_drawing", json!({"path": bad}));
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outside.path(), dir.path().join("out")).unwrap();
            err(&mut s, "open_drawing", json!({"path": "out/x.ymc"}));
            std::os::unix::fs::symlink(dir.path().join("a.ymc"), dir.path().join("l.ymc")).unwrap();
            err(&mut s, "open_drawing", json!({"path": "l.ymc"}));
        }
        assert_eq!(s.serial, 1, "失敗した open は図面を入れ替えない");
    }

    #[test]
    fn open_rejects_broken_files() {
        let dir = TempDir::new("file-broken");
        std::fs::write(dir.path().join("bad.ymc"), b"not a drawing").unwrap();
        std::fs::write(dir.path().join("bad.dxf"), [0xff_u8, 0xfe, 0x00]).unwrap();
        let mut s = server(&dir);
        let msg = err(&mut s, "open_drawing", json!({"path": "bad.ymc"}));
        assert!(msg.contains("図面として読めません"), "{msg}");
        let msg = err(&mut s, "open_drawing", json!({"path": "bad.dxf"}));
        assert!(msg.contains("UTF-8"), "{msg}");
    }

    #[test]
    fn save_writes_native_files_that_read_back() {
        let dir = TempDir::new("file-save");
        let mut s = server(&dir);
        cad_core_add_line(&mut s);
        let r = ok(&mut s, "save_drawing", json!({"path": "out"}));
        assert_eq!(r["format"], "ymc");
        assert_eq!(r["overwrote"], false);
        assert!(!s.doc.is_dirty());

        let saved = dir.path().join("out.ymc");
        let bytes = std::fs::read(&saved).unwrap();
        assert_eq!(
            bytes,
            native::write::write_to_bytes(&s.doc),
            "書いたのは図面そのもの"
        );
        let back = native::read::read_from_bytes(&bytes).unwrap();
        assert_eq!(back.entities().len(), 1);

        // path を省くと同じファイルへ。上書きの確認は要らない。
        cad_core_add_line(&mut s);
        let r = ok(&mut s, "save_drawing", json!({}));
        assert_eq!(r["overwrote"], true);
        assert_eq!(
            native::read::read_from_file(&saved)
                .unwrap()
                .entities()
                .len(),
            2
        );
    }

    #[test]
    fn save_without_a_file_needs_a_path() {
        let dir = TempDir::new("file-save-nopath");
        let mut s = server(&dir);
        let msg = err(&mut s, "save_drawing", json!({}));
        assert!(msg.contains("path"), "{msg}");
    }

    #[test]
    fn save_asks_before_overwriting_other_files() {
        let dir = TempDir::new("file-save-other");
        let other = write_sample(&dir, "other.ymc");
        let before = std::fs::read(&other).unwrap();
        let mut s = server(&dir);
        let msg = err(&mut s, "save_drawing", json!({"path": "other.ymc"}));
        assert!(msg.contains("overwrite"), "{msg}");
        assert_eq!(std::fs::read(&other).unwrap(), before, "書き換えていない");

        ok(
            &mut s,
            "save_drawing",
            json!({"path": "other.ymc", "overwrite": true}),
        );
        assert_ne!(std::fs::read(&other).unwrap(), before);
    }

    /// 開いた後に他のプロセスが書き換えたファイルへは、確認なしに上書きしない。
    #[test]
    fn save_asks_when_the_file_changed_on_disk() {
        let dir = TempDir::new("file-save-changed");
        let path = write_sample(&dir, "a.ymc");
        let mut s = server(&dir);
        ok(&mut s, "open_drawing", json!({"path": "a.ymc"}));
        assert_eq!(
            ok(&mut s, "drawing_info", json!({}))["file_changed_on_disk"],
            false
        );

        // 大きさを変える（更新時刻の分解能に頼らない）。
        let mut other = std::fs::read(&path).unwrap();
        other.extend_from_slice(b"extra");
        std::fs::write(&path, &other).unwrap();
        assert_eq!(
            ok(&mut s, "drawing_info", json!({}))["file_changed_on_disk"],
            true
        );

        let msg = err(&mut s, "save_drawing", json!({}));
        assert!(msg.contains("他のプログラム"), "{msg}");
        assert_eq!(std::fs::read(&path).unwrap(), other);
        ok(&mut s, "save_drawing", json!({"overwrite": true}));
        assert_eq!(
            ok(&mut s, "drawing_info", json!({}))["file_changed_on_disk"],
            false
        );
    }

    /// 開いた .dxf へ path なしで保存すると、確認なしに非可逆の DXF で上書きしてしまうので拒む（設計原則 9）。
    /// .ymc のパスか、.dxf のパスの明示を求める。
    #[test]
    fn save_without_a_path_does_not_overwrite_an_opened_dxf() {
        let dir = TempDir::new("file-save-dxf-nopath");
        let dxf = write_sample(&dir, "a.dxf");
        let before = std::fs::read(&dxf).unwrap();
        let mut s = server(&dir);
        ok(&mut s, "open_drawing", json!({"path": "a.dxf"}));
        cad_core_add_line(&mut s);

        let msg = err(&mut s, "save_drawing", json!({}));
        assert!(msg.contains(".ymc") && msg.contains("DXF"), "{msg}");
        let msg = err(&mut s, "save_drawing", json!({"overwrite": true}));
        assert!(
            msg.contains(".ymc"),
            "overwrite でも path なしは拒む: {msg}"
        );
        assert_eq!(std::fs::read(&dxf).unwrap(), before, "書き換えていない");

        // 明示の .dxf は許す（開いたファイルなので overwrite は要らない）。
        let r = ok(&mut s, "save_drawing", json!({"path": "a.dxf"}));
        assert_eq!(r["format"], "dxf");
        assert_ne!(std::fs::read(&dxf).unwrap(), before);
        // .ymc へ保存した後は、path なしで .ymc へ上書きできる。
        ok(&mut s, "save_drawing", json!({"path": "a.ymc"}));
        cad_core_add_line(&mut s);
        let r = ok(&mut s, "save_drawing", json!({}));
        assert_eq!(r["format"], "ymc");
    }

    /// path を省いた保存は、前に開いた・保存したパスを書く前に検査し直す。開いた後に親ディレクトリを
    /// root の外を指すシンボリックリンクへ差し替えられたら拒む（PR #84 レビューの非ブロッキング 4）。
    #[cfg(unix)]
    #[test]
    fn save_without_a_path_rechecks_the_directory() {
        let dir = TempDir::new("file-save-recheck");
        let outside = TempDir::new("file-save-recheck-outside");
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        write_sample(&dir, "sub/a.ymc");
        let mut s = server(&dir);
        ok(&mut s, "open_drawing", json!({"path": "sub/a.ymc"}));

        // sub を root の外を指すリンクへ差し替える（外にも同じ名前のファイルを置く）。
        std::fs::rename(dir.path().join("sub"), dir.path().join("sub-moved")).unwrap();
        write_sample(&outside, "a.ymc");
        let outside_before = std::fs::read(outside.path().join("a.ymc")).unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("sub")).unwrap();

        for args in [json!({}), json!({"overwrite": true})] {
            let msg = err(&mut s, "save_drawing", args);
            assert!(msg.contains("root"), "{msg}");
        }
        assert_eq!(
            std::fs::read(outside.path().join("a.ymc")).unwrap(),
            outside_before,
            "root の外へ書いていない"
        );
    }

    #[test]
    fn save_rejects_paths_outside_the_rules() {
        let dir = TempDir::new("file-save-bad");
        let outside = TempDir::new("file-save-outside");
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let mut s = server(&dir);
        for bad in [
            json!("sub/../a.ymc"),
            json!("../a.ymc"),
            json!(outside.path().join("a.ymc").display().to_string()),
            json!("a.txt"),
            json!("nodir/a.ymc"),
        ] {
            err(
                &mut s,
                "save_drawing",
                json!({"path": bad, "overwrite": true}),
            );
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outside.path(), dir.path().join("out")).unwrap();
            err(
                &mut s,
                "save_drawing",
                json!({"path": "out/a.ymc", "overwrite": true}),
            );
            let target = write_sample(&dir, "real.ymc");
            std::os::unix::fs::symlink(&target, dir.path().join("link.ymc")).unwrap();
            err(
                &mut s,
                "save_drawing",
                json!({"path": "link.ymc", "overwrite": true}),
            );
        }
        assert!(
            std::fs::read_dir(outside.path()).unwrap().next().is_none(),
            "root の外には何も書いていない"
        );
    }

    #[test]
    fn saving_dxf_returns_warnings() {
        let dir = TempDir::new("file-save-dxf");
        let mut s = server(&dir);
        s.doc
            .apply(Box::new(AddEntities::one(
                "XLINE",
                Entity::new(
                    Geometry::Xline(Xline::horizontal(Point2::new(0.0, 0.0))),
                    LayerId::ZERO,
                ),
            )))
            .unwrap();
        let r = ok(&mut s, "save_drawing", json!({"path": "x.dxf"}));
        assert_eq!(r["format"], "dxf");
        let warnings: Vec<&str> = r["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w.as_str().unwrap())
            .collect();
        assert!(
            warnings.iter().any(|w| w.contains("非可逆")),
            "{warnings:?}"
        );
        assert!(
            warnings.iter().any(|w| w.contains("作図線 1 本")),
            "{warnings:?}"
        );
        let text = std::fs::read_to_string(dir.path().join("x.dxf")).unwrap();
        assert!(text.contains("AC1009"));
    }

    #[test]
    fn drawing_info_reports_the_state() {
        let dir = TempDir::new("file-info");
        let mut s = server(&dir);
        let r = ok(&mut s, "drawing_info", json!({}));
        assert_eq!(r["drawing"], s.tag().name());
        assert!(r["drawing"].as_str().unwrap().ends_with("-1"));
        assert_eq!(r["path"], Value::Null);
        assert_eq!(r["dirty"], false);
        assert_eq!(r["bbox"], Value::Null);
        assert_eq!(r["current_layer"], "0");
        assert_eq!(r["roots"][0], dir.canonical().display().to_string());

        cad_core_add_line(&mut s);
        let r = ok(&mut s, "drawing_info", json!({}));
        assert_eq!(r["dirty"], true);
        assert_eq!(r["entity_count"], 1);
        assert_eq!(r["can_undo"], true);
        assert_eq!(r["undo_name"], "LINE");
        assert_eq!(r["bbox"]["max"], json!({"x": 1.0, "y": 1.0}));
    }
}
