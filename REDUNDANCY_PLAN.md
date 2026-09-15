# SwiftUI Redundancy Deduplication Plan

## Context

The `SwiftUI/` directory is a WIP migration from legacy AppKit code in
`PopupWindow.swift` and `workspace_switcher.swift`. The SwiftUI module is
**not yet compiled** by the build script (`bin/workspace_switcher.sh` only
compiles `main.swift`, `workspace_switcher.swift`, `PopupWindow.swift`).

## Identified Duplications

### 1. `PopupColors` struct — EXACT DUPLICATE

- **Legacy:** `PopupWindow.swift:344` (public)
- **SwiftUI:** `SwiftUI/Models/DataModels.swift:150`
- Both have identical properties and identical default color values
- **Plan:** Keep `PopupWindow.swift` as single source. SwiftUI imports it.

### 2. `PopupRow` protocol — EXACT DUPLICATE

- **Legacy:** `PopupWindow.swift:554` (public)
- **SwiftUI:** `SwiftUI/Models/PopupRow.swift:5`
- SwiftUI adds `searchText` to protocol (not in legacy)
- **Plan:** Add `searchText` to legacy protocol, delete SwiftUI copy.

### 3. Row types (`WorkspaceRow`, `CommandRow`, `FieldRow`) — DUPLICATED

- **Legacy:** `workspace_switcher.swift:1384-1431`
- **SwiftUI:** `SwiftUI/Models/PopupRow.swift:30-65`
- **Plan:** Unify on SwiftUI init style (pure data, no side effects).

### 4. `PopupFuzzy` enum — NEAR DUPLICATE

- **Legacy:** `PopupWindow.swift:583` (returns `positions` from `matchToken`)
- **SwiftUI:** `SwiftUI/Utilities/PopupFuzzy.swift:5` (simplified, no positions)
- **Plan:** Keep legacy version (has `positions` needed for match highlighting). Delete SwiftUI copy.

### 5. Filter logic — LOGIC DUPLICATE

- **Legacy:** `workspace_switcher.swift:2089` — `filter(_ query:) -> [PopupRow]`
- **SwiftUI:** `AppState.swift:55` — `filterRows(query:workspaces:commands:)`
- **Plan:** Extract pure function `PopupFuzzy.filterRows(...)`. Both call it.

### 6. `iconForApp` + `missingIcon` — DUPLICATED

- **Legacy:** `workspace_switcher.swift` (~1340-1380)
- **SwiftUI:** `AppState.swift:101-124` + `:142-162`
- **Plan:** Extract to shared `IconResolver`. Both use it.

### 7. `VoiceState` / `VoiceRecorder.State` — DUPLICATED

- **Legacy:** `workspace_switcher.swift:1442` — `enum State` in `VoiceRecorder`
- **SwiftUI:** `PopupRow.swift:92` — `enum VoiceState`
- Same 4 cases, same raw values
- **Plan:** Keep `VoiceRecorder.State`. Delete SwiftUI copy.

### 8. `PopupMode` vs `commandMode: Bool` — CONCEPTUAL DUPLICATE

- **Legacy:** tracks mode with `var commandMode: Bool`
- **SwiftUI:** `enum PopupMode` with `.list` / `.editor`
- **Plan:** Target: replace Bool with enum. Defer until full migration.

## Testing Strategy (REQUIRED for every step)

For **each** deduplication:

1. **Equivalence test** — run BOTH old and new with same inputs, assert same outputs
2. **Compilation test** — `swiftc` must compile all sources without errors
3. **Post-deletion test** — `bin/ui-test.sh` must pass
4. **Build test** — `./build.sh --force` must succeed

Tests go in `Tests/test_redundancy.swift`.

## Execution Order

1. Write behavioral equivalence tests first (prove old == new)
2. Consolidate `PopupColors` → test → delete SwiftUI copy
3. Consolidate `PopupRow` protocol → test → delete SwiftUI copy
4. Consolidate row types → test → delete legacy copies
5. Consolidate `PopupFuzzy` → test → delete SwiftUI copy
6. Consolidate `iconForApp` / `missingIcon` → test → delete duplicates
7. Consolidate filter logic → test → delete duplicates
8. Consolidate `VoiceState` → test → delete SwiftUI copy
9. Run full `bin/ui-test.sh` suite
10. Run `./build.sh --force`

## Proposed File Changes

| File | Change |
|------|--------|
| `PopupWindow.swift` | Add `searchText` to `PopupRow` protocol; single source for `PopupColors`, `PopupFuzzy` |
| `SwiftUI/Models/DataModels.swift` | Remove `PopupColors` |
| `SwiftUI/Models/PopupRow.swift` | Remove protocol, row types, `VoiceState` |
| `SwiftUI/Utilities/PopupFuzzy.swift` | Delete entirely |
| `SwiftUI/Models/AppState.swift` | Remove `filterRows`, `iconForApp`, `missingIcon` |
| `workspace_switcher.swift` | Extract `IconResolver`, shared filter function |
| `Tests/test_redundancy.swift` | New — behavioral equivalence tests |
