# MCP サーバー（`ymcad-mcp`）

LLM（Claude Code などの MCP クライアント）が、ymcad の図面ファイル（`.ymc` / `.dxf`）を開いて調べ、
描いて変え、保存できるようにするサーバーです。標準入出力で JSON-RPC を話します（1 行 1 メッセージ）。
GUI のアプリ（`cad-app`）には触りません。

> **段階 1（1a 骨組み・1b 作図と変更・1c 描画・1d コンポーネント）の内容です**（Issue #70）。
> 起動中のアプリとつなぐ段階 2 は別の Issue で扱います。
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

段階 1b（2026-10-09）: 同じく Claude Code 2.1.294（haiku）から、レイヤ「外形」を作り、閉じたポリラインと
半径 `"25*1.5"`（式の文字列）の円を 1 回の `add_entities` で描き、円を `move_entities` の `copy: true` で複製して
`house.ymc` へ保存させた。保存したファイルは `validate_ymc.py --expect polyline=1,circle=2` に通った。

## 約束ごと

| 項目 | 約束 |
|---|---|
| 角度 | 入出力とも**度**（内部はラジアン。円弧は開始角から終了角へ反時計回り） |
| 座標 | `f64` の数値。点は `{"x": 1.0, "y": 2.0}`（入力は `[1.0, 2.0]` も可） |
| 数値の入力 | JSON の数値か**式の文字列**（`"100*2+5"`・`"sqrt(2)*50"`・`"cos(60)*10"`。式の中の角度は度。パラメータは使えない）。0 除算・負の平方根・NaN・無限大はエラー |
| 境界ボックス | `{"min": 点, "max": 点}`。図形が無い・無限（作図線）は `null` |
| 図形の種類 | `line` / `circle` / `arc` / `xline` / `polyline` / `instance` |
| 図形 ID | `d<起動の印>-<図面の通し番号>e<番号>g<世代>`（例 `d3fa9c1-2e0g0`）。**新規・開くたびに図面の通し番号が進み、前の図面の ID は拒まれます。** 起動の印（16 進 6 桁）はサーバーの起動ごとに変わるので、つなぎ直す前の ID も拒まれます |
| ファイル形式 | **拡張子だけで決まります。** `.ymc` が保存形式（無損失）、`.dxf` は交換用（R12・非可逆。保存すると警告が返ります）。拡張子が無ければ `.ymc` を付けます |
| 失敗 | 道具の失敗は `isError: true` と日本語の説明。LLM が読んで引数を直せるように書いてあります |
| 結果 | 成功は `structuredContent`（オブジェクト）と、同じ JSON の text の両方で返します |
| 図面の変更 | 図面を変える道具は、**1 回の呼び出しで Undo 1 回ぶん**（`undo` の 1 回 = 呼び出し 1 回）。**1 つでも不正な値があれば何も変えません**（一部だけ処理しない） |
| 編集できない図形 | **非表示・ロック中のレイヤの図形**を対象にすると、呼び出しごと拒みます（アプリでも選べない図形なので）。置く・移す先のレイヤも同じ。`update_layer` で表示する・ロックを外してから |
| 新しい図形の ID | `add_entities` は `ids`、複製（`copy` / `keep_original`）は `created` に、入力と同じ順で返します。**`undo` の後に `redo` すると、作り直された図形の ID は変わります**（取り消してやり直した図形は新しく作られるため。消した図形を `undo` で戻すと同じ ID です）。下の「redo で ID が変わることの影響」も読んでください |
| インスタンスの ID | `insert_component` の `id` と `define_component` の `instance` は、**`undo` → `redo` でも同じ ID** です（この 2 つの道具の中で ID を保っています） |

### redo で ID が変わることの影響（Issue #92）

cad-core の `AddEntities`（`add_entities` と複製）と `AddLayer`（`add_layer`）は、やり直しのたびに新しい ID を振ります
（アプリでも同じ。[Issue #92](https://github.com/yukimasui/ymcad/issues/92) で直す予定）。そのため、**作った図形・レイヤを
後の呼び出しが使っていると、`redo{steps: 2}` のようなまとめてのやり直しが壊れます**。

- **後の呼び出しのやり直しが失敗する**: `add_entities` → その ID で `move_entities` → `undo{steps:2}` → `redo{steps:2}` は、
  2 回目のやり直しが「エンティティが見つかりません」で失敗します。失敗した操作はやり直しの列から消え、もう戻せません
- **図形が黙って存在しないレイヤに付く**: `add_layer` → そのレイヤに `add_entities` → `undo{steps:2}` → `redo{steps:2}` は、
  失敗を返さずに図形の `layer` が `null` になります（どの道具からも変えられず、保存するとレイヤ 0 として書かれます）

避け方: 作ってすぐ後の操作を取り消したいときは、`undo` / `redo` をまたがず、消して作り直す（`delete_entities` →
`add_entities`）か、`undo` の後は `redo` せずに呼び直してください。`redo` した後は `list_entities` / `list_layers` で
ID とレイヤを取り直してください。`insert_component` / `define_component` で置いたインスタンスはこの影響を受けません。

## 道具

### ファイル・照会・履歴（段階 1a）

| 道具 | 内容 | 図面・ファイルを |
|---|---|---|
| `new_drawing{discard_changes?}` | 空の新規図面にする | 図面を捨てる |
| `open_drawing{path, discard_changes?}` | `.ymc` / `.dxf` を開く | 図面を捨てる |
| `save_drawing{path?, overwrite?}` | 保存する。`path` を省くと開いた（前に保存した）`.ymc` へ。**開いたのが `.dxf` なら `path` を省けない**（`.ymc` か、明示の `.dxf`） | **ファイルを書く** |
| `drawing_info` | 図面名・パス・形式・未保存か・数・範囲・取り消せるか・ファイルが他で書き換えられたか・root・`poisoned`（内部エラーで保存できない） | 読むだけ |
| `list_entities{layer?, type?, bbox?, limit?, offset?}` | 図形の要約（id・type・layer・bbox）の一覧。既定 100 件・上限 1000 件。続きは `next_offset` | 読むだけ |
| `get_entities{ids}` | 図形の全体（レイヤ・色・グループ・形）。1 つでも使えない ID があれば全体を拒む | 読むだけ |
| `list_layers` | レイヤ（名前・色・表示・ロック・線種・現在か・図形の数） | 読むだけ |
| `list_components{name?, contents?}` | コンポーネント定義（パラメータ・束縛の式・インスタンスの数）。束縛の対象 `field` は図形の JSON の項目名（`start.x`・`radius`・`vertices[3].y` など）。`contents: true` で定義の中の図形（`index` と `geometry`）も返す | 読むだけ |
| `undo{steps?}` / `redo{steps?}` | 取り消し・やり直し（既定 1 回、上限 256 回。尽きたら止まる） | 図面を変える |
| `render{format?, width?, height?, region?, background?}` | 図面を PNG / SVG にして返す（下の「描画」） | 読むだけ |

### 作図・変更・変形・レイヤ（段階 1b）

| 道具 | 内容 |
|---|---|
| `add_entities{entities, layer?}` | 図形をまとめて描く。`layer` を省くと現在レイヤ。種類と形は下の表。返り値 `ids` |
| `modify_entities{changes: [{id, set}]}` | `set` に書いた項目だけ変える（ID・レイヤ・色はそのまま）。`get_entities` の `geometry` をそのまま渡してもよい |
| `delete_entities{ids}` | 消す（`undo` で同じ ID のまま戻る） |
| `move_entities{ids, delta, copy?}` | 平行移動。`copy: true` で複製（`created`） |
| `rotate_entities{ids, center, angle_deg, copy?}` | 回転（度、反時計回りが正） |
| `scale_entities{ids, center, factor, copy?}` | 拡大縮小。`factor` は 0 より大きい（裏返すなら `mirror_entities`） |
| `mirror_entities{ids, axis_a, axis_b, keep_original?}` | 2 点を通る直線で鏡に映す。既定は元を置き換え、`keep_original: true` で複製 |
| `set_entity_layer{ids, layer}` | 別のレイヤへ移す |
| `add_layer{name, color, linetype?}` | レイヤを作る。`color` は ACI 1〜255、`linetype` は `continuous` / `dashed` / `center` / `hidden`。同名は拒む |
| `update_layer{name, rename_to?, color?, visible?, locked?, linetype?, make_current?}` | 指定した項目だけ変える（全部で Undo 1 回）。レイヤ 0 の名前は変えられない |
| `delete_layer{name}` | 図形ごと消す（`deleted_entities` に数）。レイヤ 0・現在レイヤは消せない。ロック中のレイヤに図形があれば拒む |

`add_entities` で描ける形（`modify_entities` の `set` も同じ項目名）:

| `type` | 項目 |
|---|---|
| `line` | `start`, `end` |
| `circle` | `center`, `radius` |
| `arc` | `center`, `radius`, `start_angle`, `end_angle`（度。開始から終了へ反時計回り）か、3 点 `start`, `through`, `end` |
| `xline`（無限の作図線） | `origin` と、`angle`（度）・`direction`・`through`（もう 1 つの通過点）のどれか |
| `polyline` | `vertices`（点の配列）, `closed`（省略時 `false`。閉じるなら頂点 3 個以上） |

インスタンス（`instance`）は `add_entities` では置けません（`insert_component` で置く）。
`modify_entities` では配置（`origin`・`rotation`・`scale`・`flipped`）を変えられます。
参照するコンポーネントとパラメータの上書き（`overrides`）は変えられません（同じ値を渡し返すのはかまいません）。
上書きは `set_instance_params`、別のコンポーネントにするなら `delete_entities` と `insert_component` で。

拒むもの: 長さ 0 の線分・半径 0 以下の円・一直線上の 3 点の円弧・頂点の足りないポリライン・NaN・無限大・
絶対値が 10 億（`1e9`）を超える座標、変形した結果がこれらになるもの、種類の変更、ポリラインの頂点の数の変更
（作り直すなら `delete_entities` と `add_entities`。ID は変わります）。

### コンポーネント（段階 1d）

コンポーネントはパラメトリックなブロックです。定義は**ふつうの図形 + 疎な式の束縛**（ADR-0029）で、
パラメータは型付き（数値・真偽・選択）、既定値と束縛は**テキストの式**（ADR-0031。式の中の角度は度）。
インスタンスはパラメータを個別に上書きでき、上書きしていないパラメータは定義の既定値に従います。

| 道具 | 内容 | Undo の名前 |
|---|---|---|
| `define_component{name, origin, from_ids? \| entities?, replace_with_instance?}` | 定義を作る。`from_ids` なら既定で元の図形を消し、同じ場所にインスタンス 1 つを置く（アプリの COMPONENT と同じ。`instance` に ID）。`entities`（`add_entities` と同じ形。`instance` で入れ子も可）なら定義だけ。返り値の `contents` は中身の `index` と種類 | `COMPONENT` |
| `set_component_params{component, params}` | パラメータの宣言を**丸ごと置き換える**（下の表）。使われなくなるインスタンスの上書きは `warnings` | `PARAM` |
| `bind{component, entity_index, slot, expr}` | 定義の中の図形 1 項目に式を束縛する（同じ項目は置き換え）。`slot` は `list_components` の `field` と同じ綴り | `BIND` |
| `insert_component{component, origin, rotation_deg?, scale?, flipped?, layer?, params?}` | インスタンスを置く。`params` の上書きも同じ 1 回で | `INSERT` |
| `set_instance_params{id, values}` | インスタンスの上書きを変える。値を `null` にすると既定値へ戻す | `PSET` |

`set_component_params` の `params` の各要素（`list_components` の `params` の形 ── `choices`・`range`・`'候補'` の既定値 ── も
そのまま渡せます）:

| 項目 | 内容 |
|---|---|
| `name` | 式の中でそのまま書ける名前（英字・日本語・下線で始め、空白・記号・全角英数字なし。`真`・`if`・`sin` などは不可） |
| `type` | `number` / `bool` / `choice` |
| `default` | `number`: 数値か式（他のパラメータを参照してよい。例 `"幅 / 2"`）。`bool`: `true` / `false` か式。`choice`: 候補の名前（省くと最初の候補） |
| `min` / `max` | `number` の範囲（両端を含む。両方そろえる） |
| `options` | `choice` の候補（引用符・前後の空白・全角の記号を含まない文字列） |

`bind` の `slot`（角度は度）:

| 図形 | 項目 |
|---|---|
| `line` | `start.x`, `start.y`, `end.x`, `end.y` |
| `circle` | `center.x`, `center.y`, `radius` |
| `arc` | `center.x`, `center.y`, `radius`, `start_angle`, `end_angle` |
| `xline` | `origin.x`, `origin.y`, `angle` |
| `polyline` | `vertices[i].x`, `vertices[i].y` |
| `instance`（入れ子） | `origin.x`, `origin.y`, `rotation`, `scale` |

上書きの値（`insert_component` の `params`・`set_instance_params` の `values`）は**宣言の型で読みます**: 数値は JSON の数値か
式の文字列（パラメータは使えない）、真偽は `true` / `false`、選択は候補の名前。

拒むもの: 既にある名前・使えない名前、宣言されていないパラメータの参照、既定値どうしの循環、型の違う既定値・範囲の外の既定値、
束縛が使っているパラメータを消す・数値でなくすこと、図形に合わない `slot`・範囲の外の `entity_index`・数値にならない式、
型・範囲・候補に合わない上書き、非表示・ロック中のレイヤの図形（`from_ids`・`set_instance_params` の対象・置く先）。
既定のパラメータで使えない値（半径が 0 以下など）になる束縛は拒まず `warnings` に出します（そのインスタンスではその項目が定義のままになる）。
自分自身を入れ子にする定義は作れません（作る前なので「コンポーネントがありません」で拒みます）。

例（窓: 幅で線分の長さが変わる）:

```json
{"name": "define_component", "arguments": {"name": "窓", "origin": [0, 0], "entities": [
  {"type": "line", "start": [0, 0], "end": [100, 0]}, {"type": "circle", "center": [50, 20], "radius": 10}]}}
{"name": "set_component_params", "arguments": {"component": "窓", "params": [
  {"name": "幅", "type": "number", "default": 100, "min": 10, "max": 500},
  {"name": "向き", "type": "choice", "options": ["左", "右"]}]}}
{"name": "bind", "arguments": {"component": "窓", "entity_index": 0, "slot": "end.x", "expr": "幅"}}
{"name": "insert_component", "arguments": {"component": "窓", "origin": [300, 0], "params": {"幅": 200, "向き": "右"}}}
{"name": "set_instance_params", "arguments": {"id": "<insert_component の id>", "values": {"幅": 150, "向き": null}}}
```

各道具の引数の詳しい説明は `tools/list` の `description` / `inputSchema` にあります（日本語）。
**知らない引数を渡すと、動かす前に拒みます**（綴り違いのフラグを黙って無視しないため）。

## 描画（`render`）

図面を画像にして返します。LLM が自分の描いた図を目で確かめるための道具です。

| 引数 | 内容 |
|---|---|
| `format` | `png`（既定。`image` ブロック）/ `svg`（`text` ブロック）/ `both`（image と text） |
| `width` / `height` | 画像の大きさ px。既定 1024 × 768、**一辺 16〜4096**（超えると拒みます）。LLM が見るなら 1024〜1568 px で十分。Claude の API は長辺が約 1568 px を超える画像を縮小して読む |
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
- **返す大きさの上限**（超えたら `isError` で拒み、図面は変わりません）
  - **PNG は 3.5 MiB まで**（生のバイト数。base64 にして約 4.9 MB で、Claude の API の画像 1 枚の上限（約 5 MB）に収まる）。
    超えたら `width` / `height` を下げるか `region` で絞るよう促します。既定の 1024 × 768 なら密な図面でも収まります
  - **SVG は 256 KiB まで**（`format` が `svg` / `both` のとき。図形 2000 個ほど）。LLM が本文を読む形式なので小さくしてあります。
    超えたら `region` で絞るよう促します（SVG の大きさは `width` / `height` にほぼ依りません）。図形が多い図面は PNG で見ます。
    SVG は書きながら大きさを見て、上限を超えた時点で打ち切ります（頂点が数百万のポリライン 1 本でも、書き終えてから止まりません）
  - `format` が `png` のときの途中の SVG（読まれずにラスタにされる）は 8 MiB まで
- 表示範囲が円・円弧の円周に囲まれて線が 1 本も見えないときは、その図形を書かず `entities_outside_view` に数えます
  （拡大して大きな円の内側を見るとき。円弧は弧の範囲までは見ないので、円周が範囲を通れば数えます）
- 白い背景（`light`）で黒に寄せるのは白（ACI 7）だけです。ACI の対応表（`AciColor::rgb`）に、ほかに白に近い色は
  ありません（ACI 10 以降は灰 160。黄 ACI 2 は輝度が高いが、色そのものに意味があるので寄せません）
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
- **開いた `.dxf` へ `path` なしで保存しない**（設計原則 9）。DXF は非可逆なので、確認なしに上書きすると
  作図線・グループ・パラメータなどが黙って失われる。`.ymc` のパスか、DXF へ書くなら `.dxf` のパスを明示させる
- **図面を変える道具が内部エラー（panic）で止まったら、その図面は保存できなくなる**（図面が書きかけかもしれないため）。
  `drawing_info` の `poisoned` が `true` になる。`open_drawing` / `new_drawing` で外れる
- 上限: 1 行 4 MiB（超えた行は読み捨てて `-32700`）、開くファイル 64 MiB、一覧 1000 件、ID 1000 個、undo / redo 256 回、
  1 回で描く・変える図形 1000 個（コンポーネントの中身も）、パラメータ 256 個・選択の候補 256 個、コンポーネント名・パラメータ名・候補 255 文字、ポリラインの頂点 10,000 個、図面の図形 100 万個、座標の絶対値 10 億、式 1 KiB、
  レイヤ名 255 文字、画像の一辺 4096 px（最大でも 4096 × 4096 × 4 = 64 MiB の画素）、PNG 3.5 MiB・SVG 256 KiB（`render` の返す大きさ）、
  **道具の結果（JSON の text）256 KiB**（読むだけの道具は超えたら `isError` で絞り込みを促す。図面を変える道具は
  変更を済ませたうえで結果だけ省く。`render` の画像・SVG のブロックはこの数に入らない）
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
# render の SVG（xml.etree で解析）と PNG（シグネチャ・IHDR・CRC・IDAT）、コンポーネントの通し
# （定義 → パラメータ → 束縛 → 上書き付きの配置 → 上書きの変更 → components.ymc。--expect で instance= の件数まで）もここで検査する
mkdir -p /tmp/mcp
cargo run -p cad-core --example write_sample -- /tmp/mcp/sample.ymc
cargo run -p cad-core --example write_sample -- /tmp/mcp/sample.dxf
python3 tools/mcp_smoke.py target/release/ymcad-mcp /tmp/mcp
```
