//! アプリケーション本体。

use std::time::{Duration, Instant};

use cad_core::geom::{Aabb, Point2};
use cad_core::Document;

use crate::cmdline::Submission;
use crate::component_panel::{ComponentPanel, PanelRequest};
use crate::drafting::{self, Drafting};
use crate::file_ops::{self, FileOps, FileOutcome};
use crate::hover::Hover;
use crate::input::{self, ViewAction};
use crate::layer_panel::{LayerPanel, PanelNotice};
use crate::properties_panel::PropertiesPanel;
use crate::render;
use crate::resolved::ResolvedInstances;
use crate::ribbon::Ribbon;
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
/// プロパティパネルの既定の幅 [px]。レイヤ・コンポーネントより狭い（値が主で、長い表は無い）。
const PROPERTIES_PANEL_WIDTH: f32 = 300.0;
/// レイヤ・コンポーネントパネルの既定の幅 [px]。レイヤの 1 行（約 310px）が収まる幅。
const SIDE_PANEL_WIDTH: f32 = 370.0;
/// 右側のパネルを何枚開いても作図領域に残す幅の最小 [px]。
///
/// 外側のパネルから順に、残りの幅からこれを引いた分までしか広がれない。ユーザーがドラッグで
/// 広げた後や狭い画面でも毎フレーム効く（保存された幅も範囲に収められる）。
/// ただし画面が狭く、開いているパネルの最小幅の合計を引くとこれを下回るときは、パネルの
/// 最小幅を優先して作図領域を狭める（[`CadApp::canvas_min_width`]。パネルが幅 0 で見えなく
/// なったり、隣のパネルを覆ったりしない）。
const MIN_CANVAS_WIDTH: f32 = 400.0;
/// 各パネルの最小幅 [px]。中身（レイヤの 1 行、プロパティの表）が読める幅。
///
/// 内側のパネルの分を外側のパネルが空けておく（[`right_panel`]）。中身がこれより広くても、
/// 各パネルは自分の幅の中で横スクロールするので、隣のパネルへはみ出さない（[`own_width`]）。
const LAYER_PANEL_MIN_WIDTH: f32 = 330.0;
const COMPONENT_PANEL_MIN_WIDTH: f32 = 240.0;
const PROPERTIES_PANEL_MIN_WIDTH: f32 = 200.0;
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
    /// 直交モード（F8）と極トラッキング（F10）。UI の状態なので保存しない（ADR-0038）。
    drafting: Drafting,
    /// コンポーネントインスタンスの展開結果。
    ///
    /// 派生データなので `Document` ではなくここに持ち、
    /// `Document::revision()` をキーに再構築する（ADR-0011）。
    resolved: ResolvedInstances,
    /// レイヤパネル。
    layer_panel: LayerPanel,
    /// コンポーネントのパネル。
    component_panel: ComponentPanel,
    /// プロパティパネル（選んだ図形の値とレイヤ）。
    properties_panel: PropertiesPanel,
    /// リボン（画面上端のタブつきアイコンバー）。
    ribbon: Ribbon,
    /// ファイル操作と未保存確認。
    files: FileOps,
    /// 終了してよいと判断した状態。
    quitting: bool,
    /// このフレームで吸着したスナップ候補。
    snapped: Option<cad_core::snap::SnapCandidate>,
    /// ホバーで強調する選択プレビュー（Issue #34、ADR-0042）。クリックもここの索引で拾う。
    hover: Hover,
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
    /// ステータスバーの座標の欄の文字数。一度広がったら縮めない（[`coord_width_for`]）。
    coord_width: usize,
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
            drafting: Drafting::new(),
            resolved: ResolvedInstances::new(),
            layer_panel: LayerPanel::new(),
            component_panel: ComponentPanel::new(),
            properties_panel: PropertiesPanel::new(),
            ribbon: Ribbon::new(),
            files: FileOps::new(),
            quitting: false,
            snapped: None,
            hover: Hover::new(),
            rect_drag: None,
            cursor_model: None,
            font_status,
            draw_timer: DrawTimer::new(),
            initialized: false,
            coord_width: COORD_MIN_WIDTH,
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

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            // 座標は小数点以下 4 桁で表示する。欄の幅は 12 文字（±999999.9999）から始め、
            // それより大きい座標が出たら広げて、そのあとは縮めない（PR #39）。
            self.coord_width = coord_width_for(self.coord_width, self.cursor_model);
            let w = self.coord_width;
            let coords = match self.cursor_model {
                Some(p) => format!("X {:>w$.4}  Y {:>w$.4}", p.x, p.y),
                None => format!("X {:>w$}  Y {:>w$}", "-", "-"),
            };
            // 情報表示は文字を選ばせない（乗せても I ビームにしない。ほかの部品とそろえる）。
            ui.add(egui::Label::new(egui::RichText::new(coords).monospace()).selectable(false));
            ui.separator();
            // 切り替え部品（OSNAP / ORTHO / POLAR / DYN）は座標の直後に置く（Issue #38）。
            let separator_width = self.status_toggles(ui);

            // 情報表示。幅が足りなければ右から順に、区切り線ごと省く（文字の途中で切らない）。
            // 作図中に見るレイヤと選択を先にして、最後まで残す（Issue #38 / PR #39）。
            let layer_name = self
                .doc
                .layers()
                .get(self.doc.layers().current())
                .map_or("?", |l| l.name.as_str());
            // 長いレイヤ名は省略して後ろの項目を押し出さない。全文はツールチップで見せる。
            let short_name = ellipsize(layer_name, LAYER_NAME_MAX_CHARS);
            let layer_tooltip = (short_name != layer_name).then(|| format!("レイヤ {layer_name}"));
            let mut items = vec![
                InfoItem {
                    tooltip: layer_tooltip,
                    ..InfoItem::new(format!("レイヤ {short_name}"))
                },
                InfoItem::new(format!("選択 {}", self.session.selection.len())),
                InfoItem::new(format!("要素 {}", self.doc.entities().len())),
                InfoItem::new(format!("倍率 {:.6}", self.viewport.scale())),
            ];
            if self.font_status.is_none() {
                // フォントが無いとこの文言自体が □ になるので、英語も併記する。
                items.push(InfoItem {
                    color: Some(egui::Color32::from_rgb(0xff, 0x70, 0x43)),
                    ..InfoItem::new("日本語フォント未検出 (Japanese font not found)".to_owned())
                });
            }
            // 60fps の予算は 16.6ms。実測がそれを大きく下回っていることを見せる。
            // 開発者向けの情報なので最後に置く。数値は固定幅にして、境目の幅で
            // フレームごとに出たり消えたりしないようにする。
            let (avg, max) = self.draw_timer.stats_ms();
            items.push(InfoItem::new(format!(
                "描画 平均{avg:>6.2}ms 最大{max:>6.2}ms"
            )));
            show_while_fits(ui, items, separator_width);
        });
    }

    fn command_area(&mut self, ui: &mut egui::Ui) {
        let prompt = self.session.prompt();
        self.session.cmdline.show_bottom(ui, &prompt);
        // 動的入力がオンなら入力欄はカーソル横にあり、まだ描いていない。
        // 確定はそちらを描いたあと（`dynamic_input_area`）で行う。
        if !self.session.cmdline.frame_is_dynamic() {
            let submission = self.session.cmdline.finish_frame();
            self.apply_submission(submission);
        }
    }

    /// カーソル横の入力欄を描き、確定操作を実行する（動的入力がオンのときだけ）。
    ///
    /// キャンバスの矩形とマウス位置が要るので、キャンバスを描いたあとに呼ぶ。
    fn dynamic_input_area(&mut self, ctx: &egui::Context) {
        if !self.session.cmdline.frame_is_dynamic() {
            return;
        }
        // キャンバスでのクリックでツールが進んでいるかもしれないので、ここで取り直す。
        let prompt = self.session.prompt();
        let tool_active = self.session.has_active_tool();
        // 寸法入力の欄のライブ値。固定をかけた後のカーソル（＝ラバーバンドの先）から測る。
        let dimension = self.session.dimension_base().map(|base| {
            self.cursor_model
                .map(|c| crate::cmdline::dimension::live(base, c))
        });
        self.session.cmdline.show_floating(
            ctx,
            &prompt,
            self.viewport.rect(),
            tool_active,
            dimension,
        );
        let submission = self.session.cmdline.finish_frame();
        if submission != Submission::None {
            // キャンバスはもう描き終えているので、確定の結果（ラバーバンドや新しい図形）は
            // 次のフレームで描かれる。その次のフレームを待たせずに確実に起こす。
            ctx.request_repaint();
        }
        self.apply_submission(submission);
    }

    /// オブジェクトスナップを切り替え、履歴に残す。
    fn toggle_osnap(&mut self) {
        self.snap.toggle();
        let state = if self.snap.is_enabled() { "ON" } else { "OFF" };
        self.session
            .cmdline
            .info(format!("オブジェクトスナップ: {state}"));
    }

    /// 直交モード・極トラッキングを切り替え、履歴に残す。他方は変えない（ADR-0038）。
    fn toggle_drafting(&mut self, mode: drafting::Mode) {
        let msg = self.drafting.toggle(mode);
        self.session.cmdline.info(msg);
    }

    /// スナップ前のカーソル（モデル座標）から、ラバーバンドとクリックに使う点を決める。
    /// スナップの吸着 → 直交 → 極の順（ADR-0038）。寸法入力の固定はこの後に `Session` がかける。
    fn track(&self, raw: Point2) -> drafting::Tracked {
        drafting::track_cursor(
            self.drafting,
            &self.session,
            &self.viewport,
            self.snapped.map(|s| s.point),
            raw,
        )
    }

    /// 動的入力を切り替え、履歴に残す。
    fn toggle_dynamic_input(&mut self) {
        let state = if self.session.cmdline.toggle_dynamic() {
            "ON"
        } else {
            "OFF"
        };
        self.session.cmdline.info(format!("動的入力: {state}"));
    }

    /// コマンドラインで確定された操作を実行する。
    fn apply_submission(&mut self, submission: Submission) {
        if submission == Submission::None {
            return;
        }
        self.session.handle_submission(submission, &mut self.doc);
        self.drain_session_actions();
    }

    /// コマンドが出したビュー操作と UI 要求（パネルの開閉・ファイル操作）を処理する。
    ///
    /// コマンドラインからでもリボンからでも、始まったコマンドの後始末は同じ。
    fn drain_session_actions(&mut self) {
        for action in self.session.take_view_actions() {
            self.apply_view_action(action);
        }
        for action in self.session.take_ui_actions() {
            match action {
                UiAction::ToggleLayerPanel => self.layer_panel.toggle(),
                UiAction::ToggleComponentPanel => self.component_panel.toggle(),
                UiAction::TogglePropertiesPanel => self.properties_panel.toggle(),
                UiAction::File(a) => {
                    let outcome = self.files.request(a, &mut self.doc);
                    self.report_file_outcome(outcome);
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
        // モーダル（未保存確認）が出ている間はどちらも扱わない（Issue #24）。
        let modal = self.files.is_confirming();
        if !modal && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F3)) {
            self.toggle_osnap();
        }
        // F12 で動的入力を切り替える（AutoCAD と同じキー）。F3 と同じく TextEdit は消費しない。
        if !modal && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F12)) {
            self.toggle_dynamic_input();
        }
        // F8 で直交、F10 で極トラッキング（AutoCAD と同じキー）。F3 と同じく TextEdit は消費しない。
        // モーダルが出ている間は F3 / F12 と同じく扱わない。
        if !modal {
            for mode in drafting::take_key_toggles(ui) {
                self.toggle_drafting(mode);
            }
        }

        // カーソル横の入力欄の基準。キャンバスの外にいる間は最後の位置に留める。
        if let Some(pos) = response.hover_pos() {
            self.session.cmdline.set_cursor_anchor(pos);
        }

        let raw_cursor = response
            .hover_pos()
            .map(|p| self.viewport.screen_to_model(p));

        // スナップは点の入力を待っているときだけ効かせる。
        // 選択操作中にマーカーが出ると邪魔になるため。
        // 図形を指す段階（TRIM / EXTEND / FILLET / CHAMFER など）でも効かせない。交点へ吸い付くと
        // 指した位置がずれ、TRIM がどちら側を切るか決められなくなる（ADR-0042）。
        let snap_wanted = self.session.wants_point() && !self.session.wants_entity();
        self.snapped = match (raw_cursor, snap_wanted) {
            (Some(c), true) => {
                self.snap
                    .update_px(&self.doc, c, &self.viewport, self.session.last_point())
            }
            _ => {
                self.snap.release();
                None
            }
        };

        // 吸着していればそれを実際のカーソル位置として扱う。吸着していなければ
        // 直交・極トラッキングをかける。クリック（`place_point`）も同じ `track` を通す。
        let tracked = raw_cursor.map(|c| self.track(c));
        let cursor = tracked.map(|t| t.point);
        // 直接距離入力と寸法入力の向きはこの位置（固定をかける前）から決める。
        self.session.set_cursor(cursor);
        // 寸法入力で固定した値（錠前）をかける。ラバーバンドはこの位置で描き、
        // クリックも同じ計算（`Session::constrain`）を通す（`place_point`）。
        self.cursor_model = cursor.map(|c| self.session.rubber_band(c));

        let active_drag = self.handle_pointer(&response, ui);

        // ---- 描画 ----
        // ホバーの強調と結果プレビューの計算も描画時間に入れる（Issue #34。1 万図形で重くならないかを見る）。
        let started = Instant::now();

        // クリックと同じ位置（`cursor_model`）・同じ拾い半径で、クリックしたら拾われるものを決める。
        // 矩形選択のドラッグ中は強調しない（離したら矩形で選ばれるので、乗せた図形とは関係ない）。
        let hover_at = if active_drag.is_none() && self.rect_drag.is_none() {
            self.cursor_model
        } else {
            None
        };
        let pick_tolerance = self.viewport.px_to_model_len(PICK_RADIUS_PX);
        self.hover
            .update(&self.session, &self.doc, hover_at, pick_tolerance);

        painter.rect_filled(response.rect, 0.0, ui.visuals().extreme_bg_color);
        render::draw_grid(&painter, &self.viewport, ui.visuals());
        render::draw_origin_marker(&painter, &self.viewport);
        // ホバーの縁取りは図形の下に敷く。上に重ねると線の色（選択色・レイヤ色）が変わり、
        // 「もう選んだか」が乗せている間は見えなくなる（PR #63 の操作レビュー）。
        render::draw_hover(
            &painter,
            &self.doc,
            &self.viewport,
            self.hover.highlighted(),
            &mut self.resolved,
        );
        render::draw_entities(
            &painter,
            &self.doc,
            &self.viewport,
            &self.session.selection,
            &mut self.resolved,
            self.session.editing(),
        );

        // TRIM / EXTEND の結果プレビュー（Issue #34 段階 2）。強調している図形をクリックしたら
        // 消える部分・伸びる部分。消える部分は下の線を消してから描くので、図形の後に描く。
        render::draw_entity_preview(
            &painter,
            &self.viewport,
            self.doc.definitions(),
            self.hover.entity_preview(),
            ui.visuals().extreme_bg_color,
        );

        let preview = self.session.preview(self.cursor_model, &self.doc);
        render::draw_preview(&painter, &self.viewport, self.doc.definitions(), &preview);

        if let Some(hit) = tracked.and_then(|t| t.polar) {
            render::draw_polar_guide(&painter, &self.viewport, &hit);
        }

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
        // スナップ点が入力されるのが OSNAP の要点。吸着していなければ直交・極をかける。
        // ラバーバンド（`canvas`）と同じ `track` を通すので、見えている線の先と一致する。
        let model = self.track(self.viewport.screen_to_model(pos)).point;
        // 寸法入力で固定した値（錠前）をかける。ラバーバンド（`canvas`）と同じ計算を通すので、
        // 見えている線の先とクリックで入る点が一致する。固定値から点が決まらない位置
        // （角度だけ固定してその反対側など）では、`Enter` と同じく点を入れずにエラーにする。
        match self.session.constrain(model) {
            // ホバーの強調と同じ索引で拾う（`Session::click_target` を通る）。
            Ok(model) => self.session.handle_click(
                model,
                shift,
                pick_tolerance,
                &mut self.doc,
                self.hover.picker(),
            ),
            Err(e) => self.session.cmdline.error(e.message()),
        }
        self.snap.release();
    }
}

impl CadApp {
    /// ステータスバーの切り替え部品（OSNAP / ORTHO / POLAR / DYN）と「コマンド実行中」。
    ///
    /// 座標の直後に置く。幅が足りないとステータスバーは右端から切れるので、
    /// クリックで操作する部品を情報表示より先にする（Issue #38。PR #35 で ORTHO / POLAR が
    /// 増え、幅 800px で DYN が画面外になってクリックできなかった）。
    ///
    /// 戻り値は区切り線 1 本ぶんの幅（線と前後の間隔）。後ろの情報表示が入り切るかの判定に使う。
    fn status_toggles(&mut self, ui: &mut egui::Ui) -> f32 {
        // `OSNAP` は吸着中に `OSNAP:最近点` のように伸びる。欄の幅をいちばん長い表示で
        // 固定し、後ろの部品の押す位置が吸着のたびに動かないようにする（PR #39）。
        let osnap_width = cad_core::snap::SnapKind::all()
            .into_iter()
            .map(|k| text_width(ui, &format!("OSNAP:{}", k.label())))
            .fold(text_width(ui, "OSNAP"), f32::max);
        let osnap_text = if self.snap.is_enabled() {
            // 吸着中はその種別を出す。マーカーの形と合わせて確認できるように。
            let label = self.snap.held().map_or_else(
                || "OSNAP".to_owned(),
                |c| format!("OSNAP:{}", c.kind.label()),
            );
            egui::RichText::new(label)
                .monospace()
                .color(render::ON_COLOR)
        } else {
            egui::RichText::new("osnap")
                .monospace()
                .color(ui.visuals().weak_text_color())
        };
        // DYN と同じく、クリックで切り替える部品として見せる（選択を切って指のカーソル）。
        let osnap_label = fixed_width(ui, osnap_width, |ui| {
            ui.add(
                egui::Label::new(osnap_text)
                    .selectable(false)
                    .sense(egui::Sense::click()),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("オブジェクトスナップの ON/OFF  F3（点を指定するときに効く）")
        });
        if osnap_label.clicked() {
            self.toggle_osnap();
        }
        ui.separator();
        if let Some(mode) = drafting::status_toggles(ui, self.drafting) {
            self.toggle_drafting(mode);
        }
        // 動的入力。OSNAP と同じ見せ方にし、クリックでも切り替えられるようにする。
        let dyn_text = if self.session.cmdline.is_dynamic() {
            egui::RichText::new("DYN")
                .monospace()
                .color(render::ON_COLOR)
        } else {
            egui::RichText::new("dyn")
                .monospace()
                .color(ui.visuals().weak_text_color())
        };
        // ラベルは既定で文字を選べるので、そのままだとホバーで I ビームになる。
        // クリックで切り替える部品なので、選択を切って指のカーソルにする。
        let dyn_label = ui
            .add(
                egui::Label::new(dyn_text)
                    .selectable(false)
                    .sense(egui::Sense::click()),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("動的入力（カーソル横の入力欄）の ON/OFF  F12");
        if dyn_label.clicked() {
            self.toggle_dynamic_input();
        }
        ui.separator();
        // 「コマンド実行中」の欄は待機中も空のまま幅を確保する。出たり消えたりするたびに
        // 後ろの表示が跳ねないように（PR #39）。
        const RUNNING: &str = "コマンド実行中";
        let running_width = text_width(ui, RUNNING);
        let running = self.session.has_active_tool();
        fixed_width(ui, running_width, |ui| {
            if running {
                ui.colored_label(
                    egui::Color32::from_rgb(0xff, 0xc1, 0x07),
                    egui::RichText::new(RUNNING).monospace(),
                );
            }
        });
        // 区切り線の幅はスタイルで決まるので、実際に描いた前後の位置から測る。
        let before = ui.cursor().min.x;
        ui.separator();
        ui.cursor().min.x - before
    }
}

/// 等幅の文字列の幅 [px]。
fn text_width(ui: &egui::Ui, text: &str) -> f32 {
    ui.painter()
        .layout_no_wrap(
            text.to_owned(),
            egui::TextStyle::Monospace.resolve(ui.style()),
            egui::Color32::PLACEHOLDER,
        )
        .size()
        .x
}

/// 幅を `width` に固定した欄に中身を描く（中身が短くても幅を確保する）。
fn fixed_width<R>(ui: &mut egui::Ui, width: f32, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.allocate_ui_with_layout(
        egui::vec2(width, ui.spacing().interact_size.y),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_min_width(width);
            add(ui)
        },
    )
    .inner
}

/// 情報表示を前から順に、区切り線つきで横に並べる。入り切らない項目に来たら、
/// その項目から後ろを区切り線ごと省く（文字の途中で切らない。後ろの短い項目も出さない）。
///
/// `separator_width` は区切り線 1 本ぶんの幅。先頭の項目の前の区切り線は描き済みとする。
fn show_while_fits(ui: &mut egui::Ui, items: Vec<InfoItem>, separator_width: f32) {
    for (i, item) in items.into_iter().enumerate() {
        let separator = if i == 0 { 0.0 } else { separator_width };
        if text_width(ui, &item.text) + separator > ui.available_width() {
            return;
        }
        if i > 0 {
            ui.separator();
        }
        let rich = egui::RichText::new(item.text).monospace();
        let rich = match item.color {
            Some(c) => rich.color(c),
            None => rich,
        };
        // 文字を選ばせない。乗せても I ビームにならないようにする（Issue #41）。
        let response = ui.add(egui::Label::new(rich).selectable(false));
        if let Some(tip) = item.tooltip {
            response.on_hover_text(tip);
        }
    }
}

/// ステータスバーの情報表示の 1 項目。
struct InfoItem {
    text: String,
    color: Option<egui::Color32>,
    /// 省略したときの全文など。
    tooltip: Option<String>,
}

impl InfoItem {
    fn new(text: String) -> Self {
        Self {
            text,
            color: None,
            tooltip: None,
        }
    }
}

/// 座標の欄の最小の文字数（`-999999.9999` が入る）。
const COORD_MIN_WIDTH: usize = 12;

/// 座標の欄の文字数。`prev` より狭くはしない。
///
/// 欄の幅が座標の桁で伸び縮みすると、±999999.9999 の境をまたぐたびに後ろの切り替え部品が
/// 約 78px 跳ねる（PR #39 の操作レビュー）。一度広がったら、図面を入れ替えるまで縮めない。
fn coord_width_for(prev: usize, cursor: Option<Point2>) -> usize {
    cursor.map_or(prev, |p| {
        prev.max(format!("{:.4}", p.x).len())
            .max(format!("{:.4}", p.y).len())
    })
}

/// ステータスバーに出すレイヤ名の最大の文字数（省略記号を含む）。
const LAYER_NAME_MAX_CHARS: usize = 16;

/// `max_chars` 文字を超えたら、末尾を「…」にして `max_chars` 文字に収める。
fn ellipsize(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let mut short: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    short.push('…');
    short
}

/// 右側のパネル。幅は、残りの幅から作図領域の最小幅を引いた分までに毎フレーム収める。
///
/// `reserved` … このパネルより内側（あとで置く）の、開いているパネルの最小幅の合計。
/// これを空けておかないと、3 枚開いたとき内側のパネルが最小幅より狭くなる。
/// `canvas_min` … 作図領域に残す最小幅（[`CadApp::canvas_min_width`]）。
fn right_panel(
    id: &'static str,
    default_width: f32,
    min_width: f32,
    reserved: f32,
    canvas_min: f32,
    ui: &egui::Ui,
) -> egui::Panel {
    let room = (ui.available_width() - canvas_min).max(0.0);
    let max = if room - reserved >= min_width {
        room - reserved
    } else {
        room
    };
    egui::Panel::right(id)
        .default_size(default_width)
        .min_size(min_width)
        .max_size(max)
}

/// パネルの中身を、パネル自身の幅で切る（収まらない分は横スクロールで届く）。
///
/// egui の `Panel` は、中身がパネルの幅より広いと中身を隣のパネルの上へはみ出して描く。
/// 後から描くパネルがその上を塗るので、先に描いたパネルの左側（見出しやボタン）が隠れる。
fn own_width<R>(
    ui: &mut egui::Ui,
    id: &'static str,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    egui::ScrollArea::horizontal()
        .id_salt(id)
        .auto_shrink([false, true])
        .show(ui, add_contents)
        .inner
}

impl CadApp {
    /// ファイル操作の結果をコマンドラインへ出す。
    fn report_file_outcome(&mut self, outcome: FileOutcome) {
        match outcome {
            FileOutcome::Nothing => {}
            // 保存。図面はそのままなので、選択・スナップ・座標の欄は変えない（Issue #41）。
            FileOutcome::Ok(msg) => self.session.cmdline.info(msg),
            FileOutcome::Replaced(msg) => {
                // 図面が入れ替わったので、前の図面に結びついた状態（実行中のツール・選択・
                // コンポーネントの編集）とスナップを捨てる（ADR-0039）。
                // 座標の欄も最小の幅へ戻す（広がったままにしない）。
                self.session.document_replaced();
                // 念のための無効化。`document_replaced` が選択を空にして選択の版が進むので、
                // 通常は要約のキャッシュのキーが変わって作り直される。版番号が前の図面と
                // 偶然重なっても古い要約が残らないよう、明示的に捨てておく（必須の処理ではない）。
                self.properties_panel.invalidate();
                // ピック用の索引とホバーの結果、結果プレビューの境界の列も版番号をキーにしているので、
                // 前の図面のものを捨てる（PR #63 のレビュー B1。残すとクリックでも新しい図面の図形を
                // 拾えず、TRIM / EXTEND のプレビューは前の図面の境界で計算される）。
                self.hover = Hover::new();
                self.session.cmdline.info(msg);
                self.snap.release();
                self.coord_width = COORD_MIN_WIDTH;
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

    /// 内側に置くパネルのうち開いているものの最小幅の合計（[`right_panel`] の `reserved`）。
    /// `with_components` … コンポーネントパネルも内側にあるか（レイヤパネルのとき）。
    fn reserved_width(&self, with_components: bool) -> f32 {
        let components = if with_components && self.component_panel.is_open() {
            COMPONENT_PANEL_MIN_WIDTH
        } else {
            0.0
        };
        let properties = if self.properties_panel.is_open() {
            PROPERTIES_PANEL_MIN_WIDTH
        } else {
            0.0
        };
        components + properties
    }

    /// 作図領域に残す最小幅。`MIN_CANVAS_WIDTH` と、画面幅から開いているパネルの最小幅の
    /// 合計を引いた残りの、小さいほう（狭い画面ではパネルの最小幅を先に確保する）。
    fn canvas_min_width(&self, total_width: f32) -> f32 {
        let mut panels = 0.0;
        if self.layer_panel.is_open() {
            panels += LAYER_PANEL_MIN_WIDTH;
        }
        panels += self.reserved_width(true);
        MIN_CANVAS_WIDTH.min((total_width - panels).max(0.0))
    }

    /// レイヤパネルを描画し、返ってきたコマンドを適用する。
    fn layer_area(&mut self, ui: &mut egui::Ui) {
        if !self.layer_panel.is_open() {
            return;
        }
        let busy = self.session.active_command().is_some();
        let reserved = self.reserved_width(true);
        let canvas_min = self.canvas_min_width(ui.max_rect().width());
        right_panel(
            "layers",
            SIDE_PANEL_WIDTH,
            LAYER_PANEL_MIN_WIDTH,
            reserved,
            canvas_min,
            ui,
        )
        .show(ui, |ui| {
            own_width(ui, "layers_scroll", |ui| {
                let commands = self.layer_panel.show(
                    ui,
                    &self.doc,
                    &self.session.selection,
                    busy,
                    self.session.drop_note(&self.doc),
                );
                for cmd in commands {
                    self.session.apply_external(cmd, &mut self.doc);
                }
                match self.layer_panel.take_notice() {
                    Some(PanelNotice::Info(text)) => self.session.cmdline.info(text),
                    Some(PanelNotice::Error(text)) => self.session.cmdline.error(text),
                    None => {}
                }
            });
        });
    }
}

impl CadApp {
    /// コンポーネントパネルを描画し、返ってきたコマンドと依頼を処理する。
    fn component_area(&mut self, ui: &mut egui::Ui) {
        if !self.component_panel.is_open() {
            return;
        }
        let reserved = self.reserved_width(false);
        let canvas_min = self.canvas_min_width(ui.max_rect().width());
        right_panel(
            "components",
            SIDE_PANEL_WIDTH,
            COMPONENT_PANEL_MIN_WIDTH,
            reserved,
            canvas_min,
            ui,
        )
        .show(ui, |ui| {
            own_width(ui, "components_scroll", |ui| {
                let (commands, request) = self.component_panel.show(
                    ui,
                    &self.doc,
                    &self.session.selection,
                    self.session.editing(),
                );
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
        });
    }
}

impl CadApp {
    /// プロパティパネルを描画し、返ってきたコマンドを適用する。Ctrl+1 もここで扱う。
    fn properties_area(&mut self, ui: &mut egui::Ui) {
        self.handle_properties_shortcut(ui.ctx());
        if !self.properties_panel.is_open() {
            return;
        }
        let canvas_min = self.canvas_min_width(ui.max_rect().width());
        right_panel(
            "properties",
            PROPERTIES_PANEL_WIDTH,
            PROPERTIES_PANEL_MIN_WIDTH,
            0.0,
            canvas_min,
            ui,
        )
        .show(ui, |ui| {
            own_width(ui, "properties_scroll", |ui| {
                // 選択待ちを含め、コマンドを実行している間は表示だけにする。
                let busy = self.session.active_command().is_some();
                let commands = self.properties_panel.show(
                    ui,
                    &self.doc,
                    &self.session.selection,
                    busy,
                    self.session.drop_note(&self.doc),
                );
                // 選択から外れたときの案内は `Session::apply_external` が出す（レイヤパネルと共通）。
                for cmd in commands {
                    self.session.apply_external(cmd, &mut self.doc);
                }
            });
        });
    }

    /// Ctrl+1 でプロパティパネルを開閉する。
    ///
    /// キーの持ち主はコマンドラインの扱いに従う（ADR-0035）。日本語の変換中、パネルの入力欄に
    /// フォーカスがあるとき、モーダル（未保存確認）が出ているときは奪わない。ボタンやコマンド名と
    /// 違って履歴には残さず、打ちかけの文字も捨てない（キーボードで開閉するだけの操作）。
    /// 実行中のコマンドは中断しない（開閉の他のコマンドと同じ。ADR-0037）。
    fn handle_properties_shortcut(&mut self, ctx: &egui::Context) {
        let cmdline = &self.session.cmdline;
        if !cmdline.owns_keys() || cmdline.is_composing() {
            return;
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Num1)) {
            self.properties_panel.toggle();
        }
    }

    /// 図面とセッションを直接触る（スナップショットのテストが図形を並べるため。`ui_snapshot` は別モジュール）。
    #[cfg(test)]
    pub fn parts_mut(&mut self) -> (&mut Document, &mut Session) {
        (&mut self.doc, &mut self.session)
    }
}

impl CadApp {
    /// リボンの状態（スクリーンショットのテスト用。`ui_snapshot` は別モジュールなので）。
    #[cfg(test)]
    pub fn ribbon(&self) -> &Ribbon {
        &self.ribbon
    }

    /// 画面上端のリボンを描き、押されたコマンドを始める。
    ///
    /// **コマンド名を打つのと同じ扱い**（`Session::start_command_from_ui`）。
    /// コマンドラインのキー処理（`begin_frame`）より前に呼ぶ。押した結果（実行中の
    /// ツールや候補を出すか）を、同じフレームのコマンドラインの扱いに反映させるため。
    fn ribbon_area(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("ribbon").show(ui, |ui| {
            let active = self.session.active_command();
            if let Some(name) = self.ribbon.show(ui, active) {
                self.session.start_command_from_ui(name, &mut self.doc);
                self.drain_session_actions();
            }
        });
    }
}

impl eframe::App for CadApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // 順序に依存する: `handle_file_input` が先でなければ、このフレームで開いた
        // モーダルを `begin_frame` が知らず、1 フレーム分キーを奪ってしまう（Issue #24）。
        self.handle_file_input(&ctx);
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(self.window_title()));
        self.ribbon_area(ui);

        // ツール実行中と選択待ち中は候補を出さない。座標やオプションを打つ段階なので、
        // コマンド名の候補が出ると邪魔になる。
        let allow_suggestions = !self.session.has_active_tool();
        // 寸法入力の基点。Tab / Esc / Enter の扱いがこれで変わるので、キーを取る前に渡す。
        let dimension_base = self.session.dimension_base();
        self.session.cmdline.set_dimension_base(dimension_base);
        self.session
            .cmdline
            .begin_frame(&ctx, allow_suggestions, self.files.is_confirming());
        // Ctrl+A（全選択）。効く段階か（待機中・選択待ちだけ）は `select_all` が決める。
        // 確定（Enter）より前に選ぶ。同じフレームに Ctrl+A と Enter が来たら、選んでから確定する。
        if self.session.cmdline.take_select_all() {
            self.session.select_all(&self.doc);
        }
        egui::Panel::bottom("cmdline").show(ui, |ui| self.command_area(ui));
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        self.layer_area(ui);
        self.component_area(ui);
        self.properties_area(ui);
        egui::CentralPanel::no_frame().show(ui, |ui| self.canvas(ui));
        self.dynamic_input_area(&ctx);
    }
}

#[cfg(test)]
mod behavior_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use cad_core::geom::tolerance::eq_len;

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
        assert!(eq_len(avg, 2.0), "平均は 2.0ms のはず: {avg}");
        assert!(eq_len(max, 3.0), "最大は 3.0ms のはず: {max}");
    }

    /// 窓を越えても古いサンプルで壊れないこと。
    #[test]
    fn draw_timer_wraps_around() {
        let mut t = DrawTimer::new();
        for _ in 0..(DrawTimer::WINDOW * 3) {
            t.push(Duration::from_micros(500));
        }
        let (avg, max) = t.stats_ms();
        assert!(eq_len(avg, 0.5));
        assert!(eq_len(max, 0.5));
    }

    /// 座標の欄は桁の多い座標で広がり、小さい座標に戻っても縮まない。
    #[test]
    fn coord_width_grows_and_never_shrinks() {
        let w = coord_width_for(COORD_MIN_WIDTH, Some(Point2::new(210.0, -159.5)));
        assert_eq!(w, COORD_MIN_WIDTH, "小さい座標は最小の幅");
        assert_eq!(
            coord_width_for(w, Some(Point2::new(-999_999.0, 0.0))),
            COORD_MIN_WIDTH,
            "-999999.0000 は 12 文字に入る"
        );
        let w = coord_width_for(w, Some(Point2::new(0.0, 12_345_678.0)));
        assert_eq!(w, "12345678.0000".len(), "大きい座標で広がる");
        assert_eq!(
            coord_width_for(w, Some(Point2::new(1.0, 1.0))),
            w,
            "縮まない"
        );
        assert_eq!(coord_width_for(w, None), w, "カーソルが無くても縮まない");
    }

    /// 長いレイヤ名は 16 文字に省略する（文字数で数える。日本語も 1 文字）。
    #[test]
    fn long_layer_names_are_ellipsized() {
        assert_eq!(ellipsize("0", 16), "0");
        assert_eq!(
            ellipsize("ちょうど十六文字のレイヤ名です。", 16),
            "ちょうど十六文字のレイヤ名です。"
        );
        let long = "とても長いレイヤの名前で後ろの表示を押し出してしまう";
        let short = ellipsize(long, 16);
        assert_eq!(short.chars().count(), 16);
        assert!(short.ends_with('…'));
        assert!(long.starts_with(short.trim_end_matches('…')));
    }

    /// 既定の図面範囲は空でないこと（ZOOM ALL が無反応にならない）。
    #[test]
    fn default_limits_are_not_empty() {
        let b = default_drawing_limits();
        assert!(!b.is_empty());
        assert!(b.width() > 0.0 && b.height() > 0.0);
    }
}
