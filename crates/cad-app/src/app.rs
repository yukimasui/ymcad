//! アプリケーション本体。

use std::time::{Duration, Instant};

use cad_core::geom::{Aabb, Point2};
use cad_core::Document;

use crate::cmdline::Submission;
use crate::component_panel::{ComponentPanel, PanelRequest};
use crate::file_ops::{self, FileOps, FileOutcome};
use crate::input::{self, ViewAction};
use crate::layer_panel::LayerPanel;
use crate::render;
use crate::resolved::ResolvedInstances;
use crate::selection::WindowMode;
use crate::session::{Session, UiAction};
use crate::snap::SnapState;
use crate::viewport::Viewport;

/// ZOOM ALL で使う既定の図面範囲（A3 横 420 × 297 mm）。
///
/// AutoCAD の ZOOM ALL は図面限界と図形範囲の広い方に合わせる。
/// 図面限界は本来 DXF の `$LIMMIN` / `$LIMMAX` に対応するドキュメントの属性なので、
/// Phase 6 で `Document` へ移して DXF と入出力する。それまでは定数で代用する。
fn default_drawing_limits() -> Aabb {
    Aabb::new(Point2::ORIGIN, Point2::new(420.0, 297.0))
}

/// ZOOM 時に取る余白の割合。
const FIT_MARGIN: f64 = 0.05;
/// クリック選択の拾い半径 [px]。画面上で一定になるようモデル空間へ換算して使う。
const PICK_RADIUS_PX: f32 = 6.0;
/// この距離[px]を超えてドラッグしたら、クリックではなく矩形選択とみなす。
const DRAG_THRESHOLD_PX: f32 = 4.0;

/// 直近の描画時間を保持して平均と最大を出す。
///
/// 「10,000 要素で 60fps」という性能目標に対する実測値を画面に出すために使う。
/// egui は必要なときだけ再描画するので、フレーム間隔ではなく
/// **こちらが描画に費やした時間**を測る。60fps の予算は 16.6ms。
#[derive(Debug)]
struct DrawTimer {
    samples: [Duration; Self::WINDOW],
    next: usize,
    filled: usize,
}

impl DrawTimer {
    const WINDOW: usize = 60;

    fn new() -> Self {
        Self {
            samples: [Duration::ZERO; Self::WINDOW],
            next: 0,
            filled: 0,
        }
    }

    fn push(&mut self, d: Duration) {
        self.samples[self.next] = d;
        self.next = (self.next + 1) % Self::WINDOW;
        self.filled = (self.filled + 1).min(Self::WINDOW);
    }

    /// (平均, 最大) をミリ秒で返す。
    fn stats_ms(&self) -> (f64, f64) {
        if self.filled == 0 {
            return (0.0, 0.0);
        }
        let used = &self.samples[..self.filled];
        let sum: Duration = used.iter().sum();
        let max = used.iter().max().copied().unwrap_or_default();
        let avg = sum.as_secs_f64() * 1000.0 / f64::from(u32::try_from(self.filled).unwrap_or(1));
        (avg, max.as_secs_f64() * 1000.0)
    }
}

/// 矩形選択のドラッグ中の状態。
#[derive(Clone, Copy, Debug)]
struct RectDrag {
    /// ドラッグ開始位置（スクリーン座標）。
    from: egui::Pos2,
    /// 開始時に Shift が押されていたか（選択解除モード）。
    shift: bool,
}

/// ymcad のアプリケーション状態。
pub struct CadApp {
    /// 図面。変更は必ず `Document::apply` / `undo` / `redo` 経由で行う。
    doc: Document,
    /// モデル空間とスクリーン空間の対応。
    viewport: Viewport,
    /// コマンドライン・ツール・選択。
    session: Session,
    /// オブジェクトスナップ。
    snap: SnapState,
    /// コンポーネントインスタンスの展開結果。
    ///
    /// 派生データなので `Document` ではなくここに持ち、
    /// `Document::revision()` をキーに再構築する（ADR-0011）。
    resolved: ResolvedInstances,
    /// レイヤパネル。
    layer_panel: LayerPanel,
    /// コンポーネントのパネル。
    component_panel: ComponentPanel,
    /// ファイル操作と未保存確認。
    files: FileOps,
    /// 終了してよいと判断した状態。
    quitting: bool,
    /// このフレームで吸着したスナップ候補。
    snapped: Option<cad_core::snap::SnapCandidate>,
    /// 矩形選択のドラッグ中の状態。
    rect_drag: Option<RectDrag>,
    /// 直近フレームのカーソル位置（モデル座標）。
    cursor_model: Option<Point2>,
    /// 読み込めた日本語フォントの情報。読み込めなかった場合は `None`。
    font_status: Option<String>,
    /// 描画時間の実測。
    draw_timer: DrawTimer,
    /// 起動直後に一度だけ図面範囲へフィットさせるためのフラグ。
    initialized: bool,
}

impl CadApp {
    /// 初期状態のアプリを作る。
    #[must_use]
    pub fn new(font_status: Option<String>) -> Self {
        Self {
            doc: Document::new(),
            viewport: Viewport::default(),
            session: Session::new(),
            snap: SnapState::new(),
            resolved: ResolvedInstances::new(),
            layer_panel: LayerPanel::new(),
            component_panel: ComponentPanel::new(),
            files: FileOps::new(),
            quitting: false,
            snapped: None,
            rect_drag: None,
            cursor_model: None,
            font_status,
            draw_timer: DrawTimer::new(),
            initialized: false,
        }
    }

    /// ZOOM EXTENTS の対象範囲。図面が空なら既定の図面範囲を使う。
    fn extents(&self) -> Aabb {
        let b = self.doc.bbox();
        if b.is_empty() {
            default_drawing_limits()
        } else {
            b
        }
    }

    /// ZOOM ALL の対象範囲。図面限界と図形範囲の広い方。
    fn all_bounds(&self) -> Aabb {
        default_drawing_limits().union(self.doc.bbox())
    }

    fn apply_view_action(&mut self, action: ViewAction) {
        match action {
            ViewAction::Pan(delta) => self.viewport.pan_px(delta),
            ViewAction::ZoomAt { anchor, factor } => self.viewport.zoom_about(anchor, factor),
            ViewAction::ZoomExtents => {
                let b = self.extents();
                self.viewport.zoom_to_fit(b, FIT_MARGIN);
            }
            ViewAction::ZoomAll => {
                let b = self.all_bounds();
                self.viewport.zoom_to_fit(b, FIT_MARGIN);
            }
        }
    }

    // ---- UI ---------------------------------------------------------------

    fn status_bar(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            // 座標は小数点以下 4 桁で表示する。
            match self.cursor_model {
                Some(p) => ui.monospace(format!("X {:>14.4}   Y {:>14.4}", p.x, p.y)),
                None => ui.monospace(format!("X {:>14}   Y {:>14}", "-", "-")),
            };
            ui.separator();
            ui.monospace(format!("倍率 {:.6}", self.viewport.scale()));
            ui.separator();
            ui.monospace(format!("要素 {}", self.doc.entities().len()));
            ui.separator();
            let layer_name = self
                .doc
                .layers()
                .get(self.doc.layers().current())
                .map_or("?", |l| l.name.as_str());
            ui.monospace(format!("画層 {layer_name}"));
            ui.separator();
            ui.monospace(format!("選択 {}", self.session.selection.len()));
            ui.separator();
            if self.snap.is_enabled() {
                // 吸着中はその種別を出す。マーカーの形と合わせて確認できるように。
                let label = self.snap.held().map_or_else(
                    || "OSNAP".to_owned(),
                    |c| format!("OSNAP:{}", c.kind.label()),
                );
                ui.colored_label(
                    egui::Color32::from_rgb(0xc6, 0xff, 0x00),
                    egui::RichText::new(label).monospace(),
                );
            } else {
                ui.weak(egui::RichText::new("osnap").monospace());
            }
            ui.separator();
            if self.session.has_active_tool() {
                ui.colored_label(
                    egui::Color32::from_rgb(0xff, 0xc1, 0x07),
                    egui::RichText::new("コマンド実行中").monospace(),
                );
                ui.separator();
            }

            // 60fps の予算は 16.6ms。実測がそれを大きく下回っていることを見せる。
            let (avg, max) = self.draw_timer.stats_ms();
            ui.monospace(format!("描画 平均{avg:.2}ms 最大{max:.2}ms"));

            if self.font_status.is_none() {
                ui.separator();
                ui.colored_label(
                    egui::Color32::from_rgb(0xff, 0x70, 0x43),
                    "日本語フォント未検出",
                );
            }
        });
    }

    fn command_area(&mut self, ui: &mut egui::Ui) {
        let prompt = self.session.prompt();
        // ツール実行中と選択待ち中は候補を出さない。座標やオプションを打つ段階なので、
        // コマンド名の候補が出ると邪魔になる。
        let allow_suggestions = !self.session.has_active_tool();
        let submission = self.session.cmdline.show(ui, &prompt, allow_suggestions);
        if submission != Submission::None {
            self.session.handle_submission(submission, &mut self.doc);
            for action in self.session.take_view_actions() {
                self.apply_view_action(action);
            }
            for action in self.session.take_ui_actions() {
                match action {
                    UiAction::ToggleLayerPanel => self.layer_panel.toggle(),
                    UiAction::ToggleComponentPanel => self.component_panel.toggle(),
                    UiAction::File(a) => {
                        let outcome = self.files.request(a, &mut self.doc);
                        self.report_file_outcome(outcome);
                    }
                }
            }
        }
    }

    fn canvas(&mut self, ui: &mut egui::Ui) {
        let (response, painter) =
            ui.allocate_painter(ui.available_size(), egui::Sense::click_and_drag());
        self.viewport.set_rect(response.rect);

        // 初回だけ図面範囲へ合わせる。以降はユーザーの操作に任せる。
        if !self.initialized && response.rect.width() > 0.0 {
            let b = self.all_bounds();
            self.viewport.zoom_to_fit(b, FIT_MARGIN);
            self.initialized = true;
        }

        for action in input::collect_view_actions(&response, ui, &self.viewport) {
            self.apply_view_action(action);
        }

        // F3 で OSNAP を切り替える。コマンドラインより先に取る必要はないが、
        // TextEdit は F3 を消費しないのでここで拾って問題ない。
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F3)) {
            self.snap.toggle();
            let state = if self.snap.is_enabled() { "ON" } else { "OFF" };
            self.session
                .cmdline
                .info(format!("オブジェクトスナップ: {state}"));
        }

        let raw_cursor = response
            .hover_pos()
            .map(|p| self.viewport.screen_to_model(p));

        // スナップは点の入力を待っているときだけ効かせる。
        // 選択操作中にマーカーが出ると邪魔になるため。
        self.snapped = match (raw_cursor, self.session.wants_point()) {
            (Some(c), true) => {
                self.snap
                    .update_px(&self.doc, c, &self.viewport, self.session.last_point())
            }
            _ => {
                self.snap.release();
                None
            }
        };

        // 吸着していればそれを実際のカーソル位置として扱う。
        self.cursor_model = self.snapped.map(|s| s.point).or(raw_cursor);

        let active_drag = self.handle_pointer(&response, ui);

        // ---- 描画 ----
        let started = Instant::now();

        painter.rect_filled(response.rect, 0.0, ui.visuals().extreme_bg_color);
        render::draw_grid(&painter, &self.viewport, ui.visuals());
        render::draw_origin_marker(&painter, &self.viewport);
        render::draw_entities(
            &painter,
            &self.doc,
            &self.viewport,
            &self.session.selection,
            &mut self.resolved,
        );

        let preview = self.session.preview(self.cursor_model, &self.doc);
        render::draw_preview(&painter, &self.viewport, self.doc.definitions(), &preview);

        if let Some(candidate) = &self.snapped {
            render::draw_snap_marker(&painter, &self.viewport, candidate, true);
        }

        if let Some((rect, mode)) = active_drag {
            render::draw_selection_rect(&painter, rect, mode);
        }

        self.draw_timer.push(started.elapsed());

        if response.dragged_by(egui::PointerButton::Middle) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        } else if response.contains_pointer() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }
    }

    /// マウス操作を処理し、描画すべき選択矩形があれば返す。
    ///
    /// # クリック判定に `clicked_by()` だけを使わない理由
    ///
    /// egui が押下〜解放を「クリック」と認めるのは、**押した位置から 6.0px 以内**かつ
    /// **押していた時間が 0.8 秒未満**のときだけ
    /// （`egui::InputOptions` の `max_click_dist` / `max_click_duration`）。
    /// どちらかを外れると `Response::clicked_by()` は `false` になり、
    /// egui はその操作をドラッグとして扱う。
    ///
    /// CAD の作図では、狙いを定めてゆっくり押す・押しながら微妙に手が動く、が日常的に起きる。
    /// `clicked_by()` だけで点を拾っていると、そうした操作が**黙って捨てられる**。
    ///
    /// そこで **「キャンバス上で主ボタンが離された」ことを合図にする**。
    /// `drag_stopped_by()` は距離超過でも時間超過でも発火するので、両方の原因を一度に塞げる。
    ///
    /// なお egui 側の閾値（`ctx.options_mut`）を緩める案は採らない。
    /// ダブルクリック判定にも影響し、時間の条件は別途上げる必要があるため。
    ///
    /// この判定は `egui::Response` の状態に依存するため単体テストで再現できない。
    /// 変更したら手動で確認すること。
    fn handle_pointer(
        &mut self,
        response: &egui::Response,
        ui: &egui::Ui,
    ) -> Option<(egui::Rect, WindowMode)> {
        let shift = ui.input(|i| i.modifiers.shift);
        let pick_tolerance = self.viewport.px_to_model_len(PICK_RADIUS_PX);

        // 主ボタンが離されたか。クリックと判定されなかった解放もここで拾う。
        let released = response.clicked_by(egui::PointerButton::Primary)
            || response.drag_stopped_by(egui::PointerButton::Primary);

        // 座標は「離した位置」を使う。スナップはその時点のカーソル位置から
        // 計算されているので、離した位置なら**マーカーの出ている点と実際に入る点が一致する**。
        // 押した位置を使うと、スナップ表示と入力結果がずれる。
        let released_pos = response.interact_pointer_pos();

        // ---- 点の入力待ち中 ----
        //
        // この状態では矩形選択に入らないので、離されたら常に点として拾ってよい。
        // 距離の閾値は設けない（手が動いていても拾えるようにするのが目的）。
        if self.session.wants_point() {
            if released {
                if let Some(pos) = released_pos {
                    self.place_point(pos, shift, pick_tolerance);
                }
            }
            return None;
        }

        // ---- 左ドラッグによる矩形選択 ----
        if response.drag_started_by(egui::PointerButton::Primary) {
            if let Some(from) = response.interact_pointer_pos() {
                self.rect_drag = Some(RectDrag { from, shift });
            }
        }

        if let Some(drag) = self.rect_drag {
            let current = released_pos
                .or_else(|| response.hover_pos())
                .unwrap_or(drag.from);
            let mode = WindowMode::from_drag(f64::from(drag.from.x), f64::from(current.x));
            let rect = egui::Rect::from_two_pos(drag.from, current);

            if response.drag_stopped_by(egui::PointerButton::Primary) {
                self.rect_drag = None;
                if rect.width() > DRAG_THRESHOLD_PX || rect.height() > DRAG_THRESHOLD_PX {
                    let model_rect = Aabb::new(
                        self.viewport.screen_to_model(rect.min),
                        self.viewport.screen_to_model(rect.max),
                    );
                    self.session
                        .handle_rect_select(model_rect, mode, drag.shift, &mut self.doc);
                } else {
                    // 動きが小さいならクリック扱い。ここで自分で処理する。
                    // 後段の clicked_by() に任せると、egui がドラッグと判定していた場合に
                    // 取りこぼす（DRAG_THRESHOLD_PX 4.0 は egui の 6.0 より小さいので、
                    // この閾値では egui の判定を肩代わりできない）。
                    self.place_point(current, drag.shift, pick_tolerance);
                }
                return None;
            }
            return Some((rect, mode));
        }

        // ---- ドラッグを伴わない素のクリック ----
        if released {
            if let Some(pos) = released_pos {
                self.place_point(pos, shift, pick_tolerance);
            }
        }

        None
    }

    /// スクリーン座標を入力点として `Session` へ渡す。
    fn place_point(&mut self, pos: egui::Pos2, shift: bool, pick_tolerance: f64) {
        // 吸着していればその点を使う。クリック位置そのままではなく
        // スナップ点が入力されるのが OSNAP の要点。
        let model = self
            .snapped
            .map_or_else(|| self.viewport.screen_to_model(pos), |s| s.point);
        self.session
            .handle_click(model, shift, pick_tolerance, &mut self.doc);
        self.snap.release();
    }
}

impl CadApp {
    /// ファイル操作の結果をコマンドラインへ出す。
    fn report_file_outcome(&mut self, outcome: FileOutcome) {
        match outcome {
            FileOutcome::Nothing => {}
            FileOutcome::Ok(msg) => {
                self.session.cmdline.info(msg);
                // 図面が入れ替わったので、選択とスナップの状態を捨てる。
                self.session.selection.clear();
                self.snap.release();
            }
            FileOutcome::Failed(msg) => self.session.cmdline.error(msg),
            FileOutcome::Quit => self.quitting = true,
        }
    }

    /// ショートカットとウィンドウの終了要求を処理する。
    fn handle_file_input(&mut self, ctx: &egui::Context) {
        // 確認ダイアログ表示中はショートカットを受け付けない。
        if !self.files.is_confirming() {
            if let Some(action) = file_ops::shortcut(ctx) {
                let outcome = self.files.request(action, &mut self.doc);
                self.report_file_outcome(outcome);
            }
        }

        // ウィンドウの ✕ ボタン。未保存なら一旦止めて確認する。
        if ctx.input(|i| i.viewport().close_requested()) && !self.quitting {
            if self.doc.is_dirty() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                let outcome = self
                    .files
                    .request(file_ops::FileAction::Quit, &mut self.doc);
                self.report_file_outcome(outcome);
            } else {
                self.quitting = true;
            }
        }

        let outcome = self.files.show_confirm(ctx, &mut self.doc);
        self.report_file_outcome(outcome);

        if self.quitting {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    /// ウィンドウタイトル。ファイル名と未保存マークを出す。
    fn window_title(&self) -> String {
        let name = self.doc.path().and_then(|p| p.file_name()).map_or_else(
            || "名称未設定".to_owned(),
            |n| n.to_string_lossy().into_owned(),
        );
        let dirty = if self.doc.is_dirty() { "*" } else { "" };
        format!("{dirty}{name} — ymcad")
    }

    /// レイヤパネルを描画し、返ってきたコマンドを適用する。
    fn layer_area(&mut self, ui: &mut egui::Ui) {
        if !self.layer_panel.is_open() {
            return;
        }
        egui::Panel::right("layers")
            .default_size(460.0)
            .show(ui, |ui| {
                let commands = self
                    .layer_panel
                    .show(ui, &self.doc, &self.session.selection);
                for cmd in commands {
                    self.session.apply_external(cmd, &mut self.doc);
                }
            });
    }
}

impl CadApp {
    /// コンポーネントパネルを描画し、返ってきたコマンドと依頼を処理する。
    fn component_area(&mut self, ui: &mut egui::Ui) {
        if !self.component_panel.is_open() {
            return;
        }
        egui::Panel::right("components")
            .default_size(460.0)
            .show(ui, |ui| {
                let (commands, request) =
                    self.component_panel
                        .show(ui, &self.doc, &self.session.selection);
                for cmd in commands {
                    self.session.apply_external(cmd, &mut self.doc);
                }
                if let Some(PanelRequest::Insert(def)) = request {
                    // 名前を打たせずに INSERT を始める。
                    self.session.start_tool_directly(
                        Box::new(crate::tools::component::InsertTool::for_definition(def)),
                        &mut self.doc,
                    );
                }
            });
    }
}

impl eframe::App for CadApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_file_input(&ctx);
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(self.window_title()));

        egui::Panel::bottom("cmdline").show(ui, |ui| self.command_area(ui));
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        self.layer_area(ui);
        self.component_area(ui);
        egui::CentralPanel::no_frame().show(ui, |ui| self.canvas(ui));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draw_timer_reports_zero_when_empty() {
        let t = DrawTimer::new();
        assert_eq!(t.stats_ms(), (0.0, 0.0));
    }

    #[test]
    fn draw_timer_averages_and_takes_max() {
        let mut t = DrawTimer::new();
        t.push(Duration::from_micros(1000)); // 1.0ms
        t.push(Duration::from_micros(3000)); // 3.0ms
        let (avg, max) = t.stats_ms();
        assert!((avg - 2.0).abs() < 1e-9, "平均は 2.0ms のはず: {avg}");
        assert!((max - 3.0).abs() < 1e-9, "最大は 3.0ms のはず: {max}");
    }

    /// 窓を越えても古いサンプルで壊れないこと。
    #[test]
    fn draw_timer_wraps_around() {
        let mut t = DrawTimer::new();
        for _ in 0..(DrawTimer::WINDOW * 3) {
            t.push(Duration::from_micros(500));
        }
        let (avg, max) = t.stats_ms();
        assert!((avg - 0.5).abs() < 1e-9);
        assert!((max - 0.5).abs() < 1e-9);
    }

    /// 既定の図面範囲は空でないこと（ZOOM ALL が無反応にならない）。
    #[test]
    fn default_limits_are_not_empty() {
        let b = default_drawing_limits();
        assert!(!b.is_empty());
        assert!(b.width() > 0.0 && b.height() > 0.0);
    }
}
