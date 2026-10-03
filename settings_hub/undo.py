"""The last N edits (any file ws-settings wrote), newest last. Each record
is a hunk: `before` lines replaced by `after` lines at `at` (0-based).
Undo puts `before` back only when `after` is still there (byte-identical)."""
from __future__ import annotations

import json
import os
import time

from . import paths


def _log() -> str:
    return os.path.join(paths.cache_dir(), "settings-undo.json")


def _load() -> list:
    try:
        with open(_log(), encoding="utf-8") as fh:
            return json.load(fh)
    except (OSError, ValueError):
        return []


def _save(recs: list) -> None:
    limit = 20
    try:
        limit = max(1, int(paths.hub("undo-limit", "20")))
    except ValueError:
        pass
    tmp = _log() + ".tmp"
    with open(tmp, "w", encoding="utf-8") as fh:
        json.dump(recs[-limit:], fh, indent=1)
    os.replace(tmp, _log())


def record(real: str, old: list, new: list, label: dict) -> dict:
    i = 0
    while i < len(old) and i < len(new) and old[i] == new[i]:
        i += 1
    j_old, j_new = len(old), len(new)
    while j_old > i and j_new > i and old[j_old - 1] == new[j_new - 1]:
        j_old -= 1
        j_new -= 1
    rec = {"time": time.strftime("%Y-%m-%d %H:%M:%S"), "file": real, "at": i,
           "before": old[i:j_old], "after": new[i:j_new], **label}
    recs = _load()
    recs.append(rec)
    _save(recs)
    return rec


def entries() -> list:
    return _load()


def undo_last() -> dict:
    """Revert the newest record. → the record (+ "file"); raises on drift."""
    from .writer import WriteError, read, write_atomic
    recs = _load()
    if not recs:
        raise WriteError("nothing to undo")
    rec = recs[-1]
    real, text, _ = read(rec["file"])
    lines = text.split("\n")
    after, before = rec["after"], rec["before"]
    # the hunk's place may have moved (other edits above it): nearest match
    spots = [k for k in range(0, len(lines) - len(after) + 1) if lines[k:k + len(after)] == after] \
        if after else [rec["at"]]
    if not spots:
        raise WriteError(f"{real} changed since that edit; not undoing (see `ws-settings undo --list`)")
    k = min(spots, key=lambda s: abs(s - rec["at"]))
    new = lines[:k] + before + lines[k + len(after):]
    write_atomic(real, "\n".join(new))
    recs.pop()
    _save(recs)
    return rec
