"""Path shelf store: canonical paths, the normalized store list, rename
re-keying and the paths.json format. Swift keeps the queue, timers and the
lock-guarded snapshot plumbing."""
from __future__ import annotations

import json
import os
import stat as stat_mod
from urllib.parse import unquote, urlsplit

MAX_LIMIT = 25
WHYS = ("created", "modified", "downloaded", "clipboard", "filefast", "copied", "screenshot")
ACTIVITY_WHYS = ("created", "modified", "downloaded")


def canonical(p):
    """Full realpath + stat: {"path", "isFile", "isDir"} or None (missing
    paths, sockets and devices are not files)."""
    try:
        real = os.path.realpath(p)
        st = os.stat(real)
    except (OSError, TypeError, ValueError):
        return None
    return {"path": real,
            "isFile": stat_mod.S_ISREG(st.st_mode),
            "isDir": stat_mod.S_ISDIR(st.st_mode)}


def normalize(p):
    s = p or ""
    if s.startswith("file://"):
        u = urlsplit(s)
        if u.scheme == "file":
            s = unquote(u.path)
    s = os.path.expanduser(s)
    s = os.path.normpath(s)
    if s == "/tmp" or s.startswith("/tmp/"):
        s = "/private" + s
    return s


def bump(items, path, why, at, limit):
    """Move/insert to the top; an edit keeps the last WHY, anything else
    replaces it; cap at limit."""
    items = [dict(i) for i in items or []]
    idx = next((k for k, i in enumerate(items) if i.get("path") == path), None)
    if idx is not None:
        it = items.pop(idx)
        it["at"] = at
        if why != "modified":
            it["why"] = why
        items.insert(0, it)
    else:
        items.insert(0, {"path": path, "at": at, "why": why})
    if len(items) > limit:
        del items[limit:]
    return items


def dedup(items):
    seen = set()
    out = []
    for i in items or []:
        p = i.get("path")
        if p in seen:
            continue
        seen.add(p)
        out.append(dict(i))
    return out


def finalize(items, limit):
    """Newest first, deduplicated, capped."""
    ordered = sorted((dict(i) for i in items or []),
                     key=lambda i: i.get("at") or 0, reverse=True)
    return dedup(ordered)[:limit]


def load_candidates(raw):
    """paths.json entries -> the ones that still exist and pass the
    activity rule (dirs only ever come from non-activity whys). The app
    filters its ignore rules between this and finalize()."""
    out = []
    for d in raw if isinstance(raw, list) else []:
        if not isinstance(d, dict):
            continue
        p, t = d.get("path"), d.get("at")
        if not isinstance(p, str) or isinstance(t, bool) or not isinstance(t, (int, float)):
            continue
        c = canonical(p)
        if c is None:
            continue
        why = d.get("why") if d.get("why") in WHYS else "modified"
        keep = c["isFile"] if why in ACTIVITY_WHYS else (c["isFile"] or c["isDir"])
        if not keep:
            continue
        out.append({"path": c["path"], "at": float(t), "why": why})
    return out


def save(path, items) -> None:
    try:
        os.makedirs(os.path.dirname(path), exist_ok=True)
        data = [{"path": i.get("path"), "at": i.get("at"), "why": i.get("why")}
                for i in items or []]
        with open(path, "w", encoding="utf-8") as fh:
            json.dump(data, fh)
    except OSError:
        pass


def rekey(p, old, new):
    """The path after a rename of `old` to `new`, else None."""
    if p == old:
        return new
    if p.startswith(old + "/"):
        return new + p[len(old):]
    return None
