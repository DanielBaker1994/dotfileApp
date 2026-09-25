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
     releases: every version of the projects -> <file>
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

Honors commands.conf [jira] enabled (THE SWITCH): disabled -> no network,
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
import signal
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import jira_api  # noqa: E402
import jira_config  # noqa: E402
import jira_log  # noqa: E402
import jira_status  # noqa: E402

LEGACY_POLL_STATE = os.path.join(jira_config.CACHE_DIR, "poll-state")
KEYS_DIR = os.path.join(jira_config.CACHE_DIR, "endpoints")
FIELDS_SEEN = os.path.join(jira_config.CACHE_DIR, "fields_seen.json")
POLL_LOG = os.path.join(jira_config.CACHE_DIR, "poll.log")
POLL_LOG_MAX = 2 * 1024 * 1024      # rotate: keep the newest half past 2MB
ERROR_RETRY = 300       # a failed endpoint is retried (resumed) after min(window, 5m)
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

    def wait(self, msg: str, secs: float) -> None:
        say(msg, self.job)
        reason = msg.split(" - ")[0]
        jira_status.set_progress(job=self.job, stage=self.label, waitingUntil=time.time() + secs,
                                 reason=reason, message=f"{self.label}: {reason} - resuming in {fmt_secs(secs)}")


def parse_args(argv: list) -> dict:
    o = {"init": False, "window": "", "projects": "", "dry": False, "quiet": False,
         "force": False, "describe": False, "cancel": False, "live": False, "directory": False,
         "rebuild": False, "setup": False, "step": ""}
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


def job_fields(ep: dict, team: dict) -> tuple:
    """(api fields, publish keys) from THIS job's own columns."""
    spec = jira_config.job_columns(ep)
    return (jira_config.api_fields(team=team, columns=spec),
            jira_config.publish_keys(columns=spec))


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


def run_shared_sync(ctx: Ctx, window: str) -> dict:
    """ONE streamed sync for every plain issue job, then publish them all.
    A project added to the scope since the last sync is fully synced first
    (an incremental window would only bring its recently updated tickets)."""
    c, cfg = ctx.c, ctx.cfg
    projects = ctx.sync_projects()
    if not projects:
        raise jira_config.ConfigError(jira_config.NO_SCOPE)
    page, cap = ctx.sync_limits()
    have = synced_projects()
    added = [p for p in projects if have is not None and p not in have] if window != "full" else []
    if added:
        rep = Reporter(SYNC + "-added", "New projects")
        c.on_wait = rep.wait
        rep.start(f"full sync of the projects added to the scope: {', '.join(added)}")
        jira_api.sync(c, "full", projects=added, api_fields=ctx.sync_fields(),
                      fetch_comments=bool(cfg["fetchComments"]), snapshot_keep=0, quiet=True,
                      default_projects=False, page_size=page, max_total=cap, name=SYNC + "-added",
                      checkpoint_every=int(cfg["checkpointEvery"] or 500), progress=rep.page,
                      flushed=lambda keys: publish_plain(ctx, quiet=True))
    rep = Reporter(SYNC, "Issue cache")
    c.on_wait = rep.wait
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
    path = os.path.join(ctx.out_dir, ep["file"])
    if typ == "releases":
        rep.start(f"versions of {', '.join(plist)}")
        items = release_items(jira_api.releases(c, projects=plist))
        publish(path, items, False)
        return len(items)
    # custom jql: its own streamed search (its key set = its rows)
    kf = keys_file(ep["name"])
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
               extra={"syncedProjects": ctx.sync_projects()})
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


def main(argv: list) -> int:
    global QUIET
    o = parse_args(argv)
    QUIET = o["quiet"]
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
        print(f"enabled: {base['enabled']}  (commands.conf [jira])")
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
               extra=None if err else {"syncedProjects": ctx.sync_projects()})
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
