#!/usr/bin/env python3
"""jira_poll.py - the polling SCHEDULER: runs only the endpoints that are due,
refreshes the shared cache, publishes the json files the jira window reads,
and records everything in ~/.cache/jira/status.json.

launchd fires this every 60s (StartInterval, survives sleep/wake). Each
endpoint in ~/.config/jira/config.json has its own `window` (10m, 1h, ...):
an endpoint is DUE when its window has elapsed since its last run. A tick
where nothing is due is a cheap no-op (still refreshes status.json).

SCOPE: team.json project_keys = the projects in scope, the source of truth.
Every job is limited to them ("*" = all of them, never the whole site); with
no scope nothing runs. SETUP: until config.json setup.state is "done"
(`--setup`: each step run and confirmed one at a time), the scheduled tick
does nothing.

Per tick:
  1. window = the last SUCCESSFUL run's start minus pollMarginMinutes (Jira
     index lag) - a missed tick (sleep, machine off) is covered automatically.
     No previous run -> the legacy poll-state LAST_POLL, else 10m; no cache
     at all (or an unfinished full sync) -> full sync.
  2. issues:   plain issue jobs (no custom jql) share ONE sync of their
               projects per tick (status entry "sync", the "issue cache"),
               then each publishes its <file> from the cache - overlapping
               jobs never re-query the same tickets. A job with its own jql
               runs its own search. Syncs are STREAMED: the cache is written
               every checkpointEvery issues with a resume point
               (~/.cache/jira/checkpoints/NAME.json), so a failure at ticket
               9,000 keeps 1-8,999 and the next run continues from there.
               Comments come in the search itself (one request per page).
     releases: every version of the projects -> <file>; the ones in
               config.json releaseBlacklist -> blacklist_release.json
     favorites: re-query the pinned issues (config.json favorites, the ☆
               in the Jira window; key in (...), clamped to the scope) ->
               favorites.json. No pins = no request.
     directory: projects + assignable users + statuses / types / priorities
               / fields + releases + labels -> ~/.cache/jira/directory.json
               (the pickers' lists; weekly - user search is expensive; no
               tab). Checkpointed per project: a rerun resumes / retries
               only what failed.
  3. record lastRun / lastSuccess / nextRun / status / items / lastError.

Rate limits: 429 / 5xx / network errors (and a 401 after the token already
worked in this run) are waited out per REQUEST (Retry-After, else backoff)
within rateLimitMaxWaitMinutes - a job never restarts from page 1.

Log: ~/.cache/jira/poll.log (always written, --quiet only silences stderr):
every stage, page (done / total / %, ETA), wait and error. status.json
`progress` carries the live line the Jira Config window shows.
~/.cache/jira/debug.log (jira_log.py): the same plus every request, retry,
stage entry/exit and traceback, each tagged [job/stage/project];
~/.cache/jira/raw/<run>/: every response body + headers as received.

Guarded by an flock on ~/.cache/jira/poll.lock: a second invocation while a
poll runs exits at once (status.json lastSkipped records it). A lock held
longer than lockStaleMinutes belongs to a hung poll: that process is
terminated and the lock taken over.

Honors commands.toml [jira] enabled (THE SWITCH): disabled -> no network,
no publish, just status {enabled:false} — unless `poll-when-disabled = true`
(the menu-bar "keep polling" choice). --force overrides (menu Poll Now).

Usage:
  jira_poll.py                    run whatever is due
  jira_poll.py --projects '*'     run every enabled endpoint now ("all"
                                  works too unless a job is named "all")
  jira_poll.py --projects SAM1,releases   run these endpoints now
  jira_poll.py --init             full sync of the (selected/all) endpoints
                                  (resumes an interrupted full sync)
  jira_poll.py --rebuild          start from scratch: wipe the issue cache,
                                  resume points and versions, re-populate
                                  everything (config rebuildOnNextPoll: the
                                  next tick does it; cleared when complete)
  jira_poll.py --setup [--step NAME]   the one-time setup: connection, scope,
                                  then every job one at a time, each recorded
                                  in config.json setup.steps; steps already
                                  ok are skipped; --step reruns one. All ok ->
                                  setup.state = done (scheduled polling on)
  jira_poll.py --favorite add|remove KEY...          pin / unpin issues
  jira_poll.py --release-view [KEY...]   one tab file per release (its issues,
                                  from the cache) for the release view window
  jira_poll.py --blacklist-release add|remove KEY... hide / restore releases
  jira_poll.py --favorite-release add|remove KEY... star releases (Jira sidebar)
  jira_poll.py --pin-label add|remove NAME...       pin labels (Jira sidebar LABELS)
  jira_poll.py --pin-board add|remove ID...         pin agile boards (Jira sidebar
                                  BOARDS): reads the board's columns, filter
                                  JQL + quick filters, adds a board-ID job
  jira_poll.py --sprints BOARD_ID                 the board's active + future sprints (JSON;
                                  kanban boards have none)
  jira_poll.py --board-catalog [--cached]         projects in scope -> boards -> sprints
                                  (active, future, closed); the poll tick refreshes it
                                  hourly; --cached = the last answer (board_catalog.json)
  jira_poll.py --board-sprint-keys BOARD SPRINT|current   keys of the board's issues
                                  in that sprint (the board view's sprint picker)
  jira_poll.py --pin-view add|remove BOARD|SPRINT|MODE...   board views pinned to
                                  the Jira sidebar (SPRINT = id|current|all,
                                  MODE = columns|table)
  jira_poll.py --pin-sprint add|remove SPRINT@BOARD...   pin sprints (Jira sidebar
                                  SPRINTS): adds a sprint-ID job (`sprint = ID`)
  jira_poll.py --board-sprint ID on|off   scrum board job: only open sprints
  jira_poll.py --board-quickfilter ID QF...   the keys of the board's issues that
                                  match its quick filters (ANDed, like Jira);
                                  one key-only search; prints {ok, keys}
  jira_poll.py --import-filters   your favourite Jira filters -> one job each
                                  (filter-ID, scope ANDed in); prints them
  jira_poll.py --my-work          the sidebar's MY WORK views from the cache
                                  (assigned to me / reported by me / updated today)
  jira_poll.py --label-view NAME  the label's issues (from the cache) as a tab
                                  file for the sidebar's LABELS pin
                                  (both: config.json + the tabs rewritten
                                  from local data at once; no request, no lock)
  jira_poll.py --window 2h        explicit window override (implies now)
  jira_poll.py --dry-run          print the plan (due, windows, JQL fields);
                                  no network, no writes
  jira_poll.py --describe         JSON for the dashboard window: every
                                  endpoint's schedule, status, full JQL and
                                  the full curl of each request (real token),
                                  plus the [jira] columns -> API fields map
  jira_poll.py --live-search      criteria JSON on stdin (see
                                  jira_config.criteria_jql) -> one search now,
                                  published as <outDir>/search.json (the Jira
                                  window's search tab). No lock, no cache
                                  merge. Prints {ok, count, total, jql, curl}.
                                  With --dry-run: only the JQL + curl.
  jira_poll.py --directory        refresh directory.json now (= Force Poll of
                                  the directory job)
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
import re
import select
import signal
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import jira_api  # noqa: E402
import jira_config  # noqa: E402
import jira_log  # noqa: E402
import jira_status  # noqa: E402
import jira_paths  # noqa: E402

_DEFAULTS_FILE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "defaults.json")
# cache-file names live in pylib/paths.json; the poll limits, sprint rank and
# MY WORK vocabulary in jira/defaults.json (one home each)
with open(_DEFAULTS_FILE, encoding="utf-8") as _fh:
    _DEFAULTS = json.load(_fh)

LEGACY_POLL_STATE = jira_paths.cache_file("legacyPollState")
KEYS_DIR = jira_paths.cache_file("keysDir")
FIELDS_SEEN = jira_paths.cache_file("fieldsSeen")
POLL_LOG = jira_paths.cache_file("pollLog")
POLL_LOG_MAX = _DEFAULTS["logging"]["poll_log_max"]  # rotate: keep the newest half past 2MB
ERROR_RETRY = _DEFAULTS["poll"]["error_retry"]  # a failed endpoint is retried (resumed) after min(window, 5m)
SYNC = "sync"           # the shared issue-cache sync: status entry + checkpoint name

QUIET = False


def log(line: str) -> None:
    """Append to poll.log (no secrets: messages never carry the token)."""
    try:
        os.makedirs(jira_config.CACHE_DIR, exist_ok=True)
        if os.path.exists(POLL_LOG) and os.path.getsize(POLL_LOG) > POLL_LOG_MAX:
            with open(POLL_LOG, "rb") as fh:
                fh.seek(-POLL_LOG_MAX // 2, os.SEEK_END)
                tail = fh.read()
            with open(POLL_LOG, "wb") as fh:
                fh.write(tail[tail.find(b"\n") + 1:])
        with open(POLL_LOG, "a", encoding="utf-8") as fh:
            fh.write(f"{time.strftime('%Y-%m-%d %H:%M:%S')} [{os.getpid()}] {line}\n")
    except OSError:
        pass


def say(msg: str, tag: str = "") -> None:
    line = f"[{tag}] {msg}" if tag else msg
    log(line)
    jira_log.LOG.info("%s", line, stacklevel=2)
    if not QUIET:
        print(f"jira-poll: {line}", file=sys.stderr)


def fmt_secs(s: float) -> str:
    s = int(max(0, s))
    return f"{s}s" if s < 60 else f"{s // 60}m" if s < 3600 else f"{s // 3600}h {s % 3600 // 60}m"


class Reporter:
    """One job's progress -> poll.log (every page) + status.json `progress`
    (the Jira Config window's live line) + the Client's rate-limit waits."""

    def __init__(self, job: str, label: str):
        self.job, self.label = job, label
        self.t0 = time.time()
        self.pages = 0

    def start(self, msg: str) -> None:
        say(msg, self.job)
        jira_status.set_progress(job=self.job, stage=self.label, message=f"{self.label}: {msg}",
                                 done=0, total=None, pct=None, etaSeconds=None, waitingUntil=None)

    def page(self, done: int, total, hwm: float, new: int) -> None:
        self.pages += 1
        el = max(0.001, time.time() - self.t0)
        pct = min(100, int(done * 100 / total)) if total else None
        eta = (total - done) / (new / el) if total and new and total > done else None
        msg = f"{self.label}: {done:,}" + (f" / {total:,} ({pct}%)" if total else " issues") \
            + (f" · ~{fmt_secs(eta)} left" if eta else "")
        say(f"page {self.pages}: {done:,}" + (f"/{total:,} ({pct}%)" if total else "")
            + f" · {new / el:.1f}/s" + (f" · ETA {fmt_secs(eta)}" if eta else "")
            + (f" · up to {time.strftime('%Y-%m-%d %H:%M', time.localtime(hwm))}" if hwm else ""),
            self.job)
        jira_status.set_progress(job=self.job, stage=self.label, done=done, total=total, pct=pct,
                                 etaSeconds=int(eta) if eta else None, message=msg, waitingUntil=None,
                                 reason=None)

    def step(self, msg: str, done=None, total=None, log_it: bool = False) -> None:
        """A stage of a non-issue job (directory): the live line; poll.log
        only when log_it (per project, not per page - debug.log has pages)."""
        if log_it:
            say(msg, self.job)
        pct = min(100, int(done * 100 / total)) if total and done is not None else None
        jira_status.set_progress(job=self.job, stage=self.label, done=done, total=total, pct=pct,
                                 etaSeconds=None, message=f"{self.label}: {msg}", waitingUntil=None,
                                 reason=None)

    def note(self, msg: str) -> None:
        say(msg, self.job)
        jira_status.set_progress(job=self.job, stage=self.label, message=f"{self.label}: {msg}",
                                 waitingUntil=None, reason=None)

    def wait(self, msg: str, secs: float) -> None:
        say(msg, self.job)
        reason = msg.split(" - ")[0]
        jira_status.set_progress(job=self.job, stage=self.label, waitingUntil=time.time() + secs,
                                 reason=reason, message=f"{self.label}: {reason} - resuming in {fmt_secs(secs)}")


def parse_args(argv: list) -> dict:
    o = {"init": False, "window": "", "projects": "", "dry": False, "quiet": False,
         "force": False, "describe": False, "cancel": False, "live": False, "directory": False,
         "rebuild": False, "setup": False, "step": "", "favorite": None, "blacklist": None,
         "release_view": None, "label_view": None, "board_sprint": None,
         "import_filters": False, "my_work": False}
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
        elif name == "--live-search":
            o["live"] = True
        elif name == "--directory":
            o["directory"] = True
        elif name == "--force":
            o["force"] = True
        elif name == "--rebuild":
            o["rebuild"] = True
        elif name == "--setup":
            o["setup"] = True
        elif name == "--step":
            o["step"] = val()
        elif name == "--release-view":
            # --release-view [KEY...]: the keys = releases to include even if
            # blacklisted (the one double-clicked in blacklist_release.json)
            o["release_view"] = argv[i + 1:]
            return o
        elif name == "--label-view":
            o["label_view"] = val()
        elif name == "--sprints":
            o["sprints"] = val()
        elif name == "--board-sprint-keys":
            o["board_sprint_keys"] = argv[i + 1:i + 3]
            return o
        elif name == "--board-catalog":
            o["board_catalog"] = True
        elif name == "--cached":
            o["cached"] = True
        elif name == "--board-sprint":
            o["board_sprint"] = argv[i + 1:i + 3]
            return o
        elif name == "--board-quickfilter":
            o["board_quick"] = argv[i + 1:]
            return o
        elif name == "--import-filters":
            o["import_filters"] = True
        elif name == "--my-work":
            o["my_work"] = True
        elif name in ("--favorite", "--blacklist-release", "--favorite-release", "--pin-label", "--pin-board",
                      "--pin-sprint", "--pin-view"):
            # --favorite add|remove KEY...  (the rest of argv = the keys)
            op = val()
            o[{"--favorite": "favorite", "--blacklist-release": "blacklist",
               "--favorite-release": "favorite_release", "--pin-label": "pin_label",
               "--pin-board": "pin_board", "--pin-sprint": "pin_sprint",
               "--pin-view": "pin_view"}[name]] = (op, argv[i + 1:])
            return o
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


def choose_window(name: str, entry: dict, o: dict, margin: int, needs_full: bool = False,
                  fallback: str = "") -> str:
    if o["init"]:
        return "full"
    if o["window"]:
        return o["window"]
    if not os.path.exists(jira_api.CACHE_FILE):
        return "full"
    if jira_api.load_checkpoint(name).get("mode") == "full":
        return "full"     # finish (resume) an interrupted full sync
    base = entry.get("lastSuccess") or fallback
    if not base and needs_full:
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
    note_fields_seen(os.path.basename(path), items)


def note_fields_seen(file: str, items: list) -> None:
    """The field catalog's "seen" half: field -> files where it had a value
    (~/.cache/jira/fields_seen.json). Best effort."""
    seen = jira_api.read_json(FIELDS_SEEN, {})
    if not isinstance(seen, dict):
        seen = {}
    filled = {k for it in items for k, v in it.items() if v not in ("", None, [], {})}
    for f in set(seen) | {k for it in items for k in it}:
        files = [x for x in seen.get(f, []) if x != file]
        if f in filled:
            files.append(file)
        seen[f] = sorted(files)
    try:
        jira_api.write_json(FIELDS_SEEN, seen)
    except OSError:
        pass


def ep_path(ep: dict, out_dir: str) -> str:
    """Where a job publishes: its tab file, or directory.json."""
    if ep.get("type") == "directory":
        return jira_api.DIRECTORY_FILE
    return os.path.join(out_dir, ep.get("file", ""))


def keys_file(name: str) -> str:
    return os.path.join(KEYS_DIR, f"{name}.keys.json")


def issues_for(ep: dict, cache: dict, team: dict | None = None) -> list:
    """A job's rows from the cache; with `team`, clamped to the scope."""
    projects = ep.get("projects", "*") if team is None else jira_config.job_projects(ep, team)
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


def release_rows(rels: list) -> list:
    """release_items + each version's id (versionId: the app's "open in
    browser" goes to /projects/KEY/versions/ID - a release has no /browse)."""
    ids = {f"{r.get('project')}-{r.get('name')}": str(r.get("id") or "") for r in rels}
    return [dict(it, versionId=ids.get(it["key"], "")) for it in release_items(rels)]


def release_sort(rows: list) -> list:
    """release_items' order (newest date first) for rows moved between tabs."""
    return sorted(rows, key=lambda e: (e.get("releaseDate", ""), e.get("title", "")))[::-1]


def split_blacklist(rows: list, blacklist: list) -> tuple:
    """(shown, hidden): hidden = the rows whose key is blacklisted."""
    bl = set(blacklist or [])
    return [r for r in rows if r.get("key") not in bl], [r for r in rows if r.get("key") in bl]


def config_list(cfg, key: str) -> list:
    v = cfg.data.get(key) if hasattr(cfg, "data") else (cfg or {}).get(key)
    return [x for x in v if isinstance(x, str) and x] if isinstance(v, list) else []


def favorites_endpoint(cfg) -> dict:
    return next((e for e in cfg.endpoints if e.get("type") == "favorites"),
                dict(jira_config.FAVORITES_ENDPOINT))


def favorite_rows(cfg, team: dict, out_dir: str, extra: list | None = None) -> list:
    """favorites.json: the pinned keys (pin order, newest first) from the issue
    cache; a key the cache lacks (e.g. pinned from the live search, not
    polled yet) keeps the row the app passed or the row already published."""
    ep = favorites_endpoint(cfg)
    _, pkeys = job_fields(ep, team)
    cache = jira_api.read_json(jira_api.CACHE_FILE, {})
    cache = cache if isinstance(cache, dict) else {}
    path = os.path.join(out_dir, ep.get("file") or jira_config.FAVORITES_FILE)
    old = jira_api.read_json(path, [])
    known = {r.get("key"): r for r in (old if isinstance(old, list) else []) if isinstance(r, dict)}
    known.update({r.get("key"): r for r in (extra or []) if isinstance(r, dict)})
    rows = []
    for k in config_list(cfg, "favorites"):
        if k in cache:
            rows.append(shape([cache[k]], pkeys)[0])
        elif k in known:
            rows.append({p: str(known[k].get(p) or "") for p in pkeys})
    return rows


def run_favorites(ctx: "Ctx", ep: dict, rep) -> int:
    """Re-query the pinned issues (key in (...), clamped to the scope) into
    the cache, then publish favorites.json. A key Jira rejects (deleted /
    moved issue) is left out of the query and the search retried once."""
    c, cfg, team = ctx.c, ctx.cfg, ctx.team
    keys = config_list(cfg, "favorites")
    path = os.path.join(ctx.out_dir, ep["file"])
    if keys:
        f_ep, _ = job_fields(ep, team)
        plist = jira_config.job_projects(ep, team)
        ask = list(keys)
        for tries in (1, 2):
            rep.start(f"re-query {len(ask)} pinned issue(s)")
            try:
                jira_api.sync(c, "full", projects=plist, jql="key in (" + ", ".join(ask) + ")",
                              api_fields=f_ep, fetch_comments=bool(cfg["fetchComments"]),
                              snapshot_keep=0, quiet=True, default_projects=False, name=ep["name"],
                              checkpoint_every=int(cfg["checkpointEvery"] or 500), progress=rep.page)
                break
            except jira_api.ApiError as err:
                bad = [k for k in ask if k in str(err)]
                if err.code != 400 or not bad or tries == 2 or len(bad) == len(ask):
                    raise
                say(f"skipping pinned key(s) Jira rejects: {', '.join(bad)}", ep["name"])
                ask = [k for k in ask if k not in bad]
    rows = favorite_rows(cfg, team, ctx.out_dir)
    publish(path, rows, False)
    return len(rows)


def job_fields(ep: dict, team: dict) -> tuple:
    """(api fields, publish keys) from THIS job's own columns."""
    spec = jira_config.job_columns(ep)
    return (jira_config.api_fields(team=team, columns=spec),
            jira_config.publish_keys(columns=spec))


def side_dir(out_dir: str, name: str) -> str:
    """A folder next to outDir (its files are not tabs)."""
    d = os.path.join(os.path.dirname(os.path.expanduser(out_dir).rstrip("/")), name)
    os.makedirs(d, exist_ok=True)
    return d


def job_path(out_dir: str, ep: dict) -> str:
    """Where a job publishes: outDir (a tab), except a pinned board's job
    (BOARD_DIR: the sidebar's BOARDS row shows it, no tab of its own)."""
    if ep.get("boardId"):
        return os.path.join(side_dir(out_dir, jira_config.BOARD_DIR), ep["file"])
    if ep.get("sideDir") in (jira_config.MY_WORK_DIR,):
        return os.path.join(side_dir(out_dir, ep["sideDir"]), ep["file"])
    return os.path.join(out_dir, ep["file"])


def plain_issue_job(ep: dict) -> bool:
    """An issue job without its own query: a view over the shared sync."""
    return ep.get("type", "issues") == "issues" and not ep.get("jql")


class Ctx:
    """Everything one poll / setup run shares."""

    def __init__(self, o: dict, cfg, team: dict, c=None):
        self.o, self.cfg, self.team, self.c = o, cfg, team, c
        self.out_dir = os.path.expanduser(cfg["outDir"] or jira_config.OUT_DIR_DEFAULT)
        self.margin = int(cfg["pollMarginMinutes"] or 5)
        self.plain = [e for e in cfg.endpoints if plain_issue_job(e) and e.get("enabled", True)]

    def sync_projects(self) -> list:
        """The shared sync's projects: every plain job's (clamped) projects,
        in scope order."""
        want = {p for e in self.plain for p in jira_config.job_projects(e, self.team)}
        return [p for p in jira_config.scope_projects(self.team) if p in want]

    def sync_fields(self) -> list:
        out: list = []
        for e in self.plain:
            for f in job_fields(e, self.team)[0]:
                if f not in out:
                    out.append(f)
        return out or jira_config.api_fields(team=self.team)

    def sync_limits(self) -> tuple:
        pages = [e["maxResults"] for e in self.plain if e.get("maxResults")]
        caps = [e.get("maxTotal") for e in self.plain]
        return (max(pages) if pages else None,
                max(caps) if caps and all(caps) else None)

    def sync_wsec(self) -> int:
        ws = [jira_config.parse_window(e.get("window", "10m")) for e in self.plain]
        return min(ws) if ws else 600

    def sync_window(self, status: dict) -> str:
        entry = jira_status.endpoint_entry(status, SYNC)
        # before the shared sync existed the plain jobs synced themselves:
        # the oldest of their last successes is where it continues
        olds = sorted(x for x in (jira_status.endpoint_entry(status, e["name"]).get("lastSuccess")
                                  for e in self.plain) if x)
        return choose_window(SYNC, entry, self.o, self.margin, fallback=olds[0] if olds else "")


def synced_projects() -> list | None:
    """The projects the issue cache holds in full (None = unknown: before
    this was tracked every sync covered its projects)."""
    v = jira_status.endpoint_entry(jira_status.read(), SYNC).get("syncedProjects")
    return v if isinstance(v, list) else None


def synced_fields() -> list | None:
    v = jira_status.endpoint_entry(jira_status.read(), SYNC).get("syncedFields")
    return v if isinstance(v, list) else None


def backfill_fields(ctx: "Ctx", projects: list) -> list:
    """Fields the shared sync asks for that the cache was never filled with
    (a column / group field added later): fetch ONLY those (+ key) for every
    scoped issue, once, and merge them into the cache - far lighter than a
    full re-sync. Before syncedFields was tracked: the GROUP_FIELDS sources.
    Returns the fields now in the cache."""
    c = ctx.c
    want = ctx.sync_fields()
    have = synced_fields()
    if have is None:
        group = {s for f in jira_config.GROUP_FIELDS for s in jira_config.FIELD_SOURCES[f]}
        group |= set(jira_config.epic_link_ids())
        have = [f for f in want if f not in group]
    missing = [f for f in want if f not in have]
    if not missing:
        return want
    rep = Reporter(SYNC + "-fields", "New fields")
    c.on_wait = rep.wait
    c.on_note = rep.note
    rep.start(f"filling {', '.join(missing)} for the cached issues of {', '.join(projects)}")
    jql = "project in (" + ", ".join(f'"{jira_config.jql_quote(p)}"' for p in projects) + ") ORDER BY key ASC"
    cache = jira_api.read_json(jira_api.CACHE_FILE, {})
    cache = cache if isinstance(cache, dict) else {}
    vers = jira_api.read_json(jira_api.VERSIONS_FILE, {})
    n = 0
    for got, total, _ in c.search_pages(jql, ",".join(missing)):
        for i in got:
            k = i.get("key")
            if k in cache:
                e = jira_api.cache_entry(i, vers if isinstance(vers, dict) else {}, None, missing)
                e.pop("key", None)
                cache[k].update(e)
        n += len(got)
        rep.page(n, total, time.time(), len(got))
    jira_api.write_json(jira_api.CACHE_FILE, cache, mode=0o600)
    say(f"filled {', '.join(missing)} on {n:,} issue(s)", SYNC)
    return want


def run_shared_sync(ctx: Ctx, window: str) -> dict:
    """ONE streamed sync for every plain issue job, then publish them all.
    A project added to the scope since the last sync is fully synced first
    (an incremental window would only bring its recently updated tickets)."""
    c, cfg = ctx.c, ctx.cfg
    projects = ctx.sync_projects()
    if not projects:
        raise jira_config.ConfigError(jira_config.NO_SCOPE)
    page, cap = ctx.sync_limits()
    if window != "full" and synced_projects() is not None:
        ctx.filled = backfill_fields(ctx, projects)
    else:
        ctx.filled = ctx.sync_fields()
    have = synced_projects()
    added = [p for p in projects if have is not None and p not in have] if window != "full" else []
    if added:
        rep = Reporter(SYNC + "-added", "New projects")
        c.on_wait = rep.wait
        c.on_note = rep.note
        rep.start(f"full sync of the projects added to the scope: {', '.join(added)}")
        jira_api.sync(c, "full", projects=added, api_fields=ctx.sync_fields(),
                      fetch_comments=bool(cfg["fetchComments"]), snapshot_keep=0, quiet=True,
                      default_projects=False, page_size=page, max_total=cap, name=SYNC + "-added",
                      checkpoint_every=int(cfg["checkpointEvery"] or 500), progress=rep.page,
                      flushed=lambda keys: publish_plain(ctx, quiet=True))
    rep = Reporter(SYNC, "Issue cache")
    c.on_wait = rep.wait
    c.on_note = rep.note
    resumed = jira_api.load_checkpoint(SYNC)
    rep.start(f"{'full sync' if window == 'full' else 'sync since ' + window} of "
              f"{', '.join(projects)}" + (f" - resuming at {resumed.get('hwmText')} "
                                          f"({resumed.get('fetched', 0):,} done)" if resumed else ""))
    res = jira_api.sync(c, window, projects=projects, api_fields=ctx.sync_fields(),
                        fetch_comments=bool(cfg["fetchComments"]),
                        snapshot_keep=int(cfg.get("snapshotKeep", 30)), quiet=True,
                        default_projects=False, page_size=page, max_total=cap, name=SYNC,
                        checkpoint_every=int(cfg["checkpointEvery"] or 500), progress=rep.page,
                        flushed=lambda keys: publish_plain(ctx, quiet=True))
    say(f"done: {res['fetched']:,} issue(s) fetched{' (resumed)' if res['resumed'] else ''}, "
        f"cache {res['total']:,}, {c.requests} request(s)", SYNC)
    return res


def publish_plain(ctx: Ctx, only: str = "", quiet: bool = False) -> dict:
    """Publish the plain issue jobs (or one) from the cache -> {name: rows}."""
    global QUIET
    cache = jira_api.read_json(jira_api.CACHE_FILE, {})
    if not isinstance(cache, dict):
        cache = {}
    out = {}
    was = QUIET
    QUIET = QUIET or quiet
    try:
        for ep in ctx.plain:
            if only and ep["name"] != only:
                continue
            _, pkeys = job_fields(ep, ctx.team)
            items = shape(issues_for(ep, cache, ctx.team), pkeys)
            path = os.path.join(ctx.out_dir, ep["file"])
            if quiet:
                jira_api.write_json(path, items, mode=0o644)
            else:
                publish(path, items, False)
            out[ep["name"]] = len(items)
    finally:
        QUIET = was
    return out


def run_job(ctx: Ctx, ep: dict, window: str) -> int:
    """One directory / releases / custom-jql job. Returns the item count."""
    c, cfg, team = ctx.c, ctx.cfg, ctx.team
    plist = jira_config.job_projects(ep, team)
    if not plist:
        raise jira_config.ConfigError(jira_config.NO_SCOPE)
    typ = ep.get("type", "issues")
    rep = Reporter(ep["name"], f"{ep['name']} ({typ})")
    c.on_wait = rep.wait
    c.on_note = rep.note
    if typ == "directory":
        rep.start(f"directory of {', '.join(plist)} - projects, users, statuses/types/priorities, "
                  "fields, releases, labels (live steps in debug.log)")
        last = {"key": None}

        def progress(stage, msg, done=None, total=None):
            # poll.log: the first line of each stage / project (done = the
            # project's index); the window + debug.log: every page
            rep.step(msg, done, total, log_it=(stage, done) != last["key"])
            last["key"] = (stage, done)

        d = jira_api.directory(c, projects=plist, quiet=True, progress=progress, name=ep["name"],
                               resume_hours=float(cfg["directoryResumeHours"] or 24))
        jira_api.write_json(jira_api.DIRECTORY_FILE, d)
        say(f"{len(d['projects'])} project(s), {len(d['users'])} user(s), {len(d['versions'])} release(s), "
            f"{len(d['labels'])} label(s)"
            + (f", {len(d['warnings'])} warning(s): {'; '.join(d['warnings'])}" if d["warnings"] else ""),
            ep["name"])
        return len(d["users"])
    path = job_path(ctx.out_dir, ep)
    if typ == "releases":
        rep.start(f"versions of {', '.join(plist)}")
        items, hidden = split_blacklist(release_rows(jira_api.releases(c, projects=plist)),
                                        config_list(cfg, "releaseBlacklist"))
        publish(path, items, False)
        publish(os.path.join(ctx.out_dir, jira_config.BLACKLIST_RELEASE_FILE), hidden, False)
        return len(items)
    if typ == "favorites":
        return run_favorites(ctx, ep, rep)
    # custom jql: its own streamed search (its key set = its rows)
    kf = keys_file(ep["name"])
    if window != "full" and not os.path.exists(kf):
        # its key set is gone (a job removed and added again, a reset): a
        # "since" window would publish only what changed since = nothing
        say("no key set yet - full sync", ep["name"])
        window = "full"
    fresh = window == "full" and not jira_api.load_checkpoint(ep["name"])
    base_keys = set() if fresh else set(jira_api.read_json(kf, []))
    f_ep, k_ep = job_fields(ep, team)
    rep.start(f"{'full sync' if window == 'full' else 'sync since ' + window}: {ep['jql']}")

    def flushed(keys):
        jira_api.write_json(kf, sorted(base_keys | set(keys)))

    jira_api.sync(c, window, projects=plist, jql=ep["jql"], api_fields=f_ep,
                  fetch_comments=bool(cfg["fetchComments"]),
                  snapshot_keep=int(cfg.get("snapshotKeep", 30)), quiet=True,
                  default_projects=False, page_size=ep.get("maxResults"),
                  max_total=ep.get("maxTotal"), name=ep["name"],
                  checkpoint_every=int(cfg["checkpointEvery"] or 500), progress=rep.page,
                  flushed=flushed)
    cache = jira_api.read_json(jira_api.CACHE_FILE, {})
    items = shape(issues_for(ep, cache, team), k_ep)
    publish(path, items, False)
    return len(items)


def record(name: str, started: str, window: str, err: str, curl: str, items, wsec: int,
           extra: dict | None = None) -> None:
    """One job's result -> status.json (lastSuccess = the run's START, so the
    next window also covers what changed while a long sync ran)."""
    ran = jira_status.now_str()

    def fn(d):
        e = jira_status.endpoint_entry(d, name)
        e["lastRun"] = ran
        e["lastWindow"] = window
        e["status"] = "error" if err else "ok"
        e["lastError"] = err
        e["lastCurl"] = curl if err else ""   # the failing request, runnable ($JIRA_TOKEN)
        if not err:
            e["lastSuccess"] = started
            e["items"] = items
        if extra:
            e.update(extra)
        nx = next_run(e, wsec)
        e["nextRun"] = jira_status.now_str(nx) if nx else "due"
    jira_status.update(fn)


def logged(name: str, c, fn):
    """fn() as one debug.log stage (▶ / ◀ / ✗ + traceback)."""
    with jira_log.stage(name, c=c) as st:
        res = fn()
        st.items = res if isinstance(res, int) else res[0] if isinstance(res, tuple) and \
            isinstance(res[0], int) else (res or {}).get("total") if isinstance(res, dict) else None
        return res


def attempt(fn) -> tuple:
    """Run fn() -> (result, err, curl); a failed job keeps its partial
    progress (checkpoint) and is resumed by the next run."""
    try:
        return fn(), "", ""
    except jira_api.ApiError as e:
        return None, str(e), e.curl
    except jira_config.ConfigError as e:
        return None, str(e), ""
    except (OSError, ValueError) as e:
        return None, f"{type(e).__name__}: {e}", ""


def columns_meta(spec: str, aliases: dict) -> list:
    """Parsed columns + the Jira API field(s) each one fetches."""
    out = []
    for col in jira_config.parse_columns(spec):
        f = col["field"]
        if f in jira_config.FIELD_SOURCES:
            src = jira_config.FIELD_SOURCES[f]
        else:
            src = [aliases[f]["id"] if f in aliases else f]
        col.update({"apiFields": src, "label": aliases.get(f, {}).get("label", ""),
                    "description": aliases.get(f, {}).get("description", "")})
        out.append(col)
    return out


def team_own(path: str) -> dict:
    """team.json as written (keys normalized), without the defaults."""
    raw = jira_api.read_json(os.path.expanduser(path), {})
    raw = jira_config._norm_keys(raw) if isinstance(raw, dict) else {}
    return {k: raw[k] for k in jira_config.TEAM_EDITABLE if k in raw}


def describe(cfg, team: dict) -> dict:
    """Everything the Jira Config window shows, in one JSON: no network
    (curls are built by a dry Client), no writes."""
    status = jira_status.read()
    c = jira_api.Client.from_config(cfg, dry=True)
    aliases = jira_config.custom_field_aliases(team)
    margin = int(cfg["pollMarginMinutes"] or 5)
    known_projects = jira_config.scope_projects(team)
    template = jira_config.read_section("jira").get("columns", "")

    scope = jira_config.scope_projects(team)
    ctx = Ctx({"init": False, "window": ""}, cfg, team, c)

    def issue_requests(name, plist, jql, window, fields, page_size=None, max_total=None):
        c.captured = []
        res = jira_api.sync(c, window, projects=plist or None, jql=jql, api_fields=fields,
                            fetch_comments=bool(cfg["fetchComments"]), quiet=True,
                            default_projects=False, page_size=page_size, max_total=max_total,
                            name=name)
        reqs = [{"purpose": "search, oldest first (first page; startAt / nextPageToken pages "
                            "follow; comments ride along in the `comment` field)", "curl": x}
                for x in c.captured]
        if "fixVersions" in fields:
            reqs.append({"purpose": "release dates - one per project in the results",
                         "curl": c.curl_cmd(c.url(c.path(
                             "project_versions", project=(plist or known_projects or ["PROJ"])[0])))})
        return res["jql"], reqs

    shared = None
    if ctx.plain and scope:
        try:
            page, cap = ctx.sync_limits()
            sw = ctx.sync_window(status)
            shared = (sw,) + issue_requests(SYNC, ctx.sync_projects(), "", sw, ctx.sync_fields(),
                                            page, cap)
        except (jira_config.ConfigError, jira_api.ApiError):
            shared = None

    eps = []
    for ep in cfg.endpoints:
        entry = jira_status.endpoint_entry(status, ep["name"])
        typ = ep.get("type", "issues")
        try:
            wsec = jira_config.parse_window(ep.get("window", "10m"))
        except jira_config.ConfigError:
            wsec = 0
        projects = ep.get("projects", "*")
        plist = jira_config.job_projects(ep, team)
        spec = jira_config.job_columns(ep)
        fields, _ = job_fields(ep, team)
        reqs, jql, notes, window = [], "", [], "-"
        if not scope:
            notes.append(jira_config.NO_SCOPE)
        try:
            if typ == "directory":
                # dry: /project returns nothing, so name the projects it would page
                users_of = plist or known_projects or ["PROJ"]
                c.captured = []
                jira_api.directory(c, projects=users_of)
                purposes = [f"project {p}" for p in users_of]
                purposes += [f"assignable users of {p} (paginated)" for p in users_of]
                purposes += ["statuses", "issue types", "priorities", "fields"]
                purposes += [f"releases of {p}" for p in users_of]
                purposes += [f"labels of {p} (labelled issues, fields=labels)" for p in users_of]
                reqs += [{"purpose": purposes[i] if i < len(purposes) else "", "curl": x}
                         for i, x in enumerate(c.captured)]
            elif typ == "releases":
                c.captured = []
                jira_api.releases(c, projects=plist or ["PROJ"])
                reqs += [{"purpose": f"versions of {p}", "curl": x}
                         for p, x in zip(plist or ["PROJ"], c.captured)]
                hidden = config_list(cfg, "releaseBlacklist")
                if hidden:
                    notes.append(f"{len(hidden)} blacklisted release(s) go to "
                                 f"{jira_config.BLACKLIST_RELEASE_FILE} instead (Cmd+K in the Jira window)")
            elif typ == "favorites":
                favs = config_list(cfg, "favorites")
                notes.append(f"re-queries the {len(favs)} pinned issue(s) (☆ in the Jira window) every run"
                             if favs else "no pinned issues yet (☆ next to a row's checkbox) - no request")
                if favs:
                    window = "full"
                    jql, r = issue_requests(ep["name"], plist, "key in (" + ", ".join(favs) + ")",
                                            window, fields, None, None)
                    reqs += r
            elif plain_issue_job(ep):
                if shared:
                    window, jql, r = shared
                    reqs += r
                others = [e["name"] for e in ctx.plain if e["name"] != ep["name"]]
                notes.append("shares ONE issue-cache sync per tick"
                             + (f" with {', '.join(others)}" if others else "")
                             + " - this tab is published from the cache (no queries of its own)")
            else:
                window = choose_window(ep["name"], entry, {"init": False, "window": ""}, margin,
                                       needs_full=True)
                jql, r = issue_requests(ep["name"], plist, ep["jql"], window, fields,
                                        ep.get("maxResults"), ep.get("maxTotal"))
                reqs += r
        except (jira_config.ConfigError, jira_api.ApiError) as err:
            notes.append(f"cannot build request: {err}")
        nx = next_run(entry, wsec) if wsec else None
        eps.append({
            "name": ep["name"], "type": typ, "window": ep.get("window", "10m"),
            "enabled": ep.get("enabled", True), "file": ep.get("file", ""),
            "path": ep_path(ep, os.path.expanduser(cfg["outDir"] or jira_config.OUT_DIR_DEFAULT)),
            "maxResults": ep.get("maxResults") or 0, "maxTotal": ep.get("maxTotal") or 0,
            "projects": projects, "extraJql": ep.get("userJql", ep.get("jql", "")),
            "job": ep.get("job") or ep.get("template") or "", "args": ep.get("args") or {},
            "nextWindow": window, "jql": jql, "requests": reqs, "notes": notes,
            "columnsSpec": spec, "columns": columns_meta(spec, aliases), "apiFields": fields,
            "status": entry.get("status", "never run"), "lastRun": entry.get("lastRun", ""),
            "lastSuccess": entry.get("lastSuccess", ""), "lastWindow": entry.get("lastWindow", ""),
            "nextRun": jira_status.now_str(nx) if nx else "due",
            "items": entry.get("items"), "lastError": entry.get("lastError", ""),
            "lastCurl": entry.get("lastCurl", ""), "scopedProjects": plist,
            "sharedSync": plain_issue_job(ep),
        })

    # the live search tab (Cmd+F in the Jira window)
    ls_spec = jira_config.live_search_columns(cfg)
    live = {"file": jira_config.LIVE_SEARCH_FILE, "columnsSpec": ls_spec,
            "columns": columns_meta(ls_spec, aliases),
            "maxResults": cfg.live_search.get("maxResults") or jira_config.LIVE_SEARCH_MAX,
            "path": os.path.join(os.path.expanduser(cfg["outDir"] or jira_config.OUT_DIR_DEFAULT),
                                 jira_config.LIVE_SEARCH_FILE)}
    dirdata = jira_api.read_json(jira_api.DIRECTORY_FILE, {})
    if not isinstance(dirdata, dict):
        dirdata = {}

    # field catalog: every column any job/search defines + every field seen
    # in published data + what can be added (base keys, custom aliases)
    defined: dict = {}
    for kind, lst in (("job", eps), ("live", [{"name": "search", **live}])):
        for j in lst:
            for col in j.get("columns") or []:
                d = defined.setdefault(col["field"], {"field": col["field"], "titles": [],
                                                      "usedBy": [], "apiFields": col["apiFields"],
                                                      "label": col["label"]})
                if col["title"] not in d["titles"]:
                    d["titles"].append(col["title"])
                d["usedBy"].append(f"{kind}:{j['name']}")
    seen = jira_api.read_json(FIELDS_SEEN, {})
    if not isinstance(seen, dict):
        seen = {}
    avail = list(dict.fromkeys(list(jira_config.BASE_WINDOW_KEYS) + ["updated"] + list(aliases)
                               + list(defined) + sorted(seen)))
    catalog = []
    own_labels = (team.get("field_labels") or {})
    no_own = {**team, "field_labels": {}}
    for f in avail:
        d = defined.get(f, {"field": f, "titles": [], "usedBy": [],
                            "apiFields": columns_meta(f, aliases)[0]["apiFields"]})
        # label = the field's ONE display name (Definitions ▸ Fields);
        # defaultLabel = what it falls back to without a team.json rename
        catalog.append({**d, "seenIn": seen.get(f, []),
                        "label": jira_config.field_label(team, f, aliases),
                        "defaultLabel": jira_config.field_label(no_own, f, aliases),
                        "renamed": f in own_labels,
                        "custom": f in aliases, "base": f in jira_config.BASE_FIELD_LABELS})

    sync_entry = jira_status.endpoint_entry(status, SYNC)
    ck = jira_api.load_checkpoint(SYNC)
    setup = jira_config.setup_state(cfg.data)
    done_steps = setup.get("steps") or {}
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
        "progress": status.get("progress") if held else None,
        "pollLog": POLL_LOG,
        "debugLog": jira_log.DEBUG_LOG,
        "rawDir": jira_log.latest_raw_dir(),
        "rawRoot": jira_log.RAW_DIR,
        "scope": scope,
        "setup": {"state": setup.get("state", "done"),
                  "steps": [{**st, **(done_steps.get(st["name"]) or {"state": "pending"})}
                            for st in setup_steps(cfg)]},
        "rebuildOnNextPoll": cfg["rebuildOnNextPoll"] or False,
        "issueCache": {"projects": ctx.sync_projects() if scope else [],
                       "jobs": [e["name"] for e in ctx.plain],
                       "status": sync_entry.get("status", "never run"),
                       "lastRun": sync_entry.get("lastRun", ""),
                       "lastSuccess": sync_entry.get("lastSuccess", ""),
                       "lastError": sync_entry.get("lastError", ""),
                       "lastCurl": sync_entry.get("lastCurl", ""),
                       "items": sync_entry.get("items"),
                       "nextWindow": shared[0] if shared else "",
                       "jql": shared[1] if shared else "",
                       "requests": shared[2] if shared else [],
                       "checkpoint": {k: ck.get(k) for k in ("hwmText", "fetched", "total", "mode",
                                                             "savedAt")} if ck else None},
        "projectKeys": team.get("project_keys") or [],
        "columnsTemplate": template,
        "searchDefaults": {k: c.sd(k) for k in jira_config.DEFAULT_SEARCH},
        "liveSearch": live,
        "directory": {"path": jira_api.DIRECTORY_FILE, "fetchedAt": dirdata.get("fetchedAt", ""),
                      "forProjects": dirdata.get("forProjects") or [],
                      "warnings": dirdata.get("warnings") or [],
                      "counts": {k: len(dirdata.get(k) or []) for k in
                                 ("projects", "users", "statuses", "issueTypes", "priorities", "fields",
                                  "versions", "labels")}},
        "team": {k: team.get(k) for k in jira_config.TEAM_EDITABLE},
        # team.json's own values (what --team-set edits; `team` = merged with defaults)
        "teamOwn": team_own(team.get("path", jira_config.TEAM_JSON)),
        "teamJobs": [j.get("key") for j in team.get("jobs") or [] if isinstance(j, dict)]
        + [k for k in (team.get("jql_templates") or {})],
        "catalog": catalog, "availableFields": avail,
        "loginCurl": c.captured[0] if c.captured else "",
        "endpoints": eps,
    }


def field_types() -> dict:
    """Jira field id -> schema type from directory.json ({} before the first
    directory run): tells the live search `~` (text) from `=`."""
    d = jira_api.read_json(jira_api.DIRECTORY_FILE, {})
    return {f.get("id"): f.get("type") or "" for f in (d.get("fields") or [] if isinstance(d, dict) else [])
            if isinstance(f, dict)}


def live_search(dry: bool) -> int:
    """--live-search: criteria JSON on stdin -> one search now, rows written
    to <outDir>/search.json (the Jira window's search tab). No lock and no
    cache merge (a live search never waits on / disturbs the poll)."""
    def result(code, **kw):
        print(json.dumps({"ok": code == 0, **kw}))
        return code

    try:
        crit = json.load(sys.stdin)
        cfg = jira_config.load()
        team = jira_config.load_team(cfg.data)
        jql = jira_config.criteria_jql(crit, team, field_types())
    except ValueError as err:
        return result(2, error=f"bad criteria: {err}")
    except jira_config.ConfigError as err:
        return result(2, error=str(err))
    ls = cfg.live_search
    spec = jira_config.live_search_columns(cfg)
    fields = jira_config.api_fields(team=team, columns=spec)
    pkeys = jira_config.publish_keys(columns=spec)
    cap = int((crit.get("maxResults") if isinstance(crit, dict) else 0) or ls.get("maxResults")
              or jira_config.LIVE_SEARCH_MAX)
    c = jira_api.Client.from_config(cfg, dry=True)
    c.captured = []
    c.search(jql, ",".join(fields), max_total=cap)
    curl = c.captured[0] if c.captured else ""
    if dry:
        return result(0, jql=jql, curl=curl, maxResults=cap, fields=fields)
    c = jira_api.Client.from_config(cfg)
    try:
        res = c.search(jql, ",".join(fields), max_total=cap)
    except jira_api.ApiError as err:
        return result(1, error=str(err), jql=jql, curl=err.curl or curl)
    vers = jira_api.read_json(jira_api.VERSIONS_FILE, {})
    aliases = jira_config.custom_field_aliases(team)
    rows = [jira_api.cache_entry(i, vers if isinstance(vers, dict) else {}, None, fields, aliases)
            for i in res["issues"]]
    out_dir = os.path.expanduser(cfg["outDir"] or jira_config.OUT_DIR_DEFAULT)
    path = os.path.join(out_dir, jira_config.LIVE_SEARCH_FILE)
    jira_api.write_json(path, shape(rows, pkeys), mode=0o644)
    note_fields_seen(jira_config.LIVE_SEARCH_FILE, rows)
    return result(0, count=len(rows), total=res.get("total"), more=not res["isLast"],
                  jql=jql, curl=curl, file=path, maxResults=cap)


def setup_steps(cfg) -> list:
    """The one-time setup, in order - each step is run and confirmed alone."""
    eps = [e for e in cfg.endpoints if e.get("enabled", True)]
    steps = [{"name": "connection", "label": "Connection", "detail": "log in (/myself) + your Jira time zone"},
             {"name": "scope", "label": "Projects in scope", "detail": "each project you entered exists and is readable"}]
    for e in eps:
        if e.get("type") == "directory":
            steps.append({"name": e["name"], "label": f"Directory ({e['name']})",
                          "detail": "users, statuses, types, releases, labels for the pickers"})
    for e in eps:
        if e.get("type") == "releases":
            steps.append({"name": e["name"], "label": f"Releases ({e['name']})",
                          "detail": "every version of the projects in scope"})
    if any(plain_issue_job(e) for e in eps):
        steps.append({"name": SYNC, "label": "Issue cache (full sync)",
                      "detail": "every ticket of the projects in scope - streamed, resumable"})
        for e in eps:
            if plain_issue_job(e):
                steps.append({"name": e["name"], "label": f"Tab {e['name']}",
                              "detail": f"publish {e.get('file')} from the cache"})
    for e in eps:
        if e.get("type", "issues") == "issues" and e.get("jql"):
            steps.append({"name": e["name"], "label": f"Query job {e['name']}",
                          "detail": "its own JQL, full sync"})
    return steps


def save_setup(st: dict) -> None:
    jira_config.save({"setup": st})


def run_setup_step(ctx: Ctx, name: str) -> tuple:
    """-> (items, message). Raises on failure."""
    c, team, cfg = ctx.c, ctx.team, ctx.cfg
    if name == "connection":
        try:
            os.unlink(jira_api.TZ_FILE)
        except OSError:
            pass
        me = c.get(c.path("myself")) or {}
        tz = jira_api.jira_tz(c)
        return 1, f"logged in as {me.get('displayName') or me.get('name') or '?'}" + (f" · {tz}" if tz else "")
    if name == "scope":
        scope = jira_config.scope_projects(team)
        if not scope:
            raise jira_config.ConfigError(jira_config.NO_SCOPE)
        # only the projects the user entered - the site's list is never fetched
        names, missing = [], []
        for k in scope:
            try:
                names.append(f"{k} ({(c.get(c.path('project', project=k)) or {}).get('name') or k})")
            except jira_api.ApiError as err:
                if err.code not in (403, 404):
                    raise
                missing.append(k)
        if missing:
            raise jira_config.ConfigError(f"not found / not readable with this token: {', '.join(missing)} "
                                          "- fix the projects in scope")
        return len(scope), ", ".join(names)
    if name == SYNC:
        started = jira_status.now_str()
        res = run_shared_sync(ctx, "full")
        counts = publish_plain(ctx)
        record(SYNC, started, "full", "", "", res["total"], ctx.sync_wsec(),
               extra={"syncedProjects": ctx.sync_projects(), "syncedFields": ctx.sync_fields()})
        for e in ctx.plain:
            record(e["name"], started, "full", "", "", counts.get(e["name"]),
                   jira_config.parse_window(e.get("window", "10m")))
        return res["total"], f"{res['total']:,} issue(s) cached" + (" (resumed)" if res["resumed"] else "")
    ep = cfg.endpoint(name)
    if ep is None:
        raise jira_config.ConfigError(f"unknown step '{name}'")
    started = jira_status.now_str()
    wsec = jira_config.parse_window(ep.get("window", "10m"))
    if plain_issue_job(ep):
        n = publish_plain(ctx, only=name).get(name, 0)
        record(name, started, "-", "", "", n, wsec)
        return n, f"{n:,} row(s) in {ep.get('file')}"
    window = "full" if ep.get("type", "issues") == "issues" else "-"
    n = run_job(ctx, ep, window)
    record(name, started, window, "", "", n, wsec)
    return n, f"{n:,} item(s)"


def run_setup(o: dict, cfg, team: dict) -> int:
    st = dict(jira_config.setup_state(cfg.data))
    steps = dict(st.get("steps") or {})
    order = [x["name"] for x in setup_steps(cfg)]
    if o["step"] and o["step"] not in order:
        print(f"jira-poll: unknown setup step '{o['step']}' (steps: {', '.join(order)})", file=sys.stderr)
        return 2
    todo = [o["step"]] if o["step"] else [n for n in order if (steps.get(n) or {}).get("state") != "ok"]
    ctx = Ctx(o, cfg, team, jira_api.Client.from_config(cfg))
    jira_log.describe_config(cfg, team)
    say(f"setup: {len(todo)} step(s) to run: {', '.join(todo) or '(none)'}", "setup")
    code = 0
    for name in todo:
        steps[name] = {"state": "running", "at": jira_status.now_str()}
        save_setup({**st, "steps": steps})
        jira_status.set_progress(job=name, stage="setup", message=f"Setup: {name}…", done=None,
                                 total=None, pct=None, etaSeconds=None, waitingUntil=None)
        (res, err, curl) = attempt(lambda: logged(f"setup:{name}", ctx.c, lambda: run_setup_step(ctx, name)))
        if err:
            steps[name] = {"state": "error", "at": jira_status.now_str(), "error": err, "curl": curl,
                           "rawDir": jira_log.RAW.dir if jira_log.RAW else "",
                           "debugLog": jira_log.DEBUG_LOG}
            save_setup({**st, "steps": steps})
            say(f"✗ {name}: {err}", "setup")
            code = 1
            break      # later steps build on this one: fix it, then rerun
        items, msg = res
        steps[name] = {"state": "ok", "at": jira_status.now_str(), "items": items, "message": msg}
        save_setup({**st, "steps": steps})
        say(f"✓ {name}: {msg}", "setup")
    if all((steps.get(n) or {}).get("state") == "ok" for n in order):
        if st.get("state") != "done":
            say("setup complete - scheduled polling starts now", "setup")
        st["state"] = "done"
    save_setup({**st, "steps": steps})
    return code


def wipe_cache() -> None:
    """--rebuild: the issue cache, resume points, versions and key sets go;
    published tabs are overwritten as the rebuild streams in."""
    for f in (jira_api.CACHE_FILE, jira_api.VERSIONS_FILE, jira_api.STATE_FILE):
        try:
            os.unlink(f)
        except OSError:
            pass
    jira_api.clear_checkpoints()
    if os.path.isdir(KEYS_DIR):
        for f in os.listdir(KEYS_DIR):
            try:
                os.unlink(os.path.join(KEYS_DIR, f))
            except OSError:
                pass

    def fn(d):
        for e in d.get("endpoints") or []:
            e.pop("lastSuccess", None)
    jira_status.update(fn)


def edit_pins(kind: str, op: str, keys: list) -> int:
    """--favorite / --blacklist-release add|remove KEY...: update config.json
    and republish the affected tabs from local data at once (no request, no
    lock) - the next poll refreshes them. stdin (optional, favorites): JSON
    rows the app already shows, for keys the issue cache doesn't hold.
    Prints {ok, keys, count, files}."""
    def result(ok, **kw):
        print(json.dumps({"ok": ok, **kw}))
        return 0 if ok else 1
    if op not in ("add", "remove") or not keys:
        return result(False, problems=[f"{kind} add|remove KEY..."])
    try:
        cfg = jira_config.load()
        team = jira_config.load_team(cfg.data)
    except jira_config.ConfigError as err:
        return result(False, problems=[str(err)])
    out_dir = os.path.expanduser(cfg["outDir"] or jira_config.OUT_DIR_DEFAULT)
    field = "favorites" if kind == "favorites" else "releaseBlacklist"
    cur = config_list(cfg, field)
    if op == "add":
        new = [k for k in keys if k not in cur] + cur     # newest first
    else:
        new = [k for k in cur if k not in keys]
    jira_config.save({field: new})
    cfg.data[field] = new
    if kind == "favorites":
        extra = []
        # the app pipes the rows it shows (then EOF); an inherited open
        # stdin with nothing coming must not block
        if op == "add" and not sys.stdin.isatty() and select.select([sys.stdin], [], [], 1)[0]:
            try:
                got = json.loads(sys.stdin.read() or "[]")
                extra = got if isinstance(got, list) else []
            except ValueError:
                extra = []
        ep = favorites_endpoint(cfg)
        path = os.path.join(out_dir, ep.get("file") or jira_config.FAVORITES_FILE)
        rows = favorite_rows(cfg, team, out_dir, extra)
        jira_api.write_json(path, rows, mode=0o644)
        return result(True, keys=new, count=len(rows), files=[path])
    # releases: every releases tab + the blacklist tab re-split; a restored
    # release goes back to the first releases tab (the next poll re-sorts)
    bl_path = os.path.join(out_dir, jira_config.BLACKLIST_RELEASE_FILE)
    tabs = [os.path.join(out_dir, e["file"]) for e in cfg.endpoints
            if e.get("type") == "releases" and e.get("file")]
    if not tabs:
        return result(False, problems=["no releases job in config.json"])
    hidden_rows = jira_api.read_json(bl_path, [])
    hidden_rows = hidden_rows if isinstance(hidden_rows, list) else []
    moved_back = []
    all_hidden = []
    for i, t in enumerate(tabs):
        rows = jira_api.read_json(t, [])
        rows = rows if isinstance(rows, list) else []
        if i == 0:
            moved_back = [r for r in hidden_rows if r.get("key") not in set(new)]
            rows = rows + [r for r in moved_back if r.get("key") not in {x.get("key") for x in rows}]
        shown, hid = split_blacklist(rows, new)
        all_hidden += hid
        jira_api.write_json(t, release_sort(shown), mode=0o644)
    keep = [r for r in hidden_rows if r.get("key") in set(new)]
    seen = {r.get("key") for r in all_hidden}
    hidden = all_hidden + [r for r in keep if r.get("key") not in seen]
    jira_api.write_json(bl_path, release_sort(hidden), mode=0o644)
    return result(True, keys=new, count=len(hidden), files=tabs + [bl_path])


def edit_favorite_releases(op: str, keys: list) -> int:
    """--favorite-release add|remove KEY...: the starred releases (config.json
    favoriteReleases, newest first) the Jira window pins in its sidebar; a
    click shows the release's issues (--release-view). Prints {ok, keys}."""
    if op not in ("add", "remove") or not keys:
        print(json.dumps({"ok": False, "problems": ["--favorite-release add|remove KEY..."]}))
        return 1
    try:
        cfg = jira_config.load()
    except jira_config.ConfigError as err:
        print(json.dumps({"ok": False, "problems": [str(err)]}))
        return 1
    cur = config_list(cfg, "favoriteReleases")
    new = ([k for k in keys if k not in cur] + cur) if op == "add" else [k for k in cur if k not in keys]
    jira_config.save({"favoriteReleases": new})
    print(json.dumps({"ok": True, "keys": new}))
    return 0


def edit_pinned_labels(op: str, names: list) -> int:
    """--pin-label add|remove NAME...: the labels the Jira window pins in its
    sidebar (config.json pinnedLabels, pin order); a click shows the label's
    issues (--label-view). Prints {ok, labels}."""
    names = [n.strip() for n in names if n.strip()]
    if op not in ("add", "remove") or not names:
        print(json.dumps({"ok": False, "problems": ["--pin-label add|remove NAME..."]}))
        return 1
    try:
        cfg = jira_config.load()
    except jira_config.ConfigError as err:
        print(json.dumps({"ok": False, "problems": [str(err)]}))
        return 1
    cur = config_list(cfg, "pinnedLabels")
    new = (cur + [n for n in names if n not in cur]) if op == "add" else [n for n in cur if n not in names]
    jira_config.save({"pinnedLabels": new})
    print(json.dumps({"ok": True, "labels": new}))
    return 0


def label_view(name: str) -> int:
    """--label-view NAME: the issues carrying the label, from the issue cache
    (newest first, the main issue job's columns) -> LABEL_VIEW_DIR/<NAME>.json.
    Local only (no request, no lock). Files of labels no longer pinned (and
    not this one) are removed. Prints {ok, dir, file, count}."""
    try:
        cfg = jira_config.load()
        team = jira_config.load_team(cfg.data)
    except jira_config.ConfigError as err:
        print(json.dumps({"ok": False, "problems": [str(err)]}))
        return 1
    out_dir = os.path.expanduser(cfg["outDir"] or jira_config.OUT_DIR_DEFAULT)
    view_dir = os.path.join(os.path.dirname(out_dir.rstrip("/")), jira_config.LABEL_VIEW_DIR)
    os.makedirs(view_dir, exist_ok=True)
    main_ep = next((e for e in cfg.endpoints if plain_issue_job(e) and e.get("name") == "all"),
                   next((e for e in cfg.endpoints if plain_issue_job(e)), {}))
    _, pkeys = job_fields(main_ep, team)
    cache = jira_api.read_json(jira_api.CACHE_FILE, {})
    cache = cache if isinstance(cache, dict) else {}
    hits = [e for e in cache.values() if isinstance(e, dict)
            and name in (x.strip() for x in str(e.get("labels") or "").split(","))]
    items = shape(sort_updated_desc(hits), pkeys)
    f = release_view_file("label", name)
    jira_api.write_json(os.path.join(view_dir, f), items, mode=0o644)
    keep = {release_view_file("label", n) for n in config_list(cfg, "pinnedLabels")} | {f}
    for old in os.listdir(view_dir):
        if old.endswith(".json") and old not in keep:
            try:
                os.unlink(os.path.join(view_dir, old))
            except OSError:
                pass
    print(json.dumps({"ok": True, "dir": view_dir, "file": f, "count": len(items)}))
    return 0


def strip_order(jql: str) -> str:
    return re.sub(r"\s+ORDER\s+BY\s+.*$", "", jql or "", flags=re.I | re.S).strip()


def board_job(bid: str, jql: str, sprint: bool, columns: str) -> dict:
    q = f"({jql}) AND sprint in openSprints()" if sprint else jql
    return {"name": f"board-{bid}", "type": "issues", "jql": q, "boardJql": jql, "boardId": bid,
            "sprintOnly": bool(sprint), "file": f"board-{bid}.json", "window": "15m",
            "projects": "*", "enabled": True, "columns": columns}


def reset_job(name: str) -> None:
    """A job whose JQL changed starts over (its key set + checkpoint)."""
    for f in (keys_file(name), jira_api.checkpoint_path(name)):
        try:
            os.unlink(f)
        except OSError:
            pass


def board_info(c, bid: str, status_ids: dict) -> dict:
    """GET the board's configuration (columns -> status names, its filter),
    the filter's JQL and the quick filters. A column status id the
    directory doesn't know (no directory job yet, a status from a project
    outside the scope) -> ONE GET /status fills `status_ids` in place:
    otherwise the columns were saved empty and every card sat in "Not on
    the board"."""
    conf = c.get(c.path("board_configuration", board_id=bid)) or {}
    fid = str((conf.get("filter") or {}).get("id") or "")
    jql = strip_order((c.get(c.path("filter", filter_id=fid)) or {}).get("jql") or "") if fid else ""
    wanted = {str(st.get("id")) for col in ((conf.get("columnConfig") or {}).get("columns") or [])
              for st in col.get("statuses") or []}
    if wanted - set(status_ids):
        try:
            for v in c.get(c.path("statuses")) or []:
                if isinstance(v, dict) and v.get("id") is not None and v.get("name"):
                    status_ids[str(v["id"])] = v["name"]
        except jira_api.ApiError as err:
            say(f"board {bid}: status names not read ({err}) - columns may stay empty", "boards")
    cols = []
    for col in ((conf.get("columnConfig") or {}).get("columns") or []):
        names = [status_ids.get(str(st.get("id")), "") for st in col.get("statuses") or []]
        cols.append({"name": col.get("name") or "", "statuses": [n for n in names if n]})
    try:
        qf = c.get(c.path("board_quickfilters", board_id=bid), "maxResults=50") or {}
        quick = [{"id": str(q.get("id")), "name": q.get("name") or "", "jql": q.get("jql") or ""}
                 for q in qf.get("values") or [] if isinstance(q, dict)]
    except jira_api.ApiError:
        quick = []
    return {"id": bid, "name": conf.get("name") or bid, "type": conf.get("type") or "",
            "filterId": fid, "jql": jql, "columns": cols, "quickFilters": quick}


SPRINT_RANK = dict(_DEFAULTS["sprints"]["rank"])


def sprint_row(s: dict, bid: str) -> dict:
    return {"id": str(s.get("id")), "name": s.get("name") or str(s.get("id")), "state": s.get("state") or "",
            "board": bid, "startDate": s.get("startDate") or "", "endDate": s.get("endDate") or "",
            "completeDate": s.get("completeDate") or "", "goal": s.get("goal") or ""}


def sort_sprints(rows: list) -> list:
    """active, then future (soonest first), then closed (newest first)."""
    def when(s):
        return s["completeDate"] or s["endDate"] or s["startDate"]
    closed = sorted((s for s in rows if s["state"] == "closed"), key=when, reverse=True)
    rest = sorted((s for s in rows if s["state"] != "closed"),
                  key=lambda s: (SPRINT_RANK.get(s["state"], 3), s["startDate"] or "9999"))
    return [s for s in rest if SPRINT_RANK.get(s["state"], 3) < 2] + closed + \
        [s for s in rest if SPRINT_RANK.get(s["state"], 3) > 2]


def fetch_sprints(c, bid: str, states: str = "active,future,closed", trace: list | None = None,
                  limit: int = 500) -> tuple:
    """GET /board/ID/sprint, every page -> (sprints, why_none). A 400 / 404 =
    the board has no sprints: a kanban board, or a team-managed board whose
    Sprints feature is off ("The board does not support sprints")."""
    out, start, page = [], 0, 50
    path = c.path("board_sprints", board_id=bid)
    while True:
        qs = f"state={states}&startAt={start}&maxResults={page}"
        try:
            got = c.get(path, qs) or {}
        except jira_api.ApiError as err:
            if err.code not in (400, 404):
                raise
            body = str(err)
            msg = ""
            try:
                msg = (json.loads(body[body.index("{"):]).get("errorMessages") or [""])[0]
            except (ValueError, AttributeError):
                pass
            if trace is not None:
                trace.append(f"GET {path}?state={states} -> {err.code} {msg or 'no sprints'}")
            return [], msg or f"HTTP {err.code}"
        vals = [x for x in got.get("values") or [] if isinstance(x, dict) and x.get("id") is not None]
        out += [sprint_row(x, bid) for x in vals]
        start += len(vals)
        if got.get("isLast", True) or not vals or len(out) >= limit:
            break
    if trace is not None:
        trace.append(f"GET {path}?state={states} -> {len(out)} sprint(s)")
    return sort_sprints(out), ""


def board_sprints(c, bid: str, states: str = "active,future") -> list:
    """The board's sprints (scrum only: a kanban board answers 400 -> [])."""
    return fetch_sprints(c, bid, states)[0]


def board_kind(b: dict, why_none: str) -> str:
    """What the browser says a board is."""
    t = b.get("type") or ""
    if t == "kanban":
        return "kanban"
    if t == "simple":
        return "team-managed" + (", sprints off" if why_none else "")
    return t or "board"


def board_catalog_path() -> str:
    return os.path.join(jira_api.CACHE_DIR, jira_config.BOARD_CATALOG_FILE)


def board_catalog(c, keys: list) -> dict:
    """Projects in scope -> their boards -> each board's sprints, the way Jira
    links them and never wider: GET /board?projectKeyOrId=KEY per project
    (paged), then GET /board/ID/sprint (every state, paged) once per board.
    `trace` = the requests in order, for the window to show where it looked."""
    trace, problems, projects, seen = [], [], [], {}
    bpath = c.path("boards")
    for proj in keys:
        boards, start = [], 0
        try:
            while True:
                got = c.get(bpath, f"projectKeyOrId={jira_api.qenc(proj)}&startAt={start}&maxResults=50") or {}
                vals = [b for b in got.get("values") or [] if isinstance(b, dict) and b.get("id") is not None]
                boards += vals
                start += len(vals)
                if got.get("isLast", True) or not vals or len(boards) >= 500:
                    break
        except jira_api.ApiError as err:
            problems.append(f"{proj}: {err}")
            trace.append(f"GET {bpath}?projectKeyOrId={proj} -> error {err.code}")
            projects.append({"key": proj, "boards": [], "error": str(err)})
            continue
        trace.append(f"GET {bpath}?projectKeyOrId={proj} -> {len(boards)} board(s)")
        rows = []
        for b in boards:
            bid = str(b["id"])
            if bid not in seen:
                try:
                    sprints, why = fetch_sprints(c, bid, trace=trace)
                except jira_api.ApiError as err:
                    sprints, why = [], f"error {err.code}"
                    problems.append(f"board {bid}: {err}")
                seen[bid] = {"id": bid, "name": b.get("name") or bid, "type": b.get("type") or "",
                             "sprints": sprints, "noSprints": why,
                             "kind": board_kind(b, why)}
            rows.append(seen[bid])
        projects.append({"key": proj, "boards": rows})
    return {"ok": not problems, "projects": projects, "trace": trace, "problems": problems,
            "boards": len(seen), "sprints": sum(len(b["sprints"]) for b in seen.values()),
            "fetchedAt": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "forProjects": keys}


def run_board_catalog(cached: bool) -> int:
    """--board-catalog [--cached]: the boards + sprints browser's data (JSON),
    also kept in the cache; --cached = the last answer, no request."""
    try:
        cfg = jira_config.load()
        team = jira_config.load_team(cfg.data)
    except jira_config.ConfigError as err:
        print(json.dumps({"ok": False, "projects": [], "problems": [str(err)]}))
        return 1
    keys = jira_config.scope_projects(team)
    if cached:
        got = jira_api.read_json(board_catalog_path(), None)
        if isinstance(got, dict) and got.get("forProjects") == keys:
            print(json.dumps({**got, "cached": True}))
            return 0
        print(json.dumps({"ok": False, "projects": [], "problems": ["no cached catalog"], "cached": True}))
        return 1
    if not keys:
        print(json.dumps({"ok": False, "projects": [], "problems": [jira_config.NO_SCOPE]}))
        return 1
    out = board_catalog(jira_api.Client.from_config(cfg), keys)
    jira_api.write_json(board_catalog_path(), out, mode=0o644)
    print(json.dumps(out))
    return 0 if out["ok"] else 1


def sprints_cache_path() -> str:
    return os.path.join(jira_api.CACHE_DIR, jira_config.SPRINTS_FILE)


def list_sprints(bid: str) -> int:
    """--sprints BOARD_ID -> {ok, sprints: [...]}: one request."""
    try:
        cfg = jira_config.load()
        sprints = board_sprints(jira_api.Client.from_config(cfg), bid)
    except (jira_config.ConfigError, jira_api.ApiError) as err:
        print(json.dumps({"ok": False, "sprints": [], "problems": [str(err)]}))
        return 1
    print(json.dumps({"ok": True, "sprints": sprints}))
    return 0


def sprint_job(s: dict, columns: str) -> dict:
    return {"name": f"sprint-{s['id']}", "type": "issues", "jql": f"sprint = {s['id']}",
            "boardId": s["board"], "sprintId": s["id"], "file": f"sprint-{s['id']}.json",
            "window": "15m", "projects": "*", "enabled": True, "columns": columns}


def edit_pinned_sprints(op: str, refs: list) -> int:
    """--pin-sprint add|remove SPRINT@BOARD...: config.json pinnedSprints + a
    sprint-ID job per sprint (`sprint = ID`, scope ANDed in by the sync)
    publishing into BOARD_DIR, + name / state / board in sprints.json. remove
    needs only the ids. Prints {ok, sprints, problems}."""
    refs = [str(x).strip() for x in refs if str(x).strip()]
    if op not in ("add", "remove") or not refs:
        print(json.dumps({"ok": False, "problems": ["--pin-sprint add|remove SPRINT@BOARD..."]}))
        return 1
    try:
        cfg = jira_config.load()
    except jira_config.ConfigError as err:
        print(json.dumps({"ok": False, "problems": [str(err)]}))
        return 1
    cache = jira_api.read_json(sprints_cache_path(), {})
    cache = cache if isinstance(cache, dict) else {}
    eps = [dict(e) for e in cfg.endpoints]
    cur = config_list(cfg, "pinnedSprints")
    problems = []
    if op == "add":
        c = jira_api.Client.from_config(cfg)
        by_board = {}
        for r in refs:
            sid, _, bid = r.partition("@")
            by_board.setdefault(bid, []).append(sid)
        cat = jira_api.read_json(board_catalog_path(), {}) or {}
        cat_sprints = {s["id"]: s for p in cat.get("projects") or [] for b in p.get("boards") or []
                       for s in b.get("sprints") or []}
        for bid, sids in by_board.items():
            try:
                known = {k: v for k, v in cat_sprints.items() if v.get("board") == bid}
                if any(x not in known for x in sids):
                    known = {s["id"]: s for s in fetch_sprints(c, bid)[0]}
            except jira_api.ApiError as err:
                problems.append(f"board {bid}: {err}")
                continue
            for sid in sids:
                s = known.get(sid)
                if not s:
                    problems.append(f"sprint {sid}: not on board {bid}")
                    continue
                cache[sid] = s
                old = next((e for e in eps if e.get("name") == f"sprint-{sid}"), None)
                job = sprint_job(s, (old or {}).get("columns") or main_columns(cfg))
                if old is None:
                    eps.append(job)
                else:
                    eps[eps.index(old)] = {**old, **job, "window": old.get("window", job["window"]),
                                           "enabled": old.get("enabled", True)}
                if sid not in cur:
                    cur.append(sid)
        ids = [r.partition("@")[0] for r in refs]
    else:
        ids = [r.partition("@")[0] for r in refs]
        cur = [x for x in cur if x not in ids]
        names = {f"sprint-{x}" for x in ids}
        for e in [e for e in eps if e.get("name") in names]:
            try:
                os.unlink(job_path(cfg["outDir"] or jira_config.OUT_DIR_DEFAULT, e))
            except OSError:
                pass
            reset_job(e["name"])
        eps = [e for e in eps if e.get("name") not in names]
        for x in ids:
            cache.pop(x, None)
    jira_config.save({"pinnedSprints": cur, "endpoints": eps})
    jira_api.write_json(sprints_cache_path(), cache, mode=0o644)
    ok = op == "remove" or all(x in cur for x in ids)
    print(json.dumps({"ok": ok, "sprints": cur, "jobs": [f"sprint-{x}" for x in cur], "problems": problems}))
    return 0 if ok else 1


def boards_cache_path() -> str:
    return os.path.join(jira_api.CACHE_DIR, jira_config.BOARDS_FILE)


def main_columns(cfg) -> str:
    ep = next((e for e in cfg.endpoints if plain_issue_job(e) and e.get("name") == "all"),
              next((e for e in cfg.endpoints if plain_issue_job(e)), {}))
    return ep.get("columns") or ""


def edit_pinned_boards(op: str, ids: list) -> int:
    """--pin-board add|remove ID...: config.json pinnedBoards + one custom-jql
    job per board (board-ID: the board's filter JQL, scope ANDed in by the
    sync; scrum boards start at open sprints only) publishing into
    BOARD_DIR, + the board's columns / quick filters in boards.json. Add =
    3 requests per board, once. Prints {ok, boards, jobs}."""
    ids = [str(x).strip() for x in ids if str(x).strip()]
    if op not in ("add", "remove") or not ids:
        print(json.dumps({"ok": False, "problems": ["--pin-board add|remove ID..."]}))
        return 1
    try:
        cfg = jira_config.load()
    except jira_config.ConfigError as err:
        print(json.dumps({"ok": False, "problems": [str(err)]}))
        return 1
    cache = jira_api.read_json(boards_cache_path(), {})
    cache = cache if isinstance(cache, dict) else {}
    eps = [dict(e) for e in cfg.endpoints]
    cur = config_list(cfg, "pinnedBoards")
    problems = []
    if op == "add":
        d = jira_api.read_json(jira_api.DIRECTORY_FILE, {})
        status_ids = dict((d or {}).get("statusIds") or {})
        c = jira_api.Client.from_config(cfg)
        for bid in ids:
            try:
                info = board_info(c, bid, status_ids)
            except jira_api.ApiError as err:
                problems.append(f"board {bid}: {err}")
                continue
            if not info["jql"]:
                problems.append(f"board {bid}: no filter JQL readable")
                continue
            cache[bid] = info
            old = next((e for e in eps if e.get("name") == f"board-{bid}"), None)
            # re-pinning keeps the job's own schedule, columns and sprint switch;
            # a new board fetches every issue (the board view's sprint picker
            # narrows it, past sprints included)
            sprint = old.get("sprintOnly", False) if old else False
            job = board_job(bid, info["jql"], sprint, (old or {}).get("columns") or main_columns(cfg))
            if old is None:
                eps.append(job)
            else:
                if old.get("jql") != job["jql"]:
                    reset_job(job["name"])
                eps[eps.index(old)] = {**old, **job, "window": old.get("window", job["window"]),
                                       "enabled": old.get("enabled", True)}
            if bid not in cur:
                cur.append(bid)
    else:
        cur = [b for b in cur if b not in ids]
        names = {f"board-{b}" for b in ids}
        for e in [e for e in eps if e.get("name") in names]:
            try:
                os.unlink(job_path(cfg["outDir"] or jira_config.OUT_DIR_DEFAULT, e))
            except OSError:
                pass
            reset_job(e["name"])
        eps = [e for e in eps if e.get("name") not in names]
        for b in ids:
            cache.pop(b, None)
    jira_config.save({"pinnedBoards": cur, "endpoints": eps})
    jira_api.write_json(boards_cache_path(), cache, mode=0o644)
    ok = op == "remove" or all(b in cur for b in ids)
    print(json.dumps({"ok": ok, "boards": cur, "jobs": [f"board-{b}" for b in cur], "problems": problems}))
    return 0 if ok else 1


def board_sprint(args: list) -> int:
    """--board-sprint ID on|off: a board job's open-sprints-only switch."""
    if len(args) != 2 or args[1] not in ("on", "off"):
        print(json.dumps({"ok": False, "problems": ["--board-sprint ID on|off"]}))
        return 1
    cfg = jira_config.load()
    eps = [dict(e) for e in cfg.endpoints]
    ep = next((e for e in eps if e.get("name") == f"board-{args[0]}"), None)
    if not ep or not ep.get("boardJql"):
        print(json.dumps({"ok": False, "problems": [f"board {args[0]} is not pinned"]}))
        return 1
    on = args[1] == "on"
    ep.update(jql=board_job(args[0], ep["boardJql"], on, "")["jql"], sprintOnly=on)
    reset_job(ep["name"])
    jira_config.save({"endpoints": eps})
    print(json.dumps({"ok": True, "board": args[0], "sprintOnly": on}))
    return 0


def board_quickfilter(args: list) -> int:
    """--board-quickfilter ID QF...: the board job's JQL AND each picked quick
    filter's JQL (boards.json), inside the scope, fields=key -> {ok, keys}.
    The window narrows the board's rows to them (quick filters are JQL: the
    app can't evaluate them itself)."""
    if len(args) < 2:
        print(json.dumps({"ok": False, "problems": ["--board-quickfilter ID QF..."]}))
        return 1
    bid, qids = args[0], args[1:]
    try:
        cfg = jira_config.load()
        team = jira_config.load_team(cfg.data)
    except jira_config.ConfigError as err:
        print(json.dumps({"ok": False, "problems": [str(err)]}))
        return 1
    ep = next((e for e in cfg.endpoints if e.get("name") == f"board-{bid}"), None)
    info = (jira_api.read_json(boards_cache_path(), {}) or {}).get(bid) or {}
    qfs = {q.get("id"): q for q in info.get("quickFilters") or []}
    scope = jira_config.scope_projects(team)
    if not ep or not scope:
        print(json.dumps({"ok": False, "problems": [f"board {bid} is not pinned" if not ep else jira_config.NO_SCOPE]}))
        return 1
    clauses = [f"({ep['jql']})"] + [f"({strip_order(qfs[q]['jql'])})" for q in qids if q in qfs and qfs[q].get("jql")]
    clauses.append("project in (" + ", ".join(f'"{jira_config.jql_quote(p)}"' for p in scope) + ")")
    jql = " AND ".join(clauses)
    c = jira_api.Client.from_config(cfg)
    keys = []
    try:
        for got, _, _ in c.search_pages(jql, "key", max_total=int(ep.get("maxTotal") or 10000)):
            keys += [i.get("key") for i in got if i.get("key")]
    except jira_api.ApiError as err:
        print(json.dumps({"ok": False, "problems": [str(err)], "jql": jql}))
        return 1
    print(json.dumps({"ok": True, "keys": keys, "jql": jql}))
    return 0


def board_sprint_keys(args: list) -> int:
    """--board-sprint-keys BOARD SPRINT: the keys of the board's issues in one
    sprint (SPRINT = its id, or "current" = the board's open sprints), inside
    the scope, fields=key -> {ok, keys}. The board view narrows the board's
    rows to them (its job's rows carry no sprint field)."""
    if len(args) != 2:
        print(json.dumps({"ok": False, "problems": ["--board-sprint-keys BOARD SPRINT|current"]}))
        return 1
    bid, sprint = args
    try:
        cfg = jira_config.load()
        team = jira_config.load_team(cfg.data)
    except jira_config.ConfigError as err:
        print(json.dumps({"ok": False, "problems": [str(err)]}))
        return 1
    info = (jira_api.read_json(boards_cache_path(), {}) or {}).get(bid) or {}
    scope = jira_config.scope_projects(team)
    if not scope:
        print(json.dumps({"ok": False, "problems": [jira_config.NO_SCOPE]}))
        return 1
    if sprint == "current":
        which = "sprint in openSprints()"
    elif sprint.isdigit():
        which = f"sprint = {sprint}"
    else:
        print(json.dumps({"ok": False, "problems": [f"not a sprint: {sprint}"]}))
        return 1
    clauses = ([f"({info['jql']})"] if info.get("jql") else []) + [which]
    clauses.append("project in (" + ", ".join(f'"{jira_config.jql_quote(p)}"' for p in scope) + ")")
    jql = " AND ".join(clauses)
    c = jira_api.Client.from_config(cfg)
    keys = []
    try:
        for got, _, _ in c.search_pages(jql, "key", max_total=5000):
            keys += [i.get("key") for i in got if i.get("key")]
    except jira_api.ApiError as err:
        print(json.dumps({"ok": False, "problems": [str(err)], "jql": jql}))
        return 1
    print(json.dumps({"ok": True, "keys": keys, "jql": jql}))
    return 0


def edit_pinned_views(op: str, specs: list) -> int:
    """--pin-view add|remove BOARD|SPRINT|MODE...: board views pinned to the
    Jira sidebar (config.json pinnedBoardViews, pin order). SPRINT = a sprint
    id, current or all; MODE = columns or table. Prints {ok, views}."""
    specs = [x.strip() for x in specs if x.strip()]
    bad = [x for x in specs if len(x.split("|")) != 3]
    if op not in ("add", "remove") or not specs or bad:
        print(json.dumps({"ok": False, "problems": ["--pin-view add|remove BOARD|SPRINT|MODE..."]}))
        return 1
    try:
        cfg = jira_config.load()
    except jira_config.ConfigError as err:
        print(json.dumps({"ok": False, "problems": [str(err)]}))
        return 1
    cur = config_list(cfg, "pinnedBoardViews")
    new = (cur + [x for x in specs if x not in cur]) if op == "add" else [x for x in cur if x not in specs]
    jira_config.save({"pinnedBoardViews": new})
    print(json.dumps({"ok": True, "views": new}))
    return 0


def board_columns_empty(info) -> bool:
    cols = (info or {}).get("columns") or []
    return bool(cols) and not any(col.get("statuses") for col in cols)


def heal_board_columns(cfg) -> None:
    """A pinned board saved with columns but no statuses in any of them (it
    was pinned before the directory had status ids) is read again once per
    tick until it has them. Never fails the tick."""
    cache = jira_api.read_json(boards_cache_path(), {})
    if not isinstance(cache, dict):
        return
    bad = [b for b in config_list(cfg, "pinnedBoards") if board_columns_empty(cache.get(b))]
    if not bad:
        return
    status_ids = dict((jira_api.read_json(jira_api.DIRECTORY_FILE, {}) or {}).get("statusIds") or {})
    try:
        c = jira_api.Client.from_config(cfg)
        for bid in bad:
            info = board_info(c, bid, status_ids)
            if board_columns_empty(info):
                continue
            # keep anything the pin stored besides the board's own answer
            cache[bid] = {**cache.get(bid, {}), **info}
            say(f"board {bid}: columns re-read ({len(info['columns'])} columns)", "boards")
    except (jira_api.ApiError, jira_config.ConfigError) as err:
        say(f"board columns: {err}", "boards")
    jira_api.write_json(boards_cache_path(), cache, mode=0o644)


def refresh_board_catalog(cfg, team: dict, force: bool = False) -> None:
    """The poll tick keeps board_catalog.json (scope -> boards -> sprints, the
    Jira sidebar's BOARDS and each board's sprint picker) fresh: asked again
    every boardCatalogMinutes (60) or when the scope changed. Never fails the
    tick."""
    keys = jira_config.scope_projects(team)
    if not keys:
        return
    got = jira_api.read_json(board_catalog_path(), None)
    try:
        age = time.time() - os.path.getmtime(board_catalog_path())
    except OSError:
        age = None
    every = 60 * int(cfg.data.get("boardCatalogMinutes") or 60)
    if not force and isinstance(got, dict) and got.get("forProjects") == keys and age is not None and age < every:
        return
    try:
        out = board_catalog(jira_api.Client.from_config(cfg), keys)
    except (jira_api.ApiError, jira_config.ConfigError) as err:
        say(f"boards + sprints: {err}", "boards")
        return
    jira_api.write_json(board_catalog_path(), out, mode=0o644)
    say(f"boards + sprints: {out['boards']} board(s), {out['sprints']} sprint(s)", "boards")


def import_filters() -> int:
    """--import-filters: GET /filter/favourite -> a custom-jql job per filter
    (filter-ID, ORDER BY dropped; the sync ANDs the scope in). Existing
    filter jobs keep their schedule / columns; a changed JQL starts over.
    Prints {ok, added, updated, filters: [{id, name, job}]}."""
    try:
        cfg = jira_config.load()
    except jira_config.ConfigError as err:
        print(json.dumps({"ok": False, "problems": [str(err)]}))
        return 1
    c = jira_api.Client.from_config(cfg)
    try:
        got = c.get(c.path("favourite_filters")) or []
    except jira_api.ApiError as err:
        print(json.dumps({"ok": False, "problems": [str(err)]}))
        return 1
    eps = [dict(e) for e in cfg.endpoints]
    added, updated, out = 0, 0, []
    for f in got if isinstance(got, list) else []:
        fid, jql = str(f.get("id") or ""), strip_order(f.get("jql") or "")
        if not fid or not jql:
            continue
        name = f"filter-{fid}"
        title = re.sub(r"[^A-Za-z0-9 ._-]+", "", f.get("name") or name).strip() or name
        old = next((e for e in eps if e.get("name") == name), None)
        if old is None:
            eps.append({"name": name, "type": "issues", "jql": jql, "file": f"{title}.json",
                        "filterId": fid, "window": "30m", "projects": "*", "enabled": True,
                        "columns": main_columns(cfg)})
            added += 1
        elif old.get("jql") != jql:
            old["jql"] = jql
            reset_job(name)
            updated += 1
        out.append({"id": fid, "name": f.get("name") or "", "job": name})
    files = [e.get("file") for e in eps if e.get("file")]
    for e in eps:   # two filters with one title: the id keeps the files apart
        if e.get("filterId") and files.count(e.get("file")) > 1:
            e["file"] = f"{e['file'][:-5]} {e['filterId']}.json"
    jira_config.save({"endpoints": eps})
    print(json.dumps({"ok": True, "added": added, "updated": updated, "filters": out}))
    return 0


ME_FILE = jira_paths.cache_file("me")


def me(cfg) -> set:
    """The Jira user's names as the cache writes them (person(): Server
    username / Cloud display name), from /myself once, then cached."""
    path = ME_FILE
    got = jira_api.read_json(path, {})
    if not got:
        got = jira_api.Client.from_config(cfg).get("/rest/api/2/myself") or {}
        got = {k: got.get(k) for k in ("name", "displayName", "accountId", "emailAddress")}
        jira_api.write_json(path, got, mode=0o644)
    return {v for v in (got.get("name"), got.get("displayName")) if v}


MY_WORK = [tuple(v) for v in _DEFAULTS["my_work"]["views"]]
_WATCHING = _DEFAULTS["my_work"]["watching"]
WATCHING_JOB = _WATCHING["name"]


def ensure_watching_job(cfg) -> bool:
    """MY WORK ▸ Watching needs Jira (watcher = currentUser()): a custom-jql
    job writing MY_WORK_DIR/watching.json, added once. True = just added."""
    if any(e.get("name") == WATCHING_JOB for e in cfg.endpoints):
        return False
    eps = [dict(e) for e in cfg.endpoints] + [{
        "name": WATCHING_JOB, "type": _WATCHING["type"], "jql": _WATCHING["jql"],
        "file": _WATCHING["file"], "sideDir": jira_config.MY_WORK_DIR,
        "window": _WATCHING["window"], "projects": _WATCHING["projects"],
        "enabled": _WATCHING["enabled"], "columns": main_columns(cfg)}]
    jira_config.save({"endpoints": eps})
    return True


def my_work() -> int:
    """--my-work: the sidebar's MY WORK views, local filters over the issue
    cache (no request but /myself once) -> MY_WORK_DIR/<view>.json.
    Prints {ok, dir, views: [{id, title, file, count}]}."""
    try:
        cfg = jira_config.load()
        team = jira_config.load_team(cfg.data)
        names = me(cfg)
    except (jira_config.ConfigError, jira_api.ApiError) as err:
        print(json.dumps({"ok": False, "problems": [str(err)]}))
        return 1
    d = side_dir(cfg["outDir"] or jira_config.OUT_DIR_DEFAULT, jira_config.MY_WORK_DIR)
    main_ep = next((e for e in cfg.endpoints if plain_issue_job(e) and e.get("name") == "all"),
                   next((e for e in cfg.endpoints if plain_issue_job(e)), {}))
    _, pkeys = job_fields(main_ep, team)
    cache = jira_api.read_json(jira_api.CACHE_FILE, {})
    items = [e for e in (cache.values() if isinstance(cache, dict) else []) if isinstance(e, dict)]
    today = time.strftime("%Y-%m-%d")
    tests = {"mine": lambda e: e.get("assignee") in names,
             "reported": lambda e: e.get("reporter") in names,
             "today": lambda e: str(e.get("updated") or "").startswith(today)}
    views = []
    for vid, title in MY_WORK:
        rows = shape(sort_updated_desc([e for e in items if tests[vid](e)]), pkeys)
        jira_api.write_json(os.path.join(d, f"{vid}.json"), rows, mode=0o644)
        views.append({"id": vid, "title": title, "file": f"{vid}.json", "count": len(rows)})
    # Watching: its own job's file (polled; None = not fetched yet)
    added = ensure_watching_job(cfg)
    got = jira_api.read_json(os.path.join(d, "watching.json"), None)
    views.append({"id": "watching", "title": "Watching", "file": "watching.json", "job": WATCHING_JOB,
                  "count": len(got) if isinstance(got, list) else None, "jobAdded": added})
    print(json.dumps({"ok": True, "dir": d, "views": views}))
    return 0


def release_view_file(project: str, name: str) -> str:
    safe = "".join(ch if ch.isalnum() or ch in "._- " else "_" for ch in f"{project}-{name}")
    return safe.strip() + ".json"


def release_view(extra_keys: list) -> int:
    """--release-view [KEY...]: the Jira window's release view - one tab file
    per release (releases.json rows + the given blacklisted keys) holding the
    issues whose fix versions name it, from the issue cache, shaped with the
    main issue job's columns. Local only (no request, no lock). Stale files
    are removed. Prints {ok, dir, releases: [{key, file, count}]}."""
    try:
        cfg = jira_config.load()
        team = jira_config.load_team(cfg.data)
    except jira_config.ConfigError as err:
        print(json.dumps({"ok": False, "problems": [str(err)]}))
        return 1
    out_dir = os.path.expanduser(cfg["outDir"] or jira_config.OUT_DIR_DEFAULT)
    view_dir = os.path.join(os.path.dirname(out_dir.rstrip("/")), jira_config.RELEASE_VIEW_DIR)
    os.makedirs(view_dir, exist_ok=True)
    rels = []
    for e in cfg.endpoints:
        if e.get("type") == "releases" and e.get("file"):
            got = jira_api.read_json(os.path.join(out_dir, e["file"]), [])
            rels += [r for r in (got if isinstance(got, list) else []) if isinstance(r, dict)]
    hidden = jira_api.read_json(os.path.join(out_dir, jira_config.BLACKLIST_RELEASE_FILE), [])
    rels += [r for r in (hidden if isinstance(hidden, list) else [])
             if isinstance(r, dict) and r.get("key") in set(extra_keys)]
    main_ep = next((e for e in cfg.endpoints if plain_issue_job(e) and e.get("name") == "all"),
                   next((e for e in cfg.endpoints if plain_issue_job(e)), {}))
    _, pkeys = job_fields(main_ep, team)
    cache = jira_api.read_json(jira_api.CACHE_FILE, {})
    cache = cache if isinstance(cache, dict) else {}
    by_release: dict = {}
    for e in cache.values():
        if not isinstance(e, dict):
            continue
        for n in str(e.get("release") or "").split(","):
            by_release.setdefault((e.get("project") or "", n.strip()), []).append(e)
    out, written, seen = [], set(), set()
    for r in rels:
        key = r.get("key") or ""
        if key in seen:
            continue
        seen.add(key)
        proj, name = r.get("project") or "", r.get("release") or r.get("title") or ""
        items = shape(sort_updated_desc(by_release.get((proj, name), [])), pkeys)
        f = release_view_file(proj, name)
        jira_api.write_json(os.path.join(view_dir, f), items, mode=0o644)
        written.add(f)
        out.append({"key": key, "file": f, "count": len(items)})
    for f in os.listdir(view_dir):
        if f.endswith(".json") and f not in written:
            try:
                os.unlink(os.path.join(view_dir, f))
            except OSError:
                pass
    print(json.dumps({"ok": True, "dir": view_dir, "releases": out}))
    return 0


def main(argv: list) -> int:
    global QUIET
    o = parse_args(argv)
    QUIET = o["quiet"]
    if o["release_view"] is not None:
        return release_view(o["release_view"])
    if o["label_view"] is not None:
        return label_view(o["label_view"])
    if o.get("pin_label"):
        return edit_pinned_labels(*o["pin_label"])
    if o.get("pin_board"):
        return edit_pinned_boards(*o["pin_board"])
    if o.get("pin_sprint"):
        return edit_pinned_sprints(*o["pin_sprint"])
    if o.get("sprints"):
        return list_sprints(o["sprints"])
    if o.get("pin_view"):
        return edit_pinned_views(*o["pin_view"])
    if o.get("board_sprint_keys") is not None:
        return board_sprint_keys(o["board_sprint_keys"])
    if o.get("board_catalog"):
        return run_board_catalog(bool(o.get("cached")))
    if o.get("board_quick") is not None:
        return board_quickfilter(o["board_quick"])
    if o.get("board_sprint") is not None:
        return board_sprint(o["board_sprint"])
    if o.get("import_filters"):
        return import_filters()
    if o.get("my_work"):
        return my_work()
    if o["favorite"]:
        return edit_pins("favorites", *o["favorite"])
    if o["blacklist"]:
        return edit_pins("releases", *o["blacklist"])
    if o.get("favorite_release"):
        return edit_favorite_releases(*o["favorite_release"])
    if o["cancel"]:
        return cancel_running()
    if o["live"]:
        return live_search(o["dry"])
    if o["directory"]:
        try:
            names = [e["name"] for e in jira_config.load().endpoints if e.get("type") == "directory"]
        except jira_config.ConfigError as err:
            print(f"jira-poll: {err}", file=sys.stderr)
            return 2
        if not names:
            print("jira-poll: no directory job in config.json (add one: type directory)",
                  file=sys.stderr)
            return 2
        o["projects"] = ",".join(names)
        o["force"] = True
    if o["rebuild"]:
        # the flag first: if another poll holds the lock, the next tick rebuilds
        jira_config.save({"rebuildOnNextPoll": True})
        o["force"] = True
    if o["setup"]:
        o["force"] = True
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
            d["problems"] = cfg.problems() + bad + [f"team: {x}" for x in jira_config.team_problems(team)] \
                + jira_config.scope_problems(cfg.endpoints, team) \
                + ([] if jira_config.scope_projects(team) else [jira_config.NO_SCOPE])
        except jira_config.ConfigError as err:
            d = {"problems": [str(err)], "endpoints": [], "columns": []}
        print(json.dumps(d, indent=2))
        return 0
    enabled = jira_config.jira_enabled()
    active = jira_config.poll_active()
    base = {"script": jira_status.POLL_SCRIPT, "curlLog": jira_api.CURL_LOG, "pollLog": POLL_LOG,
            "debugLog": jira_log.DEBUG_LOG,
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
        say(str(err))
        return 2
    base["configNotes"] = cfg.notes
    if not o["dry"]:
        label = "setup" + (f"-{o['step']}" if o["step"] else "") if o["setup"] else \
            ("rebuild" if o["rebuild"] else o["projects"].replace(",", "+") if o["projects"] else "tick")
        jira_log.setup(label, ["jira_poll.py", *argv], cfg)
        base["rawDir"] = jira_log.RAW.dir if jira_log.RAW else ""
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
        say(msg)
        return 2
    setup = jira_config.setup_state(cfg.data)
    scheduled = not (o["force"] or o["projects"] or o["init"] or o["window"])
    if setup.get("state") != "done" and scheduled and not o["dry"]:
        jira_status.update(lambda d: set_base(d, status="setup pending", lastCheck=jira_status.now_str(),
                                              lastError=""))
        return 0
    if not jira_config.scope_projects(team) and not o["setup"]:
        msg = jira_config.NO_SCOPE
        jira_status.update(lambda d: set_base(d, status="error", lastError=msg,
                                              lastCheck=jira_status.now_str()))
        say(msg)
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
        if o["setup"]:
            jira_status.update(lambda d: set_base(d, lock={"held": True, "pid": os.getpid(),
                                                           "since": lock.since}))
            return run_setup(o, cfg, team)
        return poll(o, cfg, team, base, set_base, lock)
    except Cancelled:
        def mark(d):
            for e in d.get("endpoints") or []:
                if e.get("status") == "running":
                    e["status"] = "cancelled"
                    e["lastError"] = "cancelled by user (progress kept - the next run resumes)"
            set_base(d, status="cancelled", lastError="poll cancelled",
                     lastRun=jira_status.now_str())
        jira_status.update(mark)
        if o["setup"]:
            st = jira_config.setup_state(jira_config.load().data)
            steps = {k: (dict(v, state="cancelled") if (v or {}).get("state") == "running" else v)
                     for k, v in (st.get("steps") or {}).items()}
            save_setup({**st, "steps": steps})
        say("cancelled (progress kept - the next run resumes)")
        return 130
    finally:
        lock.release()
        if not o["dry"]:
            jira_status.update(lambda d: d.update(lock={"held": False, "pid": None, "since": None},
                                                  progress=None))


def poll(o: dict, cfg, team: dict, base: dict, set_base, lock: Lock) -> int:
    status = jira_status.read()
    now = time.time()
    rebuild = cfg["rebuildOnNextPoll"]
    if rebuild and not o["dry"]:
        if rebuild is True:
            wipe_cache()
            jira_config.save({"rebuildOnNextPoll": "resume"})
            say("rebuild: issue cache, resume points and versions wiped - re-populating everything",
                "rebuild")
            status = jira_status.read()
        else:
            say("rebuild: continuing the interrupted rebuild", "rebuild")
        o["init"] = True
        o["projects"] = "*"
    names = [n.strip() for n in o["projects"].split(",") if n.strip()] if o["projects"] else []
    forced = bool(names) or o["init"] or bool(o["window"])
    # "*" = every enabled job; "all" too, unless a job is literally named "all"
    everything = "*" in names or ("all" in names and not cfg.endpoint("all"))
    unknown = [n for n in names if n not in ("*", "all", SYNC) and not cfg.endpoint(n)]
    if unknown:
        print(f"jira-poll: unknown endpoint(s): {', '.join(unknown)} "
              f"(known: {', '.join(e['name'] for e in cfg.endpoints)})", file=sys.stderr)
        return 2
    plan = []
    for ep in cfg.endpoints:
        wanted = (not names or everything) and ep.get("enabled", True) or ep["name"] in names
        entry = jira_status.endpoint_entry(status, ep["name"])
        wsec = jira_config.parse_window(ep.get("window", "10m"))
        nxt = next_run(entry, wsec)
        due = wanted and (forced or nxt is None or now >= nxt)
        if due:
            plan.append((ep, entry, wsec))
    ctx = Ctx(o, cfg, team)
    sync_due = SYNC in names or any(plain_issue_job(ep) for ep, _, _ in plan)
    others = [(ep, entry, wsec) for ep, entry, wsec in plan if not plain_issue_job(ep)]
    fields = jira_config.api_fields(team=team)
    pkeys = jira_config.publish_keys()

    if o["dry"]:
        print(f"config:  {cfg.path} ({cfg.source})")
        print(f"enabled: {base['enabled']}  (commands.toml [jira])")
        print(f"scope:   {', '.join(jira_config.scope_projects(team)) or '(none)'}")
        print(f"setup:   {jira_config.setup_state(cfg.data).get('state')}")
        print(f"fields:  {','.join(fields)}")
        print(f"publish: {','.join(pkeys)}")
        if ctx.plain:
            print(f"  {'(sync)':<10} {'issues':<8} every {fmt_secs(ctx.sync_wsec()):<4} "
                  f"{'DUE' if sync_due else 'not due':<8} window={ctx.sync_window(status):<17} "
                  f"-> {jira_api.CACHE_FILE} ({', '.join(ctx.sync_projects())})")
        for ep in cfg.endpoints:
            entry = jira_status.endpoint_entry(status, ep["name"])
            hit = any(p[0] is ep for p in plan)
            w = "shared" if plain_issue_job(ep) else \
                choose_window(ep["name"], entry, o, ctx.margin, needs_full=True) \
                if ep.get("type", "issues") == "issues" else "-"
            print(f"  {ep['name']:<10} {ep.get('type', 'issues'):<8} every {ep.get('window'):<4} "
                  f"{'DUE' if hit else 'not due':<8} window={w:<17} -> {ep_path(ep, ctx.out_dir)}")
        return 0

    def mark_running(d):
        set_base(d, lock={"held": True, "pid": os.getpid(), "since": lock.since},
                 lastCheck=jira_status.now_str())
        # an enabled tick replaces a stale "disabled"/config-error state even
        # when nothing is due (the endpoints' own results follow below)
        if d.get("status") in (None, "disabled", "setup pending") or \
                str(d.get("lastError", "")).startswith("config"):
            d["status"] = "idle"
            d["lastError"] = ""
        known = {e["name"] for e in cfg.endpoints} | {SYNC}
        d["endpoints"] = [e for e in d.get("endpoints", []) if e.get("name") in known]
        for ep in cfg.endpoints:
            e = jira_status.endpoint_entry(d, ep["name"])
            wsec = jira_config.parse_window(ep.get("window", "10m"))
            e.update({"type": ep.get("type", "issues"), "window": ep.get("window"),
                      "enabled": ep.get("enabled", True), "file": ep.get("file", ""),
                      "path": ep_path(ep, ctx.out_dir)})
            if any(p[0] is ep for p in plan) or (sync_due and ep in ctx.plain):
                e["status"] = "running"
            nx = next_run(e, wsec)
            e["nextRun"] = jira_status.now_str(nx) if nx else "due"
        if sync_due:
            jira_status.endpoint_entry(d, SYNC).update(status="running", type="sync")
    jira_status.update(mark_running)

    if not names:
        refresh_board_catalog(cfg, team)
        heal_board_columns(cfg)
    if not plan and not sync_due:
        say("nothing due")
        jira_status.update(lambda d: None)
        return 0

    ctx.c = c = jira_api.Client.from_config(cfg)
    jira_log.describe_config(cfg, team)
    failures = []
    if sync_due and ctx.plain:
        window = ctx.sync_window(status)
        started = jira_status.now_str()
        res, err, curl = attempt(lambda: logged(SYNC, c, lambda: run_shared_sync(ctx, window)))
        if err:
            say(f"failed: {err} - progress kept; the next run resumes", SYNC)
        counts = publish_plain(ctx)     # partial data is still better than none
        record(SYNC, started, window, err, curl, (res or {}).get("total"), ctx.sync_wsec(),
               extra=None if err else {"syncedProjects": ctx.sync_projects(),
                                       "syncedFields": getattr(ctx, "filled", None) or ctx.sync_fields()})
        for ep in ctx.plain:
            record(ep["name"], started, window, err, curl, counts.get(ep["name"]),
                   jira_config.parse_window(ep.get("window", "10m")))
        if err:
            failures.append(f"issue cache: {err}")
    for ep, entry, wsec in others:
        window = choose_window(ep["name"], entry, o, ctx.margin, needs_full=True) \
            if ep.get("type", "issues") == "issues" else "-"
        started = jira_status.now_str()
        say(f"window={window}", ep["name"])
        items, err, curl = attempt(lambda: logged(ep["name"], c, lambda: run_job(ctx, ep, window)))
        if err:
            say(f"failed: {err}", ep["name"])
            failures.append(f"{ep['name']}: {err}")
        record(ep["name"], started, window, err, curl, items, wsec)
    final = "error" if failures else "ok"
    last_err = "; ".join(failures)
    if rebuild and not failures:
        jira_config.save({"rebuildOnNextPoll": False})
        say("rebuild complete", "rebuild")
    jira_status.update(lambda d: set_base(d, lastRun=jira_status.now_str(), status=final,
                                          lastError=last_err, requests=c.requests,
                                          retries=c.retries, rateLimitWaited=int(c.waited)))
    # the legacy poll-state keeps the old doctor / tooling working
    try:
        cache = jira_api.read_json(jira_api.CACHE_FILE, {})
        with open(LEGACY_POLL_STATE, "w", encoding="utf-8") as fh:
            fh.write(f"LAST_POLL={jira_status.now_str()}\nWINDOW=per-endpoint\nSTATUS={final}\n"
                     f"ITEMS={len(cache)}\nOUTPUTS={len(plan)}\nERROR={last_err}\n")
    except OSError:
        pass
    say(f"poll complete: {len(plan)} job(s), status={final}, {c.requests} request(s)"
        + (f", {c.retries} retried, {fmt_secs(c.waited)} waited on rate limits" if c.retries else ""))
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
