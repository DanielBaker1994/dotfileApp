"""Jira field knowledge shared by the poller scripts and the app: the
column codec, the built-in field labels and the merged label map.

ONE implementation: jira/jira_config.py imports these (its old copies are
gone) and the app asks the helper for parse/serialize/labels.

The columns spec is a list of objects — the JSON form the app and config.json
carry:

    [{"field": "key", "width": 8, "filter": true, "sort": true},
     {"field": "updated", "width": 10, "align": "right", "sort": true}]

`title` overrides the field's label (team.json field_labels win); `width` is a
percent (0 = share the leftover), `align` left|right|center, `filter`/`sort`
the flags. Defaults are omitted. The legacy one-line string
`field:Title:width:align:flags, ...` still parses (and is migrated on load).
"""
from __future__ import annotations

import json
import jsonmgr
import re

# a field's display name (column header, search filter title) unless team.json
# field_labels renames it - ONE label per field
# the built-in table lives in jira/defaults.json (data, not code)
# lazy (worker surface, like jira_data): a broken jira/defaults.json fails at
# the first touch and is retried on the next one - the old eager load turned
# one transient bad file into permanently dead jira.columns_* methods for the
# life of the worker. Internal uses go through jsonmgr.field (a bare module
# name does not pass through __getattr__).
jsonmgr.lazy_module(__name__, globals(), {
    "BASE_FIELD_LABELS": ("jira/defaults", ("base_field_labels",), dict),
})

_ALIGNS = ("left", "right", "center")


def _truthy(v) -> bool:
    if isinstance(v, bool):
        return v
    return str(v).strip().lower() in ("true", "yes", "1", "on")


def _col_obj(o: dict):
    """One structured column object -> the internal shape (or None)."""
    field = str(o.get("field") or "").strip()
    if not field:
        return None
    try:
        width = float(o.get("width") or 0)
    except (TypeError, ValueError):
        width = 0.0
    raw_px = o.get("width_px")
    try:
        width_px = float(raw_px) if raw_px not in (None, "") else None
    except (TypeError, ValueError):
        width_px = None
    align = str(o.get("align") or "left").strip().lower()
    return {"field": field, "title": str(o.get("title") or "") or field,
            "width": max(0.0, width), "width_px": width_px,
            "align": align if align in _ALIGNS else "left",
            "sortable": _truthy(o.get("sort")), "filterable": _truthy(o.get("filter"))}


def _parse_legacy(spec: str) -> list:
    """`field:Title:width:align:flags, ...` -> the internal shape. Flags are
    filter / sort, joined with + or split by :/|//; missing parts default
    (title = field, width 0, align left); negative widths clamp to 0."""
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
        if align not in _ALIGNS:
            align = "left"
        flags = set()
        for s in seg[4:]:
            flags.update(f.strip().lower() for f in re.split(r"[+/|]", s) if f.strip())
        cols.append({"field": field, "title": title, "width": max(0.0, width), "width_px": None,
                     "align": align, "sortable": "sort" in flags, "filterable": "filter" in flags})
    return cols


def parse_columns(spec) -> list:
    """`columns` -> [{field,title,width,align,sortable,filterable}]. Accepts
    the structured list (or its JSON text) and the legacy one-line string."""
    if isinstance(spec, (list, tuple)):
        return [c for c in (_col_obj(o) for o in spec if isinstance(o, dict)) if c]
    text = spec.strip() if isinstance(spec, str) else ""
    if text.startswith("["):
        try:
            data = json.loads(text)
        except ValueError:
            return []
        return parse_columns(data) if isinstance(data, list) else []
    return _parse_legacy(text)


def columns_json(cols, titles: bool = True) -> list:
    """The structured form: [{field, title?, width?, align?, filter?, sort?}].
    Defaults are omitted (width 0 = auto, align left, no flags); `titles=False`
    drops the title (the field label supplies it)."""
    out = []
    for c in cols or []:
        field = c.get("field") or ""
        if not field:
            continue
        obj = {"field": field}
        title = c.get("title") or ""
        if titles and title and title != field:
            obj["title"] = title
        width = float(c.get("width") or 0)
        if width > 0:
            obj["width"] = int(width) if width == round(width) else round(width, 1)
        px = c.get("width_px")
        if px:
            px = float(px)
            obj["width_px"] = int(px) if px == round(px) else round(px, 1)
        if (c.get("align") or "left") != "left":
            obj["align"] = c.get("align")
        if c.get("filterable"):
            obj["filter"] = True
        if c.get("sortable"):
            obj["sort"] = True
        out.append(obj)
    return out


def serialize_columns(cols, titles: bool = True) -> str:
    """The JSON text of [`columns_json`] (the `--set-columns` transport and
    the helper reply; config.json stores the list itself)."""
    return json.dumps(columns_json(cols, titles), separators=(",", ":"), ensure_ascii=False)


def _norm_key(k: str) -> str:
    return re.sub(r"[\s-]+", "_", k.strip()).lower()


def merged_labels(team) -> dict:
    """The base labels + team.json custom_fields aliases + field_labels
    renames (field_labels wins; section keys are matched loosely:
    `custom-fields`, `Custom Fields`, ...)."""
    out = dict(jsonmgr.field("jira/defaults", "base_field_labels"))
    if not isinstance(team, dict):
        return out
    for k, v in team.items():
        if not isinstance(k, str) or _norm_key(k) != "custom_fields" or not isinstance(v, dict):
            continue
        for alias, spec in v.items():
            d = {}
            if isinstance(spec, dict):
                for k2, v2 in spec.items():
                    if isinstance(k2, str):
                        d[_norm_key(k2)] = v2
            label = d.get("label")
            out[alias] = label if isinstance(label, str) and label else alias
    for k, v in team.items():
        if not isinstance(k, str) or _norm_key(k) != "field_labels" or not isinstance(v, dict):
            continue
        # the old Swift cast rejected the whole dict on any non-string value
        if not all(isinstance(x, str) for x in v.values()):
            continue
        for f, label in v.items():
            if label.strip():
                out[f] = label.strip()
    return out
