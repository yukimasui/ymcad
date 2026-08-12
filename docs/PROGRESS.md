# 作業状況

**このファイルは作業を中断・再開するための引き継ぎメモ。** 実装を進めたら都度更新すること。
設計判断そのものは `docs/DECISIONS.md`（ADR）に、モジュール構成は `docs/ARCHITECTURE.md` に書く。

最終更新: 2026-08-12 (Phase 6 完了 — 全 Phase 実装完了)

---

## 現在地

| | |
|---|---|
| 完了 Phase | **Phase 0 〜 6 すべて** |
| 進行中 Phase | **なし。ユーザーの目視確認と `main` へのマージ判断待ち** |
| 現在ブランチ | `develop` |
| GitHub push | `develop` は push 済み。**`main` へのマージ/PR は事前にユーザー確認が必要** |

### ブランチ運用

- `main` … 保護対象。直接コミットしない。全 Phase 完了後に `develop` からマージ（**要ユーザー確認**）
- `develop` … 各 Phase の統合先
- `feature/phase-N-<名前>` … 各 Phase の作業ブランチ。完了時に `--no-ff` で `develop` へマージ

---

## ユーザーへの引き継ぎ（2026-08-12 時点）

**全 Phase の実装が完了し、`develop` に push 済み。`main` は手つかず。**

### 次にやること

1. **目視確認** — 下の「目視確認が必要な項目」を上から順に。`cargo run --release` で起動する
2. **DXF の相互運用確認** — `sudo apt install librecad` してから `librecad docs/sample.dxf`。
   これだけは `sudo` が要るため自動で実施できなかった唯一の受け入れ基準
3. **`main` へのマージ判断** — 指示書の「main に PR 等を行う際は必ず事前にユーザーの確認」に従い、
   こちらからは実施していない

### 不具合が見つかったら

指示書の「ユーザーの不具合報告などがあれば修正してから PR 等を進める」に従い、
先に修正する。修正は該当 Phase の `feature/` ブランチではなく、
新しく `fix/` ブランチを切って `develop` へ入れるのがよい。

### 自動検証の状況

| 項目 | 結果 |
|---|---|
| `cargo fmt --all -- --check` | ✅ |
| `cargo clippy --workspace --all-targets -- -D warnings` | ✅ |
| `cargo test --workspace` | ✅ 454 件 |
| `cargo build --workspace --release` | ✅ 警告なし |
| `cad-core` が UI 非依存 | ✅ |
| `cad-core` が f64 のみ | ✅ |
| f64→f32 の縮小が `viewport.rs` のみ | ✅ |
| トレランスが `tolerance.rs` に一元管理 | ✅ |
| DXF R12 構造検査 | ✅ |

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
- [ ] Phase 4: **スナップマーカーがチラつかないこと**、ヒステリシスの効き具合
  - 端点付近でカーソルを小刻みに動かしてもマーカーが点滅しないか
  - マーカーの形で種類が判別できるか（ステータスバーに種別名も出る）
- [ ] Phase 5: **レイヤパネルの操作感**（`LAYER` または `LA` で開く）
  - 追加/削除/リネーム(名前をダブルクリック)/色/表示/ロック/線種の操作
  - 破線がズームしても同じピッチに見えるか
- [x] Phase 6: 書き出した DXF が LibreCAD で開けること … **2026-08-12 ユーザー確認済み**

---

## Phase 進捗

- [x] **Phase 0** — IME 検証スパイク … 全 5 項目 PASS。`docs/DECISIONS.md` ADR-0001 参照
- [x] **Phase 1** — 基盤 … 受け入れ基準すべて充足。ユーザー目視確認済み
- [x] **Phase 2** — ビューポート操作 … 自動検証は充足。60fps のみユーザー目視待ち
- [x] **Phase 3** — 作図コマンドとコマンドライン UI … 自動検証は全項目充足。操作感のみユーザー目視待ち
- [x] **Phase 4** — オブジェクトスナップ … 自動検証は全項目充足。チラつきのみユーザー目視待ち
- [x] **Phase 5** — レイヤと画層プロパティ … 受け入れ基準すべて自動検証で充足
- [x] **Phase 6** — DXF 入出力とファイル操作 … 受け入れ基準すべて充足

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

## Phase 4 の結果（完了）

ブランチ: `feature/phase-4-osnap`

### 実装
- [x] 6 種のスナップ（端点 / 中点 / 中心 / 交点 / 垂線 / 最近点）
- [x] 優先順位 端点 > 中点 > 中心 > 交点 > 垂線 > 最近点（同順位なら近い順）
- [x] AutoCAD 準拠のマーカー（四角 / 三角 / 円 / ✕ / 直角記号 / 砂時計）+ ツールチップ
- [x] **ヒステリシス**（取得 10px / 解放 16px、判定基準は吸着点）
- [x] `F3` で ON/OFF トグル、ステータスバーに現在の吸着種別を表示
- [x] 四分木による空間インデックス（`Document::revision()` をキーにした派生キャッシュ）

### 受け入れ基準
- [x] エンティティ 10,000 件でスナップ検出が 16ms 以内
      → **実測 336ns（release）/ 1.9µs（debug）**。`snap_detection_meets_frame_budget`
- [x] 交点スナップが線分同士・線分と円・円同士で正しく動作
      → `snap/detect.rs` の交点テスト群（線分×円弧も含む）
- [ ] マーカーがカーソル移動に対してチラつかない → **ユーザー目視待ち**
      （ヒステリシス自体は `snap.rs: holds_snap_between_acquire_and_release_radius` で検証済み）

### この Phase の設計判断
- **ヒステリシスの判定基準は吸着点**（ADR-0010）。カーソル基準だと吸着した瞬間に
  基準が動いてしまい履歴が効かない
- **空間インデックスは `Document` に入れず派生キャッシュにする**（ADR-0011）。
  全コマンドにインデックス更新と巻き戻しを実装させずに済み、Undo/Redo でも自動で無効化される
- 四分木の分割境界をまたぐ要素は親ノードに残す（子へ複製しない）標準的な方式

### 積み残し
- 四分木は毎回まるごと再構築している。10,000 件でミリ秒オーダーなので当面問題ないが、
  100,000 件規模では差分更新を検討する
- スナップ種別ごとの ON/OFF UI は未実装（`SnapState::set_mode` は用意済み）。Phase 5 で検討
- 円の四半円点（Quadrant）スナップは指示書の対象外のため未実装

---

## Phase 5 の結果（完了）

ブランチ: `feature/phase-5-layers`

### 実装
- [x] レイヤパネル（`LAYER` / `LA` で開閉）: 一覧・追加・削除・リネーム
- [x] レイヤごとの色（ACI 9 色のパレット）・表示/非表示・ロック
- [x] 現在レイヤの切り替え、ステータスバーへの現在画層表示
- [x] 選択中エンティティのレイヤ移動
- [x] 線種 4 種（`Continuous` / `Dashed` / `Center` / `Hidden`）の画面表示

### 受け入れ基準
- [x] 非表示レイヤのエンティティが描画・選択の**両方**から除外される
      → `layer_panel.rs: hidden_layer_is_excluded_from_both_render_and_selection`
      （スナップからも除外されることを `hidden_layer_produces_no_snap_candidates` で確認）
- [x] ロックレイヤのエンティティが選択・編集できない
      → `layer_panel.rs: locked_layer_is_visible_but_not_selectable`
- [x] レイヤ操作が Undo で巻き戻る → `layer_panel.rs: layer_property_changes_undo`
      および `command/layer_ops.rs` の全コマンドの execute→undo→redo テスト

### この Phase の設計判断
- **パネルは `Document` を変更せずコマンドを返す**（ADR-0012）。
  GUI から直接 `LayerTable` をいじる抜け道を作らない
- **破線パターンは画面 px で定義**（ADR-0013）。図面単位だと拡大時に実線に見えてしまう
- レイヤ 0 は削除・リネーム不可、現在レイヤは削除不可。リネームは `by_name` 索引も更新する

### 積み残し
- 線種は DXF へ書き出さない（指示書の「画面表示のみ」に従う）。
  Phase 6 では全エンティティを `CONTINUOUS` で書き出す
- ACI 色は 1〜9 のみ。10 以降は灰色にフォールバックする
- スナップ種別ごとの ON/OFF UI は未実装（`SnapState::set_mode` は用意済み）

---

## Phase 6 の結果（完了）

ブランチ: `feature/phase-6-dxf`

### 実装
- [x] DXF R12 (AC1009) の書き出し: `LINE` / `CIRCLE` / `ARC` / `POLYLINE`+`VERTEX`+`SEQEND`
- [x] DXF R12 の読み込み: 上記 + 他ソフト由来の `LWPOLYLINE`
- [x] `LAYER` テーブルの入出力（名前・色・表示/非表示・ロック・線種）
- [x] 新規 / 開く / 保存 / 名前を付けて保存、`Ctrl+N` `Ctrl+O` `Ctrl+S` `Ctrl+Shift+S`
- [x] `NEW` / `OPEN` / `SAVE` / `SAVEAS` / `QUIT` コマンド
- [x] ファイルダイアログ（`rfd`）
- [x] 未保存変更がある状態での終了確認（ウィンドウの ✕ も横取りする）

### 受け入れ基準
- [x] 自作 DXF の書き出し → 読み込みでエンティティが完全一致
      → `crates/cad-core/tests/dxf_roundtrip.rs`（34 件、`cargo test` に組み込み済み）
- [x] 座標精度が往復で 1e-9 以内
      → 小数 12 桁で書き出し、1e6 規模でも往復誤差 1e-13。`eq_len` / `eq_angle` で検証
- [x] 書き出した DXF が LibreCAD で開ける
      → **2026-08-12 にユーザーが LibreCAD で開けることを確認済み。**
      あわせて R12 構造を独立に検査する `tools/validate_dxf_r12.py` も通過している

### この Phase の設計判断
- **DXF は外部クレートを使わず自前実装**（ADR-0014）。`dxf` クレートは既定で
  新しいバージョンを書き出すため、指定漏れで R12 でないファイルが静かに出る事故が起きやすい
- **未保存確認は 3 択**（ADR-0015）。「破棄しますか？」だけでは作業を失うか操作を諦めるかの二択になる
- 角度のラジアン↔度変換は 2 箇所だけに閉じ込めた。`ARC` の 50/51 は度なので、
  ここを間違えると小さなテストでは気づけない π/180 のずれになる

### 積み残し
- 線種は DXF へ書き出さない（指示書の「画面表示のみ」に従う）
- 図面限界（`$LIMMIN` / `$LIMMAX`）はまだ `Document` に持たせておらず、
  ZOOM ALL は A3 横の定数を使っている
- ACI 色は 1〜9 のみ厳密

---

## Issue #7 の結果（追加コマンド 11 種・完了）

3 段階に分け、段階ごとに `develop` へマージして動作確認してもらった。

| 段階 | ブランチ | コマンド | 確認 |
|---|---|---|---|
| 1 | `feature/issue-7-transform-commands` | ROTATE / SCALE / MIRROR | ✅ |
| 2 | `feature/issue-7-xline-group` | XLINE / GROUP / UNGROUP / EXPLODE | ✅ |
| 3 | `feature/issue-7-trim-extend-fillet` | TRIM / EXTEND / FILLET / CHAMFER | 未 |

### 段階 3 で足した土台

計画の段階で「ツールを書く前に埋める必要がある穴」を 5 つ洗い出していた。結果は以下。

| 穴 | 埋め方 |
|---|---|
| コマンド実行中の図形ピック | `StepInput::Entity { id, at }` と `Tool::wants_entity()`。拾うのは `Session`（ADR-0024） |
| 交点が「点」しか返らない | `intersect::line_params_against` を追加。線分上のパラメータ `t ∈ [0,1]` を返す |
| 無限直線の交点が無い | `intersect::line_params_extended`。`t > 1` が終点の先、`t < 0` が始点の手前 |
| 接円弧の構成が無い | `geom/corner.rs` を新設。`fillet` / `chamfer` |
| 値の記憶場所が無い | `ToolSettings` を `Session` が持ち、`StepOutcome::Setting` で書き戻す（ADR-0025） |

段階 2 の XLINE オフセットで暫定に使っていた `nearest_line_at` は、
1 番目の穴が埋まったので削除し、`StepInput::Entity` に置き換えた。

### 踏んだ落とし穴

- **角の「残す側」の判定で頂点を選んでしまう。** 交点そのものが線分の端点でもある場合
  （`0,0`-`10,0` と `0,0`-`0,10` のような普通の角）、
  「クリック位置に近いほうの端点」を選ぶと交点自身が選ばれ、方向ベクトルが零になる。
  `away = (pick - apex).normalized()` を先に決め、**その向きで遠いほうの端点**を採る形に直した
- **`EXPLODE` のロールバック漏れ。** `?` による早期リターンだと、
  途中で失敗したとき既に分解済みの要素が図面に残る。`let ... else` で明示的に巻き戻す形に直した
- **`CreateGroup` の Redo で `GroupId` が変わる。** ADR-0004 と同じ問題。
  確保した ID を Undo でも捨てず `restore_group` で戻す（ADR-0022）
- いずれも**自分で書いたテストが先に見つけた**。UI からは再現しにくい経路だった

### 積み残し
- `ROTATE` / `SCALE` の `R`（参照）オプション。対話が 2 段増えるので基本形の確認を優先した
- ポリラインの部分トリム。`EXPLODE` で分解すれば代替できる
- 円・円弧を含む角の `FILLET` / `CHAMFER`

---

## Issue #11 の結果（ネイティブ形式 `.ymc`・完了）

ブランチ: `feature/native-file-format`

DXF R12 を保存形式として使っていたため、保存して開き直すと図面が変わっていた。
DXF は交換用の形式で、自分の図面を保持する器としては役割が違う。

### 実装

| 段階 | 内容 | コミット |
|---|---|---|
| 1 | アトミック保存（形式と無関係な単独のバグ修正） | `031f6d0` |
| 2 | ネイティブ形式 `.ymc` と外部検証スクリプト | `f2d59ac` |
| 3 | アプリ側の接続。DXF を交換専用に降格 | `6f9c1d9` |

無損失になったもの（DXF では落ちていた）:

- 作図線（`XLINE`）— DXF では長い `LINE` に化けていた
- グループ所属 — DXF では消えていた
- 線種 — DXF では `CONTINUOUS` 固定だった
- **日本語のレイヤ名・グループ名** — DXF では大文字化・空白除去で壊れていた
- 座標が `f64` のビット一致（テキストの桁数丸めが無い）

### 踏んだ落とし穴

- **`Xline::new` で復元すると方向ベクトルが 1 ULP ずれる。**
  中の `normalized()` が除算をやり直すため `0.7071067811865475` →
  `0.7071067811865476` に動く。**往復が非可逆になり形式の存在理由に反する。**
  値はそのまま使い「単位ベクトルであること」を検査する形にした
  （`Xline::new` は非ゼロなら何でも受け取って正規化するので、
  壊れたファイルに対する検査としてはこちらのほうが強い）
- **`Document::clear_history()` の呼び忘れ。** 忘れると読み込みに使ったコマンド
  （`AddLayer` 等）が Undo できてしまう。`dxf/read.rs` は `mark_saved` の
  直前で呼んでいるので、真似るときは 2 行セットで見ること
- **`std::env::set_current_dir` はプロセス全体に効く。**
  「裸のファイル名での保存」をテストしようとして使い、並列実行される他のテストを
  壊しかけた。純粋関数（`parent_dir`）の単体テストに置き換えた

### 設計判断（詳細は ADR-0026 / ADR-0027）

- **SQLite は却下。** 永続化すべき状態が実質 3 種で全部メモリに読むので、
  データベースではなく `Vec` のシリアライズ。空間索引はすでに四分木にあり
  SQL の利点がほぼ無い。C 25 万行と wasm ビューアの道を失う対価に見合わない
- **`EntityId` は永続化しない。** アリーナのスロット割り当て方針に反して
  穴が永久に残る。意味を持つのは ID の値ではなく走査順（= 描画順）
- **レイヤ・グループは ID ではなくファイル内の添字で参照する**
- **角度はラジアンで書く。** DXF の度変換（π/180 ずれの温床）を通さない
- **読み込みも `Document::apply` 経由**（ADR-0003 を読み込みでも例外にしない）
- **形式は拡張子だけで決める。** 隠れた状態を作らない

### 検証で強くしたこと

`tools/validate_ymc.py` を追加し、**両方の検証スクリプトを CI で自動実行**するようにした
（従来 `validate_dxf_r12.py` は手動でしか走っていなかった）。

いちばん効く検査は「**ファイル末尾でぴったり尽きること**」。
書き出し漏れ・過剰があれば余りバイトか不足として必ず露見する。

検証スクリプト自体が fail-open になっていないことも CI で確かめる
（壊したファイルで失敗すること + 正常なファイルで合格することの両方向）。

### 積み残し
- **自動保存とクラッシュ復帰は未実装。** アトミック保存が守るのは
  「保存済みのファイルが壊れないこと」だけで、**未保存の変更が失われることは防げない**。
  退避先（保存済み図面は隣に置く / 未保存図面はどこへ？）と間隔が設計の分岐なので、
  着手前にユーザーへ確認する
- 移行フレームワークは作っていない。形式 v2 が必要になったら、
  `native/read.rs` のバージョン検査を分岐に変える
- 差分保存は作っていない（速度が問題になってから）

### ユーザーの目視確認が必要な項目
- [ ] 作図線・グループ・日本語レイヤ名を含む図面を `.ymc` で保存 → 開き直して保たれるか
- [ ] 同じ図面を `.dxf` で保存 → 警告が出るか。開き直すと落ちるか（役割分担が見えるか）
- [ ] `.dxf` で開いたファイルの `Ctrl+S` が DXF のままか
- [ ] `python3 tools/validate_ymc.py` が保存したファイルを通すか

---

## Issue #13 の状況（コンポーネント・段階 1 完了）

ブランチ: `feature/component-instances`

ブロック機能を「パラメトリックなコンポーネント」として再定義した
（背景と却下した案は ADR-0028）。**3 段階のうち段階 1 が完了。**

| 段階 | 内容 | 状況 |
|---|---|---|
| 1 | 定義とインスタンス | ✅ 実装済み（ユーザー確認待ち） |
| 2 | 型付きパラメータと式 | ❌ 未着手 |
| 3 | インプレース編集とパラメータパネル | ❌ 未着手 |

### 段階 1 でできること

- `COMPONENT`（`B` / `BLOCK`）… 選択をコンポーネント化。
  **その場でインスタンスに置き換わる**（AutoCAD の `BLOCK` は選択を消す）
- `INSERT`（`I`）… 位置・回転（度）・一様倍率を指定して配置
- `REDEFINE`（`RD`）… 定義の中身を差し替え、**全インスタンスが追従**
- `EXPLODE`（`X`）… インスタンスを中身へ。**中身のレイヤに戻る**
- インスタンスに `MOVE` / `COPY` / `ROTATE` / `SCALE` / `MIRROR` が効く
- インスタンス内部の端点・中点・中心・交点にスナップできる
- 入れ子（循環はコマンドが拒否）
- `.ymc` 形式 v2 で往復。**v1 のファイルは読めるまま**
- DXF は `BLOCKS` + `INSERT`（パラメータは失われるので警告）

### 踏んだ落とし穴

- **鏡像が (基点・回転・正の倍率) では表現できない。**
  2 次元の相似変換は反射を独立した成分として持つ。`Placement::flipped` が必要。
  合成規則（回転 = 2θ − φ、反転を反転）は `Arc` の鏡像と同じ形（ADR-0020）
- **負の一様倍率は反射ではなく 180 度回転**（2 次元では行列式が +1）。
  反転フラグを立てると別の図形になる
- **反転は回転より先に適用する。** 反射と回転は交換しない。
  一様倍率はどちらとも交換するので順序を問わない
- **`clear_history()` の呼び忘れ**（Issue #11 と同じ）。
  `dxf/read.rs` は `mark_saved` の直前で呼んでいるので、真似るときは 2 行セットで見る
- **`git checkout <file>` で未コミットのテストを消した。**
  実装をわざと壊して戻すときは**バックアップから復元する**こと。
  `git checkout` は HEAD に戻すので、コミット後に書いた分が消える
- **選択が既にあると `SelectionReady` が即座に届く**（`Session::start_tool`）。
  `GroupTool` の既存テストを真似て `enter` を挟むと、それが次の段階へ流れ込む
- **Reject 後もツールは生きている。** テストで中断しないと次の入力が
  そのツールに食われ、別の結果になる

### テストの歯を確認した

実装をわざと壊して落ちることを 5 パターンで確認済み
（反転フラグを消す / 鏡像の回転規則を変える / 反転と回転の順序を入れ替える /
負の倍率を反転として扱う / 深さ上限を消す）。

**軸は「変換と解決が可換であること」。** 「インスタンスを変換してから解決」と
「解決してから各図形を変換」が一致すれば合成規則が正しい。
変換ごとに期待値を書くより 1 つの性質で全部押さえられる。

図形の比較は係数を直接見ず**弧上の点を標本**する。回転や鏡像は三角関数を通るので
1 ULP では収まらず、標本にすれば角度の比較が長さの比較に落ちて折り返しも吸収される。

### 積み残し
- **段階 2（パラメータと式）が本体。** ダイナミックブロックのアクションの
  置き換えが今回の主眼なので、ここまで行かないと「再定義」が完成しない
- 段階 3（インプレース編集・パラメータパネル）
- **束縛の添字問題**（ADR-0029 の代償）。定義の中身を編集して要素の順序が
  変わると束縛の指す先がずれる。段階 2 で `SetDefinitionContents` に
  検証・再マップの責務を持たせる必要がある
- `PURGE`（未使用の定義を消す）コマンドは `DeleteDefinition` があるが UI が無い
- コンポーネント一覧のパネルが無い（`INSERT` で名前を打つしかない）

### 🐛 未修正の不具合（次のセッションはここから）

**MOVE などで確定前のプレビュー（ラバーバンド）が表示されなくなった。**
2026-08-13 にユーザーから報告。段階 1 の実装で入った退行の可能性が高い
（`crates/cad-app/src/render.rs` の `draw_entities` / `draw_geometry` を触っている）。

調査の出発点:

- `render.rs::draw_preview` → `draw_geometry` の経路。
  `Geometry::Instance(_) => {}` の腕を足したときに何か壊していないか
- `Session::preview(cursor, doc)` が空を返していないか
  （`MoveTool::translated_selection` あたり）
- `app.rs` の描画順。`draw_entities` にキャッシュ更新（`resolved.refresh`）を
  足したので、`draw_preview` との前後関係が変わっていないか

**`develop` へのマージと PR はこの修正が済むまで進めない**
（`CLAUDE.md`「ユーザーの不具合報告があれば修正してから PR 等を進める」）。

### ユーザーの目視確認が必要な項目

段階 1 の機能は 2026-08-13 にユーザー確認済み（上記の不具合を除く）。

- [x] 選択 → `B` でその場でインスタンスに置き換わるか（図形が消えないか）
- [x] `I` で複数配置でき、回転（度）と倍率が効くか
- [x] **`RD` で定義を差し替えると全インスタンスが変わるか**
- [x] インスタンスに `MOVE` / `RO` / `SC` / `MI` が効くか（特に **`MI` の鏡像**）
- [x] インスタンス内部の端点・中点にスナップできるか
- [x] `X` で中身へ分解され、中身のレイヤに戻るか
- [x] `.ymc` で保存 → 開き直して定義・入れ子・反転が保たれるか
- [x] `.dxf` で保存 → `BLOCKS`/`INSERT` になり、パラメータ喪失の警告が出るか
- [x] `U` ですべて戻るか（`COMPONENT` は 1 回で戻ること）

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
- ~~**DXF ビューア未導入。**~~ → 解消。2026-08-12 に LibreCAD で開けることを確認済み。
- **候補ウィンドウは入力欄の左端にアンカーされる**（egui 0.36 の仕様）。実害は軽微なので受け入れ済み。
