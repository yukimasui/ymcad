# ymcad

AutoCAD ライクな操作性を持つ **2D 専用** CAD アプリケーション。Rust + egui 製、Ubuntu ネイティブ。

> **状態: Phase 1（基盤）まで実装済み。** 作図コマンドは Phase 3 で実装します。
> 進捗と次にやることは [`docs/PROGRESS.md`](docs/PROGRESS.md) を参照してください。

## ビルドと実行

### Ubuntu の依存パッケージ

```bash
sudo apt install build-essential pkg-config libssl-dev \
  libgtk-3-dev libxkbcommon-dev libwayland-dev
```

日本語 UI のためのフォントも必要です（未導入だと文字が □ になります）。

```bash
sudo apt install fonts-noto-cjk
```

### ビルド

```bash
cargo build --release
cargo run --release
```

### X11 で起動する

Wayland で不調な場合は X11 (XWayland) にフォールバックできます。

```bash
WAYLAND_DISPLAY= cargo run --release
```

> `WINIT_UNIX_BACKEND` は winit 0.30 で廃止されています。winit は `WAYLAND_DISPLAY` が
> 空なら `DISPLAY` を見て X11 を選ぶため、上記のように環境変数を空にして起動します。

## 実装済みの機能

| 機能 | 状態 |
|---|---|
| ジオメトリ（点・ベクトル・線分・円・円弧・AABB・交点計算） | ✅ Phase 1 |
| 世代つきアリーナによるエンティティ管理 | ✅ Phase 1 |
| レイヤ（色・表示/非表示・ロック） | ✅ Phase 1（UI は Phase 5） |
| Command による Undo / Redo（履歴 256 段） | ✅ Phase 1 |
| グリッド表示（1/2/5 系列で自動段階変更）・原点マーカー | ✅ Phase 1 |
| カーソル座標のリアルタイム表示 | ✅ Phase 1 |
| パン / ズーム / ZOOM EXTENTS / ZOOM ALL | ⬜ Phase 2 |
| コマンドライン UI と作図コマンド | ⬜ Phase 3 |
| オブジェクトスナップ（OSNAP） | ⬜ Phase 4 |
| レイヤパネル・線種 | ⬜ Phase 5 |
| DXF R12 入出力・ファイル操作 | ⬜ Phase 6 |

## 実装予定のコマンド（Phase 3）

| コマンド | エイリアス | 内容 |
|---|---|---|
| LINE | `L` | 連続線分。Enter で終了、`C` でクローズ |
| CIRCLE | `C` | 中心 + 半径。オプション `D`（直径）、`2P`（2点） |
| ARC | `A` | 3 点指定 |
| RECTANGLE | `REC` | 対角 2 点 |
| POLYLINE | `PL` | 連結ポリライン |
| ERASE | `E` | 選択オブジェクト削除 |
| MOVE | `M` | 基点 + 目的点 |
| COPY | `CO` | 基点 + 目的点、複数回コピー継続 |
| UNDO | `U` | 元に戻す |
| REDO | — | やり直し |

## キーバインド

### 実装済み

| キー / 操作 | 動作 |
|---|---|
| （なし） | Phase 1 ではビュー表示のみ |

### 実装予定

| キー / 操作 | 動作 | Phase |
|---|---|---|
| 中ボタンドラッグ | パン | 2 |
| ホイール | カーソル位置を中心にズーム | 2 |
| `Z` → `E` | ZOOM EXTENTS（全要素にフィット） | 2 |
| `Z` → `A` | ZOOM ALL（図面範囲にフィット） | 2 |
| `Enter` / `Space` | コマンド確定（空なら直前のコマンドを再実行） | 3 |
| `Esc` | 実行中コマンドの中断、選択解除 | 3 |
| 左→右ドラッグ | 窓選択（完全内包） | 3 |
| 右→左ドラッグ | 交差選択（交差を含む） | 3 |
| `Shift` + クリック | 選択解除 | 3 |
| `F3` | オブジェクトスナップの ON/OFF | 4 |
| `Ctrl+N` / `Ctrl+O` / `Ctrl+S` | 新規 / 開く / 保存 | 6 |

### 座標の直接入力（Phase 3）

| 書式 | 意味 |
|---|---|
| `100,50` | 絶対座標 |
| `@100,50` | 直前の点からの相対座標 |
| `@100<45` | 直前の点からの相対極座標（長さ 100、角度 45°） |

## 構成

```
crates/
├── cad-core/   ジオメトリ・エンティティ・コマンド・DXF（UI 非依存）
└── cad-app/    egui アプリケーション（入力処理・描画）
```

`cad-core` は egui / eframe / winit に依存しません（将来 wasm ビューアを載せる余地を残すため）。
この不変条件は CI で機械的に検査しています。

設計の詳細は [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md)、
判断の経緯は [`docs/DECISIONS.md`](docs/DECISIONS.md) を参照してください。

## 開発

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## ライセンス

MIT
