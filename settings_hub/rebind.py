"""Rebinding AeroSpace and herdr keys: swap the keys on ONE line, refuse a
key that's already taken there, check the file with the tool's own checker
(`aerospace reload-config --dry-run`, `herdr config check`), apply, and undo
automatically when the check fails. App keys are not rebindable: they live
in the Swift code; [shortcuts] is only their label list."""
from __future__ import annotations

from .tables import AERO_KEY, AERO_MOD, HERDR_KEY  # data/tables.json

import os
import re

try:
    import tomllib
except ImportError:
    tomllib = None

from . import apply, catalog, chords, conflicts, undo, writer
from .model import MODS, Chord, Stroke

from config_text import config_entry_span, config_line_parts, toml_array, toml_string  # noqa: E402



def to_aerospace(st: Stroke) -> str:
    key = AERO_KEY.get(st.key, st.key.lower() if len(st.key) == 1 else st.key.lower())
    return "-".join([m for m in ("alt", "cmd", "ctrl", "shift") if m in st.mods] + [key])


def to_herdr(c: Chord, prefix: Stroke) -> str:
    def one(st):
        key = HERDR_KEY.get(st.key, st.key.lower())
        return "+".join([m for m in ("ctrl", "alt", "shift", "cmd") if m in st.mods] + [key])
    if len(c.strokes) == 2 and c.strokes[0] == prefix:
        return "prefix+" + one(c.strokes[1])
    if len(c.strokes) != 1:
        raise ValueError("herdr keys are one key, or prefix + one key")
    return one(c.strokes[0])


def parse_any(text: str, layer: str, prefix: Stroke | None) -> Chord | None:
    """`prefix+g` (herdr), `alt-shift-h` (AeroSpace), `⌘⇧K`, `hyper+/`,
    `cmd+shift+y`, `opt+h` → Chord."""
    t = text.strip()
    if not t:
        return None
    if t.startswith("prefix+"):
        return chords.herdr(t, prefix) if layer == "herdr" else None
    if "+" not in t and re.fullmatch(r"[a-z]+(-[A-Za-z0-9]+)+", t):
        return chords.aerospace(t)
    if t[:1] in "⌃⌥⇧⌘":
        return chords.parse_human(t)
    st = chords.plus_stroke(re.sub(r"(?i)^hyper\+", "ctrl+alt+shift+cmd+", t))
    return chords.chord(st) if st else chords.parse_human(t)


def _herdr_prefix() -> Stroke:
    from . import paths
    try:
        with open(paths.herdr_conf(), "rb") as fh:
            p = (tomllib.load(fh).get("keys", {}) or {}).get("prefix") or "ctrl+b"
    except (OSError, ValueError, AttributeError):
        p = "ctrl+b"
    return chords.plus_stroke(p)


def bind(layer: str, old: str, new: str, mode: str = "main", force: bool = False,
         no_apply: bool = False) -> tuple:
    cat = catalog.build(with_settings=False)
    prefix = _herdr_prefix() if layer == "herdr" else None
    rows = [r for r in cat.keys if r.layer == layer and (layer != "aerospace" or r.view == mode)]
    # find the row: by its current keys, or (herdr) by action name
    oc = parse_any(old, layer, prefix)
    hit = [r for r in rows if oc and any(c == oc or (layer == "aerospace" and r.view != "main"
                                                       and c.strokes[-1:] == oc.strokes[-1:]) for c in r.chords)]
    if not hit and layer == "herdr":
        hit = [r for r in rows if r.action.replace(" ", "_") == old or r.action == old]
    if not hit:
        return 2, [f"no {layer} key matches {old!r}" + (f" in mode {mode}" if layer == "aerospace" else "")]
    if len(hit) > 1:
        return 2, [f"{old!r} matches several {layer} keys: " + ", ".join(f"{r.display} ({r.action})" for r in hit),
                   "name it by its keys"]
    row = hit[0]
    nc = parse_any(new, layer, prefix)
    if nc is None:
        return 2, [f"can't read the new keys {new!r} (e.g. cmd+shift+y, hyper+/, alt-h, prefix+g)"]
    taken = [r for r in rows if r is not row and any(c.first == nc.first if layer == "aerospace" else c == nc
                                                      for c in r.chords)]
    if layer == "aerospace" and mode != "main":
        taken = [r for r in rows if r is not row and any(c.strokes[-1] == nc.strokes[-1] for c in r.chords)]
    if taken:
        return 2, [f"{nc.text} is already bound in {layer} {row.view}: {taken[0].action}"]
    # other layers that lose (or take) the key
    warn = []
    probe = type(row)(layer=row.layer, view=row.view, chords=[nc], chord_text=new, action=row.action)
    others = [r for r in cat.keys if r.id != row.id]
    for c in conflicts.find(others + [probe]):
        if probe in (c.winner, c.loser) and c.level != "info":
            other = c.loser if c.winner is probe else c.winner
            warn.append(f"{other.layer} {other.view} {other.display} ({other.action}): {c.why}")
    if warn and not force:
        return 2, ["not bound — the new keys clash:"] + ["  " + w for w in warn] + ["add --force to bind anyway"]
    # write
    path = row.source_file
    line_no = row.line
    if line_no <= 0:
        return 1, [f"can't find {row.display} in {path}"]
    with open(os.path.realpath(path), encoding="utf-8") as fh:
        line = fh.read().split("\n")[line_no - 1]
    if layer == "aerospace":
        span = config_entry_span(line)
        if not span:
            return 1, [f"{path}:{line_no}: not a binding line"]
        new_line = line[:span[0]] + to_aerospace(nc.strokes[-1]) + line[span[1]:]
    else:
        try:
            nv = to_herdr(nc, prefix)
        except ValueError as e:
            return 2, [str(e)]
        span = config_entry_span(line)
        indent, tail = config_line_parts(line)
        key_text = line[span[0]:span[1]]
        data = tomllib.loads(line.strip() + "\n") if tomllib else {}
        v = next(iter(data.values()), "")
        if isinstance(v, list):
            v = [nv if x == row.chord_text else x for x in v]
            val = toml_array(v)
        else:
            val = toml_string(nv)
        new_line = f"{indent}{key_text} = {val}{tail}"
    label = {"kind": "bind", "layer": layer, "old": row.display, "new": nc.text, "action": row.action}
    try:
        res = writer.replace_line(path, line_no, line, new_line, label)
    except (writer.WriteError, OSError) as e:
        return 1, [f"not bound: {e}"]
    out = [f"bound: {layer} {row.view} {row.display} → {nc.text} ({row.action})  "
           f"({os.path.basename(res['file'])}:{line_no})"] + ["warning: " + w for w in warn]
    ok, msg = apply.aerospace_check() if layer == "aerospace" else apply.herdr_check()
    if not ok:
        undo.undo_last()
        return 1, out + [f"{layer} rejected the file, undone: {msg}"]
    if not no_apply:
        r = apply.aerospace() if layer == "aerospace" else apply.herdr()
        out.append(("applied: " if r["ok"] else "apply failed: ") + " — ".join(x for x in (r["ran"], r["note"]) if x))
        if not r["ok"]:
            return 1, out
    return 0, out
