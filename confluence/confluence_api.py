#!/usr/bin/env python3
"""confluence_api.py - Confluence search for the app (one JSON object on stdout).

Every request goes through jira_api.Client (the same curl, auth, retries and
curl.log as Jira), against the Confluence site in confluence config.json -
see confluence_config.py. Nothing is cached: results and pages go straight
to stdout.

  --check                  config status {ok, site, auth, configPath, spaces,
                           favorites, problems} (never the token)
  --save                   stdin JSON {site, email, token, auth} merged into
                           config.json (the token never touches argv)
  --detect-auth            stdin JSON {site, email, token}: GET
                           /rest/api/user/current with each auth mode; the first
                           non-anonymous 200 wins -> {ok, auth, user}
  --myself                 the saved config's user
  --add-space KEY...       GET /rest/api/space/KEY each, then add to the scope
  --remove-space KEY...
  --search                 stdin criteria {query, mode all|phrase|any, titleOnly,
                           spaces[], types[], modified, mine, sort
                           relevance|recent, favorites (bool: within them),
                           next (a _links.next), start, limit}
                           -> {ok, cql, curl, total, next, terms, results[]}
  --page ID                the rendered page (body.view) + metadata
  --favorite add|remove ID...   stdin (add): the rows' JSON (title, space, url…)
  --favorites              the saved list, refreshed with ONE `id in (…)` search
                           (missing pages are flagged, never dropped)
  --import-saved           merge Confluence's own "saved for later"
                           (favourite = currentUser()) into the favorites

Errors: {ok: false, error, curl?} and exit 1 (2 = bad input / config).
"""
from __future__ import annotations

import datetime as dt
import html
import json
import os
import re
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import confluence_config as cc  # noqa: E402
import jira_api  # type: ignore  # noqa: E402  (path set up by confluence_config)
import jira_log  # type: ignore  # noqa: E402

jira_api.CURL_LOG = os.path.join(cc.CACHE_DIR, "curl.log")
HL_OPEN, HL_CLOSE = "@@@hl@@@", "@@@endhl@@@"
SEARCH_EXPAND = "content.space,content.version,content.ancestors,content.container"


class ConfluenceClient(jira_api.Client):
    product = "Confluence"
    token_env = "CONFLUENCE_TOKEN"
    setup_hint = "fix it in the Confluence view's Setup (⚙)"


def client(cfg: cc.Config, site=None, token=None, email=None, auth=None) -> ConfluenceClient:
    c = ConfluenceClient(cc.norm_site(site) if site else cfg.site, token if token is not None else cfg["token"],
                         email=email if email is not None else (cfg["email"] or ""),
                         auth=auth or cfg.auth, team={}, timeout=int(cfg["timeoutSeconds"] or 20))
    c.max_wait = max(0, float(cfg["rateLimitMaxWaitMinutes"] or 0)) * 60
    c.delay = max(0, int(cfg["requestDelayMs"] or 0)) / 1000.0
    c.on_wait = lambda msg, secs: print(f"confluence-api: {msg}", file=sys.stderr)
    c.on_note = lambda msg: print(f"confluence-api: {msg}", file=sys.stderr)
    return c


def emit(obj: dict, code: int = 0) -> int:
    print(json.dumps(obj))
    return code


def fail(msg: str, code: int = 1, **kw) -> int:
    return emit({"ok": False, "error": msg, **kw}, code)


def qs(**kw) -> str:
    return "&".join(f"{k}={jira_api.qenc(str(v))}" for k, v in kw.items() if v is not None and v != "")


# ------------------------------------------------------------------ shaping

def utf16_len(s: str) -> int:
    return len(s.encode("utf-16-le")) // 2


def parse_hl(s: str) -> tuple[str, list]:
    """'a @@@hl@@@b@@@endhl@@@ c' -> ('a b c', [[2, 1]]): plain text + hit
    ranges in UTF-16 units (what NSString / NSRange count)."""
    s = html.unescape(re.sub(r"<[^>]+>", "", s or ""))
    out, hits, pos, start = [], [], 0, 0
    for part in re.split(f"({re.escape(HL_OPEN)}|{re.escape(HL_CLOSE)})", s):
        if part == HL_OPEN:
            start = pos
        elif part == HL_CLOSE:
            hits.append([start, pos - start])
        else:
            part = re.sub(r"\s+", " ", part)
            out.append(part)
            pos += utf16_len(part)
    return "".join(out), [h for h in hits if h[1] > 0]


def local_hits(text: str, terms: list) -> list:
    """Hit ranges for text the server didn't mark (fallback search, favorites)."""
    hits = []
    for t in terms:
        words = [re.escape(w) for w in t["text"].split()]
        if not words:
            continue
        rx = r"\b" + r"\s+".join(words) + (r"\w*" if t.get("prefix") else r"")
        for m in re.finditer(rx, text, re.I):
            hits.append([utf16_len(text[:m.start()]), utf16_len(m.group(0))])
    return sorted(hits)


def friendly(when: str) -> str:
    try:
        d = dt.datetime.fromisoformat(when.replace("Z", "+00:00"))
    except (ValueError, AttributeError):
        return ""
    days = (dt.datetime.now(dt.timezone.utc) - d).days
    if days <= 0:
        return "today"
    if days == 1:
        return "yesterday"
    if days < 30:
        return f"{days}d ago"
    return d.strftime("%b %-d, %Y")


def web_url(site: str, link: str) -> str:
    if not link:
        return ""
    if link.startswith(("http://", "https://")):
        return link
    return site.rstrip("/") + ("" if link.startswith("/") else "/") + link


def shape(r: dict, site: str, terms: list, favs: set) -> dict:
    """One /rest/api/search result (or a bare content object) -> a row."""
    c: dict = r["content"] if isinstance(r.get("content"), dict) else r
    title, thits = parse_hl(r.get("title") or c.get("title") or "")
    excerpt, ehits = parse_hl(r.get("excerpt") or "")
    if not thits:
        thits = local_hits(title, terms)
    if excerpt and not ehits:
        ehits = local_hits(excerpt, terms)
    space = c.get("space") or {}
    ver = c.get("version") or {}
    links = c.get("_links") or {}
    typ = c.get("type") or ""
    link = links.get("webui") or r.get("url") or ""
    container = c.get("container") or {}
    if typ == "attachment":
        link = links.get("download") or link
    when = ver.get("when") or r.get("lastModified") or ""
    return {
        "id": str(c.get("id") or ""), "type": typ, "title": title, "titleHits": thits,
        "excerpt": excerpt, "hits": ehits,
        "space": space.get("key") or "", "spaceName": space.get("name") or
        (r.get("resultGlobalContainer") or {}).get("title") or "",
        "path": " › ".join(a.get("title", "") for a in (c.get("ancestors") or [])),
        "container": container.get("title") or "",
        "url": web_url(site, link), "modified": when, "modifiedText": friendly(when),
        "author": ((ver.get("by") or {}).get("displayName")) or "",
        "favorite": str(c.get("id") or "") in favs,
    }


# ------------------------------------------------------------------ commands

def do_search(cfg: cc.Config, crit: dict) -> int:
    favs = {str(f.get("id")) for f in cfg["favorites"] if isinstance(f, dict)}
    if crit.get("favorites"):
        crit = {**crit, "ids": sorted(favs)}
    try:
        cql = cc.criteria_cql(crit, cfg)
    except cc.ConfigError as err:
        return fail(str(err), 2)
    limit = max(1, min(100, int(crit.get("limit") or (cfg["search"] or {}).get("limit") or 25)))
    c = client(cfg)
    terms = cc.terms(crit)
    t0 = time.time()
    nxt = crit.get("next") or ""
    path, q = "/rest/api/search", qs(cql=cql, start=crit.get("start") or None, limit=limit,
                                     excerpt="highlight", expand=SEARCH_EXPAND)
    if nxt:
        path, _, q = nxt.partition("?")
    fallback = False
    try:
        try:
            data = c.get(path, q) or {}
        except jira_api.ApiError as err:
            if err.code != 404 or not path.endswith("/rest/api/search"):
                raise
            # an older Server / DC: content search (no excerpts)
            fallback = True
            path = "/rest/api/content/search"
            q = qs(cql=cql, start=crit.get("start") or None, limit=limit,
                   expand="space,version,ancestors,container")
            data = c.get(path, q) or {}
    except jira_api.ApiError as err:
        return fail(str(err), 1, cql=cql, curl=err.curl)
    rows = [shape(r, cfg.site, terms, favs) for r in data.get("results") or []]
    total = data.get("totalSize", data.get("size", len(rows)))
    nlink = (data.get("_links") or {}).get("next") or ""
    return emit({"ok": True, "cql": cql, "curl": c.curl_cmd(c.url(path, q), masked=True),
                 "total": total, "count": len(rows), "next": nlink, "fallback": fallback,
                 "terms": terms, "elapsed": round(time.time() - t0, 2), "results": rows,
                 "site": cfg.site})


def do_page(cfg: cc.Config, pid: str) -> int:
    if not pid.isdigit():
        return fail(f"not a content id: {pid}", 2)
    c = client(cfg)
    try:
        j = c.get(f"/rest/api/content/{pid}",
                  qs(expand="body.view,space,version,ancestors,history,container,metadata.labels")) or {}
    except jira_api.ApiError as err:
        return fail(str(err), 1, curl=err.curl)
    row = shape(j, cfg.site, [], set())
    labels = [x.get("name") for x in ((j.get("metadata") or {}).get("labels") or {}).get("results") or []]
    return emit({"ok": True, **row, "html": ((j.get("body") or {}).get("view") or {}).get("value") or "",
                 "labels": labels, "mediaType": (j.get("metadata") or {}).get("mediaType") or "",
                 "created": (j.get("history") or {}).get("createdDate") or "",
                 "creator": ((j.get("history") or {}).get("createdBy") or {}).get("displayName") or "",
                 "site": cfg.site})


def me_of(c: ConfluenceClient) -> dict:
    me = c.get("/rest/api/user/current") or {}
    if me.get("type") == "anonymous":
        # DC answers 200 "anonymous" when the credentials were ignored
        raise jira_api.ApiError(401, "the server treated the request as anonymous - the token was "
                                     "not accepted with this auth mode",
                                c.curl_cmd(c.url("/rest/api/user/current"), masked=True))
    return me


def do_detect(cfg: cc.Config, given: dict) -> int:
    site = cc.norm_site(given.get("site") or cfg["site"])
    token = given.get("token") or cfg["token"]
    email = given.get("email") if given.get("email") is not None else (cfg["email"] or "")
    if not site or not token:
        return fail("site and token are required", 2, tried=[])
    tried, last = [], ""
    for mode in jira_api.auth_order(site, email):
        c = client(cfg, site=site, token=token, email=email, auth=mode)
        c.max_wait = 0
        try:
            me = me_of(c)
        except jira_api.ApiError as err:
            tried.append({"mode": mode, "http": err.code, "error": str(err)})
            last = err.curl or last
            continue
        tried.append({"mode": mode, "http": 200})
        return emit({"ok": True, "auth": mode, "site": site, "email": email if mode == "basic" else "",
                     "user": me.get("displayName") or me.get("username") or "", "tried": tried})
    hint = "" if email else " (a Cloud API token also needs the account email)"
    return fail("no auth mode was accepted" + hint, 1, tried=tried, curl=last)


def do_save(cfg: cc.Config, given: dict) -> int:
    for k in ("site", "email", "token", "auth"):
        if k in given and given[k] is not None:
            cfg.data[k] = cc.norm_site(given[k]) if k == "site" else given[k]
    cfg.save()
    return do_check(cfg)


def do_check(cfg: cc.Config) -> int:
    return emit({"ok": not cfg.problems(), "site": cfg.site, "auth": cfg.auth, "email": cfg["email"] or "",
                 "hasToken": bool(cfg["token"]), "configPath": cfg.path, "problems": cfg.problems(),
                 "spaces": cfg["spaces"], "favorites": len(cfg["favorites"]),
                 "limit": (cfg["search"] or {}).get("limit") or 25})


def do_spaces(cfg: cc.Config, op: str, keys: list) -> int:
    keys = [k.strip().upper() for k in keys if k.strip()]
    if not keys:
        return fail("no space keys given", 2)
    spaces = [s for s in cfg["spaces"] if isinstance(s, dict)]
    if op == "remove":
        spaces = [s for s in spaces if s.get("key") not in keys]
    else:
        c = client(cfg)
        errors = []
        for k in keys:
            try:
                j = c.get(f"/rest/api/space/{jira_api.qenc(k)}") or {}
            except jira_api.ApiError as err:
                errors.append(f"{k}: " + ("no such space (or no access)" if err.code == 404 else str(err)))
                continue
            spaces = [s for s in spaces if s.get("key") != k] + [{"key": j.get("key") or k,
                                                                  "name": j.get("name") or k}]
        if errors:
            cfg.data["spaces"] = spaces
            cfg.save()
            return fail("; ".join(errors), 1, spaces=spaces)
    cfg.data["spaces"] = spaces
    cfg.save()
    return emit({"ok": True, "spaces": spaces})


FAV_KEYS = ("id", "title", "space", "spaceName", "type", "url", "path")


def do_favorite(cfg: cc.Config, op: str, ids: list, rows: list) -> int:
    ids = [str(i) for i in ids if str(i).strip()]
    favs = [f for f in cfg["favorites"] if isinstance(f, dict)]
    if op == "remove":
        favs = [f for f in favs if str(f.get("id")) not in ids]
    elif op == "add":
        meta = {str(r.get("id")): r for r in rows if isinstance(r, dict)}
        now = time.strftime("%Y-%m-%dT%H:%M:%S")
        for i in reversed(ids):
            if any(str(f.get("id")) == i for f in favs):
                continue
            r = meta.get(i) or {"id": i, "title": f"Page {i}"}
            favs.insert(0, {**{k: r.get(k, "") for k in FAV_KEYS}, "id": i, "added": now})
    else:
        return fail("usage: --favorite add|remove ID...", 2)
    cfg.data["favorites"] = favs
    cfg.save()
    return emit({"ok": True, "favorites": favs})


def do_favorites(cfg: cc.Config, refresh: bool = True) -> int:
    """The saved list, refreshed in ONE request. A page the search no longer
    returns (deleted / no access) stays, flagged `missing`."""
    favs = [f for f in cfg["favorites"] if isinstance(f, dict) and str(f.get("id", "")).isdigit()]
    rows = [{**f, "favorite": True, "titleHits": [], "hits": [], "excerpt": ""} for f in favs]
    if not favs or not refresh or cfg.problems():
        return emit({"ok": True, "results": rows, "total": len(rows), "refreshed": False})
    c = client(cfg)
    cql = f"id in ({', '.join(str(f['id']) for f in favs)})"
    try:
        data = c.get("/rest/api/content/search", qs(cql=cql, limit=min(200, len(favs)),
                                                    expand="space,version,ancestors,container")) or {}
    except jira_api.ApiError as err:
        return emit({"ok": True, "results": rows, "total": len(rows), "refreshed": False,
                     "warning": f"could not refresh favorites: {err}", "curl": err.curl})
    live = {r["id"]: r for r in (shape(x, cfg.site, [], set()) for x in data.get("results") or [])}
    changed = False
    out = []
    for f in favs:
        r = live.get(str(f["id"]))
        if r:
            upd = {k: r.get(k, "") for k in FAV_KEYS}
            if any(f.get(k) != v for k, v in upd.items()) or f.get("missing"):
                changed = True
            f2 = {**f, **upd}
            f2.pop("missing", None)
            out.append((f2, {**r, "favorite": True, "excerpt": "", "hits": []}))
        else:
            changed = changed or not f.get("missing")
            f2 = {**f, "missing": True}
            out.append((f2, {**f2, "favorite": True, "titleHits": [], "hits": [],
                             "excerpt": "not found - deleted, moved out of reach, or no access"}))
    if changed:
        cfg.data["favorites"] = [f for f, _ in out]
        cfg.save()
    return emit({"ok": True, "results": [r for _, r in out], "total": len(out), "refreshed": True})


def do_import_saved(cfg: cc.Config) -> int:
    c = client(cfg)
    rows, path = [], "/rest/api/content/search"
    q = qs(cql="favourite = currentUser() ORDER BY lastmodified DESC", limit=100,
           expand="space,version,ancestors,container")
    try:
        while path and len(rows) < 500:
            data = c.get(path, q) or {}
            rows += [shape(x, cfg.site, [], set()) for x in data.get("results") or []]
            nxt = (data.get("_links") or {}).get("next") or ""
            path, _, q = nxt.partition("?")
    except jira_api.ApiError as err:
        return fail(str(err), 1, curl=err.curl)
    have = {str(f.get("id")) for f in cfg["favorites"] if isinstance(f, dict)}
    new = [r for r in rows if r["id"] not in have]
    now = time.strftime("%Y-%m-%dT%H:%M:%S")
    cfg.data["favorites"] = [f for f in cfg["favorites"] if isinstance(f, dict)] + \
        [{**{k: r.get(k, "") for k in FAV_KEYS}, "added": now} for r in new]
    cfg.save()
    return emit({"ok": True, "found": len(rows), "added": len(new)})


def read_stdin_json(default=None):
    try:
        raw = sys.stdin.read() if not sys.stdin.isatty() else ""
    except OSError:
        raw = ""
    if not raw.strip():
        return default
    return json.loads(raw)


def main(argv: list) -> int:
    if not argv or argv[0] in ("-h", "--help"):
        print(__doc__)
        return 0
    cmd, rest = argv[0], argv[1:]
    try:
        cfg = cc.load()
    except cc.ConfigError as err:
        return fail(str(err), 2)
    if cmd == "--check":
        return do_check(cfg)
    try:
        given = read_stdin_json({}) if cmd in ("--save", "--detect-auth", "--search", "--favorite") else {}
    except ValueError as err:
        return fail(f"stdin is not JSON: {err}", 2)
    if cmd == "--save":
        return do_save(cfg, given or {})
    if cmd == "--detect-auth":
        return do_detect(cfg, given or {})
    if cmd == "--favorite":
        if len(rest) < 2:
            return fail("usage: --favorite add|remove ID...", 2)
        return do_favorite(cfg, rest[0], rest[1:], given if isinstance(given, list) else [])
    if cmd == "--remove-space":
        return do_spaces(cfg, "remove", rest)
    if cmd == "--favorites" and "--no-refresh" in rest:
        return do_favorites(cfg, refresh=False)
    if cfg.problems():
        return fail("Confluence is not set up: " + "; ".join(cfg.problems()), 2, setup=True)
    jira_log.setup("confluence " + cmd, argv, cfg, cache_dir=cc.CACHE_DIR)
    jira_log.secret(cfg["token"] or "")
    if cmd == "--myself":
        try:
            me = me_of(client(cfg))
        except jira_api.ApiError as err:
            return fail(str(err), 1, curl=err.curl)
        return emit({"ok": True, "user": me.get("displayName") or "", "raw": me})
    if cmd == "--add-space":
        return do_spaces(cfg, "add", rest)
    if cmd == "--search":
        return do_search(cfg, given or {})
    if cmd == "--page":
        return do_page(cfg, rest[0] if rest else "")
    if cmd == "--favorites":
        return do_favorites(cfg)
    if cmd == "--import-saved":
        return do_import_saved(cfg)
    return fail(f"unknown command {cmd} (see --help)", 2)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
