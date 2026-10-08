//! プロパティパネルの項目と、コンポーネントの束縛（式）の対応（Issue #31 段階 3）。
//!
//! egui に依存しない。インプレース編集中（ADR-0033）に選んだ中身について、「どの項目の値が
//! 定義のどの束縛（[`Slot`]）で決まるか」を決める。束縛で決まる項目は表示だけにし、横に式を出す。
//! パネルで変えても、`ENDCOMP` で書き戻した後は式の値で上書きされ、変えた値が黙って消えるため。
//! 束縛の無い項目は編集中でも変えられ、`ENDCOMP` の後も定義に残る。
//!
//! # 対応表は 1 か所（[`source`]）
//!
//! 項目（[`Key`]）から、値が決まる元のスロットを引く表は [`source`] だけ。表がすべての項目を
//! 網羅していること、束縛できるスロットがどれかの項目（ポリラインの頂点は [`EntityBindings::vertex_rows`]）
//! に必ず現れることは単体テストで固定している。
//!
//! # ほかの値から求める項目（線分の長さ・角度）
//!
//! 長さ・角度は両端点から決まる。変えると端点が動くので、**元のどれかが束縛されていれば表示だけ**
//! にする（[`Source::Derived`]。規則はこの 1 か所）。
//!
//! # 図面の軸と定義の軸
//!
//! 編集中の中身は、入口のインスタンスの配置で図面へ置かれている（`component::place`）。パネルの
//! 値は図面の座標で、束縛は定義の座標を指すので、配置の回転で対応が変わる（[`Frame`]）。
//!
//! - 回転が 0°・180° … 図面の X ↔ 定義の X、Y ↔ Y
//! - 回転が 90°・270° … 図面の X ↔ 定義の Y、Y ↔ X
//! - それ以外 … 図面の X も Y も、定義の X と Y の両方から決まる。どちらかが束縛されていれば
//!   両方とも表示だけ（変えると、束縛された側の定義の値も動く）
//! - 反転した配置では、円弧の開始角と終了角が入れ替わる（鏡像で向きが逆になる。ADR-0020）
//!
//! 回転の判定は `eq_angle` で、ぴったりでなければ「それ以外」になる。迷ったら表示だけにする側へ
//! 倒れる（束縛された値を変えさせてしまうより、変えられない方がまし）。

use std::f64::consts::{FRAC_PI_2, PI};

use cad_core::component::{Binding, Placement, Slot};
use cad_core::geom::tolerance::eq_angle;
use cad_core::Geometry;

use crate::properties::{fmt_num, Kind};
use crate::properties_edit::{Field, Toggle};
use crate::tools::param::slot_input_name;

/// 編集できる項目（数値か、はい・いいえか）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    /// 数値の項目。
    Number(Field),
    /// はい・いいえの項目。
    Toggle(Toggle),
}

/// 点の X か Y か。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    /// X。
    X,
    /// Y。
    Y,
}

impl Axis {
    fn label(self) -> &'static str {
        match self {
            Self::X => "X",
            Self::Y => "Y",
        }
    }
}

/// 項目の値が、定義のどのスロットから決まるか（配置を考える前の、定義の座標での対応）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// 点の X か Y。図面の軸と定義の軸の対応は配置の回転で決まる（[`Frame`]）。
    Coord {
        /// 点の X のスロット。
        x: Slot,
        /// 点の Y のスロット。
        y: Slot,
        /// この項目が図面の X か Y か。
        axis: Axis,
    },
    /// 1 つのスロットだけで決まる（半径・倍率・作図線の角度・インスタンスの回転）。
    Scalar(Slot),
    /// 円弧の開始角・終了角。反転した配置では `other` の側で決まる。
    ArcAngle {
        /// 反転していないときのスロット。
        own: Slot,
        /// 反転しているときのスロット（もう片方の角）。
        other: Slot,
    },
    /// ほかの値から求める項目（線分の長さ・角度）。`from` のどれかが束縛されていれば表示だけ。
    Derived {
        /// 元の値のスロット。
        from: &'static [Slot],
        /// 元の値の呼び名（案内に出す）。
        what: &'static str,
    },
    /// 束縛できる値が無い（ポリラインの閉じ、インスタンスの反転）。いつでも編集できる。
    Unbindable,
}

/// 線分の両端点。長さ・角度はここから決まる。
const LINE_ENDS: &[Slot] = &[Slot::LineAx, Slot::LineAy, Slot::LineBx, Slot::LineBy];

/// **項目とスロットの対応表。** 種類 `kind` にその項目が無ければ `None`。
///
/// 項目を足したら、ここにも足すこと（`every_item_has_a_source` が落ちて知らせる）。
#[must_use]
pub fn source(kind: Kind, key: Key) -> Option<Source> {
    use Field as F;
    use Key::{Number as N, Toggle as T};
    let coord = |x, y, axis| Source::Coord { x, y, axis };
    Some(match (kind, key) {
        (Kind::Line, N(F::LineStartX)) => coord(Slot::LineAx, Slot::LineAy, Axis::X),
        (Kind::Line, N(F::LineStartY)) => coord(Slot::LineAx, Slot::LineAy, Axis::Y),
        (Kind::Line, N(F::LineEndX)) => coord(Slot::LineBx, Slot::LineBy, Axis::X),
        (Kind::Line, N(F::LineEndY)) => coord(Slot::LineBx, Slot::LineBy, Axis::Y),
        (Kind::Line, N(F::LineLength | F::LineAngle)) => Source::Derived {
            from: LINE_ENDS,
            what: "端点",
        },

        (Kind::Circle, N(F::CenterX)) => coord(Slot::CircleCx, Slot::CircleCy, Axis::X),
        (Kind::Circle, N(F::CenterY)) => coord(Slot::CircleCx, Slot::CircleCy, Axis::Y),
        (Kind::Circle, N(F::Radius)) => Source::Scalar(Slot::CircleR),

        (Kind::Arc, N(F::CenterX)) => coord(Slot::ArcCx, Slot::ArcCy, Axis::X),
        (Kind::Arc, N(F::CenterY)) => coord(Slot::ArcCx, Slot::ArcCy, Axis::Y),
        (Kind::Arc, N(F::Radius)) => Source::Scalar(Slot::ArcR),
        (Kind::Arc, N(F::ArcStart)) => Source::ArcAngle {
            own: Slot::ArcStart,
            other: Slot::ArcEnd,
        },
        (Kind::Arc, N(F::ArcEnd)) => Source::ArcAngle {
            own: Slot::ArcEnd,
            other: Slot::ArcStart,
        },

        // 頂点はパネルの項目に無い（数も変えない。ADR-0040）。束縛された頂点は
        // `EntityBindings::vertex_rows` が表示だけの行として出す。
        (Kind::Polyline, T(Toggle::Closed)) => Source::Unbindable,

        (Kind::Xline, N(F::XlineX)) => coord(Slot::XlineOx, Slot::XlineOy, Axis::X),
        (Kind::Xline, N(F::XlineY)) => coord(Slot::XlineOx, Slot::XlineOy, Axis::Y),
        (Kind::Xline, N(F::XlineAngle)) => Source::Scalar(Slot::XlineAngle),

        (Kind::Instance, N(F::BaseX)) => coord(Slot::InstanceX, Slot::InstanceY, Axis::X),
        (Kind::Instance, N(F::BaseY)) => coord(Slot::InstanceX, Slot::InstanceY, Axis::Y),
        (Kind::Instance, N(F::Rotation)) => Source::Scalar(Slot::InstanceRotation),
        (Kind::Instance, N(F::Scale)) => Source::Scalar(Slot::InstanceScale),
        (Kind::Instance, T(Toggle::Flipped)) => Source::Unbindable,

        _ => return None,
    })
}

/// 図面の軸と定義の軸の対応。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axes {
    /// X ↔ X、Y ↔ Y（回転 0°・180°）。
    Same,
    /// X ↔ Y、Y ↔ X（回転 90°・270°）。
    Swapped,
    /// 図面の X・Y がどちらも定義の X と Y から決まる（それ以外の回転）。
    Mixed,
}

/// 編集の入口になったインスタンスの配置のうち、項目とスロットの対応を変えるもの。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    /// 軸の対応。
    pub axes: Axes,
    /// 反転しているか（円弧の開始角・終了角が入れ替わる）。
    pub flipped: bool,
}

impl Frame {
    /// 恒等の配置（無回転・非反転）。
    #[cfg(test)]
    pub const IDENTITY: Self = Self {
        axes: Axes::Same,
        flipped: false,
    };

    /// 配置から決める。倍率と基点は対応を変えない（一様倍率・平行移動なので）。
    #[must_use]
    pub fn of(p: Placement) -> Self {
        let r = p.rotation;
        let axes = if eq_angle(r, 0.0) || eq_angle(r, PI) {
            Axes::Same
        } else if eq_angle(r, FRAC_PI_2) || eq_angle(r, -FRAC_PI_2) {
            Axes::Swapped
        } else {
            Axes::Mixed
        };
        Self {
            axes,
            flipped: p.flipped,
        }
    }
}

impl Source {
    /// 配置 `frame` のもとで、この項目の値が決まる定義のスロット。
    #[must_use]
    pub fn slots(self, frame: Frame) -> Vec<Slot> {
        match self {
            Self::Coord { x, y, axis } => match (frame.axes, axis) {
                (Axes::Same, Axis::X) | (Axes::Swapped, Axis::Y) => vec![x],
                (Axes::Same, Axis::Y) | (Axes::Swapped, Axis::X) => vec![y],
                (Axes::Mixed, _) => vec![x, y],
            },
            Self::Scalar(s) => vec![s],
            Self::ArcAngle { own, other } => vec![if frame.flipped { other } else { own }],
            Self::Derived { from, .. } => from.to_vec(),
            Self::Unbindable => Vec::new(),
        }
    }
}

/// 式の案内で、式を省略せずに出す最大の文字数。超えたら省略してツールチップで全体を出す。
pub const EXPR_MAX_CHARS: usize = 12;

/// 束縛（式）で決まるため、表示だけにする項目の理由。
#[derive(Clone, Debug, PartialEq)]
pub enum Lock {
    /// 値そのものが式で決まる。（スロット, 式）の組。
    Bound(Vec<(Slot, String)>),
    /// 元の値のどれかが式で決まる（線分の長さ・角度）。
    Derived {
        /// 元の値の呼び名（`端点`）。
        what: &'static str,
        /// 束縛されている元の値の（スロット, 式）。
        exprs: Vec<(Slot, String)>,
    },
}

/// 長い式を `EXPR_MAX_CHARS` 文字で省略する（文字数で数える。日本語の名前でも途中で切れない）。
#[must_use]
pub fn shorten(expr: &str) -> String {
    if expr.chars().count() <= EXPR_MAX_CHARS {
        return expr.to_owned();
    }
    let head: String = expr.chars().take(EXPR_MAX_CHARS - 1).collect();
    format!("{}…", head.trim_end())
}

impl Lock {
    fn exprs(&self) -> &[(Slot, String)] {
        match self {
            Self::Bound(e) | Self::Derived { exprs: e, .. } => e,
        }
    }

    /// 値の横に出す短い案内（`← 式「幅」`、`← 端点の式から`）。[`Lock::badge_parts`] をつないだもの。
    #[must_use]
    pub fn badge(&self) -> String {
        let (head, rest) = self.badge_parts();
        format!("{head}{rest}")
    }

    /// 短い案内を「必ず見せる頭」と「入り切らなければ省略してよい残り」に分けたもの。
    ///
    /// パネルは案内を折り返さずに 1 行で出し、幅が足りなければ残り（式の側）だけを省略する。
    /// 頭（`← 式`）まで削ると、何の印か分からなくなる。式の全体はツールチップにある。
    #[must_use]
    pub fn badge_parts(&self) -> (&'static str, String) {
        match self {
            Self::Bound(exprs) => {
                let quoted: String = exprs
                    .iter()
                    .map(|(_, e)| format!("「{}」", shorten(e)))
                    .collect();
                ("← 式", quoted)
            }
            Self::Derived { what, .. } => ("← ", format!("{what}の式から")),
        }
    }

    /// ツールチップ。式を省略せずに出し、表示だけにしている理由を言う。
    #[must_use]
    pub fn tooltip(&self) -> String {
        let head = match self {
            Self::Bound(_) => "コンポーネントの式で決まる値です。".to_owned(),
            Self::Derived { what, .. } => {
                format!("{what}がコンポーネントの式で決まるため、この値も変えられません。")
            }
        };
        let lines: Vec<String> = self
            .exprs()
            .iter()
            .map(|(slot, e)| format!("  {}{} = {e}", slot_input_name(*slot), ordinal(*slot)))
            .collect();
        format!(
            "{head}\n{}\n（スロット名は定義の座標）\nここで変えても ENDCOMP の後に式の値へ戻るため、表示だけにしています。\n式は BIND（BI）で変えます",
            lines.join("\n")
        )
    }
}

/// ポリラインの頂点のスロットなら、数え方の注記（`頂点Y2` は 0 始まりなので `（3 つ目の頂点）`）。
fn ordinal(slot: Slot) -> String {
    match slot {
        Slot::PolylineVx(i) | Slot::PolylineVy(i) => {
            format!("（{} つ目の頂点）", u64::from(i) + 1)
        }
        _ => String::new(),
    }
}

/// 束縛された頂点の、表示だけの行（ポリラインの頂点はパネルの項目に無いので、別に出す）。
#[derive(Clone, Debug, PartialEq)]
pub struct VertexRow {
    /// 項目名（`頂点 1 X`。番号は BIND と同じ 0 始まり）。
    pub label: String,
    /// 表示用に整えた値（図面の座標）。
    pub value: String,
    /// 式。
    pub lock: Lock,
}

/// インプレース編集中に選んだ中身 1 つの束縛。
#[derive(Clone, Debug, PartialEq)]
pub struct EntityBindings {
    frame: Frame,
    /// （スロット, 式）。定義の束縛のうち、この中身を指すもの。
    exprs: Vec<(Slot, String)>,
}

impl EntityBindings {
    /// 編集の入口の配置 `placement` と、この中身を指す束縛から作る。
    #[must_use]
    pub fn new<'a>(placement: Placement, bindings: impl IntoIterator<Item = &'a Binding>) -> Self {
        Self {
            frame: Frame::of(placement),
            exprs: bindings
                .into_iter()
                .map(|b| (b.slot, b.expr.to_string()))
                .collect(),
        }
    }

    /// 束縛が 1 つも無いか。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.exprs.is_empty()
    }

    /// `slots` のうち束縛されているものの（スロット, 式）。
    fn bound_among(&self, slots: &[Slot]) -> Vec<(Slot, String)> {
        slots
            .iter()
            .filter_map(|s| self.exprs.iter().find(|(b, _)| b == s).cloned())
            .collect()
    }

    /// 項目 `key` を表示だけにする理由。束縛で決まらなければ `None`（編集できる）。
    #[must_use]
    pub fn lock(&self, kind: Kind, key: Key) -> Option<Lock> {
        let source = source(kind, key)?;
        let exprs = self.bound_among(&source.slots(self.frame));
        if exprs.is_empty() {
            return None;
        }
        Some(match source {
            Source::Derived { what, .. } => Lock::Derived { what, exprs },
            _ => Lock::Bound(exprs),
        })
    }

    /// ポリラインの、束縛された頂点の表示だけの行。束縛で決まる軸の行だけを出す。
    /// ポリライン以外は空。
    #[must_use]
    pub fn vertex_rows(&self, geom: &Geometry) -> Vec<VertexRow> {
        let Geometry::Polyline(p) = geom else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        for (i, v) in p.vertices.iter().enumerate() {
            let Ok(n) = u32::try_from(i) else {
                break;
            };
            for (axis, value) in [(Axis::X, v.x), (Axis::Y, v.y)] {
                let source = Source::Coord {
                    x: Slot::PolylineVx(n),
                    y: Slot::PolylineVy(n),
                    axis,
                };
                let exprs = self.bound_among(&source.slots(self.frame));
                if !exprs.is_empty() {
                    rows.push(VertexRow {
                        label: format!("頂点 {i} {}", axis.label()),
                        value: fmt_num(value),
                        lock: Lock::Bound(exprs),
                    });
                }
            }
        }
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::properties::{items, kind_of, Editor};
    use cad_core::command::{DefineComponent, InsertInstance};
    use cad_core::component::DefinitionTable;
    use cad_core::expr::parse;
    use cad_core::geom::{Arc, Circle, Line, Point2, Polyline, Xline};
    use cad_core::{Document, Entity, LayerId};

    /// 種類ごとの図形（ポリラインは閉じを編集できる頂点 3 つ）。
    fn geoms() -> Vec<Geometry> {
        vec![
            Geometry::Line(Line::new(Point2::ORIGIN, Point2::new(1.0, 0.0))),
            Geometry::Circle(Circle::new(Point2::ORIGIN, 1.0)),
            Geometry::Arc(Arc::new(Point2::ORIGIN, 1.0, 0.0, 1.0)),
            Geometry::Polyline(Polyline::new(
                vec![Point2::ORIGIN, Point2::new(1.0, 0.0), Point2::new(1.0, 1.0)],
                false,
            )),
            Geometry::Xline(Xline::horizontal(Point2::ORIGIN)),
            instance(),
        ]
    }

    /// インスタンス 1 つ（定義の ID は図面を作って得る）。
    fn instance() -> Geometry {
        let mut doc = Document::new();
        doc.apply(Box::new(DefineComponent::new(
            "COMPONENT",
            "窓",
            Point2::ORIGIN,
            vec![Entity::new(
                Geometry::Line(Line::new(Point2::ORIGIN, Point2::new(1.0, 0.0))),
                LayerId::ZERO,
            )],
        )))
        .expect("定義");
        let def = doc.definitions().by_name("窓").expect("定義がある");
        doc.apply(Box::new(InsertInstance::new(
            "INSERT",
            def,
            Placement::at(Point2::ORIGIN),
            LayerId::ZERO,
        )))
        .expect("配置");
        let id = doc.entities().ids().last().expect("インスタンス");
        doc.entities().get(id).expect("ある").geom.clone()
    }

    /// 項目の一覧のうち、編集できる項目のキー。
    fn keys(geom: &Geometry) -> Vec<Key> {
        items(geom, &DefinitionTable::new())
            .into_iter()
            .filter_map(|i| match i.editor? {
                Editor::Number(f, _) => Some(Key::Number(f)),
                Editor::Toggle(t, _) => Some(Key::Toggle(t)),
            })
            .collect()
    }

    /// すべての項目（新しい項目を足したら、`match` が網羅していないとコンパイルが止まる）。
    fn all_keys() -> Vec<Key> {
        use Field as F;
        let fields = [
            F::LineStartX,
            F::LineStartY,
            F::LineEndX,
            F::LineEndY,
            F::LineLength,
            F::LineAngle,
            F::CenterX,
            F::CenterY,
            F::Radius,
            F::ArcStart,
            F::ArcEnd,
            F::XlineX,
            F::XlineY,
            F::XlineAngle,
            F::BaseX,
            F::BaseY,
            F::Rotation,
            F::Scale,
        ];
        // 列挙の漏れを型で検出する（変種を足すとここで網羅されずに落ちる）。
        for f in fields {
            match f {
                F::LineStartX
                | F::LineStartY
                | F::LineEndX
                | F::LineEndY
                | F::LineLength
                | F::LineAngle
                | F::CenterX
                | F::CenterY
                | F::Radius
                | F::ArcStart
                | F::ArcEnd
                | F::XlineX
                | F::XlineY
                | F::XlineAngle
                | F::BaseX
                | F::BaseY
                | F::Rotation
                | F::Scale => {}
            }
        }
        let toggles = [Toggle::Closed, Toggle::Flipped];
        for t in toggles {
            match t {
                Toggle::Closed | Toggle::Flipped => {}
            }
        }
        fields
            .into_iter()
            .map(Key::Number)
            .chain(toggles.into_iter().map(Key::Toggle))
            .collect()
    }

    /// 束縛できるスロットの全部（ポリラインの頂点は 0 番で代表する）。
    fn all_slots() -> Vec<Slot> {
        let slots = vec![
            Slot::LineAx,
            Slot::LineAy,
            Slot::LineBx,
            Slot::LineBy,
            Slot::CircleCx,
            Slot::CircleCy,
            Slot::CircleR,
            Slot::ArcCx,
            Slot::ArcCy,
            Slot::ArcR,
            Slot::ArcStart,
            Slot::ArcEnd,
            Slot::XlineOx,
            Slot::XlineOy,
            Slot::XlineAngle,
            Slot::PolylineVx(0),
            Slot::PolylineVy(0),
            Slot::InstanceX,
            Slot::InstanceY,
            Slot::InstanceRotation,
            Slot::InstanceScale,
        ];
        for s in &slots {
            match s {
                Slot::LineAx
                | Slot::LineAy
                | Slot::LineBx
                | Slot::LineBy
                | Slot::CircleCx
                | Slot::CircleCy
                | Slot::CircleR
                | Slot::ArcCx
                | Slot::ArcCy
                | Slot::ArcR
                | Slot::ArcStart
                | Slot::ArcEnd
                | Slot::XlineOx
                | Slot::XlineOy
                | Slot::XlineAngle
                | Slot::PolylineVx(_)
                | Slot::PolylineVy(_)
                | Slot::InstanceX
                | Slot::InstanceY
                | Slot::InstanceRotation
                | Slot::InstanceScale => {}
            }
        }
        slots
    }

    fn bindings(slots: &[(Slot, &str)]) -> Vec<Binding> {
        slots
            .iter()
            .map(|(s, e)| Binding::new(0, *s, parse(e).expect("解析")))
            .collect()
    }

    fn with(placement: Placement, slots: &[(Slot, &str)]) -> EntityBindings {
        EntityBindings::new(placement, &bindings(slots))
    }

    fn rotated(deg: f64, flipped: bool) -> Placement {
        Placement::new(Point2::new(5.0, 7.0), deg.to_radians(), 2.0, flipped).expect("配置")
    }

    // ---- 対応表の網羅 ----------------------------------------------------------

    /// **パネルの編集できる項目は、すべて対応表にある。** 表に無い項目があると、束縛されていても
    /// 編集できてしまい、ENDCOMP の後に変えた値が黙って消える。
    #[test]
    fn every_item_has_a_source() {
        for g in geoms() {
            for key in keys(&g) {
                assert!(
                    source(kind_of(&g), key).is_some(),
                    "{:?} の {key:?} が対応表に無い",
                    kind_of(&g)
                );
            }
        }
    }

    /// 対応表には、パネルに出ない項目が無い（種類を取り違えた行が無い）。
    #[test]
    fn the_table_has_no_items_the_panel_does_not_show() {
        for g in geoms() {
            let shown = keys(&g);
            for key in all_keys() {
                assert_eq!(
                    source(kind_of(&g), key).is_some(),
                    shown.contains(&key),
                    "{:?} の {key:?}",
                    kind_of(&g)
                );
            }
        }
    }

    /// **束縛できるスロットは、どれかの項目（ポリラインの頂点は頂点の行）に必ず現れる。**
    /// 現れないスロットがあると、そのスロットの束縛はパネルのどこにも出ない。
    #[test]
    fn every_slot_is_shown_by_some_item() {
        for slot in all_slots() {
            let g = geoms()
                .into_iter()
                .find(|g| slot.fits(g))
                .unwrap_or_else(|| panic!("{slot:?} が合う図形が無い"));
            let in_items = keys(&g).into_iter().any(|key| {
                source(kind_of(&g), key).is_some_and(|s| {
                    !matches!(s, Source::Derived { .. }) && s.slots(Frame::IDENTITY).contains(&slot)
                })
            });
            let in_vertex_rows = !with(Placement::at(Point2::ORIGIN), &[(slot, "幅")])
                .vertex_rows(&g)
                .is_empty();
            assert!(
                in_items || in_vertex_rows,
                "{slot:?} を出す項目が無い（{:?}）",
                kind_of(&g)
            );
        }
    }

    /// 回転しない配置では、項目と同じ名前のスロットだけが効く（線分の始点 X ↔ 始点X 等）。
    #[test]
    fn identity_frame_maps_each_item_to_its_own_slot() {
        use Field as F;
        let cases = [
            (Kind::Line, F::LineStartX, Slot::LineAx),
            (Kind::Line, F::LineStartY, Slot::LineAy),
            (Kind::Line, F::LineEndX, Slot::LineBx),
            (Kind::Line, F::LineEndY, Slot::LineBy),
            (Kind::Circle, F::CenterX, Slot::CircleCx),
            (Kind::Circle, F::CenterY, Slot::CircleCy),
            (Kind::Circle, F::Radius, Slot::CircleR),
            (Kind::Arc, F::CenterX, Slot::ArcCx),
            (Kind::Arc, F::CenterY, Slot::ArcCy),
            (Kind::Arc, F::Radius, Slot::ArcR),
            (Kind::Arc, F::ArcStart, Slot::ArcStart),
            (Kind::Arc, F::ArcEnd, Slot::ArcEnd),
            (Kind::Xline, F::XlineX, Slot::XlineOx),
            (Kind::Xline, F::XlineY, Slot::XlineOy),
            (Kind::Xline, F::XlineAngle, Slot::XlineAngle),
            (Kind::Instance, F::BaseX, Slot::InstanceX),
            (Kind::Instance, F::BaseY, Slot::InstanceY),
            (Kind::Instance, F::Rotation, Slot::InstanceRotation),
            (Kind::Instance, F::Scale, Slot::InstanceScale),
        ];
        for (kind, field, slot) in cases {
            let s = source(kind, Key::Number(field)).expect("表にある");
            assert_eq!(s.slots(Frame::IDENTITY), vec![slot], "{kind:?} {field:?}");
        }
    }

    // ---- ほかの値から求める項目 -------------------------------------------------

    /// **線分の端点のどれかが束縛されていれば、長さ・角度も表示だけ。**
    #[test]
    fn line_length_and_angle_lock_when_any_endpoint_is_bound() {
        let identity = Placement::at(Point2::ORIGIN);
        for end in [Slot::LineAx, Slot::LineAy, Slot::LineBx, Slot::LineBy] {
            let b = with(identity, &[(end, "幅")]);
            for field in [Field::LineLength, Field::LineAngle] {
                assert_eq!(
                    b.lock(Kind::Line, Key::Number(field)),
                    Some(Lock::Derived {
                        what: "端点",
                        exprs: vec![(end, "幅".to_owned())]
                    }),
                    "{end:?} → {field:?}"
                );
            }
        }
        let free = with(identity, &[]);
        assert_eq!(free.lock(Kind::Line, Key::Number(Field::LineLength)), None);
    }

    /// 派生の項目は対応表の `Derived` だけ（線分の長さ・角度）。ほかの項目は自分のスロットだけで決まる。
    #[test]
    fn only_line_length_and_angle_are_derived() {
        for g in geoms() {
            for key in keys(&g) {
                let derived = matches!(source(kind_of(&g), key), Some(Source::Derived { .. }));
                let expected = matches!(key, Key::Number(Field::LineLength | Field::LineAngle));
                assert_eq!(derived, expected, "{:?} {key:?}", kind_of(&g));
            }
        }
    }

    // ---- 束縛の有無ごとの判定 ---------------------------------------------------

    /// 束縛のあるスロットの項目だけが表示だけ。ほかの項目は編集できる。
    #[test]
    fn only_items_of_bound_slots_are_locked() {
        let b = with(Placement::at(Point2::ORIGIN), &[(Slot::LineBx, "幅 * 2")]);
        assert_eq!(
            b.lock(Kind::Line, Key::Number(Field::LineEndX)),
            Some(Lock::Bound(vec![(Slot::LineBx, "幅 * 2".to_owned())]))
        );
        for field in [Field::LineStartX, Field::LineStartY, Field::LineEndY] {
            assert_eq!(b.lock(Kind::Line, Key::Number(field)), None, "{field:?}");
        }
    }

    /// 束縛できない項目（閉じ・反転）は、ほかのスロットが束縛されていても編集できる。
    #[test]
    fn unbindable_items_stay_editable() {
        let inst = with(
            Placement::at(Point2::ORIGIN),
            &[
                (Slot::InstanceX, "幅"),
                (Slot::InstanceRotation, "角"),
                (Slot::InstanceScale, "倍"),
            ],
        );
        assert_eq!(
            inst.lock(Kind::Instance, Key::Toggle(Toggle::Flipped)),
            None
        );
        let pl = with(
            Placement::at(Point2::ORIGIN),
            &[(Slot::PolylineVx(0), "幅")],
        );
        assert_eq!(pl.lock(Kind::Polyline, Key::Toggle(Toggle::Closed)), None);
    }

    /// 束縛が無ければ何も表示だけにしない。
    #[test]
    fn nothing_is_locked_without_bindings() {
        let b = with(rotated(30.0, true), &[]);
        assert!(b.is_empty());
        for g in geoms() {
            for key in keys(&g) {
                assert_eq!(b.lock(kind_of(&g), key), None);
            }
            assert!(b.vertex_rows(&g).is_empty());
        }
    }

    // ---- 配置による対応の変化 ---------------------------------------------------

    #[test]
    fn frame_classifies_the_rotation() {
        for (deg, axes) in [
            (0.0, Axes::Same),
            (180.0, Axes::Same),
            (-180.0, Axes::Same),
            (360.0, Axes::Same),
            (90.0, Axes::Swapped),
            (270.0, Axes::Swapped),
            (-90.0, Axes::Swapped),
            (30.0, Axes::Mixed),
            (45.0, Axes::Mixed),
            (89.0, Axes::Mixed),
        ] {
            assert_eq!(Frame::of(rotated(deg, false)).axes, axes, "{deg}°");
        }
        assert!(Frame::of(rotated(0.0, true)).flipped);
    }

    /// 90° 回した配置では、図面の X が定義の Y で決まる。
    #[test]
    fn a_quarter_turn_swaps_x_and_y() {
        let b = with(rotated(90.0, false), &[(Slot::LineAy, "高さ")]);
        assert!(b.lock(Kind::Line, Key::Number(Field::LineStartX)).is_some());
        assert_eq!(b.lock(Kind::Line, Key::Number(Field::LineStartY)), None);
    }

    /// 斜めの配置では、図面の X・Y がどちらも定義の X と Y から決まるので、両方とも表示だけ。
    #[test]
    fn an_oblique_turn_locks_both_axes() {
        let b = with(rotated(30.0, false), &[(Slot::CircleCx, "幅")]);
        for field in [Field::CenterX, Field::CenterY] {
            assert_eq!(
                b.lock(Kind::Circle, Key::Number(field)),
                Some(Lock::Bound(vec![(Slot::CircleCx, "幅".to_owned())])),
                "{field:?}"
            );
        }
        // 半径は回転と関係ない。
        assert_eq!(b.lock(Kind::Circle, Key::Number(Field::Radius)), None);
    }

    /// 反転した配置では、円弧の開始角と終了角が入れ替わる（鏡像で向きが逆になる）。
    #[test]
    fn a_flipped_frame_swaps_the_arc_angles() {
        let b = with(rotated(0.0, true), &[(Slot::ArcStart, "角")]);
        assert_eq!(b.lock(Kind::Arc, Key::Number(Field::ArcStart)), None);
        assert!(b.lock(Kind::Arc, Key::Number(Field::ArcEnd)).is_some());

        let b = with(rotated(0.0, false), &[(Slot::ArcStart, "角")]);
        assert!(b.lock(Kind::Arc, Key::Number(Field::ArcStart)).is_some());
        assert_eq!(b.lock(Kind::Arc, Key::Number(Field::ArcEnd)), None);
    }

    /// 対応の向きが `component::place` と一致している（定義の値を 1 つ動かして置き直すと、
    /// 動くのは対応表が言う項目だけ）。表と配置の計算がずれていないことを、**すべての種類**の
    /// すべてのスロット × すべての数値の項目で確かめる（ポリラインの頂点は頂点の行で）。
    /// 表に項目を足したときも、ここから漏れない。
    #[test]
    fn the_frame_agrees_with_place() {
        use crate::properties_edit::{display_value, same_on_screen};
        use cad_core::component::place;

        // 種類ごとの、どの値も 10.5 とは違う図形（スロットを 10.5 にすると必ず動く）。
        let Geometry::Instance(inst) = instance() else {
            unreachable!()
        };
        let shapes = [
            Geometry::Line(Line::new(Point2::new(1.0, 2.0), Point2::new(4.0, 6.0))),
            Geometry::Circle(Circle::new(Point2::new(1.0, 2.0), 3.0)),
            Geometry::Arc(Arc::new(Point2::new(1.0, 2.0), 3.0, 0.2, 1.4)),
            Geometry::Xline(Xline::at_angle(Point2::new(1.0, 2.0), 0.3)),
            Geometry::Instance(inst.with_placement(
                Placement::new(Point2::new(1.0, 2.0), 0.3, 1.5, false).expect("配置"),
            )),
            Geometry::Polyline(Polyline::new(
                vec![
                    Point2::new(1.0, 2.0),
                    Point2::new(4.0, 6.0),
                    Point2::new(7.0, 1.0),
                ],
                false,
            )),
        ];
        let moved_to = 10.5;
        for placement in [
            rotated(0.0, false),
            rotated(90.0, false),
            rotated(180.0, true),
            rotated(270.0, true),
            rotated(30.0, false),
            rotated(-45.0, true),
        ] {
            let frame = Frame::of(placement);
            for geom in &shapes {
                let kind = kind_of(geom);
                let slots: Vec<Slot> = all_slots()
                    .into_iter()
                    .map(|s| match s {
                        // 頂点は 1 番（0 番ではなく真ん中）で代表する。
                        Slot::PolylineVx(_) => Slot::PolylineVx(1),
                        Slot::PolylineVy(_) => Slot::PolylineVy(1),
                        other => other,
                    })
                    .filter(|s| s.fits(geom))
                    .collect();
                assert!(!slots.is_empty(), "{kind:?}");
                for slot in slots {
                    let mut moved = geom.clone();
                    assert!(slot.apply(&mut moved, moved_to), "{kind:?} {slot:?}");
                    let before = place(geom, Point2::ORIGIN, placement);
                    let after = place(&moved, Point2::ORIGIN, placement);
                    // 項目の値。
                    for key in keys(geom) {
                        let Key::Number(field) = key else {
                            continue;
                        };
                        let changed = !same_on_screen(
                            display_value(&before, field).expect("値"),
                            display_value(&after, field).expect("値"),
                        );
                        let mapped = source(kind, key)
                            .expect("表にある")
                            .slots(frame)
                            .contains(&slot);
                        assert_eq!(
                            changed, mapped,
                            "{placement:?} で {kind:?} の {slot:?} を動かすと {field:?} が変わる = {changed}"
                        );
                    }
                    // ポリラインの頂点の行（束縛で決まる軸の行だけが出る）。
                    if let (Geometry::Polyline(b), Geometry::Polyline(a)) = (&before, &after) {
                        let rows = EntityBindings::new(
                            placement,
                            &[Binding::new(0, slot, parse("幅").expect("解析"))],
                        )
                        .vertex_rows(&before);
                        for (axis, changed) in [
                            ("X", !same_on_screen(b.vertices[1].x, a.vertices[1].x)),
                            ("Y", !same_on_screen(b.vertices[1].y, a.vertices[1].y)),
                        ] {
                            let shown = rows.iter().any(|r| r.label == format!("頂点 1 {axis}"));
                            assert_eq!(
                                changed, shown,
                                "{placement:?} で {slot:?} を動かすと頂点 1 の {axis} が変わる = {changed}"
                            );
                        }
                    }
                }
            }
        }
    }

    // ---- ポリラインの頂点 -------------------------------------------------------

    /// 束縛された頂点だけ、束縛で決まる軸の行が出る（番号は BIND と同じ 0 始まり）。
    #[test]
    fn bound_vertices_get_display_only_rows() {
        let g = Geometry::Polyline(Polyline::new(
            vec![Point2::ORIGIN, Point2::new(3.0, 0.0), Point2::new(3.0, 4.0)],
            false,
        ));
        let b = with(
            Placement::at(Point2::ORIGIN),
            &[(Slot::PolylineVy(2), "高さ")],
        );
        assert_eq!(
            b.vertex_rows(&g),
            vec![VertexRow {
                label: "頂点 2 Y".to_owned(),
                value: "4.0000".to_owned(),
                lock: Lock::Bound(vec![(Slot::PolylineVy(2), "高さ".to_owned())]),
            }]
        );
        // 番号は 0 始まりなので、ツールチップに数え方を添える。
        let tip = b.vertex_rows(&g)[0].lock.tooltip();
        assert!(tip.contains("頂点Y2（3 つ目の頂点） = 高さ"), "{tip}");
        // 90° 回した配置では、定義の Y は図面の X。
        let b = with(rotated(90.0, false), &[(Slot::PolylineVy(2), "高さ")]);
        let rows = b.vertex_rows(&g);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].label, "頂点 2 X");
        // ポリライン以外には出ない。
        assert!(b.vertex_rows(&geoms()[0]).is_empty());
    }

    // ---- 案内の文 ---------------------------------------------------------------

    #[test]
    fn badges_show_the_expression_or_where_it_comes_from() {
        let bound = Lock::Bound(vec![(Slot::LineBx, "幅".to_owned())]);
        assert_eq!(bound.badge(), "← 式「幅」");
        let two = Lock::Bound(vec![
            (Slot::CircleCx, "幅".to_owned()),
            (Slot::CircleCy, "高さ".to_owned()),
        ]);
        assert_eq!(two.badge(), "← 式「幅」「高さ」");
        let derived = Lock::Derived {
            what: "端点",
            exprs: vec![(Slot::LineBx, "幅".to_owned())],
        };
        assert_eq!(derived.badge(), "← 端点の式から");
        assert!(
            derived.tooltip().contains("終点X = 幅"),
            "{}",
            derived.tooltip()
        );
    }

    /// 長い式は省略し、ツールチップには全体を出す。日本語の名前でも文字の途中で切らない。
    #[test]
    fn long_expressions_are_shortened_with_the_whole_in_the_tooltip() {
        let long = "if 開き戸 then 開口幅 * 2 + 枠厚 else 開口幅";
        let lock = Lock::Bound(vec![(Slot::LineBx, long.to_owned())]);
        let badge = lock.badge();
        assert!(badge.ends_with("…」"), "{badge}");
        assert!(badge.chars().count() < long.chars().count(), "{badge}");
        assert!(lock.tooltip().contains(long), "{}", lock.tooltip());
        assert_eq!(shorten("幅"), "幅");
        let exact: String = "あ".repeat(EXPR_MAX_CHARS);
        assert_eq!(shorten(&exact), exact, "ちょうどの長さは省略しない");
        assert_eq!(
            shorten(&"い".repeat(EXPR_MAX_CHARS + 1)).chars().count(),
            EXPR_MAX_CHARS
        );
    }
}
