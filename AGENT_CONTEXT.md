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
  signed app) + links `bin jira confluence notify vim install.conf` →
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
  over between the two installs (a checkout is never deleted: unlinked or
  renamed `…repo-<date>`; an app home goes to
  `~/.config/workspace-switcher-backups/`). Home owned by a checkout → exit
  3, nothing touched. `stack` = `symlinks.sh` (`WS_LINK_ROOT` = the home in
  app mode) + precompiled sketchybar helpers + brew services. Never git.
- `bin/preflight.sh [--json] [--mode repo|app] [--app PATH]`: ONE check list
  for INSTALL.sh (step 0) and the Setup window. Required: macOS ≥
  `MACOS_MIN`, arm64 + not on the disk image (app), swiftc + git (repo).
  Warnings: Apple on-device model (`fm available` → AI view off), python3
  (never runs the CLT stub), nvim, Homebrew + each formula / cask, config
  links, another install owning the home, a second copy of the app.
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
```

## Key files

- `main.swift` — entry point
- `workspace_switcher.swift` — app logic (~7300 lines)
- `PopupWindow.swift` — popup window framework (~8500 lines; all keys in `handleKey`)
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
  Esc: notes never (`escCloseCount 0`, Cmd+W / ✕ / hotkey hide), jira = back.
  Focus loss: ONE path for every view (`checkFocusLoss`, popups set
  `hostHandlesFocusLoss`): after `[app] focus-loss-delay` (0.3s) and
  `focusLeft(w)` → `hide(…, restoreFocus: false)` (parks, never tears down);
  within 1s of a view swap it was stolen (aerospace focusing the next
  window) → the view is re-shown instead.
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
  listed inside it; `present` = exact-case check (case-only renames).
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
- `vim/notes-init.vim` — nvim pane init (theme vars `g:ws_*` from `vimArgs`)

## AI view

- `AIWindow.swift` — `AIWindow` (a `JiraConfigNSWindow` + Confluence-style
  themed root; `SlotMember`, view `.ai`, nav id 66 placed RIGHT AFTER notes
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

- `Confluence.swift` — `ConfluenceWindow` (a `JiraConfigNSWindow` + the Jira
  Config window's themed root; `SlotMember`, view `.confluence`, nav id 65,
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
| `saveConfigValue(s)`, `removeConfigValue` | write back into commands.toml |
| `THEME`, `BAR`, `GROUP_BG`, `TEXT`, `DIM` | `[theme]` globals |
| `vimArgs(for:socket:file:)` | nvim launch args, passes `g:ws_fg/dim/sel/line` |
| `showDetail` | jira detail window (PopupConfig built here) |
| `openOutputWindow` / `openNoteWindow` / `openListWindow` / `openFilesWindow` | build `PopupConfig` per window type — per-window config goes here (`cfg.floating = cmd.float ?? settings.float` line is a good anchor) |
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
| `focusedVim()`, `vimRemote`, `vimEval`, `vimCommand` | nvim pane + RPC |
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

## Hotkey fast path (Hyper+N / T)

- ONLY two window hotkeys besides Hyper+S: Hyper+N = `window`
  (`SharedWindow.toggle`: hidden → the view you were LAST on (`last`), in it →
  hide, elsewhere → focus) and Hyper+T. No per-view hotkeys (Hyper+F / J
  removed): views switch via Ctrl+Tab, header icons, the Hyper+S palette.
  The named modes (`notes|files|jira|confluence|ai`) remain as CLI / socket
  messages.

- aerospace runs the app BINARY (`workspace-switcher window|terminal`),
  not the script: `main.swift` pings the socket (~20 ms) and exits. No
  daemon (ppid != 1) → it execs `bin/workspace_switcher.sh MODE` (cold start:
  build-if-stale + LaunchServices `open -n -g`; a launchd-parented process
  never re-execs).
- The daemon's socket thread runs `SwitcherController.hotkeyPrep()` before the
  main thread sees the message: `list-windows --focused` (→ focus file) and
  `list-windows --all` IN PARALLEL (aerospace ≈ 20-25 ms per query — the
  floor), then `move-node-to-workspace` only for our windows on another
  workspace. No sleeps. Log: `/tmp/ws-debug.log` `hotkey X: prep N ms, M ms
  to shown`.
- The script builds only when no daemon answers / `WS_BUILD_ONLY`;
  `./build.sh` runs `build-app.sh` itself. `WS_DEBUG=1` → `$TMPDIR/ws-launch.log`.
- Hyper+T (`terminal` → `slotToggleTerminal`): notes hidden / you're
  elsewhere → notes + terminal drawer focused; in it → drawer toggles (close
  = editor focused). `PopupWindow.setTerminalDrawer(_:)`.
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
- Our windows float via aerospace's dialog/panel heuristic; the
  `app-name = workspace-switcher → layout floating` rule is commented out.
- Fixes (not applied): `focus --ignore-floating <dir>` so directional
  focus only walks tiles (reach popups by hotkey), or place the window so
  its center lands clearly on one side of a tile.

## Keyboard shortcuts (user-facing)

- Esc: `esc-close` rapid presses (default 2, `[app]` or per section; 1 =
  single, 0 = never) close notes/files/jira/detail/output windows. The
  switcher palette always closes on one Esc. Find bar: one Esc.
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
- Hyper+T: notes terminal drawer (show + focus / close → editor).

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
