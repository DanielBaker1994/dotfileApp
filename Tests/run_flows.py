#!/usr/bin/env python3
"""Deterministic runner for Tests/app_flows.json, and replayer for worker findings.

No LLM involved. It owns the whole GUI-sandbox lifecycle (fixtures, start, cleanup),
drives each flow with real events through the app's own queue (`do:input:*`), and
decides every oracle it can by machine. What it cannot decide it hands on, never guesses:

  state / disk / log oracles          -> pass | fail
  independent with "cmd"              -> pass | fail (shell command, exit 0 = pass)
  independent with only "describe"    -> agent      (a worker must check it)
  visual with "ocr_find"/"ocr_absent" -> pass | fail (OCR over the sandbox app's own windows)
  visual with only "expect"           -> eyes       (PNG + OCR text saved for a look)
  reply {verb, error?}                -> pass | fail (the reply a `do:` step got: error text / ok)

A flow is PASS when nothing failed (pending agent/eyes oracles are listed), FAIL when an
oracle failed, ERROR when a step could not run or the app died.

    python3 Tests/run_flows.py --list
    python3 Tests/run_flows.py                         # every flow, fresh build of the working tree
    python3 Tests/run_flows.py --feature notes --repeat 3
    python3 Tests/run_flows.py --ids notes-type-save,switcher-close-esc --app installed
    python3 Tests/run_flows.py --replay .build/hammer/RUN/G-SOAK/findings.jsonl --repeat 3

The GUI sandbox takes the screen while it runs (a test window in front; anything typed goes
into it). Only one GUI run at a time: a lock file enforces it. Results: <out>/results.jsonl,
<out>/summary.md, <out>/shots/. Exit 0 when nothing FAILed or ERRORed (or, for --replay,
when no finding reproduced), else 1; 2 for usage/setup problems.
"""
from __future__ import annotations

import argparse
import fcntl
import glob as globmod
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SANDBOX_SH = os.path.join(ROOT, "bin", "ks-mcp-sandbox.sh")
KS_OCR = os.path.join(ROOT, "bin", "ks-ocr")
KS_INPUT = os.path.join(ROOT, "bin", "ks-input")
TABLE = os.path.join(ROOT, "Tests", "app_flows.json")
# Short on purpose: the daemon socket path must stay under 104 bytes.
SANDBOX = os.path.join(ROOT, ".build", "fr")
FRESH_APP = os.path.join(ROOT, ".build", "sandbox-app", "kitchen-sink.app")
RELEASE_BIN = os.path.join(ROOT, ".build", "rust-target", "release", "kitchen-sink")
GUI_LOCK = os.path.join(ROOT, ".build", "gui-sandbox.lock")
CODESIGN_ID = "kitchen-sink codesign"
BUNDLE_ID = "dev.danielbaker.kitchen-sink"
# --settle: how long a failing state oracle keeps re-reading state before it fails
# (0 = read once, as the table's "no retry" contract says; a slower VM needs some).
SETTLE_MS = 0
WARMUP_S = 2.0


class StepError(Exception):
    pass


def log(msg: str) -> None:
    print(msg, flush=True)


# ---------------------------------------------------------------- sandbox


class Sandbox:
    """The GUI sandbox under .build/fr, always cleaned up by stop()."""

    def __init__(self, app: str):
        self.app = app = os.path.abspath(app)
        self.env = dict(os.environ, KS_MCP_SANDBOX=SANDBOX, KS_MCP_SANDBOX_NO_BUILD="1",
                        KS_MCP_SANDBOX_APP=app)
        self.sock = self._sh("sock")
        self.home = self._sh("home")  # /tmp/ksb-<hash>, the app's HOME
        self.real_home = os.path.join(SANDBOX, "home")
        self.live_pids = set()
        self.running = False

    def _sh(self, *args: str, check: bool = True) -> str:
        r = subprocess.run([SANDBOX_SH, *args], env=self.env, capture_output=True, text=True)
        if check and r.returncode != 0:
            raise SystemExit(f"ks-mcp-sandbox.sh {' '.join(args)} failed: {r.stderr.strip() or r.stdout.strip()}")
        return r.stdout.strip()

    def reset(self) -> None:
        self._sh("reset", check=False)
        shutil.rmtree(SANDBOX, ignore_errors=True)
        os.makedirs(self.real_home, exist_ok=True)

    def start(self) -> None:
        self.live_pids = set(pgrep_live())
        out = self._sh("start", "--gui")
        log(f"sandbox: {out.splitlines()[-1] if out else 'started'}")
        self.running = True
        deadline = time.time() + 20
        while time.time() < deadline:
            if self.state(quiet=True) is not None:
                break
            time.sleep(0.5)
        else:
            raise SystemExit("sandbox never answered `state` within 20 s")
        # Warm-up: the first do: after launch is slow (and holds its connection, APP-BUGS #17);
        # let it settle so the first flow doesn't race a still-starting app.
        try:
            self.send("do:open:notes", timeout=30)
        except Exception:
            pass
        time.sleep(WARMUP_S)

    def stop(self) -> None:
        if self.app.startswith(os.path.join(ROOT, ".build")):
            # `stop` refuses a copied bundle ("not the sandbox binary"): kill it by path.
            subprocess.run(["pkill", "-f", self.app + "/Contents/MacOS/"], capture_output=True)
            time.sleep(1)
        self._sh("stop", check=False)
        if shutil.which("aerospace"):
            subprocess.run(["aerospace", "enable", "on"], capture_output=True)
        self.running = False
        left = subprocess.run(["pgrep", "-f", self.app + "/Contents/MacOS/"], capture_output=True, text=True).stdout.split()
        if left:
            subprocess.run(["kill", "-9", *left], capture_output=True)
            log(f"WARNING: sandbox app pid(s) {left} ignored SIGTERM; killed with -9")
        gone = self.live_pids - set(pgrep_live())
        if gone:
            log(f"WARNING: live kitchen-sink pid(s) {sorted(gone)} disappeared during the run")

    def restart(self) -> None:
        self.stop()
        self.start()

    # -- socket
    def send(self, line: str, timeout: float = 8.0) -> str:
        if len(line.encode()) > 2000:
            raise StepError("request too long for the socket (> 2048 bytes)")
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        s.settimeout(timeout)
        try:
            s.connect(self.sock)
            s.sendall(line.encode() + b"\n")
            # Every reply is one line. Stop at the newline: the app can keep a
            # connection open long after replying (the first do: after launch
            # stays open ~25 s, APP-BUGS.md), so waiting for EOF would stall.
            buf = b""
            while b"\n" not in buf:
                chunk = s.recv(65536)
                if not chunk:
                    break
                buf += chunk
        finally:
            s.close()
        return buf.decode(errors="replace").strip()

    def state(self, quiet: bool = False):
        try:
            return json.loads(self.send("state", timeout=4))
        except Exception:
            if not quiet:
                return None
            return None

    def do(self, verb: str) -> dict:
        t0 = time.time()
        reply = self.send(verb if verb.startswith("do:") else "do:" + verb, timeout=20)
        if time.time() - t0 > 3:
            log(f"  slow: {verb} took {time.time() - t0:.1f}s")
        try:
            v = json.loads(reply) if reply else {}
        except json.JSONDecodeError:
            raise StepError(f"{verb}: non-JSON reply {reply[:200]!r}")
        if isinstance(v, dict) and "error" in v:
            raise StepError(f"{verb}: {v['error']}")
        return v

    def logs(self) -> list[str]:
        return [os.path.join(SANDBOX, "daemon.log")] + sorted(
            globmod.glob(os.path.join(self.real_home, "logs", "*", "daemon.log")))

    def pid(self) -> int | None:
        st = self.state(quiet=True)
        return st.get("pid") if st else None

    def activate(self) -> None:
        """Bring the sandbox app to the front and make its shared window key.

        Since macOS 14 an app cannot activate itself while another app is active
        (cooperative activation), so `do:input:activate` alone is not enough: the
        external `ks-input activate` (run from the active terminal's process tree)
        asks for activation, then the in-app verb makes the shared window key."""
        pid = self.pid()
        if not pid:
            raise StepError("activate: app not answering")
        st: dict = {}
        for _ in range(4):
            # ks-input's own frontmost check can time out in a VM where no app yields
            # activation; the app's `active` / `keyWindow` state is the real test.
            subprocess.run([KS_INPUT, "activate", "--pid", str(pid)], capture_output=True, text=True)
            self.do("input:activate")
            time.sleep(0.4)
            st = self.state() or {}
            if st.get("active") and st.get("keyWindow"):
                return
            time.sleep(0.5)
        raise StepError(f"activate: app active={st.get('active')} keyWindow={st.get('keyWindow')!r}")


def pgrep_live() -> list[int]:
    r = subprocess.run(["pgrep", "-x", "kitchen-sink"], capture_output=True, text=True)
    return [int(p) for p in r.stdout.split()]


def prepare_fresh_app() -> str:
    """Copy the installed bundle, swap in a release build of the working tree, re-sign."""
    log("building release binary (cargo, live-safe) …")
    r = subprocess.run(["cargo", "build", "--release", "-p", "ws-rs", "-p", "ws-helpers"],
                       cwd=os.path.join(ROOT, "rust"),
                       env=dict(os.environ, CARGO_TARGET_DIR=os.path.join(ROOT, ".build", "rust-target")),
                       capture_output=True, text=True)
    if r.returncode != 0:
        raise SystemExit("cargo build failed:\n" + r.stderr[-3000:])
    installed = os.path.join(ROOT, "kitchen-sink.app")
    if not os.path.isdir(installed):
        raise SystemExit("missing kitchen-sink.app (run ./ws build once)")
    target_bin = os.path.join(FRESH_APP, "Contents", "MacOS", "kitchen-sink")
    if os.path.exists(target_bin) and os.path.getmtime(target_bin) >= os.path.getmtime(RELEASE_BIN):
        return FRESH_APP
    shutil.rmtree(FRESH_APP, ignore_errors=True)
    os.makedirs(os.path.dirname(FRESH_APP), exist_ok=True)
    subprocess.run(["cp", "-R", installed, FRESH_APP], check=True)
    shutil.copy2(RELEASE_BIN, target_bin)
    os.utime(target_bin)
    r = subprocess.run(["codesign", "--force", "--sign", CODESIGN_ID, "--identifier", BUNDLE_ID, FRESH_APP],
                       capture_output=True, text=True)
    if r.returncode != 0:
        raise SystemExit("codesign failed: " + r.stderr.strip())
    return FRESH_APP


# ---------------------------------------------------------------- fixtures


def toml_value(v) -> str:
    if isinstance(v, bool):
        return "true" if v else "false"
    if isinstance(v, (int, float)):
        return str(v)
    s = str(v)
    if s in ("true", "false") or re.fullmatch(r"-?\d+(\.\d+)?", s):
        return s
    return json.dumps(s, ensure_ascii=False)


def apply_config(path: str, sets: dict) -> None:
    """Set `key = value` inside each `[section]` of a commands.toml (line-level edit)."""
    lines = open(path, encoding="utf-8").read().split("\n")
    for section, kv in sets.items():
        try:
            start = lines.index(section)
        except ValueError:
            lines += ["", section]
            start = len(lines) - 1
        end = next((i for i in range(start + 1, len(lines)) if lines[i].startswith("[")), len(lines))
        for key, val in kv.items():
            new = f"{key} = {toml_value(val)}"
            hit = next((i for i in range(start + 1, end) if re.match(rf"\s*{re.escape(key)}\s*=", lines[i])), None)
            if hit is None:
                lines.insert(start + 1, new)
                end += 1
            else:
                lines[hit] = new
    open(path, "w", encoding="utf-8").write("\n".join(lines))


def make_fixtures(table: dict, real_home: str) -> None:
    conf = os.path.join(real_home, "commands.toml")
    shutil.copy2(os.path.join(ROOT, "commands.toml"), conf)
    for f in table["fixtures"]:
        p = os.path.join(real_home, f["path"])
        if f["kind"] == "dir":
            os.makedirs(p, exist_ok=True)
        elif f["kind"] == "file":
            os.makedirs(os.path.dirname(p), exist_ok=True)
            with open(p, "w", encoding="utf-8") as fh:
                fh.write(f.get("content", ""))
        elif f["kind"] == "config":
            apply_config(conf, f.get("set", {}))


# ---------------------------------------------------------------- oracles


def resolve(obj, path: str):
    cur = obj
    for part in path.split("."):
        m = re.fullmatch(r"([^\[]*)((?:\[\d+\])*)", part)
        if m is None:
            raise KeyError(path)
        name, idx = m.group(1), m.group(2)
        if name:
            if not isinstance(cur, dict) or name not in cur:
                raise KeyError(path)
            cur = cur[name]
        for i in re.findall(r"\[(\d+)\]", idx):
            if not isinstance(cur, list) or int(i) >= len(cur):
                raise KeyError(path)
            cur = cur[int(i)]
    return cur


def names(v) -> list[str]:
    out = []
    for e in v if isinstance(v, list) else []:
        if isinstance(e, dict):
            out.append(str(e.get("name", e.get("title", ""))))
        else:
            out.append(str(e).split("|")[0])
    return out


def expand_expected(o: dict, home: str) -> dict:
    o = dict(o)
    for k in ("equals", "not_equals", "contains", "contains_name"):
        if isinstance(o.get(k), str):
            o[k] = o[k].replace("$HOME", home)
    return o


def check_state(o: dict, st: dict) -> tuple[bool, str]:
    try:
        v = resolve(st, o["path"])
    except KeyError:
        return False, f"{o['path']} missing from state"
    shown = json.dumps(v, ensure_ascii=False)
    shown = shown if len(shown) < 300 else shown[:300] + "…"
    if "equals" in o and v != o["equals"]:
        return False, f"{o['path']} = {shown}, expected {o['equals']!r}"
    if "not_equals" in o and v == o["not_equals"]:
        return False, f"{o['path']} = {shown}, expected anything but {o['not_equals']!r}"
    if "min" in o:
        n = len(v) if isinstance(v, (list, str, dict)) else v
        if not isinstance(n, (int, float)) or n < o["min"]:
            return False, f"{o['path']} = {shown}, expected >= {o['min']}"
    if "length" in o and (not isinstance(v, (list, str)) or len(v) != o["length"]):
        return False, f"{o['path']} = {shown}, expected length {o['length']}"
    if "contains" in o:
        c = o["contains"]
        ok = (c in v) if isinstance(v, str) else any(
            e == c or (isinstance(e, str) and isinstance(c, str) and c in e) for e in (v or []))
        if not ok:
            return False, f"{o['path']} = {shown}, expected to contain {c!r}"
    if "contains_name" in o and o["contains_name"] not in names(v):
        return False, f"{o['path']} names {names(v)[:20]}, expected {o['contains_name']!r}"
    if "every_name_contains" in o:
        sub = o["every_name_contains"].lower()
        bad = [n for n in names(v) if sub not in n.lower()]
        if bad or not names(v):
            return False, f"{o['path']}: names without {sub!r}: {bad[:10] or '(no rows)'}"
    return True, ""


def check_disk(o: dict, home: str) -> tuple[bool, str]:
    base = os.path.join(home, o["path"])
    paths = sorted(globmod.glob(os.path.join(base, o["glob"]))) if "glob" in o else [base]
    paths = [p for p in paths if os.path.exists(p)]
    label = o["path"] + (o.get("glob") or "")
    if "exists" in o and bool(paths) != o["exists"]:
        return False, f"{label}: exists={bool(paths)}, expected {o['exists']}"
    if "count_min" in o and len(paths) < o["count_min"]:
        return False, f"{label}: {len(paths)} match(es), expected >= {o['count_min']}"
    if o.get("is_dir") and not any(os.path.isdir(p) for p in paths):
        return False, f"{label}: no directory matches"
    if "contains" in o:
        for p in paths:
            if os.path.isfile(p) and o["contains"] in open(p, encoding="utf-8", errors="replace").read():
                return True, ""
        return False, f"{label}: no file contains {o['contains']!r} ({len(paths)} checked)"
    return True, ""


def check_log(o: dict, sb: Sandbox, marks: dict) -> tuple[bool, str]:
    hits = []
    for path in sb.logs():
        try:
            with open(path, encoding="utf-8", errors="replace") as fh:
                fh.seek(marks.get(path, 0))
                new = fh.read()
        except OSError:
            continue
        for line in new.splitlines():
            if any(word in line for word in o.get("none_of", [])):
                hits.append(f"{os.path.basename(os.path.dirname(path)) or 'log'}: {line.strip()[:200]}")
    return (not hits), "; ".join(hits[:3])


def capture_app_windows(sb: Sandbox, out_png_prefix: str) -> list[str]:
    """Every on-screen window of the sandbox app (palettes, panels, overlays included).

    Captured window by window, so nothing outside the app's own windows is recorded."""
    pid = sb.pid()
    if not pid:
        return []
    r = subprocess.run([KS_INPUT, "windows", "--pid", str(pid)], capture_output=True, text=True)
    try:
        wins = json.loads(r.stdout or "[]")
    except json.JSONDecodeError:
        wins = []
    shots = []
    for i, w in enumerate(wins):
        wid = w.get("id") or w.get("windowId") or w.get("number")
        if not wid:
            continue
        png = f"{out_png_prefix}-w{i}.png"
        if subprocess.run(["screencapture", "-x", "-o", "-l", str(wid), png]).returncode == 0 and os.path.exists(png):
            shots.append(png)
    return shots


def ocr_text(png: str) -> list[str]:
    r = subprocess.run([KS_OCR, png], capture_output=True, text=True)
    try:
        return [l["text"] for l in json.loads(r.stdout)["lines"]]
    except Exception:
        return []


def check_visual(o: dict, sb: Sandbox, prefix: str) -> tuple[str, str, list[str]]:
    shots = capture_app_windows(sb, prefix)
    text = []
    for p in shots:
        lines = ocr_text(p)
        text += lines
        with open(p[:-4] + ".ocr.txt", "w", encoding="utf-8") as fh:
            fh.write("\n".join(lines))
    blob = "\n".join(text).lower()
    finds, absents = o.get("ocr_find", []), o.get("ocr_absent", [])
    if isinstance(finds, str):
        finds = [finds]
    if isinstance(absents, str):
        absents = [absents]
    if not finds and not absents:
        return "eyes", o.get("expect", ""), shots
    if not shots:
        # No capture (no Screen Recording grant, e.g. in the VM): undecidable, not a failure.
        return "eyes", "no screenshot could be taken (Screen Recording?): " + o.get("expect", ""), shots
    missing = [f for f in finds if f.lower() not in blob]
    present = [a for a in absents if a.lower() in blob]
    if missing or present:
        return "fail", f"OCR missing {missing} / unexpected {present}", shots
    return "pass", "", shots


def check_independent(o: dict, sb: Sandbox, st: dict, out: str) -> tuple[str, str]:
    if "cmd" not in o:
        return "agent", o.get("describe", "")
    state_file = os.path.join(out, "state-snapshot.json")
    json.dump(st, open(state_file, "w"))
    env = dict(os.environ, SBX_HOME=sb.home, KS_SOCKET=sb.sock, STATE_FILE=state_file, ROOT=ROOT)
    r = subprocess.run(o["cmd"], shell=True, env=env, capture_output=True, text=True, timeout=60)
    got = r.stdout.strip()
    if r.returncode != 0:
        return "fail", f"cmd exit {r.returncode}: {(r.stderr or got).strip()[:300]}"
    if "expect" in o and got != str(o["expect"]):
        return "fail", f"cmd printed {got[:200]!r}, expected {o['expect']!r}"
    return "pass", ""


# ---------------------------------------------------------------- flows


def expand(s: str, sb: Sandbox) -> str:
    return s.replace("$HOME", sb.home).replace("~/", sb.home + "/")


def run_step(sb: Sandbox, inp: str, replies: dict) -> None:
    kind, _, arg = inp.partition(" ")
    if kind == "wait":
        time.sleep(int(arg) / 1000)
        return
    if kind in ("key", "type"):
        before = (sb.state(quiet=True) or {}).get("input", {}).get("dropped", 0)
        sb.do(("input:key:" if kind == "key" else "input:text:") + arg)
        time.sleep(0.15)
        after = (sb.state(quiet=True) or {}).get("input", {})
        if after.get("dropped", 0) > before:
            raise StepError(f"{inp}: input dropped ({after.get('lastError')})")
        return
    if kind == "click-rel":
        fx, fy = (float(x) for x in arg.split())
        st = sb.state() or {}
        v = st.get("view") or "notes"
        frame = (st.get("views", {}).get(v, {}) or {}).get("frame")
        if not frame:
            raise StepError(f"{inp}: no frame for view {v!r}")
        sb.do(f"input:click:{fx * frame[2]:.0f},{fy * frame[3]:.0f}")
        return
    if inp.startswith("do:"):
        # A do: step may be the thing under test (an expected error): keep the reply
        # for `reply` oracles instead of aborting the flow.
        verb = expand(inp, sb)
        raw = sb.send(verb, timeout=20)
        try:
            replies[verb] = json.loads(raw) if raw else {}
        except json.JSONDecodeError:
            replies[verb] = {"raw": raw}
        return
    if kind == "raw":
        sb.send(expand(arg, sb))
        return
    raise StepError(f"unknown input kind: {inp!r}")


def run_flow(sb: Sandbox, flow: dict, out: str, tag: str) -> dict:
    res = {"id": flow["id"], "feature": flow.get("feature"), "run": tag, "verdict": "PASS",
           "failed": [], "pending": [], "error": None, "shots": []}
    marks = {p: os.path.getsize(p) for p in sb.logs() if os.path.exists(p)}
    replies: dict[str, object] = {}
    by_cp: dict[str | None, list[dict]] = {}
    for o in flow["oracles"]:
        by_cp.setdefault(o.get("after"), []).append(o)

    def evaluate(cp):
        st = sb.state() or {}
        for o in by_cp.pop(cp, []):
            kind = o["kind"]
            where = f"@{cp}" if cp else "@end"
            if kind == "state":
                ok, why = check_state(expand_expected(o, sb.home), st)
                deadline = time.time() + SETTLE_MS / 1000
                while not ok and time.time() < deadline:
                    time.sleep(0.25)
                    st = sb.state() or st
                    ok, why = check_state(expand_expected(o, sb.home), st)
                if not ok:
                    res["failed"].append(f"state {where}: {why}")
            elif kind == "disk":
                ok, why = check_disk(o, sb.real_home)
                if not ok:
                    res["failed"].append(f"disk {where}: {why}")
            elif kind == "log":
                ok, why = check_log(o, sb, marks)
                if not ok:
                    res["failed"].append(f"log {where}: {why}")
            elif kind == "reply":
                got = replies.get(expand(o["verb"], sb))
                if got is None:
                    res["failed"].append(f"reply {where}: {o['verb']} was never sent")
                elif "error" in o:
                    err = got.get("error") if isinstance(got, dict) else None
                    if not err or (o["error"] and o["error"] not in str(err)):
                        res["failed"].append(f"reply {where}: {o['verb']} replied {json.dumps(got)[:160]}, expected error {o['error']!r}")
                elif isinstance(got, dict) and "error" in got:
                    res["failed"].append(f"reply {where}: {o['verb']} errored: {got['error']}")
            elif kind == "independent":
                v, why = check_independent(o, sb, st, out)
                if v == "fail":
                    res["failed"].append(f"independent {where}: {why}")
                elif v == "agent":
                    res["pending"].append({"kind": "agent", "at": where, "describe": why})
            elif kind == "visual":
                prefix = os.path.join(out, "shots", f"{tag}-{flow['id']}-{cp or 'end'}")
                v, why, shots = check_visual(o, sb, prefix)
                res["shots"] += [os.path.relpath(p, out) for p in shots]
                if v == "fail":
                    res["failed"].append(f"visual {where}: {why}")
                elif v == "eyes":
                    res["pending"].append({"kind": "eyes", "at": where, "expect": why,
                                           "shots": [os.path.relpath(p, out) for p in shots]})

    try:
        for verb in flow.get("setup", []):
            try:
                sb.do(expand(verb, sb))
            except (OSError, socket.timeout) as e:
                raise StepError(f"setup {verb}: {e or 'timed out'}")
            time.sleep(0.4)
        # Setup presents the view's window; only then is there a window to make key.
        # Flows that send no key/text/click don't need focus (a panel-only flow has no
        # shared window to make key).
        if any(st["input"].split(" ")[0] in ("key", "type", "click-rel") for st in flow["steps"]):
            sb.activate()
        for i, step in enumerate(flow["steps"], 1):
            try:
                run_step(sb, step["input"], replies)
            except (OSError, socket.timeout) as e:
                raise StepError(f"step {i} {step['input']!r}: {e or 'timed out'}")
            if step.get("checkpoint"):
                evaluate(step["checkpoint"])
        evaluate(None)
        for cp in list(by_cp):
            res["failed"].append(f"checkpoint {cp!r} never reached (oracles not checked)")
    except (StepError, OSError, socket.timeout) as e:
        res["error"] = str(e)
    finally:
        for verb in flow.get("teardown", []):
            try:
                sb.do(expand(verb, sb))
            except Exception:
                pass
    if sb.state(quiet=True) is None:
        res["error"] = (res["error"] + "; " if res["error"] else "") + "app stopped answering (crash or hang)"
        res["crashed"] = True
    if res["error"]:
        res["verdict"] = "ERROR"
    elif res["failed"]:
        res["verdict"] = "FAIL"
    return res


# ---------------------------------------------------------------- replay


def finding_to_flow(f: dict) -> dict:
    """A worker finding (TESTING.md §7) as a flow whose single oracle expects the bug gone."""
    trail = [{"input": s["input"] if isinstance(s, dict) else s} for s in f.get("trail", [])]
    o = f.get("oracle", {})
    if "kind" in o:
        oracle = dict(o)
        oracle.pop("after", None)
    else:
        cmd = o.get("cmd", "false")
        oracle = {"kind": "independent", "cmd": cmd, "expect": str(o.get("expected", ""))}
    return {"id": f.get("id", "finding"), "feature": f.get("feature"), "setup": f.get("setup", []),
            "steps": trail, "oracles": [oracle], "teardown": []}


# ---------------------------------------------------------------- main


def write_summary(out: str, results: list[dict], replay: bool) -> str:
    lines = [f"# Flow run {os.path.basename(out)}", ""]
    if replay:
        lines += ["| finding | reproduced | detail |", "|---|---|---|"]
        by = {}
        for r in results:
            by.setdefault(r["id"], []).append(r)
        for fid, rs in by.items():
            rep = sum(r["verdict"] == "FAIL" for r in rs)
            err = sum(r["verdict"] == "ERROR" for r in rs)
            detail = next((r["failed"][0] for r in rs if r["failed"]), "") or next((r["error"] for r in rs if r["error"]), "")
            lines.append(f"| {fid} | {rep}/{len(rs)}{f' ({err} error)' if err else ''} | {detail[:160]} |")
    else:
        counts = {v: sum(r["verdict"] == v for r in results) for v in ("PASS", "FAIL", "ERROR")}
        pend = sum(len(r["pending"]) for r in results)
        lines += [f"PASS {counts['PASS']} · FAIL {counts['FAIL']} · ERROR {counts['ERROR']} · "
                  f"pending checks (agent/eyes) {pend}", "",
                  "| flow | run | verdict | detail |", "|---|---|---|---|"]
        for r in results:
            detail = r["error"] or "; ".join(r["failed"]) or (
                "pending: " + ", ".join(p["kind"] for p in r["pending"]) if r["pending"] else "")
            lines.append(f"| {r['id']} | {r['run']} | {r['verdict']} | {detail[:220].replace('|', '/')} |")
    text = "\n".join(lines) + "\n"
    open(os.path.join(out, "summary.md"), "w").write(text)
    return text


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--table", default=TABLE)
    ap.add_argument("--ids", help="comma-separated flow ids")
    ap.add_argument("--feature", help="only flows of this feature")
    ap.add_argument("--replay", help="findings.jsonl to replay instead of the flow table")
    ap.add_argument("--id", help="with --replay: only this finding id")
    ap.add_argument("--repeat", type=int, default=1)
    ap.add_argument("--app", default="fresh", help="'fresh' (build the working tree), 'installed', or a bundle path")
    ap.add_argument("--out", help="output dir (default .build/flow-runs/<timestamp>)")
    ap.add_argument("--list", action="store_true")
    ap.add_argument("--settle", type=int, default=0, metavar="MS",
                    help="re-read state for up to MS ms before failing a state oracle (VM: 1500)")
    ap.add_argument("--shared", action="store_true",
                    help="run flows in one app session (faster; flows can leak state into each other)")
    ap.add_argument("--allow-screen-capture", action="store_true",
                    help="also run flows that need 'screen-capture' (they capture the REAL screen)")
    a = ap.parse_args()

    global SETTLE_MS
    SETTLE_MS = a.settle
    table = json.load(open(a.table, encoding="utf-8"))
    if a.replay:
        flows = [finding_to_flow(json.loads(l)) for l in open(a.replay, encoding="utf-8") if l.strip()]
        if a.id:
            flows = [f for f in flows if f["id"] == a.id]
    else:
        flows = table["flows"]
        if a.ids:
            want = str(a.ids).split(",")
            unknown = set(want) - {f["id"] for f in flows}
            if unknown:
                print(f"unknown flow ids: {sorted(unknown)}", file=sys.stderr)
                return 2
            flows = [f for f in flows if f["id"] in want]
        if a.feature:
            flows = [f for f in flows if f.get("feature") == a.feature]
    if not a.allow_screen_capture:
        skipped = [f["id"] for f in flows if "screen-capture" in f.get("needs", [])]
        flows = [f for f in flows if "screen-capture" not in f.get("needs", [])]
        if skipped:
            log(f"skipping (needs screen-capture; --allow-screen-capture to run): {', '.join(skipped)}")
    if a.list:
        for f in flows:
            print(f"{f['id']:32} {f.get('feature', ''):16} {f.get('why', '')[:80]}")
        return 0
    if not flows:
        print("no flows selected", file=sys.stderr)
        return 2

    out = a.out or os.path.join(ROOT, ".build", "flow-runs", time.strftime("%Y%m%d-%H%M%S"))
    os.makedirs(os.path.join(out, "shots"), exist_ok=True)
    os.makedirs(os.path.dirname(GUI_LOCK), exist_ok=True)
    lock = open(GUI_LOCK, "w")
    try:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        print("another GUI sandbox run holds .build/gui-sandbox.lock (one GUI driver at a time)", file=sys.stderr)
        return 2

    app = {"fresh": None, "installed": os.path.join(ROOT, "kitchen-sink.app")}.get(a.app, a.app)
    app = app or prepare_fresh_app()
    sb = Sandbox(app)
    sb.reset()
    make_fixtures(table, sb.real_home)
    results = []
    try:
        sb.start()
        for n in range(1, a.repeat + 1):
            for i, flow in enumerate(flows):
                tag = f"r{n}"
                if not a.shared and (n, i) != (1, 0):
                    # Fresh app + fresh fixtures for every flow: no leaked cwd, sessions, settings.
                    sb.stop()
                    sb.reset()
                    make_fixtures(table, sb.real_home)
                    sb.start()
                r = run_flow(sb, flow, out, tag)
                results.append(r)
                with open(os.path.join(out, "results.jsonl"), "a", encoding="utf-8") as fh:
                    fh.write(json.dumps(r, ensure_ascii=False) + "\n")
                note = r["error"] or "; ".join(r["failed"]) or ""
                log(f"{r['verdict']:5} {tag} {flow['id']}" + (f"  — {note[:160]}" if note else "")
                    + (f"  [+{len(r['pending'])} pending]" if r["pending"] else ""))
                if r.get("crashed"):
                    log("app died; restarting the sandbox for the next flow")
                    sb.restart()
    finally:
        sb.stop()
    print()
    print(write_summary(out, results, bool(a.replay)))
    print(f"results: {out}")
    if a.replay:
        return 1 if any(r["verdict"] == "FAIL" for r in results) else 0
    return 1 if any(r["verdict"] in ("FAIL", "ERROR") for r in results) else 0


if __name__ == "__main__":
    sys.exit(main())
