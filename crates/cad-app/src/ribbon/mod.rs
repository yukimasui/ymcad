//! リボン（画面上端のタブつきアイコンバー）。Issue #26、ADR-0037。
//!
//! - 1 段目にタブ、2 段目にグループ（枠とグループ名）ごとのアイコンボタン
//! - **ボタンを押すことはコマンド名を打つのと同じ扱い。** このモジュールは押された
//!   コマンド名を返すだけで、始めるのは呼び出し側（`Session::start_command_from_ui`）
//! - 幅が足りないときは横スクロール（マウスホイールの縦回転でも横に動く）
//! - 実行中のコマンドのボタンは選択色で強調する
//!
//! ボタンはキーボードのフォーカスを取らない（`Sense::click` だけ）。押した後も
//! キー入力はそのままコマンドラインへ流れる（ADR-0035 の `owns_keys` を壊さない）。
//!
//! 選択中のタブなどの状態はここ（`cad-app` 側）に持ち、`Document` には入れない。

pub mod icons;
pub mod layout;

use crate::tools::{self, CommandSpec};
use layout::{GroupSpec, TABS};

/// アイコンの一辺 [px]。
const ICON_PX: f32 = 22.0;
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

/// スクロール領域の ID。
const SCROLL_ID: &str = "ribbon_scroll";

/// リボンの UI 状態。
#[derive(Debug, Default)]
pub struct Ribbon {
    /// 開いているタブ（`layout::TABS` の添字）。起動時はホーム（0）。
    tab: usize,
    /// 直前に描いた結果（テスト用）。
    #[cfg(test)]
    probe: Probe,
}

/// 直前のフレームに描いたもの（テストで見た目の代わりに状態を検査するため）。
#[cfg(test)]
#[derive(Clone, Debug, Default)]
pub struct Probe {
    /// 描いたタブ（見出し・矩形）。
    pub tabs: Vec<(&'static str, egui::Rect)>,
    /// 描いたボタン（コマンド名・矩形・強調したか）。
    pub buttons: Vec<DrawnButton>,
    /// ボタンの並びが見えている範囲（スクロール領域の表示範囲）。
    pub viewport: Option<egui::Rect>,
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
        self.show_tabs(ui);

        let mut pressed = None;
        let groups = TABS[self.tab].groups;
        // 横スクロールしかない領域なので、普通のホイール（縦回転）でも横に動かす。
        // egui の既定では Shift を押しながらでないと横に動かず、横ホイールの無いマウスで
        // 右端のボタンに届かない。このパネルの `Ui` だけに効く。
        ui.style_mut().always_scroll_the_only_direction = true;
        #[cfg_attr(not(test), allow(unused_variables))]
        let output = egui::ScrollArea::horizontal()
            .id_salt(SCROLL_ID)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    for group in groups {
                        if let Some(name) = self.show_group(ui, group, active) {
                            pressed = Some(name);
                        }
                    }
                });
            });
        #[cfg(test)]
        {
            self.probe.viewport = Some(output.inner_rect);
            self.probe.rect = Some(egui::Rect::from_min_max(top, ui.min_rect().max));
        }
        pressed
    }

    /// 1 段目のタブ。
    fn show_tabs(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            for (i, tab) in TABS.iter().enumerate() {
                let response = ui.selectable_label(self.tab == i, tab.title);
                #[cfg(test)]
                self.probe.tabs.push((tab.title, response.rect));
                if response.clicked() {
                    self.tab = i;
                }
            }
        });
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

    /// ボタン 1 つ。押されたら `true`。
    fn show_button(&mut self, ui: &mut egui::Ui, name: &'static str, highlighted: bool) -> bool {
        let label_font = egui::FontId::proportional(LABEL_SIZE);
        let galley =
            ui.painter()
                .layout_no_wrap(name.to_owned(), label_font, egui::Color32::PLACEHOLDER);
        let width = (galley.size().x + BUTTON_PAD * 2.0).max(BUTTON_MIN_WIDTH);
        let height = BUTTON_PAD + ICON_PX + galley.size().y + BUTTON_PAD;
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), name)
        });

        if ui.is_rect_visible(rect) {
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
            // アイコンと名前は同じ色。白一色の SVG をこの色で着色する（ADR-0037）。
            let color = if highlighted {
                visuals.selection.stroke.color
            } else {
                visuals.text_color()
            };
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
        });

        let response = match tools::lookup(name) {
            Some(spec) => response.on_hover_text(tooltip_text(spec)),
            None => response,
        };
        response.clicked()
    }
}

/// グループ名をボタンの並びの下に中央寄せで描く。幅は並びと文字の広いほう。
fn group_title(ui: &mut egui::Ui, title: &str, row_width: f32) {
    let galley = ui.painter().layout_no_wrap(
        title.to_owned(),
        egui::FontId::proportional(GROUP_TITLE_SIZE),
        ui.visuals().weak_text_color(),
    );
    let size = egui::vec2(row_width.max(galley.size().x), galley.size().y);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let pos = egui::pos2(rect.center().x - galley.size().x / 2.0, rect.top());
    ui.painter().galley(pos, galley, egui::Color32::PLACEHOLDER);
}

/// ツールチップの文。`LINE（L）連続線分`。エイリアスが無ければ `REDO　取り消した…`。
#[must_use]
pub fn tooltip_text(spec: &CommandSpec) -> String {
    if spec.aliases.is_empty() {
        format!("{}　{}", spec.name, spec.summary)
    } else {
        format!("{}（{}）{}", spec.name, spec.alias_text(), spec.summary)
    }
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
    fn tooltip_shows_name_alias_and_summary() {
        let line = tools::lookup("LINE").unwrap();
        assert_eq!(tooltip_text(line), "LINE（L）連続線分");
        let rect = tools::lookup("RECTANGLE").unwrap();
        assert_eq!(
            tooltip_text(rect),
            "RECTANGLE（RECTANG, REC）矩形（対角2点）"
        );
        let redo = tools::lookup("REDO").unwrap();
        assert_eq!(tooltip_text(redo), "REDO　取り消した操作をやり直す");
    }
}
