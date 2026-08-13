//! ymcad — AutoCAD ライクな 2D CAD。
//!
//! 起動方法は README を参照。Wayland で不調な場合は `WAYLAND_DISPLAY= cargo run` で
//! X11 (XWayland) にフォールバックできる（`WINIT_UNIX_BACKEND` は winit 0.30 で廃止済み）。

// 図面座標は f64。f32 への縮小は viewport.rs の変換関数だけに閉じ込める。
#![forbid(unsafe_code)]

mod app;
mod cmdline;
mod component_panel;
mod editing;
mod file_ops;
mod input;
mod jp_font;
mod layer_panel;
mod render;
mod resolved;
mod selection;
mod session;
mod snap;
mod tools;
mod viewport;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([640.0, 480.0])
            .with_title("ymcad"),
        ..Default::default()
    };

    eframe::run_native(
        "ymcad",
        options,
        Box::new(|cc| {
            // egui の同梱フォントは CJK グリフを持たないため、日本語 UI には必須。
            // 読み込めなくても起動は続行し、ステータスバーで警告する。
            let font = jp_font::install(&cc.egui_ctx)
                .map(|f| format!("{} (face {})", f.path.display(), f.index));
            Ok(Box::new(app::CadApp::new(font)))
        }),
    )
}
