//! 動的入力のそばに出すヒント（Issue #27）の中身・出すかどうか・置き場所。
//!
//! **egui に依存しない純粋関数だけを置く**（`cmdline/dynamic.rs` と同じ方針）。
//! 描画は `cmdline/mod.rs` の `CommandLine::show_hint`。設計判断は ADR-0048。
//!
//! # 入力欄を動かさない
//!
//! ヒントは入力欄と同じ `Area` に入れず、入力欄の枠（カーソル横の `Area`）の外側に
//! 寄り添う**別の `Area`** に出す。同じ枠に入れると、枠の大きさが変わって画面端で
//! `constrain_to` が入力欄を押し戻し、入力欄に付いて出る IME の候補ウィンドウが跳ねる
//! （ADR-0034 の追記と同じ形）。ここで決めるのはヒントの `Area` の位置だけで、
//! 入力欄の枠の位置には一切触らない。
//!
//! 扱う値はすべて画面上の論理 px。

use super::command_help;
use super::dynamic::{Bounds, Point, Size, CURSOR_OFFSET_PX};

/// 入力欄の枠とヒントの間のすき間 [px]。
pub const GAP_PX: f32 = 6.0;

/// ヒントの中身。
///
/// 段階 1 はコマンドの説明だけ。段階 2（実行中に今できること）はここに種類を足す。
/// 大きさは描画側が毎フレーム、見えない別の `Area` で測ってから置く（ADR-0048 決定 3）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Hint {
    /// 候補で `Enter` に実行されるコマンドの説明。
    Command {
        /// コマンドの正式名。説明の左に出すアイコン（リボンと同じ絵）の名前も兼ねる。
        name: &'static str,
        /// 説明（`cmdline/command_help.rs`）。
        description: &'static str,
    },
}

/// コマンドの説明のヒント。説明の無いコマンド名なら `None`（全コマンドにあることはテストで検査）。
#[must_use]
pub fn for_command(name: &'static str) -> Option<Hint> {
    command_help::description(name).map(|description| Hint::Command { name, description })
}

/// ヒントを出してよい状況か。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Situation {
    /// ステータスバーの `HINT` がオン。
    pub enabled: bool,
    /// このフレームで動的入力（カーソル横の入力欄）がオン。
    pub dynamic: bool,
    /// カーソル横の枠が見えている（何もしていないときは見えない）。
    pub box_visible: bool,
    /// IME で変換中。
    pub composing: bool,
    /// モーダル（未保存確認）が出ている。
    pub modal_open: bool,
}

/// ヒントを出してよいか。
///
/// - `HINT` がオフ・動的入力がオフなら出さない（動的入力のそばに出すものなので）
/// - 枠が見えていない（何もしていない）なら出さない
/// - **変換中は出さない。** 変換候補のウィンドウと重なりうるうえ、変換中は
///   入力欄の中身を解釈しない（ADR-0002）ので、候補もヒントも止まった状態になる
/// - モーダルの間は枠ごと隠れているので出さない
#[must_use]
pub fn allowed(s: Situation) -> bool {
    s.enabled && s.dynamic && s.box_visible && !s.composing && !s.modal_open
}

/// ヒントの左上の位置を決める。どこにも入らなければ `None`（出さない）。
///
/// - `input_box` … 入力欄の枠（カーソル横の `Area`）の、このフレームの矩形
/// - `cursor` … 枠の基準にしたカーソル位置
/// - `size` … ヒントの大きさ
/// - `bounds` … 収める領域（キャンバス）
///
/// 置き方:
///
/// 1. **縦方向で、カーソルから遠い側。** 枠がカーソルの下にあれば枠の下、上にあれば枠の上。
///    左端は枠にそろえ、領域からはみ出すぶんだけ横へ寄せる
/// 2. 1 が入らなければ、**横方向でカーソルから遠い側。** 枠がカーソルの右にあれば枠の右、
///    左にあれば枠の左。上端は枠にそろえ、はみ出すぶんだけ縦へ寄せる
/// 3. どちらも入らなければ出さない
///
/// どの置き方でも、**領域の内側**にあり、**枠にもカーソルの周り**（カーソルから
/// [`CURSOR_OFFSET_PX`] 以内。スナップマーカーと十字の範囲）**にも重ならない**ときだけ採る。
#[must_use]
pub fn place_hint(input_box: Bounds, cursor: Point, size: Size, bounds: Bounds) -> Option<Point> {
    let below = center_y(input_box) >= cursor.y;
    let right = center_x(input_box) >= cursor.x;

    let vertical = Point {
        x: fit_into(input_box.left, size.w, bounds.left, bounds.right),
        y: if below {
            input_box.bottom + GAP_PX
        } else {
            input_box.top - GAP_PX - size.h
        },
    };
    let horizontal = Point {
        x: if right {
            input_box.right + GAP_PX
        } else {
            input_box.left - GAP_PX - size.w
        },
        y: fit_into(input_box.top, size.h, bounds.top, bounds.bottom),
    };

    let cursor_zone = Bounds {
        left: cursor.x - CURSOR_OFFSET_PX,
        top: cursor.y - CURSOR_OFFSET_PX,
        right: cursor.x + CURSOR_OFFSET_PX,
        bottom: cursor.y + CURSOR_OFFSET_PX,
    };
    [vertical, horizontal].into_iter().find(|p| {
        let r = rect_at(*p, size);
        contains(bounds, r) && !overlaps(r, input_box) && !overlaps(r, cursor_zone)
    })
}

/// `start` から長さ `extent` の区間を `lo..hi` に収まるよう寄せる（入らなければ `lo` 側）。
/// 入りきらないかどうかは呼び出し側の [`contains`] で判定する。
fn fit_into(start: f32, extent: f32, lo: f32, hi: f32) -> f32 {
    start.min(hi - extent).max(lo)
}

fn center_x(r: Bounds) -> f32 {
    (r.left + r.right) / 2.0
}

fn center_y(r: Bounds) -> f32 {
    (r.top + r.bottom) / 2.0
}

fn rect_at(p: Point, size: Size) -> Bounds {
    Bounds {
        left: p.x,
        top: p.y,
        right: p.x + size.w,
        bottom: p.y + size.h,
    }
}

/// `inner` が `outer` の内側にあるか（辺が重なるのは内側に数える）。
fn contains(outer: Bounds, inner: Bounds) -> bool {
    inner.left >= outer.left
        && inner.top >= outer.top
        && inner.right <= outer.right
        && inner.bottom <= outer.bottom
}

/// 2 つの矩形が重なるか（辺が接するだけなら重ならない）。
fn overlaps(a: Bounds, b: Bounds) -> bool {
    a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmdline::dynamic;

    const CANVAS: Bounds = Bounds {
        left: 0.0,
        top: 30.0,
        right: 1000.0,
        bottom: 700.0,
    };
    /// 入力欄の枠の大きさ（候補が出ているくらい）。
    const BOX: Size = Size { w: 300.0, h: 120.0 };
    /// ヒントの大きさ。
    const HINT: Size = Size { w: 280.0, h: 50.0 };

    fn pt(x: f32, y: f32) -> Point {
        Point { x, y }
    }

    /// 動的入力と同じ置き方で枠を置き、その矩形を返す。
    fn box_at(cursor: Point, size: Size) -> Bounds {
        rect_at(dynamic::place(cursor, size, CANVAS), size)
    }

    fn assert_clear(hint: Bounds, input_box: Bounds, cursor: Point, what: &str) {
        assert!(contains(CANVAS, hint), "{what}: キャンバスの内側 {hint:?}");
        assert!(!overlaps(hint, input_box), "{what}: 枠に重ならない");
        let zone = Bounds {
            left: cursor.x - CURSOR_OFFSET_PX,
            top: cursor.y - CURSOR_OFFSET_PX,
            right: cursor.x + CURSOR_OFFSET_PX,
            bottom: cursor.y + CURSOR_OFFSET_PX,
        };
        assert!(!overlaps(hint, zone), "{what}: カーソルの周りに重ならない");
    }

    /// ふだん（枠がカーソルの右下）は枠の下に、左端をそろえて出す。
    #[test]
    fn below_the_box_normally() {
        let cursor = pt(300.0, 200.0);
        let b = box_at(cursor, BOX);
        let p = place_hint(b, cursor, HINT, CANVAS).expect("入る");
        assert_eq!(p, pt(b.left, b.bottom + GAP_PX));
    }

    /// 下端寄りで枠がカーソルの上へ回り込んだら、ヒントは枠の上（カーソルから遠い側）。
    #[test]
    fn above_the_box_when_the_box_wrapped_upward() {
        let cursor = pt(300.0, 650.0);
        let b = box_at(cursor, BOX);
        assert!(b.bottom < cursor.y, "前提: 枠はカーソルの上");
        let p = place_hint(b, cursor, HINT, CANVAS).expect("入る");
        assert_eq!(p, pt(b.left, b.top - GAP_PX - HINT.h));
    }

    /// 枠はカーソルの下に入るがヒントまでは入らない高さでは、枠の右へ回す。
    #[test]
    fn beside_the_box_when_below_does_not_fit() {
        // 枠の下端が 700 - 20 くらいになる位置。
        let cursor = pt(300.0, 700.0 - BOX.h - CURSOR_OFFSET_PX - 10.0);
        let b = box_at(cursor, BOX);
        assert!(b.top > cursor.y, "前提: 枠はカーソルの下");
        let p = place_hint(b, cursor, HINT, CANVAS).expect("入る");
        assert_eq!(p.x, b.right + GAP_PX, "枠の右");
        assert_eq!(p.y, b.top, "上端は枠にそろえる");
    }

    /// 右端寄りで枠がカーソルの左へ回り込み、下にも入らないなら、枠の左へ回す。
    #[test]
    fn left_of_the_box_when_the_box_wrapped_left() {
        let cursor = pt(950.0, 700.0 - BOX.h - CURSOR_OFFSET_PX - 10.0);
        let b = box_at(cursor, BOX);
        assert!(b.right < cursor.x, "前提: 枠はカーソルの左");
        let p = place_hint(b, cursor, HINT, CANVAS).expect("入る");
        assert_eq!(p.x, b.left - GAP_PX - HINT.w, "枠の左");
    }

    /// 枠の下に置くとき、右へはみ出すぶんは左へ寄せる（カーソルからは縦に離れている）。
    #[test]
    fn shifted_left_to_stay_inside_below_the_box() {
        let wide = Size { w: 600.0, h: 50.0 };
        let cursor = pt(500.0, 200.0);
        let b = box_at(cursor, BOX);
        let p = place_hint(b, cursor, wide, CANVAS).expect("入る");
        assert_eq!(p.x + wide.w, CANVAS.right, "右端に寄せる");
        assert_eq!(p.y, b.bottom + GAP_PX);
    }

    /// ちょうど入る位置は入るとみなす（境界の扱い）。
    #[test]
    fn exactly_fitting_below_is_accepted() {
        let b = Bounds {
            left: 100.0,
            top: 200.0,
            right: 400.0,
            bottom: CANVAS.bottom - GAP_PX - HINT.h,
        };
        let p = place_hint(b, pt(80.0, 180.0), HINT, CANVAS).expect("入る");
        assert_eq!(p.y + HINT.h, CANVAS.bottom);
    }

    /// どこにも入らなければ出さない（領域より大きい・隅で両方ふさがる）。
    #[test]
    fn nothing_when_it_fits_nowhere() {
        let huge = Size {
            w: 2000.0,
            h: 2000.0,
        };
        let cursor = pt(500.0, 300.0);
        assert_eq!(place_hint(box_at(cursor, BOX), cursor, huge, CANVAS), None);

        // キャンバスいっぱいに近い枠。上下にも左右にも余地が無い。
        let big_box = Bounds {
            left: 10.0,
            top: 40.0,
            right: 990.0,
            bottom: 690.0,
        };
        assert_eq!(place_hint(big_box, pt(5.0, 35.0), HINT, CANVAS), None);
    }

    /// キャンバスのどこにカーソルがあっても、出すなら必ず内側で、枠にもカーソルにも重ならない。
    /// 枠・ヒントの大きさも何通りか変える。
    #[test]
    fn never_overlaps_the_box_or_the_cursor_anywhere() {
        let boxes = [BOX, Size { w: 260.0, h: 40.0 }, Size { w: 480.0, h: 260.0 }];
        let hints = [HINT, Size { w: 420.0, h: 80.0 }, Size { w: 120.0, h: 30.0 }];
        let mut shown = 0;
        let mut y = CANVAS.top;
        while y <= CANVAS.bottom {
            let mut x = CANVAS.left;
            while x <= CANVAS.right {
                let cursor = pt(x, y);
                for bs in boxes {
                    let b = box_at(cursor, bs);
                    for hs in hints {
                        if let Some(p) = place_hint(b, cursor, hs, CANVAS) {
                            shown += 1;
                            assert_clear(rect_at(p, hs), b, cursor, &format!("{x},{y}"));
                        }
                    }
                }
                x += 20.0;
            }
            y += 20.0;
        }
        assert!(shown > 0, "前提: どこかでは出ている");
    }

    #[test]
    fn hidden_unless_everything_allows_it() {
        let ok = Situation {
            enabled: true,
            dynamic: true,
            box_visible: true,
            composing: false,
            modal_open: false,
        };
        assert!(allowed(ok));
        for (s, what) in [
            (
                Situation {
                    enabled: false,
                    ..ok
                },
                "HINT オフ",
            ),
            (
                Situation {
                    dynamic: false,
                    ..ok
                },
                "動的入力オフ",
            ),
            (
                Situation {
                    box_visible: false,
                    ..ok
                },
                "何もしていない",
            ),
            (
                Situation {
                    composing: true,
                    ..ok
                },
                "変換中",
            ),
            (
                Situation {
                    modal_open: true,
                    ..ok
                },
                "モーダル",
            ),
        ] {
            assert!(!allowed(s), "{what}");
        }
    }

    #[test]
    fn command_hint_carries_the_name_and_description() {
        assert_eq!(
            for_command("LINE"),
            Some(Hint::Command {
                name: "LINE",
                description: command_help::description("LINE").unwrap_or_default(),
            })
        );
        assert_eq!(for_command("NOSUCH"), None);
    }
}
