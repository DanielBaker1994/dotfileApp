"""Jira data glue: the config word lists and the status classification.

The word lists and `category` are evaluated per row by the board columns and
the directory's status options; the app draws from its own resolved data.
"""
from __future__ import annotations

import jsonmgr

# lazy (worker surface): a broken jira/defaults.json fails at the first
# method call and is retried on the next one; the old symbol names stay
# (callers unchanged) and internal uses go through jsonmgr.field
jsonmgr.lazy_module(__name__, globals(), {
    "WORDS_DEFAULTS": ("jira/defaults", ("words",), dict),
    "_LIST_KEYS": ("jira/defaults", ("words_keys",), dict),
    "DIM_FIELDS": ("jira/defaults", ("dim_fields",), list),
})


def words(values: dict) -> dict:
    """[jira] config values -> the seven lowercased word lists (defaults per
    key; an explicit value replaces only that list)."""
    words_defaults = jsonmgr.field("jira/defaults", "words")
    list_keys = jsonmgr.field("jira/defaults", "words_keys")
    out = {}
    for name, key in list_keys.items():
        raw = values.get(key) or words_defaults[key]
        out[name] = [p.strip().lower() for p in raw.split(",") if p.strip()]
    return out


def matches(lowered: str, word_list: list) -> bool:
    return any(w in lowered for w in word_list)


def category(status: str, categories: dict, word_lists: dict) -> int:
    """0 To Do / 1 In Progress / 2 Done. The directory's statusCategories win;
    the word lists classify anything it does not know."""
    cat = categories.get(status)
    if cat == "new":
        return 0
    if cat == "indeterminate":
        return 1
    if cat == "done":
        return 2
    lowered = status.lower()
    if matches(lowered, word_lists.get("done") or []) or matches(lowered, word_lists.get("cancelled") or []):
        return 2
    if matches(lowered, word_lists.get("new") or []):
        return 0
    return 1
