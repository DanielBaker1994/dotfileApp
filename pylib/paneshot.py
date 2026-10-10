"""pane-shot: CLI args, the [pane-shot] config and the herdr JSON envelope.

Process spawning stays in Swift; this is the parsing/clamping half (the
Swift suite's arg/config/JSON cases moved here, ws test paneshot)."""
from __future__ import annotations

import json
import jsonmgr

# one tri in python: config_text owns the grammar
from config_text import tri  # noqa: F401

_DATA = jsonmgr.load("pylib/paneshot")


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


USAGE = _DATA["usage"]


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


MAX_LINES = _DATA["max_lines"]


class Config:
    def __init__(self, entries=None):
        e = entries if isinstance(entries, dict) else {}
        d = _DATA["defaults"]

        def s(k, dflt):
            v = (e.get(k) or "").strip()
            return v if v else dflt

        def n(k, lo, hi):
            try:
                v = float(e[k])
            except (KeyError, TypeError, ValueError):
                v = float(d[k]["default"])
            return max(lo, min(hi, v))

        self.lines = int(n("lines", d["lines"]["min"], d["lines"]["max"]))
        self.save = tri(e.get("save")) if tri(e.get("save")) is not None else d["save"]
        self.copy = tri(e.get("copy")) if tri(e.get("copy")) is not None else d["copy"]
        self.preview = tri(e.get("preview")) if tri(e.get("preview")) is not None else d["preview"]
        self.herdr_bin = s("herdr-bin", d["herdr-bin"])
        self.ghostty_bin = s("ghostty-bin", d["ghostty-bin"])
        self.font = s("font", d["font"])
        self.font_size = n("font-size", d["font-size"]["min"], d["font-size"]["max"])
        self.background = s("background", d["background"])
        self.padding = n("padding", d["padding"]["min"], d["padding"]["max"])
        self.save_path = s("save-path", d["save-path"])
        self.filename_pattern = s("filename-pattern", d["filename-pattern"])
        if "toast" in e and isinstance(e.get("toast"), str):
            self.toast = e["toast"]
        else:
            self.toast = d["toast"]


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
