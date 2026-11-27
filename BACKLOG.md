# Backlog

Every open item for the app, newest theme first. Each entry says what is wrong or
wanted, where it lives, and what a fix probably looks like. The code is truth:
re-check an item before working on it. Items about removed features (Notes view,
Compare view, prose/PDF pop-out, SwiftTerm terminal drawer, the Swift build) were
dropped in the Nov 2026 doc consolidation; `git log -- <old file>` has the history.

Resolved bugs live in git history, not here. Test workers read `TESTING.md` §
"Bug ledger" for how to report new ones.

---

## Palette (Hyper+S) and view switcher

- **No "No results" row.** A filter that matches nothing collapses the panel to the
  search box. Fix: draw one disabled row (and make `accept()` a no-op on it) in
  `ks-core/src/views/switcher.rs`.
- **Workspace badges read as part of the command rows.** The right pane (workspace id
  chips + app icons) lines up with the command rows, so a badge looks like it belongs to
  the row beside it. By design (two panes, Tab switches) but unlabeled. Fix: a faint
  column header or a stronger divider.

## Shortcuts and key routing

- **`Cmd+K` action picker and the `Cmd+/` shortcuts sheet** are not built; keys are
  recorded and ignored. Wanted per view: open with, reveal, copy path, copy file; Jira
  and Confluence open-in-browser.
- **Palette key grammar is duplicated** between `ks-core/src/app/quick.rs` (~925-1010) and
  `ws-rs/src/app/host.rs` `route_overlay_key`. Fold into one `palette_key()` in
  `views/switcher.rs`; needs real-key testing (see `AGENTS.md`, ks-input).
- **Missing Files keys** (check each against the code first): `Cmd+O` open selection,
  `Cmd+[` / `Cmd+]` history, `Cmd+Shift+.` hidden files, Shift+arrows range extend, Tab
  path completion, Space Quick Look. Jira: `Cmd+F` live search (`views/jira_search_bar.rs`
  is the start), `Cmd+I` issue panel. Confluence: `Cmd+F` find in page, back/forward.
  `Cmd+,` should open Settings.
- **Vim search highlight** (`hlsearch`): list rows are ported (files, Jira, Confluence);
  web targets (`CSS.highlights`) are not.
- **Hide-on-focus-loss**: the setting is reported, but the old `ws-aerospace-focus`
  watcher bridge is not ported. AeroSpace re-show differs from the Swift version
  ("no jump back" in the focus suite). Ask the owner before touching either.
- **Window menu** opens no dropdown (S3, report MB-WINDOW-01). `app/menu.rs` builds
  Minimize / Zoom / Bring All to Front; not confirmable under ks-drive (clicks are
  window-relative). Re-check by hand.
- **Window-resize shortcuts are untestable under AeroSpace**: the window is tiled, so
  Ctrl+Shift+HJKL / Cmd+Opt+± snap back. Verify with `aerospace layout floating`, or
  document that they only apply when floating.

## Jira / Confluence / AI

- **Jira setup window** (`ui/jira_setup.rs`): Test Connection, Copy curl and Save & Enable
  are not verified live. Do it in an isolated daemon with every Jira env override
  (`JIRA_CONFIG_FILE`, `JIRA_TEAM_JSON`, `JIRA_CACHE_DIR`, `JIRA_CONFIG_JSON`) against
  `jira/fake_jira.py` on :8766; Save & Enable touches the real LaunchAgent plist. Its
  header text may clip at the third line.
- **Jira Group By menu** and saving fitted column widths (`columnsResized` →
  `jira_config.py --set-columns`) are not ported.
- **Jira direct spawns may ignore `WS_PYTHON`**: `app/jira_poll.rs` tries brew pythons
  before `WS_PYTHON`. Align the discovery order (the helper and Confluence already
  prefer `WS_PYTHON`).
- **`do:board:*` has no handler** ("unhandled action"); Jira registers a palette row only.
  Register a Jira `test_do` for the board verbs. Route-around: the jira CLI
  (`--board-*`, `--pin-*`, `--import-filters`).
- **`do:ai:*` / `do:confluence:*`**: verify they dispatch; if the namespaces are not
  registered, wire `views/ai.rs` / `views/confluence.rs` into `register_views`.
- **Files listing does not watch the disk.**
- **Theme menu extras**: live preview on hover, Custom Color…, Transparency ▸,
  per-view card theme menu; Jira list text zoom toasts.

## Screenshot (Hyper+X)

- Side panel: size, colour, grab, wheel, hex, grid, text options (B/I/U/S, align) and
  the layers list. Still missing: a **font family** picker (Flameshot G3).
- Recents: the ring has a Recent button and `state live` → `screenshot.recents`
  (`shown`, `count`, `highlighted`); there is still no `screenshot:recents*` verb.
- VM GUI checks: `ks-input` drags drop the mouse-up and can leave modifiers stuck
  (a stuck Shift squares every selection). For mouse moves, slow drags and Cmd-drags
  use `Tests/tools/ks_mouse.swift` (`move`, `click`, `dclick`, `rclick`, `drag [--cmd]`,
  `flags`, `release`) and
  `ks_droptarget.swift` for drop checks (2026-11-27: hover names, Cmd-drag out of a pin and the
  QR / hex picks were verified that way). Press Hyper+X only after the agent logs
  `hotkey hyper+x registered`.
- Owner's Mac checks (2026-11-27): `ks-input key` posts are dropped there (exit 0, nothing
  arrives, no Secure Input); keys go through System Events (`osascript … key code`), and
  `ks_mouse` works only outside the command sandbox. Verified there: Copy Text from Picture
  (Files menu, Recents verb), QR/hex, real herdr pane-shot from the CLI and the main
  socket, preview fit, main-app Recents verbs. **Not checked: a second display** (the Mac
  has only the built-in screen). Iterate in the VM, not on the Mac (owner rule).
- `.fast` OCR still drops a lone short cell ("4") and splits some words ("f iles") at
  every scale; a dropped cell shifts that row's columns (Text to Table pads it), and it
  mangles JSON quotes/braces from small screen text. `.accurate` reads all of it, but on
  macOS 27 (the owner's Mac): loading takes ~45-60 s per process, every OCR call in the
  process waits meanwhile (a Copy Text froze ~20 s in a probe — the "screen freezes" the
  owner saw with `auto`), and after loading it still fails *inside the app* with
  "CRImageReaderError error 1" (a standalone probe with the same image pipeline, threads
  and a barcode pass reads fine; macOS 26.6 in the VM loads in 0.2 s and works). Open:
  what in the app process differs. Default stays `fast`; a failed accurate pass falls
  back to fast. Scale is by size (`ShotOCR::scale_for`): a whole MacBook screen stays 1×.
- Non-goals and the full Flameshot comparison: `git show 64bbd4a^:PRD-screenshot-gaps.md`.
- Done 2026-11-27: Copy Text from an existing picture (Files + Find Recent Files menu,
  Recents menu, `do:screenshot:recents:copy-text:N`). The Copy Text result card (with
  next-step buttons and Undo copy) was built, then removed by the owner the same day
  ("i can view clipboard history in other ways"): Copy Text shows only the toast.

## Settings hub (`ws-settings`)

v1 (read: `keys` / `settings` / `get` / `export`, picker, Hyper+/, `doctor`) shipped.
Later: double-tap-modifier trigger, a GUI front end if the TUI falls short, making the
app's own keys configurable from `[shortcuts]` (real remapping), quiz mode. Old spec:
`git show 64bbd4a^:PRD-settings-hub.md`.

## MCP / headless control

- **Full MCP catalog deferred** — thin v1 shipped (`bin/ks-mcp`: `ks_state` / `ks_do` /
  `ks_raw`). Left out on purpose: the 150-method `ks_helper` registry, `ks_jira`,
  `ks_confluence`, `ks_notify`, `ks_settings`, `ks_launch`/`ks_reload`/`ks_restart`, the L1
  sandbox + L2 data suites, modern MCP protocol (`protocol: auto`, `2026-07-28`).
- **Wave-2 leads not fixed**: `ks_do` advertises `board:<sub>` but it always errors;
  `tool-close:<unknown>` and `key:<garbage>` succeed silently; `back` goes home, not to
  the previous view; no `show` to pair `hide`; no state schema / verb-list tool; Windows
  `Reply::peer_closed` is a stub.
- **Key-only surfaces have no headless entry**: `do:key:SPEC` records `last_key` only, so
  Cmd+K pickers, find bars and right-click menus cannot be driven headless.
- **Known failing tests** (not regressions): `Tests/test_mcp_verbs.py::test_every_do_host_action_verb_is_in_the_table`; the flaky
  `engines::path_shelf::tests::pure_add_merges_with_another_writers_entries_atomically`
  under parallel runs.

## Tooling cleanup (from the teardown handoff)

- `bin/grant-permissions.sh` / `bin/lib.sh` still grant microphone + speech-recognition TCC rows and
  `Info.plist` declares both usage strings, but no Rust code uses either. Removing them touches TCC
  grants, so it needs the owner around.
- Needs the owner around: signing-identity and TCC.db duplication in `bin/lib.sh` /
  `grant-permissions.sh`, app-kill logic in ~7 scripts, splitting `host.rs` giants.
- Owner decided (2026-11-24): the GUI test harness stays untouched; the Jira / Files / Confluence / AI views and
  main window are out of scope for teardown.

## Leads

Unverified bug leads from test runs (Oct 2026). Reproduce before fixing; `Tests/night_run.py`'s
fix phase reads the `- S<n>` bullets below (ledger-table rows with an `open` status are read
too). Move a lead to git history once fixed and replayed.

- S3 Jira board Table mode with the issue panel: the Columns/Table pill overlaps the inspector title; every header / key cell is "…".
- S4 Jira board header count clipped at 900 pt ("5 issue…").
- S4 Files: after Cmd+Up the cursor sits on `..`, not on the folder you came from, so Return goes up again (Finder selects the child).
- S4 Create File: Return with an empty name or a `/` only beeps (the button shows a message).
- S3 config: TOML arrays are accepted as scalars (`hide-on-focus-loss = [true]` acts as `true`). By design? Rejecting needs a scalar-key schema in the shared codec. Owner decision.
