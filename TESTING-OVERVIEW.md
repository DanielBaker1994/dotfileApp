# What the DeepSeek testing validates, and how it knows

Every check needs an **oracle**: something that decides pass/fail without anyone's opinion. DeepSeek
runs the cases and reports differences from the oracle. Claude re-runs anything serious, and looks at
screenshots that a check flags.

## A. Done so far: the control plumbing (waves 1–3)

| Lane / suite | What it validates | How it knows (oracle) | Found real bugs? |
|---|---|---|---|
| L1 protocol | MCP JSON-RPC handshake, ids, malformed and huge messages | JSON-RPC spec: valid JSON back, right error codes, no crash | yes (bridge) |
| L2 arguments | bad/missing tool args; read-only mode blocks writes | an error is returned; read-only refuses mutating calls | yes (bridge) |
| L3 verbs | every `do:` verb does something or errors | now replaced by `Tests/mcp_verbs.json` (below) | yes (socket) |
| L4 state invariants | random sequences of verbs keep state consistent | invariant checker (e.g. only one view current, `visible` agrees with `windows`) | yes (socket) |
| L5 concurrency | many, slow, abandoned and killed clients | no crash, no leaked fd/thread (`lsof`), daemon still answers | yes (fd leak) |
| L6 raw verbs | screenshot/compare/pane-shot safe forms | reply format; headless refuses capture | yes |
| L7 sandbox | sandbox isolation and start/stop | never touches the live app's HOME, socket or pid | yes (isolation) |
| L8 agent ergonomics | can an agent use the tools to do tasks | turns used, wrong guesses (found the 120-char description cut) | yes (descriptions) |
| `Tests/test_ks_mcp.py` (30) | bridge behaviour | fixed expectations against a fake daemon | regression lock |
| `Tests/test_mcp_verbs.py` (38 verbs) | every `do:` verb on a real headless daemon | table: expected state keys and error text per verb, plus a coverage check | found 4 app-level oddities |
| GUI phase A (13 cases) | views open, back/hide/show, compare --wait in the real windowed app | `state` frames/windows; timing of the `compare --wait` reply | all pass |

**Limit:** these prove the plumbing and the state model, **not the features**. That's why they found
almost no bugs in the app itself.

## B. Next: the app itself (re-targeted, waiting on the DeepSeek quota)

Tools being built: `bin/ks-input` sends real keys, text and mouse events, and before **every** event
it refuses unless the sandbox app is frontmost. `Tests/app_flows.json` holds 30–45 feature flows.

| Lane | What it validates | How it knows (oracle) |
|---|---|---|
| Feature flows | each user feature works when driven by real keys and mouse: notes (create, edit, save, find, grep, rename), files, compare (folders and text, next/prev, filters, swap), view switcher, Cmd+K, Cmd+/, config + reload, output, drawers, pane/vim keys, Esc, tools, Jira offline, board | **disk** (typed note text is in the file), **independent recompute** (`ls` for the files view, Python `difflib` for compare counts), **state** after each step, compared with what the docs or code promise (every expectation cites `file:line` in PRD/AGENTS/code) |
| Edge cases per feature | empty input, very long text, unicode, rapid repeats | same oracles + crash/hang/log |
| Crash / hang / log (every run) | the app never panics, freezes or logs errors | `panic` or `ERROR` in the sandbox `daemon.log`; socket not answering within 2 s; process gone |
| Random input | random real key and mouse sequences across views | crash / hang / log only (catches real crashes cheaply) |
| Visual checkpoints | what the user would see after a step (blank, clipped, overlapping, wrong view) | screenshot per checkpoint → vision-model triage → Claude reads only the flagged PNGs |
| Bug fixes in flight | `cycle-back`, `jira-poll-on` headless, `compare:open` on missing paths, `tool-close:`/`key:` errors | Rust unit tests + verb-table rows |

**Result format:** a list of app bugs, each with repro steps, the failed oracle and the evidence (log
line, file diff, screenshot). Claude re-runs every serious one before it counts.
