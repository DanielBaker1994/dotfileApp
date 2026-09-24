# workspace-switcher

Swift/AppKit macOS menu-bar app. Full project context: `AGENT_CONTEXT.md`.

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
- `workspace_switcher.swift` — app logic (~4600 lines)
- `PopupWindow.swift` — popup window framework (~6900 lines)
- `commands.conf` — config (windows, commands, colors, paths)
- `jira/jira_*.py` — python jira poller (see AGENT_CONTEXT.md "Jira poller");
  tests: `python3 Tests/test_jira_poll.py`
- `AGENT_CONTEXT.md` — architecture, window types, known bugs, code locations
- `BUG_window_shake.md` — known drag-shake bug analysis

## Jira poller (python, v2)

- Switch: `[jira] enabled` in commands.conf gates the window, the launchd
  agent (`syncJiraLaunchAgent()`, re-run on every `reloadConfig()`), and the
  poll itself (`jira_poll.py` no-ops when false unless `--force`).
- Menu: `installStatusMenus` → "Toggle Jira Poll" (`SwitcherController.toggleJiraPoll`:
  `jira_config.py --check` → setup window if missing → `jira_api.py --myself`
  → `setJiraEnabled(true)`) and "Jira Poll…" (`buildJiraPollMenu`, rebuilt on open).
  Socket messages `jira-poll-on/off/toggle`, `jira-setup` (CLI: `workspace-switcher jira-poll …`).
- Setup window: `JiraSetupWindow` (plain NSWindow above `.popUpMenu`, own key
  monitor for edit shortcuts + Esc). Token goes to python over stdin only.
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
  transforms, lock, disabled no-op, env-token override).
