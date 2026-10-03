"""★ favorites: a list of row ids in the home's settings-hub.json (user
data: gitignored in repo mode, never inside the app bundle)."""
from __future__ import annotations

import json
import os
import tempfile

from . import paths


def load() -> set:
    try:
        with open(paths.favorites_path(), encoding="utf-8") as fh:
            return set(json.load(fh).get("favorites", []))
    except (OSError, ValueError, AttributeError):
        return set()


def save(fav: set) -> None:
    p = paths.favorites_path()
    real = os.path.realpath(p)
    data = {}
    try:
        with open(real, encoding="utf-8") as fh:
            data = json.load(fh)
    except (OSError, ValueError):
        pass
    data["favorites"] = sorted(fav)
    fd, tmp = tempfile.mkstemp(dir=os.path.dirname(real), prefix=".settings-hub.")
    with os.fdopen(fd, "w", encoding="utf-8") as fh:
        json.dump(data, fh, indent=2)
        fh.write("\n")
    os.replace(tmp, real)


def toggle(row_id: str) -> bool:
    fav = load()
    on = row_id not in fav
    (fav.add if on else fav.discard)(row_id)
    save(fav)
    return on
