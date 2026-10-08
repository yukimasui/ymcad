//! コンポーネントの道具（段階 1d）: `define_component` / `set_component_params` / `bind` /
//! `insert_component` / `set_instance_params`。
//!
//! 共通の約束は [`super::mutate`]（1 回の呼び出しで `apply` 1 回、検査は適用の前にすべて済ませる、
//! 失敗すれば履歴も版番号も変わらない、非表示・ロック中のレイヤの図形は呼び出しごと拒む）。
//!
//! # 検査は cad-core のコマンドと二重にする
//!
//! 式の型・参照先・循環・範囲は cad-core のコマンド（`SetDefinitionParams` / `SetBinding` /
//! `SetInstanceOverride`）も確かめるが、失敗の説明が「値がパラメータの型または範囲に合いません」の
//! ような固定の文で、**どのパラメータのどの値か**が分からない。LLM が読んで直せるよう、同じ検査を
//! 適用の前にここで行い、名前と値を入れて説明する。コマンドの検査は最後の防波堤として残る。
//!
//! # インスタンスを置くコマンド（[`PlaceInstance`]）
//!
//! `MacroCommand` の中では、前のコマンドが作った ID（`DefineComponent` の定義の ID、
//! `InsertInstance` の図形の ID）を後のコマンドへ渡せない。そこで
//!
//! - **定義は名前で引き直す**（アプリの `cad-app/src/tools/component.rs` の `InsertNewlyDefined` と同じ
//!   橋渡し。`define_component` の置き換え）
//! - **上書きは同じコマンドの中で、自分が作った図形の ID に掛ける**（`insert_component`）
//!
//! さらに、**redo では初回の図形を同じ ID のまま戻す**（`EditCtx::restore_entity`）。cad-core の
//! `InsertInstance` は適用のたびに新しいスロットに入れるので、そのままだと redo で ID が変わり、
//! その ID を指す後のコマンド（`set_instance_params` など）の redo が失敗する（Issue #92 と同じ根）。
//! 置いたインスタンスはこの道具の返り値の ID で後から触られるので、ここでは ID を保つ。

use std::collections::BTreeMap;

use cad_core::command::{
    DefineComponent, DeleteEntities, EditCtx, InsertInstance, SetBinding, SetDefinitionParams,
    SetInstanceOverride,
};
use cad_core::component::{param_cycle, Binding, ParamDecl, Placement};
use cad_core::expr::{eval, Env, Value as ParamValue};
use cad_core::{
    CadError, Command, Definition, DefinitionId, Document, Entity, EntityId, Geometry, Instance,
    LayerId,
};
use serde_json::{json, Map, Value};

use super::draw::{join_problems, object_list};
use super::mutate::{
    apply, created_since, destination_layer, editable_targets, ensure_capacity, id_strings,
    max_slot,
};
use super::{Args, Tool, ToolResult};
use crate::convert::{
    binding_to_json, check_extent, component_to_json, deg_to_rad, describe_param_type,
    expr_from_str, geometry_from_json, geometry_type, number_from_json, param_decl_from_json,
    param_value_from_json, param_value_to_json, point_from_json, slot_fields_hint, slot_from_name,
    slot_name, PARAM_TYPES,
};
use crate::ids::format_id;
use crate::limits::{
    MAX_COORDINATE, MAX_IDS_PER_CALL, MAX_NAME_CHARS, MAX_PARAMS, MAX_SHAPES_PER_CALL,
};
use crate::server::Server;

pub(super) const DEFINE_COMPONENT: Tool = Tool {
    name: "define_component",
    title: "コンポーネントを作る",
    description: "コンポーネント（パラメトリックなブロック）の定義を作る（1 回の呼び出しが undo 1 回ぶん）。\
中身は from_ids（図面にある図形の ID）か entities（add_entities と同じ形の図形の配列）のどちらか一方で渡す。\
origin は基点（配置するとこの点が配置先に来る）。中身の座標は図面と同じ座標で書く（origin からの相対ではない）。\n\
- from_ids: 既定（replace_with_instance: true）では、元の図形を消して同じ場所にインスタンスを 1 つ置き、見た目を変えない\
（アプリの COMPONENT と同じ。置いたインスタンスの ID は instance）。replace_with_instance: false なら元の図形は残し、定義だけを作る。\
非表示・ロック中のレイヤの図形があれば拒む\n\
- entities: 定義だけを作る（中身はレイヤ 0。置くのは insert_component）\n\
返り値の contents は定義の中の図形の添字（index）と種類で、bind の entity_index に使う。\
名前は前後の空白・制御文字なし・255 文字まで・既存のコンポーネントと重ならないこと。\
パラメータは set_component_params、座標への式の束縛は bind で足す。",
    schema: || {
        (
            json!({
                "name": {
                    "type": "string",
                    "minLength": 1,
                    "description": "コンポーネント名（図面の中で一意。前後の空白・制御文字なし）",
                },
                "origin": {
                    "description": "基点 {\"x\":..,\"y\":..} か [x, y]。配置するとこの点が配置先に来る",
                },
                "from_ids": {
                    "type": "array",
                    "items": { "type": "string" },
                    "minItems": 1,
                    "maxItems": MAX_IDS_PER_CALL,
                    "description": "中身にする図面の図形の ID（この順が contents の index になる）",
                },
                "entities": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": MAX_SHAPES_PER_CALL,
                    "items": { "type": "object" },
                    "description": "中身にする図形（add_entities と同じ形。instance で他のコンポーネントを入れ子にしてもよい）",
                },
                "replace_with_instance": {
                    "type": "boolean",
                    "description": "from_ids のとき、元の図形をインスタンス 1 つに置き換えるか（既定 true）",
                },
            }),
            &["name", "origin"],
        )
    },
    read_only: false,
    destructive: true,
    idempotent: false,
    run: define_component,
};

pub(super) const SET_COMPONENT_PARAMS: Tool = Tool {
    name: "set_component_params",
    title: "コンポーネントのパラメータを決める",
    description: "コンポーネントのパラメータの宣言を、params の内容に丸ごと置き換える（1 回の呼び出しが undo 1 回ぶん。\
渡さなかったパラメータは消える。足すときも今あるものを含めて渡す。今の宣言は list_components）。\
各要素は {name, type, default, min?, max?, options?}:\n\
- type: number（数値）/ bool（真偽）/ choice（選択）\n\
- default: number は数値か式の文字列（他のパラメータを参照してよい。例 \"幅 / 2\"。式の中の角度は度）、\
bool は true / false か式、choice は候補の名前そのもの（省くと最初の候補）\n\
- min, max: number の範囲（両端を含む。両方そろえて指定）\n\
- options: choice の候補の文字列の配列\n\
list_components の params の形（choices・range）もそのまま渡せる。\
名前は式の中でそのまま書ける語（英字・日本語・下線で始め、空白・記号なし）。\
既定値の型・範囲の違い、宣言されていない名前の参照、既定値どうしの循環、束縛が使っているパラメータを消す・数値でなくすことは拒む。\
インスタンスの上書きが新しい宣言に合わなくなるときは warnings に出す（その上書きは使われず既定値になる）。",
    schema: || {
        (
            json!({
                "component": { "type": "string", "description": "コンポーネント名" },
                "params": {
                    "type": "array",
                    "maxItems": MAX_PARAMS,
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": { "type": "string" },
                            "type": { "type": "string", "enum": PARAM_TYPES },
                            "default": { "description": "既定値（数値・真偽・式の文字列・選択肢の名前）" },
                            "min": { "description": "number の下限（数値か式）" },
                            "max": { "description": "number の上限（数値か式）" },
                            "options": { "type": "array", "items": { "type": "string" }, "description": "choice の候補" },
                        },
                        "required": ["name", "type"],
                    },
                    "description": "パラメータの宣言の全部（宣言順がパネルの表示順）。空の配列で全部消す",
                },
            }),
            &["component", "params"],
        )
    },
    read_only: false,
    destructive: true,
    idempotent: true,
    run: set_component_params,
};

pub(super) const BIND: Tool = Tool {
    name: "bind",
    title: "座標に式を束縛する",
    description: "コンポーネントの定義の中の図形の 1 項目に、パラメータの式を束縛する（1 回の呼び出しが undo 1 回ぶん。\
同じ項目に束縛があれば置き換える）。entity_index は定義の中の図形の添字（define_component の contents、\
list_components の contents: true で見られる）。slot は図形の JSON の項目名で、list_components の bindings の field と同じ綴り:\n\
line: start.x, start.y, end.x, end.y / circle: center.x, center.y, radius / arc: center.x, center.y, radius, start_angle, end_angle / \
xline: origin.x, origin.y, angle / polyline: vertices[i].x, vertices[i].y / instance: origin.x, origin.y, rotation, scale\n\
座標は定義の中の座標（define_component で渡した座標のまま）。expr は数値になる式（例 \"幅 / 2\"、\"if 開く then 90 else 0\"）。\
角度の項目（start_angle・end_angle・angle・rotation）は度。宣言されていないパラメータの参照・数値にならない式は拒む。\
既定値で図形が成り立たない（半径が 0 以下など）ときは warnings に出す（そのインスタンスではその項目が定義のままになる）。",
    schema: || {
        (
            json!({
                "component": { "type": "string", "description": "コンポーネント名" },
                "entity_index": { "type": "integer", "minimum": 0, "description": "定義の中の図形の添字（0 から）" },
                "slot": { "type": "string", "description": "束縛する項目（start.x・radius・vertices[2].y など）" },
                "expr": { "description": "式の文字列（数値も可）。例 \"幅 / 2\"" },
            }),
            &["component", "entity_index", "slot", "expr"],
        )
    },
    read_only: false,
    destructive: true,
    idempotent: true,
    run: bind,
};

pub(super) const INSERT_COMPONENT: Tool = Tool {
    name: "insert_component",
    title: "コンポーネントを置く",
    description: "コンポーネントのインスタンスを 1 つ置く（パラメータの上書きを含めて 1 回の呼び出しが undo 1 回ぶん）。\
origin に定義の基点が来る。rotation_deg（度、反時計回り。既定 0）・scale（0 より大きい。既定 1）・flipped（鏡像。既定 false）。\
layer を省くと現在レイヤ（非表示・ロック中のレイヤには置けない）。\
params は {パラメータ名: 値}: number は数値か式の文字列（パラメータは使えない）、bool は true / false、choice は候補の名前。\
型・範囲・候補に合わない値は拒む。省いたパラメータは既定値。返り値の id が置いたインスタンス（undo → redo でも同じ ID）。",
    schema: || {
        (
            json!({
                "component": { "type": "string", "description": "コンポーネント名" },
                "origin": { "description": "置く位置 {\"x\":..,\"y\":..} か [x, y]" },
                "rotation_deg": { "description": "回転（度、反時計回り。数値か式。既定 0）" },
                "scale": { "description": "倍率（0 より大きい。数値か式。既定 1）" },
                "flipped": { "type": "boolean", "description": "鏡像にするか（既定 false）" },
                "layer": { "type": "string", "description": "置くレイヤ（省略時は現在レイヤ）" },
                "params": { "type": "object", "description": "パラメータの上書き {名前: 値}" },
            }),
            &["component", "origin"],
        )
    },
    read_only: false,
    destructive: false,
    idempotent: false,
    run: insert_component,
};

pub(super) const SET_INSTANCE_PARAMS: Tool = Tool {
    name: "set_instance_params",
    title: "インスタンスのパラメータを変える",
    description: "置いたインスタンス（id）のパラメータの上書きを変える（1 回の呼び出しが undo 1 回ぶん）。\
values は {パラメータ名: 値}。値を null にするとその上書きを消して既定値へ戻す。\
number は数値か式の文字列（パラメータは使えない）、bool は true / false、choice は候補の名前。\
型・範囲・候補に合わない値、宣言されていない名前、非表示・ロック中のレイヤのインスタンスは拒む（1 つでもあれば何も変えない）。\
返り値の params は変えた後の全パラメータの値。",
    schema: || {
        (
            json!({
                "id": { "type": "string", "description": "インスタンスの図形 ID" },
                "values": { "type": "object", "description": "{パラメータ名: 値 | null}" },
            }),
            &["id", "values"],
        )
    },
    read_only: false,
    destructive: true,
    idempotent: true,
    run: set_instance_params,
};

// ---- 共通 ------------------------------------------------------------------

/// 名前からコンポーネントを引く。
fn find_component(doc: &Document, name: &str) -> Result<DefinitionId, String> {
    doc.definitions().by_name(name).ok_or_else(|| {
        let names: Vec<&str> = doc
            .definitions()
            .iter()
            .map(|(_, d)| d.name.as_str())
            .collect();
        if names.is_empty() {
            format!("コンポーネント {name} はありません（まだ 1 つもありません。作るなら define_component）")
        } else {
            format!(
                "コンポーネント {name} はありません（あるのは {}）",
                names.join(", ")
            )
        }
    })
}

fn definition(doc: &Document, id: DefinitionId) -> Result<&Definition, String> {
    doc.definitions()
        .get(id)
        .ok_or_else(|| "コンポーネントの定義が見つかりません".to_owned())
}

/// 図面に直接置かれた、`def` のインスタンスの数。
fn instance_count(doc: &Document, def: DefinitionId) -> usize {
    doc.entities()
        .iter()
        .filter(|(_, e)| matches!(&e.geom, Geometry::Instance(i) if i.definition == def))
        .count()
}

/// コンポーネントの JSON（`list_components` と同じ形）。
fn component_json(doc: &Document, def: DefinitionId) -> Value {
    doc.definitions().get(def).map_or(Value::Null, |d| {
        component_to_json(d, instance_count(doc, def))
    })
}

/// コンポーネント名として使えるか。レイヤ名と同じ規則（黙って直さずに拒む）。
fn check_component_name(doc: &Document, name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("コンポーネント名が空です".to_owned());
    }
    if name.trim() != name {
        return Err(format!(
            "コンポーネント名 {name:?} の前後に空白があります（空白を除いて指定してください）"
        ));
    }
    if name.chars().any(char::is_control) {
        return Err(format!(
            "コンポーネント名 {name:?} に改行などの制御文字が含まれています"
        ));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(format!(
            "コンポーネント名が長すぎます（上限 {MAX_NAME_CHARS} 文字）"
        ));
    }
    if doc.definitions().by_name(name).is_some() {
        return Err(format!(
            "コンポーネント {name} は既にあります（list_components で一覧を見られます）"
        ));
    }
    Ok(())
}

/// パラメータの値の一覧（宣言の名前 → いまの値）。上書きを優先し、無ければ既定値。
fn effective_params(def: &Definition, overrides: &BTreeMap<String, ParamValue>) -> Value {
    let env = def.param_env(overrides);
    let out: Map<String, Value> = def
        .params
        .iter()
        .map(|p| {
            (
                p.name.clone(),
                env.get(&p.name).map_or(Value::Null, param_value_to_json),
            )
        })
        .collect();
    Value::Object(out)
}

fn overrides_json(overrides: &BTreeMap<String, ParamValue>) -> Value {
    Value::Object(
        overrides
            .iter()
            .map(|(k, v)| (k.clone(), param_value_to_json(v)))
            .collect(),
    )
}

/// インスタンスの上書きが定義の宣言に合うか（入れ子にするインスタンスの検査）。
fn check_overrides(
    def: &Definition,
    overrides: &BTreeMap<String, ParamValue>,
    what: &str,
) -> Result<(), String> {
    for (k, v) in overrides {
        let decl = def.param(k).ok_or_else(|| {
            format!(
                "{what}: コンポーネント {} にパラメータ {k} はありません",
                def.name
            )
        })?;
        if !decl.accepts(v) {
            return Err(format!(
                "{what}: 上書き {k} = {v} はパラメータ {k} の {} に合いません",
                describe_param_type(decl)
            ));
        }
    }
    Ok(())
}

/// 置くインスタンスの形を確かめる（`validate` と大きさの上限）。
fn check_instance(
    doc: &Document,
    def: DefinitionId,
    placement: Placement,
    overrides: &BTreeMap<String, ParamValue>,
) -> Result<(), String> {
    let mut inst = Instance::new(def, placement);
    inst.overrides = overrides.clone();
    let g = Geometry::Instance(inst);
    g.validate().map_err(|e| e.to_string())?;
    check_extent(&g, doc.definitions())
}

/// 点が座標の上限の内か。
fn check_point(p: cad_core::geom::Point2, what: &str) -> Result<(), String> {
    if p.x.abs() <= MAX_COORDINATE && p.y.abs() <= MAX_COORDINATE {
        Ok(())
    } else {
        Err(format!(
            "{what} の座標が上限（絶対値 {MAX_COORDINATE:e}）を超えます"
        ))
    }
}

/// `{パラメータ名: 値}` を読む。`allow_reset` なら `null` を「上書きを消す」（`None`）として受ける。
fn read_values(
    def: &Definition,
    v: &Value,
    key: &str,
    allow_reset: bool,
) -> Result<Vec<(String, Option<ParamValue>)>, String> {
    let m = v
        .as_object()
        .ok_or_else(|| format!("{key} は {{パラメータ名: 値}} のオブジェクトで指定してください"))?;
    if m.len() > MAX_PARAMS {
        return Err(format!("{key} が多すぎます（上限 {MAX_PARAMS} 個）"));
    }
    let declared: Vec<&str> = def.params.iter().map(|p| p.name.as_str()).collect();
    let mut out = Vec::with_capacity(m.len());
    let mut problems = Vec::new();
    for (name, value) in m {
        let Some(decl) = def.param(name) else {
            problems.push(if declared.is_empty() {
                format!(
                    "{key}.{name}: コンポーネント {} にはパラメータがありません（宣言は set_component_params）",
                    def.name
                )
            } else {
                format!(
                    "{key}.{name}: コンポーネント {} にパラメータ {name} はありません（あるのは {}）",
                    def.name,
                    declared.join(", ")
                )
            });
            continue;
        };
        if value.is_null() {
            if allow_reset {
                out.push((name.clone(), None));
            } else {
                problems.push(format!(
                    "{key}.{name}: null は使えません（既定値のままにするなら書かずに省いてください）"
                ));
            }
            continue;
        }
        match param_value_from_json(decl, value, &format!("{key}.{name}")) {
            Ok(v) => out.push((name.clone(), Some(v))),
            Err(e) => problems.push(e),
        }
    }
    join_problems(
        problems,
        "1 つでも使えない値があるので、何も変えていません。",
    )?;
    Ok(out)
}

// ---- インスタンスを置くコマンド ---------------------------------------------

/// どの定義を置くか。
#[derive(Debug)]
enum Target {
    /// 既にある定義（`insert_component`）。定義の ID は undo / redo で変わらない
    /// （`DefineComponent` も `DeleteDefinition` も元の ID で戻す）。
    Id(DefinitionId),
    /// 同じ `MacroCommand` の前のコマンドが作る定義（`define_component` の置き換え）。
    /// 作られるまで ID が分からないので、名前で引き直す（アプリの `InsertNewlyDefined` と同じ橋渡し）。
    Named(String),
}

/// インスタンスを 1 つ置き、パラメータの上書きを掛ける 1 つのコマンド（モジュールドキュメント）。
///
/// - 初回: `InsertInstance` で置き、作った図形の ID に `SetInstanceOverride` を順に掛ける。
///   途中で失敗したら置いた図形を外して失敗する（「全部成功するか、何も変えずに失敗するか」）
/// - 取り消し: 置いた図形を外して取っておく
/// - やり直し: 名前（または ID）で引き直した定義が、取っておいた図形の参照先と同じことを確かめ、
///   **初回と同じ ID で**戻す
///
/// 図形を変える経路は `EditCtx` だけ（設計原則 4）。cad-core のコマンドを中で使うのは、検査
/// （定義があるか・上書きの型と範囲）を cad-core と同じにするため。
#[derive(Debug)]
struct PlaceInstance {
    name: &'static str,
    target: Target,
    placement: Placement,
    layer: LayerId,
    overrides: Vec<(String, ParamValue)>,
    /// 初回に置いた図形の ID。取り消しでも捨てない（やり直しで同じ ID に戻すため）。
    placed: Option<EntityId>,
    /// 取り消しで外した図形。やり直しで `placed` の ID のまま戻す。
    stashed: Option<Entity>,
}

impl PlaceInstance {
    fn new(
        name: &'static str,
        target: Target,
        placement: Placement,
        layer: LayerId,
        overrides: Vec<(String, ParamValue)>,
    ) -> Self {
        Self {
            name,
            target,
            placement,
            layer,
            overrides,
            placed: None,
            stashed: None,
        }
    }

    fn definition(&self, ctx: &EditCtx<'_>) -> cad_core::Result<DefinitionId> {
        let id = match &self.target {
            Target::Id(id) => *id,
            Target::Named(name) => ctx
                .definitions()
                .by_name(name)
                .ok_or(CadError::DefinitionNotFound)?,
        };
        if ctx.definitions().get(id).is_none() {
            return Err(CadError::DefinitionNotFound);
        }
        Ok(id)
    }
}

impl Command for PlaceInstance {
    fn execute(&mut self, ctx: &mut EditCtx<'_>) -> cad_core::Result<()> {
        let def = self.definition(ctx)?;

        // やり直し: 取っておいた図形を同じ ID で戻す。
        if let (Some(id), Some(entity)) = (self.placed, self.stashed.as_ref()) {
            let same = matches!(&entity.geom, Geometry::Instance(i) if i.definition == def);
            if !same {
                return Err(CadError::DefinitionNotFound);
            }
            ctx.restore_entity(id, entity.clone())?;
            self.stashed = None;
            return Ok(());
        }

        // 初回。
        let mut insert = InsertInstance::new(self.name, def, self.placement, self.layer);
        insert.execute(ctx)?;
        let Some(id) = insert.created() else {
            return Err(CadError::EntityNotFound);
        };
        for (param, value) in &self.overrides {
            let mut set = SetInstanceOverride::set(self.name, id, param.clone(), value.clone());
            if let Err(e) = set.execute(ctx) {
                // 上書きは置いた図形にしか掛けていないので、図形を外せば元どおり。
                let _ = insert.undo(ctx);
                return Err(e);
            }
        }
        self.placed = Some(id);
        Ok(())
    }

    fn undo(&mut self, ctx: &mut EditCtx<'_>) -> cad_core::Result<()> {
        if let Some(id) = self.placed {
            self.stashed = Some(ctx.remove_entity(id)?);
        }
        Ok(())
    }

    fn name(&self) -> &'static str {
        self.name
    }
}

// ---- define_component --------------------------------------------------------

/// 中身が全部同じレイヤなら、そのレイヤ（アプリの `single_layer` と同じ）。
fn single_layer(contents: &[Entity]) -> Option<LayerId> {
    let first = contents.first()?.layer;
    contents.iter().all(|e| e.layer == first).then_some(first)
}

fn define_component(s: &mut Server, a: &Args) -> ToolResult {
    let name = a.req_str("name")?.to_owned();
    check_component_name(&s.doc, &name)?;
    let origin = point_from_json(
        a.value("origin")
            .ok_or_else(|| "origin（基点）を指定してください".to_owned())?,
        "origin",
    )?;
    check_point(origin, "origin")?;

    match (a.value("from_ids"), a.value("entities")) {
        (Some(_), None) => define_from_ids(s, a, name, origin),
        (None, Some(_)) => define_from_entities(s, a, name, origin),
        _ => Err(
            "中身は from_ids（図面の図形の ID）か entities（図形の配列）のどちらか一方で指定してください"
                .to_owned(),
        ),
    }
}

fn define_from_ids(
    s: &mut Server,
    a: &Args,
    name: String,
    origin: cad_core::geom::Point2,
) -> ToolResult {
    let raw = a.string_list("from_ids", MAX_IDS_PER_CALL)?;
    let replace = a.bool_or("replace_with_instance", true)?;
    let targets = editable_targets(s, &raw)?;
    // 定義の中身は図形の複製（レイヤ・色もそのまま。アプリの COMPONENT と同じ）。
    let contents: Vec<Entity> = targets
        .iter()
        .filter_map(|id| s.doc.entities().get(*id).cloned())
        .collect();
    let types: Vec<&'static str> = contents.iter().map(|e| geometry_type(&e.geom)).collect();

    let mut commands: Vec<Box<dyn Command>> = vec![Box::new(DefineComponent::new(
        "COMPONENT",
        name.clone(),
        origin,
        contents.clone(),
    ))];
    if replace {
        // 置き換えるインスタンスは、中身が載っていたレイヤへ。散っていれば現在レイヤ（アプリと同じ）。
        let layer = match single_layer(&contents) {
            Some(l) => l,
            None => destination_layer(s, None)?,
        };
        commands.push(Box::new(DeleteEntities::new("COMPONENT", targets.clone())));
        commands.push(Box::new(PlaceInstance::new(
            "COMPONENT",
            Target::Named(name.clone()),
            Placement::at(origin),
            layer,
            Vec::new(),
        )));
    }

    let before = max_slot(&s.doc);
    apply(s, "COMPONENT", commands)?;
    let instance = if replace {
        let created = created_since(&s.doc, before);
        debug_assert_eq!(created.len(), 1);
        created.first().map(|id| format_id(s.tag(), *id))
    } else {
        None
    };
    let contents_json: Vec<Value> = types
        .iter()
        .zip(&raw)
        .enumerate()
        .map(|(i, (ty, id))| json!({ "index": i, "type": ty, "from_id": id }))
        .collect();
    Ok(json!({
        "drawing": s.tag().name(),
        "component": name,
        "entity_count": contents_json.len(),
        "contents": contents_json,
        "instance": instance,
        "removed": if replace { raw.len() } else { 0 },
    }))
}

fn define_from_entities(
    s: &mut Server,
    a: &Args,
    name: String,
    origin: cad_core::geom::Point2,
) -> ToolResult {
    if a.bool_or("replace_with_instance", false)? {
        return Err(
            "replace_with_instance は from_ids のときだけ使えます（entities で作った定義を置くなら、続けて insert_component）"
                .to_owned(),
        );
    }
    let items = object_list(a, "entities", MAX_SHAPES_PER_CALL)?;
    let defs = s.doc.definitions();
    let mut contents = Vec::with_capacity(items.len());
    let mut problems = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let what = format!("entities[{i}]");
        let built = geometry_from_json(&Value::Object((*item).clone()), defs)
            .and_then(|g| check_extent(&g, defs).map(|()| g))
            .and_then(|g| match &g {
                // 入れ子のインスタンスの上書きも、定義の宣言に合うこと（合わないと黙って捨てられる）。
                Geometry::Instance(inst) => {
                    let def = defs
                        .get(inst.definition)
                        .ok_or_else(|| "コンポーネントの定義が見つかりません".to_owned())?;
                    check_overrides(def, &inst.overrides, "overrides").map(|()| g)
                }
                _ => Ok(g),
            });
        match built {
            Ok(g) => contents.push(Entity::new(g, LayerId::ZERO)),
            Err(e) => problems.push(format!("{what}: {e}")),
        }
    }
    join_problems(
        problems,
        "1 つでも使えない図形があるので、何も作っていません。",
    )?;
    let types: Vec<&'static str> = contents.iter().map(|e| geometry_type(&e.geom)).collect();

    apply(
        s,
        "COMPONENT",
        vec![Box::new(DefineComponent::new(
            "COMPONENT",
            name.clone(),
            origin,
            contents,
        ))],
    )?;
    let contents_json: Vec<Value> = types
        .iter()
        .enumerate()
        .map(|(i, ty)| json!({ "index": i, "type": ty }))
        .collect();
    Ok(json!({
        "drawing": s.tag().name(),
        "component": name,
        "entity_count": contents_json.len(),
        "contents": contents_json,
        "instance": Value::Null,
        "removed": 0,
    }))
}

// ---- set_component_params ------------------------------------------------------

fn set_component_params(s: &mut Server, a: &Args) -> ToolResult {
    let def_id = find_component(&s.doc, a.req_str("component")?)?;
    let list = a
        .value("params")
        .ok_or_else(|| "params を指定してください（全部消すなら空の配列）".to_owned())?
        .as_array()
        .ok_or_else(|| "params は宣言の配列で指定してください".to_owned())?;
    if list.len() > MAX_PARAMS {
        return Err(format!("params が多すぎます（上限 {MAX_PARAMS} 個）"));
    }
    let mut params = Vec::with_capacity(list.len());
    let mut problems = Vec::new();
    for (i, v) in list.iter().enumerate() {
        match param_decl_from_json(v, &format!("params[{i}]")) {
            Ok(p) => params.push(p),
            Err(e) => problems.push(e),
        }
    }
    join_problems(
        problems,
        "1 つでも読めない宣言があるので、何も変えていません。",
    )?;
    let def = definition(&s.doc, def_id)?;
    check_params(def, &params)?;
    let warnings = override_warnings(s, def_id, &params);

    apply(
        s,
        "PARAM",
        vec![Box::new(SetDefinitionParams::new("PARAM", def_id, params))],
    )?;
    Ok(json!({
        "drawing": s.tag().name(),
        "component": component_json(&s.doc, def_id),
        "warnings": warnings,
    }))
}

/// 宣言の全体を確かめる（`SetDefinitionParams` と同じ検査を、名前と値を入れた説明で）。
fn check_params(def: &Definition, params: &[ParamDecl]) -> Result<(), String> {
    let declared: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
    for (i, p) in params.iter().enumerate() {
        if declared[..i].contains(&p.name.as_str()) {
            return Err(format!("パラメータ名 {} が重複しています", p.name));
        }
    }
    for p in params {
        for used in p.default.referenced_vars() {
            if !declared.contains(&used) {
                return Err(format!(
                    "パラメータ {} の既定値 {} が、宣言されていない {used} を参照しています（宣言の一覧: {}）",
                    p.name,
                    p.default,
                    declared.join(", ")
                ));
            }
        }
    }
    if let Some(name) = param_cycle(params) {
        return Err(format!(
            "パラメータの既定値どうしが循環しています（{name} を含む輪）。どれかを数値などの定数にしてください"
        ));
    }

    // 既定値が宣言した型と範囲に収まること（他のパラメータを参照する既定値は、参照先の既定値で計算する）。
    let mut probe = Definition::new(def.name.clone(), def.origin, Vec::new());
    probe.params = params.to_vec();
    let env = probe.param_env(&BTreeMap::new());
    for p in params {
        if env.get(&p.name).is_some_and(|v| p.accepts(v)) {
            continue;
        }
        // param_env は合わない値を捨てるので、理由を知るために計算し直す。
        return Err(match eval(&p.default, &env) {
            Ok(v) => format!(
                "パラメータ {} の既定値 {} の値 {v} が {} に合いません",
                p.name,
                p.default,
                describe_param_type(p)
            ),
            Err(e) => format!(
                "パラメータ {} の既定値 {} を計算できません: {e}",
                p.name, p.default
            ),
        });
    }

    // 束縛が参照しているパラメータを消さない・数値でなくさないこと。
    for b in &def.bindings {
        let at = format!("束縛（entity {} の {}）", b.entity, slot_name(b.slot));
        for used in b.expr.referenced_vars() {
            if !declared.contains(&used) {
                return Err(format!(
                    "{at} の式 {} が {used} を使っているので、{used} は消せません",
                    b.expr
                ));
            }
        }
        match eval(&b.expr, &env) {
            Ok(ParamValue::Number(_)) => {}
            Ok(v) => {
                return Err(format!(
                    "この宣言にすると {at} の式 {} の値が数値でなくなります（{}）",
                    b.expr,
                    v.type_name()
                ))
            }
            Err(e) => {
                return Err(format!(
                    "この宣言にすると {at} の式 {} を計算できません: {e}",
                    b.expr
                ))
            }
        }
    }
    Ok(())
}

/// 新しい宣言で使われなくなるインスタンスの上書き（図面に直接置かれたものと、他の定義の中のもの）。
fn override_warnings(s: &Server, def: DefinitionId, params: &[ParamDecl]) -> Vec<String> {
    /// 多すぎると結果が膨らむので、ここで打ち切る。
    const MAX_WARNINGS: usize = 50;
    let check = |overrides: &BTreeMap<String, ParamValue>, at: &str, out: &mut Vec<String>| {
        for (k, v) in overrides {
            match params.iter().find(|p| &p.name == k) {
                None => out.push(format!(
                    "{at} の上書き {k} = {v} は、宣言から {k} が消えるので使われません"
                )),
                Some(p) if !p.accepts(v) => out.push(format!(
                    "{at} の上書き {k} = {v} は、新しい宣言（{}）に合わないので使われず既定値になります",
                    describe_param_type(p)
                )),
                Some(_) => {}
            }
        }
    };
    let mut out = Vec::new();
    for (id, e) in s.doc.entities().iter() {
        if let Geometry::Instance(i) = &e.geom {
            if i.definition == def {
                check(
                    &i.overrides,
                    &format!("インスタンス {}", format_id(s.tag(), id)),
                    &mut out,
                );
            }
        }
    }
    for (_, d) in s.doc.definitions().iter() {
        for (index, e) in d.entities.iter().enumerate() {
            if let Geometry::Instance(i) = &e.geom {
                if i.definition == def {
                    check(
                        &i.overrides,
                        &format!(
                            "コンポーネント {} の中のインスタンス（index {index}）",
                            d.name
                        ),
                        &mut out,
                    );
                }
            }
        }
    }
    if out.len() > MAX_WARNINGS {
        let rest = out.len() - MAX_WARNINGS;
        out.truncate(MAX_WARNINGS);
        out.push(format!("ほか {rest} 件"));
    }
    out
}

// ---- bind ----------------------------------------------------------------------

fn bind(s: &mut Server, a: &Args) -> ToolResult {
    let def_id = find_component(&s.doc, a.req_str("component")?)?;
    let def = definition(&s.doc, def_id)?;
    let index = a
        .value("entity_index")
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(|| "entity_index を 0 以上の整数で指定してください".to_owned())?;
    let entity = def.entities.get(index).ok_or_else(|| {
        format!(
            "entity_index {index} は範囲の外です（コンポーネント {} の中の図形は {} 個で、添字は 0〜{}。\
             中身は list_components の contents: true で見られます）",
            def.name,
            def.entities.len(),
            def.entities.len().saturating_sub(1)
        )
    })?;
    let ty = geometry_type(&entity.geom);
    let field = a.req_str("slot")?;
    let slot = slot_from_name(ty, field)
        .filter(|slot| slot.fits(&entity.geom))
        .ok_or_else(|| {
            format!(
                "entity {index}（{ty}）の項目 {field:?} には束縛できません（使えるのは {}）",
                slot_fields_hint(&entity.geom)
            )
        })?;
    let expr = match a.value("expr") {
        Some(Value::String(src)) => expr_from_str(src, "expr")?,
        Some(v @ Value::Number(_)) => cad_core::expr::Expr::number(number_from_json(v, "expr")?),
        _ => return Err("expr を式の文字列で指定してください（例 \"幅 / 2\"）".to_owned()),
    };

    let declared: Vec<&str> = def.params.iter().map(|p| p.name.as_str()).collect();
    for used in expr.referenced_vars() {
        if !declared.contains(&used) {
            return Err(if declared.is_empty() {
                format!(
                    "式 {expr} が {used} を参照していますが、コンポーネント {} にはパラメータがありません\
                     （先に set_component_params で宣言してください）",
                    def.name
                )
            } else {
                format!(
                    "式 {expr} が宣言されていない {used} を参照しています（宣言されているのは {}。足すなら set_component_params）",
                    declared.join(", ")
                )
            });
        }
    }
    let env: Env = def.param_env(&BTreeMap::new());
    let value = match eval(&expr, &env) {
        Ok(ParamValue::Number(n)) => n,
        Ok(v) => {
            return Err(format!(
                "式 {expr} の値が数値ではありません（既定のパラメータで {v}、{}）",
                v.type_name()
            ))
        }
        Err(e) => return Err(format!("式 {expr} を既定のパラメータで計算できません: {e}")),
    };

    // 既定のパラメータで、束縛を全部掛けた形が成り立つか（成り立たなくても拒まない。
    // その項目が定義のままになるだけで、他の値では成り立つかもしれないため）。
    let replaced = def
        .bindings
        .iter()
        .any(|b| b.entity == index && b.slot == slot);
    let binding = Binding::new(index, slot, expr);
    let mut warnings = Vec::new();
    if !value.is_finite() || (slot.must_be_positive() && value <= 0.0) {
        warnings.push(format!(
            "既定のパラメータでは {field} が {value} になり、使えない値なので定義のままになります（0 より大きい値が要ります）"
        ));
    } else {
        let mut probe = def.clone();
        probe
            .bindings
            .retain(|b| !(b.entity == index && b.slot == slot));
        probe.bindings.push(binding.clone());
        if let Some(e) = probe.evaluated_entities(&env).get(index) {
            if let Err(err) = e.geom.validate() {
                warnings.push(format!(
                    "既定のパラメータでは entity {index} の形が成り立ちません（{err}）"
                ));
            }
        }
    }

    apply(
        s,
        "BIND",
        vec![Box::new(SetBinding::new("BIND", def_id, binding.clone()))],
    )?;
    let def = definition(&s.doc, def_id)?;
    Ok(json!({
        "drawing": s.tag().name(),
        "component": def.name,
        "binding": binding_to_json(&binding, def),
        "value": value,
        "replaced": replaced,
        "warnings": warnings,
    }))
}

// ---- insert_component ------------------------------------------------------------

fn insert_component(s: &mut Server, a: &Args) -> ToolResult {
    let def_id = find_component(&s.doc, a.req_str("component")?)?;
    let origin = point_from_json(
        a.value("origin")
            .ok_or_else(|| "origin（置く位置）を指定してください".to_owned())?,
        "origin",
    )?;
    check_point(origin, "origin")?;
    let rotation = match a.value("rotation_deg") {
        None => 0.0,
        Some(v) => deg_to_rad(number_from_json(v, "rotation_deg")?),
    };
    let scale = match a.value("scale") {
        None => 1.0,
        Some(v) => number_from_json(v, "scale")?,
    };
    if scale <= 0.0 {
        return Err(format!(
            "scale は 0 より大きい数で指定してください（受け取った値: {scale}。裏返すなら flipped: true）"
        ));
    }
    let flipped = a.bool_or("flipped", false)?;
    let placement =
        Placement::new(origin, rotation, scale, flipped).map_err(|e| format!("配置: {e}"))?;
    let layer = destination_layer(s, a.opt_str("layer")?)?;

    let def = definition(&s.doc, def_id)?;
    let values = match a.value("params") {
        None => Vec::new(),
        Some(v) => read_values(def, v, "params", false)?,
    };
    let overrides: Vec<(String, ParamValue)> = values
        .into_iter()
        .filter_map(|(k, v)| v.map(|v| (k, v)))
        .collect();
    let map: BTreeMap<String, ParamValue> = overrides.iter().cloned().collect();
    check_instance(&s.doc, def_id, placement, &map)?;
    ensure_capacity(&s.doc, 1)?;

    let before = max_slot(&s.doc);
    apply(
        s,
        "INSERT",
        vec![Box::new(PlaceInstance::new(
            "INSERT",
            Target::Id(def_id),
            placement,
            layer,
            overrides,
        ))],
    )?;
    let created = created_since(&s.doc, before);
    debug_assert_eq!(created.len(), 1);
    let def = definition(&s.doc, def_id)?;
    Ok(json!({
        "drawing": s.tag().name(),
        "id": id_strings(s, &created).into_iter().next(),
        "component": def.name,
        "layer": s.doc.layers().get(layer).map(|l| l.name.as_str()),
        "overrides": overrides_json(&map),
        "params": effective_params(def, &map),
    }))
}

// ---- set_instance_params ----------------------------------------------------------

fn set_instance_params(s: &mut Server, a: &Args) -> ToolResult {
    let raw = a.req_str("id")?.to_owned();
    let target = editable_targets(s, std::slice::from_ref(&raw))?[0];
    let entity = s
        .doc
        .entities()
        .get(target)
        .ok_or_else(|| format!("ID {raw} の図形はありません"))?;
    let Geometry::Instance(inst) = &entity.geom else {
        return Err(format!(
            "ID {raw} はインスタンスではありません（{}）。パラメータを持つのは insert_component で置いたインスタンスです",
            geometry_type(&entity.geom)
        ));
    };
    let inst = inst.clone();
    let def = definition(&s.doc, inst.definition)?;
    let values = a
        .value("values")
        .ok_or_else(|| "values を指定してください".to_owned())?;
    let values = read_values(def, values, "values", true)?;
    if values.is_empty() {
        return Err("values に変えるパラメータがありません".to_owned());
    }

    let mut map = inst.overrides.clone();
    for (k, v) in &values {
        match v {
            Some(v) => {
                map.insert(k.clone(), v.clone());
            }
            None => {
                map.remove(k);
            }
        }
    }
    check_instance(&s.doc, inst.definition, inst.placement, &map)?;

    let commands: Vec<Box<dyn Command>> = values
        .into_iter()
        .map(|(k, v)| -> Box<dyn Command> {
            match v {
                Some(v) => Box::new(SetInstanceOverride::set("PSET", target, k, v)),
                None => Box::new(SetInstanceOverride::reset("PSET", target, k)),
            }
        })
        .collect();
    apply(s, "PSET", commands)?;

    let def = definition(&s.doc, inst.definition)?;
    Ok(json!({
        "drawing": s.tag().name(),
        "id": raw,
        "component": def.name,
        "overrides": overrides_json(&map),
        "params": effective_params(def, &map),
    }))
}

#[cfg(test)]
mod tests {
    use super::super::draw::tests::layered;
    use super::super::test_support::{eid, err, mutate, ok, rejected, server};
    use super::*;
    use crate::test_util::TempDir;
    use cad_core::component::resolve;
    use cad_core::geom::tolerance::{eq_angle, eq_len};
    use cad_core::native::write::write_to_bytes;

    /// 線分 1 本（e0）と円 1 つ（e1）を描き、ID を返す。
    fn two_shapes(s: &mut Server) -> (String, String) {
        let r = ok(
            s,
            "add_entities",
            json!({"entities": [
                {"type": "line", "start": [0, 0], "end": [100, 0]},
                {"type": "circle", "center": [50, 20], "radius": 10},
            ]}),
        );
        (
            r["ids"][0].as_str().unwrap().to_owned(),
            r["ids"][1].as_str().unwrap().to_owned(),
        )
    }

    /// 「窓」: 線分（index 0）と円（index 1）。パラメータ 幅（10〜500）・開く・向き。
    fn window(s: &mut Server) {
        ok(
            s,
            "define_component",
            json!({"name": "窓", "origin": [0, 0], "entities": [
                {"type": "line", "start": [0, 0], "end": [100, 0]},
                {"type": "circle", "center": [50, 20], "radius": 10},
            ]}),
        );
        ok(
            s,
            "set_component_params",
            json!({"component": "窓", "params": [
                {"name": "幅", "type": "number", "default": 100, "min": 10, "max": 500},
                {"name": "開く", "type": "bool", "default": false},
                {"name": "向き", "type": "choice", "options": ["左", "右"]},
            ]}),
        );
        ok(
            s,
            "bind",
            json!({"component": "窓", "entity_index": 0, "slot": "end.x", "expr": "幅"}),
        );
    }

    /// インスタンスを展開した形。
    fn resolved(s: &Server, id: &str) -> Vec<Geometry> {
        let target = crate::ids::resolve_ids(&s.doc, s.tag(), &[id.to_owned()]).unwrap()[0];
        let Geometry::Instance(inst) = &s.doc.entities().get(target).unwrap().geom else {
            panic!("インスタンスのはず")
        };
        resolve(inst, s.doc.definitions())
    }

    fn line_end_x(g: &Geometry) -> f64 {
        let Geometry::Line(l) = g else {
            panic!("線分のはず: {g:?}")
        };
        l.b.x
    }

    // ---- define_component ----------------------------------------------------

    /// 図面の図形から作ると、元を消して同じ場所にインスタンス 1 つ（アプリの COMPONENT と同じ）。
    /// Undo 1 回で全部戻る（定義・削除・配置を MacroCommand に束ねている）。
    #[test]
    fn define_from_ids_replaces_the_shapes_with_one_instance() {
        let dir = TempDir::new("comp-define-ids");
        let mut s = layered(&dir);
        let (line, circle) = two_shapes(&mut s);
        let r = mutate(
            &mut s,
            "define_component",
            json!({"name": "窓", "origin": [0, 0], "from_ids": [circle, line]}),
        );
        assert_eq!(s.doc.history().undo_name(), Some("COMPONENT"));
        assert_eq!(r["entity_count"], 2);
        assert_eq!(r["removed"], 2);
        assert_eq!(
            r["contents"],
            json!([
                {"index": 0, "type": "circle", "from_id": circle},
                {"index": 1, "type": "line", "from_id": line},
            ]),
            "contents は from_ids の順"
        );
        let def = s.doc.definitions().by_name("窓").unwrap();
        assert_eq!(s.doc.definitions().get(def).unwrap().entities.len(), 2);
        // 元の図形は消え、インスタンスが 1 つ（mutate の redo の後でも同じ ID）。
        let inst = r["instance"].as_str().unwrap().to_owned();
        err(&mut s, "get_entities", json!({ "ids": [line] }));
        let got = ok(&mut s, "get_entities", json!({ "ids": [inst.clone()] }));
        let g = &got["entities"][0];
        assert_eq!(g["geometry"]["type"], "instance");
        assert_eq!(g["geometry"]["component"], "窓");
        assert_eq!(g["layer"], "0", "中身が載っていたレイヤ");
        assert_eq!(resolved(&s, &inst).len(), 2, "見た目は変わらない");
    }

    /// redo では、名前で引き直した定義に、初回と同じ ID のインスタンスを戻す。
    /// 後の呼び出しがその ID を使っていても、まとめての redo が通る（Issue #92 を踏まない）。
    #[test]
    fn redo_bridges_by_name_and_keeps_instance_ids() {
        let dir = TempDir::new("comp-redo");
        let mut s = server(&dir);
        let (line, circle) = two_shapes(&mut s);
        let r = ok(
            &mut s,
            "define_component",
            json!({"name": "窓", "origin": [0, 0], "from_ids": [line, circle]}),
        );
        let replaced = r["instance"].as_str().unwrap().to_owned();
        ok(
            &mut s,
            "set_component_params",
            json!({"component": "窓", "params": [{"name": "幅", "type": "number", "default": 100}]}),
        );
        ok(
            &mut s,
            "bind",
            json!({"component": "窓", "entity_index": 0, "slot": "end.x", "expr": "幅"}),
        );
        let placed = ok(
            &mut s,
            "insert_component",
            json!({"component": "窓", "origin": [500, 0], "params": {"幅": 200}}),
        )["id"]
            .as_str()
            .unwrap()
            .to_owned();
        ok(
            &mut s,
            "set_instance_params",
            json!({"id": placed, "values": {"幅": 300}}),
        );
        ok(
            &mut s,
            "set_instance_params",
            json!({"id": replaced, "values": {"幅": 50}}),
        );
        let after = write_to_bytes(&s.doc);

        let undone = ok(&mut s, "undo", json!({"steps": 6}));
        assert_eq!(undone["count"], 6);
        assert_eq!(s.doc.definitions().len(), 0);
        let redone = ok(&mut s, "redo", json!({"steps": 6}));
        assert_eq!(redone["count"], 6, "{redone}");
        assert_eq!(write_to_bytes(&s.doc), after, "全部やり直せる");
        // ID はどちらも初回のまま。
        assert!(eq_len(line_end_x(&resolved(&s, &replaced)[0]), 50.0));
        assert!(eq_len(line_end_x(&resolved(&s, &placed)[0]), 300.0 + 500.0));
    }

    #[test]
    fn define_without_replacing_keeps_the_originals() {
        let dir = TempDir::new("comp-define-keep");
        let mut s = server(&dir);
        let (line, circle) = two_shapes(&mut s);
        let r = mutate(
            &mut s,
            "define_component",
            json!({"name": "窓", "origin": [0, 0], "from_ids": [line, circle], "replace_with_instance": false}),
        );
        assert_eq!(r["instance"], Value::Null);
        assert_eq!(r["removed"], 0);
        assert_eq!(s.doc.entities().len(), 2);
        assert_eq!(s.doc.definitions().len(), 1);
    }

    /// 図形の配列から作る。中身はレイヤ 0。他のコンポーネントの入れ子も、上書きつきで書ける。
    #[test]
    fn define_from_entities_and_nest() {
        let dir = TempDir::new("comp-define-entities");
        let mut s = server(&dir);
        window(&mut s);
        let r = mutate(
            &mut s,
            "define_component",
            json!({"name": "壁", "origin": [0, 0], "entities": [
                {"type": "polyline", "vertices": [[0, 0], [1000, 0], [1000, 100]]},
                {"type": "instance", "component": "窓", "origin": [200, 0], "overrides": {"幅": 300, "向き": "右"}},
            ]}),
        );
        assert_eq!(
            r["contents"],
            json!([{"index": 0, "type": "polyline"}, {"index": 1, "type": "instance"}])
        );
        assert_eq!(s.doc.entities().len(), 0, "定義だけを作る");
        let wall = s.doc.definitions().by_name("壁").unwrap();
        let def = s.doc.definitions().get(wall).unwrap();
        assert!(def.entities.iter().all(|e| e.layer == LayerId::ZERO));

        let placed = ok(
            &mut s,
            "insert_component",
            json!({"component": "壁", "origin": [0, 0]}),
        )["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let shapes = resolved(&s, &placed);
        assert_eq!(shapes.len(), 3, "ポリライン + 窓の線分と円");
        assert!(
            eq_len(line_end_x(&shapes[1]), 500.0),
            "入れ子の上書き 幅 = 300 が効く（200 + 300）"
        );
    }

    #[test]
    fn define_rejects_bad_requests_without_changing_anything() {
        let dir = TempDir::new("comp-define-bad");
        let mut s = layered(&dir);
        window(&mut s);
        let (line, _) = two_shapes(&mut s);
        let locked = eid(&s, 2);
        let hidden = eid(&s, 3);
        let shape = json!([{"type": "line", "start": [0, 0], "end": [1, 0]}]);
        for (args, needle) in [
            (
                json!({"name": "窓", "origin": [0, 0], "entities": shape}),
                "既に",
            ),
            (
                json!({"name": " 扉", "origin": [0, 0], "entities": shape}),
                "空白",
            ),
            (
                json!({"name": "", "origin": [0, 0], "entities": shape}),
                "空",
            ),
            (
                json!({"name": "扉\n", "origin": [0, 0], "entities": shape}),
                "空白",
            ),
            (json!({"name": "扉", "origin": [0, 0]}), "どちらか一方"),
            (
                json!({"name": "扉", "origin": [0, 0], "entities": shape, "from_ids": [line]}),
                "どちらか一方",
            ),
            (
                json!({"name": "扉", "origin": [0, 0], "entities": shape, "replace_with_instance": true}),
                "from_ids のときだけ",
            ),
            (
                json!({"name": "扉", "origin": [0, 0], "from_ids": [line, locked]}),
                "ロック中",
            ),
            (
                json!({"name": "扉", "origin": [0, 0], "from_ids": [hidden]}),
                "非表示",
            ),
            (
                json!({"name": "扉", "origin": [0, 0], "from_ids": [line, line]}),
                "2 回",
            ),
            (
                json!({"name": "扉", "origin": [0, 0], "from_ids": ["d000000-1e0g0"]}),
                "つなぎ直す前",
            ),
            // 自分自身を入れ子にする（循環）: 作る前なので、その名前のコンポーネントは無い。
            (
                json!({"name": "扉", "origin": [0, 0], "entities": [
                    {"type": "instance", "component": "扉", "origin": [0, 0]},
                ]}),
                "扉 がありません",
            ),
            (
                json!({"name": "扉", "origin": [0, 0], "entities": [
                    {"type": "instance", "component": "窓", "origin": [0, 0], "overrides": {"幅": 1}},
                ]}),
                "範囲",
            ),
            (
                json!({"name": "扉", "origin": [0, 0], "entities": [
                    {"type": "instance", "component": "窓", "origin": [0, 0], "overrides": {"高さ": 1}},
                ]}),
                "高さ",
            ),
            (
                json!({"name": "扉", "origin": [0, 0], "entities": [
                    {"type": "line", "start": [0, 0], "end": [1, 0]},
                    {"type": "circle", "center": [0, 0], "radius": "1/0"},
                ]}),
                "entities[1]",
            ),
            (
                json!({"name": "扉", "origin": [2e9, 0], "entities": shape}),
                "上限",
            ),
        ] {
            let msg = rejected(&mut s, "define_component", args.clone());
            assert!(msg.contains(needle), "{args}: {msg}");
        }
        assert_eq!(s.doc.definitions().len(), 1);
    }

    // ---- set_component_params -------------------------------------------------

    /// 3 つの型を宣言する。list_components の出力をそのまま渡し返せる。
    #[test]
    fn params_are_declared_and_round_trip_through_list_components() {
        let dir = TempDir::new("comp-params");
        let mut s = server(&dir);
        ok(
            &mut s,
            "define_component",
            json!({"name": "扉", "origin": [0, 0], "entities": [{"type": "line", "start": [0, 0], "end": [1, 0]}]}),
        );
        let r = mutate(
            &mut s,
            "set_component_params",
            json!({"component": "扉", "params": [
                {"name": "幅", "type": "number", "default": "800 + 100", "min": 600, "max": "1000"},
                {"name": "高さ", "type": "number", "default": "幅 * 2"},
                {"name": "開く", "type": "bool", "default": true},
                {"name": "種類", "type": "choice", "options": ["引違い", "開き"], "default": "開き"},
                {"name": "取っ手", "type": "choice", "options": ["左", "右"]},
            ]}),
        );
        assert_eq!(s.doc.history().undo_name(), Some("PARAM"));
        assert!(r["warnings"].as_array().unwrap().is_empty());
        let params = r["component"]["params"].clone();
        assert_eq!(params[0]["range"], json!([600.0, 1000.0]));
        assert_eq!(params[1]["default"], "幅 * 2");
        assert_eq!(params[3]["choices"], json!(["引違い", "開き"]));
        assert_eq!(params[3]["default"], "'開き'");
        assert_eq!(params[4]["default"], "'左'", "選択の既定は最初の候補");

        // list_components の params をそのまま渡し返すと、図面は 1 バイトも変わらない。
        let listed = ok(&mut s, "list_components", json!({"name": "扉"}));
        let before = write_to_bytes(&s.doc);
        mutate(
            &mut s,
            "set_component_params",
            json!({"component": "扉", "params": listed["components"][0]["params"]}),
        );
        assert_eq!(write_to_bytes(&s.doc), before);
        // 空の配列で全部消せる。
        mutate(
            &mut s,
            "set_component_params",
            json!({"component": "扉", "params": []}),
        );
        let def = s.doc.definitions().by_name("扉").unwrap();
        assert!(s.doc.definitions().get(def).unwrap().params.is_empty());
    }

    #[test]
    fn params_reject_bad_declarations() {
        let dir = TempDir::new("comp-params-bad");
        let mut s = server(&dir);
        window(&mut s); // 窓の線分の end.x は 幅 に束縛済み。
        let n = |name: &str, default: Value| json!({"name": name, "type": "number", "default": default});
        let keep = [
            n("幅", json!(100)),
            json!({"name": "開く", "type": "bool", "default": false}),
        ];
        let with = |extra: Value| {
            let mut v = keep.to_vec();
            v.push(extra);
            json!({"component": "窓", "params": v})
        };
        for (args, needle) in [
            (
                json!({"component": "無い", "params": []}),
                "無い はありません",
            ),
            (with(n("高さ", json!("真"))), "数値"),
            (with(n("高さ", json!("1/0"))), "計算できません"),
            (with(n("高さ", json!("奥行 * 2"))), "奥行"),
            (with(n("幅", json!(1))), "重複"),
            (with(n("a b", json!(1))), "名前として読めません"),
            (with(n("sin", json!(1))), "名前として読めません"),
            (with(n("真", json!(1))), "名前として読めません"),
            (with(n("Ｗ", json!(1))), "名前として読めません"),
            (with(n("高さ", json!("2 +"))), "読めません"),
            (
                with(json!({"name": "高さ", "type": "number", "default": 5, "min": 10, "max": 20})),
                "範囲",
            ),
            (
                with(json!({"name": "高さ", "type": "number", "default": 5, "min": 30, "max": 20})),
                "下限",
            ),
            (
                with(json!({"name": "高さ", "type": "number", "default": 5, "min": 0})),
                "両方",
            ),
            (
                with(json!({"name": "高さ", "type": "number", "default": 5, "options": ["a"]})),
                "choice",
            ),
            (with(json!({"name": "高さ", "type": "number"})), "default"),
            (
                with(json!({"name": "高さ", "type": "text", "default": 1})),
                "扱えません",
            ),
            (with(json!({"name": "向き", "type": "choice"})), "options"),
            (
                with(json!({"name": "向き", "type": "choice", "options": []})),
                "1 個以上",
            ),
            (
                with(json!({"name": "向き", "type": "choice", "options": ["左", "左"]})),
                "重複",
            ),
            (
                with(json!({"name": "向き", "type": "choice", "options": ["左'"]})),
                "使えません",
            ),
            (
                with(
                    json!({"name": "向き", "type": "choice", "options": ["左", "右"], "default": "上"}),
                ),
                "上",
            ),
            (
                with(json!({"name": "高さ", "type": "number", "default": 1, "unit": "mm"})),
                "不明な項目",
            ),
            // 既定値どうしの循環。
            (
                json!({"component": "窓", "params": [
                    n("幅", json!("高さ + 1")),
                    n("高さ", json!("幅 - 1")),
                ]}),
                "循環",
            ),
            // 束縛が使っている 幅 を消す・真偽にする。
            (json!({"component": "窓", "params": []}), "幅 は消せません"),
            (
                json!({"component": "窓", "params": [{"name": "幅", "type": "bool", "default": true}]}),
                "数値でなくなります",
            ),
        ] {
            let msg = rejected(&mut s, "set_component_params", args.clone());
            assert!(msg.contains(needle), "{args}: {msg}");
        }
    }

    /// 宣言を変えて使われなくなるインスタンスの上書きは warnings に出す（拒まない。アプリと同じ）。
    #[test]
    fn params_warn_about_overrides_that_stop_working() {
        let dir = TempDir::new("comp-params-warn");
        let mut s = server(&dir);
        window(&mut s);
        let id = ok(
            &mut s,
            "insert_component",
            json!({"component": "窓", "origin": [0, 0], "params": {"幅": 400, "向き": "右"}}),
        )["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let r = mutate(
            &mut s,
            "set_component_params",
            json!({"component": "窓", "params": [
                {"name": "幅", "type": "number", "default": 100, "min": 10, "max": 300},
                {"name": "開く", "type": "bool", "default": false},
            ]}),
        );
        let warnings = r["warnings"].to_string();
        assert!(warnings.contains(&id), "{warnings}");
        assert!(warnings.contains("幅 = 400"), "{warnings}");
        assert!(warnings.contains("向き"), "{warnings}");
        assert!(
            eq_len(line_end_x(&resolved(&s, &id)[0]), 100.0),
            "合わない上書きは使われず既定値"
        );
    }

    // ---- bind -------------------------------------------------------------------

    /// 束縛すると全インスタンスが追従する。角度の項目は度。同じ項目は置き換える。
    #[test]
    fn bind_drives_coordinates_with_expressions() {
        let dir = TempDir::new("comp-bind");
        let mut s = server(&dir);
        ok(
            &mut s,
            "define_component",
            json!({"name": "扇", "origin": [0, 0], "entities": [
                {"type": "arc", "center": [0, 0], "radius": 10, "start_angle": 0, "end_angle": 45},
                {"type": "polyline", "vertices": [[0, 0], [10, 0], [10, 10]]},
            ]}),
        );
        ok(
            &mut s,
            "set_component_params",
            json!({"component": "扇", "params": [
                {"name": "開き", "type": "number", "default": 90},
                {"name": "長さ", "type": "number", "default": 30},
            ]}),
        );
        let id = ok(
            &mut s,
            "insert_component",
            json!({"component": "扇", "origin": [0, 0]}),
        )["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let r = mutate(
            &mut s,
            "bind",
            json!({"component": "扇", "entity_index": 0, "slot": "end_angle", "expr": "開き"}),
        );
        assert_eq!(s.doc.history().undo_name(), Some("BIND"));
        assert_eq!(r["value"], 90.0);
        assert_eq!(r["replaced"], false);
        assert_eq!(r["binding"]["field"], "end_angle");
        assert_eq!(r["binding"]["entity_type"], "arc");
        let Geometry::Arc(a) = &resolved(&s, &id)[0] else {
            panic!("円弧のはず")
        };
        assert!(
            eq_angle(a.end_angle, std::f64::consts::FRAC_PI_2),
            "角度は度"
        );

        mutate(
            &mut s,
            "bind",
            json!({"component": "扇", "entity_index": 1, "slot": "vertices[2].y", "expr": "長さ / 2"}),
        );
        let r = mutate(
            &mut s,
            "bind",
            json!({"component": "扇", "entity_index": 1, "slot": "vertices[2].y", "expr": "長さ"}),
        );
        assert_eq!(r["replaced"], true);
        let def = s.doc.definitions().by_name("扇").unwrap();
        assert_eq!(s.doc.definitions().get(def).unwrap().bindings.len(), 2);
        let Geometry::Polyline(p) = &resolved(&s, &id)[1] else {
            panic!("ポリラインのはず")
        };
        assert!(eq_len(p.vertices[2].y, 30.0));

        // list_components の contents と bindings で、束縛の位置を読み直せる。
        let listed = ok(
            &mut s,
            "list_components",
            json!({"name": "扇", "contents": true}),
        );
        let c = &listed["components"][0];
        assert_eq!(c["contents"][1]["geometry"]["type"], "polyline");
        assert_eq!(c["bindings"][1]["entity"], 1);
        assert_eq!(c["bindings"][1]["field"], "vertices[2].y");
    }

    #[test]
    fn bind_rejects_what_cannot_be_bound() {
        let dir = TempDir::new("comp-bind-bad");
        let mut s = server(&dir);
        window(&mut s);
        let b = |index: Value, slot: &str, expr: Value| json!({"component": "窓", "entity_index": index, "slot": slot, "expr": expr});
        for (args, needle) in [
            (
                json!({"component": "無い", "entity_index": 0, "slot": "end.x", "expr": "1"}),
                "無い はありません",
            ),
            (b(json!(2), "end.x", json!("幅")), "範囲の外"),
            (b(json!(-1), "end.x", json!("幅")), "0 以上の整数"),
            (b(json!(1), "start.x", json!("幅")), "center.x"),
            (b(json!(0), "radius", json!("幅")), "start.x"),
            (b(json!(0), "end.z", json!("幅")), "束縛できません"),
            (b(json!(0), "end.x", json!("高さ")), "高さ"),
            (b(json!(0), "end.x", json!("開く")), "数値ではありません"),
            (b(json!(0), "end.x", json!("幅 / 0")), "計算できません"),
            (b(json!(0), "end.x", json!("幅 +")), "読めません"),
            (b(json!(0), "end.x", json!(true)), "式の文字列"),
            (
                b(json!(0), "end.x", json!("1+".repeat(600) + "1")),
                "長すぎ",
            ),
        ] {
            let msg = rejected(&mut s, "bind", args.clone());
            assert!(msg.contains(needle), "{args}: {msg}");
        }
        // ポリラインの頂点の番号は範囲内であること。
        ok(
            &mut s,
            "define_component",
            json!({"name": "枠", "origin": [0, 0], "entities": [
                {"type": "polyline", "vertices": [[0, 0], [1, 0]]},
            ]}),
        );
        let msg = rejected(
            &mut s,
            "bind",
            json!({"component": "枠", "entity_index": 0, "slot": "vertices[2].x", "expr": "1"}),
        );
        assert!(msg.contains("0〜1"), "{msg}");
    }

    /// 既定値で使えない値になる束縛は、拒まずに warnings で知らせる。
    #[test]
    fn bind_warns_when_defaults_give_unusable_values() {
        let dir = TempDir::new("comp-bind-warn");
        let mut s = server(&dir);
        window(&mut s);
        let r = mutate(
            &mut s,
            "bind",
            json!({"component": "窓", "entity_index": 1, "slot": "radius", "expr": "幅 - 100"}),
        );
        assert!(r["warnings"].to_string().contains("0 より大きい"), "{r}");
        let r = mutate(
            &mut s,
            "bind",
            json!({"component": "窓", "entity_index": 0, "slot": "end.x", "expr": "0"}),
        );
        assert!(r["warnings"].to_string().contains("成り立ちません"), "{r}");
    }

    // ---- insert_component -----------------------------------------------------

    /// 置くのと上書きが 1 回の呼び出し = undo 1 回。上書きは型ごとに読む。
    #[test]
    fn insert_places_with_overrides_in_one_step() {
        let dir = TempDir::new("comp-insert");
        let mut s = layered(&dir);
        window(&mut s);
        let r = mutate(
            &mut s,
            "insert_component",
            json!({"component": "窓", "origin": [1000, 0], "rotation_deg": 90, "scale": "1/2",
                   "layer": "WALL", "params": {"幅": "100 * 2", "開く": true, "向き": "右"}}),
        );
        assert_eq!(s.doc.history().undo_name(), Some("INSERT"));
        assert_eq!(r["layer"], "WALL");
        assert_eq!(
            r["overrides"],
            json!({"幅": 200.0, "開く": true, "向き": "右"})
        );
        assert_eq!(
            r["params"],
            json!({"幅": 200.0, "開く": true, "向き": "右"})
        );
        let id = r["id"].as_str().unwrap().to_owned();
        let got = ok(&mut s, "get_entities", json!({ "ids": [id.clone()] }));
        let g = &got["entities"][0]["geometry"];
        assert!(eq_len(g["rotation"].as_f64().unwrap(), 90.0), "度");
        assert_eq!(g["scale"], 0.5);
        // 幅 200 の線分を半分にして 90° 回すと、(1000, 0) から (1000, 100) へ。
        let Geometry::Line(l) = &resolved(&s, &id)[0] else {
            panic!("線分のはず")
        };
        assert!(
            l.b.eq_tol(cad_core::geom::Point2::new(1000.0, 100.0)),
            "{l:?}"
        );

        // 上書きなし: 既定値。
        let r = mutate(
            &mut s,
            "insert_component",
            json!({"component": "窓", "origin": [0, 0], "flipped": true}),
        );
        assert_eq!(r["overrides"], json!({}));
        assert_eq!(r["params"]["幅"], 100.0);
        assert_eq!(r["params"]["向き"], "左");
    }

    #[test]
    fn insert_rejects_bad_values_without_placing_anything() {
        let dir = TempDir::new("comp-insert-bad");
        let mut s = layered(&dir);
        window(&mut s);
        let at = |params: Value| json!({"component": "窓", "origin": [0, 0], "params": params});
        for (args, needle) in [
            (
                json!({"component": "無い", "origin": [0, 0]}),
                "あるのは 窓",
            ),
            (at(json!({"高さ": 1})), "高さ はありません"),
            (at(json!({"幅": 5})), "範囲 10〜500"),
            (at(json!({"幅": 600})), "範囲 10〜500"),
            (at(json!({"幅": true})), "数値"),
            (at(json!({"幅": "幅 * 2"})), "パラメータは使えません"),
            (at(json!({"幅": "1/0"})), "計算できません"),
            (at(json!({"開く": 1})), "真偽"),
            (at(json!({"向き": "上"})), "左 / 右"),
            (at(json!({"向き": 1})), "選択"),
            (at(json!({"幅": null})), "null"),
            (at(json!([1])), "オブジェクト"),
            (
                json!({"component": "窓", "origin": [0, 0], "scale": 0}),
                "0 より大きい",
            ),
            (
                json!({"component": "窓", "origin": [0, 0], "scale": -2}),
                "flipped",
            ),
            (
                json!({"component": "窓", "origin": [0, 0], "rotation_deg": "1/0"}),
                "計算できません",
            ),
            (
                json!({"component": "窓", "origin": [0, 0], "layer": "LOCK"}),
                "ロック中",
            ),
            (
                json!({"component": "窓", "origin": [0, 0], "layer": "HIDE"}),
                "非表示",
            ),
            (
                json!({"component": "窓", "origin": [0, 0], "layer": "無い"}),
                "レイヤ 無い はありません",
            ),
            (
                json!({"component": "窓", "origin": [0, 0], "scale": 1e8}),
                "上限",
            ),
            (json!({"component": "窓"}), "origin"),
        ] {
            let msg = rejected(&mut s, "insert_component", args.clone());
            assert!(msg.contains(needle), "{args}: {msg}");
        }
    }

    // ---- set_instance_params ----------------------------------------------------

    #[test]
    fn instance_params_are_set_and_reset_in_one_step() {
        let dir = TempDir::new("comp-pset");
        let mut s = server(&dir);
        window(&mut s);
        let id = ok(
            &mut s,
            "insert_component",
            json!({"component": "窓", "origin": [0, 0], "params": {"向き": "右"}}),
        )["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let r = mutate(
            &mut s,
            "set_instance_params",
            json!({"id": id, "values": {"幅": 250, "開く": true}}),
        );
        assert_eq!(s.doc.history().undo_name(), Some("PSET"));
        assert_eq!(
            r["overrides"],
            json!({"幅": 250.0, "開く": true, "向き": "右"})
        );
        assert!(eq_len(line_end_x(&resolved(&s, &id)[0]), 250.0));

        // null で既定値へ戻す。戻すのと変えるのを混ぜても 1 回。
        let r = mutate(
            &mut s,
            "set_instance_params",
            json!({"id": id, "values": {"幅": null, "向き": "左", "開く": null}}),
        );
        assert_eq!(r["overrides"], json!({"向き": "左"}));
        assert_eq!(
            r["params"],
            json!({"幅": 100.0, "開く": false, "向き": "左"})
        );
        assert!(eq_len(line_end_x(&resolved(&s, &id)[0]), 100.0));
    }

    #[test]
    fn instance_params_reject_bad_targets_and_values() {
        let dir = TempDir::new("comp-pset-bad");
        let mut s = layered(&dir);
        window(&mut s);
        let inst = ok(
            &mut s,
            "insert_component",
            json!({"component": "窓", "origin": [0, 0], "layer": "WALL"}),
        )["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let line = eid(&s, 1);
        for (args, needle) in [
            (
                json!({"id": line, "values": {"幅": 200}}),
                "インスタンスではありません",
            ),
            (json!({"id": inst, "values": {}}), "ありません"),
            (
                json!({"id": inst, "values": {"高さ": 1}}),
                "高さ はありません",
            ),
            (json!({"id": inst, "values": {"幅": 1}}), "範囲"),
            (
                json!({"id": inst, "values": {"幅": 200, "開く": "yes"}}),
                "values.開く",
            ),
            (json!({"id": inst, "values": {"向き": "上"}}), "左 / 右"),
            (json!({"id": inst, "values": 3}), "オブジェクト"),
            (
                json!({"id": "d000000-1e0g0", "values": {"幅": 200}}),
                "つなぎ直す前",
            ),
        ] {
            let msg = rejected(&mut s, "set_instance_params", args.clone());
            assert!(msg.contains(needle), "{args}: {msg}");
        }
        // ロック中のレイヤのインスタンスは変えられない。
        ok(
            &mut s,
            "update_layer",
            json!({"name": "WALL", "locked": true}),
        );
        let msg = rejected(
            &mut s,
            "set_instance_params",
            json!({"id": inst, "values": {"幅": 200}}),
        );
        assert!(msg.contains("ロック中"), "{msg}");
    }
}
