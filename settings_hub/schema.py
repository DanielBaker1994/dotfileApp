"""Validation through the APP (`workspace-switcher config-check`), so no
range / enum is copied into python. Offline fallback: the cached
`config-schema`, then the row's inferred type. Allowed values read from doc
comments are only a warning (--force writes anyway)."""
from __future__ import annotations

import json
import os
import re
import subprocess

from . import paths

_HEX = re.compile(r"^(#|0x)?[0-9A-Fa-f]{6}([0-9A-Fa-f]{2})?$")


def _bin() -> str | None:
    b = paths.app_binary()
    return b if os.access(b, os.X_OK) else None


def schema() -> dict:
    """config-schema JSON, cached by the binary's mtime + size."""
    cache = os.path.join(paths.cache_dir(), "config-schema.json")
    b = _bin()
    if b:
        st = os.stat(b)
        stamp = f"{st.st_mtime_ns}:{st.st_size}"
        try:
            with open(cache, encoding="utf-8") as fh:
                got = json.load(fh)
            if got.get("_stamp") == stamp:
                return got
        except (OSError, ValueError):
            pass
        try:
            out = subprocess.run([b, "config-schema"], capture_output=True, text=True, timeout=5)
            got = json.loads(out.stdout)
            got["_stamp"] = stamp
            with open(cache, "w", encoding="utf-8") as fh:
                json.dump(got, fh)
            return got
        except (OSError, ValueError, subprocess.SubprocessError):
            pass
    try:
        with open(cache, encoding="utf-8") as fh:
            return json.load(fh)
    except (OSError, ValueError):
        return {}


def check_app(section: str, key: str, value: str):
    """The app's verdict: (ok, problem) or None when the binary can't say."""
    b = _bin()
    if not b:
        return None
    try:
        out = subprocess.run([b, "config-check", section, key, value],
                             capture_output=True, text=True, timeout=5)
        got = json.loads(out.stdout)
        return bool(got.get("ok")), got.get("problem", "")
    except (OSError, ValueError, subprocess.SubprocessError):
        return None


def check_file(path: str):
    """validateConfig over a whole file: [issue] or None (no binary)."""
    b = _bin()
    if not b:
        return None
    try:
        out = subprocess.run([b, "config-check", "--file", path], capture_output=True, text=True, timeout=10)
        return json.loads(out.stdout).get("issues", [])
    except (OSError, ValueError, subprocess.SubprocessError):
        return None


def validate(section: str, key: str, value: str, row=None) -> tuple:
    """→ (errors, warnings). Errors refuse the write; warnings need --force."""
    errors, warnings = [], []
    if value == "":
        return errors, warnings          # empty = "use the default" (the app agrees)
    if "\n" in value or "\r" in value:
        return ["commands.toml values are one line"], warnings
    got = check_app(section, key, value)
    if got is not None:
        if not got[0]:
            errors.append(got[1])
    else:
        sc = schema()
        rng = sc.get("numberRanges", {}).get(key)
        if key in sc.get("boolKeys", []) and value not in ("true", "false"):
            errors.append(f"'{value}' is not true/false")
        elif rng:
            try:
                n = float(value)
                if not rng[0] <= n <= rng[1]:
                    errors.append(f"{value} is outside {rng[0]:g}…{rng[1]:g}")
            except ValueError:
                errors.append(f"'{value}' is not a number")
        elif key in sc.get("enumKeys", {}) and value.lower() not in sc["enumKeys"][key]:
            errors.append(f"'{value}' is not one of {' | '.join(sc['enumKeys'][key])}")
        elif row is not None:
            if row.type == "bool" and value not in ("true", "false"):
                errors.append(f"'{value}' is not true/false")
            elif row.type == "number" and not re.fullmatch(r"-?\d+(\.\d+)?", value):
                warnings.append(f"'{value}' doesn't look like a number")
            elif row.type == "color" and not _HEX.match(value):
                warnings.append(f"'{value}' doesn't look like a hex color")
    if not errors and row is not None and row.type == "enum" and row.allowed and \
            value not in row.allowed:
        warnings.append(f"'{value}' is not one of {' | '.join(row.allowed)} (from the comment above it)")
    return errors, warnings
