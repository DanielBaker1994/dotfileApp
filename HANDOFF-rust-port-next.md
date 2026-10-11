# ▶ NEXT SESSION START HERE (written Oct 11 2026, end of session)

Read this section, then `PARITY-shortcuts.md` (the full Swift-vs-Rust shortcut
table with per-row status), then `AGENTS.md` / `rule.md`. The rest of this file
below the line is older context.

## Why gaps kept slipping through, and the one-shot fix

**Run `bin/parity-audit.sh` first** (about 30 s, writes `.build/parity-audit.md`). It lists:
items written but never called, stub markers, Swift `do:` verbs vs Rust,
nvim→app verbs, and `[shortcuts]` rows per view. Work the list; re-run it to
show progress.

Root causes (Oct 11 finding):
1. **`#![allow(dead_code)]` in `rust/ws-rs/src/main.rs` hides it.** With dead
   code forced on there are **709 written-but-unused items**. Among them:
   `PaneFocusRing` and `PaneProvider` (the Ctrl+HJKL ring and pane
   registration), `VimRows`/`VimText`/`VimWeb`/`VimTarget` (normal mode and `/`
   search targets), `NoteFindModel` users, and `AIState::handle_key`. The port
   wrote models plus unit tests, then never connected them to the AppKit side.
   Green tests proved the model, not the feature.
2. **Silent stubs look like success.** `do:notes-find` returns OK and only
   logs "not modelled yet", and the router treats Cmd+K as handled. A key
   that is caught but does nothing reads as "routed" in an audit.
3. **Audits judged "routed", not "does the thing".** The previous session's
   audit marked rows by whether a key path existed. A row is ✅ only after
   the behavior is seen in the live app (screenshot or `state` field
   reflecting real AppKit focus).
4. **Shortcuts live outside keyDown.** nvim lua (`ask_app`) → socket verbs,
   AeroSpace bindings, menus, and right-click menus. The audit script covers the
   socket and nvim sources. Menus and right-click menus still need a manual
   pass against Swift `menuItem(` calls.

Suggested guardrails: when something gets wired, remove its dead-code entry
(the count should only go down). Make stubs loud (a toast "not ported: X")
instead of a silent log. Have `state.pane` report the real first responder,
not the model.

## Owner's notes and concerns (verbatim intent)

- "Are you saying you have finished?" — **No.** The previous session fixed a
  handful of items and over-summarised. Do not report done until the owner's
  workflow keys actually work in the live app, shown by screenshots.
- The owner works keyboard-first, tmux-style, and expects **every** shortcut
  the Swift app had. Missing so far, called out by name:
  1. **Ctrl+H/J/K/L pane focus** — move the keyboard between sidebar, editor,
     list, preview and terminal, with the thin silver focus ring, like Swift's
     `PaneNav`.
  2. **`/` search on non-nvim panes** — vim-like `/` `?` `n` `N` (plus j/k,
     gg/G, Ctrl+D/U, i/a, the NORMAL chip) on lists, sidebars, preview pages
     and Compare sides (Swift `VimKeys` / `VimSearch`).
  3. **Leader+sf in notes nvim** — the interactive picker over open notes
     (Swift `NoteFindWindow`).
  4. **Leader+sg in notes nvim** — ripgrep the open notes; Return jumps to the
     line (also `NoteFindWindow`).
- Previously reported and now fixed (verify they still work, don't redo):
  Ctrl+B W popup look, Ctrl+B T prose toggle, the prose capsule, the Theme
  menu (header app icon ▸ Theme).
- Standing owner rules: no bash+sleep UI suites; verify with socket probes +
  screenshots; fix the code, iterate fast. Don't obsess over the vim pane.

## Verified state of each missing item (code facts, Oct 11)

1. **Ctrl+HJKL** — `PrefixState::handle` (ui/popup.rs) returns
   `PaneMove(dir)` → `route_key_event` → `SwitcherController::pane_move` →
   `ControllerInner.pane_nav.move_dir(dir, &inner.panel_list())`. That is a
   **model only**: `panel_list()` is built from `inner.panes` (fed by
   tests/`do:`), not from the real AppKit views, and nothing calls
   `makeFirstResponder` or draws a ring. Port Swift `PaneNav.swift`
   (`PaneNav.shared.move(_:in:)`, `track`, the ring overlay) and each view's
   `navPanes` (`PopupWindow: PaneProvider` at PopupWindow.swift ~11351:
   sidebar/search/page/list/editor/terminal/browser), plus the nvim-first rule
   (`vim pane: its own splits first` — Swift asks nvim `wincmd` before leaving
   the pane). Keep `do:pane:h|j|k|l` / `state.pane` reporting real focus.
2. **`/` search + normal mode** — `panes/vim_keys.rs` + `panes/vim_search.rs`
   exist (ported, unit-tested) and `VimKeys` is stored on the controller
   (host.rs `vim_keys: VimKeys::new()`), but **`route_key_event` never calls
   it**. Swift calls `VimKeys.shared.handle(e, in: w)` inside
   `SharedWindow.prefixKey` *before* the armed-prefix check, and
   `VimKeys.searching(in:)` first of all. Wire it there, target the
   focused pane from item 1, and draw the NORMAL/INSERT chip + search bar.
3. + 4. **Leader+sf / Leader+sg** — `vim/init.lua:67-69` sends
   `ask_app('notes-find' | 'notes-grep')` → socket `do:notes-find` /
   `do:notes-grep` → host.rs `do_host_action` arm
   `"jira-jump" | "notes-find" | "notes-grep"` just logs "not modelled yet".
   The model `NoteFindModel` exists in `views/notes.rs` ("Find / grep window
   model"); the window does not. Port `NoteFindWindow.swift` (313 lines) on
   `ui/list_panel.rs::ListPanel` (same pattern as the rebuilt Ctrl+B W
   switcher in `views/switcher.rs` `switch_ui`), `[notes-find]` config, then
   jump nvim via `VimPane::command("call cursor(N,1)")` / `vim_open(path)`.
   `jira-jump` (`SidebarJump.swift`) is the same shape — do it alongside.
   Also missing socket verbs (from the audit): `jira-poll-on`, `jira-poll-off`,
   `jira-setup`.

## Everything else still open (priority order)

5. **Cmd+K action picker** (`actionPicker`, PopupWindow/CompareWindow; Jira:
   copy / copy URL+title / open, 1-9 in menu) — currently logs only.
6. **Per-view keys not wired** (see PARITY tables): AI (`AIState::handle_key`
   exists, unconnected), paths (`PathsShelf::key` exists), Jira
   (`JiraDashboard.handleKey`: Cmd+F, Cmd+I, Ctrl+Space, Tab…), Confluence
   (Cmd+Return, Cmd+C/⇧C, Cmd+D, Cmd+G, Cmd+R), files (←/→, Tab complete, Space
   Quick Look, Cmd+C/X/V/D/Delete/Z, Cmd+Shift+N, Cmd+[ ], Cmd+↑↓, rename F2/
   Cmd+R, Cmd+Shift+.), Compare + compare-folders (~45 rows, none routed).
7. **Sidebar keyboard nav** (`PopupTabsBar.handleNavKey`: ↑↓/Ctrl+N/P,
   Return, Space, Home/End, letter jump, Esc, Delete) — needs item 1 first.
8. **Hyper+S palette** still renders rows as plain `NSButton`s — move to
   `ListPanel` like the switcher (`views/switcher.rs` `PalettePanel`).
9. **Theme menu extras**: live preview on hover, Custom Color…, Transparency ▸,
   per-view card theme menu; list-view text zoom (`setTextZoom`, Cmd+± on
   files/jira currently resizes the window).
10. Switcher "where" text for Jira/Confluence/Compare/AI (`whereText`).
11. Older items further down this file: hide-on-focus-loss bridge, notes vim
    `:q` relaunch, AeroSpace re-show (ask owner), screenshot/compare gaps.

## Done in the Oct 11 session (uncommitted at time of writing)

Ctrl+B T (prose first) / Ctrl+B L fallback; Ctrl+B W redrawn
(`views/switcher.rs` `switch_ui` on `ui/list_panel.rs`); in-window prose view
`views/notes_prose.rs` (Swift page JS verbatim in `views/prose_js/`), capsule
clicks, doc-style menu, Cmd+Shift+P/O, Cmd+P PDF, prose search keys; Cmd+N /
Ctrl+N; Ctrl+Shift+J/K VM cursors; Cmd+± vs Cmd+Opt+± swap fixed; Cmd+/ sheet
`ui/shortcuts.rs`; header icon menu with Theme ▸ (`header_icon_menu`,
`apply_theme_preset`, `after_theme_change` in host.rs); `ui/toast.rs`;
`ui/menu_kit.rs` gained submenus/headers/swatches. 695 cargo tests green.

## How to verify fast (learned this session)

- Real keys: `osascript -e 'tell application "System Events" to key code 11 using control down'`
  (B=11, T=17, W=13, H=4, J=38, K=40, L=37, `/`=44 — `keystroke "/"` sends
  keypad 75, don't use it). Clicks: `cliclick c:X,Y` (screen points, top-left).
- Window shot: `screencapture -x -o -l $(echo state | nc -U -w3 $TMPDIR/ws-notes.sock | jq -r .views.notes.wid) out.png`.
- Capture panics: `./ws build --build-only` then
  `open -g --stderr <file> kitchen-sink.app --args notes`. A RefCell
  double-borrow panics and kills the daemon silently (happened once:
  never call a helper that borrows `state` while holding `borrow_mut`).
- Stop driving the UI the moment the owner is using the machine.

---

# Rust port — next agent brief (post-cutover parity)

**Goal:** the Swift app was ported to Rust and cut over on Oct 10 2026. Finish
**like-for-like parity**: every behavior the Swift app had, the Rust app has.
Read `AGENTS.md` (current, Rust-focused) and `rule.md` first. Repo:
`/Users/danielbaker/.config/kitchen-sink` (`git rev-parse --show-toplevel`).
The owner commits on request — **nothing from the cutover is committed yet**.

## State you inherit

- `./ws build` = `bin/build-app.sh` = `cargo build` (rust/) → repo-root
  `kitchen-sink.app` (signed, repo mode). The live daemon is the Rust build.
- Green: `./ws test all` (654 ws-rs tests + Python), `cd rust && cargo test`,
  `build-app.sh --dist` + `Tests/test_install.sh` (55/0). Pre-cutover the UI
  suite was 85/0 and strict parity vs Swift had 0 diffs over 29 actions
  (the 85/0 is the legacy suite's green — see "UI shell suites" below: it
  does not evidence parity).
- **The Swift sources are the parity reference.** They are still on disk
  (the owner may run `git rm -q *.swift Tests/*.swift` at any time). After that,
  read them from git: `git show 608d07d:PopupWindow.swift` (608d07d = last
  pre-cutover commit). Grep a Swift symbol, read ~60 lines; never read
  `PopupWindow.swift` / `kitchen_sink.swift` whole (10k+ lines each). Rust
  comments name the Swift type they port (`SwitcherController`, `testQuery`…).
- Human follow up - we hit our usage limits, in 29268ae0a4065eba5f0c2bc114d7af56726693bf I removed swift sources.
- Human follow up - 209f34e55d4a3f48f6fd3c4dc70e1229578adab4 is the loast commit from claude.
- The Swift app can no longer be run side by side (it is not built any more),
  so parity is checked **against the Swift source**, not a live Swift daemon.
  If you need live Swift behavior, build it at 608d07d in a separate worktree
  (`git worktree add /tmp/ks-swift 608d07d && cd /tmp/ks-swift && bin/build-app.sh`)
  — never touch the live repo-root bundle for that.

## Shortcut parity (Oct 11 2026)

`PARITY-shortcuts.md` is the full Swift-vs-Rust shortcut audit (the `[shortcuts]`
table plus the Swift key handlers) with per-row status and the remaining fix
order. Start there for any keyboard work.

## Environment rules (learned the hard way)

- **UI verification needs an unlocked screen.** While locked (`ioreg -n Root -d1 -a |
  grep -A1 CGSSessionScreenIsLocked`) AX sees zero windows and nothing can
  activate. If locked, do code + `cargo test` + socket-probe work only.
- Don't loop UI runs (the suites are deleted — see "UI shell suites" below).
  Probe with `echo state | nc -U -w3 $TMPDIR/ws-notes.sock` / `do:ACTION` and
  capture screenshots; rebuild (`./ws build`) before checking.
- For an isolated test daemon: `open -n -g --env TMPDIR=$RT/ --env
  WS_RS_ASSET_DIR=$PWD <bundle> --args notes` (LaunchServices → correct TCC).
  pid-scope with `lsof -U | grep <sock>`; never `pkill -f kitchen-sink`.
- `PYTHONDONTWRITEBYTECODE=1`; Python stays behavior-stable; SwiftTerm checkout
  is read-only (the shim `rust/swiftterm-shim/shim/WSShim.swift` is ours).


## UI shell suites: deleted (owner decision)

**Owner decision (Oct 10 2026): the UI test suites (`ui`, `vim`, `focus`) are
deleted — do not bring them back.** They were slow and ineffective, and green
runs never evidenced parity. Verify UI work with **artifacts and screenshots**
instead. What was done in the past — bash driving the UI with sleeps and flaky
back-and-forth — is not good.

Kept for context (why green never meant parity):
It has been my experience that most of your UI testing has been wildly poor. It has fucked me over waiting for you to switch back and forth and 
the results have been poor. It kills iteration. Fix the fucking code, take screenshots and use artifacts. ui-test-vim.sh this kind of shit, you obsess over vim over and over
again and it wastes so much fucking time. Figure out how to iterate quicker on this port, spending 2.5 seconds sleeping and then having flaky back and forth is a waste of time.

## Visual parity: reference material

The look was iterated with **Claude artifacts** in the past (that workflow
worked well); those artifacts still exist. Ask the owner for the links and use
them to re-establish the intended look — the Rust port is currently far off
it, visually. They are the closest thing to a reference until a golden set
exists.

For pixel-level reference there is no golden set yet. If required:

1. Build the Swift app at `608d07d` in a separate worktree (see "State you
   inherit" — never touch the live repo-root bundle).
2. Run it side by side with the Rust daemon (isolated `TMPDIR`s; launch
   through LaunchServices so TCC is right — see "Environment rules").
3. Capture screenshots of every view/state from BOTH apps (notes, files, jira,
   confluence, ai, compare, palette, tool panels) via the same scripted
   actions used for `rs-parity.py`.
4. Commit the Swift captures into the repo as the reference set (a new
   `docs/parity/` or `samples/parity/` dir — owner's call) and use them as
   goldens for the Rust port.




### 2. Hide-on-focus-loss focus bridge (well scoped)
Swift: `kitchen_sink.swift` `startFocusBridge` (~line 2561) +
`focusBridgeName = "ws-aerospace-focus"`, and `hideOnFocusLoss` handling in
`SharedWindow.swift` / `PopupWindow.swift`. AeroSpace's `on-focus-changed`
writes the focused window id to `$TMPDIR/ws-aerospace-focus`; Swift watches it
(DispatchSource) and, with `[app] hide-on-focus-loss = true`, hides the shared
window when focus leaves it (combined with `NSApp.isActive && keyWindow`).
Rust reports `hideOnFocusLoss` in `state` but implements none of it. Port into
`app/host.rs` (daemon UI drain or a watcher thread → `UiQueue`). Unit-test the
decision logic; verify by hand (focus moves + a screenshot — no UI suite; see "UI shell suites" they fucking sucked.).



### 3. Notes vim pane `:q` relaunch (hardest; debugging)
`views/notes.rs` `vim_poll_exit` + `views/notes_vim.rs` `VimPane::poll_exit/build`.
Observed: the exit edge fires (shim `terminalExitCode` 0), a new `WSShim` is built
and `startProcess` forks nvim (pid logged), but the child dies at once and the new
shim's `processTerminated` never fires (`exitCode` stays -1, `terminalRunning`
false); no nvim remains. Swift: `TerminalAutoRestart` (PopupWindow.swift ~6911)
restarts in the `processTerminated` callback. Suspects: the new child's exit
status/stderr (nvim `--listen` socket still bound by the old instance?
`$HOME/.cache/kitchen-sink/nvim-notes-<pid>.sock`), the old view's teardown
racing the new one, or starting outside the termination callback. Probe with a
temporary `NSLog` in the shim (`processFailedToStart`, shellPid after start) and
an `eprintln!` in `poll_exit` (remove after). Then verify by hand with
`[notes] vim-mode = true`: `:q` in the notes vim pane and watch the restart
(no UI suite — it was deleted, see "UI shell suites").

### 4. AeroSpace re-show ("no jump back") — needs the owner's call
After hide → `aerospace workspace 1` → hotkey, Swift leaves the window on
workspace 2 (the `config/aerospace/aerospace.toml` on-window-detected rule) with
focus staying on 1; Rust makes AeroSpace follow to 2. Ask the owner which is
wanted before changing anything; if Swift's, compare the show sequence
(`SharedWindow.present` / `PopupWindow.unpark` vs Rust `present` in
`daemon_ui`) for activation/ordering timing.

### 5. Remaining feature gaps (each self-contained; port from the Swift file)
- `Cmd+K` action picker — `PopupWindow.swift` / `CompareWindow.swift` `actionPicker`.
- `Cmd+/` shortcuts sheet — `showShortcuts` (CardWindow/PopupWindow/…; rows from `[shortcuts]`).
- `do:notes-find` / `do:notes-grep` window — `NoteFindWindow.swift` (Rust model
  `NoteFindModel` in `views/notes.rs` exists; the window doesn't).
- `do:jira-jump` sidebar jump — `SidebarJump.swift`.
- `terminalPanel` state — Swift `TerminalPanel.swift` `testState()`; Rust reports `{"shown"}` only.
- File-browser inline rename — `InlineRename.swift`; filefast `PathShelf` add.
- pane-shot preview window — `Screenshot.swift` `func preview(_ img:`.
- Screenshot side panel / save card / `modePill` (models only in `views/screenshot.rs`).
- Compare `current.summary` label (`CompareWindow.swift` `identicalSummary` /
  `binarySummary`), live `scrollY`.
For each: add/adjust `state` fields + `do:` hooks to match Swift's `testState`
/ `testQuery`, unit tests, then a socket probe.
