# Agent prompt: continue the Swift→Rust port (kitchen-sink) — Phase-4 gate, take 2

You are continuing an in-flight port. Everything is on disk; read the current
state before writing anything. Repo: `/Users/danielbaker/.config/kitchen-sink`
(the REAL repo — `git rev-parse --show-toplevel` first; `rule.md` forbids git
in backups). This supersedes `PROMPT-rust-port-phase4.md`.

## Read first (in order)
1. `PLAN-rust-port.md` — the plan and the **Progress** section, especially the
   last two "Wave notes" paragraphs (source of truth; updated at the end of the
   gate wave).
2. `AGENTS.md` (repo root) — repo map, rules, build/test commands.
   `AGENT_CONTEXT.md`/`CLAUDE.md` are legacy.
3. `rust/ws-rs/src/app/host.rs` — the controller + daemon UI bridge
   (`UiCommand`/`UiQueue`, `install_ui`, `present`, `show_palette`/`show_switcher`,
   `run_paths_cmd`, `toggle_drawer`, `route_key_event` with the Ctrl+B prefix and
   the vim-pane key routing, `poll_notes_vim`).
4. `rust/ws-rs/src/views/notes.rs` (`NotesSurface`: sidebar/editor/drawers/vim)
   and `rust/ws-rs/src/views/notes_vim.rs` (the nvim pane: spawn/args/env,
   socket + `.images.json` sidecar, RPC glue, `open`/`flush`/`poll_exit`).
5. `rust/ws-rs/src/ui/popup.rs` — `route_key` + `PrefixState` (pure) and
   `install_local_monitor`; `rust/ws-rs/src/views/switcher.rs` (the two palette
   panels) and `views/paths.rs` (the tool window).
6. `bin/ui-test.sh`, `bin/ui-test-vim.sh`, `bin/ui-test-focus.py`,
   `bin/rs-parity.py` — the gate harnesses (all now support `WS_BIN`).

## Where things stand (end of the gate wave)
- **591 Rust tests green** (586 ws-rs + 5 helpers); `cargo build` warning-free;
  `bin/build-rust-app.sh` builds the signed, isolated
  `.build/rust-app/kitchen-sink.app` (dev bundles get no Resources payload —
  run them with `WS_RS_ASSET_DIR=$PWD`).
- **`bin/rs-parity.py --strict` over a 30-action `do:` sequence: 0 missing
  keys, 0 nested mismatches** (the harness also diffs dotted contract keys:
  `palette`, `viewSwitcher.shown`, `terminalPanel.shown`, `paths.shown`,
  `views.notes.terminal`, `views.{notes,files,jira}.shown`).
- **`bin/ui-test.sh`: 78–79 passed, 0 failed, 2 skipped.**
  **`Tests/test_install.sh`: 55 passed, 0 failed** (needs `--dist` first).
  **`./ws test all`: green.**
- **`bin/ui-test-vim.sh`: 30 passed, 16 failed** — the nvim pane exists and
  works (typing, `:w`, `open:`, Cmd+V, `:q` relaunch, checktime poll verified
  live); failures are the image-paste feature, focus races during the run, and
  stale suite expectations (see Remaining).
- **`bin/ui-test-focus.py`: 19 passed, 8 failed** — AeroSpace rule vs the
  suite's floating expectation, and the compare view's `current`/`sections`
  state + session flow.
- **Everything is UNCOMMITTED** (the whole `rust/` tree is staged in the index;
  this wave's changes + `rust/ws-rs/src/views/notes_vim.rs` are unstaged/
  untracked). Do NOT commit.

## Running the app (owner-facing; also your dev loop)
Your Swift app holds `$TMPDIR/ws-notes.sock` when it runs, so the Rust binary
just forwards to it and exits. To run/test the Rust app without touching Swift:
```bash
D=$(mktemp -d)
TMPDIR="$D" WS_RS_ASSET_DIR="$PWD" \
  .build/rust-app/kitchen-sink.app/Contents/MacOS/kitchen-sink &     # palette
B=.build/rust-app/kitchen-sink.app/Contents/MacOS/kitchen-sink
TMPDIR="$D" "$B" notes            # shared window + nvim
TMPDIR="$D" "$B" "open:/tmp/x.md" # open a note (vim follows)
printf 'state\n' | nc -U "$D/ws-notes.sock" | jq
echo do:toggle-terminal | nc -U "$D/ws-notes.sock"
```
Kill the Rust daemon by path (`pkill -f "rust-app/kitchen-sink.app"`) or by the
pid from `lsof -U | grep "$D/ws-notes.sock"`. To run it as THE app: quit Swift
first, then `open -n .build/rust-app/kitchen-sink.app` (hotkeys via
`bin/kitchen_sink.sh` with `WS_BIN` set).

## Gate harness commands
```bash
cd rust && cargo test && cargo build                 # 591 green, warning-free
./bin/build-rust-app.sh [--force|--dist]
# parity (isolated TMPDIRs; NEVER drive do: against the owner's live socket):
E=$(mktemp -d); TMPDIR="$E" ./kitchen-sink.app/Contents/MacOS/kitchen-sink show &
D=$(mktemp -d); TMPDIR="$D" WS_RS_ASSET_DIR="$PWD" .build/rust-app/kitchen-sink.app/Contents/MacOS/kitchen-sink show &
python3 bin/rs-parity.py --swift-sock "$E/ws-notes.sock" --rust-sock "$D/ws-notes.sock" --strict \
  --actions "state,do:open:notes,do:open:files,do:cycle,do:home,do:back,do:hide,do:open:jira,do:toggle-terminal,do:toggle-terminal,do:term,do:term,do:switcher,do:switcher-hide,do:reset-size,do:open:confluence,do:open:ai,do:open:compare,do:open:notes,do:header-style:aurora,do:header-style:quiet,do:esc-hides:notes:on,do:esc-hides:notes:off,do:pane:h,do:paths:show,do:paths:select:1,do:paths:hide,do:tool:paths,do:tool-close:paths,do:open:notes"
# UI suites (isolated TMPDIR; run ONCE, they take over the app):
TMPDIR=$D WS_BIN="$PWD/.build/rust-app/kitchen-sink.app/Contents/MacOS/kitchen-sink" bin/ui-test.sh
TMPDIR=$D WS_BIN=... bin/ui-test-focus.py     # moves workspaces, restores them
TMPDIR=$D WS_BIN=... bin/ui-test-vim.sh       # daemon must already be up in $D with notes open
```

## Remaining work (priority order)
1. **vim suite leftovers** (each item = one area; parallelizable):
   - **pasteboard-image paste**: Cmd+V with an image on the clipboard must save
     `assets/img-<epoch>.png` next to the note and insert `![](assets/…)` at
     the cursor (`PopupWindow.vimPaste` + `saveImage`; `NSPasteboard` +
     `NSBitmapImageRep` PNG encode already exist in `views/screenshot.rs`).
   - **inline-image overlay** (optional for the suite; it only reads the
     `.images.json` sidecar that `vim/init.lua` writes): mirror
     `VimImageOverlay` if you want pixels.
   - **focus races**: several suite checks failed only because another app
     stole focus mid-run (Cmd+V/A/Z/C verified working live). Re-run once and
     triage the real failures.
   - **stale expectations**: the suite assumes `esc-close = 2` (shipped is 0 —
     Swift behaves the same) and `dd` yanking to the clipboard (init.lua
     deliberately black-holes normal-mode deletes). Either update the suite or
     leave and document — do NOT change app behavior for them.
2. **`ui-test-focus.py`**: implement the compare view's `current` state
   (`sections`, `cursorRow`, `scrollY`) + the `do:compare:open:<l>|<r>` session
   flow, and decide the AeroSpace question: the owner's `aerospace.toml`
   `on-window-detected` rule (bundle-id match) tiles + moves the shared window
   to ws 2, while the suite expects floating on the focused workspace. Check
   what the live Swift app does before touching either side.
3. **list-key execution** (`KeyAction::List*` still passes through) and the
   **full command palette** (workspaces + commands; today `show` uses a
   `ViewSwitcherPanel` stand-in — see PLAN).
4. **screenshot `do:` main-thread marshalling**: `do:screenshot:*` runs on the
   socket thread (`MainThreadMarker::new()` is None), so the real overlay/pin
   windows only work when the model path is driven on main. Mirror Swift's
   2 s main-thread semaphore for `do:` or a narrow screenshot-only dispatch.
5. **Cutover (only when the gate is green)**: flip `./ws`/`bin/build-app.sh` to
   cargo, retarget `bin/kitchen_sink.sh`/`INSTALL.sh`/preflight, migrate
   `notify/notify_poll.py` to the rust AX helpers (`--dist` still compiles the
   Swift ones), delete the Swift tree (keep the SwiftTerm shim), update
   `AGENT_CONTEXT.md`/`install.conf`.

## How to work (owner's directives)
- Spawn **cheap `general` subagents** (`opencode-go/deepseek-v4.1-flash`), one
  per file/area, in parallel where files are disjoint. Each agent owns a fixed
  list of files and must NOT edit anything else (`main.rs`, any `mod.rs`,
  `Cargo.toml`, `host.rs`, or another agent's module). Prefer to own
  `main.rs`/`host.rs`/`registry.rs`/`Cargo.toml` yourself.
- Every agent keeps `cargo test` green, runs `cargo build`, adds tests for pure
  logic; AppKit calls are `#[cfg(target_os = "macos")]`, guard
  `MainThreadMarker::new()`, never panic in ObjC callbacks.
- Keep the socket contract byte-compatible (`state` keys, `do:` grammar);
  Python is frozen; `commands.toml`'s format is frozen; config over code.
- Run the `deslop` skill on the diff and the `code-review` skill (two axes)
  before calling a wave done.
- Do NOT commit; leave changes uncommitted for review. Update the **Progress**
  section of `PLAN-rust-port.md` when you finish.

## Gotchas learned this wave
- **Two `kitchen-sink` processes can run at once** (Swift + Rust): `pgrep -x
  kitchen-sink`, `tell process "kitchen-sink"` and `pkill -f kitchen-sink` are
  all ambiguous. The suites are now pid-scoped (`lsof -U | grep <sock>`); keep
  it that way. Never `pkill -f kitchen-sink` — the owner's live app matches.
- **The nvim child `nvim --embed --listen …` is nvim's own server child** (nvim
  ≥0.10 TUI splits); do NOT add `--embed` to the launch args yourself.
- **Dev bundles have no Resources payload** (`bundle_resources` only runs for
  `--dist`) — run with `WS_RS_ASSET_DIR=$PWD` or `-u <missing init.lua>` makes
  nvim wedge in a "Press ENTER" prompt and RPC hangs.
- `[app] esc-close = 0` in the shipped config (Esc consumes without hiding);
  `vim/init.lua` maps normal-mode `d/c/x` to the black-hole register on
  purpose. Don't "fix" either.
- `bin/ui-test.sh` temporarily sets `vim-mode = false`; the vim suite needs it
  true (shipped) and the daemon already running with notes open before it
  starts. It moves the owner's workspaces and restores them.
- Screen Recording is TCC-blocked for agent shells; verify visually via System
  Events/AX or the app's own screenshot path.
- `cargo test` does not rebuild the plain binary — `cargo build -p ws-rs`
  before launching `target/debug/kitchen-sink`.
- `config_text::tri` calls the Python helper — don't use it for cold-start
  parsing (`host.rs` has `tri_pure`).
- The `borders` daemon perturbs window-chrome checks (`brew services stop
  borders`, restart after).
- A background app (e.g. Firefox) can steal focus mid-suite and make guarded
  helpers abort; re-run and triage before believing failures.

## Definition of done for a wave
`cd rust && cargo test` green, `cargo build` warning-free, the Rust `.app`
visibly shows the shared window and answers `state`/`do:` identically to the
schema in `PLAN-rust-port.md`'s Progress, `rs-parity --strict` is 0/0, and —
for the gate wave — `ui-test.sh`, `ui-test-vim.sh`, `ui-test-focus.py`, every
`run-tests.sh` suite and `test_install.sh` pass against the Rust binary.
