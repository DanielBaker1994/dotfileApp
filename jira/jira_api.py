#!/usr/bin/env python3
"""jira_api.py - lightweight Jira REST API query tool + sync engine
(Server / Data Center with a Bearer token, or Cloud with email + token).

Usage: jira_api.py [OPTIONS] [ISSUE-KEY]

  With no arguments, starts the interactive wizard.

Discovery / setup:
  --init            interactively write ~/.config/jira/config.json (chmod 600)
  --myself          connectivity test: GET /rest/api/2/myself (prints JSON)
  --site/--email/--token VALUE   override the config for this run
  --auth bearer|basic            override the auth mode for this run
  --token-stdin     read the token from stdin (keeps it out of `ps`)
  --list-fields     every Jira field id + name (find customfield ids to map
                    in team.json custom_fields); "mapped" = your alias

curl (every request is a plain curl - see and re-run it yourself):
  --curl            PRINT the curl command(s) instead of running them, e.g.
                      jira_api.py --curl              (the /myself auth test)
                      jira_api.py --curl -p PROJ -R   (the search request)
                      jira_api.py --curl --job partial_search --arg query=foo
  --mask            with --curl: write $JIRA_TOKEN instead of the real token
  On any HTTP failure the failing request is printed as a runnable curl.

team.json (~/.config/jira/team.json, example: jira/team.example.json):
  --jobs                  list jobs + JQL templates (and their args)
  --job KEY, --template KEY   run a job / jql_templates entry as the query
  --arg NAME=VALUE        fill a job/template {placeholder} (repeatable);
                          {projects} defaults to project_keys
  --board ID|NAME         issues on an agile board (boards[] id or name);
                          combines with -j / --job / filters as JQL

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
DIRECTORY_FILE = os.path.join(CACHE_DIR, "directory.json")   # the pickers' lists
DUMP_DIR = os.path.join(CACHE_DIR, "dumps")
CURL_LOG = os.path.join(CACHE_DIR, "curl.log")
CURL_LOG_MAX = 5 * 1024 * 1024        # rotate: keep the newest ~2MB past 5MB
CURL_DUMP = "/tmp/jira_api_dump.txt"
TRACE_FILE = "/tmp/jira_api_trace.txt"

QUERY_FIELDS = "summary,status,assignee,fixVersions,description,updated,priority,labels"


class ApiError(Exception):
    def __init__(self, code: int, msg: str, curl: str = ""):
        super().__init__(msg)
        self.code = code
        self.curl = curl    # the failing request as a runnable curl ($JIRA_TOKEN)


def die(msg: str, code: int = 1):
    print(f"jira-api: {msg}", file=sys.stderr)
    sys.exit(code)


# ------------------------------------------------------------ http (curl)

class Client:
    """Runs every request through the real `curl` binary so the line logged
    to curl.log is EXACTLY what ran and re-runs verbatim in a terminal.

    Paths come from team.json `api_endpoints` (resolve_path). Auth:
      bearer  -H 'Authorization: Bearer TOKEN'   (Server / Data Center PAT)
      basic   -u 'EMAIL:TOKEN'                   (Cloud)
    `dry` (--curl): print each request as a curl command instead of running it.
    """

    def __init__(self, site: str, token: str, email: str = "", auth: str = "bearer",
                 team: dict | None = None, debug=False, verbose=False, timeout=None,
                 dry=False, mask=False):
        self.site = site.rstrip("/")
        self.email = email
        self.token = token
        self.auth = auth
        self.team = team if team is not None else jira_config.load_team()
        self.debug = debug
        self.verbose = verbose
        sd = self.team.get("search_defaults") or {}
        self.timeout = int(timeout or sd.get("timeout_seconds") or 30)
        self.dry = dry
        self.mask = mask
        self.captured: list | None = None   # dry + list: collect curls instead of printing
        self.requests = 0

    @classmethod
    def from_config(cls, cfg, **kw) -> "Client":
        return cls(cfg.site, cfg["token"], email=cfg["email"] or "", auth=cfg.auth,
                   team=jira_config.load_team(cfg.data), **kw)

    def sd(self, k: str) -> int:
        v = (self.team.get("search_defaults") or {}).get(k)
        return int(v if v is not None else jira_config.DEFAULT_SEARCH[k])

    def path(self, name: str, **params) -> str:
        return jira_config.resolve_path(self.team, name, self.site, **params)

    def url(self, path: str, qs: str = "") -> str:
        u = path if path.startswith(("http://", "https://")) else f"{self.site}{path}"
        return f"{u}?{qs}" if qs else u

    def curl_argv(self, url: str, method: str = "GET", masked: bool = False) -> list:
        """The canonical, copy-pasteable request:
        curl -X GET -H 'Content-Type: application/json' -H 'Authorization: Bearer T' 'URL'"""
        tok = "$JIRA_TOKEN" if masked else self.token
        argv = ["curl", "-X", method, "-H", "Content-Type: application/json"]
        if self.auth == "basic":
            argv += ["-u", f"{self.email}:{tok}"]
        else:
            argv += ["-H", f"Authorization: Bearer {tok}"]
        return argv + [url]

    def curl_cmd(self, url: str, method: str = "GET", masked: bool = False) -> str:
        """curl_argv as one shell line. Masked: the token becomes $JIRA_TOKEN
        inside double quotes, so it still runs after `export JIRA_TOKEN=...`."""
        parts = []
        for a in self.curl_argv(url, method, masked):
            if masked and "$JIRA_TOKEN" in a:
                parts.append(f'"{a}"')
            elif a == "curl" or re.match(r"^-[A-Za-z]$", a) or a in ("GET", "POST"):
                parts.append(a)
            else:
                parts.append("'" + a.replace("'", "'\\''") + "'")
        return " ".join(parts)

    def get(self, path: str, qs: str = ""):
        url = self.url(path, qs)
        if self.dry:
            self.requests += 1
            cmd = self.curl_cmd(url, masked=self.mask)
            if self.captured is not None:
                self.captured.append(cmd)
            else:
                print(cmd)
            return None
        if self.debug:
            print(f"jira-api: GET {url}", file=sys.stderr)
        base = self.curl_argv(url)
        # run = the canonical command + silent/timeout/status-code flags
        argv = base[:1] + ["-sS", "-m", str(self.timeout)] + base[1:-1] + ["-w", "\\n%{http_code}", url]
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
        repro = self.curl_cmd(url, masked=True)
        if p.returncode != 0:
            raise ApiError(0, f"curl failed ({p.stderr.strip() or 'exit ' + str(p.returncode)}): {url}", repro)
        if code >= 400:
            who = ("email + API token (basic)" if self.auth == "basic"
                   else "personal access token (Authorization: Bearer)")
            if code == 401:
                raise ApiError(code, f"authentication failed (HTTP 401) - the {who} was rejected by "
                                     f"{self.site}; fix it in the setup sheet or 'jira_api.py --init'", repro)
            if code == 403:
                raise ApiError(code, "forbidden (HTTP 403) - your account lacks permission for this "
                                     "query (or a CAPTCHA is pending - log in once in a browser)", repro)
            if code == 404:
                raise ApiError(code, f"not found (HTTP 404): {body[:300]}", repro)
            raise ApiError(code, f"API error HTTP {code}: {body[:300]}", repro)
        try:
            return json.loads(body) if body.strip() else None
        except ValueError:
            raise ApiError(code, f"non-JSON response from {url} (HTTP {code}; wrong site URL or an "
                                 f"SSO login page?): {body[:200]}", repro)

    def search(self, jql: str, fields: str, max_total: int | None = None,
               page_size: int | None = None) -> dict:
        """All issues matching `jql` (up to max_total), paginating the way the
        configured search endpoint expects: v2 /search (Server/DC) uses
        startAt/total; Cloud's /search/jql uses nextPageToken. page_size =
        maxResults of one request (default: search_defaults.max_results_search).
        Returns {issues, isLast, total} (total: v2 only, else None)."""
        path = self.path("search")
        page = page_size or self.sd("max_results_search")
        total = None
        if max_total:
            page = min(page, max_total)
        issues: list = []
        start, token, last = 0, "", True
        while True:
            qs = f"jql={qenc(jql)}&fields={fields}&maxResults={page}"
            if path.endswith("/search/jql"):
                if token:
                    qs += f"&nextPageToken={qenc(token)}"
            else:
                qs += f"&startAt={start}"
            body = self.get(path, qs) or {}
            got = body.get("issues") or []
            issues.extend(got)
            if path.endswith("/search/jql"):
                last = body.get("isLast", True)
                token = body.get("nextPageToken") or ""
                if not last and not token:
                    raise ApiError(0, "pagination: no nextPageToken but isLast=false")
            else:
                start += len(got)
                total = int(body.get("total") or 0)
                last = not got or start >= total
            if last or (max_total and len(issues) >= max_total):
                break
        if max_total and len(issues) > max_total:
            issues, last = issues[:max_total], False
        return {"issues": issues, "isLast": last, "total": total}

    def board_issues(self, board_id, jql: str, fields: str, max_total: int) -> dict:
        path = self.path("board_issues", board_id=board_id)
        page = min(self.sd("page_size"), max_total)
        issues: list = []
        start = 0
        while True:
            qs = f"fields={fields}&maxResults={page}&startAt={start}"
            if jql:
                qs = f"jql={qenc(jql)}&" + qs
            body = self.get(path, qs) or {}
            got = body.get("issues") or []
            issues.extend(got)
            start += len(got)
            if not got or start >= int(body.get("total") or 0) or len(issues) >= max_total:
                break
        return {"issues": issues[:max_total], "isLast": len(issues) <= max_total}


def log_curl(argv: list, code) -> None:
    """Append one copy-pasteable line: `<time> <code>  curl ...`. The file
    holds the real token (that's what makes it re-runnable), so it is kept
    chmod 600 inside ~/.cache/jira."""
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
    """--verbose: one runnable curl per configured api_endpoints entry."""
    samples = {"key": "PROJ-1", "project": "PROJ", "board_id": "1"}
    lines = [f"--- {time.strftime('%Y-%m-%d %H:%M:%S')} ---",
             f"jira_api.py - raw curl per team.json api_endpoints ({c.auth} auth; "
             "export JIRA_TOKEN=... first)",
             f"every request actually made is also logged to {CURL_LOG}", ""]
    for name in (c.team.get("api_endpoints") or {}):
        try:
            path = c.path(name, **samples)
        except jira_config.ConfigError as err:
            lines.append(f"{name}: {err}")
            continue
        qs = f"jql={qenc('project = PROJ ORDER BY updated DESC')}&maxResults=25" if name == "search" else ""
        lines += [f"{name}:  GET {path}", "   " + c.curl_cmd(c.url(path, qs), masked=True)]
    lines += ["", ""]
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


def cache_entry(issue: dict, vers_map: dict, comments, api_fields: list,
                aliases: dict | None = None) -> dict:
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
    # team.json custom_fields: the alias (e.g. package_info) is a window field
    for alias, spec in (aliases or {}).items():
        if spec["id"] in fetched:
            e[alias] = stringify(f.get(spec["id"]))
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
    body = c.search(jql, QUERY_FIELDS, maxn)
    if not c.dry:
        render(body, output)


def issue(c: Client, key: str, output: str) -> None:
    body = c.get(c.path("issue", key=key))
    if not c.dry:
        render({"issues": [body]}, output)


def project_versions(c: Client, proj: str) -> list:
    vs = c.get(c.path("project_versions", project=proj)) or []
    days = c.sd("versions_lookback_days")
    if days > 0:
        cutoff = time.strftime("%Y-%m-%d", time.localtime(time.time() - days * 86400))
        vs = [v for v in vs if not v.get("releaseDate") or v["releaseDate"] >= cutoff]
    return vs


def default_projects(c: Client) -> list:
    """team.json project_keys, else every project the token can see."""
    pk = c.team.get("project_keys") or []
    return list(pk) if pk else [p.get("key") for p in (c.get(c.path("projects")) or [])]


def list_fields(c: Client) -> list:
    """/field -> [{id,name,custom,mapped}] so custom fields can be mapped."""
    mapped = {v["id"]: a for a, v in jira_config.custom_field_aliases(c.team).items()}
    out = []
    for f in c.get(c.path("fields")) or []:
        out.append({"id": f.get("id"), "name": f.get("name"), "custom": bool(f.get("custom")),
                    "mapped": mapped.get(f.get("id"), "")})
    return sorted(out, key=lambda x: (not x["custom"], x["name"] or ""))


def releases(c: Client, project: str = "", projects: list | None = None) -> list:
    """Every release (version): [{project,name,released,releaseDate,description}].
    One versions call per project - the date lives on the version object."""
    if project:
        keys = [project]
    elif projects:
        keys = projects
    else:
        keys = default_projects(c)
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


def directory(c: Client, projects: list | None = None, quiet: bool = True) -> dict:
    """The pickers' lists (the weekly `directory` job): every visible project,
    the assignable users of `projects` (else team.json project_keys, else every
    project - one paginated call each), statuses, issue types, priorities and
    fields. A project the token may not read is skipped (noted in `warnings`)."""
    out: dict = {"projects": [], "users": [], "statuses": [], "issueTypes": [], "priorities": [],
                 "fields": [], "warnings": []}
    for p in c.get(c.path("projects")) or []:
        out["projects"].append({"key": p.get("key") or "", "name": p.get("name") or ""})
    keys = list(projects or c.team.get("project_keys") or []) or [p["key"] for p in out["projects"]]
    page = max(1, c.sd("max_results_users"))
    users: dict = {}
    for proj in keys:
        start = 0
        while True:
            try:
                got = c.get(c.path("assignable_users"),
                            f"project={qenc(proj)}&startAt={start}&maxResults={page}")
            except ApiError as err:
                if err.code in (401,):
                    raise
                out["warnings"].append(f"users of {proj}: {err}")
                break
            if not isinstance(got, list):
                break
            for u in got:
                # Server/DC: `name` is what JQL takes; Cloud: accountId
                uid = u.get("accountId") or u.get("name") or u.get("key")
                if not uid:
                    continue
                e = users.setdefault(uid, {"id": uid, "name": u.get("displayName") or uid,
                                           "username": u.get("name") or "",
                                           "email": u.get("emailAddress") or "",
                                           "active": u.get("active", True) is not False,
                                           "projects": []})
                if proj not in e["projects"]:
                    e["projects"].append(proj)
            start += len(got)
            if len(got) < page or start >= 50000:
                break
        if not quiet:
            print(f"jira-api: directory: {proj}: {sum(proj in u['projects'] for u in users.values())} "
                  "user(s)", file=sys.stderr)
    out["users"] = sorted(users.values(), key=lambda u: u["name"].lower())
    for key, name in (("statuses", "statuses"), ("issueTypes", "issue_types"),
                      ("priorities", "priorities")):
        try:
            vals = c.get(c.path(name)) or []
        except ApiError as err:
            out["warnings"].append(f"{name}: {err}")
            continue
        out[key] = sorted({v.get("name") for v in vals if isinstance(v, dict) and v.get("name")},
                          key=str.lower)
    try:
        flds = c.get(c.path("fields")) or []
    except ApiError as err:
        flds = []
        out["warnings"].append(f"fields: {err}")
    out["fields"] = sorted(({"id": f.get("id"), "name": f.get("name") or f.get("id"),
                             "custom": bool(f.get("custom")),
                             "type": ((f.get("schema") or {}).get("type") or "")}
                            for f in flds if isinstance(f, dict) and f.get("id")),
                           key=lambda f: (not f["custom"], (f["name"] or "").lower()))
    out["fetchedAt"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    out["forProjects"] = keys
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
         fetch_comments=True, snapshot_keep=30, quiet=False, debug=False,
         default_projects=True, page_size=None, max_total=None) -> dict:
    """Load issues updated within `window` (optionally only `projects` / a
    custom `jql`), merge them into the cache, return stats + matched keys."""
    fields = api_fields or jira_config.api_fields(team=c.team)
    aliases = jira_config.custom_field_aliases(c.team)
    since = window_since(window)
    clauses = []
    if jql:
        clauses.append(f"({jql})")
    if default_projects:   # an explicit JQL may already scope its projects
        projects = projects or c.team.get("project_keys") or None
    if projects:
        clauses.append("project in (" + ", ".join(f'"{p}"' for p in projects) + ")")
    clauses.append(f'updated >= "{since}"' if since else "project != null")
    q = " AND ".join(clauses)
    if debug:
        print(f"jira-api: JQL: {q}", file=sys.stderr)
    raw = c.search(q, ",".join(fields), max_total=max_total, page_size=page_size)["issues"]
    if c.dry:
        return {"fetched": 0, "changed": 0, "total": 0, "keys": [], "snapshot": "", "jql": q}
    # version name -> {released, date} per project: the release date + flag
    # live on the VERSION object, not the issue's fixVersions
    vers_map = read_json(VERSIONS_FILE, {})
    if not isinstance(vers_map, dict):
        vers_map = {}
    if "fixVersions" in fields:
        ttl = c.sd("cache_timeout_seconds")
        fresh = ttl > 0 and os.path.exists(VERSIONS_FILE) and \
            time.time() - os.path.getmtime(VERSIONS_FILE) < ttl
        for proj in sorted({((i.get("fields") or {}).get("project") or {}).get("key", "")
                            for i in raw} - {""}):
            if fresh and proj in vers_map:
                continue
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
            body = c.get(c.path("issue", key=i.get("key")), "fields=comment") or {}
            comments = [{"author": alt((cm.get("author") or {}).get("displayName"), ""),
                         "body": alt(cm.get("body"), ""),
                         "created": alt(cm.get("created"), ""),
                         "updated": alt(cm.get("updated"), "")}
                        for cm in ((body.get("fields") or {}).get("comment") or {}).get("comments") or []]
        entries[i.get("key")] = cache_entry(i, vers_map, comments, fields, aliases)
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

    def quiet_get(name, qs="", **params):
        try:
            return c.get(c.path(name, **params), qs) or []
        except (ApiError, jira_config.ConfigError):
            return []

    projs = [f"{p.get('key')}|{p.get('name')}" for p in quiet_get("projects")]
    pk = c.team.get("project_keys") or []
    if pk:
        projs = [p for p in projs if p.split("|", 1)[0] in pk] or [f"{k}|team.json" for k in pk]
    while True:
        p = menu("Project", f.project or default_project, projs)
        if not p or not projs or any(p == o.split("|", 1)[0] for o in projs):
            break
        print(f'Project "{p}" not found on {c.site} - choose a listed key or 0 for any', file=sys.stderr)
    f.project = p
    if p:
        users = ["me|currentUser()"] + [
            f"{alt(u.get('name'), u.get('displayName'))}|{u.get('displayName')}"
            for u in quiet_get("assignable_users",
                               f"project={qenc(p)}&maxResults={c.sd('max_results_users')}")]
        a = menu("Assignee", f.assignee, users)
        f.assignee = "currentUser()" if a == "me" else a
        vers = sorted({v.get("name") for v in quiet_get("project_versions", project=p)})
        f.release = menu("Release (fixVersion)", f.release, vers)
    sts = sorted({s.get("name") for s in quiet_get("statuses")})
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
    site = ask("Jira site URL (e.g. https://jira.example.com): ").strip()
    if not site:
        die("site URL required")
    auth = ask("Auth - bearer (Server/Data Center personal access token) or "
               "basic (Cloud email + API token) [bearer]: ").strip().lower() or "bearer"
    if auth not in jira_config.AUTH_MODES:
        die(f"auth must be one of {'/'.join(jira_config.AUTH_MODES)}")
    email = ""
    if auth == "basic":
        email = ask("Email: ").strip()
        if not email:
            die("email required for basic auth")
    import getpass
    token = getpass.getpass("Personal access token: " if auth == "bearer" else
                            "API token (https://id.atlassian.com/manage-profile/security/api-tokens): ").strip()
    if not token:
        die("token required")
    proj = ask("Default project key (Enter for none): ").strip()
    mx = ask("Default max results [25]: ").strip() or "25"
    jira_config.save({"site": site, "auth": auth, "email": email, "token": token,
                      "defaultProject": proj, "defaultMax": int(mx) if mx.isdigit() else 25})
    print(f"Wrote {path} (chmod 600). Test with: {sys.argv[0]} --myself   "
          f"(print the curl: {sys.argv[0]} --myself --curl)")


# ------------------------------------------------------------ main

def parse_args(argv: list):
    o = {"init": False, "myself": False, "site": "", "email": "", "token": "", "auth": "",
         "token_stdin": False, "max": "", "output": "table", "debug": False,
         "verbose": False, "no_auth": False, "sync": "", "releases": False,
         "interactive": False, "issue": "", "curl": False, "mask": False, "job": "",
         "args": {}, "jobs": False, "board": "", "list_fields": False}
    f = Filters()
    valued = {"-p": "project", "--project": "project", "-a": "assignee", "--assignee": "assignee",
              "-r": "release", "--release": "release", "-s": "status", "--status": "status",
              "-u": "reporter", "--reporter": "reporter", "-t": "text", "--text": "text",
              "-d": "date", "--date": "date", "-j": "jql", "--jql": "jql",
              "-S": "sort", "--sort": "sort"}
    opt_valued = {"--site": "site", "--email": "email", "--token": "token", "--auth": "auth",
                  "--job": "job", "--template": "job", "--board": "board", "-n": "max",
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
        elif name == "--curl":
            o["curl"] = True
        elif name == "--mask":
            o["mask"] = True
        elif name == "--jobs":
            o["jobs"] = True
        elif name == "--list-fields":
            o["list_fields"] = True
        elif name == "--arg":
            k, sep, v = val().partition("=")
            if not sep or not k:
                die("--arg wants NAME=VALUE")
            o["args"][k.strip()] = v
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
    if o["auth"] and o["auth"] not in jira_config.AUTH_MODES:
        die(f"--auth must be one of {'/'.join(jira_config.AUTH_MODES)}")
    if o["max"] and not o["max"].isdigit():
        die("--max must be a number")
    return o, f


def list_jobs(team: dict) -> None:
    """--jobs: team.json jobs (with their args) and every JQL template."""
    for j in team.get("jobs") or []:
        args = ", ".join(f"{a.get('name')}" + (f" ({a['label']})" if a.get("label") else "")
                         for a in j.get("args") or [] if isinstance(a, dict))
        print(f"job  {j.get('key')}: {j.get('name') or ''}" + (f"  args: {args}" if args else ""))
    for k, t in (team.get("jql_templates") or {}).items():
        need = sorted(set(jira_config.PLACEHOLDER_RE.findall(t)) - {"projects"})
        print(f"jql  {k}: {t}" + (f"  args: {', '.join(need)}" if need else ""))
    print(f"project_keys: {', '.join(team.get('project_keys') or []) or '(none - all projects)'}")
    print(f"team config: {team.get('path')}")


def board_id(team: dict, ref: str) -> str:
    for b in team.get("boards") or []:
        if isinstance(b, dict) and ref in (str(b.get("id")), b.get("name")):
            return str(b.get("id"))
    if ref.isdigit():
        return ref
    die(f"unknown board '{ref}' (team.json boards: id or name)")


def main(argv: list) -> int:
    o, f = parse_args(argv)
    if o["init"]:
        do_init()
        return 0
    if o["token_stdin"]:
        o["token"] = sys.stdin.readline().strip()
    try:
        cfg = jira_config.load(o["site"], o["email"], o["token"])
        if o["auth"]:
            cfg.data["auth"] = o["auth"]
        team = jira_config.load_team(cfg.data)
    except jira_config.ConfigError as err:
        die(str(err), 2)
    if o["jobs"]:
        list_jobs(team)
        return 0
    need = ["site", "token"] + (["email"] if cfg.auth == "basic" else [])
    missing = [f"JIRA_{k.upper()}" for k in need if not cfg[k]]
    if missing and not (o["curl"] and o["mask"] and missing == ["JIRA_TOKEN"]):
        die(f"missing config: {' '.join(missing)} ({cfg.auth} auth) - run 'jira_api.py --init', "
            "the setup sheet, or export them", 2)
    for n in cfg.notes:
        print(f"jira-api: {n}", file=sys.stderr)
    c = Client(cfg.site, cfg["token"], email=cfg["email"] or "", auth=cfg.auth, team=team,
               debug=o["debug"], verbose=o["verbose"], dry=o["curl"], mask=o["mask"])
    maxn = int(o["max"] or cfg["defaultMax"] or 25)
    try:
        if o["verbose"]:
            trace(f"--- {time.strftime('%Y-%m-%d %H:%M:%S')} jira_api.py {' '.join(argv)}")
            print(f"jira-api: trace appended to {TRACE_FILE}", file=sys.stderr)
        if not o["no_auth"] and not o["myself"] and not c.dry:
            me = c.get(c.path("myself")) or {}
            print(f"jira-api: login OK ({me.get('displayName') or 'unknown user'})", file=sys.stderr)
        if o["verbose"]:
            show_curl_ref(c)
            nothing = not any([f.project, f.assignee, f.release, f.status, f.reporter, f.text,
                               f.date, f.jql, o["issue"], o["sync"], o["interactive"],
                               f.recent, o["myself"], o["releases"], o["job"], o["board"],
                               o["list_fields"]])
            if nothing:
                return 0
        if o["myself"]:
            me = c.get(c.path("myself"))
            if not c.dry:
                print(json.dumps(me, indent=2, ensure_ascii=False))
            return 0
        if o["list_fields"]:
            fl = list_fields(c)
            if not c.dry:
                print(json.dumps(fl, indent=2, ensure_ascii=False))
            return 0
        if o["releases"]:
            rel = releases(c, f.project)
            if not c.dry:
                print(json.dumps(rel, indent=2, ensure_ascii=False))
            return 0
        if o["sync"]:
            sync(c, o["sync"], fetch_comments=bool(cfg["fetchComments"]),
                 snapshot_keep=int(cfg.get("snapshotKeep", 30)), debug=o["debug"])
            return 0
        if o["issue"]:
            issue(c, o["issue"], o["output"])
            return 0
        if o["job"]:
            if f.project and "projects" not in o["args"]:
                o["args"]["projects"] = [p.strip() for p in f.project.split(",") if p.strip()]
            f.jql = jira_config.job_jql(team, o["job"], o["args"])
        if o["board"]:
            jql = f.jql or build_jql(f)
            if o["debug"]:
                print(f"jira-api: board JQL: {jql or '<none>'}", file=sys.stderr)
            body = c.board_issues(board_id(team, o["board"]), jql, QUERY_FIELDS, maxn)
            if not c.dry:
                render(body, o["output"])
            return 0
        if (o["interactive"] or not argv) and not c.dry:
            interactive(c, f, cfg["defaultProject"], maxn, o["output"], o["debug"])
            return 0
        if c.dry and not (f.jql or build_jql(f)):
            c.get(c.path("myself"))   # bare --curl = the connectivity test
            return 0
        run_query(c, f, maxn, o["output"], o["debug"])
    except jira_config.ConfigError as err:
        die(str(err), 2)
    except ApiError as err:
        if err.curl:
            # the exact failing request, runnable (the error line stays LAST:
            # the menu bar / setup sheet show the last stderr line)
            print(f"jira-api: reproduce: export JIRA_TOKEN=<token>; {err.curl}", file=sys.stderr)
            print(f"jira-api: (real command with token: {CURL_LOG}, or add --curl to print it)",
                  file=sys.stderr)
        die(str(err))
    except KeyboardInterrupt:
        return 130
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
