//! システムから日本語対応フォントを探して egui に追加する。
//!
//! egui の同梱フォント (Ubuntu-Light / Hack) は CJK グリフを持たないため、
//! これを行わないと日本語がすべて豆腐 (□) になり、IME 検証そのものが成立しない。
//!
//! 既定フォントの **後ろ** に追加するため、ラテン文字は egui 既定の見た目を保ち、
//! グリフが無い CJK のみがこのフォントにフォールバックする。

use std::path::PathBuf;
use std::sync::Arc;

/// 探索候補。`(パス, .ttc 内のフェイス index)`。先頭から順に存在チェックする。
///
/// Noto Sans CJK は TrueType Collection なので index 指定が必要。
/// `fc-query` で確認した Ubuntu 24.04 でのフェイス順は index 0 = `Noto Sans CJK JP`。
const CANDIDATES: &[(&str, u32)] = &[
    ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 0),
    ("/usr/share/fonts/opentype/noto/NotoSansCJK-VF.otf.ttc", 0),
    ("/usr/share/fonts/truetype/fonts-japanese-gothic.ttf", 0),
    (
        "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
        0,
    ),
];

/// 実際に読み込めたフォントの情報。画面に表示して検証時の手がかりにする。
pub struct LoadedFont {
    pub path: PathBuf,
    pub index: u32,
}

/// 日本語フォントを `ctx` に追加する。見つからなければ `None`。
pub fn install(ctx: &egui::Context) -> Option<LoadedFont> {
    let (path, index) = CANDIDATES
        .iter()
        .find(|(p, _)| std::path::Path::new(p).is_file())
        .copied()?;

    let bytes = std::fs::read(path).ok()?;

    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "jp".to_owned(),
        Arc::new(egui::FontData {
            font: bytes.into(),
            index,
            tweak: Default::default(),
        }),
    );

    // 既定フォントの後ろに置く = ラテンは既定、CJK だけここへフォールバック。
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .push("jp".to_owned());
    }

    ctx.set_fonts(fonts);

    Some(LoadedFont {
        path: PathBuf::from(path),
        index,
    })
}
