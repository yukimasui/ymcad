//! コンポーネントのパネル。
//!
//! # なぜ作ったか
//!
//! パラメータをコマンドラインから打つ形（`PARAM` / `BIND` / `PSET`）は、
//! **日本語入力を通すたびに手が止まって使い物にならなかった**。
//! 数値ひとつ変えるのに IME の確定を挟むのは、CAD の操作として重すぎる。
//!
//! そこで**打たずに操作できる**パネルを用意した。
//!
//! - 数値 … ドラッグで変える（範囲があればスライダ）
//! - 真偽 … チェックボックス
//! - 選択 … ドロップダウン
//! - 配置 … 一覧から選んでボタン
//!
//! **文字を打つのは名前を付けるときだけ**にする、というのが方針。
//!
//! # `Document` は変更しない
//!
//! パネルは [`Command`] を返すだけで、適用は呼び出し側が行う
//! （`docs/DECISIONS.md` の ADR-0012。レイヤパネルと同じ）。

use cad_core::command::SetInstanceOverride;
use cad_core::component::{DefinitionId, ParamDecl};
use cad_core::expr::{ParamType, Value};
use cad_core::{Command, Document, EntityId, Geometry};

use crate::selection::Selection;

/// 数値をドラッグで変えるときの 1 px あたりの変化量。
///
/// 図面の寸法は 3〜4 桁が普通なので、1 px = 1 単位だと動かしすぎる。
const DRAG_SPEED: f64 = 0.5;

/// パネルからの依頼のうち、コマンドで表せないもの。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PanelRequest {
    /// この定義を配置したい（`INSERT` を名前入力なしで始める）。
    Insert(DefinitionId),
}

/// コンポーネントのパネル。
#[derive(Debug, Default)]
pub struct ComponentPanel {
    open: bool,
}

impl ComponentPanel {
    /// 初期状態（閉じている）。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 開いているか。
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// 開閉を切り替える。
    pub fn toggle(&mut self) {
        self.open = !self.open;
    }

    /// パネルを描画し、実行すべきコマンドと依頼を返す。
    #[must_use]
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        doc: &Document,
        selection: &Selection,
    ) -> (Vec<Box<dyn Command>>, Option<PanelRequest>) {
        let mut commands: Vec<Box<dyn Command>> = Vec::new();
        let mut request = None;
        if !self.open {
            return (commands, request);
        }

        ui.heading("コンポーネント");
        ui.separator();

        Self::show_definitions(ui, doc, &mut request);
        ui.separator();
        Self::show_parameters(ui, doc, selection, &mut commands);

        (commands, request)
    }

    /// 定義の一覧。**名前を打たずに配置できる**ようにする。
    fn show_definitions(ui: &mut egui::Ui, doc: &Document, request: &mut Option<PanelRequest>) {
        ui.label("定義");
        if doc.definitions().is_empty() {
            ui.weak("（まだありません。選択して COMPONENT で作れます）");
            return;
        }

        egui::ScrollArea::vertical()
            .id_salt("component_definitions")
            .auto_shrink([false, true])
            .max_height(160.0)
            .show(ui, |ui| {
                for (id, def) in doc.definitions().iter() {
                    ui.horizontal(|ui| {
                        if ui.button("配置").clicked() {
                            *request = Some(PanelRequest::Insert(id));
                        }
                        ui.label(&def.name);
                        ui.weak(format!(
                            "（要素 {} / パラメータ {}）",
                            def.entities.len(),
                            def.params.len()
                        ));
                    });
                }
            });
    }

    /// 選択中のインスタンスのパラメータ。
    fn show_parameters(
        ui: &mut egui::Ui,
        doc: &Document,
        selection: &Selection,
        commands: &mut Vec<Box<dyn Command>>,
    ) {
        ui.label("パラメータ");

        let targets = selected_instances(doc, selection);
        if targets.is_empty() {
            ui.weak("（インスタンスを選択すると出ます）");
            return;
        }
        if targets.len() > 1 {
            ui.weak(format!(
                "（{} 個選択中。1 つだけ選ぶと編集できます）",
                targets.len()
            ));
            return;
        }

        let id = targets[0];
        let Some(entity) = doc.entities().get(id) else {
            return;
        };
        let Geometry::Instance(inst) = &entity.geom else {
            return;
        };
        let Some(def) = doc.definitions().get(inst.definition) else {
            return;
        };
        if def.params.is_empty() {
            ui.weak(format!("（「{}」にパラメータはありません）", def.name));
            return;
        }

        let env = def.param_env(&inst.overrides);
        egui::ScrollArea::vertical()
            .id_salt("component_parameters")
            .auto_shrink([false, true])
            .max_height(280.0)
            .show(ui, |ui| {
                for decl in &def.params {
                    let overridden = inst.overrides.contains_key(&decl.name);
                    let current = env.get(&decl.name).cloned();
                    Self::show_param_row(ui, id, decl, current, overridden, commands);
                }
            });
    }

    /// パラメータ 1 行。
    fn show_param_row(
        ui: &mut egui::Ui,
        target: EntityId,
        decl: &ParamDecl,
        current: Option<Value>,
        overridden: bool,
        commands: &mut Vec<Box<dyn Command>>,
    ) {
        ui.horizontal(|ui| {
            // 上書き中は名前を強調する。何を変えたかが一目で分かる。
            if overridden {
                ui.strong(&decl.name);
            } else {
                ui.label(&decl.name);
            }

            let Some(value) = current else {
                ui.weak("（値を決められません）");
                return;
            };

            if let Some(new_value) = Self::value_widget(ui, decl, &value) {
                commands.push(Box::new(SetInstanceOverride::set(
                    "PSET",
                    target,
                    decl.name.clone(),
                    new_value,
                )));
            }

            // 上書きしているときだけ戻せる。
            if overridden
                && ui
                    .button("戻す")
                    .on_hover_text("定義の既定値へ戻す")
                    .clicked()
            {
                commands.push(Box::new(SetInstanceOverride::reset(
                    "PSET",
                    target,
                    decl.name.clone(),
                )));
            }
        });
    }

    /// 型に応じた入力欄。値が変わったら新しい値を返す。
    ///
    /// **文字を打たせない。** 数値はドラッグ、真偽はチェック、選択は一覧。
    fn value_widget(ui: &mut egui::Ui, decl: &ParamDecl, value: &Value) -> Option<Value> {
        match (&decl.ty, value) {
            (ParamType::Number, Value::Number(n)) => {
                let mut v = *n;
                let response = match decl.range {
                    // 範囲があるならスライダ。端が見えるので操作しやすい。
                    Some((lo, hi)) => ui.add(
                        egui::Slider::new(&mut v, lo..=hi).clamping(egui::SliderClamping::Always),
                    ),
                    None => ui.add(egui::DragValue::new(&mut v).speed(DRAG_SPEED)),
                };
                // `changed()` はドラッグ中も真になる。コマンドは 1 手ずつ積まれるが、
                // Undo で 1 段ずつ戻るのは「動かした量が戻る」ので自然。
                (response.changed() && v != *n).then_some(Value::Number(v))
            }
            (ParamType::Bool, Value::Bool(b)) => {
                let mut v = *b;
                ui.checkbox(&mut v, "").changed().then_some(Value::Bool(v))
            }
            (ParamType::Choice(options), Value::Choice(c)) => {
                let mut picked = c.clone();
                let mut changed = false;
                egui::ComboBox::from_id_salt(("param", &decl.name))
                    .selected_text(c.as_str())
                    .show_ui(ui, |ui| {
                        for o in options {
                            if ui.selectable_label(picked == *o, o.as_str()).clicked() {
                                picked = o.clone();
                                changed = true;
                            }
                        }
                    });
                changed.then_some(Value::Choice(picked))
            }
            // 型と値がずれている。コマンド層が防いでいるので通常は起きない。
            _ => {
                ui.weak(format!("（{} として扱えません）", value.type_name()));
                None
            }
        }
    }
}

/// 選択されているインスタンスの ID。
fn selected_instances(doc: &Document, selection: &Selection) -> Vec<EntityId> {
    selection
        .iter()
        .filter(|id| {
            doc.entities()
                .get(*id)
                .is_some_and(|e| matches!(e.geom, Geometry::Instance(_)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cad_core::command::{DefineComponent, InsertInstance, SetBinding, SetDefinitionParams};
    use cad_core::component::{Binding, ParamDecl, Placement, Slot};
    use cad_core::expr::parse;
    use cad_core::geom::{Line, Point2};
    use cad_core::{Entity, LayerId};

    /// パラメータ 3 種を持つ定義と、その配置 1 つ。
    fn doc_with_params() -> (Document, EntityId) {
        let mut doc = Document::new();
        doc.apply(Box::new(DefineComponent::new(
            "COMPONENT",
            "窓",
            Point2::ORIGIN,
            vec![Entity::new(
                Geometry::Line(Line::new(Point2::ORIGIN, Point2::new(1.0, 0.0))),
                LayerId::ZERO,
            )],
        )))
        .expect("定義");
        let def = doc.definitions().by_name("窓").expect("あるはず");

        let params = vec![
            ParamDecl::number("幅", 900.0).with_range(300.0, 3000.0),
            ParamDecl::boolean("両開き", false),
            ParamDecl::choice("種別", vec!["引違い".to_owned(), "開き".to_owned()])
                .expect("候補あり"),
        ];
        doc.apply(Box::new(SetDefinitionParams::new("PARAM", def, params)))
            .expect("宣言");
        doc.apply(Box::new(SetBinding::new(
            "BIND",
            def,
            Binding::new(0, Slot::LineBx, parse("幅").expect("解析")),
        )))
        .expect("束縛");
        doc.apply(Box::new(InsertInstance::new(
            "INSERT",
            def,
            Placement::at(Point2::ORIGIN),
            LayerId::ZERO,
        )))
        .expect("配置");

        let id = doc.entities().ids().next().expect("あるはず");
        (doc, id)
    }

    /// パネルを 1 フレーム描画し、返ってきたコマンドと依頼を得る。
    fn run_panel(
        panel: &mut ComponentPanel,
        doc: &Document,
        selection: &Selection,
    ) -> (Vec<Box<dyn Command>>, Option<PanelRequest>) {
        // egui 0.36 のパネルは `&Context` ではなく `&mut Ui` を取るので、
        // テストでは `Ui` を直接作る（`docs/PROGRESS.md` の既知の落とし穴）。
        let ctx = egui::Context::default();
        ctx.begin_pass(egui::RawInput::default());
        let mut ui = egui::Ui::new(
            ctx.clone(),
            egui::Id::new("component_panel_test"),
            egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(400.0, 800.0),
            )),
        );
        let out = panel.show(&mut ui, doc, selection);
        let mut finished = ctx.end_pass();
        // `TexturesDelta` は未適用のまま drop すると panic する。
        finished.textures_delta.clear();
        out
    }

    #[test]
    fn a_closed_panel_does_nothing() {
        let (doc, _) = doc_with_params();
        let mut panel = ComponentPanel::new();
        assert!(!panel.is_open());

        let (commands, request) = run_panel(&mut panel, &doc, &Selection::new());
        assert!(commands.is_empty());
        assert!(request.is_none());
    }

    #[test]
    fn toggle_opens_and_closes() {
        let mut panel = ComponentPanel::new();
        panel.toggle();
        assert!(panel.is_open());
        panel.toggle();
        assert!(!panel.is_open());
    }

    /// **開いていれば描画できること（panic しないこと）。**
    ///
    /// 選択の有無・型の違うパラメータをすべて通す。
    #[test]
    fn the_panel_renders_in_every_state() {
        let (doc, inst) = doc_with_params();
        let mut panel = ComponentPanel::new();
        panel.toggle();

        // 選択なし。
        let (c, r) = run_panel(&mut panel, &doc, &Selection::new());
        assert!(c.is_empty() && r.is_none());

        // インスタンスを 1 つ選択（パラメータ 3 種が並ぶ）。
        let mut one = Selection::new();
        one.insert(inst);
        let (c, r) = run_panel(&mut panel, &doc, &one);
        assert!(c.is_empty(), "触っていなければコマンドは出ない");
        assert!(r.is_none());
    }

    /// 定義が無い図面でも描画できること。
    #[test]
    fn an_empty_document_renders() {
        let doc = Document::new();
        let mut panel = ComponentPanel::new();
        panel.toggle();
        let (c, r) = run_panel(&mut panel, &doc, &Selection::new());
        assert!(c.is_empty() && r.is_none());
    }

    /// インスタンス以外を選んでいてもパラメータ欄が壊れないこと。
    #[test]
    fn selecting_a_plain_entity_shows_nothing_to_edit() {
        let (mut doc, _) = doc_with_params();
        doc.apply(Box::new(cad_core::command::AddEntities::one(
            "LINE",
            Entity::new(
                Geometry::Line(Line::new(Point2::ORIGIN, Point2::new(1.0, 1.0))),
                LayerId::ZERO,
            ),
        )))
        .expect("追加");
        let plain = doc.entities().ids().last().expect("あるはず");

        let mut sel = Selection::new();
        sel.insert(plain);
        let mut panel = ComponentPanel::new();
        panel.toggle();
        let (c, _) = run_panel(&mut panel, &doc, &sel);
        assert!(c.is_empty());
    }

    /// **選択されているインスタンスだけを拾うこと。**
    #[test]
    fn only_instances_are_treated_as_targets() {
        let (mut doc, inst) = doc_with_params();
        doc.apply(Box::new(cad_core::command::AddEntities::one(
            "LINE",
            Entity::new(
                Geometry::Line(Line::new(Point2::ORIGIN, Point2::new(1.0, 1.0))),
                LayerId::ZERO,
            ),
        )))
        .expect("追加");
        let plain = doc.entities().ids().last().expect("あるはず");

        let mut sel = Selection::new();
        sel.insert(inst);
        sel.insert(plain);
        assert_eq!(
            selected_instances(&doc, &sel),
            vec![inst],
            "線分は対象にしない"
        );
    }

    /// 複数のインスタンスを選んでいるときは編集させないこと。
    ///
    /// どれの値を出すか決められないので、まとめて変える設計にするまでは避ける。
    #[test]
    fn multiple_instances_are_not_editable() {
        let (mut doc, first) = doc_with_params();
        let def = doc.definitions().by_name("窓").expect("あるはず");
        doc.apply(Box::new(InsertInstance::new(
            "INSERT",
            def,
            Placement::at(Point2::new(50.0, 0.0)),
            LayerId::ZERO,
        )))
        .expect("2 つ目");
        let second = doc.entities().ids().last().expect("あるはず");

        let mut sel = Selection::new();
        sel.insert(first);
        sel.insert(second);
        assert_eq!(selected_instances(&doc, &sel).len(), 2);

        let mut panel = ComponentPanel::new();
        panel.toggle();
        let (c, _) = run_panel(&mut panel, &doc, &sel);
        assert!(c.is_empty(), "複数選択では編集させない");
    }
}
