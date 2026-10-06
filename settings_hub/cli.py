"""ws-settings CLI. Exit 0 ok, 1 error, 2 bad input; errors on stderr."""
from __future__ import annotations

import argparse
import json
import os
import shlex
import shutil
import subprocess
import sys
import time

from . import apply, catalog, chords, conflicts, doctor, export, favorites, parity, paths, schema, search
from . import rebind, undo, writer


def err(msg: str, code: int = 1) -> int:
    print(f"ws-settings: {msg}", file=sys.stderr)
    return code


def _table(rows: list, widths: list) -> None:
    cols = shutil.get_terminal_size((120, 40)).columns
    fixed = sum(widths[:-1]) + 2 * (len(widths) - 1)
    last = max(20, cols - fixed)
    for r in rows:
        cells = []
        for i, c in enumerate(r):
            w = widths[i] if i < len(widths) - 1 else last
            c = str(c).replace("\n", " ")
            cells.append((c if len(c) <= w else c[:w - 1] + "…").ljust(w) if i < len(r) - 1 else c[:w])
        print("  ".join(cells).rstrip())


def _mods_arg(s: str | None) -> set:
    if not s:
        return set()
    out = set()
    for m in s.replace("+", ",").split(","):
        m = m.strip().lower()
        if not m:
            continue
        if m == "hyper":
            out |= {"ctrl", "alt", "shift", "cmd"}
        elif m in chords.MOD_WORDS:
            out.add(chords.MOD_WORDS[m])
        else:
            raise ValueError(f"unknown modifier {m!r} (ctrl, opt, shift, cmd, hyper)")
    return out


# ------------------------------------------------------------------ verbs
def cmd_keys(a) -> int:
    try:
        mods = _mods_arg(a.mods)
    except ValueError as e:
        return err(str(e), 2)
    cat = catalog.build(with_settings=False, only=[a.layer] if a.layer else None)
    rows = cat.keys
    if a.view:
        rows = [r for r in rows if r.view == a.view or r.view.startswith(a.view)]
    fav = favorites.load() if a.favorites else None
    rows = search.filter_rows(rows, " ".join(a.query), mods, fav)
    if a.json:
        print(json.dumps([export.key_json(r) for r in rows], indent=2, ensure_ascii=False))
        return 0
    if not rows:
        return err("no matching keys", 1)
    fav = favorites.load()
    _table([("★" if r.id in fav else " ", r.layer, r.view, r.display, r.action) for r in rows],
           [1, 9, 18, 26, 0])
    return 0


def cmd_settings(a) -> int:
    cat = catalog.build(only=[])
    rows = cat.settings
    if a.section:
        rows = [r for r in rows if r.section == a.section]
        if not rows:
            return err(f"no section [{a.section}]", 2)
    rows = search.filter_rows(rows, " ".join(a.query))
    if a.json:
        print(json.dumps([export.setting_json(s) for s in rows], indent=2, ensure_ascii=False))
        return 0
    if not rows:
        return err("no matching settings", 1)
    _table([(f"[{s.section}]", s.key, s.value if s.set else f"({s.value})", s.doc or s.line_doc)
            for s in rows], [16, 22, 30, 0])
    return 0


def _split_id(sk: str):
    if "." not in sk:
        return None
    sec, key = sk.split(".", 1)
    return (sec, key) if sec and key else None


def _find_setting(sec: str, key: str):
    cat = catalog.build(only=[])
    for s in cat.settings:
        if s.section == sec and s.key == key:
            return s
    return None


def cmd_get(a) -> int:
    sk = _split_id(a.key)
    if not sk:
        return err("use SECTION.KEY, e.g. screenshot.save-path", 2)
    row = _find_setting(*sk)
    if row is None:
        return err(f"[{sk[0]}] {sk[1]} is not set", 1)
    if a.json:
        print(json.dumps(export.setting_json(row), indent=2, ensure_ascii=False))
    else:
        print(row.value if row.set else "")
    return 0


def do_set(sec: str, key: str, value: str, force: bool = False, no_apply: bool = False) -> tuple:
    """Shared by the CLI and the picker → (code, [lines to show])."""
    if sec == "jira" and key == "enabled":
        if value not in ("true", "false"):
            return 2, ["[jira] enabled is true or false"]
        b = paths.app_binary()
        rc = subprocess.run([b, "jira-poll", "on" if value == "true" else "off"],
                            capture_output=True, text=True).returncode if os.access(b, os.X_OK) else 127
        if rc == 0:
            return 0, ["ran: workspace-switcher jira-poll " + ("on" if value == "true" else "off"),
                       "  (the app's own switch: it checks the setup and starts / stops the poller)"]
        return 1, ["[jira] enabled goes through the running app (workspace-switcher jira-poll on|off),",
                   "which isn't running; start the app first"]
    row = _find_setting(sec, key)
    errors, warnings = schema.validate(sec, key, value, row)
    if errors:
        return 2, [f"refused: [{sec}] {key} = {value!r}: {e}" for e in errors]
    if warnings and not force:
        return 2, [f"not written: {w}" for w in warnings] + ["add --force to write it anyway"]
    path = paths.commands_conf()

    def check(text: str):
        tmp = os.path.join(paths.cache_dir(), "commands.check.toml")
        with open(tmp, "w", encoding="utf-8") as fh:
            fh.write(text)
        try:
            return schema.check_file(tmp)
        finally:
            os.unlink(tmp)
    try:
        res = writer.set_setting(path, sec, key, value, check_file=check)
    except (writer.WriteError, OSError) as e:
        return 1, [f"not written: {e}"]
    out = []
    if not res["changed"]:
        return 0, [f"[{sec}] {key} is already {value!r}"]
    verb = {"replace": "changed", "insert": "added", "uncomment": "un-commented"}[res["op"]]
    out.append(f"{verb}: [{sec}] {key} = {value!r}  ({os.path.basename(res['file'])}:{res['line']})")
    out += [f"warning: {w}" for w in warnings]
    if not no_apply:
        from .settings import apply_mode
        r = apply.setting(apply_mode(sec, key))
        if r["ran"] or r["note"]:
            out.append(("applied: " if r["ok"] else "apply failed: ") + " — ".join(x for x in (r["ran"], r["note"]) if x))
        if not r["ok"]:
            return 1, out
    return 0, out


def cmd_set(a) -> int:
    sk = _split_id(a.key)
    if not sk:
        return err("use SECTION.KEY VALUE, e.g. screenshot.contrast-opacity 150", 2)
    code, lines = do_set(sk[0], sk[1], a.value, a.force, a.no_apply)
    for l in lines:
        print(l, file=sys.stderr if code else sys.stdout)
    return code


def cmd_undo(a) -> int:
    if a.list:
        recs = undo.entries()
        if not recs:
            print("nothing to undo")
        for r in reversed(recs):
            what = f"[{r.get('section')}] {r.get('key')}" if r.get("kind") == "setting" else \
                f"{r.get('layer')} {r.get('old')} → {r.get('new')}"
            print(f"{r['time']}  {os.path.basename(r['file'])}  {what}")
        return 0
    try:
        rec = undo.undo_last()
    except (writer.WriteError, OSError) as e:
        return err(str(e))
    print(f"undone: {os.path.basename(rec['file'])} restored ({rec['time']} edit)")
    if not a.no_apply:
        if rec.get("kind") == "setting":
            from .settings import apply_mode
            r = apply.setting(apply_mode(rec["section"], rec["key"]))
        elif rec.get("layer") == "aerospace":
            r = apply.aerospace()
        elif rec.get("layer") == "herdr":
            r = apply.herdr()
        else:
            r = {"ok": True, "ran": "", "note": ""}
        if r["ran"] or r["note"]:
            print(("applied: " if r["ok"] else "apply failed: ") + " — ".join(x for x in (r["ran"], r["note"]) if x))
    return 0


def cmd_bind(a) -> int:
    code, lines = rebind.bind(a.layer, a.old, a.new, mode=a.mode, force=a.force, no_apply=a.no_apply)
    for l in lines:
        print(l, file=sys.stderr if code else sys.stdout)
    return code


def cmd_conflicts(a) -> int:
    cat = catalog.build(with_settings=False)
    found = conflicts.find(cat.keys, with_system=not a.no_system)
    if not a.all:
        found = [c for c in found if c.level != "info"]
    if a.json:
        print(json.dumps([{"level": c.level, "keys": c.chord, "why": c.why,
                           "winner": export.key_json(c.winner), "loser": export.key_json(c.loser)}
                          for c in found], indent=2, ensure_ascii=False))
        return 0
    if not found:
        print("no clashes")
        return 0
    _table([(c.level, c.chord, f"{c.winner.layer} {c.winner.view}: {c.winner.action}",
             f"vs {c.loser.layer} {c.loser.view}: {c.loser.action}  — {c.why}") for c in found],
           [5, 16, 44, 0])
    return 0


def cmd_parity(a) -> int:
    cat = catalog.build(with_settings=False, only=["app"])
    try:
        found = parity.check(cat.keys, view=a.view)
    except (OSError, ValueError) as e:
        return err(f"parity data: {e}")
    if a.view and not found:
        return err(f"no view {a.view!r} in parity.toml", 2)
    shown = found if a.all else [c for c in found if c.status == "missing"]
    if a.json:
        print(json.dumps([parity.as_json(c) for c in shown], indent=2, ensure_ascii=False))
        return 0
    if not shown:
        print(f"nothing missing ({len(found)} expectations checked)")
        return 0
    _table([(c.status, c.view, c.kind, c.concept,
             (", ".join(c.missing) if c.status != "have" else ", ".join(c.keys))
             + (f"  — VS Code: {c.vscode}" if c.vscode else "") + (f"  — {c.reason}" if c.reason else ""))
            for c in shown], [7, 15, 8, 34, 0])
    gaps = sum(c.status == "missing" for c in found)
    print(f"\n{gaps} missing of {len(found)} expectations (--all shows the met + waived ones too)")
    return 0


def cmd_export(a) -> int:
    cat = catalog.build()
    text = export.to_json(cat) if a.format == "json" else export.to_markdown(cat, not a.keys_only)
    if a.out:
        with open(os.path.expanduser(a.out), "w", encoding="utf-8") as fh:
            fh.write(text + "\n")
        print(f"wrote {a.out}")
    else:
        print(text)
    return 0


def cmd_doctor(a) -> int:
    cat = catalog.build()
    res = doctor.run(cat, verbose=a.verbose)
    if a.json:
        print(json.dumps([{"level": l, "message": m} for l, m in res], indent=2, ensure_ascii=False))
    else:
        mark = {"ok": "✓", "info": "·", "warn": "!", "error": "✗"}
        for l, m in res:
            print(f"{mark[l]} {m}")
    return 1 if any(l == "error" for l, _ in res) else 0


def cmd_fav(a) -> int:
    cat = catalog.build()
    ids = {r.id for r in cat.keys} | {s.id for s in cat.settings}
    if a.id not in ids:
        return err(f"no row {a.id!r} (ids: `ws-settings keys --json` / `settings --json`)", 2)
    on = favorites.toggle(a.id)
    print(("★ " if on else "☆ ") + a.id)
    return 0


def cmd_tui(a) -> int:
    from .tui import app
    return app.run(timing=a.time)


def cmd_open(a) -> int:
    """Hyper+/: focus the picker window if it's up, else start one."""
    title = paths.hub("window-title", "ws-settings")
    aero = shutil.which("aerospace")

    def aq(*args) -> str:
        try:
            return subprocess.run(["aerospace", *args], capture_output=True, text=True, timeout=3).stdout
        except (OSError, subprocess.SubprocessError):
            return ""

    def find() -> str:
        for line in aq("list-windows", "--all", "--format", "%{window-id}|%{window-title}").splitlines():
            wid, _, t = line.partition("|")
            if t.strip() == title:
                return wid.strip()
        return ""

    def bring(wid: str, ws: str) -> None:
        # onto the workspace you're on, then focus (a new Ghostty instance
        # doesn't always take focus by itself)
        if ws:
            aq("move-node-to-workspace", "--window-id", wid, ws)
        aq("focus", "--window-id", wid)

    ws = aq("list-workspaces", "--focused").strip() if aero else ""
    if aero:
        wid = find()
        if wid:
            bring(wid, ws)
            return 0
    me = os.path.join(paths.ROOT, "bin", "ws-settings")
    # --command, not -e: launched through `open`, Ghostty asks "Allow Ghostty
    # to execute …?" for an -e program (and for ANY --keybind flag).
    # A transparent title bar with no window buttons = a plain strip in the
    # picker's own background to drag the window by (the owner's config hides
    # title bars: nothing to grab); {bg} = [theme] background, so the strip
    # and the picker are one surface. {x} {y} = centered on the screen
    # (window-position-x/y, points from the visible area's top-left)
    default = ("open -na Ghostty --args --title={title} --quit-after-last-window-closed=true "
               "--confirm-close-surface=false --macos-option-as-alt=true "
               "--macos-titlebar-style=transparent --macos-window-buttons=hidden {bg} "
               "--window-width=118 --window-height=36 "
               "--window-position-x={x} --window-position-y={y} --command={cmd}")
    tmpl = paths.hub("terminal-command", default)
    x, y = _center(aq if aero else None)
    tmpl = tmpl.replace("{x}", str(x)).replace("{y}", str(y)).replace("{bg}", _bg_flag())
    cmd = ["/usr/bin/env", f"WS_SETTINGS_T0={time.time():.3f}", me, "tui"]
    # shell-style words, no shell: {cmd} becomes the command's own words
    try:
        words = shlex.split(tmpl.replace("{title}", shlex.quote(title)))
    except ValueError as e:
        return err(f"[settings-hub] terminal-command: {e}", 2)
    argv = []
    for w in words:
        if w in ("{cmd}", "{}"):
            argv += cmd                                   # -e style: the words
        else:
            argv.append(w.replace("{cmd}", shlex.join(cmd)))   # --command={cmd}: one string
    subprocess.Popen(argv, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                     stderr=subprocess.DEVNULL, start_new_session=True)
    if aero:
        end = time.time() + 3
        while time.time() < end:
            wid = find()
            if wid:
                bring(wid, ws)
                break
            time.sleep(0.05)
    return 0


def _bg_flag() -> str:
    """--background=RRGGBB from [theme] background (AARRGGBB → its RGB), when
    the picker uses the theme's colors; else nothing."""
    import re
    if paths.hub("colors", "theme") != "theme":
        return ""
    v = paths.section("theme").get("background", "").strip().lstrip("#")
    if v.lower().startswith("0x"):
        v = v[2:]
    if re.fullmatch(r"[0-9A-Fa-f]{8}", v):
        v = v[2:]
    return f"--background={v}" if re.fullmatch(r"[0-9A-Fa-f]{6}", v) else ""


def _center(aq) -> tuple:
    """Top-left (points, from the primary screen's visible area) that centers
    the picker. Screen size: cached per monitor set (AppKit through JXA costs
    ~0.2 s); window size: what the picker measured last time (tui writes
    settings-hub-size.json), else the default 118×36 cells' 960×666."""
    cache = os.path.join(paths.cache_dir(), "settings-hub-geometry.json")
    monitors = aq("list-monitors") if aq else ""
    geo = {}
    try:
        with open(cache, encoding="utf-8") as fh:
            geo = json.load(fh)
    except (OSError, ValueError):
        pass
    if geo.get("monitors") != monitors or "vw" not in geo:
        js = ('ObjC.import("AppKit"); var s=$.NSScreen.screens.objectAtIndex(0); var v=s.visibleFrame;'
              'JSON.stringify({vw:v.size.width, vh:v.size.height, scale:s.backingScaleFactor})')
        try:
            out = subprocess.run(["osascript", "-l", "JavaScript", "-e", js], capture_output=True,
                                 text=True, timeout=3).stdout
            geo = dict(json.loads(out), monitors=monitors)
            with open(cache, "w", encoding="utf-8") as fh:
                json.dump(geo, fh)
        except (OSError, ValueError, subprocess.SubprocessError):
            geo = {"vw": 1440, "vh": 870, "scale": 2}
    ww, wh = 960, 666
    try:
        with open(os.path.join(paths.cache_dir(), "settings-hub-size.json"), encoding="utf-8") as fh:
            got = json.load(fh)
        sc = geo.get("scale") or 2
        ww, wh = int(got["xpix"] / sc) + 4, int(got["ypix"] / sc) + 34
    except (OSError, ValueError, KeyError, TypeError):
        pass
    return max(0, int((geo["vw"] - ww) / 2)), max(0, int((geo["vh"] - wh) / 2))


def main(argv=None) -> int:
    p = argparse.ArgumentParser(prog="ws-settings",
                                description="every shortcut and setting of the workspace-switcher setup")
    sub = p.add_subparsers(dest="verb")
    k = sub.add_parser("keys", help="list / search keyboard shortcuts")
    k.add_argument("query", nargs="*")
    k.add_argument("--layer", choices=list(catalog.ALL_LAYERS))
    k.add_argument("--view")
    k.add_argument("--mods", help="exact modifiers, e.g. cmd,shift or hyper")
    k.add_argument("--favorites", action="store_true")
    k.add_argument("--json", action="store_true")
    k.set_defaults(fn=cmd_keys)
    s = sub.add_parser("settings", help="list / search commands.toml settings")
    s.add_argument("query", nargs="*")
    s.add_argument("--section")
    s.add_argument("--json", action="store_true")
    s.set_defaults(fn=cmd_settings)
    g = sub.add_parser("get", help="print one setting: SECTION.KEY")
    g.add_argument("key")
    g.add_argument("--json", action="store_true")
    g.set_defaults(fn=cmd_get)
    st = sub.add_parser("set", help="change one setting: SECTION.KEY VALUE")
    st.add_argument("key")
    st.add_argument("value")
    st.add_argument("--force", action="store_true", help="write despite a warning")
    st.add_argument("--no-apply", action="store_true", help="don't reload / apply")
    st.set_defaults(fn=cmd_set)
    u = sub.add_parser("undo", help="revert the last edit")
    u.add_argument("--list", action="store_true")
    u.add_argument("--no-apply", action="store_true")
    u.set_defaults(fn=cmd_undo)
    b = sub.add_parser("bind", help="rebind an AeroSpace or herdr key: LAYER OLD NEW")
    b.add_argument("layer", choices=["aerospace", "herdr"])
    b.add_argument("old", help="the current keys (alt-h, opt+h, prefix+w) or a herdr action name")
    b.add_argument("new", help="the new keys (cmd+shift+y, hyper+/, prefix+g)")
    b.add_argument("--mode", default="main", help="aerospace binding mode (main, service)")
    b.add_argument("--force", action="store_true", help="bind even when another layer uses it")
    b.add_argument("--no-apply", action="store_true")
    b.set_defaults(fn=cmd_bind)
    c = sub.add_parser("conflicts", help="keys claimed twice")
    c.add_argument("--all", action="store_true", help="include info-level overlaps")
    c.add_argument("--no-system", action="store_true", help="skip the macOS shortcut list")
    c.add_argument("--json", action="store_true")
    c.set_defaults(fn=cmd_conflicts)
    e = sub.add_parser("export", help="cheat sheet (md) or the full catalog (json)")
    e.add_argument("format", choices=["md", "json"])
    e.add_argument("--out")
    e.add_argument("--keys-only", action="store_true")
    e.set_defaults(fn=cmd_export)
    pa = sub.add_parser("parity", help="VS Code parity: standard keys a view's panes are missing")
    pa.add_argument("--view", help="one view (notes, files, jira, confluence, compare, compare-folders, ai)")
    pa.add_argument("--all", action="store_true", help="also the expectations that are met or waived")
    pa.add_argument("--json", action="store_true")
    pa.set_defaults(fn=cmd_parity)
    d = sub.add_parser("doctor", help="sources, parse warnings, dead bindings")
    d.add_argument("--json", action="store_true")
    d.add_argument("--verbose", action="store_true")
    d.set_defaults(fn=cmd_doctor)
    f = sub.add_parser("fav", help="toggle ★ on a row id")
    f.add_argument("id")
    f.set_defaults(fn=cmd_fav)
    t = sub.add_parser("tui", help="the picker (this terminal)")
    t.add_argument("--time", action="store_true", help="log first-paint timing")
    t.set_defaults(fn=cmd_tui)
    o = sub.add_parser("open", help="Hyper+/: focus or start the picker window")
    o.set_defaults(fn=cmd_open)
    a = p.parse_args(argv)
    if not getattr(a, "fn", None):
        if sys.stdin.isatty() and sys.stdout.isatty():
            return cmd_tui(argparse.Namespace(time=False))
        p.print_help()
        return 2
    try:
        return a.fn(a)
    except BrokenPipeError:
        return 0
    except KeyboardInterrupt:
        return 130
