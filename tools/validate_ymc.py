#!/usr/bin/env python3
"""ymcad のネイティブ形式（.ymc）を Rust とは独立に検査する。

なぜ必要か
----------
ラウンドトリップテストは「自分で書いて自分で読む」ため、**書き出し側のバグを
見逃す**。書き手と読み手が同じ誤解をしていれば往復は成立してしまう。
`validate_dxf_r12.py` がこの盲点を埋めるために存在するのと同じ理由で、
ネイティブ形式にもこのスクリプトが必要になる。

**バイナリではこの盲点はテキストより深い。** DXF なら保存したファイルを
エディタで開けば構造が目で見えるが、バイナリは開いても分からない。

いちばん効く検査は「**ファイル末尾でぴったり尽きること**」。
書き出し漏れ・過剰があれば、余りバイトか不足として必ず露見する。

使い方
------
    python3 tools/validate_ymc.py drawing.ymc
    python3 tools/validate_ymc.py drawing.ymc --expect line=3,arc=1,xline=1
    python3 tools/validate_ymc.py drawing.ymc --verbose

終了コード 0 で合格、1 で不合格。
"""

from __future__ import annotations

import argparse
import struct
import sys
from pathlib import Path

# crates/cad-core/src/native/mod.rs と一致していること。
MAGIC = b"YMCAD\x1a\0\0"
MAX_KNOWN_VERSION = 2
# コンポーネント定義セクションが入った最初の版。
VERSION_WITH_COMPONENTS = 2

KIND_NAMES = {0: "line", 1: "circle", 2: "arc", 3: "xline", 4: "polyline", 5: "instance"}
LINETYPE_NAMES = {0: "continuous", 1: "dashed", 2: "center", 3: "hidden"}

COLOR_BY_LAYER = 0
COLOR_ACI = 1
OPTION_NONE = 0
OPTION_SOME = 1

FLAG_VISIBLE = 1 << 0
FLAG_LOCKED = 1 << 1

# 配置のフラグ。
PLACEMENT_FLIPPED = 1 << 0

# パラメータ値の種別。
VALUE_NUMBER = 0
VALUE_BOOL = 1
VALUE_CHOICE = 2

# 図形ごとの固定長ペイロードに含まれる f64 の個数。
# polyline と instance は可変長なので別扱い。
FIXED_F64_COUNT = {0: 4, 1: 3, 2: 5, 3: 4}


class ValidationError(Exception):
    """検査に失敗した。"""


class Cursor:
    """バイト列を前から読み進める。範囲外は必ず例外にする。"""

    def __init__(self, data: bytes) -> None:
        self.data = data
        self.pos = 0

    def take(self, n: int) -> bytes:
        if n < 0:
            raise ValidationError(f"{self.pos} バイト目: 負の長さ {n}")
        end = self.pos + n
        if end > len(self.data):
            raise ValidationError(
                f"{self.pos} バイト目: ファイルが途中で終わっています"
                f"（{n} バイト必要、残り {len(self.data) - self.pos} バイト）"
            )
        out = self.data[self.pos : end]
        self.pos = end
        return out

    def u8(self) -> int:
        return self.take(1)[0]

    def u32(self) -> int:
        return struct.unpack("<I", self.take(4))[0]

    def f64(self) -> float:
        return struct.unpack("<d", self.take(8))[0]

    def string(self) -> str:
        length = self.u32()
        raw = self.take(length)
        try:
            return raw.decode("utf-8")
        except UnicodeDecodeError as e:
            raise ValidationError(
                f"{self.pos - length} バイト目: UTF-8 として解釈できません"
            ) from e

    def remaining(self) -> int:
        return len(self.data) - self.pos


def validate(data: bytes, verbose: bool = False) -> dict[str, int]:
    """形式を検査し、図形種別ごとの件数を返す。"""
    c = Cursor(data)

    magic = c.take(len(MAGIC))
    if magic != MAGIC:
        raise ValidationError(
            f"識別子が違います: {magic!r}（期待 {MAGIC!r}）"
        )

    version = c.u32()
    if version == 0:
        raise ValidationError("形式バージョンが 0 です")
    if version > MAX_KNOWN_VERSION:
        raise ValidationError(
            f"未知の形式バージョン {version}"
            f"（このスクリプトが知っているのは {MAX_KNOWN_VERSION} まで）"
        )
    if verbose:
        print(f"形式バージョン: {version}")

    # ---- レイヤ表 ----
    layer_count = c.u32()
    layer_names: list[str] = []
    for i in range(layer_count):
        name = c.string()
        color = c.u8()
        flags = c.u8()
        linetype = c.u8()

        if linetype not in LINETYPE_NAMES:
            raise ValidationError(f"レイヤ {i}（{name}）: 未知の線種 {linetype}")
        # 未定義ビットが立っていたら、書き手と読み手の解釈がずれている兆候。
        unknown = flags & ~(FLAG_VISIBLE | FLAG_LOCKED)
        if unknown:
            raise ValidationError(
                f"レイヤ {i}（{name}）: 未定義のフラグビット {unknown:#04x}"
            )
        if name in layer_names:
            raise ValidationError(f"レイヤ名が重複しています: {name}")
        layer_names.append(name)

        if verbose:
            print(
                f"  レイヤ[{i}] {name!r} 色={color} "
                f"表示={bool(flags & FLAG_VISIBLE)} ロック={bool(flags & FLAG_LOCKED)} "
                f"線種={LINETYPE_NAMES[linetype]}"
            )

    if layer_count == 0:
        raise ValidationError("レイヤが 1 つもありません（レイヤ 0 は必ず存在するはず）")
    if "0" not in layer_names:
        raise ValidationError('既定レイヤ "0" がありません')

    # ---- グループ表 ----
    group_count = c.u32()
    group_names: list[str] = []
    for i in range(group_count):
        name = c.string()
        if name in group_names:
            raise ValidationError(f"グループ名が重複しています: {name}")
        group_names.append(name)
        if verbose:
            print(f"  グループ[{i}] {name!r}")

    # ---- エンティティ ----
    counts: dict[str, int] = {name: 0 for name in KIND_NAMES.values()}
    used_groups: set[int] = set()
    used_definitions: set[int] = set()

    entity_count = c.u32()
    for i in range(entity_count):
        read_entity(c, f"エンティティ {i}", layer_count, group_count,
                    counts, used_groups, used_definitions)

    # ---- コンポーネント定義（形式 v2 以降） ----
    #
    # 定義はエンティティより後ろにある。v1 のファイルはここが無いだけなので、
    # バージョンで分岐すれば前半をそのまま読める。
    definition_count = 0
    definition_names: list[str] = []
    if version >= VERSION_WITH_COMPONENTS:
        definition_count = c.u32()
        for i in range(definition_count):
            name = c.string()
            if name in definition_names:
                raise ValidationError(f"コンポーネント定義名が重複しています: {name}")
            definition_names.append(name)
            c.f64()  # 基点 x
            c.f64()  # 基点 y
            n = c.u32()
            for j in range(n):
                read_entity(c, f"定義 {i}（{name}）の要素 {j}", layer_count,
                            group_count, counts, used_groups, used_definitions)
            if verbose:
                print(f"  定義[{i}] {name!r} 要素 {n} 件")

    # 定義の参照が範囲内であること。**前方参照があるので最後にまとめて検査する。**
    for index in sorted(used_definitions):
        if index >= definition_count:
            raise ValidationError(
                f"コンポーネント定義の参照が範囲外 {index}（定義数 {definition_count}）"
            )

    # ---- 最も効く検査: 末尾でぴったり尽きること ----
    if c.remaining() != 0:
        raise ValidationError(
            f"{c.remaining()} バイトが読み残されています。"
            "書き出しと読み込みでレイアウトの解釈がずれています"
        )

    # メンバーのいないグループは Rust 側が復元しないので、あれば書き出しの無駄。
    orphans = [group_names[i] for i in range(group_count) if i not in used_groups]
    if orphans:
        print(
            f"警告: メンバーのいないグループが書かれています（読み戻すと消えます）: {orphans}",
            file=sys.stderr,
        )

    if verbose:
        print(f"エンティティ {entity_count} 件（定義の中身を含む内訳）: {counts}")
    return counts


def read_entity(
    c: Cursor,
    where: str,
    layer_count: int,
    group_count: int,
    counts: dict[str, int],
    used_groups: set[int],
    used_definitions: set[int],
) -> None:
    """エンティティ 1 件を読んで検査する。

    図面直下の要素と定義の中身で**同じ形**なので、同じ関数で読む。
    分けると片方だけ直し忘れる。
    """
    kind = c.u8()
    if kind not in KIND_NAMES:
        raise ValidationError(f"{where}: 未知の図形種別 {kind}")
    name = KIND_NAMES[kind]
    counts[name] += 1

    if kind == 4:  # polyline
        closed = c.u8()
        if closed not in (0, 1):
            raise ValidationError(f"{where}: closed が 0/1 ではありません（{closed}）")
        vertex_count = c.u32()
        pairs = [(c.f64(), c.f64()) for _ in range(vertex_count)]
        flat = [v for pair in pairs for v in pair]
    elif kind == 5:  # instance
        def_index = c.u32()
        used_definitions.add(def_index)
        # 基点 2 + 回転 1 + 倍率 1。
        flat = [c.f64() for _ in range(4)]
        scale = flat[3]
        if not (scale > 0.0):
            raise ValidationError(f"{where}: 配置の倍率が正ではありません（{scale}）")
        flags = c.u8()
        unknown = flags & ~PLACEMENT_FLIPPED
        if unknown:
            raise ValidationError(f"{where}: 配置に未定義のフラグビット {unknown:#04x}")
        # パラメータの個別上書き。
        n = c.u32()
        seen: set[str] = set()
        for _ in range(n):
            key = c.string()
            if key in seen:
                raise ValidationError(f"{where}: パラメータ上書きの名前が重複 {key!r}")
            seen.add(key)
            tag = c.u8()
            if tag == VALUE_NUMBER:
                c.f64()
            elif tag == VALUE_BOOL:
                b = c.u8()
                if b not in (0, 1):
                    raise ValidationError(f"{where}: 真偽値が 0/1 ではありません（{b}）")
            elif tag == VALUE_CHOICE:
                c.string()
            else:
                raise ValidationError(f"{where}: 未知のパラメータ値の種別 {tag}")
    else:
        flat = [c.f64() for _ in range(FIXED_F64_COUNT[kind])]

    # 座標に NaN / Inf が入っていたら、どこかで壊れている。
    for v in flat:
        if v != v or v in (float("inf"), float("-inf")):
            raise ValidationError(f"{where}（{name}）: 座標が有限ではありません")

    # 半径は正であること。
    if kind in (1, 2) and flat[2] <= 0.0:
        raise ValidationError(f"{where}（{name}）: 半径が正ではありません（{flat[2]}）")

    # 作図線の方向は単位ベクトルであること（Rust 側の不変条件と同じ検査）。
    if kind == 3:
        dx, dy = flat[2], flat[3]
        length = (dx * dx + dy * dy) ** 0.5
        if abs(length - 1.0) > 1e-9:
            raise ValidationError(
                f"{where}（作図線）: 方向が単位ベクトルではありません（長さ {length}）"
            )

    layer_index = c.u32()
    if layer_index >= layer_count:
        raise ValidationError(
            f"{where}: レイヤ参照が範囲外 {layer_index}（レイヤ数 {layer_count}）"
        )

    color_tag = c.u8()
    if color_tag == COLOR_ACI:
        c.u8()
    elif color_tag != COLOR_BY_LAYER:
        raise ValidationError(f"{where}: 未知の色指定 {color_tag}")

    group_tag = c.u8()
    if group_tag == OPTION_SOME:
        group_index = c.u32()
        if group_index >= group_count:
            raise ValidationError(
                f"{where}: グループ参照が範囲外 {group_index}（グループ数 {group_count}）"
            )
        used_groups.add(group_index)
    elif group_tag != OPTION_NONE:
        raise ValidationError(f"{where}: 未知のグループ指定 {group_tag}")


def parse_expect(text: str) -> dict[str, int]:
    """`line=3,arc=1` 形式を辞書にする。"""
    out: dict[str, int] = {}
    for part in text.split(","):
        part = part.strip()
        if not part:
            continue
        if "=" not in part:
            raise ValidationError(f"--expect の書式が不正です: {part!r}（name=count 形式）")
        name, _, value = part.partition("=")
        name = name.strip()
        if name not in KIND_NAMES.values():
            raise ValidationError(
                f"--expect に未知の図形種別: {name!r}（{sorted(KIND_NAMES.values())} のいずれか）"
            )
        out[name] = int(value)
    return out


def main() -> int:
    parser = argparse.ArgumentParser(
        description="ymcad のネイティブ形式（.ymc）を検査する"
    )
    parser.add_argument("path", type=Path, help="検査する .ymc ファイル")
    parser.add_argument(
        "--expect",
        help="期待する図形の内訳（例: line=3,arc=1）。指定した種別だけ照合する",
    )
    parser.add_argument("--verbose", action="store_true", help="中身を表示する")
    args = parser.parse_args()

    try:
        data = args.path.read_bytes()
    except OSError as e:
        print(f"NG: ファイルを読めません: {e}", file=sys.stderr)
        return 1

    try:
        counts = validate(data, verbose=args.verbose)
        if args.expect:
            want = parse_expect(args.expect)
            for name, n in want.items():
                if counts[name] != n:
                    raise ValidationError(
                        f"{name} の件数が合いません: 期待 {n}、実際 {counts[name]}"
                    )
    except ValidationError as e:
        print(f"NG: {e}", file=sys.stderr)
        return 1

    total = sum(counts.values())
    print(f"OK: {args.path} は妥当な .ymc です（{len(data)} バイト、図形 {total} 件）")
    return 0


if __name__ == "__main__":
    sys.exit(main())
