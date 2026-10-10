from __future__ import annotations

import json
import os
import subprocess
import time


def cpu_ticks():
    try:
        import ctypes

        class HostCpuLoad(ctypes.Structure):
            _fields_ = [("cpu_ticks", ctypes.c_uint32 * 4)]

        libc = ctypes.CDLL(None)
        info = HostCpuLoad()
        count = ctypes.c_uint32(4)
        kr = libc.host_statistics(libc.mach_host_self(), 3,
                                  ctypes.byref(info), ctypes.byref(count))
    except Exception:
        return None
    if kr != 0:
        return None
    ticks = list(info.cpu_ticks)
    busy = ticks[0] + ticks[1] + ticks[3]
    return (busy, busy + ticks[2])


def _sysctl_int(name: str):
    try:
        out = subprocess.run(["sysctl", "-n", name], capture_output=True, text=True)
        return int(out.stdout.strip())
    except (OSError, ValueError):
        return None


def ram_percent():
    try:
        pagesize = _sysctl_int("hw.pagesize")
        total = _sysctl_int("hw.memsize")
        out = subprocess.run(["vm_stat"], capture_output=True, text=True).stdout
    except OSError:
        return None
    if not pagesize or not total or total <= 0:
        return None
    stats = {}
    for line in out.split("\n"):
        if ":" not in line:
            continue
        key, value = line.split(":", 1)
        value = value.strip().rstrip(".")
        if value.isdigit():
            stats[key.strip().lower()] = int(value)
    pages = (stats.get("pages active", 0) + stats.get("pages wired down", 0)
             + stats.get("pages occupied by compressor", 0))
    return int(pages * pagesize / total * 100)


def battery():
    try:
        out = subprocess.run(["pmset", "-g", "batt"], capture_output=True, text=True).stdout
    except OSError:
        return None
    for line in out.split("\n"):
        if "InternalBattery" not in line or "%" not in line:
            continue
        segment = line.split("\t")[-1] if "\t" in line else line
        parts = [p.strip() for p in segment.split(";")]
        if not parts:
            continue
        pct_text = next((t[:-1] for t in parts[0].split() if t.endswith("%")), "")
        try:
            pct = int(pct_text)
        except ValueError:
            continue
        state = parts[1].lower() if len(parts) > 1 else ""
        return {"pct": pct, "charging": "discharging" not in state}
    return None


def read_unread(script: str) -> list:
    if not os.path.isfile(script):
        return []
    env = dict(os.environ)
    env["PATH"] = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:" + (env.get("PATH") or "")
    env["PYTHONDONTWRITEBYTECODE"] = "1"
    try:
        r = subprocess.run(["/usr/bin/env", "python3", script, "--json"],
                           capture_output=True, text=True, env=env)
    except OSError:
        return []
    if r.returncode != 0:
        return []
    try:
        obj = json.loads(r.stdout)
    except ValueError:
        return []
    chips = []
    for d in obj.get("sources") or []:
        if not isinstance(d, dict) or not isinstance(d.get("app"), str):
            continue
        count = d.get("count")
        if isinstance(count, bool):
            text = ""
        elif isinstance(count, int):
            text = "" if count == 0 else "999+" if count > 999 else str(count)
        elif isinstance(count, str):
            text = count
        else:
            text = ""
        chips.append({"app": d["app"], "count": text,
                      "mentions": d.get("mentions") or 0, "warn": bool(d.get("warn"))})
    return chips


def gather(notify_script: str, delay: float = 0.3) -> dict:
    a = cpu_ticks()
    started = time.time()
    chips = read_unread(notify_script)
    ram = ram_percent()
    batt = battery()
    left = delay - (time.time() - started)
    if left > 0:
        time.sleep(left)
    b = cpu_ticks()
    cpu = None
    if a and b and b[1] > a[1]:
        cpu = int((b[0] - a[0]) / (b[1] - a[1]) * 100 + 0.5)
    return {"cpu": cpu, "ram": ram, "battery": batt, "chips": chips}
