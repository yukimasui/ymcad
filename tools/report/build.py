#!/usr/bin/env python3
"""作業報告書を HTML に組み立てる（使い方は README.md）。

- 既定: content.html（本文の断片）中の <img src="img/..."> を base64 で埋め込み、
  1 ファイルの report.html を書き出す（手元で開く・ファイルで渡す用）
- --publish: 画像を埋め込まず img/ を相対参照したままの report.publish.html を書き出す。
  claude.ai の Artifact に、このページと img/*.png を別ファイルとして公開する。
  Artifact は渡さなかったファイルを残すので、画像の無い別のマシンで本文だけ
  更新しても、前の画像は消えない
"""
import base64
import datetime
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent
CONTENT = ROOT / "content.html"
OUT = ROOT / "report.html"

STYLE = """
:root {
  --bg: #fbfaf8; --fg: #1d1d1f; --muted: #6b6b70; --line: #e3e1dc;
  --card: #ffffff; --accent: #2f6fdb; --ok: #1f8a4c; --ng: #c2412d; --warn: #a86b00;
  --code: #f2f0ec;
}
@media (prefers-color-scheme: dark) {
  :root:not([data-theme="light"]) {
    --bg: #141416; --fg: #e9e9ec; --muted: #9a9aa2; --line: #2c2c31;
    --card: #1c1c20; --accent: #7aa7ff; --ok: #4cc27f; --ng: #ff7a66; --warn: #f0b44c;
    --code: #24242a; color-scheme: dark;
  }
}
:root[data-theme="dark"] {
  --bg: #141416; --fg: #e9e9ec; --muted: #9a9aa2; --line: #2c2c31;
  --card: #1c1c20; --accent: #7aa7ff; --ok: #4cc27f; --ng: #ff7a66; --warn: #f0b44c;
  --code: #24242a; color-scheme: dark;
}
* { box-sizing: border-box; }
body { margin: 0; background: var(--bg); color: var(--fg);
  font: 15px/1.75 "Noto Sans CJK JP", "Hiragino Sans", system-ui, sans-serif; }
main { max-width: 980px; margin: 0 auto; padding: 32px 16px 80px; }
header { border-bottom: 1px solid var(--line); margin-bottom: 24px; padding-bottom: 12px; }
h1 { font-size: 1.6rem; margin: 0 0 4px; }
h2 { font-size: 1.25rem; margin: 40px 0 12px; padding-top: 8px; border-top: 1px solid var(--line); }
h3 { font-size: 1.05rem; margin: 24px 0 8px; }
.meta { color: var(--muted); font-size: .9rem; }
.card { background: var(--card); border: 1px solid var(--line); border-radius: 10px; padding: 14px 18px; margin: 12px 0; }
table { border-collapse: collapse; width: 100%; margin: 8px 0; font-size: .93rem; display: block; overflow-x: auto; }
img { max-width: 100%; height: auto; }
th, td { border: 1px solid var(--line); padding: 6px 10px; text-align: left; vertical-align: top; }
th { background: var(--code); }
code { background: var(--code); padding: 1px 5px; border-radius: 4px; font-size: .9em; }
figure { margin: 14px 0; }
figure img { width: 100%; height: auto; border: 1px solid var(--line); border-radius: 8px; display: block; }
figcaption { color: var(--muted); font-size: .88rem; margin-top: 4px; }
.grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(300px, 1fr)); gap: 14px; }
.ok { color: var(--ok); font-weight: 600; } .ng { color: var(--ng); font-weight: 600; }
.warn { color: var(--warn); font-weight: 600; }
.tag { display: inline-block; font-size: .78rem; padding: 0 8px; border-radius: 999px; border: 1px solid var(--line); color: var(--muted); }
a { color: var(--accent); }
ol li, ul li { margin: 2px 0; }
"""


def inline_images(html: str) -> str:
    def repl(m: re.Match) -> str:
        path = ROOT / m.group(1)
        if not path.exists():
            return f'src="" data-missing="{m.group(1)}"'
        data = base64.b64encode(path.read_bytes()).decode()
        return f'src="data:image/png;base64,{data}"'

    return re.sub(r'src="(img/[^"]+\.png)"', repl, html)


def main() -> None:
    publish = "--publish" in sys.argv[1:]
    body = CONTENT.read_text(encoding="utf-8")
    if not publish:
        body = inline_images(body)
    out = ROOT / "report.publish.html" if publish else OUT
    now = datetime.datetime.now().strftime("%Y-%m-%d %H:%M")
    main_html = f"""<main>
<header><h1>ymcad 作業報告書</h1>
<div class="meta">最終更新: {now}</div></header>
{body}
</main>"""
    if publish:
        # Artifact は公開時に doctype / head / body の外枠を付けるので、中身だけを書く
        page = f"<title>ymcad 作業報告書</title>\n<style>{STYLE}</style>\n{main_html}\n"
    else:
        page = f"""<!doctype html>
<html lang="ja"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>ymcad 作業報告書</title><style>{STYLE}</style></head>
<body>{main_html}</body></html>
"""
    out.write_text(page, encoding="utf-8")
    print(f"wrote {out} ({out.stat().st_size // 1024} KB)")


if __name__ == "__main__":
    main()
