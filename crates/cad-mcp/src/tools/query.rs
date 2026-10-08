//! 照会の道具: `list_entities` / `get_entities` / `list_layers` / `list_components`。
//!
//! どれも図面を変えない。一覧は件数に上限を置き、続きは `offset` で取らせる。

use std::collections::HashMap;

use cad_core::LayerId;
use serde_json::{json, Value};

use super::{Args, Tool, ToolResult};
use crate::convert::{
    aabb_from_json, component_to_json, entity_summary_json, entity_to_json, geometry_type,
    layer_to_json, ENTITY_TYPES,
};
use crate::ids::resolve_ids;
use crate::limits::{DEFAULT_LIST_LIMIT, MAX_IDS_PER_CALL, MAX_LIST_LIMIT};
use crate::server::Server;

pub(super) const LIST_ENTITIES: Tool = Tool {
    name: "list_entities",
    title: "図形の一覧",
    description: "図形の一覧（作成順 = 描画順）。各図形は id・type・layer・bbox の要約だけで、形の中身は get_entities で取る。\
layer（レイヤ名）・type・bbox（この範囲と交わる図形。作図線はどの範囲とも交わる）で絞り込める。\
一度に返すのは limit 件まで。続きは next_offset を offset に渡す。",
    schema: || {
        (
            json!({
                "layer": { "type": "string", "description": "このレイヤの図形だけ（レイヤ名）" },
                "type": { "type": "string", "enum": ENTITY_TYPES, "description": "この種類の図形だけ" },
                "bbox": {
                    "type": "object",
                    "description": "この範囲と交わる図形だけ。{\"min\": {\"x\":..,\"y\":..}, \"max\": {\"x\":..,\"y\":..}}",
                    "properties": { "min": point_schema(), "max": point_schema() },
                    "required": ["min", "max"],
                },
                "limit": {
                    "type": "integer", "minimum": 1, "maximum": MAX_LIST_LIMIT,
                    "description": format!("返す件数の上限（既定 {DEFAULT_LIST_LIMIT}）"),
                },
                "offset": { "type": "integer", "minimum": 0, "description": "先頭から飛ばす件数（既定 0）" },
            }),
            &[],
        )
    },
    read_only: true,
    destructive: false,
    idempotent: true,
    run: list_entities,
};

pub(super) const GET_ENTITIES: Tool = Tool {
    name: "get_entities",
    title: "図形の詳細",
    description: "ID で指定した図形の全体（レイヤ・色・グループ・境界ボックス・形）。角度は度。\
形は type ごとに line{start,end} / circle{center,radius} / arc{center,radius,start_angle,end_angle（反時計回り）} / \
xline{origin,angle,direction} / polyline{vertices,closed} / instance{component,origin,rotation,scale,flipped,overrides}。\
1 つでも使えない ID（形が違う・別の図面・存在しない）があれば全体を拒む。",
    schema: || {
        (
            json!({
                "ids": {
                    "type": "array",
                    "items": { "type": "string" },
                    "minItems": 1,
                    "maxItems": MAX_IDS_PER_CALL,
                    "description": "図形 ID（list_entities などが返した d<起動の印>-<図面>e<番号>g<世代>）の配列",
                },
            }),
            &["ids"],
        )
    },
    read_only: true,
    destructive: false,
    idempotent: true,
    run: get_entities,
};

pub(super) const LIST_LAYERS: Tool = Tool {
    name: "list_layers",
    title: "レイヤの一覧",
    description: "レイヤの一覧: 名前・色（ACI 番号）・表示・ロック・線種・現在レイヤか・図形の数。",
    schema: || (json!({}), &[]),
    read_only: true,
    destructive: false,
    idempotent: true,
    run: list_layers,
};

pub(super) const LIST_COMPONENTS: Tool = Tool {
    name: "list_components",
    title: "コンポーネントの一覧",
    description: "コンポーネント（パラメトリックなブロック）定義の一覧: 名前・基点・中の図形の数・図面に直接置かれたインスタンスの数・\
パラメータ（名前・型・既定値の式・範囲）・束縛（中の図形の添字 entity と種類 entity_type、対象の項目 field、式。\
field は図形の JSON の項目名で start.x / center.y / radius / start_angle / vertices[3].x など。式の中の角度は度）。",
    schema: || (json!({}), &[]),
    read_only: true,
    destructive: false,
    idempotent: true,
    run: list_components,
};

fn point_schema() -> Value {
    json!({
        "type": "object",
        "properties": { "x": { "type": "number" }, "y": { "type": "number" } },
        "required": ["x", "y"],
    })
}

fn list_entities(s: &mut Server, a: &Args) -> ToolResult {
    let doc = &s.doc;
    let layer: Option<LayerId> = match a.opt_str("layer")? {
        None => None,
        Some(name) => Some(doc.layers().by_name(name).ok_or_else(|| {
            format!("レイヤ {name} はありません（list_layers で一覧を見られます）")
        })?),
    };
    let ty = match a.opt_str("type")? {
        None => None,
        Some(t) if ENTITY_TYPES.contains(&t) => Some(t),
        Some(t) => {
            return Err(format!(
                "type {t} は扱えません（{}）",
                ENTITY_TYPES.join(" / ")
            ))
        }
    };
    let region = a
        .value("bbox")
        .map(|v| aabb_from_json(v, "bbox"))
        .transpose()?;
    let limit = a.usize_in("limit", DEFAULT_LIST_LIMIT, 1, MAX_LIST_LIMIT)?;
    let offset = a.usize_in("offset", 0, 0, usize::MAX)?;

    let defs = doc.definitions();
    let matches = doc.entities().iter().filter(|(_, e)| {
        layer.is_none_or(|l| e.layer == l)
            && ty.is_none_or(|t| geometry_type(&e.geom) == t)
            && region.is_none_or(|r| e.bbox(defs).intersects(&r))
    });
    let mut total = 0usize;
    let mut entities = Vec::new();
    for (i, (id, e)) in matches.enumerate() {
        total += 1;
        if i >= offset && entities.len() < limit {
            entities.push(entity_summary_json(doc, s.tag(), id, e));
        }
    }
    let next = offset.saturating_add(entities.len());
    Ok(json!({
        "drawing": s.tag().name(),
        "total": total,
        "offset": offset,
        "count": entities.len(),
        "next_offset": (next < total).then_some(next),
        "entities": entities,
    }))
}

fn get_entities(s: &mut Server, a: &Args) -> ToolResult {
    let ids = a.string_list("ids", MAX_IDS_PER_CALL)?;
    let resolved = resolve_ids(&s.doc, s.tag(), &ids)?;
    let entities: Vec<Value> = resolved
        .into_iter()
        .filter_map(|id| {
            s.doc
                .entities()
                .get(id)
                .map(|e| entity_to_json(&s.doc, s.tag(), id, e))
        })
        .collect();
    Ok(json!({ "drawing": s.tag().name(), "entities": entities }))
}

fn list_layers(s: &mut Server, _: &Args) -> ToolResult {
    let doc = &s.doc;
    let counts = entity_counts_by_layer(doc);
    let current = doc.layers().current();
    let layers: Vec<Value> = doc
        .layers()
        .iter()
        .map(|(id, l)| layer_to_json(l, id == current, counts.get(&id).copied().unwrap_or(0)))
        .collect();
    Ok(json!({ "drawing": s.tag().name(), "layers": layers }))
}

/// レイヤごとの図形の数（図面に直接置かれたものだけ）。
pub(super) fn entity_counts_by_layer(doc: &cad_core::Document) -> HashMap<LayerId, usize> {
    let mut counts: HashMap<LayerId, usize> = HashMap::new();
    for (_, e) in doc.entities().iter() {
        *counts.entry(e.layer).or_default() += 1;
    }
    counts
}

fn list_components(s: &mut Server, _: &Args) -> ToolResult {
    use cad_core::Geometry;

    let doc = &s.doc;
    let mut instances = HashMap::new();
    for (_, e) in doc.entities().iter() {
        if let Geometry::Instance(i) = &e.geom {
            *instances.entry(i.definition).or_insert(0usize) += 1;
        }
    }
    let components: Vec<Value> = doc
        .definitions()
        .iter()
        .map(|(id, d)| component_to_json(d, instances.get(&id).copied().unwrap_or(0)))
        .collect();
    Ok(json!({ "drawing": s.tag().name(), "components": components }))
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{eid, err, ok, server};
    use super::*;
    use crate::test_util::TempDir;
    use cad_core::command::{
        AddEntities, AddLayer, DefineComponent, InsertInstance, SetBinding, SetDefinitionParams,
    };
    use cad_core::component::{Binding, ParamDecl, Placement, Slot};
    use cad_core::geom::{Circle, Line, Point2, Xline};
    use cad_core::{AciColor, Entity, Geometry};

    fn add(s: &mut Server, g: Geometry, layer: LayerId) {
        s.doc
            .apply(Box::new(AddEntities::one("ADD", Entity::new(g, layer))))
            .unwrap();
    }

    fn line(x: f64) -> Geometry {
        Geometry::Line(Line::new(Point2::new(x, 0.0), Point2::new(x + 1.0, 1.0)))
    }

    /// 線分 5 本（レイヤ 0）・円 1 つ（レイヤ WALL）・作図線 1 本。
    fn populated(dir: &TempDir) -> Server {
        let mut s = server(dir);
        for i in 0..5 {
            add(&mut s, line(f64::from(i) * 10.0), LayerId::ZERO);
        }
        s.doc
            .apply(Box::new(AddLayer::new("WALL", AciColor::RED)))
            .unwrap();
        let wall = s.doc.layers().by_name("WALL").unwrap();
        add(
            &mut s,
            Geometry::Circle(Circle::new(Point2::new(100.0, 100.0), 5.0)),
            wall,
        );
        add(
            &mut s,
            Geometry::Xline(Xline::vertical(Point2::new(-50.0, 0.0))),
            LayerId::ZERO,
        );
        s
    }

    #[test]
    fn list_entities_pages_through_everything() {
        let dir = TempDir::new("query-page");
        let mut s = populated(&dir);
        let r = ok(&mut s, "list_entities", json!({"limit": 3}));
        assert_eq!(r["total"], 7);
        assert_eq!(r["count"], 3);
        assert_eq!(r["next_offset"], 3);
        assert_eq!(r["entities"][0]["id"], eid(&s, 0));
        assert_eq!(r["entities"][0]["type"], "line");
        assert_eq!(r["entities"][0]["layer"], "0");

        let r = ok(&mut s, "list_entities", json!({"limit": 3, "offset": 6}));
        assert_eq!(r["count"], 1);
        assert_eq!(r["next_offset"], Value::Null);
        assert_eq!(r["entities"][0]["type"], "xline");
        assert_eq!(r["entities"][0]["bbox"], Value::Null, "作図線の範囲は無限");

        let r = ok(&mut s, "list_entities", json!({"offset": 100}));
        assert_eq!(r["count"], 0);
    }

    #[test]
    fn list_entities_filters() {
        let dir = TempDir::new("query-filter");
        let mut s = populated(&dir);
        let r = ok(&mut s, "list_entities", json!({"layer": "WALL"}));
        assert_eq!(r["total"], 1);
        assert_eq!(r["entities"][0]["type"], "circle");

        let r = ok(&mut s, "list_entities", json!({"type": "line"}));
        assert_eq!(r["total"], 5);

        // 線分 0 本目（0..1）と 1 本目（10..11）を含む範囲。作図線はどこでも交わる。
        let r = ok(
            &mut s,
            "list_entities",
            json!({"bbox": {"min": {"x": -1, "y": -1}, "max": {"x": 10.5, "y": 0.5}}}),
        );
        let types: Vec<&str> = r["entities"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["type"].as_str().unwrap())
            .collect();
        assert_eq!(types, ["line", "line", "xline"]);

        let r = ok(
            &mut s,
            "list_entities",
            json!({"type": "line", "layer": "WALL"}),
        );
        assert_eq!(r["total"], 0);
    }

    #[test]
    fn list_entities_rejects_bad_filters() {
        let dir = TempDir::new("query-bad");
        let mut s = populated(&dir);
        for bad in [
            json!({"layer": "NOPE"}),
            json!({"type": "spline"}),
            json!({"limit": 0}),
            json!({"limit": MAX_LIST_LIMIT + 1}),
            json!({"offset": -1}),
            json!({"bbox": {"min": [0, 0]}}),
        ] {
            err(&mut s, "list_entities", bad);
        }
    }

    #[test]
    fn get_entities_returns_full_geometry_in_degrees() {
        let dir = TempDir::new("query-get");
        let mut s = populated(&dir);
        let ids = json!({"ids": [eid(&s, 5), eid(&s, 0)]});
        let r = ok(&mut s, "get_entities", ids);
        let circle = &r["entities"][0];
        assert_eq!(circle["id"], eid(&s, 5));
        assert_eq!(circle["layer"], "WALL");
        assert_eq!(circle["color"], "bylayer");
        assert_eq!(circle["group"], Value::Null);
        assert_eq!(circle["geometry"]["type"], "circle");
        assert_eq!(circle["geometry"]["radius"], 5.0);
        let line = &r["entities"][1];
        assert_eq!(line["geometry"]["start"], json!({"x": 0.0, "y": 0.0}));

        let ids = json!({"ids": [eid(&s, 6)]});
        let r = ok(&mut s, "get_entities", ids);
        let xline = &r["entities"][0]["geometry"];
        assert_eq!(xline["type"], "xline");
        assert!((xline["angle"].as_f64().unwrap() - 90.0).abs() < cad_core::geom::EPS_LEN);
    }

    /// 図面を入れ替えると前の ID は拒まれる（同じ番号の図形があっても）。
    #[test]
    fn get_entities_rejects_ids_of_another_drawing() {
        let dir = TempDir::new("query-other");
        let mut s = populated(&dir);
        let old = eid(&s, 0);
        ok(&mut s, "save_drawing", json!({"path": "a.ymc"}));
        ok(&mut s, "open_drawing", json!({"path": "a.ymc"}));
        let msg = err(&mut s, "get_entities", json!({ "ids": [old] }));
        assert!(msg.contains("別の図面"), "{msg}");
        let new = eid(&s, 0);
        assert_ne!(old, new);
        let r = ok(&mut s, "get_entities", json!({ "ids": [new.clone()] }));
        assert_eq!(r["entities"][0]["id"], new);
    }

    #[test]
    fn get_entities_rejects_bad_lists() {
        let dir = TempDir::new("query-get-bad");
        let mut s = populated(&dir);
        err(&mut s, "get_entities", json!({}));
        err(&mut s, "get_entities", json!({"ids": []}));
        let one = eid(&s, 0);
        err(&mut s, "get_entities", json!({ "ids": one }));
        let ids = json!({"ids": [eid(&s, 0), eid(&s, 99)]});
        err(&mut s, "get_entities", ids);
        let too_many: Vec<String> = (0..=MAX_IDS_PER_CALL).map(|_| eid(&s, 0)).collect();
        err(&mut s, "get_entities", json!({ "ids": too_many }));
    }

    #[test]
    fn list_layers_counts_entities() {
        let dir = TempDir::new("query-layers");
        let mut s = populated(&dir);
        let r = ok(&mut s, "list_layers", json!({}));
        let layers = r["layers"].as_array().unwrap();
        assert_eq!(layers.len(), 2);
        assert_eq!(layers[0]["name"], "0");
        assert_eq!(layers[0]["entity_count"], 6);
        assert_eq!(layers[0]["current"], true);
        assert_eq!(layers[0]["linetype"], "continuous");
        assert_eq!(layers[1]["name"], "WALL");
        assert_eq!(layers[1]["color"], 1);
        assert_eq!(layers[1]["entity_count"], 1);
    }

    #[test]
    fn list_components_describes_definitions() {
        let dir = TempDir::new("query-comps");
        let mut s = server(&dir);
        let r = ok(&mut s, "list_components", json!({}));
        assert_eq!(r["components"], json!([]));

        s.doc
            .apply(Box::new(DefineComponent::new(
                "COMPONENT",
                "BOLT",
                Point2::new(0.0, 0.0),
                vec![Entity::new(line(0.0), LayerId::ZERO)],
            )))
            .unwrap();
        let def = s.doc.definitions().by_name("BOLT").unwrap();
        s.doc
            .apply(Box::new(SetDefinitionParams::new(
                "PARAMS",
                def,
                vec![ParamDecl::number("長さ", 30.0)],
            )))
            .unwrap();
        s.doc
            .apply(Box::new(SetBinding::new(
                "BIND",
                def,
                Binding::new(0, Slot::LineBx, cad_core::expr::parse("長さ + 1").unwrap()),
            )))
            .unwrap();
        s.doc
            .apply(Box::new(InsertInstance::new(
                "INSERT",
                def,
                Placement::at(Point2::new(5.0, 5.0)),
                LayerId::ZERO,
            )))
            .unwrap();
        let r = ok(&mut s, "list_components", json!({}));
        let c = &r["components"][0];
        assert_eq!(c["name"], "BOLT");
        assert_eq!(c["entity_count"], 1);
        assert_eq!(c["instance_count"], 1);
        assert_eq!(c["params"][0]["name"], "長さ");
        assert_eq!(c["params"][0]["type"], "number");
        // 束縛の対象は Debug の綴り（LineBx）ではなく、図形の JSON の項目名（PR #84 レビューの 5）。
        let b = &c["bindings"][0];
        assert_eq!(b["entity"], 0);
        assert_eq!(b["entity_type"], "line");
        assert_eq!(b["field"], "end.x");
        assert_eq!(b["expr"], "長さ + 1");

        let list = ok(&mut s, "list_entities", json!({"type": "instance"}));
        let id = list["entities"][0]["id"].clone();
        let got = ok(&mut s, "get_entities", json!({ "ids": [id] }));
        assert_eq!(got["entities"][0]["geometry"]["component"], "BOLT");
        assert_eq!(got["entities"][0]["geometry"]["rotation"], 0.0);
    }
}
