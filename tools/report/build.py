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
/* 台帳: 細い列の本文＋上の目次。状態は色つきのピル、表は横にスクロールする箱に入れる */
:root {
  --bg: #f6f7f9; --surface: #ffffff; --fg: #1b2230; --muted: #5d6677; --line: #dde1e8;
  --accent: #2457a6; --code: #eceff4;
  --ok: #1d7a46; --ok-bg: #e3f3e9; --run: #8a5a00; --run-bg: #fbf0d9;
  --wait: #6b3fa0; --wait-bg: #efe7f8; --bug: #b3261e; --bug-bg: #fbe4e2; --idea-bg: #eceff4;
  --font-body: "Noto Sans JP", "Noto Sans CJK JP", "Hiragino Sans", system-ui, sans-serif;
  --font-mono: "JetBrains Mono", ui-monospace, "Noto Sans Mono CJK JP", monospace;
}
@media (prefers-color-scheme: dark) {
  :root:not([data-theme="light"]) {
    --bg: #12151b; --surface: #1a1e26; --fg: #e6e9ef; --muted: #9aa3b2; --line: #2b313c;
    --accent: #8fb4ff; --code: #232833;
    --ok: #6fd39a; --ok-bg: #173324; --run: #f0c060; --run-bg: #3a2d10;
    --wait: #c9a7f2; --wait-bg: #2e2340; --bug: #ff9a8f; --bug-bg: #3d1d1b; --idea-bg: #232833;
    color-scheme: dark;
  }
}
:root[data-theme="dark"] {
  --bg: #12151b; --surface: #1a1e26; --fg: #e6e9ef; --muted: #9aa3b2; --line: #2b313c;
  --accent: #8fb4ff; --code: #232833;
  --ok: #6fd39a; --ok-bg: #173324; --run: #f0c060; --run-bg: #3a2d10;
  --wait: #c9a7f2; --wait-bg: #2e2340; --bug: #ff9a8f; --bug-bg: #3d1d1b; --idea-bg: #232833;
  color-scheme: dark;
}
* { box-sizing: border-box; }
body { margin: 0; background: var(--bg); color: var(--fg); font: 15px/1.75 var(--font-body); }
main { max-width: 1000px; margin: 0 auto; padding-block: 28px 80px; padding-inline: 16px; }
header { display: flex; flex-wrap: wrap; align-items: baseline; justify-content: space-between; gap: 4px 16px;
  border-bottom: 1px solid var(--line); padding-bottom: 12px; margin-bottom: 8px; }
h1 { font-size: 1.55rem; margin: 0; letter-spacing: .02em; }
h2 { font-size: 1.2rem; margin: 44px 0 10px; text-wrap: balance; scroll-margin-top: 16px; }
h3 { font-size: 1rem; margin: 22px 0 8px; }
h4, h5 { font-size: .95rem; margin: 18px 0 6px; }
p, li { max-width: 72ch; }
.meta, .stamp, .lead, .sub, .note { color: var(--muted); }
.meta { font-size: .88rem; font-variant-numeric: tabular-nums; }
.lead { margin-top: 0; font-size: .92rem; }
.stamp { margin: -4px 0 8px; font-size: .88rem; }
.sub { font-size: .82rem; }
.toc { display: flex; flex-wrap: wrap; gap: 6px; margin: 8px 0 20px; }
.toc a { font-size: .85rem; padding: 2px 10px; border: 1px solid var(--line); border-radius: 999px; text-decoration: none; color: var(--fg); background: var(--surface); }
.toc a:hover, .toc a:focus-visible { border-color: var(--accent); color: var(--accent); }
.card { background: var(--surface); border: 1px solid var(--line); border-radius: 10px; padding: 14px 18px; }
.now h2 { margin-top: 0; }
.tablewrap { overflow-x: auto; margin: 8px 0; }
table { border-collapse: collapse; width: 100%; font-size: .9rem; background: var(--surface); }
th, td { border: 1px solid var(--line); padding: 6px 10px; text-align: left; vertical-align: top; }
th { background: var(--code); font-weight: 600; white-space: nowrap; }
td.num { font-variant-numeric: tabular-nums; color: var(--muted); text-align: right; }
td.note { font-size: .85rem; }
.judge td:nth-child(2) { white-space: nowrap; }
.area { font-weight: 600; }
.pill { display: inline-block; font-size: .78rem; font-weight: 600; padding: 0 9px; border-radius: 999px; white-space: nowrap; letter-spacing: .03em; }
.pill.done { color: var(--ok); background: var(--ok-bg); }
.pill.run { color: var(--run); background: var(--run-bg); }
.pill.wait { color: var(--wait); background: var(--wait-bg); }
.pill.bug { color: var(--bug); background: var(--bug-bg); }
.pill.core, .pill.idea { color: var(--muted); background: var(--idea-bg); }
.checkgrid { display: grid; grid-template-columns: repeat(auto-fit, minmax(280px, 1fr)); gap: 12px; }
.checkgroup { min-width: 0; background: var(--surface); border: 1px solid var(--line); border-radius: 10px; padding: 4px 14px 8px; }
.checkgroup h3 { margin: 10px 0 4px; font-size: .95rem; }
ul.checks { list-style: none; padding: 0; margin: 0; }
ul.checks li { position: relative; padding-left: 1.5em; margin: 4px 0; font-size: .9rem; }
ul.checks li::before { content: ""; position: absolute; left: 0; top: .45em; width: .85em; height: .85em; border: 1.5px solid var(--muted); border-radius: 3px; }
details { border: 1px solid var(--line); border-radius: 10px; background: var(--surface); margin: 8px 0; }
summary { cursor: pointer; padding: 10px 14px; font-weight: 600; }
summary:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
.detailbody { padding: 0 14px 12px; min-width: 0; overflow-x: auto; }
code { background: var(--code); padding: 1px 5px; border-radius: 4px; font: .86em var(--font-mono); }
pre { overflow-x: auto; }
figure { margin: 0; min-width: 0; }
figure img { width: 100%; height: auto; border: 1px solid var(--line); border-radius: 8px; display: block; }
figcaption { color: var(--muted); font-size: .85rem; margin-top: 4px; }
.grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(280px, 1fr)); gap: 14px; }
img { max-width: 100%; height: auto; }
.ok { color: var(--ok); font-weight: 600; } .ng { color: var(--bug); font-weight: 600; } .warn { color: var(--run); font-weight: 600; }
.tag { display: inline-block; font-size: .78rem; padding: 0 8px; border-radius: 999px; border: 1px solid var(--line); color: var(--muted); }
a { color: var(--accent); }
ol li, ul li { margin: 2px 0; }
@media (prefers-reduced-motion: no-preference) { html { scroll-behavior: smooth; } }
"""

# 本文と等幅の書体（Google Fonts。Artifact の CSP が許す唯一の書体の置き場所）。
FONTS = '<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Noto+Sans+JP:wght@400;600;700&family=JetBrains+Mono:wght@400&display=swap">'


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
<div class="meta">最終更新 {now}</div></header>
{body}
</main>"""
    if publish:
        # Artifact は公開時に doctype / head / body の外枠を付けるので、中身だけを書く
        page = f"<title>ymcad 作業報告書</title>\n{FONTS}\n<style>{STYLE}</style>\n{main_html}\n"
    else:
        page = f"""<!doctype html>
<html lang="ja"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>ymcad 作業報告書</title>{FONTS}<style>{STYLE}</style></head>
<body>{main_html}</body></html>
"""
    out.write_text(page, encoding="utf-8")
    print(f"wrote {out} ({out.stat().st_size // 1024} KB)")


if __name__ == "__main__":
    main()
