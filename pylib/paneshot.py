"""pane-shot: CLI args, the [pane-shot] config and the herdr JSON envelope.

Process spawning stays in Swift; this is the parsing/clamping half (the
Swift suite's arg/config/JSON cases moved here, ws test paneshot)."""
from __future__ import annotations

import json


def tri(s):
    v = (s or "").lower()
    if v in ("true", "yes", "1", "on"):
        return True
    if v in ("false", "no", "0", "off"):
        return False
    return None


class Problem(Exception):
    def __init__(self, message):
        super().__init__(message)
        self.message = message


class Args:
    __slots__ = ("pane", "lines", "all", "file", "save", "copy")

    def __init__(self):
        self.pane = None
        self.lines = None
        self.all = False
        self.file = None
        self.save = None
        self.copy = None

    def __eq__(self, o):
        return isinstance(o, Args) and all(
            getattr(self, k) == getattr(o, k) for k in Args.__slots__)


USAGE = "pane-shot [--pane ID] [--lines N|all] [--file PATH|-] [--no-save] [--no-copy]"


def parse_args(words) -> Args:
    words = list(words)
    a = Args()
    i = 0

    def value(flag):
        nonlocal i
        i += 1
        if i >= len(words) or not words[i]:
            raise Problem("%s needs a value" % flag)
        return words[i]

    while i < len(words):
        w = words[i]
        if w in ("--pane", "-p"):
            a.pane = value(w)
        elif w in ("--lines", "-n"):
            v = value(w)
            if v == "all":
                a.all = True
            else:
                try:
                    n = int(v)
                except ValueError:
                    n = -1
                if n < 0:
                    raise Problem('--lines takes a number or "all", not %s' % v)
                a.lines = n
        elif w in ("--file", "-f"):
            a.file = value(w)
        elif w == "--save":
            a.save = True
        elif w == "--no-save":
            a.save = False
        elif w == "--copy":
            a.copy = True
        elif w == "--no-copy":
            a.copy = False
        elif w in ("-h", "--help"):
            raise Problem("usage: kitchen-sink " + USAGE)
        else:
            raise Problem("unknown argument %s\nusage: kitchen-sink %s" % (w, USAGE))
        i += 1
    return a


MAX_LINES = 1000


class Config:
    def __init__(self, entries=None):
        e = entries if isinstance(entries, dict) else {}
        self.lines = 200
        self.save = True
        self.copy = True
        self.preview = True
        self.herdr_bin = "~/.local/bin/herdr"
        self.ghostty_bin = "/Applications/Ghostty.app/Contents/MacOS/ghostty"
        self.font = ""
        self.font_size = 0.0
        self.background = ""
        self.padding = 16.0
        self.save_path = ""
        self.filename_pattern = "%F_%H-%M-%S pane"
        self.toast = "Screenshot of {pane} copied ({n} lines)"

        def s(k, d):
            v = (e.get(k) or "").strip()
            return v if v else d

        def n(k, d, lo, hi):
            try:
                v = float(e[k])
            except (KeyError, TypeError, ValueError):
                return float(d)
            return max(lo, min(hi, v))

        self.lines = int(n("lines", 200, 0, MAX_LINES))
        self.save = tri(e.get("save")) if tri(e.get("save")) is not None else True
        self.copy = tri(e.get("copy")) if tri(e.get("copy")) is not None else True
        self.preview = tri(e.get("preview")) if tri(e.get("preview")) is not None else True
        self.herdr_bin = s("herdr-bin", self.herdr_bin)
        self.ghostty_bin = s("ghostty-bin", self.ghostty_bin)
        self.font = s("font", "")
        self.font_size = n("font-size", 0, 0, 72)
        self.background = s("background", "")
        self.padding = n("padding", 16, 0, 200)
        self.save_path = s("save-path", "")
        self.filename_pattern = s("filename-pattern", self.filename_pattern)
        if "toast" in e and isinstance(e.get("toast"), str):
            self.toast = e["toast"]


class HerdrFailure(Exception):
    def __init__(self, message):
        super().__init__(message)
        self.message = message


def pane_from_json(text: str) -> dict:
    """herdr's JSON envelope -> {"id", "title", "viewportRows"}."""
    try:
        d = json.loads(text)
    except ValueError:
        d = None
    if not isinstance(d, dict):
        raise HerdrFailure("herdr answered no JSON")
    if isinstance(d.get("error"), dict):
        msg = d["error"].get("message")
        raise HerdrFailure(msg if isinstance(msg, str) else "herdr error")
    result = d.get("result") if isinstance(d.get("result"), dict) else {}
    p = result.get("pane") if isinstance(result.get("pane"), dict) else None
    if p is None or not isinstance(p.get("pane_id"), str):
        raise HerdrFailure("herdr answered without a pane")
    scroll = p.get("scroll") if isinstance(p.get("scroll"), dict) else {}
    rows = scroll.get("viewport_rows")
    rows = rows if isinstance(rows, int) and not isinstance(rows, bool) else 0
    title = p.get("terminal_title_stripped")
    if not (isinstance(title, str) and title):
        title = p.get("agent") if isinstance(p.get("agent"), str) else None
    if not title:
        title = p["pane_id"]
    return {"id": p["pane_id"], "title": title, "viewportRows": rows}


def lines_for(viewport: int, history: int, all_lines: bool) -> int:
    if all_lines:
        return MAX_LINES
    return min(MAX_LINES, max(1, viewport + history))


def environment(env: dict) -> dict:
    drop = {"HERDR_PANE_ID", "HERDR_TAB_ID", "HERDR_WORKSPACE_ID"}
    return {k: v for k, v in env.items() if k not in drop}
