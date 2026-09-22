# workspace-switcher Agent Context

## What This Is

A Swift/AppKit menu-bar app that provides:
- Workspace switching via AeroSpace (popup list of workspaces)
- Note editing (markdown, with tabs, voice dictation, terminal drawer, file browser)
- Jira ticket list/detail viewer
- Command palette (shell commands, output windows, file browser)
- All windows are translucent, draggable, resizable popup panels

## Project Structure

```
workspace-switcher/
├── main.swift                  # Entry point, AppDelegate setup
├── workspace_switcher.swift    # App logic: controller, commands, voice, menus (4620 lines)
├── PopupWindow.swift           # Reusable popup window framework (6866 lines)
├── commands.conf               # Config: windows, commands, colors, paths
├── build.sh                    # Build + relaunch script
├── bin/
│   ├── workspace_switcher.sh   # Main build script
│   └── ui-test.sh              # End-to-end UI tests (cliclick + osascript)
├── jira/                       # Jira sync scripts
├── config/                     # AeroSpace config
└── Vendor/SwiftTerm/           # Embedded terminal (SwiftTerm library)
```

## Key Architecture

### PopupWindow Framework (PopupWindow.swift)
- `PopupWindow` class wraps either `PopupPanel` (NSPanel, borderless) or `PopupPlainWindow` (NSWindow, titled)
- `PopupBackdrop` — rounded container with blur + tint
- `PopupChrome` — header bar with buttons, handles drag/resize
- `PopupRowView` — draws searchable row list
- Feature drawers: terminal (SwiftTerm), file browser, voice recording

### App Controller (workspace_switcher.swift)
- `SwitcherController` — manages all windows, AeroSpace IPC, command server
- `AppDelegate` — NSApplicationDelegate, menu bar status item
- `MenuTarget` — shared target for menu actions
- `VoiceRecorder` — live transcription with AVAudioEngine + SFSpeechRecognizer
- `CommandRunner` — persistent bash process for shell commands

### Window Types
| Type | Panel Class | enableDrag | sticky | Purpose |
|------|-------------|------------|--------|---------|
| Switcher popup | PopupPanel (.nonactivatingPanel) | false | false | Workspace list |
| Notes | PopupPlainWindow (.titled) | cmd.drag (default true) | true | Markdown editor |
| Jira list | PopupPlainWindow | cmd.drag | true | Ticket list |
| Jira detail | PopupPlainWindow | true | true | Single ticket view |
| Health checks | PopupPlainWindow | true | true | Shell output |
| Prettyprint | PopupPlainWindow | true | true | JSON/XML formatter |
| Files | PopupPlainWindow | cmd.drag | true | File browser |

## Build

```bash
./build.sh           # Build + relaunch
./build.sh --force   # Force rebuild
./build.sh --build-only  # Build only, no launch
```

Binary: `workspace-switcher.app/Contents/MacOS/workspace-switcher`

## Testing

```bash
bin/ui-test.sh              # Full UI test suite
bin/ui-test.sh --verbose    # Verbose output
bin/ui-test-drag.sh         # Drag shake regression test (standalone)
```

Tests use `cliclick` (brew install cliclick) + `osascript` for UI automation.

## Known Bugs

### Window Shake on Drag (see BUG_window_shake.md)
**Symptom:** Window shakes/jitters when dragged immediately after opening from menu bar.

**Root cause:** `takeFocus()` (PopupWindow.swift:5621) retries up to 10x at 150ms intervals, each time calling `makeKeyAndOrderFront`. Combined with `didBecomeActiveNotification` handler (line 5654) which also calls `makeKeyAndOrderFront`, these fight the native drag when user starts dragging during the 1.5s retry window.

**Fix needed:**
1. Cancel `takeFocus()` retry when drag starts
2. Suppress `didBecomeActiveNotification` handler during drag
3. Consider reducing retry count (10 → 3) and interval (150ms → 100ms)

**Regression test:** `bin/ui-test-drag.sh` — measures window position jitter after open+drag

## Important Code Locations

| Concern | File | Line |
|---------|------|------|
| Window creation (titled vs borderless) | PopupWindow.swift | 4417-4439 |
| takeFocus() retry loop | PopupWindow.swift | 5621-5641 |
| didBecomeActiveNotification handler | PopupWindow.swift | 5654-5670 |
| Chrome drag (performDrag) | PopupWindow.swift | 3411-3416 |
| Chrome mouseDragged (setFrameOrigin) | PopupWindow.swift | 3420-3451 |
| Backdrop drag | PopupWindow.swift | 865-873 |
| Backdrop mouseDragged | PopupWindow.swift | 875-907 |
| show() → presentList() | PopupWindow.swift | 5084-5159 |
| clampToScreen() | PopupWindow.swift | 10-22 |
| Menu bar status item install | workspace_switcher.swift | 4403-4432 |
| Notes window open | workspace_switcher.swift | 2506-3282 |
| Focus restoration | workspace_switcher.swift | 4293-4307 |

## Config (commands.conf)

INI-style config file. Key sections:
- `[app]` — shell, paths, CLI locations, socket names
- `[theme]` — app-wide color overrides (background, border, text, etc.)
- `[icons]` — per-app icon overrides
- `[notes]` — note window config (paths, voice, terminal)
- `[jira]` — Jira list window config (sources, fields, filters)
- `[health-checks]` — shell command output window
- `[prettyprint]` — JSON/XML formatter window
- `[files]` — file browser window
- Plain lines: `name = script` for shell commands

## Permissions / TCC

The app bundle needs:
- Microphone access (voice dictation)
- Speech recognition (voice dictation)
- Accessibility (AeroSpace focus integration)

TCC is keyed by bundle ID — permissions survive rebuilds.

## Common Patterns

- **Single-instance guard:** Each window type checks `subWindows.first(where:)` before creating
- **Focus restore:** `savedWID`/`savedPID` captured at open time, restored on hide
- **Sticky windows:** `config.sticky = true` prevents click-off dismiss
- **Persistent windows:** Notes/jira survive Esc hide (showPersistent), preserving terminal sessions
- **Config-driven:** New windows require only commands.conf entries + code handlers
