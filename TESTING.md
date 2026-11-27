# TESTING.md — test strategy and the worker playbook

**Audience:** the orchestrator, an agent that plans a test run, dispatches cheap DeepSeek workers,
triages what they report and writes the result. Paste the **VERBATIM** blocks into briefs unchanged.

**Goal:** find real bugs in kitchen-sink's features (Files, Jira, Confluence, AI, palette tools,
screenshot, keys) at low token cost. Hardening the MCP bridge or the socket is **not** the goal (waves
1–3 found almost no app bugs there). Swift parity is not a goal. The Notes, Compare and terminal views
are gone; flows in `Tests/app_flows.json` that still mention them are stale (see §6).

Related: `BACKLOG.md` (open bugs and leads), `Tests/app_flows.json` (feature flow table, schema in
§3), `Tests/mcp_verbs.json` (socket verb table), `Tests/usability/` (old run reports),
`AGENTS.md` "MCP server / headless control" (how the socket and sandbox work).

---

## 1. Rules for the orchestrator

1. **No oracle, no lane.** Every case names a machine check: socket state, bytes on disk, an
   independent recomputation, a log line, a crash, or OCR text. "Does this look right?" is never a
   worker's call.
2. **One GUI driver at a time.** Real input is global on this Mac. Headless lanes run in parallel
   (5–8 is fine); GUI lanes run one after another, **in the VM** (`bin/ks-vm`), never on the owner's
   screen. Never start `--gui` or `run_flows.py` on the host unprompted.
3. **Replay beats review.** A finding counts when its JSON trail replays (§5). Spend expensive tokens
   only on S1/S2 findings that replayed.
4. **Workers never edit app source, never commit, never `./ws build`, never `pkill`.** They write only
   under their lane's output folder. Check `git status` after each lane; revert anything else.
5. **Flash only:** `opencode-go/deepseek-v4.1-flash`. Never `deepseek-v4-pro` (blacklisted). On a failed
   lane send a narrower brief or `--resume-last`; do not escalate the model.
6. **Read `BACKLOG.md` first** and give each worker the open items for its feature so it skips them.
7. **Two misses, then stop.** A worker that fails a UI step twice records `BLOCKED` and moves on.
8. **Small briefs, one lane each**, with exact commands; long sequences go in a script the brief calls.
9. **Convert every verified bug into a deterministic check** (a flow, a `mcp_verbs.json` row, or a
   cargo test) the same day.

**History:** flash is reliable with a crisp oracle (crash, invalid JSON, leaked fd) and unreliable at
judging behaviour (about 2/3 of its S3 claims were noise). Unverified static-read claims are leads, not
bugs. The real feature bugs came from driving features with real input over long sessions and checking
state and disk against what was typed.

**Tiers:** T0 deterministic, no LLM (`./ws test`, `Tests/test_mcp_verbs.py`, `Tests/run_flows.py`) ·
T1 flash workers (soak, fuzz, run flows, write flows, minimise repros; ~$0.02–0.15 a lane) ·
T2 the orchestrator (triage, dedupe, replay, rank; under ~10% of tokens).

## 2. Limits of the environment

| Limit | Effect | Workaround |
|---|---|---|
| Headless has no AppKit | no rendering, focus or screenshots; `shown:true` for a panel is not evidence | trust `shown` and focus only in GUI lanes |
| `do:key:<spec>` is record-only | it sends nothing | `do:input:*` posts real events into the app's queue (§4) |
| Workers can't read images | PNGs are useless to them | `bin/ks-ocr`: PNG → text lines + boxes; assert on text |
| `window-shot` captures the shared window only | palette, popups and menus are separate windows | `ks-input shot --pid P --out f.png` |
| No `state` for Cmd+K / Cmd+/ surfaces; Jira / Confluence / AI models are placeholders | only visual oracles | OCR the capture (`ks-ocr --find`) |
| `ks-input` quirks: some combos drop (Esc, Cmd+=); Cmd once stuck system-wide; `click` sends clickCount 0; `drag` drops mouse-up | false failures | `ks-input key a b c` for fields; osascript for Esc; **always run the modifier release (§4)**; treat click-count results as tooling-suspect |
| GUI sandbox runs the *installed* bundle | tests the last `./ws build`, not the working tree | copy the bundle to `.build/sandbox-app/` and swap in the fresh binary (§4) |
| `Tests/mcp_verbs.json` lags `host.rs` | coverage test fails (see BACKLOG "Known failing tests") | the H-VERB lane fills the rows |

## 3. Flow schema (`Tests/app_flows.json`)

A table of **feature flows** the runner drives against a sandboxed windowed app with real input via
`bin/ks-input` / `do:input`. Every expectation cites a doc or code `file:line` in `sources`, and every
flow asserts at least one **non-log** oracle (state, disk, or an independent recomputation).

- **Sandbox HOME** is `$SANDBOX/home` (`bin/ks-mcp-sandbox.sh` sets `HOME`/`WS_HOME`/`XDG_*`); every
  `$HOME` in the JSON expands to it, and `disk` paths are relative to it.
- **Fixtures** are created before the run. The `"kind":"config"` fixture `cfg-sandbox`
  (`"applies_to":"all"`) is load-bearing: apply it to `$SANDBOX/home/commands.toml` before start.
- **`setup`** holds only socket `do:` verbs that reach the start state. A step may itself be
  `{"input":"do:…"}` (still a real state transition) or `{"input":"raw reload"}` (a raw verb).
- **`needs`:** `"gui"` always; `"screen-capture"` is skipped unless `--allow-screen-capture`. Jira / AI /
  Confluence flows are offline.
- **No retries.** `wait` steps are the only synchronisation. An oracle's `after` names a step's
  `checkpoint`; with no `after` it reads the end state.
- **Oracle kinds:** `state` (daemon `state` JSON: `equals`, `not_equals`, `min`, `contains`,
  `contains_name`, `length`) · `disk` (`exists`, `contains`, `is_dir`, `count_min`) · `independent` (a
  recomputation the runner performs: a direct `ls`, a Python count, a `do:` reply body; the strongest
  kind) · `visual` (a screenshot a vision model decides; only where nothing else can see it) · `log`
  (`daemon.log` must contain none of a list; every flow ends with `none_of: ["panic","ERROR"]`).
- **Limits:** `views.jira`, `views.confluence`, `views.ai` are placeholder-only over the socket, so
  those flows assert `view`/`shown` plus a visual or independent check.

## 4. Toolbox (exact commands)

All paths are relative to the repo root. `$LANE` is the lane id, `$OUT` its folder
`.build/hammer/<run>/<lane>/`.

```bash
# The VM: headless macOS 26 (Tart, `ks-test`), own window server; the host is never touched.
bin/ks-vm setup                        # once
bin/ks-vm grant                        # once: Accessibility + Screen Recording for ssh-run tools (done 2026-11-26; --check verifies)
bin/ks-vm test                         # build the tree, copy in, run all flows -> .build/flow-runs/vm-<ts>/
bin/ks-vm test --ids files-open-listing --repeat 3
bin/ks-vm test --replay .build/hammer/RUN/G-SOAK/findings.jsonl --repeat 3
bin/ks-vm ssh 'cmd'                    # repo copy at ~/ks, Python at ~/py/bin/python3
bin/ks-vm down | up | status           # Tart lives at ~/.local/bin/tart

# Headless sandbox, one per lane (parallel-safe). Keep the root short: socket path < 104 bytes.
export KS_MCP_SANDBOX=$PWD/.build/hl-$LANE
bin/ks-mcp-sandbox.sh start            # builds once; later lanes KS_MCP_SANDBOX_NO_BUILD=1
export KS_SOCKET=$(bin/ks-mcp-sandbox.sh sock)
export SBX_HOME=$(bin/ks-mcp-sandbox.sh home)    # /tmp/ksb-<hash> -> $KS_MCP_SANDBOX/home
bin/ks-mcp-sandbox.sh stop             # always, at the end

# GUI against code newer than the installed app (VM only unless the owner says otherwise):
mkdir -p .build/sandbox-app && rm -rf .build/sandbox-app/kitchen-sink.app
cp -R kitchen-sink.app .build/sandbox-app/
( cd rust && CARGO_TARGET_DIR=../.build/rust-target cargo build --release -p ws-rs -p ws-helpers )
cp .build/rust-target/release/kitchen-sink .build/sandbox-app/kitchen-sink.app/Contents/MacOS/kitchen-sink
codesign --force --sign "kitchen-sink codesign" --identifier dev.danielbaker.kitchen-sink .build/sandbox-app/kitchen-sink.app
KS_MCP_SANDBOX_NO_BUILD=1 KS_MCP_SANDBOX_APP=$PWD/.build/sandbox-app/kitchen-sink.app bin/ks-mcp-sandbox.sh start --gui
export SBX_PID=$(lsof -t "$KS_SOCKET" | head -1)    # every ks-input call uses --pid $SBX_PID
# stop: pkill -f '.build/sandbox-app/kitchen-sink.app' ; bin/ks-mcp-sandbox.sh stop

# Socket: state + start states. A do: reply is the new state or {"error": …}; requests > 2048 bytes refused.
ks() { printf '%s\n' "$1" | nc -U -w3 "$KS_SOCKET"; }
ks 'state' | jq '{view, visible, windows: (.windows|length)}'
ks 'do:open:files'     # Tests/mcp_verbs.json lists every verb. Silent no-ops exist: compare state before/after.

# Real input, preferred (events go into the app's own queue, nothing reaches other apps):
bin/ks-input activate --pid $SBX_PID && ks 'do:input:activate'   # both needed on macOS 14+
ks 'do:input:key:cmd+l'         # combos; several: 'do:input:key:ctrl+b w'
ks 'do:input:text:Hello'        # literal text into whatever has focus
ks 'do:input:click:95,100'      # points from the window's top-left; ',2' double, ',right' right-click
sleep 0.5; ks state | jq .input # {posted, dropped, lastError}; dropped>0 => run do:input:activate

# Real input, fallback (global CGEvents, quirks above; refuses unless the sandbox is frontmost; never work around that):
bin/ks-input key --pid $SBX_PID cmd+n        # or one key per char: h e l l o
bin/ks-input click --pid $SBX_PID --win <winid> 120 40
bin/ks-input windows --pid $SBX_PID
osascript -e 'tell application "System Events" to key code 53'   # Esc when ks-input drops it

# Seeing the screen without vision:
ks $'window-shot\t--view\tfiles\t--out\t'"$OUT/shots/step-07.png"
bin/ks-ocr "$OUT/shots/step-07.png" --find 'Documents'     # exit 0 found / 1 not
bin/ks-ocr "$OUT/shots/step-07.png" --absent 'error'       # exit 0 absent / 1 present

# Logs and crashes (end of every case):
grep -E 'panicked|already borrowed|ERROR' "$KS_MCP_SANDBOX/daemon.log" "$SBX_HOME"/logs/*/daemon.log 2>/dev/null
ls -t ~/Library/Logs/DiagnosticReports/ | grep -i kitchen-sink | head -3
ks 'state' >/dev/null || echo HANG-OR-DEAD

# Flow runner (T0, no LLM). It owns the GUI sandbox, gives each flow a fresh app, decides state/disk/log/OCR
# oracles by machine; what it can't decide comes back `pending` with PNG + OCR text saved.
python3 Tests/run_flows.py --list | --feature files --repeat 3 | --replay $OUT/findings.jsonl

# GUI cleanup (VERBATIM; run even after a failure):
osascript -e 'tell application "System Events" to key up {command, control, option, shift}'
bin/ks-mcp-sandbox.sh stop || true
```

Disk truth: Files root `$SBX_HOME/files/` (with the `cfg-sandbox` fixture). Check with `ls`, `cat`,
`shasum`, never through the app. Silent keys: `run_flows.py` flags any shortcut-style key step with no
visible effect within 1 s under "Silent keys" in `summary.md` (never a FAIL).

## 5. Oracles, lanes, briefs, findings

| Want to know | Strongest oracle | Fallback |
|---|---|---|
| the right view is showing | `state.view`, `views.<v>.shown` (GUI only) | OCR of the header |
| a list/count is right | independent recompute (`ls`) vs `state` rows | OCR row count |
| an overlay opened | OCR `--find` a label only in the overlay | NEEDS-EYES |
| no crash or hang | daemon.log, DiagnosticReports, socket answers in 3 s | — |
| it looks right | **not a worker oracle**: NEEDS-EYES only | — |

**Lanes.** Headless (parallel): **H-WALK** seeded random `do:` walk from `mcp_verbs.json` happy cases
checking invariants (one `current` view, `visible` agrees with `windows`, no panic; report the shortest
failing prefix); **H-CFG** config fuzz (mutate a copy of `commands.toml`, `reload`, expect applied or
reported error, never a half-state); **H-VERB** fill uncovered verb rows. Authoring: **A-FLOW** write 5–10
new flows in the exact schema from one feature area's docs. GUI (serial, VM): **G-FLOW** run
`run_flows.py` and investigate FAILs; **G-SOAK** a fixed cycle ≥ 50 times with a perturbation every 5th
(view round trip, palette open+Esc, hide+show, external file touch), recording the first failing cycle;
**G-MONKEY** 500 seeded random keys/clicks from an allow-list, crash/hang/log oracle only. Usability
(report-only, findings go to `USABILITY.md`, never to the fix phase): **U-TASK** six everyday tasks as
flows using only what a user can read; **U-KEYS** a key × surface consistency audit. Follow-up: **R-MIN**
replay a finding 3 times, then minimise the trail.

**Brief header (VERBATIM)**

```text
You are a test worker for the kitchen-sink macOS app. You run tests and report. You do NOT fix anything.

HARD RULES
- Never edit files outside $OUT. Never edit app source, never git commit, never run ./ws, never pkill,
  never touch the live app or its socket. Only use the sandbox socket in $KS_SOCKET.
- Never use deepseek-v4-pro or any other model.
- GUI lanes: before each input burst run `bin/ks-input activate --pid $SBX_PID`; if it fails, stop and
  report BLOCKED. Never click outside the app window.
- A step that fails twice: record BLOCKED with what you saw, move on. No retry loops.
- Do not judge looks. Pass/fail comes only from the ORACLE below. If only eyes could decide, write
  NEEDS-EYES <png> <what to look at> and continue.
- Known bugs (do not re-report): <paste BACKLOG.md rows for the feature>
- Finish within <N> cases or <M> minutes, then write the report even if incomplete.

ENVIRONMENT  KS_SOCKET=<…>  SBX_HOME=<…>  SBX_PID=<GUI only>  OUT=<…>  KS_MCP_SANDBOX=<…>
LANE <id>: <goal> · Setup · Cases / cycle · ORACLE per case · Stop condition
REPORT: $OUT/findings.jsonl (one JSON object per finding) + $OUT/summary.md (run/pass/fail/blocked counts,
one line per finding)
```

Keep a filled brief under ~150 lines.

**Finding format** (one line per finding in `findings.jsonl`):

```json
{"id":"G-SOAK-03","sev":"S2","feature":"files/filter","title":"filter stops after view round trip",
 "mode":"gui","seed":17,"setup":["do:open:files"],
 "trail":[{"input":"key esc esc"},{"input":"key cmd+l"},{"input":"type doc"}],
 "oracle":{"cmd":"…","expected":"1","actual":"0"},
 "repeat":"4/5","evidence":["shots/c41.png","daemon.log:1204"],"suspect":"rust/ws-rs/src/views/files.rs"}
```

`trail` uses the flow input kinds (`key`, `type`, `click-rel`, `wait`, `do:`, `raw`), so a trail is a
flow and replay = the flow runner. **Severity:** S1 crash/hang/data loss · S2 a feature stops working or
text goes to the wrong place · S3 wrong with a workaround · S4 cosmetic. `repeat` is required; a 1/1
finding is a lead until R-MIN replays it.

## 6. Triage, runbook, what's next

**Triage (no expensive reviewer):** (1) script checks: `git status` clean apart from `$OUT`,
`findings.jsonl` parses, every finding has `trail`/`oracle`/`repeat`; (2) dedupe against `BACKLOG.md` and
across lanes; (3) replay each S1/S2 and S3 with `repeat` ≥ 2/3 via `run_flows.py --replay … --repeat 3`,
drop what doesn't reproduce; (4) a finding whose "expected" was the worker's opinion is `by-design?` and
goes to the owner; (5) spend expensive tokens only on replayed S1/S2 and ≤ 5 NEEDS-EYES PNGs; (6) record
verified bugs as `- S<n>` bullets under "Leads" in `BACKLOG.md` (or a table row with status `open`;
`Tests/night_run.py` reads both), keep the full run in `Tests/usability/RUN-<date>.md`, and write each
verdict to `.build/hammer/<run>/ledger.jsonl` (`id, verdict: verified|not-reproduced|misread|by-design?,
evidence`); (7) lock each verified bug with a flow or cargo test.

**Runbook:**

```bash
RUN=$(date +%Y-%m-%d)-a ; mkdir -p .build/hammer/$RUN ; git status --short > .build/hammer/$RUN/git-before.txt
( cd rust && CARGO_TARGET_DIR=../.build/rust-target cargo build --release -p ws-rs -p ws-helpers )
for L in H-WALK H-CFG H-VERB; do KS_MCP_SANDBOX=$PWD/.build/hl-$L KS_MCP_SANDBOX_NO_BUILD=1 bin/ks-mcp-sandbox.sh start; done
for L in H-WALK H-CFG H-VERB A-FLOW-files; do   # one brief file per lane, in parallel
  mkdir -p .build/hammer/$RUN/$L
  node ~/.claude/skills/opencode-delegate/scripts/relay.mjs --brief .build/hammer/$RUN/briefs/$L.txt \
    --model opencode-go/deepseek-v4.1-flash --cd "$PWD" --timeout 90m --out-dir .build/hammer/$RUN/$L/relay \
    > .build/hammer/$RUN/$L/relay.log 2>&1 &
done ; wait
# GUI lanes: one at a time, in the VM. Then stop every sandbox, triage, commit docs only (on main).
```

`result.json` in each `relay/` folder holds the worker's final message; treat "all passed" as a claim and
the files in `$OUT` as the evidence. A full wave costs well under $1 in flash.

**Next (ranked):** `do:input` drag and scroll; convert pending agent/eyes oracles to `ocr_find`/`cmd`;
`state` for the blind spots (Cmd+K picker rows, shortcuts card, first responder per window); fill the
missing verb-table rows; prune or rewrite the stale Notes/Compare/terminal flows in `app_flows.json`;
Screen Recording grant in the VM so visual oracles get shots.
