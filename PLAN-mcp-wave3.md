# PLAN-mcp-wave3.md — GUI testing and better DeepSeek results for the kitchen-sink MCP surface

Written 2026-10-13 after waves 1 and 2. Audience: the planner (Opus) who will turn this into a
concrete test plan, and the Sonnet orchestrator who will run it. Read `MCP-TEST-FINDINGS.md` first
(§7 is the current state); `PLAN-mcp-testing.md` is the wave-1 plan and its lane cards (L1–L9) are
still the lane definitions. Orchestration recipe: `MCP-TEST-FINDINGS.md` §6 and the
`opencode-delegate` skill.

## 1. What is being tested (and what is not)

- **Under test:** the MCP control surface. That is `bin/ks-mcp.py` (stdio MCP server, tools `ks_state`,
  `ks_do`, `ks_raw`), the daemon's socket verbs (`rust/ws-rs/src/app/socket.rs`, `host.rs`
  `do_action` / `do_host_action` / `raw_deferred`), and the sandbox launcher `bin/ks-mcp-sandbox.sh`.
- **Not under test:** app features themselves (notes, jira, compare rendering...) except through what the
  socket reports about them.
- **DeepSeek v4.1 (`opencode-go/deepseek-v4.1-flash`, retry tier `deepseek-v4-pro`) plays two roles:**
  tester (writes and runs cases, reports findings) and, in lane L8, simulated *user* of the MCP tools.
  The goal of the project: an agent can drive the app through MCP safely, with failures visible.

## 2. Where we are

Done and committed on `main` (`d21d2b1`, `a113fc8`, `a0be604`, `3daf7e2`): all wave-1 findings F1–F5 and
seeds F0-1..F0-4 are fixed, plus 5 new wave-2 bugs. `Tests/test_ks_mcp.py` (25 cases, about 5 s) locks the
bridge; Rust unit tests lock unknown-verb errors and over-length refusal. Headless sandbox is isolated
(`HOME`/`XDG_*` redirected) and refuses capture verbs. Details: `MCP-TEST-FINDINGS.md` §7.1–7.2.

**Not done / unproven (this plan's inputs):**

1. **L8 result is untrustworthy.** Relay reported `failed` (exit 1) though output was written; the
   worker claimed it saw no verb list and no capture warning, but both are in the committed tool text.
   Unknown whether opencode passes the full `description` to the model, or truncates it, or the run
   used a stale server process. Needs diagnosis before any L8 conclusion.
2. **GUI is untested after the fixes.** All wave-2 testing was headless. The headless guard is
   `main_queue == None`; the GUI paths install it (`main.rs` lines about 199 and 225), so the real app
   should be unaffected, but nothing has exercised `start --gui`, `pane-shot` or real `screenshot` since.
   Also the Windows/Linux `Reply::peer_closed` is a stub or unbuilt.
3. **Open S3 leads** (`MCP-TEST-FINDINGS.md` §7.3), none verified beyond the workers' claims.
4. **Open design decisions** (section 6): headless "shown: true" claims (F0-7), `back` semantics, missing
   `show`, advertise-or-implement `board:<sub>`.
5. **`compare --wait`**'s 300 s design was never exercised (a 330 s run is needed).

## 3. What we learned about working with DeepSeek v4.1-flash

Facts from two waves, about $0.8 total:

- Cheap and fast: 4 to 12 min per lane, $0.02 to $0.12 per lane, 6 lanes in parallel without trouble.
- **Reports are leads, not evidence.** Wave 1: about two thirds of S3 claims were noise or known behaviour;
  one S2 family rested on a 12 s wait against a 300 s design. Wave 2: all 4 S1/S2 claims I re-ran were real
  (L1-63, L1-65, L2-11b, L5-13) but several S3s are design opinions ("headless should not claim shown").
  A worker with a clear oracle (crash, invalid JSON, leaked fd) is reliable; one asked to judge
  "is this behaviour right" is not.
- Implementation workers (W1/W2/W3 in the fix pass) did well on narrow, file-scoped briefs with a verify
  command: all three produced diffs I could accept after review. Splitting by file prevented conflicts.
- Mechanics that bit us: the opencode relay needs an allow rule or a script file; long pasted commands
  break; `result.json.touchedFiles` lists the whole dirty tree (use the `git status` diff plus the event
  log instead); the relay's `status: failed` for L8 needs diagnosing (see 2.1).
- DeepSeek cannot be assumed to read images. Treat any step that needs *looking at a PNG* as Claude's job.

## 4. How to get better results from DeepSeek (the strategy to plan around)

1. **Give workers oracles, not opinions.** Generate a verb inventory from `host.rs` (every explicit arm
   and every prefix in `do_host_action`, plus `RAW_VERBS` and the registry hooks) into one table:
   verb, argument grammar, expected state delta, expected error on bad input. Workers diff real behaviour
   against that table. They then report mismatches, not "this feels wrong". Commit the table as
   `Tests/mcp_verbs.json` so it also drives a deterministic test (see 5.2).
2. **Two stage triage.** Flash finds; `deepseek-v4-pro` adjudicates the ambiguous S3s against the table
   and the AGENTS.md "limits" list; the orchestrator re-runs only S1/S2 and 3 sampled S3s per lane. Record
   the verdict in one ledger file (`.build/mcp-hammer/ledger.jsonl`: id, claim, verdict
   verified / not-reproduced / misread / by-design, evidence command).
3. **Convert every verified finding into a deterministic test the same day** (done for the fixes in
   `Tests/test_ks_mcp.py`; keep doing it). The hammer lanes then only need to find *new* things.
4. **Property and fuzz lanes are where flash earns its keep** (L1/L2/L4 found real crashes). Bias the next
   wave toward fuzz with crash oracles: malformed JSON-RPC, huge/odd ids, odd unicode, random verb
   sequences with the state-invariant checker (L4), connection abuse (L5). Reduce "judge this" lanes.
5. **Brief hygiene.** One lane card per brief, filled socket/out paths, explicit "never do" list,
   explicit stop condition, a `--resume-last` delta brief naming missing case ids for any lane under 60% of
   its cases, then v4-pro once, then report the gap.
6. **Budget and cadence.** Whole-wave cost is about $0.5; re-run the full wave after every batch of
   fixes. A 5 minute check-in is enough.

## 5. Work packages (ordered; each has an acceptance test)

### 5.1 Resolve L8 validity (first, small)
- Reproduce what opencode sends: run the L8 setup (default sandbox, `opencode.json` toggled), have a
  trivial worker call `ks_do` and quote the tool description it sees verbatim. Compare with
  `bin/ks-mcp --list-tools` and the real `tools/list` output.
- Find out why relay returned `exitCode 1` (check `.build/mcp-hammer/runs/L8/events.jsonl` tail and `relay.log`).
- **Acceptance:** a one-line statement in `MCP-TEST-FINDINGS.md` §7.4: either "descriptions reach the
  model" (then the worker's complaint is a wording finding and the verb list/warning needs to be more
  prominent, for example moved into a `ks_help` tool) or "truncated at N chars" (then shorten and reorder).
  Then re-run L8 and record turns used, wrong guesses, helpful and misleading messages per task.

### 5.2 Deterministic verb-table test (replaces most of L3)
- Build `Tests/mcp_verbs.json` from `host.rs` (section 4.1) and a Python test that walks it against a
  headless sandbox (reuse `Tests/ks_mcp_client.py`). No LLM involved.
- **Acceptance:** `./ws test mcp-verbs` (or an extra suite in `./ws test mcp`) passes; every verb in the
  table has a happy path and a bad-argument case; adding a verb to `host.rs` without a table row fails a
  coverage check (grep-based is fine).
- DeepSeek's role afterwards: L3 becomes "find verbs or argument shapes missing from the table".

### 5.3 GUI testing (L9 and beyond) — needs the owner present
Facts from AGENTS.md (MCP section) that shape the design: `start --gui` launches the real windowed app
via `open -n -g` (so TCC grants apply); it runs `aerospace enable off` for the whole session and `stop`
re-enables it; the sandbox GUI pops UI onto the owner's screen; raw-exec of the binary never answers the
socket (always launch via `open`); `pane-shot` renders a pane to a PNG, bare `screenshot` may raise an
interactive overlay; `do:key:` is record-only. The owner's memories also say: drive features with real
input and screenshots (hooks prove only the model), and stop after 2 UI misses instead of retrying.

Proposed split, to be refined by the planner:
- **GUI-A (state and geometry, DeepSeek-friendly, no images):** with `--gui`, drive `do:` verbs and
  assert `windows`, `views.*.shown`, frames and geometry from `state` and `screenshot -g`
  (`--print-geometry`). Check the headless refusal does **not** trigger in GUI mode (regression for the
  `is_headless` guard) and that `compare --wait` completes when the compare window is closed.
- **GUI-B (pixels, Claude-reviewed):** `pane-shot` PNGs for a fixed list of (view, size, theme) cases;
  compare against goldens in `samples/parity` or new goldens; Opus/Sonnet reads the PNGs for anything the
  pixel diff flags. DeepSeek only runs the capture matrix and the byte/hash diff.
- **GUI-C (real input):** real mouse/keys via the owner's established approach (see memory
  "test-ui-with-real-input"), not `do:key:`. Scope this narrowly: a handful of end-to-end flows.
- **Safety envelope:** one orchestrator-run script wraps start/stop, records the live-app pid before and
  after, asserts `aerospace` is re-enabled on exit (trap), and aborts the lane if the live app pid changes.
  Workers never run `start --gui` themselves. Schedule when the owner is at the machine.
- **Acceptance:** GUI-A and GUI-B each run to completion once, with the live app untouched and AeroSpace
  restored, and the report lists which cases needed a human look.

### 5.4 Fix or decide the open leads (after owner decisions, section 6)
- `board:<sub>`: remove from `ks_do` description or implement. `tool-close:`/`key:` strictness.
  `compare:select` bounds. `show` verb. State schema (a `ks_help`/`ks_verbs` tool or `state.schema`).
- Each fix gets a test in 5.2's table and a regression case in `Tests/test_ks_mcp.py` if it is bridge-side.

### 5.5 Remaining coverage gaps
- `compare --wait` full-duration run (330 s) on a sandbox, with and without a window close.
- Build the Windows and Linux targets for the `Reply`/`socket.rs` changes (`bin/linux-check.sh`,
  `bin/windows-check.sh`) and give Windows a real `peer_closed` if feasible.
- Finish the idle-client fix properly (F0-6 is mitigated to 250 ms, not removed): a per-connection
  thread or a nonblocking accept loop. Only do this if a measured need appears.

## 6. Decisions needed from the owner (planner should surface these, not guess)

1. Should a headless daemon report `shown: true` for things that cannot exist headless
   (`paths:show`, `tool:<panel>`, switcher), or refuse? (F0-7; agents can't tell state from reality.)
2. `back`: previous view (what an agent expects) or home (current)? Add `show` to pair `hide`?
3. Is `board:<sub>` going to be implemented, or removed from the advertised list?
4. How much owner screen time is acceptable for GUI testing, and at what hours? AeroSpace goes off for
   the duration.
5. Is the cost ceiling still "a dollar a wave"? (Current waves are about $0.5.)

### 6.1 Owner answers (2026-10-13)

1. **Refuse.** A headless daemon refuses `paths:show`, `tool:<panel>`, switcher and anything else that
   cannot exist without a window, with an `error: ... (headless)` reply, instead of claiming `shown: true`.
2. **`back` = previous view** (a view history, not home). Add a `show:<view>` verb to pair with `hide`.
3. **Implement `board:<sub>`.**
4. **GUI budget: about 60 minutes**, run in the background. Keep Anthropic interactive time minimal;
   DeepSeek does the running, Claude only briefs, reviews diffs and reads flagged PNGs.
5. **Cost ceiling unchanged**; DeepSeek cost is not a concern. Anthropic token use is the constraint.

## 7. Constraints that stay in force

- Never `./ws build`, never `pkill`; the sandbox is stopped only by its pidfile or the pid holding its own
  socket. Never touch the live app's socket from a worker. Workers never commit; the orchestrator commits
  on `main` (no feature branches) after re-running the gates.
- Work only in files a brief lists; other uncommitted work in the tree is the owner's.
- Re-run every S1/S2 yourself on a fresh sandbox before believing it.
- Reason from the code before another rebuild/click cycle; stop after 2 misses on UI work.
- `opencode.json` is tracked; any toggle for L8 must be restored (the wave-2 script does this) and never
  committed.

## 8. Reference: files and commands

- Findings and ledger: `MCP-TEST-FINDINGS.md`; raw data `.build/mcp-hammer/` (uncommitted, wave 1 in `wave1/`).
- Harness: `Tests/ks_mcp_client.py`; briefs `.build/mcp-hammer/briefs/L1..L8.txt`; dispatch scripts
  `.build/mcp-hammer/dispatch-wave2.sh`, `run-l8.sh`.
- Sandboxes: `KS_MCP_SANDBOX=$PWD/.build/mh/Lx KS_MCP_SANDBOX_NO_BUILD=1 bin/ks-mcp-sandbox.sh start|stop`
  (build once with `cd rust && CARGO_TARGET_DIR=../.build/rust-target cargo build --release -p ws-rs -p ws-helpers`).
- Unit tests: `python3 -B Tests/test_ks_mcp.py`; Rust: `cd rust/ws-rs && CARGO_TARGET_DIR=/tmp/ks-w2-target cargo test --bin kitchen-sink`.
- Dispatch: `node ~/.claude/skills/opencode-delegate/scripts/relay.mjs --brief B --model opencode-go/deepseek-v4.1-flash --cd REPO --timeout 90m --out-dir D`.
