"""Jira board: the column/card mapping.

The board page itself is generated in Rust (`views/jira.rs` `board_page`);
this module only answers the column/card model.
"""
from __future__ import annotations

import jira_data
import jira_pages
import jsonmgr

# lazy (worker surface): a broken jira/defaults.json fails at the first
# method call and is retried on the next one; a done column only keeps
# DONE_LIMIT cards (the Table view shows them all)
jsonmgr.lazy_module(__name__, globals(), {
    "DONE_LIMIT": ("jira/defaults", ("boards", "done_limit")),
    "OTHER_COLUMN": ("jira/defaults", ("boards", "other_column")),
    "_HOT_PATTERN": ("jira/defaults", ("boards", "hot_words"),
                     lambda hot: "|".join(hot)),
    "_COLOR_KEYS": ("jira/defaults", ("boards", "color_keys"), list),
})


def _s(x) -> str:
    return x if isinstance(x, str) else ""


def board_columns(params: dict) -> dict:
    """Rows -> the board's columns. With a board spec, statuses map to their
    column and the rest land in 'Not on the board'; without one, the three
    status categories are the columns. A fully-done column clamps to
    DONE_LIMIT cards."""
    rows = [r for r in (params.get("rows") or []) if isinstance(r, dict)]
    spec = [c for c in (params.get("columns") or []) if isinstance(c, dict)]
    cats = params.get("categories") or {}
    done_limit = jsonmgr.field("jira/defaults", "boards", "done_limit")
    other_column = jsonmgr.field("jira/defaults", "boards", "other_column")
    words = params.get("words") or {}
    people = params.get("people") or {}
    names = params.get("categoryNames") or list(jira_pages.CATEGORY_NAMES)
    names = list(names)
    if len(names) < 3:   # a short list would IndexError on cat == 2 below
        names += list(jira_pages.CATEGORY_NAMES)[len(names):3]

    if spec:
        cols = [{"name": _s(c.get("name")),
                 "statuses": [s for s in (c.get("statuses") or []) if isinstance(s, str)],
                 "cards": []} for c in spec]
    else:
        cols = [{"name": n, "statuses": [], "cards": []} for n in names]

    other = []
    for r in rows:
        def f(k):
            return _s(r.get(k))
        st = f("status")
        assignee = f("assignee")
        card = {"key": f("key"),
                "title": f("title") or _s(r.get("rowTitle")),
                "type": f("type"), "priority": f("priority"),
                "assignee": (people.get(assignee) or assignee) if assignee else "",
                "status": st,
                "cat": jira_data.category(st, cats, words)}
        if spec:
            for col in cols:
                if st in col["statuses"]:
                    col["cards"].append(card)
                    break
            else:
                other.append(card)
        else:
            cols[card["cat"]]["cards"].append(card)

    out = []
    for c in cols:
        done = bool(c["cards"]) and all(k["cat"] == 2 for k in c["cards"])
        keep = c["cards"][:done_limit] if done else c["cards"]
        out.append({"name": c["name"], "cards": keep, "more": len(c["cards"]) - len(keep)})
    if other:
        out.append({"name": other_column, "cards": other, "more": 0})
    return {"columns": out}
