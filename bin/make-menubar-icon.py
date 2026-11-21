#!/usr/bin/env python3
"""Regenerate menubar_icon.png (36x36 = 18pt @2x template: black + alpha) and
app_icon.png (512x512 Dock/Finder icon) from bin/bonsai-source.png (black bonsai
silhouette on white, cropped to its bounds).
Needs Pillow. Run from anywhere; writes into the repo root."""
from PIL import Image, ImageDraw, ImageFilter
import os
HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.join(HERE, "..")
src = Image.open(os.path.join(HERE, "bonsai-source.png")).convert("L")

def menubar():
    FIT = 35  # 0.5px margin so edges are not clipped
    w, h = src.size
    k = FIT / max(w, h)
    r = src.resize((max(1, round(w * k)), max(1, round(h * k))), Image.LANCZOS)
    alpha = Image.eval(r, lambda v: 255 - v)
    a = Image.new("L", (36, 36), 0)
    a.paste(alpha, ((36 - alpha.width) // 2, (36 - alpha.height) // 2))
    out = Image.new("RGBA", (36, 36), (0, 0, 0, 0))
    out.putalpha(a)
    out.save(os.path.join(ROOT, "menubar_icon.png"))

def app_icon(px=512):
    S = 4  # supersample the tile edge
    big = px * S
    tile = Image.new("L", (big, big), 0)
    ImageDraw.Draw(tile).rounded_rectangle([0, 0, big - 1, big - 1], radius=int(big * 0.2237), fill=255)
    tile = tile.resize((px, px), Image.LANCZOS)
    bg = Image.new("RGBA", (px, px), (0x1b, 0x24, 0x3b, 255))   # deep indigo
    w, h = src.size
    sw = int(px * 0.66); sh = round(h * sw / w)
    scene = src.resize((sw, sh), Image.LANCZOS)
    ink = Image.new("RGBA", (sw, sh), (0xf3, 0xee, 0xe2, 255))   # washi white
    bg.paste(ink, ((px - sw) // 2, (px - sh) // 2), Image.eval(scene, lambda v: 255 - v))
    bg.putalpha(tile)
    bg.save(os.path.join(ROOT, "app_icon.png"))

menubar()
app_icon()
