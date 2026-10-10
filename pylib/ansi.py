"""ANSI parser/grid/theme: text -> styled cells, plus the xterm256 and
ghostty theme tables.

The CoreText renderer stays in Swift (per-cell drawing); the theme table is
kept here as the tested reference and for future frontends. The Swift
suite's parser cases moved here (ws test ansi).
"""
from __future__ import annotations

import json
import os
import re
import unicodedata

_DATA_FILE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "ansi.json")
# the ghostty-mirror theme defaults + the base-16 palette are data;
# the xterm cube / grays are generated
with open(_DATA_FILE, encoding="utf-8") as _fh:
    _DATA = json.load(_fh)

NONE = ("none",)


def index(n: int):
    return ("index", n)


def rgb(r: int, g: int, b: int):
    return ("rgb", (r, g, b))


class RGB:
    __slots__ = ("r", "g", "b")

    def __init__(self, r, g, b):
        self.r, self.g, self.b = int(r), int(g), int(b)

    @classmethod
    def from_hex(cls, s):
        s = (s or "").strip()
        if s.startswith("#"):
            s = s[1:]
        if len(s) != 6:
            return None
        try:
            v = int(s, 16)
        except ValueError:
            return None
        return cls((v >> 16) & 0xFF, (v >> 8) & 0xFF, v & 0xFF)

    def __eq__(self, o):
        return isinstance(o, RGB) and (self.r, self.g, self.b) == (o.r, o.g, o.b)

    def __repr__(self):
        return "RGB(%d, %d, %d)" % (self.r, self.g, self.b)


_BASE16 = [RGB.from_hex(h) for h in _DATA["base16"]]


class Style:
    __slots__ = ("fg", "bg", "bold", "dim", "italic", "underline", "inverse", "strike")

    def __init__(self, fg=NONE, bg=NONE, bold=False, dim=False, italic=False,
                 underline=False, inverse=False, strike=False):
        self.fg, self.bg = fg, bg
        self.bold, self.dim, self.italic = bold, dim, italic
        self.underline, self.inverse, self.strike = underline, inverse, strike

    def copy(self):
        return Style(self.fg, self.bg, self.bold, self.dim, self.italic,
                     self.underline, self.inverse, self.strike)

    def __eq__(self, o):
        return isinstance(o, Style) and all(
            getattr(self, k) == getattr(o, k) for k in Style.__slots__)

    def __repr__(self):
        return "Style(fg=%r, bg=%r, bold=%s, dim=%s, italic=%s, underline=%s, inverse=%s, strike=%s)" % (
            self.fg, self.bg, self.bold, self.dim, self.italic,
            self.underline, self.inverse, self.strike)


class Cell:
    __slots__ = ("text", "style", "width")

    def __init__(self, text, style, width):
        self.text, self.style, self.width = text, style, width

    def __eq__(self, o):
        return (isinstance(o, Cell) and self.text == o.text and self.width == o.width
                and self.style == o.style)


class Grid:
    def __init__(self):
        self.rows = []

    @property
    def columns(self) -> int:
        return max((len(r) for r in self.rows), default=0)


def _graphemes(text: str):
    out = []
    for ch in text:
        if out and (out[-1].endswith("\u200d") or unicodedata.combining(ch) or ch == "\ufe0f"):
            out[-1] += ch
        else:
            out.append(ch)
    return out


def cell_width(c: str) -> int:
    if any(ord(ch) == 0xFE0F for ch in c):
        return 2
    if not c:
        return 1
    first = ord(c[0])
    if 0x1F300 <= first <= 0x1FAFF:
        return 2
    for lo, hi in ((0x1100, 0x115F), (0x2E80, 0x303E), (0x3041, 0x33FF), (0x3400, 0x4DBF),
                   (0x4E00, 0x9FFF), (0xA000, 0xA4CF), (0xAC00, 0xD7A3), (0xF900, 0xFAFF),
                   (0xFE30, 0xFE4F), (0xFF00, 0xFF60), (0xFFE0, 0xFFE6), (0x20000, 0x3FFFD)):
        if lo <= first <= hi:
            return 2
    return 1


def apply_sgr(params: str, s: Style) -> None:
    codes = []
    for x in re.split(r"[;:]", params):
        try:
            codes.append(int(x))
        except ValueError:
            codes.append(0)
    if not codes:
        codes = [0]
    i = 0
    n = len(codes)

    def extended():
        nonlocal i
        if i + 1 >= n:
            return None
        if codes[i + 1] == 5 and i + 2 < n:
            v = max(0, min(255, codes[i + 2]))
            i += 2
            return index(v)
        if codes[i + 1] == 2 and i + 4 < n:
            def c(v):
                return max(0, min(255, v))
            v = rgb(c(codes[i + 2]), c(codes[i + 3]), c(codes[i + 4]))
            i += 4
            return v
        return None

    while i < n:
        c = codes[i]
        if c == 0:
            s.fg = s.bg = NONE
            s.bold = s.dim = s.italic = s.underline = s.inverse = s.strike = False
        elif c == 1:
            s.bold = True
        elif c == 2:
            s.dim = True
        elif c == 3:
            s.italic = True
        elif c == 4:
            s.underline = True
        elif c == 7:
            s.inverse = True
        elif c == 9:
            s.strike = True
        elif c == 21:
            s.underline = True
        elif c == 22:
            s.bold = s.dim = False
        elif c == 23:
            s.italic = False
        elif c == 24:
            s.underline = False
        elif c == 27:
            s.inverse = False
        elif c == 29:
            s.strike = False
        elif 30 <= c <= 37:
            s.fg = index(c - 30)
        elif c == 38:
            x = extended()
            if x is not None:
                s.fg = x
        elif c == 39:
            s.fg = NONE
        elif 40 <= c <= 47:
            s.bg = index(c - 40)
        elif c == 48:
            x = extended()
            if x is not None:
                s.bg = x
        elif c == 49:
            s.bg = NONE
        elif c == 58:
            extended()
        elif 90 <= c <= 97:
            s.fg = index(c - 90 + 8)
        elif 100 <= c <= 107:
            s.bg = index(c - 100 + 8)
        i += 1


def parse(text: str) -> Grid:
    g = Grid()
    row = []
    col = 0
    style = Style()
    chars = _graphemes(text)
    i = 0

    def put(s, w):
        nonlocal col
        while len(row) < col:
            row.append(Cell(" ", style.copy(), 1))
        cell = Cell(s, style.copy(), w)
        if col < len(row):
            row[col] = cell
        else:
            row.append(cell)
        col += 1
        if w == 2:
            rest = Cell("", style.copy(), 0)
            if col < len(row):
                row[col] = rest
            else:
                row.append(rest)
            col += 1

    def newline():
        nonlocal row, col
        g.rows.append(row)
        row = []
        col = 0

    while i < len(chars):
        c = chars[i]
        if c == "\u001b":
            i += 1
            if i >= len(chars):
                break
            nxt = chars[i]
            if nxt == "[":
                params = ""
                i += 1
                while i < len(chars) and not (0x40 <= ord(chars[i][0]) <= 0x7E):
                    params += chars[i]
                    i += 1
                if i < len(chars) and chars[i] == "m":
                    apply_sgr(params, style)
                i += 1
            elif nxt in ("]", "P", "_", "^"):
                i += 1
                while i < len(chars):
                    if chars[i] == "\u0007":
                        i += 1
                        break
                    if chars[i] == "\u001b" and i + 1 < len(chars) and chars[i + 1] == "\\":
                        i += 2
                        break
                    i += 1
            elif nxt in ("(", ")", "*", "+"):
                i += 2
            else:
                i += 1
            continue
        if c in ("\r\n", "\n"):
            newline()
        elif c == "\r":
            col = 0
        elif c == "\t":
            stop = (col // 8 + 1) * 8
            while col < stop:
                put(" ", 1)
        else:
            a = ord(c[0])
            if not (a < 0x20 or a == 0x7F):
                put(c, cell_width(c))
        i += 1
    if row:
        newline()
    while g.rows and all((c.text == " " or c.text == "") and c.style.bg == NONE
                         and not c.style.inverse for c in g.rows[-1]):
        g.rows.pop()
    return g


# ---------------------------------------------------------------- themes

class Theme:
    def __init__(self):
        d = _DATA["theme"]
        self.foreground = RGB.from_hex(d["foreground"])
        self.background = RGB.from_hex(d["background"])
        self.palette = xterm256()
        self.font_name = d["font_name"]
        self.font_size = float(d["font_size"])
        self.bold_is_bright = bool(d["bold_is_bright"])
        self.display_p3 = bool(d["display_p3"])

    def colors(self, s: Style):
        def res(c, bright):
            if c[0] == "none":
                return None
            if c[0] == "index":
                n = c[1]
                return self.palette[n + 8 if bright and n < 8 else n]
            return RGB(*c[1])

        fg = res(s.fg, self.bold_is_bright and s.bold) or self.foreground
        bg = res(s.bg, False)
        if s.inverse:
            fg, bg = (bg or self.background), fg
        return (fg, bg)


def xterm256() -> list:
    p = list(_BASE16)
    steps = (0, 95, 135, 175, 215, 255)
    for r in steps:
        for g in steps:
            for b in steps:
                p.append(RGB(r, g, b))
    for i in range(24):
        v = 8 + i * 10
        p.append(RGB(v, v, v))
    return p


def ghostty(text: str) -> Theme:
    t = Theme()
    font_set = False
    for line in (text or "").split("\n"):
        at = line.find(" = ")
        if at < 0:
            continue
        k = line[:at].strip()
        v = line[at + 3:].strip()
        if k == "background":
            c = RGB.from_hex(v)
            if c:
                t.background = c
        elif k == "foreground":
            c = RGB.from_hex(v)
            if c:
                t.foreground = c
        elif k == "font-family":
            if not font_set and v:
                t.font_name = v
                font_set = True
        elif k == "font-size":
            try:
                s = float(v)
            except ValueError:
                s = 0
            if s > 0:
                t.font_size = s
        elif k == "bold-is-bright":
            t.bold_is_bright = v == "true"
        elif k == "window-colorspace":
            t.display_p3 = v == "display-p3"
        elif k == "palette":
            parts = v.split("=", 1)
            if len(parts) == 2:
                try:
                    n = int(parts[0])
                except ValueError:
                    continue
                c = RGB.from_hex(parts[1])
                if 0 <= n < 256 and c:
                    t.palette[n] = c
    return t
