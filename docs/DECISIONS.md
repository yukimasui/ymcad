# 設計判断の記録 (ADR)

1 判断 = 1 エントリ。決めた内容と、**なぜそう決めたか**を残す。

---

## ADR-0001: IME 検証結果 (Phase 0)

- **状態**: 確定 — **合格**。egui / eframe を採用し Phase 1 へ進む。
- **日付**: 2026-08-11

### 背景

ymcad は AutoCAD ライクなコマンドラインを持つ。テキストエンティティ自体は非スコープだが、コマンド入力欄とレイヤ名で日本語入力を受ける可能性がある。`egui` の IME サポートが Ubuntu で成立しないなら GUI の技術選定そのものを見直す必要があるため、実装に先立って検証する。

### 検証環境

| 項目 | 値 |
|---|---|
| OS | Ubuntu 24.04.4 LTS |
| セッション | Wayland (`XDG_SESSION_TYPE=wayland`)、`DISPLAY=:0` あり (XWayland 利用可) |
| IME | ibus (`QT_IM_MODULE=ibus`, `XMODIFIERS=@im=ibus`, `GTK_IM_MODULE` は未設定) |
| toolchain | rustc / cargo 1.96.0 |
| GUI | eframe / egui 0.36.1、winit 0.30.13 |

`GTK_IM_MODULE` が未設定でも問題ない。winit は IME に GTK を経由しない。

### ソース調査で確定した事実

egui 0.36.1 / egui-winit 0.36.1 / winit 0.30.13 のソースを直接確認した結果:

1. **アプリ側の IME API 呼び出しは不要。**
   `egui-winit-0.36.1/src/lib.rs:1152` が `window.set_ime_allowed()` を、同 `:1177` が
   `window.set_ime_cursor_area()` を自動で呼ぶ。トリガーは `PlatformOutput::ime` で、これは
   フォーカスを持つ `TextEdit` 自身が毎フレーム書き込む。`NativeOptions` の opt-in も環境変数も不要。

2. **`ImeEvent::Enabled` / `Disabled` は届かない。**
   egui 0.36 で両者は `#[deprecated = "No longer used by egui"]`
   (`egui-0.36.1/src/data/input/ime_event.rs:8,35`)。egui-winit は winit の該当イベントを
   意図的に捨てている（winit#2498「X11 と Wayland で意味が異なる」ため）。
   生きているのは `Preedit` / `Commit` / `DeleteSurrounding` の 3 つ。

3. **候補ウィンドウは `IMEOutput::rect`（ウィジェット全体）にアンカーされる。**
   `cursor_rect`（キャレット位置）も計算されるが egui-winit は使っていない。
   結果として候補ウィンドウは「キャレット直下」ではなく「入力欄の左端」に出る。
   横長のコマンドライン UI では軽微な違和感になりうるが、機能上の欠陥ではない。

4. **未確定文字列はアプリの `&mut String` に直接書き込まれる。**
   `TextEdit` は `ImeEvent::Preedit` を受けて `insert_text_at` でバッファへ挿入し、
   `Commit` 時に `clear_preedit_text` してから確定文字列を入れる。
   → **変換中にコマンドラインのバッファをパースしてはいけない。** Phase 3 の設計制約になる
   （[ADR-0002](#adr-0002-コマンドラインは変換中にパースしない-phase-3) 参照）。

5. **`WINIT_UNIX_BACKEND` は winit 0.30 で廃止済み。**
   バックエンド選択は `WAYLAND_DISPLAY`／`WAYLAND_SOCKET` が非空なら Wayland、
   でなければ `DISPLAY` を見て X11。X11 で試すには `WAYLAND_DISPLAY= cargo run` とする。
   なお `set_ime_cursor_area` は **X11 では位置のみ有効でサイズが無視される**
   (winit 0.30.13 のドキュメント記載)。Wayland は `zwp_text_input_v3::set_cursor_rectangle`
   で完全に実装されており、この点では **Wayland のほうが X11 より対応が良い**。

6. **既知の ibus + Wayland 不具合は修正済み。**
   egui#7485（ibus + Wayland で 1 文字しか入力できない）は 2026-04-06 に close 済みで、
   修正は egui 0.35 に入っている。egui 0.36.1 時点で ibus + Wayland の未解決 issue は無い。
   未解決なのは fcitx5 系（egui#7975, #2529）で、本環境の構成には該当しない。

7. **egui の同梱フォントは CJK グリフを持たない。**
   これを放置すると日本語がすべて豆腐 (□) になり、「IME が壊れている」という誤判定を招く。
   `/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc` のフェイス index 0
   (`Noto Sans CJK JP`、`fc-query` で確認) を明示的に読み込む必要がある。
   epaint 0.36 は skrifa 経由で `.ttc` の face index 指定に対応している
   (`epaint-0.36.1/src/text/fonts.rs:162`)。

   → **Phase 1 の `cad-app` にもこのフォント読み込みが必須**。スパイクの
   `spikes/ime-check/src/jp_font.rs` をそのまま移植する。

### 手動検証の結果

`spikes/ime-check` をユーザーが起動し、中央の入力欄①と下部の入力欄②（本体のコマンドラインと同じ位置）の両方で `nihongo` → 変換 → 確定を実施した。

| # | 確認項目 | Wayland (ibus + mozc) | 備考 |
|---|---|---|---|
| 1 | 変換候補ウィンドウが表示されるか | ✅ | mozc の候補ポップアップが出る |
| 2 | 候補ウィンドウがカーソル位置に追従するか | ✅ | 入力欄①と②で表示位置が変わることを確認 |
| 3 | 未確定文字列がインライン表示されるか | ✅ | 入力欄の**中に下線付き**で表示された |
| 4 | 確定文字列が正しくバッファに入るか | ✅ | ①②の両方で `Commit` を受信、文字化け・欠落なし |
| 5 | IME 有効化 API の明示呼び出しが必要か | ✅ **不要** | 入力欄クリックで【項目5】が緑化、外すと復帰。アプリ側は IME API を一切呼んでいない |

日本語は豆腐にならず正しく描画された（Noto Sans CJK JP の明示読み込みによる）。

X11 (`WAYLAND_DISPLAY= cargo run`) での比較は未実施。Wayland ネイティブが本命であり、そちらが全項目 PASS のため Phase 0 の判断には不要と判断した。Phase 1 で `cad-app` が起動できるようになった時点で X11 起動確認を行う（申し送り事項）。

### 決定

**egui / eframe 0.36 を採用し、そのまま Phase 1 へ進む。** IME 回避策（X11 バックエンド強制、外部ダイアログ、独自 IME ブリッジ）はいずれも不要。

派生する実装上の決定:

1. **`cad-app` でも日本語フォントの明示読み込みを必須とする。**
   `spikes/ime-check/src/jp_font.rs` をそのまま `cad-app` へ移植する。
2. **IME 系 API はアプリ側から呼ばない。** egui-winit の自動処理に任せる。
   ただしコマンドライン確定時の変換中断だけは `Memory::interrupt_ime()` を使う
   （[ADR-0002](#adr-0002-コマンドラインは変換中にパースしない-phase-3) 参照）。
3. **候補ウィンドウが入力欄左端にアンカーされる件は仕様として受け入れる。**
   ymcad のコマンドラインは横長のため候補がキャレットから離れて出るが、
   egui 0.36 の実装に由来するもので、回避には egui 本体の改変が要る。実害は軽微。

---

## ADR-0002: コマンドラインは変換中にパースしない (Phase 3)

- **状態**: 予定（Phase 3 で実装）
- **日付**: 2026-08-11

### 背景

ADR-0001 の調査 4 のとおり、`TextEdit` は未確定文字列をアプリのバッファへ直接書き込む。加えて:

- IME がかな入力モードのとき、`@100,50` は `＠１００，５０` のように全角で入りうる
- 変換中は winit が `KeyboardInput` を送らないため、Enter によるコマンド確定ハンドラが沈黙し、確定用の Enter で予期せず発火する

座標直接入力（`100,50` / `@100,50` / `@100<45`）を扱う ymcad のコマンドラインでは、これが誤入力に直結する。

### 決定

Phase 3 のコマンドライン実装で以下を守る:

1. アプリ状態に `composing: bool` を持つ。`ImeEvent::Preedit { text, .. }` で `text` が非空なら `true`、`Commit` で `false`。
2. **`composing == true` の間はバッファを一切パースしない。**
3. パース前に ASCII 範囲を NFKC 正規化する（`＠` `，` `＜` `０-９` → 半角）。
4. コマンドを確定する Enter は、IME を確定する Enter とは**別の打鍵**であることを要求する。

