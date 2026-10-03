"""Health of the sources: found / missing, parse warnings, undocumented
settings, dead bindings (an exec target that doesn't exist)."""
from __future__ import annotations

import os
import re
import shlex
import sys

from . import paths
from .model import Catalog


def _exec_target(cmd: str) -> str | None:
    m = re.match(r"\s*exec(?:-and-forget)?\s+(.*)$", cmd)
    if not m:
        return None
    try:
        words = shlex.split(m.group(1))
    except ValueError:
        return None
    return words[0] if words else None


def _missing(target: str) -> bool:
    t = os.path.expanduser(target)
    if "/" not in t:
        return not any(os.access(os.path.join(d, t), os.X_OK)
                       for d in os.environ.get("PATH", "").split(":") if d)
    return not os.path.exists(t)


def run(cat: Catalog, verbose: bool = False) -> list:
    """[(level, message)] — level: ok | info | warn | error."""
    out = []
    for layer, p in cat.sources.items():
        if p:
            real = os.path.realpath(p)
            out.append(("ok", f"{layer}: {p}" + (f" → {real}" if real != p else "")))
        else:
            out.append(("warn", f"{layer}: source not found"))
    aero = paths.aerospace_conf()
    home_copy = os.path.join(paths.home(), "config", "aerospace", "aerospace.toml")
    if os.path.exists(home_copy) and os.path.exists(aero) and \
            os.path.realpath(aero) != os.path.realpath(home_copy):
        out.append(("warn", f"AeroSpace reads {aero}, not the repo's {home_copy}"))
    if os.path.exists(os.path.expanduser("~/.aerospace.toml")) and \
            os.path.exists(os.path.expanduser("~/.config/aerospace/aerospace.toml")):
        out.append(("warn", "both ~/.aerospace.toml and ~/.config/aerospace/aerospace.toml exist (AeroSpace refuses)"))
    for f, line, msg in cat.warnings:
        out.append(("warn", f"{os.path.basename(f)}:{line}: {msg}" if line else f"{os.path.basename(f)}: {msg}"))
    # dead bindings
    for r in cat.keys:
        if r.layer == "aerospace":
            for part in (r.raw.split("=", 1)[1:] or [""]):
                for cmd in re.findall(r"'([^']*)'|\"([^\"]*)\"", part):
                    t = _exec_target(cmd[0] or cmd[1])
                    if t and _missing(t):
                        out.append(("error", f"aerospace {r.display}: runs {t}, which doesn't exist"))
        if r.layer == "herdr" and r.action.startswith(("shell:", "popup:")):
            t = r.action.split(":", 1)[1].strip().split(" ")[0]
            if t and _missing(t):
                out.append(("error", f"herdr {r.display}: runs {t}, which doesn't exist"))
    # undocumented settings the app's schema doesn't know either
    from . import schema
    sc = schema.schema()
    known = set(sc.get("boolKeys", [])) | set(sc.get("numberRanges", {})) | \
        set(sc.get("colorKeys", [])) | set(sc.get("enumKeys", {}))
    undoc = {}
    for s in cat.settings:
        if s.set and not s.doc and not s.line_doc and s.key not in known:
            undoc.setdefault(s.section, []).append(s.key)
    n = sum(len(v) for v in undoc.values())
    if n:
        out.append(("info", f"{n} settings have no description in commands.toml"
                            + ("" if verbose else " (--verbose lists them)")))
        if verbose:
            for sec, keys in undoc.items():
                out.append(("info", f"  [{sec}] {', '.join(keys)}"))
    unparsed = [r for r in cat.keys if r.layer == "app" and not r.chords and r.kind != "gesture"]
    for r in unparsed:
        out.append(("warn", f"[shortcuts] {r.view}: {r.chord_text!r}: no key found in the label"))
    if sys.version_info < (3, 11):
        out.append(("error", "python < 3.11"))
    return out
