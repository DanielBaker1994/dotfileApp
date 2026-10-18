# Shortcut parity — Swift (608d07d) vs Rust port

> **Stale marks:** many ❌/🟡 rows below were fixed on Oct 11 (second session).
> The current status is the top section of `HANDOFF-rust-port-next.md`.


One-time audit (Oct 11 2026). Sources: the `[shortcuts]` table in
`commands.toml` (unchanged since Swift — it is what Hyper+/ / `ws-settings`
lists) **plus** the Swift key handlers, read from git:

```bash
SW=$(mktemp -d); git archive 608d07d $(git ls-tree -r --name-only 608d07d | grep '\.swift$' | grep -v '^Tests/\|^rust/\|^notify/') | tar -x -C $SW
rg -n 'func handleKey|override func keyDown|func prefixKey|func modifiedKey|func overlayKey' $SW
```

Swift key path, in order: `SharedWindow.prefixKey` (Ctrl+B prefix, Ctrl+HJKL,
`VimKeys`, `PopupTabsBar.handleNavKey`) → `PopupWindow.handleKey`
(prose search → `overlayKey` → `windowSizeKey` → `modifiedKey` → prose ↑↓ →
`jumpLeaderKey` (Space s f) → `editorKey`/`listKey`) → per-view
`handleKey` overrides (`JiraDashboard`, `Confluence`, `AIWindow`,
`CompareWindow`, `CompareFolderView`, `CardWindow`, `ScreenshotOverlay`,
`PathsWindow`). Rust: `route_key_event` (host.rs) → `PrefixState::handle` →
`popup::route_key`; per-view Rust key models exist for AI (`AIState::handle_key`)
and paths (`PathsShelf::key`) but are **not connected**.

Legend: ✅ works · 🟡 partial / differs · ❌ missing · ❓ unverified

## Global / shared window

| Shortcut | Swift | Rust status |
|---|---|---|
| Ctrl+B L | previous view; falls back to notes⇄files when no previous | ✅ (fallback added) |
| Ctrl+B W | view switcher (`ViewSwitcher.swift`) | ✅ drawn rows on `ListPanel` (number, icon, name, where, ● here); Jira/Confluence/Compare/AI "where" text still empty |
| Ctrl+B T | notes: editor ⇄ prose (`toggleProseFromPrefix`), else terminal panel | ✅ |
| Ctrl+B B | sidebar ⇄ rail | ✅ |
| Ctrl+B Ctrl+B / Ctrl+B Ctrl+HJKL | pass real key through | ✅ (state machine) |
| Ctrl+H/J/K/L | move focus between panes (focus ring) | ❌ key caught, but only a model over test panes moves; no AppKit focus, no ring |
| Ctrl+Shift+H/J/K/L | resize window / drawer | ✅ window; drawer heights ❓ |
| Ctrl+Shift+J/K in vim pane | VM add cursor down/up | ✅ |
| Cmd+\\ | sidebar rail | ✅ |
| Cmd+± / Cmd+0 | text zoom (prose zooms its page) | ✅ notes (prose page / font sizes); 🟡 list views still resize the window (`setTextZoom` not ported) |
| Cmd+Opt+± | grow / shrink window | ✅ |
| Cmd+W | hide | ✅ |
| Cmd+/ | shortcuts sheet (`showShortcuts`, rows from `[shortcuts]`) | ✅ `ui/shortcuts.rs` (all views use the overlay; Swift used an alert for AI/Confluence) |
| Cmd+K | action picker (`actionPicker`) | ❌ logs "not modelled" |
| Ctrl+W | delete word in any text field | 🟡 compare editor only ❓ elsewhere |
| Ctrl+Tab / Ctrl+Shift+Tab | cycle views | ✅ |
| Header app icon ▸ Theme | preset menu w/ live preview, scopes, custom color, transparency | 🟡 Theme ▸ (tone groups, swatches, notes scopes, Reset) + Reset Size/Colors, Open Config, Shortcuts; no live hover preview, Custom Color…, Transparency ▸ |
| Sidebar ↑↓/Ctrl+N/P, Return, Space, Home/End, letter, Esc, Delete | `PopupTabsBar.handleNavKey` | ❌ no Rust counterpart |
| j/k, gg/G, Ctrl+D/U, / ?, n/N, i/a (normal mode) | `VimKeys` | ❌ `VimKeys` held in state, never consulted |
| Preview ↑↓/PgUp/PgDn/Space, Cmd+A/C | preview pane | ❓ |

## Palette (Hyper+S)

| / command mode, ↑↓/Ctrl+N/P, Return, Esc | ✅ |

## Notes

| Shortcut | Rust status |
|---|---|
| Cmd+Shift+P reading view ⇄ editor | ✅ `views/notes_prose.rs` (scripts ported verbatim, vim line sync both ways) |
| Cmd+Shift+O prose in own window | ✅ |
| Prose switch capsule (Prose / nvim / ◐ style / ⤢ pop out) | ✅ (◐ = `doc_templates` menu) |
| Esc in prose | ✅ (+ `/` find, n/N, gg/G, Ctrl+D/U, j/k scroll) |
| Cmd+P (prose) export PDF | ✅ (path copied + toast) |
| Cmd+S save / Cmd+F find / Cmd+O open path | ✅ (vim + editor) |
| Cmd+N new note now / Ctrl+N new from template | ✅ |
| Space s g / Space s f (nvim → `notes-grep` / `notes-find`) | ✅ `views/note_find.rs` (`NoteFindUi`); the notes pane sends them raw over `g:ws_sock`; the host VimKeys `<leader>sf` opens `PaneFindUi` on the non-nvim panes |
| Space i insert picker | ✅ (nvim-side) |
| Cmd+C/V/Ctrl+V editor + terminal | ✅ |

## Files

| Shortcut | Rust status |
|---|---|
| ↑↓ / Ctrl+N/P, Return | ✅ |
| ← / → parent / open | ❌ |
| Tab complete path | ❌ |
| Cmd+L focus filter | 🟡 action is a no-op |
| Cmd+K copy path | ✅ |
| Cmd+R / F2 rename | ❌ logs "not modelled" |
| Space Quick Look | ❌ |
| Cmd+C/X/V files, Cmd+D, Cmd+Delete, Cmd+Z, Cmd+Shift+N, Cmd+Opt+V | ❌ (Copy/Paste/Cut/Undo return false) |
| Cmd+[ / Cmd+], Cmd+Up / Cmd+Down | ❌ |
| Shift+↑↓, Home/End/PgUp/PgDn | 🟡 paging ✅, extend ❌ |
| Cmd+Shift+. hidden files | ❌ |
| `**` / `term` in filter | ❓ |
| Esc Esc hide | ✅ |

## Jira (`JiraDashboard.handleKey`)

| Shortcut | Rust status |
|---|---|
| ↑↓ / Ctrl+N/P, Return | ✅ (search field + list move) |
| Tab, Home/End/PgUp/PgDn | ❓ |
| Cmd+F live search, Cmd+I issue panel | ❌ |
| Space s f sidebar jump (`SidebarJump.swift`) | ❌ |
| Cmd+K actions, 1-9 in it | ❌ |
| Esc back / close search | 🟡 field Esc only |

## Confluence (`Confluence.handleKey`)

| Return / ↑↓ / Ctrl+N/P / Esc | ✅ (search field) |
|---|---|
| Cmd+Return, Cmd+C / Cmd+Shift+C, Cmd+D, Cmd+G / Cmd+Shift+G, Cmd+L / Cmd+F, Cmd+R | ❌ |

## AI (`AIWindow.handleKey`)

| Ctrl/Cmd+Enter run, Cmd+L, Cmd+Z/Shift+Z, Esc stop | ❌ `AIState::handle_key` exists, unwired (Esc-stop ✅ via field) |
|---|---|

## Compare / compare-folders (`CompareWindow.handleKey`, `CompareFolderView.handleKey`)

All ~45 rows (Cmd+0, Cmd+N, Ctrl+N/P diffs, Opt+←/→ copy, Cmd+1-4 filters,
Cmd+F/G, Ctrl+G, Cmd+S, Cmd+O, Cmd+L, Cmd+V, Cmd+Opt+X, F5, Cmd+K, folder
tree keys, Quick Look, sync…): ❌ no key routing for compare views (only the
section editor's Ctrl+W). The compare model has the operations (`do:compare:*`).

## Paths (`PathsWindow`)

| ↑↓, Return, Cmd+C, Cmd+Shift+C, Space/Cmd+Y, Cmd+O, Cmd+R, Cmd+Shift+R, Cmd+Delete, Esc/Cmd+W | 🟡 `PathsShelf::key` modelled; field Esc/Return wired, the rest ❓ |
|---|---|

## Screenshot overlay

Tools / selection / undo / copy / save / OCR keys: ✅ `ScreenshotState::handle_key`
is wired in the overlay's own monitor (per-key verification ❓).

## Done this pass (Oct 11)

Ctrl+B T/L, switcher look, in-window prose + capsule, Cmd+Shift+P/O, Cmd+P,
Cmd+N/Ctrl+N, Cmd+± swap fix, Cmd+/ sheet, header-icon Theme menu, shared
`ui/toast.rs` (Swift `ScreenToast`; nothing drew toasts before).

Open observation: once, a view-switcher panel showed while `viewSwitcher.shown`
was false. Not reproduced: open → Esc closes it cleanly.

## Fix order (remaining)

1. Cmd+K action picker (`actionPicker`).
2. Palette (Hyper+S) rows are still plain `NSButton`s — move to `ListPanel` like the switcher.
3. Wire per-view keys: AI, paths, jira, confluence, files, compare.
4. `VimKeys` normal mode + sidebar nav keys + real pane focus.
