#!/usr/bin/env python3
"""Unread counts for the Hyper+S status row (kitchen-sink).

  notify_poll.py --json        badges + cached API state → one JSON object
                               (starts a background --poll when stale)
  notify_poll.py --poll        run the API sources now → state.json

Config = `[notifications]` in commands.toml (read with jira_config.read_section,
same line rules as the app). Per source NAME in `sources`: NAME-enabled,
NAME-app (bundle id), NAME-count (dock = Dock badge, window = the app's own
window via helpers/NAME_unread.swift — Webex draws no Dock badge), NAME-tag,
NAME-api. @mentions = the API
source (webex only for now), shown until the badge drops to 0 or the API says
the space was read. State: ~/.cache/notifications/state.json, log poll.log.
--json prints {"sources": [{name, app, tag, count, mentions, warn}]}: only the
sources with something to show unless hide-when-zero = false.
"""
from __future__ import annotations

import fcntl
import json
import os
import re
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(os.path.dirname(HERE), "jira"))
sys.path.insert(0, HERE)

import jira_config  # noqa: E402

CACHE = os.environ.get("NOTIFY_CACHE_DIR") or os.path.expanduser("~/.cache/notifications")
STATE = os.path.join(CACHE, "state.json")
LOCK = os.path.join(CACHE, "poll.lock")
LOG = os.path.join(CACHE, "poll.log")

# the [notifications] defaults live in notify/defaults.json and load here
with open(os.path.join(HERE, "defaults.json"), encoding="utf-8") as _fh:
    _ALL = json.load(_fh)

DEFAULTS = _ALL["defaults"]

API_SOURCES = set(_ALL["api_sources"])


def config() -> dict:
    return {**DEFAULTS, **jira_config.read_section("notifications")}


def sources(cfg: dict) -> list[str]:
    out = []
    for s in cfg["sources"].split(","):
        s = s.strip()
        if s and jira_config.truthy(cfg.get(f"{s}-enabled", "false")):
            out.append(s)
    return out


def log(msg: str) -> None:
    os.makedirs(CACHE, exist_ok=True)
    with open(LOG, "a", encoding="utf-8") as fh:
        fh.write(time.strftime("%F %T ") + msg + "\n")


# -- Dock badge ----------------------------------------------------------------
def parse_badge(out: str):
    """`lsappinfo info -only StatusLabel ASN` output → int count, "•"
    (non-numeric badge) or 0 (no badge: lsappinfo then prints a `[ NULL ]`
    record)."""
    m = re.search(r'"label"\s*=\s*"([^"]*)"', out)
    if not m or not m.group(1).strip():
        return 0
    label = m.group(1).strip()
    return int(label) if label.isdigit() else "•"


HELPERS = os.path.join(HERE, "helpers")
HELPER_BIN = os.path.expanduser("~/.cache/kitchen-sink/helpers")


def helper(name: str, *argv: str):
    """stdout of helpers/NAME.swift (built into ~/.cache/kitchen-sink/helpers, rebuilt
    when the source changes); None when it can't run or fails (no
    Accessibility permission, no swiftc, app not running)."""
    src, exe = os.path.join(HELPERS, name + ".swift"), os.path.join(HELPER_BIN, name)
    try:
        if not os.path.exists(exe) or os.path.getmtime(src) > os.path.getmtime(exe):
            os.makedirs(HELPER_BIN, exist_ok=True)
            subprocess.run(["swiftc", "-O", src, "-o", exe], capture_output=True,
                           timeout=300, check=True)
        r = subprocess.run([exe, *argv], capture_output=True, text=True, timeout=5)
    except (OSError, subprocess.SubprocessError):
        return None
    if r.returncode != 0:
        if r.returncode != 3:            # 3 = the app isn't running: not an error
            log((r.stderr.strip() or f"{name}: exit {r.returncode}"))
        return None
    return r.stdout


def dock_badges():
    """{bundle id: badge text} for every Dock item, read from the Dock itself
    (helpers/dock_badges.swift); None when the helper can't run."""
    text = helper("dock_badges")
    if text is None:
        return None
    out = {}
    for line in text.splitlines():
        bid, _, label = line.partition("\t")
        out[bid] = label.strip()
    return out


def parse_window(text: str):
    """helpers/webex_unread output → (count, [space titles])."""
    count, spaces = 0, []
    for line in text.splitlines():
        kind, _, val = line.partition("\t")
        if kind == "count":
            val = val.strip()
            count = int(val) if val.isdigit() else "•" if val else 0
        elif kind == "space" and val.strip():
            spaces.append(val.strip())
    return count, spaces


def window_unread(cfg: dict, name: str):
    """(count, [space titles]) the app's own window shows (`NAME-count =
    "window"`: Webex draws no Dock badge), None when it can't be read."""
    if cfg.get(f"{name}-count", "dock") != "window":
        return None
    text = helper(f"{name}_unread", cfg[f"{name}-app"])
    return parse_window(text) if text is not None else None


def unread(cfg: dict, name: str, dock: dict | None):
    """(count, spaces): the app window when configured and readable, else the
    Dock badge."""
    win = window_unread(cfg, name)
    return win if win is not None else (badge(cfg[f"{name}-app"], dock), [])


def badge(bundle: str, dock: dict | None = None):
    """The app's badge: int count, "•" (non-numeric) or 0; None = not running
    and not in the Dock. The Dock is the source of truth (it shows badges
    lsappinfo misses); lsappinfo is the fallback for apps not kept in the Dock."""
    if dock and bundle in dock:
        label = dock[bundle]
        return 0 if not label else int(label) if label.isdigit() else "•"
    def run(*a):
        return subprocess.run(["lsappinfo", *a], capture_output=True, text=True,
                              timeout=3).stdout.strip()
    try:
        asn = run("find", f"bundleid={bundle}")
        return parse_badge(run("info", "-only", "StatusLabel", asn)) if asn else None
    except (OSError, subprocess.TimeoutExpired):
        return None


# -- state ---------------------------------------------------------------------
def load_state() -> dict:
    try:
        with open(STATE, encoding="utf-8") as fh:
            return json.load(fh)
    except (OSError, ValueError):
        return {}


def save_state(st: dict) -> None:
    os.makedirs(CACHE, exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=CACHE, prefix=".state-")
    with os.fdopen(fd, "w", encoding="utf-8") as fh:
        json.dump(st, fh)
    os.replace(tmp, STATE)


def run_poll(cfg: dict) -> dict:
    """One API poll of every enabled API source (flock: one at a time)."""
    os.makedirs(CACHE, exist_ok=True)
    with open(LOCK, "w") as lk:
        try:
            fcntl.flock(lk, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError:
            return load_state()
        st = load_state()
        for s in sources(cfg):
            if s not in API_SOURCES or not jira_config.truthy(cfg.get(f"{s}-api", "false")):
                st.pop(s, None)
                continue
            import webex_api  # the only API source so far (API_SOURCES)
            res = webex_api.poll(int(cfg["webex-rooms"]), float(cfg["mention-max-age-hours"]))
            res["at"] = time.time()
            if not res["ok"]:
                log(f"{s}: {res['error']}")
            st[s] = res
        st["at"] = time.time()
        save_state(st)
    return st


# -- snapshot → the switcher's status row --------------------------------------
def chip(name: str, cfg: dict, count, api: dict | None) -> dict:
    """One source's status: count (int, "•", 0, or None = not running),
    @mentions and warn (the API needs attention); `shown` = worth a chip."""
    mentions = 0
    warn = bool(api and not api.get("ok"))
    if api and api.get("ok"):
        mentions = api.get("mentions", 0)
    if count == 0:
        mentions = 0                     # the app says everything is read
    if count is None and api and api.get("ok"):
        count = api.get("unread", 0)     # app not running: fall back to the API
    return {"name": name, "app": cfg[f"{name}-app"], "tag": cfg.get(f"{name}-tag", name.upper()),
            "count": count, "mentions": mentions, "warn": warn,
            "shown": bool(count not in (None, 0) or mentions or warn)}


def api_sources(cfg: dict) -> set[str]:
    return {s for s in sources(cfg)
            if s in API_SOURCES and jira_config.truthy(cfg.get(f"{s}-api", "false"))}


def spawn_poll() -> None:
    subprocess.Popen([sys.executable, os.path.abspath(__file__), "--poll"],
                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                     start_new_session=True)


def snapshot(cfg: dict) -> dict:
    st = load_state()
    api_on = api_sources(cfg)
    if api_on and time.time() - st.get("at", 0) >= float(cfg["poll-seconds"]):
        spawn_poll()
    hide = jira_config.truthy(cfg["hide-when-zero"])
    dock = dock_badges()
    out = []
    for s in sources(cfg):
        c = chip(s, cfg, unread(cfg, s, dock)[0], st.get(s) if s in api_on else None)
        if c["shown"] or not hide:
            out.append(c)
    return {"sources": out}


def main(argv: list[str]) -> int:
    cfg = config()
    if not jira_config.truthy(cfg["enabled"]):
        if "--json" in argv:
            print(json.dumps({"sources": []}))
        return 0
    if "--poll" in argv:
        st = run_poll(cfg)
        if "--print" in argv:
            print(json.dumps(st, indent=2))
        return 0
    if "--json" in argv:
        print(json.dumps(snapshot(cfg)))
        return 0
    print(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
