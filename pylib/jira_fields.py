"""Jira field knowledge shared by the poller scripts and the app: the
column-spec codec and the built-in field labels.

ONE implementation: jira/jira_config.py imports these (its old copies are
gone) and the app asks the helper for parse/serialize."""
from __future__ import annotations

import re

# a field's display name (column header, search filter title) unless team.json
# field_labels renames it - ONE label per field
BASE_FIELD_LABELS = {
    "key": "Key", "title": "Title", "status": "Status", "assignee": "Assignee",
    "reporter": "Reporter", "priority": "Priority", "labels": "Labels",
    "description": "Description", "project": "Project", "updated": "Updated",
    "release": "Fix versions", "releaseLabel": "Release", "releaseDate": "Release date",
    "releaseStatus": "Released", "comments": "Comments",
    "components": "Components", "epic": "Epic / parent",
}


def parse_columns(spec: str) -> list:
    """`field:Title:width:align:flags, ...` -> [{field,title,width,align,
    sortable,filterable}]. flags: filter / sort, joined with + (filter+sort)
    or as extra :segments. Missing parts default (title = field, width 0 =
    share the leftover width, align left); negative widths clamp to 0."""
    cols = []
    for part in (spec or "").split(","):
        part = part.strip()
        if not part:
            continue
        seg = [s.strip() for s in part.split(":")]
        field = seg[0]
        if not field:
            continue
        title = seg[1] if len(seg) > 1 and seg[1] else field
        try:
            width = float(seg[2]) if len(seg) > 2 and seg[2] else 0.0
        except ValueError:
            width = 0.0
        align = seg[3].lower() if len(seg) > 3 and seg[3] else "left"
        if align not in ("left", "right", "center"):
            align = "left"
        flags = set()
        for s in seg[4:]:
            flags.update(f.strip().lower() for f in re.split(r"[+/|]", s) if f.strip())
        cols.append({"field": field, "title": title, "width": max(0.0, width), "align": align,
                     "sortable": "sort" in flags, "filterable": "filter" in flags})
    return cols


def serialize_columns(cols, titles: bool = True) -> str:
    """The inverse: integral widths stay integers, fractional ones keep one
    decimal; flags come back as `filter+sort` (filter first)."""
    out = []
    for c in cols or []:
        width = float(c.get("width") or 0)
        w = str(int(width)) if width == round(width) else "%.1f" % width
        flags = [("filter" if c.get("filterable") else None),
                 ("sort" if c.get("sortable") else None)]
        flags = "+".join(f for f in flags if f)
        seg = "%s:%s:%s:%s" % (c.get("field") or "",
                               (c.get("title") or "") if titles else "",
                               w, c.get("align") or "left")
        out.append(seg + (":" + flags if flags else ""))
    return ", ".join(out)
