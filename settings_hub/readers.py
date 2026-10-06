"""Key readers: one per source, each → [KeyRow]. Read-only; never raise on
a missing or broken file (the catalog gets a warning instead)."""
from __future__ import annotations

import hashlib
import json
import os
import re
import subprocess

try:
    import tomllib
except ImportError:  # python < 3.11: bin/ws-settings refuses earlier
    tomllib = None

from . import chords, paths
from .model import Catalog, KeyRow

from config_text import (config_entry, config_section_entries,  # noqa: E402
                         config_section_header)


def _read(path: str):
    try:
        with open(path, encoding="utf-8") as fh:
            return fh.read()
    except OSError:
        return None


def comment_above(lines: list, i: int) -> str:
    """The contiguous `#` comment lines right above line index i, joined."""
    out = []
    j = i - 1
    while j >= 0 and lines[j].strip().startswith("#"):
        out.append(lines[j].strip().lstrip("#").strip())
        j -= 1
    return " ".join(reversed([o for o in out if o]))


# ------------------------------------------------------------------ app
APP_RO = "built into the app (the [shortcuts] line is its label; the text is editable)"


def app_shortcuts(cat: Catalog) -> list:
    path = paths.commands_conf()
    text = _read(path)
    cat.sources["app"] = path if text is not None else ""
    if text is None:
        return []
    lines = text.split("\n")
    rows = []
    for i, key, what in config_section_entries(lines, "shortcuts"):
        view, sep, label = key.partition(":")
        if not sep:
            cat.warn(path, i + 1, f"[shortcuts] {key!r}: no 'view:' prefix")
            view, label = "all", key
        view, label = view.strip(), label.strip()
        cs, gestures = chords.app_label(label)
        rows.append(KeyRow(layer="app", view=view, chords=cs, chord_text=label, action=what,
                           source_file=path, line=i + 1, readonly_reason=APP_RO,
                           kind="key" if cs else "gesture", raw=lines[i]))
    return rows


# ------------------------------------------------------------------ aerospace
def _scan_tables(lines: list) -> dict:
    """{(table, key): line index} for `key = …` lines (codec per line)."""
    out, cur = {}, None
    for i, raw in enumerate(lines):
        h = config_section_header(raw)
        if h is not None:
            cur = h
            continue
        e = config_entry(raw)
        if e and cur is not None:
            out.setdefault((cur, e[0]), i)
    return out


def _action_text(v) -> str:
    return " ; ".join(v) if isinstance(v, list) else str(v)


def aerospace(cat: Catalog) -> list:
    path = paths.aerospace_conf()
    text = _read(path)
    cat.sources["aerospace"] = path if text is not None else ""
    if text is None or tomllib is None:
        return []
    try:
        data = tomllib.loads(text)
    except Exception as e:  # noqa: BLE001
        cat.warn(path, 0, f"aerospace.toml does not parse: {e}")
        return []
    lines = text.split("\n")
    where = _scan_tables(lines)
    modes = data.get("mode", {})
    # the service mode's entry key (main binding whose action is `mode service`)
    entries = {}
    for k, v in modes.get("main", {}).get("binding", {}).items():
        for name in modes:
            if name != "main" and f"mode {name}" in _action_text(v):
                entries[name] = chords.aerospace(k)
    rows = []
    for mode, body in modes.items():
        for k, v in body.get("binding", {}).items():
            c = chords.aerospace(k)
            i = where.get((f"mode.{mode}.binding", k), -1)
            if c is None:
                cat.warn(path, i + 1, f"aerospace binding {k!r}: can't read the chord")
                continue
            if mode != "main" and entries.get(mode):
                c = chords.chord(entries[mode].first, c.first)
            rows.append(KeyRow(layer="aerospace", view=mode, chords=[c], chord_text=k,
                               action=_action_text(v), source_file=path, line=i + 1,
                               doc=comment_above(lines, i) if i >= 0 else "",
                               editable=True, raw=lines[i] if i >= 0 else ""))
    return rows


def link_mirrors(rows: list) -> None:
    """An aerospace binding that runs our binary / launcher takes its action
    text from the [shortcuts] `all:` row with the same chord."""
    app = {}
    for r in rows:
        if r.layer == "app" and r.view == "all":
            for c in r.chords:
                app.setdefault(c.text, r)
    for r in rows:
        if r.layer != "aerospace" or not r.chords:
            continue
        if "kitchen-sink" in r.action or "kitchen_sink" in r.action or "ws-settings" in r.action:
            hit = app.get(r.chords[0].text)
            if hit:
                r.mirror_of = hit.id
                r.doc = (r.action + ("  ·  " + r.doc if r.doc else ""))
                r.action = hit.action


# ------------------------------------------------------------------ herdr
def herdr(cat: Catalog) -> list:
    path = paths.herdr_conf()
    text = _read(path)
    cat.sources["herdr"] = path if text is not None else ""
    if text is None or tomllib is None:
        return []
    try:
        data = tomllib.loads(text)
    except Exception as e:  # noqa: BLE001
        cat.warn(path, 0, f"herdr config does not parse: {e}")
        return []
    lines = text.split("\n")
    keys = data.get("keys", {}) or {}
    prefix = chords.plus_stroke(keys.get("prefix") or "ctrl+b")
    where = _scan_tables(lines)
    # [[keys.command]] blocks in order → the line of each one's `key =`
    cmd_lines, cur = [], None
    for i, raw in enumerate(lines):
        if raw.strip() == "[[keys.command]]":
            cur = len(cmd_lines)
            cmd_lines.append(-1)
            continue
        if config_section_header(raw) is not None:
            cur = None
            continue
        e = config_entry(raw)
        if cur is not None and e and e[0] == "key":
            cmd_lines[cur] = i
    rows = []
    for action, v in keys.items():
        if action in ("command", "prefix"):
            continue
        vals = v if isinstance(v, list) else [v]
        i = where.get(("keys", action), -1)
        for s in vals:
            if not isinstance(s, str) or not s:
                continue
            c = chords.herdr(s, prefix)
            if c is None:
                cat.warn(path, i + 1, f"herdr {action} = {s!r}: can't read the chord")
                continue
            rows.append(KeyRow(layer="herdr", view="terminal", chords=[c], chord_text=s,
                               action=action.replace("_", " "), source_file=path, line=i + 1,
                               doc=comment_above(lines, i) if i >= 0 else "", editable=True,
                               raw=lines[i] if i >= 0 else "", id=f"herdr:{action}:{s}"))
    for n, cmd in enumerate(keys.get("command", []) or []):
        s = cmd.get("key", "")
        c = chords.herdr(s, prefix)
        i = cmd_lines[n] if n < len(cmd_lines) else -1
        if c is None:
            cat.warn(path, i + 1, f"herdr command key {s!r}: can't read the chord")
            continue
        what = cmd.get("description") or f"{cmd.get('type', 'command')}: {cmd.get('command', '')}"
        head = i
        while head > 0 and lines[head].strip() != "[[keys.command]]":
            head -= 1
        rows.append(KeyRow(layer="herdr", view="terminal", chords=[c], chord_text=s, action=what,
                           source_file=path, line=i + 1,
                           doc=comment_above(lines, head) if head >= 0 else "",
                           editable=True, raw=lines[i] if i >= 0 else "",
                           id=f"herdr:command:{s}"))
    return rows


# ------------------------------------------------------------------ ghostty
def ghostty(cat: Catalog) -> list:
    path = paths.ghostty_conf()
    text = _read(path)
    cat.sources["ghostty"] = path if text is not None else ""
    if text is None:
        return []
    lines = text.split("\n")
    rows = []
    for i, raw in enumerate(lines):
        m = re.match(r"\s*keybind\s*=\s*(.+?)\s*$", raw)
        if not m:
            continue
        spec = m.group(1)
        view = "terminal"
        while True:
            pm = re.match(r"(global|all|unconsumed|performable):", spec)
            if not pm:
                break
            if pm.group(1) == "global":
                view = "global"
            spec = spec[pm.end():]
        # the trigger ends at the first '=' that isn't the key itself ("super+=")
        cut = spec.find("=", 1)
        while cut > 0 and spec[cut - 1] == "+":
            cut = spec.find("=", cut + 1)
        if cut < 0:
            continue
        trig, action = spec[:cut], spec[cut + 1:]
        c = chords.ghostty(trig)
        if c is None:
            cat.warn(path, i + 1, f"ghostty keybind {trig!r}: can't read the chord")
            continue
        if action == "unbind":
            action = "unbound (the key goes to the program in the terminal)"
        rows.append(KeyRow(layer="ghostty", view=view, chords=[c], chord_text=trig,
                           action=action.replace("_", " "), source_file=path, line=i + 1,
                           doc=comment_above(lines, i), raw=raw,
                           readonly_reason="Ghostty's config (edit it in Ghostty)"))
    return rows


# ------------------------------------------------------------------ vim
VIM_MODES = {"n": "normal", "x": "visual", "v": "visual", "s": "select", "o": "operator",
             "i": "insert", "c": "command", "t": "terminal"}
_LUA = ('lua local p=vim.fn.fnamemodify(%s,":p"); local i=vim.fn.getscriptinfo({name=p})[1];'
        'local out={}; if i then for _,m in ipairs({"n","x","s","o","i","c","t"}) do '
        'for _,k in ipairs(vim.api.nvim_get_keymap(m)) do if k.sid==i.sid then '
        'table.insert(out,{mode=m,lhs=k.lhs,rhs=k.rhs or "",lnum=k.lnum,desc=k.desc or ""}) '
        'end end end end vim.fn.writefile({vim.json.encode(out)}, %s)')


def _vim_maps_nvim(init: str):
    """nvim's own view of the maps the init file set (loops included).
    Cached by the file's path + mtime. None = nvim unavailable / failed."""
    try:
        st = os.stat(init)
    except OSError:
        return None
    key = hashlib.sha1(f"{os.path.realpath(init)}:{st.st_mtime_ns}:{st.st_size}".encode()).hexdigest()[:16]
    cache = os.path.join(paths.cache_dir(), f"settings-vim-{key}.json")
    try:
        with open(cache, encoding="utf-8") as fh:
            return json.load(fh)
    except (OSError, ValueError):
        pass
    out = cache + ".tmp"
    nvim = paths.vim_bin()
    try:
        subprocess.run([nvim, "--headless", "-n", "-i", "NONE", "--noplugin", "-u", init,
                        "+" + _LUA % (json.dumps(init), json.dumps(out)), "+qa!"],
                       stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                       stderr=subprocess.DEVNULL, timeout=5, check=False)
        with open(out, encoding="utf-8") as fh:
            got = json.load(fh)
        os.replace(out, cache)
        return got
    except (OSError, ValueError, subprocess.SubprocessError):
        return None


_MAP = re.compile(r"^\s*([nvxsoilct]?)(?:nore)?map!?\s+(?:<(?:silent|buffer|expr|nowait|unique)>\s*)*(\S+)\s+(.*)$")


def vim(cat: Catalog) -> list:
    init = paths.vim_init()
    text = _read(init)
    cat.sources["vim"] = init if text is not None else ""
    if text is None:
        return []
    lines = text.split("\n")
    maps = _vim_maps_nvim(init) if init.endswith(".vim") or init.endswith(".lua") else None
    if maps is None:
        maps = []
        for i, raw in enumerate(lines):
            m = _MAP.match(raw)
            if m:
                maps.append({"mode": m.group(1) or "n", "lhs": m.group(2), "rhs": m.group(3), "lnum": i + 1})
        if re.search(r"^\s*(for|execute)\b", text, re.M):
            cat.warn(init, 0, "some vim maps are made by a loop / execute; "
                              "only literal map lines are listed (nvim not available)")
    rows = []
    for m in maps:
        c = chords.vim(m["lhs"])
        if c is None:
            continue
        i = int(m.get("lnum") or 0) - 1
        mode = VIM_MODES.get(m["mode"], m["mode"])
        rows.append(KeyRow(layer="vim", view=f"notes vim ({mode})", chords=[c], chord_text=m["lhs"],
                           action=m.get("desc") or f"→ {m['rhs']}", source_file=init, line=i + 1,
                           doc=comment_above(lines, i) if i >= 0 else "",
                           readonly_reason="the notes vim pane's init file",
                           id=f"vim:{m['mode']}:{m['lhs']}"))
    return rows


READERS = {"app": app_shortcuts, "aerospace": aerospace, "herdr": herdr,
           "ghostty": ghostty, "vim": vim}
