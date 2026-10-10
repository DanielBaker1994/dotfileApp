from __future__ import annotations

import os
import shutil
import threading

_STACKS = {}
_NEXT = [1]
_ALLOC = threading.Lock()


def stack_new(limit: int = 50) -> int:
    with _ALLOC:
        handle = _NEXT[0]
        _NEXT[0] += 1
    _STACKS[handle] = {"limit": limit, "stack": []}
    return handle


def stack_drop(handle) -> None:
    _STACKS.pop(handle, None)


def stack_state(handle) -> dict:
    s = _STACKS.get(handle)
    return {"canUndo": bool(s and s["stack"]), "count": len(s["stack"]) if s else 0}


def stack_forget(handle) -> None:
    s = _STACKS.get(handle)
    if s:
        s["stack"] = []


def stack_collapse(handle, since: int, what: str) -> None:
    s = _STACKS.get(handle)
    if not s:
        return
    stack = s["stack"]
    if since < 0 or len(stack) - since <= 1:
        return
    tail = stack[since:]
    del stack[since:]
    stack.append({"kind": "group", "what": what, "records": tail})


def _push(handle, record) -> None:
    s = _STACKS.get(handle)
    if not s:
        return
    s["stack"].append(record)
    if len(s["stack"]) > s["limit"]:
        del s["stack"][:len(s["stack"]) - s["limit"]]


def free_url(name: str, directory: str) -> str:
    path = os.path.join(directory, name)
    base, ext = os.path.splitext(name)
    n = 2
    while os.path.exists(path):
        path = os.path.join(directory, "%s %d%s" % (base, n, ext))
        n += 1
    return path


def copy_url(path: str) -> str:
    name = os.path.basename(path)
    base, ext = os.path.splitext(name)
    stem = name if not base else base
    copy = ("%s copy" % stem) if (not ext or not base) else ("%s copy%s" % (stem, ext))
    return free_url(copy, os.path.dirname(path))


def _note(out: dict, name: str, error: Exception) -> None:
    if out["failed"] is None:
        out["failed"] = "%s: %s" % (name, error)


def _copy(src: str, dst: str) -> None:
    if os.path.islink(src) or not os.path.isdir(src):
        shutil.copy2(src, dst, follow_symlinks=False)
    else:
        shutil.copytree(src, dst, symlinks=True)


def transfer(paths: list, into: str, move: bool, handle) -> dict:
    out = {"changes": [], "failed": None}
    dest = os.path.normpath(into)
    for p in paths:
        src = os.path.normpath(p)
        if dest == src or dest.startswith(src + "/"):
            if out["failed"] is None:
                out["failed"] = "%s: can't go inside itself" % os.path.basename(p)
            continue
        if move and os.path.normpath(os.path.dirname(src)) == dest:
            continue
        target = free_url(os.path.basename(p), into)
        try:
            if move:
                shutil.move(src, target)
            else:
                _copy(src, target)
            out["changes"].append([src if move else None, target])
        except OSError as e:
            _note(out, os.path.basename(p), e)
    if out["changes"]:
        if move:
            _push(handle, {"kind": "moved", "pairs": [[c[0], c[1]] for c in out["changes"]]})
        else:
            _push(handle, {"kind": "created", "paths": [c[1] for c in out["changes"]]})
    return out


def duplicate(paths: list, handle) -> dict:
    out = {"changes": [], "failed": None}
    for p in paths:
        target = copy_url(p)
        try:
            _copy(p, target)
            out["changes"].append([None, target])
        except OSError as e:
            _note(out, os.path.basename(p), e)
    if out["changes"]:
        _push(handle, {"kind": "created", "paths": [c[1] for c in out["changes"]]})
    return out


def _trash(paths: list) -> dict:
    out = {"changes": [], "failed": None}
    trash_dir = os.path.expanduser("~/.Trash")
    for p in paths:
        try:
            target = free_url(os.path.basename(p), trash_dir)
            shutil.move(p, target)
            out["changes"].append([p, target])
        except OSError as e:
            _note(out, os.path.basename(p), e)
    return out


def trash(paths: list, handle) -> dict:
    out = _trash(paths)
    if out["changes"]:
        _push(handle, {"kind": "trashed", "pairs": [[c[0], c[1]] for c in out["changes"]]})
    return out


def create(name: str, directory: str, folder: bool, handle) -> dict:
    out = {"changes": [], "failed": None}
    target = free_url(name, directory)
    try:
        if folder:
            os.mkdir(target)
        else:
            open(target, "x").close()
        out["changes"].append([None, target])
        _push(handle, {"kind": "created", "paths": [target]})
    except OSError as e:
        _note(out, os.path.basename(target), e)
    return out


def record_rename(frm: str, to: str, handle) -> None:
    _push(handle, {"kind": "moved", "pairs": [[frm, to]]})


def _move_back(pairs: list) -> dict:
    out = {"changes": [], "failed": None}
    for frm, to in reversed(pairs):
        same_file = frm.lower() == to.lower()
        if os.path.exists(frm) and not same_file:
            if out["failed"] is None:
                out["failed"] = "%s: something else is there now" % os.path.basename(frm)
            continue
        try:
            shutil.move(to, frm)
            out["changes"].append([to, frm])
        except OSError as e:
            _note(out, os.path.basename(to), e)
    return out


def place(items: list, move: bool, clash: str, handle) -> dict:
    out = {"changes": [], "failed": None}
    group = []
    trashed = []
    made = []
    moved = []
    for src, want in items:
        if want == src or want.startswith(src + "/"):
            if out["failed"] is None:
                out["failed"] = "%s: can't go inside itself" % os.path.basename(src)
            continue
        dst = want
        if os.path.exists(dst):
            if clash == "skip":
                continue
            if clash == "keepBoth":
                dst = free_url(os.path.basename(want), os.path.dirname(want))
            else:
                t = _trash([dst])
                if t["failed"]:
                    if out["failed"] is None:
                        out["failed"] = t["failed"]
                    continue
                trashed += [[c[0] or "", c[1]] for c in t["changes"]]
        parent = os.path.dirname(dst)
        first_new = None
        while parent not in ("/", "") and not os.path.exists(parent):
            first_new = parent
            parent = os.path.dirname(parent)
        try:
            os.makedirs(os.path.dirname(dst), exist_ok=True)
            if move:
                shutil.move(src, dst)
            else:
                _copy(src, dst)
            out["changes"].append([src if move else None, dst])
            if first_new:
                made.append(first_new)
            if move:
                moved.append([src, dst])
            elif first_new is None:
                made.append(dst)
        except OSError as e:
            _note(out, os.path.basename(src), e)
    top = {m for m in made if not any(o != m and m.startswith(o + "/") for o in made)}
    made_top = []
    for m in made:
        if m in top and m not in made_top:
            made_top.append(m)
    if trashed:
        group.append({"kind": "trashed", "pairs": trashed})
    if made_top:
        group.append({"kind": "created", "paths": made_top})
    if moved:
        group.append({"kind": "moved", "pairs": moved})
    if group:
        n = len(out["changes"])
        name = os.path.basename(out["changes"][0][1]) if out["changes"] else ""
        what = ("move" if move else "copy") + (" of %s" % name if n == 1 else " of %d items" % n)
        _push(handle, {"kind": "group", "what": what, "records": group})
    return out


def undo(handle):
    s = _STACKS.get(handle)
    if not s or not s["stack"]:
        return None
    what, outcome = _undo_record(s["stack"].pop())
    return {"what": what, "outcome": outcome}


def _n(c: int, one: str) -> str:
    return one if c == 1 else "%d items" % c


def _undo_record(record: dict):
    kind = record.get("kind")
    if kind == "group":
        out = {"changes": [], "failed": None}
        for rec in reversed(record["records"]):
            _, u = _undo_record(rec)
            out["changes"] += u["changes"]
            if out["failed"] is None:
                out["failed"] = u["failed"]
        return record["what"], out
    if kind == "moved":
        pairs = record["pairs"]
        one = "move"
        if pairs:
            frm, to = pairs[0]
            same = os.path.dirname(frm) == os.path.dirname(to)
            one = ("rename of " if same else "move of ") + os.path.basename(frm)
        what = one if len(pairs) == 1 else "move of %d items" % len(pairs)
        return what, _move_back(pairs)
    if kind == "created":
        paths = record["paths"]
        live = [p for p in paths if os.path.exists(p)]
        one = os.path.basename(paths[0]) if paths else ""
        return "creating " + _n(len(paths), one), _trash(live)
    pairs = record["pairs"]
    one = os.path.basename(pairs[0][0]) if pairs else ""
    return "trash of " + _n(len(pairs), one), _move_back(pairs)
