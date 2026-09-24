#!/usr/bin/env python3
"""jira_poll.py - the polling SCHEDULER: runs only the endpoints that are due,
refreshes the shared cache, publishes the json files the jira window reads,
and records everything in ~/.cache/jira/status.json.

launchd fires this every 60s (StartInterval, survives sleep/wake). Each
endpoint in ~/.config/jira/config.json has its own `window` (10m, 1h, ...):
an endpoint is DUE when its window has elapsed since its last run. A tick
where nothing is due is a cheap no-op (still refreshes status.json).

Per due endpoint:
  1. window = its last SUCCESSFUL run minus pollMarginMinutes (Jira index
     lag) - a missed tick (sleep, machine off) is covered automatically.
     No previous run -> the legacy poll-state LAST_POLL, else 10m; no cache
     at all -> full sync.
  2. issues:   sync that window (projects "*" or a list, optional custom
               `jql`) into ~/.cache/jira/jiras.json, publish <file>
     releases: every version of the projects -> <file>
  3. record lastRun / lastSuccess / nextRun / status / items / lastError.

Guarded by an flock on ~/.cache/jira/poll.lock: a second invocation while a
poll runs exits at once (status.json lastSkipped records it). A lock held
longer than lockStaleMinutes belongs to a hung poll: that process is
terminated and the lock taken over.

Honors commands.conf [jira] enabled (THE SWITCH): disabled -> no network,
no publish, just status {enabled:false} — unless `poll-when-disabled = true`
(the menu-bar "keep polling" choice). --force overrides (menu Poll Now).

Usage:
  jira_poll.py                    run whatever is due
  jira_poll.py --projects all     run every enabled endpoint now
  jira_poll.py --projects SAM1,releases   run these endpoints now
  jira_poll.py --init             full sync of the (selected/all) endpoints
  jira_poll.py --window 2h        explicit window override (implies now)
  jira_poll.py --dry-run          print the plan (due, windows, JQL fields);
                                  no network, no writes
  jira_poll.py --describe         JSON for the dashboard window: every
                                  endpoint's schedule, status, full JQL and
                                  the full curl of each request (real token),
                                  plus the [jira] columns -> API fields map
  jira_poll.py --cancel           stop the running poll (SIGTERM to the lock
                                  holder); its endpoints become "cancelled"
  jira_poll.py --quiet            no progress output (launchd)
  jira_poll.py --force            run even when [jira] enabled = false

Exit codes: 0 ok / nothing due / disabled, 1 an endpoint failed after
retries, 2 config error, 3 skipped (another poll holds the lock).
"""
from __future__ import annotations

import fcntl
import json
import os
import signal
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import jira_api  # noqa: E402
import jira_config  # noqa: E402
import jira_status  # noqa: E402

LEGACY_POLL_STATE = os.path.join(jira_config.CACHE_DIR, "poll-state")
KEYS_DIR = os.path.join(jira_config.CACHE_DIR, "endpoints")
MAX_TRIES = 3
RETRY_BASE = 5          # seconds; 5, 10 between the 3 attempts
ERROR_RETRY = 300       # a failed endpoint is retried after min(window, 5m)

QUIET = False


def say(msg: str) -> None:
    if not QUIET:
        print(f"jira-poll: {msg}", file=sys.stderr)


def parse_args(argv: list) -> dict:
    o = {"init": False, "window": "", "projects": "", "dry": False, "quiet": False,
         "force": False, "describe": False, "cancel": False}
    i = 0
    while i < len(argv):
        a = argv[i]
        name, eq, inline = a.partition("=")

        def val():
            nonlocal i
            if eq:
                return inline
            if i + 1 >= len(argv):
                print(f"jira-poll: {a} needs a value", file=sys.stderr)
                sys.exit(2)
            i += 1
            return argv[i]

        if name == "--init":
            o["init"] = True
        elif name == "--window":
            o["window"] = val()
        elif name in ("--projects", "--endpoint", "--endpoints"):
            o["projects"] = val()
        elif name == "--dry-run":
            o["dry"] = True
        elif name == "--quiet":
            o["quiet"] = True
        elif name == "--describe":
            o["describe"] = True
        elif name == "--cancel":
            o["cancel"] = True
        elif name == "--force":
            o["force"] = True
        elif name in ("-h", "--help"):
            print(__doc__.strip())
            sys.exit(0)
        else:
            print(f"jira-poll: unknown option: {a} (see --help)", file=sys.stderr)
            sys.exit(2)
        i += 1
    return o


# ------------------------------------------------------------ lock

class Lock:
    def __init__(self, path: str, stale_minutes: int):
        self.path = path
        self.stale = max(1, stale_minutes) * 60
        self.fh = None
        self.since = ""

    def _try(self) -> bool:
        os.makedirs(os.path.dirname(self.path), exist_ok=True)
        fh = open(self.path, "a+")
        try:
            fcntl.flock(fh, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError:
            fh.close()
            return False
        self.since = jira_status.now_str()
        fh.seek(0)
        fh.truncate()
        fh.write(json.dumps({"pid": os.getpid(), "since": self.since}))
        fh.flush()
        self.fh = fh
        return True

    def holder(self) -> dict:
        try:
            with open(self.path, encoding="utf-8") as fh:
                return json.loads(fh.read() or "{}")
        except (OSError, ValueError):
            return {}

    def acquire(self) -> bool:
        if self._try():
            return True
        h = self.holder()
        since = jira_status.parse_time(h.get("since", ""))
        pid = h.get("pid")
        if since and time.time() - since > self.stale and isinstance(pid, int) and pid > 1:
            say(f"lock held by pid {pid} since {h.get('since')} (> {self.stale // 60}m) - breaking it")
            try:
                os.kill(pid, signal.SIGTERM)
            except OSError:
                pass
            for _ in range(20):
                time.sleep(0.25)
                if self._try():
                    return True
        return False

    def release(self) -> None:
        if self.fh:
            try:
                self.fh.seek(0)
                self.fh.truncate()
                fcntl.flock(self.fh, fcntl.LOCK_UN)
                self.fh.close()
            except OSError:
                pass
            self.fh = None


class Cancelled(Exception):
    """SIGTERM (dashboard Cancel / a stale-lock breaker) mid-poll."""


def on_sigterm(signum, frame):
    raise Cancelled()


def cancel_running() -> int:
    """--cancel: SIGTERM the process holding the poll lock (if any)."""
    lock = Lock(jira_status.POLL_LOCK, 10)
    if lock._try():             # nobody held it
        lock.release()
        print("jira-poll: no poll is running")
        return 0
    pid = lock.holder().get("pid")
    if not isinstance(pid, int) or pid <= 1:
        print("jira-poll: lock held but no pid recorded", file=sys.stderr)
        return 1
    try:
        os.kill(pid, signal.SIGTERM)
    except OSError as err:
        print(f"jira-poll: cannot stop pid {pid}: {err}", file=sys.stderr)
        return 1
    print(f"jira-poll: sent SIGTERM to poll pid {pid}")
    return 0


# ------------------------------------------------------------ scheduling

def legacy_last_poll() -> str:
    try:
        with open(LEGACY_POLL_STATE, encoding="utf-8") as fh:
            for line in fh:
                if line.startswith("LAST_POLL="):
                    return line.split("=", 1)[1].strip().replace("T", " ")
    except OSError:
        pass
    return ""


def next_run(entry: dict, window_s: int) -> float | None:
    last = jira_status.parse_time(entry.get("lastRun", ""))
    if last is None:
        return None
    if entry.get("status") == "error":
        return last + min(window_s, ERROR_RETRY)
    return last + window_s


def choose_window(ep: dict, entry: dict, o: dict, margin: int) -> str:
    if o["init"]:
        return "full"
    if o["window"]:
        return o["window"]
    if not os.path.exists(jira_api.CACHE_FILE):
        return "full"
    base = entry.get("lastSuccess") or ""
    if not base and ep.get("jql"):
        return "full"     # a custom query needs its whole key set once
    if not base:
        base = legacy_last_poll()
    t = jira_status.parse_time(base)
    if t is None:
        return "10m"
    return time.strftime("%Y-%m-%d %H:%M", time.localtime(t - margin * 60))


# ------------------------------------------------------------ publishing

def sort_updated_desc(items: list) -> list:
    return sorted(items, key=lambda e: e.get("updated") or "", reverse=True)


def shape(entries: list, keys: list) -> list:
    return [{k: (e.get(k) if isinstance(e.get(k), str) else ("" if e.get(k) is None else
                 jira_api.stringify(e.get(k)))) for k in keys} for e in entries]


def publish(path: str, items: list, dry: bool) -> None:
    if dry:
        say(f"dry-run: would write {len(items)} item(s) to {path}")
        return
    jira_api.write_json(path, items, mode=0o644)
    say(f"wrote {len(items)} item(s) to {path}")


def keys_file(name: str) -> str:
    return os.path.join(KEYS_DIR, f"{name}.keys.json")


def issues_for(ep: dict, cache: dict) -> list:
    projects = ep.get("projects", "*")
    if ep.get("jql"):
        keys = set(jira_api.read_json(keys_file(ep["name"]), []))
        items = [v for k, v in cache.items() if k in keys]
    else:
        items = list(cache.values())
    if projects != "*":
        allowed = set(projects)
        items = [v for v in items if v.get("project") in allowed]
    return sort_updated_desc(items)


def release_items(rels: list) -> list:
    out = [{
        "key": f"{r.get('project')}-{r.get('name')}",
        "title": r.get("name") or "",
        "status": "Released" if r.get("released") else "Upcoming",
        "assignee": "",
        "release": r.get("name") or "",
        "releaseLabel": f"{r.get('name')} ({r['releaseDate']})" if r.get("releaseDate") else (r.get("name") or ""),
        "releaseDate": r.get("releaseDate") or "",
        "releaseStatus": "Released" if r.get("released") else "Upcoming",
        "priority": "", "labels": "", "description": "", "reporter": "",
        "project": r.get("project") or "",
    } for r in rels]
    # jq `sort_by(...) | reverse`: ties end up in REVERSED input order
    return sorted(out, key=lambda e: (e["releaseDate"], e["title"]))[::-1]


def run_endpoint(c, ep: dict, window: str, cfg, fields: list, pkeys: list, out_dir: str,
                 dry: bool) -> int:
    """One endpoint, one attempt. Returns the published item count."""
    path = os.path.join(out_dir, ep["file"])
    projects = ep.get("projects", "*")
    plist = None if projects == "*" else list(projects)
    if ep.get("type", "issues") == "releases":
        items = release_items(jira_api.releases(c, projects=plist))
        publish(path, items, dry)
        return len(items)
    res = jira_api.sync(c, window, projects=plist, jql=ep.get("jql", ""), api_fields=fields,
                        fetch_comments=bool(cfg["fetchComments"]),
                        snapshot_keep=int(cfg.get("snapshotKeep", 30)), quiet=QUIET)
    if ep.get("jql"):
        kf = keys_file(ep["name"])
        keys = set() if window == "full" else set(jira_api.read_json(kf, []))
        keys.update(res["keys"])
        jira_api.write_json(kf, sorted(keys))
    cache = jira_api.read_json(jira_api.CACHE_FILE, {})
    items = shape(issues_for(ep, cache), pkeys)
    publish(path, items, dry)
    return len(items)


def describe(cfg, team: dict) -> dict:
    """Everything the dashboard shows, in one JSON: no network (curls are
    built by a dry Client), no writes."""
    status = jira_status.read()
    c = jira_api.Client.from_config(cfg, dry=True)
    fields = jira_config.api_fields(team=team)
    aliases = jira_config.custom_field_aliases(team)
    margin = int(cfg["pollMarginMinutes"] or 5)
    known_projects = list(team.get("project_keys") or []) or jira_config._cache_projects()
    eps = []
    for ep in cfg.endpoints:
        entry = jira_status.endpoint_entry(status, ep["name"])
        typ = ep.get("type", "issues")
        try:
            wsec = jira_config.parse_window(ep.get("window", "10m"))
        except jira_config.ConfigError:
            wsec = 0
        projects = ep.get("projects", "*")
        plist = None if projects == "*" else list(projects)
        c.captured = []
        reqs, jql, notes = [], "", []
        window = "-"
        try:
            if typ == "releases":
                jira_api.releases(c, projects=plist)
                reqs += [{"purpose": "list projects" if plist is None and not team.get("project_keys")
                          else "versions", "curl": x} for x in c.captured]
                if plist is None and not team.get("project_keys"):
                    for p in known_projects:
                        reqs.append({"purpose": f"versions of {p} (one per project)",
                                     "curl": c.curl_cmd(c.url(c.path("project_versions", project=p)))})
                    notes.append("projects = \"*\": every project the token can see gets one versions call")
            else:
                window = choose_window(ep, entry, {"init": False, "window": ""}, margin)
                res = jira_api.sync(c, window, projects=plist, jql=ep.get("jql", ""),
                                    api_fields=fields, fetch_comments=False, quiet=True)
                jql = res["jql"]
                reqs += [{"purpose": "search (first page; startAt / nextPageToken pages follow)",
                          "curl": x} for x in c.captured]
                if cfg["fetchComments"]:
                    reqs.append({"purpose": "comments - one per changed issue (example key)",
                                 "curl": c.curl_cmd(c.url(c.path("issue", key="KEY-1"), "fields=comment"))})
                if "fixVersions" in fields:
                    reqs.append({"purpose": "release dates - one per project in the results",
                                 "curl": c.curl_cmd(c.url(c.path("project_versions",
                                                                 project=(plist or known_projects or ["PROJ"])[0])))})
        except (jira_config.ConfigError, jira_api.ApiError) as err:
            notes.append(f"cannot build request: {err}")
        nx = next_run(entry, wsec) if wsec else None
        eps.append({
            "name": ep["name"], "type": typ, "window": ep.get("window", "10m"),
            "enabled": ep.get("enabled", True), "file": ep.get("file", ""),
            "path": os.path.join(os.path.expanduser(cfg["outDir"] or jira_config.OUT_DIR_DEFAULT),
                                 ep.get("file", "")),
            "projects": projects, "extraJql": ep.get("jql", ""),
            "job": ep.get("job") or ep.get("template") or "", "args": ep.get("args") or {},
            "nextWindow": window, "jql": jql, "requests": reqs, "notes": notes,
            "status": entry.get("status", "never run"), "lastRun": entry.get("lastRun", ""),
            "lastSuccess": entry.get("lastSuccess", ""), "lastWindow": entry.get("lastWindow", ""),
            "nextRun": jira_status.now_str(nx) if nx else "due",
            "items": entry.get("items"), "lastError": entry.get("lastError", ""),
            "lastCurl": entry.get("lastCurl", ""),
        })
    sec = jira_config.read_section("jira")
    cols = []
    for col in jira_config.parse_columns(sec.get("columns", "")):
        f = col["field"]
        if f in jira_config.FIELD_SOURCES:
            src = jira_config.FIELD_SOURCES[f]
        else:
            src = [aliases[f]["id"] if f in aliases else f]
        col.update({"apiFields": src, "label": aliases.get(f, {}).get("label", ""),
                    "description": aliases.get(f, {}).get("description", "")})
        cols.append(col)
    avail = list(jira_config.BASE_WINDOW_KEYS) + ["updated"] + list(aliases)
    lock = Lock(jira_status.POLL_LOCK, 10)
    held = not lock._try()
    if not held:
        lock.release()
    c.captured = []
    c.get(c.path("myself"))
    return {
        "enabled": jira_config.jira_enabled(), "backgroundPoll": jira_config.poll_active()
        and not jira_config.jira_enabled(),
        "site": cfg.site, "auth": cfg.auth, "hasToken": bool(cfg["token"]),
        "configPath": cfg.path, "teamPath": team.get("path", jira_config.TEAM_JSON),
        "teamExists": os.path.exists(team.get("path", jira_config.TEAM_JSON)),
        "commandsConf": jira_config.COMMANDS_CONF, "statusPath": jira_status.STATUS_FILE,
        "curlLog": jira_api.CURL_LOG, "pollScript": jira_status.POLL_SCRIPT,
        "tick": "60s (launchd StartInterval)", "pollMarginMinutes": margin,
        "fetchComments": bool(cfg["fetchComments"]),
        "status": status.get("status", ""), "lastRun": status.get("lastRun", ""),
        "lastError": status.get("lastError", ""),
        "lock": {"held": held, **(lock.holder() if held else {})},
        "projectKeys": team.get("project_keys") or [],
        "apiFields": fields, "columns": cols, "availableFields": list(dict.fromkeys(avail)),
        "loginCurl": c.captured[0] if c.captured else "",
        "endpoints": eps,
    }


def main(argv: list) -> int:
    global QUIET
    o = parse_args(argv)
    QUIET = o["quiet"]
    if o["cancel"]:
        return cancel_running()
    if o["describe"]:
        try:
            cfg = jira_config.load()
            team = jira_config.load_team(cfg.data)
            bad = []
            for ep in cfg.endpoints:
                if not ep.get("jql") and (ep.get("job") or ep.get("template")):
                    try:
                        ep["jql"] = jira_config.endpoint_jql(ep, team)
                    except jira_config.ConfigError as err:   # show the job, flag the problem
                        bad.append(f"endpoint '{ep.get('name')}': {err}")
            d = describe(cfg, team)
            d["problems"] = cfg.problems() + bad + [f"team: {x}" for x in jira_config.team_problems(team)]
        except jira_config.ConfigError as err:
            d = {"problems": [str(err)], "endpoints": [], "columns": []}
        print(json.dumps(d, indent=2))
        return 0
    enabled = jira_config.jira_enabled()
    active = jira_config.poll_active()
    base = {"script": jira_status.POLL_SCRIPT, "curlLog": jira_api.CURL_LOG,
            "config": jira_config.CONFIG_JSON, "commandsConf": jira_config.COMMANDS_CONF,
            "enabled": enabled, "backgroundPoll": active and not enabled}

    def set_base(d, **kw):
        d.update(base)
        d.update(kw)

    if not active and not o["force"] and not o["dry"]:
        jira_status.update(lambda d: set_base(d, status="disabled",
                                              lastCheck=jira_status.now_str()))
        return 0
    try:
        cfg = jira_config.load()
    except jira_config.ConfigError as err:
        jira_status.update(lambda d: set_base(d, status="error", lastError=str(err),
                                              lastCheck=jira_status.now_str()))
        print(f"jira-poll: {err}", file=sys.stderr)
        return 2
    base["configNotes"] = cfg.notes
    probs = cfg.problems()
    try:
        team = jira_config.load_team(cfg.data)
        probs += [f"team: {x}" for x in jira_config.team_problems(team)]
        # job / template endpoints -> plain jql (in memory; config.json untouched)
        for ep in cfg.endpoints:
            if not ep.get("jql") and (ep.get("job") or ep.get("template")):
                ep["jql"] = jira_config.endpoint_jql(ep, team)
    except jira_config.ConfigError as err:
        probs.append(str(err))
        team = {}
    if probs:
        msg = "config: " + "; ".join(probs)
        jira_status.update(lambda d: set_base(d, status="error", lastError=msg,
                                              lastCheck=jira_status.now_str()))
        print(f"jira-poll: {msg}", file=sys.stderr)
        return 2

    lock = Lock(jira_status.POLL_LOCK, int(cfg["lockStaleMinutes"] or 10))
    if not o["dry"] and not lock.acquire():
        h = lock.holder()
        say(f"another poll is running (pid {h.get('pid')} since {h.get('since')}) - skipping")
        jira_status.update(lambda d: set_base(d, lastSkipped={
            "at": jira_status.now_str(), "reason": "skipped-locked",
            "pid": os.getpid(), "holder": h.get("pid")},
            lock={"held": True, "pid": h.get("pid"), "since": h.get("since")}))
        return 3
    signal.signal(signal.SIGTERM, on_sigterm)
    try:
        return poll(o, cfg, team, base, set_base, lock)
    except Cancelled:
        def mark(d):
            for e in d.get("endpoints") or []:
                if e.get("status") == "running":
                    e["status"] = "cancelled"
                    e["lastError"] = "cancelled by user"
            set_base(d, status="cancelled", lastError="poll cancelled",
                     lastRun=jira_status.now_str())
        jira_status.update(mark)
        say("cancelled")
        return 130
    finally:
        lock.release()
        if not o["dry"]:
            jira_status.update(lambda d: d.update(lock={"held": False, "pid": None, "since": None}))


def poll(o: dict, cfg, team: dict, base: dict, set_base, lock: Lock) -> int:
    status = jira_status.read()
    now = time.time()
    names = [n.strip() for n in o["projects"].split(",") if n.strip()] if o["projects"] else []
    forced = bool(names) or o["init"] or bool(o["window"])
    unknown = [n for n in names if n != "all" and not cfg.endpoint(n)]
    if unknown:
        print(f"jira-poll: unknown endpoint(s): {', '.join(unknown)} "
              f"(known: {', '.join(e['name'] for e in cfg.endpoints)})", file=sys.stderr)
        return 2
    plan = []
    for ep in cfg.endpoints:
        wanted = (not names or "all" in names) and ep.get("enabled", True) or ep["name"] in names
        entry = jira_status.endpoint_entry(status, ep["name"])
        wsec = jira_config.parse_window(ep.get("window", "10m"))
        nxt = next_run(entry, wsec)
        due = wanted and (forced or nxt is None or now >= nxt)
        if due:
            plan.append((ep, entry, wsec))
    margin = int(cfg["pollMarginMinutes"] or 5)
    fields = jira_config.api_fields(team=team)
    pkeys = jira_config.publish_keys()
    out_dir = os.path.expanduser(cfg["outDir"] or jira_config.OUT_DIR_DEFAULT)

    if o["dry"]:
        print(f"config:  {cfg.path} ({cfg.source})")
        print(f"enabled: {base['enabled']}  (commands.conf [jira])")
        print(f"fields:  {','.join(fields)}")
        print(f"publish: {','.join(pkeys)}")
        for ep in cfg.endpoints:
            entry = jira_status.endpoint_entry(status, ep["name"])
            hit = any(p[0] is ep for p in plan)
            w = choose_window(ep, entry, o, margin) if ep.get("type", "issues") == "issues" else "-"
            print(f"  {ep['name']:<10} {ep.get('type', 'issues'):<8} every {ep.get('window'):<4} "
                  f"{'DUE' if hit else 'not due':<8} window={w:<17} -> {os.path.join(out_dir, ep['file'])}")
        return 0

    def mark_running(d):
        set_base(d, lock={"held": True, "pid": os.getpid(), "since": lock.since},
                 lastCheck=jira_status.now_str())
        # an enabled tick replaces a stale "disabled"/config-error state even
        # when nothing is due (the endpoints' own results follow below)
        if d.get("status") in (None, "disabled") or str(d.get("lastError", "")).startswith("config"):
            d["status"] = "idle"
            d["lastError"] = ""
        known = {e["name"] for e in cfg.endpoints}
        d["endpoints"] = [e for e in d.get("endpoints", []) if e.get("name") in known]
        for ep in cfg.endpoints:
            e = jira_status.endpoint_entry(d, ep["name"])
            wsec = jira_config.parse_window(ep.get("window", "10m"))
            e.update({"type": ep.get("type", "issues"), "window": ep.get("window"),
                      "enabled": ep.get("enabled", True), "file": ep["file"],
                      "path": os.path.join(out_dir, ep["file"])})
            if any(p[0] is ep for p in plan):
                e["status"] = "running"
            nx = next_run(e, wsec)
            e["nextRun"] = jira_status.now_str(nx) if nx else "due"
    jira_status.update(mark_running)

    if not plan:
        say("nothing due")
        jira_status.update(lambda d: None)
        return 0

    c = jira_api.Client.from_config(cfg)
    failures = []
    for ep, entry, wsec in plan:
        window = choose_window(ep, entry, o, margin) if ep.get("type", "issues") == "issues" else "-"
        say(f"{ep['name']}: window={window}")
        err = ""
        curl = ""
        items = None
        for attempt in range(1, MAX_TRIES + 1):
            try:
                items = run_endpoint(c, ep, window, cfg, fields, pkeys, out_dir, False)
                err = ""
                break
            except jira_api.ApiError as e:
                err = str(e)
                curl = e.curl
                say(f"{ep['name']}: attempt {attempt}/{MAX_TRIES} failed: {err}")
                if e.code in (401, 403) or attempt == MAX_TRIES:
                    break   # auth errors never fix themselves on retry
                time.sleep(RETRY_BASE * 2 ** (attempt - 1))
            except (OSError, ValueError) as e:
                err = f"{type(e).__name__}: {e}"
                break
        ran = jira_status.now_str()

        def record(d, ep=ep, err=err, items=items, window=window, ran=ran, wsec=wsec, curl=curl):
            e = jira_status.endpoint_entry(d, ep["name"])
            e["lastRun"] = ran
            e["lastWindow"] = window
            e["status"] = "error" if err else "ok"
            e["lastError"] = err
            e["lastCurl"] = curl if err else ""   # the failing request, runnable ($JIRA_TOKEN)
            if not err:
                e["lastSuccess"] = ran
                e["items"] = items
            nx = next_run(e, wsec)
            e["nextRun"] = jira_status.now_str(nx) if nx else "due"
        jira_status.update(record)
        if err:
            failures.append(f"{ep['name']}: {err}")
    final = "error" if failures else "ok"
    last_err = "; ".join(failures)
    jira_status.update(lambda d: set_base(d, lastRun=jira_status.now_str(), status=final,
                                          lastError=last_err, requests=c.requests))
    # the legacy poll-state keeps the old doctor / tooling working
    try:
        cache = jira_api.read_json(jira_api.CACHE_FILE, {})
        with open(LEGACY_POLL_STATE, "w", encoding="utf-8") as fh:
            fh.write(f"LAST_POLL={jira_status.now_str()}\nWINDOW=per-endpoint\nSTATUS={final}\n"
                     f"ITEMS={len(cache)}\nOUTPUTS={len(plan)}\nERROR={last_err}\n")
    except OSError:
        pass
    say(f"poll complete: {len(plan)} endpoint(s), status={final}, {c.requests} request(s)")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
