#!/usr/bin/env python3
"""jira_api.py - lightweight Jira Cloud REST API query tool + sync engine.

Usage: jira_api.py [OPTIONS] [ISSUE-KEY]

  With no arguments, starts the interactive wizard.

Discovery / setup:
  --init            interactively write ~/.config/jira/config.json (chmod 600)
  --myself          print the authenticated user (JSON)
  --site/--email/--token VALUE   override the config for this run
  --token-stdin     read the token from stdin (keeps it out of `ps`)

Filters (combined with AND into JQL):
  -p, --project KEY      project = "KEY"
  -a, --assignee NAME    assignee = "NAME"    NAME~ means fuzzy (assignee ~)
  -r, --release NAME     fixVersion = "NAME"  (release == fix version)
  -s, --status NAME      status = "NAME"
  -u, --reporter NAME    reporter = "NAME"
  -t, --text TERM        text ~ "TERM"
  -d, --date EXPR        FIELD[:OP]VALUE, e.g. updated:-7d, due:>2024-05-01,
                         created:startOfMonth(); FIELD defaults to "updated"
  -j, --jql RAW          raw JQL, overrides all filters

Ordering / limits:
  -R, --recent           ORDER BY updated DESC (recently updated first)
  -S, --sort EXPR        ORDER BY EXPR, e.g. "priority", "created ASC"
  -n, --max N            max results (default: config defaultMax, 25)

Output:
  -o, --output table|json   table (default) or JSON shape:
                            {key,title,status,assignee,release,priority,labels,description}
  --releases                print ALL releases (versions) as JSON across every
                            project (or just -p PROJECT):
                            [{project,name,released,releaseDate,description}]
  -verbose, --verbose       append the raw curl endpoint reference to
                            /tmp/jira_api_dump.txt and a timestamped request
                            trace to /tmp/jira_api_trace.txt
  --debug                   print the JQL and request URLs
  --no-auth-check           skip the /myself login verification at startup

Sync (load recent changes into the local cache):
  --sync WINDOW     load issues updated within WINDOW, merge into cache
                    WINDOW: 30m | 2h | 7d | 1w | YYYY-MM-DD | full
                            (datetime windows are in the site's local time)
                    cache:  ~/.cache/jira/jiras.json (keyed by issue key)
                    snapshots: ~/.cache/jira/dumps/YYYY_MM_DD[_N].json

Every HTTP request is appended to ~/.cache/jira/curl.log (chmod 600) as a
literal, copy-pasteable curl command line with its timestamp + HTTP code.
The API fields= list comes from commands.conf [jira] (columns + field keys).
"""
from __future__ import annotations

import json
import os
import re
import shlex
import subprocess
import sys
import time
import urllib.parse

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import jira_config  # noqa: E402

CACHE_DIR = jira_config.CACHE_DIR
CACHE_FILE = os.path.join(CACHE_DIR, "jiras.json")
STATE_FILE = os.path.join(CACHE_DIR, "state")
VERSIONS_FILE = os.path.join(CACHE_DIR, "versions.json")
DUMP_DIR = os.path.join(CACHE_DIR, "dumps")
CURL_LOG = os.path.join(CACHE_DIR, "curl.log")
CURL_LOG_MAX = 5 * 1024 * 1024        # rotate: keep the newest ~2MB past 5MB
CURL_DUMP = "/tmp/jira_api_dump.txt"
TRACE_FILE = "/tmp/jira_api_trace.txt"

QUERY_FIELDS = "summary,status,assignee,fixVersions,description,updated,priority,labels"


class ApiError(Exception):
    def __init__(self, code: int, msg: str):
        super().__init__(msg)
        self.code = code


def die(msg: str, code: int = 1):
    print(f"jira-api: {msg}", file=sys.stderr)
    sys.exit(code)


# ------------------------------------------------------------ http (curl)

class Client:
    """Runs every request through the real `curl` binary so the line logged
    to curl.log is EXACTLY what ran (same auth / cert behaviour as the old
    bash tool) and re-runs verbatim in a terminal."""

    def __init__(self, site: str, email: str, token: str, debug=False, verbose=False,
                 timeout=30):
        self.site = site.rstrip("/")
        self.email = email
        self.token = token
        self.debug = debug
        self.verbose = verbose
        self.timeout = timeout
        self.requests = 0

    def url(self, path: str, qs: str = "", ver: int = 2) -> str:
        u = f"{self.site}/rest/api/{ver}{path}"
        return f"{u}?{qs}" if qs else u

    def argv(self, url: str) -> list:
        return ["curl", "-sS", "-m", str(self.timeout), "-u", f"{self.email}:{self.token}",
                "-H", "Accept: application/json", "-w", "\\n%{http_code}", url]

    def get(self, path: str, qs: str = "", ver: int = 2):
        url = self.url(path, qs, ver)
        if self.debug:
            print(f"jira-api: GET {url}", file=sys.stderr)
        argv = self.argv(url)
        t0 = time.time()
        try:
            p = subprocess.run(argv, capture_output=True, text=True)
        except FileNotFoundError:
            die("curl is not installed (brew install curl)")
        self.requests += 1
        out = p.stdout
        body, _, code_s = out.rpartition("\n")
        code = int(code_s) if code_s.strip().isdigit() else 0
        log_curl(argv, code if p.returncode == 0 else f"ERR{p.returncode}")
        if self.verbose:
            trace(f"GET {url} -> {code} ({time.time() - t0:.2f}s, {len(body)} bytes)")
        if p.returncode != 0:
            raise ApiError(0, f"curl failed ({p.stderr.strip() or 'exit ' + str(p.returncode)}): {url}")
        if code >= 400:
            if code == 401:
                raise ApiError(code, "authentication failed (HTTP 401) - check email / token "
                                     "(setup sheet or 'jira_api.py --init')")
            if code == 403:
                raise ApiError(code, "forbidden (HTTP 403) - your account lacks permission for this query")
            if code == 404:
                raise ApiError(code, f"not found (HTTP 404): {body[:300]}")
            raise ApiError(code, f"API error HTTP {code}: {body[:300]}")
        try:
            return json.loads(body) if body.strip() else None
        except ValueError:
            raise ApiError(code, f"non-JSON response from {url}: {body[:200]}")


def log_curl(argv: list, code) -> None:
    """Append one copy-pasteable line: `<time> <code>  curl ...`. The file
    holds the basic-auth credentials (that's what makes it re-runnable), so
    it is kept chmod 600 inside ~/.cache/jira."""
    try:
        os.makedirs(CACHE_DIR, exist_ok=True)
        if os.path.exists(CURL_LOG) and os.path.getsize(CURL_LOG) > CURL_LOG_MAX:
            with open(CURL_LOG, "rb") as fh:
                fh.seek(-2 * 1024 * 1024, os.SEEK_END)
                tail = fh.read()
            tail = tail[tail.find(b"\n") + 1:]
            with open(CURL_LOG, "wb") as fh:
                fh.write(tail)
        fd = os.open(CURL_LOG, os.O_WRONLY | os.O_APPEND | os.O_CREAT, 0o600)
        with os.fdopen(fd, "a", encoding="utf-8") as fh:
            fh.write(f"{time.strftime('%Y-%m-%d %H:%M:%S')} {code}  {shlex.join(argv)}\n")
        os.chmod(CURL_LOG, 0o600)
    except OSError:
        pass


def trace(msg: str) -> None:
    try:
        with open(TRACE_FILE, "a", encoding="utf-8") as fh:
            fh.write(f"+{time.time():.6f} {msg}\n")
    except OSError:
        pass


def show_curl_ref(c: Client) -> None:
    u = f"{c.email}:{c.token}"
    s = c.site
    lines = [
        f"--- {time.strftime('%Y-%m-%d %H:%M:%S')} ---",
        f'jira_api.py - raw curl endpoints (basic auth: -u "{u}")',
        f"every request actually made is also logged to {CURL_LOG}",
        "",
        "1) Search issues (v3 JQL - used by every query; old /rest/api/2/search is gone)",
        f"   GET /rest/api/3/search/jql?jql=<urlencoded>&fields={QUERY_FIELDS}&maxResults=<n>",
        f'   curl -u "{u}" "{s}/rest/api/3/search/jql?jql=project%20%3D%20SAM1%20ORDER%20BY%20updated%20DESC&fields={QUERY_FIELDS}&maxResults=25"',
        "2) Single issue (v2 - plain-text description)",
        "   GET /rest/api/2/issue/{key}",
        f'   curl -u "{u}" "{s}/rest/api/2/issue/SAM1-1"',
        "3) List projects (interactive menu)",
        "   GET /rest/api/2/project",
        f'   curl -u "{u}" "{s}/rest/api/2/project"',
        "4) Assignable users (interactive assignee suggestions)",
        "   GET /rest/api/2/user/assignable/search?project=KEY&maxResults=50",
        f'   curl -u "{u}" "{s}/rest/api/2/user/assignable/search?project=SAM1&maxResults=50"',
        "5) Versions / releases (interactive release menu; maps to fixVersion)",
        "   GET /rest/api/2/project/{key}/versions",
        f'   curl -u "{u}" "{s}/rest/api/2/project/SAM1/versions"',
        "6) Statuses (interactive status menu)",
        "   GET /rest/api/2/status",
        f'   curl -u "{u}" "{s}/rest/api/2/status"',
        "7) Current user (login verification)",
        "   GET /rest/api/2/myself",
        f'   curl -u "{u}" "{s}/rest/api/2/myself"',
        "",
        "Notes:",
        "  - Token: https://id.atlassian.com -> Security -> API tokens",
        "  - v3 search paginates with nextPageToken (no \"total\" field)",
        "  - v3 descriptions are Atlassian Document Format (ADF)",
        "", "",
    ]
    with open(CURL_DUMP, "a", encoding="utf-8") as fh:
        fh.write("\n".join(lines))
    print(f"jira-api: curl reference appended to {CURL_DUMP}", file=sys.stderr)


def qenc(v: str) -> str:
    return urllib.parse.quote(v, safe="")


# ------------------------------------------------------------ transforms

def alt(*vals):
    """jq's `a // b`: first value that is not null/false (\"\" counts!);
    the last alternative is returned as-is when none qualifies."""
    for v in vals[:-1]:
        if v is not None and v is not False:
            return v
    return vals[-1] if vals else None


def adf_text(v) -> str:
    """jq: [.. | objects | select(has("text")) | .text] | join("\\n") for an
    ADF document; plain strings pass through."""
    if isinstance(v, dict):
        out = []

        def walk(n):
            if isinstance(n, dict):
                if "text" in n:
                    out.append(n["text"] if isinstance(n["text"], str) else json.dumps(n["text"]))
                for val in n.values():
                    walk(val)
            elif isinstance(n, list):
                for x in n:
                    walk(x)
        walk(v)
        return "\n".join(out)
    if v is None:
        return ""
    return v if isinstance(v, str) else json.dumps(v)


def person(v) -> str:
    if not isinstance(v, dict):
        return ""
    return alt(v.get("name"), v.get("displayName")) or ""


def stringify(v) -> str:
    """Raw Jira field (custom columns) -> display string."""
    if v is None:
        return ""
    if isinstance(v, str):
        return v
    if isinstance(v, (int, float, bool)):
        return json.dumps(v)
    if isinstance(v, dict):
        if v.get("type") == "doc":
            return adf_text(v)
        for k in ("displayName", "name", "value", "key"):
            if isinstance(v.get(k), str):
                return v[k]
        return json.dumps(v, ensure_ascii=False)
    if isinstance(v, list):
        return ", ".join(stringify(x) for x in v)
    return str(v)


def simple_row(issue: dict) -> dict:
    """--output json shape (the legacy to_json)."""
    f = issue.get("fields") or {}
    return {
        "key": issue.get("key"),
        "title": alt(f.get("summary"), "") ,
        "status": alt((f.get("status") or {}).get("name"), ""),
        "assignee": person(f.get("assignee")),
        "release": ", ".join(v.get("name", "") for v in (f.get("fixVersions") or [])),
        "priority": alt((f.get("priority") or {}).get("name"), ""),
        "labels": ", ".join(f.get("labels") or []),
        "description": alt(adf_text(f.get("description")) if f.get("description") is not None else None, ""),
    }


def cache_entry(issue: dict, vers_map: dict, comments, api_fields: list) -> dict:
    """Full cache entry (the legacy sync jq), plus any extra raw field that
    [jira] columns asked for. Only keys whose source fields were fetched are
    set, so a merge never blanks a field a narrower request didn't include."""
    f = issue.get("fields") or {}
    proj = (f.get("project") or {}).get("key") or ""
    pv = vers_map.get(proj) or {}
    names = [v.get("name", "") for v in (f.get("fixVersions") or [])]
    fetched = set(api_fields)
    e = {"key": issue.get("key")}
    if "summary" in fetched:
        e["title"] = alt(f.get("summary"), "")
    if "status" in fetched:
        e["status"] = alt((f.get("status") or {}).get("name"), "")
    if "assignee" in fetched:
        e["assignee"] = person(f.get("assignee"))
    if "fixVersions" in fetched:
        labels, dates, flags = [], [], []
        for n in names:
            info = pv.get(n)
            d = (info or {}).get("date") or ""
            labels.append(n if d == "" else f"{n} ({d})")
            if d:
                dates.append(d)
            if info is not None:
                flags.append(bool(info.get("released")))
        e["release"] = ", ".join(names)
        e["releaseLabel"] = ", ".join(labels)
        e["releaseDate"] = ", ".join(dates)
        e["releaseStatus"] = "" if not flags else ("Released" if any(flags) else "Upcoming")
    if "priority" in fetched:
        e["priority"] = alt((f.get("priority") or {}).get("name"), "")
    if "labels" in fetched:
        e["labels"] = ", ".join(f.get("labels") or [])
    if "description" in fetched:
        d = f.get("description")
        e["description"] = adf_text(d) if d is not None else ""
    if "updated" in fetched:
        e["updated"] = alt(f.get("updated"), "")
    if "reporter" in fetched:
        e["reporter"] = person(f.get("reporter"))
    if "project" in fetched:
        e["project"] = proj
    if comments is not None:
        e["comments"] = comments
    known = {s for srcs in jira_config.FIELD_SOURCES.values() for s in srcs}
    for fld in api_fields:
        if fld not in known:
            e[fld] = stringify(f.get(fld))
    return e


# ------------------------------------------------------------ cache io

def read_json(path: str, default):
    try:
        with open(path, encoding="utf-8") as fh:
            return json.load(fh)
    except (OSError, ValueError):
        return default


def write_json(path: str, obj, mode=0o644, indent=2) -> None:
    d = os.path.dirname(path)
    os.makedirs(d, exist_ok=True)
    tmp = os.path.join(d, f".{os.path.basename(path)}.{os.getpid()}")
    with open(tmp, "w", encoding="utf-8") as fh:
        json.dump(obj, fh, indent=indent, ensure_ascii=False)
        fh.write("\n")
    os.chmod(tmp, mode)
    os.replace(tmp, path)


def snapshot(cache: dict, keep: int) -> str:
    os.makedirs(DUMP_DIR, exist_ok=True)
    day = time.strftime("%Y_%m_%d")
    n = 0
    while True:
        path = os.path.join(DUMP_DIR, f"{day}.json" if n == 0 else f"{day}_{n}.json")
        if not os.path.exists(path):
            break
        n += 1
    write_json(path, cache, mode=0o600)
    if keep > 0:
        snaps = sorted((os.path.join(DUMP_DIR, f) for f in os.listdir(DUMP_DIR)
                        if f.endswith(".json")), key=os.path.getmtime)
        for old in snaps[:-keep]:
            try:
                os.unlink(old)
            except OSError:
                pass
    return path


# ------------------------------------------------------------ jql / queries

class Filters:
    def __init__(self):
        self.project = self.assignee = self.release = self.status = ""
        self.reporter = self.text = self.date = self.jql = self.sort = ""
        self.recent = False


def parse_date_spec(spec: str) -> str:
    if ":" in spec:
        field, val = spec.split(":", 1)
    else:
        field, val = "updated", spec
    m = re.match(r"^([<>]=?|=|~)(.*)$", val)
    if m:
        op, val = m.group(1), m.group(2)
    elif re.match(r"^-[0-9]", val):
        op = ">="
    else:
        op = "="
    if not val:
        die(f"empty value in date spec: {spec}")
    return f"{field} {op} {val}"


def build_jql(f: Filters) -> str:
    if f.jql:
        return f.jql
    parts = []
    if f.project:
        parts.append(f'project = "{f.project}"')
    if f.assignee:
        if f.assignee.endswith("~"):
            parts.append(f'assignee ~ "{f.assignee[:-1]}"')
        else:
            parts.append(f'assignee = "{f.assignee}"')
    if f.release:
        parts.append(f'fixVersion = "{f.release}"')
    if f.status:
        parts.append(f'status = "{f.status}"')
    if f.reporter:
        parts.append(f'reporter = "{f.reporter}"')
    if f.text:
        parts.append(f'text ~ "{f.text}"')
    if f.date:
        parts.append(parse_date_spec(f.date))
    q = " AND ".join(parts)
    if f.recent:
        q += " ORDER BY updated DESC"
    elif f.sort:
        q += f" ORDER BY {f.sort}"
    return q


def render(body: dict, output: str) -> None:
    issues = body.get("issues") or []
    if not issues:
        print("No issues found")
        return
    if output == "json":
        print(json.dumps([simple_row(i) for i in issues], indent=2, ensure_ascii=False))
        return
    rows = [["KEY", "STATUS", "ASSIGNEE", "RELEASE", "UPDATED", "TITLE"]]
    for i in issues:
        f = i.get("fields") or {}
        rel = ",".join(v.get("name", "") for v in (f.get("fixVersions") or [])) or "-"
        rows.append([
            i.get("key", ""),
            alt((f.get("status") or {}).get("name"), "-"),
            alt((f.get("assignee") or {}).get("displayName"), "-"),
            rel,
            (f.get("updated") or "-")[:10],
            (f.get("summary") or "").replace("\t", " "),
        ])
    widths = [max(len(r[c]) for r in rows) for c in range(len(rows[0]) - 1)]
    for r in rows:
        print("  ".join(r[c].ljust(widths[c]) for c in range(len(widths))) + "  " + r[-1])
    if body.get("isLast", True):
        print(f"Total: {len(issues)}")
    else:
        print(f"Showing {len(issues)} (more pages available; raise -n to fetch more)")


def run_query(c: Client, f: Filters, maxn: int, output: str, debug: bool) -> None:
    jql = build_jql(f)
    if debug:
        print(f"jira-api: JQL: {jql or '<empty>'}", file=sys.stderr)
    if not jql:
        die("no filters given - Jira requires a bounded query (add -p/-a/-s/-d/... or -j)")
    body = c.get("/search/jql", f"jql={qenc(jql)}&fields={QUERY_FIELDS}&maxResults={maxn}", 3)
    render(body or {}, output)


def issue(c: Client, key: str, output: str) -> None:
    render({"issues": [c.get(f"/issue/{key}")]}, output)


def project_versions(c: Client, proj: str) -> list:
    return c.get(f"/project/{proj}/versions") or []


def releases(c: Client, project: str = "", projects: list | None = None) -> list:
    """Every release (version): [{project,name,released,releaseDate,description}].
    One versions call per project - the date lives on the version object."""
    if project:
        keys = [project]
    elif projects:
        keys = projects
    else:
        keys = [p.get("key") for p in (c.get("/project") or [])]
    out = []
    for p in keys:
        try:
            vs = project_versions(c, p)
        except ApiError:
            if project:
                raise
            continue
        for v in vs:
            out.append({"project": p, "name": v.get("name"),
                        "released": alt(v.get("released"), False),
                        "releaseDate": alt(v.get("releaseDate"), ""),
                        "description": alt(v.get("description"), "")})
    return out


def window_since(w: str):
    """--sync WINDOW -> JQL bound (None = full)."""
    if w == "full":
        return None
    if re.match(r"^[0-9]+[smhdw]$", w):
        return f"-{w}"
    if re.match(r"^[0-9]{4}-[0-9]{2}-[0-9]{2}$", w):
        return w
    if re.match(r"^[0-9]{4}-[0-9]{2}-[0-9]{2}[T ][0-9]{2}:[0-9]{2}(:[0-9]{2})?Z?$", w):
        # JQL accepts "yyyy-MM-dd HH:mm" (no seconds)
        return w.rstrip("Z").replace("T", " ")[:16]
    die(f"invalid window: {w} (use 30m, 2h, 7d, 1w, 2026-09-10, or full)", 2)


def sync(c: Client, window: str, projects=None, jql: str = "", api_fields=None,
         fetch_comments=True, snapshot_keep=30, quiet=False, debug=False) -> dict:
    """Load issues updated within `window` (optionally only `projects` / a
    custom `jql`), merge them into the cache, return stats + matched keys."""
    fields = api_fields or jira_config.api_fields()
    since = window_since(window)
    clauses = []
    if jql:
        clauses.append(f"({jql})")
    if projects:
        clauses.append("project in (" + ", ".join(f'"{p}"' for p in projects) + ")")
    clauses.append(f'updated >= "{since}"' if since else "project != null")
    q = " AND ".join(clauses)
    if debug:
        print(f"jira-api: JQL: {q}", file=sys.stderr)
    raw = []
    token = ""
    while True:
        qs = f"jql={qenc(q)}&fields={','.join(fields)}&maxResults=100"
        if token:
            qs += f"&nextPageToken={qenc(token)}"
        page = c.get("/search/jql", qs, 3) or {}
        raw.extend(page.get("issues") or [])
        if page.get("isLast", True):
            break
        token = page.get("nextPageToken") or ""
        if not token:
            raise ApiError(0, "pagination: no nextPageToken but isLast=false")
    # version name -> {released, date} per project: the release date + flag
    # live on the VERSION object, not the issue's fixVersions
    vers_map = read_json(VERSIONS_FILE, {})
    if not isinstance(vers_map, dict):
        vers_map = {}
    if "fixVersions" in fields:
        for proj in sorted({((i.get("fields") or {}).get("project") or {}).get("key", "")
                            for i in raw} - {""}):
            try:
                vers_map[proj] = {v.get("name"): {"released": alt(v.get("released"), False),
                                                  "date": alt(v.get("releaseDate"), "")}
                                  for v in project_versions(c, proj)}
            except ApiError:
                continue
        write_json(VERSIONS_FILE, vers_map)
    entries = {}
    for i in raw:
        comments = None
        if fetch_comments:
            body = c.get(f"/issue/{i.get('key')}", "fields=comment") or {}
            comments = [{"author": alt((cm.get("author") or {}).get("displayName"), ""),
                         "body": alt(cm.get("body"), ""),
                         "created": alt(cm.get("created"), ""),
                         "updated": alt(cm.get("updated"), "")}
                        for cm in ((body.get("fields") or {}).get("comment") or {}).get("comments") or []]
        entries[i.get("key")] = cache_entry(i, vers_map, comments, fields)
    cache = read_json(CACHE_FILE, {})
    if not isinstance(cache, dict):
        cache = {}
    if entries:
        for k, e in entries.items():
            merged = dict(cache.get(k) or {})
            merged.update(e)
            cache[k] = merged
        write_json(CACHE_FILE, cache, mode=0o600)
    os.makedirs(CACHE_DIR, exist_ok=True)
    with open(STATE_FILE, "w", encoding="utf-8") as fh:
        fh.write(f"LAST_SYNC={time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())}\n"
                 f"WINDOW={window}\nUPDATED={len(entries)}\nTOTAL={len(cache)}\n")
    if not quiet:
        print(f"jira-api: sync {window}: fetched {len(raw)}, changed {len(entries)}, "
              f"cache now {len(cache)}", file=sys.stderr)
    snap = ""
    if entries and cache:
        snap = snapshot(cache, snapshot_keep)
        if not quiet:
            print(f"jira-api: snapshot: {snap}", file=sys.stderr)
    return {"fetched": len(raw), "changed": len(entries), "total": len(cache),
            "keys": list(entries), "snapshot": snap, "jql": q}


# ------------------------------------------------------------ interactive

def ask(prompt: str) -> str:
    try:
        import readline  # noqa: F401  (line editing for input())
    except ImportError:
        pass
    try:
        return input(prompt)
    except EOFError:
        return ""


def menu(label: str, default: str, options: list) -> str:
    print(f"{label} [{default or 'any'}]", file=sys.stderr)
    if options:
        for i, d in enumerate(options, 1):
            if "|" in d:
                a, b = d.split("|", 1)
                print(f"  {i:2d}) {a} ({b})", file=sys.stderr)
            else:
                print(f"  {i:2d}) {d}", file=sys.stderr)
        print("  0) any", file=sys.stderr)
    reply = ask("> ").strip() or default
    if reply == "0":
        return ""
    if reply.isdigit() and 1 <= int(reply) <= len(options):
        return options[int(reply) - 1].split("|", 1)[0]
    return reply


def interactive(c: Client, f: Filters, default_project: str, default_max: int,
                output: str, debug: bool) -> None:
    print("jira-api interactive - Enter accepts the [default], 0 = any\n")

    def quiet_get(path, qs=""):
        try:
            return c.get(path, qs) or []
        except ApiError:
            return []

    projs = [f"{p.get('key')}|{p.get('name')}" for p in quiet_get("/project")]
    while True:
        p = menu("Project", f.project or default_project, projs)
        if not p or not projs or any(p == o.split("|", 1)[0] for o in projs):
            break
        print(f'Project "{p}" not found on {c.site} - choose a listed key or 0 for any', file=sys.stderr)
    f.project = p
    if p:
        users = ["me|currentUser()"] + [
            f"{alt(u.get('name'), u.get('displayName'))}|{u.get('displayName')}"
            for u in quiet_get("/user/assignable/search", f"project={qenc(p)}&maxResults=50")]
        a = menu("Assignee", f.assignee, users)
        f.assignee = "currentUser()" if a == "me" else a
        vers = sorted({v.get("name") for v in quiet_get(f"/project/{p}/versions")})
        f.release = menu("Release (fixVersion)", f.release, vers)
    sts = sorted({s.get("name") for s in quiet_get("/status")})
    f.status = menu("Status", f.status, sts)
    days = ask("Only updated within last N days, 0 = all [30]: ").strip() or "30"
    if days != "0":
        f.date = f"updated:-{days}d"
    sorts = ["updated DESC|recently updated first", "created DESC|newest created first",
             "priority|priority", "key|key"]
    s = menu("Sort", f.sort or "updated DESC", sorts)
    if s:
        f.sort = s
    m = ask(f"Max results [{default_max}]: ").strip()
    print()
    run_query(c, f, int(m) if m.isdigit() else default_max, output, debug)


def do_init() -> None:
    path = jira_config.CONFIG_JSON
    print(f"jira-api setup - writing config to {path}")
    site = ask("Jira site URL (e.g. https://your-org.atlassian.net): ").strip()
    if not site:
        die("site URL required")
    email = ask("Email: ").strip()
    if not email:
        die("email required")
    import getpass
    token = getpass.getpass("API token (https://id.atlassian.com/manage-profile/security/api-tokens): ").strip()
    if not token:
        die("token required")
    proj = ask("Default project key (Enter for none): ").strip()
    mx = ask("Default max results [25]: ").strip() or "25"
    jira_config.save({"site": site, "email": email, "token": token, "defaultProject": proj,
                      "defaultMax": int(mx) if mx.isdigit() else 25})
    print(f"Wrote {path} (chmod 600). Test with: {sys.argv[0]} --myself")


# ------------------------------------------------------------ main

def parse_args(argv: list):
    o = {"init": False, "myself": False, "site": "", "email": "", "token": "",
         "token_stdin": False, "max": "", "output": "table", "debug": False,
         "verbose": False, "no_auth": False, "sync": "", "releases": False,
         "interactive": False, "issue": ""}
    f = Filters()
    valued = {"-p": "project", "--project": "project", "-a": "assignee", "--assignee": "assignee",
              "-r": "release", "--release": "release", "-s": "status", "--status": "status",
              "-u": "reporter", "--reporter": "reporter", "-t": "text", "--text": "text",
              "-d": "date", "--date": "date", "-j": "jql", "--jql": "jql",
              "-S": "sort", "--sort": "sort"}
    opt_valued = {"--site": "site", "--email": "email", "--token": "token", "-n": "max",
                  "--max": "max", "-o": "output", "--output": "output", "--sync": "sync",
                  "--config": None}
    i = 0
    while i < len(argv):
        a = argv[i]
        name, eq, inline = a.partition("=") if a.startswith("--") else (a, "", "")

        def val():
            nonlocal i
            if eq:
                return inline
            if i + 1 >= len(argv):
                die(f"option {a} requires a value")
            i += 1
            return argv[i]

        if name in ("-h", "--help"):
            print(__doc__.strip())
            sys.exit(0)
        elif name == "--init":
            o["init"] = True
        elif name == "--myself":
            o["myself"] = True
        elif name == "--token-stdin":
            o["token_stdin"] = True
        elif name in ("-R", "--recent"):
            f.recent = True
        elif name in ("-verbose", "--verbose"):
            o["verbose"] = True
        elif name == "--releases":
            o["releases"] = True
        elif name == "--no-auth-check":
            o["no_auth"] = True
        elif name in ("-i", "--interactive"):
            o["interactive"] = True
        elif name == "--debug":
            o["debug"] = True
        elif name in valued:
            setattr(f, valued[name], val())
        elif name in opt_valued:
            v = val()
            if opt_valued[name]:
                o[opt_valued[name]] = v
        elif a.startswith("-"):
            die(f"unknown option: {a} (see --help)")
        else:
            if o["issue"]:
                die(f"unexpected argument: {a}")
            o["issue"] = a
        i += 1
    if o["output"] not in ("table", "json"):
        die("--output must be 'table' or 'json'")
    if o["max"] and not o["max"].isdigit():
        die("--max must be a number")
    return o, f


def main(argv: list) -> int:
    o, f = parse_args(argv)
    if o["init"]:
        do_init()
        return 0
    if o["token_stdin"]:
        o["token"] = sys.stdin.readline().strip()
    try:
        cfg = jira_config.load(o["site"], o["email"], o["token"])
    except jira_config.ConfigError as err:
        die(str(err), 2)
    missing = [f"JIRA_{k.upper()}" for k in ("site", "email", "token") if not cfg[k]]
    if missing:
        die(f"missing config: {' '.join(missing)} - run 'jira_api.py --init', the setup sheet, or export them", 2)
    for n in cfg.notes:
        print(f"jira-api: {n}", file=sys.stderr)
    c = Client(cfg.site, cfg["email"], cfg["token"], debug=o["debug"], verbose=o["verbose"])
    maxn = int(o["max"] or cfg["defaultMax"] or 25)
    try:
        if o["verbose"]:
            trace(f"--- {time.strftime('%Y-%m-%d %H:%M:%S')} jira_api.py {' '.join(argv)}")
            print(f"jira-api: trace appended to {TRACE_FILE}", file=sys.stderr)
        if not o["no_auth"] and not o["myself"]:
            me = c.get("/myself") or {}
            print(f"jira-api: login OK ({me.get('displayName') or 'unknown user'})", file=sys.stderr)
        if o["verbose"]:
            show_curl_ref(c)
            nothing = not any([f.project, f.assignee, f.release, f.status, f.reporter, f.text,
                               f.date, f.jql, o["issue"], o["sync"], o["interactive"],
                               f.recent, o["myself"], o["releases"]])
            if nothing:
                return 0
        if o["myself"]:
            print(json.dumps(c.get("/myself"), indent=2, ensure_ascii=False))
            return 0
        if o["releases"]:
            print(json.dumps(releases(c, f.project), indent=2, ensure_ascii=False))
            return 0
        if o["sync"]:
            sync(c, o["sync"], fetch_comments=bool(cfg["fetchComments"]),
                 snapshot_keep=int(cfg.get("snapshotKeep", 30)), debug=o["debug"])
            return 0
        if o["issue"]:
            issue(c, o["issue"], o["output"])
            return 0
        if o["interactive"] or not argv:
            interactive(c, f, cfg["defaultProject"], maxn, o["output"], o["debug"])
            return 0
        run_query(c, f, maxn, o["output"], o["debug"])
    except ApiError as err:
        die(str(err))
    except KeyboardInterrupt:
        return 130
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
