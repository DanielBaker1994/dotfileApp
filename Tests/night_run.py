#!/usr/bin/env python3
"""Unattended overnight bug hunt + fix pipeline (DeepSeek v4.1-flash workers, VM for GUI).

    caffeinate -i -s python3 Tests/night_run.py            # everything, ~5-7 h
    python3 Tests/night_run.py --phases find,run           # a subset

Phases (each logs to .build/night/<run>/, failures never stop the next phase):
  find   parallel, nothing on screen: 4 flow-author workers (new flows from the docs) +
         3 headless hunters (compare vs difflib, config fuzz, random state walk)
  run    validate the authored flows, run them in the VM (Tests/run_flows.py, no LLM)
  triage one worker per failing new flow (serial, VM): flow wrong -> corrected flow, or app bug
  fix    one worker per bug (serial, main tree): commit only if cargo test passes AND the
         full VM table has no new failures; otherwise the work is stashed (git stash list)
  final  full VM run + SUMMARY.md

Workers never commit; this script commits (docs/tests, and gated fixes) on main.
"""
from __future__ import annotations

import argparse
import concurrent.futures as cf
import json
import os
import re
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RELAY = os.path.expanduser("~/.claude/skills/opencode-delegate/scripts/relay.mjs")
MODEL = "opencode-go/deepseek-v4.1-flash"
TABLE = os.path.join(ROOT, "Tests", "app_flows.json")
NIGHT_TABLE = os.path.join(ROOT, "Tests", "app_flows_night.json")  # synced into the VM
OWNER_UNTRACKED = ["PLAN-prose-popout.md"]
ENV = dict(os.environ, PATH=os.path.expanduser("~/.local/bin") + ":" + os.environ["PATH"],
           CARGO_TARGET_DIR=os.path.join(ROOT, ".build", "rust-target"))

RUN = time.strftime("%Y%m%d-%H%M")
OUT = os.path.join(ROOT, ".build", "night", RUN)
LOG = []


def log(msg: str) -> None:
    line = f"[{time.strftime('%H:%M:%S')}] {msg}"
    print(line, flush=True)
    LOG.append(line)
    with open(os.path.join(OUT, "night.log"), "a") as fh:
        fh.write(line + "\n")


def sh(cmd: str, timeout: int = 3600, cwd: str = ROOT) -> tuple[int, str]:
    try:
        r = subprocess.run(cmd, shell=True, cwd=cwd, env=ENV, capture_output=True, text=True, timeout=timeout)
        return r.returncode, (r.stdout + r.stderr)
    except subprocess.TimeoutExpired as e:
        return 124, f"timeout after {timeout}s: {e}"


def git_commit(paths: list[str], message: str) -> int:
    if subprocess.run(["git", "add", "-A", "--", *paths], cwd=ROOT).returncode != 0:
        return 1
    return subprocess.run(["git", "commit", "-q", "-m", message], cwd=ROOT).returncode


def git_dirty() -> list[str]:
    _, out = sh("git status --porcelain")
    return [l for l in out.splitlines() if l.strip() and not any(l.endswith(f) for f in OWNER_UNTRACKED)]


def stash(label: str) -> None:
    if git_dirty():
        excl = " ".join(f"':!{f}'" for f in OWNER_UNTRACKED)
        rc, out = sh(f"git stash push -u -m 'night {RUN}: {label}' -- . {excl}")
        log(f"  stashed ({label}): rc={rc} {out.strip()[:200]}")


def worker(name: str, brief: str, timeout_min: int = 45) -> str:
    """Run one DeepSeek worker; returns its final message ('' on failure)."""
    d = os.path.join(OUT, "workers", name)
    os.makedirs(d, exist_ok=True)
    bf = os.path.join(d, "brief.txt")
    open(bf, "w").write(brief)
    rc, out = sh(f"node {RELAY} --brief {bf} --model {MODEL} --cd {ROOT} --timeout {timeout_min}m "
                 f"--out-dir {d}/relay > {d}/relay.log 2>&1", timeout=timeout_min * 60 + 300)
    try:
        res = json.load(open(os.path.join(d, "relay", "result.json")))
        msg = res.get("finalMessage") or ""
    except Exception:
        msg = ""
    open(os.path.join(d, "report.md"), "w").write(msg)
    log(f"  worker {name}: rc={rc}, report {len(msg)} chars")
    return msg


COMMON_RULES = """HARD RULES
- Never run ./ws (it kills the owner's live app), never pkill, never git commit/stash/checkout/reset.
- Never start the GUI sandbox on this Mac (`ks-mcp-sandbox.sh start --gui`): GUI work happens ONLY
  inside the test VM via `bin/ks-vm ssh '…'` / `bin/ks-vm test …`. Headless sandboxes are fine.
- Never use any model other than the one you are; never edit files outside what the task names.
- Two failed attempts at the same step: write down what you saw and move on. No retry loops.
- Pass/fail comes from machine checks (state JSON, files on disk, exit codes, logs), never opinion.
Read TESTING.md §3 for the exact commands, Tests/app_flows.md for the flow schema, APP-BUGS.md for
known bugs (do not re-report them).
"""


# ---------------------------------------------------------------- phase: find

AUTHOR_AREAS = {
    "files": "the Files view (views/files.rs): listing, filter (Cmd+L), Return/Backspace navigation, rename (F2/Cmd+R; rename keeps the extension), new folder/file, duplicate (Cmd+D), copy path, hidden toggle, sorting, Cmd+Z undo",
    "compare": "Compare (views/compare.rs): text compare of two files, folder compare, Cmd+1/2 filters, swap sides, copy across, Home/End/Up/Down, Return on a folder row, start page Cmd+0, identical files, empty files",
    "notes": "Notes with the PLAIN editor (vim-mode=false in the fixture; no nvim in the VM): Cmd+N, typing, Cmd+S save to disk, Cmd+F find bar, tabs in the sidebar, rename, switching notes, unicode, long text, the prose toggle",
    "shell": "the shared window and its surfaces: Ctrl+B W switcher, Ctrl+Tab view cycling, Cmd+0, the palette tools (Pretty Print, Text to Table, Create File, Health Checks, Find Recent Files), paths shelf, terminal/browser drawers, esc-hides, header styles, config reload, Cmd+W",
}

AUTHOR_BRIEF = """TASK: write NEW feature test flows for the kitchen-sink app: {area}.
Repo: {root}. Read: Tests/app_flows.md (runner contract), Tests/app_flows.json (schema + the 45
existing flows; do not duplicate them), TESTING.md §3.0/§3.7b, PRODUCT.md, commands.toml shortcut rows,
and the source files named above.

OUTPUT (the ONLY file you write): {out}/flows.json = {{"fixtures": [...new fixtures, may be empty...],
"flows": [...8 to 10 new flows...]}}. Same schema as Tests/app_flows.json. Every flow id starts with
"n-{key}-". Each flow: id, feature, why, sources (real file:line you read), needs ["gui"], setup,
steps, oracles, teardown.
- Inputs: "key <combo>", "type <text>", "wait <ms>", "do:<verb>", "raw <verb>" (no click-rel).
- Oracles must be machine-decidable: state / disk / log / reply, or independent WITH a "cmd" (bash,
  exit 0 = pass; env STATE_FILE, SBX_HOME, ROOT). NO visual oracles (no screen capture in the VM).
  Every flow ends with {{"kind":"log","none_of":["panic","ERROR"]}}.
- Fixtures: reuse the existing ones (paths relative to the sandbox HOME); new files go in "fixtures".
- Aim at real bugs: edge cases (empty, unicode, long names, rapid repeats, undo after an action,
  doing X then switching view and back), not happy paths already covered.
- Check every state path you assert actually exists: look at a real state dump with
  `bin/ks-vm ssh 'cat ~/ks/.build/fr/tmp/state.json'` is NOT available; instead read the state
  builders (host.rs state_with_registry, the view's test_state) and the existing flows' paths.
VERIFY your flows in the VM before finishing (this runs them for real, ~10 s per flow, nothing on screen):
  cp {out}/flows.json /tmp/x.json   # then build a table: base table + your flows
  python3 -c 'import json;b=json.load(open("Tests/app_flows.json"));n=json.load(open("{out}/flows.json"));b["fixtures"]+=n.get("fixtures",[]);b["flows"]=n["flows"];json.dump(b,open("Tests/app_flows_{key}.json","w"),indent=1,ensure_ascii=False)'
  bin/ks-vm test --table Tests/app_flows_{key}.json
  (then delete Tests/app_flows_{key}.json). Only ONE worker may use the VM at a time: if
  `bin/ks-vm test` prints "another GUI sandbox run holds", wait 2 minutes and try again (max 5 times).
A flow that fails: decide from the app's code whether YOUR expectation is wrong (fix the flow) or the
app is wrong (keep the flow, add "suspected_bug": "<one line>" to it). Report at the end: flows written,
pass/fail in the VM, suspected bugs.
""" + COMMON_RULES

HUNT_BRIEFS = {
    "h-cmp": """TASK: hunt bugs in kitchen-sink's text/folder COMPARE engine, headless (nothing on screen).
Start a headless sandbox: export KS_MCP_SANDBOX={root}/.build/hl-cmp; bin/ks-mcp-sandbox.sh start
(KS_SOCKET=$(bin/ks-mcp-sandbox.sh sock), SBX_HOME=$(bin/ks-mcp-sandbox.sh home)). Write pairs of files
under $SBX_HOME/cmp/ and load each with: printf 'compare\\t%s\\t%s\\n' A B | nc -U -w5 "$KS_SOCKET", then
read state (printf 'state\\n' | nc -U -w3 "$KS_SOCKET") .compare.current (sections, important, rows,
leftLines, rightLines). Compare against Python difflib (SequenceMatcher opcodes: non-equal blocks =
sections; line counts). At least 40 cases: empty vs empty, empty vs text, identical, CRLF vs LF, no
trailing newline, unicode/emoji, tabs vs spaces, 5000 lines with one change, all lines changed, one
huge line (200 KB), binary-ish bytes, same content different mtime; and folder pairs via
printf 'compare\\t%s/\\t%s/\\n' (same size different content, nested dirs, empty dir, symlink).
Also watch {root}/.build/hl-cmp/daemon.log for panics after every case.
OUTPUT: {out}/findings.jsonl (one JSON per mismatch/crash: id, sev S1-S4, title, files (contents or a
python one-liner that recreates them), expected (difflib), actual (state), repeat k/3) and
{out}/summary.md. Stop the sandbox at the end: bin/ks-mcp-sandbox.sh stop.
""",
    "h-cfg": """TASK: hunt crashes/bad handling of broken config in kitchen-sink, headless (nothing on screen).
Start a headless sandbox: export KS_MCP_SANDBOX={root}/.build/hl-cfg; bin/ks-mcp-sandbox.sh start;
KS_SOCKET=$(bin/ks-mcp-sandbox.sh sock); the config is $KS_MCP_SANDBOX/home/commands.toml.
For each of at least 30 mutations (keep a pristine copy; restore it between cases): missing section,
wrong value type (string for bool, list for string, negative/huge numbers, NaN), unknown keys,
duplicate keys/sections, empty file, only comments, invalid TOML syntax, unicode keys/values, very
long values (100 KB), paths that do not exist, palette-first naming unknown ids, theme colours that
are not hex: write it, send `printf 'reload\\n' | nc -U -w5 "$KS_SOCKET"`, then check (1) the socket
still answers `state` within 3 s, (2) no 'panicked' in $KS_MCP_SANDBOX/daemon.log, (3) whether the app
applied, rejected (look for commands.toml.broken-* / last-known-good, AGENTS.md) or half-applied it.
A crash, hang or silent half-applied state is a finding.
OUTPUT: {out}/findings.jsonl (id, sev, title, mutation (exact text or a sed/python recipe), expected,
actual, repeat k/3) and {out}/summary.md. Stop the sandbox at the end.
""",
    "h-walk": """TASK: random-walk the kitchen-sink socket state machine, headless (nothing on screen).
Start: export KS_MCP_SANDBOX={root}/.build/hl-walk; bin/ks-mcp-sandbox.sh start; KS_SOCKET=$(bin/ks-mcp-sandbox.sh sock).
Write a Python script ({out}/walk.py) that, for seeds 1..20, sends 300 random `do:` verbs drawn from the
happy and bad cases in Tests/mcp_verbs.json (plus random arguments: empty, unicode, 1500-char strings,
negative numbers) and after EVERY step checks: reply is valid JSON, socket answers `state` in 3 s,
no 'panicked' in $KS_MCP_SANDBOX/daemon.log, and invariants (view is null or a known view; frames are
4 non-negative numbers; at most one view has key=true). On a violation: shrink to the shortest failing
prefix (re-run same seed, halve), record it, continue with the next seed. Restart the sandbox if it dies.
OUTPUT: {out}/findings.jsonl (id, sev, title, seed, minimal verb list, invariant broken, log line,
repeat k/3), {out}/summary.md. Stop the sandbox at the end.
""",
}


def phase_find() -> None:
    log("phase find: 4 authors + 3 hunters in parallel")
    jobs = {}
    with cf.ThreadPoolExecutor(max_workers=7) as ex:
        for key, area in AUTHOR_AREAS.items():
            d = os.path.join(OUT, "find", f"a-{key}")
            os.makedirs(d, exist_ok=True)
            jobs[ex.submit(worker, f"a-{key}", AUTHOR_BRIEF.format(area=area, key=key, out=d, root=ROOT), 75)] = key
        for key, brief in HUNT_BRIEFS.items():
            d = os.path.join(OUT, "find", key)
            os.makedirs(d, exist_ok=True)
            jobs[ex.submit(worker, key, brief.format(out=d, root=ROOT) + COMMON_RULES, 60)] = key
        for f in cf.as_completed(jobs):
            try:
                f.result()
            except Exception as e:
                log(f"  worker {jobs[f]} crashed: {e}")
    for f in os.listdir(os.path.join(ROOT, "Tests")):
        if re.fullmatch(r"app_flows_(files|compare|notes|shell)\.json", f):
            os.remove(os.path.join(ROOT, "Tests", f))
    dirty = git_dirty()
    if dirty:
        log(f"  workers left edits outside their output dirs: {dirty[:8]} -> stashing")
        stash("find-phase stray edits")


# ---------------------------------------------------------------- phase: run

VALID_INPUT = re.compile(r"^(key |type |wait \d+$|do:|raw )")
VALID_ORACLE = {"state", "disk", "log", "reply", "independent"}


def validate_flow(f: dict, known_ids: set) -> str | None:
    for k in ("id", "feature", "setup", "steps", "oracles"):
        if k not in f:
            return f"missing {k}"
    if not str(f["id"]).startswith("n-") or f["id"] in known_ids:
        return "bad or duplicate id"
    for s in f["steps"]:
        if not isinstance(s, dict) or not VALID_INPUT.match(str(s.get("input", ""))):
            return f"bad step {s}"
    for o in f["oracles"]:
        if o.get("kind") not in VALID_ORACLE:
            return f"bad oracle kind {o.get('kind')}"
        if o["kind"] == "independent" and "cmd" not in o:
            return "independent oracle without cmd"
    if "screen-capture" in f.get("needs", []):
        return "needs screen-capture"
    return None


def vm_test(args: str, timeout: int = 2400) -> tuple[int, dict]:
    """bin/ks-vm test ARGS -> (rc, {flow_id: verdict})."""
    rc, out = sh(f"bin/ks-vm test {args}", timeout=timeout)
    m = re.search(r"results: (\.build/flow-runs/vm-[\d-]+)", out)
    verdicts: dict = {}
    if m:
        p = os.path.join(ROOT, m.group(1), "results.jsonl")
        if os.path.exists(p):
            for line in open(p):
                r = json.loads(line)
                # with --repeat, a flow counts as failing if any run failed
                if r["id"] not in verdicts or r["verdict"] in ("FAIL", "ERROR"):
                    verdicts[r["id"]] = r
    return rc, verdicts


def phase_run() -> dict:
    log("phase run: validate authored flows, run them in the VM")
    base = json.load(open(TABLE))
    known = {f["id"] for f in base["flows"]}
    new_flows, fixtures = [], []
    for key in AUTHOR_AREAS:
        p = os.path.join(OUT, "find", f"a-{key}", "flows.json")
        try:
            doc = json.load(open(p))
        except Exception as e:
            log(f"  a-{key}: no usable flows.json ({e})")
            continue
        for fl in doc.get("flows", []):
            why = validate_flow(fl, known)
            if why:
                log(f"  drop {fl.get('id')}: {why}")
                continue
            known.add(fl["id"])
            new_flows.append(fl)
        fixtures += [x for x in doc.get("fixtures", []) if isinstance(x, dict) and "path" in x]
    log(f"  {len(new_flows)} valid new flows")
    table = dict(base, fixtures=base["fixtures"] + fixtures, flows=new_flows)
    json.dump(table, open(NIGHT_TABLE, "w"), indent=1, ensure_ascii=False)
    json.dump({"fixtures": fixtures, "flows": new_flows}, open(os.path.join(OUT, "new-flows.json"), "w"),
              indent=1, ensure_ascii=False)
    if not new_flows:
        return {}
    _, v = vm_test("--table Tests/app_flows_night.json --repeat 2")
    res = {}
    for fl in new_flows:
        res[fl["id"]] = v.get(fl["id"], {}).get("verdict", "MISSING")
    log("  VM: " + ", ".join(f"{k}={x}" for k, x in res.items()))
    json.dump(res, open(os.path.join(OUT, "run-verdicts.json"), "w"), indent=1)
    return res


# ---------------------------------------------------------------- phase: triage

TRIAGE_BRIEF = """TASK: a new kitchen-sink test flow FAILS in the test VM. Decide why. You change no repo file.
Flow (JSON): {flow}
Runner result: {result}
Is the flow's expectation wrong (it misreads the app's code/docs), or does the app misbehave?
- Read the app code the flow's "sources" cite and the code that handles its keys/verbs.
- Reproduce in the VM only (never on this Mac): `bin/ks-vm test --table Tests/app_flows_night.json --ids {fid}`
  (one VM user at a time: if it says another run holds the lock, wait 2 min, retry up to 5 times), or
  trace by hand: `bin/ks-vm ssh 'cd ~/ks && …'` using TESTING.md §3.1-3.6 commands (paths under ~/ks).
OUTPUT (only file you write): {out}/verdict.json =
  {{"flow":"{fid}","verdict":"app-bug"|"flow-wrong"|"flaky","evidence":"<file:line + what you saw>",
    "bug_title":"<one line, if app-bug>","severity":"S1-S4","fixed_flow":<corrected flow JSON if flow-wrong, else null>}}
""" + COMMON_RULES


def phase_triage(verdicts: dict) -> list[dict]:
    failing = [k for k, v in verdicts.items() if v in ("FAIL", "ERROR")][:10]
    log(f"phase triage: {len(failing)} failing new flows")
    new = json.load(open(os.path.join(OUT, "new-flows.json")))
    flows = {f["id"]: f for f in new["flows"]}
    bugs = []
    for fid in failing:
        d = os.path.join(OUT, "triage", fid)
        os.makedirs(d, exist_ok=True)
        worker(f"t-{fid}", TRIAGE_BRIEF.format(flow=json.dumps(flows[fid], ensure_ascii=False),
                                               result=verdicts[fid], fid=fid, out=d), 30)
        try:
            v = json.load(open(os.path.join(d, "verdict.json")))
        except Exception:
            log(f"  {fid}: no verdict")
            continue
        log(f"  {fid}: {v.get('verdict')} — {str(v.get('bug_title') or v.get('evidence'))[:120]}")
        if v.get("verdict") == "flow-wrong" and isinstance(v.get("fixed_flow"), dict):
            flows[fid] = v["fixed_flow"]
        elif v.get("verdict") == "app-bug":
            bugs.append({"flow": fid, **v})
        if git_dirty():
            stash(f"triage {fid} stray edits")
    new["flows"] = list(flows.values())
    json.dump(new, open(os.path.join(OUT, "new-flows.json"), "w"), indent=1, ensure_ascii=False)
    json.dump(bugs, open(os.path.join(OUT, "bugs.json"), "w"), indent=1, ensure_ascii=False)
    return bugs


def promote_flows() -> list[str]:
    """Append new flows that passed (re-run after triage corrections) to Tests/app_flows.json."""
    new = json.load(open(os.path.join(OUT, "new-flows.json")))
    if not new["flows"]:
        return []
    base = json.load(open(TABLE))
    json.dump(dict(base, fixtures=base["fixtures"] + new["fixtures"], flows=new["flows"]),
              open(NIGHT_TABLE, "w"), indent=1, ensure_ascii=False)
    _, v = vm_test("--table Tests/app_flows_night.json --repeat 2")
    bugs = {b["flow"] for b in json.load(open(os.path.join(OUT, "bugs.json")))} if os.path.exists(
        os.path.join(OUT, "bugs.json")) else set()
    keep = [f for f in new["flows"] if v.get(f["id"], {}).get("verdict") == "PASS" or f["id"] in bugs]
    if not keep:
        return []
    # Append as text so the hand-formatted table keeps its layout.
    text = open(TABLE, encoding="utf-8").read().rstrip()
    idx = text.rfind("]")  # end of "flows", the last key
    add = ",\n" + ",\n".join("    " + json.dumps(f, ensure_ascii=False) for f in keep) + "\n  "
    text = text[:idx].rstrip() + add + text[idx:]
    if new["fixtures"]:
        fx_end = text.rfind("]", 0, text.find('\n  "flows": ['))  # end of "fixtures"
        text = text[:fx_end].rstrip() + ",\n" + ",\n".join(
            "    " + json.dumps(x, ensure_ascii=False) for x in new["fixtures"]) + "\n  " + text[fx_end:]
    json.loads(text)
    open(TABLE, "w", encoding="utf-8").write(text + "\n")
    rc = git_commit(["Tests/app_flows.json"], f"tests: {len(keep)} new flows from the night run {RUN}\n\n"
                    "DeepSeek-authored, run in the VM; the failing ones expose bugs listed in "
                    f".build/night/{RUN}/bugs.json.\n\nCo-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>")
    log(f"  promoted {len(keep)} flows (commit rc={rc})")
    return [f["id"] for f in keep]


# ---------------------------------------------------------------- phase: fix

KNOWN_BUGS = [
    {"id": "#16", "flow": "compare-next-diff", "title": "Right after a text compare opens, Ctrl+N / Ctrl+P "
     "(next / previous difference) do nothing: focus is on the sidebar (WSSidebarView), so host.rs "
     "route_key_event never reaches CompareModel::handle_key's 45/35 arms (compare.rs ~4105-4125), or the "
     "sidebar consumes them. Real HID keys behave the same."},
    {"id": "#1", "flow": "files-recursive-search", "title": "Typing **deep** in the Files filter "
     "(recursive search, commands.toml:477) lists no rows although files/notes/deep.txt exists: "
     "QueryMode::Recursive (views/files.rs) has no working listing."},
    {"id": "#17", "flow": None, "title": "The first do: verb after a GUI launch replies at once but keeps the "
     "socket connection open ~25 s (later verbs close immediately). Find what holds the Reply (socket.rs "
     "Reply / host.rs raw_deferred / a first-launch path) and close the connection right after the reply. "
     "Add a Rust unit test if feasible."},
    {"id": "#6", "flow": None, "title": ".md files created on disk in the notes folder never appear in the "
     "Notes sidebar; Cmd+N and a view round trip don't rescan. The owner wants them to appear (at least "
     "after re-opening the Notes view). Add a Rust unit test for the rescan."},
]

FIX_BRIEF = """TASK: fix one bug in the kitchen-sink app (Rust, rust/ws-rs). Smallest correct change.
Bug {bid}: {title}
{flow_line}
Repo {root}. Read AGENTS.md (build/test, gotchas) and rule.md first.
GATES you must run and pass before finishing (exact commands):
  cd {root}/rust && CARGO_TARGET_DIR={root}/.build/rust-target cargo test --release -p ws-rs
{vm_gate}
Add or extend a unit test that fails without your fix where feasible.
NEVER run ./ws, pkill, git commit/stash/checkout/reset; never start the GUI sandbox on this Mac (GUI only
via bin/ks-vm). Do not touch Tests/run_flows.py, Tests/night_run.py, bin/ks-vm, bin/ks-mcp-sandbox.sh,
TESTING.md. If you cannot find a safe fix in your time, change nothing and explain why.
REPORT: root cause (file:line), the diff summary, gate outputs.
"""


def full_vm_failures() -> set:
    _, v = vm_test("", timeout=2400)
    return {k for k, r in v.items() if r["verdict"] in ("FAIL", "ERROR")} if v else {"<no results>"}


def phase_fix(new_bugs: list[dict]) -> list[str]:
    bugs = KNOWN_BUGS + [{"id": f"N{i + 1}", "flow": b["flow"], "title": f"{b.get('bug_title')} — evidence: "
                          f"{b.get('evidence')}"} for i, b in enumerate(new_bugs[:3])]
    log(f"phase fix: {len(bugs)} bugs, serial")
    base_fail = full_vm_failures()
    log(f"  baseline VM failures: {sorted(base_fail)}")
    fixed = []
    for b in bugs:
        if git_dirty():
            stash(f"before fix {b['id']}")
        flow_line = f"Failing flow: {b['flow']} (Tests/app_flows.json)." if b["flow"] else ""
        vm_gate = (f"  bin/ks-vm test --ids {b['flow']}      # must PASS (runs in the VM, nothing on screen)"
                   if b["flow"] else "")
        worker(f"fix-{b['id'].strip('#')}", FIX_BRIEF.format(bid=b["id"], title=b["title"], flow_line=flow_line,
                                                                   root=ROOT, vm_gate=vm_gate), 60)
        if not git_dirty():
            log(f"  {b['id']}: no change")
            continue
        rc, out = sh("cd rust && cargo test --release -p ws-rs 2>&1 | tail -3", timeout=1800)
        if rc != 0 or "FAILED" in out:
            log(f"  {b['id']}: cargo test failed -> stash")
            stash(f"fix {b['id']} failed cargo test")
            continue
        now_fail = full_vm_failures()
        new_fails = now_fail - base_fail
        if new_fails and "<no results>" not in new_fails:
            # Cold-start flakiness: count a new failure only if it fails again (2 more runs).
            _, again = vm_test("--ids " + ",".join(sorted(new_fails)) + " --repeat 2")
            still = {k for k, r in again.items() if r["verdict"] in ("FAIL", "ERROR")}
            log(f"  {b['id']}: new failures {sorted(new_fails)}, still failing on re-run {sorted(still)}")
            now_fail = (now_fail - new_fails) | still
            new_fails = still
        flow_ok = (not b["flow"]) or (b["flow"] not in now_fail)
        if new_fails or not flow_ok:
            log(f"  {b['id']}: VM gate failed (new fails {sorted(new_fails)}, flow ok {flow_ok}) -> stash")
            stash(f"fix {b['id']} failed VM gate")
            continue
        files = [l[3:].strip().strip('"') for l in git_dirty()]
        rc = git_commit(files, f"fix {b['id']}: {b['title'][:60]}\n\n"
                        f"Night run {RUN}, DeepSeek v4.1-flash. Gates: cargo test -p ws-rs, full VM flow "
                        f"table (no new failures{', ' + b['flow'] + ' passes' if b['flow'] else ''}). Not "
                        f"reviewed by Claude yet: .build/night/{RUN}/workers/fix-{b['id'].strip('#')}/report.md"
                        "\n\nCo-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>")
        log(f"  {b['id']}: FIXED, committed (rc={rc})")
        fixed.append(b["id"])
        base_fail = now_fail
    return fixed


# ---------------------------------------------------------------- main


def summary(fixed: list[str], promoted: list[str], bugs: list[dict]) -> None:
    lines = [f"# Night run {RUN}", "", f"Fixed + committed: {fixed or 'none'}",
             f"New flows promoted: {len(promoted)}", f"New app bugs (triaged): {len(bugs)}", ""]
    for b in bugs:
        lines.append(f"- {b.get('severity')} {b['flow']}: {b.get('bug_title')} ({str(b.get('evidence'))[:160]})")
    lines += ["", "## Headless hunters"]
    for key in HUNT_BRIEFS:
        p = os.path.join(OUT, "find", key, "findings.jsonl")
        n = sum(1 for l in open(p) if l.strip()) if os.path.exists(p) else 0
        lines.append(f"- {key}: {n} findings ({p})")
    _, st = sh("git stash list | grep 'night " + RUN + "' || true")
    lines += ["", "## Stashed (failed gates or stray edits)", st.strip() or "none", "", "## Log", *LOG]
    open(os.path.join(OUT, "SUMMARY.md"), "w").write("\n".join(lines) + "\n")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--phases", default="find,run,triage,fix,final")
    a = ap.parse_args()
    os.makedirs(OUT, exist_ok=True)
    phases = a.phases.split(",")
    log(f"night run {RUN}: phases {phases}")
    sh("bin/ks-vm up", timeout=600)
    verdicts, bugs, fixed, promoted = {}, [], [], []
    try:
        if "find" in phases:
            phase_find()
        if "run" in phases:
            verdicts = phase_run()
        if "triage" in phases and verdicts:
            bugs = phase_triage(verdicts)
            promoted = promote_flows()
        if "fix" in phases:
            fixed = phase_fix(bugs)
        if "final" in phases:
            log(f"final VM failures: {sorted(full_vm_failures())}")
    except Exception as e:
        log(f"night run aborted: {e!r}")
    finally:
        if os.path.exists(NIGHT_TABLE):
            os.remove(NIGHT_TABLE)
        summary(fixed, promoted, bugs)
        sh("bin/ks-vm down", timeout=300)
        log(f"done: {OUT}/SUMMARY.md")
    return 0


if __name__ == "__main__":
    sys.exit(main())
