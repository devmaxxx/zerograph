#!/usr/bin/env python3
"""Render docs/assets/comparison.gif: one frame per measurement, zerograph against graphify and GitNexus.

The numbers come from docs/bench/2026-09-04-three-graphs-results.md (beauty-crm at 502e8a6d) and,
for the embedder and rerank rows, the 0.6.0 rows of bench/history/runs.jsonl. graphify's 14.6 M
build tokens are the cold build in docs/superpowers/specs/2026-09-01-zerograph-design.md; the
2026-09-04 results show 0 because that run built off a warm cache. Needs Google Chrome
and Pillow: `python3 scripts/comparison-gif.py`.
"""
import os
import subprocess
import sys
import tempfile

from PIL import Image, ImageChops

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "docs", "assets", "comparison.gif")
CHROME = os.environ.get("CHROME", "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome")
BG = (13, 17, 23)

Z0, Z1, Z2, R, G, N = "#1a7f37", "#2ea043", "#3fb950", "#7ee787", "#d29922", "#a371f7"
ZG = "<b>zerograph</b>"

# (title, hint, rows of (who, value, label, colour), lower_is_better, foot)
FRAMES = [
    ("Questions answered", "out of 82", [
        (f"{ZG} · no embedder", 58, "58", Z0),
        (f"{ZG} · e5-small", 61, "61", Z1),
        (f"{ZG} · gemma (default)", 72, "72", Z2),
        (f"{ZG} · gemma + rerank", 82, "82", R),
        ("graphify", 32, "32", G),
        ("GitNexus", 12, "12", N),
    ], False, "Rerank (sonnet) is opt-in and the only step that spends model tokens, about 14k a question."),
    ("LLM tokens to build the graph", "lower is better", [
        (ZG, 0, "0", Z2),
        ("graphify", 14.6, "14.6 M", G),
        ("GitNexus", 0, "0", N),
    ], True, "graphify, a cold build, reads every file with a model; zerograph and GitNexus build locally."),
    ("Answer size", "median characters, lower is better", [
        (ZG, 593, "593", Z2),
        ("graphify", 6688, "6 688", G),
        ("GitNexus", 6246, "6 246", N),
    ], True, "What an agent reads per question: about 150 tokens against 1.6k."),
    ("Answer time", "median ms, lower is better", [
        (ZG, 346, "346", Z2),
        ("graphify", 1467, "1 467", G),
        ("GitNexus", 1424, "1 424", N),
    ], True, "A fresh process per question; zerograph serve answers in 66 ms."),
    ("Impact: files found", "out of 123", [
        (ZG, 123, "123", Z2),
        ("graphify", 57, "57", G),
        ("GitNexus", 78, "78", N),
    ], False, "16 change targets: every file that reaches each one."),
    ("Call chains traced", "out of 8", [
        (ZG, 8, "8", Z2),
        ("graphify", 5, "5", G),
        ("GitNexus", 2, "2", N),
    ], False, "Shortest call chain between two symbols, including 2 correct refusals."),
    ("Size on disk", "MB, lower is better", [
        (ZG, 32, "32", Z2),
        ("graphify", 123, "123", G),
        ("GitNexus", 598, "598", N),
    ], True, "The whole index for one checkout."),
]

CSS = """
*{box-sizing:border-box;margin:0}
body{background:#0d1117;color:#e6edf3;font:18px/1.4 -apple-system,"SF Pro Text","Segoe UI",Inter,sans-serif;width:900px;padding:8px}
h1{font-size:34px;letter-spacing:-.5px}
h1 b{color:#3fb950}
.sub{color:#8b949e;margin:6px 0 20px;font-size:16px}
.card{background:#161b22;border:1px solid #30363d;border-radius:12px;padding:22px 24px}
.lbl{font-size:19px;font-weight:600;margin-bottom:8px}
.lbl span{color:#8b949e;font-weight:400;font-size:15px}
.row{display:grid;grid-template-columns:240px 1fr 80px;align-items:center;gap:12px;margin:6px 0}
.who{font-size:16px;color:#8b949e;white-space:nowrap}
.who b{color:#e6edf3;font-weight:500}
.track{height:22px;background:#21262d;border-radius:4px;overflow:hidden}
.bar{height:100%;border-radius:4px}
.val{font-size:18px;font-variant-numeric:tabular-nums;text-align:right;white-space:nowrap}
.best{color:#3fb950;font-weight:600}
.foot{color:#8b949e;font-size:14px;margin-top:14px}
"""


def page(title, hint, rows, lower, foot):
    top = max(v for _, v, _, _ in rows)
    best = min(v for _, v, _, _ in rows) if lower else top
    out = []
    for who, v, label, colour in rows:
        bar = f'<div class="bar" style="width:{100 * v / top:.1f}%;background:{colour}"></div>' if v else ""
        cls = "val best" if v == best else "val"
        out.append(f'<div class="row"><span class="who">{who}</span><div class="track">{bar}</div><span class="{cls}">{label}</span></div>')
    return (f'<!doctype html><html><head><meta charset="utf-8"><style>{CSS}</style></head><body>'
            f'<h1><b>zerograph</b> vs graphify vs GitNexus</h1><div class="sub">Same repository, same questions.</div>'
            f'<div class="card"><div class="lbl">{title} <span>{hint}</span></div>{"".join(out)}</div>'
            f'<div class="foot">{foot}</div></body></html>')


def shoot(html, tmp, i):
    src = os.path.join(tmp, f"{i}.html")
    png = os.path.join(tmp, f"{i}.png")
    with open(src, "w") as f:
        f.write(html)
    subprocess.run([CHROME, "--headless=new", "--disable-gpu", "--hide-scrollbars",
                    "--force-device-scale-factor=2", "--window-size=900,800",
                    f"--screenshot={png}", f"file://{src}"],
                   check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    im = Image.open(png).convert("RGB")
    box = ImageChops.difference(im, Image.new("RGB", im.size, BG)).getbbox()
    return im.crop((0, 0, im.size[0], box[3] + 16))


def main():
    with tempfile.TemporaryDirectory() as tmp:
        shots = [shoot(page(*frame), tmp, i) for i, frame in enumerate(FRAMES)]
    # One canvas height for every frame, so the README does not jump between them.
    h = max(s.size[1] for s in shots)
    frames = []
    for s in shots:
        canvas = Image.new("RGB", (s.size[0], h), BG)
        canvas.paste(s, (0, 0))
        frames.append(canvas.quantize(colors=64, method=Image.MEDIANCUT))
    frames[0].save(OUT, save_all=True, append_images=frames[1:], duration=2500, loop=0, optimize=True)
    print(OUT, frames[0].size, f"{os.path.getsize(OUT) // 1024} KiB", file=sys.stderr)


if __name__ == "__main__":
    main()
