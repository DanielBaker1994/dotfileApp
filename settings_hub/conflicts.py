"""Clash report: the same key claimed twice where both could fire.

- A GLOBAL key (AeroSpace main mode, Ghostty `global:`, macOS) is taken
  before any app sees it: the other row never fires.
- A Ghostty terminal keybind is taken before herdr (herdr runs inside it).
- Two rows of one layer + view on the same key.
"""
from __future__ import annotations

import os
import tomllib
from dataclasses import dataclass

from . import chords
from .model import KeyRow

DATA = os.path.join(os.path.dirname(os.path.abspath(__file__)), "data", "system_shortcuts.toml")


@dataclass
class Clash:
    level: str        # error | warn | info
    chord: str
    winner: KeyRow
    loser: KeyRow
    why: str


def system_rows() -> list:
    try:
        with open(DATA, "rb") as fh:
            data = tomllib.load(fh)
    except (OSError, ValueError):
        return []
    rows = []
    for s in data.get("shortcut", []):
        cs, _ = chords.app_label(s["keys"])
        rows.append(KeyRow(layer="macos", view="system", chords=cs, chord_text=s["keys"],
                           action=s["does"], source_file=DATA,
                           readonly_reason="macOS (System Settings ▸ Keyboard)"))
    return rows


def _global(r: KeyRow) -> bool:
    return (r.layer == "aerospace" and r.view == "main") or \
        (r.layer == "ghostty" and r.view == "global") or r.layer == "macos"


def _label(r: KeyRow) -> str:
    return f"{r.layer} {r.view}"


def find(keys: list, with_system: bool = True) -> list:
    rows = list(keys) + (system_rows() if with_system else [])
    out, seen = [], set()

    def add(level, chord, w, l, why):
        k = (chord, w.id, l.id)
        if k not in seen and (chord, l.id, w.id) not in seen:
            seen.add(k)
            out.append(Clash(level, chord, w, l, why))

    globals_ = [(c.first, r) for r in rows if _global(r) for c in r.chords if len(c.strokes) == 1]
    for st, g in globals_:
        for r in rows:
            if r is g or r.mirror_of == g.id or g.mirror_of == r.id:
                continue
            if r.layer == "aerospace" and r.view != "main":
                continue
            for c in r.chords:
                if c.first != st:
                    continue
                if r.layer == "app" and g.mirror_of and r.view == "all":
                    continue
                if _global(r):
                    if g.layer == "macos" and r.layer != "macos":
                        add("warn", st.text, r, g, f"{_label(r)} overrides the macOS shortcut")
                    elif r.layer == "macos":
                        continue
                    elif g.layer != r.layer:
                        add("error", st.text, g, r, "two global layers claim it")
                    continue
                if g.layer == "macos":
                    continue      # apps may use system-ish keys inside themselves
                add("error", st.text, g, r, f"{_label(g)} takes it first: {_label(r)} never fires")
    # Ghostty terminal keybinds run before herdr
    for g in [r for r in rows if r.layer == "ghostty" and r.view == "terminal" and not r.action.startswith("unbound")]:
        for gc in g.chords:
            for r in rows:
                if r.layer == "herdr" and any(c.first == gc.first for c in r.chords):
                    add("error", gc.first.text, g, r, "Ghostty takes it before herdr")
    # same layer + view
    by = {}
    for r in rows:
        if _global(r) and r.layer == "macos":
            continue
        for c in r.chords:
            by.setdefault((r.layer, r.view, c.text), []).append(r)
    for (layer, view, ct), rs in by.items():
        uniq = []
        for r in rs:
            if r not in uniq:
                uniq.append(r)
        for a, b in zip(uniq, uniq[1:]):
            add("warn", ct, a, b, f"bound twice in {layer} {view}")
    # app: an `all:` key that a view also binds (the view's own wins there)
    app_all = [(c.text, r) for r in rows if r.layer == "app" and r.view == "all" for c in r.chords]
    for ct, a in app_all:
        for r in rows:
            if r.layer == "app" and r.view != "all" and r.action != a.action and \
                    any(c.text == ct for c in r.chords):
                add("info", ct, r, a, f"the {r.view} view uses it for something else")
    order = {"error": 0, "warn": 1, "info": 2}
    out.sort(key=lambda c: (order[c.level], c.chord))
    return out
