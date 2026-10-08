//! レイヤの道具: `add_layer` / `update_layer` / `delete_layer`。
//!
//! 共通の約束は [`super::mutate`]（1 回の呼び出しで `apply` 1 回）。

use cad_core::command::{
    AddLayer, DeleteLayer, EditCtx, RenameLayer, SetCurrentLayer, SetLayerProperties,
};
use cad_core::layer::LineType;
use cad_core::{AciColor, Command, Document, LayerId};
use serde_json::{json, Value};

use super::mutate::apply;
use super::query::entity_counts_by_layer;
use super::{Args, Tool, ToolResult};
use crate::convert::{aci_from_json, layer_to_json, linetype_from_json, LINETYPES};
use crate::limits::MAX_LAYER_NAME_CHARS;
use crate::server::Server;

pub(super) const ADD_LAYER: Tool = Tool {
    name: "add_layer",
    title: "レイヤを作る",
    description: "レイヤを作る。color は ACI の色番号 1〜255（1 赤・2 黄・3 緑・4 水色・5 青・6 紫・7 白）、\
linetype は continuous / dashed / center / hidden（省略時 continuous）。同じ名前のレイヤがあれば拒む。\
1 回の呼び出しが undo 1 回ぶん。現在レイヤは変えない（変えるなら update_layer の make_current）。",
    schema: || {
        (
            json!({
                "name": name_schema("作るレイヤの名前"),
                "color": color_schema(),
                "linetype": linetype_schema(),
            }),
            &["name", "color"],
        )
    },
    read_only: false,
    destructive: false,
    idempotent: false,
    run: add_layer,
};

pub(super) const UPDATE_LAYER: Tool = Tool {
    name: "update_layer",
    title: "レイヤを変える",
    description: "レイヤ（name）の名前・色・表示・ロック・線種を、指定した項目だけ変える。make_current: true で現在レイヤにする\
（新しく描く図形の既定の置き場所）。全部で 1 回の呼び出しが undo 1 回ぶん。レイヤ 0 の名前は変えられない。\
非表示・ロック中のレイヤの図形は他の道具で変えられないので、変える前に visible: true / locked: false にする。",
    schema: || {
        (
            json!({
                "name": { "type": "string", "description": "変えるレイヤの名前（いまの名前）" },
                "rename_to": name_schema("新しい名前"),
                "color": color_schema(),
                "visible": { "type": "boolean", "description": "表示するか" },
                "locked": { "type": "boolean", "description": "ロックするか（ロック中の図形は変えられない）" },
                "linetype": linetype_schema(),
                "make_current": { "type": "boolean", "description": "true なら現在レイヤにする" },
            }),
            &["name"],
        )
    },
    read_only: false,
    destructive: true,
    idempotent: true,
    run: update_layer,
};

pub(super) const DELETE_LAYER: Tool = Tool {
    name: "delete_layer",
    title: "レイヤを消す",
    description: "レイヤを、そこに置かれた図形ごと消す（消えた図形の数を deleted_entities に返す。undo で同じ ID のまま戻る）。\
レイヤ 0・現在レイヤは消せない。ロック中のレイヤに図形があれば拒む（先に update_layer で locked: false）。\
コンポーネントの定義の中の図形がこのレイヤを使っていると、保存したときにレイヤ 0 に移るので warnings に出す。",
    schema: || {
        (
            json!({ "name": { "type": "string", "description": "消すレイヤの名前" } }),
            &["name"],
        )
    },
    read_only: false,
    destructive: true,
    idempotent: false,
    run: delete_layer,
};

fn name_schema(description: &str) -> Value {
    json!({
        "type": "string",
        "minLength": 1,
        "description": format!("{description}（前後に空白を付けない・改行などの制御文字を含めない・{MAX_LAYER_NAME_CHARS} 文字まで）"),
    })
}

fn color_schema() -> Value {
    json!({
        "type": "integer", "minimum": 1, "maximum": 255,
        "description": "ACI の色番号 1〜255（1 赤・2 黄・3 緑・4 水色・5 青・6 紫・7 白）",
    })
}

fn linetype_schema() -> Value {
    json!({ "type": "string", "enum": LINETYPES, "description": "線種" })
}

/// レイヤ名として使えるか。アプリのレイヤパネルは前後の空白を落として受け取るが、
/// ここでは黙って直さずに拒む（LLM が渡した名前と違う名前のレイヤができると、後の呼び出しで見つからない）。
fn check_layer_name(doc: &Document, name: &str, own: Option<LayerId>) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("レイヤ名が空です".to_owned());
    }
    if name.trim() != name {
        return Err(format!(
            "レイヤ名 {name:?} の前後に空白があります（空白を除いて指定してください）"
        ));
    }
    if name.chars().any(char::is_control) {
        return Err(format!(
            "レイヤ名 {name:?} に改行などの制御文字が含まれています"
        ));
    }
    if name.chars().count() > MAX_LAYER_NAME_CHARS {
        return Err(format!(
            "レイヤ名が長すぎます（上限 {MAX_LAYER_NAME_CHARS} 文字）"
        ));
    }
    match doc.layers().by_name(name) {
        Some(other) if Some(other) != own => Err(format!(
            "レイヤ {name} は既にあります（list_layers で一覧を見られます）"
        )),
        _ => Ok(()),
    }
}

fn find_layer(doc: &Document, name: &str) -> Result<LayerId, String> {
    doc.layers()
        .by_name(name)
        .ok_or_else(|| format!("レイヤ {name} はありません（list_layers で一覧を見られます）"))
}

/// 作ったレイヤの JSON。
fn layer_json(doc: &Document, id: LayerId) -> Value {
    let counts = entity_counts_by_layer(doc);
    doc.layers().get(id).map_or(Value::Null, |l| {
        layer_to_json(
            l,
            id == doc.layers().current(),
            counts.get(&id).copied().unwrap_or(0),
        )
    })
}

/// レイヤを作り、線種まで設定する 1 つのコマンド。
///
/// `AddLayer` は作ったレイヤの ID を適用するまで返さないので、`SetLayerProperties` を
/// `MacroCommand` で後ろに並べられない（作る前に ID を指定できない）。そこで `AddLayer` を包み、
/// 適用の中で作ったレイヤの線種だけを変える。取り消しは `AddLayer` の取り消し（レイヤごと消す）に任せる。
/// 変えるのは自分が作ったばかりのレイヤだけで、図形には触らない。
#[derive(Debug)]
struct AddLayerWithLinetype {
    add: AddLayer,
    linetype: LineType,
}

impl Command for AddLayerWithLinetype {
    fn execute(&mut self, ctx: &mut EditCtx<'_>) -> cad_core::Result<()> {
        self.add.execute(ctx)?;
        let set = self
            .add
            .created()
            .ok_or(cad_core::CadError::LayerNotFound)
            .and_then(|id| ctx.layer_mut(id).map(|l| l.linetype = self.linetype));
        if let Err(e) = set {
            // 「全部成功するか、何も変えずに失敗するか」を守る。
            let _ = self.add.undo(ctx);
            return Err(e);
        }
        Ok(())
    }

    fn undo(&mut self, ctx: &mut EditCtx<'_>) -> cad_core::Result<()> {
        self.add.undo(ctx)
    }

    fn name(&self) -> &'static str {
        self.add.name()
    }
}

fn add_layer(s: &mut Server, a: &Args) -> ToolResult {
    let name = a.req_str("name")?.to_owned();
    let color = aci_from_json(
        a.value("color")
            .ok_or_else(|| "color を指定してください".to_owned())?,
        "color",
    )?;
    let linetype = a
        .value("linetype")
        .map(|v| linetype_from_json(v, "linetype"))
        .transpose()?;
    // `LayerTable::insert` は同じ名前があると既存の ID を返し、`AddLayer` の取り消しが
    // **既存のレイヤを消してしまう**。必ず先に拒む。
    check_layer_name(&s.doc, &name, None)?;

    let add = AddLayer::new(name.clone(), color);
    let command: Box<dyn Command> = match linetype {
        None | Some(LineType::Continuous) => Box::new(add),
        Some(linetype) => Box::new(AddLayerWithLinetype { add, linetype }),
    };
    apply(s, "LAYER_ADD", vec![command])?;
    let id = find_layer(&s.doc, &name)?;
    Ok(json!({ "drawing": s.tag().name(), "layer": layer_json(&s.doc, id) }))
}

fn update_layer(s: &mut Server, a: &Args) -> ToolResult {
    let name = a.req_str("name")?;
    let id = find_layer(&s.doc, name)?;
    let rename_to = a.opt_str("rename_to")?;
    let color: Option<AciColor> = a
        .value("color")
        .map(|v| aci_from_json(v, "color"))
        .transpose()?;
    let visible = a
        .value("visible")
        .map(|_| a.bool_or("visible", true))
        .transpose()?;
    let locked = a
        .value("locked")
        .map(|_| a.bool_or("locked", false))
        .transpose()?;
    let linetype = a
        .value("linetype")
        .map(|v| linetype_from_json(v, "linetype"))
        .transpose()?;
    let make_current = a.bool_or("make_current", false)?;

    let mut commands: Vec<Box<dyn Command>> = Vec::new();
    if color.is_some() || visible.is_some() || locked.is_some() || linetype.is_some() {
        let mut props = SetLayerProperties::new(id);
        if let Some(c) = color {
            props = props.color(c);
        }
        if let Some(v) = visible {
            props = props.visible(v);
        }
        if let Some(v) = locked {
            props = props.locked(v);
        }
        if let Some(t) = linetype {
            props = props.linetype(t);
        }
        commands.push(Box::new(props));
    }
    if let Some(new_name) = rename_to.filter(|n| *n != name) {
        if id == LayerId::ZERO {
            return Err("レイヤ 0 の名前は変えられません".to_owned());
        }
        check_layer_name(&s.doc, new_name, Some(id))?;
        commands.push(Box::new(RenameLayer::new(id, new_name)));
    }
    if make_current && s.doc.layers().current() != id {
        commands.push(Box::new(SetCurrentLayer::new(id)));
    }
    if commands.is_empty() {
        return Err(
            "変える項目がありません（rename_to・color・visible・locked・linetype・make_current のどれかを指定してください）"
                .to_owned(),
        );
    }
    apply(s, "LAYER", commands)?;
    Ok(json!({ "drawing": s.tag().name(), "layer": layer_json(&s.doc, id) }))
}

fn delete_layer(s: &mut Server, a: &Args) -> ToolResult {
    let name = a.req_str("name")?;
    let id = find_layer(&s.doc, name)?;
    if id == LayerId::ZERO {
        return Err("レイヤ 0 は消せません".to_owned());
    }
    if id == s.doc.layers().current() {
        return Err(format!(
            "レイヤ {name} は現在レイヤなので消せません（先に update_layer の make_current で別のレイヤを現在にしてください）"
        ));
    }
    let count = entity_counts_by_layer(&s.doc)
        .get(&id)
        .copied()
        .unwrap_or(0);
    let locked = s.doc.layers().get(id).is_some_and(|l| l.locked);
    if locked && count > 0 {
        return Err(format!(
            "レイヤ {name} はロック中で、図形が {count} 個あります。図形ごと消すなら先に update_layer で locked: false にしてください"
        ));
    }
    let in_definitions = definition_entities_on(&s.doc, id);

    apply(s, "LAYER_DELETE", vec![Box::new(DeleteLayer::new(id))])?;
    let mut warnings = Vec::new();
    if in_definitions > 0 {
        warnings.push(format!(
            "コンポーネントの定義の中の図形 {in_definitions} 個がレイヤ {name} を使っています。保存すると、それらはレイヤ 0 として書かれます"
        ));
    }
    Ok(json!({
        "drawing": s.tag().name(),
        "deleted_layer": name,
        "deleted_entities": count,
        "warnings": warnings,
    }))
}

/// コンポーネントの定義の中で、`layer` に置かれた図形の数。
///
/// `DeleteLayer` は図面に直接置かれた図形しか消さない（定義の中身はコンポーネントの一部なので触らない）。
/// 残った図形は消えたレイヤを指したままになり、`.ymc` の書き出しはそれをレイヤ 0 として書く。
fn definition_entities_on(doc: &Document, layer: LayerId) -> usize {
    doc.definitions()
        .iter()
        .flat_map(|(_, d)| d.entities.iter())
        .filter(|e| e.layer == layer)
        .count()
}

#[cfg(test)]
mod tests {
    use super::super::draw::tests::layered;
    use super::super::test_support::{eid, err, mutate, ok, rejected, server};
    use super::*;
    use crate::test_util::TempDir;

    fn layer_of<'a>(list: &'a Value, name: &str) -> &'a Value {
        list["layers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|l| l["name"] == name)
            .unwrap_or_else(|| panic!("レイヤ {name} が無い: {list}"))
    }

    #[test]
    fn add_layer_with_color_and_linetype() {
        let dir = TempDir::new("layers-add");
        let mut s = server(&dir);
        let r = mutate(&mut s, "add_layer", json!({"name": "壁", "color": 1}));
        assert_eq!(r["layer"]["name"], "壁");
        assert_eq!(r["layer"]["color"], 1);
        assert_eq!(r["layer"]["linetype"], "continuous");
        assert_eq!(r["layer"]["current"], false, "現在レイヤは変えない");
        // 線種つきでも 1 回の apply（undo 1 回でレイヤごと消える。mutate がバイト列で確かめる）。
        let r = mutate(
            &mut s,
            "add_layer",
            json!({"name": "中心線", "color": 4, "linetype": "CENTER"}),
        );
        assert_eq!(r["layer"]["linetype"], "center");
        assert_eq!(s.doc.history().undo_name(), Some("LAYER_ADD"));
        let list = ok(&mut s, "list_layers", json!({}));
        assert_eq!(layer_of(&list, "中心線")["linetype"], "center");
    }

    /// `LayerTable::insert` は同じ名前で既存の ID を返すので、`AddLayer` を通すと取り消しで
    /// 既存のレイヤが消える。必ず先に拒むこと。
    #[test]
    fn add_layer_rejects_duplicates_and_bad_names() {
        let dir = TempDir::new("layers-add-bad");
        let mut s = layered(&dir);
        for (args, needle) in [
            (json!({"name": "WALL", "color": 1}), "既に"),
            (json!({"name": "0", "color": 1}), "既に"),
            (json!({"name": "", "color": 1}), "空"),
            (json!({"name": "  ", "color": 1}), "空"),
            (json!({"name": " A", "color": 1}), "空白"),
            (json!({"name": "A\nB", "color": 1}), "制御文字"),
            (
                json!({"name": "x".repeat(MAX_LAYER_NAME_CHARS + 1), "color": 1}),
                "長すぎ",
            ),
            (json!({"name": "A", "color": 0}), "1〜255"),
            (json!({"name": "A", "color": 256}), "1〜255"),
            (json!({"name": "A", "color": "red"}), "1〜255"),
            (json!({"name": "A"}), "color"),
            (
                json!({"name": "A", "color": 1, "linetype": "dotted"}),
                "扱えません",
            ),
        ] {
            let msg = rejected(&mut s, "add_layer", args.clone());
            assert!(msg.contains(needle), "{args}: {msg}");
        }
        let list = ok(&mut s, "list_layers", json!({}));
        assert_eq!(
            layer_of(&list, "WALL")["entity_count"],
            1,
            "既存のレイヤはそのまま"
        );
    }

    /// 複数の項目の変更は 1 つの MacroCommand（undo 1 回）。Macro を外すと履歴が 2 つ以上増えて落ちる。
    #[test]
    fn update_layer_is_one_undo_step() {
        let dir = TempDir::new("layers-update");
        let mut s = layered(&dir);
        let r = mutate(
            &mut s,
            "update_layer",
            json!({"name": "WALL", "rename_to": "外壁", "color": 5, "linetype": "dashed", "make_current": true}),
        );
        assert_eq!(r["layer"]["name"], "外壁");
        assert_eq!(r["layer"]["color"], 5);
        assert_eq!(r["layer"]["linetype"], "dashed");
        assert_eq!(r["layer"]["current"], true);
        assert_eq!(s.doc.history().undo_name(), Some("LAYER"));
        // 名前が変わっても図形は同じレイヤのまま。
        let wall_line = eid(&s, 1);
        let got = ok(&mut s, "get_entities", json!({ "ids": [wall_line] }));
        assert_eq!(got["entities"][0]["layer"], "外壁");

        // ロックを外すと、その図形を変えられるようになる。
        let locked = eid(&s, 2);
        err(&mut s, "delete_entities", json!({ "ids": [locked] }));
        mutate(
            &mut s,
            "update_layer",
            json!({"name": "LOCK", "locked": false}),
        );
        ok(&mut s, "delete_entities", json!({ "ids": [locked] }));
        mutate(
            &mut s,
            "update_layer",
            json!({"name": "HIDE", "visible": true}),
        );
    }

    #[test]
    fn update_layer_rejects_bad_changes() {
        let dir = TempDir::new("layers-update-bad");
        let mut s = layered(&dir);
        for (args, needle) in [
            (json!({"name": "0", "rename_to": "ZERO"}), "レイヤ 0"),
            (json!({"name": "WALL", "rename_to": "LOCK"}), "既に"),
            (json!({"name": "WALL", "rename_to": ""}), "空"),
            (json!({"name": "WALL"}), "変える項目"),
            (json!({"name": "WALL", "rename_to": "WALL"}), "変える項目"),
            (json!({"name": "WALL", "make_current": false}), "変える項目"),
            (json!({"name": "NOPE", "color": 1}), "ありません"),
            (json!({"name": "WALL", "color": 300}), "1〜255"),
            (json!({"name": "WALL", "visible": "yes"}), "true か false"),
            // 1 つでもだめなら他の項目も変えない。
            (
                json!({"name": "WALL", "color": 3, "rename_to": "HIDE"}),
                "既に",
            ),
        ] {
            let msg = rejected(&mut s, "update_layer", args.clone());
            assert!(msg.contains(needle), "{args}: {msg}");
        }
    }

    #[test]
    fn delete_layer_takes_its_entities_and_undo_restores_them() {
        let dir = TempDir::new("layers-delete");
        let mut s = layered(&dir);
        let wall_line = eid(&s, 1);
        let r = mutate(&mut s, "delete_layer", json!({"name": "WALL"}));
        assert_eq!(r["deleted_entities"], 1);
        assert_eq!(r["warnings"], json!([]));
        err(&mut s, "get_entities", json!({ "ids": [wall_line] }));
        ok(&mut s, "undo", json!({}));
        let got = ok(&mut s, "get_entities", json!({ "ids": [wall_line] }));
        assert_eq!(
            got["entities"][0]["layer"], "WALL",
            "同じ ID・同じレイヤで戻る"
        );

        // 非表示のレイヤは消せる（ロックではないので）。
        mutate(&mut s, "delete_layer", json!({"name": "HIDE"}));

        ok(
            &mut s,
            "update_layer",
            json!({"name": "WALL", "make_current": true}),
        );
        for (name, needle) in [
            ("0", "レイヤ 0"),
            ("WALL", "現在レイヤ"),
            ("LOCK", "ロック中"),
            ("NOPE", "ありません"),
        ] {
            let msg = rejected(&mut s, "delete_layer", json!({ "name": name }));
            assert!(msg.contains(needle), "{name}: {msg}");
        }
        // ロック中でも図形が無ければ消せる。
        ok(&mut s, "add_layer", json!({"name": "EMPTY", "color": 2}));
        ok(
            &mut s,
            "update_layer",
            json!({"name": "EMPTY", "locked": true}),
        );
        let r = mutate(&mut s, "delete_layer", json!({"name": "EMPTY"}));
        assert_eq!(r["deleted_entities"], 0);
    }

    /// 定義の中の図形が使うレイヤを消すと、保存でレイヤ 0 に移ることを警告する。
    #[test]
    fn delete_layer_warns_about_component_contents() {
        use cad_core::command::DefineComponent;
        use cad_core::geom::{Line, Point2};
        use cad_core::{Entity, Geometry};

        let dir = TempDir::new("layers-delete-def");
        let mut s = layered(&dir);
        let wall = s.doc.layers().by_name("WALL").unwrap();
        s.doc
            .apply(Box::new(DefineComponent::new(
                "COMPONENT",
                "BOLT",
                Point2::new(0.0, 0.0),
                vec![Entity::new(
                    Geometry::Line(Line::new(Point2::new(0.0, 0.0), Point2::new(1.0, 0.0))),
                    wall,
                )],
            )))
            .unwrap();
        let r = ok(&mut s, "delete_layer", json!({"name": "WALL"}));
        let w = r["warnings"][0].as_str().unwrap();
        assert!(w.contains("BOLT") || w.contains("1 個"), "{w}");
    }
}
