"""Jira directory cache: the poller's directory.json parsed once per
(path, mtime) in the worker; the app mirrors the returned data.

Only the shape the app consumes is promised (projects, users, statuses,
statusCategories, issueTypes, priorities, fields, versions, boards, labels,
fetchedAt); unknown keys pass through untouched."""
from __future__ import annotations

import json
import os
import threading

_LOCK = threading.Lock()
_CACHE = {"path": None, "stamp": None, "data": None}


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
