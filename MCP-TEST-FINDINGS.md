# MCP-TEST-FINDINGS.md — wave 1 hammer test of the kitchen-sink MCP bridge

> **Status update 2026-10-13: wave-1 items F1–F5 and seeds F0-1 to F0-4 are fixed** (commits
> `d21d2b1`, `a113fc8`, `a0be604`, `3daf7e2`; fix table in §4, wave 2 in §7, next steps in
> `PLAN-mcp-wave3.md`). The verdict and "required fixes" below are the **wave-1 snapshot**, kept
> for history; do not read them as current. Still open: L8 validity, the S3 leads in §7.3, GUI
> testing (L9), F0-7.


Run date: 2026-10-12 · code at `4ee476f` (+ harness `be8504b`) · plan: `PLAN-mcp-testing.md`
Workers: `opencode-go/deepseek-v4.1-flash` · raw data: `.build/mcp-hammer/` (not committed)

## 1. Verdict (wave 1, superseded by §7)

**Do not run the full hammer test (L5 concurrency, L8 agent use, L9 GUI) yet.** Fix the
items in §4 first, then re-run wave 1 plus L5 and L8 against the fixed code.

Why:
- The sandbox is not isolated: it can write into the owner's real `~/.cache/kitchen-sink/`,
  and `ks_raw pane-shot` captures the owner's real screen even in headless mode (F0-3).
  A test that drives it can touch real data.
- A typo in `ks_do` is indistinguishable from success, so any agent (or test) that guesses an
  action sees "OK" either way. Test results are only meaningful once failure is visible.
- The suite that is supposed to guard the server (`Tests/test_ks_mcp.py`) does not exist.

What held up: the state model (L4: 20 walks × 200 steps, zero invariant violations), and
most of protocol conformance and read-only enforcement (L1, L2).

## 2. Scoreboard (wave 1)

| Lane | What | Cases logged | Fails (worker-reported) | Status |
|---|---|---|---|---|
| L1 | protocol conformance | 48 | S1×1 S2×3 S3×4 S4×1 | done |
| L2 | arguments + read-only | 38 | S3×2 S4×2 | done |
| L3 | `do:` verb coverage | 169 | S2×2 S3×22 S4×2 | done |
| L4 | state invariants (random walks) | 26 | none | done |
| L6 | raw verbs (safe subset) | 28 | S2×7 S3×2 S4×1 | done |
| L7 | sandbox lifecycle + isolation | 18 | S1×2 S3×2 S4×1 | done |
| L5 | concurrency / leaks | — | — | **not run** (deferred until fixes) |
| L8 | agent ergonomics | — | — | **not run** (deferred until fixes) |
| L9 | GUI + `pane-shot` | — | — | not run (needs owner present) |

Fail counts are the workers' claims. Only the items in §3 marked **Confirmed** were
re-run by the orchestrator. Everything else is unverified and should be treated as a lead.

Safety: no worker command touched the live app (no `pkill`, `./ws`, or live socket in
the event logs). `git status` drift vs the baseline was the owner's own concurrent edits.

## 3. Findings

### Confirmed (re-run by orchestrator on a fresh L1 sandbox)

| id | sev | finding | repro | where |
|---|---|---|---|---|
| F1 | S1 | Invalid UTF-8 on stdin crashes `ks-mcp`: traceback, exit 1, no JSON-RPC reply. The agent's session dies. | `printf '\xff\xfe\n' > bad.txt; $C raw bad.txt` (L1-23) | `bin/ks-mcp.py` `serve()` stdin read |
| F2 | S2 | `tools/call` with `params` as a non-empty list, or `initialize` with `params` a string, returns `-32603 "internal error: 'list' object has no attribute 'get'"`. Wrong code (should be `-32602`) and leaks a Python exception. Empty list `[]` is handled correctly. | `{"method":"tools/call","params":[1]}` (L1-31c, L1-36a/b) | `bin/ks-mcp.py` `handle_request()` |
| F3 | S2 | `do:tool:nope` fabricates `tools.nope = {shown: true}` in state for a tool that does not exist (also `tool:` with empty name, L3-611). | `$C call ks_do '{"action":"tool:nope"}'` (L3-242) | `rust/ws-rs/src/app/host.rs` `tool:` handling |
| F4 | S3 | Unknown `do:` verb returns full state, identical to success. A typo is undetectable. (Seed F0-5; also hits `do:file:/etc/hosts`, `do:notes:bogus`, and many more in L3.) | `$C call ks_do '{"action":"bogus-xyz"}'` | `host.rs` `do_action` |

### Supported by evidence, not re-run

| id | sev | finding | why not re-run |
|---|---|---|---|
| F5 | S1 | Sandbox daemon writes `config-<hash>.json` into the owner's real `~/.cache/kitchen-sink/` (L7-06 used a unique marker string and found it there; L7-07 saw the same under a redirected `HOME`). `bin/ks-mcp-sandbox.sh` redirects `TMPDIR`, `WS_HOME`, `JIRA_*`, `NOTIFY_CACHE_DIR`, `WS_SETTINGS_CACHE` but not `HOME`/`XDG_CACHE_HOME`, and `config.rs` also hardcodes `~/.cache/ws-crash.log`, `~/.cache/aero-debug`, `~/.cache/ws-aero.log`. | Re-running writes to the owner's real cache again. Verify against a throwaway `HOME` instead. |

### Seed findings from planning (not re-tested in wave 1)

| id | sev | finding |
|---|---|---|
| F0-1 | S2 | `Tests/test_ks_mcp.py` does not exist and was never committed, but `./ws test mcp`, AGENTS.md and BACKLOG.md say it locks the server. `./ws test mcp` fails. |
| F0-2 | S2 | Socket `LISTEN_BACKLOG = 4` (`rust/ws-rs/src/app/socket.rs:41`): bursts get `ECONNREFUSED`, which `ks-mcp` reports as "daemon not answering… start the app" although the daemon is up. |
| F0-3 | S1 | `ks_raw pane-shot` on the headless sandbox captures the owner's real screen and saves outside the sandbox. Capture verbs are not isolated. |
| F0-4 | S3 | `ks_do` actions over about 2 KB are silently truncated (`READ_MAX = 2048`). |
| F0-6 | S3 | One idle client stalls every other client about 2 s (single accept loop, `SO_RCVTIMEO` 2 s). |
| F0-7 | S4 | `do:toggle-terminal` on headless leaves `terminal` false; decide report vs refuse. |

### Open questions from the workers (unverified, likely noise or by-design)

- L3 reports about 15 verbs as "silently succeeds" (`show`, `select:0`, `prettyprint`, `filefast`,
  `health`, …). Several are probably valid verbs with no visible headless effect or the wrong
  form; they are the same root cause as F4 and cannot be judged until unknown verbs error.
- L3 reports `tool:health`, `tool:text-table`, `tool:prettyprint`, `tool:filefast` as
  `shown: true` on headless. Same question as F0-7: should headless claim a UI effect?
- L3 reports `paths:select:-1`, `paths:select:x` and an empty value become 0, and
  `paths:select:99999` is accepted with no bounds check.
- L3/L6 report misleading errors: `compare:folder-bogus` says "no folder session"; a bad
  `screenshot` option reports "no answer".
- L6 reports seven S2s that `compare --wait` "does not self-release". **Weak evidence:** the
  worker waited 12 s, but the verb is designed to block up to 300 s. Needs a 330 s run to say
  anything.

## 4. Fixes required before re-testing (in order) — all done, see status table in §7.1

1. **Isolate the sandbox (F5, F0-3).** In `bin/ks-mcp-sandbox.sh` set `HOME` and
   `XDG_CACHE_HOME` (plus anything `config.rs` resolves from `~`) to dirs under the sandbox root.
   In headless mode, make `pane-shot` and screenshot verbs return an error instead of capturing.
   *Done when:* a mixed workload leaves nothing newer than a start marker under `~/.cache`,
   `~/Library`, or `/private/tmp`, and `ks_raw pane-shot` on a sandbox returns an error.
2. **Make failure visible (F4, F3).** Unknown `do:` verbs return `{"error":"unknown action …"}`
   (→ `isError`); `tool:` rejects empty or unknown names; `paths:select:` rejects non-numbers.
   *Done when:* `ks_do {"action":"bogus-xyz"}` returns `isError`.
3. **Harden the bridge (F1, F2).** Decode stdin with `errors="replace"` (or catch the decode error
   and reply `-32700`); validate `params` is an object and reply `-32602` otherwise.
   *Done when:* the L1-23, L1-31c and L1-36 repros return a JSON-RPC error and the server stays up.
4. **Socket robustness (F0-2, F0-6, F0-4).** Raise the listen backlog (about 64); stop one idle
   connection from blocking others; return "busy, retry" instead of "start the app"; error on
   over-length actions instead of truncating.
5. **Write `Tests/test_ks_mcp.py` (F0-1)** as a fake-daemon suite from the passing L1/L2 cases, so
   items 1–4 stay fixed and `./ws test mcp` works.
6. **Tool text for agents.** List valid `do:` actions (or add a `ks_verbs` tool) and warn in the
   `ks_raw` description that `pane-shot`/`screenshot` capture the real screen. Needed for L8.

## 5. Then re-run

After 1–3 at minimum: re-run L1, L2, L3, L6 and L7 (cheap), then L5 alone, then L8. Run L7's
cache-leak check against a throwaway `HOME` first. Wave-1 harness, briefs and dispatch script
are reusable: `Tests/ks_mcp_client.py`, `.build/mcp-hammer/briefs/`, `.build/mcp-hammer/dispatch-wave1.sh`.

Note for the next run: dispatching the workers needs a permission rule for
`node ~/.claude/skills/opencode-delegate/scripts/relay.mjs` (the auto-mode classifier blocked it
once), and long commands should go in a script file, not a pasted one-liner.

## 6. How Sonnet orchestrates the DeepSeek v4.1 workers (reusable recipe)

**A skill already exists: `opencode-delegate`** (`~/.claude/skills/opencode-delegate/`, invoke
with the Skill tool or just ask "delegate this to OpenCode"). It wraps `opencode run` in
`scripts/relay.mjs` and writes a structured `result.json`. Its references cover brief writing,
dispatch/poll, multi-task queues and review. This plan's orchestrator section (§O of
`PLAN-mcp-testing.md`) is the project-specific layer on top. Nothing new needs installing.

**Prerequisites (checked once, all true on this machine):** `opencode` on PATH (`opencode v2.0.26`),
`opencode auth list` shows a stored provider, models `opencode-go/deepseek-v4.1-flash` (cheap,
fast) and `opencode-go/deepseek-v4-pro` (retry tier) available.

**The loop**
1. **Write one self-contained brief per lane.** Workers see only the brief plus the repo: no chat
   history. Paste the rules, the harness and exactly one lane card; fill in the socket and output
   paths. Never give a worker the whole plan (weaker models drift when handed choices).
2. **Dispatch in parallel, detached.** One `relay.mjs` call per lane with `--brief`, `--model
   opencode-go/deepseek-v4.1-flash`, `--cd <repo>`, `--timeout 90m`, `--out-dir runs/<lane>`.
   Put the loop in a script file (`.build/mcp-hammer/dispatch-wave1.sh`), not a pasted one-liner.
3. **Check in, don't poll tightly.** Wave 1 took 4 to 11 minutes per lane (about 12 minutes end to
   end, 6 lanes at once), far under the 75 minute time box. A 5 minute check is enough next time.
4. **Review before believing.** Read `result.json` (`status`, `cost`), then the worker's
   `findings.jsonl` and `summary.md`. Diff `git status` against the saved baseline. Grep
   `events.jsonl` for forbidden commands (`pkill`, `./ws`, the live socket).
5. **Re-run every S1/S2 repro yourself** on a fresh sandbox. In wave 1 about two thirds of the
   S3s were noise or already-known behaviour, and one S2 family (`compare --wait`) rested on a
   12 s wait against a 300 s design. DeepSeek reports are leads, not evidence.
6. **Rework once, then escalate:** a `--resume-last` delta brief naming the missing case ids, then
   `deepseek-v4-pro` once, then report the gap.

**Gotchas hit in wave 1**
- Claude Code's auto-mode classifier blocked the dispatch ("Create Unsafe Agents"): the relay
  runs a write-capable `build` agent with `--auto`. Fix: an allow rule for
  `Bash(node ~/.claude/skills/opencode-delegate/scripts/relay.mjs:*)`, or have the owner run the
  script with `!`.
- A long pasted command line wrapped in the terminal and broke. Use a script file.
- `result.json.touchedFiles` just lists the whole dirty tree when the owner has uncommitted work,
  so it is not evidence a worker edited anything. Use the `git status` diff plus the event log.
- Worker briefs must forbid the live app's MCP server and capture verbs (`pane-shot`, `screenshot`
  without `-g`): the workers get `opencode.json`'s enabled `kitchen-sink` entry by default.

**Cost (wave 1):** about $0.30 for six lanes (L1 $0.03, L2 $0.05, L3 $0.07, L4 $0.05, L6 $0.04,
L7 $0.06). Cheap enough to re-run the whole wave after the fixes.

## 7. Wave 2 (2026-10-13): re-run on the fixed code

Code at `d21d2b1` then `a113fc8`. Workers: `opencode-go/deepseek-v4.1-flash`, lanes L1–L4, L6, L7 in
parallel, then L5 alone, then L8. Raw data `.build/mcp-hammer/{L1..L8}`, wave 1 archived in
`.build/mcp-hammer/wave1/` (neither committed). Cost about $0.4 for L1–L7 plus about $0.04 for L8.
No worker changed any repo file outside `.build/`; the forbidden-command grep hits were the brief's own rule text.

### 7.1 Status of every wave-1 finding

| id | status | commit | how verified |
|---|---|---|---|
| F1 invalid UTF-8 crash | **fixed** | d21d2b1 | repro replies `-32700`, server stays up, exit 0; regression test |
| F2 non-object `params` | **fixed** | d21d2b1 | `-32602`; regression test |
| F3 `tool:nope` fabricates state | **fixed** | d21d2b1 | error reply; Rust unit test |
| F4 unknown `do:` echoes state | **fixed** | d21d2b1 | `{"error":"unknown action …"}` live; Rust unit test |
| F5 sandbox writes real `~/.cache` | **fixed, verified** | d21d2b1 | throwaway sandbox: `config-*.json` landed under sandbox home. `paths.json`/`recent.json` in the real cache are written by the **live app's** watcher, not the sandbox |
| F0-1 `Tests/test_ks_mcp.py` missing | **fixed** | d21d2b1 | 25 cases, about 5 s, `./ws test mcp` command run directly (not via `./ws`) |
| F0-2 backlog 4 | **fixed** | d21d2b1 | 40 simultaneous raw `state` connects all answered; L5: 64 ok, refusals only past the backlog |
| F0-3 `pane-shot` captures real screen | **fixed (headless)** | d21d2b1 | headless refuses `pane-shot` and `screenshot`. **GUI mode not retested** |
| F0-4 2 KB truncation | **fixed** | d21d2b1, a113fc8 | daemon: `request too long`; bridge refuses client-side with a clear message |
| F0-6 idle client stalls others | **mitigated** | d21d2b1 | stall cut from 2 s to 250 ms, not eliminated (single accept loop) |
| F0-7 headless claims UI effects | **open (decision)** | | see §7.3 |

### 7.2 New wave-2 findings that were fixed

| id | sev | finding | commit |
|---|---|---|---|
| L1-63 | S1 | JSON nested about 200,000 deep raised `RecursionError` and killed the bridge (reproduced) | a113fc8 |
| L1-65 | S2 | id `1e400` echoed as invalid `Infinity` (reproduced) | a113fc8 |
| L2-11b | S2 | action over 2 KB reported "daemon not answering" (reproduced) | a113fc8 |
| L5-13 | S2 | abandoned `compare --wait` leaked one fd and one thread each; `+5`/`+5` after 5 calls (reproduced), now flat | a0be604 |
| L6-02, L7-18 | S3 | `ks_raw` returned a daemon `error:` reply as a success result | 3daf7e2 |

### 7.3 Open leads (worker claims; only the S1/S2 above were re-run by the orchestrator)

- `ks_do` text lists `board:<sub>` but every `board:*` errors (L3-127..129). The description should not advertise it, or the verb should work.
- `tool-close:<unknown|empty>`, `key:<garbage>`, `hide` when nothing is shown: silent success (L3-045/107, L8).
- `compare:select:99999` no bounds check, `compare:next` with no session succeeds (L3-111/116).
- Headless `paths:show`, `tool:<panel>`, `switcher` report `shown: true` with no window (L3-010/075/095..100, L8-05). Same question as F0-7: report model state, or refuse in headless?
- `back` goes home, not to the previous view; there is no `show` to pair `hide` (L8-04, L8-10).
- Concurrent `sandbox start` x2 misreports the serving pid (L7-14).
- Not issues: request split over two writes or pipelined requests get no useful reply (protocol is one request per connection, L5-06/21); bare `do:` message has an empty name (L5-23, cosmetic).

### 7.4 L8 (agent ergonomics) — result NOT yet trustworthy

The relay reported `status: failed` (exit code 1) although `findings.jsonl` and `summary.md` were written
(10 cases: 7 pass, 2 fail, 1 blocked). The worker said it saw **no** verb list and **no** capture warning,
yet both are in the committed `ks_do` / `ks_raw` descriptions (`bin/ks-mcp --list-tools` works). Until
someone confirms that the descriptions reach the model through opencode, treat these L8 claims as
unproven: "no action list", "no capture warning" (rated S2). Other L8 observations stand as leads:
state has no schema (`visible` vs `windows` ambiguous); 6 of 7 header styles are not discoverable;
MCP has no file-write tool so L8-08 could not run.

**Resolved (wave 3, 2026-10-13):** descriptions are **truncated at 120 chars** before the model sees them.
opencode 2.0.26 drives MCP tools through its Code Mode catalog, which renders each tool as
`first line of description, slice(0,117)+"..."` (constant `y9=120` in the binary). So the worker saw
the `ks_do` opening sentence and nothing of the verb list, and `ks_raw` cut at "interactive fo..." before
the WARNING. Not a stale server (`d21d2b1` landed 47 min before the run). Both L8 complaints share this one
cause, and are real for any client that shows only the first line. Fix: put the key fact in the first
line, under 117 chars, and move the verb list into a `ks_help` tool. The relay's `failed`/exit 1 is
opencode's own non-zero exit after a transient `provider.invalid-output` ("stream ended without
finish_reason", `runs/L8/events.jsonl:104`); the run itself completed.

### 7.5 Corrections to statements elsewhere

AGENTS.md said headless screenshot verbs capture and that unknown `do:` echoes state: both were
wrong after d21d2b1 and are now corrected. The "§5 Then re-run" plan above has been carried out
except L9; see `PLAN-mcp-wave3.md` for what comes next.

## 8. Wave 3 (2026-10-13): owner decisions, ks_help, first GUI run

Owner decisions: `PLAN-mcp-wave3.md` §6.1. Landed: `7de045a` (headless refuses window-only verbs;
`do:back` walks a 32-deep view history; `do:show[:<view>]`; `board:*` reports "not implemented in the
Rust port yet"), `1e96003` (first description lines ≤110 chars, offline `ks_help`). The Jira board feature
itself (list, columns, sprint/mode, pin — 8 Swift sub-verbs at `608d07d`) is unported; a DeepSeek attempt
built a fake model disconnected from the real view and was rejected.

**First GUI run** (`.build/mcp-hammer/wave3/gui-run.sh`, DeepSeek v4-pro, 12 min): tests a copy of the
bundle holding the fresh release binary, re-signed with "kitchen-sink codesign" (same designated
requirement, so TCC applies; `KS_MCP_SANDBOX_APP` selects it). Live app pid unchanged and AeroSpace
restored. Phase A 13/13 pass: GUI mode does not hit the headless refusal; 7 views open with real frames
and windows; back/back/hide/show correct in the real app; `compare --wait` returned 0.06 s after `do:hide`.
`screenshot -g` is **not** capture-free (still runs the overlay), so geometry cross-checks were skipped.
Phase B produced no view captures: `pane-shot` renders **herdr terminal panes** (and `--file` renders),
not shared-window views, and the sandbox HOME has no herdr. Pixel testing of views needs a non-interactive
window capture verb (or symlinking herdr into the sandbox HOME for terminal panes). Raw results:
`.build/mcp-hammer/wave3/gui/`.
