//! コマンドの説明（Issue #27）。候補で `Enter` に実行される行のそばに出すヒントの中身。
//!
//! # `CommandSpec` に書き足さない理由（ADR-0048）
//!
//! 候補の一覧に出す一行説明（`CommandSpec::summary`）とは別に、もう少し詳しい説明を
//! **コマンド名をキーにした別の表**に置く。コマンド表（`tools/mod.rs`）はリボン（#26）や
//! ほかの機能も触るので、説明の追加・推敲のたびに同じファイルで衝突しないようにする。
//! 表が離れていても漏れないように、全コマンドに説明があること・余分な名前が無いことを
//! テストで検査する。
//!
//! # 書き方
//!
//! - 1 行目は何をするか（`summary` より少し詳しく）
//! - 2 行目は操作の流れ（`→` でつなぐ）とオプションのキー。キーは先に書いて目で拾いやすくする
//! - 行は `\n` で分ける。1 行は全角 30 文字ぶんまで（半角は 0.5 と数える）。ヒントの枠の幅には
//!   上限（`cmdline/mod.rs` の `HINT_MAX_WIDTH`）があり、超えると `（Y / N）` のような
//!   切れ目で折り返されて読みにくい。テストで検査する

/// 1 項目 = (コマンド名, 説明)。並びは `COMMANDS` と同じにしてある（意味は無い）。
static HELP: &[(&str, &str)] = &[
    (
        "LINE",
        "連続した線分を引く\n始点 → 次の点 → …　Enter で終了 / C で閉じる",
    ),
    (
        "CIRCLE",
        "円を描く\n中心 → 半径　D で直径を指定 / 2P で直径の両端 2 点",
    ),
    ("ARC", "3 点を通る円弧を描く\n始点 → 通過点 → 終点"),
    (
        "RECTANGLE",
        "辺が水平・垂直の矩形を 1 本の閉じたポリラインで描く\n1 つ目の角 → 対角の角",
    ),
    (
        "POLYLINE",
        "つながった 1 本のポリラインを描く\n始点 → 次の点 → …　Enter で終了 / C で閉じる",
    ),
    (
        "XLINE",
        "両方向に無限に伸びる作図線を引く\n2 点 / H 水平 · V 垂直 · A 角度 · O オフセット",
    ),
    (
        "ERASE",
        "選んだ図形を消す\n図形を選ぶ → Enter（先に選んであればすぐ消える）",
    ),
    (
        "MOVE",
        "選んだ図形を移動する\n図形を選ぶ → Enter → 基点 → 目的点",
    ),
    (
        "COPY",
        "選んだ図形を複写する（何個でも続けて置ける）\n図形を選ぶ → Enter → 基点 → 目的点 → …　Enter で終了",
    ),
    (
        "STRETCH",
        "交差窓に入った点だけを動かして形を変える\n右→左ドラッグで囲む → Enter → 基点 → 目的点",
    ),
    (
        "ROTATE",
        "選んだ図形を回転する（角度は度）\n図形を選ぶ → Enter → 基点 → 角度　C で元を残す",
    ),
    (
        "SCALE",
        "選んだ図形を拡大縮小する\n図形を選ぶ → Enter → 基点 → 尺度　C で元を残す",
    ),
    (
        "MIRROR",
        "選んだ図形を対称軸で反転した複写を作る\n図形を選ぶ → Enter → 軸の 2 点 → 元を消すか Y / N",
    ),
    (
        "TRIM",
        "線分のクリックした部分を交点で切り取る（境界の指定は不要）\n切り取る部分をクリック → …　Enter で終了",
    ),
    (
        "EXTEND",
        "線分を、クリックした側の端から次の交点まで伸ばす\n伸ばす側の端をクリック → …　Enter で終了",
    ),
    (
        "FILLET",
        "2 本の線分の角を丸める（半径は次回も使われる）\nR で半径 → 1 本目 → 2 本目",
    ),
    (
        "CHAMFER",
        "2 本の線分の角を面取りする（距離は次回も使われる）\nD で距離 2 つ → 1 本目 → 2 本目",
    ),
    (
        "GROUP",
        "選んだ図形を 1 つのグループにまとめる\n図形を選ぶ → Enter → 名前（Enter で既定名）",
    ),
    (
        "UNGROUP",
        "選んだ図形が属するグループを解く（図形は残る）\n図形を選ぶ → Enter",
    ),
    (
        "EXPLODE",
        "ポリラインを線分に、コンポーネントを中身の図形に分解する\n図形を選ぶ → Enter",
    ),
    (
        "COMPONENT",
        "選んだ図形をコンポーネントにし、その場でインスタンスにする\n図形を選ぶ → Enter → 基点 → 名前（Enter で既定名）",
    ),
    (
        "INSERT",
        "コンポーネントを配置する\n名前 → 位置 → 回転角（Enter で 0）→ 倍率（Enter で 1）",
    ),
    (
        "REDEFINE",
        "コンポーネントの中身を差し替える（全インスタンスに反映）\n図形を選ぶ → Enter → 基点 → 差し替える名前",
    ),
    (
        "EDITCOMP",
        "コンポーネントをその場で、いつものコマンドで編集する\nインスタンスをクリック → 編集 → ENDCOMP で確定",
    ),
    (
        "ENDCOMP",
        "コンポーネントの編集を終え、定義へ書き戻す\nすぐ実行",
    ),
    (
        "COMPONENTS",
        "コンポーネントのパネルを開閉する（配置・パラメータの変更）\nすぐ実行",
    ),
    (
        "PARAM",
        "コンポーネントにパラメータを宣言する（型は既定値から決まる）\nコンポーネント名 → パラメータ名 → 既定値",
    ),
    (
        "BIND",
        "コンポーネントの座標をパラメータの式で動かす\n編集中は点をクリック → X / Y → 式（番号で選べる）",
    ),
    (
        "PSET",
        "インスタンスのパラメータの値を変える\nインスタンスをクリック → パラメータ名 → 値　R で既定に戻す",
    ),
    (
        "ZOOM",
        "表示する範囲を変える\nA 図面全体 / E 図形のある範囲",
    ),
    ("UNDO", "直前の操作を 1 つ取り消す（続けて押せばさらに戻る）\nすぐ実行　REDO でやり直す"),
    ("REDO", "UNDO で取り消した操作を 1 つやり直す\nすぐ実行"),
    ("LAYER", "レイヤパネルを開閉する（作成・表示・ロック・現在レイヤ）\nすぐ実行"),
    (
        "PROPERTIES",
        "プロパティパネルを開閉する（選んだ図形の値とレイヤ）\nすぐ実行　Ctrl+1 でも",
    ),
    (
        "SELECTALL",
        "選べる図形をすべて選ぶ（非表示・ロック中のレイヤは除く）\nすぐ実行　Ctrl+A でも",
    ),
    ("NEW", "新しい図面にする（未保存なら確かめる）\nすぐ実行　Ctrl+N でも"),
    ("OPEN", "図面ファイル（.ymc / .dxf）を開く（未保存なら確かめる）\nすぐ実行　Ctrl+O でも"),
    ("SAVE", "図面を上書き保存する\nすぐ実行　Ctrl+S でも"),
    (
        "SAVEAS",
        "名前を付けて保存する（拡張子で .ymc / .dxf を選ぶ）\nすぐ実行　Ctrl+Shift+S でも",
    ),
    ("QUIT", "ymcad を終了する（未保存なら確かめる）\nすぐ実行"),
];

/// コマンド名（正式名）に対応する説明。無ければ `None`。
#[must_use]
pub fn description(name: &str) -> Option<&'static str> {
    HELP.iter().find(|(n, _)| *n == name).map(|(_, d)| *d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::COMMANDS;

    /// 全コマンドに説明があること。コマンドを足して説明を書き忘れると落ちる。
    #[test]
    fn every_command_has_a_description() {
        for spec in COMMANDS {
            assert!(
                description(spec.name).is_some(),
                "{} の説明が無い（cmdline/command_help.rs の HELP に足す）",
                spec.name
            );
        }
    }

    /// 表の名前がすべて実在するコマンドで、重複が無いこと。
    /// 名前を打ち間違えると、その説明は黙って使われない。
    #[test]
    fn help_table_has_no_stray_or_duplicate_names() {
        let mut seen = std::collections::BTreeSet::new();
        for (name, _) in HELP {
            assert!(
                COMMANDS.iter().any(|c| c.name == *name),
                "{name} というコマンドは無い"
            );
            assert!(seen.insert(*name), "{name} が表で重複している");
        }
        assert_eq!(HELP.len(), COMMANDS.len());
    }

    /// 説明が空でなく、1 行目（何をするか）と 2 行目（流れ）の 2 行であること。
    /// 候補の一行説明をそのまま写しただけになっていないこと。
    #[test]
    fn descriptions_have_a_lead_and_a_flow_line() {
        for spec in COMMANDS {
            let text = description(spec.name).unwrap_or_default();
            let lines: Vec<&str> = text.lines().collect();
            assert_eq!(lines.len(), 2, "{}: 2 行で書く（{text:?}）", spec.name);
            assert!(
                lines.iter().all(|l| !l.trim().is_empty()),
                "{}: 空の行がある",
                spec.name
            );
            assert_ne!(lines[0], spec.summary, "{}: 一行説明と同じ", spec.name);
        }
    }

    /// 1 行が全角 30 文字ぶん（半角は 0.5）以内であること。ヒントの枠の最大幅で折り返さない目安。
    #[test]
    fn lines_are_short_enough_not_to_wrap() {
        const MAX_UNITS: f64 = 30.0;
        for (name, text) in HELP {
            for line in text.lines() {
                let units: f64 = line
                    .chars()
                    .map(|c| if c.is_ascii() { 0.5 } else { 1.0 })
                    .sum();
                assert!(
                    units <= MAX_UNITS,
                    "{name}: 長すぎる（{units} > {MAX_UNITS}）: {line}"
                );
            }
        }
    }

    #[test]
    fn unknown_names_have_no_description() {
        assert_eq!(description("NOSUCH"), None);
        assert_eq!(
            description("L"),
            None,
            "エイリアスでは引かない（正式名だけ）"
        );
    }
}
