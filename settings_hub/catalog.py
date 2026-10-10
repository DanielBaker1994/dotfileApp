"""Build the whole catalog: every key reader + the settings."""
from __future__ import annotations

from .tables import LAYERS as ALL_LAYERS  # data/tables.json

from . import paths, readers, settings
from .model import Catalog



def layers() -> list:
    v = paths.hub("layers")
    if not v:
        return list(ALL_LAYERS)
    return [x.strip() for x in v.split(",") if x.strip() in ALL_LAYERS]


def build(with_settings: bool = True, only: list | None = None) -> Catalog:
    cat = Catalog()
    for name in only or layers():
        cat.keys.extend(readers.READERS[name](cat))
    readers.link_mirrors(cat.keys)
    if with_settings:
        cat.settings = settings.read_settings(cat)
    return cat
