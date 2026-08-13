//! 幾何の座標に式を束縛する仕組み。
//!
//! # 疎な上書き
//!
//! 定義の中身は**ふつうの [`Entity`]** で、式は入っていない（ADR-0029）。
//! 式を持つのは束縛だけで、**パラメトリックにしたい座標だけ**を疎に上書きする。
//! 束縛が 1 つも無ければクラシックなブロックとして振る舞う。
//!
//! # 角度の単位
//!
//! **角度のスロットは度で受け取る。** 式の中の角度は度という約束
//! （[`crate::expr`] のモジュールドキュメント）に合わせ、
//! ここでラジアンへ直す。内部表現がラジアンであることを式の書き手に見せない。
//!
//! # 束縛が指す先がずれる問題
//!
//! 束縛は「[`Definition::entities`](super::Definition::entities) への添字 + どのスカラーか」で
//! 座標を指す。**定義の中身を差し替えると、添字の指す先が変わる。**
//!
//! そこで **[`Definition`](super::Definition) は中身と束縛を必ず一緒に持ち替える**
//! ことにした（`replace_contents` が両方を受け取る）。
//! コマンドが「添字が範囲内か」「スロットが図形の種類に合うか」を検査するので、
//! **`Document` に入っている定義に、指す先の無い束縛は存在しない。**
//!
//! 中身だけを差し替えると束縛は捨てられる。段階 3 のインプレース編集では
//! 「編集しても束縛を保つ」ことが要るので、そのときは定義内で安定な ID を
//! エンティティへ持たせる必要がある（`docs/PROGRESS.md` の積み残し）。

use crate::entity::{Entity, Geometry};
use crate::expr::{Expr, ParamType, Value};

/// 束縛できるスカラーの位置。
///
/// **既存の変種の意味を変えない。** 永続化される値なので、
/// 変えると過去に保存したファイルの束縛が別の座標を指すようになる。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Slot {
    /// 線分の始点 X。
    LineAx,
    /// 線分の始点 Y。
    LineAy,
    /// 線分の終点 X。
    LineBx,
    /// 線分の終点 Y。
    LineBy,
    /// 円の中心 X。
    CircleCx,
    /// 円の中心 Y。
    CircleCy,
    /// 円の半径。**正でなければならない。**
    CircleR,
    /// 円弧の中心 X。
    ArcCx,
    /// 円弧の中心 Y。
    ArcCy,
    /// 円弧の半径。**正でなければならない。**
    ArcR,
    /// 円弧の開始角。**度**で受け取る。
    ArcStart,
    /// 円弧の終了角。**度**で受け取る。
    ArcEnd,
    /// 作図線の通過点 X。
    XlineOx,
    /// 作図線の通過点 Y。
    XlineOy,
    /// 作図線の方向。**度**で受け取る。
    XlineAngle,
    /// ポリラインの頂点 X。
    PolylineVx(u32),
    /// ポリラインの頂点 Y。
    PolylineVy(u32),
    /// インスタンスの配置の X。
    InstanceX,
    /// インスタンスの配置の Y。
    InstanceY,
    /// インスタンスの回転。**度**で受け取る。
    InstanceRotation,
    /// インスタンスの倍率。**正でなければならない。**
    InstanceScale,
}

impl Slot {
    /// 表示用の名前。
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::LineAx => "始点X",
            Self::LineAy => "始点Y",
            Self::LineBx => "終点X",
            Self::LineBy => "終点Y",
            Self::CircleCx | Self::ArcCx => "中心X",
            Self::CircleCy | Self::ArcCy => "中心Y",
            Self::CircleR | Self::ArcR => "半径",
            Self::ArcStart => "開始角",
            Self::ArcEnd => "終了角",
            Self::XlineOx => "通過点X",
            Self::XlineOy => "通過点Y",
            Self::XlineAngle => "角度",
            Self::PolylineVx(_) => "頂点X",
            Self::PolylineVy(_) => "頂点Y",
            Self::InstanceX => "配置X",
            Self::InstanceY => "配置Y",
            Self::InstanceRotation => "回転",
            Self::InstanceScale => "倍率",
        }
    }

    /// 角度を表すスロットか。**度からラジアンへ直す必要がある。**
    #[must_use]
    pub fn is_angle(self) -> bool {
        matches!(
            self,
            Self::ArcStart | Self::ArcEnd | Self::XlineAngle | Self::InstanceRotation
        )
    }

    /// 正でなければならないスロットか。
    #[must_use]
    pub fn must_be_positive(self) -> bool {
        matches!(self, Self::CircleR | Self::ArcR | Self::InstanceScale)
    }

    /// この図形に対して意味を持つスロットか。
    ///
    /// **コマンドが束縛を受け取る前に必ず検査する。** 合わないスロットを許すと、
    /// 解決のたびに黙って無視される束縛が図面に残る。
    #[must_use]
    pub fn fits(self, geom: &Geometry) -> bool {
        match geom {
            Geometry::Line(_) => matches!(
                self,
                Self::LineAx | Self::LineAy | Self::LineBx | Self::LineBy
            ),
            Geometry::Circle(_) => {
                matches!(self, Self::CircleCx | Self::CircleCy | Self::CircleR)
            }
            Geometry::Arc(_) => matches!(
                self,
                Self::ArcCx | Self::ArcCy | Self::ArcR | Self::ArcStart | Self::ArcEnd
            ),
            Geometry::Xline(_) => {
                matches!(self, Self::XlineOx | Self::XlineOy | Self::XlineAngle)
            }
            Geometry::Polyline(p) => match self {
                Self::PolylineVx(i) | Self::PolylineVy(i) => (i as usize) < p.vertices.len(),
                _ => false,
            },
            Geometry::Instance(_) => matches!(
                self,
                Self::InstanceX | Self::InstanceY | Self::InstanceRotation | Self::InstanceScale
            ),
        }
    }

    /// このスロットが受け取る値の型。いまはすべて数値。
    #[must_use]
    pub fn value_type(self) -> ParamType {
        ParamType::Number
    }

    /// 図形の該当スカラーを `value` に差し替える。
    ///
    /// 差し替えられなければ `false` を返し、**図形は変更しない**。
    /// 起きるのは以下の場合で、いずれも「その座標だけ定義のままにする」ことで
    /// 図形が消えたり壊れたりしないようにしている。
    ///
    /// - スロットが図形に合わない（コマンドが弾くので通常は起きない）
    /// - 値が有限でない
    /// - 正でなければならないスロットに 0 以下が来た
    ///
    /// **角度のスロットは度として受け取り、ここでラジアンへ直す。**
    pub fn apply(self, geom: &mut Geometry, value: f64) -> bool {
        if !value.is_finite() {
            return false;
        }
        if self.must_be_positive() && value <= 0.0 {
            return false;
        }
        let v = if self.is_angle() {
            value.to_radians()
        } else {
            value
        };

        match (self, geom) {
            (Self::LineAx, Geometry::Line(l)) => l.a.x = v,
            (Self::LineAy, Geometry::Line(l)) => l.a.y = v,
            (Self::LineBx, Geometry::Line(l)) => l.b.x = v,
            (Self::LineBy, Geometry::Line(l)) => l.b.y = v,

            (Self::CircleCx, Geometry::Circle(c)) => c.center.x = v,
            (Self::CircleCy, Geometry::Circle(c)) => c.center.y = v,
            (Self::CircleR, Geometry::Circle(c)) => c.radius = v,

            (Self::ArcCx, Geometry::Arc(a)) => a.center.x = v,
            (Self::ArcCy, Geometry::Arc(a)) => a.center.y = v,
            (Self::ArcR, Geometry::Arc(a)) => a.radius = v,
            (Self::ArcStart, Geometry::Arc(a)) => a.start_angle = v,
            (Self::ArcEnd, Geometry::Arc(a)) => a.end_angle = v,

            (Self::XlineOx, Geometry::Xline(x)) => x.origin.x = v,
            (Self::XlineOy, Geometry::Xline(x)) => x.origin.y = v,
            // 方向は単位ベクトルという不変条件があるので、角度から作り直す。
            (Self::XlineAngle, Geometry::Xline(x)) => {
                x.direction = crate::geom::Vec2::new(v.cos(), v.sin());
            }

            (Self::PolylineVx(i), Geometry::Polyline(p)) => match p.vertices.get_mut(i as usize) {
                Some(vertex) => vertex.x = v,
                None => return false,
            },
            (Self::PolylineVy(i), Geometry::Polyline(p)) => match p.vertices.get_mut(i as usize) {
                Some(vertex) => vertex.y = v,
                None => return false,
            },

            (Self::InstanceX, Geometry::Instance(inst)) => inst.placement.origin.x = v,
            (Self::InstanceY, Geometry::Instance(inst)) => inst.placement.origin.y = v,
            (Self::InstanceRotation, Geometry::Instance(inst)) => inst.placement.rotation = v,
            (Self::InstanceScale, Geometry::Instance(inst)) => inst.placement.scale = v,

            // スロットと図形の組み合わせが合わない。
            _ => return false,
        }
        true
    }
}

/// 座標 1 つへの式の束縛。
#[derive(Clone, Debug, PartialEq)]
pub struct Binding {
    /// [`Definition::entities`](super::Definition::entities) への添字。
    pub entity: usize,
    /// どのスカラーか。
    pub slot: Slot,
    /// **検証済みの**式。文字列は持たない。
    pub expr: Expr,
}

impl Binding {
    /// 束縛を作る。
    #[must_use]
    pub fn new(entity: usize, slot: Slot, expr: Expr) -> Self {
        Self { entity, slot, expr }
    }

    /// 中身に対して妥当か（添字が範囲内で、スロットが図形に合う）。
    #[must_use]
    pub fn fits(&self, entities: &[Entity]) -> bool {
        entities
            .get(self.entity)
            .is_some_and(|e| self.slot.fits(&e.geom))
    }
}

/// パラメータの宣言。
#[derive(Clone, Debug, PartialEq)]
pub struct ParamDecl {
    /// 名前。式の中でこの名前で参照する。
    pub name: String,
    /// 型。
    pub ty: ParamType,
    /// 既定値の式。**他のパラメータを参照してよい**（循環はコマンドが弾く）。
    pub default: Expr,
    /// 数値パラメータの許容範囲（下限, 上限）。両端を含む。
    ///
    /// **範囲外の値はコマンドが拒否する。** 0 倍率や負の半径を作らせないための柵。
    pub range: Option<(f64, f64)>,
}

impl ParamDecl {
    /// 数値パラメータ。
    #[must_use]
    pub fn number(name: impl Into<String>, default: f64) -> Self {
        Self {
            name: name.into(),
            ty: ParamType::Number,
            default: Expr::number(default),
            range: None,
        }
    }

    /// 真偽パラメータ。
    #[must_use]
    pub fn boolean(name: impl Into<String>, default: bool) -> Self {
        Self {
            name: name.into(),
            ty: ParamType::Bool,
            default: Expr::Literal(Value::Bool(default)),
            range: None,
        }
    }

    /// 選択パラメータ。既定値は先頭の候補。
    ///
    /// 候補が空なら `None`（値を取りようがない）。
    #[must_use]
    pub fn choice(name: impl Into<String>, options: Vec<String>) -> Option<Self> {
        let first = options.first()?.clone();
        Some(Self {
            name: name.into(),
            ty: ParamType::Choice(options),
            default: Expr::Literal(Value::Choice(first)),
            range: None,
        })
    }

    /// 範囲を付けた複製。
    #[must_use]
    pub fn with_range(mut self, lo: f64, hi: f64) -> Self {
        self.range = Some((lo, hi));
        self
    }

    /// 値がこの宣言を満たすか（型と範囲）。
    #[must_use]
    pub fn accepts(&self, value: &Value) -> bool {
        if !self.ty.accepts(value) {
            return false;
        }
        match (self.range, value) {
            (Some((lo, hi)), Value::Number(n)) => *n >= lo && *n <= hi,
            _ => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::{parse, Env, ParamType, Value};
    use crate::geom::tolerance::eq_len;
    use crate::geom::{Arc, Circle, Line, Point2, Polyline, Vec2, Xline};
    use crate::layer::LayerId;

    fn p(x: f64, y: f64) -> Point2 {
        Point2::new(x, y)
    }

    fn line() -> Geometry {
        Geometry::Line(Line::new(p(0.0, 0.0), p(1.0, 1.0)))
    }

    fn circle() -> Geometry {
        Geometry::Circle(Circle::new(p(0.0, 0.0), 1.0))
    }

    fn arc() -> Geometry {
        Geometry::Arc(Arc::new(p(0.0, 0.0), 1.0, 0.0, 1.0))
    }

    fn xline() -> Geometry {
        Geometry::Xline(Xline::new(p(0.0, 0.0), Vec2::new(1.0, 0.0)).expect("作図線"))
    }

    fn polyline(n: usize) -> Geometry {
        Geometry::Polyline(Polyline::new(vec![p(0.0, 0.0); n], false))
    }

    // ---- スロットと図形の対応 ---------------------------------------------

    /// **スロットが図形の種類に合うかを正しく判定すること。**
    ///
    /// 合わない束縛を通すと、解決のたびに黙って無視される束縛が図面に残る。
    #[test]
    fn slots_only_fit_their_own_geometry() {
        assert!(Slot::LineAx.fits(&line()));
        assert!(!Slot::LineAx.fits(&circle()));
        assert!(!Slot::CircleR.fits(&line()));
        assert!(Slot::CircleR.fits(&circle()));
        assert!(Slot::ArcStart.fits(&arc()));
        assert!(!Slot::ArcStart.fits(&circle()), "円に開始角は無い");
        assert!(Slot::XlineAngle.fits(&xline()));
        assert!(!Slot::XlineAngle.fits(&line()));
    }

    /// ポリラインの頂点番号は範囲内でなければならないこと。
    #[test]
    fn polyline_vertex_slots_are_range_checked() {
        let pl = polyline(3);
        assert!(Slot::PolylineVx(0).fits(&pl));
        assert!(Slot::PolylineVy(2).fits(&pl));
        assert!(!Slot::PolylineVx(3).fits(&pl), "頂点は 0..3");
        assert!(!Slot::PolylineVy(99).fits(&pl));
    }

    // ---- 値の適用 ---------------------------------------------------------

    #[test]
    fn apply_sets_the_scalar() {
        let mut g = line();
        assert!(Slot::LineBx.apply(&mut g, 5.0));
        let Geometry::Line(l) = &g else { panic!() };
        assert!(eq_len(l.b.x, 5.0));
        assert!(eq_len(l.b.y, 1.0), "他の座標は変わらない");
    }

    /// **角度のスロットは度で受け取り、ラジアンへ直すこと。**
    ///
    /// 式の中の角度は度という約束（座標入力の `@100<45` と同じ）。
    /// 内部表現がラジアンであることを式の書き手に見せない。
    #[test]
    fn angle_slots_take_degrees() {
        let mut g = arc();
        assert!(Slot::ArcStart.apply(&mut g, 90.0));
        let Geometry::Arc(a) = &g else { panic!() };
        assert!(
            eq_len(a.start_angle, std::f64::consts::FRAC_PI_2),
            "90 度がラジアンになる: {}",
            a.start_angle
        );
    }

    /// 作図線の角度は**単位ベクトルを作り直す**こと（不変条件を守る）。
    #[test]
    fn xline_angle_rebuilds_a_unit_direction() {
        let mut g = xline();
        assert!(Slot::XlineAngle.apply(&mut g, 90.0));
        let Geometry::Xline(x) = &g else { panic!() };
        assert!(eq_len(x.direction.len(), 1.0), "単位ベクトルのまま");
        assert!(eq_len(x.direction.x, 0.0) && eq_len(x.direction.y, 1.0));
    }

    /// **半径に 0 以下を入れさせないこと。**
    ///
    /// 入れると縮退した図形になる。その座標だけ定義のままにする。
    #[test]
    fn positive_only_slots_reject_zero_and_negative() {
        for bad in [0.0, -1.0] {
            let mut g = circle();
            assert!(!Slot::CircleR.apply(&mut g, bad), "半径 {bad} は拒否");
            let Geometry::Circle(c) = &g else { panic!() };
            assert!(eq_len(c.radius, 1.0), "元の値のまま");
        }
    }

    /// **`NaN` や無限大を座標へ入れさせないこと。**
    #[test]
    fn non_finite_values_are_rejected() {
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut g = line();
            assert!(!Slot::LineAx.apply(&mut g, bad));
            let Geometry::Line(l) = &g else { panic!() };
            assert!(eq_len(l.a.x, 0.0), "元の値のまま");
        }
    }

    /// 合わないスロットは図形を変えないこと。
    #[test]
    fn a_mismatched_slot_leaves_the_geometry_alone() {
        let mut g = line();
        assert!(!Slot::CircleR.apply(&mut g, 5.0));
        assert_eq!(g, line());
    }

    #[test]
    fn polyline_vertices_can_be_bound() {
        let mut g = polyline(2);
        assert!(Slot::PolylineVx(1).apply(&mut g, 7.0));
        let Geometry::Polyline(pl) = &g else { panic!() };
        assert!(eq_len(pl.vertices[1].x, 7.0));
        assert!(eq_len(pl.vertices[0].x, 0.0), "他の頂点は変わらない");
        // 範囲外は拒否。
        assert!(!Slot::PolylineVx(9).apply(&mut g, 1.0));
    }

    // ---- ParamDecl --------------------------------------------------------

    #[test]
    fn number_params_check_their_range() {
        let d = ParamDecl::number("幅", 900.0).with_range(300.0, 3000.0);
        assert!(d.accepts(&Value::Number(900.0)));
        assert!(d.accepts(&Value::Number(300.0)), "下限を含む");
        assert!(d.accepts(&Value::Number(3000.0)), "上限を含む");
        assert!(!d.accepts(&Value::Number(299.0)));
        assert!(!d.accepts(&Value::Number(3001.0)));
        assert!(!d.accepts(&Value::Bool(true)), "型が違う");
    }

    #[test]
    fn a_param_without_a_range_accepts_any_number() {
        let d = ParamDecl::number("幅", 1.0);
        assert!(d.accepts(&Value::Number(-1.0e9)));
        assert!(d.accepts(&Value::Number(1.0e9)));
    }

    #[test]
    fn choice_params_take_their_first_option_as_default() {
        let d = ParamDecl::choice("種別", vec!["引違い".to_owned(), "開き".to_owned()])
            .expect("候補があるので作れる");
        assert_eq!(
            d.ty,
            ParamType::Choice(vec!["引違い".to_owned(), "開き".to_owned()])
        );
        assert_eq!(
            eval_default(&d),
            Value::Choice("引違い".to_owned()),
            "既定値は先頭の候補"
        );
        assert!(d.accepts(&Value::Choice("開き".to_owned())));
        assert!(!d.accepts(&Value::Choice("FIX".to_owned())), "候補外は拒否");
    }

    /// 候補が無い選択パラメータは作れないこと（値を取りようがない）。
    #[test]
    fn a_choice_param_needs_at_least_one_option() {
        assert!(ParamDecl::choice("種別", Vec::new()).is_none());
    }

    fn eval_default(d: &ParamDecl) -> Value {
        crate::expr::eval(&d.default, &Env::new()).expect("定数なので評価できる")
    }

    // ---- Binding ----------------------------------------------------------

    #[test]
    fn a_binding_fits_only_when_the_target_exists_and_matches() {
        let entities = vec![Entity::new(line(), LayerId::ZERO)];
        let ok = Binding::new(0, Slot::LineAx, parse("幅").expect("解析"));
        assert!(ok.fits(&entities));

        // 添字が範囲外。
        let out_of_range = Binding::new(1, Slot::LineAx, parse("幅").expect("解析"));
        assert!(!out_of_range.fits(&entities));

        // スロットが図形に合わない。
        let wrong_slot = Binding::new(0, Slot::CircleR, parse("幅").expect("解析"));
        assert!(!wrong_slot.fits(&entities));
    }

    #[test]
    fn slot_labels_are_present() {
        for s in [
            Slot::LineAx,
            Slot::CircleR,
            Slot::ArcStart,
            Slot::XlineAngle,
            Slot::PolylineVx(0),
            Slot::InstanceScale,
        ] {
            assert!(!s.label().is_empty());
        }
    }

    #[test]
    fn angle_and_positive_slots_are_classified() {
        assert!(Slot::ArcStart.is_angle());
        assert!(Slot::InstanceRotation.is_angle());
        assert!(!Slot::LineAx.is_angle());
        assert!(Slot::CircleR.must_be_positive());
        assert!(Slot::InstanceScale.must_be_positive());
        assert!(!Slot::LineAx.must_be_positive());
    }
}
