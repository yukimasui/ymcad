//! リボンのアイコン。`crates/cad-app/assets/icons/<コマンド名>.svg` を埋め込んで表示する。
//!
//! # SVG ファイルにしている理由（ADR-0037）
//!
//! 絵をコードから切り離し、**ファイルを差し替えてビルドし直すだけで絵が変わる**ように
//! するため。ユーザーが 1 つずつ外部のツールで描き直して置き換えられる。
//! コード側に持つのは「コマンド名 → SVG のバイト列」の表（[`ICONS`]）だけ。
//!
//! # SVG の作法（詳しくは `assets/icons/README.md`）
//!
//! - `viewBox="0 0 24 24"`、**白一色（`#ffffff`）**の線・塗りで描く。色は表示側が
//!   `tint`（乗算）で付ける。通常は文字色、実行中のボタンは選択色
//! - 補助線（元の形・消える部分）は白のまま不透明度を下げる（`stroke-opacity` など）。
//!   乗算なので、どの色に着色しても「薄い同じ色」になる
//! - 線幅は 1.5 にそろえる。文字（`<text>`）は使わない（`svg_text` 機能を入れていないので出ない）
//! - `currentColor` は使わない。ルートに `fill="none"` を書く（どちらも書かないと黒になる）
//!
//! # 差し替えの口
//!
//! リボン（`ribbon/mod.rs`）がアイコンに触るのは [`paint`] だけ。網羅のテストは
//! このファイルの末尾にある（全コマンドに SVG がある・余分なファイルが無い・全部読める・
//! ラスタライズした画素が白一色）。

/// 1 アイコン = (コマンド名, SVG のバイト列)。
macro_rules! icon {
    ($name:literal) => {
        (
            $name,
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/assets/icons/",
                $name,
                ".svg"
            ))
            .as_slice(),
        )
    };
}

/// リボンに置く全コマンドのアイコン。並びは見やすさのためだけで、意味は無い。
static ICONS: &[(&str, &[u8])] = &[
    icon!("LINE"),
    icon!("POLYLINE"),
    icon!("CIRCLE"),
    icon!("ARC"),
    icon!("RECTANGLE"),
    icon!("XLINE"),
    icon!("ERASE"),
    icon!("MOVE"),
    icon!("COPY"),
    icon!("STRETCH"),
    icon!("ROTATE"),
    icon!("SCALE"),
    icon!("MIRROR"),
    icon!("TRIM"),
    icon!("EXTEND"),
    icon!("FILLET"),
    icon!("CHAMFER"),
    icon!("EXPLODE"),
    icon!("GROUP"),
    icon!("UNGROUP"),
    icon!("UNDO"),
    icon!("REDO"),
    icon!("COMPONENT"),
    icon!("REDEFINE"),
    icon!("EDITCOMP"),
    icon!("ENDCOMP"),
    icon!("INSERT"),
    icon!("PARAM"),
    icon!("BIND"),
    icon!("PSET"),
    icon!("COMPONENTS"),
    icon!("ZOOM"),
    icon!("LAYER"),
    icon!("NEW"),
    icon!("OPEN"),
    icon!("SAVE"),
    icon!("SAVEAS"),
];

/// コマンド名に対応する SVG のバイト列。
#[must_use]
pub fn svg(name: &str) -> Option<&'static [u8]> {
    ICONS.iter().find(|(n, _)| *n == name).map(|(_, b)| *b)
}

/// `rect` の中にコマンドのアイコンを `color` で描く。アイコンが無ければ何も描かずに `false`。
///
/// SVG の読み込み（ラスタライズ）は `egui_extras` の画像ローダーが行い、大きさごとに
/// キャッシュされる。ローダーはリボンが描く前に入れている（`Ribbon::show`）。
pub fn paint(ui: &egui::Ui, name: &str, rect: egui::Rect, color: egui::Color32) -> bool {
    let Some(bytes) = svg(name) else {
        return false;
    };
    // URI の拡張子 `.svg` で egui_extras の SVG ローダーが選ばれる。
    egui::Image::from_bytes(format!("bytes://ribbon/{name}.svg"), bytes)
        .tint(color)
        .fit_to_exact_size(rect.size())
        .paint_at(ui, rect);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ribbon::layout;
    use crate::tools::COMMANDS;

    fn icon_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/icons")
    }

    /// リボンに置いた全コマンドに SVG があること。
    #[test]
    fn every_placed_command_has_an_icon() {
        for name in layout::placed_commands() {
            assert!(
                svg(name).is_some(),
                "{name} のアイコンが無い（assets/icons/{name}.svg と ribbon/icons.rs の表）"
            );
        }
    }

    /// 表の名前がリボンに置いたコマンドだけで、重複が無いこと。
    #[test]
    fn icon_table_has_no_stray_or_duplicate_names() {
        let placed: Vec<_> = layout::placed_commands().collect();
        let mut seen = std::collections::BTreeSet::new();
        for (name, _) in ICONS {
            assert!(
                placed.contains(name),
                "{name} はリボンに無いのにアイコンの表にある"
            );
            assert!(seen.insert(*name), "{name} が表で重複している");
        }
        assert!(
            COMMANDS.len() > ICONS.len(),
            "除外したコマンド（QUIT）の分だけ少ない前提"
        );
    }

    /// `assets/icons/` に、対応するコマンドの無い余分なファイルが無いこと。
    /// 名前を打ち間違えた SVG を置くと、表から参照されずに黙って使われないため。
    #[test]
    fn no_stray_files_in_the_icon_folder() {
        for entry in std::fs::read_dir(icon_dir()).expect("assets/icons が読める") {
            let path = entry.expect("項目が読める").path();
            let file = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_owned();
            if file == "README.md" {
                continue;
            }
            let stem = file
                .strip_suffix(".svg")
                .unwrap_or_else(|| panic!("SVG 以外のファイルがある: {file}"));
            assert!(svg(stem).is_some(), "{file} に対応するコマンドが表に無い");
        }
    }

    /// 全 SVG が `usvg`（egui_extras が使うもの）で読め、ラスタライズできること。
    #[test]
    fn every_icon_can_be_rasterized() {
        for (name, bytes) in ICONS {
            let image = egui_extras::image::load_svg_bytes(bytes, &Default::default())
                .unwrap_or_else(|e| panic!("{name}.svg が読めない: {e}"));
            assert_eq!(image.size, [24, 24], "{name}.svg は 24×24 で描く");
            assert!(
                image.pixels.iter().any(|p| p.a() > 0),
                "{name}.svg が何も描いていない"
            );
        }
    }

    /// 描かれた画素がすべて白（不透明度だけが違う）であること。
    ///
    /// 表示側の `tint` は乗算なので、白以外の画素はその色のまま残る。とくに黒は
    /// どの色に着色しても黒で、暗いテーマではアイコンが消える。
    ///
    /// 属性の文字列ではなく**ラスタライズした結果**で確かめる。文字列の検査では、
    /// `stroke="currentColor"`（`color` が無ければ黒になる）や、ルートの `fill="none"` を
    /// 書き忘れた図形（SVG の既定の塗りは黒）を見逃していた（PR #32 のレビュー）。
    /// 画素は乗算済みアルファなので、白なら `r == g == b == a` になる。半透明の補助線も同じ
    /// （縁の補間でもずれないことを確かめたので、差は許さない）。
    #[test]
    fn icons_are_white_only() {
        for (name, bytes) in ICONS {
            for hint in [
                egui::SizeHint::default(),
                egui::SizeHint::Size {
                    width: 72,
                    height: 72,
                    maintain_aspect_ratio: true,
                },
            ] {
                let image =
                    egui_extras::image::load_svg_bytes_with_size(bytes, hint, &Default::default())
                        .unwrap_or_else(|e| panic!("{name}.svg が読めない: {e}"));
                let bad = image
                    .pixels
                    .iter()
                    .filter(|p| p.a() > 0)
                    .filter(|p| {
                        let a = p.a();
                        [p.r(), p.g(), p.b()].iter().any(|&c| c != a)
                    })
                    .count();
                assert_eq!(
                    bad, 0,
                    "{name}.svg に白でない画素が {bad} 個ある（色は #ffffff だけ。\
                     currentColor や、ルートの fill=\"none\" の書き忘れは黒になる）"
                );
            }
        }
    }
}
