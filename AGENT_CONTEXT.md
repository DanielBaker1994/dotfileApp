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

ONE build: `bin/build-app.sh` (compiles every top-level `*.swift`, SwiftTerm
lib, sign, TCC) — used by build.sh, `bin/workspace_switcher.sh` and
INSTALL.sh. Install/uninstall/build names + paths (bundle id, brew deps,
launchd agent, caches) live in `install.conf`.

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
- `jira/jira_*.py` — python jira poller (see AGENT_CONTEXT.md "Jira poller";
  `jira_log.py` = debug.log + raw response dumps);
  tests: `python3 Tests/test_jira_poll.py`
- `vim/notes-init.vim` — nvim pane init (theme vars `g:ws_*` from `vimArgs`)

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
  not "fix your token". Tests swap `jira_api.SLEEP`.
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
- Switch: `[jira] enabled` in commands.conf gates the window, the launchd
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
- Jira Config window look: `JiraConfigNSWindow` (titled + hidden titlebar,
  `_cornerRadius` override, header clicks caught in `sendEvent`) +
  `themedRoot` (blur, card tint, border, a real `PopupChrome` header: ✕ ·
  icon · title). Status lines are one sentence; details live in tooltips.
  Job pages show "Defined in config.json › endpoints › NAME" + Open config.json.
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
  releases (`project_releases`: unarchived versions) and labels
  (`project_labels`: labels of the newest `search_defaults.labels_max_issues`
  labelled issues, fields=labels) → `~/.cache/jira/directory.json`.
  `jira_poll.py --directory` = run it now.
- Live search (replaced saved searches; `migrate_v3` drops `searches` and
  their `search-*.json` tabs): Cmd+F in the Jira window (`PopupWindow.onCommandF`,
  list mode only) or icon menu "Search Jira…" → `JiraSearchPanel`
  (`JiraSearch.swift`: child panel docked above/below the Jira window; free
  text (`text ~`: title, description, comments) + Projects + "+ Filter" rows;
  every known value (users, status, type, priority, Release, Labels — the
  last two scoped to the picked projects) is a `JiraMultiPicker` over the
  directory, never typed text; only "… contains" rows are text; no Raw JQL;
  last criteria in UserDefaults; filter rows live in a height-capped
  scroll view (`rowsScroll`) so `place()` keeps the panel docked above the
  window instead of overlapping it). Drawn in the Jira window's theme
  (`applyTheme`: appearance, blur + tint, `JiraInputBox`, `JiraChoiceButton`,
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
| `showToast(_:symbol:)` | Raycast-style bottom-center pill (fade/rise, 1.4 s); used by Cmd+K copy; explicit frames, icon+text centered |
| `PopupPlainWindow._cornerRadius` | makes the system window frame use `config.cornerRadius` (else macOS 26's 16pt frame peeks out around the card) |
| `PopupChrome.closeButtonRect` | ✕ glyph far-left of the drag header (`PopupConfig.headerCloseButton`, default on; titled windows only); icon sits right of it (`leftInset`) |
| `PopupFileBrowser.updatePartFocus` | which part has focus: filter bar (bright 2px outline) / list / preview (`partRing`); KVO on `firstResponder` |
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

- Esc: `esc-close` rapid presses (default 2, `[app]` or per section; 1 =
  single, 0 = never) close notes/files/jira/detail/output windows. The
  switcher palette always closes on one Esc. Find bar: one Esc.
- Ctrl+Tab / Ctrl+Shift+Tab: next/prev tab (notes, jira sources), wraps.
- File browser: Ctrl+N/P next/prev result, Cmd+K copy selected row's
  absolute path (+ toast), Cmd+L focus filter bar, Tab completes, Enter opens.
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
- `ThemePreset` (13 colors, swatch = mini window). Live: `setTextColors(…,
  palette:, border:)` → `pushColors()` walks `PopupThemeable` views; also
  the shell's ANSI colors (`ansiPalette`) and vim (`vimPaletteLets` →
  `g:ws_accent…`, ONE `--cmd`: nvim allows max 10).
- AppKit forms: `ThemedPushButton` (`role` .primary/.danger),
  `ThemedPopUpButton`, `PopupTableRowView`; initial colors from
  `PopupThemeDefaults.colors`. Jira table cells: `jiraCellTone`.

## Config defaults worth knowing

- `[app] float` default **false** (windows are normal, not floating).
- `[app] esc-close` default 2; per-section `esc-close` (alias `vim-esc-close`).
- `[app] copy-toast` default `Copied {} to clipboard` (`{}` = ~-path; empty = off).
- Vim pane (`vim/notes-init.vim`): `number` + `cursorline` on; cursor-line
  color `g:ws_line` = highlight color blended 50% toward the card color.

## Verifying vim-pane changes without UI tests

```bash
S=$(ls -t ~/.cache/workspace-switcher/nvim-notes-*.sock | head -1)
nvim --server "$S" --remote-expr 'execute("set number? cursorline?")'
```
