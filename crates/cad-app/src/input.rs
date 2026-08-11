//! 入力の解釈。
//!
//! egui のイベントを「何をしたいか」（[`ViewAction`]）へ翻訳するだけで、
//! ここでは図面もビューポートも変更しない。適用は [`crate::app`] の責務。

use crate::viewport::Viewport;

/// ホイールの移動量がこの points ぶんで 1 段ズームする。
const SCROLL_POINTS_PER_ZOOM_STEP: f32 = 50.0;
/// 1 段あたりのズーム倍率。
const ZOOM_STEP: f64 = 1.1;

/// ビューに対する操作。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ViewAction {
    /// スクリーン上の移動量ぶん図面をずらす。
    Pan(egui::Vec2),
    /// `anchor` のモデル座標を固定したままズームする。
    ZoomAt { anchor: egui::Pos2, factor: f64 },
    /// 全エンティティが収まるようフィットする（ZOOM EXTENTS）。
    ZoomExtents,
    /// 図面範囲にフィットする（ZOOM ALL）。
    ZoomAll,
}

/// キー入力の途中状態。
///
/// AutoCAD の `Z` → `E` のような 2 段のキー入力を扱う。
///
/// Phase 3 で常設のコマンドラインを実装したら、ZOOM は他のコマンドと同様に
/// コマンドラインへ統合し、この暫定的な状態機械は置き換える。
#[derive(Debug, Default)]
pub struct KeySequence {
    /// `Z` を受け取り、オプション（`E` / `A`）を待っている。
    awaiting_zoom_option: bool,
}

impl KeySequence {
    /// 入力待ちのプロンプト文字列。何も待っていなければ `None`。
    #[must_use]
    pub fn prompt(&self) -> Option<&'static str> {
        self.awaiting_zoom_option
            .then_some("ZOOM オプションを指定 [全体(A)/範囲(E)]:")
    }

    /// 途中状態を捨てる（`Esc` 用）。
    pub fn reset(&mut self) {
        self.awaiting_zoom_option = false;
    }
}

/// このフレームの入力から [`ViewAction`] を集める。
///
/// `response` は図面キャンバスのもの。
#[must_use]
pub fn collect_view_actions(
    response: &egui::Response,
    ui: &egui::Ui,
    vp: &Viewport,
    keys: &mut KeySequence,
) -> Vec<ViewAction> {
    let mut actions = Vec::new();

    // ---- パン: 中ボタンドラッグ ----
    if response.dragged_by(egui::PointerButton::Middle) {
        let delta = response.drag_delta();
        if delta != egui::Vec2::ZERO {
            actions.push(ViewAction::Pan(delta));
        }
    }

    // ---- ズーム: ホイール ----
    //
    // カーソル直下のモデル座標を固定するため、必ずアンカーを渡す。
    // カーソルが画面外なら画面中心を使う。
    let anchor = response
        .hover_pos()
        .or_else(|| ui.input(|i| i.pointer.latest_pos()))
        .unwrap_or_else(|| vp.rect().center());

    if response.contains_pointer() {
        let (scroll_y, pinch) = ui.input(|i| (i.smooth_scroll_delta.y, f64::from(i.zoom_delta())));

        // CAD ではホイールはスクロールではなくズーム。
        // ホイール手前（下）で縮小、奥（上）で拡大。
        if scroll_y != 0.0 {
            let steps = f64::from(scroll_y / SCROLL_POINTS_PER_ZOOM_STEP);
            actions.push(ViewAction::ZoomAt {
                anchor,
                factor: ZOOM_STEP.powf(steps),
            });
        }

        // ピンチ操作（タッチパッド）も同じ扱いにする。
        if (pinch - 1.0).abs() > f64::EPSILON {
            actions.push(ViewAction::ZoomAt {
                anchor,
                factor: pinch,
            });
        }
    }

    // ---- キー入力 ----
    ui.input(|i| {
        if i.key_pressed(egui::Key::Escape) {
            keys.reset();
            return;
        }

        if keys.awaiting_zoom_option {
            if i.key_pressed(egui::Key::E) {
                actions.push(ViewAction::ZoomExtents);
                keys.awaiting_zoom_option = false;
            } else if i.key_pressed(egui::Key::A) {
                actions.push(ViewAction::ZoomAll);
                keys.awaiting_zoom_option = false;
            }
        } else if i.key_pressed(egui::Key::Z) {
            keys.awaiting_zoom_option = true;
        }
    });

    actions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_clears_pending_sequence() {
        let mut k = KeySequence {
            awaiting_zoom_option: true,
        };
        assert!(k.prompt().is_some());
        k.reset();
        assert!(k.prompt().is_none());
    }

    /// ホイール 1 ノッチ（50 points 相当）でちょうど 1 段ズームすること。
    #[test]
    fn one_wheel_notch_is_one_zoom_step() {
        let steps = f64::from(SCROLL_POINTS_PER_ZOOM_STEP / SCROLL_POINTS_PER_ZOOM_STEP);
        let factor = ZOOM_STEP.powf(steps);
        assert!((factor - ZOOM_STEP).abs() < 1e-12);
    }

    /// ホイールを奥へ回すと拡大、手前へ回すと縮小になること。
    #[test]
    fn wheel_direction_maps_to_zoom_direction() {
        let zoom_in = ZOOM_STEP.powf(f64::from(SCROLL_POINTS_PER_ZOOM_STEP) / 50.0);
        let zoom_out = ZOOM_STEP.powf(f64::from(-SCROLL_POINTS_PER_ZOOM_STEP) / 50.0);
        assert!(zoom_in > 1.0, "奥へ回したら拡大");
        assert!(zoom_out < 1.0, "手前へ回したら縮小");
        // 往復すれば元に戻る。
        assert!((zoom_in * zoom_out - 1.0).abs() < 1e-12);
    }
}
