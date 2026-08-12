# アーキテクチャ

## クレート構成

```
ymcad/
├── crates/
│   ├── cad-core/   ジオメトリ・エンティティ・コマンド・ファイル入出力（UI 非依存、f64 のみ）
│   └── cad-app/    egui アプリケーション（入力処理・描画）
└── spikes/
    └── ime-check/  Phase 0 の IME 検証用。ワークスペース外
```

依存の向きは **`cad-app` → `cad-core`** の一方向のみ。逆流は CI で検査している。

### なぜ 2 クレートに分けるか

`cad-core` を UI 非依存に保つことで、将来 wasm ビューアや CLI コンバータを
追加するときに図面モデルをそのまま再利用できる。本プロトタイプでは wasm ビルドは
非スコープだが、後から分離するのは高くつくので最初から分けている。

## `cad-core`

```
src/
├── lib.rs
├── error.rs          CadError
├── geom/
│   ├── tolerance.rs  EPS_LEN / EPS_ANGLE / EPS_REL と比較関数
│   ├── point.rs      Point2, Vec2
│   ├── line.rs       Line
│   ├── arc.rs        Arc, Circle
│   ├── aabb.rs       Aabb
│   └── intersect.rs  交点計算
├── entity/
│   ├── id.rs         EntityId { index, generation }
│   ├── kind.rs       Geometry, Entity
│   └── store.rs      世代つきアリーナ
├── layer.rs          LayerId, Layer, LayerTable, AciColor, ColorSpec
├── command/
│   ├── edit_ctx.rs   EditCtx — 変更できる唯一の経路
│   ├── mod.rs        trait Command, MacroCommand
│   ├── stack.rs      UndoStack
│   └── basic.rs      AddEntities, DeleteEntities
└── document.rs       Document
```

## `cad-app`

```
src/
├── main.rs      eframe 起動
├── app.rs       CadApp（eframe::App の実装）
├── viewport.rs  モデル空間 ↔ スクリーン空間の変換
├── render.rs    グリッド・原点マーカーの描画
└── jp_font.rs   日本語フォントの読み込み
```

---

## 死守する設計原則

以下は後から変更するとコストが激増するため、Phase 1 で組み込み済み。

### 1. 座標はすべて f64

図面座標・内部計算はすべて `f64`。`f32` になるのは
`viewport.rs` の `model_to_screen` / `model_to_screen_vec` / `model_len_to_px` で
egui へ渡す直前だけ。

`cad-core` に `f32` が無いこと、`cad-app` の `as f32` が `viewport.rs` に閉じていることを
CI で検査している。

### 2. トレランスは `geom/tolerance.rs` に一元管理

長さの比較は **絶対値の下限 + 相対誤差** のハイブリッド。

```text
|a - b| <= max(EPS_LEN, EPS_REL * max(|a|, |b|))
```

純粋な絶対値比較にしない理由は [ADR-0003](DECISIONS.md#adr-0003-トレランスは絶対値下限--相対誤差のハイブリッドにする) を参照。
ソース中にトレランスを直書きしていないことを CI で検査している。

### 3. エンティティストアは世代つきアリーナ

`EntityId { index: u32, generation: u32 }`。`Vec` の添字を ID にしない。

`slotmap` ではなく自前実装にした理由と、スロットを再利用しない方針については
[ADR-0004](DECISIONS.md#adr-0004-エンティティストアは-slotmap-ではなく自前のアリーナにする) を参照。

### 4. エンティティを変更できるのは Command だけ

**これが本プロジェクトの中核不変条件**で、規約ではなく型で強制している。

```
Document::apply / undo / redo
        ↓  EditCtx を組み立てて渡す
Command::execute / undo
        ↓  EditCtx のメソッド経由
EntityStore / LayerTable の pub(crate) な変更系メソッド
```

強制のしかたは 3 層:

1. `EntityStore` / `LayerTable` の変更系メソッドはすべて `pub(crate)`。
   `cad-app` は **別クレート**なので、Rust のモジュール可視性により名前を呼ぶことすらできない。
   lint ではなくコンパイルエラーで、`unsafe` 以外に抜け道が無い。
2. `EditCtx` はフィールドが private、`new` が `pub(crate)`、さらに private な ZST
   `Seal` を持つため構造体リテラルでも作れない。
3. `Command` は `&mut Document` ではなく `&mut EditCtx` を受け取る。
   コマンドから `Document::apply` を再帰的に呼べないので、Undo エントリの入れ子や
   再入が原理的に起こらない。複合操作は `MacroCommand` で表現する。

`Document` には `entities_mut` も `DerefMut` も **意図的に生やしていない**。
1 つでも抜け道を作った時点で Undo の正しさが失われる。

### 5. モデル空間 → スクリーン空間の変換は 1 箇所

`viewport.rs` の `Viewport` に集約し、順変換と逆変換を必ずペアで提供する。

アフィン行列ではなく **相似変換（モデル空間の中心 + 一様スケール）** で保持している。
理由は [ADR-0005](DECISIONS.md#adr-0005-ビューポート変換は相似変換で保持する) を参照。

---

## 派生キャッシュの扱い

`Document` は変更のたびに増える `revision: u64` を持つ。
Phase 4 で導入する空間インデックスや描画キャッシュは、**`Document` の中には置かず**
`cad-app` 側の派生キャッシュとして持ち、`revision` をキーに再構築する。

こうすることで:

- すべてのコマンドがインデックスの更新を意識しなくてよい（コマンドの実装量が半分になる）
- Undo / Redo でも `revision` が進むので、巻き戻しでキャッシュが自動的に無効化される

## 描画バックエンド

egui の `Painter` を使用。`wgpu` によるカスタムレンダラは、性能目標
（10,000 要素で 60fps）を満たせないことが実測で判明した場合にのみ検討する。
変更する場合は事前にユーザーの確認を取る。
