//! リボン（画面上端のタブつきアイコンバー）。Issue #26、ADR-0037。
//!
//! - 1 段目にタブ、その直後（区切り線の右）にどのタブでも押せるクイックアクセス
//!   （UNDO / REDO / SAVE）
//! - 2 段目にグループ（枠とグループ名）ごとのアイコンボタン
//! - **ボタンを押すことはコマンド名を打つのと同じ扱い。** このモジュールは押された
//!   コマンド名を返すだけで、始めるのは呼び出し側（`Session::start_command_from_ui`）
//! - 幅が足りないときは横スクロール（マウスホイールの縦回転でも横に動く）。送れる側の端を
//!   背景色へ消すぼかしと「›」で示し、その帯を押すと表示幅の 3/4 だけ送る
//! - 実行中のコマンドのボタンは選択色で強調する。そのコマンドが別のタブにあれば、
//!   そのタブの名前の下に選択色の下線を引く
//!
//! ボタンはキーボードのフォーカスを取らない。`Sense::CLICK`（`FOCUSABLE` を含まない）で
//! 作るので、Tab 移動でも止まらない（`Sense::click()` は `FOCUSABLE` を含む）。押した後も
//! キー入力はそのままコマンドラインへ流れる（ADR-0035 の `owns_keys` を壊さない）。
//!
//! 選択中のタブなどの状態はここ（`cad-app` 側）に持ち、`Document` には入れない。

pub mod icons;
pub mod layout;

use crate::tools::{self, CommandSpec};
use layout::{GroupSpec, QUICK_ACCESS, TABS};

/// アイコンの一辺 [px]。
const ICON_PX: f32 = 22.0;
/// クイックアクセス（タブの行）のアイコンの一辺 [px]。タブの見出しの高さに収める。
const QUICK_ICON_PX: f32 = 16.0;
/// ボタンの内側の余白 [px]。
const BUTTON_PAD: f32 = 2.0;
/// ボタンの最小幅 [px]。
const BUTTON_MIN_WIDTH: f32 = 30.0;
/// ボタンの下に出すコマンド名の文字の大きさ [pt]。
const LABEL_SIZE: f32 = 9.0;
/// グループ名の文字の大きさ [pt]。
const GROUP_TITLE_SIZE: f32 = 9.5;
/// ボタンの角の丸み [px]。
const CORNER: u8 = 3;
/// 実行中のコマンドがあるタブに引く下線の太さ [px] と、見出しの左右から詰める幅 [px]。
const TAB_MARK_THICKNESS: f32 = 2.0;
const TAB_MARK_INSET: f32 = 4.0;
/// 横に送れる端のぼかしの幅 [px] と段数。この帯全体が「送る」ボタンになる。
const FADE_WIDTH: f32 = 28.0;
const FADE_STEPS: u16 = 14;
/// 「›」「‹」を押したときに送る量（表示幅に対する割合）。少し重ねて、どこまで見ていたかを残す。
const PAGE_FRACTION: f32 = 0.75;

/// スクロール領域の ID。
const SCROLL_ID: &str = "ribbon_scroll";

/// リボンの UI 状態。
#[derive(Debug, Default)]
pub struct Ribbon {
    /// 開いているタブ（`layout::TABS` の添字）。起動時はホーム（0）。
    tab: usize,
    /// 「›」「‹」で押された送り先（次のフレームでスクロール領域へ渡す）。
    scroll_to: Option<f32>,
    /// 直前に描いた結果（テスト用）。
    #[cfg(test)]
    probe: Probe,
}

/// 直前のフレームに描いたもの（テストで見た目の代わりに状態を検査するため）。
#[cfg(test)]
#[derive(Clone, Debug, Default)]
pub struct Probe {
    /// 描いたタブ（見出し・矩形・実行中のコマンドの印を付けたか）。
    pub tabs: Vec<(&'static str, egui::Rect, bool)>,
    /// 描いたボタン（コマンド名・矩形・強調したか・クイックアクセスか）。
    pub buttons: Vec<DrawnButton>,
    /// ボタンの並びが見えている範囲（スクロール領域の表示範囲）。
    pub viewport: Option<egui::Rect>,
    /// 左・右へ送れることを示したか。
    pub overflow: (bool, bool),
    /// 「‹」「›」の帯（押せる範囲）。出していなければ `None`。
    pub hints: [Option<egui::Rect>; 2],
    /// 横スクロールの位置 [px]。
    pub offset: f32,
    /// リボン全体の矩形。
    pub rect: Option<egui::Rect>,
}

/// 描いたボタン 1 つ（テスト用）。
#[cfg(test)]
#[derive(Clone, Debug)]
pub struct DrawnButton {
    pub name: &'static str,
    pub rect: egui::Rect,
    pub highlighted: bool,
    pub quick: bool,
}

impl Ribbon {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 開いているタブの見出し（テスト用）。
    #[cfg(test)]
    #[must_use]
    pub fn tab_title(&self) -> &'static str {
        TABS[self.tab].title
    }

    /// 直前に描いた結果（テスト用）。
    #[cfg(test)]
    pub fn probe(&self) -> &Probe {
        &self.probe
    }

    /// リボンを描く。押されたボタンのコマンド名を返す。
    ///
    /// `active` … 実行中のコマンド名（`Session::active_command`）。そのボタンを強調する。
    pub fn show(&mut self, ui: &mut egui::Ui, active: Option<&str>) -> Option<&'static str> {
        #[cfg(test)]
        {
            self.probe = Probe::default();
        }
        // アイコン（SVG）を表示する画像ローダー。入っていれば何もしないので毎回呼んでよい。
        // 起動時（main.rs）ではなくここで入れるのは、テスト用のハーネスを含め、
        // リボンを描くどの経路でも入れ忘れないようにするため。
        egui_extras::install_image_loaders(ui.ctx());
        #[cfg(test)]
        let top = ui.min_rect().min;

        let mut pressed = None;
        ui.horizontal(|ui| {
            self.show_tabs(ui, active);
            // クイックアクセスはタブの直後に区切り線を挟んで置く。右端だと 1280px で
            // タブから約 950px 離れ、視線の外になっていた（PR #32 の操作レビュー）。
            ui.separator();
            ui.spacing_mut().item_spacing.x = 1.0;
            for &name in QUICK_ACCESS {
                if self.show_quick_button(ui, name, active == Some(name)) {
                    pressed = Some(name);
                }
            }
        });

        let groups = TABS[self.tab].groups;
        // 横スクロールしかない領域なので、普通のホイール（縦回転）でも横に動かす。
        // egui の既定では Shift を押しながらでないと横に動かず、横ホイールの無いマウスで
        // 右端のボタンに届かない。このパネルの `Ui` だけに効く。
        ui.style_mut().always_scroll_the_only_direction = true;
        let mut area = egui::ScrollArea::horizontal()
            .id_salt(SCROLL_ID)
            .auto_shrink([false, true]);
        if let Some(x) = self.scroll_to.take() {
            area = area.horizontal_scroll_offset(x);
        }
        let output = area.show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for group in groups {
                    if let Some(name) = self.show_group(ui, group, active) {
                        pressed = Some(name);
                    }
                }
            });
        });
        let offset = output.state.offset.x;
        let visible = output.inner_rect;
        let max_offset = (output.content_size.x - visible.width()).max(0.0);
        let overflow = overflow_sides(offset, output.content_size.x, visible.width());
        for (side, (show, dir)) in [(overflow.0, -1.0_f32), (overflow.1, 1.0_f32)]
            .into_iter()
            .enumerate()
        {
            if !show {
                continue;
            }
            let rect = hint_rect(visible, dir);
            // ボタンより後に登録するので、帯の上のクリックはこちらが取る（下のボタンへ届かない）。
            // 当初は絵だけで、「›」を押すと下に隠れたボタン（幅 800px では LAYER）が押されていた
            // （PR #32 の操作レビュー）。
            let response = ui
                .interact(
                    rect,
                    ui.id().with((SCROLL_ID, "hint", side)),
                    egui::Sense::CLICK,
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text(if dir > 0.0 {
                    "右へ送る"
                } else {
                    "左へ送る"
                });
            paint_overflow_hint(ui, rect, dir, response.hovered());
            if response.clicked() {
                let step = dir * visible.width() * PAGE_FRACTION;
                self.scroll_to = Some((offset + step).clamp(0.0, max_offset));
                ui.ctx().request_repaint();
            }
            #[cfg(test)]
            {
                self.probe.hints[side] = Some(rect);
            }
        }
        #[cfg(test)]
        {
            self.probe.viewport = Some(visible);
            self.probe.offset = offset;
            self.probe.overflow = overflow;
            self.probe.rect = Some(egui::Rect::from_min_max(top, ui.min_rect().max));
        }
        pressed
    }

    /// 1 段目のタブ。実行中のコマンドが別のタブにあれば、その見出しの名前の下に
    /// 選択色の下線を引く。
    ///
    /// 当初はタブとタブの間に点を出していたが、区切り文字に見え、どちらのタブの印か
    /// 分かりにくかった（隣の「表示・ファイル」にも「・」がある。PR #32 の操作レビュー）。
    fn show_tabs(&mut self, ui: &mut egui::Ui, active: Option<&str>) {
        let marked = active.and_then(layout::tab_of).filter(|&t| t != self.tab);
        for (i, tab) in TABS.iter().enumerate() {
            let response = ui.selectable_label(self.tab == i, tab.title);
            let mark = marked == Some(i);
            if mark {
                let r = response.rect;
                ui.painter().hline(
                    (r.left() + TAB_MARK_INSET)..=(r.right() - TAB_MARK_INSET),
                    r.bottom() - TAB_MARK_THICKNESS / 2.0,
                    egui::Stroke::new(TAB_MARK_THICKNESS, ui.visuals().selection.stroke.color),
                );
            }
            #[cfg(test)]
            self.probe.tabs.push((tab.title, response.rect, mark));
            let response = if mark {
                response.on_hover_text("このタブのコマンドを実行中")
            } else {
                response
            };
            if response.clicked() {
                self.tab = i;
            }
        }
    }

    /// グループ 1 つ（枠・ボタンの並び・グループ名）。押されたコマンド名を返す。
    fn show_group(
        &mut self,
        ui: &mut egui::Ui,
        group: &GroupSpec,
        active: Option<&str>,
    ) -> Option<&'static str> {
        let mut pressed = None;
        egui::Frame::new()
            .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
            .corner_radius(CORNER)
            .inner_margin(egui::Margin::symmetric(3, 1))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(1.0, 0.0);
                ui.vertical(|ui| {
                    let row = ui.horizontal(|ui| {
                        for &name in group.commands {
                            if self.show_button(ui, name, active == Some(name)) {
                                pressed = Some(name);
                            }
                        }
                    });
                    group_title(ui, group.title, row.response.rect.width());
                });
            });
        pressed
    }

    /// ボタン 1 つ（アイコンと名前）。押されたら `true`。
    fn show_button(&mut self, ui: &mut egui::Ui, name: &'static str, highlighted: bool) -> bool {
        let label_font = egui::FontId::proportional(LABEL_SIZE);
        let galley =
            ui.painter()
                .layout_no_wrap(name.to_owned(), label_font, egui::Color32::PLACEHOLDER);
        let width = (galley.size().x + BUTTON_PAD * 2.0).max(BUTTON_MIN_WIDTH);
        let height = BUTTON_PAD + ICON_PX + galley.size().y + BUTTON_PAD;
        let (rect, response) = allocate_button(ui, name, egui::vec2(width, height));

        if ui.is_rect_visible(rect) {
            let color = paint_button_background(ui, rect, &response, highlighted);
            let icon_rect = egui::Rect::from_center_size(
                egui::pos2(rect.center().x, rect.top() + BUTTON_PAD + ICON_PX / 2.0),
                egui::vec2(ICON_PX, ICON_PX),
            );
            icons::paint(ui, name, icon_rect, color);
            let text_pos = egui::pos2(rect.center().x - galley.size().x / 2.0, icon_rect.bottom());
            ui.painter().galley(text_pos, galley, color);
        }

        #[cfg(test)]
        self.probe.buttons.push(DrawnButton {
            name,
            rect,
            highlighted,
            quick: false,
        });
        with_tooltip(response, name).clicked()
    }

    /// クイックアクセスのボタン（アイコンだけ。名前はツールチップ）。押されたら `true`。
    fn show_quick_button(
        &mut self,
        ui: &mut egui::Ui,
        name: &'static str,
        highlighted: bool,
    ) -> bool {
        let size = egui::vec2(QUICK_ICON_PX + 2.0 * BUTTON_PAD + 2.0, QUICK_ICON_PX + 2.0);
        let (rect, response) = allocate_button(ui, name, size);
        if ui.is_rect_visible(rect) {
            let color = paint_button_background(ui, rect, &response, highlighted);
            let icon_rect = egui::Rect::from_center_size(
                rect.center(),
                egui::vec2(QUICK_ICON_PX, QUICK_ICON_PX),
            );
            icons::paint(ui, name, icon_rect, color);
        }
        #[cfg(test)]
        self.probe.buttons.push(DrawnButton {
            name,
            rect,
            highlighted,
            quick: true,
        });
        with_tooltip(response, name).clicked()
    }
}

/// ボタンの場所を取る。クリックだけを受け、フォーカスは取らない（`Sense::CLICK`）。
/// 読み上げやテストのため、コマンド名をボタンの名前として登録する。
fn allocate_button(
    ui: &mut egui::Ui,
    name: &'static str,
    size: egui::Vec2,
) -> (egui::Rect, egui::Response) {
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::CLICK);
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), name));
    (rect, response)
}

/// ボタンの地（強調・押下・ホバー）を塗り、アイコンと名前に使う色を返す。
///
/// アイコンと名前は同じ色。白一色の SVG をこの色で着色する（ADR-0037）。
fn paint_button_background(
    ui: &egui::Ui,
    rect: egui::Rect,
    response: &egui::Response,
    highlighted: bool,
) -> egui::Color32 {
    let visuals = ui.visuals();
    let fill = if highlighted {
        Some(visuals.selection.bg_fill)
    } else if response.is_pointer_button_down_on() {
        Some(visuals.widgets.active.weak_bg_fill)
    } else if response.hovered() {
        Some(visuals.widgets.hovered.weak_bg_fill)
    } else {
        None
    };
    if let Some(fill) = fill {
        ui.painter().rect_filled(rect, CORNER, fill);
    }
    if highlighted {
        visuals.selection.stroke.color
    } else {
        visuals.text_color()
    }
}

/// ツールチップを付ける（2 行。[`tooltip_lines`]）。
fn with_tooltip(response: egui::Response, name: &str) -> egui::Response {
    let Some(spec) = tools::lookup(name) else {
        return response;
    };
    let (what, typed) = tooltip_lines(spec);
    response.on_hover_ui(|ui| {
        ui.label(what);
        ui.label(egui::RichText::new(typed).weak());
    })
}

/// グループ名をボタンの並びの下に中央寄せで描く。幅は並びと文字の広いほう。
///
/// 色はボタン下の名前と同じ文字色。弱い文字色ではコントラストが低く読みにくかった
/// （PR #32 の操作レビュー。9.5pt でコントラスト比 約 2.7:1）。
fn group_title(ui: &mut egui::Ui, title: &str, row_width: f32) {
    let galley = ui.painter().layout_no_wrap(
        title.to_owned(),
        egui::FontId::proportional(GROUP_TITLE_SIZE),
        ui.visuals().text_color(),
    );
    let size = egui::vec2(row_width.max(galley.size().x), galley.size().y);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let pos = egui::pos2(rect.center().x - galley.size().x / 2.0, rect.top());
    ui.painter().galley(pos, galley, egui::Color32::PLACEHOLDER);
}

/// 横スクロールで (左, 右) にまだ送れるか。端から 0.5px 未満は送れないとみなす
/// （スクロール位置の丸めで、端に着いても印が残らないように）。
#[must_use]
fn overflow_sides(offset: f32, content: f32, visible: f32) -> (bool, bool) {
    let max = (content - visible).max(0.0);
    (offset > 0.5, offset < max - 0.5)
}

/// 「›」（`dir` が正）または「‹」の帯。表示範囲の端から `FADE_WIDTH` の幅。
fn hint_rect(visible: egui::Rect, dir: f32) -> egui::Rect {
    let x = if dir > 0.0 {
        (visible.right() - FADE_WIDTH)..=visible.right()
    } else {
        visible.left()..=(visible.left() + FADE_WIDTH)
    };
    egui::Rect::from_x_y_ranges(x, visible.y_range())
}

/// 帯に、背景色へ消えるぼかしと「›」（「‹」）を描く。乗せているときは地を明るくする。
///
/// スクロールバーは egui の既定でマウスを乗せたときしか出ず、切れ目がグループの境目に
/// 重なると送れることに気づけなかった（PR #32 の操作レビュー）。
fn paint_overflow_hint(ui: &egui::Ui, band: egui::Rect, dir: f32, hovered: bool) {
    let painter = ui.painter();
    let bg = ui.visuals().panel_fill;
    let ink = ui.visuals().strong_text_color();
    let strip = FADE_WIDTH / f32::from(FADE_STEPS);
    let edge = if dir > 0.0 { band.right() } else { band.left() };
    // 端に近いほど濃い。f32::from(u16) で回して `as f32` を避ける。
    for k in 0..FADE_STEPS {
        let t = f32::from(k + 1) / f32::from(FADE_STEPS);
        let outer = edge - dir * (FADE_WIDTH - strip * f32::from(k + 1));
        let inner = outer - dir * strip;
        let rect = egui::Rect::from_x_y_ranges(outer.min(inner)..=outer.max(inner), band.y_range());
        painter.rect_filled(rect, 0.0, bg.gamma_multiply(t));
    }
    if hovered {
        let knob = egui::Rect::from_x_y_ranges(
            if dir > 0.0 {
                (edge - 14.0)..=edge
            } else {
                edge..=(edge + 14.0)
            },
            band.y_range(),
        );
        painter.rect_filled(knob, CORNER, ui.visuals().widgets.hovered.weak_bg_fill);
    }
    // 山形（›）。字形ではなく線で描く（フォントに依らない）。
    let cx = edge - dir * 7.0;
    let cy = band.center().y;
    let (w, h) = (3.5, 6.0);
    painter.line(
        vec![
            egui::pos2(cx - dir * w, cy - h),
            egui::pos2(cx + dir * w, cy),
            egui::pos2(cx - dir * w, cy + h),
        ],
        egui::Stroke::new(2.0, ink),
    );
}

/// ツールチップの 2 行。1 行目「TRIM — 説明」、2 行目「打つ: TR / TRIM」。
///
/// 打てる名前は短い順（エイリアス → 正式名）。括弧を重ねた 1 行
/// （`TRIM（TR）切り取り（…）`）は、どこまでが名前か分かりにくかった（PR #32 の操作レビュー）。
#[must_use]
pub fn tooltip_lines(spec: &CommandSpec) -> (String, String) {
    let mut typed: Vec<&str> = spec.aliases.to_vec();
    typed.sort_by_key(|a| a.len());
    typed.push(spec.name);
    (
        format!("{} — {}", spec.name, spec.summary),
        format!("打つ: {}", typed.join(" / ")),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{CommandKind, COMMANDS};

    /// ツールの `name()` がコマンド表の正式名と一致すること。
    /// 一致しないと、実行中のボタンの強調（`Session::active_command` との比較）が外れる。
    #[test]
    fn tool_names_match_the_command_table() {
        for spec in COMMANDS {
            if let CommandKind::Tool(make) = spec.kind {
                assert_eq!(make().name(), spec.name);
            }
        }
    }

    #[test]
    fn tooltip_shows_name_summary_and_what_to_type() {
        let lines = |name| tooltip_lines(tools::lookup(name).unwrap());
        assert_eq!(
            lines("TRIM"),
            (
                "TRIM — 線分を切り取る（他の全図形が境界）".to_owned(),
                "打つ: TR / TRIM".to_owned()
            )
        );
        assert_eq!(
            lines("RECTANGLE").1,
            "打つ: REC / RECTANG / RECTANGLE",
            "短い順"
        );
        assert_eq!(lines("REDO").1, "打つ: REDO", "エイリアスが無くても同じ形");
    }

    #[test]
    fn overflow_is_shown_only_on_the_sides_that_can_scroll() {
        assert_eq!(
            overflow_sides(0.0, 500.0, 800.0),
            (false, false),
            "収まっている"
        );
        assert_eq!(
            overflow_sides(0.0, 900.0, 800.0),
            (false, true),
            "右へ送れる"
        );
        assert_eq!(overflow_sides(50.0, 900.0, 800.0), (true, true), "途中");
        assert_eq!(
            overflow_sides(100.0, 900.0, 800.0),
            (true, false),
            "右端まで送った"
        );
        assert_eq!(
            overflow_sides(99.8, 900.0, 800.0),
            (true, false),
            "丸めの誤差は端とみなす"
        );
    }
}
