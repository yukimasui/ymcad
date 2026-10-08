//! リボンの割り当て表（タブ → グループ → コマンド名）。
//!
//! # `CommandSpec` に書き足さず、ここに別の表として持つ理由
//!
//! コマンド表（ADR-0016）は**コマンドの定義**（名前・エイリアス・説明・実体）で、
//! どのタブのどのグループに置くかは**画面の都合**。`CommandSpec` にタブ情報を持たせると、
//! 並びや見た目を変えるたびにコマンド表に触ることになり、コマンドを足す別の作業と衝突する。
//!
//! 代わりに、表の食い違いをテストで機械的に検出する
//! （[`tests::every_command_is_placed_exactly_once_or_excluded`] ほか）。
//! コマンドを足してリボンに置き忘れると、そのテストが落ちて気づける。
//!
//! 割り当てと理由は ADR-0037。

/// タブ 1 枚。
#[derive(Debug)]
pub struct TabSpec {
    /// タブの見出し。
    pub title: &'static str,
    /// 左から順のグループ。
    pub groups: &'static [GroupSpec],
}

/// グループ 1 つ（枠とグループ名で囲まれたボタンのまとまり）。
#[derive(Debug)]
pub struct GroupSpec {
    /// グループ名（ボタンの下に出す）。
    pub title: &'static str,
    /// 左から順のコマンド名。`tools::COMMANDS` の正式名で書く（エイリアスは不可）。
    pub commands: &'static [&'static str],
}

/// リボンの全タブ。起動時は先頭（ホーム）を開く。
///
/// 日常の作図・修正・グループ・レイヤは全部「ホーム」に置く。線を引いてすぐトリムするような
/// 日常操作でタブを切り替えさせないため（Issue #26 の「世間の不満」）。
/// UNDO / REDO / SAVE はタブには置かず、タブの行の右端に常に出す（[`QUICK_ACCESS`]）。
pub static TABS: &[TabSpec] = &[
    TabSpec {
        title: "ホーム",
        groups: &[
            GroupSpec {
                title: "作図",
                commands: &["LINE", "POLYLINE", "CIRCLE", "ARC", "RECTANGLE", "XLINE"],
            },
            GroupSpec {
                title: "修正",
                commands: &[
                    "ERASE", "MOVE", "COPY", "STRETCH", "ROTATE", "SCALE", "MIRROR", "TRIM",
                    "EXTEND", "FILLET", "CHAMFER", "EXPLODE",
                ],
            },
            GroupSpec {
                title: "グループ",
                commands: &["GROUP", "UNGROUP"],
            },
            GroupSpec {
                title: "レイヤ",
                commands: &["LAYER"],
            },
            GroupSpec {
                title: "プロパティ",
                commands: &["PROPERTIES"],
            },
            // 末尾に足した（AutoCAD もホームの右寄りの「ユーティリティ」に置く）。既存のボタンの
            // 位置を動かさないため。Ctrl+A を知らなくても見つけられるように置く（ADR-0044）。
            GroupSpec {
                title: "選択",
                commands: &["SELECTALL"],
            },
        ],
    },
    TabSpec {
        title: "コンポーネント",
        groups: &[
            GroupSpec {
                title: "定義・編集",
                commands: &["COMPONENT", "REDEFINE", "EDITCOMP", "ENDCOMP"],
            },
            GroupSpec {
                title: "配置",
                commands: &["INSERT"],
            },
            GroupSpec {
                title: "パラメータ",
                commands: &["PARAM", "BIND", "PSET"],
            },
            GroupSpec {
                title: "一覧",
                commands: &["COMPONENTS"],
            },
        ],
    },
    TabSpec {
        title: "表示・ファイル",
        groups: &[
            GroupSpec {
                title: "表示",
                commands: &["ZOOM"],
            },
            GroupSpec {
                title: "ファイル",
                commands: &["NEW", "OPEN", "SAVEAS"],
            },
        ],
    },
];

/// タブの行の右端に**どのタブを開いていても**出すボタン（AutoCAD のクイックアクセス
/// ツールバーに当たる）。左から順。
///
/// どのタブでも要るのに打つしかなかったもの（ymcad には `Ctrl+Z` が無い）。
/// ここに置いたコマンドはタブには置かない（1 つのコマンドの置き場所は 1 つ。網羅のテスト）。
pub static QUICK_ACCESS: &[&str] = &["UNDO", "REDO", "SAVE"];

/// リボンに**わざと出さない**コマンド。
///
/// - `QUIT` … 押し間違えるとアプリが閉じかける（未保存なら確認が出るが、保存済みなら
///   そのまま閉じる）。ウィンドウの ✕ と、打って使う `QUIT` で足りる
// 本体からは参照しない。網羅のテストが「わざと出さない」ことを確かめるための表。
#[cfg_attr(not(test), allow(dead_code))]
pub static EXCLUDED: &[&str] = &["QUIT"];

/// リボンに置いた全コマンド名を、タブ → グループ → ボタン → クイックアクセスの順に返す（テスト用）。
#[cfg(test)]
pub fn placed_commands() -> impl Iterator<Item = &'static str> {
    TABS.iter()
        .flat_map(|t| t.groups.iter())
        .flat_map(|g| g.commands.iter().copied())
        .chain(QUICK_ACCESS.iter().copied())
}

/// そのコマンドが置かれているタブの添字。クイックアクセスや除外のものは `None`。
#[must_use]
pub fn tab_of(name: &str) -> Option<usize> {
    TABS.iter()
        .position(|t| t.groups.iter().any(|g| g.commands.contains(&name)))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::tools::COMMANDS;

    /// 名前ごとに「リボン（タブとクイックアクセスの合計）に置いた回数」と
    /// 「除外リストに入れた回数」を数える。
    fn counts() -> BTreeMap<&'static str, (usize, usize)> {
        let mut map: BTreeMap<&'static str, (usize, usize)> = BTreeMap::new();
        for name in placed_commands() {
            map.entry(name).or_default().0 += 1;
        }
        for name in EXCLUDED {
            map.entry(name).or_default().1 += 1;
        }
        map
    }

    /// コマンド表の全コマンドが「リボン（タブかクイックアクセスのどちらか）にちょうど 1 回」か
    /// 「除外リストにちょうど 1 回」のどちらか一方に入っていること。
    /// コマンドを足して置き忘れるとここで落ちる。クイックアクセスに置いたものをタブにも
    /// 置くと 2 回になって落ちる（置き場所は 1 つ）。
    #[test]
    fn every_command_is_placed_exactly_once_or_excluded() {
        let counts = counts();
        for spec in COMMANDS {
            let (placed, excluded) = counts.get(spec.name).copied().unwrap_or_default();
            assert!(
                (placed, excluded) == (1, 0) || (placed, excluded) == (0, 1),
                "{} がリボンに {placed} 回、除外リストに {excluded} 回ある。\
                 どちらかに 1 回だけ入れること（ribbon/layout.rs）",
                spec.name
            );
        }
    }

    /// 割り当て表と除外リストに、コマンド表に無い名前（打ち間違い・エイリアス・
    /// 消したコマンド）が無いこと。
    #[test]
    fn no_unknown_names_in_the_ribbon_tables() {
        for name in counts().keys() {
            assert!(
                COMMANDS.iter().any(|c| c.name == *name),
                "{name} はコマンド表の正式名ではない（エイリアスや消したコマンドを書いていないか）"
            );
        }
    }

    /// タブ名・グループ名が空でなく、タブの中でグループ名が重複せず、空のグループが無いこと。
    #[test]
    fn tabs_and_groups_are_well_formed() {
        assert!(!TABS.is_empty());
        for tab in TABS {
            assert!(!tab.title.is_empty());
            assert!(!tab.groups.is_empty(), "{} にグループが無い", tab.title);
            let mut seen = std::collections::BTreeSet::new();
            for g in tab.groups {
                assert!(!g.title.is_empty());
                assert!(
                    seen.insert(g.title),
                    "{} でグループ名が重複: {}",
                    tab.title,
                    g.title
                );
                assert!(!g.commands.is_empty(), "{} が空", g.title);
            }
        }
    }

    /// 起動時に開く先頭タブはホームで、日常の作図・修正・レイヤが全部そこにあること。
    /// UNDO / REDO / SAVE はタブではなくクイックアクセス（どのタブでも押せる）。
    #[test]
    fn home_tab_comes_first_and_holds_daily_commands() {
        let home = &TABS[0];
        assert_eq!(home.title, "ホーム");
        let names: Vec<_> = home.groups.iter().flat_map(|g| g.commands).collect();
        for daily in ["LINE", "CIRCLE", "MOVE", "COPY", "TRIM", "FILLET", "LAYER"] {
            assert!(names.contains(&&daily), "{daily} がホームに無い");
        }
        for anywhere in ["UNDO", "REDO", "SAVE"] {
            assert!(QUICK_ACCESS.contains(&anywhere), "{anywhere}");
            assert_eq!(tab_of(anywhere), None, "{anywhere} はタブに置かない");
        }
        assert_eq!(tab_of("LINE"), Some(0));
        assert_eq!(tab_of("INSERT"), Some(1));
        assert_eq!(tab_of("QUIT"), None);
    }
}
