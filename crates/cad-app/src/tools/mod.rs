//! 対話的なコマンド（ツール）の枠組み。
//!
//! AutoCAD のコマンドは「プロンプトを出す → 点やオプションを受け取る →
//! 図形を作る」という多段の対話になる。その状態機械を [`Tool`] として表す。
//!
//! # 責務の分け方
//!
//! - [`Tool`] は **入力を溜めて、確定したら [`Command`] を組み立てて返す**だけ。
//!   図面を直接いじらない。適用は `Document::apply` の仕事。
//! - 確定前のラバーバンドは [`Tool::preview`] が返す。これは
//!   **`Document` に入らない派生データ**で、Undo の対象にもならない。
//!   「途中の図形をとりあえず図面に入れて後で消す」ことは絶対にしない
//!   （Undo 履歴が汚れ、`EditCtx` を唯一の変更経路にした意味が失われる）。

pub mod draw;
pub mod edit;

use cad_core::geom::{Aabb, Point2};
use cad_core::{Command, Document, Geometry, LayerId};

use crate::file_ops::FileAction;
use crate::input::ViewAction;
use crate::selection::Selection;

/// ツールに渡す 1 手ぶんの入力。
#[derive(Clone, Debug, PartialEq)]
pub enum StepInput {
    /// 点が指定された（クリック、または座標の直接入力）。
    Point(Point2),
    /// オプションや文字列が入力された（`C`、`D`、`2P` など）。大文字化済み。
    Word(String),
    /// 数値が入力された（半径など）。
    Number(f64),
    /// 空のまま確定された（コマンドの終了を意味することが多い）。
    Enter,
    /// 選択が確定した。
    ///
    /// [`Tool::wants_selection`] が真のツールにだけ送られる。
    /// これを受け取った時点で `ctx.selection` に対象が入っている。
    SelectionReady,
}

/// ツールが 1 手を処理した結果。
#[derive(Debug)]
pub enum StepOutcome {
    /// まだ入力を待つ。
    Continue,
    /// コマンドを適用して終了する。
    Apply(Box<dyn Command>),
    /// コマンドを適用して、同じツールのまま入力を続ける。
    ///
    /// LINE の各セグメントや COPY の複数回コピーで使う。
    ApplyAndContinue(Box<dyn Command>),
    /// ビュー操作を行って終了する（ZOOM）。
    View(ViewAction),
    /// 何も適用せずに終了する。
    Finish,
    /// 入力を受け付けず、メッセージを出して同じ状態のまま待つ。
    Reject(String),
}

/// ツールが図面を読むための文脈。
#[derive(Clone, Copy)]
pub struct ToolCtx<'a> {
    /// 図面（読み取り専用）。
    pub doc: &'a Document,
    /// 現在の選択。
    pub selection: &'a Selection,
    /// 新規作成する要素が入るレイヤ。
    pub layer: LayerId,
    /// 選択に使われた交差窓の矩形（モデル座標）。
    ///
    /// STRETCH が「どの点を動かすか」を決めるのに使う。
    /// AutoCAD は交差窓を複数回重ねられるので、1 つではなくスライスで持つ。
    /// クリックや窓選択だけで選ばれた場合は空になり、STRETCH は丸ごと移動になる。
    pub crossing_rects: &'a [Aabb],
}

/// 対話的コマンドの状態機械。
pub trait Tool: std::fmt::Debug {
    /// コマンド名（`"LINE"` など）。履歴表示と再実行に使う。
    fn name(&self) -> &'static str;

    /// いま表示すべきプロンプト（例: `線分の次の点を指定:`）。
    fn prompt(&self) -> String;

    /// 相対座標入力（`@100,50`）の基準となる直前の点。
    fn last_point(&self) -> Option<Point2> {
        None
    }

    /// 開始時に選択を必要とするか（ERASE / MOVE / COPY）。
    fn wants_selection(&self) -> bool {
        false
    }

    /// 1 手ぶんの入力を処理する。
    fn step(&mut self, input: StepInput, ctx: &ToolCtx<'_>) -> StepOutcome;

    /// カーソル位置に応じたラバーバンド。`Document` には入らない。
    fn preview(&self, _cursor: Point2, _ctx: &ToolCtx<'_>) -> Vec<Geometry> {
        Vec::new()
    }
}

/// コマンドの実体。
#[derive(Debug)]
pub enum CommandKind {
    /// 対話的なツール。呼ぶたびに新しい状態機械を作る。
    Tool(fn() -> Box<dyn Tool>),
    /// 対話を伴わないコマンド。
    Immediate(Immediate),
}

/// コマンド 1 つぶんの定義。
///
/// `match` 文ではなくこの表にしているのは、**コマンドを列挙できるようにする**ため。
/// コマンドラインの候補表示は先頭一致で全コマンドを走査する必要があり、
/// `match` のままでは名前を取り出せない。
pub struct CommandSpec {
    /// 正式名（`"LINE"`）。
    pub name: &'static str,
    /// エイリアス（`["L"]`）。
    pub aliases: &'static [&'static str],
    /// 候補一覧に出す一行説明。
    pub summary: &'static str,
    /// 実体。
    pub kind: CommandKind,
}

impl std::fmt::Debug for CommandSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 中身の関数ポインタは出しても意味がないので、名前だけ見せる。
        f.debug_struct("CommandSpec")
            .field("name", &self.name)
            .field("aliases", &self.aliases)
            .finish_non_exhaustive()
    }
}

impl CommandSpec {
    /// 入力がこのコマンドの名前かエイリアスと完全一致するか。`input` は大文字化済みであること。
    fn matches_exactly(&self, input: &str) -> bool {
        self.name == input || self.aliases.contains(&input)
    }

    /// 入力がこのコマンドの名前かエイリアスの先頭に一致するか。`input` は大文字化済みであること。
    fn starts_with(&self, input: &str) -> bool {
        self.name.starts_with(input) || self.aliases.iter().any(|a| a.starts_with(input))
    }

    /// 候補一覧に出すエイリアスの表示（`"L"`、`"RECTANG, REC"`）。
    #[must_use]
    pub fn alias_text(&self) -> String {
        self.aliases.join(", ")
    }
}

/// 全コマンド。候補一覧にもこの順で出る。
pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        name: "LINE",
        aliases: &["L"],
        summary: "連続線分",
        kind: CommandKind::Tool(|| Box::new(draw::LineTool::default())),
    },
    CommandSpec {
        name: "CIRCLE",
        aliases: &["C"],
        summary: "円（中心+半径 / 直径 / 2点）",
        kind: CommandKind::Tool(|| Box::new(draw::CircleTool::default())),
    },
    CommandSpec {
        name: "ARC",
        aliases: &["A"],
        summary: "円弧（3点指定）",
        kind: CommandKind::Tool(|| Box::new(draw::ArcTool::default())),
    },
    CommandSpec {
        name: "RECTANGLE",
        aliases: &["RECTANG", "REC"],
        summary: "矩形（対角2点）",
        kind: CommandKind::Tool(|| Box::new(draw::RectangleTool::default())),
    },
    CommandSpec {
        name: "POLYLINE",
        aliases: &["PLINE", "PL"],
        summary: "連結ポリライン",
        kind: CommandKind::Tool(|| Box::new(draw::PolylineTool::default())),
    },
    CommandSpec {
        name: "XLINE",
        aliases: &["XL"],
        summary: "無限長の作図線",
        kind: CommandKind::Tool(|| Box::new(draw::XlineTool::default())),
    },
    CommandSpec {
        name: "ERASE",
        aliases: &["E", "DEL"],
        summary: "選択オブジェクトの削除",
        kind: CommandKind::Tool(|| Box::new(edit::EraseTool)),
    },
    CommandSpec {
        name: "MOVE",
        aliases: &["M"],
        summary: "移動（基点→目的点）",
        kind: CommandKind::Tool(|| Box::new(edit::MoveTool::default())),
    },
    CommandSpec {
        name: "COPY",
        aliases: &["CO", "CP"],
        summary: "複写（複数回継続）",
        kind: CommandKind::Tool(|| Box::new(edit::CopyTool::default())),
    },
    CommandSpec {
        name: "STRETCH",
        aliases: &["S"],
        summary: "交差範囲内の点だけを移動",
        kind: CommandKind::Tool(|| Box::new(edit::StretchTool::default())),
    },
    CommandSpec {
        name: "ROTATE",
        aliases: &["RO"],
        summary: "回転（基点+角度）",
        kind: CommandKind::Tool(|| Box::new(edit::RotateTool::default())),
    },
    CommandSpec {
        name: "SCALE",
        aliases: &["SC"],
        summary: "拡大縮小（基点+尺度）",
        kind: CommandKind::Tool(|| Box::new(edit::ScaleTool::default())),
    },
    CommandSpec {
        name: "MIRROR",
        aliases: &["MI"],
        summary: "鏡像（対称軸の2点）",
        kind: CommandKind::Tool(|| Box::new(edit::MirrorTool::default())),
    },
    CommandSpec {
        name: "GROUP",
        aliases: &["G"],
        summary: "選択をグループ化",
        kind: CommandKind::Tool(|| Box::new(edit::GroupTool::default())),
    },
    CommandSpec {
        name: "UNGROUP",
        aliases: &["UNG"],
        summary: "グループを解除",
        kind: CommandKind::Tool(|| Box::new(edit::UngroupTool)),
    },
    CommandSpec {
        name: "EXPLODE",
        aliases: &["X"],
        summary: "ポリラインを線分へ分解",
        kind: CommandKind::Tool(|| Box::new(edit::ExplodeTool)),
    },
    CommandSpec {
        name: "ZOOM",
        aliases: &["Z"],
        summary: "表示範囲（全体 / 範囲）",
        kind: CommandKind::Tool(|| Box::new(edit::ZoomTool)),
    },
    CommandSpec {
        name: "UNDO",
        aliases: &["U"],
        summary: "直前の操作を取り消す",
        kind: CommandKind::Immediate(Immediate::Undo),
    },
    CommandSpec {
        name: "REDO",
        aliases: &[],
        summary: "取り消した操作をやり直す",
        kind: CommandKind::Immediate(Immediate::Redo),
    },
    CommandSpec {
        name: "LAYER",
        aliases: &["LA"],
        summary: "レイヤパネルの開閉",
        kind: CommandKind::Immediate(Immediate::LayerPanel),
    },
    CommandSpec {
        name: "NEW",
        aliases: &[],
        summary: "新規図面",
        kind: CommandKind::Immediate(Immediate::File(FileAction::New)),
    },
    CommandSpec {
        name: "OPEN",
        aliases: &[],
        summary: "DXF を開く",
        kind: CommandKind::Immediate(Immediate::File(FileAction::Open)),
    },
    CommandSpec {
        name: "SAVE",
        aliases: &["QSAVE"],
        summary: "上書き保存",
        kind: CommandKind::Immediate(Immediate::File(FileAction::Save)),
    },
    CommandSpec {
        name: "SAVEAS",
        aliases: &[],
        summary: "名前を付けて保存",
        kind: CommandKind::Immediate(Immediate::File(FileAction::SaveAs)),
    },
    CommandSpec {
        name: "QUIT",
        aliases: &["EXIT"],
        summary: "終了",
        kind: CommandKind::Immediate(Immediate::File(FileAction::Quit)),
    },
];

/// 候補一覧に出す最大件数。
pub const MAX_SUGGESTIONS: usize = 8;

/// 名前またはエイリアスの完全一致でコマンドを引く。大文字小文字は区別しない。
#[must_use]
pub fn lookup(input: &str) -> Option<&'static CommandSpec> {
    let upper = input.trim().to_uppercase();
    if upper.is_empty() {
        return None;
    }
    COMMANDS.iter().find(|c| c.matches_exactly(&upper))
}

/// 先頭一致する候補を返す。名前とエイリアスの両方を見る。
///
/// 並び順は「エイリアス完全一致 → 名前の先頭一致 → エイリアスのみの先頭一致」。
/// `L` と打ったとき `LINE`（エイリアス完全一致）が先頭に来てほしいため。
#[must_use]
pub fn suggestions(prefix: &str) -> Vec<&'static CommandSpec> {
    let upper = prefix.trim().to_uppercase();
    if upper.is_empty() {
        return Vec::new();
    }

    let mut hits: Vec<&'static CommandSpec> =
        COMMANDS.iter().filter(|c| c.starts_with(&upper)).collect();

    // 安定ソートなので、同じ順位のものは COMMANDS の並び順が保たれる。
    hits.sort_by_key(|c| {
        if c.aliases.contains(&upper.as_str()) {
            0
        } else if c.name.starts_with(&upper) {
            1
        } else {
            2
        }
    });
    hits.truncate(MAX_SUGGESTIONS);
    hits
}

/// コマンド名またはエイリアスからツールを作る。
///
/// 大文字小文字は区別しない。
#[must_use]
pub fn create(input: &str) -> Option<Box<dyn Tool>> {
    match lookup(input)?.kind {
        CommandKind::Tool(make) => Some(make()),
        CommandKind::Immediate(_) => None,
    }
}

/// 即座に実行できるコマンドか調べる。
///
/// UNDO / REDO などは対話が無いのでツールにしない。
#[must_use]
pub fn immediate(input: &str) -> Option<Immediate> {
    match lookup(input)?.kind {
        CommandKind::Immediate(cmd) => Some(cmd),
        CommandKind::Tool(_) => None,
    }
}

/// 対話を伴わないコマンド。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Immediate {
    /// 直前の操作を取り消す。
    Undo,
    /// 取り消した操作をやり直す。
    Redo,
    /// レイヤパネルの開閉。
    LayerPanel,
    /// ファイル操作。
    File(FileAction),
}

impl Immediate {
    /// 履歴表示に使う名前。
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Undo => "UNDO",
            Self::Redo => "REDO",
            Self::LayerPanel => "LAYER",
            Self::File(a) => a.command_name(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_resolve_to_expected_tools() {
        for (alias, expected) in [
            ("L", "LINE"),
            ("line", "LINE"),
            ("C", "CIRCLE"),
            ("A", "ARC"),
            ("REC", "RECTANGLE"),
            ("PL", "POLYLINE"),
            ("E", "ERASE"),
            ("M", "MOVE"),
            ("CO", "COPY"),
            ("Z", "ZOOM"),
        ] {
            let tool = create(alias).unwrap_or_else(|| panic!("{alias} が解決できない"));
            assert_eq!(tool.name(), expected, "エイリアス {alias}");
        }
    }

    /// 指示書のコマンド表にある正式名がすべて起動できること。
    #[test]
    fn canonical_names_resolve() {
        for name in [
            "LINE",
            "CIRCLE",
            "ARC",
            "RECTANGLE",
            "POLYLINE",
            "ERASE",
            "MOVE",
            "COPY",
        ] {
            assert!(create(name).is_some(), "{name} が起動できない");
        }
    }

    #[test]
    fn case_is_ignored() {
        assert!(create("  line  ").is_some());
        assert!(create("LiNe").is_some());
    }

    #[test]
    fn unknown_command_is_rejected() {
        assert!(create("NOPE").is_none());
        assert!(create("").is_none());
    }

    #[test]
    fn undo_and_redo_are_immediate() {
        assert_eq!(immediate("U"), Some(Immediate::Undo));
        assert_eq!(immediate("undo"), Some(Immediate::Undo));
        assert_eq!(immediate("REDO"), Some(Immediate::Redo));
        assert_eq!(immediate("LA"), Some(Immediate::LayerPanel));
        assert_eq!(immediate("save"), Some(Immediate::File(FileAction::Save)));
        assert_eq!(immediate("OPEN"), Some(Immediate::File(FileAction::Open)));
        assert_eq!(immediate("LINE"), None);
    }

    /// UNDO は即時コマンド、ツールとしては存在しないこと。
    #[test]
    fn undo_is_not_a_tool() {
        assert!(create("UNDO").is_none());
        assert!(create("REDO").is_none());
    }

    #[test]
    fn editing_tools_want_selection() {
        for name in ["ERASE", "MOVE", "COPY"] {
            assert!(
                create(name).unwrap().wants_selection(),
                "{name} は選択を必要とするはず"
            );
        }
        for name in ["LINE", "CIRCLE", "ARC"] {
            assert!(
                !create(name).unwrap().wants_selection(),
                "{name} は選択を必要としないはず"
            );
        }
    }

    /// 表のデータ化で候補が引けるようになったこと。
    #[test]
    fn suggestions_match_prefix_of_name() {
        let names: Vec<_> = suggestions("LI").iter().map(|c| c.name).collect();
        assert_eq!(names, vec!["LINE"]);

        let names: Vec<_> = suggestions("RE").iter().map(|c| c.name).collect();
        assert!(names.contains(&"RECTANGLE"));
        assert!(names.contains(&"REDO"));
    }

    /// エイリアスにも先頭一致すること。
    #[test]
    fn suggestions_match_prefix_of_alias() {
        // "PL" は POLYLINE のエイリアス完全一致。
        let names: Vec<_> = suggestions("PL").iter().map(|c| c.name).collect();
        assert_eq!(names, vec!["POLYLINE"]);
    }

    /// エイリアス完全一致が先頭に来ること。"C" は CIRCLE のエイリアスであり、
    /// COPY の名前の先頭でもあるので、CIRCLE が先に来てほしい。
    #[test]
    fn exact_alias_match_is_ranked_first() {
        let names: Vec<_> = suggestions("C").iter().map(|c| c.name).collect();
        assert_eq!(names.first(), Some(&"CIRCLE"), "実際の順: {names:?}");
        assert!(names.contains(&"COPY"), "COPY も候補に出るはず: {names:?}");
    }

    #[test]
    fn suggestions_ignore_case_and_whitespace() {
        let lower: Vec<_> = suggestions("li").iter().map(|c| c.name).collect();
        let padded: Vec<_> = suggestions("  LI  ").iter().map(|c| c.name).collect();
        assert_eq!(lower, vec!["LINE"]);
        assert_eq!(padded, vec!["LINE"]);
    }

    #[test]
    fn empty_prefix_yields_no_suggestions() {
        assert!(suggestions("").is_empty());
        assert!(suggestions("   ").is_empty());
    }

    #[test]
    fn unknown_prefix_yields_no_suggestions() {
        assert!(suggestions("XYZZY").is_empty());
    }

    /// 候補は上限で打ち切られること。
    #[test]
    fn suggestions_are_capped() {
        // 全コマンドに先頭一致する接頭辞は無いので、上限そのものを検査する。
        let all = suggestions("S");
        assert!(all.len() <= MAX_SUGGESTIONS);
        assert!(
            COMMANDS.len() > MAX_SUGGESTIONS,
            "上限の検査に意味がある前提"
        );
    }

    /// 表のどのコマンドも、自分の名前で引ける。
    #[test]
    fn every_command_resolves_by_its_own_name() {
        for spec in COMMANDS {
            let found = lookup(spec.name).unwrap_or_else(|| panic!("{} が引けない", spec.name));
            assert_eq!(found.name, spec.name);
            for alias in spec.aliases {
                let found = lookup(alias).unwrap_or_else(|| panic!("{alias} が引けない"));
                assert_eq!(found.name, spec.name, "エイリアス {alias}");
            }
        }
    }

    /// 名前とエイリアスが表全体で重複していないこと。
    /// 重複すると先に書いた方が勝ち、後のコマンドが起動できなくなる。
    #[test]
    fn command_names_and_aliases_are_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for spec in COMMANDS {
            assert!(seen.insert(spec.name), "名前が重複: {}", spec.name);
            for alias in spec.aliases {
                assert!(seen.insert(alias), "エイリアスが重複: {alias}");
            }
        }
    }

    /// すべてのコマンドに説明文があること（候補一覧に出るため）。
    #[test]
    fn every_command_has_a_summary() {
        for spec in COMMANDS {
            assert!(!spec.summary.is_empty(), "{} に説明が無い", spec.name);
        }
    }

    #[test]
    fn stretch_is_registered_with_alias_s() {
        let spec = lookup("S").expect("S が引けない");
        assert_eq!(spec.name, "STRETCH");
        assert_eq!(create("STRETCH").unwrap().name(), "STRETCH");
        assert!(create("S").unwrap().wants_selection());
    }
}
