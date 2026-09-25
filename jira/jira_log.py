"""jira_log.py - where the poller IS and what Jira actually answered.

debug.log  ~/.cache/jira/debug.log (python `logging`, rotated 10MB x 5).
           One line per event, each tagged with the run and the stage path:
             2026-09-25 12:27:03.412 DEBUG run=20260925-122703-889 [directory/labels/KAN]
                 jira_api.get:262  GET https://... -> 200 1.84s 48,210B raw=0142
           `stage(name)` brackets a unit of work: `▶ name` on entry, `◀ name
           done in 42.1s (37 requests, 812 items)` on exit, `✗ name failed
           after ...` + the traceback on error. poll.log stays the short
           summary; everything said there is here too.

raw/       ~/.cache/jira/raw/<run>-<label>/ (0700): EVERY response exactly as
           received, before any parsing - including 401 / 5xx bodies:
             NNNN-<endpoint>-<code>.json|.body   the body (.body = not JSON,
                                                 e.g. an SSO login page)
             NNNN-<endpoint>-<code>.meta.json    url, attempt, http code,
                                                 curl exit/stderr, timing,
                                                 ALL response headers, repro
             manifest.jsonl                      one line per request
           config.json rawCapture (on), rawKeepDays (3), rawMaxMB (2048).

Nothing here ever holds the token: registered secrets are scrubbed from
every formatted line, request URLs never carry it.
"""
from __future__ import annotations

import contextlib
import contextvars
import json
import logging
import logging.handlers
import os
import re
import shutil
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import jira_config  # noqa: E402

DEBUG_LOG = os.path.join(jira_config.CACHE_DIR, "debug.log")
DEBUG_LOG_MAX = 10 * 1024 * 1024
DEBUG_LOG_BACKUPS = 5
RAW_DIR = os.path.join(jira_config.CACHE_DIR, "raw")

LOG = logging.getLogger("jira")
LOG.addHandler(logging.NullHandler())   # library use (tests, --describe): silent
LOG.propagate = False

RUN_ID = ""
RAW: "RawStore | None" = None           # set by setup(); None = no capture
_SECRETS: set = set()
_WHERE = contextvars.ContextVar("jira_where", default="")


def secret(value: str) -> None:
    """Never let `value` (the token) reach debug.log."""
    if value and len(value) >= 4:
        _SECRETS.add(value)


class _Where(logging.Filter):
    def filter(self, record):
        record.run = RUN_ID or "-"
        record.where = _WHERE.get() or "-"
        return True


class _Formatter(logging.Formatter):
    def format(self, record):
        out = super().format(record)
        for s in _SECRETS:
            out = out.replace(s, "***")
        return out


def setup(label: str, argv: list | None = None, cfg=None) -> str:
    """Open debug.log (+ the raw store) for this process. Idempotent."""
    global RUN_ID, RAW
    if RUN_ID:
        return RUN_ID
    RUN_ID = time.strftime("%Y%m%d-%H%M%S") + f"-{os.getpid()}"
    level = "DEBUG"
    if cfg is not None:
        level = str(cfg["logLevel"] or "DEBUG").upper()
        secret(cfg["token"] or "")
    try:
        os.makedirs(jira_config.CACHE_DIR, exist_ok=True)
        h = logging.handlers.RotatingFileHandler(DEBUG_LOG, maxBytes=DEBUG_LOG_MAX,
                                                 backupCount=DEBUG_LOG_BACKUPS, encoding="utf-8")
        os.chmod(DEBUG_LOG, 0o600)
    except OSError:
        return RUN_ID
    h.setFormatter(_Formatter("%(asctime)s.%(msecs)03d %(levelname)-5s run=%(run)s [%(where)s] "
                              "%(module)s.%(funcName)s:%(lineno)d  %(message)s", "%Y-%m-%d %H:%M:%S"))
    h.addFilter(_Where())
    LOG.addHandler(h)
    LOG.setLevel(getattr(logging, level, logging.DEBUG))
    LOG.info("════ %s  (python %s, pid %d)  argv: %s", label, sys.version.split()[0], os.getpid(),
             " ".join(argv or []) or "(none)")
    if cfg is not None and cfg["rawCapture"] is not False:
        RAW = RawStore(label)
        RAW.prune(float(cfg["rawKeepDays"] if cfg["rawKeepDays"] is not None else 3),
                  float(cfg["rawMaxMB"] or 2048))
    return RUN_ID


def describe_config(cfg, team: dict) -> None:
    """The settings this run works with (never the token)."""
    if not LOG.isEnabledFor(logging.INFO):
        return
    LOG.info("config: %s  site=%s auth=%s email=%s token=%s", cfg.path, cfg.site, cfg.auth,
             cfg["email"] or "-", "set" if cfg["token"] else "MISSING")
    LOG.info("config: scope=%s  rateLimitMaxWaitMinutes=%s requestDelayMs=%s checkpointEvery=%s "
             "fetchComments=%s", ",".join(jira_config.scope_projects(team)) or "(none)",
             cfg["rateLimitMaxWaitMinutes"], cfg["requestDelayMs"], cfg["checkpointEvery"],
             cfg["fetchComments"])
    LOG.info("config: search_defaults=%s", json.dumps(team.get("search_defaults") or {}, sort_keys=True))
    for e in cfg.endpoints:
        LOG.info("config: job %s type=%s window=%s projects=%s enabled=%s%s%s", e.get("name"),
                 e.get("type", "issues"), e.get("window"), e.get("projects", "*"), e.get("enabled", True),
                 f" maxResults={e['maxResults']}" if e.get("maxResults") else "",
                 f" jql={e['jql']!r}" if e.get("jql") else "")
    if RAW:
        LOG.info("raw responses -> %s", RAW.dir)


def where() -> str:
    return _WHERE.get()


class Stage:
    items = None      # set inside the block: "N items" on the ◀ line
    note = ""         # extra text for the ◀ line


@contextlib.contextmanager
def stage(name: str, detail: str = "", c=None):
    """▶ name ... ◀ name done in Xs (N requests, M items) / ✗ name failed.
    Nested stages form the [a/b/c] path on every line logged inside."""
    parent = _WHERE.get()
    token = _WHERE.set(f"{parent}/{name}" if parent else name)
    st = Stage()
    t0, r0 = time.time(), getattr(c, "requests", None)
    LOG.info("▶ %s%s", name, f" - {detail}" if detail else "", stacklevel=3)
    try:
        yield st
    except BaseException as err:
        el = time.time() - t0
        if isinstance(err, Exception) and not getattr(err, "_jira_logged", False):
            LOG.error("✗ %s failed after %.1fs: %s: %s", name, el, type(err).__name__, err,
                      exc_info=True, stacklevel=3)
            try:
                err._jira_logged = True
            except AttributeError:
                pass
        else:
            LOG.error("✗ %s failed after %.1fs (%s)", name, el, type(err).__name__, stacklevel=3)
        raise
    else:
        bits = []
        if r0 is not None:
            bits.append(f"{c.requests - r0} request(s)")
        if st.items is not None:
            bits.append(f"{st.items:,} item(s)" if isinstance(st.items, int) else str(st.items))
        if st.note:
            bits.append(st.note)
        LOG.info("◀ %s done in %.1fs%s", name, time.time() - t0,
                 f" ({', '.join(bits)})" if bits else "", stacklevel=3)
    finally:
        _WHERE.reset(token)


# ------------------------------------------------------------ raw responses

def slug(url: str) -> str:
    """https://site/rest/api/2/user/assignable/search?x -> user_assignable_search"""
    path = re.sub(r"^https?://[^/]+", "", url).split("?")[0]
    path = re.sub(r"^/rest/(api|agile)/[^/]+/", "", path)
    return re.sub(r"[^A-Za-z0-9]+", "_", path).strip("_")[:60] or "root"


def parse_headers(path: str) -> tuple:
    """curl -D file -> (status line, [[name, value], ...]) of the LAST response
    (a 100-continue / redirect writes several blocks)."""
    try:
        with open(path, encoding="utf-8", errors="replace") as fh:
            text = fh.read()
    except OSError:
        return "", []
    blocks = [b for b in re.split(r"\r?\n\r?\n", text) if b.strip()]
    if not blocks:
        return "", []
    lines = blocks[-1].splitlines()
    return lines[0].strip(), [[k.strip(), v.strip()] for k, _, v in (ln.partition(":") for ln in lines[1:]) if k]


class RawStore:
    """One run's folder of raw responses (created on the first request)."""

    def __init__(self, label: str):
        self.dir = os.path.join(RAW_DIR, f"{RUN_ID}-{re.sub(r'[^A-Za-z0-9_.-]+', '_', label)[:40]}")
        self.seq = 0
        self.made = False

    def _mkdir(self) -> bool:
        if not self.made:
            try:
                os.makedirs(self.dir, mode=0o700, exist_ok=True)
                os.chmod(RAW_DIR, 0o700)
                self.made = True
            except OSError:
                return False
        return True

    def next_headers(self) -> str | None:
        """The -D file for the next request (read back by save())."""
        if not self._mkdir():
            return None
        return os.path.join(self.dir, f"{self.seq + 1:04d}.headers.tmp")

    def save(self, *, method: str, url: str, attempt: int, code: int, curl_exit: int, stderr: str,
             elapsed: float, body: str, headers_file: str | None, repro: str) -> str:
        """-> the body file's path ('' when capture failed)."""
        if not self._mkdir():
            return ""
        self.seq += 1
        status, headers = parse_headers(headers_file) if headers_file else ("", [])
        if headers_file:
            try:
                os.unlink(headers_file)
            except OSError:
                pass
        stem = f"{self.seq:04d}-{slug(url)}-{code if curl_exit == 0 else f'curl{curl_exit}'}"
        ext = ".json" if body.lstrip()[:1] in ("{", "[") else ".body"
        meta = {"seq": self.seq, "time": time.strftime("%Y-%m-%d %H:%M:%S"), "stage": where(),
                "method": method, "url": url, "attempt": attempt, "httpCode": code,
                "curlExit": curl_exit, "curlStderr": stderr.strip(), "elapsedSeconds": round(elapsed, 3),
                "bytes": len(body.encode("utf-8", errors="replace")), "body": stem + ext,
                "statusLine": status, "headers": headers, "repro": repro}
        try:
            for name, text in ((stem + ext, body), (stem + ".meta.json", json.dumps(meta, indent=2))):
                fd = os.open(os.path.join(self.dir, name), os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
                with os.fdopen(fd, "w", encoding="utf-8") as fh:
                    fh.write(text)
            with open(os.path.join(self.dir, "manifest.jsonl"), "a", encoding="utf-8") as fh:
                fh.write(json.dumps({k: meta[k] for k in ("seq", "time", "stage", "method", "url",
                                                          "attempt", "httpCode", "curlExit", "bytes",
                                                          "elapsedSeconds", "body")}) + "\n")
        except OSError:
            return ""
        return os.path.join(self.dir, stem + ext)

    def prune(self, keep_days: float, max_mb: float) -> None:
        """Drop run folders older than keep_days, then the oldest until raw/
        fits in max_mb (this run's folder is never touched)."""
        try:
            runs = sorted((os.path.join(RAW_DIR, d) for d in os.listdir(RAW_DIR)
                           if os.path.isdir(os.path.join(RAW_DIR, d))), key=os.path.getmtime)
        except OSError:
            return
        runs = [r for r in runs if r != self.dir]
        cutoff = time.time() - keep_days * 86400

        def size(d):
            return sum(os.path.getsize(os.path.join(root, f)) for root, _, fs in os.walk(d) for f in fs)

        sizes = {r: size(r) for r in runs}
        total = sum(sizes.values())
        for r in runs:
            if os.path.getmtime(r) >= cutoff and total <= max_mb * 1024 * 1024:
                continue
            shutil.rmtree(r, ignore_errors=True)
            total -= sizes[r]
            LOG.debug("raw: pruned %s", os.path.basename(r))


def latest_raw_dir() -> str:
    """The newest run folder (the Jira Config window's "Raw responses")."""
    try:
        runs = [os.path.join(RAW_DIR, d) for d in os.listdir(RAW_DIR)
                if os.path.isdir(os.path.join(RAW_DIR, d))]
    except OSError:
        return ""
    return max(runs, key=os.path.getmtime) if runs else ""
