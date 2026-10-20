# PLAN — usability hammer (soak + cross-feature), started 2026-10-20

Goal: find REAL APP BUGS (not bridge plumbing): features that work then stop mid-session
(intellisense/completion), and surfaces that disagree (prose inline render vs the pop-out window).
Parity with Swift is not a goal. Prior run: RUN-2026-10-13.md, cases: CASES.md.

## Constraints (from AGENTS.md + owner memory)
- Never `./ws build`, never `pkill` the live app (pid of the owner's live daemon). Sandbox only.
- One GUI at a time: only ONE agent may drive keyboard/mouse. GUI lanes run serially.
  Headless lanes use a SECOND sandbox root (`KS_MCP_SANDBOX=.build/mcp-sandbox-hl`) and run in parallel.
- Real input (`ks-input key a b c`, not `type`); activate + check exit code before each input.
  Cmd can stick: after every GUI lane run the modifier-release + flags check from ks-input-pitfalls.
- Stop after 2 misses on one UI step; reason from code, don't loop rebuild/click.
- Oracles first: socket `state`, daemon.log (`panicked`, `already borrowed`), new .ips files,
  disk ground truth. Screenshots (`pane-shot`, Read the PNG) only for render judgement.
- Commit on main only; no feature branches. Agents do NOT edit app source; they report.
- Known open (don't re-report, avoid): AI tab crash (host.rs ai_surface RefCell), Cmd+/ no-op,
  Window menu empty. Lanes must not click the AI tab.

## Lanes
| id | model | mode | what |
|----|-------|------|------|
| G1 | sonnet | GUI serial | Intellisense/completion soak: N>=40 cycles of type trigger -> popup appears? -> accept/dismiss -> perturb (switch view, open/close drawer, palette, resize, blur/refocus). Record first failing iteration + action trail. |
| G2 | sonnet | GUI serial | Prose: inline render vs pop-out. Notes with image/diagram/table/code: open pop-out, compare, close/reopen, theme switch, right-click Copy PDF Path in both (see PLAN-prose-popout.md H1-H4), `pgrep` for the prose child. |
| G3 | sonnet | GUI serial | Cross-surface matrix: views x surfaces (palette, drawer, pop-out, sheets): focus, key still works, process alive. |
| H1 | haiku | headless parallel | State-transition soak over socket: random `do:open:*` sequences, 300+ steps, invariants (one focused view, frames sane, daemon answers, no panic in log). |
| H2 | haiku | headless parallel | Notes ops against disk truth: create/rename/delete/search/open via `do:`; compare `ls`/file bytes vs state. |
| H3 | haiku | headless parallel | Log/crash watcher + triage: tail both sandbox logs and DiagnosticReports during other lanes; summarize. |

## Output
Each lane writes `Tests/usability/hammer-2026-10-20/<lane>.md`: per finding `ID · severity S1-S4 ·
repeat N/N · action trail · expected vs actual · likely file`. Orchestrator merges into
`Tests/usability/RUN-2026-10-20.md` and commits (docs only).

## Order
1. Start headless sandbox (hl root), launch H1-H3 in background.
2. Start GUI sandbox; run G1 -> G2 -> G3 serially; stop sandbox (re-enables AeroSpace) after each.
3. Merge, rank, report. Screen must stay unlocked (`caffeinate -d` for the duration).
