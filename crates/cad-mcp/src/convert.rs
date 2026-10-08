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
//! | 数値の入力 | JSON の数値か、**式の文字列**（`"100*2+5"`・`"sqrt(2)*50"`。式の中の角度は度） |
//! | 束縛の対象 | 図形の JSON の項目名（`start.x`・`radius`・`vertices[3].y` など。[`slot_name`]） |
//!
//! 数値は有限でなければならない。`NaN` や無限大は座標へ流れると図形が消えて
//! 原因が追えなくなる（設計原則 6）ので、入力の時点で止める。式の計算でも同じ
//! （0 除算・負の平方根はエラーになる）。作った図形は [`check_extent`] で大きさの上限も確かめる。

use std::collections::BTreeMap;

use cad_core::component::{Binding, ParamDecl, Placement, Slot};
use cad_core::expr::{ParamType, Value as ParamValue};
use cad_core::geom::{Aabb, Arc, Circle, Line, Point2, Polyline, Vec2, Xline};
use cad_core::layer::LineType;
use cad_core::{AciColor, ColorSpec, Definition, Layer};
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
/// JSON の数値か、**式の文字列**（`cad-core` の式。パラメータは使えない。角度は度）を受け付ける。
/// LLM が `100*2+5` や `sqrt(2)*50` を自分で計算して丸めずに済むように（ADR-0031: パラメータは
/// テキストの式）。式は空の環境で計算し、数値にならない（真偽・選択肢）ものは拒む。
///
/// # Errors
///
/// 数値でも文字列でもない・式として読めない・計算できない・有限でない場合。
pub fn number_from_json(v: &Value, what: &str) -> Result<f64, String> {
    let n = match v {
        Value::Number(_) => v
            .as_f64()
            .ok_or_else(|| format!("{what} は数値で指定してください（受け取った値: {v}）"))?,
        Value::String(src) => eval_number(src, what)?,
        _ => {
            return Err(format!(
                "{what} は数値か式の文字列で指定してください（受け取った値: {v}）"
            ))
        }
    };
    if n.is_finite() {
        Ok(n)
    } else {
        Err(format!("{what} が有限の数値ではありません"))
    }
}

/// 式の文字列を数値にする（[`number_from_json`] の文字列の場合）。
fn eval_number(src: &str, what: &str) -> Result<f64, String> {
    use cad_core::expr::{eval, parse, Env};
    if src.len() > crate::limits::MAX_EXPR_BYTES {
        return Err(format!(
            "{what} の式が長すぎます（上限 {} バイト）",
            crate::limits::MAX_EXPR_BYTES
        ));
    }
    let expr = parse(src).map_err(|e| format!("{what} の式 {src:?} を読めません: {e}"))?;
    match eval(&expr, &Env::new()) {
        Ok(ParamValue::Number(n)) => Ok(n),
        Ok(other) => Err(format!(
            "{what} の式 {src:?} の値が数値ではありません（{}）",
            other.type_name()
        )),
        Err(e) => Err(format!(
            "{what} の式 {src:?} を計算できません: {e}（ここではパラメータは使えません）"
        )),
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
/// 別の書き方も受け付ける（作図の道具のため）:
///
/// | 種類 | 書き方 |
/// |---|---|
/// | 円弧 | `center`・`radius`・`start_angle`・`end_angle`（度、反時計回り）か、3 点 `start`・`through`・`end` |
/// | 作図線 | `origin` と、`angle`（度）・`direction`・`through`（もう 1 つの通過点）のどれか |
///
/// 作図線の `angle` と `direction` が両方あれば `direction` を使い、`angle` と食い違えば拒む
/// （[`geometry_to_json`] は両方を出すので、出力をそのまま渡せる）。
///
/// インスタンスの `overrides`（パラメータの上書き）も読む（[`geometry_to_json`] の出力をそのまま
/// 渡せるように）。**型と範囲はここでは確かめない**。上書きを変える経路はコンポーネントの道具
/// （`SetInstanceOverride`）で、`ReplaceGeometries` は上書きが元と違えば拒む（ADR-0040）。
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
        "arc" => &[
            "type",
            "center",
            "radius",
            "start_angle",
            "end_angle",
            "start",
            "through",
            "end",
        ],
        "xline" => &["type", "origin", "angle", "direction", "through"],
        "polyline" => &["type", "vertices", "closed"],
        "instance" => &[
            "type",
            "component",
            "origin",
            "rotation",
            "scale",
            "flipped",
            "overrides",
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
        "arc" => {
            let by_points = ["start", "through", "end"];
            let by_center = ["center", "radius", "start_angle", "end_angle"];
            let has = |keys: &[&str]| keys.iter().any(|k| m.contains_key(*k));
            if has(&by_points) {
                if has(&by_center) {
                    return Err(
                        "arc は center・radius・start_angle・end_angle か、3 点 start・through・end のどちらか一方で指定してください"
                            .to_owned(),
                    );
                }
                let start = point_from_json(get("start")?, "start")?;
                let through = point_from_json(get("through")?, "through")?;
                let end = point_from_json(get("end")?, "end")?;
                let arc = Arc::from_3_points(start, through, end).ok_or_else(|| {
                    "arc の 3 点（start・through・end）が一直線上にあるか重なっていて、円弧になりません"
                        .to_owned()
                })?;
                Geometry::Arc(arc)
            } else {
                Geometry::Arc(Arc::new(
                    point_from_json(get("center")?, "center")?,
                    number_from_json(get("radius")?, "radius")?,
                    deg_to_rad(number_from_json(get("start_angle")?, "start_angle")?),
                    deg_to_rad(number_from_json(get("end_angle")?, "end_angle")?),
                ))
            }
        }
        "xline" => {
            let origin = point_from_json(get("origin")?, "origin")?;
            if let Some(t) = m.get("through") {
                if m.contains_key("angle") || m.contains_key("direction") {
                    return Err(
                        "xline の向きは angle・direction・through のどれか 1 つで指定してください"
                            .to_owned(),
                    );
                }
                let through = point_from_json(t, "through")?;
                let x = Xline::through(origin, through)
                    .ok_or_else(|| "xline の origin と through が同じ点です".to_owned())?;
                let g = Geometry::Xline(x);
                g.validate().map_err(|e| e.to_string())?;
                return Ok(g);
            }
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
                        "xline は angle（度）・direction・through のどれかで向きを指定してください"
                            .to_owned(),
                    )
                }
            };
            Geometry::Xline(xline.ok_or_else(|| "xline の向きが長さ 0 です".to_owned())?)
        }
        "polyline" => {
            let vertices = vertices_from_json(get("vertices")?)?;
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
            let mut instance = Instance::new(definition, placement);
            if let Some(o) = m.get("overrides") {
                instance.overrides = overrides_from_json(o)?;
            }
            Geometry::Instance(instance)
        }
        _ => unreachable!("種類は上で絞り込み済み"),
    };
    geom.validate().map_err(|e| e.to_string())?;
    Ok(geom)
}

/// 既存の図形の、`set` に書いた項目だけを変えた形を作る（`modify_entities`）。
///
/// **書かなかった項目は元の値をそのまま使う**（度 ↔ ラジアンの往復もしないので、変えない項目は
/// ビット単位で元のまま）。項目名と書き方は [`geometry_from_json`] と同じで、別の書き方
/// （円弧の 3 点・作図線の `through`）も使える。`type` は書いてもよいが、元と同じでなければ拒む。
///
/// 変えられないもの（`ReplaceGeometries` の約束。ADR-0040）は、ここで分かりやすい説明で拒む:
/// 種類・ポリラインの頂点の数・インスタンスの参照する定義・インスタンスのパラメータの上書き。
///
/// 作った形は `Geometry::validate` に通す。大きさの上限（[`check_extent`]）は呼び出し側で見る。
///
/// # Errors
///
/// 知らない項目・変えられない項目・形として成立しない場合。
pub fn apply_fields(
    current: &Geometry,
    set: &Map<String, Value>,
    defs: &DefinitionTable,
) -> Result<Geometry, String> {
    let ty = geometry_type(current);
    if let Some(t) = set.get("type") {
        if t.as_str() != Some(ty) {
            return Err(format!(
                "図形の種類は変えられません（いまは {ty}）。種類を変えるなら delete_entities と add_entities で作り直してください（ID は変わります）"
            ));
        }
    }
    let allowed: &[&str] = match current {
        Geometry::Line(_) => &["type", "start", "end"],
        Geometry::Circle(_) => &["type", "center", "radius"],
        Geometry::Arc(_) => &[
            "type",
            "center",
            "radius",
            "start_angle",
            "end_angle",
            "start",
            "through",
            "end",
        ],
        Geometry::Xline(_) => &["type", "origin", "angle", "direction", "through"],
        Geometry::Polyline(_) => &["type", "vertices", "closed"],
        Geometry::Instance(_) => &[
            "type",
            "component",
            "origin",
            "rotation",
            "scale",
            "flipped",
            "overrides",
        ],
    };
    if let Some(extra) = set.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(format!(
            "{ty} に不明な項目 {extra} があります（使えるのは {}）",
            allowed.join(", ")
        ));
    }
    let point = |key: &str, old: Point2| match set.get(key) {
        None => Ok(old),
        Some(v) => point_from_json(v, key),
    };
    let number = |key: &str, old: f64| match set.get(key) {
        None => Ok(old),
        Some(v) => number_from_json(v, key),
    };
    let angle = |key: &str, old_rad: f64| match set.get(key) {
        None => Ok(old_rad),
        Some(v) => number_from_json(v, key).map(deg_to_rad),
    };
    let flag = |key: &str, old: bool| match set.get(key) {
        None => Ok(old),
        Some(v) => v
            .as_bool()
            .ok_or_else(|| format!("{ty} の {key} は true か false で指定してください")),
    };

    let geom = match current {
        Geometry::Line(l) => Geometry::Line(Line::new(point("start", l.a)?, point("end", l.b)?)),
        Geometry::Circle(c) => Geometry::Circle(Circle::new(
            point("center", c.center)?,
            number("radius", c.radius)?,
        )),
        Geometry::Arc(a) => {
            let by_points = ["start", "through", "end"];
            if by_points.iter().any(|k| set.contains_key(*k)) {
                // 3 点で書くなら丸ごと置き換える（一部の点だけでは形が決まらない）。
                let mut whole = set.clone();
                whole.insert("type".into(), json!("arc"));
                geometry_from_json(&Value::Object(whole), defs)?
            } else {
                Geometry::Arc(Arc::new(
                    point("center", a.center)?,
                    number("radius", a.radius)?,
                    angle("start_angle", a.start_angle)?,
                    angle("end_angle", a.end_angle)?,
                ))
            }
        }
        Geometry::Xline(x) => {
            let keeps_direction = !["angle", "direction", "through"]
                .iter()
                .any(|k| set.contains_key(*k));
            if keeps_direction {
                Geometry::Xline(Xline {
                    origin: point("origin", x.origin)?,
                    direction: x.direction,
                })
            } else {
                let mut whole = set.clone();
                whole.insert("type".into(), json!("xline"));
                whole
                    .entry("origin")
                    .or_insert_with(|| point_to_json(x.origin));
                geometry_from_json(&Value::Object(whole), defs)?
            }
        }
        Geometry::Polyline(p) => {
            let vertices = match set.get("vertices") {
                None => p.vertices.clone(),
                Some(v) => vertices_from_json(v)?,
            };
            if vertices.len() != p.vertices.len() {
                return Err(format!(
                    "ポリラインの頂点の数は変えられません（いまは {} 個、指定は {} 個）。\
                     数を変えるなら delete_entities と add_entities で作り直してください（ID は変わります）",
                    p.vertices.len(),
                    vertices.len()
                ));
            }
            Geometry::Polyline(Polyline::new(vertices, flag("closed", p.closed)?))
        }
        Geometry::Instance(i) => {
            if let Some(c) = set.get("component") {
                let now = defs.get(i.definition).map(|d| d.name.as_str());
                if c.as_str() != now {
                    return Err(
                        "インスタンスの参照するコンポーネントは変えられません（置き換えるなら削除して挿し直してください）"
                            .to_owned(),
                    );
                }
            }
            if let Some(o) = set.get("overrides") {
                if overrides_from_json(o)? != i.overrides {
                    return Err(
                        "インスタンスのパラメータ（overrides）は modify_entities では変えられません。\
                         同じ値を渡し返すのはかまいません（get_entities の出力をそのまま使えるように）"
                            .to_owned(),
                    );
                }
            }
            let pl = i.placement;
            let placement = Placement::new(
                point("origin", pl.origin)?,
                angle("rotation", pl.rotation)?,
                number("scale", pl.scale)?,
                flag("flipped", pl.flipped)?,
            )
            .map_err(|e| e.to_string())?;
            Geometry::Instance(i.with_placement(placement))
        }
    };
    geom.validate().map_err(|e| e.to_string())?;
    Ok(geom)
}

/// ポリラインの頂点の配列を読む。数に上限（[`crate::limits::MAX_POLYLINE_VERTICES`]）。
fn vertices_from_json(v: &Value) -> Result<Vec<Point2>, String> {
    let list = v
        .as_array()
        .ok_or_else(|| "polyline の vertices は点の配列で指定してください".to_owned())?;
    if list.len() > crate::limits::MAX_POLYLINE_VERTICES {
        return Err(format!(
            "polyline の頂点が多すぎます（上限 {} 個）",
            crate::limits::MAX_POLYLINE_VERTICES
        ));
    }
    list.iter()
        .enumerate()
        .map(|(i, p)| point_from_json(p, &format!("vertices[{i}]")))
        .collect()
}

/// インスタンスのパラメータの上書きを JSON から読む。数値・真偽・選択肢（文字列）。
///
/// **文字列は選択肢として読む**（式としては計算しない）。上書きの値は式ではなく値なので。
fn overrides_from_json(v: &Value) -> Result<BTreeMap<String, ParamValue>, String> {
    let m = v.as_object().ok_or_else(|| {
        "instance の overrides は {パラメータ名: 値} のオブジェクトで指定してください".to_owned()
    })?;
    m.iter()
        .map(|(k, v)| {
            let value = match v {
                Value::Bool(b) => ParamValue::Bool(*b),
                Value::String(c) => ParamValue::Choice(c.clone()),
                Value::Number(_) => {
                    ParamValue::Number(number_from_json(v, &format!("overrides.{k}"))?)
                }
                _ => {
                    return Err(format!(
                        "overrides.{k} は数値・真偽・選択肢の文字列で指定してください"
                    ))
                }
            };
            Ok((k.clone(), value))
        })
        .collect()
}

/// 図形の大きさが上限（[`crate::limits::MAX_COORDINATE`]）の内か確かめる。
///
/// `Geometry::validate` は有限かどうかしか見ないので、`1e300` の座標や、変形した結果
/// 桁あふれ寸前の図形は通ってしまう。描画・スナップ・トレランスの計算が成り立たない大きさを
/// 図面へ入れないための柵。範囲は境界ボックスで見る（円・円弧は半径を含む）。
/// 作図線と、作図線を含むインスタンスは範囲が無限なので、通過点（基点）と倍率で見る。
///
/// # Errors
///
/// 上限を超える場合。
pub fn check_extent(g: &Geometry, defs: &DefinitionTable) -> Result<(), String> {
    use crate::limits::MAX_COORDINATE;
    let within = |p: Point2| p.x.abs() <= MAX_COORDINATE && p.y.abs() <= MAX_COORDINATE;
    let ok = match g {
        Geometry::Xline(x) => within(x.origin),
        Geometry::Instance(i) if !g.is_bounded(defs) => {
            within(i.placement.origin) && i.placement.scale <= MAX_COORDINATE
        }
        _ => {
            let b = g.bbox(defs);
            b.is_empty() || (within(b.min) && within(b.max))
        }
    };
    if ok {
        Ok(())
    } else {
        Err(format!(
            "{} の座標・大きさが上限（絶対値 {MAX_COORDINATE:e}）を超えます",
            geometry_type(g)
        ))
    }
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

// ---- レイヤ ----------------------------------------------------------------

/// 線種の名前（JSON）。DXF の名前の小文字。
pub const LINETYPES: [&str; 4] = ["continuous", "dashed", "center", "hidden"];

/// 線種を JSON の名前にする。
#[must_use]
pub fn linetype_name(t: LineType) -> &'static str {
    match t {
        LineType::Continuous => "continuous",
        LineType::Dashed => "dashed",
        LineType::Center => "center",
        LineType::Hidden => "hidden",
    }
}

/// 線種を名前から読む（大文字小文字は問わない）。[`linetype_name`] の逆。
///
/// # Errors
///
/// 知らない名前の場合。
pub fn linetype_from_json(v: &Value, what: &str) -> Result<LineType, String> {
    let name = v.as_str().ok_or_else(|| {
        format!(
            "{what} は文字列（{}）で指定してください",
            LINETYPES.join(" / ")
        )
    })?;
    LineType::all()
        .into_iter()
        .find(|t| linetype_name(*t).eq_ignore_ascii_case(name))
        .ok_or_else(|| format!("{what} {name} は扱えません（{}）", LINETYPES.join(" / ")))
}

/// レイヤの色（ACI 1〜255）を読む。0（BYBLOCK）と 256（BYLAYER）はレイヤの色にならないので拒む。
///
/// # Errors
///
/// 整数でない・範囲の外の場合。
pub fn aci_from_json(v: &Value, what: &str) -> Result<AciColor, String> {
    v.as_u64()
        .and_then(|n| u8::try_from(n).ok())
        .filter(|n| *n >= 1)
        .map(AciColor)
        .ok_or_else(|| format!("{what} は ACI の色番号 1〜255 の整数で指定してください（1 赤・2 黄・3 緑・4 水色・5 青・6 紫・7 白）"))
}

/// レイヤ 1 つを JSON にする。
#[must_use]
pub fn layer_to_json(layer: &Layer, current: bool, entity_count: usize) -> Value {
    json!({
        "name": layer.name,
        "color": layer.color.0,
        "visible": layer.visible,
        "locked": layer.locked,
        "linetype": linetype_name(layer.linetype),
        "current": current,
        "entity_count": entity_count,
    })
}

// ---- コンポーネント ----------------------------------------------------------

/// 束縛の対象（[`Slot`]）を、図形の JSON の項目名で表す（`start.x`・`radius`・`vertices[3].y` など）。
///
/// Rust の `Debug` の綴り（`PolylineVx(3)`）を JSON の約束にしない（PR #84 レビューの非ブロッキング 5）。
/// 名前は [`geometry_to_json`] の項目名に合わせてあるので、LLM は `get_entities` の出力と突き合わせられる。
/// 逆は [`slot_from_name`]。角度（`start_angle`・`angle`・`rotation`）は度。
#[must_use]
pub fn slot_name(slot: Slot) -> String {
    match slot {
        Slot::LineAx => "start.x".into(),
        Slot::LineAy => "start.y".into(),
        Slot::LineBx => "end.x".into(),
        Slot::LineBy => "end.y".into(),
        Slot::CircleCx | Slot::ArcCx => "center.x".into(),
        Slot::CircleCy | Slot::ArcCy => "center.y".into(),
        Slot::CircleR | Slot::ArcR => "radius".into(),
        Slot::ArcStart => "start_angle".into(),
        Slot::ArcEnd => "end_angle".into(),
        Slot::XlineOx | Slot::InstanceX => "origin.x".into(),
        Slot::XlineOy | Slot::InstanceY => "origin.y".into(),
        Slot::XlineAngle => "angle".into(),
        Slot::PolylineVx(i) => format!("vertices[{i}].x"),
        Slot::PolylineVy(i) => format!("vertices[{i}].y"),
        Slot::InstanceRotation => "rotation".into(),
        Slot::InstanceScale => "scale".into(),
    }
}

/// 図形の種類（[`ENTITY_TYPES`]）と項目名から [`Slot`] を求める。[`slot_name`] の逆。
///
/// 同じ項目名（`center.x`・`origin.x`）が種類によって別の [`Slot`] になるので、種類も受け取る。
#[must_use]
pub fn slot_from_name(geometry_type: &str, name: &str) -> Option<Slot> {
    let fixed = match (geometry_type, name) {
        ("line", "start.x") => Some(Slot::LineAx),
        ("line", "start.y") => Some(Slot::LineAy),
        ("line", "end.x") => Some(Slot::LineBx),
        ("line", "end.y") => Some(Slot::LineBy),
        ("circle", "center.x") => Some(Slot::CircleCx),
        ("circle", "center.y") => Some(Slot::CircleCy),
        ("circle", "radius") => Some(Slot::CircleR),
        ("arc", "center.x") => Some(Slot::ArcCx),
        ("arc", "center.y") => Some(Slot::ArcCy),
        ("arc", "radius") => Some(Slot::ArcR),
        ("arc", "start_angle") => Some(Slot::ArcStart),
        ("arc", "end_angle") => Some(Slot::ArcEnd),
        ("xline", "origin.x") => Some(Slot::XlineOx),
        ("xline", "origin.y") => Some(Slot::XlineOy),
        ("xline", "angle") => Some(Slot::XlineAngle),
        ("instance", "origin.x") => Some(Slot::InstanceX),
        ("instance", "origin.y") => Some(Slot::InstanceY),
        ("instance", "rotation") => Some(Slot::InstanceRotation),
        ("instance", "scale") => Some(Slot::InstanceScale),
        _ => None,
    };
    if fixed.is_some() || geometry_type != "polyline" {
        return fixed;
    }
    // vertices[<10 進>].x / .y
    let rest = name.strip_prefix("vertices[")?;
    let (index, axis) = rest.split_once("].")?;
    if index.is_empty() || !index.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let i: u32 = index.parse().ok()?;
    match axis {
        "x" => Some(Slot::PolylineVx(i)),
        "y" => Some(Slot::PolylineVy(i)),
        _ => None,
    }
}

/// パラメータの宣言を JSON にする。
#[must_use]
pub fn param_decl_to_json(p: &ParamDecl) -> Value {
    let (ty, choices) = match &p.ty {
        ParamType::Number => ("number", None),
        ParamType::Bool => ("bool", None),
        ParamType::Choice(c) => ("choice", Some(c.clone())),
    };
    json!({
        "name": p.name,
        "type": ty,
        "choices": choices,
        "default": p.default.to_string(),
        "range": p.range.map(|(lo, hi)| json!([lo, hi])),
    })
}

/// 束縛を JSON にする。`entity` は定義の中の図形の添字、`field` は [`slot_name`]。
#[must_use]
pub fn binding_to_json(b: &Binding, def: &Definition) -> Value {
    json!({
        "entity": b.entity,
        "entity_type": def.entities.get(b.entity).map(|e| geometry_type(&e.geom)),
        "field": slot_name(b.slot),
        "label": b.slot.label(),
        "expr": b.expr.to_string(),
    })
}

/// コンポーネント定義を JSON にする。
#[must_use]
pub fn component_to_json(def: &Definition, instance_count: usize) -> Value {
    json!({
        "name": def.name,
        "origin": point_to_json(def.origin),
        "entity_count": def.entities.len(),
        "instance_count": instance_count,
        "params": def.params.iter().map(param_decl_to_json).collect::<Vec<_>>(),
        "bindings": def.bindings.iter().map(|b| binding_to_json(b, def)).collect::<Vec<_>>(),
    })
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
            json!({"x": "1+", "y": 2}),
            json!({"x": true, "y": 2}),
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
        assert!(number_from_json(&json!(null), "v").is_err());
    }

    /// JSON の数値は最後のビットまで正確に読む（serde_json の float_roundtrip）。既定の読み取りは
    /// 1 ULP ずれることがあり、get_entities の出力を modify_entities へ渡し返すと形が黙ってずれた。
    #[test]
    fn json_numbers_round_trip_exactly() {
        for x in [
            0.969_015_731_406_869_6_f64,
            1.0 / 3.0,
            std::f64::consts::PI * 1e5,
            123.456_789_012_345_67,
        ] {
            let text = json!(x).to_string();
            let back: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(back.as_f64().unwrap().to_bits(), x.to_bits(), "{text}");
        }
    }

    /// 数値の代わりに式の文字列を書ける。式の中の角度は度。
    #[test]
    fn numbers_accept_expressions() {
        let n = |v: Value| number_from_json(&v, "v");
        assert!(eq_len(n(json!("100*2+5")).unwrap(), 205.0));
        assert!(eq_len(n(json!("sqrt(2)*sqrt(2)")).unwrap(), 2.0));
        assert!(eq_len(n(json!("cos(60)")).unwrap(), 0.5), "角度は度");
        let p = point_from_json(&json!({"x": "10/4", "y": "-3"}), "p").unwrap();
        assert_eq!(p, Point2::new(2.5, -3.0));

        for (bad, needle) in [
            (json!("1/0"), "計算できません"),
            (json!("sqrt(-1)"), "計算できません"),
            (json!("幅 * 2"), "パラメータ"),
            (json!("1 +"), "読めません"),
            (json!(""), "読めません"),
            (json!("1 < 2"), "数値ではありません"),
            (
                json!("x".repeat(crate::limits::MAX_EXPR_BYTES + 1)),
                "長すぎ",
            ),
        ] {
            let e = n(bad.clone()).unwrap_err();
            assert!(e.contains(needle), "{bad}: {e}");
        }
        // 式で作った図形も validate を通る（半径 0 は拒む）。
        let g = geometry_from_json(
            &json!({"type": "circle", "center": ["1+1", 0], "radius": "5*2"}),
            &defs(),
        )
        .unwrap();
        assert_eq!(
            g,
            Geometry::Circle(Circle::new(Point2::new(2.0, 0.0), 10.0))
        );
        assert!(geometry_from_json(
            &json!({"type": "circle", "center": [0, 0], "radius": "5-5"}),
            &defs()
        )
        .is_err());
    }

    /// 円弧は 3 点でも、作図線は 2 点でも書ける。
    #[test]
    fn arcs_by_three_points_and_xlines_through_two() {
        let d = defs();
        let g = geometry_from_json(
            &json!({"type": "arc", "start": [10, 0], "through": [0, 10], "end": [-10, 0]}),
            &d,
        )
        .unwrap();
        let Geometry::Arc(a) = g else {
            panic!("円弧のはず")
        };
        assert!(a.center.eq_tol(Point2::new(0.0, 0.0)));
        assert!(eq_len(a.radius, 10.0));
        assert!(eq_angle(a.start_angle, 0.0) && eq_angle(a.end_angle, std::f64::consts::PI));

        // 通る点が下側なら、反時計回りに 180°→360°。
        let Geometry::Arc(b) = geometry_from_json(
            &json!({"type": "arc", "start": [10, 0], "through": [0, -10], "end": [-10, 0]}),
            &d,
        )
        .unwrap() else {
            panic!("円弧のはず")
        };
        assert!(b.contains_angle(-std::f64::consts::FRAC_PI_2));
        assert!(!b.contains_angle(std::f64::consts::FRAC_PI_2));

        for bad in [
            json!({"type": "arc", "start": [0, 0], "through": [1, 1], "end": [2, 2]}),
            json!({"type": "arc", "start": [0, 0], "through": [1, 1]}),
            json!({"type": "arc", "start": [0, 0], "through": [1, 1], "end": [2, 0], "radius": 1}),
            json!({"type": "xline", "origin": [1, 1], "through": [1, 1]}),
            json!({"type": "xline", "origin": [0, 0], "through": [1, 1], "angle": 45}),
        ] {
            assert!(geometry_from_json(&bad, &d).is_err(), "{bad} を拒むこと");
        }

        let Geometry::Xline(x) = geometry_from_json(
            &json!({"type": "xline", "origin": [0, 0], "through": [0, 5]}),
            &d,
        )
        .unwrap() else {
            panic!("作図線のはず")
        };
        assert!(eq_angle(x.angle(), std::f64::consts::FRAC_PI_2));
    }

    /// 大きすぎる座標は有限でも拒む。作図線は通過点で見る。
    #[test]
    fn extent_is_bounded() {
        use crate::limits::MAX_COORDINATE;
        let d = defs();
        let ok = Geometry::Line(Line::new(
            Point2::new(-MAX_COORDINATE, 0.0),
            Point2::new(MAX_COORDINATE, 0.0),
        ));
        assert!(check_extent(&ok, &d).is_ok());
        for bad in [
            Geometry::Line(Line::new(
                Point2::new(0.0, 0.0),
                Point2::new(MAX_COORDINATE * 2.0, 0.0),
            )),
            Geometry::Circle(Circle::new(Point2::new(0.0, 0.0), MAX_COORDINATE * 2.0)),
            Geometry::Xline(Xline::horizontal(Point2::new(0.0, -MAX_COORDINATE * 2.0))),
        ] {
            assert!(check_extent(&bad, &d).is_err(), "{bad:?}");
        }
    }

    /// 束縛の対象の名前は、図形の JSON の項目名と同じ綴りで、種類と組で読み戻せる。
    #[test]
    fn slot_names_round_trip() {
        let all = [
            ("line", Slot::LineAx),
            ("line", Slot::LineAy),
            ("line", Slot::LineBx),
            ("line", Slot::LineBy),
            ("circle", Slot::CircleCx),
            ("circle", Slot::CircleCy),
            ("circle", Slot::CircleR),
            ("arc", Slot::ArcCx),
            ("arc", Slot::ArcCy),
            ("arc", Slot::ArcR),
            ("arc", Slot::ArcStart),
            ("arc", Slot::ArcEnd),
            ("xline", Slot::XlineOx),
            ("xline", Slot::XlineOy),
            ("xline", Slot::XlineAngle),
            ("polyline", Slot::PolylineVx(0)),
            ("polyline", Slot::PolylineVy(12)),
            ("instance", Slot::InstanceX),
            ("instance", Slot::InstanceY),
            ("instance", Slot::InstanceRotation),
            ("instance", Slot::InstanceScale),
        ];
        for (ty, slot) in all {
            let name = slot_name(slot);
            assert_eq!(slot_from_name(ty, &name), Some(slot), "{ty} {name}");
            assert!(!name.contains('('), "Debug の綴りではない: {name}");
        }
        assert_eq!(slot_name(Slot::PolylineVy(12)), "vertices[12].y");
        for (ty, bad) in [
            ("line", "radius"),
            ("polyline", "vertices[].x"),
            ("polyline", "vertices[-1].x"),
            ("polyline", "vertices[1].z"),
            ("circle", "start.x"),
        ] {
            assert_eq!(slot_from_name(ty, bad), None, "{ty} {bad}");
        }
    }

    #[test]
    fn layer_attributes_are_read_strictly() {
        assert_eq!(
            linetype_from_json(&json!("DASHED"), "t").unwrap(),
            LineType::Dashed
        );
        for t in LineType::all() {
            assert_eq!(
                linetype_from_json(&json!(linetype_name(t)), "t").unwrap(),
                t
            );
        }
        assert!(linetype_from_json(&json!("dotted"), "t").is_err());
        assert_eq!(aci_from_json(&json!(255), "c").unwrap(), AciColor(255));
        for bad in [json!(0), json!(256), json!(-1), json!(1.5), json!("1")] {
            assert!(aci_from_json(&bad, "c").is_err(), "{bad}");
        }
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
