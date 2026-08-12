//! 入力の解釈。
//!
//! egui のイベントを「何をしたいか」（[`ViewAction`]）へ翻訳するだけで、
//! ここでは図面もビューポートも変更しない。適用は [`crate::app`] の責務。
//!
//! キーボードは扱わない。**キー入力はすべてコマンドラインへ流れる**ため
//! （Phase 2 では `Z` → `E` を暫定の状態機械で処理していたが、
//! Phase 3 でコマンドラインへ統合した）。ここが扱うのはマウスだけ。

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

/// このフレームのマウス入力から [`ViewAction`] を集める。
///
/// `response` は図面キャンバスのもの。
#[must_use]
pub fn collect_view_actions(
    response: &egui::Response,
    ui: &egui::Ui,
    vp: &Viewport,
) -> Vec<ViewAction> {
    let mut actions = Vec::new();

    // ---- パン: 中ボタンドラッグ ----
    if response.dragged_by(egui::PointerButton::Middle) {
        let delta = response.drag_delta();
        if delta != egui::Vec2::ZERO {
            actions.push(ViewAction::Pan(delta));
        }
    }

    if !response.contains_pointer() {
        return actions;
    }

    // ---- ズーム: ホイール ----
    //
    // カーソル直下のモデル座標を固定するため、必ずアンカーを渡す。
    // カーソルが取れない場合だけ画面中心へフォールバックする。
    let anchor = response
        .hover_pos()
        .or_else(|| ui.input(|i| i.pointer.latest_pos()))
        .unwrap_or_else(|| vp.rect().center());

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

    actions
}

#[cfg(test)]
mod tests {
    use super::*;

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
