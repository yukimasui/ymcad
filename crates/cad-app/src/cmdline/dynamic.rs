//! カーソル横の動的入力（Issue #20 段階 A）の配置と表示判定。
//!
//! **egui に依存しない純粋関数だけを置く。** 画面に出さずに挙動を単体テストで
//! 固定するため。egui の型との変換は呼び出し側（`cmdline/mod.rs`）で行う。
//!
//! 扱う値はすべて画面上の論理 px。図面の座標（`f64`）はここに来ない。

/// カーソルから入力欄までのずらし量 [px]（縦横とも）。
///
/// スナップの解放半径 16px とマーカーの半辺 4.5px を足しても 20.5px で、
/// 斜めに 20px ずらせば角までの距離は約 28px になる。
/// **スナップマーカーとカーソル十字を覆わない**ための値。
pub const CURSOR_OFFSET_PX: f32 = 20.0;

/// エラーをカーソル横に出しておく時間 [秒]。履歴には期限なく残る。
pub const ERROR_DISPLAY_SECS: f64 = 3.0;

/// 画面上の点 [px]。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

/// 大きさ [px]。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Size {
    pub w: f32,
    pub h: f32,
}

/// 収めたい領域 [px]。y は下向き（egui の画面座標と同じ）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Bounds {
    /// 中央の点。基準点がまだ一度も決まっていないときに使う。
    #[must_use]
    pub fn center(self) -> Point {
        Point {
            x: (self.left + self.right) / 2.0,
            y: (self.top + self.bottom) / 2.0,
        }
    }
}

/// 入力欄の左上の位置を決める。
///
/// - 通常はカーソルの右下（[`CURSOR_OFFSET_PX`] だけずらす）
/// - 右へはみ出すならカーソルの左側へ、下へはみ出すなら上側へ回り込む
/// - 回り込んでも収まらなければ領域の内側へ寄せる。
///   **領域より大きくても左上より外には出さない**（読み始めの位置が見えるように）
#[must_use]
pub fn place(anchor: Point, size: Size, bounds: Bounds) -> Point {
    Point {
        x: place_axis(anchor.x, size.w, bounds.left, bounds.right),
        y: place_axis(anchor.y, size.h, bounds.top, bounds.bottom),
    }
}

/// 1 軸ぶんの配置。`lo..hi` が領域、`extent` が入力欄の長さ。
fn place_axis(anchor: f32, extent: f32, lo: f32, hi: f32) -> f32 {
    let after = anchor + CURSOR_OFFSET_PX;
    let start = if after + extent <= hi {
        after
    } else {
        anchor - CURSOR_OFFSET_PX - extent
    };
    // 順番が大事。先に右下端で抑え、最後に左上端で抑えると、
    // 領域より大きいときに左上が優先される。
    start.min(hi - extent).max(lo)
}

/// 変換中は位置を固定する。
///
/// 変換が始まったフレームで直前の位置（`previous`）を覚え、変換の間はそこに留める。
/// 確定・取り消しで固定を外し、計算した位置（`computed`）へ戻す。
///
/// 毎フレーム追従させると、入力欄に付いて出る IME の候補ウィンドウが
/// マウスの動きに合わせて跳ねる（Issue #20 の試作で確認する項目）。
pub fn resolve_position(
    composing: bool,
    frozen: &mut Option<Point>,
    previous: Option<Point>,
    computed: Point,
) -> Point {
    if !composing {
        *frozen = None;
        return computed;
    }
    *frozen.get_or_insert(previous.unwrap_or(computed))
}

/// カーソル横を見せるかどうかの判断材料。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Activity {
    /// コマンドを実行中（選択待ちを含む）。
    pub tool_active: bool,
    /// 入力欄に文字がある。
    pub has_input: bool,
    /// IME で変換中。
    pub composing: bool,
    /// エラーの表示期間内。
    pub error_shown: bool,
}

/// カーソル横に何かを見せるか。
///
/// **何もしていないときは出さない**（慣れた人にはうるさい、という不満への対策）。
/// どれか 1 つでも当てはまれば出す。
#[must_use]
pub fn is_visible(a: Activity) -> bool {
    a.tool_active || a.has_input || a.composing || a.error_shown
}

/// エラーを出した時刻 `shown_at` から見て、`now` でまだ表示期間内なら残り秒数を返す。
///
/// 残り時間は「期限が来たら消すための再描画」を予約するのに使う。
/// 何も操作しないと egui は再描画しないので、予約しないとエラーが出たまま残る。
#[must_use]
pub fn error_remaining(shown_at: f64, now: f64) -> Option<f64> {
    let remaining = ERROR_DISPLAY_SECS - (now - shown_at);
    (remaining > 0.0).then_some(remaining)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CANVAS: Bounds = Bounds {
        left: 0.0,
        top: 30.0,
        right: 1000.0,
        bottom: 700.0,
    };
    const BOX: Size = Size { w: 200.0, h: 80.0 };

    fn pt(x: f32, y: f32) -> Point {
        Point { x, y }
    }

    #[test]
    fn placed_below_right_of_the_cursor_normally() {
        let p = place(pt(300.0, 300.0), BOX, CANVAS);
        assert_eq!(p, pt(300.0 + CURSOR_OFFSET_PX, 300.0 + CURSOR_OFFSET_PX));
    }

    #[test]
    fn wraps_to_the_left_near_the_right_edge() {
        let p = place(pt(900.0, 300.0), BOX, CANVAS);
        assert_eq!(p.x, 900.0 - CURSOR_OFFSET_PX - BOX.w, "左側へ回り込む");
        assert_eq!(p.y, 300.0 + CURSOR_OFFSET_PX, "縦はそのまま");
        assert!(p.x + BOX.w < 900.0, "カーソルに被らない");
    }

    #[test]
    fn wraps_upward_near_the_bottom_edge() {
        let p = place(pt(300.0, 650.0), BOX, CANVAS);
        assert_eq!(p.x, 300.0 + CURSOR_OFFSET_PX, "横はそのまま");
        assert_eq!(p.y, 650.0 - CURSOR_OFFSET_PX - BOX.h, "上側へ回り込む");
        assert!(p.y + BOX.h < 650.0, "カーソルに被らない");
    }

    #[test]
    fn wraps_both_ways_in_the_bottom_right_corner() {
        let p = place(pt(990.0, 690.0), BOX, CANVAS);
        assert_eq!(
            p,
            pt(
                990.0 - CURSOR_OFFSET_PX - BOX.w,
                690.0 - CURSOR_OFFSET_PX - BOX.h
            )
        );
    }

    /// ちょうど収まる位置では回り込まない（境界の扱い）。
    #[test]
    fn exactly_fitting_does_not_wrap() {
        let x = CANVAS.right - BOX.w - CURSOR_OFFSET_PX;
        let p = place(pt(x, 300.0), BOX, CANVAS);
        assert_eq!(p.x, x + CURSOR_OFFSET_PX);
        assert_eq!(p.x + BOX.w, CANVAS.right);
    }

    /// どこに置いても領域からはみ出さないこと。
    #[test]
    fn always_stays_inside_the_canvas() {
        let mut y = CANVAS.top;
        while y <= CANVAS.bottom {
            let mut x = CANVAS.left;
            while x <= CANVAS.right {
                let p = place(pt(x, y), BOX, CANVAS);
                assert!(p.x >= CANVAS.left && p.x + BOX.w <= CANVAS.right, "{x},{y}");
                assert!(p.y >= CANVAS.top && p.y + BOX.h <= CANVAS.bottom, "{x},{y}");
                x += 25.0;
            }
            y += 25.0;
        }
    }

    /// 回り込むと上・左へはみ出す位置（キャンバスの左上寄り + 大きな入力欄）でも収める。
    #[test]
    fn clamped_when_wrapping_would_overflow_the_other_side() {
        let tall = Size { w: 200.0, h: 600.0 };
        // 下には入らず、上へ回り込むと top を越える。
        let p = place(pt(300.0, 200.0), tall, CANVAS);
        assert_eq!(p.y, CANVAS.top, "上端に寄せる");
        assert_eq!(p.y + tall.h, 630.0);
    }

    /// キャンバスより大きくても左上より外へは出ない。
    #[test]
    fn larger_than_canvas_never_goes_past_the_top_left() {
        let huge = Size {
            w: 5000.0,
            h: 5000.0,
        };
        for anchor in [pt(0.0, 30.0), pt(500.0, 400.0), pt(1000.0, 700.0)] {
            let p = place(anchor, huge, CANVAS);
            assert_eq!(p, pt(CANVAS.left, CANVAS.top), "{anchor:?}");
        }
    }

    #[test]
    fn bounds_center_is_the_midpoint() {
        assert_eq!(CANVAS.center(), pt(500.0, 365.0));
    }

    #[test]
    fn hidden_when_nothing_is_going_on() {
        assert!(!is_visible(Activity::default()));
    }

    #[test]
    fn shown_for_each_kind_of_activity() {
        let cases = [
            Activity {
                tool_active: true,
                ..Activity::default()
            },
            Activity {
                has_input: true,
                ..Activity::default()
            },
            Activity {
                composing: true,
                ..Activity::default()
            },
            Activity {
                error_shown: true,
                ..Activity::default()
            },
        ];
        for a in cases {
            assert!(is_visible(a), "{a:?}");
        }
    }

    #[test]
    fn error_is_shown_within_the_period() {
        assert_eq!(error_remaining(10.0, 10.0), Some(ERROR_DISPLAY_SECS));
        assert_eq!(error_remaining(10.0, 11.0), Some(ERROR_DISPLAY_SECS - 1.0));
    }

    #[test]
    fn error_is_hidden_after_the_period() {
        assert_eq!(error_remaining(10.0, 10.0 + ERROR_DISPLAY_SECS), None);
        assert_eq!(error_remaining(10.0, 100.0), None);
    }

    #[test]
    fn follows_the_cursor_while_not_composing() {
        let mut frozen = None;
        let p = resolve_position(false, &mut frozen, Some(pt(1.0, 1.0)), pt(50.0, 60.0));
        assert_eq!(p, pt(50.0, 60.0));
        assert_eq!(frozen, None);
    }

    /// 変換が始まった時点の位置に留まり、確定で追従に戻ること。
    #[test]
    fn frozen_while_composing_and_released_on_commit() {
        let mut frozen = None;
        let start = pt(10.0, 20.0);

        // 変換開始: 直前の位置で固定する。
        let p = resolve_position(true, &mut frozen, Some(start), pt(50.0, 60.0));
        assert_eq!(p, start);

        // 変換中にマウスが動いても動かない。
        let p = resolve_position(true, &mut frozen, Some(start), pt(300.0, 400.0));
        assert_eq!(p, start);

        // 確定で追従を再開する。
        let p = resolve_position(false, &mut frozen, Some(start), pt(300.0, 400.0));
        assert_eq!(p, pt(300.0, 400.0));
        assert_eq!(frozen, None);
    }

    /// 直前の位置が無い（初回）なら、計算した位置で固定する。
    #[test]
    fn freezes_at_the_computed_position_when_there_is_no_previous() {
        let mut frozen = None;
        let p = resolve_position(true, &mut frozen, None, pt(50.0, 60.0));
        assert_eq!(p, pt(50.0, 60.0));
        assert_eq!(frozen, Some(pt(50.0, 60.0)));
    }
}
