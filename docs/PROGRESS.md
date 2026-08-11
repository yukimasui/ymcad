# 作業状況

**このファイルは作業を中断・再開するための引き継ぎメモ。** 実装を進めたら都度更新すること。
設計判断そのものは `docs/DECISIONS.md`（ADR）に、モジュール構成は `docs/ARCHITECTURE.md` に書く。

最終更新: 2026-08-12 (Phase 3 完了)

---

## 現在地

| | |
|---|---|
| 完了 Phase | **Phase 0 / 1 / 2 / 3（作図コマンドとコマンドライン UI）** |
| 進行中 Phase | **Phase 4（オブジェクトスナップ）— 着手前** |
| 現在ブランチ | `develop` |
| GitHub push | `develop` は push 済み。**`main` へのマージ/PR は事前にユーザー確認が必要** |

### ブランチ運用

- `main` … 保護対象。直接コミットしない。全 Phase 完了後に `develop` からマージ（**要ユーザー確認**）
- `develop` … 各 Phase の統合先
- `feature/phase-N-<名前>` … 各 Phase の作業ブランチ。完了時に `--no-ff` で `develop` へマージ

---

## 自律作業モード（2026-08-11 〜）

ユーザー不在のため、**Phase 2 以降を承認待ちなしで進める**ことになった。
再開時にユーザーが Phase 単位でレビューできるよう、以下を守ること。

### ルール

1. **Phase ごとに必ずコミットを分ける。** Phase をまたぐコミットを作らない。
2. Phase 完了時に `feature/phase-N-<名前>` を `--no-ff` で `develop` へマージし、
   マージコミットのメッセージに受け入れ基準の充足状況を書く。
   → ユーザーは `git log --graph` と各マージコミットだけで Phase 単位の差分を追える。
3. **各 Phase の完了時にこの PROGRESS.md を更新する。**
   受け入れ基準のうち「ユーザー目視が必要な項目」は `[ ]` のまま残し、
   下の「### 目視確認が必要な項目」に積み上げていく。
4. 設計判断は都度 `docs/DECISIONS.md` に ADR として追記する。
5. **`main` へのマージ・PR は絶対に行わない。** 必ずユーザーの確認を待つ。
6. 受け入れ基準を満たせない Phase が出たら、そこで止めて理由を PROGRESS.md に記録する。
   満たさないまま次の Phase へ進まない。
7. 指示書の「必ず確認を取る」項目（トレランス方針の変更、エンティティモデルの構造変更、
   描画バックエンドの変更、非スコープ項目への着手）に該当したら、**勝手に決めずに止めて**
   選択肢とトレードオフを PROGRESS.md に書き、ユーザーの判断を待つ。

### 目視確認が必要な項目（再開時にユーザーへ依頼する）

- [ ] Phase 2: **60fps 維持**（ステータスバーの「描画 平均/最大 ms」が 16.6ms を大きく下回るか）
  - カーソル直下の固定は単体テストで検証済みだが、体感も見てほしい
- [ ] Phase 3: **コマンドラインの操作感**（キー入力が常にコマンドラインへ流れるか、
  Space/Enter 確定、空 Enter の再実行、ラバーバンドの見え方）
  - 窓選択（左→右、青の実線）/ 交差選択（右→左、緑の破線）の色と方向
  - IME で日本語変換中にコマンドが誤爆しないか（`[変換中]` 表示が出る）
- [ ] Phase 4: スナップマーカーがチラつかないこと、ヒステリシスの効き具合
- [ ] Phase 5: レイヤパネルの操作感
- [ ] Phase 6: 書き出した DXF が LibreCAD / QCAD で開けること（`sudo apt install librecad` が必要）

---

## Phase 進捗

- [x] **Phase 0** — IME 検証スパイク … 全 5 項目 PASS。`docs/DECISIONS.md` ADR-0001 参照
- [x] **Phase 1** — 基盤 … 受け入れ基準すべて充足。ユーザー目視確認済み
- [x] **Phase 2** — ビューポート操作 … 自動検証は充足。60fps のみユーザー目視待ち
- [x] **Phase 3** — 作図コマンドとコマンドライン UI … 自動検証は全項目充足。操作感のみユーザー目視待ち
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
- [x] アプリ起動でグリッドと原点マーカーが表示される（ユーザー目視で確認済み。座標表示の更新も確認）
- [x] `cad-core/Cargo.toml` に UI 系依存が無い（`cargo tree` で `cad-core` 単独を確認）

---

## Phase 2 の結果（完了）

ブランチ: `feature/phase-2-viewport`

### 実装
- [x] パン（中ボタンドラッグ）
- [x] ズーム（ホイール、**カーソル位置が中心**）。タッチパッドのピンチも同じ扱い
- [x] `Z` → `E`（ZOOM EXTENTS）/ `Z` → `A`（ZOOM ALL）。`Esc` で途中状態を破棄
- [x] ステータスバーのカーソル座標（小数点以下 4 桁）※ Phase 1 で実装済み
- [x] グリッドの 1/2/5 系列による自動段階変更 ※ Phase 1 で実装済み
- [x] 起動時に ZOOM ALL でフィット
- [x] 描画時間の実測をステータスバーに表示（60fps 基準の確認用）

### 受け入れ基準
- [x] ズーム倍率 1e-6〜1e6 で座標表示・描画が破綻しない
  - `viewport.rs: roundtrip_across_full_zoom_range`（変換の往復が 1px 以内）
  - `render.rs: grid_line_count_stays_sane_across_zoom_range`（線数が 200 本以下）
- [x] ホイールズーム時、カーソル直下のモデル座標が動かない（誤差 1px 以内）
  - `viewport.rs: zoom_about_keeps_anchor_fixed`（倍率 1e-6〜1e6 × 3 アンカー × 4 倍率）
- [ ] 60fps を維持する → **ユーザー目視待ち**。描画時間をステータスバーに実測表示している

### この Phase で判明したこと
- 倍率のドリフトは実測で問題にならなかった（20,000 回の操作で相対誤差 1e-9 未満）。
  整数ズームレベル方式は採用しない（ADR-0006）
- グリッド線の本数に上限を設けたので、描画コストがズーム倍率によらず一定になった（ADR-0007）

### 積み残し
- ZOOM ALL の図面限界は暫定で A3 横（420×297mm）の定数。
  Phase 6 で DXF の `$LIMMIN` / `$LIMMAX` と対応づけて `Document` へ移す
- `Z` → `E` / `Z` → `A` は暫定の状態機械。Phase 3 でコマンドラインへ統合する

---

## Phase 3 の結果（完了）

ブランチ: `feature/phase-3-commands`

### 実装
- [x] 常設のコマンドライン。**キー入力は常にここへ流れる**（入力欄のクリック不要）
- [x] `Enter` / `Space` で確定、空 `Enter` で直前コマンド再実行、`Esc` で中断・選択解除
- [x] 実行中コマンドのプロンプト表示、履歴 10 行表示
- [x] 座標直接入力 3 形式（`100,50` / `@100,50` / `@100<45`）+ 全角の正規化
- [x] 作図: LINE(`L`, `C` で閉じる) / CIRCLE(`C`, `D` 直径 / `2P` 2点) / ARC(`A`, 3点)
      / RECTANGLE(`REC`) / POLYLINE(`PL`)
- [x] 編集: ERASE(`E`) / MOVE(`M`) / COPY(`CO`, 複数回継続) / UNDO(`U`) / REDO
- [x] ZOOM(`Z`) をコマンドラインへ統合（Phase 2 の暫定キー処理は削除）
- [x] 選択: クリック / 窓選択（左→右、青） / 交差選択（右→左、緑の破線） / `Shift` で解除
- [x] 選択中エンティティのハイライト、確定前のラバーバンド表示

### 受け入れ基準
- [x] 全コマンドがエイリアスで起動する … `tools/mod.rs: aliases_resolve_to_expected_tools`
- [x] 空 Enter による直前コマンド再実行 … `session.rs: repeat_last_command_draws_again`
- [x] 全操作が Undo/Redo で正しく巻き戻る
      … `session.rs: every_draw_command_round_trips_through_undo_redo`、
      `erase_and_undo`、`move_and_undo_restores_position`、`copy_continues_for_multiple_copies`
- [x] 座標直接入力の 3 形式すべてが動作 … `session.rs: all_three_coordinate_forms_work`
- [x] 窓選択と交差選択が方向で正しく切り替わる … `selection.rs: window_requires_containment_crossing_does_not`
- [ ] 操作感 → **ユーザー目視待ち**

### この Phase の設計判断
- **ラバーバンドは `Document` に入れない**（ADR-0008）。`Tool::preview` が返す派生データ。
  途中の図形を図面に入れて後で消すと Undo 履歴が汚れ、`EditCtx` を唯一の変更経路にした
  意味が失われる
- **交差選択は矩形の 4 辺との交点で厳密に判定**（ADR-0009）。bbox の重なりだけだと
  円のように bbox に隙間のある図形で誤検出する
- **IME 変換中はコマンドラインを一切解釈しない**（ADR-0002 の実装）。`[変換中]` を画面表示

### 積み残し
- `RECTANGLE` の回転矩形、`POLYLINE` の円弧セグメントは未対応（指示書の範囲外）
- 絶対極座標 `100<45` は指示書の 3 形式に含まれないため未対応
- OSNAP が無いため、既存図形の端点に正確に吸着できない → Phase 4 で解決

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
