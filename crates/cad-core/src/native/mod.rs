//! ymcad のネイティブ図面形式（`.ymc`）。
//!
//! # なぜ DXF ではなくこの形式があるのか
//!
//! DXF は **データ交換のための**形式で、アプリが自分の図面を保持する器ではない。
//! AutoCAD 自身のネイティブ形式も DWG（バイナリ）で、DXF は交換用と分かれている。
//!
//! DXF R12 を保存形式として使っていたころ、保存して開き直すと図面が変わっていた。
//!
//! | 落ちていたもの | 原因 |
//! |---|---|
//! | `XLINE`（作図線） | R12 に `XLINE` が無く、長い `LINE` に化けた |
//! | グループ所属 | R12 に `GROUP` が無い |
//! | 線種 | `CONTINUOUS` 固定で書き出していた |
//! | レイヤ名 | R12 向けに大文字化・空白除去していた（**日本語名が壊れる**） |
//!
//! この形式は**無損失**。往復しても図面は完全に一致する。
//! DXF は交換専用（インポート / エクスポート）に降格した。
//!
//! # なぜデータベース（SQLite）ではないのか
//!
//! 検討して却下した。詳細は `docs/DECISIONS.md` の ADR-0026。要点は 3 つ。
//!
//! - 永続化すべき状態が小さくて単純すぎる。実質 3 種
//!   （エンティティ / レイヤ / グループ）で関係は各 1 本、しかも全部メモリに読む。
//!   これはデータベースではなく `Vec` のシリアライズ
//! - SQL クエリの利点がほぼ無い。空間索引はすでに `cad-app` 側の四分木にある（ADR-0011）
//! - `cad-core` の依存パッケージゼロと wasm ビューアの道を失う対価に見合わない
//!
//! # 形式
//!
//! リトルエンディアン固定幅。文字列は `u32` の長さ + UTF-8。
//! **サニタイズしない**（日本語のレイヤ名・グループ名がそのまま通る）。
//!
//! ```text
//! magic          8 bytes  "YMCAD\x1a\0\0"
//! format_version u32
//! layer_count    u32
//!   per layer:   name | color u8 | flags u8 | linetype u8
//! group_count    u32
//!   per group:   name
//! entity_count   u32
//!   per entity:  kind u8 | 幾何ペイロード
//!                | layer_index u32
//!                | color_tag u8   (0=ByLayer, 1=Aci[+u8])
//!                | group_tag u8   (0=None,    1=Some[+u32 group_index])
//!
//! --- 以下は形式 v2 以降 ---
//! definition_count u32
//!   per definition: name | origin(2×f64)
//!                 | entity_count u32 → エンティティと同じ形
//!                 --- 以下は形式 v3 以降 ---
//!                 | param_count u32   → 宣言（名前・型・範囲・既定値の式）
//!                 | binding_count u32 → 束縛（添字・スロット・式）
//! ```
//!
//! **式は木のまま前置記法で書く。** 文字列で持つと読み込みのたびに
//! 字句解析と型検査をやり直すことになり、`Document` に入っている式は
//! 常に妥当という約束（`crate::expr`）が保てない。
//!
//! **定義はエンティティより後に置く。** v1 のファイルは定義セクションが無いだけで
//! 前半がそのまま読めるので、後方互換の分岐が「最後まで読んだら終わり」で済む。
//!
//! 定義の中のエンティティも入れ子のインスタンスを持てるので、
//! **定義セクションは自分より後ろの定義を参照できる**（前方参照）。
//! 添字で参照しているだけなので読み込み順に依存しない。
//!
//! **レイヤとグループは ID 値ではなくファイル内の添字で参照する。**
//! 読み込み側が「ファイル添字 → 実際の ID」の対応表を作って解決するので、
//! 将来 ID の割り当て方が変わっても壊れない。
//!
//! **`EntityId` は書かない。** `entity/store.rs` のスロット割り当て方針
//! （スロットの回収はファイル読み込み時にのみ行う / 空きスロットは世代番号を持たない）に
//! 従い、読み込みでは順次挿入して ID を振り直す。Undo 履歴を永続化しないので、
//! 読み込み後に古い `EntityId` を参照する者はいない。意味を持つのは ID の値ではなく
//! **走査順（= 描画順）**で、これはファイル順に挿入すれば正確に保たれる。
//!
//! **角度はラジアンで書く。** DXF ライタは度へ変換していて、これは
//! `docs/PROGRESS.md` の「既知の落とし穴」に挙がっている π/180 ずれの温床。
//! ネイティブ形式では内部表現のまま書き、変換を 1 つ減らす。

pub mod read;
pub mod write;

/// ファイルの先頭に置く識別子。
///
/// `\x1a` は DOS の EOF 文字で、誤ってテキストとして表示したときに
/// そこで止まるようにする慣習。末尾の `\0\0` は 8 バイトに揃えるための詰め物。
pub(crate) const MAGIC: &[u8; 8] = b"YMCAD\x1a\0\0";

/// 現在書き出す形式のバージョン。
///
/// これより大きいバージョンのファイルは**読まずに断る**。
/// 中途半端に読んで壊れた図面を見せるより、開けないと言うほうがよい。
///
/// | 版 | 追加されたもの |
/// |---|---|
/// | 1 | 最初の形式（レイヤ・グループ・エンティティ） |
/// | 2 | コンポーネント定義とインスタンス |
/// | 3 | パラメータの宣言と式の束縛 |
///
/// **古い版は読めるまま保つ。** 追加は常に既存の並びの**後ろ**へ足す。
pub(crate) const FORMAT_VERSION: u32 = 3;

/// コンポーネント定義セクションが入った最初の版。
pub(crate) const VERSION_WITH_COMPONENTS: u32 = 2;

/// パラメータと束縛が入った最初の版。
pub(crate) const VERSION_WITH_PARAMS: u32 = 3;

/// 標準のファイル拡張子（ドットなし）。
pub const EXTENSION: &str = "ymc";

/// `Geometry` の変種を表すタグ。
///
/// **既存の値は絶対に変えない。** 変えると過去に保存したファイルが読めなくなる。
/// 新しい図形は新しい値を足す。
pub(crate) mod kind {
    /// [`crate::Geometry::Line`]。
    pub const LINE: u8 = 0;
    /// [`crate::Geometry::Circle`]。
    pub const CIRCLE: u8 = 1;
    /// [`crate::Geometry::Arc`]。
    pub const ARC: u8 = 2;
    /// [`crate::Geometry::Xline`]。
    pub const XLINE: u8 = 3;
    /// [`crate::Geometry::Polyline`]。
    pub const POLYLINE: u8 = 4;
    /// [`crate::Geometry::Instance`]（形式 v2 以降）。
    pub const INSTANCE: u8 = 5;
}

/// インスタンスの配置に付くフラグのビット位置。
pub(crate) mod placement_flags {
    /// 鏡像反転しているか。
    ///
    /// **2 次元の相似変換では反射を (基点・回転・正の倍率) で表せない**ので、
    /// 独立したフラグが必要（`component::Placement` のドキュメントを参照）。
    pub const FLIPPED: u8 = 1 << 0;
}

/// パラメータ値の型を表すタグ（形式 v2 以降）。
pub(crate) mod value_tag {
    /// 数値。`f64` が続く。
    pub const NUMBER: u8 = 0;
    /// 真偽。`u8` が続く。
    pub const BOOL: u8 = 1;
    /// 選択肢。文字列が続く。
    pub const CHOICE: u8 = 2;
}

/// 色の指定方法を表すタグ。
pub(crate) mod color_tag {
    /// レイヤの色に従う。
    pub const BY_LAYER: u8 = 0;
    /// ACI で個別指定。`u8` が続く。
    pub const ACI: u8 = 1;
}

/// `Option` を表すタグ（グループ所属に使う）。
pub(crate) mod option_tag {
    /// 値なし。
    pub const NONE: u8 = 0;
    /// 値あり。中身が続く。
    pub const SOME: u8 = 1;
}

/// レイヤの真偽属性を詰めるビット位置。
pub(crate) mod layer_flags {
    /// 表示するか。
    pub const VISIBLE: u8 = 1 << 0;
    /// ロックされているか。
    pub const LOCKED: u8 = 1 << 1;
}

/// パラメータの型を表すタグ（形式 v3 以降）。
pub(crate) mod param_type {
    /// 数値。
    pub const NUMBER: u8 = 0;
    /// 真偽。
    pub const BOOL: u8 = 1;
    /// 選択肢。候補の個数と文字列が続く。
    pub const CHOICE: u8 = 2;
}

/// 式の節点を表すタグ（形式 v3 以降）。
///
/// **前置記法で書く。** 子の個数はタグから決まるので、括弧や終端記号が要らない。
pub(crate) mod expr_tag {
    /// 定数。値のタグが続く。
    pub const LITERAL: u8 = 0;
    /// パラメータ参照。名前が続く。
    pub const VAR: u8 = 1;
    /// 単項演算。演算子と子 1 つ。
    pub const UNARY: u8 = 2;
    /// 二項演算。演算子と子 2 つ。
    pub const BINARY: u8 = 3;
    /// 条件式。子 3 つ。
    pub const IF: u8 = 4;
    /// 1 引数の関数。関数番号と子 1 つ。
    pub const CALL1: u8 = 5;
    /// 2 引数の関数。関数番号と子 2 つ。
    pub const CALL2: u8 = 6;
}

/// 束縛できるスカラーの位置を表すタグ（形式 v3 以降）。
///
/// **既存の値は変えない。** 変えると過去のファイルの束縛が別の座標を指す。
pub(crate) mod slot_tag {
    /// 線分の始点 X。
    pub const LINE_AX: u8 = 0;
    /// 線分の始点 Y。
    pub const LINE_AY: u8 = 1;
    /// 線分の終点 X。
    pub const LINE_BX: u8 = 2;
    /// 線分の終点 Y。
    pub const LINE_BY: u8 = 3;
    /// 円の中心 X。
    pub const CIRCLE_CX: u8 = 4;
    /// 円の中心 Y。
    pub const CIRCLE_CY: u8 = 5;
    /// 円の半径。
    pub const CIRCLE_R: u8 = 6;
    /// 円弧の中心 X。
    pub const ARC_CX: u8 = 7;
    /// 円弧の中心 Y。
    pub const ARC_CY: u8 = 8;
    /// 円弧の半径。
    pub const ARC_R: u8 = 9;
    /// 円弧の開始角。
    pub const ARC_START: u8 = 10;
    /// 円弧の終了角。
    pub const ARC_END: u8 = 11;
    /// 作図線の通過点 X。
    pub const XLINE_OX: u8 = 12;
    /// 作図線の通過点 Y。
    pub const XLINE_OY: u8 = 13;
    /// 作図線の角度。
    pub const XLINE_ANGLE: u8 = 14;
    /// ポリラインの頂点 X。頂点番号が続く。
    pub const POLYLINE_VX: u8 = 15;
    /// ポリラインの頂点 Y。頂点番号が続く。
    pub const POLYLINE_VY: u8 = 16;
    /// インスタンスの配置 X。
    pub const INSTANCE_X: u8 = 17;
    /// インスタンスの配置 Y。
    pub const INSTANCE_Y: u8 = 18;
    /// インスタンスの回転。
    pub const INSTANCE_ROTATION: u8 = 19;
    /// インスタンスの倍率。
    pub const INSTANCE_SCALE: u8 = 20;
}

/// 線種を表すタグ。
///
/// [`kind`] と同じく、**既存の値は変えない**。
pub(crate) mod linetype {
    /// 実線。
    pub const CONTINUOUS: u8 = 0;
    /// 破線。
    pub const DASHED: u8 = 1;
    /// 一点鎖線。
    pub const CENTER: u8 = 2;
    /// 隠線。
    pub const HIDDEN: u8 = 3;
}
