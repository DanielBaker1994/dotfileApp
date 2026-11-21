#!/usr/bin/env python3
"""Regenerate menubar_icon.png (36x36 = 18pt @2x template: black + alpha), the
bonsai silhouette shown in the macOS menu bar. Source art: bin/bonsai-source.png
(black tree on white, cropped to its bounds). Needs Pillow. Run from the repo root."""
from PIL import Image
import os
HERE = os.path.dirname(os.path.abspath(__file__))
OUT_PX = 36
FIT = 35  # leave a 0.5px margin so edges are not clipped
src = Image.open(os.path.join(HERE, "bonsai-source.png")).convert("L")
w, h = src.size
s = FIT / max(w, h)
src = src.resize((max(1, round(w * s)), max(1, round(h * s))), Image.LANCZOS)
alpha = Image.eval(src, lambda v: 255 - v)
out = Image.new("RGBA", (OUT_PX, OUT_PX), (0, 0, 0, 0))
a = Image.new("L", (OUT_PX, OUT_PX), 0)
a.paste(alpha, ((OUT_PX - alpha.width) // 2, (OUT_PX - alpha.height) // 2))
out.putalpha(a)
out.save(os.path.join(HERE, "..", "menubar_icon.png"))
