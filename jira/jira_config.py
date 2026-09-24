#!/usr/bin/env python3
"""jira_config.py - load / validate / migrate the jira poller config.

Single source of truth for everything the python poller reads:

  ~/.config/jira/config.json     site, auth, token, poll endpoints (chmod 600)
  ~/.config/jira/team.json       team schema - custom_fields, field_mappings,
                                 project_keys, jobs, api_endpoints, boards,
                                 search_defaults, jql_templates (no secrets,
                                 shareable; example: jira/team.example.json)
  commands.conf [jira]           enabled (THE SWITCH) + columns (render
                                 fields AND the API fields= param)

The legacy env-style ~/.config/jira/config (JIRA_SITE=... lines, written by
the old jira-api.sh --init) is migrated to config.json on first load; the old
file is left untouched.

Auth: "auth" = "bearer" (Server / Data Center personal access token, sent as
`Authorization: Bearer <token>`, no email) or "basic" (Cloud: email + API
token). Unset -> basic when an email is configured, else bearer.

Credential precedence: --site/--email/--token flags > config.json value >
exported JIRA_SITE / JIRA_EMAIL / JIRA_TOKEN (env only fills EMPTY config
values, and every such fill is recorded as a note the status surface shows).

CLI (used by the menu bar + setup sheet):
  jira_config.py --path            print the config.json path
  jira_config.py --check           validate; JSON {ok, problems, notes, ...}
  jira_config.py --show            print config.json with the token masked
  jira_config.py --fields          print the API fields= list from [jira]
  jira_config.py --team            print the resolved team.json (defaults merged)
  jira_config.py --save            merge a JSON object from stdin into
                                   config.json (site/email/token/...); the
                                   token travels on stdin, never argv
  jira_config.py --set-window NAME WINDOW     change one endpoint's schedule
  jira_config.py --set-enabled NAME true|false
  jira_config.py --upsert-endpoint   JSON object on stdin: add / replace one
                                     poll job (validated; JSON result)
  jira_config.py --upsert-search     same for a saved search
  jira_config.py --delete-endpoint NAME | --delete-search NAME
  jira_config.py --set-columns endpoint|search NAME SPEC
                                     one job's columns (the jira window's
                                     header drags save through this)

Poll jobs ("endpoints") and searches each own their columns (one-line
`field:Title:width:align:flags, ...`). [jira] columns in commands.conf is only
the starter template for new ones (and the fallback for a tab no job owns);
jobs created before per-job columns get a copy of it once, on load.
Stdlib only (python 3.9+: launchd may run the CLT /usr/bin/python3).
"""
from __future__ import annotations

import json
import os
import re
import shlex
import sys
import tempfile

HOME = os.path.expanduser("~")
JIRA_DIR = os.path.dirname(os.path.abspath(__file__))
WS_ROOT = os.path.dirname(JIRA_DIR)

CONFIG_JSON = os.environ.get("JIRA_CONFIG_JSON") or os.path.join(HOME, ".config/jira/config.json")
TEAM_JSON = os.environ.get("JIRA_TEAM_JSON") or os.path.join(HOME, ".config/jira/team.json")
LEGACY_CONFIG = os.environ.get("JIRA_CONFIG_FILE") or os.path.join(HOME, ".config/jira/config")
COMMANDS_CONF = os.environ.get("WS_COMMANDS_CONF") or os.path.join(WS_ROOT, "commands.conf")

CACHE_DIR = os.environ.get("JIRA_CACHE_DIR") or os.path.join(HOME, ".cache/jira")
OUT_DIR_DEFAULT = os.path.join(HOME, ".cache/workspace-switcher/jira_json")

DEFAULTS = {
    "site": "",
    "auth": "",
    "email": "",
    "token": "",
    "defaultProject": "",
    "defaultMax": 25,
    "pollMarginMinutes": 5,
    "lockStaleMinutes": 10,
    "fetchComments": True,
    "snapshotKeep": 30,
    "outDir": OUT_DIR_DEFAULT,
    "endpoints": [],
    "searches": [],
}

DEFAULT_ENDPOINTS = [
    {"name": "all", "window": "10m", "projects": "*", "type": "issues",
     "file": "all.json", "enabled": True},
    {"name": "releases", "window": "1h", "projects": "*", "type": "releases",
     "file": "releases.json", "enabled": True},
]

ENDPOINT_TYPES = ("issues", "releases")
WINDOW_RE = re.compile(r"^(\d+)([smhdw])$")
WINDOW_UNITS = {"s": 1, "m": 60, "h": 3600, "d": 86400, "w": 604800}

# window json field -> the Jira REST field(s) it is derived from. A column /
# [jira] field that is NOT listed here is passed through verbatim as a raw
# Jira field (e.g. created, duedate, customfield_10010) and stringified.
FIELD_SOURCES = {
    "key": [],
    "title": ["summary"],
    "status": ["status"],
    "assignee": ["assignee"],
    "release": ["fixVersions"],
    "releaseLabel": ["fixVersions"],
    "releaseDate": ["fixVersions"],
    "releaseStatus": ["fixVersions"],
    "priority": ["priority"],
    "labels": ["labels"],
    "description": ["description"],
    "updated": ["updated"],
    "reporter": ["reporter"],
    "project": ["project"],
    "comments": [],   # separate per-issue call (fetchComments)
}
# always requested: updated drives sort + windows, project drives the
# per-project files and the versions (release date) lookup
ALWAYS_API_FIELDS = ["updated", "project"]
# the legacy window-json shape (kept identical so copy-fields / checkbox /
# detail window keep working); column fields are ADDED on top
BASE_WINDOW_KEYS = ["key", "title", "status", "assignee", "release", "releaseLabel",
                    "releaseDate", "releaseStatus", "priority", "labels",
                    "description", "reporter", "project"]
# [jira] keys whose field names feed the API request
FIELD_KEYS = ("primary", "content", "detail", "trailing", "body", "filter",
              "filters", "copy-fields")


AUTH_MODES = ("bearer", "basic")

# ------------------------------------------------------------ team.json
# Every Jira REST path the tools call. Relative paths hang off /rest/api/2;
# "board*" paths off /rest/agile/1.0; paths starting /rest/ are used as-is.
# {name} placeholders are filled per request (url-encoded).
DEFAULT_API_ENDPOINTS = {
    "myself": "/myself",
    "search": "/search",
    "issue": "/issue/{key}",
    "projects": "/project",
    "project_versions": "/project/{project}/versions",
    "statuses": "/status",
    "fields": "/field",
    "assignable_users": "/user/assignable/search",
    "board_issues": "/board/{board_id}/issue",
}
API_BASE = "/rest/api/2"
AGILE_BASE = "/rest/agile/1.0"
CLOUD_SEARCH = "/rest/api/3/search/jql"   # Cloud removed v2 /search

DEFAULT_SEARCH = {
    "max_results_users": 50,
    "max_results_search": 50,     # page size of one search request
    "page_size": 50,              # page size of one board request
    "timeout_seconds": 30,        # curl -m
    "cache_timeout_seconds": 0,   # re-use versions.json this long (0 = always refetch)
    "versions_lookback_days": 0,  # drop releases dated older than this (0 = keep all)
}

# {projects} = the project list ("A", "B"); every other {name} is a job /
# --arg value (quotes inside values are escaped for JQL).
DEFAULT_JQL_TEMPLATES = {
    "partial_search": 'project in ({projects}) AND (summary ~ "{query}" OR description ~ "{query}") '
                      "ORDER BY updated DESC",
    "users_search": "project in ({projects}) ORDER BY updated ASC",
    "assignee_search": 'project in ({projects}) AND assignee = "{username}" ORDER BY updated DESC',
    "reporter_search": 'project in ({projects}) AND reporter = "{username}" ORDER BY updated DESC',
    "assignee_reporter_search": 'project in ({projects}) AND assignee = "{assignee}" '
                                'AND reporter = "{reporter}" ORDER BY updated DESC',
    "release_search": 'project = "{project}" AND fixVersion = "{version}"',
    "release_search_all": 'project = "{project}" AND fixVersion = "{version}" ORDER BY created ASC',
}

TEAM_KEYS = ("custom_fields", "field_mappings", "project_keys", "jobs", "api_endpoints",
             "boards", "search_defaults", "jql_templates")
CUSTOMFIELD_RE = re.compile(r"^customfield[ _-]*(\d+)$", re.I)
PLACEHOLDER_RE = re.compile(r"\{(\w+)\}")


class ConfigError(Exception):
    pass


def norm_key(k: str) -> str:
    """'Field ID' / 'field-id' / 'Field_id' -> 'field_id'; 'customfield 15262'
    -> 'customfield_15262' (hand-written team files are forgiven)."""
    k = str(k).strip()
    m = CUSTOMFIELD_RE.match(k)
    if m:
        return f"customfield_{m.group(1)}"
    return re.sub(r"[\s-]+", "_", k).lower()


def norm_field_id(v) -> str:
    v = str(v or "").strip()
    m = CUSTOMFIELD_RE.match(v)
    return f"customfield_{m.group(1)}" if m else v


def _norm_keys(obj):
    """Normalize every dict key of the team schema (see norm_key)."""
    if isinstance(obj, dict):
        return {norm_key(k): _norm_keys(v) for k, v in obj.items()}
    if isinstance(obj, list):
        return [_norm_keys(x) for x in obj]
    return obj


def load_team(cfg_data: dict | None = None) -> dict:
    """team.json merged over the defaults; the same keys inside config.json
    win (a single-file setup works too). Raises ConfigError on bad JSON."""
    team = {k: {} for k in TEAM_KEYS}
    team.update({"project_keys": [], "jobs": [], "boards": [],
                 "api_endpoints": dict(DEFAULT_API_ENDPOINTS),
                 "search_defaults": dict(DEFAULT_SEARCH),
                 "jql_templates": dict(DEFAULT_JQL_TEMPLATES)})
    layers = []
    path = (cfg_data or {}).get("teamConfig") or TEAM_JSON
    path = os.path.expanduser(path)
    if os.path.exists(path):
        try:
            with open(path, encoding="utf-8") as fh:
                t = json.load(fh)
            if not isinstance(t, dict):
                raise ValueError("top level is not an object")
        except (OSError, ValueError) as err:
            raise ConfigError(f"{path}: invalid JSON ({err})")
        layers.append(t)
    if cfg_data:
        layers.append({k: v for k, v in cfg_data.items() if norm_key(k) in TEAM_KEYS})
    explicit_api: list = []
    for layer in layers:
        layer = _norm_keys(layer)
        if isinstance(layer.get("api_endpoints"), dict):
            explicit_api += list(layer["api_endpoints"])
        for k in TEAM_KEYS:
            if k not in layer or layer[k] is None:
                continue
            v = layer[k]
            if isinstance(team[k], dict) and isinstance(v, dict):
                team[k].update(v)
            else:
                team[k] = v
    team["path"] = path
    team["_explicit_api"] = explicit_api
    return team


def custom_field_aliases(team: dict) -> dict:
    """alias -> {id, label, description} from custom_fields, plus every
    field_mappings id (alias = the id itself)."""
    out = {}
    for alias, spec in (team.get("custom_fields") or {}).items():
        if isinstance(spec, dict):
            fid = norm_field_id(spec.get("field_id") or spec.get("id"))
            if fid:
                out[alias] = {"id": fid, "label": spec.get("label") or alias,
                              "description": spec.get("description") or ""}
        elif isinstance(spec, str) and spec:
            out[alias] = {"id": norm_field_id(spec), "label": alias, "description": ""}
    for fid, label in (team.get("field_mappings") or {}).items():
        fid = norm_field_id(fid)
        if fid and fid not in out:
            out[fid] = {"id": fid, "label": str(label), "description": ""}
    return out


def team_problems(team: dict) -> list:
    p = []
    for alias, spec in (team.get("custom_fields") or {}).items():
        if isinstance(spec, dict) and not (spec.get("field_id") or spec.get("id")):
            p.append(f"custom_fields.{alias}: field_id missing")
    for fid in (team.get("field_mappings") or {}):
        if not norm_field_id(fid).startswith("customfield_"):
            p.append(f"field_mappings: '{fid}' is not a customfield_NNNNN id")
    pk = team.get("project_keys")
    if not (isinstance(pk, list) and all(isinstance(x, str) for x in pk)):
        p.append("project_keys must be a list of project keys")
    for i, j in enumerate(team.get("jobs") or []):
        if not isinstance(j, dict) or not j.get("key"):
            p.append(f"jobs[{i}]: key missing")
            continue
        if not (j.get("jql") or j.get("template") or j["key"] in (team.get("jql_templates") or {})):
            p.append(f"job '{j['key']}': no jql, template, or jql_templates.{j['key']}")
    for b in team.get("boards") or []:
        if not isinstance(b, dict) or not b.get("id"):
            p.append(f"boards: entry without id: {b}")
    return p


def jql_quote(v) -> str:
    return str(v).replace("\\", "\\\\").replace('"', '\\"')


def render_jql(template: str, args: dict, team: dict) -> str:
    """Fill {placeholders}. {projects} defaults to project_keys (quoted,
    comma-joined); a missing value raises ConfigError naming it."""
    args = dict(args or {})
    if "projects" not in args:
        pk = team.get("project_keys") or []
        if pk:
            args["projects"] = ", ".join(f'"{jql_quote(p)}"' for p in pk)
    elif isinstance(args["projects"], (list, tuple)):
        args["projects"] = ", ".join(f'"{jql_quote(p)}"' for p in args["projects"])
    missing = [n for n in PLACEHOLDER_RE.findall(template) if n not in args]
    if missing:
        hint = " (set project_keys in team.json)" if "projects" in missing else ""
        raise ConfigError(f"JQL template needs: {', '.join(dict.fromkeys(missing))}{hint}")

    def sub(m):
        v = args[m.group(1)]
        return v if m.group(1) == "projects" else jql_quote(v)
    return PLACEHOLDER_RE.sub(sub, template)


def find_job(team: dict, key: str) -> dict | None:
    for j in team.get("jobs") or []:
        if isinstance(j, dict) and j.get("key") == key:
            return j
    return None


def job_jql(team: dict, key: str, args: dict) -> str:
    """A job's JQL: its own `jql`, else jql_templates[job.template or key].
    Plain template names work as jobs too."""
    job = find_job(team, key) or {}
    tmpl = job.get("jql") or (team.get("jql_templates") or {}).get(job.get("template") or key)
    if not tmpl:
        raise ConfigError(f"unknown job/template '{key}' (see --jobs)")
    merged = {}
    for a in job.get("args") or []:
        if isinstance(a, dict) and a.get("name") and "default" in a:
            merged[a["name"]] = a["default"]
    merged.update(job.get("values") or {})
    merged.update(args or {})
    return render_jql(tmpl, merged, team)


def endpoint_jql(ep: dict, team: dict) -> str:
    """A poll endpoint's extra JQL: `jql`, or `job` / `template` + `args`."""
    if ep.get("jql"):
        return ep["jql"]
    key = ep.get("job") or ep.get("template")
    if not key:
        return ""
    args = dict(ep.get("args") or {})
    if "projects" not in args and isinstance(ep.get("projects"), list) and ep["projects"]:
        args["projects"] = ep["projects"]     # the endpoint's own project list fills {projects}
    jql = job_jql(team, key, args)
    # sync() ANDs this with its own window clause - an ORDER BY can't be nested
    return re.sub(r"\s+ORDER\s+BY\s+.*$", "", jql, flags=re.I | re.S)


def resolve_path(team: dict, name: str, site: str = "", **params) -> str:
    """api_endpoints[name] with {params} url-encoded, as a /rest/... path."""
    import urllib.parse
    eps = team.get("api_endpoints") or DEFAULT_API_ENDPOINTS
    raw = eps.get(name) or DEFAULT_API_ENDPOINTS.get(name)
    if not raw:
        raise ConfigError(f"api_endpoints.{name} not defined")
    if name == "search" and "search" not in (team.get("_explicit_api") or ()) \
            and raw == DEFAULT_API_ENDPOINTS["search"] and ".atlassian.net" in site:
        raw = CLOUD_SEARCH
    missing = [n for n in PLACEHOLDER_RE.findall(raw) if n not in params]
    if missing:
        raise ConfigError(f"api_endpoints.{name} ({raw}) needs: {', '.join(missing)}")
    path = PLACEHOLDER_RE.sub(lambda m: urllib.parse.quote(str(params[m.group(1)]), safe=""), raw)
    if path.startswith(("http://", "https://", "/rest/")):
        return path
    if not path.startswith("/"):
        path = "/" + path
    return (AGILE_BASE if name.startswith("board") else API_BASE) + path


# ------------------------------------------------------------ commands.conf

def read_section(name: str, path: str | None = None) -> dict:
    """[name] of commands.conf as {key: value} (same line rules as the app:
    trimmed, '#' comments skipped, first '=' splits)."""
    path = path or COMMANDS_CONF
    out: dict = {}
    try:
        with open(path, encoding="utf-8") as fh:
            lines = fh.read().split("\n")
    except OSError:
        return out
    cur = None
    for raw in lines:
        s = raw.strip()
        if not s or s.startswith("#"):
            continue
        if s.startswith("[") and s.endswith("]"):
            cur = s[1:-1].strip()
            continue
        if cur != name or "=" not in s:
            continue
        k, v = s.split("=", 1)
        out[k.strip()] = v.strip()
    return out


def truthy(v) -> bool:
    return str(v).strip().lower() in ("true", "yes", "1", "on")


def jira_enabled(path: str | None = None) -> bool:
    return truthy(read_section("jira", path).get("enabled", "false"))


def poll_active(path: str | None = None) -> bool:
    """The poller runs when jira is enabled, or when the user chose to keep
    polling in the background after disabling it (`poll-when-disabled`)."""
    sec = read_section("jira", path)
    return truthy(sec.get("enabled", "false")) or truthy(sec.get("poll-when-disabled", "false"))


def parse_columns(spec: str) -> list:
    """`field:Title:width:align:flags, ...` -> [{field,title,width,align,
    sortable,filterable}]. flags: filter / sort, joined with + (filter+sort)
    or as extra :segments. Missing parts default (title = field, width 0 =
    share the leftover width, align left)."""
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
        cols.append({"field": field, "title": title, "width": width, "align": align,
                     "sortable": "sort" in flags, "filterable": "filter" in flags})
    return cols


def window_fields(section: dict | None = None, columns: str | None = None) -> list:
    """Every window-json field the [jira] section references, columns first
    (ordered, de-duplicated). Empty section -> the legacy base keys.
    `columns` = one job's own columns spec (replaces [jira] columns)."""
    sec = dict(read_section("jira") if section is None else section)
    if columns is not None:
        sec["columns"] = columns
    seen: list = []

    def add(f):
        f = f.strip()
        if f and f not in seen:
            seen.append(f)

    for c in parse_columns(sec.get("columns", "")):
        add(c["field"])
    for k in FIELD_KEYS:
        for f in (sec.get(k) or "").split(","):
            add(f)
    return seen or list(BASE_WINDOW_KEYS)


def api_fields(section: dict | None = None, team: dict | None = None,
               columns: str | None = None) -> list:
    """The Jira fields= list: sources of every referenced window field plus
    the always-needed ones. THE coupling between [jira] columns and the API.
    A team.json custom_fields alias (e.g. package_info) maps to its id."""
    aliases = custom_field_aliases(team or {})
    out: list = []
    for f in window_fields(section, columns) + ALWAYS_API_FIELDS:
        srcs = FIELD_SOURCES.get(f) if f in FIELD_SOURCES else [aliases[f]["id"] if f in aliases else f]
        for src in srcs:
            if src not in out:
                out.append(src)
    return out


def publish_keys(section: dict | None = None, columns: str | None = None) -> list:
    """Keys written to each window json: the legacy base shape plus any
    extra referenced field (comments stay cache-only - not a string)."""
    keys = list(BASE_WINDOW_KEYS)
    for f in window_fields(section, columns):
        if f not in keys and f != "comments":
            keys.append(f)
    return keys


def job_columns(job: dict, section: dict | None = None) -> str:
    """A poll job's / search's own columns spec; [jira] columns when unset."""
    spec = (job or {}).get("columns")
    if isinstance(spec, str) and spec.strip():
        return spec.strip()
    sec = read_section("jira") if section is None else section
    return sec.get("columns", "")


def columns_problem(spec: str) -> str:
    """'' when a columns spec is valid (same rules as the app's
    configValueProblem for `columns`), else the reason."""
    if "\n" in spec:
        return "columns must stay on one line"
    if not spec.strip():
        return "at least one column is required"
    for part in spec.split(","):
        seg = [x.strip() for x in part.split(":")]
        if not seg[0]:
            return "an entry has no field name"
        if len(seg) > 2 and seg[2]:
            try:
                w = float(seg[2])
            except ValueError:
                return f"'{seg[0]}' width '{seg[2]}' is not a number (percent)"
            if not 0 <= w <= 100:
                return f"'{seg[0]}' width {seg[2]} must be 0-100"
        if len(seg) > 3 and seg[3] and seg[3].lower() not in ("left", "right", "center"):
            return f"'{seg[0]}' align '{seg[3]}' is not left | right | center"
    return ""


NAME_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.-]{0,39}$")

# saved searches: kind -> label + the argument the user fills. Run on demand
# (jira_poll.py --search NAME), published as search-<name>.json = a tab.
SEARCH_KINDS = {
    "reporter": {"label": "Reporter", "arg": "username", "argLabel": "Reporter (username)"},
    "assignee": {"label": "Assignee", "arg": "username", "argLabel": "Assignee (username)"},
    "project": {"label": "Project", "arg": "project", "argLabel": "Project key"},
    "text": {"label": "Text in summary / description", "arg": "query", "argLabel": "Search text"},
    "jql": {"label": "Custom JQL", "arg": "jql", "argLabel": "JQL"},
    "job": {"label": "team.json job / template", "arg": "job", "argLabel": "Job key"},
}


def search_jql(search: dict, team: dict) -> str:
    """The JQL a saved search runs (no ORDER BY: sync ANDs it with its own
    clause). projects = the search's list, else team.json project_keys."""
    kind = search.get("kind", "text")
    args = dict(search.get("args") or {})
    projects = search.get("projects")
    if not (isinstance(projects, list) and projects):
        projects = team.get("project_keys") or []
    if kind == "jql":
        q = (args.get("jql") or search.get("jql") or "").strip()
        if not q:
            raise ConfigError("custom JQL is empty")
        return re.sub(r"\s+ORDER\s+BY\s+.*$", "", q, flags=re.I | re.S)
    if kind == "job":
        ep = {"job": args.get("job") or search.get("job"), "args": {k: v for k, v in args.items()
                                                                    if k != "job"}}
        if projects:
            ep["projects"] = projects
        if not ep["job"]:
            raise ConfigError("no team.json job chosen")
        return endpoint_jql(ep, team)
    spec = SEARCH_KINDS.get(kind)
    if not spec:
        raise ConfigError(f"unknown search kind '{kind}' ({', '.join(SEARCH_KINDS)})")
    v = str(args.get(spec["arg"], "")).strip()
    if not v:
        raise ConfigError(f"{spec['argLabel']} is empty")
    q = jql_quote(v)
    clause = {"reporter": f'reporter = "{q}"', "assignee": f'assignee = "{q}"',
              "project": f'project = "{q}"',
              "text": f'(summary ~ "{q}" OR description ~ "{q}")'}[kind]
    if projects and kind != "project":
        clause = "project in (" + ", ".join(f'"{jql_quote(p)}"' for p in projects) + ") AND " + clause
    return clause


# ------------------------------------------------------------ config.json

def parse_window(w: str) -> int:
    """'10m' -> 600 seconds. Raises ConfigError."""
    m = WINDOW_RE.match(str(w).strip())
    if not m:
        raise ConfigError(f"invalid window '{w}' (use 90s, 10m, 1h, 1d, 1w)")
    n = int(m.group(1)) * WINDOW_UNITS[m.group(2)]
    if n < 60:
        raise ConfigError(f"window '{w}' is shorter than the 60s launchd tick")
    return n


def parse_legacy(path: str) -> dict:
    """The old env-style config (KEY='value' lines) as a dict."""
    out = {}
    with open(path, encoding="utf-8") as fh:
        for line in fh:
            line = line.strip()
            if not line or line.startswith("#") or "=" not in line:
                continue
            k, v = line.split("=", 1)
            k = k.strip()
            if k.startswith("export "):
                k = k[7:].strip()
            try:
                parts = shlex.split(v, comments=True)
                v = parts[0] if parts else ""
            except ValueError:
                v = v.strip().strip("'\"")
            out[k] = v
    return out


def _cache_projects() -> list:
    try:
        with open(os.path.join(CACHE_DIR, "jiras.json"), encoding="utf-8") as fh:
            data = json.load(fh)
        return sorted({v.get("project", "") for v in data.values() if v.get("project")})
    except (OSError, ValueError, AttributeError):
        return []


def migrate_legacy(legacy: dict) -> dict:
    """config.json content from the legacy env config. Per-project endpoints
    mirror what the old agent published (--projects all -> one file per
    project found in the cache; JIRA_POLL_PROJECTS when it lists keys)."""
    def num(k, d):
        try:
            return int(legacy.get(k) or d)
        except ValueError:
            return d

    cfg = dict(DEFAULTS)
    cfg.update({
        "site": legacy.get("JIRA_SITE", ""),
        "email": legacy.get("JIRA_EMAIL", ""),
        "token": legacy.get("JIRA_TOKEN", ""),
        "defaultProject": legacy.get("JIRA_DEFAULT_PROJECT", ""),
        "defaultMax": num("JIRA_MAX", 25),
        "pollMarginMinutes": num("JIRA_POLL_MARGIN", 5),
        "outDir": legacy.get("JIRA_POLL_OUT_DIR") or OUT_DIR_DEFAULT,
    })
    projs_spec = (legacy.get("JIRA_POLL_PROJECTS") or "").strip()
    if projs_spec and projs_spec != "all":
        projs = [p.strip() for p in projs_spec.split(",") if p.strip()]
    else:
        projs = _cache_projects()
    eps = [dict(DEFAULT_ENDPOINTS[0])]
    for p in projs:
        eps.append({"name": p, "window": "30m", "projects": [p], "type": "issues",
                    "file": f"{p}.json", "enabled": True})
    eps.append(dict(DEFAULT_ENDPOINTS[1]))
    cfg["endpoints"] = eps
    return cfg


def write_json_600(path: str, obj) -> None:
    """Atomic write (temp + rename), chmod 600 before the rename."""
    d = os.path.dirname(path)
    os.makedirs(d, exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=d, prefix="." + os.path.basename(path) + ".")
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as fh:
            json.dump(obj, fh, indent=2)
            fh.write("\n")
        os.chmod(tmp, 0o600)
        os.replace(tmp, path)
    except BaseException:
        try:
            os.unlink(tmp)
        except OSError:
            pass
        raise


class Config:
    """Resolved poller config. `notes` lists human-readable facts the user
    must be told (env overrides, migration); `problems` lists what is wrong."""

    def __init__(self, data: dict, path: str, notes: list, source: str):
        self.data = data
        self.path = path
        self.notes = notes
        self.source = source   # "config.json" | "legacy" | "none"

    def __getitem__(self, k):
        return self.data.get(k, DEFAULTS.get(k))

    def get(self, k, d=None):
        v = self.data.get(k)
        return d if v is None else v

    @property
    def site(self) -> str:
        return (self.data.get("site") or "").rstrip("/")

    @property
    def auth(self) -> str:
        a = str(self.data.get("auth") or "").strip().lower()
        if a in AUTH_MODES:
            return a
        return "basic" if self.data.get("email") else "bearer"

    @property
    def endpoints(self) -> list:
        return self.data.get("endpoints") or []

    def endpoint(self, name: str) -> dict | None:
        for e in self.endpoints:
            if e.get("name") == name:
                return e
        return None

    @property
    def searches(self) -> list:
        return self.data.get("searches") or []

    def search(self, name: str) -> dict | None:
        for s in self.searches:
            if s.get("name") == name:
                return s
        return None

    def problems(self) -> list:
        p = []
        for k in ("site", "token") + (("email",) if self.auth == "basic" else ()):
            if not self.data.get(k):
                p.append(f"{k} missing (setup sheet, jira_api.py --init, or export JIRA_{k.upper()})")
        a = str(self.data.get("auth") or "").strip().lower()
        if a and a not in AUTH_MODES:
            p.append(f"auth '{a}' must be one of {'/'.join(AUTH_MODES)}")
        if self.site and not self.site.startswith(("https://", "http://")):
            p.append(f"site '{self.site}' must start with https://")
        names = set()
        for i, e in enumerate(self.endpoints):
            n = e.get("name") or f"#{i}"
            if n in names:
                p.append(f"endpoint '{n}': duplicate name")
            names.add(n)
            if e.get("type", "issues") not in ENDPOINT_TYPES:
                p.append(f"endpoint '{n}': type must be one of {'/'.join(ENDPOINT_TYPES)}")
            try:
                parse_window(e.get("window", "10m"))
            except ConfigError as err:
                p.append(f"endpoint '{n}': {err}")
            pr = e.get("projects", "*")
            if pr != "*" and not (isinstance(pr, list) and all(isinstance(x, str) for x in pr)):
                p.append(f"endpoint '{n}': projects must be \"*\" or a list of keys")
            if not e.get("file"):
                p.append(f"endpoint '{n}': file missing")
            if isinstance(e.get("columns"), str) and columns_problem(e["columns"]):
                p.append(f"endpoint '{n}': {columns_problem(e['columns'])}")
        snames = set()
        for i, sr in enumerate(self.searches):
            n = sr.get("name") or f"#{i}"
            if n in snames:
                p.append(f"search '{n}': duplicate name")
            snames.add(n)
            if sr.get("kind", "text") not in SEARCH_KINDS:
                p.append(f"search '{n}': kind must be one of {'/'.join(SEARCH_KINDS)}")
            if isinstance(sr.get("columns"), str) and columns_problem(sr["columns"]):
                p.append(f"search '{n}': {columns_problem(sr['columns'])}")
        files = [x.get("file") for x in self.endpoints + self.searches if x.get("file")]
        for f in sorted({f for f in files if files.count(f) > 1}):
            p.append(f"file '{f}' is written by more than one job/search")
        return p

    def masked(self) -> dict:
        d = dict(self.data)
        t = d.get("token") or ""
        d["token"] = (t[:4] + "…" + t[-4:]) if len(t) > 12 else ("…" if t else "")
        return d


def load(site=None, email=None, token=None, migrate=True) -> Config:
    """Resolve the config: config.json (migrating the legacy file when only
    that exists), env fills empty credentials, flags override everything."""
    notes: list = []
    source = "none"
    data = dict(DEFAULTS)
    if os.path.exists(CONFIG_JSON):
        try:
            with open(CONFIG_JSON, encoding="utf-8") as fh:
                loaded = json.load(fh)
            if not isinstance(loaded, dict):
                raise ValueError("top level is not an object")
            data.update(loaded)
            source = "config.json"
        except (OSError, ValueError) as err:
            raise ConfigError(f"{CONFIG_JSON}: invalid JSON ({err})")
    elif os.path.exists(LEGACY_CONFIG):
        data = migrate_legacy(parse_legacy(LEGACY_CONFIG))
        source = "legacy"
        if migrate:
            try:
                write_json_600(CONFIG_JSON, data)
                notes.append(f"migrated {LEGACY_CONFIG} -> {CONFIG_JSON}")
                source = "config.json"
            except OSError as err:
                notes.append(f"reading legacy {LEGACY_CONFIG} (cannot write config.json: {err})")
    if not data.get("endpoints"):
        data["endpoints"] = [dict(e) for e in DEFAULT_ENDPOINTS]
    # per-job columns: jobs from before get their own copy of [jira] columns
    # once (written back; env / flag credentials are applied AFTER this so
    # they never leak into the file)
    template = read_section("jira").get("columns", "")
    if migrate and source == "config.json" and template:
        missing = [e for e in data["endpoints"] + (data.get("searches") or [])
                   if isinstance(e, dict) and not e.get("columns")]
        if missing:
            try:
                with open(CONFIG_JSON, encoding="utf-8") as fh:
                    raw = json.load(fh)
                for e in raw.get("endpoints") or []:
                    if isinstance(e, dict) and not e.get("columns"):
                        e["columns"] = template
                for e in raw.get("searches") or []:
                    if isinstance(e, dict) and not e.get("columns"):
                        e["columns"] = template
                write_json_600(CONFIG_JSON, raw)
                for e in missing:
                    e["columns"] = template
                notes.append(f"gave {len(missing)} job(s) their own copy of [jira] columns")
            except (OSError, ValueError) as err:
                notes.append(f"per-job columns migration skipped: {err}")
    for k in ("site", "email", "token"):
        env = os.environ.get("JIRA_" + k.upper(), "")
        if env and not data.get(k):
            data[k] = env
            notes.append(f"using env JIRA_{k.upper()} (config empty)")
    for k, v in (("site", site), ("email", email), ("token", token)):
        if v:
            data[k] = v
    return Config(data, CONFIG_JSON, notes, source)


def save(updates: dict) -> str:
    """Merge `updates` into config.json (creating it, keeping endpoints)."""
    base = dict(DEFAULTS)
    if os.path.exists(CONFIG_JSON):
        with open(CONFIG_JSON, encoding="utf-8") as fh:
            base.update(json.load(fh))
    elif os.path.exists(LEGACY_CONFIG):
        base = migrate_legacy(parse_legacy(LEGACY_CONFIG))
    allowed = set(DEFAULTS) | {"endpoints", "searches", "teamConfig"}
    for k, v in updates.items():
        if k in allowed and v is not None:
            base[k] = v
    # a credential that only exists in the environment is persisted (the
    # setup sheet tells the user "save will persist it into config.json")
    for k in ("site", "email", "token"):
        env = os.environ.get("JIRA_" + k.upper(), "")
        if env and not base.get(k):
            base[k] = env
    if isinstance(base.get("site"), str):
        base["site"] = base["site"].strip().rstrip("/")
    if not base.get("endpoints"):
        base["endpoints"] = [dict(e) for e in DEFAULT_ENDPOINTS]
    write_json_600(CONFIG_JSON, base)
    return CONFIG_JSON


def edit_jobs(cmd: str, args: list) -> int:
    """Dashboard writers. Print JSON {ok, problems, name}; exit 0 / 1. The
    edited config is validated as a whole BEFORE anything is written."""
    def result(ok, problems=(), **extra):
        print(json.dumps({"ok": ok, "problems": list(problems), **extra}))
        return 0 if ok else 1

    try:
        cfg = load()
    except ConfigError as err:
        return result(False, [str(err)])
    eps = [dict(e) for e in cfg.endpoints]
    srs = [dict(x) for x in cfg.searches]
    if cmd in ("--upsert-endpoint", "--upsert-search"):
        try:
            obj = json.load(sys.stdin)
            if not isinstance(obj, dict):
                raise ValueError("expected a JSON object")
        except ValueError as err:
            return result(False, [f"bad input: {err}"])
        is_ep = cmd == "--upsert-endpoint"
        name = str(obj.get("name") or "").strip()
        if not NAME_RE.match(name):
            return result(False, ["name is required: letters, digits, _ . - (max 40)"])
        lst = eps if is_ep else srs
        cur = next((x for x in lst if x.get("name") == name), None)
        new = dict(cur or {})
        new.update({k: v for k, v in obj.items() if v is not None})
        new["name"] = name
        new.setdefault("file", f"{name}.json" if is_ep else f"search-{name}.json")
        if not new.get("columns"):
            new["columns"] = read_section("jira").get("columns", "")
        if is_ep:
            new.setdefault("type", "issues")
            new.setdefault("window", "30m")
            new.setdefault("projects", "*")
            new.setdefault("enabled", True)
        else:
            new.setdefault("kind", "text")
            new.setdefault("args", {})
        if cur is None:
            lst.append(new)
        else:
            lst[lst.index(cur)] = new
        probs = Config({**cfg.data, "endpoints": eps, "searches": srs}, CONFIG_JSON, [],
                       cfg.source).problems()
        probs = [x for x in probs if "missing (setup" not in x]  # creds: not this edit's concern
        if not is_ep:
            try:
                search_jql(new, load_team(cfg.data))
            except ConfigError as err:
                probs.append(f"search '{name}': {err}")
        if is_ep and (new.get("job") or new.get("template")):
            try:
                endpoint_jql(new, load_team(cfg.data))
            except ConfigError as err:
                probs.append(f"endpoint '{name}': {err}")
        if probs:
            return result(False, probs, name=name)
        save({"endpoints": eps, "searches": srs})
        return result(True, name=name, created=cur is None)
    if cmd in ("--delete-endpoint", "--delete-search"):
        if len(args) != 1:
            return result(False, [f"{cmd} NAME"])
        lst = eps if cmd == "--delete-endpoint" else srs
        keep = [x for x in lst if x.get("name") != args[0]]
        if len(keep) == len(lst):
            return result(False, [f"unknown name '{args[0]}'"])
        if cmd == "--delete-endpoint":
            if not keep:
                return result(False, ["the last poll job cannot be deleted (disable it instead)"])
            save({"endpoints": keep})
        else:
            save({"searches": keep})
        return result(True, name=args[0])
    # --set-columns endpoint|search NAME SPEC
    if len(args) != 3 or args[0] not in ("endpoint", "search"):
        return result(False, ["--set-columns endpoint|search NAME SPEC"])
    kind, name, spec = args
    bad = columns_problem(spec)
    if bad:
        return result(False, [bad])
    lst = eps if kind == "endpoint" else srs
    hit = next((x for x in lst if x.get("name") == name), None)
    if hit is None:
        return result(False, [f"unknown {kind} '{name}'"])
    hit["columns"] = spec.strip()
    save({"endpoints": eps} if kind == "endpoint" else {"searches": srs})
    return result(True, name=name)


def main(argv: list) -> int:
    if not argv or argv[0] in ("-h", "--help"):
        print(__doc__.strip())
        return 0
    cmd = argv[0]
    if cmd == "--path":
        print(CONFIG_JSON)
        return 0
    if cmd == "--fields":
        print(",".join(api_fields()))
        return 0
    if cmd == "--save":
        try:
            updates = json.load(sys.stdin)
            if not isinstance(updates, dict):
                raise ValueError("expected a JSON object")
        except ValueError as err:
            print(f"jira-config: bad input: {err}", file=sys.stderr)
            return 2
        print(save(updates))
        return 0
    if cmd in ("--upsert-endpoint", "--upsert-search", "--delete-endpoint", "--delete-search",
               "--set-columns"):
        return edit_jobs(cmd, argv[1:])
    if cmd in ("--set-window", "--set-enabled"):
        if len(argv) != 3:
            print(f"jira-config: {cmd} NAME VALUE", file=sys.stderr)
            return 2
        name, value = argv[1], argv[2]
        try:
            cfg = load()
            if cmd == "--set-window":
                parse_window(value)
        except ConfigError as err:
            print(f"jira-config: {err}", file=sys.stderr)
            return 2
        eps = [dict(e) for e in cfg.endpoints]
        hit = [e for e in eps if e.get("name") == name or name == "all"]
        if not hit:
            print(f"jira-config: unknown endpoint '{name}'", file=sys.stderr)
            return 2
        for e in hit:
            if cmd == "--set-window":
                e["window"] = value
            else:
                e["enabled"] = truthy(value)
        save({"endpoints": eps})
        print(f"{name}: {'window' if cmd == '--set-window' else 'enabled'} = {value}")
        return 0
    try:
        cfg = load()
    except ConfigError as err:
        if cmd == "--check":
            print(json.dumps({"ok": False, "exists": True, "path": CONFIG_JSON,
                              "problems": [str(err)], "notes": []}))
            return 1
        print(f"jira-config: {err}", file=sys.stderr)
        return 2
    if cmd == "--show":
        print(json.dumps(cfg.masked(), indent=2))
        return 0
    if cmd == "--team":
        try:
            print(json.dumps(load_team(cfg.data), indent=2))
        except ConfigError as err:
            print(f"jira-config: {err}", file=sys.stderr)
            return 2
        return 0
    if cmd == "--check":
        probs = cfg.problems()
        try:
            team = load_team(cfg.data)
            probs += [f"team: {x}" for x in team_problems(team)]
        except ConfigError as err:
            probs.append(str(err))
        print(json.dumps({
            "ok": not probs, "exists": os.path.exists(CONFIG_JSON), "path": CONFIG_JSON,
            "source": cfg.source, "problems": probs, "notes": cfg.notes,
            "site": cfg.site, "auth": cfg.auth, "email": cfg["email"], "hasToken": bool(cfg["token"]),
            "defaultProject": cfg["defaultProject"], "defaultMax": cfg["defaultMax"],
            "endpoints": [{k: e.get(k) for k in ("name", "type", "window", "enabled", "file")}
                          for e in cfg.endpoints],
        }))
        return 0 if not probs else 1
    print(f"jira-config: unknown option {cmd} (see --help)", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
