# PRD — Hyper+S palette: Raycast-style back navigation

## Problem
Esc inside a tool opened from the palette (Find Recent Files, Create File, Text to Table,
Pretty Print) closes everything and drops you back in the previous app. Raycast treats the
palette as the root of a navigation stack: Esc pops one level.

## Behavior (Raycast model)
| Where you are | Esc does |
|---|---|
| Palette, filter text present | clears the filter (unchanged) |
| Palette, empty filter (root) | closes the palette, hands focus back to the previous app |
| Tool/paths opened from the palette | closes the tool and **reopens the palette** |
| Tool opened any other way (CLI, Hyper+X, ...) | closes as before |
| Click-off / window button / Return-save | closes as before (no palette) |

Raycast reference: Esc pops to the previous screen; only on the root command list does it hide
the window. Opening a command keeps the root one Esc away.

## Implementation
- `quick.rs`: `QuickUi.back_to` records the surface a palette pick opened (+ the app to return to).
- Paths shelf: `PathsEvent::Escape` -> `surface_closed("paths", true)`.
- Tool panels: their Esc handlers differ (several monitors), so `poll_tick` detects the
  shown -> hidden transition and checks `NSApp.currentEvent` is an Esc key-down < 0.5 s old.
- `surface_closed` shows the palette again and restores the hand-back pid so the final Esc
  still returns to the original app.
- Palette width 560 -> 440 (`PALETTE_WIDTH`), a single narrow list like Raycast.

## Out of scope
Palette query is not restored on return (fresh list). Main-daemon views are unchanged.

## Verify (needs an unlocked screen)
Hyper+S -> Pretty Print -> Esc: palette is back. Esc again: gone, previous app focused.
