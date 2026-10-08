//! 作図・変更・削除の道具: `add_entities` / `modify_entities` / `delete_entities`。
//!
//! 共通の約束は [`super::mutate`]。

use cad_core::command::{AddEntities, DeleteEntities, ReplaceGeometries};
use cad_core::{Entity, Geometry};
use serde_json::{json, Map, Value};

use super::mutate::{
    apply, created_since, destination_layer, editable_targets, ensure_capacity, id_strings,
    max_slot,
};
use super::{Args, Tool, ToolResult};
use crate::convert::{apply_fields, check_extent, geometry_from_json};
use crate::limits::{MAX_IDS_PER_CALL, MAX_SHAPES_PER_CALL};
use crate::server::Server;

/// `add_entities` で作れる種類。インスタンスは `insert_component` で置く。
const DRAWABLE: [&str; 5] = ["line", "circle", "arc", "xline", "polyline"];

pub(super) const ADD_ENTITIES: Tool = Tool {
    name: "add_entities",
    title: "図形を描く",
    description: "図形をまとめて描く（1 回の呼び出しが undo 1 回ぶん）。entities の各要素は type と形:\n\
- line: start, end\n\
- circle: center, radius\n\
- arc: center, radius, start_angle, end_angle（度。start から end へ反時計回り）か、3 点 start, through, end\n\
- xline（無限の作図線）: origin と、angle（度）・direction・through（もう 1 つの通過点）のどれか\n\
- polyline: vertices（点の配列）, closed（省略時 false。閉じるなら頂点 3 個以上）\n\
点は {\"x\":..,\"y\":..} か [x, y]。数値は JSON の数値か式の文字列（\"100*2+5\"、\"sqrt(2)*50\"。式の中の角度は度）。\n\
layer（省略時は現在レイヤ）に置く。色はレイヤに従う。長さ 0 の線分・半径 0 の円・一直線上の 3 点の円弧・NaN・大きすぎる座標は拒み、\
1 つでも拒まれたら何も描かない。返り値の ids は entities と同じ順の新しい図形 ID。",
    schema: || {
        (
            json!({
                "entities": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": MAX_SHAPES_PER_CALL,
                    "items": {
                        "type": "object",
                        "properties": { "type": { "type": "string", "enum": DRAWABLE } },
                        "required": ["type"],
                    },
                    "description": "描く図形の配列。各要素は {\"type\": \"line\", \"start\": [0,0], \"end\": [100,0]} など",
                },
                "layer": { "type": "string", "description": "置くレイヤの名前（省略時は現在レイヤ）" },
            }),
            &["entities"],
        )
    },
    read_only: false,
    destructive: false,
    idempotent: false,
    run: add_entities,
};

pub(super) const MODIFY_ENTITIES: Tool = Tool {
    name: "modify_entities",
    title: "図形の形を変える",
    description: "既存の図形の形を、set に書いた項目だけ変える（ID・レイヤ・色は変わらない。1 回の呼び出しが undo 1 回ぶん）。\
項目名は get_entities の geometry と同じ（line: start, end / circle: center, radius / arc: center, radius, start_angle, end_angle か 3 点 start, through, end / \
xline: origin, angle・direction・through / polyline: vertices, closed / instance: origin, rotation, scale, flipped）。角度は度、数値は式の文字列も可。\
get_entities の geometry をそのまま set に渡してもよい。変えられないもの: 種類、ポリラインの頂点の数（作り直すなら delete_entities と add_entities）、\
インスタンスの component と overrides（同じ値なら渡してよい。上書きは set_instance_params で変える）。非表示・ロック中のレイヤの図形、1 つでも成立しない形があれば何も変えない。",
    schema: || {
        (
            json!({
                "changes": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": MAX_SHAPES_PER_CALL,
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string", "description": "図形 ID" },
                            "set": { "type": "object", "description": "変える項目（例 {\"radius\": 25}、{\"end\": [100, \"50*2\"]}）" },
                        },
                        "required": ["id", "set"],
                        "additionalProperties": false,
                    },
                    "description": "変更の配列。同じ ID は 1 回だけ",
                },
            }),
            &["changes"],
        )
    },
    read_only: false,
    destructive: true,
    idempotent: true,
    run: modify_entities,
};

pub(super) const DELETE_ENTITIES: Tool = Tool {
    name: "delete_entities",
    title: "図形を消す",
    description: "ID で指定した図形を消す（1 回の呼び出しが undo 1 回ぶん。undo で同じ ID のまま戻る）。\
1 つでも使えない ID（形が違う・別の図面・存在しない・重複）や、非表示・ロック中のレイヤの図形があれば何も消さない。",
    schema: || (ids_schema("消す図形の ID の配列"), &["ids"]),
    read_only: false,
    destructive: true,
    idempotent: false,
    run: delete_entities,
};

/// `ids` 引数のスキーマ。
pub(super) fn ids_schema(description: &str) -> Value {
    json!({
        "ids": {
            "type": "array",
            "items": { "type": "string" },
            "minItems": 1,
            "maxItems": MAX_IDS_PER_CALL,
            "description": description,
        },
    })
}

/// オブジェクトの配列の引数（必須・1 個以上 `max` 個以下）。
pub(super) fn object_list<'a>(
    a: &'a Args,
    key: &str,
    max: usize,
) -> Result<Vec<&'a Map<String, Value>>, String> {
    let list = a
        .value(key)
        .ok_or_else(|| format!("{key} を指定してください"))?
        .as_array()
        .ok_or_else(|| format!("{key} は配列で指定してください"))?;
    if list.is_empty() || list.len() > max {
        return Err(format!("{key} は 1 個以上 {max} 個以下で指定してください"));
    }
    list.iter()
        .enumerate()
        .map(|(i, v)| {
            v.as_object()
                .ok_or_else(|| format!("{key}[{i}] はオブジェクトで指定してください"))
        })
        .collect()
}

/// 問題の一覧を 1 つのエラーにする。
pub(super) fn join_problems(problems: Vec<String>, tail: &str) -> Result<(), String> {
    if problems.is_empty() {
        Ok(())
    } else {
        Err(format!("{}\n{tail}", problems.join("\n")))
    }
}

fn add_entities(s: &mut Server, a: &Args) -> ToolResult {
    let items = object_list(a, "entities", MAX_SHAPES_PER_CALL)?;
    let layer = destination_layer(s, a.opt_str("layer")?)?;
    let defs = s.doc.definitions();

    let mut entities = Vec::with_capacity(items.len());
    let mut problems = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let ty = item.get("type").and_then(Value::as_str).unwrap_or_default();
        if ty == "instance" {
            problems.push(format!(
                "entities[{i}]: インスタンスは add_entities では置けません（insert_component で置きます。パラメータの上書きもそこで渡せます）"
            ));
            continue;
        }
        let built = geometry_from_json(&Value::Object((*item).clone()), defs)
            .and_then(|g| check_extent(&g, defs).map(|()| g));
        match built {
            Ok(g) => entities.push(Entity::new(g, layer)),
            Err(e) => problems.push(format!("entities[{i}]: {e}")),
        }
    }
    join_problems(
        problems,
        "1 つでも描けない図形があるので、何も描いていません。",
    )?;
    ensure_capacity(&s.doc, entities.len())?;

    let before = max_slot(&s.doc);
    let count = entities.len();
    apply(s, "ADD", vec![Box::new(AddEntities::many("ADD", entities))])?;
    let created = created_since(&s.doc, before);
    debug_assert_eq!(created.len(), count);
    Ok(json!({
        "drawing": s.tag().name(),
        "ids": id_strings(s, &created),
        "count": created.len(),
        "layer": s.doc.layers().get(layer).map(|l| l.name.as_str()),
    }))
}

fn modify_entities(s: &mut Server, a: &Args) -> ToolResult {
    let changes = object_list(a, "changes", MAX_SHAPES_PER_CALL)?;
    let mut raw_ids = Vec::with_capacity(changes.len());
    let mut sets = Vec::with_capacity(changes.len());
    for (i, c) in changes.iter().enumerate() {
        if let Some(extra) = c.keys().find(|k| *k != "id" && *k != "set") {
            return Err(format!(
                "changes[{i}] に不明な項目 {extra} があります（id と set だけ。形の項目は set の中に書きます）"
            ));
        }
        let id = c
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("changes[{i}].id を文字列で指定してください"))?;
        let set = c
            .get("set")
            .and_then(Value::as_object)
            .ok_or_else(|| format!("changes[{i}].set をオブジェクトで指定してください"))?;
        if set.is_empty() {
            return Err(format!("changes[{i}].set に変える項目がありません"));
        }
        raw_ids.push(id.to_owned());
        sets.push(set);
    }
    let targets = editable_targets(s, &raw_ids)?;

    let defs = s.doc.definitions();
    let mut replacements = Vec::with_capacity(targets.len());
    let mut problems = Vec::new();
    for ((id, set), raw) in targets.iter().zip(&sets).zip(&raw_ids) {
        let Some(e) = s.doc.entities().get(*id) else {
            continue;
        };
        match apply_fields(&e.geom, set, defs).and_then(|g| check_extent(&g, defs).map(|()| g)) {
            Ok(g) => replacements.push((*id, g)),
            Err(err) => problems.push(format!("{raw}: {err}")),
        }
    }
    join_problems(
        problems,
        "1 つでも変えられない図形があるので、何も変えていません。",
    )?;

    apply(
        s,
        "MODIFY",
        vec![Box::new(ReplaceGeometries::new("MODIFY", replacements))],
    )?;
    Ok(json!({
        "drawing": s.tag().name(),
        "ids": id_strings(s, &targets),
        "count": targets.len(),
    }))
}

fn delete_entities(s: &mut Server, a: &Args) -> ToolResult {
    let raw = a.string_list("ids", MAX_IDS_PER_CALL)?;
    let targets = editable_targets(s, &raw)?;
    let count = targets.len();
    apply(
        s,
        "ERASE",
        vec![Box::new(DeleteEntities::new("ERASE", targets))],
    )?;
    Ok(json!({
        "drawing": s.tag().name(),
        "deleted": count,
        "entity_count": s.doc.entities().len(),
    }))
}

/// 形が変わる・作られる図形を、適用の前に確かめる（変形の道具）。
///
/// `make` で結果の形を試しに求め、`Geometry::validate`（NaN・無限大・退化）と大きさの上限に通す。
pub(super) fn precheck(
    s: &Server,
    targets: &[cad_core::EntityId],
    make: impl Fn(&Geometry) -> Geometry,
) -> Result<(), String> {
    let defs = s.doc.definitions();
    let ids = id_strings(s, targets);
    let mut problems = Vec::new();
    for (id, raw) in targets.iter().zip(&ids) {
        let Some(e) = s.doc.entities().get(*id) else {
            continue;
        };
        let g = make(&e.geom);
        if let Err(err) = g
            .validate()
            .map_err(|e| e.to_string())
            .and_then(|()| check_extent(&g, defs))
        {
            problems.push(format!("{raw}: 変形した結果が成り立ちません（{err}）"));
        }
    }
    join_problems(
        problems,
        "1 つでも成り立たない図形があるので、何も変えていません。",
    )
}

#[cfg(test)]
pub(super) mod tests {
    use super::super::test_support::{eid, err, mutate, ok, rejected, server};
    use super::*;
    use crate::test_util::TempDir;
    use cad_core::command::{AddLayer, SetLayerProperties};
    use cad_core::geom::tolerance::{eq_angle, eq_len};
    use cad_core::AciColor;

    /// ロック中のレイヤ LOCK・非表示のレイヤ HIDE・普通のレイヤ WALL を持つ図面。
    /// 各レイヤに線分を 1 本ずつ置く（レイヤ 0 に 1 本、WALL・LOCK・HIDE に 1 本ずつ = e0..e3）。
    pub fn layered(dir: &TempDir) -> Server {
        let mut s = server(dir);
        for (name, color) in [("WALL", 1), ("LOCK", 2), ("HIDE", 3)] {
            s.doc
                .apply(Box::new(AddLayer::new(name, AciColor(color))))
                .unwrap();
        }
        for (i, layer) in ["0", "WALL", "LOCK", "HIDE"].into_iter().enumerate() {
            let x = f64::from(u32::try_from(i).unwrap()) * 10.0;
            let r = ok(
                &mut s,
                "add_entities",
                json!({"layer": layer, "entities": [{"type": "line", "start": [x, 0], "end": [x, 5]}]}),
            );
            assert_eq!(r["count"], 1);
        }
        let lock = s.doc.layers().by_name("LOCK").unwrap();
        let hide = s.doc.layers().by_name("HIDE").unwrap();
        s.doc
            .apply(Box::new(SetLayerProperties::new(lock).locked(true)))
            .unwrap();
        s.doc
            .apply(Box::new(SetLayerProperties::new(hide).visible(false)))
            .unwrap();
        s
    }

    fn geometry_of(s: &mut Server, id: &str) -> Value {
        let r = ok(s, "get_entities", json!({ "ids": [id] }));
        r["entities"][0]["geometry"].clone()
    }

    #[test]
    fn add_entities_draws_every_kind_in_one_step() {
        let dir = TempDir::new("draw-add");
        let mut s = server(&dir);
        let r = mutate(
            &mut s,
            "add_entities",
            json!({"entities": [
                {"type": "line", "start": [0, 0], "end": [100, 0]},
                {"type": "circle", "center": {"x": 50, "y": 50}, "radius": "5*2"},
                {"type": "arc", "center": [0, 0], "radius": 10, "start_angle": 0, "end_angle": 90},
                {"type": "arc", "start": [10, 0], "through": [0, 10], "end": [-10, 0]},
                {"type": "xline", "origin": [0, 0], "angle": 45},
                {"type": "xline", "origin": [0, 0], "through": [0, 1]},
                {"type": "polyline", "vertices": [[0, 0], [10, 0], [10, 10]], "closed": true},
            ]}),
        );
        assert_eq!(r["count"], 7);
        assert_eq!(r["layer"], "0");
        assert_eq!(s.doc.entities().len(), 7);
        assert_eq!(s.doc.history().undo_name(), Some("ADD"));

        // redo で ID は振り直されるので、一覧から取り直して形を確かめる。
        let list = ok(&mut s, "list_entities", json!({}));
        let ids: Vec<String> = list["entities"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["id"].as_str().unwrap().to_owned())
            .collect();
        let circle = geometry_of(&mut s, &ids[1]);
        assert_eq!(circle["radius"], 10.0, "式の文字列");
        let arc = geometry_of(&mut s, &ids[2]);
        assert!(eq_len(arc["end_angle"].as_f64().unwrap(), 90.0), "角度は度");
        let arc3 = geometry_of(&mut s, &ids[3]);
        assert!(eq_len(arc3["radius"].as_f64().unwrap(), 10.0));
        assert!(eq_len(arc3["end_angle"].as_f64().unwrap(), 180.0));
        let xline = geometry_of(&mut s, &ids[5]);
        assert!(eq_len(xline["angle"].as_f64().unwrap(), 90.0));
    }

    /// 返り値の ID は entities と同じ順の、新しく作られた図形。
    #[test]
    fn add_entities_returns_the_new_ids_in_order() {
        let dir = TempDir::new("draw-add-ids");
        let mut s = layered(&dir);
        // 途中のスロットを空けておいても、新しい ID だけを拾う。
        let first = eid(&s, 0);
        ok(&mut s, "delete_entities", json!({ "ids": [first] }));
        let r = ok(
            &mut s,
            "add_entities",
            json!({"entities": [
                {"type": "circle", "center": [0, 0], "radius": 1},
                {"type": "line", "start": [0, 0], "end": [1, 0]},
            ]}),
        );
        assert_eq!(r["ids"], json!([eid(&s, 4), eid(&s, 5)]));
        let got = ok(&mut s, "get_entities", json!({ "ids": r["ids"].clone() }));
        assert_eq!(got["entities"][0]["geometry"]["type"], "circle");
        assert_eq!(got["entities"][1]["geometry"]["type"], "line");
    }

    #[test]
    fn add_entities_rejects_bad_shapes_without_drawing_anything() {
        let dir = TempDir::new("draw-add-bad");
        let mut s = server(&dir);
        let good = json!({"type": "line", "start": [0, 0], "end": [1, 0]});
        for (bad, needle) in [
            (
                json!({"type": "line", "start": [1, 1], "end": [1, 1]}),
                "長さが 0",
            ),
            (
                json!({"type": "circle", "center": [0, 0], "radius": 0}),
                "半径",
            ),
            (
                json!({"type": "circle", "center": [0, 0], "radius": -3}),
                "半径",
            ),
            (
                json!({"type": "arc", "start": [0, 0], "through": [1, 1], "end": [2, 2]}),
                "一直線",
            ),
            (
                json!({"type": "polyline", "vertices": [[0, 0], [1, 1]], "closed": true}),
                "頂点",
            ),
            (
                json!({"type": "line", "start": [0, 0], "end": [2_000_000_000.0, 0]}),
                "上限",
            ),
            (
                json!({"type": "circle", "center": [0, 0], "radius": "1e308*10"}),
                "計算できません",
            ),
            (
                json!({"type": "circle", "center": [0, 0], "radius": "1/0"}),
                "計算できません",
            ),
            (
                json!({"type": "circle", "center": [0, 0], "radius": "幅"}),
                "パラメータ",
            ),
            (
                json!({"type": "circle", "center": [0, 0], "radius": "2 +"}),
                "読めません",
            ),
            (
                json!({"type": "instance", "component": "X", "origin": [0, 0]}),
                "insert_component",
            ),
            (json!({"type": "spline"}), "扱えません"),
            (
                json!({"type": "line", "start": [0, 0], "end": [1, 0], "layer": "0"}),
                "不明な項目",
            ),
            (json!("line"), "オブジェクト"),
        ] {
            let msg = rejected(
                &mut s,
                "add_entities",
                json!({ "entities": [good.clone(), bad.clone()] }),
            );
            assert!(msg.contains(needle), "{bad}: {msg}");
            assert!(
                msg.contains("entities[1]") || msg.contains("オブジェクト"),
                "{msg}"
            );
        }
        rejected(&mut s, "add_entities", json!({ "entities": [] }));
        rejected(&mut s, "add_entities", json!({}));
        let too_many: Vec<Value> = (0..=MAX_SHAPES_PER_CALL).map(|_| good.clone()).collect();
        rejected(&mut s, "add_entities", json!({ "entities": too_many }));
        assert_eq!(s.doc.entities().len(), 0);
    }

    #[test]
    fn add_entities_respects_layers() {
        let dir = TempDir::new("draw-add-layers");
        let mut s = layered(&dir);
        let line = json!([{"type": "line", "start": [0, 0], "end": [1, 0]}]);
        let r = mutate(
            &mut s,
            "add_entities",
            json!({"entities": line, "layer": "WALL"}),
        );
        assert_eq!(r["layer"], "WALL");
        for (layer, needle) in [
            ("LOCK", "ロック中"),
            ("HIDE", "非表示"),
            ("NOPE", "ありません"),
        ] {
            let msg = rejected(
                &mut s,
                "add_entities",
                json!({"entities": line, "layer": layer}),
            );
            assert!(msg.contains(needle), "{layer}: {msg}");
        }
        // 現在レイヤがロック中なら、layer を省いても描かない。
        let lock = s.doc.layers().by_name("LOCK").unwrap();
        s.doc
            .apply(Box::new(cad_core::command::SetCurrentLayer::new(lock)))
            .unwrap();
        let msg = rejected(&mut s, "add_entities", json!({ "entities": line }));
        assert!(msg.contains("ロック中"), "{msg}");
    }

    #[test]
    fn modify_changes_only_the_given_fields() {
        let dir = TempDir::new("draw-modify");
        let mut s = server(&dir);
        let r = ok(
            &mut s,
            "add_entities",
            json!({"entities": [
                {"type": "arc", "center": [0, 0], "radius": 10, "start_angle": 10, "end_angle": 100},
                {"type": "line", "start": [0, 0], "end": [10, 0]},
                {"type": "polyline", "vertices": [[0, 0], [10, 0], [10, 10]]},
                {"type": "xline", "origin": [0, 0], "angle": 30},
            ]}),
        );
        let ids: Vec<String> = serde_json::from_value(r["ids"].clone()).unwrap();
        let before: Vec<Geometry> = s
            .doc
            .entities()
            .iter()
            .map(|(_, e)| e.geom.clone())
            .collect();

        let r = mutate(
            &mut s,
            "modify_entities",
            json!({"changes": [
                {"id": ids[0], "set": {"radius": "5*5"}},
                {"id": ids[1], "set": {"end": [20, "10/2"]}},
                {"id": ids[2], "set": {"closed": true}},
                {"id": ids[3], "set": {"origin": [5, 5]}},
            ]}),
        );
        assert_eq!(r["count"], 4);
        assert_eq!(s.doc.history().undo_name(), Some("MODIFY"));
        let after: Vec<Geometry> = s
            .doc
            .entities()
            .iter()
            .map(|(_, e)| e.geom.clone())
            .collect();
        let (Geometry::Arc(a0), Geometry::Arc(a1)) = (&before[0], &after[0]) else {
            panic!("円弧")
        };
        assert_eq!(a1.radius, 25.0);
        // 変えていない角度は度 ↔ ラジアンを往復せず、ビット単位で元のまま。
        assert_eq!(a1.start_angle.to_bits(), a0.start_angle.to_bits());
        assert_eq!(a1.end_angle.to_bits(), a0.end_angle.to_bits());
        let Geometry::Line(l) = &after[1] else {
            panic!("線分")
        };
        assert_eq!((l.a.x, l.b.x, l.b.y), (0.0, 20.0, 5.0));
        let Geometry::Polyline(p) = &after[2] else {
            panic!("ポリライン")
        };
        assert!(p.closed);
        let (Geometry::Xline(x0), Geometry::Xline(x1)) = (&before[3], &after[3]) else {
            panic!("作図線")
        };
        assert_eq!(x1.direction, x0.direction, "向きは元のまま");

        // 作図線の向きを angle で変える・円弧を 3 点で置き換える。
        mutate(
            &mut s,
            "modify_entities",
            json!({"changes": [
                {"id": ids[3], "set": {"angle": 90}},
                {"id": ids[0], "set": {"start": [10, 0], "through": [0, 10], "end": [-10, 0]}},
            ]}),
        );
        let g = geometry_of(&mut s, &ids[3]);
        assert!(eq_len(g["angle"].as_f64().unwrap(), 90.0));
        let Some(Geometry::Arc(a)) = s.doc.entities().iter().next().map(|(_, e)| e.geom.clone())
        else {
            panic!("円弧")
        };
        assert!(eq_len(a.radius, 10.0) && eq_angle(a.start_angle, 0.0));
    }

    /// get_entities の geometry をそのまま set に渡せる（インスタンスの overrides も。PR #84 レビューの 6）。
    #[test]
    fn modify_accepts_get_entities_output_unchanged() {
        use cad_core::command::{
            DefineComponent, InsertInstance, SetDefinitionParams, SetInstanceOverride,
        };
        use cad_core::component::{ParamDecl, Placement};
        use cad_core::geom::{Line, Point2};
        use cad_core::LayerId;

        let dir = TempDir::new("draw-modify-passthrough");
        let mut s = server(&dir);
        ok(
            &mut s,
            "add_entities",
            json!({"entities": [
                {"type": "line", "start": [0, 0], "end": [3, 4]},
                {"type": "circle", "center": [1, 1], "radius": 2.5},
                {"type": "arc", "center": [0, 0], "radius": 1, "start_angle": 14.3, "end_angle": 114.6},
                {"type": "xline", "origin": [1, 2], "angle": 28.6},
                {"type": "polyline", "vertices": [[0, 0], [1, 0], [1, 1]], "closed": true},
            ]}),
        );
        s.doc
            .apply(Box::new(DefineComponent::new(
                "COMPONENT",
                "BOLT",
                Point2::new(0.0, 0.0),
                vec![Entity::new(
                    Geometry::Line(Line::new(Point2::new(0.0, 0.0), Point2::new(1.0, 0.0))),
                    LayerId::ZERO,
                )],
            )))
            .unwrap();
        let def = s.doc.definitions().by_name("BOLT").unwrap();
        s.doc
            .apply(Box::new(SetDefinitionParams::new(
                "P",
                def,
                vec![ParamDecl::number("長さ", 30.0)],
            )))
            .unwrap();
        s.doc
            .apply(Box::new(InsertInstance::new(
                "INSERT",
                def,
                Placement::new(Point2::new(5.0, 5.0), 0.5, 2.0, true).unwrap(),
                LayerId::ZERO,
            )))
            .unwrap();
        let inst = eid(&s, 5);
        let inst_id =
            crate::ids::resolve_ids(&s.doc, s.tag(), std::slice::from_ref(&inst)).unwrap()[0];
        s.doc
            .apply(Box::new(SetInstanceOverride::set(
                "OVERRIDE",
                inst_id,
                "長さ",
                cad_core::expr::Value::Number(45.0),
            )))
            .unwrap();

        // ラジアンの角度は、度で出して読み戻すと最後のビットがずれうる（0.1 刻みの 20 本のどれかは必ずずれる）。
        let arcs: Vec<Entity> = (1..=20)
            .map(|k| {
                let a = 0.1 * f64::from(k) + 0.012_345_678_9;
                Entity::new(
                    Geometry::Arc(cad_core::geom::Arc::new(
                        Point2::new(0.0, 0.0),
                        1.0,
                        a,
                        a * 2.0,
                    )),
                    LayerId::ZERO,
                )
            })
            .collect();
        assert!(
            arcs.iter().any(|e| {
                let Geometry::Arc(a) = &e.geom else {
                    return false;
                };
                crate::convert::deg_to_rad(crate::convert::rad_to_deg(a.start_angle)).to_bits()
                    != a.start_angle.to_bits()
            }),
            "往復でずれる角度を含めること"
        );
        s.doc
            .apply(Box::new(AddEntities::many("ADD", arcs)))
            .unwrap();

        let list = ok(&mut s, "list_entities", json!({}));
        let ids: Vec<Value> = list["entities"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["id"].clone())
            .collect();
        let got = ok(&mut s, "get_entities", json!({ "ids": ids }));
        assert_eq!(
            got["entities"][5]["geometry"]["overrides"],
            json!({"長さ": 45.0})
        );
        let changes: Vec<Value> = got["entities"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| json!({"id": e["id"], "set": e["geometry"]}))
            .collect();
        let before: Vec<Geometry> = s
            .doc
            .entities()
            .iter()
            .map(|(_, e)| e.geom.clone())
            .collect();
        let bytes_before = cad_core::native::write::write_to_bytes(&s.doc);
        // クライアントとの間は文字列なので、いったん JSON の文字列にして読み戻したものを渡す
        // （serde_json の既定の読み取りでは小数が 1〜2 ULP ずれ、ここで落ちる）。
        let changes: Value = serde_json::from_str(&json!(changes).to_string()).unwrap();
        mutate(&mut s, "modify_entities", json!({ "changes": changes }));
        assert_eq!(
            cad_core::native::write::write_to_bytes(&s.doc),
            bytes_before,
            "get した値をそのまま渡し返しても、形は 1 ビットも変わらない"
        );
        let after: Vec<Geometry> = s
            .doc
            .entities()
            .iter()
            .map(|(_, e)| e.geom.clone())
            .collect();
        for (b, a) in before.iter().zip(&after) {
            match (b, a) {
                (Geometry::Arc(x), Geometry::Arc(y)) => {
                    assert!(
                        eq_angle(x.start_angle, y.start_angle)
                            && eq_angle(x.end_angle, y.end_angle)
                    );
                }
                (Geometry::Xline(x), Geometry::Xline(y)) => {
                    assert!(x.direction.eq_tol(y.direction))
                }
                (Geometry::Instance(x), Geometry::Instance(y)) => {
                    assert_eq!(x.overrides, y.overrides);
                    assert!(eq_angle(x.placement.rotation, y.placement.rotation));
                    assert_eq!(x.placement.flipped, y.placement.flipped);
                }
                _ => assert_eq!(b, a),
            }
        }

        // 上書き・参照先を変えるのは拒む（set_instance_params・insert_component の役目）。配置は変えられる。
        let msg = rejected(
            &mut s,
            "modify_entities",
            json!({"changes": [{"id": inst, "set": {"overrides": {"長さ": 50}}}]}),
        );
        assert!(msg.contains("overrides"), "{msg}");
        assert!(
            msg.contains("set_instance_params"),
            "1d の道具へ案内する: {msg}"
        );
        let msg = rejected(
            &mut s,
            "modify_entities",
            json!({"changes": [{"id": inst, "set": {"component": "NUT"}}]}),
        );
        assert!(msg.contains("コンポーネント"), "{msg}");
        assert!(
            msg.contains("insert_component"),
            "1d の道具へ案内する: {msg}"
        );
        mutate(
            &mut s,
            "modify_entities",
            json!({"changes": [{"id": inst, "set": {"rotation": 90, "scale": 3}}]}),
        );
        let g = geometry_of(&mut s, &inst);
        assert!(eq_len(g["rotation"].as_f64().unwrap(), 90.0));
        assert_eq!(g["overrides"], json!({"長さ": 45.0}), "上書きは残る");
    }

    #[test]
    fn modify_rejects_what_it_cannot_change() {
        let dir = TempDir::new("draw-modify-bad");
        let mut s = layered(&dir);
        let r = ok(
            &mut s,
            "add_entities",
            json!({"entities": [
                {"type": "circle", "center": [0, 0], "radius": 5},
                {"type": "polyline", "vertices": [[0, 0], [10, 0], [10, 10]]},
            ]}),
        );
        let (circle, poly) = (r["ids"][0].clone(), r["ids"][1].clone());
        let line = eid(&s, 0);
        let other_drawing = format!("d{:06x}-{}e0g0", s.session, s.serial + 1);
        for (changes, needle) in [
            (json!([{"id": circle, "set": {"radius": 0}}]), "半径"),
            (
                json!([{"id": circle, "set": {"radius": "1/0"}}]),
                "計算できません",
            ),
            (
                json!([{"id": circle, "set": {"center": [5_000_000_000.0, 0]}}]),
                "上限",
            ),
            (json!([{"id": circle, "set": {"type": "arc"}}]), "種類"),
            (
                json!([{"id": circle, "set": {"start": [0, 0]}}]),
                "不明な項目",
            ),
            (json!([{"id": circle, "set": {}}]), "変える項目"),
            (json!([{"id": circle, "radius": 3}]), "不明な項目"),
            (
                json!([{"id": poly, "set": {"vertices": [[0, 0], [1, 1]]}}]),
                "頂点の数",
            ),
            (
                json!([{"id": eid(&s, 2), "set": {"end": [20, 5]}}]),
                "ロック中",
            ),
            (
                json!([{"id": eid(&s, 3), "set": {"end": [30, 5]}}]),
                "非表示",
            ),
            (
                json!([{"id": eid(&s, 99), "set": {"end": [1, 5]}}]),
                "ありません",
            ),
            (
                json!([{"id": other_drawing, "set": {"end": [1, 5]}}]),
                "別の図面",
            ),
            (
                json!([{"id": "e0", "set": {"end": [1, 5]}}]),
                "形が違います",
            ),
            (
                json!([{"id": line, "set": {"end": [1, 5]}}, {"id": line, "set": {"end": [2, 5]}}]),
                "2 回",
            ),
            // 1 つでもだめなら、正しい変更も含めて何もしない。
            (
                json!([{"id": line, "set": {"end": [1, 5]}}, {"id": circle, "set": {"radius": -1}}]),
                "何も変えていません",
            ),
        ] {
            let msg = rejected(&mut s, "modify_entities", json!({ "changes": changes }));
            assert!(msg.contains(needle), "{changes}: {msg}");
        }
    }

    #[test]
    fn delete_entities_keeps_ids_through_undo() {
        let dir = TempDir::new("draw-delete");
        let mut s = layered(&dir);
        let (a, b) = (eid(&s, 0), eid(&s, 1));
        let r = mutate(&mut s, "delete_entities", json!({ "ids": [a, b] }));
        assert_eq!(r["deleted"], 2);
        assert_eq!(s.doc.history().undo_name(), Some("ERASE"));
        err(&mut s, "get_entities", json!({ "ids": [a] }));
        ok(&mut s, "undo", json!({}));
        ok(&mut s, "get_entities", json!({ "ids": [a, b] }));

        for ids in [
            json!([eid(&s, 0), eid(&s, 2)]),
            json!([eid(&s, 3)]),
            json!([eid(&s, 0), eid(&s, 0)]),
            json!([eid(&s, 50)]),
            json!([]),
        ] {
            rejected(&mut s, "delete_entities", json!({ "ids": ids }));
        }
        assert_eq!(s.doc.entities().len(), 4);
    }
}
