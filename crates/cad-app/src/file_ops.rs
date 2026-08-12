//! ファイル操作（新規 / 開く / 保存 / 名前を付けて保存）と未保存確認。
//!
//! # 未保存の変更を失わせない
//!
//! 新規・開く・終了はいずれも現在の図面を捨てる操作なので、
//! 未保存の変更があるときは必ず確認を挟む。
//!
//! 確認が必要な操作は即座に実行せず [`PendingAction`] として保留し、
//! ユーザーが「保存する / 保存しない」を選んでから実行する。
//! 「破棄しますか？」だけを聞いて保存の機会を与えない作りにはしない。
//!
//! # 2 つのファイル形式
//!
//! | 形式 | 役割 | 往復 |
//! |---|---|---|
//! | `.ymc` | **ネイティブ。既定** | 無損失 |
//! | `.dxf` | 交換用（他の CAD とのやりとり） | 非可逆。保存時に警告が出る |
//!
//! **形式は拡張子だけで決める。** 他に判別材料を持ち込まない
//! （「前回の形式を覚える」等の隠れた状態を作らない）。
//!
//! `.dxf` で開いたファイルの上書き保存は **`.dxf` のまま**にする。
//! 勝手に別の形式へ移すほうが驚きが大きい。非可逆であることは
//! 保存のたびに出る警告で伝わり続ける。

use std::path::Path;

use cad_core::{dxf, native, Document};

/// 交換用形式の拡張子。
const DXF_EXTENSION: &str = "dxf";

/// ネイティブ形式の拡張子。
const NATIVE_EXTENSION: &str = native::EXTENSION;

/// パスが交換用形式（DXF）を指しているか。
///
/// 大文字小文字は無視する（`DRAWING.DXF` も DXF として扱う）。
/// **これ以外の判別方法を作らないこと。**
fn is_dxf(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(DXF_EXTENSION))
}

/// ユーザーが要求したファイル操作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileAction {
    /// 新規図面。
    New,
    /// 開く。
    Open,
    /// 上書き保存（パスが無ければ名前を付けて保存になる）。
    Save,
    /// 名前を付けて保存。
    SaveAs,
    /// アプリを終了する。
    Quit,
}

impl FileAction {
    /// 現在の図面を捨てる操作か。未保存確認が必要かの判断に使う。
    #[must_use]
    pub fn discards_document(self) -> bool {
        matches!(self, Self::New | Self::Open | Self::Quit)
    }

    /// 履歴表示に使うコマンド名。
    #[must_use]
    pub fn command_name(self) -> &'static str {
        match self {
            Self::New => "NEW",
            Self::Open => "OPEN",
            Self::Save => "SAVE",
            Self::SaveAs => "SAVEAS",
            Self::Quit => "QUIT",
        }
    }

    /// 確認ダイアログに出す操作名。
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::New => "新規作成",
            Self::Open => "ファイルを開く",
            Self::Save => "保存",
            Self::SaveAs => "名前を付けて保存",
            Self::Quit => "終了",
        }
    }
}

/// 確認待ちの操作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingAction(pub FileAction);

/// 操作の結果。コマンドラインへ出すメッセージを返す。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileOutcome {
    /// 何も起きなかった（ダイアログでキャンセルされた等）。
    Nothing,
    /// 成功。表示するメッセージ。
    Ok(String),
    /// 失敗。表示するメッセージ。
    Failed(String),
    /// アプリを終了してよい。
    Quit,
}

/// ファイル操作の状態。
#[derive(Debug, Default)]
pub struct FileOps {
    /// 未保存確認の対象。
    pending: Option<PendingAction>,
}

impl FileOps {
    /// 初期状態。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 確認ダイアログを出している最中か。
    #[must_use]
    pub fn is_confirming(&self) -> bool {
        self.pending.is_some()
    }

    /// 操作を要求する。未保存なら確認を挟み、そうでなければ即実行する。
    pub fn request(&mut self, action: FileAction, doc: &mut Document) -> FileOutcome {
        if action.discards_document() && doc.is_dirty() {
            self.pending = Some(PendingAction(action));
            return FileOutcome::Nothing;
        }
        Self::execute(action, doc)
    }

    /// 未保存確認のモーダルを描画する。
    ///
    /// 決着がついたらその結果を返す。
    pub fn show_confirm(&mut self, ctx: &egui::Context, doc: &mut Document) -> FileOutcome {
        let Some(PendingAction(action)) = self.pending else {
            return FileOutcome::Nothing;
        };

        let mut outcome = FileOutcome::Nothing;
        let mut close = false;

        egui::Modal::new(egui::Id::new("unsaved_changes")).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.heading("保存されていない変更があります");
            ui.add_space(6.0);
            let name = doc.path().and_then(|p| p.file_name()).map_or_else(
                || "(名称未設定)".to_owned(),
                |n| n.to_string_lossy().into_owned(),
            );
            ui.label(format!("{name} の変更を保存しますか？"));
            ui.label(format!("この後 {} を行います。", action.label()));
            ui.add_space(10.0);

            ui.horizontal(|ui| {
                if ui.button("保存する").clicked() {
                    match Self::execute(FileAction::Save, doc) {
                        FileOutcome::Ok(msg) => {
                            // 保存できたときだけ元の操作へ進む。
                            outcome = match Self::execute(action, doc) {
                                FileOutcome::Nothing => FileOutcome::Ok(msg),
                                other => other,
                            };
                            close = true;
                        }
                        FileOutcome::Nothing => {
                            // 保存ダイアログがキャンセルされた。何もしない。
                        }
                        failed => {
                            outcome = failed;
                            close = true;
                        }
                    }
                }
                if ui.button("保存しない").clicked() {
                    outcome = Self::execute(action, doc);
                    close = true;
                }
                if ui.button("キャンセル").clicked() {
                    close = true;
                }
            });
        });

        if close {
            self.pending = None;
        }
        outcome
    }

    /// 確認を挟まずに実行する。
    fn execute(action: FileAction, doc: &mut Document) -> FileOutcome {
        match action {
            FileAction::New => {
                *doc = Document::new();
                FileOutcome::Ok("新規図面を作成しました".to_owned())
            }
            FileAction::Open => Self::open(doc),
            FileAction::Save => match doc.path().map(Path::to_path_buf) {
                Some(path) => Self::save_to(doc, &path),
                None => Self::save_as(doc),
            },
            FileAction::SaveAs => Self::save_as(doc),
            FileAction::Quit => FileOutcome::Quit,
        }
    }

    fn open(doc: &mut Document) -> FileOutcome {
        // ネイティブ形式を先に並べて既定にする。
        let Some(path) = rfd::FileDialog::new()
            .add_filter("ymcad 図面", &[NATIVE_EXTENSION])
            .add_filter("DXF 図面（交換用）", &[DXF_EXTENSION])
            .set_title("図面を開く")
            .pick_file()
        else {
            return FileOutcome::Nothing;
        };

        let loaded = if is_dxf(&path) {
            dxf::read::read_from_file(&path)
        } else {
            native::read::read_from_file(&path)
        };

        match loaded {
            Ok(mut loaded) => {
                loaded.mark_saved(Some(path.clone()));
                *doc = loaded;
                FileOutcome::Ok(format!("開きました: {}", path.display()))
            }
            Err(e) => FileOutcome::Failed(format!("読み込みに失敗しました: {e}")),
        }
    }

    fn save_as(doc: &mut Document) -> FileOutcome {
        // 既に保存先があるならその名前を出す（形式も引き継がれる）。
        let default_name = doc.path().and_then(|p| p.file_name()).map_or_else(
            || format!("drawing.{NATIVE_EXTENSION}"),
            |n| n.to_string_lossy().into_owned(),
        );

        let Some(path) = rfd::FileDialog::new()
            .add_filter("ymcad 図面", &[NATIVE_EXTENSION])
            .add_filter("DXF 図面（交換用）", &[DXF_EXTENSION])
            .set_title("名前を付けて保存")
            .set_file_name(default_name)
            .save_file()
        else {
            return FileOutcome::Nothing;
        };

        // 拡張子を省略されたらネイティブ形式にする。
        let path = if path.extension().is_some() {
            path
        } else {
            path.with_extension(NATIVE_EXTENSION)
        };
        Self::save_to(doc, &path)
    }

    fn save_to(doc: &mut Document, path: &Path) -> FileOutcome {
        // 拡張子で形式を決める。DXF だけが警告を返す。
        let result = if is_dxf(path) {
            dxf::write::write_to_file(doc, path)
        } else {
            native::write::write_to_file(doc, path).map(|()| Vec::new())
        };

        match result {
            Ok(warnings) => {
                doc.mark_saved(Some(path.to_path_buf()));
                let mut msg = format!("保存しました: {}", path.display());
                // DXF R12 で表現できず近似したものは黙って落とさず必ず伝える (ADR-0021)。
                // ネイティブ形式は無損失なので、ここに来る警告は無い。
                for w in warnings {
                    msg.push_str("\n  警告: ");
                    msg.push_str(&w);
                }
                FileOutcome::Ok(msg)
            }
            Err(e) => FileOutcome::Failed(format!("保存に失敗しました: {e}")),
        }
    }
}

/// このフレームのキー入力からファイル操作を拾う。
///
/// `Ctrl+N` / `Ctrl+O` / `Ctrl+S` / `Ctrl+Shift+S`。
#[must_use]
pub fn shortcut(ctx: &egui::Context) -> Option<FileAction> {
    ctx.input_mut(|i| {
        let ctrl = egui::Modifiers::CTRL;
        let ctrl_shift = egui::Modifiers::CTRL.plus(egui::Modifiers::SHIFT);
        // 名前を付けて保存を先に判定する。Ctrl+S と食い合うため。
        if i.consume_key(ctrl_shift, egui::Key::S) {
            Some(FileAction::SaveAs)
        } else if i.consume_key(ctrl, egui::Key::S) {
            Some(FileAction::Save)
        } else if i.consume_key(ctrl, egui::Key::O) {
            Some(FileAction::Open)
        } else if i.consume_key(ctrl, egui::Key::N) {
            Some(FileAction::New)
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discarding_actions_are_identified() {
        assert!(FileAction::New.discards_document());
        assert!(FileAction::Open.discards_document());
        assert!(FileAction::Quit.discards_document());
        assert!(!FileAction::Save.discards_document());
        assert!(!FileAction::SaveAs.discards_document());
    }

    #[test]
    fn starts_without_confirmation() {
        assert!(!FileOps::new().is_confirming());
    }

    /// 変更が無ければ確認を挟まずに新規作成すること。
    #[test]
    fn new_on_clean_document_runs_immediately() {
        let mut doc = Document::new();
        let mut ops = FileOps::new();
        assert!(!doc.is_dirty());

        let outcome = ops.request(FileAction::New, &mut doc);
        assert!(matches!(outcome, FileOutcome::Ok(_)));
        assert!(!ops.is_confirming());
    }

    /// 未保存の変更があれば確認を挟むこと。
    #[test]
    fn new_on_dirty_document_asks_first() {
        use cad_core::command::AddEntities;
        use cad_core::geom::{Line, Point2};
        use cad_core::{Entity, Geometry, LayerId};

        let mut doc = Document::new();
        doc.apply(Box::new(AddEntities::one(
            "LINE",
            Entity::new(
                Geometry::Line(Line::new(Point2::ORIGIN, Point2::new(1.0, 0.0))),
                LayerId::ZERO,
            ),
        )))
        .unwrap();
        assert!(doc.is_dirty());

        let mut ops = FileOps::new();
        let outcome = ops.request(FileAction::New, &mut doc);
        assert_eq!(outcome, FileOutcome::Nothing, "即座には実行しない");
        assert!(ops.is_confirming(), "確認を待つ");
        assert_eq!(doc.entities().len(), 1, "図面はまだ捨てられていない");
    }

    /// 変更が無ければ終了要求はそのまま通ること。
    #[test]
    fn quit_on_clean_document_passes_through() {
        let mut doc = Document::new();
        let mut ops = FileOps::new();
        assert_eq!(ops.request(FileAction::Quit, &mut doc), FileOutcome::Quit);
    }

    #[test]
    fn action_labels_are_present() {
        for a in [
            FileAction::New,
            FileAction::Open,
            FileAction::Save,
            FileAction::SaveAs,
            FileAction::Quit,
        ] {
            assert!(!a.label().is_empty());
        }
    }

    // ---- 形式の振り分け -------------------------------------------------

    /// 交換用形式（DXF）と判定されるのは `.dxf` だけであること。
    #[test]
    fn only_the_dxf_extension_selects_the_exchange_format() {
        for p in ["a.dxf", "/tmp/図面.dxf", "a.b.dxf"] {
            assert!(is_dxf(Path::new(p)), "{p} は DXF のはず");
        }
        for p in [
            "a.ymc",
            "/tmp/図面.ymc",
            "a",         // 拡張子なし
            "a.dxf.ymc", // 末尾が .ymc
            "dxf",       // 拡張子ではなくファイル名
            "a.dwg",     // 別形式
        ] {
            assert!(!is_dxf(Path::new(p)), "{p} は DXF でないはず");
        }
    }

    /// 大文字小文字を無視すること。
    ///
    /// 他の CAD が `.DXF` で書き出すことがあるので、拾えないと開けなくなる。
    #[test]
    fn the_dxf_extension_is_matched_case_insensitively() {
        for p in ["a.DXF", "a.Dxf", "a.dXf"] {
            assert!(is_dxf(Path::new(p)), "{p} は DXF のはず");
        }
    }

    /// 拡張子が 2 つの形式で衝突していないこと。
    #[test]
    fn the_two_extensions_are_distinct() {
        assert_ne!(NATIVE_EXTENSION, DXF_EXTENSION);
        assert_eq!(
            NATIVE_EXTENSION, "ymc",
            "既存ファイルが開けなくなるので変えない"
        );
    }

    /// 保存が形式ごとに正しく振り分けられ、読み戻せること。
    ///
    /// `save_to` はダイアログを開かないので、テストから直接叩ける。
    #[test]
    fn save_to_dispatches_on_the_extension() {
        use cad_core::command::AddEntities;
        use cad_core::geom::{Line, Point2};
        use cad_core::{Entity, Geometry, LayerId};

        let dir = std::env::temp_dir().join(format!("ymcad_fileops_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("テスト用ディレクトリ");

        let mut doc = Document::new();
        doc.apply(Box::new(AddEntities::one(
            "LINE",
            Entity::new(
                Geometry::Line(Line::new(Point2::ORIGIN, Point2::new(3.0, 4.0))),
                LayerId::ZERO,
            ),
        )))
        .unwrap();

        // ネイティブ形式。
        let ymc = dir.join("drawing.ymc");
        assert!(matches!(
            FileOps::save_to(&mut doc, &ymc),
            FileOutcome::Ok(_)
        ));
        assert!(!doc.is_dirty(), "保存済みになること");
        assert_eq!(doc.path(), Some(ymc.as_path()), "保存先が記録されること");
        assert!(
            native::read::read_from_file(&ymc).is_ok(),
            "ネイティブ形式として読めること"
        );

        // 交換用形式。
        let dxf_path = dir.join("drawing.dxf");
        assert!(matches!(
            FileOps::save_to(&mut doc, &dxf_path),
            FileOutcome::Ok(_)
        ));
        assert!(
            dxf::read::read_from_file(&dxf_path).is_ok(),
            "DXF として読めること"
        );
        assert_eq!(
            doc.path(),
            Some(dxf_path.as_path()),
            "保存先が DXF へ移ること（以降の上書き保存も DXF のまま）"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **ネイティブ形式の保存では警告が出ないこと。**
    ///
    /// 作図線とグループを含む図面は DXF では警告が出る。同じ図面を
    /// ネイティブ形式で保存したときに警告が出ないことが、無損失であることの表れ。
    #[test]
    fn native_save_reports_no_warnings_where_dxf_does() {
        use cad_core::command::{AddEntities, CreateGroup};
        use cad_core::geom::{Point2, Vec2, Xline};
        use cad_core::{Entity, EntityId, Geometry, LayerId};

        let dir = std::env::temp_dir().join(format!("ymcad_warn_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("テスト用ディレクトリ");

        let mut doc = Document::new();
        let x = Xline::new(Point2::ORIGIN, Vec2::new(1.0, 1.0)).expect("作図線");
        doc.apply(Box::new(AddEntities::one(
            "XLINE",
            Entity::new(Geometry::Xline(x), LayerId::ZERO),
        )))
        .unwrap();
        let ids: Vec<EntityId> = doc.entities().ids().collect();
        doc.apply(Box::new(CreateGroup::new("GROUP", "組", ids)))
            .unwrap();

        let FileOutcome::Ok(dxf_msg) = FileOps::save_to(&mut doc, &dir.join("a.dxf")) else {
            panic!("DXF 保存は成功するはず");
        };
        assert!(
            dxf_msg.contains("警告"),
            "DXF では非可逆であることを伝えるはず: {dxf_msg}"
        );

        let FileOutcome::Ok(ymc_msg) = FileOps::save_to(&mut doc, &dir.join("a.ymc")) else {
            panic!("ネイティブ保存は成功するはず");
        };
        assert!(
            !ymc_msg.contains("警告"),
            "ネイティブ形式は無損失なので警告は出ないはず: {ymc_msg}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
