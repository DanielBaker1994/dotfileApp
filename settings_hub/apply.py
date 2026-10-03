"""Make a change take effect. Each step says what it ran and how it went;
nothing here edits files."""
from __future__ import annotations

import json
import os
import shutil
import subprocess

from . import paths


def _run(argv: list, timeout: int = 20) -> tuple:
    try:
        p = subprocess.run(argv, capture_output=True, text=True, timeout=timeout)
        return p.returncode, (p.stdout + p.stderr).strip()
    except FileNotFoundError:
        return 127, f"{argv[0]}: not found"
    except subprocess.TimeoutExpired:
        return 124, f"{argv[0]}: timed out"


def _app(verb: str) -> dict:
    b = paths.app_binary()
    if not os.access(b, os.X_OK):
        return {"ok": True, "ran": "", "note": "app binary not found; applies on next start"}
    rc, out = _run([b, verb], timeout=20)
    if rc == 0:
        note = "re-read by the running app" if verb == "reload" else "the app restarts"
        try:
            got = json.loads(out.splitlines()[-1])
            bad = [i["message"] for i in got.get("issues", []) if i.get("fatal")]
            if bad:
                return {"ok": False, "ran": f"workspace-switcher {verb}", "note": "; ".join(bad)}
        except (ValueError, IndexError):
            pass
        return {"ok": True, "ran": f"workspace-switcher {verb}", "note": note}
    if "not running" in out:
        return {"ok": True, "ran": f"workspace-switcher {verb}", "note": "the app isn't running; applies on next start"}
    return {"ok": False, "ran": f"workspace-switcher {verb}", "note": out or f"exit {rc}"}


def setting(mode: str) -> dict:
    if mode == "reload":
        return _app("reload")
    if mode == "restart":
        return _app("restart")
    if mode == "trigger":
        return {"ok": True, "ran": "", "note": "read on the next use (no reload needed)"}
    if mode == "open":
        return {"ok": True, "ran": "", "note": "applies the next time that view / window opens"}
    if mode == "sketchybar":
        if not shutil.which("sketchybar"):
            return {"ok": True, "ran": "", "note": "sketchybar not installed"}
        rc, out = _run(["sketchybar", "--reload"])
        return {"ok": rc == 0, "ran": "sketchybar --reload", "note": out or ("done" if rc == 0 else f"exit {rc}")}
    return {"ok": True, "ran": "", "note": ""}


def aerospace_check() -> tuple:
    """(ok, output) of a dry-run parse of AeroSpace's config."""
    if not shutil.which("aerospace"):
        return True, "aerospace not installed"
    rc, out = _run(["aerospace", "reload-config", "--dry-run", "--no-gui"])
    return rc == 0, out


def aerospace() -> dict:
    if not shutil.which("aerospace"):
        return {"ok": True, "ran": "", "note": "aerospace not installed"}
    rc, out = _run(["aerospace", "reload-config", "--no-gui"])
    return {"ok": rc == 0, "ran": "aerospace reload-config", "note": out or ("done" if rc == 0 else f"exit {rc}")}


def _herdr() -> str:
    return paths.expand(paths.hub("herdr-bin")) or shutil.which("herdr") or paths.expand("~/.local/bin/herdr")


def herdr_check() -> tuple:
    h = _herdr()
    if not os.access(h, os.X_OK):
        return True, "herdr not installed"
    rc, out = _run([h, "config", "check"])
    return rc == 0, out


def herdr() -> dict:
    h = _herdr()
    if not os.access(h, os.X_OK):
        return {"ok": True, "ran": "", "note": "herdr not installed"}
    rc, out = _run([h, "server", "reload-config"])
    if rc != 0 and ("not running" in out.lower() or "connect" in out.lower()):
        return {"ok": True, "ran": "herdr server reload-config", "note": "herdr isn't running; applies on next start"}
    return {"ok": rc == 0, "ran": "herdr server reload-config", "note": out or ("done" if rc == 0 else f"exit {rc}")}
