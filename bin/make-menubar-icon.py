#!/usr/bin/env python3
"""Regenerate menubar_icon.png (36x36 = 18pt @2x template: black + alpha), the
kitchen-sink stencil shown in the macOS menu bar. Needs Pillow. Run from the repo root."""
from PIL import Image, ImageDraw
import sys
S = 64  # px per point at render time
W = H = 18
def render(out_px, path):
    im = Image.new("L", (W*S, H*S), 0)
    d = ImageDraw.Draw(im)
    p = lambda x: int(x*S)
    box = lambda x0,y0,x1,y1: [p(x0),p(y0),p(x1),p(y1)]
    # counter top
    d.rounded_rectangle(box(0.8, 10.0, 17.2, 11.6), radius=p(0.5), fill=255)
    # basin (stencil gap under counter), rounded bowl
    d.rounded_rectangle(box(3.0, 12.4, 15.0, 17.0), radius=p(2.2), fill=255)
    d.rectangle(box(3.0, 12.4, 15.0, 14.0), fill=255)
    # drain hole cut-out
    d.ellipse(box(8.3, 14.2, 9.7, 15.6), fill=0)
    # faucet post
    d.rounded_rectangle(box(11.6, 4.2, 13.2, 9.4), radius=p(0.3), fill=255)
    # base plate
    d.rounded_rectangle(box(10.6, 8.6, 14.2, 9.4), radius=p(0.3), fill=255)
    # gooseneck: thick arc from post top curving left and down
    t = 1.6
    cx0, cy0, cx1, cy1 = 6.2, 1.2, 13.2, 8.2   # arc bounding circle
    d.arc(box(cx0, cy0, cx1, cy1), start=180, end=360, fill=255, width=p(t))
    # spout tip going down on the left
    d.rounded_rectangle(box(6.2, 4.4, 7.8, 7.0), radius=p(0.3), fill=255)
    # handle knob
    d.rounded_rectangle(box(13.0, 5.6, 16.4, 6.8), radius=p(0.5), fill=255)
    # water drop
    d.ellipse(box(6.5, 7.6, 7.5, 8.8), fill=255)
    out = Image.new("RGBA", im.size, (0,0,0,0))
    out.putalpha(im)
    out = out.resize((out_px, out_px), Image.LANCZOS)
    out.save(path)
render(36, "menubar_icon.png")
