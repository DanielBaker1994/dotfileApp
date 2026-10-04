# workspace-switcher

Swift/AppKit macOS menu-bar app. `CLAUDE.md` is a symlink to this file
(`AGENT_CONTEXT.md`) — edit it once. Don't read the big Swift files whole:
grep the symbol from the code map below, then read ~60 lines around it.

## Non-negotiable rules

Read and honor `rule.md` before touching any code. Key points:

- Edit shortcuts (Cmd+C/V/X/A/Z and Ctrl+C/V) must work in every text input.
- Right-click menus in file browsers/terminal must offer obvious actions.
- Config over code: user-facing strings/sizes/paths live in `commands.toml`.
- NEVER run git commands in a backup workspace — verify you're in the real repo first.
- NEVER rebuild or modify the SwiftTerm checkout unless explicitly told to. It
  is fetched (not committed, outside the repo) by `bin/ensure-swiftterm.sh` at
  the `install.conf` `SWIFTTERM_DIR` (`../SwiftTerm`) `SWIFTTERM_PIN` +
  `patches/swiftterm-cellstorage-cache.patch`; don't read it either, too
  expensive.

## Build

```bash
./build.sh            # Build + relaunch
./build.sh --force    # Force rebuild
./build.sh --build-only
bin/make-dmg.sh       # the distributable .dmg (.build/dist) — see Install modes
```

ONE build: `bin/build-app.sh` (compiles every top-level `*.swift`, SwiftTerm
lib, sign, TCC) — used by build.sh, `bin/workspace_switcher.sh` and
INSTALL.sh. Install/uninstall/build names + paths (bundle id, brew deps,
launchd agent, caches) live in `install.conf`. Configs (`CONFIG_DIRS`:
aerospace, sketchybar, borders) are per-file SYMLINKS from `~/.config/<name>`
into the repo's `config/<name>` — edit the repo copy; INSTALL.sh backs up real
files in the way, UNINSTALL.sh removes only links into the repo.

## Install modes (repo + DMG)

- `~/.config/workspace-switcher` ("the home") is THE path every external
  config points at (aerospace.toml hotkeys, sketchybarrc, notifications.sh,
  the jira launchd agent). Repo install: the home is the checkout (or a link
  to it). App install (DMG): a real directory — the user's `commands.toml`,
  `rules/`, `config/` (copies seeded from the bundle, never links into the
  signed app) + links `bin jira confluence notify vim pylib settings_hub install.conf` →
  `workspace-switcher.app/Contents/Resources/…` (relative) and ONE absolute
  link `workspace-switcher.app` → the app. `.install` = marker (`mode`,
  `app`, `version`, `seed <sha> <file>`).
- Swift paths (`workspace_switcher.swift` top): `isRepoBuild` (commands.toml
  + bin/build-app.sh beside the bundle), `assetDir` (repo / Contents/Resources:
  jira, confluence, vim, bin, icons), `userDir` (commands.toml, rules: repo /
  the home), `homeDir` (`$WS_HOME` overrides — tests). `main.swift` sets
  `PYTHONDONTWRITEBYTECODE` (a `__pycache__` in the bundle breaks its
  signature) and `WS_COMMANDS_CONF` for the python side.
- `bin/setup-home.sh app APP [--switch] | repo | stack | status`: seeds
  (untouched file follows a new default, an edited one is kept + `FILE.new`,
  a deleted rule stays deleted), heals the links when the app moved, hands
  over between the two installs. NO backups: only links are ever replaced; a
  real file / dir in the way (a checkout above all) → error, untouched
  (`--switch` only unlinks a home that is a link; `repo` refuses an app
  home). Home owned by a checkout → exit 3, nothing touched. `stack` = `symlinks.sh` (`WS_LINK_ROOT` = the home in
  app mode) + precompiled sketchybar helpers + brew services. Never git.
- `bin/preflight.sh [--json] [--mode repo|app] [--app PATH]`: ONE check list
  for INSTALL.sh (step 0) and the Setup window. Required: macOS ≥
  `MACOS_MIN`, arm64 + not on the disk image (app), swiftc + git (repo).
  Warnings: Apple on-device model (`fm available` → AI view off), python3
  (never runs the CLT stub), nvim, Homebrew + each formula / cask, config
  links, another install owning the home, a second copy of the app,
  Screen Recording (/screenshot; asks the RUNNING daemon over its socket
  — `screenshot-permission`, answered on the socket thread — since only
  the app's own process can tell).
- `SetupWindow.swift`: `AppInstall.ensureHome()` first thing in main.swift
  (marker check; runs setup-home.sh only when something changed),
  `SetupWindow` (rows from preflight JSON, Fix per row: `move-app`,
  `setup-home`, `brew:` / `cask:`, `stack`, `url:`, `term:`; opt-in "Set Up
  Hotkeys & Menu Bar…"; output log). Opens on first run / new version / not
  in Applications; menu bar ▸ Setup & Health Check…, `workspace-switcher
  setup`. `[setup]` in commands.toml (not a palette command).
- `bin/build-app.sh --dist` → `.build/dist/<app>` (Resources per
  `install.conf` `RESOURCE_*`, `commands.default.toml` = the repo's minus
  personal bits + jira off, `helpers-bin/`; never kills the daemon, never
  touches TCC). `bin/make-dmg.sh`: `DEVELOPER_ID` + `NOTARY_PROFILE` →
  hardened runtime (`entitlements.plist`) + notarize + staple; empty → the
  self-signed cert / ad-hoc with a warning. Info.plist version / min macOS /
  icon are generated from install.conf at build time.
- Tests: `Tests/test_install.sh` (needs the dist bundle; throwaway `$HOME`).

## Tests

```bash
bin/ui-test.sh              # Full UI test suite (cliclick + osascript)
bin/ui-test.sh --verbose
bin/ui-test-focus.py        # show / hide / focus / follow-the-workspace, timed (~20 s)
bin/run-tests.sh nvim       # the vim pane's RPC client against a real nvim
bin/run-tests.sh screenshot # /screenshot's model: button ring, undo, pixelate, render
```

- `bin/ui-test-focus.py [single show hide follow stranded swap focus esc]`:
  drives the REAL paths — the hotkey binary as aerospace runs it,
  `aerospace workspace` / `focus`, Esc via System Events (cliclick's keys
  don't reach apps from every session) — and checks AeroSpace's view
  (layout floating, which workspace) + frame stillness (no shudder), with ms
  budgets. Moves your workspaces while it runs; restores workspace, focus,
  view, esc switches and commands.toml.

- Daemon state for tests: the socket answers `state` (JSON: view, frames,
  drawers open, focus pane, tabs…) and `do:ACTION` (cycle, hide, open:VIEW,
  toggle-terminal…) — `SwitcherController.testQuery` + `PopupWindow.testState`.
  ui-test.sh: `ws_state` / `ws_do` / `wait_state` (poll, no sleeps).
  Quick checks: the `ui-check` skill (`.claude/skills/ui-check`).
- No source-grep "tests": a check must drive the app or unit-test code.
- ui-test.sh snapshots commands.toml and restores it byte-for-byte on exit.

## Key files

- `main.swift` — entry point (one-daemon lock hand-over)
- `NvimRPC.swift` — the notes vim pane's msgpack-RPC client (AppKit-free;
  `bin/run-tests.sh nvim`)
- `Screenshot.swift` + `ScreenshotOverlay.swift` + `ScreenshotPin.swift` +
  `ScreenshotAnnotations.swift` — /screenshot, the Flameshot-style capture
  tool (see "/screenshot" below; model tested by `bin/run-tests.sh screenshot`)
- `PaneShot.swift` + `AnsiRender.swift` — `pane-shot`, the herdr pane's
  full-height capture (see "/pane-shot" below; `bin/run-tests.sh ansi`)
- `CompareText.swift` + `ComparePane.swift` + `CompareWindow.swift` — the
  Compare view (Text Compare; see "Compare view" below; engine tested by
  `bin/run-tests.sh compare`)
- `PathShelf.swift` + `PathsWindow.swift` — the /paths recent-file shelf
  (see "/paths" below; `bin/run-tests.sh paths`)
- `workspace_switcher.swift` — app logic (~9200 lines)
- `PopupWindow.swift` — popup window framework (~11700 lines; all keys in `handleKey`;
  `init` = `makePanel` / `makeBackdrop` + the `build…` steps, each wiring its own hooks)
- `CardWindow.swift` — `CardNSWindow` + `CardWindowController`, the base of the
  Confluence / AI / Jira Config windows: the popup-card surface (`themedRoot`),
  header clicks, `setSlotNav`, the shared keys (sheet edit keys, Ctrl+Tab,
  Cmd+W; subclasses override `handleKey` / `keyBeforeSheet`), the kitchen
  sink's head (`iconMenu(view:)` / `popUpIconMenu`) and `SlotMember`
  (subclasses override `didShow`). Questions: `confirm(…)` / `prompt(…)`
  (`ConfirmOverlay`), Cmd+/ card: `showShortcutsCard` (`ShortcutsOverlay`).
  Edit keys: `editKey` — Cmd+A/X/C/V pass through to the Edit menu (it
  fires BEFORE local monitors: routing them too pasted twice), Cmd+Z and
  Ctrl+C/V are routed (`JiraEditKeys`). Text on a tint: `PopupColors.over` +
  `ensure` (4.5:1 against the real composited background)
- `ConfigText.swift` — commands.toml's one-line TOML codec (`configEntry`,
  `configLine`, `configSetting`, `tri`); Foundation only (`bin/run-tests.sh config`)
- `ProcessRun.swift` — `runProcess`: run a program to completion, stdin fed,
  stdout + stderr drained concurrently (Foundation only — the tested files use it)
- Closure actions: `menuItem(title) { … }` (a `ClosureMenuItem` owns its
  closure — nothing to retain on the side), `ClosureTarget` for buttons
- `JiraDashboard.swift` — the Jira Config window (`JiraDashboardWindow`, `JiraColumnEditor`)
- `JiraSearch.swift` — Cmd+F live search (`JiraSearchPanel`), `JiraMultiPicker`, `JiraDirectory`
- `SharedWindow.swift` — ONE window for notes + jira (`SharedWindow`, `SlotView`,
  `SlotMember`): members are the existing windows, swapped in place
  (`PopupWindow.park()` / `unpark(frame:)`); jira back stack (detail /
  releases / config) with back + home in the RIGHT header bar (ids 63/62,
  after the view's own buttons). Header, every view: ✕ · kitchen sink (the
  app's icon menu, `appIcon`, `[app] app-icon`) · view switcher ICONS
  files / notes / AI / jira / confluence — files first + the default view
  (`PopupChrome.navIcons` / `navOn`, ids 60/64/61,
  `[app] notes-icon` / `files-icon` / `jira-icon`) — set in `decorate`.
  Views share the DRAWER-LESS frame (`slotBaseFrame` = `PopupWindow.baseFrame`;
  `unpark(frame:)` re-grows notes by its open drawers). Hotkey
  toggle uses the launcher's focus file (`toggleCommand` → `slot.hotkey`);
  focus hand-back only when the whole window hides. `[app] shared-window`.
  Esc: jira / output = back (`slot.back(esc: true)`); at the top of a view
  Esc HIDES only when that view's kitchen sink "Esc Hides Window" is on
  (`escHideCount(v)` = the section's `esc-close`, else `[app] esc-close`,
  default 0; `setEscHides`, `escapeAtTop`). Notes counts it in its own panes
  (`escCloseCount`, vim: Normal mode only). Cmd+W / ✕ / hotkey always hide.
  Focus loss: ONE path for every view (`checkFocusLoss`, popups set
  `hostHandlesFocusLoss`): after `[app] focus-loss-delay` (0.3s) and
  `focusLeft(w)` → `hide(…, restoreFocus: false)` (parks, never tears down);
  within 1s of a view swap it was stolen (aerospace focusing the next
  window) → the view is re-shown instead.
  Float vs tile is NOT the app's: aerospace.toml's first on-window-detected
  rule decides for every window of the bundle id (now `layout v_accordion`,
  which can't un-float: AeroSpace still lists the shared window as
  floating); members sit at `.normal` level (`present` normalizes Jira
  Config's popUpMenu+1). No `[app] float`, no layout / retile IPC. Tool
  panels (below) are invisible to AeroSpace.
  AeroSpace's closed-windows cache (its lock-screen defence, AeroSpace
  `closedWindowsCache.swift`): an ordered-out window = a closed one → it
  snapshots the WHOLE world; the same window id reappearing RESTORES it
  (monitor flips back to the workspace you hid on, tiles snap to old
  sizes). `workspace N` doesn't clear it; `aerospace eval true` does. So
  every hidden view is shown after `SharedWindow.clearAerospaceCache()`
  (hotkeyPrep does it in parallel; `present` otherwise, ≤0.25 s).
  Frame: `frame` refuses off-screen rects (AeroSpace's hidden corner),
  tracks drags / resizes, and lands on `targetScreen` (the focused
  workspace's monitor from hotkeyPrep, `place(_:from:to:)`).
- `RecentFiles.swift` — the file browser's pinned "Recent" + "Arrived" views:
  ONE FSEvents stream on / (`inScope` prefix filter first), origin from the
  quarantine xattr + kMDItemWhereFroms (`origin`), Spotlight seed,
  `recent.json`; `PopupFileBrowser.virtualLists` / `showVirtual` /
  `recentChanged()`; `[files] recent*`, `recent-scope`, `start = recent`.
  `entries()` reads a lock-guarded snapshot built in `publish()` — never
  `queue.sync` from main. Previews load on `previewQueue` (ImageIO
  downsampled images, `previewGen` drops stale results, 24-entry cache).
  Zoxide favorites were removed. Renames: the stream is IgnoreSelf, so the
  browser reports its OWN rename / move / copy (`FileDrag.onFileOp` →
  `RecentFiles.ownChange`, snapshot patched synchronously, row keeps its
  place); other apps' renames pair old + new event by inode
  (`UseExtendedData`, `departed`) so a renamed folder keeps the files
  listed inside it; ONE name per file: `canonical` = the folder's
  realpath + the name (store + seed; the /private/tmp scan skips symlinks —
  /tmp/zzlink → /tmp listed every file twice); `present` = exact-case check (case-only renames).
  Tests: `bin/run-tests.sh recent` (`Tests/test_recent_files.swift`,
  `// sources:` header = compiled with the app's file; synthetic events +
  a live stream on a temp home).
- Preload (`[app] preload`, default true): `SwitcherController.prewarmSlot()`
  (launch + `reloadConfig()`) builds each missing view hidden, one per
  main-loop turn: openers set `PopupWindow.quietShow` from `slotPrewarming`
  (`presentList` does everything but order front), then
  `SharedWindow.prepare` sets the frame + `decorate`. Log: `preload VIEW: N ms`.
- Shared window members: notes, files, jira (+ detail / releases / config),
  output windows (`currentOutputName`). Only the notes terminal drawer and
  JiraSetupWindow live outside it.
- Signing: `bin/build-app.sh` clears stray `*.cstemp` (a killed codesign
  leaves one and every later sign fails → ad-hoc → TCC re-prompts);
  `bin/grant-permissions.sh` pre-grants mic, speech, Downloads, Desktop,
  Documents (TCC.db rows with the stable cert's csreq) after every build.
- `commands.toml` — config (windows, commands, colors, paths). Valid TOML,
  read LINE BY LINE: every reader/writer goes through `configEntry` /
  `configLine` (Swift) or `jira_config.config_entry` / `config_line`
  (python) — never split on '=' by hand. Values reach code as strings;
  lists are one comma-separated string; one-line entries only.
- `jira/jira_*.py` — python jira poller (see AGENT_CONTEXT.md "Jira poller";
  `jira_log.py` = debug.log + raw response dumps);
  tests: `python3 Tests/test_jira_poll.py`
- `vim/init.lua` — nvim pane init (theme vars `g:ws_*` from `vimArgs`)

## ws-settings — every shortcut + setting (Hyper+/)

- Spec / plan: `PRD-settings-hub.md`, `PLAN-settings-hub.md` (owner
  decisions). Standalone python (stdlib, ≥ 3.11 for `tomllib`):
  `settings_hub/` + launcher `bin/ws-settings` (finds a python ≥ 3.11 —
  AeroSpace's PATH has the CLT 3.9 — sets PYTHONDONTWRITEBYTECODE, `-B`).
  Works with the app quit; the app is only asked to validate / apply.
- ONE python codec: `pylib/config_text.py` (moved out of `jira_config.py`,
  which re-exports `config_entry` / `config_line`; 3.9-compatible):
  + `config_section_entries`, `config_setting` (= Swift `configSetting`),
  `config_line_parts` / `config_set_line` (keep indent + trailing comment),
  `config_entry_span` (rebind swaps only the key), `toml_array`.
  `confluence_config.set_section_value` uses it (it used to drop comments
  and replace a symlinked commands.toml with a file).
- Readers (`readers.py`, each → `KeyRow`): `[shortcuts]` (label parser:
  "Ctrl+Shift+H / J / K / L", "P D A", "Esc Esc", "1-9", gestures),
  AeroSpace (the file AeroSpace reads; service-mode keys carry their entry
  key; a binding that runs our binary is a `mirror_of` its `all:` row),
  herdr `[keys]` + `[[keys.command]]` (prefix = `[keys] prefix` / ctrl+b; no
  defaults), Ghostty `keybind` (`global:` = a global layer), vim (headless
  `nvim --noplugin -u INIT` → `nvim_get_keymap` filtered to the init's sid,
  cached by mtime; static `*map` parse as fallback). Settings
  (`settings.py`): every `[section]` key, docs from the `#   key   text`
  blocks (above the header + inside the body; the file-top "Section keys"
  block as fallback), commented-out `# key = v` rows (`set: false`),
  enums from `a | b | c` in the doc.
- Writes (`writer.py`): realpath (a link stays a link), ONE hunk, the
  app's `config-check --file` on the result, re-read + retry once on a
  concurrent change, mkstemp + fsync + chmod + os.replace; un-comments a
  `# key = …` in place, else after the section's last entry. Undo:
  `~/.cache/workspace-switcher/settings-undo.json` (hunks, byte-identical).
  Validation (`schema.py`) = the APP: `workspace-switcher config-check
  SECTION KEY VALUE` (Swift `configValueProblem`, section rules included)
  + `config-schema` (cached by binary mtime) offline; a doc-comment enum
  miss is only a warning (`--force`). `[jira] enabled` goes through
  `workspace-switcher jira-poll on|off`.
- Apply (`apply.py`, per `settings.apply_mode`): socket / CLI `reload`
  (`reloadConfig()`, replies `{ok, commands, usingBackup, issues}`; never
  starts a daemon), `restart` (`restartDaemon()`: [theme] + launch-only
  [app] keys), trigger sections ([screenshot], [pane-shot]) nothing, views
  read on open ([confluence], [ai], [setup]), `sketchybar --reload`.
  `config-schema` / `config-check` run at the very top of main.swift (no
  AppInstall, no lock).
- Rebind (`rebind.py`, AeroSpace + herdr only — app keys are Swift):
  refuses a key taken in the same mode / layer, clash with another layer
  needs `--force`, `aerospace reload-config --dry-run --no-gui` / `herdr
  config check` after the write → auto-undo on failure, then
  `aerospace reload-config` / `herdr server reload-config`.
- Clashes (`conflicts.py`): global layers (AeroSpace main, Ghostty
  `global:`, `data/system_shortcuts.toml`) beat every other layer; Ghostty
  terminal keybinds beat herdr; same layer + view duplicates; app `all:` vs
  a view (info). (herdr's resize_pane_* Opt+H/J/K/L were removed for the
  AeroSpace focus clash; herdr resize mode = Ctrl+B, R.)
- Picker (`tui/`): `Picker` = model (tests drive it), `View` = curses.
  Keys decoded by hand (`keys.Decoder`: CSI / SS3 / kitty CSI-u /
  bracketed paste / Alt as ESC-x); the picker pushes kitty "disambiguate"
  (`CSI > 1 u`) so Cmd+X / Z / Shift+Z arrive — Ghostty keeps Cmd+A / Cmd+C
  (select / copy its own selection); Cmd+V = bracketed paste. Ctrl+C =
  copy (quits only with an empty search), Ctrl+V = pbpaste. Esc: editor →
  search + filters → quit. Separators are ACS lines (a long "─" run becomes
  REP, which Ghostty draws once). No LANG under `open` → forces a UTF-8
  locale. Every cell is painted each frame (unwritten cells kept the
  terminal's own bg). Mouse = SGR 1006 (`?1000h ?1006h`): `View.hits` =
  (y, x0, x1, action) from the last draw — column headers sort
  (`Picker.sort_by`: ▲ → ▼ → source order; `View.columns` is the ONE
  layout for rows, headers and clicks), ⌃ ⌥ ⇧ ⌘ ★ ⚠ toggles, row click /
  double-click (= Return), wheel. ⚠ / Alt+W = clashes first. Test hook
  `WS_SETTINGS_STATE=FILE` (JSON after every key / click).
- Hyper+/ = `ws-settings open`: focuses the window titled `ws-settings` if
  AeroSpace lists it, else `[settings-hub] terminal-command` (default `open
  -na Ghostty --args --title=… --command=…`: `-e` or ANY `--keybind` flag
  makes Ghostty ask "Allow Ghostty to execute …?"), then waits for the
  window and moves + focuses it on your workspace. Its own
  `--macos-titlebar-style=transparent --macos-window-buttons=hidden
  --background=<[theme] background>` = a plain strip in the picker's color
  to drag by (the owner's config hides title bars → nothing to drag; the
  picker draws on the terminal's default bg so strip + body match) and `--window-position-x/y` = centered: points
  from the primary screen's visible top-left; screen size via JXA AppKit,
  cached per `aerospace list-monitors`; window size from the picker's
  last TIOCGWINSZ pixels (`settings-hub-size.json`). aerospace.toml float
  rule by title sits BEFORE the Ghostty → workspace 1 rule. ~0.4-0.9 s
  hotkey → painted (a cold Ghostty instance; the picker paints in ~20 ms).
- Favorites: `settings-hub.json` in the home (gitignored). Tests:
  `bin/run-tests.sh settings` (`Tests/test_settings_hub.py`: fixtures +
  stubs, PTY-driven picker; `WS_LIVE=1` adds the running-daemon round trip).

## Tool panels — /filefast, /paths, /prettyprint, /health-checks

- They act like their OWN apps, never part of the shared window. Root
  cause they fix: macOS activation is per app, and every shown
  `PopupWindow` raises itself on `didBecomeActive` (`installMonitors`) —
  so anything that activated the app for a tool (a titled window, a click,
  `focusSubWindow`, the palette's `restore: true` hand-back) brought the
  shared window along.
- RULE: a tool panel never activates the app and never reacts to app
  activation. `PopupConfig.toolPanel`: always a borderless
  `.nonactivatingPanel` `PopupPanel` (even with editMode / enableDrag),
  `hidesOnDeactivate = false`, no `didBecomeActive` observer, `takeFocus`
  never activates. Header ✕ + buttons work through the shared
  `HeaderClickWindow` band (`HeaderClickTracker`, also PopupBaseWindow's).
  AeroSpace ignores them (NSPanel = AXSystemDialog, no close button): they
  float (`float` default true), stay up across workspaces, Esc / ✕ closes.
- `NSApp.isActive` reads TRUE while a tool panel is key even though no
  activation happened (the other app stays frontmost) — never use it to
  tell "the user is in our window"; `userInOurWindow` skips a key tool
  panel (Hyper+N from one focuses the shared window).
- Host: `isToolPanel(cmd)` (filefast, paths, prettyprint, screenshot by name + output commands with
  `panel = true`, i.e. `[health-checks]`), `openTool` = the ONE opener
  (palette `accept` hides with `restore: false`; `do:tool:NAME`),
  `raiseToolPanel` (re-open: orderFrontRegardless + makeKey, no
  activation), `reclaimToolKey` (the 0.25 s key take-back after the palette
  hides). NO focus hand-back on close: the app never activated, so the
  keyboard returns to the frontmost app by itself (a saved window id could
  sit on a workspace you've left — `aerospace focus` would yank you back).
  Excluded from both focus bridges; `panel` output commands are not
  `.output` slot views.
- Test: `bin/ui-test-focus.py tools` (state `tools` = each panel's
  testState + wid / level incl. `header` button rects, `activations`,
  `frontmostPid`; `do:tool-close:NAME`): open / re-run / click each with
  another app frontmost → `activations` unchanged, frontmost not us, the
  shared view not key and not moved, AeroSpace doesn't list it.

## /paths — recent-file shelf (Hyper+S → "file paths")

- `PathShelf.swift` (AppKit-light, tested: `bin/run-tests.sh paths`):
  `PathShelf.shared` = ≤25 rows (`[paths] limit`, hard cap 25), newest
  first, `~/.cache/workspace-switcher/paths.json`; rows are CANONICAL
  (`realpath`: a symlinked folder is one row, /tmp = /private/tmp).
  Feeds: `RecentFiles.onKept` / `onRenamed` (the ONE FSEvents stream; needs
  `[files] recent` on) → regular files only → `IgnoreRules`; `ClipboardPaths`
  (0.5 s `changeCount` timer; file URLs or text that is 1-5 existing paths;
  nspasteboard.org concealed / transient skipped; our own copies via
  `ownWrite`); filefast saves, Files-view Copy Path, `FileDrag.onDragOut`.
  Clipboard / explicit paths skip the ignore rules; files or folders.
- `IgnoreRules`: gitignore syntax, git / ripgrep precedence — global git
  excludes < per folder `.gitignore` (inside a repo only) < `.ignore` <
  `.rgignore` (deeper wins) < `config/paths.ignore` (`[paths] ignore-file`;
  ~/ and absolute patterns OK). Native regex matcher, cached per folder,
  re-stat ≤ every 2 s; an ignored folder hides everything below (git).
  Parity-tested against `git check-ignore`. `load()` re-applies the rules
  to stored rows.
- `PathsWindow.swift`: filefast's recipe — a `PopupWindow` (switcher look,
  `onFilter` = query hook, `onKeyPreview` = all keys) + the file browser's
  `FileListPane` in a scroll view under the filter box (drag-out = real
  file drags, multi-select, right-click menu limited by `canPerform` to
  Quick Look + Copy). Built once, kept (`showPersistent`); sticky + floating
  (stays above Webex while you drag); placed on the mouse's screen, top at
  20%. Return = `[paths] return` (file | path | open): "file" =
  `writeFiles` — one pasteboard item per file with `.fileURL` AND the path
  as `.string` (Cmd+V attaches in chat / mail apps, pastes the path in a
  terminal). Cmd+C path text (a filter-box selection wins), Cmd+Shift+C
  file, Space (empty filter) / Cmd+Y Quick Look, Cmd+O, Cmd+R rename,
  Cmd+Shift+R reveal, Cmd+Delete forget a row, Esc / Cmd+W close.
- Controller: `configurePathShelf()` (from `configureRecentFiles`, i.e.
  launch + every reload; no `[paths]` = all feeds unhooked), `showPaths`,
  `pathsWindow`, `clipboardPaths`. Socket: `do:paths:show|hide|return|
  select:N`, state `paths` {shown, key, level, rows[{path, why}], frame}.

## /pane-shot — full-height capture of the focused herdr pane

- Chrome's "full size screenshot" for a terminal pane: never scroll, lay
  the text out again off screen. NOT Ghostty: herdr draws its whole UI as
  one full-screen app inside Ghostty, so Ghostty's scrollback / search /
  `write_scrollback_file` never hold one pane's history. herdr's server
  does: `herdr pane read ID --source recent --format ansi --lines N` (SGR
  colors, soft-wrapped at the pane width, ~15 ms). Server cap: 1000 rows
  (`Herdr.maxLines`; `--lines 6000` over the raw socket still gets 1000).
  Alternate-screen apps (nvim, htop) have no scrollback → the screen only.
- `workspace-switcher pane-shot [--pane ID] [--lines N|all] [--file PATH|-]
  [--no-save] [--no-copy]` (main.swift → socket `pane-shot<TAB>args`, the
  CLI waits for ONE reply line: saved path / `copied` / `error: …`). No
  `--pane` = `herdr pane current` = herdr's focused pane (works from any
  process; `Herdr.environment` drops `HERDR_PANE_ID` etc. a daemon started
  from a pane would inherit). Rows = the pane's viewport + `[pane-shot]
  lines` (200). `--file -` = stdin (the CLI writes a temp file).
- `PaneShot.swift` (Foundation): `PaneShotArgs`, `PaneShotConfig`
  (`[pane-shot]`, not a palette command), `Herdr` (JSON answers on stdout;
  errors = `{"error":…}` on STDERR + exit 1). `AnsiRender.swift`
  (Foundation + CoreText): `AnsiGrid.parse` (SGR incl. 256 / truecolor,
  CR / tab / OSC handling, wide cells, trailing blank rows trimmed),
  `AnsiTheme.ghostty` (`ghostty +show-config`: font, size, fg / bg, all
  256 palette entries, bold-is-bright, display-p3), `AnsiRender.image`
  (glyphs the font has are pinned to their cell with `CTFontDrawGlyphs`;
  the rest = `CTLineDraw` fallback — which MOVES the text position, so it
  is reset before every pinned glyph; 2×, 1× past 32k px).
- Delivery: `ScreenshotController.paneShot` (end of Screenshot.swift) →
  `copy` (PNG + TIFF, the toast) + `save` (`[screenshot] save-path` unless
  `[pane-shot] save-path`, → /paths). Log `pane-shot ID: N rows, W×H px,
  M ms` (260 rows ≈ 0.7 s, mostly the PNG + TIFF encode). State
  `paneShot` {pane, title, rows, size, copied, path, ms | error}.
- Trigger: a herdr `[[keys.command]] type = "shell"` binding running the
  app binary (dotfiles), or anything else that can run the CLI.

## /screenshot — Flameshot-style capture (Hyper+X)

- Spec: `PRD-screenshot.md`. Hyper+X (aerospace.toml) runs the binary
  `workspace-switcher screenshot`; main.swift sends `screenshot<TAB>args…`
  (tab-separated: paths may hold spaces); `-r` / `-g` use `sendRequest`
  (the reply comes on the same connection when the user finishes; the
  socket thread hands the fd to the session, never blocks the accept
  loop); no daemon → `bin/workspace_switcher.sh screenshot` cold start.
  Palette `/screenshot` = `openTool` → `showScreenshot` (0.15 s so the
  palette isn't in the frozen image). `[screenshot]` in commands.toml
  (read on every trigger via `ScreenshotConfig.load`; dist default =
  Flameshot's values, build-app.sh awk).
- `ScreenshotAnnotations.swift` (no AppKit, `bin/run-tests.sh screenshot`):
  `ShotTool` (ring names, letters, symbols, tooltips, default sizes;
  `ring(spec, badge:)` puts the W/H badge after the last drawing tool),
  `ShotObject` / `ShotDocument` (SNAPSHOT undo, `undo-limit`, `coalesce`
  for wheel notches, `updateLive` while dragging = one step on mouse-up,
  counters renumbered after every change with per-bubble `numberOffset`),
  `ButtonRing.layout` (port of Flameshot's ButtonHandler incl. the
  all-blocked → inside fallback; a grown tiny selection is shifted on
  screen first), `ShotSnap`, `ShotPixelate.secureBlocks` (blocks ONLY from
  the 1-4 px band outside the rect, averaged + smoothed; grid in points),
  `ShotRenderer` (ONE drawing path for the overlay and the output;
  top-left point space; `render` = crop × backing scale), `ShotFiles`
  (strftime pattern, " 2" clash suffix, `-p` dir/file), `ShotArgs`
  (Flameshot's CLI), `ShotState` (`~/.cache/workspace-switcher/
  screenshot-state.json`: per-tool sizes, color, text style, grid, last
  region).
- `Screenshot.swift`: `ScreenshotController` (`SwitcherController.screenshot`,
  prewarmed 1.5 s after launch: one `ShotOverlayPanel` per display +
  `SCShareableContent`, refreshed on screen changes). Trigger: permission
  (`CGPreflightScreenCaptureAccess`; no → `CGRequestScreenCaptureAccess`
  once, toast, Privacy pane, NO overlay) → optional delay →
  `SCScreenshotManager.captureImage` per display in parallel (points ×
  backing scale, no cursor) → `begin`. Log: `screenshot: capture N ms, M
  ms to overlay` (≈ 90 / 115 ms on the owner's Mac). Outputs: copy = ONE
  pasteboard item PNG + TIFF; save → `PathShelf` (`why = screenshot`) +
  toast; pin → `PinPanel`; `ScreenToast` = the toast pill
  (`makeToastPill` / `animateToastPill`, shared with
  `PopupWindow.showToast`) on its own non-activating panel.
- `ScreenshotOverlay.swift`: `ShotOverlayPanel` (borderless
  `.nonactivatingPanel`, `.screenSaver` level — set AFTER
  `isFloatingPanel`, which resets it to .floating — covers the menu bar /
  sketchybar, `constrainFrameRect` passthrough); the frozen image is the
  unflipped backdrop layer's `contents`, `ShotOverlayView` (flipped)
  draws veil + selection + objects in dirty rects. `ShotSession` = the
  brain: per-display selection (a drag on another display moves it there,
  its drawings reset), mouse (handles: Shift mirror / Cmd aspect; drag
  inside = move; a click on an object selects + moves it), keys
  (`handleKey`, from the controller's local keyDown monitor: tool
  letters, arrows, Cmd+C/S/A/M/Z/Q, Delete; anything else swallowed so
  Cmd+Q never quits the daemon; a focused NSText gets typing +
  `JiraEditKeys.route`), Esc chain: save card → text → shortcuts card →
  color wheel → grab color → side panel → selected object → tool → close.
  Views: `ShotButton` (80 ms emerge), help card, Tool Settings tab, size
  indicator, `ShotWheelView` (right-click), `ShotLoupe` (G / magnifier),
  `ShotTextField` (NSTextView in the object's font), `ShotSidePanel`
  (size, color, HSV disc, hex, grid, text style, Layers), `ShotSaveCard`.
- Copy Text mode (OCR, ScreenOCR-style): `ScreenshotText.swift` (Foundation
  + Vision, no AppKit) — `ShotOCR.recognize` (`VNRecognizeTextRequest`
  .accurate, auto language unless `text-languages`; crops < 64 px tall
  upscaled 2×) + `ShotOCR.join` (rows by vertical center, left → right, a
  gap taller than a line = blank line). Overlay: `ShotSession.textMode`
  (Tab / O / `ShotModePill` top-center; `start-mode`, CLI `screenshot
  text`): no ring / tab, dashed marquee, mouse-up = `finish(.text)`;
  ⇧⌘C / ring `copy-text` from screenshot mode. `ShotOutcome.text` →
  `ScreenshotController.deliverText`: the crop WITHOUT drawings, Vision off
  main, one `.string` item (`copyText`, `onOwnPasteboardWrite`), toast
  `text-toast` / `no-text-toast`; `-r` answers the text. Hooks:
  `do:screenshot:show-text | mode:text|screenshot | text`; state
  `textMode`, `modePill`, `last {outcome: text, chars, text, ms}`.
- Save (spike A5): an `NSSavePanel` shown from the non-activating overlay
  appears but never gets the keyboard (key window empty, the other app
  stays frontmost); clicking it ACTIVATES the app. So Cmd+S opens
  `ShotSaveCard` ON the overlay: a path field (save-path + pattern, stem
  selected), Return saves, Esc closes only the card
  (`sendsActionOnEndEditing = false`, or leaving the field saved).
  `save-path-fixed` / `-p` skip it.
- Permission: TCC Screen Recording lives in the SYSTEM database —
  `bin/grant-permissions.sh` can't pre-grant it; the stable signing cert
  keeps the grant across rebuilds. Granting it restarts the daemon.
- Test hooks: `do:screenshot:show[:MS] | select:X,Y,W,H | tool:NAME|none |
  draw:X1,Y1,X2,Y2 | key:SPEC (cmd+shift+z, esc, left…) | copy | accept |
  save:PATH | save-ok | pin | unpin | close | side-panel`; state
  `screenshot` {shown, permission, displays[{id, frame, scale, key, wid,
  level}], selection, buttons[{name, x, y, w, h}], tool, size, color,
  objects[{type, bbox, number}], selected, canUndo/Redo, sidePanel,
  helpShown, editingText, wheel, grabbing, saveCard, pins, pinStates,
  last {outcome, size, path, copied}}. `bin/ui-test-focus.py tools` has a
  screenshot case (0 activations, frontmost unchanged, AeroSpace doesn't
  list the overlay).

## AI view

- `AIWindow.swift` — `AIWindow` (a `CardWindowController`; `SlotMember`, view `.ai`, nav id 66 placed RIGHT AFTER notes
  in `SharedWindow.navIcons`, `[app] ai-icon` ("" = violet sparkles glyph);
  shares the frame). `[ai]` in commands.toml: enabled, fm-bin, pandoc-bin,
  rules-dir, context-tokens (4096), split, font / font-size, copy-toast.
  Opened from the Hyper+S palette (`/ai`), the menu bar, CLI / socket `ai`.
- Rules = every `.md` in `rules-dir` (repo `rules/`: `grammar-check` =
  grammar ONLY, `then:` → `markdown-format` = layout ONLY (also its own
  pill), `ask` = free-form prompt, plain). ONE job per rule: fm's small model
  given grammar + formatting in one prompt duplicated text and reworded.
  Extra keys: `prompt:` (a line before the text — without it the model
  ANSWERS the draft / follows instructions inside it), `then:` (chain:
  `AIRule.chain`, the next rule gets the answer; a later step that fails
  keeps the earlier answer), `keep-words:` (`WordGuard`: a layout answer that
  adds/loses words is thrown away, status warns), `csv-tables:`
  (`CSVTables.convert`: comma rows → a Markdown table, in code, before the
  model). `AIRule` lives in `AIFormat.swift` (testable without the window).
  Tests: `bin/run-tests.sh ai` (pure), `ai-live` (the rules through fm). One
  `PopupTabsBar` pill each (`closable = false`, `menuFor` right-click: Edit
  in Notes → `openNoteFile`, Reveal, Copy Path, Duplicate, Delete). "+" =
  `jiraFormSheet` → a template file, opened in notes. Dir watched
  (`DispatchSource`), re-read on show and before every run.
- `AIRule.load`: `---` frontmatter (name, output diff|plain, greedy,
  guardrails, use-case, model, placeholder, protect-code, chunk; unknown
  keys → ⚠ in the command line) + body = `-i` instructions, REFLOWED
  (`Reflow.instructions`: fm's ~3B model ignores hard-wrapped rules — it
  wrapped answers in ``` and added emoji until the lines were joined).
- Run (`AIFormat.swift` helpers): `CodeGuard` swaps fenced blocks + `inline`
  for `[[CODEn]]` (+ one instruction line) and restores them; a dropped
  token → warning status. `TokenBudget.parts` splits long text at blank
  lines to fit `context-tokens` (chunk: default for diff rules), parts run
  one after another. Own `Process`: `fm respond --stream -i … [flags]`,
  input on STDIN, stdout streamed (reader thread posts chunks then the
  finish in order; `runGen` drops stale runs). `AnswerCleanup.unwrapFence`
  drops a whole-answer ``` wrapper BEFORE restore. Footer = `fm count-tokens
  -q` (debounced) vs the window. `fm available` checked once.
- Right pane: `ConfSegmented` over `PaneMode` Diff | Markdown | Outlook |
  Webex (click; plain rules drop Diff). Diff = `WordDiff` (spacing-only changes are not
  marked). Outlook / Webex = `WKWebView` preview of EXACTLY what Copy
  writes: `RichText.html` = pandoc `-f gfm -t html --syntax-highlighting=none`
  + every style inline (`styled`: Aptos 11pt, bordered tables, code
  blocks); Webex has no tables → `tablesAsText` (aligned block in a fence).
  ⧉ Copy = ONE pasteboard item: html + rtf (NSAttributedString
  from the HTML, main thread) + the Markdown as text; the button follows
  the last preview picked (`aiTarget`). No pandoc → Markdown only.
- Keys (local monitor): Ctrl/Cmd+Return run, Cmd+L input, Cmd+/ shortcuts
  (`[shortcuts] "ai: …"`), Esc = stop a run (never closes), Cmd+C/A in the
  preview, rest → `JiraEditKeys.route`.

## Compare view (PRD-compare.md; Text Compare + Folder Compare)

- Files: `CompareText.swift` (Foundation only): `TextSide` (decode: UTF-8 ±
  BOM, UTF-16 LE/BE with BOM, else Latin-1; NUL in the first 8 KB = binary;
  per-line `EOL` incl. `none` for a last line without newline; `encoded()`
  is byte-exact, nil when Latin-1 can't hold an edit → saved as UTF-8),
  `Importance` (`key(line, eol)` = the comparison key; `.exact` = git's
  view), `LineDiff` (a PORT of git's xhistogram.c + xdl_change_compact +
  the indent heuristic; Myers via CollectionDifference where git falls
  back; a work list, no recursion), `TextCompare` (rows `CompareRow` l/r
  line or -1 filler, kind same/changed/leftOnly/rightOnly + `important`;
  sections = runs of `isDiff` rows; `replace` → `TextSide.replace` →
  `rediff` = only ± `rediffContext` (50) same rows around the edit;
  `copyRows` / `copySection` = Ctrl+R / L; per-side undo stacks), `CharDiff`
  (the old AI `WordDiff`, moved here: `diff` / `changes` for the AI view,
  `marks` = changed spans for drawing), `BinaryCompare`.
  `ComparePane.swift`: `CompareSession` (one pair + view state: filter →
  `visible` display rows, cursor/anchor/focus, dirty = side's undo count vs
  `cleanDepth`, `waiters` for `--wait`, DispatchSource `watchers`),
  `ComparePaneView` (ONE flipped document view draws both sides + the
  gutter → synced scroll; dirty-rect rows only; marks cached per
  `version`), `CompareThumbnail` (1 px wide bitmap, cached), `CompareDetails`
  (line details), `CompareEditor` (the section editor: NSTextView laid over
  one side's section; text = lines each + "\n"; commits on click away,
  Ctrl+N/P, Esc, Cmd+S, park). `CompareWindow.swift`: `CompareConfig`
  (`[compare]` read once per open/show; `*-label` strings), `CompareRecent`
  (`~/.cache/workspace-switcher/compare-recent.json`; git + pasted sessions
  never stored), `CompareWindow` (a `CardWindowController`; `current` =
  `.compare` with session pills + "+" start page, `sub` = `.compareText`).
- Shared window: `SlotView.compare` (nav id 67 `navCompare`, after
  confluence in `navIcons`, `compareEnabled()`), `.compareText` (`isSub`,
  `push` puts `.compare` under it, `back` returns to it), `.compare` in
  `escViews`. Dismissal = the shared paths (✕ / Cmd+W → `onSlotHide`,
  Hyper+N `toggle`, focus loss `checkFocusLoss`, Ctrl+Tab via
  `CardWindowController.routeKey`). Esc chain (`CompareWindow.escape`): find
  bar / path field / section editor → a running diff (> 60k lines runs off
  main, `diffGen`) → row selection → `.compareText` back → `escapeAtTop`.
  `slotPark(stopVoice:)`: commits the editor; a whole-window hide answers
  every `--wait` and drops clean git sessions.
- Entry: palette `/compare` (`paletteCommands`, `showCompare`), header icon,
  menu bar "Compare…", `FileListPane.onCompare` / `comparePick` (file
  browser + /paths right-click: "Select for Compare", "Compare to “NAME”",
  "Compare" with two marked; two files or two folders), drops on a
  pane, clipboard (an empty side takes Cmd+V; right-click "Paste Clipboard
  Here"), CLI `workspace-switcher compare [--wait] [--title1 T] [--title2 T]
  LEFT [RIGHT]` (main.swift makes paths absolute, socket `compare<TAB>…`;
  `--wait` = `sendRequest`, the daemon writes `done` when the session closes
  or the window hides; no daemon → `open -g` the app + retry). git:
  `difftool.ws.cmd = …/workspace-switcher compare --wait --title1 "$BASE"
  "$LOCAL" "$REMOTE"`. `compare` alone = a hotkey mode (`hotkeyModes`).
- Keys: `CompareWindow.handleKey` (§7.2.4 of the PRD; `[shortcuts]
  "compare: …"` rows, Cmd+/ sheet). Cmd+K = an NSMenu of every action
  (card windows have no `showActionPicker`). Typing / Return / double-click
  opens the section editor; inside it the text view owns the keys (edit
  keys via `JiraEditKeys.route`).
- Config `[compare]`: enabled, in-palette, label, esc-close, font /
  font-size (default notes), context-lines, tab-width, ignore-* (importance
  defaults), gutter-arrows, max-lines, recent, `*-label`; content /
  time-tolerance / exclude / use-gitignore / ignore-file are for Folder
  Compare (phase 2). Validation: `configValueProblem` section "compare"
  (`recent` is a count here, a switch in [files]) + `configNumberKeys`.
- Log: `compare text: N lines (both sides), diff M ms, paint P ms (open →
  painted T ms)`, `compare edit: N rows, re-diff M ms`.
- Hooks: `do:compare:open:L|R`, `open-sub:L|R` (push `.compareText`),
  `paste:left|right:TEXT` (`\n`), `edit:left|right:TEXT`, `next`, `prev`,
  `copy-right`, `copy-left`, `filter:NAME`, `swap`, `save:left|right`,
  `back`, `undo`, `redo`, `cursor:N`, `select:N`, `start`, `close-session[:force]`,
  `close-all`, `sheet-cancel`, `key:SPEC` (through `routeKey`). State
  `compare` {view, sessions, startPage, sheet, close (✕ in cliclick
  coords), current {sections, important, unimportant, cursorRow, filter,
  focus, rows[0..50], editing, scrollY, …}, subView, folder: null}.
- Tests: `bin/run-tests.sh compare` (round trips, importance, rows, copy +
  undo, windowed re-diff fuzz vs full diff, ≥ 50-pair parity corpus vs `git
  diff --no-index --histogram --indent-heuristic -U0` from seeded mutations
  of repo files + repo history, timings); `bin/ui-test-focus.py compare`.
- Recent rows: ONE click opens; `CompareRecentList.refreshMissing` (per side,
  re-run on every open) → ⚠ + "missing" + the gone side struck through +
  tooltip; opening one explains itself in `showStartHint` (sets
  `start.needsLayout` — root alone left the hint 0 pt tall = "nothing
  happens"). Right-click ▸ Remove All Missing.
- Pasted text: start page "Compare Pasted Text…" (`startPasted`: the clipboard
  = left, focus right); Cmd+V into ANY pane (`pasteClipboard` → `setPasted`: an
  empty side takes it, a side with text is REPLACED by an undoable
  `model.replace`; a copied file opens instead). A pair with a pasted side
  becomes a Recent row when its session closes (`rememberPasted`: both sides
  written to `~/.cache/workspace-switcher/compare-pasted/`, pruned by
  `CompareRecent.save`; the row reads "pasted text ⇆ …"). Hook
  `do:compare:start-paste`.
- Folder Compare: `CompareFolder.swift` (Foundation only: `FolderScan.run` =
  both trees walked off main + paired by relative path — case per volume,
  NFC = NFD, symlinks as links; quick test = size + mtime ± `time-tolerance`;
  `FolderTree.settle` = a folder's color is what is below it, `unknown` until
  the content answers; `FolderContent` = 1 MB byte compare with early exit +
  the Text Compare normalizer for "unimportant" (blue); `FolderTree.rows` =
  filter / flatten / name filter (`*.swift, !*.o`)). `CompareFolderView.swift`:
  `FolderSession`, `FolderTreeView` (ONE drawn view for both sides + the glyph
  column), `FolderPage` (toolbar, keys, menus, file actions). A folder session
  is a `CompareSession` with `.folder` set (text model empty); `openPair` sends
  two folders to `openFolders`, folder + file refuses with a hint. Return /
  double-click on a file pair = `folderOpenPair` → `createSub` +
  `openSubPair` + `slot.push(.compareText)` (Esc = `slot.back`, the folder
  keeps its cursor + expansion). Actions go through `FileOps.place` (copy /
  move to the mirrored path, parents made, clash = Replace (old → Trash) /
  Keep Both / Skip from a sheet, ONE undo record `.group`); a folder pair is
  merged (only differing / orphan children move), identical items skipped.
  After any op the page rescans (`rescan(keepStatus:)`, expansion + cursor by
  key; `contentCache` by path + size + mtime avoids re-reading). Trash / rename
  / new folder act on the focused side (Tab). Cmd+Z = `FileOps.undo()` (the
  GLOBAL stack, shared with the file browser). Config: `content`,
  `time-tolerance`, `exclude`, `use-gitignore`, `ignore-file`. Hooks:
  `do:compare:open:A|B` (folders too), `folder-filter:NAME`, `folder-flatten`,
  `folder-names:GLOBS`, `folder-expand:all|none`, `folder-select:REL`,
  `folder-copy|move:right|left[:replace|keep|skip]` (TO that side; the clash
  answer is pre-set), `folder-trash`, `folder-undo`, `folder-open`,
  `folder-focus`, `folder-rescan`, `folder-hidden`; state `compare.folder`
  {left, right, scanning, checking, filter, counts, summary, status, rows[0..200]
  {rel, status, newer, depth, dir, expanded, left, right}}. Log `compare folder:
  N items, scan M ms`. Tests: `bin/run-tests.sh compare` (test_compare_folder.swift:
  pairing, statuses, content + rules, roll-up, filters, `FileOps.place` + undo).
- Folder Compare safety: `FolderNode.sameByMetadata` = a `.same` from size +
  mtime alone (auto / never; cleared by a content answer) → dim "=", summary
  "N same (M by date/size only)", no green "Identical", status hint
  (`same-by-date[-hint]-label`). Questions are themed cards IN the window,
  never NSAlerts: `CardWindowController.confirm(…)` → `ConfirmOverlay`
  (CardWindow.swift; owns every key in `routeKey`: Tab / ← → focus ring,
  Return / Space press, Esc = cancelIndex; default button = `.primary`,
  risky = `.danger`): clash (Cancel default, Replace red), sync preview
  (Cancel default; Trash first in danger, overwrites in warning, in a
  recessed well; Synchronize red when it trashes or overwrites), unsaved
  close (Save default, Don't Save red). Cmd+/ = `showShortcutsCard` → the
  shared `ShortcutsOverlay` (PopupWindow.swift; also what PopupWindow's
  `showShortcuts` draws), current mode's group first. Sync is a worded
  raised button (`sync-button-label`), apart from the icon row. State
  `compare.sheet` covers the card, `shortcutsCard`; `sheet-cancel` cancels it.
- Folder Compare undo: each `FolderSession` owns a `FileOps.UndoStack`
  (`undo`, carried to its successors), so its Cmd+Z never takes back a file
  browser op; `FileOps.shared` = the browser's. Every FileOps call takes
  `undo:` (default shared); `UndoStack.collapse(since:_:)` makes a multi-step
  run (Synchronize) ONE step.
- Phase 3 (folders): Space / Cmd+Y = Quick Look (`FolderPage.toggleQuickLook`,
  QLPreviewPanel data source like /paths). New roots = a successor session
  via `FolderHost.folderSwapped` (view, rules, undo kept): `setBase` (right-click
  Set as [Left / Right] Base Folder, Cmd+Down), `upOneLevel` (Cmd+Up),
  `goBack` / `goForward` (Cmd+[ / ], `back` / `forward` stacks of root pairs).
  Drag: `FolderTreeView` is a drag source (`FileDrag.begin`, file URLs, Finder
  too) and a drop target: our rows on the other side = `transfer` (Cmd = move),
  Finder files = `FileOps.transfer` into the folder under the drop. Synchronize
  (toolbar ⟳, Cmd+K, kitchen sink): `SyncPlan.make(tree, SyncMode, nameFilter:)`
  (CompareFolder.swift: Update Right / Left / Both = newer + orphans, ties
  skipped; Mirror = every difference + the far side's orphans to the Trash) →
  NSAlert preview sheet → `runSync` (place with clash replace, trash, collapse).
  Links: a symlink facing a regular file compares by its TARGET
  (`FolderSideInfo.asFile`); git sessions force `content = always`
  (`alwaysContent`: git writes the left copies at run time). `git difftool -d`
  verified for real: links edited through, hide → git returns (also from the
  pushed `.compareText`: `slotPark` finishes `CompareWindow.current`'s waiters).
- Phase 3 (text): Align With = `TextCompare.anchors` (left/right line pairs;
  `buildRows` diffs between them; any anchor → full re-diff on edits, anchors
  shift, a line-for-line rewrite keeps them); right-click Align With… / Align
  With Picked Line (`alignPick`, dashed outline; anchor rows get a ⚓︎ rule).
  Convert ▸ `trimTrailingWhitespace` / `convertLineEndings` (one undo step,
  `replace(_:_:lines:eols:)` = exact endings). Show Whitespace (kitchen sink,
  UserDefaults `compareWhitespace`): spaces as dim ·, tabs as →.
- Restore: `persistSoon` (2 s debounce from `showPage` / `syncAll` /
  `folderChanged`) / `persistNow` (park, willTerminate) write
  `compare-sessions.json` (pairs, titles, filters, cursor, importance,
  anchors; folder view + cursor key; git sessions never) + `compare-recovery/`
  (`ID-side.txt` = the side's own encoding: dirty sides and pasted sides; files
  no session needs are pruned). `restoreSessions()` (main window init) reopens
  them (`openPair(…, then:)` applies importance, recovery as ONE undoable
  replace → dirty + `recovered`, anchors, filter, cursor); Recent untouched
  while `restoring`. Status `recovered-label`.
- Hooks (phase 3): `folder-sync:MODE[:preview]` (state `folder.syncPlan`),
  `folder-base:REL[:left|right]`, `folder-up|back|forward`, `folder-quicklook`
  (state `quickLook`), `folder-drop:SIDE[:PATHS]` (no paths = drag across);
  `align:L,R` (1-based), `align-clear`, `trim:SIDE`, `eol:SIDE:lf|crlf|cr`,
  `whitespace:on|off`; state `current.anchors / eol / recovered`,
  `folder.canUndo / sharedCanUndo / back / forward`.
- Not built: word wrap, syntax highlighting (phase 3 "if wanted"), Isolate.

## Notifications pill (sketchybar)

- `config/sketchybar/plugins/notifications.sh` (sourced AFTER status.sh →
  its chips are the LEFT END of the status group: no own group, it re-runs
  status.sh's `status_bracket`, which spans `status.*` + `notif.*`) only builds items;
  every tick / `notifications_update` event runs `notify/notify_poll.py
  --tick` (config, badges, cached state → ONE `sketchybar --set` batch).
  `[notifications]` in commands.toml (skipped by `loadCommands`, not a
  palette command); `enabled = false` → no items. `updates=on` on
  `notif.tail` so a hidden pill (`hide-when-zero`) keeps ticking.
- Per source (`sources`, `NAME-enabled/app/tag/icon/api`): items
  `notif.NAME` (icon = `app.<bundle-id>` image, `NAME-icon = "app"`, else the
  text), label = the count: an iOS-style red badge ON TOP of the icon's corner (`chip_args`: narrowed `icon.width` makes the label overlap; no separate `.n` item), `.at` ("@N", or an amber dot = API error).
  Webex count (`webex-count = "window"`) = the Messaging-tab badge in the
  Webex WINDOW's AX tree (`helpers/webex_unread.swift`: `WTMessagingHubButton`
  value indicator + unread rows of `spaces_list` → popup rows; Webex draws NO
  Dock badge; `notify_poll.unread` falls back to the Dock when unreadable).
  `webex-api = false` by default (count only, no OAuth, no amber dot).
  Other sources: count = the badge the DOCK draws (`helpers/dock_badges.swift` via AX
  `AXURL` + `AXStatusLabel`, built into ~/.cache/sketchybar on demand; needs
  Accessibility for sketchybar, else poll.log says so). `lsappinfo
  StatusLabel` misses UserNotifications badges (Messages) — only the
  fallback for apps not in the Dock. Mentions are zeroed when the badge is 0.
- Click (`--event`, `$SENDER` mouse.clicked / mouse.exited.global on both
  items) → `popup_rows` rebuilt as `notif.pop.*` in `popup.notif.NAME`
  (text = icon slot, right text = label): header, sign-in (`login-command`),
  mentions + unread spaces (`webexteams://im?space=UUID` via `space_link`),
  Open, Refresh.
- Webex API (`notify/webex_api.py`, stdlib urllib): Integration OAuth
  (`--login`, local redirect server on 127.0.0.1:8765), creds + refresh token
  in `~/.config/notifications/webex.json` (0600, never commands.toml);
  401 → refresh once; 429 → Retry-After ≤ 30s. Unread = room lastActivity >
  my membership's lastSeenDate; mentions = `mentionedPeople=me` in group
  rooms newer than that (or `mention-max-age-hours`). Background `--poll`
  (fcntl lock) every `poll-seconds` → `~/.cache/notifications/state.json`.
- iMessage (`imessage`, com.apple.MobileSMS) + Outlook = badge-only sources (Outlook: enabled, Dock badge; each chip hides on its own at zero; Graph API later → add to
  `API_SOURCES`). Tests: `python3 Tests/test_notifications.py`.

## Confluence search

- `Confluence.swift` — `ConfluenceWindow` (a `CardWindowController`; `SlotMember`, view `.confluence`, nav id 65,
  `[app] confluence-icon` = `confluence_icon.png`; no header setup button —
  Setup = icon menu, menu bar, and the "Set Up Confluence…" button in the
  empty preview). Strip, 3 rows: Search | ★ Favorites · All words / Phrase /
  Any word · Title only · Search (top-left); the search box (full width);
  Spaces + Contributor `JiraMultiPicker`s, Type / Modified / Sort
  `JiraChoiceButton`s. Esc closes an open picker first. Search and Favorites
  never mix: an empty Search shows a hint; Favorites = pinned pages for
  opening (typing filters, Return opens, the Search button searches inside
  them). Filter changes are debounced (0.35s), previews 0.25s. Results = `ConfTableView` + drawn
  `ConfResultCell` (hit ranges are UTF-16 from python). Preview = `WKWebView`
  (first WebKit use): page HTML in a themed template + a JS highlighter (hits
  bar "1 of N", Cmd+G; the title is marked but not a stop); page images go
  through `wsconf://` (`ConfluenceImageLoader`, URLSession + the auth header
  read from the config file); links open in the browser. In-memory only: 20
  pages + 80 images. Esc = clear the query, never closes.
- Shares the shared window's frame like every view (no own `bigFrame`
  any more: switching views must never resize the window). `[confluence]
  width/height` only size the standalone window (`shared-window = false`).
- Python: `confluence/confluence_api.py` (one JSON object on stdout; `--check
  --save --detect-auth --add-space --remove-space --search --page --favorite
  --favorites --import-saved`) on `jira_api.Client` (`ConfluenceClient`:
  `product`, `token_env` CONFLUENCE_TOKEN, `setup_hint`; `jira_log.setup(...,
  cache_dir=)` → `~/.cache/confluence/`). `confluence_config.py`:
  config.json (`$CONFLUENCE_CONFIG_JSON` › `[confluence] config` ›
  `~/.config/confluence/config.json`; OWN site + token, not Jira's; `spaces`
  scope typed by the user, never listed; `favorites` = bookmarks), and
  `criteria_cql` (quoted parts = phrases, trailing `*` kept, Lucene junk
  dropped, picked spaces clamped to scope, no space picked = whole site;
  the scope is never injected as a default).
  `/rest/api/search` 404 → `/rest/api/content/search` (no excerpts). DC's
  "anonymous" 200 on /user/current counts as an auth failure.
- Contributor: `--users` = people (creator + last editor) on the newest
  `usersScanItems` items in the scope, paced `usersScanDelayMs`, cached in
  `~/.cache/confluence/users.json` for `usersMaxAgeHours` (partial = 1h);
  ids = Cloud accountId / DC username → `contributor in (…)`, "me" →
  `currentUser()`. Icon menu ▸ Refresh Contributor List.
- Rate limits: `Client.last_retry_after`; a 429 (or 5xx + Retry-After) not
  waited out within `rateLimitMaxWaitSeconds` (20) → `api_fail` writes the
  cooldown `~/.cache/confluence/ratelimit.json` (Retry-After else
  `cooldownSeconds`); meanwhile every command answers `{rateLimited,
  retryIn}` with NO request. Swift (`rateLimited` / `tickCooldown`): status
  countdown, then the pending search / preview reruns. Favorites refresh ≤
  every 10 min.
- Favorites: ☆ gutter / Cmd+D / right-click / preview ★ → `--favorite`;
  Cmd+2 view filters locally, the Search button searches inside them (`id in (…)`);
  `--favorites` refreshes in ONE request, vanished pages stay flagged
  `missing`; kitchen sink "Import My Saved Pages" = `favourite = currentUser()`.
- Fake site: `confluence/fake_confluence.py` (`handle()` = REST + a browsable
  HTML site; CQL evaluator; 60 seeded items; `serve` on 127.0.0.1; `curl`
  shim for tests). `bin/fake-confluence.sh start|stop|status` (writes
  `~/.config/confluence/fake.json`, flips `[confluence] config`).
  Info.plist `NSAllowsLocalNetworking` lets image loads reach it.
- Tests: `python3 Tests/test_confluence.py`.

## Jira poller (python, v2)

- SCOPE = team.json `project_keys` (`jira_config.scope_projects`), typed by
  the user (setup window "Projects in scope", Jira Config ▸ Setup,
  Definitions ▸ Projects). NEVER look projects up (no `/project` listing) and
  never query outside them: `"*"` = all of them (`job_projects` clamps
  explicit lists), the live search adds `project in (scope)`
  (`criteria_jql`), `sync()` refuses without projects, the directory job
  GETs `/project/KEY` per key. Empty scope → every job refuses (`NO_SCOPE`).
- Setup gate: config.json `setup` = {state pending|done, steps}. Pending →
  the launchd tick no-ops (status "setup pending"). `jira_poll.py --setup
  [--step NAME]` runs `setup_steps()` one at a time (connection, scope,
  directory, releases, `sync` = full issue cache, each plain tab, query
  jobs), stops at the first failure; UI = Jira Config ▸ Setup
  (`showSetup`/`updateSetup`). `migrate_setup` marks working installs done.
- Shared sync: plain issue jobs (no jql) are views over ONE `sync()` per
  tick (status entry + checkpoint name `sync`, `Ctx.sync_projects/fields`);
  `publish_plain` writes their tabs. Projects added to the scope since
  `syncedProjects` get a full sync first. Custom-jql jobs sync themselves.
- Streaming + resume: `jira_api.sync()` orders oldest-first (full: created,
  else updated), writes jiras.json every `checkpointEvery` issues + in a
  `finally`, and saves `~/.cache/jira/checkpoints/NAME.json` (high-water
  mark); a rerun of the same query resumes at `hwm - 1m` (JQL time in the
  Jira user's zone, `jira_tz` ← /myself). Comments ride in the search
  (`comment` field; per-issue GET only when truncated). v2 pages overlap
  `PAGE_OVERLAP` rows. `lastSuccess` = the run's START.
- Resilience: `Client.get` retries the SAME request on 429/502-504, curl
  network exits, and a 401 after a 2xx this run (Retry-After via `-w
  %header{retry-after}`, else backoff) within `rateLimitMaxWaitMinutes`;
  no whole-job retries. A mid-run 401 waits at least 5/10/20/40s
  (`AUTH_401_BACKOFF`; a `Retry-After: 0` is not a wait) and its final
  error names the request + the server's body ("token worked earlier"),
  not "fix your token". A search page that breaks off (curl 18/28/52/56/
  92/16) or 5xx after one retry is TOO BIG: `search_pages` halves
  maxResults (floor `MIN_PAGE` 10), re-reads the same position, and stores
  the size in `~/.cache/jira/search_page_cap.json` (per site, 7 days;
  `Client.from_config` applies it). Tests swap `jira_api.SLEEP`.
- Visibility: `~/.cache/jira/poll.log` (`say()`, always written) + status.json
  `progress` (`Reporter`, per page / `Reporter.step` per directory stage) →
  the Jira Config header / Setup page (1s timer reads status.json;
  rate-limit countdown via `waitingUntil`).
- Debug log + raw dumps (`jira/jira_log.py`, opened by `jira_log.setup()` in
  both mains, not for `--describe`/`--curl`): `~/.cache/jira/debug.log`
  (python `logging`, 10MB×5, config `logLevel`) — every line tagged
  `run=… [job/stage/project]`; `jira_log.stage(name, c=client)` logs ▶ /
  ◀ (time, requests, items) / ✗ + traceback; `Client.get` logs every
  attempt; `say()` forwards. `~/.cache/jira/raw/<run>-<label>/` (0700):
  `NNNN-<endpoint>-<code>.json|.body` = the body as received,
  `.meta.json` = url, stage, attempt, curl exit, timing, ALL response
  headers (`curl -D`), masked repro; `manifest.jsonl`. `ApiError.raw` =
  the failing body's file. Config `rawCapture`, `rawKeepDays` (3),
  `rawMaxMB` (2048). The token is scrubbed (`jira_log.secret`). Jira
  Config: Setup ▸ "Open debug.log" / "Raw Responses", Open… menu, the
  Connection page; a failed setup step stores `rawDir` + `debugLog`.
- Start over: `jira_poll.py --rebuild` / config `rebuildOnNextPoll` (true →
  wipe + "resume" → false when complete; an interrupted rebuild resumes).
- Switch: `[jira] enabled` in commands.toml gates the window, the launchd
  agent (`syncJiraLaunchAgent()`, re-run on every `reloadConfig()`), and the
  poll itself (`jira_poll.py` no-ops when false unless `--force`).
- Hyper+S "/Jira Config Window" (`[jira-config]`, `label =`, only while
  enabled) and the notes icon menu also open `showJiraDashboard`.
- Menu: `installStatusMenus` → "Enable Jira" (`SwitcherController.toggleJiraPoll`:
  `jira_config.py --check` → setup window if missing → `jira_api.py --myself`
  → `setJiraEnabled(true)`), "Toggle Jira Window", and ONE "Open Jira Config
  Window" (`showJiraDashboard`). No other jira menu items — everything else
  lives in that window. Socket messages `jira-poll-on/off/toggle`,
  `jira-setup`, `jira-dashboard` (CLI: `workspace-switcher jira-poll on|off|toggle|setup|dashboard`).
- Jira Config window look: a `CardWindowController` (CardWindow.swift:
  `CardNSWindow` = titled + hidden titlebar, `_cornerRadius` override, header
  clicks caught in `sendEvent`; `themedRoot` = blur, card tint, border, a real
  `PopupChrome` header: ✕ · icon · title). Status lines are one sentence; details live in tooltips.
  Job pages show "Defined in config.json › endpoints › NAME" + Open config.json.
- Jira Config window: `JiraDashboard.swift` (`JiraDashboardWindow` +
  `JiraColumnEditor` + `jiraFormSheet`). Master–detail: sidebar (POLL JOBS / SETTINGS:
  Live Search, Connection, Definitions) → editor per item. All data from ONE
  call: `jira_poll.py --describe` (per job: schedule, status, next window, full
  JQL, full curl of every request, `maxResults`/`maxTotal`, its columns → API
  fields; `catalog`; `liveSearch`; `searchDefaults`; `directory` summary;
  `team` = team.json merged with defaults, `teamOwn` = team.json as written)
  + `directory.json` (`JiraDirectory.load()`). Edits only via `jira_config.py
  --upsert-endpoint|--delete-endpoint|--set-columns endpoint|live NAME SPEC|
  --set-live-search|--team-set KEY` (validated, JSON result; team edits write
  ONLY `teamOwn` + the change, so built-in defaults are never pinned). Force
  Poll warns with a SHEET when the lock is held (app-modal NSAlerts open hidden
  behind this window — always use `ask(_:then:)` / `jiraFormSheet`), Stop =
  `jira_poll.py --cancel`. `reloadJiraWindow()` rebuilds an open Jira window
  after edits.
- Poll job editor: Projects = `JiraMultiPicker` (known keys only, "All
  projects" = `*`), Page size = endpoint `maxResults` (default
  `search_defaults.max_results_search`, default 500; the server may cap it), Max
  issues = `maxTotal`. Types: issues / releases / `directory`.
- Column editor: read-only rows (Header = the field's label · Field + API
  field · Width · Align · Sort · Filter); double-click / Edit… / Return opens
  the column sheet (no title box); Delete removes.
- Field labels: ONE label per field (team.json `field_labels`, else a custom
  field's `custom_fields[alias].label`, else `BASE_FIELD_LABELS` — mirrored in
  Swift `JiraPoll.baseFieldLabels`; `jira_config.field_label`). Column specs
  are written `field::width:align:flags` (`ListColumn.serialize(titles:
  false)`); the Jira window applies labels in `tabColumns` →
  `JiraPoll.labeled`. One-time `migrate_field_labels` (flag `fieldLabels`)
  moved old spec titles into `field_labels` and blanked them.
- Definitions page (`DefTab`): Projects, Fields (the catalog: every column
  field with its label; Rename… → `field_labels`, Add Custom Field… →
  `custom_fields`, Remove / Reset), API Endpoints, JQL Templates, Search
  Defaults (editable → `--team-set`), plus read-only Users, Statuses & Types
  (directory cache). Label edits call `reloadJiraWindow()`.
- Directory job (endpoint `type: directory`, weekly `1w`, no tab; added once
  by `migrate_v3`, flag `directoryJob`). Staged (`per_project` / `once`,
  each a `jira_log.stage`, progress callback → `Reporter.step`) and
  checkpointed per project/section in `checkpoints/directory-NAME.json`: a
  rerun within `directoryResumeHours` (24) resumes; a run that ends with
  skipped parts writes directory.json AND keeps the checkpoint so the
  rerun retries only those. A 401 after the token worked = a warning,
  `MAX_401_IN_ROW` (3) failing requests in a row abort. Users paging stops
  when a page brings no new ids (server ignoring startAt). Labels pages =
  `max_results_search`. `jira_api.directory()` → projects,
  assignable users of `project_keys` (paginated, merged by id: Server `name`,
  Cloud `accountId`), statuses, issue types, priorities, fields, per-project
  releases (its `versions` stage: unarchived versions) and labels (its
  `labels` stage: labels of the newest `search_defaults.labels_max_issues`
  labelled issues, fields=labels) → `~/.cache/jira/directory.json`.
  `jira_poll.py --directory` = run it now.
- Live search (replaced saved searches; `migrate_v3` drops `searches` and
  their `search-*.json` tabs): Cmd+F in the Jira window (`PopupWindow.onCommandF`,
  list mode only) or icon menu "Search Jira…" → `JiraSearchPanel`
  (`JiraSearch.swift`: a strip INSIDE the Jira window under its header —
  `PopupWindow.setTopAccessory` (list moves down; parks with the window),
  Esc = `onAccessoryEscape` closes it, Cmd+F from the list focuses it, from
  the strip closes it; while it has focus, plain keys bypass list nav; free
  text (`text ~`: title, description, comments) + Projects + "+ Filter" rows;
  every known value (users, status, type, priority, Release, Labels — the
  last two scoped to the picked projects) is a `JiraMultiPicker` over the
  directory, never typed text; only "… contains" rows are text; no Raw JQL;
  last criteria in UserDefaults; filter rows live in a height-capped
  scroll view (`rowsScroll`, capped at ~40% of the window by `place()`).
  Drawn in the Jira window's theme (`applyTheme`: mantle well + hairline,
  `JiraInputBox`, `JiraChoiceButton`,
  `ThemeButton`, custom-drawn picker pills). Run = `jira_poll.py
  --live-search` (criteria JSON on stdin → `jira_config.criteria_jql`: lists
  ORed with `in (…)`, criteria ANDed, custom aliases → `cf[N]`) → writes
  `<outDir>/search.json` (no lock, no cache merge) → `controller.jiraShowTab`
  selects that tab (`pendingJiraTab` + rebuild when the tab is new). Its
  columns/max: config.json `liveSearch` (Jira Config ▸ Live Search);
  `owner(ofTab:)` maps search.json → `("live","search", …)`.
- Per-job columns: every endpoint in config.json owns `columns` (same
  one-line format); `[jira] columns` = starter template + fallback for tabs
  no job owns (one-time migration copies it into jobs on load). Jira window:
  `tabColumns` / `JiraPoll.owner(ofTab:)` swap columns per tab
  (`setTableColumns`); header drags save to the owning job. Poller fetches
  per-job fields (`job_fields`). Catalog: defined columns + fields seen in
  published data (`~/.cache/jira/fields_seen.json`).
- Jira window: Cmd+K → `PopupWindow.showActionPicker` (↑↓ / Ctrl+N/P / Tab,
  Return, digits, Esc closes only the picker) via `onCommandK`; acts on
  ticked rows else the highlighted row (`actionRows`): Copy to clipboard /
  Copy URL and title (`SITE/browse/KEY Title` per line) / Open all in
  browser (+ copies `KEY<TAB>URL`); actions are matched by title. No "copy
  selected" header button (`PopupConfig.copyRowsButton = false`); icon menu
  is window chrome + "Search Jira…" + "Open Jira Config Window".
- Jira window filters: every `filter`-flagged column has a ▾ in its header
  (`PopupTableHeaderView.onFilter` → `PopupWindow.onTableFilter`) that opens
  a searchable multi-select `JiraMultiPicker` (popover `anchor`, Sort ↑/↓
  `extraButtons`); OR within a field, AND across fields, "a, b" cells match
  any part (`colFilters`, `cellValues`). `[jira] filters` fields that are
  not columns (labels, reporter…) stay in the filter bar, same popover
  (`PopupWindow.onFilterOpen`). Users show full name + username from the
  directory. Enter on a row = the detail window (same as double-click).
  "fit columns" = the table header's corner cell icon (over the checkbox /
  star gutter) + header right-click (`PopupWindow.onTableFit` →
  `PopupTableHeaderView.onFit` → `fitTableColumns()`).
- Filter box speed (work tabs are 5k–20k issues): `ListFilter.swift`
  (`FuzzyIndex` = lowercased UTF-8 keys built once per data change, in the
  background via `ListSession.invalidateFilter`, narrowing while typing
  forward; `SortRank` = header sort as Int ranks; `PopupFuzzy` lives there
  too). `PopupRowView.draw` culls to the DIRTY rect (bounds = the whole
  document). `compactTimestamp` is memoized. Slow keystrokes log `list 'jira'
  filter: …` (> 8 ms) to /tmp/ws-debug.log. Repro: `bin/fake-jira-tab.sh
  start [N] | stop` (Faker tab, flips `[jira] sources`); bench + parity vs
  the old matcher: `bin/run-tests.sh filter` (`WS_FILTER_JSON=` a tab file).
- Tab freshness badges: `JiraPoll.tabBadge` (status.json per job) →
  `PopupWindow.tabBadges` (dot + age; green fresh+ok, yellow stale or a
  failed run, red stale+failed; tooltip = details).
- Favorites: ☆ beside each issue row's checkbox (`PopupConfig.rowStars`,
  `PopupRow.starred`, nil = no star) or Cmd+K → `jira_poll.py --favorite
  add|remove KEY…` (config.json `favorites`, favorites.json rewritten at
  once from the cache / stdin rows). Endpoint type `favorites` (added once
  by `migrate_v3`, flag `favoritesJob`; not a setup step) re-queries `key in
  (…)` in scope every run; a key Jira rejects (400) is skipped.
- Release blacklist: Cmd+K on releases.json rows → `jira_poll.py
  --blacklist-release add|remove KEY…` (config.json `releaseBlacklist`); the
  releases job splits into releases.json + `blacklist_release.json`. Release
  rows carry `versionId`; "open in browser" = `jiraBrowseURL` (release →
  /projects/P/versions/ID, else a fixVersion JQL search; never /browse). The
  release detail window lists its issues (from jiras.json).
- Voice (notes): dictation goes to the CURSOR. vim pane: extmark region via
  `vimVoiceBegin/Update/End` (luaeval over RPC, throttled 0.2s); native
  editor: `replaceRange`. `voice-live` (default true) = text appears as you
  speak; false = held, inserted on stop.
- `jira_poll.py --projects '*'` = every job (`all` only when no job is named
  "all" — the default job IS named "all").
- Setup window: `JiraSetupWindow` (plain NSWindow above `.popUpMenu`, own key
  monitor for edit shortcuts + Esc). Token goes to python over stdin only.
  Test / Save run `jira_api.py --detect-auth` and save the mode that answers
  `/myself` with 200.
- Auth: config.json `auth` = `bearer` (Server/DC PAT, `Authorization: Bearer`,
  no email) or `basic` (Cloud email+token); unset → basic iff email set.
  `--detect-auth` (`auth_order`): `*.atlassian.net` tries basic then bearer,
  other sites bearer then basic (basic only with an email).
- Team schema: `~/.config/jira/team.json` (example `jira/team.example.json`):
  `custom_fields` (alias → field_id; aliases usable as [jira] columns),
  `field_mappings`, `project_keys`, `jobs`, `api_endpoints` (every REST path;
  `resolve_path`), `boards`, `search_defaults`, `jql_templates`. Keys are
  normalized (`norm_key`). Poll endpoints may use `job`/`template` + `args`.
- curl: every request is a canonical `curl -X GET -H 'Authorization: Bearer
  …' -H 'Accept: application/json' 'URL'` (`Client.curl_argv`; basic = `-u`;
  Content-Type only with a body). Shown curls (`curl_cmd`) are readable: `-G
  'BASE' --data-urlencode 'jql=…'` (same request; the executed argv keeps the
  encoded URL). `jira_api.py --curl
  [--mask]` prints instead of running; failures print a `$JIRA_TOKEN` repro
  line and poll status stores `lastCurl` (shown in the Jira Config window);
  setup window: "Copy curl".
- Python: `jira/jira_config.py` (config.json, legacy env-config migration,
  `api_fields()` = [jira] columns + field keys), `jira_api.py` (curl via
  subprocess → `~/.cache/jira/curl.log`), `jira_poll.py` (flock
  `~/.cache/jira/poll.lock`, per-endpoint `window` schedules),
  `jira_status.py` (`~/.cache/jira/status.json`, atomic writes).
- Table window: `table = true` + `columns` in `[jira]` → `PopupConfig.tableColumns`,
  `PopupTableHeaderView` (floating subview of the row scroll view: sort
  clicks, divider drags) + `PopupRowView.drawTableRow`. Sort persists as
  `table-sort`, widths back into `columns`. `columns` MUST stay on one line.
- Tests: `python3 Tests/test_jira_poll.py` (jq parity vs the legacy bash
  transforms, lock, disabled no-op, env-token override, criteria JQL, live
  search, directory job, page size, `--team-set`, v3 migration) +
  `ResilientSyncTests` (stateful fake Jira `FAKE_JIRA`: 429/401 retries,
  comments in search, resume after failure, shared sync, scope, setup
  gate, rebuild, added projects).

# Code map

Line numbers drift; grep the symbol names (they're stable).

## Repo facts

- Real repo: `/Users/danielbaker/.config/workspace-switcher` (rule.md still
  says `dotfileApp` — that's the old name; same rule: no git in backups).
- Backdrop (`panel.contentView`) is FLIPPED — y=0 is the top when placing overlays.
- Signing: `bin/build-app.sh` signs with the self-signed login-keychain
  cert "workspace-switcher codesign" (stable TCC grants across rebuilds);
  falls back to ad-hoc (`-`) if the cert is missing.
- `./build.sh` builds AND relaunches the app (exit 0 + no output = OK). The
  running process is `workspace-switcher.app/Contents/MacOS/workspace-switcher`.
- Python jira tests: `python3 Tests/test_jira_poll.py`. UI suites
  (`bin/ui-test*.sh`) are slow/flaky — run once at most, don't loop them.

## Where things live

### workspace_switcher.swift (host / app logic)
| Symbol | What |
|---|---|
| `struct AppSettings` / `settings` | `[app]` values (esc-close, hide-on-focus-loss, shell, …) |
| `parseAppConfig(_:)` | parses `[app]` into `settings` |
| `struct CommandSpec` | one `[section]` (note/list/files/output) |
| `makeCommand(_:_:)` | `[section]` key → CommandSpec field parsing |
| `configNumberKeys` | numeric range validation for keys (add new numeric keys here) |
| `validateConfig`, `configValueProblem` | config validation / warnings |
| `saveConfigValue(s)`, `removeConfigValue` | write back into commands.toml |
| `THEME`, `BAR`, `GROUP_BG`, `TEXT`, `DIM` | `[theme]` globals |
| `vimArgs(for:socket:file:)` | nvim launch args, passes `g:ws_fg/dim/sel/line` |
| `showDetail` | jira detail window (PopupConfig built here) |
| `openOutputWindow` / `openNoteWindow` / `openListWindow` / `openFilesWindow` | build `PopupConfig` per window type — per-window config goes here (`cfg.escCloseCount =` line is a good anchor) |
| `installStatusMenus` | menu-bar menu |
| `reloadConfig()` | re-read commands.toml |
| `handleEscape()` (SwitcherController) | switcher palette Esc (command mode → back) |

### PopupWindow.swift (window framework)
| Symbol | What |
|---|---|
| `public struct PopupConfig` | every window option (tabs, editMode, escCloseCount, floating, vim…) |
| `PopupBaseWindow` / `PopupPanel` | NSWindow/NSPanel subclasses; `cancelOperation` → `onEscape` |
| `PopupTabsBar` | tab strip; `PopupWindow.tabTitles` / `selectedTab` (setter fires `onTabChange`) |
| `FileListPane` | file list; `moveSelection(_:)`, `selection` |
| `PopupFileBrowser` | browser (notes drawer + files window); `copyRowPath`, `listView`, `searchView`, `control(_:textView:doCommandBy:)` for filter-bar Return/Tab/Up/Down |
| `PopupWindow.installMonitors()` | local keyDown monitor → `handleKey` |
| `PopupWindow.handleKey(_:_:)` | ALL keyboard routing (see order below) |
| `escStreakCloses()` | N-rapid-Esc counter (0.6 s window) |
| `showToast(_:symbol:)` | Raycast-style bottom-center pill (fade/rise, 1.4 s); used by Cmd+K copy; explicit frames, icon+text centered |
| `PopupPlainWindow._cornerRadius` | makes the system window frame use `config.cornerRadius` (else macOS 26's 16pt frame peeks out around the card) |
| `PopupChrome.closeButtonRect` | ✕ glyph far-left of the drag header (`PopupConfig.headerCloseButton`, default on; titled windows only); icon sits right of it (`leftInset`) |
| `PopupFileBrowser.updatePartFocus` | which part has focus: filter bar (bright 2px outline) / list / preview (`partRing`); KVO on `firstResponder` |
| `focusedVim()`, `vimRemote`, `vimEval`, `vimCommand` | nvim pane + RPC (`vimClient` → `NvimRPC`: ONE persistent msgpack-RPC socket, ~0.03 ms a call; it used to spawn `nvim --server` per call on the main thread) |
| `browserHasFocus`, `browserActive` | file browser focus checks |

### handleKey order (first match wins)
1. Esc streak reset on non-Esc key; Cmd+Opt+=/- font; Cmd+=/- resize.
2. `cmd || ctrl`: sheet edit keys → Ctrl+J/K pane focus → **Ctrl+Tab / Ctrl+Shift+Tab
   next/prev shared-window view (`onCycleView`), else tab cycle (wraps)** → Ctrl+Shift+HJKL resize → vim-pane shortcuts →
   terminal (Cmd+C/V only) → Cmd+L → **file browser keys (Ctrl+N/P move,
   Cmd+K copy abs path, Cmd+A/C/V/X/Z)** → generic edit keys.
3. `editMode`: terminal focused (Esc passes to shell; Nth rapid Esc closes),
   vim pane (Esc → pty; Nth rapid Esc closes only in Normal mode), find
   bar (single Esc closes bar), Esc (streak), Cmd+S, Cmd+O.
4. list navigation (Up/Down/Tab/C-n/C-p/Return), then Esc (streak).

## Quiet look (Figma direction B + C picks)

- Figma file `ZIkTJn7JSbPs6XSNF4a3Zi` (B = Quiet Page, C = Native Glass).
  Applied: `[app] header-style = "quiet"`; notes tabs = a left SIDEBAR
  (`PopupTabsBar.vertical`, `PopupConfig.tabsSidebarWidth` ← `[notes]
  sidebar-width`, 0 = the old strip): mantle card, NOTES label + "+",
  rows icon · name · last write (✕ on hover), scroll wheel; the editor
  sits right of it (`editorLeftInset` / `editorTabStripHeight` in
  PopupWindow), drawers keep the full width. Remaining strips (jira, AI,
  compare sessions): active tab = text + 2pt accent underline, no solid
  pill. View switcher (C): capsule track, current view on a raised chip
  (`drawNavIcons`). No title in any shared-window header (C):
  `themedRoot` sets `headerTitle = nil`, AI no longer pushes the rule name.
- Sidebar everywhere (`PopupTabsBar.vertical`, 34pt rows, PINNED section =
  `pinned` / `onPinned` / `pinnedMenu`, `rowIcon`, status `badges` as dot +
  text, right edge drags = `onWidthChange` → each host saves its
  `sidebar-width`): notes (+ the browser's pinned folders), jira list
  (`listLeft` / `layoutListSidebar` shift field, filters, rows), AI rules,
  Files (`PopupFileBrowser.useSidebar`: pinned + Places = Recent, Arrived,
  Home, Desktop, Documents, Downloads), Confluence (Search / Favorites),
  Compare sessions. No editor focus ring in notes. Buttons: only
  `.primary` is filled; normal / danger = outlines (`ThemedPushButton`).
  Jira detail: no header title, the page's first line = KEY — summary.
- Prose mode (NotesProse.swift): ⌘⇧P / the Prose | nvim switch (bottom
  right) / Esc back; `ProseRender` = pandoc (`RichText.pandocHTML`) else
  `basic`; `[notes] prose-font / prose-font-size / prose-width`.
- Capsule look everywhere (`CapsuleStyle` track + raised chip, PopupWindow.swift):
  view switcher, `ConfSegmented` / `ConfToggle` (compare + folder filters,
  AI preview modes, Confluence), the prose switch, and `CapsuleButtons`
  (action capsule; primary = accent chip): Compare's start actions and the
  Recent header's Clear Missing (N) / Clear All. Recent rows: ✕ on hover
  (`CompareRecentList.onRemove`). Start page: the folder button sits LEFT
  of each path field.
- Prose: default 19 px / 900 pt column; ⤢ chip / ⌘⇧O = `ProseWindow`
  (non-activating floating panel, follows the file's mtime every 1 s,
  Esc / ⌘W close, ⌘± size, close button only).
- /screenshot recents: `ShotHistory` keeps every copied / saved / pinned
  capture (`[screenshot] history`, 20) in ~/.cache/workspace-switcher/
  screenshots; clock button / ⌘R → `ShotRecentPanel` thumbnail grid
  (click = reopen as a pin; right-click Copy / Open / Reveal / Delete).
  The clock button was invisible (untinted template symbol) and dropped
  after the first session (`recentBtn` not reset in attach/detach).
- `PopupChrome.redrawAll` runs before NSApp exists (parseAppConfig at
  launch) — guarded; a non-default header-style crashed launch.

## Hotkey fast path (Hyper+N)

- ONLY one window hotkey besides Hyper+S: Hyper+N = `window`
  (`SharedWindow.toggle`: hidden → the view you were LAST on (`last`), in it →
  hide, elsewhere → focus). No per-view hotkeys (Hyper+F / J
  removed): views switch via Ctrl+Tab, header icons, the Hyper+S palette.
  The named modes (`notes|files|jira|confluence|ai|compare`) remain as CLI / socket
  messages. Hyper+X = `screenshot` (a tool panel, not a view: no
  `hotkeyPrep`, never in `hotkeyModes`; see "/screenshot").

- aerospace runs the app BINARY (`workspace-switcher window|terminal`),
  not the script: `main.swift` pings the socket (~20 ms) and exits. No
  daemon (ppid != 1) → it execs `bin/workspace_switcher.sh MODE` (cold start:
  build-if-stale + LaunchServices `open -n -g`; a launchd-parented process
  never re-execs).
- The daemon's socket thread runs `SwitcherController.hotkeyPrep()` before the
  main thread sees the message: `list-windows --focused` (→ focus file),
  `eval true` (clears AeroSpace's closed-windows cache, see the shared
  window) and `list-windows --all` (+ which workspace is focused and its
  NSScreen index → `slot.targetScreen`; an empty workspace costs one more
  `list-workspaces --focused`) IN PARALLEL (aerospace ≈ 10-25 ms per query,
  served one by one — the floor), then `move-node-to-workspace` only for
  our windows still up on another workspace. `applyHotkeyPrep` hands it to the main
  thread. No sleeps. Log: `/tmp/ws-debug.log` `hotkey X: prep N ms, M ms
  to shown`.
- AeroSpace IPC: `liveAerospaceSocket()` — a configured `aerospace-socket`
  that doesn't exist falls back to `/tmp/bobko.aerospace-<user>.sock` (a
  hard-coded user name made every call a CLI spawn on another Mac).
- ONE daemon: `acquireDaemonLock` (flock beside the socket, O_CLOEXEC, held
  for life); main.swift: a second launch forwards its request (`window`,
  `setup`, `show` → toggle, none → `ping`) and exits; it waits ≤3 s for a
  dying daemon's lock. The command socket's connections are SO_NOSIGPIPE
  (a client hanging up before its reply); never a global SIG_IGN — the
  drawer's shell and nvim would inherit it.
- Focus bridge (accessory apps can't be activated from outside): in-process
  `didBecomeKey` while inactive → self-activate at once; plus aerospace's
  `on-focus-changed` writes `$TMPDIR/ws-aerospace-focus` inline (one bash,
  no script) and the daemon watches it (DispatchSource vnode, no poll).
- The script builds only when no daemon answers / `WS_BUILD_ONLY`;
  `./build.sh` runs `build-app.sh` itself. `WS_DEBUG=1` → `$TMPDIR/ws-launch.log`.
- Hyper+T was removed (no binding, no `terminal` mode). The notes terminal
  drawer: Cmd+Opt+T / menu / `do:toggle-terminal`. `PopupWindow.setTerminalDrawer(_:)`.
- Drawer bookkeeping: `drawerInsetNow` counts only what the window really
  grew (clamped at screen height), so closing a drawer never shrinks it more;
  a resize only ever lowers it. Drawer-mode file browser is bottom-anchored,
  FIXED height (`[.width, .minYMargin]`): with a flexible height its stale
  autoresizing constraints made Auto Layout re-grow the window after the
  terminal closed.
- Vim pane font changes go through `applyVimFont()` (nudges the frame so
  SwiftTerm recomputes cols/rows → SIGWINCH). Cmd+± zoom is relative to the
  width change that actually happened (clamped = no zoom change).
- Hotkey hide needs BOTH signals (`userInOurWindow`): focus file says our
  window AND `NSApp.isActive && keyWindow`; else show + focus. Every
  `SharedWindow.hide(_ reason:)` logs its reason in /tmp/ws-debug.log.
- Keyboard Shortcuts…: `[shortcuts]` in commands.toml (`"view: keys" = "what"`,
  parsed in order into `shortcutEntries`), every view's kitchen sink menu +
  Cmd+/ → `PopupWindow.showShortcuts` (card overlay, Esc closes only it).
- Rule 6 (rule.md): popovers take key focus on open; Esc closes only them
  (`PopupWindow.transientEscape`).

## Aerospace: where a floating window sits for focus left/right

- NOT set by aerospace.toml — built into aerospace (`man aerospace-focus`).
  For `focus left|right|up|down` (alt-h/j/k/l) a floating window is part
  of the tiling tree: its parent = the smallest tiling container holding
  its CENTER point, and its left/right (up/down) place among that
  container's tiles comes from where that center lies relative to them
  (the man page doesn't document this ordering detail).
- Why it looks random: a popup centered over ONE full-screen tile has
  almost the same center as the tile → a few pixels flip "left of" vs
  "right of". The shared window's center also moves with its frame
  (notes drawers). With 2+ tiles it belongs to the
  tile under its center, not the one that looks "behind" it.
- Our windows are placed by the aerospace.toml rule `test %{app-bundle-id} =
  dev.danielbaker.workspace-switcher → layout v_accordion` (first rule; it
  can't un-float, so the shared window still reports floating). Finder,
  Preview, Webex and every `com.microsoft.*` but VS Code tile into
  v_accordion. The tool panels are NSPanels: AeroSpace never sees them.
- Monitors: `bin/aerospace-monitors.sh main|inverse|toggle|status` rewrites
  the `# >>> monitor-layout` block (main = 1-8 + letters on the main
  monitor, 9 on the secondary; inverse = the other way round); service mode
  `m` toggles. 9 always owns a screen so AeroSpace never invents "10".
- Fixes (not applied): `focus --ignore-floating <dir>` so directional
  focus only walks tiles (reach popups by hotkey), or place the window so
  its center lands clearly on one side of a tile.

## Adding / removing a global hotkey (checklist)

A Hyper+KEY hotkey (Hyper = alt-cmd-ctrl-shift, caps_lock via Karabiner) touches
these places — grep the key / mode name in all of them (Hyper+T removal is the
worked example):

1. `config/aerospace/aerospace.toml` — the `alt-cmd-ctrl-shift-KEY =
   'exec-and-forget …/workspace-switcher MODE'` binding (+ its comment). The
   repo copy; `~/.config/aerospace` is a symlink. Check with
   `aerospace reload-config --dry-run --no-gui`, apply with `--no-gui`.
2. `workspace_switcher.swift` — `SwitcherController.hotkeyModes` (message
   names that run `hotkeyPrep`; `main.swift` pings the socket for these),
   `toggleCommand(name)` (per-mode handler), the socket dispatch that calls
   it (`hotkeyModes.contains(name)`), and any helper (e.g. `slotToggle…`).
3. `main.swift` — cold-start `openCommand` mapping (`window` → files);
   `bin/workspace_switcher.sh` — usage header + its cold-start `MODE` remap.
4. `commands.toml` `[shortcuts]` — the `"all: Hyper+KEY" = "…"` row (feeds
   Keyboard Shortcuts… and the ws-settings hub, which reads aerospace.toml
   itself, so no hub change).
5. Docs: this file ("Hotkey fast path", "Keyboard shortcuts"), `PRD-*.md`
   examples, `.claude/skills/ui-check/SKILL.md` if a `do:` hook is named there.
6. NOT part of it: in-window shortcuts (`PopupWindow.handleKey`, menu-bar
   `key:` items such as Cmd+Opt+T), `do:ACTION` test hooks, Karabiner (dotfiles
   repo; only Hyper+S/J/N-style bindings live there — caps_lock → Hyper is
   generic, no per-key entry).
7. Tool-panel hotkeys (Hyper+X) are NOT in `hotkeyModes` (no `hotkeyPrep`).
8. After the edit: `./build.sh --build-only`, then reload aerospace.

## Keyboard shortcuts (user-facing)

- Esc: hides a view only where its kitchen sink "Esc Hides Window" is on
  (`esc-close` per section, default `[app] esc-close` = 0 = never; 1 =
  single, 2 = double-tap). Jira / output: Esc = back first. The switcher
  palette always closes on one Esc. Find bar: one Esc.
- Ctrl+Tab / Ctrl+Shift+Tab: next/prev shared-window VIEW in header-icon
  order (`SharedWindow.cycle`; every member's `onCycleView`), wraps. Not
  while a sheet / popover / Cmd+K picker / shortcuts card is up. Notes and
  jira source tabs are click-only now.
- File browser: Ctrl+N/P next/prev result, Cmd+K copy selected row's
  absolute path (+ toast), Cmd+L focus filter bar, Tab completes, Enter opens.
- File browser drag & drop (Finder-style, `FileDrag` + `FileListPane` drag
  source/destination + `FileDragImageView` preview): drag a row / the image
  preview out as a file URL; drop onto a folder row or the list (cwd; not in
  Recent / typed-path / recursive views — `dropDirectory` nil). Same volume =
  move, else copy; Option = copy, Cmd = move; clashes keep both ("x 2.ext");
  file promises (Photos/Safari/Mail) received. Ops run off main.
- File browser rename (`PopupFileBrowser.beginRename` / `commitRename` /
  `cancelRename`): Cmd+R, F2, right-click "Rename…" or a single click on the already-selected
  row's name (`renameClick`, fires after the double-click interval) puts a text field over
  the row's name (`FileListPane.nameRect`, stem selected). Return / Tab /
  clicking away renames, Esc cancels only the rename (`transientEscape`);
  edit shortcuts go to `renameEditor` in `handleKey`. Main list only.
- File browser file actions (`FileOps.swift` = the ops + undo stack, AppKit-free,
  `bin/run-tests.sh fileops`; `PopupFileBrowser.handleShortcut` /
  `perform(_:)` / `run`; right-click = `FileListPane.Action`). While the LIST
  has focus (in the filter bar they stay text keys): Cmd+Delete trash, Cmd+D
  duplicate, Cmd+C / Cmd+X copy / cut the FILES (file URLs + the path as
  text), Cmd+V paste into the listed folder (`opsDirectory`; Cmd+Opt+V or
  after a cut = move), Cmd+Z undo the last rename / move / copy / trash /
  drop, Cmd+A select all, Cmd+Down open. Anywhere in the browser:
  Cmd+Shift+N new folder (straight into rename), Cmd+[ / Cmd+] back /
  forward, Cmd+Up parent (Recent / search row: its enclosing folder),
  Cmd+Shift+. hidden files. Space = Quick Look (`QLPreviewPanel`, follows
  the selection), Home / End / PgUp / PgDn. Every op reports to Recent via
  `FileDrag.onFileOp` (trash = a move out of scope → dropped from the list).
- File browser multi-select (`FileListPane.marked` + cursor `selection`,
  `selectedRows`): Shift-click / Shift+Up/Down range, Cmd-click toggle; a
  drag or right-click inside it acts on all of it; setting `rows` clears it.
- Ctrl+J/K: move focus between editor / browser / terminal panes.

## Theme system (keep every window on it)

- Palette = `PopupColors` (+ `palette: PopupPalette` = accent2 / success /
  warning / danger / info). Derived tokens in `extension PopupColors`:
  depth `crust < mantle < base < surface0/1`, `accentOn`, `onAccent`,
  `tone(_:)`, `hairline`, `outline`. Draw with these, never system colors.
- Layering: header = crust (presets' `header`), tab strip / table header /
  input wells = mantle, raised buttons = text-tinted ghost fill. Active tab =
  SOLID accent pill; "on" buttons = accent-tinted (`ButtonStyle`); cursor
  rows = highlight pill + 3pt accent edge (list, file list, switcher,
  `PopupTableRowView`); focus rings = accent (`ButtonStyle.focusStroke`).
- Host builds colors with `windowColors(cmd)` (every opener) /
  `jiraWindowColors()` (Jira Config + pickers, `JC` in JiraDashboard.swift).
  Per-window keys: text/dim/highlight/accent-color + `palette` (5 hex).
- Global window switches (`addGlobalWindowItems`, ONE "Global Window
  Options ▸" submenu): the FIRST item of every
  view's icon menu (notes, files, jira/detail, confluence, ai) and of the
  menu-bar menu (re-inserted by `MenuTarget.menuNeedsUpdate` above the
  `globalGroupTag` separator): Hide When Focus Is Lost (`[app]
  hide-on-focus-loss`; `setGlobalHideOnFocusLoss` drops the views'
  `sticky`), Header Style ▸. No float / tile item (AeroSpace decides).
  Per view (its own menu): "Esc Hides Window" (`escHidesMenuItem`).
- `HeaderStyle` (PopupWindow.swift, `[app] header-style`: quiet (the card's
  own color + hairline — Figma direction B, the owner's setting) / flat / edge /
  stripe / tinted / glow / aurora): `PopupChrome.drawHeaderBackground`, built
  from the header color + accent + accent2; setting `current` redraws every
  chrome; the submenu previews on hover (`HeaderStylePreviewDelegate`).
- `ThemePreset` (13 colors, swatch = mini window; Theme menu grouped Mid
  Tones / Dark / Light by `tone`, brightest first). Live: `setTextColors(…,
  palette:, border:)` → `pushColors()` walks `PopupThemeable` views; also
  the shell's ANSI colors (`ansiPalette`) and vim (`vimPaletteLets` →
  `g:ws_accent…`, ONE `--cmd`: nvim allows max 10).
- AppKit forms: `ThemedPushButton` (`role` .primary/.danger),
  `ThemedPopUpButton`, `PopupTableRowView`; initial colors from
  `PopupThemeDefaults.colors`. Jira table cells: `jiraCellTone`.

## Config defaults worth knowing

- No `[app] float` (retired; AeroSpace places the app). Per-section `float`
  only for popup-only "/" windows (level): tool panels default true, other
  output windows false.
- `[app] esc-close` default 0 (Esc never hides); per-section `esc-close`
  (alias `vim-esc-close`) = the view's "Esc Hides Window".
- `[app] copy-toast` default `Copied {} to clipboard` (`{}` = ~-path; empty = off).
- Vim pane (`vim/init.lua`): `number` + `cursorline` on; cursor-line
  color `g:ws_line` = highlight color blended 50% toward the card color.

## Verifying vim-pane changes without UI tests

```bash
S=$(ls -t ~/.cache/workspace-switcher/nvim-notes-*.sock | head -1)
nvim --server "$S" --remote-expr 'execute("set number? cursorline?")'
```
