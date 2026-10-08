//! 変形の道具: `move_entities` / `rotate_entities` / `scale_entities` / `mirror_entities` /
//! `set_entity_layer`。
//!
//! その場で変える版と複製を作る版は、`cad-core` の別のコマンドを使う（ADR-0019: Undo の中身が
//! 根本的に違う）。どちらも結果の形を `Geometry` の同じ変換で試しに求め、適用の前に確かめる
//! （[`super::draw::precheck`]）。共通の約束は [`super::mutate`]。

use cad_core::command::{
    CopyEntities, MirrorCopyEntities, MirrorEntities, MoveEntities, MoveEntitiesToLayer,
    RotateCopyEntities, RotateEntities, ScaleCopyEntities, ScaleEntities,
};
use cad_core::geom::{Line, Point2, Vec2};
use cad_core::{Command, EntityId};
use serde_json::{json, Value};

use super::draw::{ids_schema, precheck};
use super::mutate::{
    apply, created_since, destination_layer, editable_targets, ensure_capacity, id_strings,
    max_slot,
};
use super::{Args, Tool, ToolResult};
use crate::convert::{deg_to_rad, number_from_json, point_from_json};
use crate::limits::MAX_IDS_PER_CALL;
use crate::server::Server;

pub(super) const MOVE_ENTITIES: Tool = Tool {
    name: "move_entities",
    title: "図形を動かす",
    description: "図形を delta だけ平行移動する。copy: true なら元を残して動かした複製を作り、新しい ID を created に返す\
（ids と同じ順）。1 回の呼び出しが undo 1 回ぶん。数値は式の文字列も可。非表示・ロック中のレイヤの図形があれば何もしない。",
    schema: || {
        (
            with_ids(json!({
                "delta": point_schema("移動量 {\"x\":..,\"y\":..} か [dx, dy]"),
                "copy": { "type": "boolean", "description": "true なら元を残して複製を作る（既定 false）" },
            })),
            &["ids", "delta"],
        )
    },
    read_only: false,
    destructive: true,
    idempotent: false,
    run: move_entities,
};

pub(super) const ROTATE_ENTITIES: Tool = Tool {
    name: "rotate_entities",
    title: "図形を回す",
    description: "図形を center のまわりに angle_deg 度だけ回す（正が反時計回り）。copy: true なら元を残して複製を作り、\
新しい ID を created に返す（ids と同じ順）。1 回の呼び出しが undo 1 回ぶん。非表示・ロック中のレイヤの図形があれば何もしない。",
    schema: || {
        (
            with_ids(json!({
                "center": point_schema("回転の中心"),
                "angle_deg": number_schema("回す角度（度、反時計回りが正）"),
                "copy": { "type": "boolean", "description": "true なら元を残して複製を作る（既定 false）" },
            })),
            &["ids", "center", "angle_deg"],
        )
    },
    read_only: false,
    destructive: true,
    idempotent: false,
    run: rotate_entities,
};

pub(super) const SCALE_ENTITIES: Tool = Tool {
    name: "scale_entities",
    title: "図形を拡大縮小する",
    description: "図形を center を基点に factor 倍にする（factor は 0 より大きい数。裏返すなら mirror_entities）。\
copy: true なら元を残して複製を作り、新しい ID を created に返す（ids と同じ順）。1 回の呼び出しが undo 1 回ぶん。\
潰れて長さ 0 になる・大きすぎる結果や、非表示・ロック中のレイヤの図形があれば何もしない。",
    schema: || {
        (
            with_ids(json!({
                "center": point_schema("拡大縮小の基点"),
                "factor": number_schema("倍率（0 より大きい）"),
                "copy": { "type": "boolean", "description": "true なら元を残して複製を作る（既定 false）" },
            })),
            &["ids", "center", "factor"],
        )
    },
    read_only: false,
    destructive: true,
    idempotent: false,
    run: scale_entities,
};

pub(super) const MIRROR_ENTITIES: Tool = Tool {
    name: "mirror_entities",
    title: "図形を鏡に映す",
    description: "図形を axis_a と axis_b を通る直線で鏡に映す（円弧の向き・インスタンスの反転も正しく扱う）。\
keep_original: true なら元を残して鏡像の複製を作り、新しい ID を created に返す（ids と同じ順）。既定は元を鏡像に置き換える。\
1 回の呼び出しが undo 1 回ぶん。axis_a と axis_b が同じ点なら拒む。非表示・ロック中のレイヤの図形があれば何もしない。",
    schema: || {
        (
            with_ids(json!({
                "axis_a": point_schema("鏡の直線が通る 1 点目"),
                "axis_b": point_schema("鏡の直線が通る 2 点目"),
                "keep_original": { "type": "boolean", "description": "true なら元を残して鏡像の複製を作る（既定 false）" },
            })),
            &["ids", "axis_a", "axis_b"],
        )
    },
    read_only: false,
    destructive: true,
    idempotent: false,
    run: mirror_entities,
};

pub(super) const SET_ENTITY_LAYER: Tool = Tool {
    name: "set_entity_layer",
    title: "図形のレイヤを変える",
    description: "図形を別のレイヤ（名前）へ移す。形・色・ID は変わらない。1 回の呼び出しが undo 1 回ぶん。\
移す元・移す先のどちらかが非表示・ロック中なら何もしない（先に update_layer で表示する・ロックを外す）。",
    schema: || {
        (
            with_ids(json!({
                "layer": { "type": "string", "description": "移す先のレイヤの名前" },
            })),
            &["ids", "layer"],
        )
    },
    read_only: false,
    destructive: true,
    idempotent: true,
    run: set_entity_layer,
};

fn with_ids(mut props: Value) -> Value {
    if let (Value::Object(m), Value::Object(ids)) = (&mut props, ids_schema("対象の図形 ID の配列"))
    {
        m.extend(ids);
    }
    props
}

fn point_schema(description: &str) -> Value {
    json!({
        "description": format!("{description}。数値は式の文字列も可"),
        "anyOf": [
            {
                "type": "object",
                "properties": { "x": { "type": ["number", "string"] }, "y": { "type": ["number", "string"] } },
                "required": ["x", "y"],
            },
            { "type": "array", "items": { "type": ["number", "string"] }, "minItems": 2, "maxItems": 2 },
        ],
    })
}

fn number_schema(description: &str) -> Value {
    json!({
        "type": ["number", "string"],
        "description": format!("{description}。数値か式の文字列（例 \"90/4\"）"),
    })
}

/// 必須の点の引数。
fn req_point(a: &Args, key: &str) -> Result<Point2, String> {
    point_from_json(
        a.value(key)
            .ok_or_else(|| format!("{key} を指定してください"))?,
        key,
    )
}

/// 必須の数値の引数（式の文字列も可）。
fn req_number(a: &Args, key: &str) -> Result<f64, String> {
    number_from_json(
        a.value(key)
            .ok_or_else(|| format!("{key} を指定してください"))?,
        key,
    )
}

/// 変形を適用し、結果の JSON を返す。`copy` なら複製の新しい ID を `created` に入れる。
fn finish(
    s: &mut Server,
    targets: &[EntityId],
    copy: bool,
    command: Box<dyn Command>,
    name: &'static str,
) -> ToolResult {
    if copy {
        ensure_capacity(&s.doc, targets.len())?;
    }
    let before = max_slot(&s.doc);
    apply(s, name, vec![command])?;
    let created = if copy {
        created_since(&s.doc, before)
    } else {
        Vec::new()
    };
    Ok(json!({
        "drawing": s.tag().name(),
        "ids": id_strings(s, targets),
        "count": targets.len(),
        "copied": copy,
        "created": id_strings(s, &created),
    }))
}

fn targets(s: &Server, a: &Args) -> Result<Vec<EntityId>, String> {
    let raw = a.string_list("ids", MAX_IDS_PER_CALL)?;
    editable_targets(s, &raw)
}

fn move_entities(s: &mut Server, a: &Args) -> ToolResult {
    let d = req_point(a, "delta")?;
    let delta = Vec2::new(d.x, d.y);
    let copy = a.bool_or("copy", false)?;
    let ids = targets(s, a)?;
    precheck(s, &ids, |g| g.translated(delta))?;
    let command: Box<dyn Command> = if copy {
        Box::new(CopyEntities::new("COPY", ids.clone(), delta))
    } else {
        Box::new(MoveEntities::new("MOVE", ids.clone(), delta))
    };
    finish(s, &ids, copy, command, "MOVE")
}

fn rotate_entities(s: &mut Server, a: &Args) -> ToolResult {
    let center = req_point(a, "center")?;
    let angle = deg_to_rad(req_number(a, "angle_deg")?);
    let copy = a.bool_or("copy", false)?;
    let ids = targets(s, a)?;
    precheck(s, &ids, |g| g.rotated(center, angle))?;
    let command: Box<dyn Command> = if copy {
        Box::new(RotateCopyEntities::new(
            "ROTATE",
            ids.clone(),
            center,
            angle,
        ))
    } else {
        Box::new(RotateEntities::new("ROTATE", ids.clone(), center, angle))
    };
    finish(s, &ids, copy, command, "ROTATE")
}

fn scale_entities(s: &mut Server, a: &Args) -> ToolResult {
    let center = req_point(a, "center")?;
    let factor = req_number(a, "factor")?;
    // ADR-0019: 0 は図形を点に潰し、負は鏡像になる。どちらも拡大縮小としては事故なので拒む。
    // `Geometry::scaled` は 0 で元の形を返す（最後の防波堤）ので、ここで止めないと「何も起きずに成功」になる。
    if factor <= 0.0 {
        return Err(format!(
            "factor は 0 より大きい数で指定してください（受け取った値: {factor}）。裏返すなら mirror_entities を使います"
        ));
    }
    let copy = a.bool_or("copy", false)?;
    let ids = targets(s, a)?;
    precheck(s, &ids, |g| g.scaled(center, factor))?;
    let command: Box<dyn Command> = if copy {
        Box::new(ScaleCopyEntities::new("SCALE", ids.clone(), center, factor))
    } else {
        Box::new(ScaleEntities::new("SCALE", ids.clone(), center, factor))
    };
    finish(s, &ids, copy, command, "SCALE")
}

fn mirror_entities(s: &mut Server, a: &Args) -> ToolResult {
    let axis = Line::new(req_point(a, "axis_a")?, req_point(a, "axis_b")?);
    // `Geometry::mirrored` は退化した軸で元の形を返す（最後の防波堤）ので、ここで止める。
    if axis.is_degenerate() {
        return Err("axis_a と axis_b が同じ点です。鏡の直線が決まりません".to_owned());
    }
    let keep = a.bool_or("keep_original", false)?;
    let ids = targets(s, a)?;
    precheck(s, &ids, |g| g.mirrored(&axis))?;
    let command: Box<dyn Command> = if keep {
        Box::new(MirrorCopyEntities::new("MIRROR", ids.clone(), axis))
    } else {
        Box::new(MirrorEntities::new("MIRROR", ids.clone(), axis))
    };
    finish(s, &ids, keep, command, "MIRROR")
}

fn set_entity_layer(s: &mut Server, a: &Args) -> ToolResult {
    let name = a.req_str("layer")?;
    let ids = targets(s, a)?;
    let dest = destination_layer(s, Some(name))?;
    apply(
        s,
        "LAYER_MOVE_ENTITIES",
        vec![Box::new(MoveEntitiesToLayer::new(ids.clone(), dest))],
    )?;
    Ok(json!({
        "drawing": s.tag().name(),
        "ids": id_strings(s, &ids),
        "count": ids.len(),
        "layer": name,
    }))
}

#[cfg(test)]
mod tests {
    use super::super::draw::tests::layered;
    use super::super::test_support::{eid, mutate, ok, rejected};
    use super::*;
    use crate::test_util::TempDir;
    use cad_core::geom::tolerance::{eq_angle, eq_len};
    use cad_core::Geometry;

    /// 線分 (0,0)-(10,0) を 1 本足して、その ID を返す。
    fn add_line(s: &mut Server) -> String {
        let r = ok(
            s,
            "add_entities",
            json!({"entities": [{"type": "line", "start": [0, 0], "end": [10, 0]}]}),
        );
        r["ids"][0].as_str().unwrap().to_owned()
    }

    fn line_of(s: &mut Server, id: &str) -> (Point2, Point2) {
        let ids = crate::ids::resolve_ids(&s.doc, s.tag(), &[id.to_owned()]).unwrap();
        let Some(Geometry::Line(l)) = s.doc.entities().get(ids[0]).map(|e| e.geom.clone()) else {
            panic!("線分のはず")
        };
        (l.a, l.b)
    }

    fn close(p: Point2, x: f64, y: f64) -> bool {
        eq_len(p.x, x) && eq_len(p.y, y)
    }

    #[test]
    fn move_in_place_and_as_copy() {
        let dir = TempDir::new("xf-move");
        let mut s = layered(&dir);
        let id = add_line(&mut s);
        let r = mutate(
            &mut s,
            "move_entities",
            json!({"ids": [id], "delta": {"x": "2*5", "y": 3}}),
        );
        assert_eq!(r["copied"], false);
        assert_eq!(r["created"], json!([]));
        assert_eq!(s.doc.history().undo_name(), Some("MOVE"));
        let (a, b) = line_of(&mut s, &id);
        assert!(close(a, 10.0, 3.0) && close(b, 20.0, 3.0));

        // 複製: 元は残り、新しい ID が ids と同じ順で返る。
        let first = eid(&s, 0);
        let r = ok(
            &mut s,
            "move_entities",
            json!({"ids": [id, first], "delta": [0, 100], "copy": true}),
        );
        assert_eq!(s.doc.history().undo_name(), Some("COPY"));
        let created: Vec<String> = serde_json::from_value(r["created"].clone()).unwrap();
        assert_eq!(created.len(), 2);
        let (a, _) = line_of(&mut s, &created[0]);
        assert!(close(a, 10.0, 103.0), "1 つ目は id の複製");
        let (a, _) = line_of(&mut s, &created[1]);
        assert!(close(a, 0.0, 100.0), "2 つ目は e0 の複製");
        let (a, _) = line_of(&mut s, &id);
        assert!(close(a, 10.0, 3.0), "元は動かない");
        ok(&mut s, "undo", json!({}));
        assert_eq!(s.doc.entities().len(), 5, "undo 1 回で複製が両方消える");
        mutate(
            &mut s,
            "move_entities",
            json!({"ids": [id], "delta": [1, 1], "copy": true}),
        );
    }

    #[test]
    fn rotate_scale_mirror() {
        let dir = TempDir::new("xf-rsm");
        let mut s = layered(&dir);
        let id = add_line(&mut s);
        mutate(
            &mut s,
            "rotate_entities",
            json!({"ids": [id], "center": [0, 0], "angle_deg": "180/2"}),
        );
        let (a, b) = line_of(&mut s, &id);
        assert!(
            close(a, 0.0, 0.0) && close(b, 0.0, 10.0),
            "度で 90°: {a:?} {b:?}"
        );

        mutate(
            &mut s,
            "scale_entities",
            json!({"ids": [id], "center": [0, 0], "factor": 2.5}),
        );
        let (_, b) = line_of(&mut s, &id);
        assert!(close(b, 0.0, 25.0));

        mutate(
            &mut s,
            "mirror_entities",
            json!({"ids": [id], "axis_a": [0, 0], "axis_b": [1, 1]}),
        );
        let (_, b) = line_of(&mut s, &id);
        assert!(close(b, 25.0, 0.0), "y = x で鏡に映す: {b:?}");

        // 複製の版。
        for (tool, args) in [
            (
                "rotate_entities",
                json!({"center": [0, 0], "angle_deg": 45, "copy": true}),
            ),
            (
                "scale_entities",
                json!({"center": [0, 0], "factor": 0.5, "copy": true}),
            ),
            (
                "mirror_entities",
                json!({"axis_a": [0, 0], "axis_b": [0, 1], "keep_original": true}),
            ),
        ] {
            let mut args = args;
            args["ids"] = json!([id]);
            let before = s.doc.entities().len();
            let r = mutate(&mut s, tool, args);
            assert_eq!(r["copied"], true);
            assert_eq!(r["created"].as_array().unwrap().len(), 1);
            assert_eq!(s.doc.entities().len(), before + 1, "{tool}");
        }
        let (_, b) = line_of(&mut s, &id);
        assert!(close(b, 25.0, 0.0), "複製では元は変わらない");
    }

    /// 円弧の鏡像は向きを保つ（開始角と終了角が入れ替わる。ADR-0020）。cad-core の変換をそのまま使うこと。
    #[test]
    fn mirror_keeps_arcs_counter_clockwise() {
        let dir = TempDir::new("xf-mirror-arc");
        let mut s = layered(&dir);
        let r = ok(
            &mut s,
            "add_entities",
            json!({"entities": [{"type": "arc", "center": [0, 0], "radius": 10, "start_angle": 0, "end_angle": 90}]}),
        );
        let id = r["ids"][0].as_str().unwrap().to_owned();
        mutate(
            &mut s,
            "mirror_entities",
            json!({"ids": [id], "axis_a": [0, -5], "axis_b": [0, 5]}),
        );
        let got = ok(&mut s, "get_entities", json!({ "ids": [id] }));
        let g = &got["entities"][0]["geometry"];
        let start = crate::convert::deg_to_rad(g["start_angle"].as_f64().unwrap());
        let end = crate::convert::deg_to_rad(g["end_angle"].as_f64().unwrap());
        assert!(eq_angle(
            cad_core::geom::tolerance::wrap_2pi(start),
            std::f64::consts::FRAC_PI_2
        ));
        assert!(eq_angle(
            cad_core::geom::tolerance::wrap_2pi(end),
            std::f64::consts::PI
        ));
    }

    #[test]
    fn transforms_reject_bad_input_without_changing_anything() {
        let dir = TempDir::new("xf-bad");
        let mut s = layered(&dir);
        let id = add_line(&mut s);
        let (locked, hidden) = (eid(&s, 2), eid(&s, 3));
        for (tool, args, needle) in [
            (
                "move_entities",
                json!({"ids": [id], "delta": [3_000_000_000.0, 0]}),
                "上限",
            ),
            (
                "move_entities",
                json!({"ids": [id], "delta": ["1/0", 0]}),
                "計算できません",
            ),
            (
                "move_entities",
                json!({"ids": [id, locked], "delta": [1, 0]}),
                "ロック中",
            ),
            (
                "move_entities",
                json!({"ids": [hidden], "delta": [1, 0], "copy": true}),
                "非表示",
            ),
            ("move_entities", json!({"ids": [id]}), "delta"),
            (
                "move_entities",
                json!({"ids": [id, id], "delta": [1, 0]}),
                "2 回",
            ),
            (
                "rotate_entities",
                json!({"ids": [id], "center": [0, 0], "angle_deg": "x"}),
                "パラメータ",
            ),
            (
                "rotate_entities",
                json!({"ids": [locked], "center": [0, 0], "angle_deg": 30}),
                "ロック中",
            ),
            (
                "scale_entities",
                json!({"ids": [id], "center": [0, 0], "factor": 0}),
                "0 より大きい",
            ),
            (
                "scale_entities",
                json!({"ids": [id], "center": [0, 0], "factor": -2}),
                "mirror",
            ),
            (
                "scale_entities",
                json!({"ids": [id], "center": [0, 0], "factor": "1/1000000000000"}),
                "長さが 0",
            ),
            (
                "scale_entities",
                json!({"ids": [id], "center": [0, 0], "factor": 1_000_000_000}),
                "上限",
            ),
            (
                "mirror_entities",
                json!({"ids": [id], "axis_a": [1, 1], "axis_b": [1, 1]}),
                "同じ点",
            ),
            (
                "mirror_entities",
                json!({"ids": [hidden], "axis_a": [0, 0], "axis_b": [1, 1]}),
                "非表示",
            ),
        ] {
            let msg = rejected(&mut s, tool, args.clone());
            assert!(msg.contains(needle), "{tool} {args}: {msg}");
        }
    }

    #[test]
    fn set_entity_layer_moves_between_editable_layers() {
        let dir = TempDir::new("xf-layer");
        let mut s = layered(&dir);
        let (zero, wall, locked) = (eid(&s, 0), eid(&s, 1), eid(&s, 2));
        let r = mutate(
            &mut s,
            "set_entity_layer",
            json!({"ids": [zero, wall], "layer": "WALL"}),
        );
        assert_eq!(r["count"], 2);
        let got = ok(&mut s, "get_entities", json!({ "ids": [zero] }));
        assert_eq!(got["entities"][0]["layer"], "WALL");

        for (args, needle) in [
            (json!({"ids": [zero], "layer": "LOCK"}), "ロック中"),
            (json!({"ids": [zero], "layer": "HIDE"}), "非表示"),
            (json!({"ids": [zero], "layer": "NOPE"}), "ありません"),
            (json!({"ids": [locked], "layer": "0"}), "ロック中"),
            (json!({"ids": [zero]}), "layer"),
        ] {
            let msg = rejected(&mut s, "set_entity_layer", args.clone());
            assert!(msg.contains(needle), "{args}: {msg}");
        }
    }
}
