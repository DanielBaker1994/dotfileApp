#!/usr/bin/env python3
"""Haiku agents drive the real app in the test VMs (real input + screenshots), in parallel.

    caffeinate -i -s python3 Tests/haiku_run.py                 # $5 budget, both VMs
    python3 Tests/haiku_run.py --budget 2 --vms ks-test         # one VM
    python3 Tests/haiku_run.py --areas files --budget 0.5       # a quick pilot

One agent per VM at a time (one GUI sandbox per VM; Apple allows two macOS VMs per host).
Each agent is a fresh `claude -p --model haiku` whose ONLY tools are `bin/ks-drive`
(start/key/text/click/shot/state/log, scoped to its VM by KS_VM) and Read (to look at its
screenshots). It cannot edit the repo; its findings come back in its final message and this
script writes them to .build/haiku/<run>/. Nothing is fixed or committed: the owner (or the
orchestrator) reviews HAIKU.md.

Guards: a per-agent spend cap (--agent-cap), the run budget (no new agent once spent + cap
would exceed it), a per-agent wall clock, and free disk: below --min-free-gb the second VM
is retired (stopped and deleted) and the run carries on with one.
"""
from __future__ import annotations

import argparse
import json
import os
import queue
import re
import shutil
import subprocess
import threading
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
TART = os.path.expanduser("~/.local/bin/tart")
RUN = time.strftime("%Y%m%d-%H%M")
OUT = os.path.join(ROOT, ".build", "haiku", RUN)
LOG: list[str] = []
LOCK = threading.Lock()

AREAS = {
    "files": "the Files view: open it (do open:files), browse folders with the keyboard (Return, Backspace/Cmd+Up, arrows), "
             "filter (Cmd+L, type-ahead), rename (F2), new folder (Cmd+Shift+N), duplicate (Cmd+D), undo (Cmd+Z), hidden files (Cmd+Shift+.)",
    "notes": "Notes with the plain editor: switch notes in the sidebar, Cmd+N new note, type, Cmd+S save, Cmd+F find, rename, "
             "tabs, the prose (rendered) toggle, long and unicode text",
    "compare-text": "Compare two text files: open Compare (do open:compare), use its start page to pick two files under ~/compare, "
                    "jump between differences (Ctrl+N / Ctrl+P), copy a difference across and undo, swap sides, filters Cmd+1/Cmd+2",
    "compare-folder": "Folder compare: compare two folders under ~/compare, move through rows, expand folders, open a differing "
                      "file (Return) and come back (Esc), filter the rows",
    "shell": "the shared window: the view switcher (Ctrl+B then w), Ctrl+Tab between views, the command palette (Cmd+K) and its "
             "tools, the terminal and browser drawers, Esc to hide, Cmd+W, Cmd+0 window reset, the shortcuts sheet (Cmd+/)",
    "keyboard-only": "a keyboard-only user: starting from Notes, reach every view, do one real action in each, and get back, "
                     "never clicking. Note every place the keyboard gets stuck or focus is unclear",
    "visual": "visual quality: take a screenshot of every view and of every sheet/popover/drawer you can open; look for clipped "
              "or overlapping text, misaligned controls, unreadable contrast, missing focus indication, empty states with no hint",
}

BRIEF = """You are testing the kitchen-sink macOS app (a keyboard-driven notes/files/compare workspace) by
USING it like a real person, inside a test VM. Area: {area}.

Your only way to act is the shell command `bin/ks-drive` (run from the repo root); it talks to the app in
the VM. Commands:
  bin/ks-drive start                 fresh test data + launch the app (do this first; ~20 s)
  bin/ks-drive do open:files         open a view (notes, files, compare, jira…)
  bin/ks-drive key cmd+l             a real key press (combos: cmd+n, 'ctrl+b w', esc, return, f2, down…)
  bin/ks-drive text 'hello'          type text into whatever has focus
  bin/ks-drive click X Y             click at points from the window's top-left (window is 900x500)
  bin/ks-drive shot NAME             screenshot of the window -> prints the saved PNG path; LOOK at it with Read
  bin/ks-drive state [path]          the app's state JSON (e.g. `state view`, `state pane.focused`, `state views.files.rows`)
  bin/ks-drive log 30                the app's log tail (look for panic / ERROR)
After every key/text/click it prints a short summary (view, focused pane, last toast, dropped input).
Test data lives under the app's HOME: ~/notes (Alpha.md, Beta.md, Quill.md), ~/files, ~/compare.

Known capture limit: screenshots of the Notes view come out blank white (the capture can't see how
Notes draws). Do NOT report a blank/white Notes screenshot; test Notes through `state` and the other views.

How to work:
- Take a screenshot early and after anything surprising, and actually look at it.
- Try things a user would try, including slightly wrong things (Esc in odd places, keys twice, undo).
- A finding is: a crash/hang (state stops answering), something that does not do what its label/shortcut
  says, a key that does nothing with no feedback, focus you can't see or can't get back from, visual
  breakage (clipped/overlapping text, unreadable, misaligned), or a confusing flow.
- REPRODUCE each finding before reporting it: `bin/ks-drive start`, repeat the exact steps, confirm it
  happens again. Report only reproduced findings (say "2/2").
- Stop after about 25 minutes of work or when you have 6 solid findings. Budget is limited: be efficient,
  don't dump full state unless needed (use a path).

Your FINAL message must be ONLY this, nothing else:
<findings>
{{"title": "...", "kind": "crash|bug|no-feedback|focus|visual|confusing", "sev": "S1|S2|S3|S4", "steps": ["bin/ks-drive start", "bin/ks-drive do open:files", "bin/ks-drive key f2", "..."], "expected": "...", "actual": "...", "evidence": "state values / screenshot names", "repro": "2/2"}}
... one JSON object per line ...
</findings>
<notes>2-5 lines: what you covered, what you could not test and why</notes>
"""


def log(msg: str) -> None:
    line = f"[{time.strftime('%H:%M:%S')}] {msg}"
    with LOCK:
        print(line, flush=True)
        LOG.append(line)
        with open(os.path.join(OUT, "run.log"), "a") as fh:
            fh.write(line + "\n")


def sh(cmd: list[str], env: dict | None = None, timeout: int = 1800) -> tuple[int, str]:
    try:
        r = subprocess.run(cmd, cwd=ROOT, env=env, capture_output=True, text=True, timeout=timeout)
        return r.returncode, r.stdout + r.stderr
    except subprocess.TimeoutExpired:
        return 124, "timeout"


def free_gb() -> float:
    return shutil.disk_usage(os.path.expanduser("~")).free / 2**30


def vm_env(vm: str, shots: str | None = None) -> dict:
    env = dict(os.environ, KS_VM=vm, PATH=os.path.expanduser("~/.local/bin") + ":" + os.environ["PATH"])
    if shots:
        env["KS_DRIVE_OUT"] = shots
    return env


def retire_vm(vm: str) -> None:
    log(f"retiring {vm}: free disk {free_gb():.1f} GB")
    sh(["bin/ks-vm", "down"], env=vm_env(vm))
    if vm != "ks-test":
        sh([TART, "delete", vm])


def run_agent(vm: str, area: str, cap: float, minutes: int) -> dict:
    d = os.path.join(OUT, area)
    os.makedirs(os.path.join(d, "shots"), exist_ok=True)
    brief = BRIEF.format(area=AREAS[area])
    open(os.path.join(d, "brief.txt"), "w").write(brief)
    cmd = ["claude", "-p", brief, "--model", "haiku", "--output-format", "json",
           "--max-budget-usd", f"{cap:.2f}",
           "--allowedTools", "Bash(bin/ks-drive:*)", "Read"]
    t0 = time.time()
    rc, out = sh(cmd, env=vm_env(vm, os.path.join(d, "shots")), timeout=minutes * 60)
    open(os.path.join(d, "raw.json"), "w").write(out)
    res: dict = {}
    try:
        res = json.loads(out[out.index("{"):])
    except Exception:
        pass
    text = res.get("result") or ""
    cost = float(res.get("total_cost_usd") or 0.0)
    m = re.search(r"<findings>(.*?)</findings>", text, re.S)
    findings = []
    for line in (m.group(1).splitlines() if m else []):
        line = line.strip()
        if line.startswith("{"):
            try:
                findings.append(json.loads(line))
            except json.JSONDecodeError:
                pass
    nm = re.search(r"<notes>(.*?)</notes>", text, re.S)
    notes = nm.group(1).strip() if nm else ""
    with open(os.path.join(d, "findings.jsonl"), "w") as fh:
        for f in findings:
            fh.write(json.dumps(f, ensure_ascii=False) + "\n")
    open(os.path.join(d, "report.md"), "w").write(text)
    log(f"  {area} on {vm}: rc={rc}, {len(findings)} findings, ${cost:.3f}, "
        f"{(time.time() - t0) / 60:.0f} min, turns {res.get('num_turns')}")
    return {"area": area, "vm": vm, "rc": rc, "cost": cost, "findings": findings, "notes": notes,
            "minutes": round((time.time() - t0) / 60), "error": None if res else out[-300:]}


def report(results: list[dict], budget: float) -> None:
    spent = sum(r["cost"] for r in results)
    lines = [f"# Haiku app run {RUN}", "",
             f"Agents: {len(results)} · spent ${spent:.2f} of ${budget:.2f} · findings "
             f"{sum(len(r['findings']) for r in results)} (report-only, nothing fixed)", "",
             "| area | VM | findings | cost | minutes | notes |", "|---|---|---|---|---|---|"]
    for r in results:
        note = (r["notes"] or r["error"] or "").replace("\n", " ").replace("|", "/")[:200]
        lines.append(f"| {r['area']} | {r['vm']} | {len(r['findings'])} | ${r['cost']:.2f} | {r['minutes']} | {note} |")
    lines += ["", "## Findings (most severe first)", ""]
    allf = [(r["area"], f) for r in results for f in r["findings"]]
    allf.sort(key=lambda af: str(af[1].get("sev", "S9")))
    for area, f in allf:
        lines += [f"### {f.get('sev', '?')} [{area}/{f.get('kind', '?')}] {f.get('title', '')}",
                  f"- expected: {f.get('expected', '')}", f"- actual: {f.get('actual', '')}",
                  f"- evidence: {f.get('evidence', '')} · repro {f.get('repro', '?')}",
                  "- steps: `" + "` → `".join(f.get("steps") or []) + "`", ""]
    lines += ["## Log", "", *LOG]
    open(os.path.join(OUT, "HAIKU.md"), "w").write("\n".join(lines) + "\n")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--budget", type=float, default=5.0)
    ap.add_argument("--agent-cap", type=float, default=0.75)
    ap.add_argument("--minutes", type=int, default=40, help="wall clock per agent")
    ap.add_argument("--vms", default="ks-test,ks-test2")
    ap.add_argument("--areas", default=",".join(AREAS))
    ap.add_argument("--min-free-gb", type=float, default=5.0)
    ap.add_argument("--keep-vms-up", action="store_true")
    a = ap.parse_args()
    os.makedirs(OUT, exist_ok=True)
    vms = [v for v in a.vms.split(",") if v]
    areas = [x for x in a.areas.split(",") if x in AREAS]
    log(f"haiku run {RUN}: areas {areas}, VMs {vms}, budget ${a.budget}, cap ${a.agent_cap}/agent, "
        f"free disk {free_gb():.1f} GB")
    live = []
    for vm in vms:  # serial: the first build compiles, the second reuses it
        rc, out = sh(["bin/ks-vm", "sync"], env=vm_env(vm))
        if rc == 0:
            live.append(vm)
            log(f"  {vm}: synced")
        else:
            log(f"  {vm}: sync failed, not used: {out.strip()[-200:]}")
    if not live:
        log("no VM available; stopping")
        report([], a.budget)
        return 1

    todo: queue.Queue = queue.Queue()
    for x in areas:
        todo.put(x)
    results: list[dict] = []
    spent = [0.0]

    def lane(vm: str) -> None:
        while True:
            if vm != "ks-test" and free_gb() < a.min_free_gb:
                retire_vm(vm)
                return
            with LOCK:
                if spent[0] + a.agent_cap > a.budget:
                    return
                spent[0] += a.agent_cap  # reserve; settled after the agent
            try:
                area = todo.get_nowait()
            except queue.Empty:
                with LOCK:
                    spent[0] -= a.agent_cap
                return
            log(f"  start {area} on {vm}")
            r = run_agent(vm, area, a.agent_cap, a.minutes)
            sh(["bin/ks-drive", "stop"], env=vm_env(vm), timeout=120)
            with LOCK:
                spent[0] += r["cost"] - a.agent_cap
                results.append(r)

    threads = [threading.Thread(target=lane, args=(vm,)) for vm in live]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    left = []
    while not todo.empty():
        left.append(todo.get())
    if left:
        log(f"not run (budget or VMs): {left}")
    log(f"done: spent ${sum(r['cost'] for r in results):.2f}, free disk {free_gb():.1f} GB")
    report(results, a.budget)
    if not a.keep_vms_up:
        for vm in vms:
            sh(["bin/ks-vm", "down"], env=vm_env(vm))
    log(f"report: {OUT}/HAIKU.md")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
