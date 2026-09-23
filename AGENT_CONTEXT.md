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
- `AGENT_CONTEXT.md` — architecture, window types, known bugs, code locations
- `BUG_window_shake.md` — known drag-shake bug analysis
