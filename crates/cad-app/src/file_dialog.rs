//! OS のファイルダイアログを、UI スレッドを止めずに開く（Issue #98、ADR-0047）。
//!
//! # なぜ別スレッドか
//!
//! `rfd::FileDialog::pick_file` / `save_file` はダイアログが閉じるまで戻らない。UI スレッドで
//! 呼ぶと winit のイベントループが止まり、再描画もウィンドウマネージャの ping への応答も
//! できなくなる（GNOME が「応答していません」を出す）。ここではダイアログを別スレッドで開き、
//! 結果をチャネルで返す。UI 側（[`crate::file_ops::FileOps`]）は待っている間アプリをモーダルにする。
//!
//! # 「出せなかった」と「キャンセル」を分ける
//!
//! rfd 0.17 の既定のバックエンド（`xdg-portal`）は、xdg-desktop-portal が使えなければ zenity へ
//! フォールバックし、zenity も起動できなければ `log::error!` を出して `None` を返す。戻り値は
//! キャンセルと同じ `None` なので区別できない。そこで、ダイアログを開いたスレッドで rfd が
//! **`rfd::backend::xdg_desktop_portal` モジュールから** error を出したかを、ロガーで拾う
//! （[`RfdFailureLog`]）。このモジュールの error は「zenity でも開けなかった」の 1 か所だけで、
//! portal 側の失敗（`…::portal` 以下のモジュール）はフォールバックの前なので数えない。
//! 判定の根拠と限界は ADR-0047。rfd を上げるときは、この前提を読み直すこと
//! （ワークスペースの `Cargo.toml` で版を固定している）。

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::mpsc;

/// ダイアログで何をするか。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DialogRequest {
    /// 図面を開く。
    Open,
    /// 名前を付けて保存する。`default_name` は最初に入っているファイル名。
    Save {
        /// 最初に入れておくファイル名。
        default_name: String,
    },
}

impl DialogRequest {
    /// ダイアログの題（待っている間の表示にも使う）。
    #[must_use]
    pub fn title(&self) -> &'static str {
        match self {
            Self::Open => "図面を開く",
            Self::Save { .. } => "名前を付けて保存",
        }
    }
}

/// ダイアログの結果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DialogReply {
    /// ファイルが選ばれた。
    Picked(PathBuf),
    /// ユーザーがキャンセルした。
    Cancelled,
    /// ダイアログを出せなかった。理由（rfd のエラーの文）。
    Unavailable(String),
}

/// ダイアログを開く仕組み。テストでは差し替える（rfd のダイアログは返ってこないため）。
pub trait DialogBackend: std::fmt::Debug {
    /// ダイアログを開き始める。**ブロックしない。** 結果は返したチャネルに 1 回だけ届く。
    ///
    /// `parent` … 親ウィンドウ（portal のダイアログをアプリに対してモーダルにする）。
    /// `ctx` … 結果を送ったら再描画を起こすため。
    fn spawn(
        &mut self,
        request: DialogRequest,
        parent: Option<&eframe::Frame>,
        ctx: &egui::Context,
    ) -> mpsc::Receiver<DialogReply>;
}

/// ネイティブ形式・交換用形式のフィルタの名前と拡張子。
#[derive(Clone, Copy, Debug)]
pub struct Filters {
    /// ネイティブ形式の拡張子。
    pub native: &'static str,
    /// 交換用形式の拡張子。
    pub exchange: &'static str,
}

/// 本物の OS のダイアログ（rfd）。
#[derive(Debug)]
pub struct RfdDialogs {
    filters: Filters,
}

impl RfdDialogs {
    /// フィルタを与えて作る。
    #[must_use]
    pub fn new(filters: Filters) -> Self {
        Self { filters }
    }
}

impl DialogBackend for RfdDialogs {
    fn spawn(
        &mut self,
        request: DialogRequest,
        parent: Option<&eframe::Frame>,
        ctx: &egui::Context,
    ) -> mpsc::Receiver<DialogReply> {
        // テストでファイルダイアログを開くと返ってこない。理由つきで落として止める。
        if cfg!(test) {
            panic!(
                "テストで rfd のダイアログを開こうとした: {}（FileOps::with_backend で差し替えること）",
                request.title()
            );
        }
        // ネイティブ形式を先に並べて既定にする。
        let mut dialog = rfd::FileDialog::new()
            .add_filter("ymcad 図面", &[self.filters.native])
            .add_filter("DXF 図面（交換用）", &[self.filters.exchange])
            .set_title(request.title());
        if let Some(frame) = parent {
            // 親の取得に失敗したら付けないだけ（rfd が `ok()` で捨てる）。
            dialog = dialog.set_parent(frame);
        }
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            // 前のダイアログの記録が残っていても混ざらないよう、開く前に捨てる。
            let _ = take_failure();
            let picked = match request {
                DialogRequest::Open => dialog.pick_file(),
                DialogRequest::Save { default_name } => {
                    dialog.set_file_name(default_name).save_file()
                }
            };
            let reply = classify(picked, take_failure());
            // 受け手が待つのをやめていたら届かないが、それでよい。
            let _ = tx.send(reply);
            ctx.request_repaint();
        });
        rx
    }
}

/// rfd の戻り値と、そのスレッドで拾った失敗の記録から結果を決める。
fn classify(picked: Option<PathBuf>, failure: Option<String>) -> DialogReply {
    match (picked, failure) {
        (Some(path), _) => DialogReply::Picked(path),
        (None, Some(reason)) => DialogReply::Unavailable(reason),
        (None, None) => DialogReply::Cancelled,
    }
}

/// ダイアログを出せなかったときに出す文。理由と対処。
#[must_use]
pub fn unavailable_message(request: &DialogRequest, reason: &str) -> String {
    format!(
        "「{}」のダイアログを出せませんでした（{reason}）。\
         xdg-desktop-portal か zenity が必要です。\
         Ubuntu / WSL では `sudo apt install zenity` で入ります",
        request.title()
    )
}

// ---- rfd の失敗を拾うロガー ------------------------------------------------

/// 「zenity でも開けなかった」を rfd が出すモジュール（`log` の target は `module_path!()`）。
///
/// portal の失敗は `rfd::backend::xdg_desktop_portal::portal` 以下から出て、その後 zenity へ
/// フォールバックするので、**完全一致**で比べる（前方一致にすると、portal が無く zenity で
/// 開いてキャンセルしたときまで「出せなかった」になる）。
const RFD_DIALOG_TARGET: &str = "rfd::backend::xdg_desktop_portal";

thread_local! {
    /// このスレッドで rfd がダイアログを出せなかった記録。
    static FAILURE: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// このスレッドの記録を取り出して空にする。
fn take_failure() -> Option<String> {
    FAILURE.with(|f| f.borrow_mut().take())
}

/// rfd の error を拾うロガー。rfd の警告以上は標準エラーにも出す（今まではロガーが無く、
/// 黙って捨てられていた）。
#[derive(Debug)]
pub struct RfdFailureLog;

impl RfdFailureLog {
    fn is_rfd(target: &str) -> bool {
        target == "rfd" || target.starts_with("rfd::")
    }
}

impl log::Log for RfdFailureLog {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Warn && Self::is_rfd(metadata.target())
    }

    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        if record.level() == log::Level::Error && record.target() == RFD_DIALOG_TARGET {
            let text = record.args().to_string();
            FAILURE.with(|f| *f.borrow_mut() = Some(text));
        }
        eprintln!(
            "[{}] {}: {}",
            record.level(),
            record.target(),
            record.args()
        );
    }

    fn flush(&self) {}
}

/// ロガーを入れる。起動時に 1 回だけ呼ぶ。
///
/// 既に別のロガーがあれば入らない（そのときは「出せなかった」をキャンセルと区別できない
/// ので、標準エラーに書いておく）。
pub fn install_logger() {
    static LOGGER: RfdFailureLog = RfdFailureLog;
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(log::LevelFilter::Warn);
    } else {
        eprintln!(
            "ymcad: 別のロガーが入っているため、ファイルダイアログを出せないことを検出できません"
        );
    }
}

/// テスト用のダイアログ。開くよう頼まれた内容を記録し、結果はテストが [`FakeDialogs::reply`] で返す。
#[cfg(test)]
#[derive(Clone, Debug, Default)]
pub struct FakeDialogs {
    opened: std::rc::Rc<RefCell<Vec<DialogRequest>>>,
    waiting: std::rc::Rc<RefCell<std::collections::VecDeque<mpsc::Sender<DialogReply>>>>,
}

#[cfg(test)]
impl FakeDialogs {
    /// これまでに開くよう頼まれたダイアログ（古い順）。
    pub fn opened(&self) -> Vec<DialogRequest> {
        self.opened.borrow().clone()
    }

    /// 開いているダイアログのうちいちばん古いものに結果を返す。受け手が待つのをやめていたら `false`。
    pub fn reply(&self, reply: DialogReply) -> bool {
        let tx = self
            .waiting
            .borrow_mut()
            .pop_front()
            .expect("開いているダイアログが無い");
        tx.send(reply).is_ok()
    }

    /// 結果を返さずにダイアログを捨てる（スレッドが結果を送らずに終わった場合）。
    pub fn drop_all(&self) {
        self.waiting.borrow_mut().clear();
    }
}

#[cfg(test)]
impl DialogBackend for FakeDialogs {
    fn spawn(
        &mut self,
        request: DialogRequest,
        _parent: Option<&eframe::Frame>,
        _ctx: &egui::Context,
    ) -> mpsc::Receiver<DialogReply> {
        let (tx, rx) = mpsc::channel();
        self.opened.borrow_mut().push(request);
        self.waiting.borrow_mut().push_back(tx);
        rx
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use log::Log as _;

    fn emit(level: log::Level, target: &str, text: &str) {
        RfdFailureLog.log(
            &log::Record::builder()
                .level(level)
                .target(target)
                .args(format_args!("{text}"))
                .build(),
        );
    }

    /// zenity でも開けなかった（rfd がダイアログのモジュールから error を出した）ら、出せなかった。
    #[test]
    fn an_error_from_the_dialog_module_means_unavailable() {
        let _ = take_failure();
        emit(
            log::Level::Error,
            RFD_DIALOG_TARGET,
            "Failed to pick file with zenity: No such file or directory (os error 2)",
        );
        let reply = classify(None, take_failure());
        assert_eq!(
            reply,
            DialogReply::Unavailable(
                "Failed to pick file with zenity: No such file or directory (os error 2)".into()
            )
        );
        assert_eq!(take_failure(), None, "取り出したら空になる");
    }

    /// portal の失敗（フォールバックの前）だけなら、キャンセルとして扱う。
    ///
    /// portal が無く zenity で開いてキャンセルした場合がこれ。数えると、キャンセルのたびに
    /// 「出せませんでした」が出てしまう。
    #[test]
    fn portal_errors_before_the_fallback_are_not_failures() {
        let _ = take_failure();
        emit(
            log::Level::Error,
            "rfd::backend::xdg_desktop_portal::portal::libdbus",
            "Failed to connect to session bus",
        );
        emit(
            log::Level::Error,
            "rfd::backend::xdg_desktop_portal::portal",
            "OpenFile failed",
        );
        emit(log::Level::Warn, RFD_DIALOG_TARGET, "Using zenity fallback");
        assert_eq!(classify(None, take_failure()), DialogReply::Cancelled);
    }

    /// 他のクレートの error は拾わない。
    #[test]
    fn errors_from_other_crates_are_ignored() {
        let _ = take_failure();
        emit(log::Level::Error, "wgpu_core::device", "x");
        emit(log::Level::Error, "rfdx", "x");
        assert_eq!(take_failure(), None);
    }

    /// 記録はスレッドごと。別のスレッドの失敗は混ざらない（ダイアログは 1 回ごとに別スレッド）。
    #[test]
    fn failures_are_recorded_per_thread() {
        let _ = take_failure();
        std::thread::spawn(|| emit(log::Level::Error, RFD_DIALOG_TARGET, "other thread"))
            .join()
            .unwrap();
        assert_eq!(take_failure(), None);
    }

    /// 選ばれたら、失敗の記録があっても選ばれたパスを使う。
    #[test]
    fn a_picked_path_wins() {
        assert_eq!(
            classify(Some(PathBuf::from("/a.ymc")), Some("x".into())),
            DialogReply::Picked(PathBuf::from("/a.ymc"))
        );
    }

    /// 出せなかったときの文に、理由と対処（zenity の入れ方）が入る。
    #[test]
    fn the_unavailable_message_gives_the_reason_and_the_remedy() {
        let msg = unavailable_message(&DialogRequest::Open, "os error 2");
        assert!(msg.contains("図面を開く"), "{msg}");
        assert!(msg.contains("os error 2"), "{msg}");
        assert!(msg.contains("sudo apt install zenity"), "{msg}");
    }
}
