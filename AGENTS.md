# AGENTS.md — kitchen-sink agent context

Current, authoritative agent context for this repo. Docs drift; **the code is
truth**. Grep a symbol, then read ~60 lines around it. `rust/ws-rs/src/app/host.rs`
is ~4.5k lines — never read it whole. `CLAUDE.md` is a symlink to this file; the old
deep-detail doc is in git history (`git log -- AGENT_CONTEXT.md`).

## What this is (overview)

kitchen-sink is a single-instance macOS app — **Rust + AppKit via objc2**
(`rust/`, cargo workspace) — that runs as a hotkey-driven daemon: AeroSpace / CLI
launches talk to it over a Unix socket (`$TMPDIR/ws-notes.sock`, lock file beside
it), and one **shared window** hosts the views — files, jira, confluence,
AI — plus tool panels (screenshot, paths, prettyprint,
filefast, health checks, view switcher / command palette). It is a *regular* app
(`NSApplicationActivationPolicy::Regular`: menu bar + status item). Python
(`pylib/` plus `jira/`, `confluence/`, `notify/`, `settings_hub/`) is the "cold
path": the app talks to it over a persistent JSON-lines helper protocol.
The Compare view (text + folder diff) was removed in Nov 2026 by owner
decision; don't bring it back.

**Two processes.** The shared-window app above is the *main* app. Hyper+S (the
palette) and Hyper+X (screenshot) are owned by a small separate resident
process, the **quick agent** (`kitchen-sink quick`, see "Quick agent" below), so
both work with the main app not running.

History: the app was Swift until the Phase-4 cutover (Oct 2026); the Rust
port mirrors the old Swift files module for module, and Swift type names
survive in comments (`SwitcherController`,
`PopupWindow.handleKey`, `testQuery`) — use them to find the Rust counterpart.
No Swift is left: the SwiftTerm terminal panel (and its shim crate) was removed
in Nov 2026 by owner decision.

## Non-negotiable rules (condensed from `rule.md`)

1. **Edit shortcuts work in every text input.** Cmd+C/V/X/A/Z plus Ctrl+C/V;
   keys are routed explicitly (the key router `route_key_event` /
   `ui::popup::route_key` must forward edit shortcuts to the focused field
   editor; sheets route paste to their field editor and set
   `initialFirstResponder`).
2. **Right-click menus** in file browsers offer: copy file,
   copy absolute path, reveal in the app's own Files view. (The notes app
   was removed Nov 2026; markdown editing, snippets and the document-style
   picker now live in nvim (`~/.dotfiles/nvim`: `<leader>mdo` build/open,
   `<leader>mds` style, `<leader>mdi` snippets). The socket keeps its old name `ws-notes.sock`.)
3. **Config over code.** User-facing strings, sizes, and paths live in
   `commands.toml`; prefer a config change over a hard-coded one.
4. **Git only in this repo** (`/Users/danielbaker/.config/kitchen-sink`).
   Never stage/commit/change `.git` in a `.bak` copy, backup, or duplicate.
   Verify with `git rev-parse --show-toplevel` before git commands.
5. **Transient overlays own the keyboard.** Popovers/pickers take key focus on
   open and Esc closes only them, never the window under them.
6. **Python stays stable.** Keep the helper protocol and `pylib/` behavior
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
use). `--dist` →
`.build/dist/kitchen-sink.app` with resources per `install.conf` `RESOURCE_*`,
a rewritten `commands.default.toml` and the AX helpers in
`Resources/helpers-bin` (`HELPER_BINS`). Signed with the self-signed `SIGN_ID`
("kitchen-sink codesign") for stable TCC grants. **A non-dist build `pkill`s
the running daemon before swapping the binary.** `--stale` exits 0 when a
rebuild is needed (`kitchen_sink.sh` uses it for build-if-stale). `ws clean`
stops the daemon and removes `.build/rust-target` (plus `rust/target` so the
next build is a full cargo rebuild); `--all` also wipes `.build/dist` and the repo-root app bundle (a wiped
bundle is recreated by `mkdir -p` + a full build). Toolchain:
cargo (brew `rust` or rustup) + Xcode CLT.
Don't export `MACOSX_DEPLOYMENT_TARGET=26.7` (corrupt proc-macro dylibs).

**Tests.** `./ws test SUITE` → `bin/run-tests.sh SUITE`. `all` (the default)
runs the Python suites with no case of their own (`test_jira_poll.py`,
`test_confluence.py`, `test_notifications.py`) plus
the whole cargo workspace; module cases map to `cargo test -p ws-rs -p ks-core <module>`
(`filter`, `paths`, `screenshot`, `ansi`, `prose` (= `engines::ai_format`), `panes`,
`vim-keys`, `recent`, `helper-client`),
the rest are Python (`config`, `fileops`, `jsonmgr`, `helper` (Python ≥ 3.11,
`WS_PYTHON`), `settings`, `jira-*`, `confluence-*`,
…). Direct: `cd rust && cargo test` (~680 tests) or
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
echo state         | nc -U -w3 "$S" | jq '{view, visible, views: .views.files}'
echo do:open:files | nc -U -w3 "$S" | jq '.views.files | {shown, frame}'
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
  non-dist bundle (no Resources payload) at a checkout's `pylib/`;
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
  If a run dies uncleanly, run `stop` or `aerospace enable on`. Proven path: `do:open:files` returns
  real state (windows, `views.*.shown`), and `pane-shot` (raw socket or `ks_raw`) writes a real PNG
  of a live window (path under `/private/tmp`; Read it to see the pane).
  **The GUI sandbox pops UI onto the owner's screen:** `open -g` does not stop the app activating
  itself; a bare start opens+activates the shared window, `show` presents the palette (`run_daemon`
  in `main.rs`). Script uses `show` (less intrusive). A true quiet launch needs a code change (a
  skip-launch-UI env); a first attempt made `state` stop answering, so it was reverted. Raw-exec
  of the binary (not `open`) never answers the socket — always launch via `open`. Use `pane-shot` for captures;
  bare `screenshot`/`-r` may raise the interactive overlay, so `ks_raw` stays disallowed in read-only mode
  except `screenshot-permission`.
- **`bin/ks-vm`**: GUI test runs happen in a headless macOS VM (Tart `ks-test`), never on the owner's
  screen: `bin/ks-vm test [Tests/run_flows.py args]`. See `TESTING.md` §3.0.
- **`bin/ks-mcp`** (+ `bin/ks-mcp.py`): legacy stdio MCP bridge, registered in `opencode.json`
  (`kitchen-sink` live = `KS_MCP_READ_ONLY=1`; `kitchen-sink-sandbox` disabled). Tools: `ks_state`,
  `ks_do` (hidden in read-only), `ks_raw` (`screenshot` | `screenshot-permission` |
  `pane-shot` | `window-shot`); `--list-tools` debugs; tests in `Tests/test_ks_mcp.py` (`./ws test mcp`).
- Limits: headless proves model/state, **not rendering** — verify visuals with `screenshot -r` /
  `pane-shot` / `window-shot` (the daemon's own shared window straight to a PNG, no overlay);
  `do:key:` is record-only, but **`do:input:key:|text:|click:|activate`** (GUI only,
  `app/synth_input.rs`) posts real `NSEvent`s into the app's own queue, so keys and clicks take
  the normal path (key monitor, responder, nvim) with no global HID input; delivery is async
  (`state.input`). A GUI sandbox stores preferences in its own `WS_DEFAULTS_SUITE`, never the live
  app's defaults domain; `do:ai:*`/`do:confluence:*` registers are dead code. Unknown `do:` verbs, `tool:<unknown>` and
  bad `paths:select:` now return `{"error":…}` (→ `isError` in `ks_do`); still silent no-ops:
  `key:<garbage>`, `tool-close:<unknown>`, `hide` when empty. Headless
  still reports `shown: true` for panels/tools that cannot exist without AppKit (F0-7, undecided).
  Requests over 2048 bytes are refused (`request too long`), not truncated. Test status and open
  strategy and the orchestrator playbook for DeepSeek test runs: `TESTING.md`; open bugs and leads: `BACKLOG.md` ("MCP / headless control", "Leads").

## Platform

macOS only. Linux and Windows support was dropped in Nov 2026 by owner decision
(scripts, Docker image and non-macOS code removed). The old plans are in git history.

## Repo layout / code map

`~/.config/kitchen-sink` **is** this git checkout in repo mode. Repo mode
(`Paths::is_repo_build`: `commands.toml` + `bin/build-app.sh` beside the
bundle) reads/writes config and assets here; otherwise `$WS_HOME` or
`~/.config/kitchen-sink`, assets from `Contents/Resources`. `.build/`,
`kitchen-sink.app/`, `rust/target/`, `.agents/`, `commands.toml.bak` are
gitignored.

### Rust workspace (`rust/`)

- `ks-core` — the shared library crate (lib name `ks_core`; objc2 deps, **no
  dependency on ws-rs**: the compiler enforces that `host.rs`, the controller and
  the shared window are invisible to it). Everything the quick agent runs lives
  here, plus the framework the main daemon shares:
  - `platform/` — all cross-platform seams (clipboard, dispatch, ipc, lock,
    screens, capture, settings).
  - `app/` — `quick.rs` (the quick agent), `cli.rs` (client side), `socket.rs`
    (server, `Reply`, `DaemonLock`), `config.rs`, `paths.rs`, `pathfmt.rs`,
    `logging.rs`, `hotkey.rs` (AeroSpace), `menu.rs`, `process_run.rs`,
    `python_helper.rs`, `registry.rs` (`SlotView`, `Registry`, `PaletteCommand`:
    plain data types the screenshot view registers `do:` hooks into),
    `sections.rs` (pure `commands.toml` scans), `find_window.rs`, and
    **`hooks.rs`** — the seam for what only a host can do (below).
  - `ui/` — every UI-framework module except `shared_window.rs` and
    `jira_setup.rs` (popup, popup_keys, theme, chrome, style, list_panel,
    view_base, toast, menu_kit, preview_ring, sidebar, field, ...).
  - `views/` — `switcher`, `tools`, `paths`, `screenshot`, `files`,
    `files_view`, `file_popup`.
  - `engines/` — config_text, file_ops, list_filter, pane_shot, path_shelf,
    recent_files, screenshot_annotations/text, switcher_status, text_edit_keys,
    ansi_render. `panes/` — pane_nav, pane_geometry, vim_keys, vim_search.
  - **Hooks** (`app::hooks`): `set_reveal_hook` / `reveal_in_files`,
    `set_shelf_add_hook` / `screenshot_saved_to_path_shelf` (default:
    `path_shelf::pure_add`), `set_cancel_click_rename_hook`,
    `set_popup_file_actions_hook` / `popup_file_menu_actions`,
    `set_popup_rename_hook` / `popup_rename_commit` (default: `rename_into`).
    First installer wins; the main daemon installs its `daemon_ui`
    implementations at the top of `install_daemon_ui`, the quick agent installs
    only the reveal hook (forward to the main socket). Shared code must call the
    `hooks::` free functions, never reach for the host.
  - Rule: **never `use` ws-rs from ks-core.** Need something from the main
    daemon? Add a slot to `hooks.rs`.
- `ws-rs` — the app binary `kitchen-sink` (depends on `ks-core`; objc2 0.6,
  block2, dispatch2, serde, libc; **no tokio** — std threads + a main-thread
  drain timer). The moved modules are re-exported (`pub use ks_core::ui::popup;`
  in `ui/mod.rs`, `use ks_core::platform;` in `main.rs`, ...) so `crate::ui::…`
  paths in ws-rs are unchanged; new shared code goes in ks-core, and a module
  moved there must be added to the matching re-export list.
  - `main.rs` — one-shots (`help`, `config-schema`, `config-check`),
    `AppInstall::ensure_home`, the CLI client pass, then the daemon
    (`run_daemon`: socket server, NSApplication, status item, daemon UI) and
    `run_quick` (the entry for `kitchen-sink quick`).
  - `app/` — `host.rs` (`SwitcherController`: `ControllerInner` slot state
    machine, `state_json`, `do_host_action`, raw verbs, and the `daemon_ui`
    module — the shared host window, key routing, tool panels), plus
    `jira_poll.rs`, `synth_input.rs` and re-exports.
  - `ui/` — `shared_window.rs`, `jira_setup.rs` (the rest is in ks-core).
  - `views/` — `jira.rs`, `pane_find.rs` (jira-jump + `/` pane finder),
    `confluence.rs`, `ai.rs`, `setup.rs`
    (`AppInstall` + setup UI), the jira/confluence/ai views.
  - `engines/` — ai_format only.
- `ws-helpers` — `dock_badges` + `webex_unread` AX helpers (run by
  `notify/notify_poll.py`).

Stable anchors: `SwitcherController`, `ControllerInner`, `state_with_registry`,
`do_host_action`, `raw_deferred`, `route_key_event`, `route_key`, `SlotView`,
`Registry`, `AppSettings`, `Paths`, `PythonHelper`, `PopupColors`, `HeaderStyle`.

### Python packages

- `pylib/` — engines + codecs: `helper/` (the JSON-lines worker), `jsonmgr.py`,
  `config_text.py` (the commands.toml codec), `file_ops`,
  `ai_format`, `shelf`,
  `status`, `ignore_rules`, `jira_*` / `confluence_*` glue,
  plus the shipped JSON homes. `text_table/` is standalone (no helper protocol):
  `python3 -m text_table IN OUT --format outlook --fallback-out FB --meta M` —
  OCR/pasted text → table (parse = delimiter detection, formats = registry).
  The palette's "Text to Table" panel (`views/tools.rs` `ToolKind::TextTable`,
  `[text-table]`) and `bin/text-table` (clipboard → clipboard, Neovim) call it.
- `jira/` (poller CLI + API + fake server + `com.jira.poll` launchd agent),
  `confluence/` (API/CLI), `notify/` (unread counts; uses the Rust AX helpers),
  `settings_hub/` (`ws-settings` CLI/TUI; edits commands.toml
  line-preservingly). The app spawns these directly (`app/process_run.rs`);
  they are also standalone CLIs.

### Other dirs

`bin/` (build/test/install tooling; `lib.sh` for `ws_*` helpers;
`preflight.sh`, `grant-permissions.sh`, `setup-home.sh`, `symlinks.sh` at
root); `Tests/` (Python tests,
`test_install.sh`); `config/` (launchd plist, `paths.ignore`; `CONFIG_DIRS` is empty — **aerospace.toml,
bordersrc, `stack_border.sh` and `no_stray_workspaces.sh` live in `~/.dotfiles`**
(`aerospace/`, `borders/`), linked by `~/.dotfiles/symlinks.sh`; the config calls
kitchen-sink scripts through the guard `~/.dotfiles/aerospace/bin/ks`, plus `paths.ignore`); `rules/` (AI rule markdown),
`templates/`, `samples/`,
`.claude/skills/` (repo skills incl.
`ui-check`; `.agents/` is gitignored).

### Docs

`rule.md` (non-negotiables), `BACKLOG.md` (every open item), `TESTING.md` (test
strategy, flow schema, worker playbook), `PRODUCT.md`, `DESIGN.md`, `README.md`.
The old PRDs/plans/handoffs were deleted in the Nov 2026 consolidation; read them
with `git show 64bbd4a^:<file>` (e.g. `PRD-keyboard.md`, `PRD-screenshot.md`,
`PRD-settings-hub.md`, `HANDOFF-rust-port-next.md`). They name Swift files — map
them to the Rust module of the same name.

## Architecture

### Process, single instance, launch

`main.rs` handles one-shots before AppKit, runs `AppInstall::ensure_home`
(installed app only), then `app::cli::run`: with `WS_PING_ONLY` it just pings;
a daemon already holding `<socket>.lock` gets the message forwarded
(`reload`/`restart` print the JSON reply, `screenshot
--raw` blocks on the reply, `pane-shot` prints the saved path); a cold hotkey
start (not from launchd) `exec`s `bin/kitchen_sink.sh MODE` so LaunchServices
launches the bundle (TCC). Otherwise the process takes the lock and becomes the
daemon: socket server thread, `NSApplication` + main menu + status item, and
the daemon UI (`install_daemon_ui`: a 30 ms main-thread timer drains the
controller's `UiQueue` and `MainQueue`).

### Shared-window model

One host window, many views: `SlotView` (`files, jira, detail, releases,
config, output, confluence, ai`). `ControllerInner`
(`app/host.rs`) is the state machine — `open/push/back/home/present/hide/
cycle/toggle/hotkey`; `toggle`/`hotkey` hide only when the user is in the
window (key + active + frontmost), else bring it forward (`hideOrFocus`).
`present` pushes `UiCommand::Present(v)`; the daemon UI swaps the view's
content (built lazily by `content_for`) into the host window, activates and
makes it key. Views are hard-coded in `SlotView`; `commands.toml` declares
commands/windows (`[files]`, `[jira]`, … with `enabled = true`).

### Socket contract (the daemon API)

`$TMPDIR/ws-notes.sock` (`[app] notes-socket`), one newline-terminated request
per connection, served serially by `app/socket.rs`. Order: `ping` (no reply) →
`state` → `do:ACTION` → raw verbs (`reload`/`restart` (JSON),
`screenshot-permission` (`granted|denied`), `screenshot\t…`,
`pane-shot\t…` — the last three answer later through a `socket::Reply`) →
anything else is a launch/hotkey message. `state` returns one JSON line;
`do:ACTION` executes, hops to main (flush), then returns the **same full state
JSON**; errors are `{"error": …}`. `state` keys include: `view`, `visible`,
`active`, `keyWindow`, `windows`, `frontmostPid` (all read live from AppKit),
`views.<view>` (`shown`, `key`, `frame [x,y,w,h]`, `wid`, plus per-view extras),
`hideOnFocusLoss`, `escHides`, `headerStyle`, `pane`, `screenshot`,
`paneShot`, `paths`, `tools.<name>`, `palette`, `paletteCommands`,
`viewSwitcher`, `activations`, `pid`. Useful `do:` actions:
`cycle`, `cycle-back`, `hide`, `back`, `home`, `toggle`, `open:<view>`,
`switcher`, `reset-size`, `pane:h|j|k|l`,
`pane:focus:<id>`, `key:<spec>`, `header-style:<style>`,
`esc-hides:<view>:on|off`, `tool:<name>` / `tool-close:<name>`, `screenshot:*`,
`paths:*`, `sidebar-width:<view>[:px]`,
`inspector-width:jira[:px]`, `jira-inspector:show|hide|toggle` (the Jira ticket
panel), `rail:<view>[:on|off|toggle]` (the sidebar rail — Ctrl+B B on the
socket; files, jira, confluence, ai). Replies > 8 KB are fine
(accepted sockets are blocking).

**Hotkey fast path.** Messages in `hotkey::HOTKEY_MODES` (`window`, `jira`,
`files`, `confluence`, `ai`) run `hotkey_prep`
(AeroSpace IPC: focused window → focus file, focused workspace/screen, move
stray windows) before the toggle. Tool-panel hotkeys (Hyper+X screenshot) are
not in that list. `[screenshot] text-column-gap` (× line height; 0 = off) makes
Copy Text join wide-gapped blocks on a row with a tab so tables keep columns.
**Hyper+S / Hyper+X live in the quick agent** (below), not in the main
daemon; the main daemon keeps `show` / `do:` / `screenshot` on its own socket
for in-window use and tests. The rules in this paragraph apply to both.
**Hyper+S never activates kitchen-sink.** The palette and every popup it opens
(tool panels, paths, screenshot, note finders) are non-activating
panels that take the keyboard without activating the app; only a pick of
"Kitchen Sink" or a view activates (via the view's `present`). Never add
`activate_app` to a popup path: an active kitchen-sink with no window up lets
macOS raise the shared window and AeroSpace jumps to its workspace. As a
backstop the palette remembers the frontmost app (`PALETTE_RETURN`) and hands
focus back on cancel / popup picks (`palette_hand_back`, `pick_keeps_app`).
The Hyper+S palette is **one list**: the `[app] palette` commands
(`hyper_palette`; shipped: Find Recent Files, Create File, Text to Table, Pretty
Print), then the AeroSpace workspaces with their app icons (cached, refreshed
off-main). No side-by-side panes, no Tab pane switch. Ctrl+B W's `/` commands
keep the full `in-palette` list. `bin/kitchen_sink.sh` toggles a running
daemon before its lock / aerospace query, so the press costs one socket write.

### Quick agent (Hyper+S / Hyper+X)

`kitchen-sink quick [show|screenshot]` (`ks-core/src/app/quick.rs`, `main.rs`
`run_quick`, `cli.rs` `Outcome::Quick` — all in the `ks-core` crate, which cannot
see `host.rs`) is a mode of the SAME signed bundle (so the existing
Screen Recording grant applies): an Accessory app with no status item, a minimal
main menu (Edit shortcuts), **no `SwitcherController`, no shared window, no
registry, no Python worker pool** (config is read by pure scans:
`config::read_config_text_pure` / `merge_override_pure`, `host::parse_sections`).
It owns the palette (`PalettePanel`), the tool panels `prettyprint` / `filefast`
/ `text-table`, the paths shelf window ("Find Recent Files"), find-window and
the `ScreenshotController` + overlay. Event driven: every socket request is one
main-queue dispatch; the only timer is a 30 ms poll while a tool panel is open.

- **Socket** `$TMPDIR/ks-quick.sock` (`Paths::quick_socket_path`, lock beside it
  `…sock.lock`, `DaemonLock`; the name is the constant `QUICK_SOCKET_NAME`, not a config key).
  Verbs: `ping`, `toggle` / `show` / `hide` / `quit`, `state` (JSON: `pid`,
  `palette`, `paths`, `tools`, `openTools`, `screenshotBusy`),
  `screenshot[\tWORDS]` (same words / raw-reply semantics as the main daemon,
  shared `views::screenshot::run_screenshot_message`), `screenshot-permission`.
  Everything else (`do:`, `reload`, `pane-shot`, …) is the main socket's.
- **CLI**: `kitchen-sink toggle` / `quick-toggle`, `quick-show`, `quick-quit`,
  `quick-state`, `quick-screenshot [words]` (quick only; exit 1 = no agent),
  `screenshot [words]` (quick, then a running main daemon, then
  `bin/kitchen_sink.sh screenshot`). `kitchen-sink quick` with an agent already
  holding the lock just delivers its action and exits (`open -n` starts a
  second process).
- **Launch**: `bin/kitchen_sink.sh` (no mode) and `bin/kitchen_sink.sh
  screenshot` first try `quick-toggle` / `quick-screenshot` (one socket write);
  a cold start is `open -n -g kitchen-sink.app --args quick show|screenshot`
  (LaunchServices, for TCC). The aerospace Hyper+S / Hyper+X bindings both call
  the script. The script's main-app cold starts (`kill_main`) spare the agent;
  `./ws build` / `bin/lib.sh` stops (`pkill -f …/Contents/MacOS`) kill it too —
  the next press cold-starts it again.
- **Login**: `bin/quick-agent.sh install|uninstall|status` writes the LaunchAgent
  `~/Library/LaunchAgents/dev.danielbaker.kitchen-sink.quick.plist` from
  `config/launchd/…plist` (runs `open -g -n <app> --args quick`; INSTALL.sh
  installs it, UNINSTALL.sh removes it).
- **Restart / stop**: `kitchen-sink quick-quit` (then any Hyper+S press starts a
  fresh one), or `bin/kitchen_sink.sh quick`. Logs go to the usual
  `logs/YYYY_MM_DD/daemon.log` (`quick: …`, `hotkey: toggle recv->shown Nms`).
- **Picks**: agent-owned commands open locally (`quick::route_command`);
  workspaces via AeroSpace IPC; everything else (views, "Bansho"/`window`,
  jira-config, health) is forwarded to `ws-notes.sock`
  (`quick::forward_to_main`; `jira-config` → `jira-dashboard`), cold-starting the main app through `bin/kitchen_sink.sh <mode>` when
  nothing answers. Screenshot / paths "reveal in Files" open the agent's own
  Find Recent Files panel at that
  folder with the files selected (`hooks::set_reveal_hook` → `show_files_at`),
  never the main app (AeroSpace pins its window to workspace 2).
- **Pictures → screenshot editor**: in Find Recent Files and the main Files
  view, right-click a selection whose files are *all* pictures (judged by
  magic bytes, `engines::image_files`, never the extension) offers "Open in
  Screenshot Editor" (1) / "Merge N Photos…" (2+)
  (`screenshot::editor_menu_entry` → `open_images_in_editor`, the Recents
  merge path; ImageIO decode applies EXIF orientation). Each process opens
  its own editor. Titles: `[screenshot] editor-menu-title`,
  `editor-merge-menu-title`.
- **Shared state**: `~/.cache/kitchen-sink/paths.json` has two writers (main
  `PathShelf` via Python `shelf.save`, agent via `path_shelf::pure_add`); both
  write tmp + rename, and the main shelf reloads when the file's mtime moved
  (`PathShelf::refresh_from_disk`). The agent has no Recent-files FSEvents feed:
  it shows what is on disk and adds screenshots.
- **Not in the agent**: rename / compress / trash in the paths shelf (rows menu
  offers open / reveal / copy), the screenshot `do:` hooks (registered only in
  the main daemon), health checks.

### Keyboard / focus

The daemon UI installs one local `keyDown` monitor → `route_key_event`
(Ctrl+B prefix, Ctrl+H/J/K/L pane moves, vim keys, overlays — palette,
switcher, tool panels — Esc streak per `escCloseCount`/`esc-close` (0 =
never), window-size keys, edit keys) deciding through `ui::popup::route_key`.
Ctrl+B is a 1.5 s prefix: L = previous view, W = view switcher
(`views/switcher.rs`), B = sidebar rail (every `SidebarView` pane: files,
jira, confluence, ai; persisted per view as `sidebarRail.<view>`).
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
Handle-based engines (`fileops.*`, `ignore.*`) keep
state in the worker. The Python CLIs are spawned by `app/process_run.rs`.
`engines/config_text.rs` uses the helper (`config.decode`,
`config.section_entries`, `config.line`, `config.setting`, …) and caches
decodes as `~/.cache/kitchen-sink/config-<hash>.json`.

**jsonmgr** (`pylib/jsonmgr.py`) is the one manager for shipped JSON homes:
never bare `json.load` a shipped home — use `jsonmgr.load("<repo-relative path
minus .json>")`; user/runtime data keeps its own I/O.

### Config (commands.toml)

Flat `[section]` tables, one `key = value` per line: `[app]`, `[theme]`,
`[themes]`, per-view sections (`[files]`, `[jira]`, `[confluence]`,
`[ai]`, `[screenshot]`, `[pane-shot]`, `[paths]`, `[pane-find]`,
`[filefast]`), `[icons]`, `[shortcuts]`, `[prettyprint]`,
`[health-checks]`, `[setup]`, `[settings-hub]`, … Reads go through the Python
line codec (or the pure scans in `host.rs`, which unquote values like the
codec); writes through `config.line`/`config.setting` with a validation gate
(invalid current file parked as `commands.toml.broken-<epoch>`; `.bak`
refreshed; atomic write). Repo build → **this repo's** `commands.toml`;
installed app → `~/.config/kitchen-sink/commands.toml` (`WS_COMMANDS_CONF` is
exported for Python readers).
**One personal config folder** (`configDir` in `pylib/paths.json`, or
`$WS_CONFIG_DIR`; default `~/.config/kitchen-sink-local`, a sibling of the repo,
which *is* `~/.config/kitchen-sink`). It holds everything machine-local:
`commands.toml` (override), `files-favorites.json` (Files pins),
`jira/config.json` + `jira/team.json` (token, chmod 600), `confluence/config.json`.
Python resolves it in `jira_paths.CONFIG_DIR`; Rust in `config::config_dir()`
(reads paths.json); `bin/check-overrides.sh` reports it at install/setup.
`~/.config/jira` and `~/.config/confluence` are no longer read.
`config::read_config_text()` lays the override's keys over the repo file (per
`[section] key`, via `merge_override`); `save_config_values` writes a key the
override sets back to the override (`WS_COMMANDS_OVERRIDE` moves the file,
empty = off). Rust only: Python readers still see the repo commands.toml.
Both commands.toml paths show in the status item menu and the view menu.
`check-overrides.sh --offer` (INSTALL.sh step 3b) creates the folder + a
starter override and offers to move/copy files still in the old places; the
app logs the same report at launch (`config::startup_config_report`) and
toasts if `~/.config/jira` / `~/.config/confluence` reappear.
**Menu-bar only:** Accessory activation policy + `LSUIElement`; `[app]
dock-icon = true` (or `WS_DOCK_ICON=1`) brings the Dock icon back. The status
item shows `menubar_icon.png` (bonsai silhouette, template image; redraw
with `bin/make-menubar-icon.py`, which also makes `app_icon.png`; shipped via `RESOURCE_FILES`).

## Common tasks

- **Add an `[app]` setting:** add the key to `commands.toml`, parse it in
  `app/config.rs` (`AppSettings`), validate (`config_value_problem`), write back
  through the codec. Never hard-code a user-facing string/size.
- **Add a keyboard shortcut:** in-window → `route_key_event` /
  `ui::popup::route_key` (respect rule 1) + a `[shortcuts]` row. Global
  Hyper+KEY → `~/.dotfiles/aerospace/aerospace.toml` binding (via `bin/ks SCRIPT`) (then `aerospace
  reload-config --dry-run --no-gui`), `hotkey::HOTKEY_MODES` + the host's
  `toggle_command`, `bin/kitchen_sink.sh`, `commands.toml [shortcuts]`, docs.
  Hyper+S / Hyper+X are the exception: they go to the quick agent
  (`app/quick.rs`, `quick::route_command`).
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
  main.
- **Helper pool starvation + retries.** One 8-worker pool serves interactive
  calls; treat helper replies as fallible; don't
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
- **Symbols over line numbers** — line numbers drift constantly.

## Known gaps (full list: `BACKLOG.md`)

- Hide-on-focus-loss: the setting is reported, but the Swift focus bridge
  (`ws-aerospace-focus` watcher) is not ported.
- AeroSpace re-show differs from Swift ("no jump back" in the focus suite).
- Vim search highlight (`VimMatchOverlay`/`hlsearch`): rows are ported for the
  sidebar, files, Jira and Confluence lists; text (`NSTextView` temporary
  background) and web (`CSS.highlights`) targets are still unported.
- `Cmd+K` action picker, `Cmd+/` shortcuts sheet, pane-shot preview window.
  (`do:jira-jump` and the `<leader>sf` pane finder
  are ported: `views/pane_find.rs`.)
