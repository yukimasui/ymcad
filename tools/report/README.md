# 作業報告書

ユーザー向けの作業報告書（HTML）を作る仕組み。運用の決まりは `CLAUDE.md` の「作業報告書（HTML）」。

- 公開先: **https://claude.ai/artifact/NeHX7p6Hy6jRaUqFBoNXtt**（claude.ai の Artifact、非公開。持ち主だけが開ける）
- 本文: `content.html`（HTML の断片。ここを書き換える）
- 画像: `img/*.png`（**git には入れない**。`.gitignore` 済み。更新のたびに数百 KB〜1.5MB 増えるため）

## 更新のしかた

1. `content.html` を書き換える。先頭の「いまの状況」と「お願い」を最新にし、
   レビュアーの判断で決めたことは「最終ジャッジ用メモ」の表に足す
2. 新しいスクリーンショットを使うなら `cargo test -p cad-app -- --ignored ui_snapshot` で撮り、
   `target/ui-snapshots/<名前>.png` を `img/` へ**別の名前で**コピーする（同じ名前で上書きすると、
   前の版で同じ名前を指していた箇所の画像も変わる）
3. `python3 build.py --publish` で `report.publish.html`（画像を埋め込まない版）を作る
4. Artifact ツールで公開する
   - `url` に上の URL、`file_path` に `report.publish.html`
   - `files` に**新しく足した画像だけ**を `{"img/<名前>.png": "img/<名前>.png"}` の形で渡す
   - Artifact は渡さなかったファイルを残すので、前の画像は消えない
     （別のマシンで `img/` が空でも、本文だけ更新すれば前の画像はそのまま表示される）
   - 別の会話から更新するときは、先に `action: "read"` で読んでから公開する（読まずに公開すると断られる）

手元で開く・ファイルで渡す用には `python3 build.py`（画像を base64 で埋め込んだ 1 ファイルの
`report.html`。`img/` に画像が無いと、その画像は空になる）。
