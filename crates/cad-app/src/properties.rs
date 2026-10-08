//! プロパティパネルの中身を決める純粋な関数（Issue #31 段階 1）。
//!
//! egui に依存しない。図形から「表示項目の一覧」を作り、選択から「種類ごとの件数と
//! 共通のレイヤ」の要約を作る。描くのは `properties_panel.rs`。
//!
//! # 角度は度で表示する
//!
//! 図形は角度を rad で持つが、表示は度（設計原則 6: 式の中の角度は度、に合わせる）。
//! 計算で求める角度（線分・作図線の向き）と持っている角度（円弧の開始・終了、
//! インスタンスの回転）は、見た目をそろえるためどれも `[0, 360)` に正規化して出す。
//! 掃引角だけは `(0, 360]` で、1 周の円弧は `360` と出る。
//!
//! # 要約のキャッシュ
//!
//! 全選択（Ctrl+A）で 1 万図形を選んでも、フレームごとに 1 万個を数え直さない。
//! 要約は図面の版番号と選択の版番号（[`Selection::revision`]）をキーに作り直す。

use cad_core::component::DefinitionTable;
use cad_core::geom::tolerance::wrap_2pi;
use cad_core::layer::Layer;
use cad_core::{Document, Geometry, LayerId};

use crate::selection::Selection;

/// 何も選んでいないときの案内。
pub const EMPTY_NOTE: &str = "図形を選ぶと、ここに値が出ます";
/// コマンド実行中（選択待ちを含む）の案内。パネルは表示だけになる。
///
/// Esc で中断すると選択も外れる（`Session::cancel`）。「Esc を押せば変えられる」と読んで
/// 選び直しになる人が出ないよう、そう書いておく。
pub const BUSY_NOTE: &str =
    "コマンド実行中は変更できません（終えるか Esc で中断。中断すると選択も外れます）";
/// レイヤへの移動（`MoveEntitiesToLayer`）の名前。パネルから返るコマンドを見分けるのに使う。
pub const MOVE_TO_LAYER_COMMAND: &str = "LAYER_MOVE_ENTITIES";
/// 選択がまたぐレイヤが 1 つに決まらないときの、ドロップダウンの表示。
pub const MIXED_LAYER: &str = "（混在）";
/// 複数選択のとき、個々の値は出さない旨の案内。
pub const MULTI_NOTE: &str = "複数選択のため、共通の項目（レイヤ）だけ出しています";

/// 図形の種類（パネルでの表示順でもある）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Line,
    Circle,
    Arc,
    Polyline,
    Xline,
    Instance,
}

impl Kind {
    /// 画面に出す名前。
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Line => "線分",
            Self::Circle => "円",
            Self::Arc => "円弧",
            Self::Polyline => "ポリライン",
            Self::Xline => "作図線",
            Self::Instance => "インスタンス",
        }
    }
}

/// 図形の種類。
#[must_use]
pub fn kind_of(geom: &Geometry) -> Kind {
    match geom {
        Geometry::Line(_) => Kind::Line,
        Geometry::Circle(_) => Kind::Circle,
        Geometry::Arc(_) => Kind::Arc,
        Geometry::Polyline(_) => Kind::Polyline,
        Geometry::Xline(_) => Kind::Xline,
        Geometry::Instance(_) => Kind::Instance,
    }
}

/// 表示項目 1 行。段階 1 では値はすべて表示だけ。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// 項目名（`始点 X` など）。
    pub label: &'static str,
    /// 表示用に整えた値。
    pub value: String,
}

impl Item {
    fn new(label: &'static str, value: String) -> Self {
        Self { label, value }
    }
}

/// 数値を小数点以下 4 桁で表示する（ステータスバーの座標と同じ）。`-0.0000` は `0.0000` にする。
#[must_use]
pub fn fmt_num(v: f64) -> String {
    let s = format!("{v:.4}");
    if s == "-0.0000" {
        "0.0000".to_owned()
    } else {
        s
    }
}

/// 度の値を `45.0000°` の形で表示する（正規化はしない）。
#[must_use]
pub fn fmt_deg(deg: f64) -> String {
    format!("{}°", fmt_num(deg))
}

/// 角度 [rad] を `[0, 360)` の度に直して表示する。丸めて `360.0000` になる値は `0.0000` にする。
#[must_use]
pub fn fmt_angle(rad: f64) -> String {
    let s = fmt_num(wrap_2pi(rad).to_degrees());
    if s == "360.0000" {
        fmt_deg(0.0)
    } else {
        format!("{s}°")
    }
}

fn yes_no(b: bool) -> String {
    if b { "はい" } else { "いいえ" }.to_owned()
}

/// 図形 1 つの表示項目。種類ごとに要る項目だけを出す（Issue #31 の計画の表）。
#[must_use]
pub fn items(geom: &Geometry, defs: &DefinitionTable) -> Vec<Item> {
    match geom {
        Geometry::Line(l) => {
            let mid = l.midpoint();
            vec![
                Item::new("始点 X", fmt_num(l.a.x)),
                Item::new("始点 Y", fmt_num(l.a.y)),
                Item::new("終点 X", fmt_num(l.b.x)),
                Item::new("終点 Y", fmt_num(l.b.y)),
                Item::new("長さ", fmt_num(l.length())),
                Item::new("角度", fmt_angle(l.vector().angle())),
                Item::new("中点 X", fmt_num(mid.x)),
                Item::new("中点 Y", fmt_num(mid.y)),
            ]
        }
        Geometry::Circle(c) => vec![
            Item::new("中心 X", fmt_num(c.center.x)),
            Item::new("中心 Y", fmt_num(c.center.y)),
            Item::new("半径", fmt_num(c.radius)),
            Item::new("直径", fmt_num(c.radius * 2.0)),
            Item::new("円周", fmt_num(std::f64::consts::TAU * c.radius)),
        ],
        Geometry::Arc(a) => vec![
            Item::new("中心 X", fmt_num(a.center.x)),
            Item::new("中心 Y", fmt_num(a.center.y)),
            Item::new("半径", fmt_num(a.radius)),
            Item::new("開始角", fmt_angle(a.start_angle)),
            Item::new("終了角", fmt_angle(a.end_angle)),
            Item::new("掃引角", fmt_deg(a.sweep().to_degrees())),
            Item::new("弧長", fmt_num(a.length())),
        ],
        Geometry::Polyline(p) => vec![
            Item::new("頂点数", p.vertex_count().to_string()),
            Item::new("閉じ", yes_no(p.closed)),
            Item::new("長さ", fmt_num(p.length())),
        ],
        Geometry::Xline(x) => vec![
            Item::new("通過点 X", fmt_num(x.origin.x)),
            Item::new("通過点 Y", fmt_num(x.origin.y)),
            Item::new("角度", fmt_angle(x.angle())),
        ],
        Geometry::Instance(i) => {
            let name = defs
                .get(i.definition)
                .map_or_else(|| "（定義が見つかりません）".to_owned(), |d| d.name.clone());
            let p = i.placement;
            vec![
                Item::new("定義", name),
                Item::new("基点 X", fmt_num(p.origin.x)),
                Item::new("基点 Y", fmt_num(p.origin.y)),
                Item::new("回転", fmt_angle(p.rotation)),
                Item::new("倍率", fmt_num(p.scale)),
                Item::new("反転", yes_no(p.flipped)),
            ]
        }
    }
}

/// レイヤの選択肢に出す名前。ロック・非表示のレイヤは印を付ける
/// （そこへ移すと選択から外れる。選べるが、気づけるように）。
#[must_use]
pub fn layer_label(layer: &Layer) -> String {
    match (layer.locked, !layer.visible) {
        (false, false) => layer.name.clone(),
        (true, false) => format!("{}（ロック中）", layer.name),
        (false, true) => format!("{}（非表示）", layer.name),
        (true, true) => format!("{}（ロック中・非表示）", layer.name),
    }
}

/// 図形を `layer` へ移した結果、選択から外れたときの案内。
///
/// ロック・非表示のレイヤにある図形は選べない（選択の規則）ので、移すと選択が空になる。
/// パネルが黙って空の表示に戻ると、移せたのか取り消されたのか分からない。移し先の名前と、
/// 外れる理由（ロックなら編集できない、非表示なら画面から消える）を言う。
#[must_use]
pub fn moved_out_note(count: usize, layer: &Layer) -> String {
    let why = match (layer.locked, !layer.visible) {
        (true, false) => "ロック中のレイヤなので",
        (false, true) => "非表示のレイヤなので画面から消え、",
        (true, true) => "ロック中かつ非表示のレイヤなので画面から消え、",
        (false, false) => "",
    };
    format!(
        "{count} 個を「{}」へ移しました。{why}選択から外れます（U で戻せます）",
        layer.name
    )
}

/// 選択した図形のレイヤ。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommonLayer {
    /// 全部が同じレイヤ。
    One(LayerId),
    /// 2 つ以上のレイヤにまたがる。
    Mixed,
}

/// 選択の要約。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    /// 図面に実在する選択の数。
    pub total: usize,
    /// 種類ごとの件数（`Kind` の順、0 件の種類は含まない）。
    pub kinds: Vec<(Kind, usize)>,
    /// 共通のレイヤ。何も選んでいなければ `None`。
    pub layer: Option<CommonLayer>,
}

impl Summary {
    /// 種類ごとの件数を `線分 3、円 2` の形にする。
    #[must_use]
    pub fn kinds_text(&self) -> String {
        self.kinds
            .iter()
            .map(|(k, n)| format!("{} {n}", k.label()))
            .collect::<Vec<_>>()
            .join("、")
    }
}

/// 選択を要約する。図面に無い ID は数えない。
#[must_use]
pub fn summarize(doc: &Document, selection: &Selection) -> Summary {
    let mut counts = [0usize; 6];
    let mut layer: Option<CommonLayer> = None;
    let mut total = 0;
    for id in selection.iter() {
        let Some(e) = doc.entities().get(id) else {
            continue;
        };
        total += 1;
        counts[kind_of(&e.geom) as usize] += 1;
        layer = Some(match layer {
            None => CommonLayer::One(e.layer),
            Some(CommonLayer::One(l)) if l == e.layer => CommonLayer::One(l),
            Some(_) => CommonLayer::Mixed,
        });
    }
    let kinds = [
        Kind::Line,
        Kind::Circle,
        Kind::Arc,
        Kind::Polyline,
        Kind::Xline,
        Kind::Instance,
    ]
    .into_iter()
    .zip(counts)
    .filter(|(_, n)| *n > 0)
    .collect();
    Summary {
        total,
        kinds,
        layer,
    }
}

/// 要約のキャッシュ。図面の版番号と選択の版番号が変わったときだけ作り直す。
#[derive(Debug, Default)]
pub struct SummaryCache {
    key: Option<(u64, u64)>,
    summary: Summary,
    /// 作り直した回数（キャッシュが効いていることをテストで確かめる）。
    #[cfg(test)]
    recomputed: usize,
}

impl SummaryCache {
    /// 現在の選択の要約。版が変わっていなければ前回の結果をそのまま返す。
    pub fn get(&mut self, doc: &Document, selection: &Selection) -> &Summary {
        let key = (doc.revision(), selection.revision());
        if self.key != Some(key) {
            self.summary = summarize(doc, selection);
            self.key = Some(key);
            #[cfg(test)]
            {
                self.recomputed += 1;
            }
        }
        &self.summary
    }

    /// これまでに作り直した回数（テスト用）。
    #[cfg(test)]
    pub fn recomputed(&self) -> usize {
        self.recomputed
    }

    /// 次の `get` で必ず作り直す。図面が丸ごと入れ替わったとき（版番号が重なりうる）に呼ぶ。
    pub fn invalidate(&mut self) {
        self.key = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cad_core::command::{
        AddEntities, AddLayer, DefineComponent, InsertInstance, MoveEntitiesToLayer,
    };
    use cad_core::component::Placement;
    use cad_core::geom::{Arc, Circle, Line, Point2, Polyline, Xline};
    use cad_core::{AciColor, Entity};

    fn add(doc: &mut Document, geom: Geometry, layer: LayerId) -> cad_core::EntityId {
        doc.apply(Box::new(AddEntities::one("TEST", Entity::new(geom, layer))))
            .expect("追加できる");
        doc.entities().ids().last().expect("追加した図形")
    }

    fn line(ax: f64, ay: f64, bx: f64, by: f64) -> Geometry {
        Geometry::Line(Line::new(Point2::new(ax, ay), Point2::new(bx, by)))
    }

    fn values(items: &[Item]) -> Vec<(&'static str, &str)> {
        items
            .iter()
            .map(|i| (i.label, i.value.as_str()))
            .collect::<Vec<_>>()
    }

    #[test]
    fn line_shows_endpoints_length_angle_and_midpoint() {
        let defs = DefinitionTable::new();
        let got = items(&line(0.0, 0.0, 30.0, 40.0), &defs);
        assert_eq!(
            values(&got),
            vec![
                ("始点 X", "0.0000"),
                ("始点 Y", "0.0000"),
                ("終点 X", "30.0000"),
                ("終点 Y", "40.0000"),
                ("長さ", "50.0000"),
                ("角度", "53.1301°"),
                ("中点 X", "15.0000"),
                ("中点 Y", "20.0000"),
            ]
        );
    }

    /// 角度は度で出て、`[0, 360)` に収まる（負の向きは 360 を足した値、真左は 180）。
    #[test]
    fn line_angle_is_in_degrees_between_0_and_360() {
        let defs = DefinitionTable::new();
        let angle = |g: Geometry| items(&g, &defs)[5].value.clone();
        assert_eq!(angle(line(0.0, 0.0, 1.0, 0.0)), "0.0000°");
        assert_eq!(angle(line(0.0, 0.0, 0.0, 1.0)), "90.0000°");
        assert_eq!(angle(line(0.0, 0.0, -1.0, 0.0)), "180.0000°");
        assert_eq!(angle(line(0.0, 0.0, 0.0, -1.0)), "270.0000°");
        assert_eq!(angle(line(0.0, 0.0, 1.0, -1.0)), "315.0000°");
    }

    /// 丸めると 360 や -0 になる値を、`360.0000°` や `-0.0000` で見せない。
    #[test]
    fn rounding_never_shows_360_or_negative_zero() {
        // 360° のすぐ手前（丸めると 360.0000 になる）と、0 のすぐ手前の負の値。
        assert_eq!(fmt_angle(-f64::MIN_POSITIVE), "0.0000°");
        assert_eq!(fmt_angle(std::f64::consts::TAU), "0.0000°");
        assert_eq!(
            fmt_angle(std::f64::consts::TAU * (1.0 - f64::EPSILON)),
            "0.0000°"
        );
        assert_eq!(fmt_num(-f64::MIN_POSITIVE), "0.0000");
        assert_eq!(fmt_num(-0.0), "0.0000");
        assert_eq!(fmt_num(-2.5), "-2.5000");
    }

    #[test]
    fn circle_shows_center_radius_diameter_and_circumference() {
        let defs = DefinitionTable::new();
        let g = Geometry::Circle(Circle::new(Point2::new(10.0, -5.0), 2.0));
        let got = items(&g, &defs);
        assert_eq!(
            values(&got),
            vec![
                ("中心 X", "10.0000"),
                ("中心 Y", "-5.0000"),
                ("半径", "2.0000"),
                ("直径", "4.0000"),
                ("円周", "12.5664"),
            ]
        );
    }

    #[test]
    fn arc_shows_angles_in_degrees_sweep_and_length() {
        let defs = DefinitionTable::new();
        let g = Geometry::Arc(Arc::new(
            Point2::ORIGIN,
            10.0,
            std::f64::consts::FRAC_PI_2,
            std::f64::consts::PI,
        ));
        let got = items(&g, &defs);
        assert_eq!(
            values(&got),
            vec![
                ("中心 X", "0.0000"),
                ("中心 Y", "0.0000"),
                ("半径", "10.0000"),
                ("開始角", "90.0000°"),
                ("終了角", "180.0000°"),
                ("掃引角", "90.0000°"),
                ("弧長", "15.7080"),
            ]
        );
    }

    /// 開始角と終了角が一致する円弧は 1 周（掃引 360°）として出る。
    #[test]
    fn full_circle_arc_shows_a_360_degree_sweep() {
        let defs = DefinitionTable::new();
        let g = Geometry::Arc(Arc::new(Point2::ORIGIN, 1.0, 0.5, 0.5));
        let got = items(&g, &defs);
        assert_eq!(got[5], Item::new("掃引角", "360.0000°".to_owned()));
        // DXF の 0°→360° の円弧（終了角が 2π）も、終了角は 0°、掃引は 360°。
        let g = Geometry::Arc(Arc::new(Point2::ORIGIN, 1.0, 0.0, std::f64::consts::TAU));
        let got = items(&g, &defs);
        assert_eq!(got[4].value, "0.0000°");
        assert_eq!(got[5].value, "360.0000°");
    }

    #[test]
    fn polyline_shows_vertex_count_closed_and_length() {
        let defs = DefinitionTable::new();
        let open = Polyline::new(
            vec![Point2::ORIGIN, Point2::new(3.0, 0.0), Point2::new(3.0, 4.0)],
            false,
        );
        let got = items(&Geometry::Polyline(open.clone()), &defs);
        assert_eq!(
            values(&got),
            vec![("頂点数", "3"), ("閉じ", "いいえ"), ("長さ", "7.0000")]
        );
        let closed = Polyline {
            closed: true,
            ..open
        };
        let got = items(&Geometry::Polyline(closed), &defs);
        assert_eq!(
            values(&got),
            vec![("頂点数", "3"), ("閉じ", "はい"), ("長さ", "12.0000")]
        );
    }

    #[test]
    fn xline_shows_through_point_and_angle() {
        let defs = DefinitionTable::new();
        let g = Geometry::Xline(Xline::at_angle(
            Point2::new(1.0, 2.0),
            std::f64::consts::FRAC_PI_4,
        ));
        let got = items(&g, &defs);
        assert_eq!(
            values(&got),
            vec![
                ("通過点 X", "1.0000"),
                ("通過点 Y", "2.0000"),
                ("角度", "45.0000°")
            ]
        );
    }

    #[test]
    fn instance_shows_definition_name_and_placement() {
        let mut doc = Document::new();
        doc.apply(Box::new(DefineComponent::new(
            "COMPONENT",
            "窓",
            Point2::ORIGIN,
            vec![Entity::new(line(0.0, 0.0, 1.0, 0.0), LayerId::ZERO)],
        )))
        .expect("定義");
        let def = doc.definitions().by_name("窓").expect("あるはず");
        let placement = Placement::new(
            Point2::new(5.0, 6.0),
            -std::f64::consts::FRAC_PI_2,
            2.0,
            true,
        )
        .expect("配置");
        doc.apply(Box::new(InsertInstance::new(
            "INSERT",
            def,
            placement,
            LayerId::ZERO,
        )))
        .expect("配置");
        let id = doc.entities().ids().next().expect("インスタンス");
        let geom = &doc.entities().get(id).expect("あるはず").geom;
        let got = items(geom, doc.definitions());
        assert_eq!(
            values(&got),
            vec![
                ("定義", "窓"),
                ("基点 X", "5.0000"),
                ("基点 Y", "6.0000"),
                ("回転", "270.0000°"),
                ("倍率", "2.0000"),
                ("反転", "はい"),
            ]
        );
        // 定義が無い（壊れた図面）でも落ちない。
        let got = items(geom, &DefinitionTable::new());
        assert_eq!(got[0].value, "（定義が見つかりません）");
    }

    #[test]
    fn every_kind_has_a_distinct_label_and_items() {
        let defs = DefinitionTable::new();
        let geoms = [
            line(0.0, 0.0, 1.0, 0.0),
            Geometry::Circle(Circle::new(Point2::ORIGIN, 1.0)),
            Geometry::Arc(Arc::new(Point2::ORIGIN, 1.0, 0.0, 1.0)),
            Geometry::Polyline(Polyline::new(
                vec![Point2::ORIGIN, Point2::new(1.0, 0.0)],
                false,
            )),
            Geometry::Xline(Xline::horizontal(Point2::ORIGIN)),
        ];
        let mut labels = std::collections::BTreeSet::new();
        for g in &geoms {
            assert!(labels.insert(kind_of(g).label()));
            assert!(!items(g, &defs).is_empty());
        }
    }

    /// `Session::apply_external` はこの名前で「レイヤへの移動」を見分ける。cad-core 側の名前が
    /// 変わったら、案内が黙って出なくなる前にここで落とす。
    #[test]
    fn the_move_to_layer_command_name_matches_cad_core() {
        use cad_core::command::MoveEntitiesToLayer;
        use cad_core::Command as _;
        assert_eq!(
            MoveEntitiesToLayer::new(Vec::new(), LayerId::ZERO).name(),
            MOVE_TO_LAYER_COMMAND
        );
    }

    #[test]
    fn moved_out_note_names_the_destination_and_the_reason() {
        let mut l = Layer::new("ロック済み", AciColor::WHITE);
        l.locked = true;
        assert_eq!(
            moved_out_note(2, &l),
            "2 個を「ロック済み」へ移しました。ロック中のレイヤなので選択から外れます（U で戻せます）"
        );
        l.locked = false;
        l.visible = false;
        let hidden = moved_out_note(1, &l);
        assert!(hidden.contains("「ロック済み」へ移しました"), "{hidden}");
        assert!(
            hidden.contains("非表示") && hidden.contains("画面から消え"),
            "{hidden}"
        );
        l.locked = true;
        let both = moved_out_note(1, &l);
        assert!(
            both.contains("ロック中") && both.contains("非表示"),
            "{both}"
        );
    }

    #[test]
    fn layer_label_marks_locked_and_hidden_layers() {
        let mut l = Layer::new("壁", AciColor::WHITE);
        assert_eq!(layer_label(&l), "壁");
        l.locked = true;
        assert_eq!(layer_label(&l), "壁（ロック中）");
        l.visible = false;
        assert_eq!(layer_label(&l), "壁（ロック中・非表示）");
        l.locked = false;
        assert_eq!(layer_label(&l), "壁（非表示）");
    }

    /// 複数選択の要約: 種類ごとの件数と、共通のレイヤ（またがるなら混在）。
    #[test]
    fn summary_counts_kinds_and_finds_the_common_layer() {
        let mut doc = Document::new();
        doc.apply(Box::new(AddLayer::new("L1", AciColor::RED)))
            .expect("レイヤ");
        let l1 = doc.layers().by_name("L1").expect("L1");
        let a = add(&mut doc, line(0.0, 0.0, 1.0, 0.0), LayerId::ZERO);
        let b = add(&mut doc, line(0.0, 1.0, 1.0, 1.0), LayerId::ZERO);
        let c = add(
            &mut doc,
            Geometry::Circle(Circle::new(Point2::ORIGIN, 1.0)),
            LayerId::ZERO,
        );
        let d = add(&mut doc, line(0.0, 2.0, 1.0, 2.0), l1);

        let mut sel = Selection::new();
        assert_eq!(summarize(&doc, &sel), Summary::default());

        for id in [a, b, c] {
            sel.insert(id);
        }
        let s = summarize(&doc, &sel);
        assert_eq!(s.total, 3);
        assert_eq!(s.kinds, vec![(Kind::Line, 2), (Kind::Circle, 1)]);
        assert_eq!(s.kinds_text(), "線分 2、円 1");
        assert_eq!(s.layer, Some(CommonLayer::One(LayerId::ZERO)));

        sel.insert(d);
        let s = summarize(&doc, &sel);
        assert_eq!(s.kinds_text(), "線分 3、円 1");
        assert_eq!(s.layer, Some(CommonLayer::Mixed), "レイヤがまたがる");

        // 全部を同じレイヤへ移せば、また共通になる。
        doc.apply(Box::new(MoveEntitiesToLayer::new(sel.to_vec(), l1)))
            .expect("移動");
        assert_eq!(summarize(&doc, &sel).layer, Some(CommonLayer::One(l1)));
    }

    /// 図面に無い ID（消えた図形）は数えない。
    #[test]
    fn summary_ignores_ids_missing_from_the_document() {
        let mut doc = Document::new();
        let a = add(&mut doc, line(0.0, 0.0, 1.0, 0.0), LayerId::ZERO);
        let mut sel = Selection::new();
        sel.insert(a);
        doc.undo().expect("取り消し");
        let s = summarize(&doc, &sel);
        assert_eq!(s.total, 0);
        assert_eq!(s.layer, None);
    }

    /// 要約は図面か選択が変わったときだけ作り直す。1 万図形の全選択でもフレームごとに数え直さない。
    #[test]
    fn cache_recomputes_only_when_the_document_or_selection_changes() {
        let mut doc = Document::new();
        let a = add(&mut doc, line(0.0, 0.0, 1.0, 0.0), LayerId::ZERO);
        let b = add(&mut doc, line(0.0, 1.0, 1.0, 1.0), LayerId::ZERO);
        let mut sel = Selection::new();
        let mut cache = SummaryCache::default();

        assert_eq!(cache.get(&doc, &sel).total, 0);
        assert_eq!(cache.recomputed, 1);
        for _ in 0..100 {
            let _ = cache.get(&doc, &sel);
        }
        assert_eq!(cache.recomputed, 1, "何も変わらなければ作り直さない");

        sel.insert(a);
        assert_eq!(cache.get(&doc, &sel).total, 1);
        assert_eq!(cache.recomputed, 2, "選択が変わった");
        sel.insert(a);
        let _ = cache.get(&doc, &sel);
        assert_eq!(cache.recomputed, 2, "同じ図形をもう一度足しても変わらない");

        sel.insert(b);
        assert_eq!(cache.get(&doc, &sel).total, 2);
        assert_eq!(cache.recomputed, 3);

        doc.apply(Box::new(AddLayer::new("L1", AciColor::RED)))
            .expect("レイヤ");
        let _ = cache.get(&doc, &sel);
        assert_eq!(cache.recomputed, 4, "図面が変わった");

        cache.invalidate();
        let _ = cache.get(&doc, &sel);
        assert_eq!(cache.recomputed, 5, "無効化したら作り直す");
    }
}
