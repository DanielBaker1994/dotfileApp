"""log_hub.py - unified date-based log directory management.

Structure: logs/YYYY_MM_DD/{component}.log

Every writer calls log_path(component) to get today's file path.
Directories are created automatically on first write.

Where the logs/ base lives (first match wins):
  1. $WS_LOG_DIR            (tests, the MCP sandbox) - used as-is, no date subdir
  2. the repo beside pylib/ (repo mode: commands.toml + bin/build-app.sh)
  3. $WS_HOME/logs          (an installed app: ~/.config/kitchen-sink/logs)
"""
from __future__ import annotations

import os
import time
from pathlib import Path

_PKG = Path(__file__).resolve().parent          # .../pylib
_REPO = _PKG.parent                             # the repo (or Resources in an app)


def _base_dir() -> Path:
    override = os.environ.get("WS_LOG_DIR")
    if override:
        return Path(override)
    if (_REPO / "commands.toml").exists() and (_REPO / "bin" / "build-app.sh").exists():
        return _REPO / "logs"
    home = os.environ.get("WS_HOME") or "~/.config/kitchen-sink"
    return Path(os.path.expanduser(os.path.expandvars(home))) / "logs"


def today_dir() -> Path:
    """The log directory for today.

    With $WS_LOG_DIR set it is used verbatim (a flat dir: tests + the sandbox);
    otherwise it is <base>/YYYY_MM_DD.
    """
    if os.environ.get("WS_LOG_DIR"):
        return _base_dir()
    return _base_dir() / time.strftime("%Y_%m_%d")


def log_path(component: str) -> Path:
    """Path to logs/YYYY_MM_DD/{component}.log (the directory is created).

    Args:
        component: Log name without .log (e.g. "jira-debug", "notifications-poll")
    """
    day_dir = today_dir()
    day_dir.mkdir(parents=True, exist_ok=True)
    return day_dir / f"{component}.log"


def log_dir() -> Path:
    """Today's log directory (created if needed)."""
    day_dir = today_dir()
    day_dir.mkdir(parents=True, exist_ok=True)
    return day_dir


def cleanup_old_logs(keep_days: int = 30, log=None) -> int:
    """Remove day directories older than keep_days. Returns how many.

    `log` is an optional callable(msg) for a one-line note per removal.
    """
    base = _base_dir()
    if not base.exists() or os.environ.get("WS_LOG_DIR"):
        return 0

    cutoff = time.time() - (keep_days * 86400)
    removed = 0
    for day_dir in base.iterdir():
        if not day_dir.is_dir():
            continue
        name = day_dir.name
        if len(name) != 10 or name.count("_") != 2:
            continue
        try:
            if day_dir.stat().st_mtime < cutoff:
                import shutil
                shutil.rmtree(day_dir)
                removed += 1
                if log:
                    log(f"removed old log dir {day_dir}")
        except OSError:
            continue
    return removed
