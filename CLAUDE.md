# ymcad — 作業ルール

汎用的な進め方は `~/.claude/CLAUDE.md` にある。ここには **ymcad 固有のこと**だけを書く。

- ユーザーとのやり取りには猫のペルソナとして語尾に「にゃ」をつける等すること
- Gitで機能実装が問題無いことを確認できた時点で都度Commitして作業履歴を残すこと
- mainにPR等を行う際は必ず事前にユーザーの確認作業を待ってから行うこと
- ユーザーの不具合報告などがあれば修正してからPR等を進めること

---

## 死守する設計原則

**変更するとコストが激増する。壊さないこと。** 詳細は `docs/ARCHITECTURE.md`。

1. **座標はすべて `f64`。** `f32` になるのは `crates/cad-app/src/viewport.rs` で
   egui へ渡す直前だけ
2. **トレランスは `crates/cad-core/src/geom/tolerance.rs` に一元管理。**
   ソース中に `1e-9` 等を直書きしない（**テストコードも対象**）
3. **エンティティストアは世代つきアリーナ。** `Vec` の添字を ID にしない
4. **エンティティを変更できるのは `Command` だけ。**
   `EditCtx` が唯一の経路で、`pub(crate)` + private ZST `Seal` + `&mut EditCtx`
   （`&mut Document` ではない）の 3 層で型強制している。
   **`Document` に `entities_mut` や `DerefMut` を生やさない**
5. **model → screen 変換は `viewport.rs` の 1 箇所に集約。** 逆変換もペアで提供する
6. **ファイル形式は 2 つで役割が分かれている。** `.ymc`（ネイティブ・無損失）が
   保存形式、`.dxf`（R12・非可逆）は交換専用。**DXF を保存形式に戻さない。**
   形式は**拡張子だけ**で決める（`crates/cad-app/src/file_ops.rs` の `is_dxf`）
7. **`cad-core` の依存パッケージはゼロ。** 増やす前に必ずユーザーへ確認する
   （wasm ビューアの道を守るため。詳細は ADR-0026）

派生データ（ラバーバンド、空間インデックス、描画キャッシュ）は
**`Document` に入れない**。`cad-app` 側に持ち、`Document::revision()` をキーに再構築する。

## CI が機械検査している不変条件

`.github/workflows/ci.yml` の「アーキテクチャ不変条件」ジョブ。
**新しいコードもこれを破らないこと。**

| 検査 | 内容 |
|---|---|
| 依存方向 | `cad-core` が egui / eframe / winit / wgpu / rfd などに依存しないこと |
| f64 のみ | `crates/cad-core/src` に `f32` が出てこないこと |
| 縮小変換の局所化 | `as f32` が `crates/cad-app/src/viewport.rs` の外に出ないこと |
| トレランス | `1e-9` 等の直書きが `geom/tolerance.rs` の外に無いこと |

> grep の除外は「行頭が `//` の行」だけなので、**行末コメントに書いても引っかかる**。

## 検証コマンド

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --release

# 書き出したファイルを Rust とは別ロジックで検査する（CI でも自動実行される）
cargo run -p cad-core --example write_sample -- /tmp/sample.ymc
python3 tools/validate_ymc.py /tmp/sample.ymc --verbose

cargo run -p cad-core --example write_sample -- /tmp/sample.dxf
python3 tools/validate_dxf_r12.py /tmp/sample.dxf
```

ラウンドトリップテストは「自分で書いて自分で読む」ため、書き手と読み手が同じ誤解を
していれば往復が成立してしまい、**書き出しのバグを見逃す**。
`tools/` の検証スクリプトはその盲点を埋めるためにある。

**バイナリ形式（`.ymc`）ではこの盲点がテキストより深い。** DXF なら保存した
ファイルをエディタで開けば構造が目で見えるが、バイナリは開いても分からない。
`validate_ymc.py` でいちばん効く検査は「**末尾でぴったり尽きること**」。

## ブランチ運用

- `main` … 保護対象。直接コミットしない
- `develop` … 統合先
- `feature/...` / `fix/...` … 作業ブランチ。完了時に `--no-ff` で `develop` へマージ

マージコミットがレビューの単位になる。`git log --graph --merges` で
Phase / Issue ごとの区切りを追える状態を保つ。

## 環境と既知の落とし穴

`docs/PROGRESS.md` の「既知の落とし穴」を**着手前に読むこと。** 主なもの:

- **egui 0.36 は API が刷新されている。** `eframe::App` は `update()` ではなく
  **`ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame)`**。
  パネルは `egui::TopBottomPanel` ではなく **`egui::Panel::top(id)` / `Panel::bottom(id)`**、
  `show()` は `&Context` ではなく **`&mut Ui`** を取る。
  ネット上のサンプルや記憶はほぼ 0.3x 以前のもので、そのままでは通らない
- **`WINIT_UNIX_BACKEND` は winit 0.30 で廃止済み。**
  X11 で起動するには `WAYLAND_DISPLAY= cargo run`
- **egui 同梱フォントに CJK グリフは無い。** `jp_font.rs` の読み込みが無いと日本語が □ になる
- **`TextEdit` は未確定文字列をアプリの `&mut String` へ直接書き込む。**
  コマンドラインで変換中にバッファを解釈してはいけない（`docs/DECISIONS.md` ADR-0002）

## 非スコープ

`docs/ROADMAP.md` に列挙してある項目には着手しない
（3D / OFFSET / ハッチング / 寸法記入 / ブロック / 文字要素 / 印刷 / DWG /
Windows・macOS / Web・wasm）。

> TRIM / EXTEND / FILLET / CHAMFER は Issue #7 で**線分のみ**実装済み。
> ポリラインの部分トリムと、円・円弧を含む角は残課題。

**着手する必要が生じたら、勝手に始めずユーザーに確認する。**
同じく次の変更も事前確認が必要:

- トレランス方針の変更
- エンティティモデルの構造変更
- 描画バックエンドの変更（egui Painter → wgpu）
- **ファイル形式の変更**（`.ymc` の形式バージョンを上げる / DXF のバージョンを上げる）
- **`cad-core` への依存パッケージの追加**
