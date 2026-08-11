# 作業状況

**このファイルは作業を中断・再開するための引き継ぎメモ。** 実装を進めたら都度更新すること。
設計判断そのものは `docs/DECISIONS.md`（ADR）に、モジュール構成は `docs/ARCHITECTURE.md` に書く。

最終更新: 2026-08-11 (Phase 1 実装完了・目視確認待ち)

---

## 現在地

| | |
|---|---|
| 完了 Phase | **Phase 0（IME 検証スパイク）— 合格** |
| 進行中 Phase | **Phase 1（基盤）— 実装完了、ユーザー目視確認待ち** |
| 現在ブランチ | `feature/phase-1-foundation` |
| GitHub push | **未実施**（ローカルのみ。`main` への PR は事前にユーザー確認が必要） |

### ブランチ運用

- `main` … 保護対象。直接コミットしない。全 Phase 完了後に `develop` からマージ（**要ユーザー確認**）
- `develop` … 各 Phase の統合先
- `feature/phase-N-<名前>` … 各 Phase の作業ブランチ。完了時に `--no-ff` で `develop` へマージ

---

## Phase 進捗

- [x] **Phase 0** — IME 検証スパイク … 全 5 項目 PASS。`docs/DECISIONS.md` ADR-0001 参照
- [ ] **Phase 1** — 基盤（ワークスペース / ジオメトリ / アリーナ / Command / 空ビューポート）
- [ ] **Phase 2** — ビューポート操作（パン・ズーム・ZOOM E/A・グリッド）
- [ ] **Phase 3** — 作図コマンドとコマンドライン UI ← **本アプリの核心**
- [ ] **Phase 4** — オブジェクトスナップ（OSNAP）
- [ ] **Phase 5** — レイヤと画層プロパティ
- [ ] **Phase 6** — DXF 入出力とファイル操作

各 Phase の受け入れ基準は指示書のとおり。**基準を満たさないまま次へ進まない。**
各 Phase 完了時に「実装内容 / 受け入れ基準の充足 ✅❌ / 設計判断と理由 / 未解決課題 / 申し送り」を報告し、
**ユーザーの承認を待ってから次 Phase へ進む**（合意事項）。

---

## Phase 1 の作業リスト

着手したら `[ ]` → `[x]` を更新すること。ブランチ: `feature/phase-1-foundation`

### 1-A. ワークスペース土台
- [x] ルート `Cargo.toml`（`[workspace]`, `resolver = "3"`, `[workspace.dependencies]`）
- [~] `rust-toolchain.toml` は作らない方針にした（CI の `dtolnay/rust-toolchain@stable` と競合するため）。`rust-version = "1.85"` を `Cargo.toml` に記載
- [x] `crates/cad-core/Cargo.toml` … **UI 系依存を入れない**旨のコメントを冒頭に書く
- [x] `crates/cad-app/Cargo.toml`

### 1-B. `cad-core` ジオメトリ（UI 非依存・f64 のみ）
- [x] `geom/tolerance.rs` … `EPS_LEN` / `EPS_ANGLE` と比較ヘルパ
- [x] `geom/point.rs` … `Point2`, `Vec2`
- [x] `geom/line.rs` … `Line`
- [x] `geom/arc.rs` … `Arc`, `Circle`
- [x] `geom/aabb.rs` … `Aabb`（`EMPTY` を単位元にした union）
- [x] `geom/intersect.rs` … 線分×線分 / 線分×円 / 線分×円弧 / 円×円（Phase 4 で本格使用）
- [x] ユニットテスト **20 件以上**（トレランス境界値を必ず含む）

### 1-C. `cad-core` エンティティ / レイヤ / ドキュメント
- [x] `entity/id.rs` … `EntityId { index: u32, generation: u32 }`
- [x] `entity/store.rs` … generational arena。**`restore(id, e)` で ID を保存したまま復元できること**
- [x] `entity/kind.rs` … `Geometry` enum, `Entity`
- [x] `layer.rs` … `LayerId`, `Layer`, `LayerTable`, `ColorSpec`
- [x] `document.rs` … `Document`（フィールドは全て private）
- [x] `error.rs` … `CadError`

### 1-D. `cad-core` コマンド / Undo
- [x] `command/edit_ctx.rs` … `EditCtx`（**エンティティを変更できる唯一の経路**）
- [x] `command/mod.rs` … `trait Command { execute / undo / name }`
- [x] `command/stack.rs` … Undo/Redo スタック（深さ 256）
- [x] Undo/Redo のテスト（特に「削除 → Undo で `EntityId` が一致すること」）

### 1-E. `cad-app`
- [x] `jp_font.rs` … `spikes/ime-check/src/jp_font.rs` を移植（**必須**。無いと UI が豆腐になる）
- [x] `viewport.rs` … `Viewport`（model↔screen 変換。**f64→f32 narrowing はここだけ**）
- [x] `render.rs` … グリッド + 原点マーカー描画
- [x] `main.rs` / `app.rs` … eframe 起動

### 1-F. CI とドキュメント
- [x] `.github/workflows/ci.yml`（fmt / clippy -D warnings / test / build --release）
- [x] CI に `cad-core` の UI 依存検査を入れる（**fail-closed** な形。下記「落とし穴」参照）
- [x] `README.md`（概要・ビルド手順・Ubuntu 依存パッケージ・キーバインド表）
- [x] `docs/ARCHITECTURE.md`
- [x] `docs/ROADMAP.md`（非スコープ項目を将来候補として列挙）

### Phase 1 受け入れ基準
- [x] `cargo build --release` が警告なし
- [x] `cargo test` 通過（156 件。うち cad-core のジオメトリ関連は 106 件）
- [x] `cargo clippy --workspace --all-targets -- -D warnings` 通過
- [ ] アプリ起動でグリッドと原点マーカーが表示される（**ユーザー目視**）
- [x] `cad-core/Cargo.toml` に UI 系依存が無い（`cargo tree` で `cad-core` 単独を確認）

---

## 死守する設計原則（Phase 1 で組み込み、以降変更しない）

1. **座標は f64。** `f32` は描画直前の画面座標変換のみ。
2. **トレランスは `geom/tolerance.rs` に一元管理。** ソース中に裸の `1e-9` を書かない（grep で検査可能に保つ）。
3. **エンティティストアは generational arena。** `Vec` 添字を ID にしない。
4. **エンティティを変更する経路は `Command` 以外に作らない。** `EditCtx` で型強制する。
5. **model→screen 変換は 1 箇所（`viewport.rs`）に集約。** 逆変換も必ずペアで提供。

---

## 既知の落とし穴（調査済み・繰り返し踏まないこと）

- **egui 0.36 は API が刷新されている。** `eframe::App` のメソッドは `update(&ctx, &mut frame)` ではなく
  **`ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame)`**。パネルは `egui::TopBottomPanel` ではなく
  **`egui::Panel::top(id)` / `Panel::bottom(id)`**、`show()` は `&Context` ではなく **`&mut Ui`** を取る。
  ネット上のサンプルや LLM の記憶はほぼ 0.3x 以前のもので、そのままでは通らない。
- **`WINIT_UNIX_BACKEND` は winit 0.30 で廃止済み。** X11 で起動したいときは `WAYLAND_DISPLAY= cargo run`。
- **egui 同梱フォントに CJK グリフは無い。** `jp_font.rs` の移植を忘れると日本語が全部 □ になる。
- **`TextEdit` は未確定文字列をアプリの `&mut String` に直接書き込む。**
  Phase 3 のコマンドラインで変換中にバッファをパースしてはいけない（ADR-0002）。
- **CI の依存方向検査は fail-closed にすること。**
  `cargo tree -p cad-core -i egui` の「失敗を期待する」形は、ネットワークエラーやパッケージ名の typo でも
  非ゼロ終了するため通過してしまう（fail-open）。フラットな依存リストを出して禁止クレート名を grep する形にする。
- **`slotmap` には「指定した key で再挿入する API」が無い。**
  `remove` 後に再挿入すると別の key になるため、`ERASE` → `UNDO` で `EntityId` が変わり、
  Undo スタックに残る他コマンドの参照が壊れる。自前アリーナに `restore(id, e)` を持たせる理由。

---

## 申し送り・未解決

- **X11 での起動確認が未実施。** Phase 1 で `cad-app` が起動できたら `WAYLAND_DISPLAY= cargo run` を試し、
  Wayland との差異を `docs/DECISIONS.md` に記録する（指示書の要求事項）。
- **`spikes/ime-check/` は Phase 0 の役目を終えたら削除可。** 当面は再検証用に残す。
- **`libssl-dev` 未導入。** 現状の依存構成では不要の見込み。必要になったらユーザーに導入を依頼する。
- **DXF ビューア未導入。** Phase 6 の受け入れ基準（LibreCAD / QCAD で開けること）の検証時に
  `sudo apt install librecad` をユーザーへ依頼する。
- **候補ウィンドウは入力欄の左端にアンカーされる**（egui 0.36 の仕様）。実害は軽微なので受け入れ済み。
