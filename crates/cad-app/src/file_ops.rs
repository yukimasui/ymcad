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
//!
//! # ファイルダイアログは UI スレッドを止めない（Issue #98、ADR-0047）
//!
//! 開く・名前を付けて保存は、ダイアログを別スレッドで開き（[`crate::file_dialog`]）、
//! 結果が届くまでアプリをモーダルにして待つ。1 フレームの流れは
//! [`FileOps::poll`]（結果を受けて開く・保存する）→ 各モーダルの描画 → 操作の要求 →
//! [`FileOps::launch`]（フレームの最後に、頼まれたダイアログを開く）。

use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use cad_core::{dxf, native, Document};

use crate::file_dialog::{
    unavailable_message, DialogBackend, DialogReply, DialogRequest, Filters, RfdDialogs,
};

/// ダイアログを待っている間、結果を見に起きる間隔。結果を送ったスレッドも再描画を起こすので、
/// これは取りこぼしの保険。
const WAITING_REPAINT: Duration = Duration::from_millis(250);

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
    /// 成功（保存など）。図面はそのまま。表示するメッセージ。
    Ok(String),
    /// 成功して、図面を入れ替えた（NEW / OPEN）。表示するメッセージ。
    ///
    /// 呼び出し側は選択やスナップなど、前の図面に結びついた状態を捨てる。
    /// 保存（[`Self::Ok`]）と分けるのは、保存のたびに選択が外れていたため（Issue #41）。
    Replaced(String),
    /// 失敗。表示するメッセージ。
    Failed(String),
    /// アプリを終了してよい。
    Quit,
}

/// ダイアログで選んだ後に何をするか。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DialogPurpose {
    /// 選んだ図面を開く。
    Open,
    /// 選んだ名前で保存する。保存できたら `then` を続ける（未保存確認の「保存する」から来たとき）。
    SaveAs {
        /// 保存できた後に続ける操作。
        then: Option<FileAction>,
    },
}

/// 開いているファイルダイアログ。
#[derive(Debug)]
struct OpenDialog {
    purpose: DialogPurpose,
    request: DialogRequest,
    /// 結果の届くチャネル。`None` はまだ開いていない（次の [`FileOps::launch`] で開く）。
    reply: Option<mpsc::Receiver<DialogReply>>,
}

/// ファイル操作の状態。
#[derive(Debug)]
pub struct FileOps {
    /// 未保存確認の対象。
    pending: Option<PendingAction>,
    /// モーダルを開いた直後で、「保存する」にフォーカスを置く必要があるか。
    focus_save: bool,
    /// 開いている（または開こうとしている）ファイルダイアログ。
    dialog: Option<OpenDialog>,
    /// ダイアログを開く仕組み（テストでは差し替える）。
    backend: Box<dyn DialogBackend>,
}

impl Default for FileOps {
    fn default() -> Self {
        Self::new()
    }
}

impl FileOps {
    /// 初期状態。ダイアログは OS のもの（rfd）。
    #[must_use]
    pub fn new() -> Self {
        Self::with_backend(Box::new(RfdDialogs::new(Filters {
            native: NATIVE_EXTENSION,
            exchange: DXF_EXTENSION,
        })))
    }

    /// ダイアログを開く仕組みを指定して作る（テストで rfd の代わりを入れるため）。
    #[must_use]
    pub fn with_backend(backend: Box<dyn DialogBackend>) -> Self {
        Self {
            pending: None,
            focus_save: false,
            dialog: None,
            backend,
        }
    }

    /// 確認ダイアログを出している最中か。
    #[must_use]
    pub fn is_confirming(&self) -> bool {
        self.pending.is_some()
    }

    /// ファイルダイアログの結果を待っている最中か。
    #[must_use]
    pub fn is_choosing_file(&self) -> bool {
        self.dialog.is_some()
    }

    /// モーダル（未保存確認かファイルダイアログ待ち）の最中か。
    ///
    /// この間は作図・別のファイル操作・終了を受け付けない。ダイアログを待っている間に
    /// 図面が変わると、開く前に済ませた未保存確認が無意味になる（変更が黙って捨てられる）。
    #[must_use]
    pub fn is_modal(&self) -> bool {
        self.is_confirming() || self.is_choosing_file()
    }

    /// 操作を要求する。未保存なら確認を挟み、そうでなければ即実行する。
    ///
    /// ファイルダイアログを待っている間は何もしない（二重に開かない・待っている間に図面を
    /// 入れ替えない）。
    pub fn request(&mut self, action: FileAction, doc: &mut Document) -> FileOutcome {
        if self.is_choosing_file() {
            return FileOutcome::Nothing;
        }
        if action.discards_document() && doc.is_dirty() {
            self.pending = Some(PendingAction(action));
            self.focus_save = true;
            return FileOutcome::Nothing;
        }
        self.execute(action, doc)
    }

    /// 頼まれたファイルダイアログを開く。フレームの最後に呼ぶ（親ウィンドウに `frame` が要るため）。
    ///
    /// ブロックしない。結果は [`Self::poll`] で受け取る。
    pub fn launch(&mut self, parent: Option<&eframe::Frame>, ctx: &egui::Context) {
        let Some(dialog) = &mut self.dialog else {
            return;
        };
        if dialog.reply.is_none() {
            dialog.reply = Some(self.backend.spawn(dialog.request.clone(), parent, ctx));
        }
    }

    /// ファイルダイアログの結果が届いていれば、選んだファイルを開く・保存する。
    pub fn poll(&mut self, doc: &mut Document) -> FileOutcome {
        let Some(OpenDialog {
            reply: Some(rx), ..
        }) = &self.dialog
        else {
            return FileOutcome::Nothing;
        };
        let reply = match rx.try_recv() {
            Ok(reply) => reply,
            Err(mpsc::TryRecvError::Empty) => return FileOutcome::Nothing,
            // ダイアログのスレッドが結果を送らずに終わった（panic など）。
            Err(mpsc::TryRecvError::Disconnected) => {
                DialogReply::Unavailable("ダイアログのスレッドが結果を返さずに終わりました".into())
            }
        };
        let Some(OpenDialog {
            purpose, request, ..
        }) = self.dialog.take()
        else {
            return FileOutcome::Nothing;
        };
        self.finish_dialog(purpose, &request, reply, doc)
    }

    /// ダイアログの結果を受けて、今までの「開く」「名前を付けて保存」の後半を流す。
    fn finish_dialog(
        &mut self,
        purpose: DialogPurpose,
        request: &DialogRequest,
        reply: DialogReply,
        doc: &mut Document,
    ) -> FileOutcome {
        let path = match reply {
            DialogReply::Picked(path) => path,
            DialogReply::Cancelled => {
                if let DialogPurpose::SaveAs { then: Some(action) } = purpose {
                    // 未保存確認の「保存する」から来た保存をキャンセルした。確認へ戻す
                    // （今までどおり。元の操作へ進まず、「保存しない」も選び直せる）。
                    self.pending = Some(PendingAction(action));
                    self.focus_save = true;
                }
                return FileOutcome::Nothing;
            }
            DialogReply::Unavailable(reason) => {
                return FileOutcome::Failed(unavailable_message(request, &reason));
            }
        };
        match purpose {
            DialogPurpose::Open => Self::open_path(doc, &path),
            DialogPurpose::SaveAs { then } => {
                // 拡張子を省略されたらネイティブ形式にする。
                let path = if path.extension().is_some() {
                    path
                } else {
                    path.with_extension(NATIVE_EXTENSION)
                };
                let saved = Self::save_to(doc, &path);
                match (saved, then) {
                    (FileOutcome::Ok(msg), Some(action)) => {
                        self.continue_after_save(msg, action, doc)
                    }
                    (saved, _) => saved,
                }
            }
        }
    }

    /// 保存できた後に元の操作（NEW / OPEN / QUIT）へ進む。
    fn continue_after_save(
        &mut self,
        saved_msg: String,
        action: FileAction,
        doc: &mut Document,
    ) -> FileOutcome {
        match self.execute(action, doc) {
            // 続きがダイアログを開いた（OPEN）ときは、保存できたことを先に伝える。
            FileOutcome::Nothing => FileOutcome::Ok(saved_msg),
            other => other,
        }
    }

    /// ファイルダイアログを頼む。実際に開くのは [`Self::launch`]。
    fn start_dialog(&mut self, purpose: DialogPurpose, doc: &Document) {
        let request = match purpose {
            DialogPurpose::Open => DialogRequest::Open,
            DialogPurpose::SaveAs { .. } => DialogRequest::Save {
                // 既に保存先があるならその名前を出す（形式も引き継がれる）。
                default_name: doc.path().and_then(|p| p.file_name()).map_or_else(
                    || format!("drawing.{NATIVE_EXTENSION}"),
                    |n| n.to_string_lossy().into_owned(),
                ),
            },
        };
        self.dialog = Some(OpenDialog {
            purpose,
            request,
            reply: None,
        });
    }

    /// ファイルダイアログを待っている間のモーダルを描く。
    ///
    /// ダイアログは別のウィンドウ（OS のもの）なので、ここには選ぶ部品は無い。ダイアログが
    /// 見当たらない・返ってこないときのために「待つのをやめる」だけ置く。
    pub fn show_waiting(&mut self, ctx: &egui::Context) -> FileOutcome {
        let Some(dialog) = &self.dialog else {
            return FileOutcome::Nothing;
        };
        let title = dialog.request.title();
        // 裏の部品へキーが流れないよう、確認のモーダルと同じキーを奪う。
        ctx.input_mut(|i| {
            for key in [egui::Key::Enter, egui::Key::Space, egui::Key::Escape] {
                i.consume_key(egui::Modifiers::NONE, key);
            }
        });
        let mut abandon = false;
        egui::Modal::new(egui::Id::new("choosing_file"))
            .backdrop_color(egui::Color32::from_black_alpha(150))
            .show(ctx, |ui| {
                ui.set_width(420.0);
                ui.heading("ファイルを選んでいます…");
                ui.add_space(6.0);
                ui.label(format!(
                    "「{title}」のダイアログで選ぶか、キャンセルしてください。"
                ));
                ui.label("選び終わるまで、作図とファイル操作は止めています。");
                ui.add_space(10.0);
                if ui
                    .button("待つのをやめる")
                    .on_hover_text(
                        "ダイアログが見つからない・閉じないときに使います。\
                         後からダイアログで選んでも反映されません",
                    )
                    .clicked()
                {
                    abandon = true;
                }
            });
        // 待っている間も結果を受け取れるよう、ときどき起こす（結果を送ったスレッドも起こす）。
        ctx.request_repaint_after(WAITING_REPAINT);
        if abandon {
            self.dialog = None;
            return FileOutcome::Failed(format!(
                "「{title}」の結果を待つのをやめました。開いたままのダイアログで選んでも反映されません"
            ));
        }
        FileOutcome::Nothing
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

        let focus_save = std::mem::take(&mut self.focus_save);
        // Enter / Space はボタンを描く前に奪う。egui のボタンはフォーカスがあると Enter / Space で
        // 押されるので、Tab を 1 回押した後の Enter が「保存しない」になり、図面が捨てられる。
        // Enter は必ず「保存する」、Space は何もしない。「保存しない」はクリックでしか押せない。
        // キーリピートは「押した」ことにしない（`NEW` + Enter の長押しが、出てきた
        // モーダルの「保存する」を押してしまう）。消費はするので「保存しない」にも届かない。
        let enter = ctx.input_mut(|i| {
            let pressed = i.events.iter().any(|e| {
                matches!(
                    e,
                    egui::Event::Key {
                        key: egui::Key::Enter,
                        pressed: true,
                        repeat: false,
                        ..
                    }
                )
            });
            i.consume_key(egui::Modifiers::NONE, egui::Key::Enter);
            pressed
        });
        ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Space));
        let mut save = false;
        let mut discard = false;
        egui::Modal::new(egui::Id::new("unsaved_changes"))
            // 裏のコマンドラインの文字が目に入りにくいよう、既定（alpha 100）より少し暗くする。
            .backdrop_color(egui::Color32::from_black_alpha(150))
            .show(ctx, |ui| {
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
                    // Enter = 保存する。開いたときにここへフォーカスを置く。「保存しない」には
                    // Enter も 1 文字のキーも割り当てない（変更を失う操作を誤打で起こさない）。
                    let save_button = ui.button("保存する (Enter)");
                    if focus_save {
                        save_button.request_focus();
                    }
                    // フォーカスの枠を「保存する」から動かさない。Enter は常に「保存する」なので、
                    // 枠が「保存しない」へ動くと、枠と Enter の結果が食い違う。Tab は egui が
                    // フレームの最初に処理するため、消費では間に合わない。TextEdit と同じ
                    // フォーカスロックのフィルタで、Tab・矢印を egui のフォーカス移動に使わせない。
                    if save_button.has_focus() {
                        ui.memory_mut(|m| {
                            m.set_focus_lock_filter(
                                save_button.id,
                                egui::EventFilter {
                                    tab: true,
                                    horizontal_arrows: true,
                                    vertical_arrows: true,
                                    escape: false,
                                },
                            );
                        });
                    }
                    if save_button.clicked() || enter {
                        save = true;
                    }
                    if ui.button("保存しない").clicked() {
                        discard = true;
                    }
                    if ui.button("キャンセル (Esc)").clicked() {
                        close = true;
                    }
                });
            });

        if save {
            if doc.path().is_some() {
                match self.execute(FileAction::Save, doc) {
                    FileOutcome::Ok(msg) => {
                        // 保存できたときだけ元の操作へ進む。
                        outcome = self.continue_after_save(msg, action, doc);
                    }
                    failed => outcome = failed,
                }
            } else {
                // 保存先が無い。名前を付けて保存のダイアログを開き、保存できたら元の操作へ
                // 進む（[`Self::finish_dialog`]）。キャンセルしたらこの確認へ戻る。
                self.start_dialog(DialogPurpose::SaveAs { then: Some(action) }, doc);
            }
            close = true;
        } else if discard {
            outcome = self.execute(action, doc);
            close = true;
        }

        // Esc = キャンセル。`should_close()` は背景のクリックでも真になるが、誤クリックで
        // 閉じたくないので Esc だけを自分で見る（Issue #24）。
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            close = true;
        }

        if close {
            self.pending = None;
        }
        outcome
    }

    /// 確認を挟まずに実行する。ダイアログが要る操作は、ダイアログを頼んで `Nothing` を返す。
    fn execute(&mut self, action: FileAction, doc: &mut Document) -> FileOutcome {
        match action {
            FileAction::New => {
                *doc = Document::new();
                FileOutcome::Replaced("新規図面を作成しました".to_owned())
            }
            FileAction::Open => {
                self.start_dialog(DialogPurpose::Open, doc);
                FileOutcome::Nothing
            }
            FileAction::Save => match doc.path().map(Path::to_path_buf) {
                Some(path) => Self::save_to(doc, &path),
                None => {
                    self.start_dialog(DialogPurpose::SaveAs { then: None }, doc);
                    FileOutcome::Nothing
                }
            },
            FileAction::SaveAs => {
                self.start_dialog(DialogPurpose::SaveAs { then: None }, doc);
                FileOutcome::Nothing
            }
            FileAction::Quit => FileOutcome::Quit,
        }
    }

    /// 選ばれたファイルを開く。形式は拡張子で決める。
    fn open_path(doc: &mut Document, path: &Path) -> FileOutcome {
        let loaded = if is_dxf(path) {
            dxf::read::read_from_file(path)
        } else {
            native::read::read_from_file(path)
        };

        match loaded {
            Ok(mut loaded) => {
                loaded.mark_saved(Some(path.to_path_buf()));
                *doc = loaded;
                FileOutcome::Replaced(format!("開きました: {}", path.display()))
            }
            Err(e) => FileOutcome::Failed(format!("読み込みに失敗しました: {e}")),
        }
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
        assert!(
            matches!(outcome, FileOutcome::Replaced(_)),
            "新規作成は図面の入れ替え"
        );
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

        // 落ちても消えるよう、Drop で消える一時ディレクトリを使う。
        let dir = crate::test_util::TempDir::new("fileops");

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

        let dir = crate::test_util::TempDir::new("warn");

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
    }

    // ---- ファイルダイアログ（Issue #98） --------------------------------

    fn ops_with_fake() -> (FileOps, crate::file_dialog::FakeDialogs) {
        let fake = crate::file_dialog::FakeDialogs::default();
        (FileOps::with_backend(Box::new(fake.clone())), fake)
    }

    /// 開くはダイアログを頼むだけでブロックせず、結果が届くまで待つ。その間は別の操作を受け付けない。
    #[test]
    fn open_waits_for_the_dialog_without_blocking() {
        let ctx = egui::Context::default();
        let dir = crate::test_util::TempDir::new("fileops_open");
        let path = dir.join("a.ymc");
        native::write::write_to_file(&Document::new(), &path).expect("書ける");

        let (mut ops, fake) = ops_with_fake();
        let mut doc = Document::new();
        assert_eq!(
            ops.request(FileAction::Open, &mut doc),
            FileOutcome::Nothing
        );
        assert!(ops.is_choosing_file() && ops.is_modal(), "待っている");
        assert!(fake.opened().is_empty(), "開くのはフレームの最後（launch）");

        ops.launch(None, &ctx);
        ops.launch(None, &ctx);
        assert_eq!(fake.opened(), vec![DialogRequest::Open], "1 回だけ開く");
        assert_eq!(ops.poll(&mut doc), FileOutcome::Nothing, "まだ結果が無い");

        assert_eq!(
            ops.request(FileAction::New, &mut doc),
            FileOutcome::Nothing,
            "待っている間は新規作成しない"
        );
        assert_eq!(
            ops.request(FileAction::Quit, &mut doc),
            FileOutcome::Nothing
        );
        ops.launch(None, &ctx);
        assert_eq!(fake.opened().len(), 1, "二重に開かない");

        assert!(fake.reply(DialogReply::Picked(path.clone())));
        assert!(matches!(ops.poll(&mut doc), FileOutcome::Replaced(_)));
        assert!(!ops.is_modal());
        assert_eq!(doc.path(), Some(path.as_path()));
    }

    /// ダイアログのスレッドが結果を送らずに終わったら、待ち続けずに失敗を伝える。
    #[test]
    fn a_dialog_that_never_replies_is_reported_as_a_failure() {
        let (mut ops, fake) = ops_with_fake();
        let mut doc = Document::new();
        ops.request(FileAction::SaveAs, &mut doc);
        ops.launch(None, &egui::Context::default());
        fake.drop_all();
        assert!(matches!(ops.poll(&mut doc), FileOutcome::Failed(_)));
        assert!(!ops.is_modal(), "待つのをやめる");
    }

    /// 名前を付けて保存の既定の名前は、保存先があればそのファイル名（形式も引き継ぐ）。
    #[test]
    fn save_as_suggests_the_current_file_name() {
        let (mut ops, fake) = ops_with_fake();
        let mut doc = Document::new();
        ops.request(FileAction::Save, &mut doc);
        ops.launch(None, &egui::Context::default());
        assert_eq!(
            fake.opened(),
            vec![DialogRequest::Save {
                default_name: "drawing.ymc".into()
            }],
            "保存先の無い上書き保存は名前を付けて保存になる"
        );
        assert!(fake.reply(DialogReply::Cancelled));
        assert_eq!(ops.poll(&mut doc), FileOutcome::Nothing);

        doc.mark_saved(Some(std::path::PathBuf::from("/tmp/図面.dxf")));
        ops.request(FileAction::SaveAs, &mut doc);
        ops.launch(None, &egui::Context::default());
        assert_eq!(
            fake.opened()[1],
            DialogRequest::Save {
                default_name: "図面.dxf".into()
            }
        );
    }
}
