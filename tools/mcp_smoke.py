#!/usr/bin/env python3
"""ymcad の MCP サーバー（ymcad-mcp）を、Rust とは別の実装のクライアントで通しで動かす。

なぜ必要か
----------
Rust の結合テスト（crates/cad-mcp/tests/stdio.rs）は、サーバーと同じ serde_json で
メッセージを組み立てて読む。書き手と読み手が同じ誤解をしていれば通ってしまう
（validate_ymc.py がある理由と同じ）。ここでは Python の標準ライブラリだけで
JSON-RPC を話し、保存されたファイルを validate_ymc.py に通す。

やること
--------
1. バイナリを --root <作業ディレクトリ> で起動する
2. initialize（版の取り決め）→ notifications/initialized → tools/list（スキーマの形）
3. sample.dxf を開いて from_dxf.ymc へ保存（DXF → ネイティブの変換）
4. sample.ymc を開き、図形・レイヤ・コンポーネントを照会し、roundtrip.ymc へ保存
   （開いて保存しただけの .ymc は元とバイト単位で一致すること）
5. render で SVG / PNG を取り、Python 側で検査する（SVG は xml.etree で解析して要素数・座標、
   PNG はシグネチャ・IHDR の幅と高さ・CRC・IDAT の展開後の大きさ）
6. root の外・.. を含むパス・上書きの確認・開いた .dxf への path なしの保存が isError になること
7. 新規図面に、レイヤを作り、図形を描き、変え、動かし・回し・拡大し・鏡に映し（複製も）、
   レイヤを移し、消し、undo / redo し、レイヤを変え・消して edited.ymc へ保存（段階 1b）
8. 新規図面で、図形からコンポーネントを定義 → パラメータ → 束縛 → 上書き付きの配置 →
   上書きの変更 → undo / redo（置いたインスタンスの ID が変わらないこと）→ components.ymc へ保存（段階 1d）
9. stdin を閉じ、正常終了すること。stdout にプロトコル以外が出ていないこと
10. 保存したファイルを validate_ymc.py に通す（edited.ymc・components.ymc は図形の種類ごとの件数まで）

使い方
------
    cargo run -p cad-core --example write_sample -- DIR/sample.ymc
    cargo run -p cad-core --example write_sample -- DIR/sample.dxf
    python3 tools/mcp_smoke.py target/release/ymcad-mcp DIR

終了コード 0 で合格、1 で不合格。
"""

from __future__ import annotations

import base64
import json
import math
import struct
import subprocess
import sys
import xml.etree.ElementTree as ET
import zlib
from pathlib import Path

HERE = Path(__file__).resolve().parent
VALIDATE_YMC = HERE / "validate_ymc.py"

# write_sample の中身（CI の validate_ymc.py --expect と同じ）。
SAMPLE_EXPECT = "arc=1,xline=1,polyline=1,instance=3,circle=2"

# 段階 1b の通しで描いた図面の中身（edit_session の手順から数えた値）。
EDITED_EXPECT = "line=5,circle=1,arc=2,xline=0,polyline=2,instance=0"

# 段階 1d の通しで作った図面の中身。validate_ymc.py は定義の中の図形も数えるので、
# 図面のインスタンス 2 つ + 定義「窓」の中身（線分 1・円 1）。
COMPONENTS_EXPECT = "line=1,circle=1,arc=0,xline=0,polyline=0,instance=2"

EXPECTED_TOOLS = {
    "new_drawing",
    "open_drawing",
    "save_drawing",
    "drawing_info",
    "list_entities",
    "get_entities",
    "list_layers",
    "list_components",
    "undo",
    "redo",
    "render",
    # 段階 1b
    "add_entities",
    "modify_entities",
    "delete_entities",
    "move_entities",
    "rotate_entities",
    "scale_entities",
    "mirror_entities",
    "set_entity_layer",
    "add_layer",
    "update_layer",
    "delete_layer",
    # 段階 1d
    "define_component",
    "set_component_params",
    "bind",
    "insert_component",
    "set_instance_params",
}

# 図面を変える道具（readOnlyHint が false であること）。
EDITING_TOOLS = EXPECTED_TOOLS - {
    "drawing_info",
    "list_entities",
    "get_entities",
    "list_layers",
    "list_components",
    "render",
}


class SmokeError(Exception):
    pass


def check(cond: bool, message: str) -> None:
    if not cond:
        raise SmokeError(message)


class Client:
    def __init__(self, binary: Path, root: Path) -> None:
        self.proc = subprocess.Popen(
            [str(binary), "--root", str(root)],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        self.next_id = 1

    def send(self, msg: dict) -> None:
        assert self.proc.stdin is not None
        line = json.dumps(msg, ensure_ascii=False)
        check("\n" not in line, "送るメッセージに改行が入った")
        self.proc.stdin.write(line.encode("utf-8") + b"\n")
        self.proc.stdin.flush()

    def recv(self) -> dict:
        assert self.proc.stdout is not None
        line = self.proc.stdout.readline()
        check(line.endswith(b"\n"), f"返事が 1 行で来なかった: {line!r}")
        try:
            msg = json.loads(line.decode("utf-8"))
        except (UnicodeDecodeError, json.JSONDecodeError) as e:
            raise SmokeError(f"stdout にプロトコル以外が出た: {line!r} ({e})") from e
        check(msg.get("jsonrpc") == "2.0", f"jsonrpc が 2.0 でない: {msg}")
        return msg

    def request(self, method: str, params: dict | None = None) -> dict:
        req_id = self.next_id
        self.next_id += 1
        msg = {"jsonrpc": "2.0", "id": req_id, "method": method}
        if params is not None:
            msg["params"] = params
        self.send(msg)
        reply = self.recv()
        check(reply.get("id") == req_id, f"id が合わない: {reply}")
        return reply

    def call(self, name: str, arguments: dict) -> dict:
        reply = self.request("tools/call", {"name": name, "arguments": arguments})
        check("result" in reply, f"{name}: JSON-RPC のエラー: {reply}")
        return reply["result"]

    def ok(self, name: str, arguments: dict) -> dict:
        result = self.call(name, arguments)
        check(result.get("isError") is False, f"{name} {arguments} が失敗: {result}")
        structured = result.get("structuredContent")
        check(isinstance(structured, dict), f"{name}: structuredContent がオブジェクトでない")
        text = result["content"][0]
        check(text["type"] == "text", f"{name}: content[0] が text でない")
        check(json.loads(text["text"]) == structured, f"{name}: text と structuredContent が食い違う")
        return structured

    def fails(self, name: str, arguments: dict, needle: str) -> str:
        result = self.call(name, arguments)
        check(result.get("isError") is True, f"{name} {arguments} が成功してしまった: {result}")
        text = result["content"][0]["text"]
        check(needle in text, f"{name}: 説明に {needle!r} が無い: {text}")
        return text

    def close(self) -> tuple[int, bytes, bytes]:
        # stdin は communicate() が閉じる。先に閉じると Python 3.12 は閉じたファイルを
        # flush しようとして ValueError になる（3.14 は無視するので手元では気づけない）。
        try:
            rest, err = self.proc.communicate(timeout=30)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            raise SmokeError("stdin を閉じても終了しない")
        return self.proc.returncode, rest, err


def check_tool_list(tools: list) -> None:
    names = [t["name"] for t in tools]
    check(len(names) == len(set(names)), f"道具の名前が重複: {names}")
    missing = EXPECTED_TOOLS - set(names)
    check(not missing, f"道具が足りない: {sorted(missing)}")
    for t in tools:
        schema = t["inputSchema"]
        check(schema.get("type") == "object", f"{t['name']}: inputSchema が object でない")
        props = schema.get("properties", {})
        for key in schema.get("required", []):
            check(key in props, f"{t['name']}: required の {key} が properties に無い")
        ann = t.get("annotations", {})
        check(isinstance(ann.get("readOnlyHint"), bool), f"{t['name']}: readOnlyHint が無い")
        check(isinstance(ann.get("destructiveHint"), bool), f"{t['name']}: destructiveHint が無い")
        if t["name"] in EDITING_TOOLS:
            check(ann["readOnlyHint"] is False, f"{t['name']}: 図面を変えるのに readOnlyHint が true")


def count(c: Client) -> int:
    return c.ok("drawing_info", {})["entity_count"]


def edit_session(c: Client) -> None:
    """段階 1b: 新規図面に描いて変え、edited.ymc へ保存する。

    件数は EDITED_EXPECT と突き合わせる。各手順の後の図形の数も見る。
    """
    c.ok("new_drawing", {})
    info = c.ok("drawing_info", {})
    drawing = info["drawing"]

    # --- レイヤ ---------------------------------------------------------------
    wall = c.ok("add_layer", {"name": "WALL", "color": 1, "linetype": "dashed"})["layer"]
    check(wall["linetype"] == "dashed" and wall["color"] == 1, f"add_layer: {wall}")
    c.fails("add_layer", {"name": "WALL", "color": 2}, "既に")
    c.ok("add_layer", {"name": "LOCKED", "color": 2})
    c.ok("update_layer", {"name": "LOCKED", "locked": True})

    # --- 作図（1 回の呼び出しで 5 つ）-----------------------------------------
    drawn = c.ok(
        "add_entities",
        {
            "layer": "WALL",
            "entities": [
                {"type": "line", "start": [0, 0], "end": [100, 0]},
                {"type": "circle", "center": {"x": 50, "y": 50}, "radius": "5*2"},
                {"type": "arc", "start": [10, 0], "through": [0, 10], "end": [-10, 0]},
                {"type": "xline", "origin": [0, 0], "through": [1, 1]},
                {"type": "polyline", "vertices": [[0, 0], [10, 0], [10, 10]], "closed": True},
            ],
        },
    )
    ids = drawn["ids"]
    check(len(ids) == 5 and all(i.startswith(f"{drawing}e") for i in ids), f"add_entities の ID: {drawn}")
    line, circle, arc, xline, poly = ids
    check(count(c) == 5, "5 つ描けていない")
    got = c.ok("get_entities", {"ids": [circle, arc, xline]})["entities"]
    check(got[0]["geometry"]["radius"] == 10 and got[0]["layer"] == "WALL", f"式の半径・レイヤ: {got[0]}")
    a = got[1]["geometry"]
    # 3 点の円弧は中心 (0,0)・半径 10・0°→180°。度は Python の math で別に求めて比べる。
    check(
        math.isclose(a["radius"], 10)
        and math.isclose(a["start_angle"] % 360, 0, abs_tol=1e-6)
        and math.isclose(a["end_angle"], math.degrees(math.pi)),
        f"3 点の円弧: {a}",
    )
    check(math.isclose(got[2]["geometry"]["angle"], 45), f"2 点の作図線: {got[2]}")

    # 拒まれる呼び出しは何も変えない。
    c.fails("add_entities", {"layer": "LOCKED", "entities": [{"type": "line", "start": [0, 0], "end": [1, 0]}]}, "ロック中")
    c.fails(
        "add_entities",
        {"entities": [{"type": "line", "start": [0, 0], "end": [1, 0]}, {"type": "circle", "center": [0, 0], "radius": 0}]},
        "entities[1]",
    )
    c.fails("modify_entities", {"changes": [{"id": poly, "set": {"vertices": [[0, 0], [1, 1]]}}]}, "頂点の数")
    c.fails("scale_entities", {"ids": [circle], "center": [0, 0], "factor": 0}, "0 より大きい")
    check(count(c) == 5, "拒まれた呼び出しで図形の数が変わった")

    # --- 変更・変形 -------------------------------------------------------------
    c.ok("modify_entities", {"changes": [{"id": circle, "set": {"radius": 20}}]})
    moved = c.ok("move_entities", {"ids": [line], "delta": [0, 10], "copy": True})
    check(len(moved["created"]) == 1, f"move の複製: {moved}")
    line_copy = moved["created"][0]
    check(c.ok("get_entities", {"ids": [line_copy]})["entities"][0]["geometry"]["start"] == {"x": 0, "y": 10}, "複製の位置")
    rotated = c.ok("rotate_entities", {"ids": [poly], "center": [0, 0], "angle_deg": 90, "copy": True})
    check(len(rotated["created"]) == 1, f"rotate の複製: {rotated}")
    c.ok("scale_entities", {"ids": [circle], "center": [50, 50], "factor": 2})
    check(c.ok("get_entities", {"ids": [circle]})["entities"][0]["geometry"]["radius"] == 40, "拡大した半径")
    mirrored = c.ok("mirror_entities", {"ids": [arc], "axis_a": [0, -1], "axis_b": [0, 1], "keep_original": True})
    check(len(mirrored["created"]) == 1, f"mirror の複製: {mirrored}")
    c.ok("set_entity_layer", {"ids": [line_copy], "layer": "0"})
    c.ok("delete_entities", {"ids": [xline]})
    c.fails("get_entities", {"ids": [xline]}, "ありません")
    check(count(c) == 7, "変形の後の図形の数")

    # --- undo / redo: 1 回の呼び出しが undo 1 回ぶん -----------------------------
    three = [{"type": "line", "start": [0, y], "end": [5, y]} for y in (100, 110, 120)]
    c.ok("add_entities", {"entities": three})
    check(count(c) == 10, "3 本描けていない")
    undone = c.ok("undo", {})
    check(undone["undone"] == ["ADD"] and count(c) == 7, f"undo 1 回で 3 本とも消えない: {undone}")
    c.ok("redo", {})
    check(count(c) == 10, "redo で戻らない")

    # --- レイヤの変更と削除 -------------------------------------------------------
    renamed = c.ok("update_layer", {"name": "WALL", "rename_to": "外壁", "color": 3, "make_current": True})["layer"]
    check(renamed["name"] == "外壁" and renamed["current"] and renamed["color"] == 3, f"update_layer: {renamed}")
    check(c.ok("undo", {})["undone"] == ["LAYER"], "update_layer が undo 1 回ぶんでない")
    c.ok("redo", {})
    c.ok("add_layer", {"name": "TMP", "color": 4})
    c.ok("add_entities", {"layer": "TMP", "entities": [{"type": "circle", "center": [0, 0], "radius": 1}]})
    deleted = c.ok("delete_layer", {"name": "TMP"})
    check(deleted["deleted_entities"] == 1, f"delete_layer: {deleted}")
    c.fails("delete_layer", {"name": "外壁"}, "現在レイヤ")
    layers = {l["name"]: l for l in c.ok("list_layers", {})["layers"]}
    check(set(layers) == {"0", "外壁", "LOCKED"}, f"レイヤの一覧: {sorted(layers)}")
    # 3 本の線分は layer を省いたので、そのときの現在レイヤ 0 に置かれている。
    check(layers["外壁"]["entity_count"] == 6 and layers["0"]["entity_count"] == 4, f"レイヤごとの数: {layers}")
    check(count(c) == 10, "最後の図形の数")

    saved = c.ok("save_drawing", {"path": "edited.ymc"})
    check(saved["entity_count"] == 10 and saved["format"] == "ymc", f"保存: {saved}")


def components_session(c: Client) -> None:
    """段階 1d: 定義 → パラメータ → 束縛 → 上書き付きの配置 → 上書きの変更 → components.ymc へ保存。"""
    c.ok("new_drawing", {})
    drawn = c.ok(
        "add_entities",
        {"entities": [
            {"type": "line", "start": [0, 0], "end": [100, 0]},
            {"type": "circle", "center": [50, 20], "radius": 10},
        ]},
    )
    line, circle = drawn["ids"]

    # 図形からコンポーネントを作ると、元の図形は同じ場所のインスタンス 1 つに置き換わる。
    defined = c.ok("define_component", {"name": "窓", "origin": [0, 0], "from_ids": [line, circle]})
    check(defined["removed"] == 2 and defined["instance"], f"define_component: {defined}")
    check([x["type"] for x in defined["contents"]] == ["line", "circle"], f"contents: {defined}")
    first = defined["instance"]
    check(count(c) == 1, "置き換えの後の図形の数")
    c.fails("get_entities", {"ids": [line]}, "ありません")

    params = c.ok(
        "set_component_params",
        {"component": "窓", "params": [
            {"name": "幅", "type": "number", "default": 100, "min": 10, "max": 500},
            {"name": "開く", "type": "bool", "default": False},
            {"name": "向き", "type": "choice", "options": ["左", "右"]},
        ]},
    )["component"]["params"]
    check([p["name"] for p in params] == ["幅", "開く", "向き"], f"params: {params}")
    check(params[0]["range"] == [10, 500] and params[2]["choices"] == ["左", "右"], f"params: {params}")
    c.fails("set_component_params", {"component": "窓", "params": [{"name": "a b", "type": "number", "default": 1}]}, "名前")

    bound = c.ok("bind", {"component": "窓", "entity_index": 0, "slot": "end.x", "expr": "幅 * 2"})
    check(bound["value"] == 200 and bound["binding"]["field"] == "end.x", f"bind: {bound}")
    c.fails("bind", {"component": "窓", "entity_index": 0, "slot": "radius", "expr": "幅"}, "束縛できません")
    c.fails("bind", {"component": "窓", "entity_index": 0, "slot": "end.x", "expr": "高さ"}, "高さ")

    # 上書き付きの配置は 1 回の呼び出し = undo 1 回。
    placed = c.ok(
        "insert_component",
        {"component": "窓", "origin": [300, 0], "params": {"幅": 50, "向き": "右"}},
    )
    second = placed["id"]
    check(placed["overrides"] == {"幅": 50, "向き": "右"}, f"insert_component: {placed}")
    check(placed["params"] == {"幅": 50, "開く": False, "向き": "右"}, f"insert_component: {placed}")
    c.fails("insert_component", {"component": "窓", "origin": [0, 0], "params": {"幅": 5}}, "範囲")
    c.fails("insert_component", {"component": "窓", "origin": [0, 0], "params": {"向き": "上"}}, "左 / 右")
    check(count(c) == 2, "拒まれた配置で図形の数が変わった")

    changed = c.ok("set_instance_params", {"id": second, "values": {"幅": 150, "開く": True, "向き": None}})
    check(changed["overrides"] == {"幅": 150, "開く": True}, f"set_instance_params: {changed}")
    # modify_entities では上書きを変えられず、set_instance_params へ案内される。
    c.fails("modify_entities", {"changes": [{"id": second, "set": {"overrides": {"幅": 20}}}]}, "set_instance_params")

    # まとめて取り消してやり直しても、置いたインスタンスの ID は変わらない（後の呼び出しがその ID を使える）。
    check(c.ok("undo", {"steps": 2})["undone"] == ["PSET", "INSERT"], "undo の名前")
    check(count(c) == 1, "undo で配置が消えない")
    check(c.ok("redo", {"steps": 2})["count"] == 2, "redo が途中で止まった")
    got = c.ok("get_entities", {"ids": [first, second]})["entities"]
    check(got[1]["geometry"]["overrides"] == {"幅": 150, "開く": True}, f"redo の後の上書き: {got[1]}")
    check(got[1]["geometry"]["origin"] == {"x": 300, "y": 0}, f"配置: {got[1]}")

    # 束縛が効いていること: 2 つ目は 幅 150 → 線分の終点 x = 300 + 150 * 2 = 600（束縛が無ければ 400）。
    # render の既定の範囲は表示中の図形（インスタンスの中身を含む）の範囲に余白を付けたもの。
    view = c.ok("render", {"format": "svg", "width": 400, "height": 200})
    check(view["entities_drawn"] == 2, f"render: {view}")
    check(view["view"]["max"]["x"] >= 600 and view["view"]["min"]["x"] <= 0, f"束縛が効いていない: {view}")
    listed = c.ok("list_components", {"name": "窓", "contents": True})["components"][0]
    check(listed["instance_count"] == 2 and len(listed["contents"]) == 2, f"list_components: {listed}")
    check(listed["bindings"][0]["expr"] == "幅 * 2", f"bindings: {listed}")

    saved = c.ok("save_drawing", {"path": "components.ymc"})
    check(saved["entity_count"] == 2, f"保存: {saved}")


SVG_NS = "{http://www.w3.org/2000/svg}"
PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"


def png_chunks(data: bytes) -> list[tuple[bytes, bytes]]:
    """PNG をチャンク（種類, 本体）に分ける。CRC を検査し、末尾でぴったり尽きることを見る。"""
    check(data[:8] == PNG_SIGNATURE, f"PNG のシグネチャでない: {data[:8]!r}")
    pos, chunks = 8, []
    while pos < len(data):
        check(pos + 12 <= len(data), "PNG のチャンクが途中で切れている")
        (length,) = struct.unpack(">I", data[pos : pos + 4])
        kind = data[pos + 4 : pos + 8]
        body = data[pos + 8 : pos + 8 + length]
        (crc,) = struct.unpack(">I", data[pos + 8 + length : pos + 12 + length])
        check(len(body) == length, "PNG のチャンクが途中で切れている")
        check(zlib.crc32(kind + body) == crc, f"PNG の {kind!r} の CRC が合わない")
        chunks.append((kind, body))
        pos += 12 + length
    check(chunks and chunks[-1][0] == b"IEND", "PNG が IEND で終わっていない")
    check(chunks[0][0] == b"IHDR", "PNG が IHDR で始まっていない")
    return chunks


def check_png(b64: str, width: int, height: int) -> None:
    data = base64.b64decode(b64, validate=True)
    chunks = png_chunks(data)
    w, h, depth, color, _comp, _filt, interlace = struct.unpack(">IIBBBBB", chunks[0][1])
    check((w, h) == (width, height), f"PNG の大きさが違う: {(w, h)} != {(width, height)}")
    check((depth, color, interlace) == (8, 6, 0), f"PNG の形式が RGBA 8 ビットでない: {(depth, color, interlace)}")
    raw = zlib.decompress(b"".join(body for kind, body in chunks if kind == b"IDAT"))
    # 1 行ごとにフィルタ 1 バイト + RGBA 4 バイト × 幅。
    check(len(raw) == height * (1 + 4 * width), f"IDAT の展開後の大きさが合わない: {len(raw)}")


def svg_elements(svg: str) -> dict[str, list[ET.Element]]:
    root = ET.fromstring(svg)
    check(root.tag == SVG_NS + "svg", f"ルートが svg でない: {root.tag}")
    out: dict[str, list[ET.Element]] = {}
    for el in root.iter():
        out.setdefault(el.tag.removeprefix(SVG_NS), []).append(el)
    return out


def check_render(c: "Client", info: dict, entities: list[dict]) -> None:
    """render の通し検査。Python 側で独立に座標を求めて SVG と比べる。"""
    ppu_of = lambda r: r["width"] / (r["view"]["max"]["x"] - r["view"]["min"]["x"])

    # --- 既定（PNG）。結果の先頭は structuredContent と同じ JSON の text、その後ろに image ---
    result = c.call("render", {"width": 640, "height": 480})
    check(result.get("isError") is False, f"render が失敗: {result}")
    blocks = result["content"]
    check([b["type"] for b in blocks] == ["text", "image"], f"content の並び: {[b['type'] for b in blocks]}")
    check(blocks[1]["mimeType"] == "image/png", f"mimeType: {blocks[1]['mimeType']}")
    check(json.loads(blocks[0]["text"]) == result["structuredContent"], "render: text と structuredContent が違う")
    check_png(blocks[1]["data"], 640, 480)
    sc = result["structuredContent"]
    check(sc["png_bytes"] == len(base64.b64decode(blocks[1]["data"])), "png_bytes が実際の大きさと違う")
    check(sc["region_source"] == "drawing_extent" and sc["background"] == "dark", f"render の既定: {sc}")

    # 範囲を省くと、図面の範囲（drawing_info の bbox。作図線を含まない）が余白つきで入る。
    view, box = sc["view"], info["bbox"]
    check(
        view["min"]["x"] < box["min"]["x"] and view["min"]["y"] < box["min"]["y"]
        and view["max"]["x"] > box["max"]["x"] and view["max"]["y"] > box["max"]["y"],
        f"view が図面の範囲を覆っていない: {view} {box}",
    )
    # 縦横比を保つ: 1 単位あたりの px が横も縦も同じ。
    ppu = sc["pixels_per_unit"]
    check(math.isclose(ppu_of(sc), ppu) and math.isclose(480 / (view["max"]["y"] - view["min"]["y"]), ppu),
          f"pixels_per_unit が view と合わない: {sc}")

    # --- SVG: 解析して要素数と座標を見る。write_sample は表示中の図形 6 つ（作図線は非表示レイヤ）---
    r = c.call("render", {"format": "svg", "width": 640, "height": 480})
    check(r.get("isError") is False, f"render(svg) が失敗: {r}")
    svg_res = r["structuredContent"]
    svg = r["content"][1]["text"]
    check(r["content"][1]["type"] == "text" and svg_res["svg_bytes"] == len(svg), "SVG の text ブロック")
    el = svg_elements(svg)
    root = el["svg"][0]
    check((root.get("width"), root.get("height"), root.get("viewBox")) == ("640", "480", "0 0 640 480"), "svg の大きさ")
    check(svg_res["entities_drawn"] == 6 and svg_res["entities_on_hidden_layers"] == 1, f"描いた数: {svg_res}")
    # 表示中の図形は 線 1・円 1・円弧 1・ポリライン 1・インスタンス 2（中身は線 1 と円 1）。
    # 展開すると 線分 3・円 3・path 2（円弧とポリライン）になる。作図線は非表示のレイヤなので描かない。
    check(len(el.get("line", [])) == 3, f"線分の要素数: {len(el.get('line', []))}（非表示レイヤの作図線を描いていないか）")
    check(len(el.get("circle", [])) == 3, f"円の要素数: {len(el.get('circle', []))}")
    paths = el.get("path", [])
    check(len(paths) == 2, f"path の要素数: {len(paths)}")
    check(sum("A" in p.get("d", "") for p in paths) == 1, "円弧の A コマンドが 1 つでない")
    check(sum(p.get("d", "").endswith("Z") for p in paths) == 1, "閉じたポリラインの Z が 1 つでない")
    check(len(el["rect"]) == 1 and el["rect"][0].get("fill") == "#0a0a0a", "背景の rect")
    check(all(e.get("data-id", "").startswith(f"{info['drawing']}e") for tag in ("line", "circle", "path") for e in el.get(tag, [])),
          "data-id が図形 ID でない")
    check(any(l.get("stroke-dasharray") == "12 3 3 3" for l in el["line"]), "一点鎖線の破線パターンが無い")

    # 座標の対応を独立に求める: 図面の線分（レイヤ 0 の最初の線分）の端点 → 画像 px。
    # px = (x - view.min.x) * ppu、py = (view.max.y - y) * ppu（Y を反転）。
    ppu = ppu_of(svg_res)
    first = next(e for e in entities if e["geometry"]["type"] == "line")
    mine = [l for l in el["line"] if l.get("data-id") == first["id"]]
    check(len(mine) == 1, f"線分 {first['id']} の要素: {len(mine)}")
    g = first["geometry"]
    expect = {
        "x1": (g["start"]["x"] - svg_res["view"]["min"]["x"]) * ppu,
        "y1": (svg_res["view"]["max"]["y"] - g["start"]["y"]) * ppu,
        "x2": (g["end"]["x"] - svg_res["view"]["min"]["x"]) * ppu,
        "y2": (svg_res["view"]["max"]["y"] - g["end"]["y"]) * ppu,
    }
    for key, want in expect.items():
        got = float(mine[0].get(key))
        check(abs(got - want) < 0.01, f"線分の {key}: SVG {got} != 独立に求めた {want}")
    # 円弧: 0.25 → 2.75 ラジアンの円弧の始点・終点が path の M と A の終点に一致する。
    arc_ent = next(e for e in entities if e["geometry"]["type"] == "arc")["geometry"]
    arc_path = next(p for p in paths if "A" in p.get("d", ""))
    cx, cy, rad = arc_ent["center"]["x"], arc_ent["center"]["y"], arc_ent["radius"]
    pt = lambda deg: (
        (cx + rad * math.cos(math.radians(deg)) - svg_res["view"]["min"]["x"]) * ppu,
        (svg_res["view"]["max"]["y"] - (cy + rad * math.sin(math.radians(deg)))) * ppu,
    )
    d = arc_path.get("d")
    m, a = d.split("A")
    sx, sy = map(float, m.removeprefix("M").split())
    rx, ry, rot, large, sweep, ex, ey = a.split()
    s_want, e_want = pt(arc_ent["start_angle"]), pt(arc_ent["end_angle"])
    check(abs(sx - s_want[0]) < 0.01 and abs(sy - s_want[1]) < 0.01, f"円弧の始点: {d}")
    check(abs(float(ex) - e_want[0]) < 0.01 and abs(float(ey) - e_want[1]) < 0.01, f"円弧の終点: {d}")
    check(abs(float(rx) - rad * ppu) < 0.01 and rx == ry, f"円弧の半径: {d}")
    # 14° → 158°（144°）なので large-arc は 0。モデルの反時計回りは画像でも反時計回り = sweep-flag 0。
    check((large, sweep) == ("0", "0"), f"円弧の large-arc / sweep フラグ: {d}")

    # --- 範囲の指定・背景・両方 ---
    zoom = c.call("render", {
        "format": "both", "width": 800, "height": 600, "background": "light",
        "region": {"min": [0, 0], "max": [8, 6]},
    })
    check(zoom.get("isError") is False, f"region つきの render: {zoom}")
    zs = zoom["structuredContent"]
    check([b["type"] for b in zoom["content"]] == ["text", "image", "text"], "both の content の並び")
    check_png(zoom["content"][1]["data"], 800, 600)
    check(zs["region_source"] == "given" and zs["background"] == "light", f"region / background: {zs}")
    check(math.isclose(zs["pixels_per_unit"], 100.0), f"8 × 6 を 800 × 600 に収めると 100 px/単位: {zs}")
    check(zs["view"] == {"min": {"x": 0.0, "y": 0.0}, "max": {"x": 8.0, "y": 6.0}}, f"view: {zs['view']}")
    check(zs["entities_outside_view"] >= 1, f"範囲の外の図形を数えていない: {zs}")
    zel = svg_elements(zoom["content"][2]["text"])
    check(zel["rect"][0].get("fill") == "#ffffff", "light の背景")
    # light の背景では白（ACI 7）の線を黒で描く。
    check(any(e.get("stroke") == "#000000" for tag in ("line", "path") for e in zel.get(tag, [])), "白の線を黒にしていない")

    # --- 拒否 ---
    c.fails("render", {"width": 4097}, "4096")
    c.fails("render", {"height": 5000}, "4096")
    c.fails("render", {"width": 0}, "width")
    c.fails("render", {"format": "jpeg"}, "png / svg / both")
    c.fails("render", {"region": {"min": [0, 0], "max": [0, 5]}}, "0 より大きく")
    c.fails("render", {"bogus": 1}, "bogus")


def info_after_open(c: "Client") -> dict:
    return c.ok("drawing_info", {})


def run(binary: Path, root: Path) -> list[Path]:
    sample_ymc = root / "sample.ymc"
    sample_dxf = root / "sample.dxf"
    for p in (sample_ymc, sample_dxf):
        check(p.is_file(), f"{p} がありません（write_sample で作ってください）")

    # 前の実行の出力を消す（残っていると上書きの確認で止まる）。
    for name in ("from_dxf.ymc", "roundtrip.ymc", "edited.ymc", "components.ymc"):
        (root / name).unlink(missing_ok=True)

    c = Client(binary, root)

    # --- 版の取り決め ------------------------------------------------------
    r = c.request("tools/list")
    check(r.get("error", {}).get("code") == -32600, f"initialize の前の tools/list を断らない: {r}")
    r = c.request(
        "initialize",
        {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "mcp_smoke", "version": "0"}},
    )
    result = r["result"]
    check(result["protocolVersion"] == "2025-11-25", f"版を取り決められない: {result}")
    check("tools" in result["capabilities"], "tools の能力を宣言していない")
    check(result["serverInfo"]["name"] == "ymcad-mcp", "serverInfo.name")
    c.send({"jsonrpc": "2.0", "method": "notifications/initialized"})

    r = c.request("no/such/method")
    check(r.get("error", {}).get("code") == -32601, f"知らないメソッドが -32601 でない: {r}")
    check_tool_list(c.request("tools/list")["result"]["tools"])

    # --- DXF を開いてネイティブで保存 -------------------------------------
    info = c.ok("open_drawing", {"path": "sample.dxf"})
    check(info["format"] == "dxf" and info["entity_count"] > 0, f"DXF を開けない: {info}")
    # 開いた .dxf へ path なしで保存しない（設計原則 9: DXF は交換用・非可逆）。
    c.fails("save_drawing", {}, ".ymc")
    # 図面名は d<起動の印（16 進）>-<通し番号>。起動の印はプロセスごとに違う。
    session, _, serial = info["drawing"][1:].partition("-")
    check(
        len(session) == 6 and all(ch in "0123456789abcdef" for ch in session) and serial == "2",
        f"図面の名前: {info}",
    )
    saved = c.ok("save_drawing", {"path": "from_dxf.ymc"})
    check(saved["format"] == "ymc", f"拡張子で形式が決まっていない: {saved}")

    # --- ネイティブを開いて照会し、保存 -----------------------------------
    info = c.ok("open_drawing", {"path": str(sample_ymc)})
    check(info["drawing"] == f"d{session}-3", f"図面の通し番号: {info}")
    drawing = info["drawing"]
    listing = c.ok("list_entities", {"limit": 1000})
    check(listing["total"] == info["entity_count"], "list_entities の total と entity_count が違う")
    ids = [e["id"] for e in listing["entities"]]
    check(all(i.startswith(f"{drawing}e") for i in ids), f"ID に図面の名前が入っていない: {ids}")
    got = c.ok("get_entities", {"ids": ids})
    types = sorted(e["geometry"]["type"] for e in got["entities"])
    # 図面に直接置かれた図形だけ（定義の中の図形は数えない。validate_ymc.py の --expect とは違う）。
    check(
        set(types) == {"arc", "circle", "instance", "line", "polyline", "xline"},
        f"write_sample の図形の種類と違う: {types}",
    )
    # 角度は度で出ること。write_sample の円弧は 0.25 → 2.75 ラジアン。
    # 変換は Rust の rad_to_deg とは別に Python の math.degrees で求めて比べる。
    arcs = [e["geometry"] for e in got["entities"] if e["geometry"]["type"] == "arc"]
    check(len(arcs) == 1, f"円弧が 1 つでない: {arcs}")
    check(
        math.isclose(arcs[0]["start_angle"], math.degrees(0.25))
        and math.isclose(arcs[0]["end_angle"], math.degrees(2.75)),
        f"円弧の角度が度で出ていない: {arcs[0]}",
    )
    c.fails("get_entities", {"ids": [f"d{session}-2e0g0"]}, "別の図面")
    other_session = f"{(int(session, 16) ^ 1):06x}"
    c.fails("get_entities", {"ids": [f"d{other_session}-3e0g0"]}, "つなぎ直す前")
    check_render(c, info_after_open(c), got["entities"])
    layers = c.ok("list_layers", {})["layers"]
    check(any(l["name"] == "0" for l in layers), f"レイヤ 0 が無い: {layers}")
    comps = c.ok("list_components", {})["components"]
    check(len(comps) == info["component_count"] and comps, f"コンポーネントの一覧: {comps}")

    c.fails("save_drawing", {"path": "sample.dxf"}, "overwrite")
    c.ok("save_drawing", {"path": "roundtrip.ymc"})
    c.ok("save_drawing", {})  # 前に保存したファイルへ（確認なし）

    # --- 安全策 -------------------------------------------------------------
    c.fails("open_drawing", {"path": "../sample.ymc"}, "..")
    outside = root.parent / f"{root.name}-mcp-smoke-outside.ymc"
    check(outside.parent != root, "作業ディレクトリに / は使えません")
    c.fails("save_drawing", {"path": str(outside)}, "root")
    c.fails("save_drawing", {"path": "note.txt"}, "拡張子")
    c.fails("new_drawing", {"bogus": 1}, "bogus")

    edit_session(c)
    components_session(c)

    r = c.request("tools/call", {"name": "no_such_tool", "arguments": {}})
    check(r.get("error", {}).get("code") == -32602, f"知らない道具が -32602 でない: {r}")

    code, rest, err = c.close()
    check(code == 0, f"終了コードが 0 でない: {code}\n{err.decode('utf-8', 'replace')}")
    check(rest == b"", f"stdout に余計な出力: {rest!r}")
    check(not outside.exists(), "root の外に書いた")

    roundtrip = root / "roundtrip.ymc"
    check(
        roundtrip.read_bytes() == sample_ymc.read_bytes(),
        "開いて保存しただけの .ymc が元とバイト単位で一致しない",
    )
    return [root / "from_dxf.ymc", roundtrip, root / "edited.ymc", root / "components.ymc"]


def validate(paths: list[Path]) -> None:
    for p in paths:
        args = [sys.executable, str(VALIDATE_YMC), str(p)]
        if p.name == "roundtrip.ymc":
            args += ["--expect", SAMPLE_EXPECT]
        if p.name == "edited.ymc":
            args += ["--expect", EDITED_EXPECT]
        if p.name == "components.ymc":
            args += ["--expect", COMPONENTS_EXPECT]
        done = subprocess.run(args, capture_output=True, text=True)
        check(done.returncode == 0, f"validate_ymc.py が {p} を不合格にした:\n{done.stdout}{done.stderr}")
        print(f"OK: validate_ymc.py {p.name}")


def main() -> int:
    if len(sys.argv) != 3:
        print("使い方: mcp_smoke.py <ymcad-mcp のパス> <sample.ymc と sample.dxf のあるディレクトリ>", file=sys.stderr)
        return 1
    binary = Path(sys.argv[1]).resolve()
    root = Path(sys.argv[2]).resolve()
    try:
        saved = run(binary, root)
        validate(saved)
    except SmokeError as e:
        print(f"NG: {e}", file=sys.stderr)
        return 1
    print("OK: MCP サーバーを通しで動かせました")
    return 0


if __name__ == "__main__":
    sys.exit(main())
