"""Jira search glue: the live-search criteria assembly and the filter menu
model (the base kinds plus the describe catalog with its skip list)."""
from __future__ import annotations

SKIP_CATALOG_FIELDS = {
    "key", "title", "status", "assignee", "release", "releaseLabel",
    "releaseDate", "releaseStatus", "priority", "labels", "description",
    "reporter", "project", "updated", "comments",
}


def _label_for(catalog: list, field: str, fallback: str) -> str:
    """First non-empty catalog label for the field, else the fallback."""
    for c in catalog:
        if isinstance(c, dict) and c.get("field") == field:
            label = c.get("label")
            if isinstance(label, str) and label:
                return label
    return fallback


def filter_kinds(catalog) -> list:
    catalog = catalog if isinstance(catalog, list) else []

    def lbl(field, fallback):
        return _label_for(catalog, field, fallback)

    kinds = [
        {"key": "assignee", "title": lbl("assignee", "Assignee"), "kind": "users"},
        {"key": "reporter", "title": lbl("reporter", "Reporter"), "kind": "users"},
        {"key": "status", "title": lbl("status", "Status"), "kind": "list"},
        {"key": "statusCategory", "title": "Status category", "kind": "list"},
        {"key": "issuetype", "title": "Issue type", "kind": "list"},
        {"key": "priority", "title": lbl("priority", "Priority"), "kind": "list"},
        {"key": "fixVersion", "title": lbl("release", "Release"), "kind": "list"},
        {"key": "labels", "title": lbl("labels", "Labels"), "kind": "list"},
        {"key": "updated", "title": lbl("updated", "Updated") + " within", "kind": "date"},
        {"key": "created", "title": "Created within", "kind": "date"},
        {"key": "resolved", "title": "Resolved within", "kind": "date"},
        {"key": "field:title", "title": lbl("title", "Summary") + " contains", "kind": "text"},
        {"key": "field:description", "title": lbl("description", "Description") + " contains", "kind": "text"},
    ]
    for c in catalog:
        if not isinstance(c, dict):
            continue
        field = c.get("field")
        if not isinstance(field, str) or field in SKIP_CATALOG_FIELDS:
            continue
        label = c.get("label")
        if not (isinstance(label, str) and label):
            label = field
        kinds.append({"key": "field:" + field, "title": label + " contains", "kind": "text"})
    return kinds


def criteria(params: dict) -> dict:
    """UI state -> the criteria JSON jira_poll.py --live-search consumes."""
    c = {}
    text = (params.get("text") or "").strip()
    if text:
        c["text"] = text
    projects = [p for p in (params.get("projects") or []) if isinstance(p, str)]
    if projects and not params.get("projectsAll"):
        c["projects"] = projects
    fields = {}
    for r in params.get("rows") or []:
        if not isinstance(r, dict):
            continue
        key = r.get("key")
        if not isinstance(key, str) or not key:
            continue
        selected = r.get("selected")
        if isinstance(selected, list) and selected:
            c[key] = [s for s in selected if isinstance(s, str)]
            continue
        if "value" in r:
            c[key] = r.get("value") if isinstance(r.get("value"), str) else ""
            continue
        text_v = (r.get("text") or "").strip()
        if text_v and key.startswith("field:"):
            fields[key[6:]] = text_v
    if fields:
        c["fields"] = fields
    try:
        m = 0
        v = params.get("maxResults")
        if isinstance(v, int) and not isinstance(v, bool):
            m = v
        elif isinstance(v, str) and v and v == v.strip():  # Int() is strict about spaces
            m = int(v)
    except ValueError:
        m = 0
    if m > 0:
        c["maxResults"] = m
    return c
