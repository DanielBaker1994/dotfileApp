"""Jira dashboard edits: the poll-job draft assembly and the live-search
persist payload (with the limit parsing both share), plus the team.json
definition edits (custom fields, field labels, key/value pairs, search
defaults). jira_config.py re-runs the authoritative validation on save."""
from __future__ import annotations

import json
import re

_CF_RE = re.compile(r"customfield_\d+")


def _scalar_str(v) -> str:
    """The app's `str()` for JSON scalars: strings pass, numbers stringify,
    everything else serialises as JSON."""
    if isinstance(v, str):
        return v
    if v is None:
        return ""
    if isinstance(v, bool):
        return "1" if v else "0"
    if isinstance(v, int):
        return str(v)
    if isinstance(v, float):
        return "%g" % v
    return json.dumps(v, separators=(",", ":"))


def snake_case(s: str) -> str:
    """Lowercase, runs of non-alphanumerics -> single underscores; a leading
    non-letter gets an `f_` prefix."""
    parts, cur = [], []
    for ch in (s or "").lower():
        if ch.isalnum():
            cur.append(ch)
        elif cur:
            parts.append("".join(cur))
            cur = []
    if cur:
        parts.append("".join(cur))
    out = "_".join(parts)
    if out and not out[0].isalpha():
        out = "f_" + out
    return out


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


# ------------------------------------------------------- definition edits

def custom_field_entry(params: dict) -> dict:
    """The custom-field sheet: id from the typed text or a case-insensitive
    name match, label/alias defaults, entry shape, label clearing."""
    customs = [c for c in (params.get("customs") or []) if isinstance(c, dict)]
    raw = params.get("raw") or ""
    alias = (params.get("alias") or "").strip()
    label = (params.get("label") or "").strip()
    desc = (params.get("description") or "").strip()
    m = _CF_RE.search(raw)
    fid = m.group(0) if m else ""
    if not fid:
        for c in customs:
            name = c.get("name")
            if isinstance(name, str) and name.lower() == raw.lower():
                fid = _scalar_str(c.get("id"))
                break
    if not fid:
        return {"ok": False,
                "message": "✗ pick a Jira custom field (or type its customfield_NNNNN id)"}
    jira_name = next((c["name"] for c in customs
                      if _scalar_str(c.get("id")) == fid and isinstance(c.get("name"), str)), "")
    lbl = label or jira_name
    a = alias
    if not a:
        a = snake_case(lbl or fid)
    entry = {"field_id": fid, "label": lbl or a}
    if desc:
        entry["description"] = desc
    d = dict(params.get("currentCustomFields") or {})
    d[a] = entry
    fl = dict(params.get("currentLabels") or {})
    clear = fl.pop(a, None) is not None
    return {"ok": True, "alias": a,
            "save": {"key": "custom_fields", "value": d,
                     "done": ("added %s" if params.get("isNew") else "updated %s") % a},
            "followup": ({"key": "field_labels", "value": fl, "done": "label of %s" % a}
                         if clear else None)}


def field_label_save(params: dict) -> dict:
    f = params.get("field") or ""
    v = (params.get("value") or "").strip()
    default = params.get("default") or ""
    d = dict(params.get("current") or {})
    if not v or v == default:
        d.pop(f, None)
        done = "%s back to “%s”" % (f, default)
    else:
        d[f] = v
        done = "%s → “%s”" % (f, v)
    return {"ok": True, "value": d, "done": done}


def key_value_save(params: dict) -> dict:
    name = (params.get("name") or "").strip()
    val = (params.get("value") or "").strip()
    if not name or not val:
        return {"ok": False, "beep": True}
    nd = dict(params.get("current") or {})
    nd[name] = val
    return {"ok": True, "value": nd,
            "done": ("updated %s" if params.get("existing") else "added %s") % name}


def default_save(params: dict) -> dict:
    key = params.get("key") or ""
    v = (params.get("value") or "").strip()
    try:
        n = int(v)
    except ValueError:
        n = None
    if n is None or n < 0:
        return {"ok": False, "message": "✗ %s must be a whole number" % key}
    d = dict(params.get("current") or {})
    d[key] = n
    return {"ok": True, "value": d, "done": "%s = %d" % (key, n)}
