//! 図形 ↔ JSON と、度 ↔ ラジアンの変換。
//!
//! **変換はこのモジュールの 1 か所に集める。** 道具ごとに JSON の形や角度の単位を
//! 書くと、`π/180` の掛け忘れ・掛けすぎ（`docs/PROGRESS.md` の落とし穴）や、
//! 道具によって点の書き方が違うといった食い違いが紛れ込む。
//!
//! # 約束
//!
//! | 項目 | JSON での表し方 |
//! |---|---|
//! | 点 | `{"x": 1.0, "y": 2.0}`（入力は `[1.0, 2.0]` も受け付ける） |
//! | 角度 | **度**（`cad-core` の内部はラジアン） |
//! | 境界ボックス | `{"min": 点, "max": 点}`。空・無限（作図線）は `null` |
//! | 図形の種類 | `line` / `circle` / `arc` / `xline` / `polyline` / `instance` |
//!
//! 数値は有限でなければならない。`NaN` や無限大は座標へ流れると図形が消えて
//! 原因が追えなくなる（設計原則 6）ので、入力の時点で止める。

use cad_core::component::Placement;
use cad_core::expr::Value as ParamValue;
use cad_core::geom::{Aabb, Arc, Circle, Line, Point2, Polyline, Vec2, Xline};
use cad_core::ColorSpec;
use cad_core::{DefinitionTable, Document, Entity, EntityId, Geometry, Instance};
use serde_json::{json, Map, Value};

use crate::ids::DrawingTag;

/// 図形の種類の名前（JSON の `type`）。並びは `list_entities` の説明にも使う。
pub const ENTITY_TYPES: [&str; 6] = ["line", "circle", "arc", "xline", "polyline", "instance"];

/// 度 → ラジアン。
///
/// 式は `cad-core` の DXF 用の変換をそのまま使う（変換式をクレートごとに書かない）。
#[must_use]
pub fn deg_to_rad(deg: f64) -> f64 {
    cad_core::dxf::deg_to_rad(deg)
}

/// ラジアン → 度。[`deg_to_rad`] の逆変換。
#[must_use]
pub fn rad_to_deg(rad: f64) -> f64 {
    cad_core::dxf::rad_to_deg(rad)
}

// ---- 数値・点・範囲 -------------------------------------------------------

/// 有限の数値を取り出す。`what` はエラーメッセージに出す項目名。
///
/// # Errors
///
/// 数値でない・有限でない場合。
pub fn number_from_json(v: &Value, what: &str) -> Result<f64, String> {
    let n = v
        .as_f64()
        .ok_or_else(|| format!("{what} は数値で指定してください（受け取った値: {v}）"))?;
    if n.is_finite() {
        Ok(n)
    } else {
        Err(format!("{what} が有限の数値ではありません"))
    }
}

/// 点を JSON にする。
#[must_use]
pub fn point_to_json(p: Point2) -> Value {
    json!({ "x": p.x, "y": p.y })
}

/// 点を JSON から読む。`{"x": .., "y": ..}` と `[x, y]` を受け付ける。
///
/// # Errors
///
/// 形が違う・数値が有限でない場合。
pub fn point_from_json(v: &Value, what: &str) -> Result<Point2, String> {
    match v {
        Value::Object(m) => {
            if let Some(extra) = m.keys().find(|k| *k != "x" && *k != "y") {
                return Err(format!(
                    "{what} に不明な項目 {extra} があります（x と y だけ）"
                ));
            }
            let x = m
                .get("x")
                .ok_or_else(|| format!("{what} に x がありません"))?;
            let y = m
                .get("y")
                .ok_or_else(|| format!("{what} に y がありません"))?;
            Ok(Point2::new(
                number_from_json(x, &format!("{what}.x"))?,
                number_from_json(y, &format!("{what}.y"))?,
            ))
        }
        Value::Array(a) if a.len() == 2 => Ok(Point2::new(
            number_from_json(&a[0], &format!("{what}[0]"))?,
            number_from_json(&a[1], &format!("{what}[1]"))?,
        )),
        _ => Err(format!(
            "{what} は {{\"x\": 数値, \"y\": 数値}} か [x, y] で指定してください"
        )),
    }
}

/// 境界ボックスを JSON にする。空（図形が無い）と無限（作図線）は `null`。
#[must_use]
pub fn aabb_to_json(b: Aabb) -> Value {
    let finite = [b.min.x, b.min.y, b.max.x, b.max.y]
        .iter()
        .all(|v| v.is_finite());
    if b.is_empty() || !finite {
        return Value::Null;
    }
    json!({ "min": point_to_json(b.min), "max": point_to_json(b.max) })
}

/// 境界ボックスを JSON から読む。`{"min": 点, "max": 点}`。角の大小は入れ替わっていてもよい。
///
/// # Errors
///
/// 形が違う・数値が有限でない場合。
pub fn aabb_from_json(v: &Value, what: &str) -> Result<Aabb, String> {
    let m = v
        .as_object()
        .ok_or_else(|| format!("{what} は {{\"min\": 点, \"max\": 点}} で指定してください"))?;
    if let Some(extra) = m.keys().find(|k| *k != "min" && *k != "max") {
        return Err(format!(
            "{what} に不明な項目 {extra} があります（min と max だけ）"
        ));
    }
    let min = m
        .get("min")
        .ok_or_else(|| format!("{what} に min がありません"))?;
    let max = m
        .get("max")
        .ok_or_else(|| format!("{what} に max がありません"))?;
    Ok(Aabb::new(
        point_from_json(min, &format!("{what}.min"))?,
        point_from_json(max, &format!("{what}.max"))?,
    ))
}

// ---- 図形 -----------------------------------------------------------------

/// 図形の種類の名前（[`ENTITY_TYPES`] のどれか）。
#[must_use]
pub fn geometry_type(g: &Geometry) -> &'static str {
    match g {
        Geometry::Line(_) => "line",
        Geometry::Circle(_) => "circle",
        Geometry::Arc(_) => "arc",
        Geometry::Xline(_) => "xline",
        Geometry::Polyline(_) => "polyline",
        Geometry::Instance(_) => "instance",
    }
}

/// パラメータの値を JSON にする。
#[must_use]
pub fn param_value_to_json(v: &ParamValue) -> Value {
    match v {
        ParamValue::Number(n) => json!(n),
        ParamValue::Bool(b) => json!(b),
        ParamValue::Choice(c) => json!(c),
    }
}

/// 図形の形を JSON にする（`type` と形の項目）。角度は度。
#[must_use]
pub fn geometry_to_json(g: &Geometry, defs: &DefinitionTable) -> Value {
    let body = match g {
        Geometry::Line(l) => json!({ "start": point_to_json(l.a), "end": point_to_json(l.b) }),
        Geometry::Circle(c) => json!({ "center": point_to_json(c.center), "radius": c.radius }),
        Geometry::Arc(a) => json!({
            "center": point_to_json(a.center),
            "radius": a.radius,
            "start_angle": rad_to_deg(a.start_angle),
            "end_angle": rad_to_deg(a.end_angle),
        }),
        Geometry::Xline(x) => json!({
            "origin": point_to_json(x.origin),
            "angle": rad_to_deg(x.angle()),
            "direction": { "x": x.direction.x, "y": x.direction.y },
        }),
        Geometry::Polyline(p) => json!({
            "vertices": p.vertices.iter().map(|v| point_to_json(*v)).collect::<Vec<_>>(),
            "closed": p.closed,
        }),
        Geometry::Instance(i) => {
            let overrides: Map<String, Value> = i
                .overrides
                .iter()
                .map(|(k, v)| (k.clone(), param_value_to_json(v)))
                .collect();
            json!({
                "component": defs.get(i.definition).map(|d| d.name.as_str()),
                "origin": point_to_json(i.placement.origin),
                "rotation": rad_to_deg(i.placement.rotation),
                "scale": i.placement.scale,
                "flipped": i.placement.flipped,
                "overrides": overrides,
            })
        }
    };
    let mut out = Map::new();
    out.insert("type".into(), json!(geometry_type(g)));
    if let Value::Object(m) = body {
        out.extend(m);
    }
    Value::Object(out)
}

/// 図形を JSON から読む。[`geometry_to_json`] の逆。
///
/// 作図線の向きは `angle`（度）か `direction` で指定する。両方あれば `direction` を使い、
/// `angle` と食い違えば拒む（[`geometry_to_json`] は両方を出すので、出力をそのまま渡せる）。
/// インスタンスの `overrides` はまだ受け付けない（上書きの型と範囲の検証は
/// コマンド側にあり、コンポーネントの道具と一緒に入れる）。
///
/// 組み立てた図形は `Geometry::validate` に通す（長さ 0 の線分・半径 0 の円などを拒む）。
///
/// # Errors
///
/// 形が違う・数値が有限でない・図形として成立しない・コンポーネントが無い場合。
pub fn geometry_from_json(v: &Value, defs: &DefinitionTable) -> Result<Geometry, String> {
    let m = v
        .as_object()
        .ok_or_else(|| "図形はオブジェクトで指定してください".to_owned())?;
    let ty = m
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("図形の type がありません（{}）", ENTITY_TYPES.join(" / ")))?;
    let get = |key: &str| {
        m.get(key)
            .ok_or_else(|| format!("{ty} に {key} がありません"))
    };
    let allowed: &[&str] = match ty {
        "line" => &["type", "start", "end"],
        "circle" => &["type", "center", "radius"],
        "arc" => &["type", "center", "radius", "start_angle", "end_angle"],
        "xline" => &["type", "origin", "angle", "direction"],
        "polyline" => &["type", "vertices", "closed"],
        "instance" => &[
            "type",
            "component",
            "origin",
            "rotation",
            "scale",
            "flipped",
        ],
        other => {
            return Err(format!(
                "図形の種類 {other} は扱えません（{}）",
                ENTITY_TYPES.join(" / ")
            ))
        }
    };
    if let Some(extra) = m.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(format!(
            "{ty} に不明な項目 {extra} があります（使えるのは {}）",
            allowed.join(", ")
        ));
    }

    let geom = match ty {
        "line" => Geometry::Line(Line::new(
            point_from_json(get("start")?, "start")?,
            point_from_json(get("end")?, "end")?,
        )),
        "circle" => Geometry::Circle(Circle::new(
            point_from_json(get("center")?, "center")?,
            number_from_json(get("radius")?, "radius")?,
        )),
        "arc" => Geometry::Arc(Arc::new(
            point_from_json(get("center")?, "center")?,
            number_from_json(get("radius")?, "radius")?,
            deg_to_rad(number_from_json(get("start_angle")?, "start_angle")?),
            deg_to_rad(number_from_json(get("end_angle")?, "end_angle")?),
        )),
        "xline" => {
            let origin = point_from_json(get("origin")?, "origin")?;
            let xline = match (m.get("angle"), m.get("direction")) {
                (Some(a), None) => Some(Xline::at_angle(
                    origin,
                    deg_to_rad(number_from_json(a, "angle")?),
                )),
                (angle, Some(d)) => {
                    let d = point_from_json(d, "direction")?;
                    let x = Xline::new(origin, Vec2::new(d.x, d.y));
                    // 両方あるとき（get_entities の出力をそのまま渡したとき）は、食い違わないこと。
                    if let (Some(a), Some(x)) = (angle, &x) {
                        let a = deg_to_rad(number_from_json(a, "angle")?);
                        if !cad_core::geom::tolerance::eq_angle(
                            cad_core::geom::tolerance::wrap_2pi(a),
                            cad_core::geom::tolerance::wrap_2pi(x.angle()),
                        ) {
                            return Err("xline の angle と direction が食い違っています".to_owned());
                        }
                    }
                    x
                }
                (None, None) => {
                    return Err(
                        "xline は angle（度）か direction で向きを指定してください".to_owned()
                    )
                }
            };
            Geometry::Xline(xline.ok_or_else(|| "xline の向きが長さ 0 です".to_owned())?)
        }
        "polyline" => {
            let vertices = get("vertices")?
                .as_array()
                .ok_or_else(|| "polyline の vertices は点の配列で指定してください".to_owned())?
                .iter()
                .enumerate()
                .map(|(i, p)| point_from_json(p, &format!("vertices[{i}]")))
                .collect::<Result<Vec<_>, _>>()?;
            let closed = match m.get("closed") {
                None => false,
                Some(c) => c
                    .as_bool()
                    .ok_or_else(|| "polyline の closed は真偽で指定してください".to_owned())?,
            };
            Geometry::Polyline(Polyline::new(vertices, closed))
        }
        "instance" => {
            let name = get("component")?.as_str().ok_or_else(|| {
                "instance の component はコンポーネント名で指定してください".to_owned()
            })?;
            let definition = defs
                .by_name(name)
                .ok_or_else(|| format!("コンポーネント {name} がありません"))?;
            let origin = point_from_json(get("origin")?, "origin")?;
            let rotation = match m.get("rotation") {
                None => 0.0,
                Some(r) => deg_to_rad(number_from_json(r, "rotation")?),
            };
            let scale = match m.get("scale") {
                None => 1.0,
                Some(s) => number_from_json(s, "scale")?,
            };
            let flipped = match m.get("flipped") {
                None => false,
                Some(f) => f
                    .as_bool()
                    .ok_or_else(|| "instance の flipped は真偽で指定してください".to_owned())?,
            };
            let placement =
                Placement::new(origin, rotation, scale, flipped).map_err(|e| e.to_string())?;
            Geometry::Instance(Instance::new(definition, placement))
        }
        _ => unreachable!("種類は上で絞り込み済み"),
    };
    geom.validate().map_err(|e| e.to_string())?;
    Ok(geom)
}

// ---- 図形（属性つき） ------------------------------------------------------

/// 色を JSON にする。レイヤに従うなら `"bylayer"`、個別指定なら ACI の番号。
#[must_use]
pub fn color_to_json(c: ColorSpec) -> Value {
    match c {
        ColorSpec::ByLayer => json!("bylayer"),
        ColorSpec::Aci(aci) => json!(aci.0),
    }
}

/// 図形 1 つの要約（一覧用）。形の中身は含めない。
#[must_use]
pub fn entity_summary_json(doc: &Document, tag: DrawingTag, id: EntityId, e: &Entity) -> Value {
    json!({
        "id": crate::ids::format_id(tag, id),
        "type": geometry_type(&e.geom),
        "layer": layer_name(doc, e),
        "bbox": aabb_to_json(e.bbox(doc.definitions())),
    })
}

/// 図形 1 つの全体（ID・属性・形）。
#[must_use]
pub fn entity_to_json(doc: &Document, tag: DrawingTag, id: EntityId, e: &Entity) -> Value {
    let mut out = Map::new();
    out.insert("id".into(), json!(crate::ids::format_id(tag, id)));
    out.insert("layer".into(), json!(layer_name(doc, e)));
    out.insert("color".into(), color_to_json(e.color));
    out.insert(
        "group".into(),
        json!(e
            .group
            .and_then(|g| doc.groups().get(g))
            .map(|g| g.name.as_str())),
    );
    out.insert("bbox".into(), aabb_to_json(e.bbox(doc.definitions())));
    out.insert(
        "geometry".into(),
        geometry_to_json(&e.geom, doc.definitions()),
    );
    Value::Object(out)
}

fn layer_name<'a>(doc: &'a Document, e: &Entity) -> Option<&'a str> {
    doc.layers().get(e.layer).map(|l| l.name.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cad_core::geom::tolerance::{eq_angle, eq_len};
    use std::f64::consts::FRAC_PI_2;

    fn defs() -> DefinitionTable {
        DefinitionTable::new()
    }

    #[test]
    fn degrees_and_radians_are_inverse() {
        assert!(eq_angle(deg_to_rad(90.0), FRAC_PI_2));
        assert!(eq_len(rad_to_deg(FRAC_PI_2), 90.0));
    }

    #[test]
    fn point_accepts_object_and_array() {
        let a = point_from_json(&json!({"x": 1.5, "y": -2}), "p").unwrap();
        let b = point_from_json(&json!([1.5, -2]), "p").unwrap();
        assert_eq!(a, b);
        assert_eq!(a, Point2::new(1.5, -2.0));
    }

    #[test]
    fn point_rejects_wrong_shapes() {
        for bad in [
            json!({"x": 1}),
            json!({"x": 1, "y": 2, "z": 3}),
            json!([1, 2, 3]),
            json!("1,2"),
            json!({"x": "1", "y": 2}),
        ] {
            assert!(point_from_json(&bad, "p").is_err(), "{bad} を拒むこと");
        }
    }

    #[test]
    fn empty_and_unbounded_boxes_are_null() {
        assert_eq!(aabb_to_json(Aabb::EMPTY), Value::Null);
        assert_eq!(aabb_to_json(Aabb::UNBOUNDED), Value::Null);
        let b = Aabb::new(Point2::new(2.0, 3.0), Point2::new(0.0, 1.0));
        assert_eq!(
            aabb_to_json(b),
            json!({"min": {"x": 0.0, "y": 1.0}, "max": {"x": 2.0, "y": 3.0}})
        );
    }

    #[test]
    fn aabb_from_json_normalizes_corners() {
        let b = aabb_from_json(&json!({"min": [5, 5], "max": [0, 0]}), "bbox").unwrap();
        assert_eq!(b.min, Point2::new(0.0, 0.0));
        assert_eq!(b.max, Point2::new(5.0, 5.0));
        assert!(aabb_from_json(&json!({"min": [0, 0]}), "bbox").is_err());
    }

    /// 角度は出力も入力も度。90 度の円弧が π/2 ラジアンとして入ること
    /// （変換の掛け忘れ・掛けすぎを小さくない角度で捕まえる）。
    #[test]
    fn arc_angles_are_degrees_in_json() {
        let g = geometry_from_json(
            &json!({"type": "arc", "center": [0, 0], "radius": 2, "start_angle": 90, "end_angle": 180}),
            &defs(),
        )
        .unwrap();
        let Geometry::Arc(a) = &g else {
            panic!("円弧のはず")
        };
        assert!(eq_angle(a.start_angle, FRAC_PI_2));
        let out = geometry_to_json(&g, &defs());
        assert!(eq_len(out["start_angle"].as_f64().unwrap(), 90.0));
        assert!(eq_len(out["end_angle"].as_f64().unwrap(), 180.0));
    }

    /// 出力した JSON を読み戻すと同じ形になること（インスタンス以外の全種類）。
    #[test]
    fn geometry_json_round_trips() {
        let shapes = [
            Geometry::Line(Line::new(Point2::new(0.0, 0.0), Point2::new(3.0, 4.0))),
            Geometry::Circle(Circle::new(Point2::new(1.0, 1.0), 2.5)),
            Geometry::Arc(Arc::new(Point2::new(0.0, 0.0), 1.0, 0.25, 2.0)),
            Geometry::Xline(Xline::at_angle(Point2::new(1.0, 2.0), 0.5)),
            Geometry::Polyline(Polyline::new(
                vec![
                    Point2::new(0.0, 0.0),
                    Point2::new(1.0, 0.0),
                    Point2::new(1.0, 1.0),
                ],
                true,
            )),
        ];
        for g in shapes {
            let back = geometry_from_json(&geometry_to_json(&g, &defs()), &defs())
                .unwrap_or_else(|e| panic!("{g:?} を読み戻せない: {e}"));
            match (&g, &back) {
                (Geometry::Arc(a), Geometry::Arc(b)) => {
                    assert!(eq_angle(a.start_angle, b.start_angle));
                    assert!(eq_angle(a.end_angle, b.end_angle));
                    assert!(eq_len(a.radius, b.radius));
                }
                (Geometry::Xline(a), Geometry::Xline(b)) => {
                    assert!(a.origin.eq_tol(b.origin));
                    assert!(a.direction.eq_tol(b.direction));
                }
                _ => assert_eq!(g, back),
            }
        }
    }

    #[test]
    fn degenerate_and_non_finite_shapes_are_rejected() {
        let d = defs();
        // 長さ 0 の線分・半径 0 の円・頂点が足りないポリライン。
        for bad in [
            json!({"type": "line", "start": [1, 1], "end": [1, 1]}),
            json!({"type": "circle", "center": [0, 0], "radius": 0}),
            json!({"type": "circle", "center": [0, 0], "radius": -1}),
            json!({"type": "polyline", "vertices": [[0, 0]]}),
            json!({"type": "xline", "origin": [0, 0], "direction": [0, 0]}),
            json!({"type": "xline", "origin": [0, 0]}),
            json!({"type": "xline", "origin": [0, 0], "angle": 45, "direction": [1, 0]}),
            json!({"type": "spline", "points": []}),
            json!({"type": "line", "start": [0, 0], "end": [1, 1], "color": 1}),
        ] {
            assert!(geometry_from_json(&bad, &d).is_err(), "{bad} を拒むこと");
        }
        assert!(number_from_json(&json!(f64::MAX), "v").is_ok());
        assert!(number_from_json(&json!("1"), "v").is_err());
    }

    #[test]
    fn instance_needs_an_existing_component() {
        let err = geometry_from_json(
            &json!({"type": "instance", "component": "無い", "origin": [0, 0]}),
            &defs(),
        )
        .unwrap_err();
        assert!(err.contains("無い"), "{err}");
    }
}
