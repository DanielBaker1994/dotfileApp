"""Jira dashboard edits: the poll-job draft assembly and the live-search
persist payload, with the limit parsing both share (jira_config.py re-runs
the authoritative validation on upsert)."""
from __future__ import annotations


def parse_args(s: str) -> dict:
    """`args` text: comma-separated key=value pairs (first `=` splits)."""
    out = {}
    for part in (s or "").split(","):
        kv = [p.strip() for p in part.split("=", 1)]
        if len(kv) == 2 and kv[0]:
            out[kv[0]] = kv[1]
    return out


def check_limit(s, what: str) -> dict:
    """Swift Int() semantics: empty means 0 (default), otherwise a whole
    number >= 0."""
    s = (s or "").strip()
    if not s:
        return {"ok": True, "value": 0}
    try:
        n = int(s)
    except ValueError:
        n = None
    if n is None or n < 0:
        return {"ok": False, "message": "✗ %s must be a whole number (empty = default)" % what}
    return {"ok": True, "value": n}


def draft(params: dict) -> dict:
    ps = check_limit(params.get("pageSize"), "Page size")
    if not ps["ok"]:
        return ps
    mt = check_limit(params.get("maxTotal"), "Max issues")
    if not mt["ok"]:
        return mt
    typ = params.get("type") or ""
    o = {"name": (params.get("name") or "").strip(),
         "type": typ,
         "maxResults": ps["value"], "maxTotal": mt["value"],
         "window": (params.get("window") or "").strip(),
         "enabled": bool(params.get("enabled"))}
    if typ != "directory" and params.get("columns") is not None:
        o["columns"] = params["columns"]
    picked = [p for p in (params.get("projects") or []) if isinstance(p, str)]
    o["projects"] = "*" if params.get("projectsAll") or not picked else picked
    q = params.get("queryIndex") if typ == "issues" else 0
    q = q if isinstance(q, int) else 0
    o["jql"] = (params.get("jql") or "").strip() if q == 1 else ""
    title = params.get("jobTitle") or ""
    o["job"] = title[len("team.json: "):] if q >= 2 else ""
    o["args"] = parse_args(params.get("args") or "") if q >= 2 else {}
    return {"ok": True, "draft": o}


def live_persist(params: dict) -> dict:
    m = check_limit(params.get("maxResults"), "Max results")
    if not m["ok"]:
        return m
    o = {"columns": params.get("columns") or "", "maxResults": m["value"]}
    return {"ok": True, "draft": o}
