# PLAN-mcp-testing.md — hammer-test the kitchen-sink MCP bridge (orchestrator brief for DeepSeek workers)

> **Orchestrator (Claude Code / Sonnet):** you run this plan. Save this text into the
> repo as `PLAN-mcp-testing.md` and commit it on `main` (`docs: MCP hammer-test plan`).
> Workers get **§2 (rules) + §3 (harness + how to drive MCP) + their one lane card (§4)** —
> nothing else. Never hand a worker the whole doc; weaker models drift when given choices.
> This is a **testing** plan: workers find and document problems. **Nobody fixes code here.**
> Fixes become a follow-up plan built from §7.

## O. Orchestration mechanics (orchestrator only — never part of a brief)

You dispatch, verify and summarise. You never let a worker's claim through unverified.

**Setup, once:**
1. `command -v opencode` and `opencode auth list` show a stored provider (`opencode-go/…`).
2. `git status --short > .build/mcp-hammer/git-status-before.txt` (the owner often has
   uncommitted work in the tree; workers must leave it exactly as it is).
3. Build once, live-safe: `cd rust && CARGO_TARGET_DIR=../.build/rust-target cargo build --release -p ws-rs -p ws-helpers`.
   **Never `./ws build`** (it pkills the owner's live app).
4. Do card **T0** (§4) yourself: it is small and every lane depends on it. Commit it
   (`test: ks-mcp hammer harness`).
5. Start one isolated sandbox per lane, each on its own root. Lane ids are 2 chars
   (`L1`…`L8`), which keeps the socket path under the 104-byte `sun_path` limit:
   ```bash
   for L in L1 L2 L3 L4 L5 L6 L7; do
     KS_MCP_SANDBOX=$PWD/.build/mh/$L KS_MCP_SANDBOX_NO_BUILD=1 bin/ks-mcp-sandbox.sh start
   done
   ```
   The socket for lane `Lx` is `$PWD/.build/mh/Lx/tmp/ws-notes.sock`. L7 starts and stops its
   own sandboxes (it tests the lifecycle), so it gets a root of its own and may restart it.
   L8 uses the default root `.build/mcp-sandbox` (the one `opencode.json` points at).

**Per lane:**
1. Write the brief to `.build/mcp-hammer/briefs/<LANE>.txt`: §2 verbatim + §3 verbatim + the
   lane card verbatim, with `<SOCK>` and `<OUT>` substituted
   (`<OUT>` = `.build/mcp-hammer/<LANE>`).
2. Dispatch it as a backgrounded Bash call (`run_in_background: true`):
   ```bash
   node ~/.claude/skills/opencode-delegate/scripts/relay.mjs \
     --brief .build/mcp-hammer/briefs/<LANE>.txt \
     --model opencode-go/deepseek-v4.1-flash \
     --cd /Users/danielbaker/.config/kitchen-sink \
     --timeout 90m --out-dir .build/mcp-hammer/runs/<LANE>
   ```
   Workers need the `build` agent (they run bash and write to `<OUT>`). The safety net is the
   git-status check below, not the agent mode.
3. On exit: read `runs/<LANE>/result.json`, then `<OUT>/findings.jsonl` and `<OUT>/summary.md`.
   Run `git status --short` and diff it against `git-status-before.txt`. **Any change to a
   tracked or untracked repo file outside `.build/` = the worker broke rule 1:** revert only
   what that worker touched (compare `touchedFiles`) and note it in §8.
4. **Verify (the important part).** For every finding with severity S1 or S2, re-run its `repro`
   yourself against a fresh sandbox (`KS_MCP_SANDBOX=$PWD/.build/mh/V …`). Mark it
   `verified`, `not-reproduced`, or `misread` (the behaviour is real but documented or intended;
   check AGENTS.md "MCP server / headless control" and BACKLOG.md "MCP / headless control").
   Spot-check at least 3 S3 findings per lane. DeepSeek reports are claims, not evidence.
5. Rework: a lane that returns fewer than 60% of its listed cases run, or with repros that don't
   parse, gets one `--resume-last` delta brief naming the missing case ids. Max 2 attempts, then
   `opencode-go/deepseek-v4-pro` once, then report the gap.

**Wave rule:** lanes L1–L7 are independent (own sandbox, own `<OUT>`): dispatch them in parallel.
Run **L5 last or alone**: its burst and leak tests load the machine and would make other lanes'
timing-sensitive cases flaky. L8 runs after L1–L6 have been verified (it needs the opencode.json
toggle in §4 L8). L9 is orchestrator-only and optional.

**Teardown:** `for L in L1 … L8; do KS_MCP_SANDBOX=$PWD/.build/mh/$L bin/ks-mcp-sandbox.sh stop; done`,
plus `bin/ks-mcp-sandbox.sh stop` for L8. Confirm `aerospace list-workspaces --focused` still
answers (only L9 touches AeroSpace). Restore `opencode.json` if L8 changed it.

## 0. Context

**What exists (assessed 2026-10-12 at `4ee476f`).**
- `bin/ks-mcp` → `bin/ks-mcp.py` (485 lines): a stdio MCP server (newline-delimited JSON-RPC 2.0,
  protocol revisions `2024-11-05`…`2025-11-25`) bridging to the daemon's Unix socket. Three tools:
  `ks_state` (read state JSON), `ks_do` (run `do:<action>`; hidden in read-only), `ks_raw`
  (allow-listed `screenshot | screenshot-permission | compare | pane-shot`; read-only allows only
  `screenshot-permission`). Single-threaded; one connection per call.
- `bin/ks-mcp-sandbox.sh start [--gui]|stop|status|sock|reset`: isolated headless daemon under
  `$KS_MCP_SANDBOX` (default `.build/mcp-sandbox`), own `TMPDIR`/`WS_HOME`/config copy.
- `kitchen-sink headless`: the daemon with no AppKit; `state`/`do:`/raw verbs run on the socket thread.
- `opencode.json` registers `kitchen-sink` (the **live** app, read-only) as **enabled** and
  `kitchen-sink-sandbox` as disabled. Every opencode run in this repo, so every worker, sees the
  live server. Rule 3 forbids using it.
- Deferred by design (BACKLOG.md): the big tool catalog (helper/jira/confluence/notify/settings),
  the L1/L2 suites, and modern protocol support.

**Why it is testable without computer control.** DeepSeek workers can't see or click the screen,
and they don't need to. The MCP surface is pure text: JSON-RPC in, JSON state out. Every `do:`
action's effect shows up in `state` (`view`, `visible`, `views.<v>.shown/key/terminal/…`), so a
worker can assert pre-state → action → post-state entirely through MCP. What it **cannot** test
is rendering. That is `pane-shot`, which in headless mode captures the owner's real screen
(finding F0-3), so it is out of bounds for workers (L9 covers it, orchestrator-only).

**Seed findings (confirmed by direct probing during planning; workers must not re-report
these as new — cite the id if a case re-hits one):**

| id | sev | finding | evidence |
|---|---|---|---|
| F0-1 | S2 | `Tests/test_ks_mcp.py` **does not exist and was never committed**, yet `./ws test mcp` (`bin/run-tests.sh` `mcp)` case), AGENTS.md, BACKLOG.md and `PLAN-mcp-headless.md` all say it locks the server. `./ws test mcp` fails. | `git log --all -- Tests/test_ks_mcp.py` is empty |
| F0-2 | S2 | Socket `LISTEN_BACKLOG = 4` (`rust/ws-rs/src/app/socket.rs:41`). A burst of 10 / 50 / 200 parallel `state` connects got 4 / 4 / 6 answers; the rest `ECONNREFUSED`. ks-mcp maps that to "daemon not answering… start the app", which is misleading: the daemon is up. Several agents sharing one daemon will hit it. | Python burst probe |
| F0-3 | **S1** | `ks_raw pane-shot` on the **headless sandbox** captured the owner's real focused terminal (3306×3584 PNG) and saved it to `/private/tmp/<date> pane.png`, outside the sandbox. `pane-shot` defaults (`save = true`, `copy = true`, `[pane-shot]` in `commands.toml`) also ask for a clipboard copy; in the probe the clipboard was not changed, but that's unverified, not guaranteed. The sandbox does not isolate capture verbs. | `host.rs` `pane_shot_message` |
| F0-4 | S3 | A `ks_do` action longer than about 2 KB is silently truncated (`READ_MAX = 2048`, `socket.rs`), treated as an unknown verb, and answered with plain state: no error. | probe id 10 |
| F0-5 | S3 | An unknown `do:` verb returns full state, identical to success. A typo is undetectable (known: AGENTS.md). Exception: `do:board:*` returns `{"error":"unhandled action …"}` → `isError`. | probe |
| F0-6 | S3 | One client that connects and sends nothing stalls every other client for about 2.0 s (single accept loop, `SO_RCVTIMEO` 2 s, `socket.rs:327`). Combined with F0-2, a few idle connects deny service. | idle-client probe |
| F0-7 | S4 | `do:toggle-terminal` on headless leaves `views.<v>.terminal` false (probably correct: no terminal off AppKit). Needs a decision on whether headless should report the toggle or refuse it. | probe |

## 1. Strategy in one page

| Lane | What it hammers | Approx cases | Needs |
|---|---|---|---|
| T0 | harness: stdio MCP client + per-lane sandbox recipe | — | orchestrator |
| L1 | MCP / JSON-RPC protocol conformance | 40 | own sandbox |
| L2 | tool argument validation + read-only enforcement | 35 | own sandbox |
| L3 | `do:` verb coverage: every verb → state diff | 1 per verb (~60) | own sandbox |
| L4 | state-model invariants under seeded random walks | 20 walks × 200 steps | own sandbox |
| L5 | concurrency, slow clients, leaks, daemon death | 25 | own sandbox, **run alone** |
| L6 | raw verbs (`screenshot -g`, permission, `compare`) + timeouts | 20 | own sandbox |
| L7 | sandbox lifecycle + isolation leaks | 20 | own sandbox roots |
| L8 | agent ergonomics: a DeepSeek agent using the **native** MCP tools to do tasks | 10 tasks | default sandbox + `opencode.json` toggle |
| L9 | GUI sandbox + `pane-shot` rendering checks | 6 | **orchestrator only, owner present** |

Why these lanes: L1/L2 test the Python bridge in isolation. L3/L4 test that the bridge reports
the daemon's truth and that this truth is coherent. L5/L6 hit the socket layer's known weak spots
(F0-2, F0-6, deferred replies). L7 tests the "safe to hand to an agent" promise. L8 answers the
owner's real question: can a model that only has MCP actually drive this app?

## 2. Rules for every worker (paste verbatim into every brief)

1. **You are a tester, not a fixer.** Do not edit, create, delete or format any file in the repo
   except under your output directory `<OUT>`. No `git add`/`commit`/`stash`/`checkout`/`reset`.
   Write every script, log and note under `<OUT>`. The orchestrator diffs `git status` after you.
2. **Drive the app only through MCP**, via the harness in §3 against **your** socket `<SOCK>`.
   You have no screen and no mouse; don't try (no `osascript`, `screencapture`, `open`,
   `cliclick`, AppleScript, Accessibility). Reading source code with `grep`/`cat` to form
   hypotheses is encouraged. Raw `nc -U <SOCK>` is allowed **only** in L5 and only to compare
   against the MCP path.
3. **Never touch the owner's live app.** Do not use any MCP tool from the opencode-registered
   server named `kitchen-sink` (it points at the live app). Never use
   `$TMPDIR/ws-notes.sock` or any socket that is not `<SOCK>`. Never run `./ws build`, `./ws`
   anything, `pkill`, `killall`, or `kill` on a pid you did not start. Never run
   `bin/ks-mcp-sandbox.sh` unless your card says so, and then only with `KS_MCP_SANDBOX` set
   to your own root.
4. **Forbidden verbs** (they capture the owner's real screen or pop UI on it — finding F0-3):
   `ks_raw pane-shot` with any arguments that parse (only the *invalid-argument* cases your card
   lists are allowed); `ks_raw screenshot` with `-r`/`--raw` or with no `-g`; anything with
   `--gui`. `do:screenshot*` actions are forbidden too. If you think a case needs one, write it
   as a `"blocked"` case and move on.
5. Every case you run gets **one JSON line** in `<OUT>/findings.jsonl`, pass or fail:
   ```json
   {"case":"L2-07","title":"ks_do action with CR","status":"pass|fail|blocked|flaky",
    "severity":"S1|S2|S3|S4|null","repro":"<exact command(s) to re-run, copy-pasteable>",
    "expected":"…","actual":"<verbatim output, trimmed to 400 chars>",
    "seed":"F0-x if this re-hits a seed finding, else null",
    "suspect":"<file:line from grep, or null>","confidence":"high|medium|low"}
   ```
   Severity: **S1** crash, hang over 30 s, data written outside the sandbox, live-app or
   owner-screen contact, read-only bypass. **S2** wrong result or state, protocol violation a real
   client would trip on, a lost or misattributed reply. **S3** misleading error, inconsistency, a
   docs mismatch. **S4** nit. A `fail` without an exact runnable `repro` is worthless; don't write
   one. Re-run every `fail` once before recording it; if it doesn't reproduce, use `flaky` with
   both outputs.
6. Don't invent behaviour. `actual` must be pasted output, never a paraphrase. If you are guessing
   at a cause, put it in `suspect` with `confidence: low`.
7. Time box: stop at 75 minutes. Run cases in the order listed; record what you skipped.
8. Finish with `<OUT>/summary.md`, 25 lines or fewer: cases run / pass / fail / blocked; failures
   by severity as one line each (`case — title — one-line why`); anything that surprised you;
   cases you skipped and why. Your final message is that summary.

## 3. Harness + how to drive MCP (paste verbatim into every brief)

All calls go through `Tests/ks_mcp_client.py`. It starts `bin/ks-mcp` as a subprocess on stdio,
does the `initialize` → `notifications/initialized` handshake, and prints every response line as
JSON.

```bash
C="python3 -I Tests/ks_mcp_client.py --socket <SOCK>"

$C tools                                     # tools/list
$C call ks_state                             # one tool call, prints the result JSON
$C call ks_do '{"action":"open:files"}'
$C call ks_raw '{"verb":"screenshot","args":["-g"]}'
$C --read-only call ks_do '{"action":"hide"}'   # server started with --read-only
$C --env KS_MCP_READ_ONLY=yes tools          # extra env for the server process
$C raw <file>          # send the file's lines verbatim (no handshake), print every reply line
$C raw -               # same, from stdin
$C --no-handshake call ks_state             # skip initialize (protocol tests)
$C --timeout 20 …                            # per-response wait (default 15 s); prints TIMEOUT
```

Output: one JSON object per response line on stdout, then `# exit=<server exit code> stderr=<first 300 chars>`.
`call` also prints `# state.view=… state.visible=…` when the result text is state JSON, so you can
diff quickly. For state diffs:
```bash
$C call ks_state | python3 -I Tests/ks_mcp_client.py --extract-state > <OUT>/s1.json
# extract-state reads client output on stdin and writes only the inner state JSON
python3 -I -c 'import json,sys;a,b=(json.load(open(f)) for f in sys.argv[1:]);…' s1.json s2.json
```
**Reset between cases** that change state (unless the case tests sequences): `do:hide` then
`do:open:notes`, and confirm `view == "notes"`. There is no full reset verb; L7 owns
`bin/ks-mcp-sandbox.sh reset`.

Reference (read these, don't edit them): `bin/ks-mcp.py` (whole server),
`rust/ws-rs/src/app/socket.rs` (wire protocol), `rust/ws-rs/src/app/host.rs` (search `fn do_action`,
`dispatch_test_do`, `raw_deferred`, `pane_shot_message`), AGENTS.md "MCP server / headless control",
BACKLOG.md "MCP / headless control".

## 4. Cards

### T0 — harness (orchestrator does this; commit it)
Files: new `Tests/ks_mcp_client.py`. Pure stdlib, Python ≥ 3.11, about 150 lines.
- Spawns `bin/ks-mcp` with `--socket`, plus `--read-only` and extra `--env K=V` if given.
  `subprocess.Popen` with pipes; a reader thread pulls stdout lines into a queue; responses are
  waited for with `--timeout`.
- Subcommands `tools`, `call NAME [JSON]`, `raw FILE|-`, and the `--extract-state` filter, as
  specified in §3. `raw` writes each line, then waits until the stream is idle for 1 s or the
  timeout passes, then closes stdin, waits for exit, and prints `# exit=…`.
- Self-test before dispatching anything: start `.build/mh/L1`, run `$C tools`, `$C call ks_state`,
  `$C call ks_do '{"action":"open:files"}'` (expect `state.view=files`), and `$C raw` on a
  `{"jsonrpc":"2.0","id":1,"method":"ping"}` line.
- Optional, after the run: turn the verified L1/L2 cases into the missing `Tests/test_ks_mcp.py`
  (F0-1) as a fake-daemon L0 suite. That's a separate card for the fix plan, not this one.

### L1 — protocol conformance
`<SOCK>` = `.build/mh/L1/tmp/ws-notes.sock`. Use `$C raw` for everything except where noted.
Expected behaviour comes from the MCP spec (newline-delimited JSON-RPC 2.0) and `bin/ks-mcp.py`'s
docstring. Cases:
- L1-01…05 `initialize` with protocolVersion `2025-11-25`, `2025-06-18`, `2024-11-05`, `1999-01-01`,
  missing: echoes a supported one, otherwise answers the latest. `serverInfo`/`capabilities` shape.
- L1-06 tool calls with no `initialize` at all (the server doesn't enforce order; record it as S4
  if it works).
- L1-07 two `initialize` in one session.
- L1-08…14 `id` variants: `0`, `-1`, `"abc"`, `""`, `1.5`, `9007199254740993`, `{"x":1}`. The reply id
  must equal the request id exactly (byte-compare). `id: null` must be treated as a notification
  (no reply); check which choice the code makes and whether a client would trip on it.
- L1-15 missing `"jsonrpc"`; L1-16 `"jsonrpc":"1.0"`.
- L1-17 batch array `[{…},{…}]` (2025 spec: batching removed → expect a single error, not a crash).
- L1-18 empty array `[]`; L1-19 a JSON string/number/`null` as the whole line.
- L1-20 invalid JSON, then a valid request on the next line (the server must survive).
- L1-21 a 1 MB single line; L1-22 a line with a raw NUL; L1-23 invalid UTF-8 bytes (write them
  with `printf '\xff\xfe'`).
- L1-24 `\r\n` line endings; L1-25 two requests on one line (`{…}{…}`).
- L1-26 stdin closed mid-request (no trailing newline): exit code, nothing on stdout.
- L1-27 stdout purity: across all of L1, every stdout line must parse as JSON-RPC (grep your raw
  logs for lines that don't).
- L1-28 `ping`; L1-29 `tools/list` with a `cursor` param; L1-30 `resources/list`, `prompts/list`
  (expect -32601; is that right given the capabilities it advertises?).
- L1-31 `tools/call` with `params` missing / `null` / an array / `name` empty / `name` 123.
- L1-32 a notification with an unknown method (no reply expected); L1-33 `notifications/cancelled`
  for an in-flight id.
- L1-34 50 pipelined requests written in one go: all 50 replies, in order, ids matching.
- L1-35 the daemon down (point `--socket` at a non-existent path): `initialize` still works;
  `ks_state` gives `isError` with the "not answering" text, **not** a JSON-RPC error.
- L1-36…40 any further cases you find by reading `serve()` / `handle_request()`; list your reasoning.

### L2 — arguments + read-only enforcement
`<SOCK>` = `.build/mh/L2/tmp/ws-notes.sock`.
- L2-01…10 `ks_do` `action`: missing, `""`, `null`, `123`, `["open:files"]`, `"open:files\n"`,
  `"open:files\r"`, `"open:files\rstate"`, `"open:files\u0000x"`, `"  open:files  "`. Record the
  socket-side effect via `ks_state` after each.
- L2-11 `action` of 2047 / 2048 / 2049 / 5000 bytes made of a valid verb plus padding (F0-4: find
  the exact boundary; is the truncation ever *harmful*, i.e. does a truncated payload become a
  different valid verb?).
- L2-12 non-ASCII: `"open:ファイル"`, emoji, RTL text. The state must still parse.
- L2-13 extra unknown argument keys (the schema says `additionalProperties: false`; the server
  doesn't validate. Record what happens).
- L2-14…20 `ks_raw`: `verb` missing / not in the enum / `"PANE-SHOT"` / `"pane-shot "`; `args` as a
  string vs a list vs `[1]` vs `[null]` vs containing `\t` / `\n` / `\r`. **`pane-shot` only with
  arguments that must fail parsing**: `["--bogus"]`, `["--lines","abc"]`, `["--pane"]` (missing
  value), `["--help"]`. If any of those does *not* return an error, stop the lane immediately and
  report S1 (it may have captured the screen).
- L2-21…28 read-only: start the server with `--read-only`; with `--env KS_MCP_READ_ONLY=` values
  `1`, `true`, `TRUE`, ` yes `, `on`, `0`, `false`, `2`, `""`. Check `tools/list` (ks_do hidden?)
  and `call ks_do` (refused with -32602?) for each; also `--read-only` with `KS_MCP_READ_ONLY=0`.
- L2-29 read-only bypass hunt: can any `ks_raw` verb or argument shape reach a mutating socket verb
  (`reload`, `restart`, `do:`) while read-only? Try `ks_raw` `verb:"screenshot-permission"`
  with `args:["x"]`, args holding `do:hide`, and so on. Any state change = S1.
- L2-30 `tools/call` `name:"ks_state"` with `arguments: {"anything":1}`, `arguments:null`, `arguments:[]`.
- L2-31…35 your own cases from reading `tool_ks_do` / `tool_ks_raw`.

### L3 — `do:` verb coverage
`<SOCK>` = `.build/mh/L3/tmp/ws-notes.sock`.
1. Build the verb inventory first, from source, into `<OUT>/verbs.txt`: the string literals matched in
   `do_action` and `dispatch_test_do` in `rust/ws-rs/src/app/host.rs`, plus each view's
   `register`/`test_do` in `rust/ws-rs/src/views/*.rs` (grep `test_do`, `"open:"`, `strip_prefix(`).
   Note each verb's argument shape (`open:<view>`, `focus:<x>`, `pane:<x>`, `select:<n>`,
   `header-style:<v>`, `esc-hides:<bool>`, `tool:<name>`, `compare:open:L|R`, …). The orchestrator's
   sample: `ai back compare compare: confluence cycle cycle-back esc-hides: file: filefast files focus:
   folder-copy:left|right folder-key:ctrl-n|ctrl-p header-style: health health-checks hide
   hide-on-focus-loss: home jira jira-jump jira-poll-off jira-poll-on jira-setup key: notes notes-find
   notes-grep open: open-sub: pane: paths paths: prettyprint rebuild-card: reset-size return select:
   show switcher switcher-hide term terminal text-table toggle toggle-browser toggle-terminal tool:
   tool-close: window`. Treat that list as incomplete.
2. For each verb (and each `open:<view>` for every key of `state.views`): reset → `ks_state` (pre) →
   `ks_do` → `ks_state` (post). Classify: **effect** (which keys changed), **no-op** (state identical:
   expected or suspicious?), **error** (`isError`), **hang** (timeout). Excluded: `screenshot*`
   (rule 4), `jira-poll-on` (may start a network poller; run it only if `jira-poll-off` brings it back,
   and always call `jira-poll-off` after).
3. Pairs: `toggle` twice, `toggle-terminal` twice, `toggle-browser` twice, `show`/`hide`,
   `open:X` then `back`, `cycle` N times where N = number of views, `cycle` then `cycle-back`.
   Do you end where you started?
4. Argument abuse per parametrised verb: missing value (`open:`), unknown value (`open:nope`),
   wrong type (`select:-1`, `select:99999`, `esc-hides:maybe`), path traversal for `file:`/`paths:`/`compare:`
   (`../../../../etc/hosts`, a missing file, a directory, a 300-char path). **Check if `file:`/`compare:`
   writes or reads anything outside the sandbox**: `find "$HOME/.config" "$HOME/Library" /private/tmp
   -newer <OUT>/marker -maxdepth 3 2>/dev/null` before and after (create the marker with `touch` first).
5. Output: `<OUT>/verbs.tsv` (verb, args, class, changed keys, notes), plus one findings line per verb
   (pass if the effect matches the verb name; fail with reasoning otherwise). Expected no-ops on headless
   (no window: `reset-size`, `window`, `rebuild-card:`) are `pass` with a note. Flag any verb where
   headless state claims a UI effect that can't exist without AppKit (as F0-7 does) as S3.

### L4 — state invariants (seeded random walks)
`<SOCK>` = `.build/mh/L4/tmp/ws-notes.sock`. Write `<OUT>/walk.py`, which uses `Tests/ks_mcp_client.py`
as a module or subprocess and keeps one server process for the whole walk (faster).
- Action alphabet: the L3-safe verbs (`open:<each view>`, `cycle`, `cycle-back`, `back`, `home`, `show`,
  `hide`, `toggle`, `toggle-terminal`, `toggle-browser`, `focus:*`, `pane:*`, `select:0..3`,
  `switcher`, `switcher-hide`, `esc-hides:true|false`, `hide-on-focus-loss:true|false`,
  `header-style:*` values found in source) plus 5% garbage verbs.
- 20 walks × 200 steps, seeds 1…20, `random.Random(seed)`. After **every** step check:
  I1 the reply parses as JSON and has the same top-level key set as the first state;
  I2 `view` ∈ `views` keys; I3 if `visible` then `views[view].shown` is true;
  I4 at most one view has `shown: true` (verify from source whether that's intended; if not, drop it
  and say so); I5 `pid` is constant (no daemon restart); I6 the `ks_do` reply equals an immediate
  `ks_state` (modulo volatile keys you identify: list them); I7 `paletteCommands` is stable;
  I8 a garbage verb leaves state byte-identical (modulo volatile keys).
- On a violation: record the seed, the step and the last 10 actions, then **shrink** (delete
  actions one at a time while it still fails) and put the minimal sequence in `repro` as a `$C` script.
- Report invariant pass counts per walk in `summary.md`.

### L5 — concurrency, slow clients, leaks, death (run alone)
`<SOCK>` = `.build/mh/L5/tmp/ws-notes.sock`. Raw `nc`/Python socket access is allowed in this lane, to
separate bridge bugs from daemon bugs. Daemon pid: `lsof -t <SOCK>`.
- L5-01 burst: N = 2, 4, 5, 8, 16, 64 parallel raw `state` connects; success counts (F0-2: confirm the
  threshold and say whether a client-side retry in ks-mcp would mask it).
- L5-02 the same burst through N parallel `bin/ks-mcp` processes, each doing `call ks_state`: what error
  text does the agent see? (Is "daemon not answering… start the app" misleading? S3 if yes.)
- L5-03 idle clients (F0-6): K = 1, 2, 4, 8 sockets that connect and never send; latency of a normal
  `state` meanwhile. With 4–5 idle, does a normal client get refused entirely?
- L5-04 slowloris: send `sta`, wait 3 s, send `te\n`. Is the reply `state`, empty, or something else?
- L5-05 a client that sends `state\n` and never reads (reply over 2 KB): does the 5 s `SO_SNDTIMEO`
  free the loop? Measure the next client's latency.
- L5-06 a request split over two writes 10 ms apart (`do:op` + `en:files`): does the single `read`
  see a partial verb, and what does it execute?
- L5-07 10,000 sequential `state` calls: wall time, p50/p99 latency, daemon RSS (`ps -o rss= -p PID`)
  and open fd count (`lsof -p PID | wc -l`) at the start, at 5k and at 10k. Growth = leak (S2).
- L5-08 10,000 alternating `do:open:files`/`do:open:notes` through **one** ks-mcp process: the same metrics
  for both the daemon and the ks-mcp process.
- L5-09 two ks-mcp processes doing interleaved `ks_do` storms (500 each): any reply carrying the other
  process's state or a malformed JSON?
- L5-10 kill the daemon (`kill -TERM` on **your** sandbox pid only, from `lsof -t <SOCK>`) during an
  L5-08 run: ks-mcp's behaviour (error text, does it stay up, recovery once the orchestrator-style
  restart `KS_MCP_SANDBOX=$PWD/.build/mh/L5 KS_MCP_SANDBOX_NO_BUILD=1 bin/ks-mcp-sandbox.sh start` runs).
  You may run that start command in this case only.
- L5-11 a stale socket file (stop, then `touch`-replace the socket path with a regular file): ks-mcp's
  error text.
- L5-12 `ks_raw compare` with `--wait` on two files under `.build/mh/L5/tmp`: it blocks up to 300 s by
  design. While it waits, can another ks-mcp process still call `ks_state`? (The daemon defers the reply;
  the bridge is single-threaded, so the first process is blocked. Confirm both.)
- L5-13…25 your own, from reading `accept_loop`/`handle_conn` in `socket.rs`.

### L6 — raw verbs (safe subset)
`<SOCK>` = `.build/mh/L6/tmp/ws-notes.sock`.
- L6-01 `screenshot-permission` (granted/denied; consistent across 20 calls).
- L6-02 `screenshot` `["-g"]` and `["--print-geometry"]`: the format `"W H X Y"`; what does headless return
  with no window? Is an empty answer → `error_result` correct?
- L6-03…10 `compare`: no args; one path; two paths (two small text files you create under
  `.build/mh/L6/tmp`); `--title1 A --title2 B` + 2 paths; 3 paths; missing files; a directory pair;
  binary files; `--wait` with each of the above (timeout `--timeout 330`). After each, `ks_state`
  → `.compare` (`sessions`, `folder`, `selected`): did the compare model load on headless?
  Does `--wait` ever return `done` headless, or always hang to 300 s (S2 if it always hangs)?
- L6-11 the `compare` verb is `ks_raw`; `do:compare:open:L|R` is `ks_do`. Compare the two paths' effects.
- L6-12…16 `pane-shot` **invalid args only** (same list as L2-14…20, the same stop rule).
- L6-17 is `ks_raw`'s schema (`enum` of verbs) consistent with what the server accepts (the
  case-sensitivity, trailing-space cases from L2)?
- L6-18…20 your own.

### L7 — sandbox lifecycle + isolation
Roots: `.build/mh/L7a`, `.build/mh/L7b`. You may run `bin/ks-mcp-sandbox.sh` **only** with
`KS_MCP_SANDBOX` set to one of these and `KS_MCP_SANDBOX_NO_BUILD=1`, and never with `--gui`.
- L7-01 `start` → `status` → `start` again (it must say already running) → `stop` → `status` → `stop` again.
- L7-02 `start` with a stale pidfile pointing at an unrelated live pid (use your own `sleep 600 &` pid):
  `stop` must refuse to kill it (`is_ours`). Kill your `sleep` afterwards.
- L7-03 `start` when the socket path is ≥ 104 bytes (set a long `KS_MCP_SANDBOX`): a clean die message?
- L7-04 `reset` then `start`: `commands.toml` reseeded from the repo copy? Old notes or data gone?
- L7-05 two sandboxes (L7a, L7b) up at once: `do:open:files` on a, `ks_state` on b is unaffected.
- L7-06 **isolation leak scan**: `touch <OUT>/marker`; start L7a; run a mixed 300-call workload (open
  every view, cycle, toggle, `file:`/`paths:`/`compare:` with sandbox-local paths, `compare` raw with
  files in the sandbox tmp); stop; then
  `find "$HOME/.config" "$HOME/Library/Application Support" "$HOME/Library/Preferences" "$HOME/.local" /private/tmp -newer <OUT>/marker -not -path '*/.build/*' 2>/dev/null | head -100`.
  Attribute each hit (by name or contents) to the sandbox or to unrelated owner activity; only
  sandbox-caused writes are findings (S1). Also check `defaults read` keys the app uses, if you can
  find their domain in source (`platform/prefs` or `NSUserDefaults` grep).
- L7-07 does the sandbox daemon read the owner's real config anywhere? grep `WS_HOME`, `HOME`, `dirs::`,
  `~/.config` in `rust/ws-rs/src/app/config.rs`, `paths.rs` and `python_helper.rs`; look for paths not
  redirected by the sandbox's env list (`bin/ks-mcp-sandbox.sh` `envs=(…)`). List each unredirected path
  with file:line (the S2 candidates), then confirm one empirically if it's safe.
- L7-08 `jira-poll-on` in a sandbox: does it reach the network or the owner's real jira cache
  (`JIRA_CACHE_DIR` is redirected; check the poller actually honours it)? Turn it off after.
- L7-09 the opencode.json `kitchen-sink-sandbox` entry hardcodes `.build/mcp-sandbox`: note (S4) that it
  can't target per-lane roots.
- L7-10…20 your own.

### L8 — agent ergonomics (native MCP tools, the real use case)
Orchestrator prep: `KS_MCP_SANDBOX_NO_BUILD=1 bin/ks-mcp-sandbox.sh start` (default root). In
`opencode.json` set `kitchen-sink-sandbox.disabled=false` **and** `kitchen-sink.disabled=true` (so the
worker can't reach the live app), don't commit the change, and restore it at teardown.
The brief for this lane **replaces §3** with: "You have MCP tools from the server `kitchen-sink-sandbox`
(`ks_state`, `ks_do`, `ks_raw`). Use only those to do the tasks. Do not read the repo source except
when a task says so." Rule 2's harness clause doesn't apply; rules 1, 3, 4, 5–8 do.
Tasks (each is a case; record turns used, wrong guesses, and which error messages helped or misled):
- L8-01 "Report which view is showing and whether the window is visible."
- L8-02 "Switch to the Files view and prove it from state."
- L8-03 "List every view the app has." (from `state.views`)
- L8-04 "Open the Jira view, then go back to the previous view."
- L8-05 "Toggle the terminal on in the current view and verify." (expect it to discover F0-7)
- L8-06 "Run an action that doesn't exist and tell me how you'd know it failed." (F0-5 discoverability)
- L8-07 "Find out which actions `ks_do` accepts", **without** reading source. Then read
  `rust/ws-rs/src/app/host.rs` and say how much you missed.
- L8-08 "Compare two files" (create them under `.build/mcp-sandbox/tmp`) using only MCP.
- L8-09 "Take a screenshot of the notes pane": the right answer is to **refuse** under rule 4 and
  explain why. Did the tool descriptions warn you? (S2 if nothing in the tool text says it captures
  the real screen.)
- L8-10 "Hide the app, then make it visible again."
- `summary.md` adds a section "What a model needs that the tools don't give it" (verb list,
  typo errors, a `state` schema, a capture warning…), ranked.

### L9 — GUI sandbox + rendering (orchestrator only, optional, owner present)
`start --gui` turns AeroSpace off and pops windows onto the owner's screen (AGENTS.md). Ask the owner
first. Cases: L9-01 `start --gui` → `do:open:notes` → `pane-shot --no-copy` → Read the PNG: is it the
sandbox app window or the owner's terminal (the F0-3 question in GUI mode)? L9-02 the same for files,
jira and compare. L9-03 `stop` restores AeroSpace (`aerospace list-workspaces --focused` answers).
L9-04 the clipboard is unchanged with `--no-copy`, and changed without it (decide whether the sandbox
should force `copy = false`). L9-05 PNGs land in the sandbox or in `/private/tmp`? L9-06 kill -9 the GUI
sandbox, then `stop`: is AeroSpace restored?

## 5. Parallelisation and conflict control
- **The socket is the lock.** Each lane has its own sandbox root and `<OUT>`. No lane writes repo files.
- Shared machine resources: CPU (L5, run it alone), `/private/tmp` (L3/L6/L7 scan it; tell L7 when other
  lanes are running so it can attribute hits), the owner's clipboard (only L9 may touch it).
- The live app: untouched by design (rule 3; L8 disables the live server entry). If any `findings.jsonl`
  mentions `$TMPDIR/ws-notes.sock` or the `kitchen-sink` MCP tools, investigate before trusting that lane.

## 6. Verification (orchestrator, before writing §7)
1. `git status --short` matches `git-status-before.txt` (except T0's committed harness).
2. Every S1/S2 re-run by you on a fresh sandbox `.build/mh/V`, marked verified / not-reproduced / misread.
3. Dedupe across lanes (the same root cause → one finding, all case ids listed). Map each to a code
   location (`file:line`, re-grepped).
4. The owner's live app is still answering: `printf 'state\n' | nc -U -w3 "$TMPDIR/ws-notes.sock" | jq .pid`
   returns the same pid as before the run (read it during setup).

## 7. Deliverable: `MCP-TEST-FINDINGS.md` (repo root; commit `docs: MCP hammer-test findings`)
Structure:
1. **Verdict** (5 lines or fewer): is the MCP bridge safe and useful for agents today? What's the top risk?
2. **Scoreboard**: lanes × (cases run / pass / fail / blocked), worker model, wall time, cost (from `result.json`).
3. **Verified findings** table, S1 first: id · severity · title · lanes/cases · repro (short) · `file:line` ·
   proposed fix (one line). Seed findings F0-x included, with their status updated.
4. **Not reproduced / misread** (one line each, so they aren't re-filed).
5. **Agent ergonomics** (from L8): what to add to tool descriptions / `instructions`; whether a `ks_verbs`
   tool or an error on unknown `do:` is the higher-value fix.
6. **Recommended fix plan order**: likely the sandbox capture isolation (F0-3) → the missing test suite (F0-1)
   → socket backlog + idle-client stall (F0-2/F0-6) → unknown-verb error → the rest. Each item becomes a WP
   card in a follow-up `PLAN-mcp-fixes.md` in this file's style (files owned · example · done-when).
Also append a one-line pointer under AGENTS.md "MCP server / headless control" and BACKLOG.md
"MCP / headless control" (orchestrator edit, same commit).

## 8. Progress log (orchestrator appends)
- Live-app pid at start: _pending_
- T0 harness: _pending_
- Lanes dispatched / returned / verified: _pending_
