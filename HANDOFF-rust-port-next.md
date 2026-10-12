# ▶ NEXT SESSION START HERE (rewritten Oct 11 2026, third session)

Work directly on `main`. Don't create feature branches: the old
`rust-parity` branch was merged and deleted at the owner's request. Nothing
is pushed. Read this section, then `AGENTS.md` / `rule.md`.
`PARITY-shortcuts.md` has per-row tables, but its ❌/🟡 marks predate the last
two sessions; this section wins.

## Working rules (owner feedback, keep them)

- Run `bin/parity-audit.sh` first (about 30 s, writes `.build/parity-audit.md`).
- A row is done only after you see it work in the live app (a screenshot, or
  `state` from the socket). Unit tests prove the model, not the feature.
- **No retry loops.** If a live check fails, read the code and the AppKit API,
  then fix and verify once. Stop and report after two misses.
- Stubs must be loud: use `crate::ui::toast::not_ported("what")`, never a silent log.
- Test file ops only in `/tmp`. Don't open user files in other apps. Don't flip
  `[jira] enabled` on the owner's config.
- **Stop GUI driving when the owner is at the machine.** `cliclick` and
  `osascript` keys land in whatever is frontmost. Passive `screencapture -o -l
  <wid>` is fine.
- Verify fast: `./ws build`, then `echo state | nc -U -w3 $TMPDIR/ws-notes.sock | jq …`,
  clicks via `cliclick` (top-left screen points), window shots via
  `screencapture -o -l <wid>`. Swift reference: `git archive 608d07d | tar -x -C <scratch>`.
- **Isolated test daemon** for config-dependent features (vim-mode off, Jira):
  copy `commands.toml` to /tmp, edit it, then
  `open -n -g --env TMPDIR=/tmp/x/rt/ --env WS_RS_ASSET_DIR=$PWD --env WS_COMMANDS_CONF=/tmp/x/commands.toml kitchen-sink.app --args notes`.
  Stop it with `kill $(lsof -t /tmp/x/rt/ws-notes.sock)`. `[notes] paths` is
  the notes tab list, so set it to a /tmp note.
  **Jira gotcha:** `JIRA_CONFIG_JSON` alone is not isolation. With no file
  there, `jira_config.py` migrates the owner's legacy config (token included)
  into that path. Also set `JIRA_CONFIG_FILE`, `JIRA_TEAM_JSON` and
  `JIRA_CACHE_DIR` to /tmp paths, and delete them afterwards. A successful
  Save & Enable also runs `sync_launch_agent`, which touches the real
  `~/Library/LaunchAgents/com.jira.poll.plist`.
- To see a daemon's panic text, launch it with
  `open -n -g --stderr <file> kitchen-sink.app --args notes`. Once this
  session the daemon exited cleanly, with no crash report, around a
  `do:screenshot:show`, and stderr was lost. It didn't happen again with
  stderr captured.

## Done this session (all on main, each verified live unless noted)

- `0aa467b` **Compare text pane menu** (`compare_pane_menu`): right-click
  moves cursor and focus, then Copy, Copy (Line) to other side, Paste
  Clipboard Here, Select Section, Align With / Remove / Clear All, Convert
  Side, Copy Path, Reveal, Open in Notes / Default App, Open File into This
  Side, Save This Side. Edit Section and Find toast (not routed yet).
- `0539055` **Compare folder tree menu** (`compare_folder_menu`), including
  a clash prompt (Replace / Keep Both / Skip) and the fixed `set_base`, which
  used to set both roots to one side's path.
- `8fb33e7` Compare session-row menu now matches Swift `pillMenu` (Close,
  Copy / Reveal per side). The old handoff's "pill extras" list was wrong.
  Recents gain Fill the Path Fields, Remove All Missing (n), Clear All Recents.
- `523ef4d` **Jira header menu**: Fit Columns to Content (`fit_column_pcts`).
  Saving the widths and Group By toast as not ported.
- `6bd8f3d` **Screenshot pin menu**: copy, save, real pixel rotation
  (`shot_cg::rotate_png`), opacity, close. Save not clicked live, because it
  writes to the owner's screenshot folder.
- `83eb2d6` **Recent screenshots grid** (`recent_grid`): ⌘R in the overlay
  had been recorded and then ignored. Now there's the grid, click to reopen as
  a pin, and a right-click menu. Adds `menu_kit::popup_context`: a plain
  `popup` opens *below* windows above the menu level.
- `6151396` **Notes plain-editor menu** (`WSNotesTextView`): image-link items
  (Open Full Size toasts, because the FilePopup window isn't ported), Copy
  File Path, Open file at path…. Verified in an isolated daemon.
- `97bd455` **Jira setup window** (`ui/jira_setup.rs`). Verified live: it
  opens and prefills. **Not yet verified live: Test Connection, Copy curl,
  Save & Enable.** Do that first, in an isolated daemon with every Jira env
  override (see the gotcha above), against `jira/fake_jira.py` on :8766.

## Agent control plane (new, Oct 12 2026)

- `bin/ks-mcp-sandbox.sh start|stop|status|sock|reset` runs the daemon in the
  new Rust `headless` mode (`kitchen-sink headless`) against an isolated
  `TMPDIR`/`WS_HOME` under `.build/mcp-sandbox` — real `state`/`do:` semantics,
  zero GUI objects, and `./ws build` no longer needs to be involved for agent
  checks. `export KS_SOCKET=$(bin/ks-mcp-sandbox.sh sock)`, then `nc -U`.
- `bin/ks-mcp` (+ `bin/ks-mcp.py`) is a thin MCP server (`ks_state`, `ks_do`,
  `ks_raw`) registered in `opencode.json`: live entry read-only, sandbox entry
  disabled. `./ws test mcp` runs its suite. Deferrals + headless blind spots
  (Cmd+K/Cmd+/, find bars, jira `board:*`, `pending_note_open`) are listed
  under "MCP / headless control" in `BACKLOG.md`; full notes in `AGENTS.md`.

## Still open (priority order)

1. Finish verifying the Jira setup window (see above). Its header text is
   clipped at the third line (same 34 pt frame as Swift); widen it or shrink
   the font if it looks wrong.
2. Jira **live search** Cmd+F (`JiraSearchPanel`, JiraSearch.swift
   729–1366, with `JiraMultiPicker` / `JiraChoiceButton`). Still toasts.
3. Found this session:
   - Folder copy/move from **keys and Cmd+K** (`folder-copy:*` →
     `FolderPageState::transfer`) still overwrites without asking. Only the
     right-click path prompts. Route both through
     `compare_folder_transfer`.
   - Some unit test writes 1–7 byte junk `shot-*.png` files into the
     **real** `~/.cache/kitchen-sink/screenshots` (they show as blank cells
     in the grid). Find it (`record_png` callers in tests) and point it at a
     temp dir.
   - The screenshot overlay's **recent button** (`ShotRecentButton`) isn't
     drawn. ⌘R works.
   - Jira **Group By** menu and saving fitted column widths
     (`columnsResized` → `jira_config.py --set-columns`) aren't ported.
   - The **FilePopup** window (notes Open Full Size, Swift `FilePopup.show`)
     isn't ported.
4. Theme menu extras: live preview on hover, Custom Color…, Transparency ▸,
   per-view card theme menu. Jira list text zoom (toasts).
5. Small: `state.paths` (`selection`, `rowCount`) is stale between pushes. The
   files listing doesn't watch the disk. Compare `Return` begin-edit, Cmd+C
   copy-rows and Cmd+F/G find aren't routed; the text-pane menu toasts for
   them.
6. Older items further down this file: hide-on-focus-loss bridge, nvim `:q`
   relaunch, AeroSpace re-show (ask the owner), screenshot/compare gaps.
7. **Terminal / nvim right-click menus and the SwiftTerm replacement are
   parked by owner decision.** Plan, when asked: `alacritty_terminal` plus a
   Core Text `NSView` behind the shim's API (`create_terminal`,
   `start_process`, `send_text`, `set_font`, `set_colors`, running / exit
   code). It must cover selection + copy/paste, mouse reporting, scrollback,
   IME, styles, 256 + truecolor, cursor shapes, winsize and font fallback.
   Do it on `main`, after parity.

Right-click notes for new code: `NSView.hitTest(_:)` takes the point in the
*superview's* coordinates. Use `menu_kit::popup_context` from inside a
`rightMouseDown:` when the window floats above the menu level.

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
- Vim search highlight — `VimMatchOverlay`/`hlsearch` (`VimKeys.swift`). Rows
  are ported (`views/note_find.rs` `PaneFindUi`, `host_vim.rs` overlay); the
  text/web targets are still unported.
- `terminalPanel` state — Swift `TerminalPanel.swift` `testState()`; Rust reports `{"shown"}` only.
- File-browser inline rename — `InlineRename.swift`; filefast `PathShelf` add.
- pane-shot preview window — `Screenshot.swift` `func preview(_ img:`.
- Screenshot side panel / save card / `modePill` (models only in `views/screenshot.rs`).
- Compare `current.summary` label (`CompareWindow.swift` `identicalSummary` /
  `binarySummary`), live `scrollY`.
For each: add/adjust `state` fields + `do:` hooks to match Swift's `testState`
/ `testQuery`, unit tests, then a socket probe.
