# workspace-switcher

Swift/AppKit macOS menu-bar app. `CLAUDE.md` is a symlink to this file
(`AGENT_CONTEXT.md`) — edit it once. Don't read the big Swift files whole:
grep the symbol from the code map below, then read ~60 lines around it.

## Non-negotiable rules

Read and honor `rule.md` before touching any code. Key points:

- Edit shortcuts (Cmd+C/V/X/A/Z and Ctrl+C/V) must work in every text input.
- Right-click menus in file browsers/terminal must offer obvious actions.
- Config over code: user-facing strings/sizes/paths live in `commands.conf`.
- NEVER run git commands in a backup workspace — verify you're in the real repo first.
- NEVER rebuild or modify `Vendor/SwiftTerm/` unless explicitly told to. Don't ever read this  either, too expensive.

## Build

```bash
./build.sh            # Build + relaunch
./build.sh --force    # Force rebuild
./build.sh --build-only
```

## Tests

```bash
bin/ui-test.sh              # Full UI test suite (cliclick + osascript)
bin/ui-test.sh --verbose
```

## Key files

- `main.swift` — entry point
- `workspace_switcher.swift` — app logic (~7300 lines)
- `PopupWindow.swift` — popup window framework (~8500 lines; all keys in `handleKey`)
- `JiraDashboard.swift` — the Jira Config window (`JiraDashboardWindow`, `JiraColumnEditor`)
- `JiraSearch.swift` — Cmd+F live search (`JiraSearchPanel`), `JiraMultiPicker`, `JiraDirectory`
- `commands.conf` — config (windows, commands, colors, paths)
- `jira/jira_*.py` — python jira poller (see AGENT_CONTEXT.md "Jira poller");
  tests: `python3 Tests/test_jira_poll.py`
- `vim/notes-init.vim` — nvim pane init (theme vars `g:ws_*` from `vimArgs`)

## Jira poller (python, v2)

- Switch: `[jira] enabled` in commands.conf gates the window, the launchd
  agent (`syncJiraLaunchAgent()`, re-run on every `reloadConfig()`), and the
  poll itself (`jira_poll.py` no-ops when false unless `--force`).
- Menu: `installStatusMenus` → "Enable Jira" (`SwitcherController.toggleJiraPoll`:
  `jira_config.py --check` → setup window if missing → `jira_api.py --myself`
  → `setJiraEnabled(true)`), "Toggle Jira Window", and ONE "Open Jira Config
  Window" (`showJiraDashboard`). No other jira menu items — everything else
  lives in that window. Socket messages `jira-poll-on/off/toggle`,
  `jira-setup`, `jira-dashboard` (CLI: `workspace-switcher jira-poll on|off|toggle|setup|dashboard`).
- Jira Config window: `JiraDashboard.swift` (`JiraDashboardWindow` +
  `JiraColumnEditor` + `jiraFormSheet`; own file, compiled by
  `bin/workspace_switcher.sh`). Master–detail: sidebar (POLL JOBS / SETTINGS:
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
  `search_defaults.max_results_search`, the old hidden `maxResults=50`), Max
  issues = `maxTotal`. Types: issues / releases / `directory`.
- Column editor: read-only rows (Column · Field + friendly name/API field ·
  Width · Align · Sort · Filter); double-click / Edit… / Return opens the
  column sheet; Delete removes.
- Definitions page (`DefTab`): Projects, Custom Fields, API Endpoints, JQL
  Templates, Search Defaults (editable → `--team-set`), plus read-only
  Columns (catalog), Users, Statuses & Types (directory cache).
- Directory job (endpoint `type: directory`, weekly `1w`, no tab; added once
  by `migrate_v3`, flag `directoryJob`): `jira_api.directory()` → projects,
  assignable users of `project_keys` (paginated, merged by id: Server `name`,
  Cloud `accountId`), statuses, issue types, priorities, fields →
  `~/.cache/jira/directory.json`. `jira_poll.py --directory` = run it now.
- Live search (replaced saved searches; `migrate_v3` drops `searches` and
  their `search-*.json` tabs): Cmd+F in the Jira window (`PopupWindow.onCommandF`,
  list mode only) or icon menu "Search Jira…" → `JiraSearchPanel`
  (`JiraSearch.swift`: child panel docked above/below the Jira window; free
  text + Projects + "+ Filter" rows with `JiraMultiPicker`s over the
  directory; last criteria in UserDefaults). Run = `jira_poll.py
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
- `jira_poll.py --projects '*'` = every job (`all` only when no job is named
  "all" — the default job IS named "all").
- Setup window: `JiraSetupWindow` (plain NSWindow above `.popUpMenu`, own key
  monitor for edit shortcuts + Esc). Token goes to python over stdin only.
- Auth: config.json `auth` = `bearer` (Server/DC PAT, `Authorization: Bearer`,
  no email) or `basic` (Cloud email+token); unset → basic iff email set.
- Team schema: `~/.config/jira/team.json` (example `jira/team.example.json`):
  `custom_fields` (alias → field_id; aliases usable as [jira] columns),
  `field_mappings`, `project_keys`, `jobs`, `api_endpoints` (every REST path;
  `resolve_path`), `boards`, `search_defaults`, `jql_templates`. Keys are
  normalized (`norm_key`). Poll endpoints may use `job`/`template` + `args`.
- curl: every request is a canonical `curl -X GET -H 'Content-Type…' -H
  'Authorization: Bearer …' 'URL'` (`Client.curl_argv`). `jira_api.py --curl
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
  search, directory job, page size, `--team-set`, v3 migration).

# Code map

Line numbers drift; grep the symbol names (they're stable).

## Repo facts

- Real repo: `/Users/danielbaker/.config/workspace-switcher` (rule.md still
  says `dotfileApp` — that's the old name; same rule: no git in backups).
- Backdrop (`panel.contentView`) is FLIPPED — y=0 is the top when placing overlays.
- Signing: `bin/workspace_switcher.sh` signs with the self-signed login-keychain
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
| `struct AppSettings` / `settings` | `[app]` values (float, esc-close, shell, …) |
| `parseAppConfig(_:)` | parses `[app]` into `settings` |
| `struct CommandSpec` | one `[section]` (note/list/files/output) |
| `makeCommand(_:_:)` | `[section]` key → CommandSpec field parsing |
| `configNumberKeys` | numeric range validation for keys (add new numeric keys here) |
| `validateConfig`, `configValueProblem` | config validation / warnings |
| `saveConfigValue(s)`, `removeConfigValue` | write back into commands.conf |
| `THEME`, `BAR`, `GROUP_BG`, `TEXT`, `DIM` | `[theme]` globals |
| `vimArgs(for:socket:file:)` | nvim launch args, passes `g:ws_fg/dim/sel/line` |
| `showDetail` | jira detail window (PopupConfig built here) |
| `openOutputWindow` / `openNoteWindow` / `openListWindow` / `openFilesWindow` | build `PopupConfig` per window type — per-window config goes here (`cfg.floating = cmd.float ?? settings.float` line is a good anchor) |
| `installStatusMenus` | menu-bar menu |
| `reloadConfig()` | re-read commands.conf |
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
| `showToast(_:symbol:)` | Raycast-style bottom-center pill (fade/rise, 1.4 s); used by Cmd+K copy |
| `focusedVim()`, `vimRemote`, `vimEval`, `vimCommand` | nvim pane + RPC |
| `browserHasFocus`, `browserActive` | file browser focus checks |

### handleKey order (first match wins)
1. Esc streak reset on non-Esc key; Cmd+Opt+=/- font; Cmd+=/- resize.
2. `cmd || ctrl`: sheet edit keys → Ctrl+J/K pane focus → **Ctrl+Tab / Ctrl+Shift+Tab
   tab cycle (wraps)** → Ctrl+Shift+HJKL resize → vim-pane shortcuts →
   terminal (Cmd+C/V only) → Cmd+L → **file browser keys (Ctrl+N/P move,
   Cmd+K copy abs path, Cmd+A/C/V/X/Z)** → generic edit keys.
3. `editMode`: terminal focused (Esc passes to shell; Nth rapid Esc closes),
   vim pane (Esc → pty; Nth rapid Esc closes only in Normal mode), find
   bar (single Esc closes bar), Esc (streak), Cmd+S, Cmd+O.
4. list navigation (Up/Down/Tab/C-n/C-p/Return), then Esc (streak).

## Keyboard shortcuts (user-facing)

- Esc: `esc-close` rapid presses (default 3, `[app]` or per section; 1 =
  single, 0 = never) close notes/files/jira/detail/output windows. The
  switcher palette always closes on one Esc. Find bar: one Esc.
- Ctrl+Tab / Ctrl+Shift+Tab: next/prev tab (notes, jira sources), wraps.
- File browser: Ctrl+N/P next/prev result, Cmd+K copy selected row's
  absolute path (+ toast), Cmd+L focus filter bar, Tab completes, Enter opens.
- Ctrl+J/K: move focus between editor / browser / terminal panes.

## Config defaults worth knowing

- `[app] float` default **false** (windows are normal, not floating).
- `[app] esc-close` default 3; per-section `esc-close` (alias `vim-esc-close`).
- `[app] copy-toast` default `Copied {} to clipboard` (`{}` = ~-path; empty = off).
- Vim pane (`vim/notes-init.vim`): `number` + `cursorline` on; cursor-line
  color `g:ws_line` = highlight color blended 50% toward the card color.

## Verifying vim-pane changes without UI tests

```bash
S=$(ls -t ~/.cache/workspace-switcher/nvim-notes-*.sock | head -1)
nvim --server "$S" --remote-expr 'execute("set number? cursorline?")'
```
