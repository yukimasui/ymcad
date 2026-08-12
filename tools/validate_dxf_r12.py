#!/usr/bin/env python3
"""書き出した DXF が R12 (AC1009) として妥当かを独立に検証する。

`cad-core` のラウンドトリップテストは「自分で書いて自分で読む」ため、
自分の書き出しにバグがあっても通ってしまう。このスクリプトは
Rust の実装とは**別のロジック**で構造を検査し、その盲点を埋める。

LibreCAD / QCAD で開けるかの最終確認の代わりにはならないが、
ビューアが無い環境でも回せる。

使い方:
    cargo run --release            # アプリで図面を保存する
    python3 tools/validate_dxf_r12.py path/to/drawing.dxf
"""

import sys
from collections import Counter
from pathlib import Path

# R12 (AC1009) には存在しないエンティティ。書き出していたら誤り。
NOT_IN_R12 = {
    "LWPOLYLINE", "ELLIPSE", "SPLINE", "MTEXT",
    "HATCH", "REGION", "LEADER", "TOLERANCE",
}
KNOWN_SECTIONS = {"HEADER", "TABLES", "ENTITIES", "BLOCKS", "CLASSES", "OBJECTS"}
STRUCTURAL = {"SECTION", "ENDSEC", "TABLE", "ENDTAB", "EOF"}


def validate(path: Path) -> tuple[list[str], list[str], dict]:
    """(エラー, 警告, 概要) を返す。"""
    errors: list[str] = []
    warnings: list[str] = []
    lines = path.read_text(errors="replace").splitlines()

    # 1. すべての値は「グループコード行 + 値行」の対
    if len(lines) % 2 != 0:
        errors.append("行数が奇数。コードと値の対になっていない")
    pairs = [(lines[i].strip(), lines[i + 1]) for i in range(0, len(lines) - 1, 2)]
    for i, (code, _) in enumerate(pairs):
        if not code.lstrip("-").isdigit():
            errors.append(f"対 {i}: グループコード {code!r} が数値でない")

    # 2. SECTION / ENDSEC の対応
    depth = 0
    sections: list[str] = []
    for code, val in pairs:
        if code == "0" and val == "SECTION":
            depth += 1
        elif code == "0" and val == "ENDSEC":
            depth -= 1
            if depth < 0:
                errors.append("ENDSEC が SECTION より多い")
        elif code == "2" and depth == 1 and val in KNOWN_SECTIONS:
            sections.append(val)
    if depth != 0:
        errors.append(f"SECTION と ENDSEC が釣り合っていない (depth={depth})")
    for required in ("HEADER", "TABLES", "ENTITIES"):
        if required not in sections:
            errors.append(f"必須セクション {required} が無い")

    # 3. 終端
    if lines[-2:] != ["0", "EOF"]:
        errors.append("ファイルが '0' / 'EOF' で終わっていない")

    # 4. ヘッダ変数
    header = {}
    for i, (code, val) in enumerate(pairs):
        if code == "9" and i + 1 < len(pairs):
            header[val] = pairs[i + 1][1]
    if header.get("$ACADVER") != "AC1009":
        errors.append(f"$ACADVER が AC1009 でない: {header.get('$ACADVER')!r}")
    for var in ("$INSBASE", "$EXTMIN", "$EXTMAX"):
        if var not in header:
            warnings.append(f"ヘッダ変数 {var} が無い。拒否する読み手がある")

    # 5. R12 に無いエンティティ
    entities = [v for c, v in pairs if c == "0"]
    banned = NOT_IN_R12 & set(entities)
    if banned:
        errors.append(f"R12 に存在しないエンティティを書き出している: {sorted(banned)}")

    # 6. POLYLINE / VERTEX / SEQEND の入れ子
    i = 0
    while i < len(entities):
        if entities[i] == "POLYLINE":
            j = i + 1
            verts = 0
            while j < len(entities) and entities[j] == "VERTEX":
                verts += 1
                j += 1
            if j >= len(entities) or entities[j] != "SEQEND":
                errors.append(f"POLYLINE (位置 {i}) に SEQEND が続いていない")
            if verts < 2:
                errors.append(f"POLYLINE (位置 {i}) の VERTEX が {verts} 個しかない")
            i = j
        i += 1

    # 7. レイヤ表と参照の整合
    declared: set[str] = set()
    referenced: set[str] = set()
    in_layer_table = False
    for code, val in pairs:
        if code == "2" and val == "LAYER":
            in_layer_table = True
            continue
        if code == "0" and val == "ENDTAB":
            in_layer_table = False
        if in_layer_table and code == "2":
            declared.add(val)
        if not in_layer_table and code == "8":
            referenced.add(val)
    missing = referenced - declared
    if missing:
        errors.append(f"レイヤ表に無いレイヤを参照している: {sorted(missing)}")

    # 8. R12 で安全なレイヤ名か
    for name in sorted(declared):
        if name != name.upper() or " " in name:
            warnings.append(f"レイヤ名 {name!r} が R12 で安全な形でない（大文字・空白なし）")

    summary = {
        "lines": len(lines),
        "sections": sections,
        "entities": dict(Counter(e for e in entities if e not in STRUCTURAL)),
        "declared_layers": sorted(declared),
        "referenced_layers": sorted(referenced),
    }
    return errors, warnings, summary


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__)
        return 2
    path = Path(sys.argv[1])
    if not path.is_file():
        print(f"ファイルがありません: {path}")
        return 2

    errors, warnings, summary = validate(path)

    print(f"検証対象: {path} ({summary['lines']} 行)")
    print(f"セクション: {summary['sections']}")
    print(f"エンティティ: {summary['entities']}")
    print(f"宣言レイヤ: {summary['declared_layers']}")
    print(f"参照レイヤ: {summary['referenced_layers']}")
    print()

    if errors:
        print("❌ エラー:")
        for e in errors:
            print("  -", e)
    else:
        print("✅ R12 構造の検証をすべて通過")
    if warnings:
        print("⚠ 警告:")
        for w in warnings:
            print("  -", w)
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
