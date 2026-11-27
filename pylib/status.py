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


def _ram_percent_mach():
    try:
        import ctypes

        class VmStat64(ctypes.Structure):
            _fields_ = [
                ("free_count", ctypes.c_uint32),
                ("active_count", ctypes.c_uint32),
                ("inactive_count", ctypes.c_uint32),
                ("wire_count", ctypes.c_uint32),
                ("zero_fill_count", ctypes.c_uint64),
                ("reactivations", ctypes.c_uint64),
                ("pageins", ctypes.c_uint64),
                ("pageouts", ctypes.c_uint64),
                ("faults", ctypes.c_uint64),
                ("cow_faults", ctypes.c_uint64),
                ("lookups", ctypes.c_uint64),
                ("hits", ctypes.c_uint64),
                ("purges", ctypes.c_uint64),
                ("purgeable_count", ctypes.c_uint32),
                ("speculative_count", ctypes.c_uint32),
                ("decompressions", ctypes.c_uint64),
                ("compressions", ctypes.c_uint64),
                ("swapins", ctypes.c_uint64),
                ("swapouts", ctypes.c_uint64),
                ("compressor_page_count", ctypes.c_uint32),
                ("throttled_count", ctypes.c_uint32),
                ("external_page_count", ctypes.c_uint32),
                ("internal_page_count", ctypes.c_uint32),
                ("total_uncompressed_pages_in_compressor", ctypes.c_uint64),
                ("swapped_count", ctypes.c_uint64),
            ]

        pagesize = os.sysconf("SC_PAGE_SIZE")
        total = os.sysconf("SC_PHYS_PAGES") * pagesize
        if not pagesize or not total or total <= 0:
            return None
        libc = ctypes.CDLL(None)
        info = VmStat64()
        count = ctypes.c_uint32(ctypes.sizeof(info) // ctypes.sizeof(ctypes.c_uint32))
        # HOST_VM_INFO64 = 4
        kr = libc.host_statistics64(libc.mach_host_self(), 4,
                                    ctypes.byref(info), ctypes.byref(count))
        if kr != 0:
            return None
        pages = info.active_count + info.wire_count + info.compressor_page_count
        return int(pages * pagesize / total * 100)
    except Exception:
        return None


def _sysctl_int(name: str):
    try:
        out = subprocess.run(["sysctl", "-n", name], capture_output=True, text=True)
        return int(out.stdout.strip())
    except (OSError, ValueError):
        return None


def ram_percent():
    pct = _ram_percent_mach()
    if pct is not None:
        return pct
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


def _battery_iokit():
    try:
        import ctypes
        iokit = ctypes.CDLL("/System/Library/Frameworks/IOKit.framework/IOKit")
        cf = ctypes.CDLL("/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation")
        c_vp = ctypes.c_void_p
        iokit.IOPSCopyPowerSourcesInfo.restype = c_vp
        iokit.IOPSCopyPowerSourcesList.argtypes = [c_vp]
        iokit.IOPSCopyPowerSourcesList.restype = c_vp
        iokit.IOPSGetPowerSourceDescription.argtypes = [c_vp, c_vp]
        iokit.IOPSGetPowerSourceDescription.restype = c_vp
        cf.CFArrayGetCount.argtypes = [c_vp]
        cf.CFArrayGetCount.restype = ctypes.c_long
        cf.CFArrayGetValueAtIndex.argtypes = [c_vp, ctypes.c_long]
        cf.CFArrayGetValueAtIndex.restype = c_vp
        cf.CFStringCreateWithCString.argtypes = [c_vp, ctypes.c_char_p, ctypes.c_uint32]
        cf.CFStringCreateWithCString.restype = c_vp
        cf.CFDictionaryGetValue.argtypes = [c_vp, c_vp]
        cf.CFDictionaryGetValue.restype = c_vp
        cf.CFNumberGetValue.argtypes = [c_vp, ctypes.c_long, c_vp]
        cf.CFNumberGetValue.restype = ctypes.c_bool
        cf.CFStringGetCString.argtypes = [c_vp, ctypes.c_char_p, ctypes.c_long, ctypes.c_uint32]
        cf.CFStringGetCString.restype = ctypes.c_bool
        cf.CFRelease.argtypes = [c_vp]

        info = iokit.IOPSCopyPowerSourcesInfo()
        if not info:
            return None
        try:
            lst = iokit.IOPSCopyPowerSourcesList(info)
            if not lst:
                return None
            try:
                k_cur = cf.CFStringCreateWithCString(None, b"Current Capacity", 0x08000100)
                k_max = cf.CFStringCreateWithCString(None, b"Max Capacity", 0x08000100)
                k_src = cf.CFStringCreateWithCString(None, b"Power Source State", 0x08000100)
                k_type = cf.CFStringCreateWithCString(None, b"Type", 0x08000100)
                try:
                    n = cf.CFArrayGetCount(lst)
                    for i in range(n):
                        ps = cf.CFArrayGetValueAtIndex(lst, i)
                        desc = iokit.IOPSGetPowerSourceDescription(info, ps)
                        if not desc:
                            continue
                        t_ref = cf.CFDictionaryGetValue(desc, k_type)
                        buf = ctypes.create_string_buffer(64)
                        if t_ref and cf.CFStringGetCString(t_ref, buf, 64, 0x08000100):
                            if buf.value != b"InternalBattery":
                                continue
                        c_ref = cf.CFDictionaryGetValue(desc, k_cur)
                        m_ref = cf.CFDictionaryGetValue(desc, k_max)
                        if not c_ref or not m_ref:
                            continue
                        cur, mx = ctypes.c_int64(0), ctypes.c_int64(0)
                        # kCFNumberSInt64Type = 4
                        if not cf.CFNumberGetValue(c_ref, 4, ctypes.byref(cur)):
                            continue
                        if not cf.CFNumberGetValue(m_ref, 4, ctypes.byref(mx)) or mx.value <= 0:
                            continue
                        pct = int(round(cur.value * 100.0 / mx.value))
                        s_ref = cf.CFDictionaryGetValue(desc, k_src)
                        charging = True
                        if s_ref and cf.CFStringGetCString(s_ref, buf, 64, 0x08000100):
                            charging = buf.value != b"Battery Power"
                        return {"pct": pct, "charging": charging}
                    return None
                finally:
                    for k in (k_cur, k_max, k_src, k_type):
                        if k:
                            cf.CFRelease(k)
            finally:
                cf.CFRelease(lst)
        finally:
            cf.CFRelease(info)
    except Exception:
        return None


def battery():
    res = _battery_iokit()
    if res is not None:
        return res
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


def _read_unread_inproc(script: str):
    """Run notify_poll.snapshot() inside the persistent helper process instead
    of spawning a fresh Python interpreter on every Hyper+S status refresh."""
    import importlib.util
    import sys

    notify_dir = os.path.dirname(os.path.abspath(script))
    mod = sys.modules.get("_ks_notify_poll")
    if mod is None or getattr(mod, "__file__", None) != os.path.abspath(script):
        if notify_dir not in sys.path:
            sys.path.insert(0, notify_dir)
        spec = importlib.util.spec_from_file_location("_ks_notify_poll", script)
        if spec is None or spec.loader is None:
            return None
        mod = importlib.util.module_from_spec(spec)
        sys.modules["_ks_notify_poll"] = mod
        spec.loader.exec_module(mod)
    cfg = mod.config()
    if not mod.jira_config.truthy(cfg["enabled"]):
        return {"sources": []}
    return mod.snapshot(cfg)


def read_unread(script: str) -> list:
    if not os.path.isfile(script):
        return []
    obj = None
    if os.path.basename(script) == "notify_poll.py":
        try:
            obj = _read_unread_inproc(script)
        except Exception:
            obj = None
    if obj is None:
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
