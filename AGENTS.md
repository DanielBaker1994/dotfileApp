# AGENTS.md — kitchen-sink agent context

Current, authoritative agent context for this repo. Docs drift; **the code is
truth**. Grep a symbol, then read ~60 lines around it. Never read
`PopupWindow.swift` or `kitchen_sink.swift` whole — they are 10k+ lines each.
`AGENT_CONTEXT.md` (`CLAUDE.md` symlink) is legacy and now points here; its old
deep-detail text is in git history (`git log -- AGENT_CONTEXT.md`).

## What this is (overview)

kitchen-sink is a single-instance macOS app (Swift + AppKit: 48 top-level
`*.swift`, ~47.2k lines) that runs as a hotkey-driven daemon: AeroSpace /
CLI launches talk to it over a Unix socket (`$TMPDIR/ws-notes.sock`, lock file
beside it), and one **shared window** hosts the views — notes, files, jira,
confluence, AI, compare — plus tool panels (screenshot, paths, terminal, view
switcher). It is a *regular* app (`NSApp.setActivationPolicy(.regular)`: menu
bar + status item), not an accessory/LSUIElement app; docs that say "menu-bar
app" mean the status item, not the activation policy. Python (`pylib/` plus
`jira/`, `confluence/`, `notify/`, `settings_hub/`) is the frozen "cold path":
the app talks to it over a persistent JSON-lines helper protocol. A full Rust
port lives in `rust/` (cargo workspace `ws-rs`, `ws-helpers`,
`swiftterm-shim`; objc2) mirroring every Swift file module-for-module; it
builds a signed, isolated `.app` at `.build/rust-app/` and answers the same
socket contract and `state`/`do:` grammar. **Swift is authoritative until the
Phase-4 parity cutover** (`PLAN-rust-port.md`) — port behind the parity gate,
run the Swift app for live behavior.

## Non-negotiable rules (condensed from `rule.md`)

1. **Edit shortcuts work in every text input.** Cmd+C/V/X/A/Z plus Ctrl+C/V;
   keys are also routed explicitly (`PopupWindow.handleKey` must forward edit
   shortcuts to the focused field editor; sheets route paste to their field
   editor and set `alert.window.initialFirstResponder`). `rule.md`'s framing
   ("accessory app with no Edit menu") is stale — the code is `.regular` and
   installs a full App/File/Edit/Window menu; the explicit routing still
   guards the popup windows' non-standard focus.
2. **Right-click menus** in file browsers / the terminal offer: copy absolute
   path, open in notes, open in default app, reveal in Finder.
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
   open and Esc closes only them (`PopupWindow.transientEscape`), never the
   window under them.
7. **Python is frozen for the Rust port.** Keep the helper protocol and
   `pylib/` behavior byte-compatible; don't rewrite Python unless there is a
   hard blocker.

## Build / run / test

```bash
./ws                        # interactive menu; ./ws help lists everything
./ws build                  # compile + relaunch (exit 0 + no output = OK)
./ws build --force          # rebuild even if up to date
./ws build --build-only     # build, don't relaunch (WS_BUILD_ONLY=1)
./ws dmg [--no-notarize]    # distributable .dmg -> .build/dist
./ws test [SUITE|all|ui|vim|focus|install]
./ws doctor [--fix]         # full-stack health check (jira/jira-doctor.sh)
./ws check [--json]         # preflight this Mac (bin/preflight.sh)
./ws permissions grant|fix  # TCC permissions
./ws links [check|fix]      # ~/.config symlinks
./ws home app|repo|stack|status
./ws fake confluence|jira [start|stop|status]
./ws monitors main|inverse|toggle|status
```

**Swift build — `bin/build-app.sh`** (`./ws build` wraps it, ensures the
signing identity, then relaunches): one hand-rolled `swiftc -O -swift-version 5`
over every top-level `*.swift` (`SWIFT_SOURCES_GLOB`), linking
`.build/SwiftTerm/libSwiftTerm.a` (prebuilt once; rebuilt only when its
sources change) and embedding Info.plist via `-Xlinker -sectcreate __TEXT
__info_plist`. Output is the repo-root `kitchen-sink.app`; `--dist` →
`.build/dist/kitchen-sink.app` (resources per `install.conf` `RESOURCE_*`,
rewritten `commands.default.toml`, compiled `helpers-bin/`). Signed with the
self-signed cert `SIGN_ID` ("kitchen-sink codesign") for stable TCC grants;
ad-hoc fallback loses them. **A non-dist build `pkill`s the running daemon
before swapping the binary** — don't build while you need the app up.
`bin/kitchen_sink.sh` is the hotkey cold-start path (build-if-stale, then
LaunchServices `open -n -g` so TCC attaches to the bundle, not the shell).

**Rust build — `bin/build-rust-app.sh`**: `cargo build --release` in
`rust/ws-rs` with `CARGO_TARGET_DIR=.build/rust-target`; output
`.build/rust-app/kitchen-sink.app` — isolated, never touches the live Swift
bundle. `--dist` → `.build/dist/` (same path as the Swift dist — they can
overwrite each other). Mode flags: `--stale`, `--force`, `--dist`; the live
app is killed only with `WS_RUST_KILL=1`.

**Tests.** `./ws test SUITE` → `bin/run-tests.sh SUITE`. `all` (the default)
runs the three Python suites with no case of their own
(`test_jira_poll.py`, `test_confluence.py`, `test_notifications.py`) plus
**every** `Tests/test_*.swift`; the other Python suites run only via their own
case (`config`, `fileops`, `jsonmgr`, `compare`, `helper`, `settings`,
`jira-*`, `confluence-*`, `shot-model`, `prose-pdf`, … — see the `case` in
`bin/run-tests.sh`). Python resolution is `/opt/homebrew/bin/python3` else
`python3`; the `helper` suite needs **Python ≥ 3.11** (override with
`WS_PYTHON`). Swift tests are plain scripts (some carry a
`// sources:` multi-file header and are compiled with `swiftc -O`); there is
no XCTest. Run one directly with `python3 Tests/test_x.py` or
`swift Tests/test_x.swift` (quoted header files must be compiled together).
Rust: `cd rust && cargo test` (workspace; ~500 tests, all green; uses
`rust/target`, which is gitignored) or `cargo test -p ws-rs`.

UI suites: `./ws test ui` (`bin/ui-test.sh`: cliclick + osascript; takes over
the live app, snapshots/restores `commands.toml`), `./ws test vim`
(`bin/ui-test-vim.sh`: needs `[notes] vim-mode = true` and nvim; aborts if
kitchen-sink isn't frontmost), `./ws test focus` (`bin/ui-test-focus.py`:
daemon must already be running; drives the real AeroSpace hotkey path; moves
the user's workspaces and restores them; ~20 s), `./ws test install`
(`Tests/test_install.sh`: needs `.build/dist/kitchen-sink.app`, throwaway
`$HOME`). These are slow/flaky — **run at most once, don't loop them**.

Parity between stacks: `bin/rs-parity.py --rust-sock <sock>` drives a scripted
`do:` sequence against both daemons and diffs `state`. (PLAN says
`./ws rs-parity`, but the `ws` script has no such case — call the script
directly.) The two daemons bind the same socket name, so run the Rust one
with a different `TMPDIR` to test both at once.

Fast UI verification (the `ui-check` skill, `.claude/skills/ui-check/SKILL.md`):

```bash
./ws build                                   # build + relaunch
S="$TMPDIR/ws-notes.sock"
echo state              | nc -U -w3 "$S" | jq '{view, visible, views: .views.notes}'
echo do:toggle-terminal | nc -U -w3 "$S" | jq '.views.notes | {terminal, frame, drawerInset}'
```

Assert only what the change affects, then put the user's live app back the
way you found it. Prefer `state`/`do:` hooks over osascript; if a needed
field is missing, add it to `PopupWindow.testState` / `SwitcherController.testQuery`.

Build/test gotchas:

- **TCC / Screen Recording cannot be scripted.** `ws permissions fix` does
  `tccutil reset ScreenCapture $BUNDLE_ID`, rebuilds, relaunches, and opens
  System Settings — a human must click Allow once. `bin/grant-permissions.sh`
  inserts rows into the TCC DB and needs the terminal to have Full Disk Access.
- **`borders` daemon chicken-and-egg:** JankyBorders draws the focus border but
  also perturbs window-chrome checks; `brew services stop borders` for
  deterministic UI/chrome assertions, restart after.
- **`PYTHONDONTWRITEBYTECODE=1` everywhere.** A `__pycache__` inside the signed
  bundle changes its contents and breaks the signature (and TCC); `main.swift`
  and the Rust `main.rs` set it at startup, both build scripts strip
  `__pycache__`, and `bin/ws-settings` runs `python -B`.
- `WS_PING_ONLY=1 <binary>` is the liveness probe (exit 0/1) used by
  `bin/kitchen_sink.sh` and tests.
- `WS_HOME` overrides the config home (tests use it); `WS_DEBUG=1` logs the
  launch path to `$TMPDIR/ws-launch.log`; `WS_JSON_ROOT` / `WS_JSON_TRACE=1`
  redirect the JSON manager.
- Stack repairs: `./ws doctor --fix` may rebuild a stale binary through an
  older ad-hoc path (its fallback in `jira/jira-doctor.sh`); prefer
  `./ws build` for real builds.

## Repo layout / code map

`~/.config/kitchen-sink` **is** this git checkout in repo mode. `isRepoBuild`
(commands.toml + bin/build-app.sh beside the executable) decides whether
config is read/written here or in the installed home; `$WS_HOME` or
`~/.config/kitchen-sink` otherwise. `.build/`, `kitchen-sink.app/`, `.agents/`,
`commands.toml.bak` are gitignored.

### Swift files (top level, grouped)

- **Host / app** — `main.swift` (top-level code: CLI one-shots
  `config-check` / `config-schema` / `prose`; `AppInstall.ensureHome()`;
  single-instance flock + forward; then `NSApplication` +
  `AppDelegate` + `run()`). `kitchen_sink.swift` — the bulk: `AppSettings`,
  `CommandSpec`, config load/validate, `SwitcherController` (daemon host,
  socket dispatch `testQuery`, `hotkeyModes`/`hotkeyPrep`, `ensureSlotMember`,
  `reloadConfig`, status menus), `AppDelegate`, `MenuTarget`,
  `ServicesHandler`. `SetupWindow.swift` (`AppInstall` + setup/preflight UI),
  `SwitcherStatus.swift` (status row via helper `status.gather`).
- **Window framework** — `PopupWindow.swift` (window classes
  `PopupBaseWindow` / `PopupPanel` / `PopupPlainWindow`, the master
  `PopupWindow` + chrome/tabs/filter/rows/file-browser/terminal parts,
  `handleKey`, key monitors, generic Unix-socket helpers), `SharedWindow.swift`
  (`SlotView`, `SlotMember`, `SlotHostWindow`, `SharedWindow` state machine),
  `CardWindow.swift` (`CardWindowController` base for card views),
  `PaneNav.swift` + `PaneGeometry.swift` (directional pane focus + ring),
  `VimKeys.swift` + `VimSearch.swift` (vim mode in focused panes),
  `ViewSwitcher.swift`, `SidebarJump.swift`, `TextEditKeys.swift`
  (app-wide Ctrl+W word-kill), `ProcessRun.swift`, `ConfigText.swift`.
- **Views / tools** — notes: `NotesProse.swift` (prose reading view +
  `ProseWindow` + `ProseProcess`), `NoteFindWindow.swift`, `InlineRename.swift`,
  `FilePopup.swift`; files: `FilePopup.swift` (browser also used in the notes
  drawer), `FileOps.swift`, `PathShelf.swift`, `PathsWindow.swift`,
  `RecentFiles.swift`, `ListFilter.swift`; jira: `JiraDashboard.swift`,
  `JiraSearch.swift`, `JiraBoard.swift`, `JiraTicket.swift`; confluence:
  `Confluence.swift`; AI: `AIWindow.swift`, `AIFormat.swift`; compare:
  `CompareWindow.swift`, `CompareText.swift`, `CompareFolder.swift`,
  `ComparePane.swift`, `CompareFolderView.swift`; screenshot: `Screenshot.swift`,
  `ScreenshotOverlay.swift`, `ScreenshotAnnotations.swift`, `ScreenshotPin.swift`,
  `ScreenshotText.swift`, `PaneShot.swift`, `AnsiRender.swift`;
  `TerminalPanel.swift`, `PopupInspector.swift`.
- **Config files** — `commands.toml` (all user-facing config; see Architecture),
  `install.conf` (app name/bundle id `dev.danielbaker.kitchen-sink`, signing,
  SwiftTerm pin/dir, resources, brew deps, caches, MACOS_MIN 26.7),
  `Info.plist`, `entitlements.plist`.

Don't guess symbols — the stable anchors are: `AppSettings`, `parseAppConfig`,
`CommandSpec`, `loadCommands`, `validateConfig`/`configValueProblem`,
`saveConfigValue`, `SwitcherController`, `testQuery`, `handleKey`, `PopupConfig`,
`SharedWindow`, `SlotView`, `SlotMember`, `PaneNav`, `VimKeys`, `THEME`,
`PopupColors`, `ThemePreset`, `windowColors`, `PythonHelper`.

### Rust workspace (`rust/`)

- `ws-rs` — the app binary `kitchen-sink` (~46.7k lines; objc2 0.6.5, block2,
  dispatch2, serde, libc, rmpv; **no tokio** — std threads). Modules mirror
  Swift: `app/{host, socket, config, paths, python_helper, hotkey, menu,
  registry, process_run}.rs`; `engines/` (ai_format, ansi_render, config_text,
  file_ops, list_filter, nvim_rpc, pane_shot, path_shelf, recent_files,
  screenshot_annotations/text, switcher_status, text_edit_keys);
  `panes/` (pane_nav, pane_geometry, vim_keys, vim_search);
  `ui/{popup, shared_window, card, chrome, theme}.rs`; `views/` (notes, jira,
  confluence, ai, compare, screenshot, files, paths, inspector, switcher,
  setup, terminal).
- `ws-helpers` — `dock_badges` + `webex_unread` AX helpers (replace
  `notify/helpers/*.swift`); tested, but `bin/build-rust-app.sh --dist` still
  compiles the Swift helpers (`HELPER_SOURCES`) — not yet wired in.
- `swiftterm-shim` — the one accepted Swift exception: a tiny `NSView` wrapper
  (`WSShim`) around the pinned `libSwiftTerm.a`, built by its `build.rs`.

### Python packages

- `pylib/` — engines + codecs: `helper/` (the JSON-lines worker; see
  Architecture), `jsonmgr.py`, `config_text.py` (the commands.toml codec),
  `compare_*`, `file_ops`, `ai_format`, `ansi`, `shot_model`, `paneshot`,
  `shelf`, `prose_pdf`, `doc_templates`, `status`, `ignore_rules`,
  `jira_*` / `confluence_*` glue, plus the shipped JSON homes.
- `jira/` (poller CLI + API + fake server + `com.jira.poll` launchd agent),
  `confluence/` (API/CLI), `notify/` (Webex/unread + AX helper sources),
  `settings_hub/` (`ws-settings` CLI/TUI; edits commands.toml
  line-preservingly). The app runs these through helper `script.run` (or direct
  spawn for long jobs); they are also standalone CLIs.

### Other dirs

`bin/` (build/test/install tooling — see above; `lib.sh` for `ws_*` helpers;
`preflight.sh`, `grant-permissions.sh`, `setup-home.sh`, `symlinks.sh` at
root); `Tests/` (plain-script tests + `helper_fixtures/`, `snippet_render/`,
`test_install.sh`); `config/` (aerospace + borders rc, per-file symlinks from
`~/.config/<name>`, plus `paths.ignore`); `rules/` (AI rule markdown),
`templates/` (`professional-report.md`), `samples/`, `vim/` (nvim config +
snippets for the notes pane), `patches/` (SwiftTerm patch only),
`.claude/skills/` (repo skills incl. `ui-check`; `.agents/` is gitignored).

### Docs

`rule.md` (non-negotiables), `PLAN-rust-port.md` (port status + cutover gate),
`PLAN-json-manager.md`, `PLAN-data-out-of-code.md`, `PLAN-py-core.md`,
`PRD-*.md` (`PRD-keyboard.md` = pane/vim spec; `PRD-compare.md`; …),
`REVIEW-architecture.md` (known IPC/debt findings), `FINDINGS-*.md`,
`BACKLOG.md`, `PRODUCT.md`, `DESIGN.md`, `README.md`.

## Architecture

### Host, single instance, launch

`main.swift` handles CLI modes before AppKit; `AppInstall.ensureHome()`
maintains the `~/.config/kitchen-sink` home; `acquireDaemonLock` flocks
`<socket>.lock` and a second launch forwards its message to the running
instance and exits. The daemon is the GUI app itself (no headless process).
`AppDelegate.applicationDidFinishLaunching` installs the main menu, the app-wide
text-key monitor (`TextEditKeys.route`), the status item
(`installStatusMenus`), then `SwitcherController.start()` (command server,
focus bridge, prewarm).

### Shared-window model

One visible host frame, many views: `enum SlotView` (`notes, files, jira,
detail, releases, config, output, confluence, ai, compare, compareText`) and
`protocol SlotMember` (`slotWindow`, `slotShown`, `slotBaseFrame`, `slotPark`,
`slotShow`, `slotAttach`, `slotDetach`) implemented by `PopupWindow` and
`CardWindowController`. `SharedWindow` (owned by `SwitcherController.slot`) is
the state machine: `open/push/back/home/present/hide/cycle/hotkey/toggle`;
`present` parks the outgoing member, attaches the incoming one into
`SlotHostWindow`, and preserves the frame; `prefixKey` implements the Ctrl+B
prefix. Views are created lazily by `SwitcherController.ensureSlotMember`
(`.confluence` → `ConfluenceWindow.create`, `.ai` → `AIWindow.create`,
`.compare` → `CompareWindow.create`, notes/files/jira via command/`PopupConfig`
windows). Views are **hardcoded** in `SlotView`, not declared in config;
`commands.toml` declares *commands/windows* (`[notes]`, `[files]`, `[jira]`, …
with `enabled = true` → `CommandSpec` via `loadCommands`). `CardWindowController`
is the base for card views (AI/Confluence/Compare/JiraDashboard):
`routeKey` / `editKey` / `themedRoot` / `iconMenu` / `closeOrHide`.

### Socket contract (the daemon API)

`$TMPDIR/ws-notes.sock` (`[app] notes-socket`), one newline-terminated request
per connection. Server loop and dispatch live in
`SwitcherController.startCommandServer`; helper clients are `sendRequest` /
`sendLaunchMessage`. Dispatch order: `ping` (no reply) →
`screenshot-permission` (`granted|denied`) → `screenshot` → `compare` →
`pane-shot` → `reload`/`restart` (JSON) → **`state` / `do:ACTION`** → anything
else is a launch/hotkey message. `state` returns one JSON line;
`do:ACTION` executes and then returns the **same full state JSON**
(`testQuery`, main thread, 2 s semaphore); unknown actions return
`{"error":"unknown action …"}`. `state` keys include: `view`, `visible`,
`active`, `keyWindow`, `windows`, `views.<view>` (`shown`, `key`,
`frame [x,y,w,h]`, plus per-view extras: `drawerInset`, `terminal`, `browser`,
`findBar`, `pane`, `tabs`, `selectedTab`, `responder`, `selection`,
`rowCount`, `query`), `hideOnFocusLoss`, `escHides`, `headerStyle`, `pane`,
`screenshot`, `compare`, `paneShot`, `paths`, `tools.<name>`, `palette`,
`paletteCommands`, `viewSwitcher`, `terminalPanel`, `activations`,
`frontmostPid`, `pid`. Useful `do:` actions: `cycle`, `cycle-back`, `hide`,
`back`, `home`, `toggle`, `open:<view>`, `toggle-terminal`, `term`,
`notes-find`, `jira-jump`, `switcher`, `reset-size`, `pane:h|j|k|l`,
`pane:focus:<id>`, `key:<spec>`, `header-style:<style>`, `esc-hides:<view>:on|off`,
`tool:<name>` / `tool-close:<name>`, `screenshot:*`, `compare:*`, `paths:*`.

**Hotkey fast path.** Messages in `SwitcherController.hotkeyModes` (`window`,
`notes`, `voice`, `jira`, `files`, `confluence`, `ai`, `compare`) make the
socket thread run `hotkeyPrep()` before hopping to main: Aerospace IPC in
parallel (`list-windows --focused` → focus file, `list-windows --all` →
focused workspace/screen, cache clear, move stray windows), then
`applyHotkeyPrep` and `toggleCommand(name)`. Latency logs as
`hotkey <name>: prep … ms to shown`. Tool-panel hotkeys (Hyper+X screenshot)
are *not* in `hotkeyModes`. A focus bridge file (`ws-aerospace-focus`, written
by Aerospace's `on-focus-changed`, watched via DispatchSource) complements
`NSApp.isActive && keyWindow` for hide-on-focus-loss.

### Keyboard / focus

Every shown `PopupWindow` installs a local `keyDown` monitor →
`PopupWindow.keyInterceptor` first (this is `SharedWindow.prefixKey`: Ctrl+B
prefix, Ctrl+H/J/K/L pane moves, VimKeys, sidebar keys) → Esc +
`PopupWindow.transientEscape` → top-accessory handling → `PopupWindow.handleKey`,
whose order is: prose/overlay keys → window-size keys (Cmd+± resets the Esc
streak) → modified keys (Cmd/Ctrl: sheet edit keys → Ctrl+Tab tab cycle →
`Cmd+\` rail → Ctrl+Shift+HJKL pane resize → vim/terminal/host/file-browser
routers → generic edit keys) → list/edit keys. Ctrl+B is a 1.5 s prefix: L =
previous view, W = view switcher, T = terminal/prose toggle, B = sidebar rail;
Ctrl+B twice passes the real key through. Esc behavior: N rapid presses (<0.6 s
apart) per `escCloseCount`/`esc-close` (0 = never); the switcher palette always
closes on one Esc. `PaneNav` (`NavPane`/`PaneProvider`/`PaneGeometry`) moves
the silver focus ring between panes; `VimKeys` gives the focused pane
normal/insert/search modes with `VimRows`/`VimTarget` targets.

### Theme

`[theme]` → globals `THEME`, `BAR`, `GROUP_BG`, `TEXT`, `DIM`, `BORDER`,
`ACCENT`, `THEME_PALETTE`; `parseTheme`, then `windowColors(cmd)` merges
per-command color overrides into `PopupConfig`. `[themes]` presets
(`ThemePreset`, ~30 built-ins incl. Nightfox = current default); applying one
(`applyThemePreset`) persists the hex values back into `commands.toml`. Draw
with `PopupColors`/`PopupPalette` derived tokens, never system colors. Header
styles (`HeaderStyle`) via `[app] header-style`.

### Python boundary

`PythonHelper.shared` (Swift) / `app/python_helper.rs` (Rust) spawn
`python3 -B -m helper` with cwd/path `assetDir/pylib`, `PYTHONPATH=pylib`,
`PYTHONDONTWRITEBYTECODE=1`; interpreter discovery prefers `WS_PYTHON` then
brew pythons, requiring ≥ 3.11 (never the CLT stub). Protocol: one JSON object
per line — request `{"id", "method", "params"}` → reply
`{"id", "ok", "result"}` or `{"id", "ok": false, "error": {"message",
"traceback"}}`; out-of-order replies matched by id; an 8-worker thread pool;
`shutdown` drains; a broken engine module becomes `_Unavailable` instead of
killing the worker. Engines with state (`compare.*`, `folder.*`, `fileops.*`,
`ignore.*`) are handle-based: methods return a handle id, later calls pass it;
state lives in the worker. `script.run` runs the Python CLIs
(`jira/jira_poll.py`, …) as children with `sys.executable -B`, cwd = the
package dir. `ConfigText.swift` uses the helper (`config.decode`,
`config.section_entries`, `config.line`, `config.setting`, `config.tri`,
`config.resolve_binary`) and caches decodes as
`~/.cache/kitchen-sink/config-<hash>.json`; the Rust side has a real port
(`engines/config_text.rs`, `app/config.rs`).

**jsonmgr** (`pylib/jsonmgr.py`) is the one manager for shipped JSON homes:
`load(name)` (cached by mtime+size; failures never cached), `get`/`field`,
`JsonError`, an access ledger, `WS_JSON_TRACE=1`, and
`python3 pylib/jsonmgr.py --report [--json] [--check]`. Rule: never bare
`json.load` a shipped home — route it through `jsonmgr.load("<repo-relative
path minus .json>")`; user/runtime data keeps its own I/O.

### Config (commands.toml)

Flat `[section]` tables, one `key = value` per line: `[app]`, `[theme]`,
`[themes]`, per-view sections (`[notes]`, `[files]`, `[jira]`, `[confluence]`,
`[ai]`, `[compare]`, `[screenshot]`, `[pane-shot]`, `[paths]`,
`[notes-find]`, `[filefast]`), `[icons]`, `[shortcuts]` (keyboard-shortcut
sheet rows, parsed in order), `[terminal]`, `[prettyprint]`,
`[health-checks]`, `[setup]`, `[settings-hub]`, … Swift has no TOML parser —
reads go through the Python line codec (`ConfigText.swift` → helper), writes
through `config.line`/`config.setting` applied by `writeConfigText` (validation
gate; invalid current file parked as `commands.toml.broken-<epoch>`; `.bak`
auto-refreshed; atomic write). Write path: repo build → **this repo's**
`commands.toml`; installed app → `~/.config/kitchen-sink/commands.toml`;
`main.swift` exports `WS_COMMANDS_CONF` for Python readers when not a repo
build. `[app]` holds globals like `notes-socket`, `esc-close`,
`hide-on-focus-loss`, `header-style`, `vim-keys`, socket/paths/sizes.

### Where the two stacks meet

Socket `state`/`do:` grammar (byte-compatible); the launch-message verbs
(`window`, `compare\t…`, `pane-shot\t…`, `reload`/`restart`, `config-check`/
`config-schema`); `commands.toml` codec and `[app]` semantics; `ws` command
names; the JSON-lines Python helper; and `bin/rs-parity.py`, which diffs
`state` JSON between the two live daemons.

## The Rust port

Status: Phase 0–3 complete — every Swift file has a module counterpart, the
crates build a signed runnable `.app` to `.build/rust-app/`, and the daemon
answers the socket contract. ~500 tests are green (495 in `ws-rs`; measured
Oct 2026 — PLAN's "~471" is stale). The port plan and the Phase-4 cutover
checklist are in `PLAN-rust-port.md`; the feasibility PoC is
`/tmp/rust-ui-test/PORTING-TO-RUST.md` (ephemeral /tmp copy).

Remaining seams (real `todo!()` invocations, all documented in place):

- `ui/popup.rs` — `install_local_monitor` (NSEvent keyDown monitor wire-up).
- `views/screenshot.rs` — JPEG encode (`NSBitmapImageRep`); deep AppKit
  overlay drawing (`ShotSession::render` etc. — models/state are real).
- `views/notes.rs` — floating `ProseWindow` (WKWebView) and spawning
  `kitchen-sink prose`.
- `views/ai.rs` — `runPart` streaming `fm respond --stream`.
- `views/jira.rs`, `views/files.rs`, `views/paths.rs` — window/NSView tree
  builders (models complete and tested).
- `engines/ai_format.rs` — `RichText::rtf`/`copy` (pasteboard paths).
- `main.rs` — `config-schema` prints `{}`; `prose` one-shot not ported;
  no `__info_plist` embedding yet (needs a `build.rs` link arg).

Parity gate: Rust `testQuery` must emit identical `state` JSON and `do:`
behavior so `bin/ui-test*.sh`, `ui-test-focus.py`, `run-tests.sh` suites and
`test_install.sh` all pass against the Rust binary, with zero parity diffs
(`bin/rs-parity.py --strict`); then `./ws`/`bin/build-app.sh` flip to cargo,
`notify_poll.py` moves to the Rust AX helpers, and the Swift tree is deleted
(SwiftTerm shim stays).

## Common tasks

- **Add an `[app]` setting:** add the key to `commands.toml` + parse it in
  `parseAppConfig`/`AppSettings` (`kitchen_sink.swift`; add numeric ranges to
  `configNumberKeys`); validate in `validateConfig`; write back via
  `saveConfigValue`. Mirror it in `rust/ws-rs/src/app/config.rs`. Never
  hard-code a user-facing string/size.
- **Add a keyboard shortcut:** in-window → `PopupWindow.handleKey` (respect
  the documented order and rule 1) + a `[shortcuts]` row. Global Hyper+KEY →
  checklist: `config/aerospace/aerospace.toml` binding (then
  `aerospace reload-config --dry-run --no-gui`, apply `--no-gui`);
  `SwitcherController.hotkeyModes` + `toggleCommand`; `main.swift`
  cold-start mapping + `bin/kitchen_sink.sh`; `commands.toml [shortcuts]`;
  docs. Tool-panel hotkeys stay out of `hotkeyModes`.
- **Add a view:** extend `SlotView` + header nav ids (60–67 region),
  `SwitcherController.ensureSlotMember`, conform to `SlotMember`, add the
  `[section]` + `enabled` gate; the Rust hub is `app/registry.rs` (frozen
  interface — register there too).
- **Add a socket `do:` hook:** dispatch in `SwitcherController.testQuery`
  (Swift) / `do_host_action` + `registry.dispatch_test_do` (Rust); expose any
  new state in `testState`/`testQuery` (and the Rust `state_with_registry`) so
  tests can assert on it; use `echo do:... | nc -U -w3 $TMPDIR/ws-notes.sock`.
- **Verify UI changes fast:** the `ui-check` workflow above (build → query the
  live socket → assert → restore). Add missing state fields rather than
  scripting UI. Real key/mouse paths need synthetic input — `osascript`
  System Events key codes (cliclick may not reach apps from an agent session).
- **Repair the stack:** `./ws doctor [--fix]`, `./ws check`, `./ws permissions
  fix`, `./ws links fix`, `./ws home status`. Logs: `$TMPDIR/ws-launch.log`,
  `/tmp/ws-debug.log`, `~/.cache/ws-crash.log`, `~/.cache/ws-aero.log`.

## Gotchas / footguns

- **Key routing order is load-bearing.** `keyInterceptor` (prefix/pane/vim)
  runs before everything; `transientEscape` must be set while any overlay
  (rename field, picker) is up and cleared when it closes; the Esc streak
  counter resets on any non-Esc key. Never swallow an edit shortcut without
  forwarding it.
- **Main-thread discipline.** No blocking helper `callSync` on interactive or
  draw paths — `REVIEW-architecture.md` documents the compare-draw marks path
  doing exactly that (freezes up to the timeout). Cache, prewarm, or go async.
- **Helper pool starvation + retries.** One 8-worker pool serves interactive
  calls and multi-minute `script.run` jobs; on worker restart every pending
  call is retried once — including non-idempotent `fileops.*` mutations
  (known debt). Treat helper replies as fallible; don't memoize failures.
- **Window setup ordering.** `PaneNav.refresh` runs on the next async turn,
  before AppKit re-layouts collapsed sidebars — the ring can keep a stale width
  (`BACKLOG.md`). Drawers: `drawerInsetNow` only counts growth.
- **JankyBorders skips `NSPanel`s / non-normal window levels** (floating
  tags). A window that should show the focus border must be a plain `NSWindow`
  at `.normal` level — see `FINDINGS-borders-pdf-window.md`; stop the `borders`
  service for deterministic chrome checks.
- **Config writes can silently no-op** if the Python `config.setting` call
  times out under helper load; invalid edits are parked, `.bak` is the
  last-known-good. In repo mode you are editing this checkout's
  `commands.toml` — UI tests snapshot/restore it; don't leave it dirty.
- **Sign/TCC.** Ad-hoc signing loses TCC grants; `ws permissions` expects the
  self-signed cert. `grant-permissions.sh` needs Full Disk Access. Screen
  Recording needs one manual Allow (`ws permissions fix` opens the pane).
- **`__pycache__` in a signed bundle breaks it** — keep
  `PYTHONDONTWRITEBYTECODE=1` / `-B` for every python spawn; build scripts
  strip it defensively.
- **`./ws build` kills and relaunches the app**; UI tests take it over, steal
  focus, and mutate `commands.toml` (restored). Don't run them concurrently
  with other app work; run once.
- **SwiftTerm is read-only** (fetched into `../SwiftTerm`); changes go in
  `patches/`. Never rebuild `build_term_lib` unless asked.
- **Python is frozen** for the port; keep `pylib/`, `jira/`, `confluence/`,
  `notify/`, `settings_hub/` behavior and the helper protocol intact.
- **Symbols over line numbers** — line numbers drift (they do, constantly).

## Unresolved / uncertain (don't trust these without checking)

- `SidebarJump.swift` has no clearly named Rust module — possibly folded into
  another view module, unverified.
- `ws-helpers` binaries are tested but **not yet bundled**: `--dist` still
  compiles `notify/helpers/*.swift` via swiftc.
- PLAN references `./ws rs-parity`; no such `ws` case exists — use
  `bin/rs-parity.py` directly, and give the Rust daemon its own `TMPDIR` to
  avoid socket-name collisions.
- `bin/ui-test.sh`'s `/tmp/ws-test` fixture (section ~20) has no creator in the
  repo; that section fails unless the dir pre-exists.
- Doc drift found: "menu-bar app" (it's a regular app + status item); `rule.md`'s
  "accessory app with no Edit menu" (the code is `.regular` and installs a full
  App/File/Edit/Window main menu); PLAN's "~471 tests" (now ~500); the refresh
  prompt's "~55 top-level Swift files" (48 files, ~47.2k lines).
