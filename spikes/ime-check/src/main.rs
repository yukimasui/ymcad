//! ymcad Phase 0 — IME 検証スパイク
//!
//! egui の `TextEdit` 上で Ubuntu の日本語入力 (ibus / fcitx5) が成立するかを、
//! **人間が目視で判定できる形** に可視化するための最小アプリ。
//!
//! 判定してほしい 5 項目:
//!   1. 変換候補ウィンドウが表示されるか
//!   2. 候補ウィンドウがテキストカーソル位置に追従するか
//!   3. 未確定文字列 (プリエディット) がインライン表示されるか
//!   4. 確定した文字列が正しくバッファに入るか
//!   5. IME 有効化 API (`set_ime_allowed` 等) の明示的な呼び出しが必要か
//!
//! 事前にソースで確認済みの事実:
//!
//! - egui-winit 0.36 が `set_ime_allowed` / `set_ime_cursor_area` を自動で呼ぶ
//!   (egui-winit-0.36.1/src/lib.rs:1152, 1177)。本スパイクはアプリ側から IME 系 API を
//!   一切呼んでいないので、「これで動くなら明示呼び出しは不要」と結論できる (項目 5)。
//! - `ImeEvent::Enabled` / `Disabled` は egui 0.36 で `#[deprecated]`、かつ egui-winit が
//!   winit の該当イベントを捨てている。**これらは決して飛んでこないのが正常**。
//! - 候補ウィンドウの位置は `IMEOutput::cursor_rect` ではなく `IMEOutput::rect`
//!   (TextEdit ウィジェット全体の矩形) を `set_ime_cursor_area` に渡して決まる。
//!   つまり候補ウィンドウは「キャレット直下」ではなく「入力欄の左端」に出るのが
//!   egui 0.36 の仕様。項目 2 はこれを踏まえて判定すること。
//! - `WINIT_UNIX_BACKEND` は winit 0.30 で廃止済み。X11 で試すときは
//!   `WAYLAND_DISPLAY= cargo run` と、環境変数を空にして起動する。

mod jp_font;

use eframe::egui;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([820.0, 700.0])
            .with_title("ymcad Phase 0 — IME 検証スパイク"),
        ..Default::default()
    };

    eframe::run_native(
        "ymcad-ime-check",
        options,
        Box::new(|cc| {
            let font = jp_font::install(&cc.egui_ctx);
            Ok(Box::new(App::new(font)))
        }),
    )
}

/// 画面ログ 1 行分。
struct LogLine {
    frame: u64,
    time: f64,
    kind: Kind,
    text: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Preedit,
    Commit,
    DeleteSurrounding,
    ImeOther,
    Text,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Self::Preedit => "Ime::Preedit",
            Self::Commit => "Ime::Commit",
            Self::DeleteSurrounding => "Ime::DeleteSurrounding",
            Self::ImeOther => "Ime::(other)",
            Self::Text => "Text",
        }
    }

    fn color(self) -> egui::Color32 {
        match self {
            Self::Preedit => egui::Color32::from_rgb(0xff, 0xc1, 0x07),
            Self::Commit => egui::Color32::from_rgb(0x4c, 0xaf, 0x50),
            Self::DeleteSurrounding => egui::Color32::from_rgb(0xff, 0x70, 0x43),
            Self::ImeOther => egui::Color32::GRAY,
            Self::Text => egui::Color32::from_rgb(0x64, 0xb5, 0xf6),
        }
    }
}

struct App {
    /// 画面中央の TextEdit (ウィンドウ中程での候補ウィンドウ追従を見る)
    center_input: String,
    /// 画面下部の TextEdit (ymcad 本体のコマンドラインと同じ位置での挙動を見る)
    bottom_input: String,
    log: Vec<LogLine>,
    log_text_events: bool,
    n_preedit: usize,
    n_commit: usize,
    /// 直近フレームで egui が要求した IME 位置 (これが出ていれば set_ime_cursor_area が呼ばれている)
    ime_output: Option<egui::output::IMEOutput>,
    frame: u64,
    font: Option<jp_font::LoadedFont>,
    env: Vec<(&'static str, String)>,
}

impl App {
    fn new(font: Option<jp_font::LoadedFont>) -> Self {
        let env = [
            "XDG_SESSION_TYPE",
            "WAYLAND_DISPLAY",
            "DISPLAY",
            "GTK_IM_MODULE",
            "QT_IM_MODULE",
            "XMODIFIERS",
        ]
        .into_iter()
        .map(|k| (k, std::env::var(k).unwrap_or_else(|_| "(未設定)".to_owned())))
        .collect();

        Self {
            center_input: String::new(),
            bottom_input: String::new(),
            log: Vec::new(),
            log_text_events: true,
            n_preedit: 0,
            n_commit: 0,
            ime_output: None,
            frame: 0,
            font,
            env,
        }
    }

    /// このフレームの生イベントを拾ってログに積む。
    fn collect_events(&mut self, ctx: &egui::Context) {
        let (events, time) = ctx.input(|i| (i.events.clone(), i.time));

        for ev in events {
            let (kind, text) = match ev {
                egui::Event::Ime(ime) => self.describe_ime(&ime),
                egui::Event::Text(t) if self.log_text_events => {
                    (Kind::Text, format!("{t:?}"))
                }
                _ => continue,
            };

            self.log.push(LogLine {
                frame: self.frame,
                time,
                kind,
                text,
            });
        }

        // 無制限に伸ばさない。
        const MAX: usize = 400;
        if self.log.len() > MAX {
            self.log.drain(..self.log.len() - MAX);
        }
    }

    #[allow(deprecated)] // Enabled / Disabled は egui 0.36 では未使用だが、届くか確認したいので明示的に扱う
    fn describe_ime(&mut self, ime: &egui::ImeEvent) -> (Kind, String) {
        match ime {
            egui::ImeEvent::Preedit {
                text,
                active_range_chars,
            } => {
                self.n_preedit += 1;
                let range = match active_range_chars {
                    Some(r) => format!("{}..{}", r.start, r.end),
                    None => "なし".to_owned(),
                };
                let note = if text.is_empty() {
                    "  ← 空 = 変換の取り消し / IME 解除"
                } else {
                    ""
                };
                (
                    Kind::Preedit,
                    format!("text={text:?}  active_range={range}{note}"),
                )
            }
            egui::ImeEvent::Commit(text) => {
                self.n_commit += 1;
                (Kind::Commit, format!("text={text:?}"))
            }
            egui::ImeEvent::DeleteSurrounding {
                before_chars,
                after_chars,
            } => (
                Kind::DeleteSurrounding,
                format!("before={before_chars} after={after_chars}"),
            ),
            egui::ImeEvent::Enabled => (Kind::ImeOther, "Enabled".to_owned()),
            egui::ImeEvent::Disabled => (Kind::ImeOther, "Disabled".to_owned()),
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.frame += 1;
        self.collect_events(&ctx);

        egui::Panel::top("header").show(ui, |ui| {
            ui.add_space(4.0);
            ui.heading("ymcad Phase 0 — IME 検証スパイク");
            ui.label(
                "下または中央の入力欄に半角/全角キーで IME を ON にして「にほんご」と打ち、\
                 変換・確定してください。受信したイベントは下のログに出ます。",
            );
            ui.add_space(2.0);

            ui.collapsing("環境情報 / フォント", |ui| {
                egui::Grid::new("env").num_columns(2).show(ui, |ui| {
                    for (k, v) in &self.env {
                        ui.monospace(*k);
                        ui.monospace(v);
                        ui.end_row();
                    }
                });
                ui.separator();
                match &self.font {
                    Some(f) => ui.monospace(format!(
                        "日本語フォント: {} (face index {})",
                        f.path.display(),
                        f.index
                    )),
                    None => ui.colored_label(
                        egui::Color32::RED,
                        "日本語フォントが見つかりません。日本語は □ になります。\n\
                         `sudo apt install fonts-noto-cjk` を実行してください。",
                    ),
                };
                ui.monospace(
                    "このアプリは set_ime_allowed / set_ime_cursor_area を自前では呼んでいません。\n\
                     (egui-winit が自動で呼びます)",
                );
            });
            ui.add_space(4.0);
        });

        egui::Panel::bottom("cmdline").show(ui, |ui| {
            ui.add_space(6.0);
            ui.label("② 下部の入力欄 (ymcad 本体のコマンドラインと同じ位置):");
            ui.add(
                egui::TextEdit::singleline(&mut self.bottom_input)
                    .hint_text("コマンド:")
                    .desired_width(f32::INFINITY)
                    .font(egui::TextStyle::Monospace),
            );
            ui.add_space(6.0);
        });

        egui::CentralPanel::default().show(ui, |ui| {
            ui.label("① 中央の入力欄:");
            ui.add(
                egui::TextEdit::singleline(&mut self.center_input)
                    .hint_text("ここに日本語を入力")
                    .desired_width(f32::INFINITY),
            );

            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.label("確定済みバッファ:");
                ui.monospace(format!("中央={:?}", self.center_input));
                ui.monospace(format!("下部={:?}", self.bottom_input));
            });

            // TextEdit の描画後に読むこと。ここに値が出ていれば
            // egui は IME 位置を winit へ伝えている。
            self.ime_output = ctx.output(|o| o.ime);
            match self.ime_output {
                Some(ime) => {
                    ui.colored_label(
                        egui::Color32::from_rgb(0x4c, 0xaf, 0x50),
                        "【項目5】set_ime_allowed(true) が egui-winit により自動で呼ばれている状態",
                    );
                    ui.monospace(format!(
                        "  候補ウィンドウのアンカー rect = ({:.1}, {:.1})-({:.1}, {:.1})  ← これが set_ime_cursor_area に渡る",
                        ime.rect.min.x, ime.rect.min.y, ime.rect.max.x, ime.rect.max.y,
                    ));
                    ui.monospace(format!(
                        "  キャレット cursor_rect     = ({:.1}, {:.1})-({:.1}, {:.1})  ← egui-winit は未使用",
                        ime.cursor_rect.min.x,
                        ime.cursor_rect.min.y,
                        ime.cursor_rect.max.x,
                        ime.cursor_rect.max.y,
                    ));
                }
                None => {
                    ui.monospace(
                        "【項目5】IME 位置要求なし = set_ime_allowed(false)。入力欄をクリックすると true に変わります。",
                    );
                }
            }

            ui.separator();

            ui.horizontal(|ui| {
                ui.label(format!(
                    "Preedit 受信 {} 回 / Commit 受信 {} 回",
                    self.n_preedit, self.n_commit
                ));
                ui.separator();
                ui.checkbox(&mut self.log_text_events, "Text イベントも記録");
                if ui.button("ログを消去").clicked() {
                    self.log.clear();
                    self.n_preedit = 0;
                    self.n_commit = 0;
                }
            });

            ui.add_space(4.0);
            ui.label(
                "イベントログ (新しいものが下)。期待: Preedit が数回 → Commit。\
                 Enabled / Disabled は egui 0.36 では廃止済みで、飛んでこないのが正常です。",
            );

            egui::ScrollArea::vertical()
                .stick_to_bottom(true)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if self.log.is_empty() {
                        ui.weak("まだイベントを受信していません。");
                    }
                    for line in &self.log {
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            ui.monospace(format!("#{:>5} {:>8.2}s", line.frame, line.time));
                            ui.colored_label(line.kind.color(), line.kind.label());
                            ui.monospace(&line.text);
                        });
                    }
                });
        });
    }
}
