# Agent prompt: continue the Swift→Rust port (kitchen-sink) — Phase-4 gate & last seams

You are continuing an in-flight port. Everything is on disk; read the current
state before writing anything. Repo: `/Users/danielbaker/.config/kitchen-sink`
(the REAL repo — `git rev-parse --show-toplevel` first; `rule.md` forbids git
in backups). The previous session's history is gone — the facts below are the
handoff. This supersedes `PROMPT-rust-port-next.md`.

## Read first (in order)
1. `PLAN-rust-port.md` — the plan and the **Progress** section (source of
   truth; updated at the end of the last wave, including both "Wave notes"
   paragraphs).
2. `AGENTS.md` (repo root) — repo map, rules, build/test commands.
   `AGENT_CONTEXT.md`/`CLAUDE.md` are legacy.
3. `rust/ws-rs/src/app/host.rs` — the controller + daemon UI bridge
   (`UiCommand`/`UiQueue`, `install_ui`, `nav_clicked`, `build_palette`,
   `content_for`, and the macOS `daemon_ui` module with `install_daemon_ui` /
   `WSDaemonUi` / `route_key_event` / `perform_edit_op`).
4. `rust/ws-rs/src/ui/popup.rs` — the pure `route_key` plus the real
   `install_local_monitor` / `key_input_from_event` / `MonitorHandle`.
5. `rust/ws-rs/src/main.rs` — daemon startup (helper config, menu/status item,
   UI bridge install, default view, the `prose` CLI path).

## Where things stand (end of the last wave)
- **549 Rust tests green** (544 ws-rs + 5 helpers); `cargo build`
  warning-free; `bin/build-rust-app.sh --force` builds a signed
  `.build/rust-app/kitchen-sink.app` with `__TEXT,__info_plist` embedded.
- The daemon **shows the shared window** on start / launch message /
  `do:open:*`: `SlotHostWindow` + `PopupChrome` header + view-switcher
  buttons + ✕/Cmd+W close + AppKit main menu + status item, `.regular`
  activation policy. `DEFAULT_VIEW = SlotView::Notes`.
- **The keyboard spine is live**: `ui/popup.rs::install_local_monitor` is a
  real local `NSEvent` keyDown monitor; the daemon UI installs it, runs the
  pure `route_key` with the current view's `escHideCount`, keeps the 0.6 s
  Esc streak (hides on close), forwards Ctrl edit ops down the responder
  chain, and passes everything else through. Verified live on the signed
  bundle: Cmd+W hides via the menu; one Esc with `do:esc-hides:notes:on`
  hides (the shipped `[app] esc-close = 0` means Esc is consumed without
  hiding — matches Swift).
- **Embeddable content** wired in `host.rs::content_for`: notes
  (sidebar + editor), files, jira, compare, confluence (search strip /
  results / live `wsconf://` preview) and ai (rules/input pane + live preview
  web view). Confluence/AI build with default/empty models — no helper or
  network on the build path.
- Also landed: the screenshot overlay key monitor (`views/screenshot.rs` →
  `ShotSession::handle_key` / `PinPanel`), `ai_format`
  `RichText::rtf`/`copy` (real AppKit pasteboard), and the notes
  `ProseProcess::launch` pop-out control.
- `bin/rs-parity.py --actions state` vs the live Swift daemon: **0 missing
  keys**; the only scalar mismatch is the by-design fresh-start
  `view`/`visible` (the Rust daemon presents the default view; Swift stays
  hidden until asked).
- **Everything is UNCOMMITTED.** The whole `rust/` tree is staged in the
  index; `rust/ws-rs/src/app/host.rs`, `rust/ws-rs/src/views/confluence.rs`
  and `PLAN-rust-port.md` additionally carry unstaged updates. Do NOT commit.

## Commands
```bash
cd rust && cargo test                 # 549 green
cd rust && cargo build                # must stay warning-free
./bin/build-rust-app.sh --force       # -> .build/rust-app/kitchen-sink.app (signed)
# dev daemon (isolated socket; point the python helper at the checkout's pylib):
D=$(mktemp -d); TMPDIR="$D" WS_RS_ASSET_DIR="$PWD" \
  .build/rust-app/kitchen-sink.app/Contents/MacOS/kitchen-sink &
printf 'state\n' | nc -U "$D/ws-notes.sock"
python3 bin/rs-parity.py --swift-sock "$TMPDIR/ws-notes.sock" \
  --rust-sock "$D/ws-notes.sock" --actions state
```
- **NEVER run `bin/rs-parity.py`'s default `do:` actions against the real
  `$TMPDIR/ws-notes.sock` while the owner's Swift app is up** — it changes
  their UI. `--actions state` only, or an isolated `TMPDIR` for the Rust
  daemon.
- **Do not `pkill -f kitchen-sink`** — the owner's live Swift app matches.
  Kill the Rust one by path: `pkill -f "rust-app/kitchen-sink.app"` or
  `pkill -f "target/debug/kitchen-sink"`.
- `cargo test` does **not** rebuild the plain binary — run
  `cargo build -p ws-rs` before launching `target/debug/kitchen-sink`.
- `bin/ui-test.sh` hardcodes
  `BIN="$ROOT/../kitchen-sink.app/Contents/MacOS/kitchen-sink"` (the Swift
  bundle). Running it against the Rust binary needs a `WS_BIN`-style env
  override (small edit to the script) or a temporary swap — decide this
  before starting the gate.

## Remaining work

### A. Last seams before the gate (each item = one file/area; parallelizable)
1. **Paths tool window** — `views/paths.rs` `PathsWindow::build` /
   `content_view` exist but nothing shows it. Mirror Swift `showPaths`
   (`kitchen_sink.swift:5706`): a standalone tool window
   (`pathsWindow = PathsWindow(...)`, `show()`, `reclaimToolKey`), opened by
   the `[paths]` command / `do:paths:*`. The socket `do:paths:*` grammar is
   frozen; decide the Rust home (host.rs + registry).
2. **Deeper in-window key routing** — the shared window's monitor consumes
   only Esc + edit ops today; the Swift `keyInterceptor` (Ctrl+B prefix,
   Ctrl+H/J/K/L pane moves, VimKeys) and list-key execution still pass
   through. Wire the `SharedWindow.prefixKey` equivalent (popup.rs +
   host.rs), keeping the documented `handleKey` order and rule 1 (edit
   shortcuts always forwarded).
3. **Notes nvim / vim-mode** — `views/notes.rs` has **no nvim wiring**
   (grep "nvim" = 0 hits). Swift's notes pane embeds nvim when
   `[notes] vim-mode = true`, and `bin/ui-test-vim.sh` depends on it
   (socket `$HOME/.cache/kitchen-sink/nvim-notes-<pid>.sock`, remote-expr
   checks). This is the biggest remaining surface gap; `engines/nvim_rpc.rs`
   (real msgpack-RPC) already exists.
4. **Window titles / chrome parity** — the daemon's `SlotHostWindow` never
   sets a window title; `bin/ui-test.sh` finds windows via System Events
   window title (`notes`, `files`, ...). Set titles to mirror Swift
   (per-view name) in `daemon_ui`.
5. **Small fidelity** — the AI surface's Copy should go through
   `RichText::copy` (HTML + RTF + text) now that it exists; the surface's Run
   only calls `begin_run` (no `fm` spawn); confluence/AI surfaces use
   empty/default models (live search wiring is future); screenshot pins are
   models (no real `NSPanel`) and the finish/teardown render chain is
   unwired.

### B. Phase 4 gate (do NOT start the cutover until this passes)
6. `bin/rs-parity.py --strict` over the full `do:` sequence (isolated
   `TMPDIR` for the Rust daemon).
7. `bin/ui-test.sh`, `bin/ui-test-vim.sh`, `bin/ui-test-focus.py`, every
   `run-tests.sh` suite, and `test_install.sh` against the Rust binary;
   zero parity diffs, green suites. Known fixture gap: `ui-test.sh`'s
   `/tmp/ws-test` section has no creator in the repo.
8. Run the `code-review` + `deslop` skills over the branch.

### C. Cutover
9. Flip `./ws` / `bin/build-app.sh` to the cargo path; retarget
   `bin/kitchen_sink.sh`, `INSTALL.sh`, preflight.
10. Migrate `notify/notify_poll.py` to the Rust AX helpers
    (`rust/ws-helpers`; `--dist` still compiles the Swift helpers today).
11. Delete the Swift tree (keep the SwiftTerm shim), update
    `AGENT_CONTEXT.md`/`CLAUDE.md`/`install.conf`.

## How to work (the owner's directives)
- Spawn **cheap `general` subagents** (`opencode-go/deepseek-v4.1-flash`), one
  per file/area, in parallel where files are disjoint. Each agent owns a
  fixed list of files and must NOT edit anything else (`main.rs`, any
  `mod.rs`, `Cargo.toml`, `host.rs`, or another agent's module). Prefer to
  own `main.rs`/`host.rs`/`registry.rs`/`Cargo.toml` yourself.
- Every agent must keep `cargo test` green, run `cargo build`, and add tests
  for pure logic. AppKit calls are `#[cfg(target_os = "macos")]`; guard
  `MainThreadMarker::new()` and never panic in ObjC callbacks.
- Keep the socket contract byte-compatible (`state` keys, `do:` grammar);
  Python is frozen; `commands.toml`'s format is frozen.
- Config over code; user-facing strings live in `commands.toml`.
- Do NOT commit; leave changes uncommitted for review. Update the
  **Progress** section of `PLAN-rust-port.md` when you finish.

## Gotchas learned last wave
- **`pgrep -f kitchen-sink` matches your own shell** (the command line
  contains the pattern). Get the daemon PID via `lsof -U | grep <sock>`;
  target it in System Events with `first process whose unix id is <pid>`.
- **Screen Recording is TCC-blocked for agent shells** — `screencapture`
  returns wallpaper only. Verify visually via System Events (window
  bounds/AX) or the app's own screenshot path; ask the owner for a pixel
  check when it matters.
- `[app] esc-close = 0` in the shipped config: Esc is consumed but does not
  hide unless `do:esc-hides:<view>:on` (or a section `esc-close`) is set —
  matches Swift.
- Fresh-start `state.view`/`visible` differ from the hidden Swift daemon by
  design (the Rust daemon presents the default view); `rs-parity` reports
  only this.
- One one-off test flake was observed (543 passed / 1 failed) and never
  reproduced in 8 consecutive runs — capture the failing test name if it
  returns.
- The off-screen 500×500 `TUINSWindow` per `NSTextView` is AppKit's
  text-input services window — benign, not counted in `state.windows`.
- `config_text::tri` calls the Python helper — do not use it for cold-start
  parsing (`host.rs` has `tri_pure` for raw-text scans).
- The daemon UI is a 30 ms `NSTimer` draining the `UiCommand` queue on the
  main thread; tests stay headless (no `install_ui` → no UI commands).
- The `borders` daemon perturbs window-chrome checks — `brew services stop
  borders` for deterministic UI/chrome assertions, restart after.

## Definition of done for a wave
`cd rust && cargo test` green, `cargo build` warning-free, the Rust `.app`
visibly shows the shared window and answers `state`/`do:` identically to the
schema in `PLAN-rust-port.md`'s Progress, and — for the gate wave — the UI
suites pass against the Rust binary with zero parity diffs.
