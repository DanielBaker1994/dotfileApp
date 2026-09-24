#!/usr/bin/env python3
"""jira_config.py - load / validate / migrate the jira poller config.

Single source of truth for everything the python poller reads:

  ~/.config/jira/config.json     site, auth, endpoints (chmod 600)
  commands.conf [jira]           enabled (THE SWITCH) + columns (render
                                 fields AND the API fields= param)

The legacy env-style ~/.config/jira/config (JIRA_SITE=... lines, written by
the old jira-api.sh --init) is migrated to config.json on first load; the old
file is left untouched.

Credential precedence: --site/--email/--token flags > config.json value >
exported JIRA_SITE / JIRA_EMAIL / JIRA_TOKEN (env only fills EMPTY config
values, and every such fill is recorded as a note the status surface shows).

CLI (used by the menu bar + setup sheet):
  jira_config.py --path            print the config.json path
  jira_config.py --check           validate; JSON {ok, problems, notes, ...}
  jira_config.py --show            print config.json with the token masked
  jira_config.py --fields          print the API fields= list from [jira]
  jira_config.py --save            merge a JSON object from stdin into
                                   config.json (site/email/token/...); the
                                   token travels on stdin, never argv
  jira_config.py --set-window NAME WINDOW     change one endpoint's schedule
  jira_config.py --set-enabled NAME true|false
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
LEGACY_CONFIG = os.environ.get("JIRA_CONFIG_FILE") or os.path.join(HOME, ".config/jira/config")
COMMANDS_CONF = os.environ.get("WS_COMMANDS_CONF") or os.path.join(WS_ROOT, "commands.conf")

CACHE_DIR = os.environ.get("JIRA_CACHE_DIR") or os.path.join(HOME, ".cache/jira")
OUT_DIR_DEFAULT = os.path.join(HOME, ".cache/workspace-switcher/jira_json")

DEFAULTS = {
    "site": "",
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


class ConfigError(Exception):
    pass


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


def window_fields(section: dict | None = None) -> list:
    """Every window-json field the [jira] section references, columns first
    (ordered, de-duplicated). Empty section -> the legacy base keys."""
    sec = read_section("jira") if section is None else section
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


def api_fields(section: dict | None = None) -> list:
    """The Jira fields= list: sources of every referenced window field plus
    the always-needed ones. THE coupling between [jira] columns and the API."""
    out: list = []
    for f in window_fields(section) + ALWAYS_API_FIELDS:
        for src in FIELD_SOURCES.get(f, [f]):
            if src not in out:
                out.append(src)
    return out


def publish_keys(section: dict | None = None) -> list:
    """Keys written to each window json: the legacy base shape plus any
    extra referenced field (comments stay cache-only - not a string)."""
    keys = list(BASE_WINDOW_KEYS)
    for f in window_fields(section):
        if f not in keys and f != "comments":
            keys.append(f)
    return keys


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
    def endpoints(self) -> list:
        return self.data.get("endpoints") or []

    def endpoint(self, name: str) -> dict | None:
        for e in self.endpoints:
            if e.get("name") == name:
                return e
        return None

    def problems(self) -> list:
        p = []
        for k in ("site", "email", "token"):
            if not self.data.get(k):
                p.append(f"{k} missing (setup sheet, jira_api.py --init, or export JIRA_{k.upper()})")
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
    allowed = set(DEFAULTS) | {"endpoints"}
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
    if cmd == "--check":
        probs = cfg.problems()
        print(json.dumps({
            "ok": not probs, "exists": os.path.exists(CONFIG_JSON), "path": CONFIG_JSON,
            "source": cfg.source, "problems": probs, "notes": cfg.notes,
            "site": cfg.site, "email": cfg["email"], "hasToken": bool(cfg["token"]),
            "defaultProject": cfg["defaultProject"], "defaultMax": cfg["defaultMax"],
            "endpoints": [{k: e.get(k) for k in ("name", "type", "window", "enabled", "file")}
                          for e in cfg.endpoints],
        }))
        return 0 if not probs else 1
    print(f"jira-config: unknown option {cmd} (see --help)", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
