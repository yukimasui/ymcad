//! パラメータの宣言・束縛・上書きのツール。
//!
//! # コマンドラインで扱うための工夫
//!
//! 束縛は「定義の中の何番目の図形か」を指す必要があるが、
//! 定義の中身は図面に出ていないのでクリックで指せない。
//! そこで **プロンプトに一覧を出す**（プロンプトは複数行を表示できる）。
//! 番号とスロット名を見ながら打てるので、番号を覚えておく必要がない。
//!
//! パラメータパネル（段階 3）が入れば、この対話は要らなくなる。
//! それまでの間、**コマンドラインだけで一通り試せる**ことを優先した。
//!
//! # 型は既定値から決める
//!
//! `PARAM` は型を聞かない。既定値の式を評価して
//!
//! - 数値になれば `Number`
//! - 真偽になれば `Bool`
//! - `引違い|開き` のように `|` で区切られていれば `Choice`
//!
//! と決める。型を別に聞くと対話が 1 段増えるうえ、
//! 既定値と型がずれる入力ができてしまう。

use cad_core::command::{SetBinding, SetDefinitionParams, SetInstanceOverride};
use cad_core::component::{Binding, DefinitionId, ParamDecl, Slot};
use cad_core::expr::{eval, parse, Env, ParamType, Value};
use cad_core::geom::Point2;
use cad_core::{Document, EntityId, Geometry};

use super::{StepInput, StepOutcome, Tool, ToolCtx};

/// 選択肢を区切る記号。
const CHOICE_SEPARATOR: char = '|';

// ---------------------------------------------------------------------------
// 共通のヘルパ
// ---------------------------------------------------------------------------

/// 定義名の一覧を「あるものはこれ」と案内する形にする。
fn known_definitions(doc: &Document) -> String {
    let names: Vec<&str> = doc
        .definitions()
        .iter()
        .map(|(_, d)| d.name.as_str())
        .collect();
    if names.is_empty() {
        "（まだありません。COMPONENT で作成してください）".to_owned()
    } else {
        names.join(" / ")
    }
}

/// 定義名を引く。無ければ案内つきで断る。
fn find_definition(doc: &Document, name: &str) -> Result<DefinitionId, StepOutcome> {
    doc.definitions().by_name(name).ok_or_else(|| {
        StepOutcome::Reject(format!(
            "「{name}」がありません。あるのは: {}",
            known_definitions(doc)
        ))
    })
}

/// 数値として解釈された入力を名前へ戻す。
///
/// `1` のような入力は `Session::interpret` が数値にしてしまう。
fn number_as_name(n: f64) -> String {
    if n.fract() == 0.0 && n.is_finite() {
        format!("{n:.0}")
    } else {
        n.to_string()
    }
}

/// この図形に束縛できるスロットの一覧。
fn slots_for(geom: &Geometry) -> Vec<Slot> {
    let all = [
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
        Slot::InstanceX,
        Slot::InstanceY,
        Slot::InstanceRotation,
        Slot::InstanceScale,
    ];
    let mut out: Vec<Slot> = all.into_iter().filter(|s| s.fits(geom)).collect();
    // ポリラインは頂点の数だけ増えるので別に足す。
    if let Geometry::Polyline(p) = geom {
        for i in 0..p.vertices.len() {
            let i = u32::try_from(i).unwrap_or(0);
            out.push(Slot::PolylineVx(i));
            out.push(Slot::PolylineVy(i));
        }
    }
    out
}

/// クリックで指せる「つまみ」。
///
/// 図形の上のどこをクリックしたかで、駆動したい座標を決める。
/// 番号やスロット名を打たせないための仕組み。
#[derive(Debug, Clone, Copy)]
enum Handle {
    /// 点。X と Y の 2 つのスロットを持つ。
    Point {
        /// この点の位置。クリックとの距離を測る。
        at: Point2,
        /// X のスロット。
        x: Slot,
        /// Y のスロット。
        y: Slot,
        /// 表示名。
        label: &'static str,
    },
    /// 単独のスカラー（半径・角度・倍率）。
    Scalar {
        /// 代表点。クリックとの距離を測る。
        at: Point2,
        /// スロット。
        slot: Slot,
        /// 表示名。
        label: &'static str,
    },
}

impl Handle {
    fn position(self) -> Point2 {
        match self {
            Self::Point { at, .. } | Self::Scalar { at, .. } => at,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Point { label, .. } | Self::Scalar { label, .. } => label,
        }
    }
}

/// 図形の上でクリックして指せるつまみを並べる。
///
/// **ポリラインの頂点は数が可変**なので、頂点ごとに 1 つ作る。
fn handles(geom: &Geometry) -> Vec<Handle> {
    match geom {
        Geometry::Line(l) => vec![
            Handle::Point {
                at: l.a,
                x: Slot::LineAx,
                y: Slot::LineAy,
                label: "始点",
            },
            Handle::Point {
                at: l.b,
                x: Slot::LineBx,
                y: Slot::LineBy,
                label: "終点",
            },
        ],
        Geometry::Circle(c) => vec![
            Handle::Point {
                at: c.center,
                x: Slot::CircleCx,
                y: Slot::CircleCy,
                label: "中心",
            },
            Handle::Scalar {
                // 円周の右端を半径のつまみにする。
                at: Point2::new(c.center.x + c.radius, c.center.y),
                slot: Slot::CircleR,
                label: "半径",
            },
        ],
        Geometry::Arc(a) => vec![
            Handle::Point {
                at: a.center,
                x: Slot::ArcCx,
                y: Slot::ArcCy,
                label: "中心",
            },
            Handle::Scalar {
                at: a.start_point(),
                slot: Slot::ArcStart,
                label: "開始角",
            },
            Handle::Scalar {
                at: a.end_point(),
                slot: Slot::ArcEnd,
                label: "終了角",
            },
            Handle::Scalar {
                at: a.point_at(0.5),
                slot: Slot::ArcR,
                label: "半径",
            },
        ],
        Geometry::Xline(x) => vec![
            Handle::Point {
                at: x.origin,
                x: Slot::XlineOx,
                y: Slot::XlineOy,
                label: "通過点",
            },
            Handle::Scalar {
                at: x.point_at(1.0),
                slot: Slot::XlineAngle,
                label: "角度",
            },
        ],
        Geometry::Polyline(p) => p
            .vertices
            .iter()
            .enumerate()
            .filter_map(|(i, v)| {
                let i = u32::try_from(i).ok()?;
                Some(Handle::Point {
                    at: *v,
                    x: Slot::PolylineVx(i),
                    y: Slot::PolylineVy(i),
                    label: "頂点",
                })
            })
            .collect(),
        Geometry::Instance(i) => vec![
            Handle::Point {
                at: i.placement.origin,
                x: Slot::InstanceX,
                y: Slot::InstanceY,
                label: "配置",
            },
            Handle::Scalar {
                at: i.placement.origin,
                slot: Slot::InstanceRotation,
                label: "回転",
            },
            Handle::Scalar {
                at: i.placement.origin,
                slot: Slot::InstanceScale,
                label: "倍率",
            },
        ],
    }
}

/// クリック位置に最も近いつまみ。
fn nearest_handle(geom: &Geometry, at: Point2) -> Option<Handle> {
    handles(geom).into_iter().min_by(|a, b| {
        let (da, db) = ((a.position() - at).len(), (b.position() - at).len());
        da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
    })
}

/// パラメータを番号で選べるように並べる。
///
/// **数字は ASCII なので日本語入力を通さない。** 名前を打つより速い。
fn numbered_params(names: &[String]) -> String {
    names
        .iter()
        .enumerate()
        .map(|(i, n)| format!("{}: {n}", i + 1))
        .collect::<Vec<_>>()
        .join("  ")
}

/// スロットの入力名（`終点X`、`頂点X2` のような形）。
fn slot_input_name(slot: Slot) -> String {
    match slot {
        Slot::PolylineVx(i) | Slot::PolylineVy(i) => format!("{}{i}", slot.label()),
        other => other.label().to_owned(),
    }
}

// ---------------------------------------------------------------------------
// PARAM — パラメータを宣言する
// ---------------------------------------------------------------------------

/// 定義にパラメータを宣言する（同名があれば置き換える）。
#[derive(Debug, Default)]
pub struct ParamTool {
    state: ParamState,
}

#[derive(Debug, Default, Clone, PartialEq)]
enum ParamState {
    #[default]
    Definition,
    Name {
        def: DefinitionId,
    },
    Default {
        def: DefinitionId,
        name: String,
    },
}

impl Tool for ParamTool {
    fn name(&self) -> &'static str {
        "PARAM"
    }

    /// 名前も既定値の式も、打ったままの文字列で受け取る。
    fn wants_raw_text(&self) -> bool {
        true
    }

    fn prompt(&self) -> String {
        match &self.state {
            ParamState::Definition => "コンポーネント名を入力:".to_owned(),
            ParamState::Name { .. } => "パラメータ名を入力:".to_owned(),
            ParamState::Default { name, .. } => format!(
                "「{name}」の既定値を入力:\n\u{3000}数値・式 … 900 / 高さ * 2 + 10\
                 \n\u{3000}真偽     … 真 / 偽\
                 \n\u{3000}選択肢   … 引違い|開き|FIX"
            ),
        }
    }

    fn step(&mut self, input: StepInput, ctx: &ToolCtx<'_>) -> StepOutcome {
        let text = match input {
            StepInput::Word(w) => w,
            StepInput::Number(n) => number_as_name(n),
            StepInput::Enter | StepInput::SelectionReady => return StepOutcome::Finish,
            StepInput::Point(_) | StepInput::Entity { .. } => {
                return StepOutcome::Reject("文字で入力してください".to_owned())
            }
        };

        match self.state.clone() {
            ParamState::Definition => match find_definition(ctx.doc, &text) {
                Ok(def) => {
                    self.state = ParamState::Name { def };
                    StepOutcome::Continue
                }
                Err(reject) => reject,
            },
            ParamState::Name { def } => {
                if text.trim().is_empty() {
                    return StepOutcome::Reject("パラメータ名を入力してください".to_owned());
                }
                self.state = ParamState::Default { def, name: text };
                StepOutcome::Continue
            }
            ParamState::Default { def, name } => Self::commit(ctx, def, &name, &text),
        }
    }
}

impl ParamTool {
    /// 既定値の入力から宣言を組み立てる。
    fn commit(ctx: &ToolCtx<'_>, def: DefinitionId, name: &str, text: &str) -> StepOutcome {
        let decl = match Self::declare(name, text) {
            Ok(d) => d,
            Err(msg) => return StepOutcome::Reject(msg),
        };

        // 同名は置き換え、無ければ末尾へ足す。
        let Some(definition) = ctx.doc.definitions().get(def) else {
            return StepOutcome::Reject("コンポーネントが見つかりません".to_owned());
        };
        let mut params = definition.params.clone();
        match params.iter().position(|p| p.name == decl.name) {
            Some(i) => params[i] = decl,
            None => params.push(decl),
        }

        StepOutcome::Apply(Box::new(SetDefinitionParams::new("PARAM", def, params)))
    }

    /// 既定値の入力から型を決めて宣言を作る。
    fn declare(name: &str, text: &str) -> Result<ParamDecl, String> {
        let trimmed = text.trim();

        // ---- 選択肢 ----
        if trimmed.contains(CHOICE_SEPARATOR) {
            let options: Vec<String> = trimmed
                .split(CHOICE_SEPARATOR)
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect();
            if options.len() < 2 {
                return Err("選択肢は「引違い|開き」のように 2 つ以上並べてください".to_owned());
            }
            return ParamDecl::choice(name, options)
                .ok_or_else(|| "選択肢を作れませんでした".to_owned());
        }

        // ---- 式として解釈し、評価結果の型で決める ----
        let expr = parse(trimmed).map_err(|e| format!("既定値を解釈できません: {e}"))?;
        if let Some(first) = expr.referenced_vars().first() {
            return Err(format!(
                "既定値が他のパラメータ「{first}」を参照しています。\
                 先にそちらを宣言してください"
            ));
        }
        let value = eval(&expr, &Env::new()).map_err(|e| format!("既定値を評価できません: {e}"))?;

        let ty = match &value {
            Value::Number(_) => ParamType::Number,
            Value::Bool(_) => ParamType::Bool,
            Value::Choice(c) => ParamType::Choice(vec![c.clone()]),
        };
        Ok(ParamDecl {
            name: name.to_owned(),
            ty,
            default: expr,
            range: None,
        })
    }
}

// ---------------------------------------------------------------------------
// BIND — 座標に式を束縛する
// ---------------------------------------------------------------------------

/// 定義の中の座標に式を束縛する。
#[derive(Debug, Default)]
pub struct BindTool {
    state: BindState,
}

#[derive(Debug, Default, Clone, PartialEq)]
enum BindState {
    /// 最初の状態。
    ///
    /// **編集中なら図形をクリックで指す**。編集していなければ、
    /// 打たれた文字列を定義名として受けて従来の道へ落ちる。
    #[default]
    Pick,
    Entity {
        def: DefinitionId,
        listing: String,
    },
    SlotChoice {
        def: DefinitionId,
        entity: usize,
        listing: String,
    },
    /// 点を指したので X か Y かを選ぶ。
    ///
    /// `X` / `Y` は ASCII 1 文字なので**日本語入力を通さない**。
    Axis {
        def: DefinitionId,
        entity: usize,
        x: Slot,
        y: Slot,
        label: &'static str,
    },
    Expression {
        def: DefinitionId,
        entity: usize,
        slot: Slot,
        /// 何を束縛しているか（`終点X` など）。取り違えを防ぐために出す。
        target: String,
        params: String,
        /// 番号で選べるパラメータ名（宣言順）。
        names: Vec<String>,
    },
}

impl Tool for BindTool {
    fn name(&self) -> &'static str {
        "BIND"
    }

    /// 定義名・スロット名・式のいずれも、打ったままの文字列で受け取る。
    /// 図形を指す段階だけは対象外。
    fn wants_raw_text(&self) -> bool {
        !matches!(self.state, BindState::Pick)
    }

    fn prompt(&self) -> String {
        match &self.state {
            BindState::Pick => {
                "パラメータで動かしたい点をクリック（編集中でなければ名前を入力）:".to_owned()
            }
            BindState::Entity { listing, .. } => {
                format!("束縛する要素の番号を入力:\n{listing}")
            }
            BindState::SlotChoice { listing, .. } => {
                format!("束縛する座標の名前を入力:\n{listing}")
            }
            BindState::Axis { label, .. } => {
                format!("「{label}」のどちらを動かすか [X / Y]:")
            }
            BindState::Expression { target, params, .. } => {
                format!("「{target}」を動かす式を入力（番号で選べます）:\n\u{3000}{params}")
            }
        }
    }

    /// 編集中は図形をクリックで指す。
    fn wants_entity(&self) -> bool {
        matches!(self.state, BindState::Pick)
    }

    fn step(&mut self, input: StepInput, ctx: &ToolCtx<'_>) -> StepOutcome {
        // ---- クリックで指す（編集中）----
        if let StepInput::Entity { id, at } = input {
            return self.pick_by_click(ctx, id, at);
        }

        let text = match input {
            StepInput::Word(w) => w,
            StepInput::Number(n) => number_as_name(n),
            StepInput::Enter | StepInput::SelectionReady => return StepOutcome::Finish,
            StepInput::Point(_) => {
                return match self.state {
                    // 編集していないのに `BIND` を始めたら、定義名から辿る道へ落とす。
                    BindState::Pick => {
                        StepOutcome::Reject("図形の上をクリックしてください".to_owned())
                    }
                    _ => StepOutcome::Reject("文字で入力してください".to_owned()),
                };
            }
            StepInput::Entity { .. } => unreachable!("上で処理済み"),
        };

        match self.state.clone() {
            // 編集していないときは、最初の入力を定義名として受ける。
            BindState::Pick => self.pick_definition(ctx, &text),
            BindState::Entity { def, .. } => self.pick_entity(ctx, def, &text),
            BindState::SlotChoice { def, entity, .. } => self.pick_slot(ctx, def, entity, &text),
            BindState::Axis {
                def,
                entity,
                x,
                y,
                label,
            } => self.pick_axis(ctx, def, entity, x, y, label, &text),
            BindState::Expression {
                def,
                entity,
                slot,
                names,
                ..
            } => Self::commit(def, entity, slot, &names, &text),
        }
    }
}

impl BindTool {
    /// 編集中の図形をクリックして、そこのつまみを選ぶ。
    fn pick_by_click(&mut self, ctx: &ToolCtx<'_>, id: EntityId, at: Point2) -> StepOutcome {
        let Some(session) = ctx.editing else {
            return StepOutcome::Reject(
                "クリックで指せるのは編集中だけです（EDITCOMP で始めます）".to_owned(),
            );
        };
        let (members, origins) = session.members(ctx.doc);
        let Some(pos) = members.iter().position(|m| *m == id) else {
            return StepOutcome::Reject("編集中の図形をクリックしてください".to_owned());
        };
        let Some(Some(entity)) = origins.get(pos) else {
            return StepOutcome::Reject(
                "編集中に描いた図形はまだ束縛できません（ENDCOMP で確定してから）".to_owned(),
            );
        };
        let Some(geom) = ctx.doc.entities().get(id).map(|e| &e.geom) else {
            return StepOutcome::Reject("選択が古くなっています".to_owned());
        };
        let Some(handle) = nearest_handle(geom, at) else {
            return StepOutcome::Reject("この図形には動かせる点がありません".to_owned());
        };

        let def = session.definition();
        match handle {
            Handle::Point { x, y, label, .. } => {
                self.state = BindState::Axis {
                    def,
                    entity: *entity,
                    x,
                    y,
                    label,
                };
                StepOutcome::Continue
            }
            Handle::Scalar { slot, .. } => {
                self.ask_expression(ctx, def, *entity, slot, handle.label().to_owned())
            }
        }
    }

    /// 点のどちらの軸を動かすかを決める。
    #[allow(clippy::too_many_arguments)]
    fn pick_axis(
        &mut self,
        ctx: &ToolCtx<'_>,
        def: DefinitionId,
        entity: usize,
        x: Slot,
        y: Slot,
        label: &'static str,
        text: &str,
    ) -> StepOutcome {
        let slot = match text.trim().to_ascii_uppercase().as_str() {
            "X" => x,
            "Y" => y,
            other => {
                return StepOutcome::Reject(format!(
                    "「{label}」の X か Y を選んでください（入力: {other}）"
                ))
            }
        };
        // 「始点」＋「X」で「始点X」と見せる。
        self.ask_expression(
            ctx,
            def,
            entity,
            slot,
            format!("{label}{}", text.trim().to_ascii_uppercase()),
        )
    }

    /// 式の入力へ進む。使えるパラメータを番号つきで見せる。
    fn ask_expression(
        &mut self,
        ctx: &ToolCtx<'_>,
        def: DefinitionId,
        entity: usize,
        slot: Slot,
        target: String,
    ) -> StepOutcome {
        let Some(definition) = ctx.doc.definitions().get(def) else {
            return StepOutcome::Reject("コンポーネントが見つかりません".to_owned());
        };
        let names: Vec<String> = definition.params.iter().map(|p| p.name.clone()).collect();
        let params = if names.is_empty() {
            "（パラメータがありません。PARAM で宣言してください）".to_owned()
        } else {
            numbered_params(&names)
        };
        self.state = BindState::Expression {
            def,
            entity,
            slot,
            target,
            params,
            names,
        };
        StepOutcome::Continue
    }

    fn pick_definition(&mut self, ctx: &ToolCtx<'_>, name: &str) -> StepOutcome {
        let def = match find_definition(ctx.doc, name) {
            Ok(d) => d,
            Err(reject) => return reject,
        };
        let Some(definition) = ctx.doc.definitions().get(def) else {
            return StepOutcome::Reject("コンポーネントが見つかりません".to_owned());
        };
        if definition.entities.is_empty() {
            return StepOutcome::Reject("そのコンポーネントは空です".to_owned());
        }

        // 番号と種別を並べて見せる。**番号を覚えておく必要をなくす。**
        let listing = definition
            .entities
            .iter()
            .enumerate()
            .map(|(i, e)| format!("\u{3000}{i}: {}", e.geom.type_name()))
            .collect::<Vec<_>>()
            .join("\n");
        self.state = BindState::Entity { def, listing };
        StepOutcome::Continue
    }

    fn pick_entity(&mut self, ctx: &ToolCtx<'_>, def: DefinitionId, text: &str) -> StepOutcome {
        let Ok(index) = text.trim().parse::<usize>() else {
            return StepOutcome::Reject("要素の番号を数字で入力してください".to_owned());
        };
        let Some(definition) = ctx.doc.definitions().get(def) else {
            return StepOutcome::Reject("コンポーネントが見つかりません".to_owned());
        };
        let Some(entity) = definition.entities.get(index) else {
            return StepOutcome::Reject(format!(
                "要素 {index} はありません（0〜{}）",
                definition.entities.len().saturating_sub(1)
            ));
        };

        let slots = slots_for(&entity.geom);
        let listing = format!(
            "\u{3000}{}",
            slots
                .iter()
                .map(|s| slot_input_name(*s))
                .collect::<Vec<_>>()
                .join(" / ")
        );
        self.state = BindState::SlotChoice {
            def,
            entity: index,
            listing,
        };
        StepOutcome::Continue
    }

    fn pick_slot(
        &mut self,
        ctx: &ToolCtx<'_>,
        def: DefinitionId,
        entity: usize,
        text: &str,
    ) -> StepOutcome {
        let Some(definition) = ctx.doc.definitions().get(def) else {
            return StepOutcome::Reject("コンポーネントが見つかりません".to_owned());
        };
        let Some(target) = definition.entities.get(entity) else {
            return StepOutcome::Reject("要素が見つかりません".to_owned());
        };

        let wanted = text.trim();
        let Some(slot) = slots_for(&target.geom)
            .into_iter()
            .find(|s| slot_input_name(*s) == wanted)
        else {
            return StepOutcome::Reject(format!("「{wanted}」は選べません"));
        };

        self.ask_expression(ctx, def, entity, slot, slot_input_name(slot))
    }

    /// 式を受け取って束縛する。
    ///
    /// **数字だけならパラメータの番号として扱う。** 名前を打たずに済ませるため。
    fn commit(
        def: DefinitionId,
        entity: usize,
        slot: Slot,
        names: &[String],
        text: &str,
    ) -> StepOutcome {
        let trimmed = text.trim();

        // 数字 1 つならパラメータの番号。ASCII なので日本語入力を通さない。
        let source = match trimmed.parse::<usize>() {
            Ok(n) if n >= 1 && n <= names.len() => names[n - 1].clone(),
            Ok(n) if !names.is_empty() => {
                return StepOutcome::Reject(format!("番号は 1〜{} です（入力: {n}）", names.len()))
            }
            _ => trimmed.to_owned(),
        };

        match parse(&source) {
            Ok(expr) => StepOutcome::Apply(Box::new(SetBinding::new(
                "BIND",
                def,
                Binding::new(entity, slot, expr),
            ))),
            Err(e) => StepOutcome::Reject(format!("式を解釈できません: {e}")),
        }
    }
}

// ---------------------------------------------------------------------------
// PSET — インスタンスのパラメータを上書きする
// ---------------------------------------------------------------------------

/// インスタンスのパラメータを上書きする（`R` でリセット）。
#[derive(Debug, Default)]
pub struct ParamSetTool {
    state: PsetState,
}

#[derive(Debug, Default, Clone, PartialEq)]
enum PsetState {
    /// インスタンスをクリックで指す。
    #[default]
    Pick,
    Name {
        target: EntityId,
        listing: String,
    },
    Value {
        target: EntityId,
        param: String,
        hint: String,
    },
}

impl Tool for ParamSetTool {
    fn name(&self) -> &'static str {
        "PSET"
    }

    fn prompt(&self) -> String {
        match &self.state {
            PsetState::Pick => "パラメータを変えるインスタンスをクリック:".to_owned(),
            PsetState::Name { listing, .. } => {
                format!("パラメータ名を入力:\n{listing}")
            }
            PsetState::Value { param, hint, .. } => {
                format!("「{param}」の値を入力 <R でリセット>:\n\u{3000}{hint}")
            }
        }
    }

    fn wants_entity(&self) -> bool {
        matches!(self.state, PsetState::Pick)
    }

    /// インスタンスを指す段階以外は、打ったままの文字列で受け取る。
    fn wants_raw_text(&self) -> bool {
        !matches!(self.state, PsetState::Pick)
    }

    fn step(&mut self, input: StepInput, ctx: &ToolCtx<'_>) -> StepOutcome {
        if let StepInput::Entity { id, .. } = input {
            return self.pick_instance(ctx, id);
        }

        let text = match input {
            StepInput::Word(w) => w,
            StepInput::Number(n) => number_as_name(n),
            StepInput::Enter | StepInput::SelectionReady => return StepOutcome::Finish,
            StepInput::Point(_) => {
                return StepOutcome::Reject("インスタンスの上をクリックしてください".to_owned())
            }
            StepInput::Entity { .. } => unreachable!("上で処理済み"),
        };

        match self.state.clone() {
            PsetState::Pick => {
                StepOutcome::Reject("インスタンスの上をクリックしてください".to_owned())
            }
            PsetState::Name { target, .. } => self.pick_param(ctx, target, &text),
            PsetState::Value { target, param, .. } => Self::commit(ctx, target, &param, &text),
        }
    }
}

impl ParamSetTool {
    fn pick_instance(&mut self, ctx: &ToolCtx<'_>, id: EntityId) -> StepOutcome {
        let Some(entity) = ctx.doc.entities().get(id) else {
            return StepOutcome::Reject("選択が古くなっています".to_owned());
        };
        let Geometry::Instance(inst) = &entity.geom else {
            return StepOutcome::Reject(
                "コンポーネントのインスタンスをクリックしてください".to_owned(),
            );
        };
        let Some(def) = ctx.doc.definitions().get(inst.definition) else {
            return StepOutcome::Reject("コンポーネントが見つかりません".to_owned());
        };
        if def.params.is_empty() {
            return StepOutcome::Reject(format!(
                "「{}」にはパラメータがありません（PARAM で宣言してください）",
                def.name
            ));
        }

        // いまの値と、上書き中かどうかを見せる。
        let env = def.param_env(&inst.overrides);
        let listing = def
            .params
            .iter()
            .map(|p| {
                let value = env
                    .get(&p.name)
                    .map_or_else(|| "?".to_owned(), ToString::to_string);
                let mark = if inst.overrides.contains_key(&p.name) {
                    "＊"
                } else {
                    "　"
                };
                format!("\u{3000}{mark}{} = {value}", p.name)
            })
            .collect::<Vec<_>>()
            .join("\n");

        self.state = PsetState::Name {
            target: id,
            listing: format!("{listing}\n\u{3000}（＊ は上書き中）"),
        };
        StepOutcome::Continue
    }

    fn pick_param(&mut self, ctx: &ToolCtx<'_>, target: EntityId, name: &str) -> StepOutcome {
        let Some(decl) = Self::declaration(ctx, target, name.trim()) else {
            return StepOutcome::Reject(format!("「{}」というパラメータはありません", name.trim()));
        };

        let hint = match &decl.ty {
            ParamType::Number => decl.range.map_or_else(
                || "数値".to_owned(),
                |(lo, hi)| format!("数値（{lo} 〜 {hi}）"),
            ),
            ParamType::Bool => "真 / 偽".to_owned(),
            ParamType::Choice(options) => options.join(" / "),
        };
        self.state = PsetState::Value {
            target,
            param: decl.name,
            hint,
        };
        StepOutcome::Continue
    }

    /// 対象インスタンスの定義からパラメータの宣言を引く。
    fn declaration(ctx: &ToolCtx<'_>, target: EntityId, name: &str) -> Option<ParamDecl> {
        let entity = ctx.doc.entities().get(target)?;
        let Geometry::Instance(inst) = &entity.geom else {
            return None;
        };
        let def = ctx.doc.definitions().get(inst.definition)?;
        def.param(name).cloned()
    }

    fn commit(ctx: &ToolCtx<'_>, target: EntityId, param: &str, text: &str) -> StepOutcome {
        let trimmed = text.trim();

        // `R` でリセット（上書きを消して既定値へ戻す）。
        if trimmed.eq_ignore_ascii_case("R") {
            return StepOutcome::Apply(Box::new(SetInstanceOverride::reset("PSET", target, param)));
        }

        let Some(decl) = Self::declaration(ctx, target, param) else {
            return StepOutcome::Reject("パラメータが見つかりません".to_owned());
        };

        // 型に応じて値を作る。
        let value = match &decl.ty {
            ParamType::Choice(options) => {
                if options.iter().any(|o| o == trimmed) {
                    Value::Choice(trimmed.to_owned())
                } else {
                    return StepOutcome::Reject(format!("選べるのは: {}", options.join(" / ")));
                }
            }
            // 数値・真偽は式として解釈する（`900 / 2` のような入力を許す）。
            _ => match parse(trimmed).map(|e| eval(&e, &Env::new())) {
                Ok(Ok(v)) => v,
                Ok(Err(e)) => return StepOutcome::Reject(format!("値を評価できません: {e}")),
                Err(e) => return StepOutcome::Reject(format!("値を解釈できません: {e}")),
            },
        };

        if !decl.accepts(&value) {
            let why = match (&decl.ty, decl.range) {
                (ParamType::Number, Some((lo, hi))) => format!("{lo} 〜 {hi} の数値"),
                (ParamType::Number, None) => "数値".to_owned(),
                (ParamType::Bool, _) => "真 または 偽".to_owned(),
                (ParamType::Choice(o), _) => o.join(" / "),
            };
            return StepOutcome::Reject(format!("この値は使えません。必要なのは: {why}"));
        }

        StepOutcome::Apply(Box::new(SetInstanceOverride::set(
            "PSET", target, param, value,
        )))
    }
}
