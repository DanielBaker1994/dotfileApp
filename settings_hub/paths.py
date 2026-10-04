"""Where everything lives. Functions, not constants: tests point them at a
temp home through the environment ($WS_HOME, $WS_COMMANDS_CONF, …)."""
from __future__ import annotations

import os
import sys

PKG = os.path.dirname(os.path.abspath(__file__))      # NOT realpath: the home's link
ROOT = os.path.dirname(PKG)                            # repo, or the home in app mode
sys.path.insert(0, os.path.join(ROOT, "pylib"))

from config_text import config_entry, config_section_header  # noqa: E402

_hub_cache: dict = {}


def expand(p: str) -> str:
    return os.path.expanduser(os.path.expandvars(p)) if p else p


def home() -> str:
    return expand(os.environ.get("WS_HOME") or "~/.config/workspace-switcher")


def commands_conf() -> str:
    """$WS_COMMANDS_CONF › the repo beside this package (Swift isRepoBuild:
    commands.toml + bin/build-app.sh) › the home's commands.toml."""
    env = os.environ.get("WS_COMMANDS_CONF")
    if env:
        return expand(env)
    if os.path.exists(os.path.join(ROOT, "commands.toml")) and \
            os.path.exists(os.path.join(ROOT, "bin", "build-app.sh")):
        return os.path.join(ROOT, "commands.toml")
    return os.path.join(home(), "commands.toml")


def section(name: str) -> dict:
    """[name] of commands.toml (codec), cached per file mtime."""
    path = commands_conf()
    try:
        mt = os.stat(path).st_mtime_ns
    except OSError:
        return {}
    key = (path, mt, name)
    if key in _hub_cache:
        return _hub_cache[key]
    out, cur = {}, None
    with open(path, encoding="utf-8") as fh:
        for raw in fh.read().split("\n"):
            h = config_section_header(raw)
            if h is not None:
                cur = h
                continue
            if cur == name:
                e = config_entry(raw)
                if e:
                    out[e[0]] = e[1]
    _hub_cache[key] = out
    return out


def hub(key: str, default: str = "") -> str:
    v = section("settings-hub").get(key, "")
    return v if v != "" else default


def aerospace_conf() -> str:
    """What AeroSpace reads: ~/.aerospace.toml, else $XDG_CONFIG_HOME/aerospace/aerospace.toml."""
    own = hub("aerospace-config")
    if own:
        return expand(own)
    dot = expand("~/.aerospace.toml")
    if os.path.exists(dot):
        return dot
    xdg = os.environ.get("XDG_CONFIG_HOME") or expand("~/.config")
    return os.path.join(xdg, "aerospace", "aerospace.toml")


def herdr_conf() -> str:
    return expand(hub("herdr-config", "~/.config/herdr/config.toml"))


def ghostty_conf() -> str:
    own = hub("ghostty-config")
    if own:
        return expand(own)
    for p in ("~/.config/ghostty/config.ghostty", "~/.config/ghostty/config",
              "~/Library/Application Support/com.mitchellh.ghostty/config.ghostty",
              "~/Library/Application Support/com.mitchellh.ghostty/config"):
        if os.path.exists(expand(p)):
            return expand(p)
    return expand("~/.config/ghostty/config.ghostty")


def vim_init() -> str:
    own = hub("vim-init") or section("notes").get("vim-init", "")
    if own:
        return expand(own)
    return os.path.join(ROOT, "vim", "init.lua")


def vim_bin() -> str:
    return expand(hub("vim-bin") or section("notes").get("vim-bin", "") or "nvim")


def app_binary() -> str:
    env = os.environ.get("WS_SETTINGS_BIN")
    if env:
        return expand(env)
    for base in (ROOT, home()):
        p = os.path.join(base, "workspace-switcher.app", "Contents", "MacOS", "workspace-switcher")
        if os.path.exists(p):
            return p
    return os.path.join(home(), "workspace-switcher.app", "Contents", "MacOS", "workspace-switcher")


def socket_path() -> str:
    tmp = os.environ.get("TMPDIR") or "/tmp/"
    name = section("app").get("notes-socket", "") or "ws-notes.sock"
    return os.path.join(tmp, name)


def cache_dir() -> str:
    d = expand(os.environ.get("WS_SETTINGS_CACHE") or "~/.cache/workspace-switcher")
    os.makedirs(d, exist_ok=True)
    return d


def favorites_path() -> str:
    return expand(os.environ.get("WS_SETTINGS_FAVORITES") or os.path.join(home(), "settings-hub.json"))
