# Refactor plan: port kitchen-sink from Swift/AppKit to Rust

Local plan (the GitHub issue is skipped by request, matching
`PLAN-swift-to-python.md`). Owner decisions are locked below.

Reference: `/tmp/rust-ui-test/PORTING-TO-RUST.md` (working PoC — `objc2` drives
the same AppKit objects, including the private `_cornerRadius` override, glass,
`drawRect:` and target/action).

## Progress

**Iteration style superseded (Oct 2026):** the remaining work now follows
`PROMPT-rust-port-phase4c.md` — finish the code port first, run every UI
suite once in a single final batch, then cut over. Do not loop the gate
suites mid-wave.

Every Swift file now has a Rust counterpart under `rust/`; 598 Rust tests
green; the crate builds a signed, runnable `.app` whose daemon **shows the
shared window** (a `SlotHostWindow` with the `PopupChrome` header + the view
switcher) on start and on every launch/`do:open:*`, and answers the socket
contract. Python is untouched.

- [x] Phase 0.1 — `rust/ws-rs` workspace (members `ws-rs`, `ws-helpers`,
      `swiftterm-shim`) + module tree mirroring the Swift files;
      `bin/build-rust-app.sh` builds a signed, verifiable `.app` to
      `.build/rust-app/` (isolated; never touches the live Swift bundle).
- [x] Phase 0.2 — `PythonHelper` Rust client (real `ping` round-trip against the
      frozen `pylib` helper) + `ProcessRun`; `app/paths.rs`, `app/hotkey.rs`,
      `app/config.rs` (AppSettings/CommandSpec/validate/save).
- [x] Phase 0.3 — `ui/theme.rs` (PopupColors/Palette/Tone, HeaderStyle,
      30 ThemePreset incl. Nightfox, theme globals, NSColor bridge).
- [x] Phase 0.4/0.5 — `ui/popup.rs` (`_cornerRadius` subclass, pure `route_key`),
      `ui/card.rs`, `ui/chrome.rs` (all HeaderStyles drawn),
      `ui/shared_window.rs` (state machine + `SlotHostWindow`),
      `panes/pane_nav.rs`, `panes/vim_keys.rs`.
- [x] Phase 0.6 — `app/socket.rs` (Unix command server + `DaemonLock`) and
      `app/host.rs` (`SwitcherController` implements `CommandHandler`:
      `state`/`do:`/`reload`/launch; `register_views` wires the views).
      `main.rs` runs a real daemon; verified over a live socket: `state` (0
      missing keys vs the Swift daemon), `do:open:notes`, `reload`, `ping`.
- [x] Phase 0.7 — `swiftterm-shim` crate (`_cornerRadius`-style ObjC bridge) +
      `views/terminal.rs`; linked against the pinned `libSwiftTerm.a` with the
      `/usr/lib/swift` rpath; no-op path also builds.
- [x] Phase 0.8 — `bin/build-rust-app.sh` (cargo → signed `.app`).
- [x] Phase 0.9 — `bin/rs-parity.py` (drives both daemons, reports missing
      `state` keys + scalar mismatches).
- [x] Phase 1 — engines: pane_geometry, vim_search, list_filter, text_edit_keys,
      screenshot_annotations, screenshot_text (real Vision OCR), config_text,
      file_ops, ai_format, switcher_status, ansi_render (real CoreText),
      pane_shot, recent_files, path_shelf, nvim_rpc (real msgpack-RPC).
- [x] Phase 2/3 — views: notes/prose, jira (dashboard/search/board/ticket),
      confluence (real WKWebView + `wsconf://` handler), ai (real WKWebView
      preview), compare (text + folder mirrors), screenshot (real
      ScreenCaptureKit capture), files, paths, inspector, switcher, setup,
      terminal.
- [x] `ws-helpers` — `dock_badges` + `webex_unread` ported to Rust (objc2 AX);
      verified byte-identical stdout vs the Swift helpers on live data.
- [x] The daemon **shows the window** (`app/host.rs` `UiCommand` queue + the
      `daemon_ui` bridge; `main.rs` installs it): a real `SlotHostWindow` with
      a `PopupChrome` header, the view-switcher buttons (real
      `navClicked` routing), the ✕/Cmd+W close path, the AppKit main menu + a
      status item, `.regular` activation policy. The content area embeds the
      ported surfaces as they land (`views::*::build_content`): notes
      (sidebar + editor), files (browser + info pane), jira (tabs + list +
      detail), compare (two-pane diff + thumbnail/editor/folder-tree classes),
      confluence (sidebar + search strip + results + live `wsconf://` preview)
      and ai (rules/input pane + live preview web view).
- [x] Remaining view seams from the last wave: `views/jira.rs` `build`,
      the `views/compare.rs` drawing layer (`ComparePaneView`,
      `FolderTreeView`, `CompareEditor`, `CompareThumbnail`),
      `views/{notes,files,paths}.rs` window surfaces (+ `ProseWindow`/
      `ProseProcess`, and the `prose` CLI one-shot), `views/ai.rs`
      `runPart` streaming `fm respond --stream` (+ `poll_stream`), and the
      `views/screenshot.rs` overlay panels + JPEG encode + pasteboard copy.
- [x] `config-schema` JSON (semantically identical to the Swift output; the
      only byte diff is Swift's long-form float for the `split` range),
      full palette-command population (`paletteCommands` now matches the live
      Swift daemon exactly), `__info_plist` embed via
      `rust/ws-rs/build.rs` `-sectcreate`.
- [x] The keyboard spine: `ui/popup.rs` `install_local_monitor` is a real
      local `NSEvent` keyDown monitor; the daemon UI installs it and runs the
      pure `route_key` against the current view's `escHideCount`, keeps the
      0.6 s Esc streak (hides on close) and forwards Ctrl edit ops down the
      responder chain. Embeddable content for confluence/ai
      (`views::{confluence,ai}::build_content`, wired in
      `host.rs::content_for`); the screenshot overlay key monitor
      (`views/screenshot.rs`: local keyDown -> `ShotSession::handle_key` /
      `PinPanel`); `ai_format` `RichText::rtf`/`copy` pasteboard paths; the
      notes `ProseProcess::launch` pop-out control.
- [x] Paths tool window + `tool:paths` (the `do:paths:show|hide|return|select:N`
      grammar and the `[paths] enabled` gate), notes drawers
      (`do:toggle-terminal` / `do:toggle-browser` + `views.notes.terminal` /
      `.browser` / `.drawerInset`), window titles (`cmd.windowName`), live
      frame / wid / key / window-count / activations state, the `show` palette
      + Ctrl+B W view switcher (two `ViewSwitcherPanel`s; the full command
      palette stays a spine TODO), the Ctrl+B prefix + Ctrl+HJKL pane moves,
      and the notes nvim pane (`views/notes_vim.rs` + the `notes.rs`
      integration: spawn, `open:`, the Esc policy, edit-shortcut translation,
      `:q` relaunch, the `checktime` watcher poll).
- [ ] Remaining before the Phase 4 cutover:
      - vim-suite leftovers: pasteboard-image paste (`![](assets/…)` + the
        inline-image flow), list-key execution on real focused lists, the full
        command palette (workspaces + commands), and the `ui-test-vim.sh`
        checks that encode older defaults (`esc-close = 2` vs the shipped
        `esc-close = 0`; `dd` -> clipboard vs init.lua's black-hole maps);
      - `ui-test-focus.py`: the AeroSpace `on-window-detected` rule (tiles +
        moves the shared window to ws 2) vs the suite's floating expectation,
        and the compare view's `current`/`sections` state + session flow;
      - run the `code-review` + `deslop` skills over the branch;
      - flip `./ws`/`bin/build-app.sh` to cargo, migrate `notify_poll.py` to the
        Rust AX helpers, delete the Swift tree.

Wave notes (Oct 2026): fresh-start `state` has 0 missing keys vs the Swift
daemon; `paletteCommands` / `escHides` / `hideOnFocusLoss` / `headerStyle`
match; `reload` reports `loadCommands().count` (10 for the shipped config) and
the palette parses `in-palette` purely (no helper needed at cold start);
`view`/`visible` differ by design while the Rust daemon presents the default
view (Swift stays hidden until asked). Verify with an isolated `TMPDIR` so the
live Swift daemon is untouched. `config-schema` is semantically identical to
the Swift output (Swift prints `0.2`/`0.8` in long form; we print the shortest
round-trip).

Wave notes (next wave): verified live on the signed `.build/rust-app` bundle
with an isolated `TMPDIR` — `state` has 0 missing keys vs the Swift daemon
(only the by-design fresh-start `view`/`visible`), `do:open:confluence` /
`do:open:ai` present the new embeddable surfaces, Cmd+W hides via the menu and
one Esc with `do:esc-hides:notes:on` hides through the local key monitor
(`esc-close = 0` in the shipped config means Esc consumes without hiding,
matching Swift). 549 tests green (544 ws-rs + 5 helpers); `cargo build`
warning-free.

Wave notes (Phase-4 gate wave, Oct 2026): the gate suites now run against the
Rust binary with an isolated `TMPDIR` (the owner's Swift daemon is untouched):
- `bin/rs-parity.py --strict` over a 30-action `do:` sequence (open/cycle/hide/
  drawers/term/switcher/paths/tool/esc-hides/pane/header-style/compare/
  confluence/ai): **0 missing keys, 0 nested mismatches** — the harness now
  also diffs the dotted contract keys the UI suites assert (`palette`,
  `viewSwitcher.shown`, `terminalPanel.shown`, `paths.shown`,
  `views.notes.terminal`, `views.{notes,files,jira}.shown`); `views.notes.browser`
  is excluded because the Swift daemon has no `do:toggle-browser`.
- `bin/ui-test.sh`: **79 passed, 0 failed, 2 skipped** (WS_BIN override,
  pid-scoped AX helpers, self-created `/tmp/ws-test` fixture, `enabled = true`
  in the appended `[files-test]`).
- `Tests/test_install.sh` against a `--dist` Rust bundle: **55 passed, 0 failed**.
- `./ws test all` (the Python + Swift contract suites): green.
- `bin/ui-test-vim.sh`: **30 passed, 16 failed** with the nvim pane landed —
  the suite's `WSPID`/`wframe`/`wcount`/`vim_pid`/`front()` helpers were made
  pid-scoped / real-cmdline (they were ambiguous across two `kitchen-sink`
  stacks); the remaining failures are the image-paste feature (unimplemented),
  focus races during the run (Cmd+V/A/Z/C verified working live), and stale
  expectations (`esc-close = 2` vs the shipped 0; `dd` -> clipboard vs
  init.lua's black-hole maps; the `--embed` pgrep).
- `bin/ui-test-focus.py`: **19 passed, 8 failed** (the AeroSpace
  `on-window-detected` rule vs the floating/focused-workspace expectation; the
  compare view's `current`/`sections` state); its `kept()` crash on empty
  sessions was guarded.
Also landed this wave: `state.palette` as a bool, real screenshot/compare
controller state, `WS_BIN` overrides in `kitchen_sink.sh`/`ui-test*.sh`/
`ui-test-focus.py`, the `WS_RS_ASSET_DIR` asset override for non-dist dev
bundles, and the `views/notes_vim.rs` pane (586 Rust tests, warning-free).

Wave notes (Oct 2026, code-first handoff): 598 ws-rs tests green, warning-free.
Landed: `do:compare:open[:sub]:L|R` + `compare:back` + Esc-from-compareText,
the compare close-confirmation sheet (`state.compare.sheet`,
`close-session[:force]`/`sheet-cancel`), dirty-edit semantics for
`compare:edit` (real undo edit), folder `open`/`drop`/`summary` gaps, vim
image paste (`save_pasteboard_image` + `VimPane::paste`, `nvim_paste` first),
`:q` relaunch refocus (`TerminalAutoRestart` mirror), and the `do:screenshot:*`
main-thread hop (`MainQueue` + 2 s budget). Suite findings (fix in the final
batch, do not chase now): `bin/ui-test-vim.sh` was 44/46 on its clean run and
its Esc section sends `esc-hides:` without the `do:` prefix (silently
ignored); `bin/ui-test-focus.py`'s AeroSpace "floating / focused workspace"
expectations are stale vs the owner's `aerospace.toml` rule (both the live
Swift and Rust windows land on workspace 2) — update the suite, not the app.

## Problem Statement

The app is ~47k lines of Swift (`PopupWindow.swift` 11.5k, `kitchen_sink.swift`
10.2k, 53 more files) built by one hand-rolled `swiftc` invocation in
`bin/build-app.sh`. The PoC proves Rust can drive AppKit to the same depth, but
the port is enormous and the two biggest files are a shared critical path that
everything depends on. We want a single Rust binary as the final artifact,
developed without breaking the live Swift app.

## Solution

Stand up a Rust workspace in-repo (`rust/ws-rs`) whose modules mirror the Swift
filenames. Freeze a **spine**: app host, config access, Python client, theme,
window framework base, registries, socket contract, SwiftTerm shim, build and a
parity harness. Then port every view in aggressive parallel against the frozen
interfaces. Keep building and shipping the Swift app throughout; **cut over only
when the Rust binary passes the full parity gate**, then delete Swift.

Rust and Swift are both ObjC objects, so the Rust binary and the Swift app can
share the same socket contract and be diffed (`state` JSON) during development.

## Commits

### Phase 0 — Spine (serial critical path; additive under `rust/`)

1. `rust/ws-rs` workspace + deps, module skeleton mirroring the Swift files with
   stubs -> `cargo check` green.
2. Config access + `PythonHelper` Rust client + `ProcessRun` (Foundation-only)
   ported 1:1, with Rust equivalence tests.
3. Theme (`THEME`/`BAR`/`GROUP_BG`/`TEXT`/`DIM`, `PopupColors`, `HeaderStyle`,
   `ThemePreset`).
4. `PopupConfig` + `PopupBaseWindow`/`PopupPanel`/`PopupWindow` base + `handleKey`
   core (the documented key order) + `PopupChrome`.
5. Registries + `SharedWindow`/`SlotView`/`SlotMember` + `CardWindow`: nav icons,
   palette commands, pane providers, vim targets, `do:`/`state` dispatch.
6. Socket server + `state`/`do:` envelope + launch messages + daemon lock +
   `hotkeyPrep`/AeroSpace IPC + menu bar.
7. SwiftTerm shim crate + terminal plumbing interface.
8. `bin/build-rust-app.sh` (cargo -> `.app`, `__info_plist` embed via `build.rs`
   `cargo:rustc-link-arg`, icon, sign with the existing cert,
   `grant-permissions.sh` unchanged).
9. Parity harness + dev tools: `./ws rs-parity` (script a `do:` sequence, diff
   Swift vs Rust `state`), window-server enumerator + pixel-probe.

### Phase 1 — Leaf engines (parallel, ~5-6 agents)

10. PaneGeometry · VimSearch · ListFilter · NvimRPC · TextEditKeys.
11. AnsiRender · PaneShot · ScreenshotAnnotations · ScreenshotText.
12. ConfigText · FileOps · CompareText · CompareFolder · RecentFiles · PathShelf ·
    SwitcherStatus · AIFormat (thin facades over `pylib` -> mostly the client
    boundary).

### Phase 2 — Tool panels & simple views (parallel)

13. PathsWindow · ViewSwitcher · TerminalPanel · SetupWindow · SidebarJump ·
    InlineRename · NoteFindWindow.

### Phase 3 — Views (aggressive parallel, one agent per family)

14. Notes + Prose (`NotesProse`, `FilePopup`, `InlineRename`).
15. Jira (`JiraDashboard`, `JiraSearch`, `JiraBoard`, `JiraTicket`).
16. Confluence (WKWebView + `wsconf://`).
17. AI (`AIWindow`, `AIFormat`).
18. Compare (`CompareText`, `CompareFolder`, `ComparePane`, `CompareWindow`,
    `CompareFolderView`).
19. Screenshot (`Screenshot`, `ScreenshotOverlay`, `ScreenshotPin`,
    `ScreenshotAnnotations`, `ScreenshotText`).

### Phase 4 — Cutover (gated)

20. Run `ui-test.sh`, `ui-test-vim.sh`, `ui-test-focus.py`, every
    `run-tests.sh` suite and `test_install.sh` against the Rust binary; require
    zero parity diffs and green suites.
21. Flip `./ws`/`bin/build-app.sh` to the cargo path; retarget
    `bin/kitchen_sink.sh`, `INSTALL.sh`, preflight.
22. Delete the Swift app files (keep the SwiftTerm shim + now-Rust AX helpers);
    update `AGENT_CONTEXT.md`/`CLAUDE.md`/`install.conf`.

### Phase 5 — Polish

23. Run the `deslop` and `code-review` skills over the branch; drop the Swift
    toolchain requirement except for the terminal shim.

## Decision Document

- **Target artifact:** one Rust binary (`ws-rs`) replacing `kitchen-sink` at
  cutover. Developed behind a parity gate; Swift stays authoritative until then.
- **Layout:** `rust/ws-rs` module per Swift file, names matching
  (`ui/popup.rs`, `app/host.rs`, `views/jira.rs`, ...). Cargo workspace so the
  SwiftTerm shim is a separate crate.
- **Terminal:** keep the pinned `libSwiftTerm.a`; a tiny Swift shim crate exposes
  the terminal `NSView` to Rust (the one accepted Swift exception).
- **AX notify helpers:** port `notify/helpers/dock_badges.swift` and
  `webex_unread.swift` to Rust (objc2 Accessibility); update
  `notify/notify_poll.py` to invoke the Rust helper.
- **Python frozen:** same persistent JSON-lines helper; the Rust client mirrors
  `PythonHelper.swift`; preserve `PYTHONDONTWRITEBYTECODE`, `WS_COMMANDS_CONF`,
  `-B`.
- **Contracts preserved byte-for-byte:** socket `$TMPDIR/<notes-socket>`, the
  `state` JSON keys, the `do:ACTION` grammar, launch messages (`window`,
  `compare\t...`, `pane-shot\t...`, `reload`/`restart`,
  `config-check`/`config-schema`), the nvim RPC sidecar socket.
- **Spine-owned shared files:** `host.rs`, `socket.rs`, `main.rs`, `registry.rs`,
  menu/nav/palette wiring. View agents register via spine APIs and own only
  their own files — this is what makes aggressive parallelism conflict-free.
- **Toolchain:** rustup-managed stable rustc (reproducible); target
  `aarch64-apple-darwin`, min macOS `26.7` matching `install.conf`.

## Testing Decisions

- **Contract-first:** the Rust `testQuery` must emit identical `state` JSON and
  `do:` behavior so `bin/ui-test*.sh`, `ui-test-focus.py` and the `ui-check`
  skill work unchanged. This is the primary cutover gate.
- **Unit parity:** port each `// sources:` Swift test case-for-case to Rust
  `#[test]` (Foundation-only files port cleanly). The Python suites already
  cover the engines and stay untouched.
- **Parity harness:** diff `state` between the Swift and Rust binaries over a
  scripted `do:` sequence; any mismatch blocks cutover.
- **Dev loop:** the two PoC tools (window-server enumerator + pixel-probe) become
  `./ws` subcommands for deterministic UI assertions. Remember the `borders`
  daemon chicken-and-egg (`brew services stop borders`) for chrome checks.

## Out of Scope

- Rewriting `pylib/`, `jira/`, `confluence/`, `notify/`, `settings_hub/` Python
  (frozen), the `commands.toml` format, the AeroSpace config interface, and
  external tools (herdr/nvim/pandoc/weasyprint).

## Further Notes / Risks

- **Big-bang integration risk** — mitigated by the frozen spine, a
  compile-everything scaffold, keeping Swift authoritative, and the parity gate.
- The spine is large (the two 10k+ files) and serial; aggressive parallelism
  only helps the views.
- `Retained`/`Weak` ARC parity, `autoreleasepool` in loops, delegate boilerplate,
  async `WKURLSchemeHandler`, ScreenCaptureKit/Speech/Vision flows (crates exist:
  `objc2-web-kit`, `-speech`, `-vision`, `-screen-capture-kit`, `-av-foundation`,
  `-pdf-kit`, `-quick-look-ui`; verbose but present).
- Build: `__info_plist` embedding, code-signing, TCC — all must be reproduced by
  cargo.
