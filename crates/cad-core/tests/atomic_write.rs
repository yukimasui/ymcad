//! 保存がアトミックであることの検証。
//!
//! `atomic_write` モジュール自体は `pub(crate)` なので、
//! **公開 API（`dxf::write::write_to_file`）を通して**外側から性質を確かめる。
//! 内部実装ではなく「保存に失敗しても図面を失わない」という利用者から見た
//! 約束を固定するのが目的。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use cad_core::command::AddEntities;
use cad_core::dxf;
use cad_core::geom::{Line, Point2};
use cad_core::{Document, Entity, Geometry, LayerId};

/// テストごとに別のディレクトリを使うための連番。
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// 空のテスト用ディレクトリを作る。
fn test_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ymcad_atomic_{tag}_{}_{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("テスト用ディレクトリを作れませんでした");
    dir
}

fn sample_doc() -> Document {
    let mut doc = Document::new();
    let geom = Geometry::Line(Line::new(Point2::new(0.0, 0.0), Point2::new(10.0, 5.0)));
    doc.apply(Box::new(AddEntities::one(
        "LINE",
        Entity::new(geom, LayerId::ZERO),
    )))
    .expect("線分を追加できませんでした");
    doc
}

/// ディレクトリ内に一時ファイル（`.` 始まり、`.tmp-` を含む）が残っていないこと。
fn assert_no_temp_files(dir: &Path) {
    let leftovers: Vec<String> = fs::read_dir(dir)
        .expect("ディレクトリを読めませんでした")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains(".tmp-"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "一時ファイルが残っています: {leftovers:?}"
    );
}

#[test]
fn successful_save_writes_the_file_and_leaves_no_temp_file() {
    let dir = test_dir("ok");
    let path = dir.join("drawing.dxf");

    dxf::write::write_to_file(&sample_doc(), &path).expect("保存できるはず");

    let text = fs::read_to_string(&path).expect("保存したファイルを読めるはず");
    assert!(text.contains("LINE"), "内容が書かれていること");
    assert!(text.trim_end().ends_with("EOF"), "末尾まで書かれていること");
    assert_no_temp_files(&dir);

    let _ = fs::remove_dir_all(&dir);
}

/// 上書き保存が成立すること。前の内容が残らないこと。
#[test]
fn overwriting_replaces_the_previous_contents() {
    let dir = test_dir("overwrite");
    let path = dir.join("drawing.dxf");

    fs::write(&path, "古い内容").expect("下準備の書き込み");
    dxf::write::write_to_file(&sample_doc(), &path).expect("上書きできるはず");

    let text = fs::read_to_string(&path).expect("読めるはず");
    assert!(!text.contains("古い内容"), "前の内容が残っていない");
    assert!(text.contains("LINE"));
    assert_no_temp_files(&dir);

    let _ = fs::remove_dir_all(&dir);
}

/// **この形式の存在理由。** 保存が失敗しても、保存前のファイルが無傷であること。
///
/// `std::fs::write` は既存ファイルを切り詰めてから書くので、この状況で
/// 元の内容が失われる。置き換えに失敗したときに「前の図面が残っている」ことを固定する。
///
/// 失敗を起こす手立てとして、保存先を**中身のあるディレクトリ**にする。
/// ファイルをディレクトリへ `rename` することはできないので、置き換えの段階で必ず失敗する。
#[test]
fn failed_save_leaves_the_existing_file_untouched() {
    let dir = test_dir("fail");

    // 保存先を、中身のあるディレクトリにする。
    let target = dir.join("drawing.dxf");
    fs::create_dir_all(&target).expect("ディレクトリを作れるはず");
    let inside = target.join("大事なもの.txt");
    fs::write(&inside, "失われてはいけない").expect("下準備の書き込み");

    let err = dxf::write::write_to_file(&sample_doc(), &target)
        .expect_err("ディレクトリへは保存できないので失敗するはず");
    assert!(
        matches!(err, cad_core::CadError::Io(_)),
        "入出力エラーとして報告されること: {err:?}"
    );

    // 保存先は手つかずで、中身も無事。
    assert!(target.is_dir(), "保存先が壊されていないこと");
    assert_eq!(
        fs::read_to_string(&inside).expect("中身が読めるはず"),
        "失われてはいけない",
        "失敗した保存が既存のデータを壊していないこと"
    );
    // 一時ファイルは保存先の親（= dir）に作られる。片付けられていること。
    assert_no_temp_files(&dir);

    let _ = fs::remove_dir_all(&dir);
}

/// 一時ファイルが作れない場合も、エラーとして返ること（panic しないこと）。
#[test]
fn save_into_a_missing_directory_is_an_error() {
    let dir = test_dir("missing");
    let path = dir.join("ない階層").join("drawing.dxf");

    let err = dxf::write::write_to_file(&sample_doc(), &path)
        .expect_err("親ディレクトリが無いので失敗するはず");
    assert!(matches!(err, cad_core::CadError::Io(_)), "{err:?}");
    assert!(!path.exists(), "ファイルが作られていないこと");

    let _ = fs::remove_dir_all(&dir);
}

/// 同じ保存先へ連続で保存できること。
///
/// 一時ファイル名がプロセス ID だけだと 2 回目が衝突する。連番を持つ理由の固定。
#[test]
fn repeated_saves_to_the_same_path_all_succeed() {
    let dir = test_dir("repeat");
    let path = dir.join("drawing.dxf");
    let doc = sample_doc();

    for i in 0..5 {
        dxf::write::write_to_file(&doc, &path).unwrap_or_else(|e| panic!("{i} 回目で失敗: {e}"));
    }

    assert!(fs::read_to_string(&path).is_ok_and(|t| t.contains("LINE")));
    assert_no_temp_files(&dir);

    let _ = fs::remove_dir_all(&dir);
}

// 裸のファイル名（ディレクトリ部分が無いパス）の扱いは、カレントディレクトリを
// 変える必要があってテスト間で干渉する（`set_current_dir` はプロセス全体に効く）。
// 純粋関数として `atomic_write` モジュール内の単体テストで固定している。
