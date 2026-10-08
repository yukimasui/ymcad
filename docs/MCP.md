# MCP サーバー（`ymcad-mcp`）

LLM（Claude Code などの MCP クライアント）が、ymcad の図面ファイル（`.ymc` / `.dxf`）を開いて調べ、
保存できるようにするサーバーです。標準入出力で JSON-RPC を話します（1 行 1 メッセージ）。
GUI のアプリ（`cad-app`）には触りません。

> **段階 1c（描画）までの内容です。** 作図・変更（1b）とコンポーネントの操作（1d）は
> 別の段階で足します（Issue #70）。
> 設計判断は `docs/DECISIONS.md` の ADR-0046。

## ビルド

```bash
cargo build --release -p cad-mcp
# → target/release/ymcad-mcp
```

依存は `cad-core`・`serde_json`・`resvg`（PNG にするため。`default-features` なしで、文字・画像の読み込みは入れません）です。
`cad-core` の依存はゼロのままで、GUI のクレートは入りません。

## Claude Code への登録

```bash
claude mcp add ymcad -- /絶対パス/ymcad/target/release/ymcad-mcp --root /絶対パス/図面のディレクトリ
```

- `--root` は読み書きを許すディレクトリです（複数指定できます）。**省略するとサーバーを起動した
  カレントディレクトリ**になり、そのことを stderr に出します。図面を置くディレクトリを明示するのを勧めます
- 登録先の範囲は `claude mcp add` の `-s local|project|user` で選べます（既定は `local`）。
  リポジトリに `.mcp.json` はコミットしていません（パスが人ごとに違うため）
- 登録できたかは `claude mcp list` で確かめられます

一時的に試すだけなら、登録せずに設定ファイルを渡せます。

```bash
cat > /tmp/ymcad-mcp.json <<'EOF'
{"mcpServers": {"ymcad": {"type": "stdio",
  "command": "/絶対パス/ymcad/target/release/ymcad-mcp",
  "args": ["--root", "/絶対パス/図面のディレクトリ"]}}}
EOF
claude --mcp-config /tmp/ymcad-mcp.json
```

### 動作確認（2026-10-08）

Claude Code 2.1.294 から `--mcp-config` でつなぎ、`sample.ymc`（`write_sample` の出力）を開いて図形の内訳と
円弧の角度（度）を答えさせ、`copy.ymc` へ保存させた。保存したファイルは `validate_ymc.py` に通り、元とバイト単位で一致した。
通信の記録では、Claude Code はまず `server/discover`（仕様 2026-07-28 の新しい形）を送り、`-32601` を受けて
`initialize`（版 2025-11-25）へ戻っていた（下の「版」）。

## 約束ごと

| 項目 | 約束 |
|---|---|
| 角度 | 入出力とも**度**（内部はラジアン。円弧は開始角から終了角へ反時計回り） |
| 座標 | `f64` の数値。点は `{"x": 1.0, "y": 2.0}`（入力は `[1.0, 2.0]` も可） |
| 境界ボックス | `{"min": 点, "max": 点}`。図形が無い・無限（作図線）は `null` |
| 図形の種類 | `line` / `circle` / `arc` / `xline` / `polyline` / `instance` |
| 図形 ID | `d<図面の通し番号>e<番号>g<世代>`（例 `d2e0g0`）。**新規・開くたびに図面の通し番号が進み、前の図面の ID は拒まれます** |
| ファイル形式 | **拡張子だけで決まります。** `.ymc` が保存形式（無損失）、`.dxf` は交換用（R12・非可逆。保存すると警告が返ります）。拡張子が無ければ `.ymc` を付けます |
| 失敗 | 道具の失敗は `isError: true` と日本語の説明。LLM が読んで引数を直せるように書いてあります |
| 結果 | 成功は `structuredContent`（オブジェクト）と、同じ JSON の text の両方で返します |
| 図面の変更 | 1b 以降の図面を変える道具は、1 回の呼び出しで Undo 1 回ぶんにします（`undo` の 1 回 = 呼び出し 1 回） |

## 道具（段階 1a）

| 道具 | 内容 | 図面・ファイルを |
|---|---|---|
| `new_drawing{discard_changes?}` | 空の新規図面にする | 図面を捨てる |
| `open_drawing{path, discard_changes?}` | `.ymc` / `.dxf` を開く | 図面を捨てる |
| `save_drawing{path?, overwrite?}` | 保存する。`path` を省くと開いた（前に保存した）ファイルへ | **ファイルを書く** |
| `drawing_info` | 図面名・パス・形式・未保存か・数・範囲・取り消せるか・ファイルが他で書き換えられたか・root | 読むだけ |
| `list_entities{layer?, type?, bbox?, limit?, offset?}` | 図形の要約（id・type・layer・bbox）の一覧。既定 100 件・上限 1000 件。続きは `next_offset` | 読むだけ |
| `get_entities{ids}` | 図形の全体（レイヤ・色・グループ・形）。1 つでも使えない ID があれば全体を拒む | 読むだけ |
| `list_layers` | レイヤ（名前・色・表示・ロック・線種・現在か・図形の数） | 読むだけ |
| `list_components` | コンポーネント定義（パラメータ・束縛の式・インスタンスの数） | 読むだけ |
| `undo{steps?}` / `redo{steps?}` | 取り消し・やり直し（既定 1 回、上限 256 回。尽きたら止まる） | 図面を変える |
| `render{format?, width?, height?, region?, background?}` | 図面を PNG / SVG にして返す（下の「描画」） | 読むだけ |

各道具の引数の詳しい説明は `tools/list` の `description` / `inputSchema` にあります（日本語）。
**知らない引数を渡すと、動かす前に拒みます**（綴り違いのフラグを黙って無視しないため）。

## 描画（`render`）

図面を画像にして返します。LLM が自分の描いた図を目で確かめるための道具です。

| 引数 | 内容 |
|---|---|
| `format` | `png`（既定。`image` ブロック）/ `svg`（`text` ブロック）/ `both`（image と text） |
| `width` / `height` | 画像の大きさ px。既定 1024 × 768、**一辺 16〜4096**（超えると拒みます） |
| `region` | 見せるモデルの範囲 `{"min": [x, y], "max": [x, y]}`。幅と高さは 0 より大きく。省くと、表示中の図形（インスタンスの中身を含む。作図線は除く）の範囲に 5% の余白を付けた範囲 |
| `background` | `dark`（既定。アプリと同じ暗い背景 `#0a0a0a`）/ `light`（白。このとき白（ACI 7）の線は黒で描く） |

- 結果の先頭の text（= `structuredContent`）に **`view`**（画像に見えているモデルの範囲）と
  **`pixels_per_unit`**（1 モデル単位あたりの px）が入ります。縦横比は保つので、`region` と画像の縦横比が違えば
  `view` は `region` より広くなります（`region` は中央に収まる）。画像の位置 (px, py)（左上が原点、下向きが +）の
  モデル座標は `x = view.min.x + px / pixels_per_unit`、`y = view.max.y - py / pixels_per_unit`（**モデルの Y は上向き**）
- 描いた図形・範囲の外で省いた図形・非表示のレイヤで省いた図形の数が `entities_drawn` /
  `entities_outside_view` / `entities_on_hidden_layers` に入ります（画像が空のとき、範囲が違うのか非表示なのか分かるように）
- 色はレイヤ・図形の色（ACI）、破線は線種（`dash_pattern_px`）のとおり。**文字は描きません**。
  インスタンスは中身に展開し、インスタンス自身のレイヤ・色・線種で描きます（アプリと同じ）
- SVG の各要素の `data-id` は図形 ID です（SVG の本文を読む LLM が、どの要素がどの図形か辿れます）。
  座標は画像の px そのもので、PNG は SVG をそのままラスタにしたものです
- SVG は 8 MiB まで。超えたら `region` で絞るよう促して拒みます
- 作図線と、範囲を横切る線分・ポリラインは範囲で切って描きます。半径が 1e5 px を超える円・円弧（大きな円の一部だけを
  拡大して見るとき）は、見える角度の窓だけを折れ線にして描きます

## 安全策

- **読み書きできるのは `--root` の配下だけ。** 相対パスは最初の root から解決します
  - `..` を含むパスは、root の配下を指していても拒みます
  - 読み込みも書き込みも実体のパスを求めて（`canonicalize`）、root の配下か確かめます。
    途中のディレクトリが root の外を指すシンボリックリンクなら拒みます
  - **既存のファイルがシンボリックリンクなら、指す先が配下でも拒みます**
  - 保存先のディレクトリは作りません
- **拡張子は `.ymc` / `.dxf` だけ**（大文字小文字は問わない）
- **ディスクに書くのは `save_drawing` だけ。** 書き込みはアトミック（一時ファイル + rename。失敗してもファイルは元のまま）
- 人に確かめる代わりに、**明示のフラグ**を付けて呼び直させます
  - 未保存の変更がある図面を `new_drawing` / `open_drawing` で捨てる → `discard_changes: true`
  - 開いている図面のファイル以外の既存ファイルへ保存する → `overwrite: true`
  - 開いた後に他のプログラムが書き換えたファイル（更新時刻か大きさが違う）へ保存する → `overwrite: true`
- 上限: 1 行 4 MiB（超えた行は読み捨てて `-32700`）、開くファイル 64 MiB、一覧 1000 件、ID 1000 個、undo / redo 256 回、
  画像の一辺 4096 px（最大でも 4096 × 4096 × 4 = 64 MiB の画素）、SVG 8 MiB
- ネットワークもシェルも使いません
- 守れないもの: 検査と読み書きの間に他のプロセスがファイルを差し替える競合。同じ大きさで更新時刻の分解能の内に
  書き換えられたファイル（ADR-0046）

## プロトコル

- 標準入出力。1 行 1 メッセージ。**stdout にはプロトコルのメッセージだけ**を書き、案内やログは stderr。
  stdin が尽きたら正常終了します
- 扱うメソッド: `initialize` / `notifications/initialized` / `ping` / `tools/list` / `tools/call`
- **版**: `initialize` で取り決める形（2025-11-25 / 2025-06-18 / 2025-03-26 / 2024-11-05）。要求された版が一覧に
  あればそれを、無ければ 2025-11-25 を返します。仕様の最新 2026-07-28（`initialize` を廃し要求ごとに版を載せる形）には
  まだ対応していません。`server/discover` には `-32601` を返すので、両方に対応するクライアントは `initialize` へ戻ります
- `initialize` の前の `tools/*` は `-32600`。通知には返事をしません。一括要求（配列）は扱いません（`-32600`）
- エラーコード: `-32700`（JSON として読めない・長すぎる・UTF-8 でない）/ `-32600`（形の違う要求）/
  `-32601`（知らないメソッド）/ `-32602`（`params` の形・知らない道具）

## 手で動かす

```bash
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{}}}' \
  '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"drawing_info","arguments":{}}}' \
  | target/release/ymcad-mcp --root /tmp
```

## 検査

```bash
cargo test -p cad-mcp        # 単体（Server::handle を直接）と結合（バイナリを起動）

# Python の標準ライブラリだけのクライアントで通しで動かし、保存したファイルを validate_ymc.py に通す（CI でも実行）。
# render の SVG（xml.etree で解析）と PNG（シグネチャ・IHDR・CRC・IDAT）もここで検査する
mkdir -p /tmp/mcp
cargo run -p cad-core --example write_sample -- /tmp/mcp/sample.ymc
cargo run -p cad-core --example write_sample -- /tmp/mcp/sample.dxf
python3 tools/mcp_smoke.py target/release/ymcad-mcp /tmp/mcp
```
