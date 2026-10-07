# ymcad 委譲の共通ルール

このファイルはオーケストレーターが書いた。実装担当・コードレビュアー・操作レビュアーの全員が最初に読むこと。
オーケストレーターの運用は `CLAUDE.md` の「作業体制 — オーケストレーター運用」。

## リポジトリと作業ツリー
- 統合先は `develop`。`main` には触れない
- 作業ツリーはオーケストレーターが `tools/wt.sh` でリポジトリ直下の `.worktrees/<名前>` に作り、パスを指示に書く。**自分に割り当てられた作業ツリーだけで作業する**。メインの作業ツリー（リポジトリのルート）や他の `.worktrees/*` には触れない
  - シェルのカレントディレクトリはコマンドごとに戻ることがあるので、コマンドは毎回 `cd <割り当てのパス> && ...` の形で実行する。**割り当てのパスが見つからないときは、メインの作業ツリーで代わりに作業せず、止まって報告する**
  - cargo のビルド先は全作業ツリーで `<リポジトリ>/target` を共有している（`.worktrees/.cargo/config.toml`）。他のエージェントのビルド中は「Blocking waiting for file lock」で待たされるが正常。`CARGO_TARGET_DIR` を勝手に変えない（依存の再コンパイルで重くなる）。タイムアウトは長め（`timeout 1800` 程度）に取る
- 禁止: push、ブランチ切り替え、マージ、`git stash`（作業ツリー間で共有のため）、`git add -A`（未追跡の `.claude/` がある）、`git reset --hard`
- コミットは実装担当だけ。`git -c user.name=Claude -c user.email=noreply@anthropic.com commit ...`。Conventional Commits、日本語、本文は「なぜそうしたか・採らなかった選択肢」。1 コミット = 1 つの意味のある変更。末尾に次の 2 行（モデル名は自分のもの）:
  ```
  Co-Authored-By: Claude <モデル名> <noreply@anthropic.com>
  ```
  （オーケストレーターから別の帰属行を指示されたら、それに従う）

## 必ず読むもの
1. `CLAUDE.md` … 「死守する設計原則」「CI が機械検査している不変条件」「検証コマンド」「非スコープ」「事前確認が必要な変更」
2. `docs/PROGRESS.md` の「既知の落とし穴」（egui 0.36 の API 刷新、TextEdit・フォーカス・IME、egui_kittest の使い方、作図領域が縮む件）
3. `docs/DECISIONS.md` の関連 ADR（とくに ADR-0002 IME、ADR-0016 コマンド表、ADR-0024 ピック、ADR-0032 パネル、ADR-0034〜0036 動的入力・キーの持ち主・寸法入力）
4. 担当の GitHub Issue（`gh issue view <番号>`）

## 破ってはいけない制約（検査コマンドつき）
- `cad-core` に依存を足さない・UI 依存を入れない: `cargo tree -p cad-core --edges normal --prefix none | grep -E "egui|eframe|winit|wgpu|rfd"` が空
- 座標は f64。`as f32` は `crates/cad-app/src/viewport.rs` の外に書かない（`cad-app` は clippy の `cast_possible_truncation` / `cast_precision_loss` もエラー）
- トレランス（`1e-9` 等）を直書きしない（テストも）。`cad_core::geom::tolerance` を使う
- エンティティを変えるのは `Command` だけ（`EditCtx`）。`Document` に `entities_mut` などを生やさない
- 派生データ・UI 状態を `Document` に入れない
- **非スコープ（OFFSET・ハッチング・寸法記入・文字・外部参照・拘束ソルバ・印刷・DWG・3D 等）と、事前確認が必要な変更（新しい図形の種類などエンティティモデルの構造変更、ファイル形式の変更、トレランス方針、描画バックエンド）には着手しない。** 必要になったら手を止めて報告する
- 日本語入力（ADR-0002）: 変換中はバッファを解釈しない・キーを奪わない。入力欄の位置を動かす変更は、変換開始前後で入力欄の矩形が変わらないことをテストで守る

## 検証コマンド（実装担当は報告前に全部通す。レビュアーも自分で流す）
```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --release
cargo test -p cad-app -- --ignored ui_snapshot      # target/ui-snapshots/*.png
```
加えて `.github/workflows/ci.yml` の「アーキテクチャ不変条件」ジョブと同じ grep。

## テストの書き方
- 純粋なロジックは egui に依存しない関数にして単体テスト
- 操作の振る舞いは `crates/cad-app/src/app/behavior_tests.rs` の形（egui_kittest、GPU なし、`#[ignore]` なし）
- 見た目は `crates/cad-app/src/ui_snapshot.rs`（`#[ignore]`、PNG を撮るだけ）。**撮った PNG は自分で Read ツールで開いて確認する**
- 新しいテストは、修正前（またはわざと壊した状態）で落ちることを確かめる

## 実装担当の報告に含めるもの
コミット一覧 / 設計判断と、仕様から外れた点・迷った点 / 受け入れ基準ごとの ✅・❌（自動で確かめられないものは「未検証」）/ 新しいテストの名前と検査内容・わざと壊して落ちたもの / 検証コマンドの実結果（件数）/ PNG の絶対パスと目視結果 / egui で詰まった点

## レビュアー共通
- オーケストレーターが `tools/wt.sh review <PR 番号>` で作ったレビュー用の作業ツリー（`.worktrees/review-<ブランチ>`、detach）で見る。最新でなければ `git fetch origin && git checkout --detach origin/<ブランチ>`（この detach だけはブランチ切り替え禁止の例外）。**ファイルの修正・コミット・push はしない**
- 判定は **OK**（マージを止める問題が無い）か **NG**。指摘は「ファイル:行（または画面・操作）/ 問題 / 期待する修正」。ブロッキングと非ブロッキングを分ける
- **PR の CI の結果も確かめる**（`gh pr checks <番号>`）。手元は日本語フォントがあるが CI には無いなど、環境の違いで結果が変わるテストに注意する。CI が赤なら NG
- 判定と指摘を `gh pr comment <番号>` で PR に残す。本文冒頭に「コードレビュアー（Opus）による判定: OK/NG」または「操作レビュアー（Opus）による判定: OK/NG」
- 確かめるための一時的なテストやコードは、作業ツリー外（scratchpad）の作業コピーで行い、終わったら消す。**scratchpad は他のエージェントと共有なので、作業コピーのディレクトリ名には必ず PR 番号と役割を入れる**（例: `scratchpad/pr32-ux-wc`）。既存のディレクトリは使わない。**作業コピーのビルド先（`CARGO_TARGET_DIR`）は共有の `<リポジトリ>/target` と分ける**（共有すると作業ツリーで流したテストに一時テストが混ざり、件数が狂う）。依存の再コンパイルで重くなるので、作業コピーは本当に必要なときだけ作る。`git archive` で取り出したファイルは更新時刻が古いので、ビルド先を共有すると cargo が作り直さない点にも注意
