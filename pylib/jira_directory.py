"""Jira directory cache: the poller's directory.json parsed once per
(path, mtime) in the worker; the app mirrors the returned data.

Only the shape the app consumes is promised (projects, users, statuses,
statusCategories, issueTypes, priorities, fields, versions, boards, labels,
fetchedAt); unknown keys pass through untouched."""
from __future__ import annotations

import json
import os
import re
import threading

import jira_data
import jira_pages

_LOCK = threading.Lock()
_CACHE = {"path": None, "stamp": None, "data": None}

# how many entries a picker shows before folding the tail (matches the app's
# JiraMultiPicker.topN; folded entries carry unused=true)
TOP_N = 50


def _is_str(x) -> bool:
    return isinstance(x, str)


def _int_or_zero(x) -> int:
    return x if isinstance(x, int) and not isinstance(x, bool) else 0


def _string_list(v) -> list:
    """Exact match for Swift's `as? [String]`: any non-string element means
    the whole value is rejected."""
    if isinstance(v, list) and all(_is_str(x) for x in v):
        return list(v)
    return []


def _dicts(v) -> list:
    if isinstance(v, list) and all(isinstance(x, dict) for x in v):
        return v
    return []


def _parse(path: str) -> dict:
    try:
        with open(path, encoding="utf-8") as fh:
            o = json.load(fh)
    except (OSError, ValueError):
        o = {}
    if not isinstance(o, dict):
        o = {}

    projects = []
    for p in _dicts(o.get("projects")):
        key = p.get("key")
        if _is_str(key):
            projects.append({"key": key, "name": p.get("name") if _is_str(p.get("name")) else ""})

    users = []
    for u in _dicts(o.get("users")):
        uid = u.get("id")
        if not _is_str(uid):
            continue
        users.append({"id": uid,
                      "name": u.get("name") if _is_str(u.get("name")) else uid,
                      "username": u.get("username") if _is_str(u.get("username")) else "",
                      "email": u.get("email") if _is_str(u.get("email")) else "",
                      "projects": _string_list(u.get("projects"))})

    fields = []
    for f in _dicts(o.get("fields")):
        fid = f.get("id")
        if not _is_str(fid):
            continue
        fields.append({"id": fid,
                       "name": f.get("name") if _is_str(f.get("name")) else fid,
                       "custom": bool(f.get("custom")) if isinstance(f.get("custom"), bool) else False,
                       "type": f.get("type") if _is_str(f.get("type")) else ""})

    versions = []
    for v in _dicts(o.get("versions")):
        name = v.get("name")
        if not _is_str(name):
            continue
        versions.append({"name": name,
                         "project": v.get("project") if _is_str(v.get("project")) else "",
                         "releaseDate": v.get("releaseDate") if _is_str(v.get("releaseDate")) else "",
                         "released": bool(v.get("released")) if isinstance(v.get("released"), bool) else False,
                         "id": v.get("id") if _is_str(v.get("id")) else ""})

    boards = []
    for b in _dicts(o.get("boards")):
        bid = b.get("id")
        if _is_str(bid):
            boards.append({"id": bid,
                           "name": b.get("name") if _is_str(b.get("name")) else bid,
                           "type": b.get("type") if _is_str(b.get("type")) else "",
                           "projects": _string_list(b.get("projects"))})

    labels = []
    for l in _dicts(o.get("labels")):
        name = l.get("name")
        if _is_str(name):
            labels.append({"name": name,
                           "projects": _string_list(l.get("projects")),
                           "count": _int_or_zero(l.get("count"))})

    statuses = _string_list(o.get("statuses"))
    categories = {k: v for k, v in (o.get("statusCategories") or {}).items()
                  if _is_str(k) and _is_str(v)} if isinstance(o.get("statusCategories"), dict) else {}

    return {"fetchedAt": o.get("fetchedAt") if _is_str(o.get("fetchedAt")) else "",
            "projects": projects, "users": users, "statuses": statuses,
            "statusCategories": categories,
            "issueTypes": _string_list(o.get("issueTypes")),
            "priorities": _string_list(o.get("priorities")),
            "fields": fields, "versions": versions, "boards": boards, "labels": labels}


def empty() -> dict:
    return _parse("")


def load(path: str) -> dict:
    try:
        stamp = os.stat(path).st_mtime
    except OSError:
        return empty()
    with _LOCK:
        c = _CACHE
        if c["path"] != path or c["stamp"] != stamp:
            c.update(path=path, stamp=stamp, data=_parse(path))
        return c["data"]


# ------------------------------------------------------------ picker options

def _option(id_, title, detail="", group="", unused=False) -> dict:
    return {"id": id_, "title": title, "detail": detail, "group": group, "unused": unused}


def _matches(projects, scope) -> bool:
    if not scope:
        return True
    return any(p in scope for p in projects)


def project_options(d: dict, scope) -> list:
    names = {p["key"]: p["name"] for p in d.get("projects") or []}
    return [_option(k, k, names.get(k, "")) for k in scope or []]


def user_options(d: dict, me: bool = True) -> list:
    out = [_option("currentUser()", "Me", "currentUser()")] if me else []
    for u in d.get("users") or []:
        who = []
        if u["username"] and u["username"] != u["name"]:
            who.append(u["username"])
        if u["email"]:
            who.append(u["email"])
        projects = ", ".join(u["projects"])
        out.append(_option(u["id"], u["name"], " · ".join(who + ([projects] if projects else []))))
    return out


def value_options(values) -> list:
    return [_option(v, v) for v in values or [] if isinstance(v, str)]


def status_options(d: dict, words: dict, category_names=None) -> list:
    category_names = category_names or list(jira_pages.CATEGORY_NAMES)
    cats = d.get("statusCategories") or {}
    return [_option(s, s, "", category_names[jira_data.category(s, cats, words)])
            for s in d.get("statuses") or []]


def _natural_key(name: str) -> list:
    """Case-folded natural ordering (the localizedStandardCompare stand-in)."""
    return [int(part) if part.isdigit() else part for part in re.split(r"(\d+)", name.casefold())]


def _fold_tail(opts: list) -> list:
    if len(opts) <= TOP_N + 5:
        return opts
    return [dict(o, unused=(i >= TOP_N)) for i, o in enumerate(opts)]


def label_options(d: dict, scope=None) -> list:
    labs = [l for l in d.get("labels") or [] if _matches(l["projects"], scope)]
    labs.sort(key=lambda l: (-l["count"], _natural_key(l["name"])))
    out = []
    for l in labs:
        n = "%d issue%s · " % (l["count"], "" if l["count"] == 1 else "s") if l["count"] > 0 else ""
        out.append(_option(l["name"], l["name"], n + ", ".join(l["projects"])))
    return _fold_tail(out)


def version_options(d: dict, scope=None) -> list:
    order, by = [], {}
    for v in d.get("versions") or []:
        if not _matches([v["project"]], scope):
            continue
        if v["name"] not in by:
            order.append(v["name"])
            by[v["name"]] = []
        by[v["name"]].append(v)
    out = []
    for name in order:
        vs = by[name]
        date = next((v["releaseDate"] for v in vs if v["releaseDate"]), "no date")
        state = "released" if all(v["released"] for v in vs) else "unreleased"
        detail = " · ".join([", ".join(v["project"] for v in vs), date, state])
        out.append(_option(name, name, detail))
    return out
