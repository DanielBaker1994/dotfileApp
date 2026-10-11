# ▶ NEXT SESSION START HERE (rewritten Oct 11 2026, second session)

Branch `rust-parity` (off `main`, 20 commits, not merged, not pushed). Read
this section, then `AGENTS.md` / `rule.md`. `PARITY-shortcuts.md` has
per-row tables, but its ❌/🟡 marks predate this session; this section wins.

## Working rules (owner feedback, keep them)

- Run `bin/parity-audit.sh` first (about 30 s, writes `.build/parity-audit.md`).
  Socket and nvim verbs are now all ✅. Dead code is down from 709 to about 600 items.
- A row is done only after you see it work in the live app (a screenshot, or
  `state.pane` from real AppKit focus). Unit tests prove the model, not the feature.
- **No retry loops.** If a live check fails, read the code and the AppKit API
  semantics, then fix and verify once. Stop and report after two misses. (The
  last session burned six rebuilds on an `NSView.hitTest` coordinate mistake.)
- Stubs must be loud: use `crate::ui::toast::not_ported("what")`, never a silent log.
- Test file ops only in `/tmp`. Don't open user files in other apps (Return
  on a files row opens it in the default app). Don't flip `[jira] enabled`
  on the owner's config (`jira-poll off` really disables polling).
- Verify fast: `./ws build`, then `echo state | nc -U -w3 $TMPDIR/ws-notes.sock | jq .pane`,
  `do:pane:focus:<id>`, real keys via `osascript … key code N using …`,
  clicks via `cliclick`, window shots via `screencapture -o -l <wid>`.
  Swift reference: `git archive 608d07d | tar -x -C <scratch>`.

## Done this session (each verified live unless noted)

- Loud `not ported` toasts for remaining stubs (`UiCommand::Toast`).
- **Ctrl+H/J/K/L** over real panes (`app/live_panes.rs`, each view's `nav_panes()`),
  silver focus ring, nvim splits first. `state.pane` reports the live responder.
- **Sidebar rail keyboard**: arrows / Ctrl+N/P, Return, Space, Esc, Delete, letter jump, cursor ring.
- **Vim keys on non-nvim panes** (`app/host/daemon_ui/host_vim.rs`): j/k, gg/G,
  Ctrl+D/U, `/` `?` `n` `N` with a search bar, i/a ↔ Esc between field and list,
  NORMAL/INSERT badge. Rows, NSTextView and WKWebView targets.
- **Leader+sf / leader+sg** (`views/note_find.rs`, `do:notes-find|grep`);
  grep jumps nvim to the line. **`do:jira-jump`** sidebar jump.
- `jira-poll on|off|toggle|dashboard` (`app/jira_poll.rs`: config check,
  login test, launch agent sync). Only the already-on path was live-tested
  (the owner's Jira is enabled).
- **Cmd+K action picker** (`ui/action_picker.rs`) for Jira and Compare.
- Per-view keys: AI (Ctrl/Cmd+Return, Esc stop, Cmd+L), Paths shelf (Cmd+C,
  Cmd+Shift+C, Space / Cmd+Y, Cmd+O, Cmd+R, Cmd+Shift+R, Cmd+Delete), Jira
  (Cmd+I, Ctrl+Space, Tab, Home/End/Page), Confluence (Cmd+L/F/G/D/R,
  Cmd+Return, Cmd+C; copy unverified because no site is configured on this
  Mac), Files (history, hidden, new folder, ← →, Space Quick Look, Cmd+C/X/V/D/Z,
  Cmd+Delete), Compare text + folder (`CompareWindowModel::handle_key`).
- **Inline rename** (`ui/inline_rename.rs`): files Cmd+R / F2, paths Cmd+R.
- Hyper+S palette redrawn on `ListPanel` with split rows.
- Each view focuses a sensible pane on show. Switcher "where" text for every
  view. Cmd+± zooms the file list text. The notes editor now shrinks while
  the terminal drawer is open (nvim rows used to sit under it).

## Still open (priority order)

1. **Right-click menus** (rule.md #2). Already in Rust: files rows, paths rows,
   notes sidebar, Jira rows, Confluence rows, AI rules, Compare sidebar pills
   + recents (partial). Missing, with Swift items:
   - Compare **text pane** (`ComparePane.rightMouseDown` → `CompareWindow.paneMenu`):
     Copy · Copy to other side · Copy Line to other side · Paste Clipboard Here ·
     Edit Section · Select Section · Find… · Align With… / Cancel / Remove This
     Alignment / Clear All Alignments · Copy Path · Reveal in Finder · Open in
     Notes · Open in Default App · Open File into This Side… · Save This Side.
     The model has most of these (`CompareWindowModel::test_do`).
   - Compare **folder tree** (`CompareFolderView.menu(_:_:)`): Copy/Move to
     Right/Left · Rename… · New Folder… · Move to Trash · Copy Path · Reveal ·
     Open in Notes · Open in Default App · Quick Look · Set as Base Folder
     (Left/Right) · Exclude "<name>".
   - Compare **pill** extras: Copy to other side, Copy Line, Paste Clipboard
     Here, Edit Section, Select Section. **Recents** extras: Fill the Path
     Fields, Remove All Missing, Clear All Recents.
   - Jira **column header**: Fit Columns to Content.
   - Screenshot **history grid**: Reopen as Pin · Copy · Open in Preview · Reveal
     in Finder · Delete. Screenshot **pin**: Copy to clipboard · Save to file ·
     Rotate Right/Left · Increase/Decrease Opacity · Close.
   - Notes **plain editor** (vim-mode off, image links): Open Full Size · copy
     image path · Copy File Path · Open file at path….
   - **Terminal / nvim pane menus are parked by owner decision** (see 6).
     For any new right-click code: `NSView.hitTest(_:)` takes the point in the
     *superview's* coordinates. For a window's content view, pass
     `event.locationInWindow()` unconverted. Converting into the (flipped)
     content view mirrors y.
2. Jira **setup window** (`JiraSetupWindow`) and Jira **live search** Cmd+F
   (`JiraSearchPanel`). Both toast not-ported now. Both are large forms.
3. Theme menu extras: live preview on hover, Custom Color…, Transparency ▸,
   per-view card theme menu. Jira list text zoom (toasts).
4. Small: `state.paths` (`selection`, `rowCount`) is stale between pushes. The
   files listing doesn't watch the disk (a new file shows only after re-entering
   the folder). Compare `Return` begin-edit and Cmd+C copy-rows are not routed.
   Compare text Cmd+F/G find is not routed.
5. Older items further down this file: hide-on-focus-loss bridge, nvim `:q`
   relaunch, AeroSpace re-show (ask the owner), screenshot/compare gaps.
6. **Terminal strategy (owner wants this):** drop SwiftTerm for a Rust-only
   build. SwiftTerm costs a Swift toolchain step, the fetched checkout,
   `patches/swiftterm-cellstorage-cache.patch`, the precompiled static lib and
   `rust/swiftterm-shim/`. Plan: `alacritty_terminal` (VT parser, grid,
   PTY / event loop) plus our own renderer, a Core Text–drawn `NSView` behind
   the same API the shim exposes today (`create_terminal`, `start_process`,
   `send_text`, `set_font`, `set_colors`, running / exit code), so the notes
   nvim pane, the drawer and the `/terminal` panel switch over in one place.
   Must cover: selection + copy/paste, mouse reporting (nvim), scrollback,
   IME / dead keys, bold/italic/underline, 256 + truecolor, cursor shapes,
   resize → PTY winsize, font fallback, and performance good enough for nvim.
   Do it after parity, as its own branch.

## Uncommitted

`samples/graphviz-showcase.md` line 1 changed from `doc paper clean` to
`doc executive`. Probably from keystrokes sent into nvim during testing; the
owner decides whether to revert.


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
