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

use std::path::Path;

use cad_core::dxf;
use cad_core::Document;

/// ファイルダイアログで使う拡張子。
const DXF_EXTENSION: &str = "dxf";

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
        let Some(path) = rfd::FileDialog::new()
            .add_filter("DXF 図面", &[DXF_EXTENSION])
            .set_title("DXF を開く")
            .pick_file()
        else {
            return FileOutcome::Nothing;
        };

        match dxf::read::read_from_file(&path) {
            Ok(mut loaded) => {
                loaded.mark_saved(Some(path.clone()));
                *doc = loaded;
                FileOutcome::Ok(format!("開きました: {}", path.display()))
            }
            Err(e) => FileOutcome::Failed(format!("読み込みに失敗しました: {e}")),
        }
    }

    fn save_as(doc: &mut Document) -> FileOutcome {
        let default_name = doc.path().and_then(|p| p.file_name()).map_or_else(
            || "drawing.dxf".to_owned(),
            |n| n.to_string_lossy().into_owned(),
        );

        let Some(path) = rfd::FileDialog::new()
            .add_filter("DXF 図面", &[DXF_EXTENSION])
            .set_title("名前を付けて保存")
            .set_file_name(default_name)
            .save_file()
        else {
            return FileOutcome::Nothing;
        };

        // 拡張子を補う。
        let path = if path.extension().is_some() {
            path
        } else {
            path.with_extension(DXF_EXTENSION)
        };
        Self::save_to(doc, &path)
    }

    fn save_to(doc: &mut Document, path: &Path) -> FileOutcome {
        match dxf::write::write_to_file(doc, path) {
            Ok(warnings) => {
                doc.mark_saved(Some(path.to_path_buf()));
                let mut msg = format!("保存しました: {}", path.display());
                // DXF R12 で表現できず近似したものは黙って落とさず必ず伝える (ADR-0021)。
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
}
