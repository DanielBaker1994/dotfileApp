"""Screenshot model: tools + button ring, geometry/snap, the pixelate maths,
file naming, CLI args, colors and the persisted state.

This is the cold half of /screenshot — the app keeps the interactive
document/undo core and every CoreGraphics/CoreText pipeline in Swift and
asks here for the pure decisions. Ported case-for-case from the Swift suite
(Tests/test_screenshot.swift), whose non-CG cases this module's tests mirror.
"""
from __future__ import annotations

import json
import math
import os
import re
import time

# ---------------------------------------------------------------- tools

DRAW_KINDS = {"pencil", "line", "arrow", "selection", "rectangle", "circle",
              "marker", "text", "counter", "pixelate", "invert"}
LETTERS = {"pencil": "p", "line": "d", "arrow": "a", "selection": "s",
           "rectangle": "r", "circle": "c", "marker": "m", "text": "t",
           "pixelate": "b", "invert": "i"}
FINISHES = {"copy", "save", "accept", "exit", "pin", "copy-text"}
DEFAULT_SIZES = {"text": 8, "marker": 5, "pixelate": 2, "counter": 1, "rectangle": 1}
SYMBOLS = {
    "pencil": "pencil", "line": "line.diagonal", "arrow": "arrow.down.left",
    "selection": "square", "rectangle": "square.fill", "circle": "circle",
    "marker": "highlighter", "text": "textformat", "counter": "1.circle",
    "pixelate": "square.grid.3x3.fill", "invert": "circle.lefthalf.filled",
    "size": "", "move": "arrow.up.and.down.and.arrow.left.and.right",
    "undo": "arrow.uturn.backward", "redo": "arrow.uturn.forward",
    "copy": "doc.on.doc", "save": "square.and.arrow.down", "accept": "checkmark",
    "exit": "xmark", "pin": "pin.fill", "recent": "clock",
    "copy-text": "text.viewfinder", "size-increase": "plus", "size-decrease": "minus",
}
TOOLTIPS = {
    "pencil": "Set the Pencil as the paint tool (P)",
    "line": "Set the Line as the paint tool (D)",
    "arrow": "Set the Arrow as the paint tool (A)",
    "selection": "Set Selection as the paint tool (S)",
    "rectangle": "Set the Rectangle as the paint tool (R)",
    "circle": "Set the Circle as the paint tool (C)",
    "marker": "Set the Marker as the paint tool (M)",
    "text": "Add text to your capture (T)",
    "counter": "Add an autoincrementing counter bubble",
    "pixelate": "Set Pixelate as the paint tool (B)",
    "invert": "Set Inverter as the paint tool (I)",
    "size": "Selection size",
    "move": "Move the selection area (⌘M)",
    "undo": "Undo the last modification (⌘Z)",
    "redo": "Redo the next modification (⇧⌘Z)",
    "copy": "Copy selection to clipboard (⌘C)",
    "save": "Save screenshot to a file (⌘S)",
    "accept": "Accept the capture (Return)",
    "exit": "Leave the capture screen (⌘Q)",
    "pin": "Pin image on the desktop",
    "recent": "Recent screenshots",
    "copy-text": "Copy the text in the selection (⇧⌘C)",
    "size-increase": "Increase tool size",
    "size-decrease": "Decrease tool size",
}
ALL_TOOLS = ["pencil", "line", "arrow", "selection", "rectangle", "circle", "marker",
             "text", "counter", "pixelate", "invert", "size", "move", "undo", "redo",
             "copy", "save", "accept", "exit", "pin", "recent", "copy-text",
             "size-increase", "size-decrease"]
DEFAULT_BUTTONS = ("pencil, line, arrow, selection, rectangle, circle, marker, text, "
                   "counter, pixelate, invert, move, undo, redo, copy, copy-text, save, "
                   "exit, pin, recent")


def tool_kind(tool: str) -> str:
    if tool in DRAW_KINDS:
        return "draw"
    if tool == "move":
        return "mode"
    if tool == "size":
        return "info"
    return "action"


def tool_is_drawing(tool: str) -> bool:
    return tool_kind(tool) == "draw"


def tool_finishes(tool: str) -> bool:
    return tool in FINISHES


def tool_default_size(tool: str) -> int:
    return DEFAULT_SIZES.get(tool, 3)


def tool_size_range(tool: str):
    return (0, 100) if tool == "rectangle" else (1, 100)


def tool_for_letter(c: str):
    c = c.lower()
    for t in ALL_TOOLS:
        if LETTERS.get(t) == c:
            return t
    return None


def ring(spec: str, badge: bool) -> list:
    """The button ring: spec names, de-duplicated; empty spec -> the default
    buttons. `badge` inserts the size bubble after the last drawing tool
    (or drops it)."""
    out = []
    for part in (spec or "").split(","):
        n = part.strip().lower()
        if n in ALL_TOOLS and n not in out:
            out.append(n)
    if not out:
        return ring(DEFAULT_BUTTONS, badge)
    if not badge:
        out = [t for t in out if t != "size"]
    elif "size" not in out:
        at = 0
        for i, t in enumerate(out):
            if tool_is_drawing(t):
                at = i + 1
        out.insert(at, "size")
    return out


# ---------------------------------------------------------------- geometry

def _round_away(x: float) -> int:
    return math.floor(x + 0.5) if x >= 0 else math.ceil(x - 0.5)


class Rect:
    __slots__ = ("x", "y", "w", "h")

    def __init__(self, x, y, w, h):
        self.x, self.y, self.w, self.h = float(x), float(y), float(w), float(h)

    @property
    def minX(self):
        return self.x

    @property
    def minY(self):
        return self.y

    @property
    def maxX(self):
        return self.x + self.w

    @property
    def maxY(self):
        return self.y + self.h

    @property
    def midX(self):
        return self.x + self.w / 2

    @property
    def midY(self):
        return self.y + self.h / 2

    def __eq__(self, o):
        return isinstance(o, Rect) and (self.x, self.y, self.w, self.h) == (o.x, o.y, o.w, o.h)

    def __repr__(self):
        return "Rect(%g, %g, %g, %g)" % (self.x, self.y, self.w, self.h)


def rect_of(a, b) -> Rect:
    return Rect(min(a[0], b[0]), min(a[1], b[1]), abs(b[0] - a[0]), abs(b[1] - a[1]))


def intersection(a: Rect, b: Rect):
    x0, y0 = max(a.minX, b.minX), max(a.minY, b.minY)
    x1, y1 = min(a.maxX, b.maxX), min(a.maxY, b.maxY)
    if x1 <= x0 or y1 <= y0:
        return None
    return Rect(x0, y0, x1 - x0, y1 - y0)


def contains_point(r: Rect, x: float, y: float) -> bool:
    return r.minX <= x <= r.maxX and r.minY <= y <= r.maxY


def contains_rect(outer: Rect, inner: Rect) -> bool:
    return (inner.minX >= outer.minX and inner.maxX <= outer.maxX
            and inner.minY >= outer.minY and inner.maxY <= outer.maxY)


def intersects(a: Rect, b: Rect) -> bool:
    return intersection(a, b) is not None


def inset(r: Rect, dx: float, dy: float) -> Rect:
    return Rect(r.x + dx, r.y + dy, r.w - 2 * dx, r.h - 2 * dy)


def dist(a, b) -> float:
    return math.hypot(a[0] - b[0], a[1] - b[1])


def seg_dist(p, a, b) -> float:
    dx, dy = b[0] - a[0], b[1] - a[1]
    l2 = dx * dx + dy * dy
    if l2 <= 0:
        return dist(p, a)
    t = max(0.0, min(1.0, ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2))
    return dist(p, (a[0] + t * dx, a[1] + t * dy))


# ---------------------------------------------------------------- snapping

def snap_angle(a, b):
    dx, dy = b[0] - a[0], b[1] - a[1]
    length = math.hypot(dx, dy)
    if length <= 0:
        return b
    step = math.pi / 4
    ang = _round_away(math.atan2(dy, dx) / step) * step
    x = a[0] + math.cos(ang) * length
    y = a[1] + math.sin(ang) * length
    if abs(x - a[0]) < 1e-9:
        x = a[0]
    if abs(y - a[1]) < 1e-9:
        y = a[1]
    return (x, y)


def snap_square(a, b):
    dx, dy = b[0] - a[0], b[1] - a[1]
    s = max(abs(dx), abs(dy))
    return (a[0] + (-s if dx < 0 else s), a[1] + (-s if dy < 0 else s))


def snap(tool: str, a, b):
    if tool in ("line", "arrow", "marker"):
        return snap_angle(a, b)
    if tool in ("selection", "rectangle", "circle", "pixelate", "invert"):
        return snap_square(a, b)
    return b


# ---------------------------------------------------------------- button ring

def default_button_size(line_height: float) -> float:
    return float(_round_away(line_height * 2.2))


def ring_layout(selection: Rect, screen: Rect, count: int, button: float) -> dict:
    """Flameshot's button ring placement, ported verbatim. Returns
    {"frames": [Rect], "inside": bool}."""
    frames = [None] * count
    if count <= 0:
        return {"frames": [], "inside": False}
    base = float(button)
    sep = math.floor(base / 4)
    ext = base + sep
    sel = intersection(selection, screen)
    if sel is None:
        sel = Rect(selection.minX, selection.minY, 0, 0)
    idx = 0
    inside = False

    def place(pts):
        nonlocal idx
        for p in pts:
            if idx >= count:
                break
            frames[idx] = Rect(p[0], p[1], base, base)
            idx += 1

    def horizontal(c, n, left_to_right):
        if n % 2 == 0:
            shift = ext * (n // 2) - sep / 2
        else:
            shift = ext * ((n - 1) // 2) + base / 2
        if not left_to_right:
            shift -= base
        x = c[0] - shift if left_to_right else c[0] + shift
        out = []
        while len(out) < n:
            out.append((x, c[1]))
            x += ext if left_to_right else -ext
        return out

    def vertical(c, n, up_to_down):
        if n % 2 == 0:
            shift = ext * (n // 2) - sep / 2
        else:
            shift = ext * ((n - 1) // 2) + base / 2
        if not up_to_down:
            shift -= base
        y = c[1] - shift if up_to_down else c[1] + shift
        out = []
        while len(out) < n:
            out.append((c[0], y))
            y += ext if up_to_down else -ext
        return out

    if sel.w < base:
        sel.x -= math.floor((base - sel.w) / 2)
        sel.w = base
    if sel.h < base:
        sel.y -= math.floor((base - sel.h) / 2)
        sel.h = base
    sel.x = max(screen.minX, min(sel.minX, screen.maxX - sel.w))
    sel.y = max(screen.minY, min(sel.minY, screen.maxY - sel.h))

    guard_loops = 0
    while idx < count and guard_loops < 64:
        guard_loops += 1
        e = sep * 2 + base

        def on_screen(a, b):
            s = inset(screen, -0.5, -0.5)
            return contains_point(s, a[0], a[1]) and contains_point(s, b[0], b[1])

        b_right = not on_screen((sel.maxX + e, sel.maxY), (sel.maxX + e, sel.minY))
        b_left = not on_screen((sel.minX - e, sel.maxY), (sel.minX - e, sel.minY))
        b_bottom = not on_screen((sel.minX, sel.maxY + e), (sel.maxX, sel.maxY + e))
        b_top = not on_screen((sel.minX, sel.minY - e), (sel.maxX, sel.minY - e))
        one_horizontal = b_right != b_left
        both_horizontal = b_right and b_left
        if b_left and both_horizontal and b_bottom and b_top:
            area = intersection(sel, screen)
            if area is None:
                area = Rect(0, 0, 0, 0)
            if int(area.w / ext) == 0:
                area = screen
            per_row = int(area.w / ext)
            if per_row <= 0:
                break
            c = [area.midX, area.maxY - ext]
            while idx < count:
                n = min(per_row, count - idx)
                place(horizontal(c, n, True))
                c[1] -= ext
            inside = True
            break
        per_row = int((sel.w + sep) / ext)
        per_col = int((sel.h + sep) / ext)
        extra = (count - idx) - (per_row + per_col) * 2
        corners = min(4, extra)
        max_extra = 1 if one_horizontal else 0 if both_horizontal else 2
        corners_top = max(0, min(corners, max_extra))
        corners -= corners_top
        corners_bottom = max(0, min(corners, max_extra))

        def adjust(c):
            if b_left:
                c[0] += math.floor(ext / 2)
            elif b_right:
                c[0] -= math.floor(ext / 2)

        if not b_bottom:
            n = max(0, min(per_row + corners_bottom, count - idx))
            c = [sel.midX, sel.maxY + sep]
            if n > per_row:
                adjust(c)
            place(horizontal(c, n, True))
        if not b_right and idx < count:
            n = max(0, min(per_col, count - idx))
            place(vertical((sel.maxX + sep, sel.midY), n, False))
        if not b_top and idx < count:
            n = max(0, min(per_row + corners_top, count - idx))
            c = [sel.midX, sel.minY - ext]
            if n == per_row + 1:
                adjust(c)
            place(horizontal(c, n, False))
        if not b_left and idx < count:
            n = max(0, min(per_col, count - idx))
            place(vertical((sel.minX - ext, sel.midY), n, True))
        if idx < count:
            grown = Rect(sel.x - ext, sel.y - ext, sel.w + 2 * ext, sel.h + 2 * ext)
            sel = intersection(grown, screen)
            if sel is None:
                break
    return {"frames": [f if f is not None else Rect(0, 0, 0, 0) for f in frames], "inside": inside}


# ---------------------------------------------------------------- color

class Color:
    __slots__ = ("r", "g", "b", "a")

    def __init__(self, r=0.0, g=0.0, b=0.0, a=1.0):
        self.r, self.g, self.b, self.a = float(r), float(g), float(b), float(a)

    @classmethod
    def from_hex(cls, s):
        s = (s or "").strip()
        if s.startswith("#"):
            s = s[1:]
        if len(s) not in (6, 8):
            return None
        try:
            v = int(s, 16)
        except ValueError:
            return None
        if len(s) == 8:
            return cls(((v >> 16) & 0xFF) / 255, ((v >> 8) & 0xFF) / 255,
                       (v & 0xFF) / 255, ((v >> 24) & 0xFF) / 255)
        return cls(((v >> 16) & 0xFF) / 255, ((v >> 8) & 0xFF) / 255, (v & 0xFF) / 255)

    @property
    def hex(self) -> str:
        def c(x):
            return _round_away(max(0.0, min(1.0, x)) * 255)
        return "#%02x%02x%02x" % (c(self.r), c(self.g), c(self.b))

    def with_alpha(self, alpha: float):
        return Color(self.r, self.g, self.b, alpha)

    @property
    def luminance(self) -> float:
        return 0.299 * self.r + 0.587 * self.g + 0.114 * self.b

    @property
    def is_dark(self) -> bool:
        return self.luminance < 0.5

    def mixed(self, o, t: float):
        return Color(self.r + (o.r - self.r) * t, self.g + (o.g - self.g) * t,
                     self.b + (o.b - self.b) * t, self.a)

    @classmethod
    def from_hsv(cls, h: float, s: float, v: float):
        i = int(math.floor(h * 6)) % 6
        f = h * 6 - math.floor(h * 6)
        p = v * (1 - s)
        q = v * (1 - f * s)
        t = v * (1 - (1 - f) * s)
        return [cls(v, t, p), cls(q, v, p), cls(p, v, t),
                cls(p, q, v), cls(t, p, v), cls(v, p, q)][i]

    @property
    def hsv(self):
        mx, mn = max(self.r, self.g, self.b), min(self.r, self.g, self.b)
        d = mx - mn
        h = 0.0
        if d > 0:
            if mx == self.r:
                h = math.fmod((self.g - self.b) / d, 6)
            elif mx == self.g:
                h = (self.b - self.r) / d + 2
            else:
                h = (self.r - self.g) / d + 4
            h /= 6
            if h < 0:
                h += 1
        return (h, 0.0 if mx == 0 else d / mx, mx)

    def __eq__(self, o):
        return isinstance(o, Color) and (self.r, self.g, self.b) == (o.r, o.g, o.b)

    def __repr__(self):
        return "Color(%g, %g, %g)" % (self.r, self.g, self.b)


WHITE = Color(1, 1, 1)
BLACK = Color(0, 0, 0)


# ---------------------------------------------------------------- pixelate

class Pixels:
    """RGBA bytes, row-major, like the app's ShotPixels."""

    __slots__ = ("width", "height", "data")

    def __init__(self, width: int, height: int, data):
        self.width, self.height, self.data = width, height, bytearray(data)

    def color(self, x: int, y: int) -> Color:
        cx = max(0, min(self.width - 1, x))
        cy = max(0, min(self.height - 1, y))
        i = (cy * self.width + cx) * 4
        return Color(self.data[i] / 255, self.data[i + 1] / 255, self.data[i + 2] / 255)


def pixelate_grid(r: Rect, size: int) -> tuple:
    f = 0.5 / max(1, size + 1)
    return (max(1, _round_away(r.w * f)), max(1, _round_away(r.h * f)))


def secure_blocks(pixels: Pixels, px: Rect, size: int, scale: float = 1) -> list:
    """The privacy pixelate: block colors interpolated ONLY from a 1-4px
    band outside the rect (the interior is never read)."""
    s = max(1, scale)
    cols, rows = pixelate_grid(Rect(0, 0, px.w / s, px.h / s), size)
    x0, y0 = math.floor(px.minX), math.floor(px.minY)
    x1, y1 = math.ceil(px.maxX), math.ceil(px.maxY)
    band = (1, 2, 3, 4)

    def inx(x):
        return 0 <= x < pixels.width

    def iny(y):
        return 0 <= y < pixels.height

    def mean(pts):
        r = g = b = n = 0.0
        for x, y in pts:
            if inx(x) and iny(y) and not (x0 <= x < x1 and y0 <= y < y1):
                c = pixels.color(x, y)
                r += c.r
                g += c.g
                b += c.b
                n += 1
        return Color(r / n, g / n, b / n) if n > 0 else None

    def span(i, n, a, b):
        lo = a + int(i / n * (b - a))
        hi = a + int((i + 1) / n * (b - a)) - 1
        return range(lo, max(lo, hi) + 1)

    def stepped(rng):
        st = max(1, len(rng) // 8)
        return list(rng)[::st]

    top, bottom = [], []
    for i in range(cols):
        xs = stepped(span(i, cols, x0, x1))
        top.append(mean([(x, y0 - k) for x in xs for k in band]))
        bottom.append(mean([(x, y1 - 1 + k) for x in xs for k in band]))
    left, right = [], []
    for j in range(rows):
        ys = stepped(span(j, rows, y0, y1))
        left.append(mean([(x0 - k, y) for y in ys for k in band]))
        right.append(mean([(x1 - 1 + k, y) for y in ys for k in band]))

    def smooth(a):
        out = []
        for k in range(len(a)):
            near = [a[i] for i in (k - 1, k, k + 1) if 0 <= i < len(a) and a[i] is not None]
            if not near:
                out.append(None)
                continue
            n = float(len(near))
            out.append(Color(sum(c.r for c in near) / n,
                             sum(c.g for c in near) / n,
                             sum(c.b for c in near) / n))
        return out

    top, bottom, left, right = smooth(top), smooth(bottom), smooth(left), smooth(right)

    def avg(cs):
        w = sum(t for _, t in cs)
        if w <= 0:
            return None
        return Color(sum(c.r * t for c, t in cs) / w,
                     sum(c.g * t for c, t in cs) / w,
                     sum(c.b * t for c, t in cs) / w)

    gray = Color(0.5, 0.5, 0.5)
    seed = [((x0 * 73_856_093) & 0xFFFFFFFF) ^ ((y0 * 19_349_663) & 0xFFFFFFFF)]
    seed[0] = (seed[0] | 1) & 0xFFFFFFFF

    def noise():
        v = seed[0]
        v ^= (v << 13) & 0xFFFFFFFF
        v ^= v >> 17
        v ^= (v << 5) & 0xFFFFFFFF
        seed[0] = v & 0xFFFFFFFF
        return (seed[0] % 1000 / 1000 - 0.5) * 0.05

    out = []
    for j in range(rows):
        row = []
        v = 0.5 if rows == 1 else j / (rows - 1)
        for i in range(cols):
            u = 0.5 if cols == 1 else i / (cols - 1)
            parts = []
            if top[i]:
                parts.append((top[i], 1 - v + 0.001))
            if bottom[i]:
                parts.append((bottom[i], v + 0.001))
            if left[j]:
                parts.append((left[j], 1 - u + 0.001))
            if right[j]:
                parts.append((right[j], u + 0.001))
            c = avg(parts) or gray
            n = noise()
            row.append(Color(max(0, min(1, c.r + n)), max(0, min(1, c.g + n)),
                             max(0, min(1, c.b + n))))
        out.append(row)
    return out


# ---------------------------------------------------------------- files

def expand_pattern(pattern: str, date=None) -> str:
    date = date or time.time()
    pattern = pattern or "%F_%H-%M"
    pattern = pattern.replace("%F", "%Y-%m-%d")
    try:
        s = time.strftime(pattern, time.localtime(date))
    except ValueError:
        s = ""
    if not s:
        s = "screenshot"
    return s.replace("/", "-")


def unique_path(dirname: str, name: str, ext: str, exists=None) -> str:
    exists = exists or os.path.exists
    d = dirname[:-1] if dirname.endswith("/") else dirname
    e = "" if not ext else "." + ext
    p = "%s/%s%s" % (d, name, e)
    n = 2
    while exists(p) and n < 10_000:
        p = "%s/%s %d%s" % (d, name, n, e)
        n += 1
    return p


def target(path: str, pattern: str, fmt: str, date=None, is_dir=None, exists=None) -> str:
    is_dir = is_dir or os.path.isdir
    exists = exists or os.path.exists
    p = os.path.expanduser(path or "")
    if is_dir(p) or p.endswith("/"):
        return unique_path(dirname=p, name=expand_pattern(pattern, date), ext=fmt, exists=exists)
    base = p.rsplit("/", 1)[-1]
    ext = "" if "." not in base or base.startswith(".") and base.count(".") == 1 \
        else base.rsplit(".", 1)[1]
    return p + "." + fmt if not ext else p


# ---------------------------------------------------------------- CLI args

class ArgsProblem(Exception):
    def __init__(self, message):
        super().__init__(message)
        self.message = message


class Args:
    __slots__ = ("mode", "path", "clipboard", "delayMs", "region", "lastRegion",
                 "acceptOnSelect", "pin", "raw", "printGeometry", "screenNumber")

    def __init__(self):
        self.mode = "gui"
        self.path = None
        self.clipboard = False
        self.delayMs = 0
        self.region = None
        self.lastRegion = False
        self.acceptOnSelect = False
        self.pin = False
        self.raw = False
        self.printGeometry = False
        self.screenNumber = None

    @property
    def isOverlay(self) -> bool:
        return self.mode in ("gui", "text")

    @property
    def wantsReply(self) -> bool:
        return self.raw or self.printGeometry

    def __eq__(self, o):
        return isinstance(o, Args) and all(
            getattr(self, k) == getattr(o, k) for k in Args.__slots__)


MODES = ("gui", "text", "full", "screen")

_REGION = re.compile(
    r"\s*([-+]?\d*\.?\d+)x([-+]?\d*\.?\d+)"
    r"(?:\+([-+]?\d*\.?\d+)\+([-+]?\d*\.?\d+))?\s*$")


def parse_region(s: str):
    m = _REGION.fullmatch(s or "")
    if not m:
        return None
    w, h = float(m.group(1)), float(m.group(2))
    x = float(m.group(3)) if m.group(3) is not None else 0.0
    y = float(m.group(4)) if m.group(4) is not None else 0.0
    if w <= 0 or h <= 0:
        return None
    return Rect(x, y, w, h)


def parse_args(words) -> Args:
    """`kitchen-sink screenshot ...` argv (raises ArgsProblem with the same
    wording the CLI prints)."""
    words = list(words)
    a = Args()
    i = 0
    if words and words[0] in MODES:
        a.mode = words[0]
        i = 1

    def value(flag):
        nonlocal i
        if i + 1 >= len(words):
            return None
        i += 1
        return words[i]

    while i < len(words):
        w = words[i]
        if w in ("-p", "--path"):
            v = value(w)
            if v is None:
                raise ArgsProblem("%s needs a path" % w)
            a.path = v
        elif w in ("-c", "--clipboard"):
            a.clipboard = True
        elif w in ("-d", "--delay"):
            v = value(w)
            if v is None or not v.isdigit():
                raise ArgsProblem("%s needs milliseconds" % w)
            a.delayMs = int(v)
        elif w == "--region":
            v = value(w)
            if v is None or (parse_region(v) is None and not v.startswith("screen")):
                raise ArgsProblem("--region WxH+X+Y | screenN")
            a.region = v
        elif w == "--last-region":
            a.lastRegion = True
        elif w in ("-s", "--accept-on-select"):
            a.acceptOnSelect = True
        elif w == "--pin":
            a.pin = True
        elif w in ("-r", "--raw"):
            a.raw = True
        elif w in ("-g", "--print-geometry"):
            a.printGeometry = True
        elif w in ("-n", "--number"):
            v = value(w)
            if v is None or not v.isdigit():
                raise ArgsProblem("-n needs a screen number")
            a.screenNumber = int(v)
        else:
            raise ArgsProblem("unknown option %s" % w)
        i += 1
    return a


# ---------------------------------------------------------------- state

DEFAULT_STATE_PATH = os.path.expanduser("~/.cache/kitchen-sink/screenshot.json")


class State:
    def __init__(self, data=None):
        data = data if isinstance(data, dict) else {}
        self.sizes = dict(data.get("sizes") or {})
        self.color = data.get("color") if isinstance(data.get("color"), str) else None
        self.style = data.get("style") if isinstance(data.get("style"), dict) else {}
        self.lastRegion = data.get("lastRegion") if isinstance(data.get("lastRegion"), dict) else None
        self.gridSize = data.get("gridSize") if isinstance(data.get("gridSize"), int) else 10
        self.grid = bool(data.get("grid"))

    def size(self, tool: str) -> int:
        v = self.sizes.get(tool)
        return v if isinstance(v, int) else tool_default_size(tool)

    def json(self) -> dict:
        out = {"sizes": self.sizes, "style": self.style, "gridSize": self.gridSize,
               "grid": self.grid}
        if self.color is not None:
            out["color"] = self.color
        if self.lastRegion is not None:
            out["lastRegion"] = self.lastRegion
        return out

    @classmethod
    def load(cls, path: str = DEFAULT_STATE_PATH):
        try:
            with open(path, encoding="utf-8") as fh:
                return cls(json.load(fh))
        except (OSError, ValueError):
            return cls()

    def save(self, path: str = DEFAULT_STATE_PATH) -> None:
        state_save(path, self.json())


# ------------------------------------------------- app-facing entry points

def args_to_dict(a: Args) -> dict:
    return {k: getattr(a, k) for k in Args.__slots__}


def state_load(path: str) -> dict:
    """The raw state.json dict (the app keeps its typed mirror)."""
    try:
        with open(path, encoding="utf-8") as fh:
            d = json.load(fh)
        return d if isinstance(d, dict) else {}
    except (OSError, ValueError):
        return {}


def state_save(path: str, state) -> None:
    try:
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w", encoding="utf-8") as fh:
            json.dump(state if isinstance(state, dict) else {}, fh)
    except OSError:
        pass
