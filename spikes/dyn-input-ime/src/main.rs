//! カーソル追従の入力欄 (ダイナミック入力風) と日本語 IME の相性を確かめる試作。
//!
//! 画面全体をキャンバスとみなし、マウスの右下に `egui::Area` を浮かせて
//! 単一行 `TextEdit` を置く。F2 で「常に追従」と「変換中は固定」を切り替えて、
//! どちらで ibus + mozc の変換・確定・候補ウィンドウが壊れないかを比較する。

mod jp_font;

use eframe::egui;

/// カーソルからのオフセット (論理 px)。
const OFFSET: f32 = 16.0;
/// 入力欄の幅。
const INPUT_WIDTH: f32 = 260.0;
/// 端での回り込み判定に使う入力欄の高さ見積もり (Area の枠込み)。
const INPUT_HEIGHT: f32 = 40.0;
const HISTORY_MAX: usize = 5;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([960.0, 640.0])
            .with_title("ymcad spike — カーソル追従入力 × IME"),
        ..Default::default()
    };
    eframe::run_native(
        "ymcad-dyn-input-ime",
        options,
        Box::new(|cc| {
            let font = jp_font::install(&cc.egui_ctx);
            Ok(Box::new(App::new(font)))
        }),
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// 変換中でも毎フレームマウスに追従する。
    AlwaysFollow,
    /// Preedit が非空の間は、変換開始時点の位置に固定する。
    FreezeWhileComposing,
}

impl Mode {
    fn label(self) -> &'static str {
        match self {
            Self::AlwaysFollow => "モード1: 常に追従",
            Self::FreezeWhileComposing => "モード2: 変換中は固定",
        }
    }

    fn toggled(self) -> Self {
        match self {
            Self::AlwaysFollow => Self::FreezeWhileComposing,
            Self::FreezeWhileComposing => Self::AlwaysFollow,
        }
    }
}

struct App {
    mode: Mode,
    buffer: String,
    history: Vec<String>,
    composing: bool,
    /// 直近フレームで Area に使った位置 (固定時の基準にもなる)。
    last_pos: egui::Pos2,
    /// 固定中の位置。`Some` の間はマウスを無視する。
    frozen_pos: Option<egui::Pos2>,
    last_mouse: egui::Pos2,
    font: Option<jp_font::LoadedFont>,
}

impl App {
    fn new(font: Option<jp_font::LoadedFont>) -> Self {
        Self {
            mode: Mode::AlwaysFollow,
            buffer: String::new(),
            history: Vec::new(),
            composing: false,
            last_pos: egui::pos2(100.0, 100.0),
            frozen_pos: None,
            last_mouse: egui::pos2(100.0, 100.0),
            font,
        }
    }

    /// カーソルの右下。右端・下端に近ければ左側・上側へ回り込ませる。
    fn place(cursor: egui::Pos2, screen: egui::Rect) -> egui::Pos2 {
        let mut p = cursor + egui::vec2(OFFSET, OFFSET);
        if p.x + INPUT_WIDTH > screen.right() {
            p.x = cursor.x - OFFSET - INPUT_WIDTH;
        }
        if p.y + INPUT_HEIGHT > screen.bottom() {
            p.y = cursor.y - OFFSET - INPUT_HEIGHT;
        }
        p.x = p.x.max(screen.left());
        p.y = p.y.max(screen.top());
        p
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        // --- 入力イベントの解釈 (Area 描画より前) ---
        // 変換中の Enter は IME の確定なので、フレーム開始時点の状態で判定する。
        let was_composing = self.composing;
        let mut saw_ime_event = false;
        let mut now_composing = was_composing;
        let (toggle, enter, mouse) = ctx.input(|i| {
            for ev in &i.events {
                if let egui::Event::Ime(ime) = ev {
                    saw_ime_event = true;
                    match ime {
                        egui::ImeEvent::Preedit { text, .. } => now_composing = !text.is_empty(),
                        egui::ImeEvent::Commit(_) => now_composing = false,
                        _ => {}
                    }
                }
            }
            (
                i.key_pressed(egui::Key::F2),
                i.key_pressed(egui::Key::Enter),
                i.pointer.latest_pos(),
            )
        });

        if toggle {
            self.mode = self.mode.toggled();
            self.frozen_pos = None;
        }
        if let Some(m) = mouse {
            self.last_mouse = m;
        }

        // 変換開始の瞬間に、直前フレームの位置で固定する。
        if self.mode == Mode::FreezeWhileComposing {
            if now_composing && !was_composing {
                self.frozen_pos = Some(self.last_pos);
            }
            if !now_composing {
                self.frozen_pos = None;
            }
        } else {
            self.frozen_pos = None;
        }
        self.composing = now_composing;

        // 変換中でも IME イベントを伴わない Enter だけが「確定した文字列を履歴へ」。
        // Commit と同フレームの Enter も IME の確定キーなので除外する。
        if enter && !was_composing && !now_composing && !saw_ime_event && !self.buffer.is_empty() {
            let text = std::mem::take(&mut self.buffer);
            self.history.push(text);
            if self.history.len() > HISTORY_MAX {
                self.history.remove(0);
            }
        }

        // --- キャンバス (画面全体) + 左上の説明 ---
        let screen = ctx.content_rect();
        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading(self.mode.label());
            ui.add_space(2.0);
            let (state, color) = if self.composing {
                ("[変換中]", egui::Color32::from_rgb(0xff, 0xc1, 0x07))
            } else {
                ("(変換していない)", egui::Color32::GRAY)
            };
            ui.colored_label(color, state);
            ui.label(format!("バッファ: {:?}", self.buffer));
            ui.label("履歴 (直近5件):");
            if self.history.is_empty() {
                ui.weak("  (なし)");
            }
            for h in &self.history {
                ui.monospace(format!("  {h}"));
            }
            ui.separator();
            ui.label("【確認手順】");
            ui.label("1. マウスを動かしながら nihongo と打ち、変換 (Space) → 確定 (Enter)");
            ui.label("2. F2 でモード切替。変換中にマウスを動かして挙動を比べる");
            ui.label("3. 候補ウィンドウが入力欄 (カーソル付近) に出るか・ずれないか");
            ui.label("4. 変換中の Enter で履歴に入らず、確定後の Enter で入るか");
            ui.label("5. 画面の右端・下端で入力欄が左・上に回り込むか");
            match &self.font {
                Some(f) => ui.weak(format!("font: {} #{}", f.path.display(), f.index)),
                None => ui.colored_label(egui::Color32::RED, "日本語フォントが見つかりません"),
            };
        });

        // --- カーソル追従の入力欄 ---
        let pos = self
            .frozen_pos
            .unwrap_or_else(|| Self::place(self.last_mouse, screen));
        self.last_pos = pos;

        let edit_id = egui::Id::new("dyn_input_edit");
        ctx.memory_mut(|m| m.request_focus(edit_id));

        egui::Area::new(egui::Id::new("dyn_input_area"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .interactable(true)
            .show(&ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.buffer)
                            .id(edit_id)
                            .desired_width(INPUT_WIDTH)
                            .hint_text("ここに入力"),
                    );
                });
            });

        // マウスが止まっていても追従/固定の切り替えが反映されるよう、常に再描画。
        ctx.request_repaint();
    }
}
