# TESTING.md — test strategy and the DeepSeek hammer playbook

**Audience:** the orchestrator, an agent that plans a run, dispatches cheap DeepSeek workers, triages
what they report and writes the result. It does not need to be a strong model if it follows this
file. Paste the blocks marked **VERBATIM** into briefs unchanged.

**Goal:** find real bugs in kitchen-sink's features (notes, nvim editing, files, compare, drawers,
palette, prose, keys) at low token cost. Hardening the MCP bridge or the socket is **not** the goal
(that was waves 1–3 and they found almost no app bugs). Swift parity is not a goal.

Related: `APP-BUGS.md` (the one bug ledger), `Tests/app_flows.json` + `Tests/app_flows.md` (feature
flow table), `Tests/mcp_verbs.json` (socket verb table), `Tests/usability/` (run reports),
`AGENTS.md` "MCP server / headless control" (how the socket and the sandbox work).

---

## 0. Ten rules for the orchestrator

1. **No oracle, no lane.** Every case names a machine check that decides pass or fail: socket state,
   bytes on disk, an independent recomputation, a log line, a crash, or OCR text. "Does this look
   right?" is never a worker's call.
2. **One GUI driver at a time.** Real keyboard and mouse input is global on this Mac. Headless lanes run
   in parallel (5–8 is fine); GUI lanes run **one after another**.
3. **Replay beats review.** A finding counts when its JSON trail replays (§7). Do not ask an expensive
   model to re-check every result: replay it with a script, and spend expensive tokens only on S1/S2
   findings that replayed.
4. **Workers never edit app source, never commit, never `./ws build`, never `pkill`.** They write only
   under their lane's output folder. Check `git status` after each lane; revert anything else.
5. **Flash only:** `opencode-go/deepseek-v4.1-flash`. Never `deepseek-v4-pro` (owner blacklisted it).
   On a failed lane, send a narrower brief or `--resume-last`; do not escalate the model.
6. **Read `APP-BUGS.md` first** and give each worker the open rows for its feature, so it skips them or
   reports "still reproduces / gone" for `re-check` rows.
7. **Two misses, then stop.** A worker that fails one UI step twice records `BLOCKED` with what it saw
   and moves on. No rebuild/click/retry loops.
8. **The owner's machine comes first.** The GUI sandbox takes over the screen and turns AeroSpace off.
   Run GUI lanes only when the owner has agreed to it. Always run the GUI cleanup (§3.8) afterwards, even
   if the lane crashed.
9. **Small briefs, one lane each.** Cheap models follow short, concrete briefs with exact commands. Put
   long command sequences in a script file and have the brief call the script.
10. **Convert every verified bug into a deterministic check** (a flow in `Tests/app_flows.json`, a row in
    `Tests/mcp_verbs.json`, or a cargo test) the same day. Hammer lanes should only need to find *new* things.

---

## 1. Strategy

### 1.1 What history says (read this before planning)

| Run | Who | Found | Lesson |
|---|---|---|---|
| MCP waves 1–3 (Oct 12–13) | 6–8 flash lanes in parallel, about $0.80 total | ~15 real bugs, **all in the bridge/socket** | Flash is reliable when the oracle is crisp (crash, invalid JSON, leaked fd). It is unreliable when asked to judge behaviour: about 2/3 of its S3 claims were noise. |
| Regression hunt (Oct 13) | flash, static read of Swift vs Rust, unverified | 213 claims, many wrong | Unverified static claims are leads, not bugs. Never merge them as findings (file deleted). |
| Usability runs (Oct 13, Oct 20) | Sonnet/Haiku, GUI with real input | Every real feature bug in `APP-BUGS.md` | Real bugs come from **driving features with real input over long sessions** and checking state, disk and nvim against what was typed. |

**So the plan is:** flash workers do the long, repetitive, oracle-checked driving and soaking that found
the real bugs, but they run *scripts and tables*. They don't run on open-ended judgement.

### 1.2 Three tiers, cheapest first

| Tier | Who | What | Cost |
|---|---|---|---|
| T0 deterministic | no LLM: `./ws test`, `Tests/test_mcp_verbs.py`, a flow runner (§10 R2) | regression lock: everything already known to work stays working | free |
| T1 workers | flash, many in parallel (headless) or one at a time (GUI) | soak, fuzz, random walks, run flows, write new flows, minimise repros | about $0.02–0.15 per lane |
| T2 judgement | the orchestrator, plus one expensive look only when needed | triage, dedupe, replay, rank; read a PNG only when OCR/state cannot decide | keep under ~10% of tokens |

### 1.3 Is MCP the right tool? Yes, as an oracle; it does not drive

- **What MCP/socket gives you:** a typed, safe surface for *reading* state (`ks_state`) and reaching
  start states (`ks_do`), with read-only enforcement for the live app. That is the oracle backbone, and
  it is the reason a non-vision model can test a GUI at all.
- **What it cannot give you today:** `do:key:<spec>` is **record-only** (it stores the spec and sends no
  key). `do:` verbs call the model directly and skip the event path where the real bugs were (focus,
  responder, nvim RPC). A lane driven only by `do:` verbs proves the model, not the feature.
- **For opencode workers, bash is as good as MCP.** `printf 'state\n' | nc -U -w3 "$KS_SOCKET"` works
  with any number of sandboxes. The MCP server config in `opencode.json` points at one fixed socket.
  Use MCP for interactive agents, and bash plus the socket for hammer lanes.
- **The biggest single coverage gain** is a real in-process input verb (§10 R1). It would let workers
  drive features through the real event path without global HID input, which removes rule 2's
  bottleneck and most `ks-input` flakiness.

---

## 2. What the environment can and cannot do

| # | Limit | Effect on testing | Workaround / status |
|---|---|---|---|
| L1 | One physical screen and one global input stream | GUI lanes cannot run in parallel; the owner can't use the Mac during them | Serial GUI lanes; parallel headless. A macOS VM (Tart, at most 2 macOS VMs per host by Apple's licence) would give 2 GUI lanes; in-process input (R1) would remove the limit |
| L2 | `do:key:` is record-only | MCP alone cannot type or press shortcuts | `bin/ks-input` (real CGEvents) for GUI; R1 to fix properly |
| L3 | Headless has no AppKit | no rendering, no focus, no nvim pane; `screenshot`/`pane-shot`/`window-shot` refused | Headless lanes test state, disk and the cold paths only |
| L4 | Headless claims `shown:true` for panels that can't exist without AppKit (F0-7) | a headless "shown" is not evidence | Only trust `shown` in GUI lanes |
| L5 | Headless never drains `pending_note_open` | `do:open:<path>` is a no-op headless | open notes in GUI lanes, or use helper fileops for disk tests |
| L6 | DeepSeek flash can't be assumed to read images | screenshots are useless to workers as pictures | **`bin/ks-ocr`** (new): PNG → text lines + boxes + blank-pane check, ~0.4 s. Workers assert on text |
| L7 | `window-shot` captures only the shared window | palette, popups, menus and pop-out windows are separate windows, so they're missing from it | `ks-input shot --pid P --out f.png` (or `screencapture -x -R <window frame>`) for those, kept inside the app's frame |
| L8 | No `state` for some surfaces: Cmd+K action picker, Cmd+/ shortcuts card, `views.notes.query` (always `""`), Jira/AI/Confluence models (placeholders) | those flows only had "visual" oracles | OCR the capture (`ks-ocr --find "<row text>"`); R3 adds state |
| L9 | `ks-input` quirks: some combos drop (Esc, Cmd+=); Cmd once stuck system-wide; `click` sends clickCount 0 (`--double` reads as rename, not open); `drag` drops the mouse-up; `type` (unicode events) doesn't reach panels/fields | false failures | use `ks-input key a b c` for text into fields; osascript for Esc/Cmd combos; paced drag tool; **always run the modifier release (§3.8)**; treat click-count-dependent results as tooling-suspect |
| L10 | nvim RPC blocks while nvim waits in a pending operator (`q`, `y`, `z`, `"`) | a stray key makes every `--remote-expr` oracle hang ~2 min | the worker sends `Esc Esc` before any RPC oracle; RPC calls get `timeout 5` |
| L11 | ~~sandbox nvim socket path 105 bytes > AF_UNIX 103~~ | nvim RPC failed on every call in the GUI sandbox, so editing behaved differently from the real app | **fixed 2026-10-20:** the sandbox HOME is a short alias `/tmp/ksb-<hash>` (`bin/ks-mcp-sandbox.sh home`) |
| L12 | GUI sandbox runs the *installed* bundle | it tests the last `./ws build`, not your working tree | fresh code: copy the bundle to `.build/sandbox-app/`, swap in `.build/rust-target/release/kitchen-sink`, codesign, `KS_MCP_SANDBOX_APP=<copy>` (§3.1) |
| L13 | The flow table's `cfg-sandbox` sets `[notes] vim-mode = false` | the nvim editor, where most notes bugs were, is not covered by flows | add nvim-mode variants of the notes flows (backlog item for the A-FLOW lane) |
| L14 | `Tests/mcp_verbs.json` lags `host.rs` | 7 verbs uncovered (`go:`, `inspector-width:`, `jira-config`, `jira-dashboard`, `jira-inspector:`, `rail:`, …); the coverage test fails | H-VERB lane fills the rows |

---

## 3. Toolbox (exact commands)

All paths are relative to the repo root `~/.config/kitchen-sink`. `$LANE` is the lane id, `$OUT` its
output folder `.build/hammer/<run>/<lane>/`.

### 3.1 Sandboxes

```bash
# Headless, one per lane (parallel-safe). Keep the root short: the socket path must stay < 104 bytes.
export KS_MCP_SANDBOX=$PWD/.build/hl-$LANE          # e.g. .build/hl-H1
bin/ks-mcp-sandbox.sh start                          # builds once (cargo, live-safe); later lanes: KS_MCP_SANDBOX_NO_BUILD=1
export KS_SOCKET=$(bin/ks-mcp-sandbox.sh sock)
export SBX_HOME=$(bin/ks-mcp-sandbox.sh home)        # /tmp/ksb-<hash> -> $KS_MCP_SANDBOX/home (fixtures go here)
bin/ks-mcp-sandbox.sh stop                           # always, at the end

# GUI (orchestrator only, one at a time, owner agreed):
pgrep -x kitchen-sink > $OUT/live-pids-before.txt
KS_MCP_SANDBOX=$PWD/.build/mcp-sandbox bin/ks-mcp-sandbox.sh start --gui   # turns AeroSpace off
export KS_SOCKET=$(bin/ks-mcp-sandbox.sh sock)
export SBX_PID=$(lsof -t "$KS_SOCKET" | head -1)    # the sandbox app; every ks-input call uses --pid $SBX_PID

# GUI against code newer than the installed app:
mkdir -p .build/sandbox-app && rm -rf .build/sandbox-app/kitchen-sink.app
cp -R kitchen-sink.app .build/sandbox-app/
( cd rust && CARGO_TARGET_DIR=../.build/rust-target cargo build --release -p ws-rs -p ws-helpers )
cp .build/rust-target/release/kitchen-sink .build/sandbox-app/kitchen-sink.app/Contents/MacOS/kitchen-sink
codesign --force --sign "kitchen-sink codesign" --identifier dev.danielbaker.kitchen-sink .build/sandbox-app/kitchen-sink.app
KS_MCP_SANDBOX_NO_BUILD=1 KS_MCP_SANDBOX_APP=$PWD/.build/sandbox-app/kitchen-sink.app bin/ks-mcp-sandbox.sh start --gui
# stop: `stop` refuses this copy ("not the sandbox binary"):
#   pkill -f '.build/sandbox-app/kitchen-sink.app' ; bin/ks-mcp-sandbox.sh stop
```

### 3.2 Socket (state + start states)

```bash
ks()  { printf '%s\n' "$1" | nc -U -w3 "$KS_SOCKET"; }            # ks 'state' | jq .view
ks 'state' | jq '{view, visible, windows: (.windows|length)}'
ks 'do:open:notes'      # do: verbs: Tests/mcp_verbs.json lists every verb with its args and expected effect
```

A `do:` reply is the new state or `{"error": …}`. Long requests (> 2048 bytes) are refused.
Silent no-ops still exist (`key:<anything>`, `tool-close:<unknown>`, `hide` when nothing shows), so
**never treat a plain success reply as proof that something happened.** Compare the state before and after.

### 3.3 Real input (GUI lanes only)

```bash
bin/ks-input activate --pid $SBX_PID || echo NOT-FRONT       # before every input burst; exit 0 = frontmost
bin/ks-input key --pid $SBX_PID cmd+n                          # combos: cmd+k, ctrl+b w, ctrl+tab, f5 …
bin/ks-input key --pid $SBX_PID h e l l o                      # text into fields/panels: one key per char
bin/ks-input type --pid $SBX_PID 'hello'                       # editor only; does NOT reach panels/fields
bin/ks-input click --pid $SBX_PID --win <winid> 120 40         # window-relative; clickCount 0 (see L9)
bin/ks-input windows --pid $SBX_PID                            # window ids + frames
osascript -e 'tell application "System Events" to key code 53' # Esc, when ks-input drops it
```

`ks-input` refuses (exit 3) unless the sandbox app is frontmost. That guard keeps input off the
owner's apps, so never work around it.

### 3.4 Seeing the screen without vision

```bash
ks $'window-shot\t--view\tnotes\t--out\t'"$OUT/shots/step-07.png"   # shared window only, replies the path
bin/ks-input shot --pid $SBX_PID --out "$OUT/shots/palette.png"     # includes separate windows (palette, menus)
bin/ks-ocr "$OUT/shots/step-07.png" | jq -r '.blank, (.lines[].text)'  # JSON: lines[{text,conf,x,y,w,h}], blank
bin/ks-ocr "$OUT/shots/step-07.png" --find 'Untitled-2.md'          # exit 0 found / 1 not found
bin/ks-ocr "$OUT/shots/step-07.png" --absent 'error'                # exit 0 absent / 1 present
```

OCR is an oracle for **text**: row names, toolbar labels, error banners, "No results", file contents in
an editor. It cannot judge layout, colour, clipping or overlap. A worker that needs those writes
`NEEDS-EYES <png> <what to check>` in its report and moves on (§8 step 5).

### 3.5 nvim (the notes editor) as an oracle

```bash
NV=$(ls "$SBX_HOME"/.cache/kitchen-sink/nvim-notes-*.sock | head -1)
timeout 5 nvim --server "$NV" --remote-expr 'mode()'           # n / i / ic …
timeout 5 nvim --server "$NV" --remote-expr 'pumvisible()'      # completion popup up?
timeout 5 nvim --server "$NV" --remote-expr "expand('%:t')"     # which note is really open
timeout 5 nvim --server "$NV" --remote-expr 'b:changedtick'     # did the keys arrive?
```

Read-only queries only. Send `Esc Esc` with ks-input first (L10).

### 3.6 Logs and crashes (every lane, at the end of every case)

```bash
grep -E 'panicked|already borrowed|ERROR' "$KS_MCP_SANDBOX/daemon.log" "$SBX_HOME"/logs/*/daemon.log 2>/dev/null
ls -t ~/Library/Logs/DiagnosticReports/ | grep -i kitchen-sink | head -3   # compare to the list taken at lane start
ks 'state' >/dev/null || echo HANG-OR-DEAD                                   # 3 s socket answer = alive
```

### 3.7 Disk truth

Notes live in `$SBX_HOME/notes/`, Files root `$SBX_HOME/files/` (with the `cfg-sandbox` fixture from
`Tests/app_flows.json`). Check with `ls`, `cat`, `shasum` and `python3 -c 'import difflib…'`, never
through the app.

### 3.8 GUI cleanup (VERBATIM; run even after a failure)

```bash
osascript -e 'tell application "System Events" to key up {command, control, option, shift}'
bin/ks-mcp-sandbox.sh stop || true          # also re-enables AeroSpace
aerospace enable on 2>/dev/null || true
pgrep -x kitchen-sink > "$OUT/live-pids-after.txt"
comm -23 <(sort "$OUT/live-pids-before.txt") <(sort "$OUT/live-pids-after.txt")   # must print nothing: the live app survived
```

---

## 4. Oracles: which to use for what

| Want to know | Strongest oracle | Fallback |
|---|---|---|
| text reached the right note | file bytes on disk after Cmd+S; nvim `b:changedtick` / `getline` | OCR of the editor |
| the right note/view is showing | nvim `expand('%:t')`; `state.view`, `views.<v>.shown` (GUI only) | OCR of the title/sidebar |
| keys go to the right pane | the keystroke changes what that pane owns (changedtick, cursor, list selection) | `state` pane/focus fields (they describe the model, so they can disagree with reality; that disagreement *is* bug #5) |
| a list/count is right | independent recompute (`ls`, `difflib`) vs `state` rows | OCR row count |
| an overlay opened (Cmd+K, Cmd+/) | OCR `--find` a row label that only exists in the overlay | NEEDS-EYES |
| no crash or hang | daemon.log, DiagnosticReports, socket answers in 3 s | — |
| it looks right | **not a worker oracle.** NEEDS-EYES only | — |

---

## 5. Lane catalogue

Each lane fills the brief template (§6). "Par" = can run in parallel with other lanes.

### Headless lanes (parallel, no screen; safe while the owner works)

**H-WALK — state random walk (soak).** 300–1000 seeded random `do:` steps drawn from
`Tests/mcp_verbs.json` happy cases. After each step: the socket answers, the reply is JSON, and these
invariants hold: at most one `current` view, `visible` agrees with `windows`, frames have non-negative
sizes, no panic in the log. Report the shortest failing prefix (re-run with the same seed, halve). Par.

**H-CMP — compare against difflib.** Generate pairs of text files (empty, identical, CRLF vs LF, unicode,
5000 lines, one-byte change, trailing newline only) and folder pairs (same size + different content,
which is bug #14's case). Load each with `ks $'compare\t<a>\t<b>'`, read `state.compare.current`, and
compare the counts with Python `difflib` / `filecmp.cmp(shallow=False)`. Par.

**H-CFG — config fuzz.** Copy `commands.toml`, apply one mutation per case (missing section, wrong type,
unknown key, huge value, duplicate key, unicode, empty file), send `reload`, then check: no panic, the
socket answers, the app either applies the change or reports an error (the `commands.toml.broken-*` /
last-known-good behaviour in AGENTS.md), never a silent half-state. Par.

**H-VERB — verb-table gaps.** Run `python3 Tests/test_mcp_verbs.py`, list the uncovered verbs, read each
one's arm in `host.rs`, and write candidate rows (happy + bad-argument case) to `$OUT/new-rows.json`. The
orchestrator merges them; the coverage test must pass afterwards. Par.

### Authoring lanes (read-only on the app, parallel)

**A-FLOW — write new feature flows.** Input: one feature area + its docs (`PRODUCT.md`, the PRD,
`commands.toml` shortcut rows, `rule.md`) + `APP-BUGS.md`. Output: 5–10 new flows in the exact
`Tests/app_flows.json` schema to `$OUT/flows.json`, each citing `file:line` sources and at least one
non-visual oracle, preferring nvim/disk/OCR oracles. Priority backlog: nvim-mode variants of the notes
flows (L13), drawer focus round trips (bug #5), long-session interleavings, Files context-menu actions,
paths shelf, palette tools other than Pretty Print. The orchestrator validates the JSON (schema +
`sources` exist) before merging. Par.

### GUI lanes (serial, one at a time, owner agreed)

**G-FLOW — run flows.** Execute a list of flow ids from `Tests/app_flows.json` exactly as written:
setup, steps, oracles; one result row per flow. No improvising. If a step can't be done with the
listed input kinds, mark it `BLOCKED <reason>`. (Until the runner R2 exists, the worker is the runner.
Give it ≤ 12 flows per brief.)

**G-SOAK — long-session soak of one feature.** A fixed cycle repeated N ≥ 50 times with a perturbation
every 5th cycle (view round trip, drawer open/close, palette open+Esc, Cmd+0, prose toggle, hide+show,
external file touch). Check the oracle each cycle (for completion: `mode()` = i/ic and `pumvisible()` = 1
after the trigger; after Tab the expansion is in the buffer). Record the **first failing cycle** and the
5 actions before it. This lane shape found bugs #2, #4 and #5.

**G-MONKEY — random real input.** Seeded random keys/clicks from an allow-list (no Cmd+Q, no AI tab while
#11 is `re-check`, no clicks outside the app window, no Hyper combos), 500 events, the crash/hang/log
oracle only. On a crash, the seed + event index is the repro. Cheap and finds crashes.

### Follow-up lanes

**R-MIN — minimise and replay.** Input: one finding with its trail. Replay it 3 times; if it reproduces,
remove steps one at a time while it still fails; output the minimal trail and `repeat k/3`. Runs in the
same mode as the lane that found the bug.

---

## 6. Brief template (VERBATIM header, then fill the card)

```text
You are a test worker for the kitchen-sink macOS app. You run tests and report. You do NOT fix anything.

HARD RULES
- Never edit files outside $OUT. Never edit app source, never git commit, never run ./ws, never pkill,
  never touch the live app or its socket. Only use the sandbox socket in $KS_SOCKET.
- Never use deepseek-v4-pro or any other model.
- GUI lanes: before each input burst run `bin/ks-input activate --pid $SBX_PID`; if it fails, stop and
  report BLOCKED. Never click outside the app window. Never click the AI tab.
- A step that fails twice: record BLOCKED with what you saw, move on. No retry loops.
- Do not judge looks. Pass/fail comes only from the ORACLE below. If only eyes could decide, write
  NEEDS-EYES <png> <what to look at> and continue.
- Known bugs (do not re-report; for "re-check" rows say "still reproduces" or "gone"): <paste rows>
- Finish within <N> cases or <M> minutes, then write the report even if incomplete.

ENVIRONMENT
  KS_SOCKET=<…>  SBX_HOME=<…>  SBX_PID=<GUI only>  OUT=<…>  KS_MCP_SANDBOX=<…>
  Commands you may use: (paste the relevant blocks from TESTING.md §3)

LANE <id>: <one-line goal>
  Setup: <fixtures, start state>
  Cases / cycle: <exact list or the generator + seed>
  ORACLE per case: <exact commands and what counts as pass>
  Stop condition: <…>

REPORT (write both files):
  $OUT/findings.jsonl  one JSON object per finding (schema below)
  $OUT/summary.md      counts: run / pass / fail / blocked / needs-eyes, then one line per finding
```

Keep the filled brief under ~150 lines. Anything longer goes into a script the brief calls.

---

## 7. Finding format and replay

One line per finding in `findings.jsonl`:

```json
{"id":"G-SOAK-03","sev":"S2","feature":"notes/completion","title":"popup stops after drawer toggle",
 "mode":"gui","seed":17,"setup":["do:open:notes"],
 "trail":[{"input":"key esc esc"},{"input":"type Go"},{"input":"key ctrl+j"},{"input":"key ctrl+k"},{"input":"type bash"}],
 "oracle":{"cmd":"timeout 5 nvim --server $NV --remote-expr 'pumvisible()'","expected":"1","actual":"0"},
 "repeat":"4/5","evidence":["shots/c41.png","daemon.log:1204"],"suspect":"rust/ws-rs/src/app/host.rs (route_key)"}
```

- `trail` uses the input kinds from `Tests/app_flows.json` (`key`, `type`, `click-rel`, `wait`, `do:`,
  `raw`), so a trail is directly a flow, and the replay tool and the flow runner are the same thing.
- **Severity:** S1 crash, hang or data loss/corruption · S2 a feature stops working, or text goes to the
  wrong place · S3 wrong but with a workaround · S4 cosmetic or minor.
- `repeat` is required. A 1/1 finding is a lead and stays out of `APP-BUGS.md` until R-MIN replays it.

---

## 8. Triage without an expensive reviewer

The orchestrator, per run:

1. **Mechanical checks (script, no LLM):** `git status` clean apart from `$OUT`; `findings.jsonl` parses;
   every finding has `trail`, `oracle`, `repeat`.
2. **Dedupe** against `APP-BUGS.md` and across lanes (same feature + same oracle failure = one bug).
3. **Replay** each S1/S2 and any S3 with `repeat` ≥ 2/3: an R-MIN lane (flash) or the flow runner. Drop
   anything that doesn't reproduce; mark it `not-reproduced` in the ledger below.
4. **Classify** with the cheap checks: is the "expected" value cited from docs/code (`sources`)? If
   expected was the worker's opinion, it is `by-design?` and goes to the owner, not to a fix.
5. **Spend expensive tokens only here:** S1/S2 bugs that replayed, plus `NEEDS-EYES` PNGs whose case
   matters (look at ≤ 5 PNGs per run, chosen by severity). One look per item, no re-run loops.
6. **Record:** append verified bugs to `APP-BUGS.md`, keep the full run in
   `Tests/usability/RUN-<date>.md` (lane table + findings + not-covered), and write every triage verdict
   to `.build/hammer/<run>/ledger.jsonl` (`id, verdict: verified|not-reproduced|misread|by-design?, evidence`).
7. **Lock:** for each verified bug, an A-FLOW or the orchestrator adds the regression flow/test (rule 10).

---

## 9. Orchestrator runbook

```bash
RUN=$(date +%Y-%m-%d)-a ; mkdir -p .build/hammer/$RUN
git status --short > .build/hammer/$RUN/git-before.txt
caffeinate -d &  CAF=$!            # GUI runs: the screen must stay unlocked

# 1. Build once (live-safe), then start one headless sandbox per headless lane.
( cd rust && CARGO_TARGET_DIR=../.build/rust-target cargo build --release -p ws-rs -p ws-helpers )
for L in H-WALK H-CMP H-CFG H-VERB; do
  KS_MCP_SANDBOX=$PWD/.build/hl-$L KS_MCP_SANDBOX_NO_BUILD=1 bin/ks-mcp-sandbox.sh start
done

# 2. Dispatch headless + authoring lanes in parallel (one brief file per lane).
for L in H-WALK H-CMP H-CFG H-VERB A-FLOW-notes A-FLOW-files; do
  mkdir -p .build/hammer/$RUN/$L
  node ~/.claude/skills/opencode-delegate/scripts/relay.mjs \
    --brief .build/hammer/$RUN/briefs/$L.txt \
    --model opencode-go/deepseek-v4.1-flash \
    --cd "$PWD" --timeout 90m --out-dir .build/hammer/$RUN/$L/relay \
    > .build/hammer/$RUN/$L/relay.log 2>&1 &
done
wait     # orchestrators that can background a command: run this block in the background, check every ~10 min

# 3. GUI lanes, ONE AT A TIME (owner agreed): start --gui, dispatch one G-* lane, wait, cleanup (§3.8). Repeat.

# 4. Stop every sandbox, triage (§8), write the report, commit docs only (on main).
for L in H-WALK H-CMP H-CFG H-VERB; do KS_MCP_SANDBOX=$PWD/.build/hl-$L bin/ks-mcp-sandbox.sh stop; done
kill $CAF
```

`result.json` in each `relay/` folder holds the worker's final message. Its `touchedFiles` lists the
whole dirty tree, so use `git status` against `git-before.txt` instead. Treat the worker's "all passed"
as a claim: the files in `$OUT` are the evidence.

**Budget guide:** a full wave (6 headless + 3 GUI lanes) costs well under $1 in flash. Orchestrator
tokens are dominated by reading reports, so keep reports to the schema and read `summary.md` first.

---

## 10. What to build next (ranked by coverage gained per effort)

| # | Item | Why | Size |
|---|---|---|---|
| R1 | **Real in-process input verb**: `do:input:key:<combo>` / `do:input:text:<s>` that builds an `NSEvent` and posts it into the app's own event queue (`NSApp.postEvent`), so it goes through the local key monitor (`host.rs` `route_key_event`), the responder chain and the terminal views exactly like typing | MCP becomes a real driver; no global HID, no stuck Cmd, no frontmost race; more than one GUI sandbox could run at once. **Spike first:** confirm that local monitors see posted events and how an inactive app's key window behaves | M |
| R2 | **Flow runner** `Tests/run_flows.py`: executes `Tests/app_flows.json` (and finding trails) deterministically with ks-input/R1 + oracles, JSON results | T0 regression lock for every flow and every verified bug; replay for triage step 3 with no LLM | M |
| R3 | **State for the blind spots**: `views.notes.query`, Cmd+K picker rows, shortcuts-card shown, the AppKit first responder per window, and the nvim mode/file in `views.notes` | turns the visual-only flows into state oracles; makes bug #5 type disagreements checkable | S–M |
| R4 | Fill the 7 missing verb-table rows (H-VERB) | the coverage test is red | S |
| R5 | nvim-mode flow variants + a notes `cfg` fixture with vim-mode on | covers the editor where most real bugs were | S |
| R6 | One macOS VM (Tart) with the app + TCC grants baked in | GUI lanes stop needing the owner's screen | M–L |

---

## Appendix: what was removed and why (2026-10-20)

Deleted because they steered agents toward plumbing, stale facts or unverified claims (all in git history,
`git log -- <file>`): `PLAN-mcp-testing.md`, `PLAN-mcp-wave3.md` (also named the blacklisted v4-pro tier),
`PLAN-mcp-headless.md` (implemented), `MCP-TEST-FINDINGS.md`, `TESTING-OVERVIEW.md` (replaced by this
file), `REGRESSIONS-swift-to-rust.md` + `PLAN-regression-hunt.md` (213 unverified parity claims; parity is
not a goal), `Tests/usability/PLAN-hammer.md` (replaced by §5), `Tests/usability/CASES.md` (hard-coded
coordinates, a missing setup doc, stale "known open" list).
