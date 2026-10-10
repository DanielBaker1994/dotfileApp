# Agent prompt: finish the Swift→Rust port, code-first (UI tests deferred to one final batch)

You are continuing the kitchen-sink Swift→Rust port. Everything is on disk and
committed; read the current state before writing anything. Repo:
`/Users/danielbaker/.config/kitchen-sink` (the REAL repo —
`git rev-parse --show-toplevel` first; `rule.md` forbids git in backups).

**This supersedes `PROMPT-rust-port-phase4b.md` and the gate-wave iteration
style in `PLAN-rust-port.md`.**

## Owner directive (non-negotiable for this plan)

1. **Finish the code port completely first.**
2. **Do NOT run the UI suites** (`bin/ui-test.sh`, `bin/ui-test-vim.sh`,
   `bin/ui-test-focus.py`, `Tests/test_install.sh`) until the very end — they
   are slow, flaky, and take over the owner's desktop. Iterate only with
   `cargo test` and isolated-daemon socket probes (`state` / `do:`).
3. **All UI suites run once, as a single end batch**, after the code work is
   done. Fix suite expectations only then (some are known-stale; see below).
4. **Parallelize with cheap `general` subagents, model
   `opencode-go/deepseek-v4.1-flash`**, one per file/area, disjoint files
   only. Keep `main.rs`, `app/host.rs`, `app/registry.rs`, `Cargo.toml` for
   yourself. Every agent keeps `cd rust && cargo test` green and
   `cargo build` warning-free.
5. Do not commit mid-wave; commit only when the owner asks (the previous
   session's work was committed and pushed at handoff).

## Read first (in order)

1. `AGENTS.md` (repo root) — repo map, rules, build/test commands. The
   authoritative agent context; `AGENT_CONTEXT.md`/`CLAUDE.md` are legacy.
2. `PLAN-rust-port.md` — phases + the **Progress** section (state as of the
   last commit) and the Phase-4 cutover checklist.
3. `rust/ws-rs/src/app/host.rs` — the controller + daemon UI bridge
   (`UiCommand`/`UiQueue`/`MainQueue`, `install_ui`, `install_main_queue`,
   `run_on_main`, `present`, `do_host_action` incl. the `compare:` /
   `screenshot:` interception, `route_key_event`, `perform_vim_op`,
   `poll_notes_vim`).
4. `rust/ws-rs/src/views/compare.rs` (`CompareWindowModel`: `test_state`,
   `test_do`, `confirm` sheet state, `set_pasted` dirty edits, folder page)
   and `rust/ws-rs/src/views/notes_vim.rs` (`VimPane::paste`,
   `save_pasteboard_image`, `paste_expr`).
5. `bin/rs-parity.py` and the probe recipe below.

## Where things stand at handoff

- **598 ws-rs tests + 5 helper tests green**; `cargo build` warning-free; the
  signed isolated `.build/rust-app/kitchen-sink.app` builds (dev bundles have
  no Resources payload — run with `WS_RS_ASSET_DIR=$PWD`).
- Every Swift file has a Rust module; the shared window shows and answers the
  full `state`/`do:` contract. `bin/rs-parity.py --strict` was 0/0 on a
  30-action sequence before this session's changes; `bin/ui-test.sh` was
  79/79.
- Landed this session (uncommitted→committed at handoff):
  - `do:compare:open[:sub]:L|R` presents compare / pushes compareText;
    `compare:back` returns; Esc in compareText steps back (never hides).
  - Compare close-confirmation sheet (`state.compare.sheet`,
    `compare:close-session[:force]`, `compare:sheet-cancel`); `compare:edit`
    now marks the side dirty via a real undo edit; folder `open`/`drop`/
    `summary` gaps filled.
  - Vim image paste: Cmd+V with an image saves `assets/img-<epoch>.png` next
    to the note and inserts `![](assets/…)` (`nvim_paste` first, bracketed
    paste fallback).
  - `:q` relaunch now refocuses the new nvim pane (Swift `TerminalAutoRestart`).
  - `do:screenshot:*` runs on the main thread (`MainQueue` + 2 s budget,
    Swift's `testQuery` main hop).
  - `bin/ui-test-vim.sh` updated for the shipped `esc-close = 0` and
    init.lua's black-hole normal-mode deletes.
- **Known suite state (do NOT chase until the final batch):**
  - `bin/ui-test-vim.sh`: 44/46 on its first clean run; the two leftovers are
    the relaunch-focus fix (landed after that run) and the suite's Esc section
    — **the suite sends `esc-hides:notes:on` WITHOUT the `do:` prefix**, so the
    action is silently ignored; fix the suite to `do:esc-hides:…` in the final
    batch. A later run cascaded into a focus loss — treat as flake.
  - `bin/ui-test-focus.py`: the compare `do:` flows were the bulk of the Rust
    failures and are now implemented (not yet re-run). The AeroSpace
    expectations ("floating, on the focused workspace") are **stale vs the
    owner's `config/aerospace/aerospace.toml`**: the
    `on-window-detected` rule tiles + moves the shared window to workspace 2,
    and the live **Swift app behaves identically** (verified) — update the
    suite's expectations in the final batch, do not change app behavior.
  - `bin/ui-test.sh` was green; re-verify once in the final batch.

## Remaining code work (priority order)

1. **List-key execution**: `KeyAction::List*` (ListMove/ListAccept/
   ListToggleSelection/ListPage) still falls through in
   `daemon_ui::route_key_event`. Wire it to the focused list (notes sidebar
   first; files/jira lists as they exist) and expose what tests need in
   `state` (selection/rows already exist per view).
2. **Full command palette** (`show` / Hyper+S): today `show` uses the
   `ViewSwitcherPanel` stand-in; Swift's palette lists workspaces + commands
   (`paletteCommands`) with keyboard navigation and execution. Port it
   (`views/switcher.rs` + `host.rs`), keeping `state.palette` /
   `paletteCommands` byte-compatible.
3. **Any remaining `todo!()`/seam items** in `PLAN-rust-port.md` that are
   pure code (e.g. inline-image overlay `VimImageOverlay` pixels — optional;
   screenshot overlay drawing is landed).
4. **Sweep for stubs**: `grep -rn "not modelled yet\|todo!\|unimplemented"`
   under `rust/ws-rs/src` and close anything the cutover needs.
5. **Final batch (only after 1–4 are done)**: run, once each —
   `bin/rs-parity.py --strict` (isolated TMPDIRs), `bin/ui-test.sh`,
   `bin/ui-test-vim.sh`, `bin/ui-test-focus.py`, `./ws test all`,
   `Tests/test_install.sh` (needs `--dist` first) — fix suite expectations
   where they encode stale defaults (documented above), fix Rust bugs
   otherwise. Then the Phase-4 cutover per `PLAN-rust-port.md`: flip
   `./ws`/`bin/build-app.sh` to cargo, retarget `bin/kitchen_sink.sh` /
   `INSTALL.sh` / preflight, migrate `notify/notify_poll.py` to the Rust AX
   helpers (`--dist` still compiles the Swift ones), delete the Swift tree
   (keep the SwiftTerm shim), update `AGENT_CONTEXT.md`/`install.conf`.

## Fast dev loop (no UI suites)

```bash
cd rust && cargo test && cargo build          # the whole loop; ~10 s
./bin/build-rust-app.sh [--force|--dist]      # only when you need the .app

# Isolated daemon (never touches the owner's live app):
D=$(mktemp -d)
TMPDIR="$D" WS_RS_ASSET_DIR="$PWD" \
  .build/rust-app/kitchen-sink.app/Contents/MacOS/kitchen-sink notes &
B=.build/rust-app/kitchen-sink.app/Contents/MacOS/kitchen-sink
TMPDIR="$D" "$B" "open:/tmp/x.md"             # launch messages
printf 'state\n' | nc -U "$D/ws-notes.sock" | jq
printf 'do:toggle-terminal\n' | nc -U "$D/ws-notes.sock"    # NOTE the do: prefix
# Kill it by pid from lsof -U | grep "$D/ws-notes.sock" (never pkill -f kitchen-sink)
```

`state`/`do:` probes + `cargo test` are the entire verification loop until the
final batch. When you need a real window check, use an isolated TMPDIR and
put the owner's app back exactly as you found it.

## Gotchas learned (read these)

- **`do:` verbs need the `do:` prefix** — `esc-hides:…` sent raw is a launch
  message and is ignored (this exact bug wasted a whole suite run).
- Two `kitchen-sink` processes can run at once (Swift + Rust): pid-scope
  everything (`lsof -U | grep <sock>`); never `pkill -f kitchen-sink`.
- Dev bundles have no Resources payload — `WS_RS_ASSET_DIR=$PWD` or nvim
  wedges on a "Press ENTER" prompt.
- `[app] esc-close = 0` shipped (Esc consumes without hiding); `vim/init.lua`
  black-holes normal-mode `d/c/x` on purpose. Do not "fix" either.
- The nvim child `nvim --embed --listen …` is nvim's own server child — do
  not add `--embed` to launch args.
- `cargo test` does not rebuild the plain binary — `cargo build -p ws-rs`
  before launching `target/debug/kitchen-sink`.
- `config_text::tri` calls the Python helper — don't use it for cold-start
  parsing (`host.rs` has `tri_pure`).
- Screen Recording is TCC-blocked for agent shells; `borders` perturbs chrome
  checks (`brew services stop borders`, restart after).
- Python is frozen; `commands.toml`'s format is frozen; config over code;
  SwiftTerm is fetched (never edit/rebuild; changes go in `patches/`).
- `PYTHONDONTWRITEBYTECODE=1` everywhere (`__pycache__` breaks the signed
  bundle).
- Swift is authoritative until the Phase-4 cutover — when in doubt, read the
  Swift source and mirror it.

## Definition of done for this plan

All code seams closed (`cargo test` green, `cargo build` warning-free, the
Rust `.app` answers `state`/`do:` identically to the Swift schema), then the
single final batch: `rs-parity --strict` 0/0, `ui-test.sh`, `ui-test-vim.sh`,
`ui-test-focus.py`, every `run-tests.sh` suite, and `test_install.sh` pass
against the Rust binary — then the Phase-4 cutover.
