"""The picker: KeyMinder-style overlay in a terminal. `Picker` is the model
(state + key handling, no curses: tests drive it); `View` draws it."""
from __future__ import annotations

import curses
import json
import locale
import os
import sys
import time
import unicodedata

from .. import catalog, conflicts, favorites, paths
from ..model import KeyRow, SettingRow
from ..tables import LAYER_NAME, MODS, MOD_SYMBOL  # data/tables.json
from .keys import Decoder, Ev
from .lineedit import LineEdit, pbcopy

# old local names kept (the data now lives in data/tables.json)
MOD_ORDER = MODS
MOD_SYM = MOD_SYMBOL


# ------------------------------------------------------------------ text
def width(s: str) -> int:
    return sum(2 if unicodedata.east_asian_width(c) in "WF" else
               (0 if unicodedata.combining(c) else 1) for c in s)


def fit(s: str, w: int) -> str:
    s = s.replace("\n", " ")
    if w <= 0:
        return ""
    if width(s) <= w:
        return s + " " * (w - width(s))
    out, n = "", 0
    for c in s:
        cw = width(c)
        if n + cw > w - 1:
            break
        out += c
        n += cw
    return out + "…" + " " * (w - n - 1)


def wrap(s: str, w: int, max_lines: int) -> list:
    words, lines, cur = s.split(), [], ""
    for wd in words:
        if cur and width(cur) + 1 + width(wd) > w:
            lines.append(cur)
            cur = wd
        else:
            cur = (cur + " " + wd) if cur else wd
        while width(cur) > w:
            lines.append(cur[:w])
            cur = cur[w:]
    if cur:
        lines.append(cur)
    if len(lines) > max_lines:
        lines = lines[:max_lines]
        lines[-1] = fit(lines[-1], w).rstrip() if width(lines[-1]) < w else lines[-1][:w - 1] + "…"
        if not lines[-1].endswith("…"):
            lines[-1] = lines[-1][:w - 1] + "…"
    return lines


# ------------------------------------------------------------------ model
class Editor:
    def __init__(self, kind: str, row, value: str, title: str, hint: str, choices=None):
        self.kind = kind            # setting | bind | label
        self.row = row
        self.field = LineEdit(value)
        self.field.anchor = 0       # whole value selected: typing replaces it
        self.title = title
        self.hint = hint
        self.choices = choices or []
        self.error = ""
        self.force = False


class Picker:
    """All picker state + key handling. Side effects go through `ops`
    (set / bind / fav / copy / reload) so tests can stub them."""

    def __init__(self, cat, favs: set, ops=None):
        self.ops = ops or RealOps()
        self.query = LineEdit("")
        self.mods: set = set()
        self.fav_only = False
        self.favs = favs
        self.cursor = 0
        self.editor: Editor | None = None
        self.status = ""
        self.quit = False
        self.sort = {"key": None, "setting": None}     # kind → (column, descending)
        self.clash_first = False                        # ⚠ rows on top
        self.load(cat)

    def load(self, cat):
        self.cat = cat
        self.clash = {}
        for c in conflicts.find(cat.keys):
            if c.level == "info":
                continue
            msg_l = f"⚠ {c.winner.layer} {c.winner.view} {c.winner.display} takes it first ({c.winner.action})" \
                if c.level == "error" else f"⚠ {c.why}: {c.winner.layer} {c.winner.display} ({c.winner.action})"
            msg_w = f"⚠ also used by {c.loser.layer} {c.loser.view}: {c.loser.action} ({c.why})"
            self.clash.setdefault(c.loser.id, []).append(msg_l)
            self.clash.setdefault(c.winner.id, []).append(msg_w)
        # the AeroSpace binding behind an app `all:` hotkey (its mirror)
        self.global_of = {r.mirror_of: r for r in cat.keys if r.mirror_of}
        self._items = None

    # -- the visible list
    def items(self) -> list:
        if self._items is not None:
            return self._items
        from .. import search
        fav = self.favs if self.fav_only else None
        q = self.query.text
        pool = [r for r in self.cat.keys if not r.mirror_of]
        keys = search.filter_rows(pool, q, self.mods, fav)
        # a key query ("hyper t", "cmd k") that hits keys shows only keys
        chord_q = q.strip() and any(search.chord_score(r, q) for r in keys)
        sets = [] if self.mods or chord_q else search.filter_rows(self.cat.settings, q, None, fav)
        keys, sets = self._sorted("key", keys), self._sorted("setting", sets)
        out = []
        if keys:
            out.append(("head", "key"))
            out += [("key", r) for r in keys]
        if sets:
            out.append(("head", "setting"))
            out += [("setting", r) for r in sets]
        self._items = out
        return out

    # -- sorting: a clicked column header (again = reverse, a third time =
    # back to the source order); ⚠ = clashes first
    SORT_KEYS = {
        "key": {"layer": lambda r: r.layer, "view": lambda r: r.view.lower(),
                "keys": lambda r: r.display.lower(), "action": lambda r: r.action.lower()},
        "setting": {"section": lambda r: r.section, "key": lambda r: r.key,
                    "value": lambda r: r.value.lower(), "doc": lambda r: (r.doc or r.line_doc).lower()},
    }

    def _sorted(self, kind: str, rows: list) -> list:
        st = self.sort.get(kind)
        if st:
            rows = sorted(rows, key=self.SORT_KEYS[kind][st[0]], reverse=st[1])
        if kind == "key" and self.clash_first:
            rows = sorted(rows, key=lambda r: r.id not in self.clash)
        return rows

    def sort_by(self, kind: str, col: str):
        if col == "warn":
            self.toggle_clash_first()
            return
        cur = self.sort.get(kind)
        if not cur or cur[0] != col:
            self.sort[kind] = (col, False)
        elif not cur[1]:
            self.sort[kind] = (col, True)
        else:
            self.sort[kind] = None
        self._changed()

    def toggle_clash_first(self):
        self.clash_first = not self.clash_first
        n = sum(1 for r in self.cat.keys if r.id in self.clash and not r.mirror_of)
        self.status = (f"⚠ clashes first ({n})" if n else "no clashes") if self.clash_first else "source order"
        self._changed()

    def toggle_mod(self, mod: str):
        self.mods ^= {mod}
        self._changed()

    def toggle_fav_only(self):
        self.fav_only = not self.fav_only
        self._changed()

    def on_click(self, action, double: bool = False):
        """A click the view mapped to an action (see View.hits)."""
        self.status = ""
        if action is None:
            return
        if self.editor:
            if action[0] == "row":      # clicking away cancels the edit
                self.editor = None
            else:
                return
        kind = action[0]
        if kind == "mod":
            self.toggle_mod(action[1])
        elif kind == "fav":
            self.toggle_fav_only()
        elif kind == "clash":
            self.toggle_clash_first()
        elif kind == "sort":
            self.sort_by(action[1], action[2])
        elif kind == "row":
            self.click_item(action[1], double)

    def click_item(self, index: int, double: bool = False):
        """A click on the list row at items()[index] (double = Return)."""
        sel = self.selectable()
        if index in sel:
            self.cursor = sel.index(index)
            if double:
                self.open_editor()

    def selectable(self) -> list:
        return [i for i, it in enumerate(self.items()) if it[0] != "head"]

    def current(self):
        sel = self.selectable()
        if not sel:
            return None
        self.cursor = max(0, min(self.cursor, len(sel) - 1))
        return self.items()[sel[self.cursor]][1]

    def _changed(self):
        self._items = None
        self.cursor = 0

    def move(self, d: int):
        n = len(self.selectable())
        if n:
            self.cursor = max(0, min(n - 1, self.cursor + d))

    def jump_group(self, forward: bool = True):
        items, sel = self.items(), self.selectable()
        if not sel:
            return
        cur_kind = items[sel[self.cursor]][0]
        order = list(range(len(sel)))
        if not forward:
            order = list(reversed(order))
        start = self.cursor
        for i in order[(order.index(start) + 1):] + order[:order.index(start)]:
            if items[sel[i]][0] != cur_kind:
                # first row of that group
                k = items[sel[i]][0]
                self.cursor = next(j for j in range(len(sel)) if items[sel[j]][0] == k)
                return

    # -- editors
    def open_editor(self):
        row = self.current()
        if row is None:
            return
        if isinstance(row, SettingRow):
            choices = row.allowed if row.allowed else []
            val = row.value
            hint = "Return saves · Esc cancels"
            if row.type == "bool":
                hint = "Space flips · Return saves · Esc cancels"
            elif choices:
                hint = "←/→ or Space picks · type any value · Return saves · Esc cancels"
            self.editor = Editor("setting", row, val, f"[{row.section}] {row.key}", hint, choices)
            if choices and row.type in ("bool", "enum"):
                self.editor.field.anchor = None
            return
        if getattr(self, "global_of", {}).get(row.id):
            row = self.global_of[row.id]     # an app hotkey AeroSpace binds: rebind THAT
        if row.editable:
            ex = "alt-shift-h / cmd+shift+y / hyper+/" if row.layer == "aerospace" else "prefix+g / ctrl+h / alt+shift+j"
            self.editor = Editor("bind", row, "", f"rebind {LAYER_NAME[row.layer]} {row.view}: {row.display}  ({row.action})",
                                 f"type the new keys, e.g. {ex} · Return binds · Esc cancels")
            return
        if row.layer == "app":
            self.editor = Editor("label", row, row.action, f"[shortcuts] {row.view}: {row.chord_text}",
                                 "the text the app's Cmd+/ card shows · Return saves · Esc cancels")
            return
        self.status = f"read-only: {row.readonly_reason}" if row.readonly_reason else "read-only"

    def _cycle_choice(self, d: int):
        e = self.editor
        if not e.choices:
            return
        cur = e.field.text
        i = e.choices.index(cur) if cur in e.choices else -1
        e.field.set(e.choices[(i + d) % len(e.choices)])

    def commit(self):
        e = self.editor
        v = e.field.text
        if e.kind == "setting":
            code, lines = self.ops.set(e.row.section, e.row.key, v, e.force)
        elif e.kind == "label":
            code, lines = self.ops.set("shortcuts", f"{e.row.view}: {e.row.chord_text}", v, e.force)
        else:
            if not v.strip():
                e.error = "type the new keys first"
                return
            code, lines = self.ops.bind(e.row.layer, e.row.chord_text if e.row.layer == "aerospace" else e.row.chord_text,
                                        v.strip(), e.row.view if e.row.layer == "aerospace" else "main", e.force)
        if code == 0:
            self.editor = None
            self.status = " · ".join(lines[:2])
            keep = self.query.text
            self.load(self.ops.reload())
            self.query.text = keep
            self._items = None
            return
        e.error = " ".join(lines)
        if any("--force" in l for l in lines):
            e.error += "  (Return again = write anyway)"
            e.force = True

    # -- keys
    def handle(self, ev: Ev):
        self.status = "" if ev.kind != "paste" else self.status
        if self.editor:
            return self._handle_editor(ev)
        m = ev.mods
        if ev.kind == "mouse":
            if ev.name == "wheelup":
                self.move(-3)
            elif ev.name == "wheeldown":
                self.move(3)
            return
        if ev.is_("esc"):
            if self.query.text or self.mods or self.fav_only or self.clash_first:
                self.query.set("")
                self.mods.clear()
                self.fav_only = False
                self.clash_first = False
                self._changed()
            else:
                self.quit = True
            return
        if ev.kind == "char" and m == {"ctrl"} and ev.name == "c" and not self.query.text:
            self.quit = True
            return
        if ev.kind == "char" and m == {"ctrl"} and ev.name == "q":
            self.quit = True
            return
        if ev.kind == "char" and m == {"alt"} and ev.name in "1234":
            self.toggle_mod(MOD_ORDER[int(ev.name) - 1])
            return
        if ev.kind == "char" and m == {"alt"} and ev.name == "f":
            self.toggle_fav_only()
            return
        if ev.kind == "char" and m == {"alt"} and ev.name == "w":
            self.toggle_clash_first()
            return
        if ev.kind == "char" and m == {"ctrl"} and ev.name == "f":
            row = self.current()
            if row is not None:
                on = self.ops.fav(row.id)
                (self.favs.add if on else self.favs.discard)(row.id)
                self.status = ("★ favorite" if on else "☆ removed from favorites")
                if self.fav_only:
                    self._items = None
            return
        if ev.kind == "char" and m == {"ctrl"} and ev.name == "y":
            row = self.current()
            if row is not None:
                pbcopy(row_text(row))
                self.status = "copied the row"
            return
        if ev.is_("up") or (ev.kind == "char" and m == {"ctrl"} and ev.name == "p"):
            self.move(-1)
            return
        if ev.is_("down") or (ev.kind == "char" and m == {"ctrl"} and ev.name == "n"):
            self.move(1)
            return
        if ev.is_("pgup"):
            self.move(-10)
            return
        if ev.is_("pgdn"):
            self.move(10)
            return
        if ev.is_("tab"):
            self.jump_group(True)
            return
        if ev.is_("tab", "shift"):
            self.jump_group(False)
            return
        if ev.is_("enter"):
            self.open_editor()
            return
        before = self.query.text
        if self.query.handle(ev) and self.query.text != before:
            self._changed()

    def _handle_editor(self, ev: Ev):
        e = self.editor
        if ev.kind == "mouse":
            return
        if ev.is_("esc"):
            self.editor = None
            return
        if ev.is_("enter"):
            self.commit()
            return
        if e.choices and e.kind == "setting":
            if ev.is_("left") or ev.is_("right"):
                self._cycle_choice(-1 if ev.name == "left" else 1)
                e.force = False
                return
            if ev.kind == "char" and ev.name == " " and not ev.mods and \
                    (e.row.type == "bool" or e.field.text in e.choices or not e.field.text):
                self._cycle_choice(1)
                e.force = False
                return
        before = e.field.text
        e.field.handle(ev)
        if e.field.text != before:
            e.error = ""
            e.force = False

    def state(self) -> dict:
        row = self.current()
        return {"query": self.query.text, "mods": sorted(self.mods), "favOnly": self.fav_only,
                "clashFirst": self.clash_first,
                "sort": {k: (list(v) if v else None) for k, v in self.sort.items()},
                "editor": None if not self.editor else {"kind": self.editor.kind, "title": self.editor.title,
                                                        "text": self.editor.field.text,
                                                        "error": self.editor.error},
                "selected": None if row is None else row.id,
                "rows": [it[1].id for it in self.items() if it[0] != "head"][:200],
                "status": self.status, "quit": self.quit}


def row_text(row) -> str:
    if isinstance(row, KeyRow):
        return f"{row.display}\t{row.action}"
    return f"[{row.section}] {row.key} = {row.value}"


class RealOps:
    def set(self, sec, key, value, force):
        from ..cli import do_set
        return do_set(sec, key, value, force=force)

    def bind(self, layer, old, new, mode, force):
        from .. import rebind
        return rebind.bind(layer, old, new, mode=mode, force=force)

    def fav(self, row_id):
        return favorites.toggle(row_id)

    def reload(self):
        return catalog.build()


# ------------------------------------------------------------------ colors
def _xterm256(hexs: str):
    h = hexs.strip().lstrip("#")
    if h.lower().startswith("0x"):
        h = h[2:]
    if len(h) == 8:
        h = h[2:]                      # AARRGGBB → RRGGBB
    if len(h) != 6:
        return None
    try:
        r, g, b = (int(h[i:i + 2], 16) for i in (0, 2, 4))
    except ValueError:
        return None
    steps = [0, 95, 135, 175, 215, 255]

    def near(v):
        return min(range(6), key=lambda i: abs(steps[i] - v))
    ci = 16 + 36 * near(r) + 6 * near(g) + near(b)
    cr, cg, cb = steps[near(r)], steps[near(g)], steps[near(b)]
    gray = max(0, min(23, round((r + g + b) / 3 - 8) // 10))
    gv = 8 + gray * 10
    if (gv - r) ** 2 + (gv - g) ** 2 + (gv - b) ** 2 < (cr - r) ** 2 + (cg - g) ** 2 + (cb - b) ** 2:
        return 232 + gray
    return ci


class Palette:
    TEXT, DIM, ACCENT, SEL, SELACC, WARN, ERR, HEAD, BAR = range(1, 10)

    def __init__(self):
        self.ok = False
        try:
            curses.start_color()
            curses.use_default_colors()
        except curses.error:
            return
        if curses.COLORS < 256:
            pairs = {self.TEXT: (-1, -1), self.DIM: (8, -1), self.ACCENT: (5, -1), self.SEL: (-1, 8),
                     self.SELACC: (5, 8), self.WARN: (3, -1), self.ERR: (1, -1), self.HEAD: (5, -1),
                     self.BAR: (-1, 0)}
        else:
            th = paths.section("theme") if paths.hub("colors", "theme") == "theme" else {}

            def c(key, fb):
                v = _xterm256(th.get(key, "")) if th.get(key) else None
                return fb if v is None else v
            text, dim, acc = c("text", 252), c("dim", 246), c("accent", 183)
            # the background stays the terminal's own: `ws-settings open` starts
            # Ghostty with --background = [theme] background, so the title
            # strip above matches exactly (a 256-color guess wouldn't)
            hi, bg, head = c("highlight", 238), -1, c("header", 234)
            pairs = {self.TEXT: (text, bg), self.DIM: (dim, bg), self.ACCENT: (acc, bg),
                     self.SEL: (text, hi), self.SELACC: (acc, hi), self.WARN: (221, bg),
                     self.ERR: (210, bg), self.HEAD: (acc, head), self.BAR: (dim, head)}
        try:
            for n, (fg, bgc) in pairs.items():
                curses.init_pair(n, fg, bgc)
            self.ok = True
        except curses.error:
            pass

    def __call__(self, n: int) -> int:
        return curses.color_pair(n) if self.ok else 0


# ------------------------------------------------------------------ view
class View:
    DETAIL = 6

    def __init__(self, scr, pk: Picker):
        self.scr, self.pk = scr, pk
        self.pal = Palette()
        self.top = 0
        self.hits = []      # (y, x0, x1, action) of everything clickable, from the last draw

    @staticmethod
    def columns(kind: str, w: int) -> list:
        """(name, title, x, width) — rows, headers and clicks share them."""
        if kind == "key":
            act = max(4, w - 3 - 9 - 14 - 24 - 6)
            return [("layer", "Layer", 3, 9), ("view", "View", 13, 14), ("keys", "Keys", 28, 24),
                    ("action", "Action", 53, act), ("warn", "⚠", w - 2, 1)]
        vw = min(30, max(8, w // 4))
        rest = max(4, w - 3 - 16 - 22 - vw - 4)
        return [("section", "Section", 3, 16), ("key", "Key", 20, 22), ("value", "Value", 43, vw),
                ("doc", "Description", 44 + vw, rest)]

    def hit(self, x: int, y: int):
        for hy, x0, x1, action in reversed(self.hits):
            if hy == y and x0 <= x < x1:
                return action
        return None
        if self.pal.ok:
            scr.bkgd(" ", self.pal(Palette.TEXT))

    def put(self, y, x, s, attr=0):
        h, w = self.scr.getmaxyx()
        if y < 0 or y >= h or x >= w:
            return
        s = s[: max(0, w - x)]
        try:
            self.scr.addstr(y, x, s, attr)
        except curses.error:
            pass      # bottom-right corner

    def hline(self, y: int):
        # ACS line, not "─" * w: ncurses compresses a long run into REP
        # (CSI n b), which Ghostty doesn't repeat for a multi-byte glyph
        h, w = self.scr.getmaxyx()
        try:
            self.scr.hline(y, 0, curses.ACS_HLINE | self.pal(Palette.DIM), w)
        except curses.error:
            pass

    def draw(self):
        pk, P = self.pk, self.pal
        scr = self.scr
        scr.erase()
        h, w = scr.getmaxyx()
        if h < 10 or w < 40:
            self.put(0, 0, "ws-settings: make the window bigger")
            scr.refresh()
            return
        # header bar
        self.hits = []
        # paint every cell: cells nothing writes this frame otherwise keep the
        # terminal's own background (darker stripes between the columns)
        for yy in range(h):
            self.put(yy, 0, " " * (w - (1 if yy == h - 1 else 0)), P(Palette.TEXT))
        items = pk.items()
        nk = sum(1 for it in items if it[0] == "key")
        ns = sum(1 for it in items if it[0] == "setting")
        self.put(0, 0, fit(f" {nk} keys · {ns} settings", w), P(Palette.BAR))
        tog = [(f" {MOD_SYM[m]} ", m in pk.mods, ("mod", m)) for m in MOD_ORDER]
        tog.append((" ★ ", pk.fav_only, ("fav",)))
        tog.append((" ⚠ ", pk.clash_first, ("clash",)))
        x = w - sum(len(t) for t, _, _ in tog) - 1
        for t, on, action in tog:
            self.put(0, x, t, (P(Palette.SELACC) | curses.A_BOLD | curses.A_REVERSE) if on else P(Palette.BAR))
            self.hits.append((0, x, x + len(t), action))
            x += len(t)
        # search line
        self.put(1, 0, " › ", P(Palette.ACCENT) | curses.A_BOLD)
        qw = w - 4
        qtext = pk.query.text
        qoff = max(0, pk.query.pos - qw + 1)
        shown = qtext[qoff:qoff + qw]
        if not qtext:
            self.put(1, 3, fit("search keys, actions, settings…  (hyper t · cmd k · save path)", qw), P(Palette.DIM))
        else:
            self.put(1, 3, fit(shown, qw), P(Palette.TEXT))
            sel = pk.query.sel()
            if sel and not pk.editor:
                a, b = max(sel[0], qoff), min(sel[1], qoff + qw)
                if b > a:
                    self.put(1, 3 + width(qtext[qoff:a]), qtext[a:b], P(Palette.SEL))
        self.hline(2)
        # list
        list_top, list_h = 3, h - 3 - self.DETAIL - 1
        sel_idx = pk.selectable()
        cur_item = sel_idx[pk.cursor] if sel_idx else -1
        if cur_item >= 0:
            if cur_item < self.top:
                self.top = max(0, cur_item - 1 if cur_item > 0 and items[cur_item - 1][0] == "head" else cur_item)
            elif cur_item >= self.top + list_h:
                self.top = cur_item - list_h + 1
        self.top = max(0, min(self.top, max(0, len(items) - list_h)))
        for n in range(list_h):
            i = self.top + n
            if i >= len(items):
                break
            kind, row = items[i]
            y = list_top + n
            if kind == "head":
                # column titles (click = sort; ▲ / ▼ = the current one)
                st = pk.sort.get(row)
                for name, title, cx, cw in self.columns(row, w):
                    mark = ""
                    if name == "warn":
                        on = pk.clash_first
                    else:
                        on = bool(st and st[0] == name)
                        mark = (" ▼" if st[1] else " ▲") if on else ""
                    label = title + mark
                    self.put(y, cx, label[:cw] if name != "warn" else label,
                             (P(Palette.ACCENT) | curses.A_BOLD | (curses.A_UNDERLINE if on else 0)))
                    self.hits.append((y, cx, cx + max(cw, 1) + (1 if name == "warn" else 0), ("sort", row, name)))
                if row == "key":
                    self.put(y, 1, " ", P(Palette.ACCENT))
                continue
            self.hits.append((y, 0, w, ("row", i)))
            on = i == cur_item
            base = P(Palette.SEL) if on else P(Palette.TEXT)
            dim = P(Palette.SELACC) if on else P(Palette.DIM)
            if on:
                self.put(y, 0, " " * w, base)
                self.put(y, 0, "▌", P(Palette.SELACC) | curses.A_BOLD)
            star = "★" if row.id in pk.favs else " "
            self.put(y, 1, star, (P(Palette.SELACC) if on else P(Palette.ACCENT)))
            if kind == "key":
                text = {"layer": LAYER_NAME.get(row.layer, row.layer), "view": row.view,
                        "keys": row.display, "action": row.action}
                style = {"layer": dim, "view": dim, "keys": base | curses.A_BOLD, "action": base}
                for name, _, cx, cw in self.columns("key", w):
                    if name == "warn":
                        self.put(y, cx, "⚠" if row.id in pk.clash else " ", P(Palette.WARN))
                    else:
                        self.put(y, cx, fit(text[name], cw), style[name])
            else:
                text = {"section": f"[{row.section}]", "key": row.key,
                        "value": row.value if row.set else f"({row.value})", "doc": row.doc or row.line_doc}
                style = {"section": dim, "key": base | curses.A_BOLD, "value": base, "doc": dim}
                for name, _, cx, cw in self.columns("setting", w):
                    self.put(y, cx, fit(text[name], cw), style[name])
        if not sel_idx:
            self.put(list_top + 1, 3, "nothing matches" + (" (Esc clears the filters)"
                                                            if pk.mods or pk.fav_only else ""),
                     P(Palette.DIM))
        # detail
        dy = h - self.DETAIL - 1
        self.hline(dy)
        cursor_at = None
        if pk.editor:
            e = pk.editor
            self.put(dy + 1, 1, fit(e.title, w - 2), P(Palette.ACCENT) | curses.A_BOLD)
            doc = ""
            if isinstance(e.row, SettingRow):
                doc = e.row.doc or e.row.line_doc
                if e.choices:
                    doc = ("options: " + " | ".join(e.choices) + ("  ·  " + doc if doc else ""))
            self.put(dy + 2, 1, fit(doc, w - 2), P(Palette.DIM))
            fw = w - 6
            t = e.field.text
            off = max(0, e.field.pos - fw + 1)
            self.put(dy + 3, 1, " › ", P(Palette.ACCENT) | curses.A_BOLD)
            self.put(dy + 3, 4, fit(t[off:off + fw], fw), P(Palette.SEL))
            sel = e.field.sel()
            if sel:
                a, b = max(sel[0], off), min(sel[1], off + fw)
                if b > a:
                    self.put(dy + 3, 4 + width(t[off:a]), t[a:b], P(Palette.SELACC) | curses.A_REVERSE)
            cursor_at = (dy + 3, 4 + width(t[off:e.field.pos]))
            if e.error:
                for k, line in enumerate(wrap(e.error, w - 2, 2)):
                    self.put(dy + 4 + k, 1, line, P(Palette.ERR))
            else:
                self.put(dy + 4, 1, fit(e.hint, w - 2), P(Palette.DIM))
        else:
            row = pk.current()
            if row is not None:
                lines = []
                if isinstance(row, KeyRow):
                    head = f"{row.display}  —  {row.action}"
                    src = f"{LAYER_NAME.get(row.layer, row.layer)} · {row.view} · " \
                          f"{os.path.basename(row.source_file)}:{row.line}"
                    note = ("Return: rebind" if row.editable or pk.global_of.get(row.id) else
                            "Return: edit the label" if row.layer == "app" else row.readonly_reason)
                    lines = [(head, P(Palette.TEXT) | curses.A_BOLD), (src + "   " + note, P(Palette.DIM))]
                    g = pk.global_of.get(row.id)
                    if g:
                        lines.append((f"global hotkey: AeroSpace {g.chord_text} "
                                      f"({os.path.basename(g.source_file)}:{g.line}) runs {g.doc.split('  ·  ')[0]}",
                                      P(Palette.DIM)))
                    for cl in pk.clash.get(row.id, [])[:1]:
                        lines.append((cl, P(Palette.WARN)))
                    if row.doc:
                        lines += [(l, P(Palette.DIM)) for l in wrap(row.doc, w - 2, 5 - len(lines))]
                else:
                    head = f"[{row.section}] {row.key} = {row.value}" + ("" if row.set else "   (commented out: the default)")
                    meta = f"{row.type}" + (f" · {' | '.join(row.allowed)}" if row.allowed else "") + \
                           f" · applies: {row.apply} · {os.path.basename(row.source_file)}:{row.line}"
                    lines = [(head, P(Palette.TEXT) | curses.A_BOLD), (meta, P(Palette.DIM))]
                    doc = row.doc + ("  " + row.line_doc if row.line_doc and row.line_doc != row.doc else "")
                    lines += [(l, P(Palette.TEXT)) for l in wrap(doc, w - 2, 3)]
                for k, (text, attr) in enumerate(lines[:self.DETAIL - 1]):
                    self.put(dy + 1 + k, 1, fit(text, w - 2), attr)
        # footer
        foot = pk.status or ("Return / double-click edit · click a column to sort · Ctrl+F ★ · "
                             "Alt+F ★ only · Alt+W ⚠ first · Alt+1-4 ⌃⌥⇧⌘ · Ctrl+Y copy · Esc back")
        self.put(h - 1, 0, fit(" " + foot, w), P(Palette.BAR) if not pk.status else P(Palette.HEAD))
        if cursor_at is None:
            cursor_at = (1, 3 + width(qtext[qoff:pk.query.pos]))
        try:
            curses.curs_set(1)
            scr.move(*cursor_at)
        except curses.error:
            pass
        scr.refresh()


# ------------------------------------------------------------------ run
def _log_time(msg: str):
    try:
        with open(os.path.join(paths.cache_dir(), "settings-hub.log"), "a", encoding="utf-8") as fh:
            fh.write(time.strftime("%Y-%m-%d %H:%M:%S ") + msg + "\n")
    except OSError:
        pass


def _save_size():
    """The window's pixel size (TIOCGWINSZ) → `ws-settings open` centers the
    next window with it."""
    try:
        import fcntl
        import struct
        import termios
        rows, cols, xpix, ypix = struct.unpack("HHHH", fcntl.ioctl(1, termios.TIOCGWINSZ, b"\0" * 8))
        if xpix and ypix and os.environ.get("WS_SETTINGS_T0"):     # only the Hyper+/ window
            with open(os.path.join(paths.cache_dir(), "settings-hub-size.json"), "w", encoding="utf-8") as fh:
                json.dump({"xpix": xpix, "ypix": ypix, "cols": cols, "rows": rows}, fh)
    except Exception:  # noqa: BLE001 — a missing size only costs the centering
        pass


def _main(scr, timing: bool) -> int:
    t_start = time.time()
    curses.raw()
    curses.noecho()
    scr.keypad(False)
    try:
        curses.set_escdelay(25)
    except AttributeError:
        pass
    # bracketed paste (Cmd+V), the window title, and the kitty keyboard
    # protocol's "disambiguate" level: Cmd+X / Cmd+Z / Cmd+Shift+Z then
    # arrive as CSI-u (Ghostty keeps Cmd+A / Cmd+C for its own select / copy)
    sys.stdout.write("\x1b[?2004h\x1b[>1u\x1b[?1000h\x1b[?1006h\x1b]2;"
                     + paths.hub("window-title", "ws-settings") + "\x07")
    sys.stdout.flush()
    _save_size()
    t_cat = time.time()
    cat = catalog.build()
    t_cat = time.time() - t_cat
    pk = Picker(cat, favorites.load())
    view = View(scr, pk)
    view.draw()
    if timing or os.environ.get("WS_SETTINGS_T0"):
        t0 = float(os.environ.get("WS_SETTINGS_T0") or t_start)
        _log_time(f"ws-settings tui: first paint {1000 * (time.time() - t_start):.0f} ms "
                  f"(catalog {1000 * t_cat:.0f} ms, since launch {1000 * (time.time() - t0):.0f} ms)")
    state_file = os.environ.get("WS_SETTINGS_STATE")

    def dump():
        if state_file:
            tmp = state_file + ".tmp"
            with open(tmp, "w", encoding="utf-8") as fh:
                json.dump(pk.state(), fh)
            os.replace(tmp, state_file)
    dump()
    dec = Decoder()
    last_click = [None, 0.0]
    while not pk.quit:
        scr.timeout(-1 if not dec.pending() else 30)
        try:
            ch = scr.get_wch()
        except curses.error:
            evs = dec.timeout()
        except KeyboardInterrupt:
            evs = [Ev("char", "c", frozenset({"ctrl"}))]
        else:
            if ch == curses.KEY_RESIZE:
                view.draw()
                continue
            if isinstance(ch, int):
                continue
            evs = dec.feed(ch)
        for ev in evs:
            if ev.kind == "mouse" and ev.name == "click":
                act = view.hit(ev.x, ev.y)
                now = time.time()
                double = act is not None and last_click[0] == act and now - last_click[1] < 0.4
                last_click[:] = [act, 0 if double else now]
                pk.on_click(act, double)
                continue
            pk.handle(ev)
        if evs:
            view.draw()
            dump()
    return 0


def run(timing: bool = False) -> int:
    # a terminal started by `open` may have no LANG: curses then draws
    # multi-byte glyphs (─ ★ ⌘) as bytes and runs off the line
    locale.setlocale(locale.LC_ALL, "")
    if "utf" not in (locale.getpreferredencoding(False) or "").lower():
        for loc in ("en_US.UTF-8", "C.UTF-8", "UTF-8"):
            try:
                locale.setlocale(locale.LC_ALL, loc)
                os.environ["LC_ALL"] = loc
                break
            except locale.Error:
                continue
    if not sys.stdin.isatty():
        print("ws-settings tui needs a terminal", file=sys.stderr)
        return 1
    try:
        return curses.wrapper(_main, timing)
    except curses.error as e:
        if os.environ.get("TERM") != "xterm-256color":
            os.environ["TERM"] = "xterm-256color"
            try:
                return curses.wrapper(_main, timing)
            except curses.error:
                pass
        print(f"ws-settings: terminal error: {e}", file=sys.stderr)
        return 1
    finally:
        sys.stdout.write("\x1b[?1006l\x1b[?1000l\x1b[<u\x1b[?2004l")
        sys.stdout.flush()
