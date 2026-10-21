# AGENTS.md — kitchen-sink agent context

Current, authoritative agent context for this repo. Docs drift; **the code is
truth**. Grep a symbol, then read ~60 lines around it. `rust/ws-rs/src/app/host.rs`
is ~4.5k lines — never read it whole. `AGENT_CONTEXT.md` (`CLAUDE.md` symlink) is
a pointer here; its old deep-detail text is in git history.

## What this is (overview)

kitchen-sink is a single-instance macOS app — **Rust + AppKit via objc2**
(`rust/`, cargo workspace) — that runs as a hotkey-driven daemon: AeroSpace / CLI
launches talk to it over a Unix socket (`$TMPDIR/ws-notes.sock`, lock file beside
it), and one **shared window** hosts the views — notes, files, jira, confluence,
AI, compare — plus tool panels (screenshot, paths, terminal, prettyprint,
filefast, health checks, view switcher / command palette). It is a *regular* app
(`NSApplicationActivationPolicy::Regular`: menu bar + status item). Python
(`pylib/` plus `jira/`, `confluence/`, `notify/`, `settings_hub/`) is the "cold
path": the app talks to it over a persistent JSON-lines helper protocol.

History: the app was Swift until the Phase-4 cutover (Oct 2026); the Rust
port mirrors the old Swift files module for module, and Swift type names
survive in comments (`SwitcherController`,
`PopupWindow.handleKey`, `testQuery`) — use them to find the Rust counterpart.
The only Swift left is the SwiftTerm shim (`rust/swiftterm-shim`).

## Non-negotiable rules (condensed from `rule.md`)

1. **Edit shortcuts work in every text input.** Cmd+C/V/X/A/Z plus Ctrl+C/V;
   keys are routed explicitly (the key router `route_key_event` /
   `ui::popup::route_key` must forward edit shortcuts to the focused field
   editor; sheets route paste to their field editor and set
   `initialFirstResponder`).
2. **Right-click menus** in file browsers / the terminal offer: copy file,
   copy absolute path, open in notes, reveal in the app's own Files view.
3. **Config over code.** User-facing strings, sizes, and paths live in
   `commands.toml`; prefer a config change over a hard-coded one.
4. **Git only in this repo** (`/Users/danielbaker/.config/kitchen-sink`).
   Never stage/commit/change `.git` in a `.bak` copy, backup, or duplicate.
   Verify with `git rev-parse --show-toplevel` before git commands.
5. **SwiftTerm is fetched, not committed** (`../SwiftTerm`, pinned by
   `install.conf`, applied `patches/swiftterm-cellstorage-cache.patch`).
   Never edit or rebuild it unless explicitly told; carry changes as a patch.
   A missing checkout is fine — the build fetches it.
6. **Transient overlays own the keyboard.** Popovers/pickers take key focus on
   open and Esc closes only them, never the window under them.
7. **Python stays stable.** Keep the helper protocol and `pylib/` behavior
   byte-compatible; change Python only with a reason (the app depends on it).

## Build / run / test

```bash
./ws                        # interactive menu; ./ws help lists everything
./ws build                  # cargo build + sign + relaunch (exit 0 = OK)
./ws build --force          # rebuild even if up to date
./ws build --build-only     # build, don't relaunch (WS_BUILD_ONLY=1)
./ws clean [--all]          # stop the app + wipe objects (force a fresh build)
./ws dmg [--no-notarize]    # distributable .dmg -> .build/dist
./ws test [SUITE|all|rust|ui|vim|focus|install]
./ws doctor [--fix]         # full-stack health check (jira/jira-doctor.sh)
./ws check [--json]         # preflight this Mac (bin/preflight.sh)
./ws permissions grant|fix  # TCC permissions
./ws links [check|fix]      # ~/.config symlinks
./ws home app|repo|stack|status
./ws fake confluence|jira [start|stop|status]
./ws monitors main|inverse|toggle|status
```

**Build — `bin/build-app.sh`** (`./ws build` wraps it, ensures the signing
identity, then relaunches via `bin/kitchen_sink.sh window`):
`cargo build --release -p ws-rs -p ws-helpers` in `rust/` with
`CARGO_TARGET_DIR=.build/rust-target`; output is the repo-root
`kitchen-sink.app` (the path AeroSpace, `kitchen_sink.sh` and the TCC grants
use). Before cargo it runs `bin/ensure-swiftterm.sh` and compiles the pinned
SwiftTerm into `.build/SwiftTerm/libSwiftTerm.a` **only when that checkout
changed** (the shim crate's `build.rs` links it). `--dist` →
`.build/dist/kitchen-sink.app` with resources per `install.conf` `RESOURCE_*`,
a rewritten `commands.default.toml` and the AX helpers in
`Resources/helpers-bin` (`HELPER_BINS`). Signed with the self-signed `SIGN_ID`
("kitchen-sink codesign") for stable TCC grants. **A non-dist build `pkill`s
the running daemon before swapping the binary.** `--stale` exits 0 when a
rebuild is needed (`kitchen_sink.sh` uses it for build-if-stale). `ws clean`
stops the daemon and removes `.build/rust-target` (plus `rust/target` so the
next build is a full cargo rebuild); `--all` also wipes `.build/SwiftTerm`,
`.build/.termbuilt`, `.build/dist` and the repo-root app bundle (a wiped
bundle is recreated by `mkdir -p` + a full build). Toolchain:
cargo (brew `rust` or rustup) + Xcode CLT (`swiftc`, for SwiftTerm + the shim).
Don't export `MACOSX_DEPLOYMENT_TARGET=26.7` (corrupt proc-macro dylibs).

**Tests.** `./ws test SUITE` → `bin/run-tests.sh SUITE`. `all` (the default)
runs the Python suites with no case of their own (`test_jira_poll.py`,
`test_confluence.py`, `test_notifications.py`, `test_snippet_render.py`) plus
the whole cargo workspace; module cases map to `cargo test -p ws-rs <module>`
(`nvim`, `filter`, `paths`, `screenshot`, `ansi`, `prose`, `panes`,
`vim-keys`, `recent`, `helper-client`, `compare` = Python + `views::compare`),
the rest are Python (`config`, `fileops`, `jsonmgr`, `helper` (Python ≥ 3.11,
`WS_PYTHON`), `settings`, `jira-*`, `confluence-*`, `shot-model`,
`prose-pdf`, `snippets`, …). Direct: `cd rust && cargo test` (~660 tests) or
`python3 Tests/test_x.py`.

UI test suites (`ui`, `vim`, `focus`) were **deleted by owner decision** — they
were slow and ineffective; do not resurrect them or replace them with more
bash+sleep automation. Verify UI work with artifacts + screenshots and the
socket probes below (an unlocked screen is required — AX sees no windows while
the session is locked). `./ws test install` (`Tests/test_install.sh`: needs
`.build/dist/kitchen-sink.app`, throwaway `$HOME`) remains.

Fast UI verification (the `ui-check` skill, `.claude/skills/ui-check/SKILL.md`):

```bash
./ws build                                   # build + relaunch
S="$TMPDIR/ws-notes.sock"
echo state              | nc -U -w3 "$S" | jq '{view, visible, views: .views.notes}'
echo do:toggle-terminal | nc -U -w3 "$S" | jq '.views.notes | {terminal, frame, drawerInset}'
```

Assert only what the change affects, then put the user's live app back the
way you found it. Prefer `state`/`do:` hooks over osascript; if a needed
field is missing, add it to `state_with_registry` / a view's `test_state`.

Build/test gotchas:

- **TCC / Screen Recording cannot be scripted.** `ws permissions fix` resets
  ScreenCapture for the bundle id, rebuilds, relaunches, and opens System
  Settings — a human must click Allow once. A daemon started straight from a
  shell gets the *shell's* TCC identity; launch through LaunchServices
  (`open -n -g kitchen-sink.app --args MODE`, or `--env K=V` for test env).
- **`borders` daemon:** perturbs window-chrome checks; `brew services stop
  borders` for deterministic UI/chrome assertions, restart after.
- **`PYTHONDONTWRITEBYTECODE=1` everywhere.** A `__pycache__` inside the signed
  bundle breaks the signature (and TCC); `main.rs` sets it at startup, the
  build strips `__pycache__`, and `bin/ws-settings` runs `python -B`.
- `WS_PING_ONLY=1 <binary> [MODE]` is the liveness probe (exit 0/1) used by
  `bin/kitchen_sink.sh` and tests.
- `WS_HOME` overrides the config home (tests use it); `WS_DEBUG=1` logs the
  launch path to `logs/YYYY_MM_DD/launch.log`; `WS_LOG_DIR` overrides the log
  directory (tests / the MCP sandbox); `WS_JSON_ROOT` / `WS_JSON_TRACE=1`
  redirect the JSON manager; `WS_RS_ASSET_DIR=<checkout>` points a non-repo,
  non-dist bundle (no Resources payload) at a checkout's `pylib/`/`vim/`;
  `WS_HELPERS_BIN` overrides where `notify_poll.py` finds the AX helpers.

## MCP server / headless control

An agent can drive and verify the app over the socket — no computer control:

- **`kitchen-sink headless`** (also `WS_HEADLESS=1` with no argv mode): the daemon binds the socket
  and runs `state`/`do:`/raw verbs inline on the socket thread with **no AppKit at all** — no
  `NSApplication`, window, menu, status item or Dock presence. `screenshot` and `pane-shot` are
  **refused** (`error: <verb> unavailable in headless mode`, since d21d2b1) so a headless daemon can
  never capture the owner's real screen; capture needs `start --gui`. Headless = `main_queue` never
  installed (`SwitcherController::is_headless`).
- **`bin/ks-mcp-sandbox.sh start|stop|status|sock|home|reset`**: an isolated headless daemon under
  `.build/mcp-sandbox` (its own `TMPDIR`/`WS_HOME`/`WS_COMMANDS_CONF` copy + jira/notify cache env,
  and `HOME`/`XDG_*` redirected under the sandbox root so nothing lands in the real `~/.cache`;
  live-safe cargo build; killed only by pidfile/`lsof -t` on its own socket). Drive it with
  `export KS_SOCKET=$(bin/ks-mcp-sandbox.sh sock)` then `printf 'state\n' | nc -U -w3 "$KS_SOCKET"`.
  The app's `HOME` is a short alias `/tmp/ksb-<hash>` of `$SANDBOX/home` (`… home` prints it) so
  nvim's `--listen` socket stays under the 103-byte AF_UNIX limit; with the long path nvim RPC failed.
  `bin/ks-ocr <png>` turns a capture into JSON text lines (+ a blank-pane check) for non-vision agents.
- **`bin/ks-mcp-sandbox.sh start --gui`** (verified 2026-10-12): launches the real windowed app,
  isolated like the headless sandbox, via `open -n -g --env … kitchen-sink.app --args show`
  (LaunchServices, so the bundle's TCC grants apply). **It runs `aerospace enable off` for the
  duration** (the `on-window-detected` rule moves/tiles the shared window and breaks capture and
  geometry checks); `stop` runs `aerospace enable on` (marker file `.build/mcp-sandbox/aerospace-disabled`).
  If a run dies uncleanly, run `stop` or `aerospace enable on`. Proven path: `do:open:notes` returns
  real state (windows, `views.*.shown`), and `pane-shot` (raw socket or `ks_raw`) writes a real PNG
  of a live window (path under `/private/tmp`; Read it to see the pane).
  **The GUI sandbox pops UI onto the owner's screen:** `open -g` does not stop the app activating
  itself; a bare start opens+activates the notes window, `show` presents the palette (`run_daemon`
  in `main.rs`). Script uses `show` (less intrusive). A true quiet launch needs a code change (a
  skip-launch-UI env); a first attempt made `state` stop answering, so it was reverted. Raw-exec
  of the binary (not `open`) never answers the socket — always launch via `open`. Use `pane-shot` for captures;
  bare `screenshot`/`-r` may raise the interactive overlay, so `ks_raw` stays disallowed in read-only mode
  except `screenshot-permission`.
- **`bin/ks-vm`**: GUI test runs happen in a headless macOS VM (Tart `ks-test`), never on the owner's
  screen: `bin/ks-vm test [Tests/run_flows.py args]`. See `TESTING.md` §3.0.
- **`bin/ks-mcp`** (+ `bin/ks-mcp.py`): legacy stdio MCP bridge, registered in `opencode.json`
  (`kitchen-sink` live = `KS_MCP_READ_ONLY=1`; `kitchen-sink-sandbox` disabled). Tools: `ks_state`,
  `ks_do` (hidden in read-only), `ks_raw` (`screenshot` | `screenshot-permission` | `compare` |
  `pane-shot` | `window-shot`); `--list-tools` debugs; tests in `Tests/test_ks_mcp.py` (`./ws test mcp`).
- Limits: headless proves model/state, **not rendering** — verify visuals with `screenshot -r` /
  `pane-shot` / `window-shot` (the daemon's own shared window straight to a PNG, no overlay);
  `do:key:` is record-only, but **`do:input:key:|text:|click:|activate`** (GUI only,
  `app/synth_input.rs`) posts real `NSEvent`s into the app's own queue, so keys and clicks take
  the normal path (key monitor, responder, nvim) with no global HID input; delivery is async
  (`state.input`). A GUI sandbox stores preferences in its own `WS_DEFAULTS_SUITE`, never the live
  app's defaults domain; `do:ai:*`/`do:confluence:*` registers are dead code. Unknown `do:` verbs, `tool:<unknown>` and
  bad `paths:select:` now return `{"error":…}` (→ `isError` in `ks_do`); still silent no-ops:
  `key:<garbage>`, `tool-close:<unknown>`, `hide` when empty, `compare:select:<out of range>`. Headless
  still reports `shown: true` for panels/tools that cannot exist without AppKit (F0-7, undecided).
  Requests over 2048 bytes are refused (`request too long`), not truncated. Test status and open
  strategy and the orchestrator playbook for DeepSeek test runs: `TESTING.md`; bug ledger: `APP-BUGS.md`. Deferrals in `BACKLOG.md` ("MCP / headless control").

## Linux (experimental)

**Current functional state of the Linux and Windows ports: `PORTS-STATUS.md` (keep it updated).**

The same core runs on Linux behind a second, **egui/eframe** frontend: controller,
socket API, Python helper, engines and view models are shared, and every AppKit half
is compiled out (`#[cfg(target_os = "macos")]`). macOS keeps AppKit, pixel-identical;
Linux is functional-first. Plan: `PLAN-cross-platform.md`.

```bash
bin/linux-check.sh check          # cargo check --workspace --all-targets in Docker
bin/linux-check.sh test [filter]  # cargo test -p ws-rs + the plain-script Python suites
bin/linux-check.sh shell          # interactive shell in the pinned image
./ws test linux                   # same as `bin/linux-check.sh check`
./ws test linux-headless          # Docker `headless` smoke: state + do:open:files
```

`bin/linux-check.sh` (Docker, native arm64; `rust/docker/linux.Dockerfile` pins rustc
1.92 + the X11/GTK/Wayland dev libs) is the truth for compiling and testing. Off macOS
the daemon always runs the egui frontend; **on macOS the same frontend runs behind
`cargo run -p ws-rs --features egui` + `WS_FRONTEND=egui`** (the dev switch for
iterating on the Linux UI on the owner's Mac — use a private `TMPDIR`/`WS_HOME`, never
the live socket). `bin/build-linux.sh` lays out a self-contained dist at
`dist/kitchen-sink/` (a generated `commands.default.toml` + a `run.sh` that exports
`WS_RS_ASSET_DIR` and execs the binary, so `dist/kitchen-sink/run.sh headless` binds
the socket with no checkout); build natively on a Linux host or in the pinned image
with `--docker`, and on Linux `./ws build` runs it.

- Works: `kitchen-sink headless` (the full socket API, no AppKit) and the egui panes —
  files, switcher/palette, paths, jira, confluence, ai (`egui_commonmark`), compare,
  the notes editor and tools/setup — plus row context menus.
- Wave 6 landed: the egui terminal panel (`frontend_egui/terminal.rs`, driven by
  `alacritty_terminal` 0.26 directly, not `egui_term`), the `platform::capture` seam
  (`xcap` 0.4.1; no egui overlay or capture UI yet), and OCR via the `tesseract` CLI
  when it is on `PATH`.
- Window manager: there is no AeroSpace on Linux. Bind your WM's keys to the existing
  CLI — `kitchen-sink notes|files|jira|confluence|ai|compare|window` (and `term`,
  `show`) — which forwards the launch message to the running daemon over
  `$TMPDIR/ws-notes.sock`; `hotkey_prep` is a no-op off macOS.
- Unsupported / known gaps: QuickLook and the tray/global menu (not started); no Linux
  screenshot capture/annotation UI yet (only the `platform::capture` seam); `status.py` still
  reads macOS power/mem APIs; "open in notes" is a no-op off macOS; `ToolsPane` cannot
  pre-select a tool tab. See `PLAN-cross-platform.md` §8.

## Repo layout / code map

`~/.config/kitchen-sink` **is** this git checkout in repo mode. Repo mode
(`Paths::is_repo_build`: `commands.toml` + `bin/build-app.sh` beside the
bundle) reads/writes config and assets here; otherwise `$WS_HOME` or
`~/.config/kitchen-sink`, assets from `Contents/Resources`. `.build/`,
`kitchen-sink.app/`, `rust/target/`, `.agents/`, `commands.toml.bak` are
gitignored.

### Rust workspace (`rust/`)

- `ws-rs` — the app binary `kitchen-sink` (~65k lines; objc2 0.6, block2,
  dispatch2, serde, libc; **no tokio** — std threads + a main-thread drain timer).
  - `main.rs` — one-shots (`help`, `config-schema`, `config-check`, `prose`),
    `AppInstall::ensure_home`, the CLI client pass, then the daemon
    (`run_daemon`: socket server, NSApplication, status item, daemon UI).
  - `app/` — `cli.rs` (the client side: forward to a running daemon, print
    replies, `compare`/`pane-shot`/`screenshot` clients, cold starts via
    `kitchen_sink.sh`, lock hand-over), `host.rs` (`SwitcherController`:
    `ControllerInner` slot state machine, `state_json`, `do_host_action`,
    raw verbs, and the `daemon_ui` module — the shared host window, key
    routing, tool panels), `socket.rs` (server, `Reply` deferred replies,
    `DaemonLock`), `config.rs` (`AppSettings`, `[app]`), `paths.rs`,
    `registry.rs` (`SlotView`, view registry, palette commands, `do:` hooks),
    `hotkey.rs` (AeroSpace prep), `menu.rs`, `python_helper.rs`, `process_run.rs`.
  - `ui/` — `popup.rs` (popup framework + `route_key`), `shared_window.rs`,
    `card.rs`, `chrome.rs`, `theme.rs`.
  - `views/` — `notes.rs` (+ `notes_vim.rs` nvim pane), `files.rs`, `jira.rs`,
    `confluence.rs`, `ai.rs`, `compare.rs`, `screenshot.rs`, `paths.rs`,
    `switcher.rs` (palette + view switcher), `terminal.rs`, `tools.rs`,
    `inspector.rs`, `setup.rs` (`AppInstall` + setup UI).
  - `engines/` — pure logic (ai_format, ansi_render, config_text, file_ops,
    list_filter, nvim_rpc, pane_shot, path_shelf, recent_files,
    screenshot_annotations/text, switcher_status, text_edit_keys).
  - `panes/` — pane_nav, pane_geometry, vim_keys, vim_search.
- `ws-helpers` — `dock_badges` + `webex_unread` AX helpers (run by
  `notify/notify_poll.py`).
- `swiftterm-shim` — the one Swift piece: `WSShim`, a tiny `NSView` around the
  pinned `libSwiftTerm.a`, built by its `build.rs`.

Stable anchors: `SwitcherController`, `ControllerInner`, `state_with_registry`,
`do_host_action`, `raw_deferred`, `route_key_event`, `route_key`, `SlotView`,
`Registry`, `AppSettings`, `Paths`, `PythonHelper`, `PopupColors`, `HeaderStyle`.

### Python packages

- `pylib/` — engines + codecs: `helper/` (the JSON-lines worker), `jsonmgr.py`,
  `config_text.py` (the commands.toml codec), `compare_*`, `file_ops`,
  `ai_format`, `ansi`, `shot_model`, `paneshot`, `shelf`, `prose_pdf`,
  `doc_templates`, `status`, `ignore_rules`, `jira_*` / `confluence_*` glue,
  plus the shipped JSON homes. `text_table/` is standalone (no helper protocol):
  `python3 -m text_table IN OUT --format outlook --fallback-out FB --meta M` —
  OCR/pasted text → table (parse = delimiter detection, formats = registry).
  The palette's "Text to Table" panel (`views/tools.rs` `ToolKind::TextTable`,
  `[text-table]`) and `bin/text-table` (clipboard → clipboard, Neovim) call it.
- `jira/` (poller CLI + API + fake server + `com.jira.poll` launchd agent),
  `confluence/` (API/CLI), `notify/` (unread counts; uses the Rust AX helpers),
  `settings_hub/` (`ws-settings` CLI/TUI; edits commands.toml
  line-preservingly). The app runs these through helper `script.run` (or direct
  spawn for long jobs); they are also standalone CLIs.

### Other dirs

`bin/` (build/test/install tooling; `lib.sh` for `ws_*` helpers;
`preflight.sh`, `grant-permissions.sh`, `setup-home.sh`, `symlinks.sh` at
root); `Tests/` (Python tests + `helper_fixtures/`, `snippet_render/`,
`test_install.sh`); `config/` (aerospace + borders rc, per-file symlinks from
`~/.config/<name>`, plus `paths.ignore`); `rules/` (AI rule markdown),
`templates/`, `samples/`, `vim/` (nvim config + snippets for the notes pane),
`patches/` (SwiftTerm patch only), `.claude/skills/` (repo skills incl.
`ui-check`; `.agents/` is gitignored).

### Docs

`rule.md` (non-negotiables), `HANDOFF-rust-port-next.md` (latest status /
open items), `PRD-*.md`
(`PRD-keyboard.md` = pane/vim spec; `PRD-compare.md`; …), `BACKLOG.md`,
`PRODUCT.md`, `DESIGN.md`, `README.md`. Older plans/PRDs name Swift files —
map them to the Rust module of the same name.

## Architecture

### Process, single instance, launch

`main.rs` handles one-shots before AppKit, runs `AppInstall::ensure_home`
(installed app only), then `app::cli::run`: with `WS_PING_ONLY` it just pings;
a daemon already holding `<socket>.lock` gets the message forwarded
(`reload`/`restart` print the JSON reply, `compare --wait` and `screenshot
--raw` block on the reply, `pane-shot` prints the saved path); a cold hotkey
start (not from launchd) `exec`s `bin/kitchen_sink.sh MODE` so LaunchServices
launches the bundle (TCC). Otherwise the process takes the lock and becomes the
daemon: socket server thread, `NSApplication` + main menu + status item, and
the daemon UI (`install_daemon_ui`: a 30 ms main-thread timer drains the
controller's `UiQueue` and `MainQueue`).

### Shared-window model

One host window, many views: `SlotView` (`notes, files, jira, detail, releases,
config, output, confluence, ai, compare, compareText`). `ControllerInner`
(`app/host.rs`) is the state machine — `open/push/back/home/present/hide/
cycle/toggle/hotkey`; `toggle`/`hotkey` hide only when the user is in the
window (key + active + frontmost), else bring it forward (`hideOrFocus`).
`present` pushes `UiCommand::Present(v)`; the daemon UI swaps the view's
content (built lazily by `content_for`) into the host window, activates and
makes it key. Views are hard-coded in `SlotView`; `commands.toml` declares
commands/windows (`[notes]`, `[files]`, … with `enabled = true`).

### Socket contract (the daemon API)

`$TMPDIR/ws-notes.sock` (`[app] notes-socket`), one newline-terminated request
per connection, served serially by `app/socket.rs`. Order: `ping` (no reply) →
`state` → `do:ACTION` → raw verbs (`reload`/`restart` (JSON),
`screenshot-permission` (`granted|denied`), `screenshot\t…`, `compare\t…`,
`pane-shot\t…` — the last three answer later through a `socket::Reply`;
`links` → the link bank's `links.v1` envelope, `PLAN-link-bank.md`) →
anything else is a launch/hotkey message. `state` returns one JSON line;
`do:ACTION` executes, hops to main (flush), then returns the **same full state
JSON**; errors are `{"error": …}`. `state` keys include: `view`, `visible`,
`active`, `keyWindow`, `windows`, `frontmostPid` (all read live from AppKit),
`views.<view>` (`shown`, `key`, `frame [x,y,w,h]`, `wid`, plus per-view extras),
`hideOnFocusLoss`, `escHides`, `headerStyle`, `pane`, `screenshot`, `compare`,
`paneShot`, `paths`, `tools.<name>`, `palette`, `paletteCommands`,
`viewSwitcher`, `terminalPanel`, `activations`, `pid`. Useful `do:` actions:
`cycle`, `cycle-back`, `hide`, `back`, `home`, `toggle`, `open:<view>`,
`toggle-terminal`, `term`, `switcher`, `reset-size`, `pane:h|j|k|l`,
`pane:focus:<id>`, `key:<spec>`, `header-style:<style>`,
`esc-hides:<view>:on|off`, `tool:<name>` / `tool-close:<name>`, `screenshot:*`,
`go:view:<view>|note:<path>|folder:<compare session id>` (present a `links` row),
`compare:*`, `paths:*`, `sidebar-width:<view>[:px]`,
`inspector-width:jira[:px]`, `jira-inspector:show|hide|toggle` (the Jira ticket
panel), `rail:<view>[:on|off|toggle]` (the sidebar rail — Ctrl+B B on the
socket; notes, files, jira, confluence, ai, compare). Replies > 8 KB are fine
(accepted sockets are blocking).

**Hotkey fast path.** Messages in `hotkey::HOTKEY_MODES` (`window`, `notes`,
`voice`, `jira`, `files`, `confluence`, `ai`, `compare`) run `hotkey_prep`
(AeroSpace IPC: focused window → focus file, focused workspace/screen, move
stray windows) before the toggle. Tool-panel hotkeys (Hyper+X screenshot) are
not in that list. `[screenshot] text-column-gap` (× line height; 0 = off) makes
Copy Text join wide-gapped blocks on a row with a tab so tables keep columns.
**Hyper+S never activates kitchen-sink.** The palette and every popup it opens
(tool panels, terminal, paths, screenshot, note finders) are non-activating
panels that take the keyboard without activating the app; only a pick of
"Kitchen Sink" or a view activates (via the view's `present`). Never add
`activate_app` to a popup path: an active kitchen-sink with no window up lets
macOS raise the shared window and AeroSpace jumps to its workspace. As a
backstop the palette remembers the frontmost app (`PALETTE_RETURN`) and hands
focus back on cancel / popup picks (`palette_hand_back`, `pick_keeps_app`).

### Keyboard / focus

The daemon UI installs one local `keyDown` monitor → `route_key_event`
(Ctrl+B prefix, Ctrl+H/J/K/L pane moves, vim keys, overlays — palette,
switcher, tool panels — Esc streak per `escCloseCount`/`esc-close` (0 =
never), window-size keys, edit keys) deciding through `ui::popup::route_key`.
Ctrl+B is a 1.5 s prefix: L = previous view, W = view switcher (herdr's
workspaces collapse at its bottom — `[links]`, `views/switcher.rs`; Ctrl+K
jumps there, Return runs `herdr-pick go-id`), T =
terminal/prose, B = sidebar rail (every `SidebarView` pane: notes, files,
jira, confluence, ai, compare; persisted per view as `sidebarRail.<view>`).
`PaneNav` moves the focus ring between panes;
`VimKeys` gives the focused pane normal/insert/search modes. Tool panels are
non-activating `NSPanel`s that take key once 0.25 s after opening
(`reclaimToolKey`).

### Theme

`[theme]` → `ui/theme.rs` (`PopupColors`, `HeaderStyle`, ~30 `[themes]`
presets incl. Nightfox = default); applying a preset persists hex values into
`commands.toml`. Draw with the derived tokens, never system colors.

### Python boundary

`PythonHelper::shared()` (`app/python_helper.rs`) spawns `python3 -B -m helper`
with cwd/path `<assets>/pylib`, `PYTHONPATH=pylib`,
`PYTHONDONTWRITEBYTECODE=1`; interpreter discovery prefers `WS_PYTHON` then brew
pythons, requiring ≥ 3.11. Protocol: one JSON object per line — request
`{"id", "method", "params"}` → reply `{"id", "ok", "result"}` or
`{"id", "ok": false, "error": {"message", "traceback"}}`; out-of-order replies
matched by id; an 8-worker pool; a killed worker restarts on the next call.
Handle-based engines (`compare.*`, `folder.*`, `fileops.*`, `ignore.*`) keep
state in the worker. `script.run` runs the Python CLIs as children.
`engines/config_text.rs` uses the helper (`config.decode`,
`config.section_entries`, `config.line`, `config.setting`, …) and caches
decodes as `~/.cache/kitchen-sink/config-<hash>.json`.

**jsonmgr** (`pylib/jsonmgr.py`) is the one manager for shipped JSON homes:
never bare `json.load` a shipped home — use `jsonmgr.load("<repo-relative path
minus .json>")`; user/runtime data keeps its own I/O.

### Config (commands.toml)

Flat `[section]` tables, one `key = value` per line: `[app]`, `[theme]`,
`[themes]`, per-view sections (`[notes]`, `[files]`, `[jira]`, `[confluence]`,
`[ai]`, `[compare]`, `[screenshot]`, `[pane-shot]`, `[paths]`, `[notes-find]`,
`[filefast]`), `[icons]`, `[shortcuts]`, `[terminal]`, `[prettyprint]`,
`[health-checks]`, `[setup]`, `[settings-hub]`, … Reads go through the Python
line codec (or the pure scans in `host.rs`, which unquote values like the
codec); writes through `config.line`/`config.setting` with a validation gate
(invalid current file parked as `commands.toml.broken-<epoch>`; `.bak`
refreshed; atomic write). Repo build → **this repo's** `commands.toml`;
installed app → `~/.config/kitchen-sink/commands.toml` (`WS_COMMANDS_CONF` is
exported for Python readers).

## Common tasks

- **Add an `[app]` setting:** add the key to `commands.toml`, parse it in
  `app/config.rs` (`AppSettings`), validate (`config_value_problem`), write back
  through the codec. Never hard-code a user-facing string/size.
- **Add a keyboard shortcut:** in-window → `route_key_event` /
  `ui::popup::route_key` (respect rule 1) + a `[shortcuts]` row. Global
  Hyper+KEY → `config/aerospace/aerospace.toml` binding (then `aerospace
  reload-config --dry-run --no-gui`), `hotkey::HOTKEY_MODES` + the host's
  `toggle_command`, `bin/kitchen_sink.sh`, `commands.toml [shortcuts]`, docs.
- **Add a view:** extend `SlotView` + nav ids, `content_for`, the registry
  (`app/registry.rs`), and the `[section]` + `enabled` gate.
- **Add a socket `do:` hook:** `do_host_action` or a view's registry hook
  (`register_test_do`; return `Some(Value::Null)` for "ok", `{"error"}` else);
  expose new state in `state_with_registry` / the view's `test_state`.
- **Add a raw verb with a late reply:** add it to `socket::RAW_VERBS`, handle
  it in `raw_deferred` and answer through the `Reply`.
- **Verify UI changes fast:** the `ui-check` workflow above.
- **Repair the stack:** `./ws doctor [--fix]`, `./ws check`, `./ws permissions
  fix`, `./ws links fix`, `./ws home status`. Logs: every log lives in
  `logs/YYYY_MM_DD/*.log` (date-based rolling, `pylib/log_hub.py`;
  `bin/ws-logs` lists them) — the app's own log is `logs/YYYY_MM_DD/daemon.log`
  (Rust `app/logging.rs` `ws_log`, the restored Swift `wsLog`: stderr + file),
  and the launch trace (`WS_DEBUG=1`) is `logs/YYYY_MM_DD/launch.log`.

## Gotchas / footguns

- **Main-thread discipline.** AppKit objects live on main (`MainThreadMarker`);
  the socket thread reaches them through `run_on_main` (blocking, 2 s) or
  `post_main` (async). Never hold the `ControllerInner` lock while waiting on
  main; lock order is `inner` before the compare model.
- **Helper pool starvation + retries.** One 8-worker pool serves interactive
  calls and long `script.run` jobs; treat helper replies as fallible; don't
  memoize failures.
- **Swift `canBecomeKey` is the ObjC selector `canBecomeKeyWindow`** (same for
  Main) — override the ObjC name in `define_class!`, or the panel never takes key.
- **AeroSpace** (`on-window-detected` moves the shared window to workspace 2)
  shapes every focus/show test; JankyBorders skips `NSPanel`s.
- **Config writes can silently no-op** if the Python `config.setting` call
  times out under helper load. In repo mode you are editing this checkout's
  `commands.toml` — UI tests snapshot/restore it; don't leave it dirty.
- **Sign/TCC.** Ad-hoc signing loses TCC grants; `ws permissions` expects the
  self-signed cert.
- **`./ws build` kills and relaunches the app**; UI tests take it over, steal
  focus, and mutate `commands.toml` (restored). Run once, not concurrently.
- **SwiftTerm is read-only** (fetched into `../SwiftTerm`); changes go in
  `patches/`. The shim (`rust/swiftterm-shim/shim/WSShim.swift`) is ours.
- **Symbols over line numbers** — line numbers drift constantly.

## Known gaps (see `HANDOFF-rust-port-next.md`)

- Notes vim pane: `:q` relaunch doesn't restart nvim (the vim suite is parked).
- Hide-on-focus-loss: the setting is reported, but the Swift focus bridge
  (`ws-aerospace-focus` watcher) is not ported.
- AeroSpace re-show differs from Swift ("no jump back" in the focus suite).
- Vim search highlight (`VimMatchOverlay`/`hlsearch`): rows are ported for the
  sidebar, files, Jira and Confluence lists; text (`NSTextView` temporary
  background) and web (`CSS.highlights`) targets are still unported.
- `Cmd+K` action picker, `Cmd+/` shortcuts sheet, pane-shot preview window,
  compare `summary` label. (`do:notes-find` / `do:notes-grep` / `do:jira-jump`
  windows are ported: `views/note_find.rs`; the notes vim pane's `leader+sf` /
  `leader+sg` route raw `notes-find` / `notes-grep`; `<leader>sf` on the
  non-nvim panes opens `PaneFindUi`.)
