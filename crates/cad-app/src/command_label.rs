//! コマンドラインの案内の頭に出す、コマンドの利用者向けの名前（Issue #73）。
//!
//! パネルやグリップの操作は `Command` を返す。その `Command` の名前（`LAYER_MOVE_ENTITIES` など）は
//! Undo の履歴のための内部の名前で、打って使うコマンドではない。そのまま案内の頭に出すと
//! 利用者には意味が無いので、**案内に名前を出す箇所は必ず [`display_name`] を通す**。
//!
//! 打って使うコマンド名（`LINE` など）は変えずに返す。名前と表示名の対応はこの表だけに置く。

/// 内部のコマンド名と、案内に出す利用者向けの名前。
///
/// `PSET` / `PARAM` は打って使うコマンドでもあるが、コンポーネントパネルの操作でも同じ名前で
/// 履歴に積まれる。パネルしか使わない人には意味が無いので、両方の経路で同じ言葉に揃える。
const LABELS: &[(&str, &str)] = &[
    ("LAYER_ADD", "レイヤの追加"),
    ("LAYER_PROPS", "レイヤの属性の変更"),
    ("LAYER_RENAME", "レイヤ名の変更"),
    ("LAYER_CURRENT", "現在レイヤの切り替え"),
    ("LAYER_DELETE", "レイヤの削除"),
    ("LAYER_MOVE_ENTITIES", "レイヤの移動"),
    ("PROPERTIES", "プロパティ"),
    ("GRIP", "グリップ編集"),
    ("PSET", "パラメータの値"),
    ("PARAM", "パラメータの宣言"),
];

/// 案内の頭に出す名前。表に無い名前（打って使うコマンド）はそのまま返す。
#[must_use]
pub fn display_name(name: &str) -> &str {
    LABELS
        .iter()
        .find(|(internal, _)| *internal == name)
        .map_or(name, |(_, label)| label)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmdline::{LineKind, Submission};
    use crate::session::Session;
    use cad_core::command::{
        AddEntities, AddLayer, DeleteLayer, MoveEntitiesToLayer, RenameLayer, ReplaceGeometries,
        SetCurrentLayer, SetLayerProperties,
    };
    use cad_core::geom::{Line, Point2};
    use cad_core::{AciColor, Command, Document, Entity, EntityId, Geometry, LayerId};

    fn line(x: f64) -> Geometry {
        Geometry::Line(Line::new(Point2::ORIGIN, Point2::new(x, 0.0)))
    }

    fn lines(s: &Session, kind: LineKind) -> Vec<String> {
        s.cmdline
            .history()
            .filter(|l| l.kind == kind)
            .map(|l| l.text.clone())
            .collect()
    }

    /// 案内の中に、表にある内部の名前が 1 つでも含まれていないか。
    fn leaked(text: &str) -> Option<&'static str> {
        LABELS
            .iter()
            .map(|(internal, _)| *internal)
            .find(|internal| text.contains(internal))
    }

    #[test]
    fn typed_command_names_pass_through() {
        for name in ["LINE", "MOVE", "UNDO", "COMPONENT", "INSERT", "BIND", "ZZZ"] {
            assert_eq!(display_name(name), name);
        }
        assert_eq!(display_name("LAYER_MOVE_ENTITIES"), "レイヤの移動");
        assert_eq!(display_name("GRIP"), "グリップ編集");
    }

    /// 表の言葉が、内部の名前を含まない・重ならない・空でないこと。
    #[test]
    fn labels_are_distinct_japanese_phrases() {
        let mut seen = std::collections::BTreeSet::new();
        for (internal, label) in LABELS {
            assert!(
                !label.is_empty() && !label.chars().all(|c| c.is_ascii()),
                "{internal}"
            );
            assert!(seen.insert(*label), "重複: {label}");
            assert!(leaked(label).is_none(), "{label}");
        }
    }

    /// cad-core のレイヤ操作がつける名前は、すべて表にある（足したのに表を忘れたら落ちる）。
    #[test]
    fn every_layer_command_in_cad_core_has_a_label() {
        let cmds: Vec<Box<dyn Command>> = vec![
            Box::new(AddLayer::new("A", AciColor::WHITE)),
            Box::new(SetLayerProperties::new(LayerId::ZERO)),
            Box::new(RenameLayer::new(LayerId::ZERO, "A")),
            Box::new(SetCurrentLayer::new(LayerId::ZERO)),
            Box::new(DeleteLayer::new(LayerId::ZERO)),
            Box::new(MoveEntitiesToLayer::new(Vec::new(), LayerId::ZERO)),
        ];
        for cmd in &cmds {
            let name = cmd.name();
            assert!(name.starts_with("LAYER_"), "{name}");
            assert_ne!(display_name(name), name, "{name} の表示名が無い");
        }
    }

    /// 図形が 1 本ある図面と、その選択。
    fn setup() -> (Session, Document, EntityId, LayerId) {
        let mut doc = Document::new();
        doc.apply(Box::new(AddLayer::new("L1", AciColor::WHITE)))
            .expect("レイヤ");
        let l1 = doc.layers().by_name("L1").expect("L1");
        doc.apply(Box::new(AddEntities::one(
            "LINE",
            Entity::new(line(10.0), LayerId::ZERO),
        )))
        .expect("図形");
        let id = doc.entities().ids().last().expect("ある");
        let mut s = Session::new();
        s.selection.insert(id);
        (s, doc, id, l1)
    }

    /// パネル操作が失敗したときのエラーに、内部のコマンド名が出ない。
    #[test]
    fn failed_panel_operations_do_not_show_internal_names() {
        let (mut s, mut doc, id, l1) = setup();
        let ghost = LayerId::ZERO;
        let cmds: Vec<(Box<dyn Command>, &str)> = vec![
            (Box::new(RenameLayer::new(ghost, "X")), "レイヤ名の変更"),
            (Box::new(RenameLayer::new(l1, "0")), "レイヤ名の変更"),
            (Box::new(DeleteLayer::new(ghost)), "レイヤの削除"),
            (
                Box::new(MoveEntitiesToLayer::new(vec![id], LayerId::ZERO)),
                "レイヤの移動",
            ),
            (
                Box::new(ReplaceGeometries::one("PROPERTIES", id, line(0.0))),
                "プロパティ",
            ),
            (
                Box::new(ReplaceGeometries::one("GRIP", id, line(0.0))),
                "グリップ編集",
            ),
        ];
        let mut expected = 0;
        for (cmd, label) in cmds {
            let before = lines(&s, LineKind::Error).len();
            s.apply_external(cmd, &mut doc);
            let errors = lines(&s, LineKind::Error);
            if errors.len() > before {
                expected += 1;
                let last = errors.last().expect("ある");
                assert!(last.starts_with(&format!("{label}: ")), "{last}");
                assert!(leaked(last).is_none(), "{last}");
            }
        }
        // 失敗する操作が 1 つも無いと、この検査は何も見ていない。
        assert!(expected >= 3, "失敗した操作が少なすぎる: {expected}");
    }

    /// パネルの操作を UNDO / REDO したときの履歴の表示に、内部のコマンド名が出ない。
    #[test]
    fn undo_and_redo_history_does_not_show_internal_names() {
        let (mut s, mut doc, id, l1) = setup();
        // 現在レイヤは消せないので、後で消す用のレイヤを別に足しておく。
        doc.apply(Box::new(AddLayer::new("EXTRA", AciColor::RED)))
            .expect("レイヤ");
        let extra = doc.layers().by_name("EXTRA").expect("EXTRA");
        let cmds: Vec<(Box<dyn Command>, &str)> = vec![
            (Box::new(AddLayer::new("L2", AciColor::RED)), "レイヤの追加"),
            (
                Box::new(SetLayerProperties::new(l1).locked(true)),
                "レイヤの属性の変更",
            ),
            (Box::new(RenameLayer::new(l1, "壁")), "レイヤ名の変更"),
            (Box::new(SetCurrentLayer::new(l1)), "現在レイヤの切り替え"),
            (
                Box::new(MoveEntitiesToLayer::new(vec![id], l1)),
                "レイヤの移動",
            ),
            (
                Box::new(ReplaceGeometries::one("PROPERTIES", id, line(5.0))),
                "プロパティ",
            ),
            (
                Box::new(ReplaceGeometries::one("GRIP", id, line(7.0))),
                "グリップ編集",
            ),
            (Box::new(DeleteLayer::new(extra)), "レイヤの削除"),
        ];
        let labels: Vec<&str> = cmds.iter().map(|(_, l)| *l).collect();
        for (cmd, _) in cmds {
            s.apply_external(cmd, &mut doc);
        }
        assert!(
            lines(&s, LineKind::Error).is_empty(),
            "{:?}",
            lines(&s, LineKind::Error)
        );

        let mut shown = Vec::new();
        for _ in &labels {
            s.handle_submission(Submission::Text("UNDO".into()), &mut doc);
            shown.push(lines(&s, LineKind::Info).pop().expect("案内"));
        }
        for _ in &labels {
            s.handle_submission(Submission::Text("REDO".into()), &mut doc);
            shown.push(lines(&s, LineKind::Info).pop().expect("案内"));
        }
        for (i, text) in shown.iter().enumerate() {
            assert!(leaked(text).is_none(), "{text}");
            let (prefix, label) = if i < labels.len() {
                ("UNDO", labels[labels.len() - 1 - i])
            } else {
                ("REDO", labels[i - labels.len()])
            };
            assert_eq!(text, &format!("{prefix}: {label}"));
        }
    }

    /// 打って使うコマンドの履歴は今までどおり（LINE などはそのまま）。
    #[test]
    fn undo_of_a_typed_command_keeps_its_name() {
        let (mut s, mut doc, _, _) = setup();
        s.handle_submission(Submission::Text("UNDO".into()), &mut doc);
        assert_eq!(
            lines(&s, LineKind::Info).pop().as_deref(),
            Some("UNDO: LINE")
        );
    }

    /// コンポーネントパネルが履歴に積む `PSET` / `PARAM` も利用者向けの言葉になる。
    #[test]
    fn component_panel_names_are_translated() {
        assert_eq!(display_name("PSET"), "パラメータの値");
        assert_eq!(display_name("PARAM"), "パラメータの宣言");
    }
}
