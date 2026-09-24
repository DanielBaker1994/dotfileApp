#!/usr/bin/env python3
"""jira_status.py - the machine-readable state of the jira poller.

~/.cache/jira/status.json is rewritten atomically (temp + rename) at the end
of EVERY jira_poll.py invocation - even a no-op "nothing is due" tick - so the
menu bar, jira-doctor and a human with `cat` all see the same truth:

  script / config / curlLog paths, enabled, lock holder, lastRun, status,
  lastError, configNotes, lastSkipped, and per endpoint: window, lastRun,
  lastSuccess, nextRun, status, items, file, lastError.

Read-modify-write goes through a tiny flock on status.lock so a poll and a
skipped (locked-out) invocation never clobber each other's fields.

CLI:
  jira_status.py            human summary (paths, last/next run per endpoint)
  jira_status.py --json     raw status.json
  jira_status.py --path     print the status.json path
  jira_status.py --note-error MSG   record a menu-bar enable failure (login
                            test failed / config invalid) so the doctor and
                            the menu show why jira stayed disabled
"""
from __future__ import annotations

import contextlib
import fcntl
import json
import os
import sys
import tempfile
import time

HOME = os.path.expanduser("~")
CACHE_DIR = os.environ.get("JIRA_CACHE_DIR") or os.path.join(HOME, ".cache/jira")
STATUS_FILE = os.path.join(CACHE_DIR, "status.json")
STATUS_LOCK = os.path.join(CACHE_DIR, "status.lock")
CURL_LOG = os.path.join(CACHE_DIR, "curl.log")
POLL_LOCK = os.path.join(CACHE_DIR, "poll.lock")
POLL_SCRIPT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "jira_poll.py")

TIME_FMT = "%Y-%m-%d %H:%M:%S"


def now_str(t: float | None = None) -> str:
    return time.strftime(TIME_FMT, time.localtime(time.time() if t is None else t))


def parse_time(s: str) -> float | None:
    try:
        return time.mktime(time.strptime(s, TIME_FMT))
    except (TypeError, ValueError):
        return None


def read() -> dict:
    try:
        with open(STATUS_FILE, encoding="utf-8") as fh:
            d = json.load(fh)
        return d if isinstance(d, dict) else {}
    except (OSError, ValueError):
        return {}


def _write(d: dict) -> None:
    os.makedirs(CACHE_DIR, exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=CACHE_DIR, prefix=".status.json.")
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as fh:
            json.dump(d, fh, indent=2)
            fh.write("\n")
        os.chmod(tmp, 0o644)
        os.replace(tmp, STATUS_FILE)
    except BaseException:
        with contextlib.suppress(OSError):
            os.unlink(tmp)
        raise


@contextlib.contextmanager
def _locked():
    os.makedirs(CACHE_DIR, exist_ok=True)
    with open(STATUS_LOCK, "a") as fh:
        fcntl.flock(fh, fcntl.LOCK_EX)
        try:
            yield
        finally:
            fcntl.flock(fh, fcntl.LOCK_UN)


def update(fn) -> dict:
    """fn(status_dict) mutates in place; the result is written atomically."""
    with _locked():
        d = read()
        fn(d)
        d.setdefault("script", POLL_SCRIPT)
        d.setdefault("curlLog", CURL_LOG)
        d["updatedAt"] = now_str()
        _write(d)
        return d


def endpoint_entry(d: dict, name: str) -> dict:
    eps = d.setdefault("endpoints", [])
    for e in eps:
        if e.get("name") == name:
            return e
    e = {"name": name}
    eps.append(e)
    return e


def summary(d: dict) -> str:
    if not d:
        return f"no status yet ({STATUS_FILE} missing - the poller has never run)"
    lock = d.get("lock") or {}
    lines = [
        f"script:   {d.get('script', POLL_SCRIPT)}",
        f"config:   {d.get('config', '?')}",
        f"curl log: {d.get('curlLog', CURL_LOG)}",
        f"status:   {d.get('status', '?')}  (enabled={d.get('enabled')}, last run {d.get('lastRun', 'never')})",
    ]
    if lock.get("held"):
        lines.append(f"lock:     held by pid {lock.get('pid')} since {lock.get('since')}")
    if d.get("lastError"):
        lines.append(f"error:    {d['lastError']}")
    if d.get("enableError"):
        lines.append(f"enable:   failed {d['enableError'].get('at')}: {d['enableError'].get('message')}")
    for n in d.get("configNotes") or []:
        lines.append(f"note:     {n}")
    if d.get("lastSkipped"):
        s = d["lastSkipped"]
        lines.append(f"skipped:  {s.get('at')} ({s.get('reason')})")
    for e in d.get("endpoints") or []:
        flag = "" if e.get("enabled", True) else " [disabled]"
        lines.append(
            f"  {e.get('name', '?'):<10} {e.get('type', ''):<8} every {e.get('window', '?'):<4} "
            f"last {e.get('lastRun') or 'never':<19}  next {e.get('nextRun') or '-':<19}  "
            f"{e.get('status', '?')}{flag}  items={e.get('items', '-')}"
            + (f"  error: {e['lastError']}" if e.get("lastError") else ""))
    return "\n".join(lines)


def main(argv: list) -> int:
    if argv and argv[0] in ("-h", "--help"):
        print(__doc__.strip())
        return 0
    if argv and argv[0] == "--path":
        print(STATUS_FILE)
        return 0
    if argv and argv[0] == "--note-error":
        msg = " ".join(argv[1:]).strip()
        update(lambda d: d.update(enableError={"at": now_str(), "message": msg} if msg else None))
        return 0
    d = read()
    if argv and argv[0] == "--json":
        print(json.dumps(d, indent=2))
        return 0 if d else 1
    print(summary(d))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
