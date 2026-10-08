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
5. root の外・.. を含むパス・上書きの確認が isError になること
6. stdin を閉じ、正常終了すること。stdout にプロトコル以外が出ていないこと
7. 保存したファイルを validate_ymc.py に通す

使い方
------
    cargo run -p cad-core --example write_sample -- DIR/sample.ymc
    cargo run -p cad-core --example write_sample -- DIR/sample.dxf
    python3 tools/mcp_smoke.py target/release/ymcad-mcp DIR

終了コード 0 で合格、1 で不合格。
"""

from __future__ import annotations

import json
import math
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
VALIDATE_YMC = HERE / "validate_ymc.py"

# write_sample の中身（CI の validate_ymc.py --expect と同じ）。
SAMPLE_EXPECT = "arc=1,xline=1,polyline=1,instance=3,circle=2"

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


def run(binary: Path, root: Path) -> list[Path]:
    sample_ymc = root / "sample.ymc"
    sample_dxf = root / "sample.dxf"
    for p in (sample_ymc, sample_dxf):
        check(p.is_file(), f"{p} がありません（write_sample で作ってください）")

    # 前の実行の出力を消す（残っていると上書きの確認で止まる）。
    for name in ("from_dxf.ymc", "roundtrip.ymc"):
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
    check(info["drawing"] == "d2", f"図面の通し番号: {info}")
    saved = c.ok("save_drawing", {"path": "from_dxf.ymc"})
    check(saved["format"] == "ymc", f"拡張子で形式が決まっていない: {saved}")

    # --- ネイティブを開いて照会し、保存 -----------------------------------
    info = c.ok("open_drawing", {"path": str(sample_ymc)})
    check(info["drawing"] == "d3", f"図面の通し番号: {info}")
    listing = c.ok("list_entities", {"limit": 1000})
    check(listing["total"] == info["entity_count"], "list_entities の total と entity_count が違う")
    ids = [e["id"] for e in listing["entities"]]
    check(all(i.startswith("d3e") for i in ids), f"ID に図面の番号が入っていない: {ids}")
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
    c.fails("get_entities", {"ids": ["d2e0g0"]}, "別の図面")
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
    return [root / "from_dxf.ymc", roundtrip]


def validate(paths: list[Path]) -> None:
    for p in paths:
        args = [sys.executable, str(VALIDATE_YMC), str(p)]
        if p.name == "roundtrip.ymc":
            args += ["--expect", SAMPLE_EXPECT]
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
