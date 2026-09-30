#!/usr/bin/env python3
"""Notifications pill backend (sketchybar `plugins/notifications.sh`).

  notify_poll.py --config-sh   [notifications] as shell assignments (build)
  notify_poll.py --tick        badges + cached API state → sketchybar --set …
                               (starts a background --poll when stale)
  notify_poll.py --poll        run the API sources now → state.json

Config = `[notifications]` in commands.toml (read with jira_config.read_section,
same line rules as the app). Per source NAME in `sources`: NAME-enabled,
NAME-app (bundle id), NAME-count (dock = Dock badge, window = the app's own
window via helpers/NAME_unread.swift — Webex draws no Dock badge), NAME-tag,
NAME-api. @mentions = the API
source (webex only for now), shown until the badge drops to 0 or the API says
the space was read. State: ~/.cache/notifications/state.json, log poll.log.
"""
from __future__ import annotations

import fcntl
import json
import os
import re
import shlex
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
EVENT = "notifications_update"

DEFAULTS = {
    "enabled": "false",
    "sources": "webex, outlook, imessage",
    "poll-seconds": "60",
    "tick-seconds": "5",
    "hide-when-zero": "true",
    "unread-color": "0xffff3b30",
    "unread-text-color": "0xffffffff",
    "mention-color": "0xffed8796",
    "warn-color": "0xffeed49f",
    "tag-color": "0xff939ab7",
    "icon-scale": "0.62",
    "popup-rows": "6",
    "popup-width": "340",
    "login-command": "open -na Ghostty --args -e {}",
    "mention-max-age-hours": "24",
    "webex-enabled": "true",
    "webex-app": "Cisco-Systems.Spark",
    "webex-tag": "WEBEX",
    "webex-icon": "app",
    "webex-api": "true",
    "webex-count": "window",
    "webex-rooms": "30",
    "outlook-enabled": "false",
    "outlook-app": "com.microsoft.Outlook",
    "outlook-tag": "MAIL",
    "outlook-icon": "app",
    "outlook-api": "false",
    "imessage-enabled": "false",
    "imessage-app": "com.apple.MobileSMS",
    "imessage-tag": "MESSAGES",
    "imessage-icon": "app",
    "imessage-api": "false",
}
API_SOURCES = {"webex"}


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


HELPERS = os.path.join(os.path.dirname(HERE), "config", "sketchybar", "helpers")
HELPER_BIN = os.path.expanduser("~/.cache/sketchybar")


def helper(name: str, *argv: str):
    """stdout of helpers/NAME.swift (built into ~/.cache/sketchybar, rebuilt
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
    subprocess.run(["sketchybar", "--trigger", EVENT], capture_output=True, check=False)
    return st


# -- tick → sketchybar ---------------------------------------------------------
# Per source NAME: notif.NAME (app icon or tag; label = the red count badge),
# notif.NAME.at ("@N" red, or an amber dot when the API needs attention).
ICON_SLOT, ICON_SLOT_BADGED = 22, 11   # icon.width: alone / under a badge


def chip_args(cfg: dict, name: str, count, api: dict | None) -> tuple[list[str], bool]:
    """sketchybar --set args for one source's items; True = worth showing."""
    mentions = 0
    warn = bool(api and not api.get("ok"))
    if api and api.get("ok"):
        mentions = api.get("mentions", 0)
    if count == 0:
        mentions = 0                     # the app says everything is read
    if count is None and api and api.get("ok"):
        count = api.get("unread", 0)     # app not running: fall back to the API
    shown = str(count) if count not in (None, 0) else ""
    if isinstance(count, int) and count:
        shown = f"{count:,}"
    # the count = the icon item's own label: an iOS-style badge drawn ON TOP of
    # the icon's top-right corner (a narrowed icon slot makes the label overlap)
    args = ["--set", f"notif.{name}", f"label.drawing={'on' if shown else 'off'}", f"label={shown}"]
    if cfg.get(f"{name}-icon", "app") == "app":
        args.append(f"icon.width={ICON_SLOT_BADGED if shown else ICON_SLOT}")
    if mentions:
        args += ["--set", f"notif.{name}.at", "drawing=on", f"label=@{mentions}",
                 f"label.color={cfg['mention-color']}", "label.font=SF Pro:Heavy:12.0",
                 "label.y_offset=0"]
    elif warn:
        args += ["--set", f"notif.{name}.at", "drawing=on", "label=●",
                 f"label.color={cfg['warn-color']}", "label.font=SF Pro:Bold:8.0",
                 "label.y_offset=6"]
    else:
        args += ["--set", f"notif.{name}.at", "drawing=off"]
    return args, bool(shown or mentions or warn)


def api_sources(cfg: dict) -> set[str]:
    return {s for s in sources(cfg)
            if s in API_SOURCES and jira_config.truthy(cfg.get(f"{s}-api", "false"))}


def spawn_poll() -> None:
    subprocess.Popen([sys.executable, os.path.abspath(__file__), "--poll"],
                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                     start_new_session=True)


def tick(cfg: dict) -> list[str]:
    st = load_state()
    api_on = api_sources(cfg)
    if api_on and time.time() - st.get("at", 0) >= float(cfg["poll-seconds"]):
        spawn_poll()
    args: list[str] = []
    any_on = False
    hide = jira_config.truthy(cfg["hide-when-zero"])
    dock = dock_badges()
    for s in sources(cfg):
        a, on = chip_args(cfg, s, unread(cfg, s, dock)[0], st.get(s) if s in api_on else None)
        # each chip hides on its own: only apps with something unread show
        args += a + ["--set", f"notif.{s}", f"drawing={'on' if on or not hide else 'off'}"]
        any_on |= on
    vis = "on" if any_on or not hide else "off"
    args += ["--set", "notif.lead", f"drawing={vis}"]   # the chips' left inset
    if vis == "off":
        for s in sources(cfg):
            args += ["--set", f"notif.{s}.at", "drawing=off",
                     "--set", f"notif.{s}", "popup.drawing=off"]
    return args


# -- popup (click) -------------------------------------------------------------
def ago(iso: str | None) -> str:
    if not iso:
        return ""
    try:
        import datetime as dt
        t = dt.datetime.fromisoformat(iso.replace("Z", "+00:00")).timestamp()
    except ValueError:
        return ""
    m = max(0, int((time.time() - t) // 60))
    return "now" if m < 1 else f"{m}m" if m < 60 else f"{m // 60}h" if m < 1440 else f"{m // 1440}d"


def popup_rows(cfg: dict, name: str, count, api: dict | None, api_enabled: bool,
               local: list[str] = ()) -> list[dict]:
    """Rows for notif.NAME's popup: {text, right, color, font, click}."""
    app = cfg[f"{name}-app"]
    title = cfg.get(f"{name}-tag", name).title()
    close = f"sketchybar --set notif.{name} popup.drawing=off"
    me = shlex.quote(os.path.abspath(__file__))
    rows: list[dict] = []
    head = [f"{count} unread" if isinstance(count, int) and count else
            "not running" if count is None else "all read" if count == 0 else "unread"]
    if api and api.get("ok") and api.get("mentions") and count != 0:
        head.append(f"@{api['mentions']}")
    rows.append({"text": f"{title} — {' · '.join(head)}", "font": "SF Pro:Bold:13.0",
                 "click": f"open -b {shlex.quote(app)}; {close}"})
    if api_enabled and api and not api.get("ok"):
        cmd = cfg["login-command"].replace(
            "{}", f"python3 {shlex.quote(os.path.join(HERE, name + '_api.py'))} --login")
        rows.append({"text": "Sign in to see @mentions" if api.get("auth") else api.get("error", "API error")[:60],
                     "right": "Sign in ›" if api.get("auth") else "Retry ›",
                     "color": cfg["warn-color"],
                     "click": (f"{cmd}; {close}" if api.get("auth") else
                               f"python3 {me} --poll; {close}")})
    elif api_enabled and api and api.get("ok"):
        limit = int(cfg["popup-rows"])
        shown = 0
        for it in api.get("items", [])[:limit]:
            rows.append({"text": f"@ {it['space']}: {it['text']}"[:70], "right": ago(it.get("at")),
                         "color": cfg["mention-color"],
                         "click": f"open {shlex.quote(it['link'] or '')} || open -b {shlex.quote(app)}; {close}"})
            shown += 1
        for sp in api.get("spaces", []):
            if shown >= limit:
                break
            if sp.get("mentions"):
                continue
            rows.append({"text": sp["title"][:60], "right": ago(sp.get("at")),
                         "click": f"open {shlex.quote(sp['link'] or '')} || open -b {shlex.quote(app)}; {close}"})
            shown += 1
        if not shown and count != 0:
            rows.append({"text": "No @mentions", "color": cfg["tag-color"]})
    if not (api_enabled and api and api.get("ok")):
        # no API: the unread spaces the app's window lists (no deep links)
        for t in list(local)[:int(cfg["popup-rows"])]:
            rows.append({"text": t[:60], "right": "new", "color": cfg["mention-color"],
                         "click": f"open -b {shlex.quote(app)}; {close}"})
    rows.append({"text": f"Open {title}", "right": "›", "click": f"open -b {shlex.quote(app)}; {close}"})
    if api_enabled:
        rows.append({"text": "Refresh", "right": "↻", "color": cfg["tag-color"],
                     "click": f"python3 {me} --poll; {close}"})
    return rows


def popup_args(cfg: dict, name: str) -> list[str]:
    st = load_state()
    on = name in api_sources(cfg)
    count, local = unread(cfg, name, dock_badges())
    rows = popup_rows(cfg, name, count, st.get(name) if on else None, on, local)
    anchor = f"notif.{name}"
    args = ["--remove", "/^notif\\.pop\\./"]
    w = int(cfg["popup-width"])
    for i, r in enumerate(rows):
        item = f"notif.pop.{name}.{i}"
        # text on the left = the icon slot (fixed width), `right` = the label
        args += ["--add", "item", item, f"popup.{anchor}",
                 "--set", item, "background.drawing=off", "icon.max_chars=46",
                 f"icon={r['text']}", f"icon.color={r.get('color', '0xffcad3f5')}",
                 f"icon.font={r.get('font', 'SF Pro:Semibold:12.0')}",
                 f"icon.width={w - 60}", "icon.padding_left=12", "icon.padding_right=0",
                 f"label={r.get('right', '')}", f"label.color={cfg['tag-color']}",
                 "label.font=SF Pro:Semibold:11.0", "label.width=48", "label.align=right",
                 "label.padding_left=0", "label.padding_right=12",
                 f"click_script={r.get('click', '')}"]
    return args


def event(cfg: dict) -> list[str]:
    """Item script entry: $NAME / $SENDER from sketchybar."""
    name, sender = os.environ.get("NAME", ""), os.environ.get("SENDER", "")
    parts = name.split(".")
    src = parts[1] if len(parts) > 1 and parts[0] == "notif" else ""
    if sender == "mouse.clicked" and src in sources(cfg):
        return popup_args(cfg, src) + ["--set", f"notif.{src}", "popup.drawing=toggle"]
    if sender == "mouse.exited.global" and src in sources(cfg):
        return ["--set", f"notif.{src}", "popup.drawing=off"]
    return tick(cfg)


def config_sh(cfg: dict) -> str:
    keys = {"ENABLED": "on" if jira_config.truthy(cfg["enabled"]) else "off",
            "SOURCES": " ".join(sources(cfg)),
            "TICK": cfg["tick-seconds"],
            "UNREAD_COLOR": cfg["unread-color"],
            "UNREAD_TEXT_COLOR": cfg["unread-text-color"],
            "TAG_COLOR": cfg["tag-color"],
            "ICON_SCALE": cfg["icon-scale"]}
    for s in sources(cfg):
        keys[f"TAG_{s}"] = cfg.get(f"{s}-tag", s.upper())
        keys[f"APP_{s}"] = cfg[f"{s}-app"]
        keys[f"ICON_{s}"] = cfg.get(f"{s}-icon", "app")
    return "\n".join(f"NOTIF_{k}={shlex.quote(str(v))}" for k, v in keys.items())


def main(argv: list[str]) -> int:
    cfg = config()
    if "--config-sh" in argv:
        print(config_sh(cfg))
        return 0
    if not jira_config.truthy(cfg["enabled"]):
        return 0
    if "--poll" in argv:
        st = run_poll(cfg)
        if "--print" in argv:
            print(json.dumps(st, indent=2))
        return 0
    if "--tick" in argv or "--event" in argv:
        args = tick(cfg) if "--tick" in argv else event(cfg)
        if "--dry-run" in argv:
            print(shlex.join(["sketchybar", *args]))
        else:
            subprocess.run(["sketchybar", *args], check=False)
        return 0
    print(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
