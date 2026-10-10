"""Jira data glue: the config word lists, status classification, workflow
steps, the table cell-style rules and the ticket-comment index.

The app fetches `style_rules` once per config generation (`jira.style`) and
evaluates the returned table locally, so per-cell drawing — and the list
grouping that runs per keystroke — never crosses the process boundary.
"""
from __future__ import annotations

import json
import os
import threading

WORDS_DEFAULTS = {
    "status-cancelled-words": "cancel, won't, wont, reject, duplicate",
    "status-done-words": "done, closed, resolved, released, complete, fixed, shipped",
    "status-blocked-words": "block, fail, impediment",
    "status-active-words": "progress, review, test, qa, develop, doing, verif",
    "status-waiting-words": "hold, wait, pending, paused",
    "status-new-words": "backlog, open, to do, todo, new, selected, triage, funnel",
    "priority-urgent-words": "highest, blocker, critical, urgent, p0, p1",
}

# list name -> [jira] config key
_LIST_KEYS = {
    "cancelled": "status-cancelled-words",
    "done": "status-done-words",
    "blocked": "status-blocked-words",
    "active": "status-active-words",
    "waiting": "status-waiting-words",
    "new": "status-new-words",
    "urgent": "priority-urgent-words",
}

DIM_FIELDS = ["key", "updated", "created", "duedate", "releasedate",
              "project", "releaselabel", "release"]


def words(values: dict) -> dict:
    """[jira] config values -> the seven lowercased word lists (defaults per
    key; an explicit value replaces only that list)."""
    out = {}
    for name, key in _LIST_KEYS.items():
        raw = values.get(key) or WORDS_DEFAULTS[key]
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


def workflow_steps(value) -> list:
    """`[jira] workflow` CSV -> the explicit step list, else None."""
    if not value:
        return None
    steps = [s.strip() for s in value.split(",") if s.strip()]
    return steps or None


def style_rules(values: dict) -> dict:
    """The evaluator table the app caches: word lists + ordered rules.
    Rule order is the classification order (first match wins)."""
    w = words(values)
    return {
        "words": w,
        "dimFields": list(DIM_FIELDS),
        "status": [
            {"words": w["cancelled"], "style": {"tone": "dim", "mark": "hollow", "quietsRow": True}},
            {"words": w["done"], "style": {"tone": "success", "mark": "filled", "quietsRow": True}},
            {"words": w["blocked"], "style": {"tone": "danger", "mark": "filled"}},
            {"words": w["active"], "style": {"tone": "info", "mark": "half"}},
            {"words": w["waiting"], "style": {"tone": "warning", "mark": "hollow"}},
        ],
        "statusFallback": {"tone": "dim", "mark": "hollow"},
        "priority": [
            {"words": w["urgent"], "style": {"tone": "danger", "tinted": True, "bold": True}},
        ],
        "priorityFallback": {"tone": "dim"},
        "releaseStatus": [
            {"contains": "unreleased", "style": {"tone": "warning", "mark": "hollow"}},
            {"contains": "released", "style": {"tone": "success", "mark": "filled"}},
        ],
    }


# ------------------------------------------------------- ticket comments

_COMMENTS_LOCK = threading.Lock()
_COMMENTS_CACHE = {"path": None, "stamp": None, "index": {}}


def _str(x) -> str:
    return x if isinstance(x, str) else ""


def _comment_rows(issue):
    """The issue's comment rows, or None when it has none (mirrors the old
    Swift cast: the whole list must be objects)."""
    if not isinstance(issue, dict):
        return None
    arr = issue.get("comments")
    if not isinstance(arr, list) or not arr:
        return None
    rows = []
    for c in arr:
        if not isinstance(c, dict):
            return None
        rows.append({"author": _str(c.get("author")),
                     "body": _str(c.get("body")),
                     "created": _str(c.get("created"))})
    return rows


def comments(path: str, key: str) -> dict:
    """One issue's comments from the poller's issue cache, parsed once per
    (path, mtime) and cached in the worker. Returns
    {"stamp": mtime|0, "comments": [{author, body, created}]}."""
    try:
        stamp = os.stat(path).st_mtime
    except OSError:
        return {"stamp": 0, "comments": []}
    with _COMMENTS_LOCK:
        cache = _COMMENTS_CACHE
        if cache["path"] != path or cache["stamp"] != stamp:
            index = {}
            try:
                with open(path, encoding="utf-8") as fh:
                    all_issues = json.load(fh)
            except (OSError, ValueError):
                all_issues = {}
            if isinstance(all_issues, dict):
                for k, issue in all_issues.items():
                    rows = _comment_rows(issue)
                    if rows is not None:
                        index[k] = rows
            cache.update(path=path, stamp=stamp, index=index)
        return {"stamp": stamp, "comments": cache["index"].get(key, [])}
