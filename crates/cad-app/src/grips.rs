//! グリップ編集の純粋関数（Issue #30 段階 1・2、ADR-0045）。
//!
//! 選んだ図形の端点・中点・中心などに「グリップ」を出し、それを掴んで動かすと形が変わる。
//! ここには egui にも `Session` にも依存しない計算だけを置く。
//!
//! - [`grips_of`] … 図形ごとのグリップの位置と種類（[`Handle`]）
//! - [`apply`] … 「そのグリップの点が `to` へ行く」形の変え方。成り立たない形は [`GripError`] で断る
//! - [`dimension_base`] / [`tracks`] … 長さ・角度（寸法入力・直接距離入力）と直交・極トラッキングの基点
//!   （ユーザー判断 1: **形の基準から**。線分の端点なら反対側の端点）
//! - [`hit`] … カーソルの下の**重なったグリップの束**（[`GripGroup`]、段階 2）。選んだ図形どうしで
//!   同じ位置にあるグリップはまとめて掴み、[`apply_group`] で全部を「その点が `to` へ」動かす
//!   （全部か無しか）。線分 4 本の矩形の角を動かすと、隣り合う 2 本がつながったまま動く
//!
//! # 断るケース
//!
//! 線分の長さ 0・円の半径 0・円弧の 3 点が一直線・**円弧の両端が重なる**（`ReplaceGeometries` は
//! 1 周の円弧として受け付けてしまうので、ここで止める。ADR-0040）。最後に
//! [`Geometry::validate`] を通すので、[`apply`] が `Ok` を返す形は `ReplaceGeometries` でも必ず通る
//! （乱数のテストで固定した）。
//!
//! # 恒等性
//!
//! 元の位置（トレランス内）へ動かしたら、元の形をそのまま返す。円弧の端点を 3 点から作り直すと
//! 浮動小数点の誤差で元とわずかに違う形になり、「動かしていないのに履歴が 1 件増える」ことになるため。

use cad_core::geom::tolerance::eq_angle;
use cad_core::geom::{Arc, Circle, Line, Point2, Polyline, Vec2};
use cad_core::{EntityId, Geometry};

/// 図形の上のどのグリップか。
///
/// 並び順（`Ord`）は同じ図形の中での順で、重なったグリップの選び方を決定的にするのに使う。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Handle {
    /// 線分の始点。
    LineStart,
    /// 線分の終点。
    LineEnd,
    /// 線分の中点（図形ごと移動）。
    LineMid,
    /// 円の中心（図形ごと移動）。
    CircleCenter,
    /// 円の四分点。0: +X、1: +Y、2: −X、3: −Y（半径を変える）。
    CircleQuadrant(u8),
    /// 円弧の始点。
    ArcStart,
    /// 円弧の終点。
    ArcEnd,
    /// 円弧の中点（掃引の真ん中）。
    ArcMid,
    /// 円弧の中心（図形ごと移動）。
    ArcCenter,
    /// ポリラインの頂点（添字）。
    Vertex(usize),
    /// ポリラインの辺の中点（辺の添字。辺 `i` は頂点 `i` → `i + 1`、閉じたポリラインの最後の辺は
    /// 最後の頂点 → 先頭の頂点）。辺を平行移動する（段階 2）。
    Edge(usize),
    /// 作図線の通過点（図形ごと移動）。
    XlineOrigin,
    /// インスタンスの基点（図形ごと移動）。
    InstanceOrigin,
}

impl Handle {
    /// 乗せたときにカーソル横へ出す案内（動かしたら何が起きるか）。
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::LineStart | Self::LineEnd | Self::ArcStart | Self::ArcEnd => "端点を動かす",
            Self::Vertex(_) => "頂点を動かす",
            Self::Edge(_) => "辺を動かす",
            Self::ArcMid => "中点を動かす",
            Self::CircleQuadrant(_) => "半径を変える",
            Self::LineMid
            | Self::CircleCenter
            | Self::ArcCenter
            | Self::XlineOrigin
            | Self::InstanceOrigin => "図形ごと移動",
        }
    }

    /// 図形ごと平行移動するグリップか。
    #[must_use]
    pub fn moves_whole(self) -> bool {
        matches!(
            self,
            Self::LineMid
                | Self::CircleCenter
                | Self::ArcCenter
                | Self::XlineOrigin
                | Self::InstanceOrigin
        )
    }
}

/// 選択中の図形に出ているグリップ 1 つ。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grip {
    /// どの図形か。
    pub id: EntityId,
    /// その図形のどのグリップか。
    pub handle: Handle,
    /// グリップの位置（モデル座標）。
    pub at: Point2,
}

/// グリップを動かした形が成り立たない理由。
#[derive(Clone, Debug, PartialEq)]
pub enum GripError {
    /// その図形にそのグリップは無い（種類が違う・1 周の円弧の端点など）。
    NotAGrip,
    /// 行き先の座標が有限でない。
    NotFinite,
    /// 線分の長さが 0 になる。
    ZeroLength,
    /// 円の半径が 0 になる。
    ZeroRadius,
    /// 円弧を作る 3 点が一直線に並ぶ（同じ点を含む）。
    Collinear,
    /// 円弧の両端が重なる（1 周の円弧になってしまう）。
    ArcEndsMeet,
    /// その他、図形として成り立たない（[`Geometry::validate`] の理由）。
    Invalid(String),
}

impl GripError {
    /// コマンドラインに出す文言。
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::NotAGrip => "このグリップは動かせません".to_owned(),
            Self::NotFinite => "座標が有限ではありません".to_owned(),
            Self::ZeroLength => "線分の長さが 0 になります".to_owned(),
            Self::ZeroRadius => "半径が 0 になります".to_owned(),
            Self::Collinear => "3 点が一直線に並ぶため円弧になりません".to_owned(),
            Self::ArcEndsMeet => "円弧の端点が重なります".to_owned(),
            Self::Invalid(why) => why.clone(),
        }
    }
}

/// 円の四分点の向き（ちょうどの単位ベクトル。`cos(90°)` の誤差を持ち込まない）。
fn quadrant_dir(k: u8) -> Vec2 {
    match k % 4 {
        0 => Vec2::new(1.0, 0.0),
        1 => Vec2::new(0.0, 1.0),
        2 => Vec2::new(-1.0, 0.0),
        _ => Vec2::new(0.0, -1.0),
    }
}

/// 円弧が 1 周か（開始角と終了角が一致する。`Arc::sweep` の約束）。
fn is_full_arc(a: &Arc) -> bool {
    eq_angle(a.start_angle, a.end_angle)
}

/// 図形のグリップの位置と種類。並びは同じ図形の中での優先順。
///
/// | 図形 | グリップ |
/// |---|---|
/// | 線分 | 始点・終点・中点 |
/// | 円 | 中心・四分点 ×4 |
/// | 円弧 | 始点・終点・中点・中心（**1 周の円弧は中心だけ**） |
/// | ポリライン | 頂点・辺の中点（閉じていれば最後の辺も） |
/// | 作図線 | 通過点 |
/// | インスタンス | 基点 |
#[must_use]
pub fn grips_of(geom: &Geometry) -> Vec<(Handle, Point2)> {
    match geom {
        Geometry::Line(l) => vec![
            (Handle::LineStart, l.a),
            (Handle::LineEnd, l.b),
            (Handle::LineMid, l.midpoint()),
        ],
        Geometry::Circle(c) => {
            let mut v = vec![(Handle::CircleCenter, c.center)];
            v.extend((0..4).map(|k| (Handle::CircleQuadrant(k), quadrant_point(c, k))));
            v
        }
        Geometry::Arc(a) => {
            if is_full_arc(a) {
                // 端点と中点は 1 点に重なるか意味を持たない。動かせるのは中心だけ。
                vec![(Handle::ArcCenter, a.center)]
            } else {
                vec![
                    (Handle::ArcStart, a.start_point()),
                    (Handle::ArcEnd, a.end_point()),
                    (Handle::ArcMid, a.mid_point()),
                    (Handle::ArcCenter, a.center),
                ]
            }
        }
        Geometry::Polyline(p) => {
            let mut v: Vec<(Handle, Point2)> = p
                .vertices
                .iter()
                .enumerate()
                .map(|(i, v)| (Handle::Vertex(i), *v))
                .collect();
            // 辺 `i` は頂点 `i` → `i + 1`（閉じていれば最後の辺は最後の頂点 → 先頭の頂点）。
            v.extend(
                p.segments()
                    .enumerate()
                    .map(|(i, s)| (Handle::Edge(i), s.midpoint())),
            );
            v
        }
        Geometry::Xline(x) => vec![(Handle::XlineOrigin, x.origin)],
        Geometry::Instance(i) => vec![(Handle::InstanceOrigin, i.placement.origin)],
    }
}

fn quadrant_point(c: &Circle, k: u8) -> Point2 {
    c.center + quadrant_dir(k) * c.radius
}

/// グリップの位置。その図形にそのグリップが無ければ `None`。
#[must_use]
pub fn position(geom: &Geometry, handle: Handle) -> Option<Point2> {
    grips_of(geom)
        .into_iter()
        .find(|(h, _)| *h == handle)
        .map(|(_, p)| p)
}

/// 長さ・角度（寸法入力・直接距離入力）の基点（ユーザー判断 1: 形の基準から）。
///
/// | グリップ | 基点 |
/// |---|---|
/// | 線分の端点 | 反対側の端点（「F8 で水平」「`100` で長さ 100」がそのままできる） |
/// | 円の四分点 | 中心（長さ = 半径） |
/// | 開いたポリラインの両端の頂点 | 隣の頂点 |
/// | それ以外（辺の中点も） | 元の位置（移動量を測る） |
#[must_use]
pub fn dimension_base(geom: &Geometry, handle: Handle) -> Option<Point2> {
    let here = position(geom, handle)?;
    Some(match (geom, handle) {
        (Geometry::Line(l), Handle::LineStart) => l.b,
        (Geometry::Line(l), Handle::LineEnd) => l.a,
        (Geometry::Circle(c), Handle::CircleQuadrant(_)) => c.center,
        (Geometry::Polyline(p), Handle::Vertex(i)) if !p.closed && p.vertices.len() >= 2 => {
            let last = p.vertices.len() - 1;
            if i == 0 {
                p.vertices[1]
            } else if i == last {
                p.vertices[last - 1]
            } else {
                here
            }
        }
        _ => here,
    })
}

/// 直交モード・極トラッキングを効かせるか。円の四分点だけ外す
/// （半径を決める点で、中心からの向きに意味が無い。水平・垂直に縛ると四分点の向きが変わるだけ）。
#[must_use]
pub fn tracks(handle: Handle) -> bool {
    !matches!(handle, Handle::CircleQuadrant(_))
}

/// グリップ `handle` の点が `to` へ行くように形を変えた結果。
///
/// - 元の位置（トレランス内）なら元の形をそのまま返す（恒等性）
/// - 成り立たない形は `Err`。`Ok` の形は [`Geometry::validate`] を通っている
///
/// # Errors
///
/// [`GripError`] の各場合。
pub fn apply(geom: &Geometry, handle: Handle, to: Point2) -> Result<Geometry, GripError> {
    let from = position(geom, handle).ok_or(GripError::NotAGrip)?;
    if !to.x.is_finite() || !to.y.is_finite() {
        return Err(GripError::NotFinite);
    }
    if to.eq_tol(from) {
        return Ok(geom.clone());
    }

    let moved = match (geom, handle) {
        (Geometry::Line(l), Handle::LineStart) => line(Line::new(to, l.b))?,
        (Geometry::Line(l), Handle::LineEnd) => line(Line::new(l.a, to))?,
        (Geometry::Circle(c), Handle::CircleQuadrant(_)) => {
            let r = c.center.dist(to);
            let circle = Circle::new(c.center, r);
            if circle.is_degenerate() {
                return Err(GripError::ZeroRadius);
            }
            Geometry::Circle(circle)
        }
        // 端点は、もう一方の端点と中点を通る 3 点円弧（ユーザー判断 7）。中点は両端と P を通る。
        (Geometry::Arc(a), Handle::ArcStart) => arc_3(to, a.mid_point(), a.end_point())?,
        (Geometry::Arc(a), Handle::ArcEnd) => arc_3(a.start_point(), a.mid_point(), to)?,
        (Geometry::Arc(a), Handle::ArcMid) => arc_3(a.start_point(), to, a.end_point())?,
        (Geometry::Polyline(p), Handle::Vertex(i)) => {
            let mut vertices = p.vertices.clone();
            vertices[i] = to;
            let moved = Polyline::new(vertices, p.closed);
            if moved.is_degenerate() {
                return Err(GripError::ZeroLength);
            }
            Geometry::Polyline(moved)
        }
        // 辺の両端の頂点を同じだけ動かす（辺の平行移動。頂点の数は変えない。ADR-0040）。
        (Geometry::Polyline(p), Handle::Edge(i)) => {
            let n = p.vertices.len();
            let d = to - from;
            let mut vertices = p.vertices.clone();
            vertices[i] += d;
            vertices[(i + 1) % n] += d;
            let moved = Polyline::new(vertices, p.closed);
            if moved.is_degenerate() {
                return Err(GripError::ZeroLength);
            }
            Geometry::Polyline(moved)
        }
        (_, h) if h.moves_whole() => geom.translated(to - from),
        _ => return Err(GripError::NotAGrip),
    };

    // 最後の関門。ここを通った形は `ReplaceGeometries` でも通る。
    moved
        .validate()
        .map_err(|e| GripError::Invalid(e.to_string()))?;
    Ok(moved)
}

fn line(l: Line) -> Result<Geometry, GripError> {
    if l.is_degenerate() {
        return Err(GripError::ZeroLength);
    }
    Ok(Geometry::Line(l))
}

/// `a` → `mid` → `c` を通る 3 点円弧。両端が重なる形（1 周になる）は断る。
fn arc_3(a: Point2, mid: Point2, c: Point2) -> Result<Geometry, GripError> {
    // 先に見る。重なった 2 点は「3 点が一直線」にもなるが、理由としては両端の重なりが分かりやすい。
    if a.eq_tol(c) {
        return Err(GripError::ArcEndsMeet);
    }
    let arc = Arc::from_3_points(a, mid, c).ok_or(GripError::Collinear)?;
    // 半径がとても大きいと、両端がトレランスより離れていても開始角と終了角が角度のトレランスで
    // 一致し、1 周の円弧になりうる。`ReplaceGeometries` はそれを受け付けるので、ここで止める。
    if is_full_arc(&arc) {
        return Err(GripError::ArcEndsMeet);
    }
    Ok(Geometry::Arc(arc))
}

/// 1 つの図形の上の複数のグリップを、どれも「その点が `to` へ」行くように順に動かした形。
///
/// [`GripGroup::parts`] が同じ図形から複数のグリップを渡すのは、ポリラインの重なった頂点
/// （始点と終点が同じ位置の開いたポリラインなど）だけ。頂点はそれぞれ別の添字を書き換えるので、
/// 順に動かしても結果は順序によらない。
///
/// # Errors
///
/// どれか 1 つでも成り立たなければ、その理由（[`apply`] と同じ）。
pub fn apply_many(geom: &Geometry, handles: &[Handle], to: Point2) -> Result<Geometry, GripError> {
    let Some((first, rest)) = handles.split_first() else {
        return Err(GripError::NotAGrip);
    };
    let mut moved = apply(geom, *first, to)?;
    for h in rest {
        moved = apply(&moved, *h, to)?;
    }
    Ok(moved)
}

/// まとめて掴んだ図形のうち、断ったもの（[`apply_group`]）。
#[derive(Clone, Debug, PartialEq)]
pub struct PartError {
    /// 断った図形の添字（[`apply_group`] に渡した列の中で）。
    pub index: usize,
    /// 理由。
    pub error: GripError,
}

/// まとめて掴んだグリップを `to` へ動かした形の列（段階 2）。
///
/// **全部か無しか。** 1 つでも成り立たなければ、最初に断った図形の添字と理由を返す
/// （呼び出し側はどれも書き換えない）。それぞれの図形は自分の規則（[`apply_many`]）で動く。
///
/// # Errors
///
/// [`PartError`]。
pub fn apply_group(
    parts: &[(&Geometry, &[Handle])],
    to: Point2,
) -> Result<Vec<Geometry>, PartError> {
    parts
        .iter()
        .enumerate()
        .map(|(index, (geom, handles))| {
            apply_many(geom, handles, to).map_err(|error| PartError { index, error })
        })
        .collect()
}

/// 選んだ図形どうしで同じ位置（[`Point2::eq_tol`]）にある、重なったグリップの束（段階 2）。
///
/// 先頭が**代表**（クリックで拾われたもの。長さ・角度の基点と直交・極は代表の規則で決まる）。
/// 束は [`hit`] だけが作るので、ホバーで大きくなるグリップの束とクリックで掴まれる束は一致する
/// （ADR-0042）。
#[derive(Clone, Debug, PartialEq)]
pub struct GripGroup {
    grips: Vec<Grip>,
}

impl GripGroup {
    /// 代表のグリップ。
    #[must_use]
    pub fn representative(&self) -> Grip {
        self.grips[0]
    }

    /// 束のグリップすべて（先頭が代表）。
    #[must_use]
    pub fn grips(&self) -> &[Grip] {
        &self.grips
    }

    /// 図形ごとに、動かすグリップ。代表の図形が先頭で、その先頭が代表のグリップ。
    ///
    /// 1 つの図形が同じ位置に複数のグリップを持つときは 1 つを選ぶ（代表の図形なら代表、ほかは
    /// 並び（[`Handle`] の順）で先のもの）。ただしポリラインの頂点どうしは全部動かす（始点と終点が
    /// 同じ位置の開いたポリラインの角を動かしても、閉じた形のまま）。
    #[must_use]
    pub fn parts(&self) -> Vec<(EntityId, Vec<Handle>)> {
        let rep = self.representative();
        let mut out: Vec<(EntityId, Vec<Handle>)> = Vec::new();
        for g in &self.grips {
            if out.iter().any(|(id, _)| *id == g.id) {
                continue;
            }
            let mut mine: Vec<Handle> = self
                .grips
                .iter()
                .filter(|o| o.id == g.id)
                .map(|o| o.handle)
                .collect();
            mine.sort();
            let chosen = if g.id == rep.id { rep.handle } else { mine[0] };
            let mut handles = vec![chosen];
            if matches!(chosen, Handle::Vertex(_)) {
                handles.extend(
                    mine.into_iter()
                        .filter(|h| *h != chosen && matches!(h, Handle::Vertex(_))),
                );
            }
            out.push((g.id, handles));
        }
        out
    }

    /// 乗せたときにカーソル横へ出す案内。図形が 1 つなら代表の案内（「端点を動かす」など）。
    /// 複数なら、どれも同じ案内ならそれ、混ざっていれば「点を動かす」に、図形の数を添える
    /// （「端点を動かす（2 個）」）。
    #[must_use]
    pub fn label(&self) -> String {
        let parts = self.parts();
        let first = parts[0].1[0].label();
        if parts.len() == 1 {
            return first.to_owned();
        }
        let same = parts.iter().all(|(_, h)| h[0].label() == first);
        let label = if same { first } else { "点を動かす" };
        format!("{label}（{} 個）", parts.len())
    }
}

/// `at` に当たるグリップの束。グリップを中心とする一辺 `2 × tolerance` の正方形（画面上で一定の
/// 大きさ）の中にあるもののうち、最も近いものと、それと同じ位置（[`Point2::eq_tol`]）にあるもの全部。
///
/// 最も近いものが同じ距離で複数あれば `EntityId` の大きい方（後から作った = 手前に描かれる方。
/// 図形のピックと同じ）。束の代表は、束の中で `EntityId` の大きい方、同じ図形なら並び
/// （[`Handle`] の順 = [`grips_of`] の順）で先のもの。束の残りは `EntityId` の大きい順。
#[must_use]
pub fn hit(grips: &[Grip], at: Point2, tolerance: f64) -> Option<GripGroup> {
    let mut best: Option<(Grip, f64)> = None;
    for g in grips {
        let d = g.at - at;
        if d.x.abs() > tolerance || d.y.abs() > tolerance {
            continue;
        }
        let dist = d.len();
        let better = match best {
            None => true,
            Some((b, bd)) => dist < bd || (dist == bd && g.id > b.id),
        };
        if better {
            best = Some((*g, dist));
        }
    }
    let (nearest, _) = best?;
    let mut group: Vec<Grip> = grips
        .iter()
        .filter(|g| g.at.eq_tol(nearest.at))
        .copied()
        .collect();
    // 代表を先頭に（EntityId の大きい順、同じ図形なら並びの順）。
    group.sort_by(|a, b| b.id.cmp(&a.id).then(a.handle.cmp(&b.handle)));
    Some(GripGroup { grips: group })
}

#[cfg(test)]
mod tests;
