# UI-TEST-FINDINGS.md — manual UI pass: Hyper+S palette + notes app

Run: 2026-10-12, live app (`kitchen-sink window`), driven with real CGEvent mouse/key input
(tool: scratchpad `ev`), screenshots read back, state cross-checked over the socket.
Not covered (session cut short): Text to Table, Find Recent Files, Health Checks, Kitchen Sink
palette rows; notes find bar, Ctrl+B T prose toggle by key, drag/resize, terminal drawer.

## Bugs

| id | sev | where | finding | repro |
|---|---|---|---|---|
| U1 | S1 | notes | **Data corruption.** After clicking `+` (new note) the new `Untitled-4.md` is selected in OPEN, but the pane keeps showing the previous file (hello.md) and keyboard input goes to hello.md, which is autosaved to disk. | Open hello.md, switch to Prose view, click `+` in OPEN, press Cmd+V / type. hello.md gained the pasted text (433 B -> 444 B); Untitled-4.md stayed the 109 B template. |
| U2 | S2 | palette | **Typing `n` or `p` in the filter field is swallowed.** "pretty" -> "retty", "nonet" -> "oet", "xpxpp" -> "xx". Cannot type "notes", "new", "prose", "pretty". | Hyper+S, type `p`. Cause: `rust/ws-rs/src/app/host.rs` ~L4665 forwards key codes 45 (n) and 35 (p) to `p.key()` and returns `Consumed` regardless of Ctrl; only Ctrl+N / Ctrl+P should be consumed (the switcher branch below does check `ctrl`). |
| U3 | S2 | palette/tools | **Ghost palette after Esc on a palette-opened tool.** Hyper+S -> click Pretty Print -> Esc: tool disappears, the palette is painted again but is dead (state `palette:false`, Down arrow moves the notes editor cursor, Esc does nothing). `tools.prettyprint.shown` stays `true`. Hyper+S twice clears it. Violates rule 6 (overlays own the keyboard). | Reproduced twice. Create File (Enter, then Esc) also leaves `tools.filefast.shown:true`. |
| U4 | S3 | palette | Right-hand shortcut column (1/2/9/G/M/Y + app icons) is row-aligned with the command list but unrelated to it (workspace badges). When filtered, a stray `Y` badge sits beside "Find Recent Files". Reads as if the badge belongs to the row. | Hyper+S, type `re`. |
| U5 | S3 | palette | Empty filter field has no placeholder/label; no-match query collapses the list with no "No results" message. | Hyper+S; type `zzq`. |
| U6 | S3 | notes prose | Prose view has no left padding (text flush against the pane edge / sidebar), long lines run to the right edge. | Notes, open hello.md, click `Prose`. |
| U7 | S4 | notes nvim | Line-number gutter: 2-digit numbers touch the pane's left border (1-digit are inset), "35 nd" renders at a different x. | Notes, hello.md in nvim view. |
| U8 | S4 | notes sidebar | Right-click menu opens with "Open in Notes" pre-highlighted before the pointer hovers it. | Right-click a file in PINNED. |

## Verified working (real input)

- Hyper+S opens the palette (`palette:true`); second Hyper+S closes it.
- Up/Down move selection, wrap at both ends; Backspace edits the filter; Esc clears a query first, closes on the second press.
- Mouse click on a palette row runs it (Pretty Print opened); Enter runs the selected row (Create File opened).
- Pretty Print: Cmd+V paste auto-formats JSON; "copy contents" writes the formatted text to the clipboard and shows a check.
- Create File: typing incl. `p`/`n`, Tab to the paste field, Esc closes, no file created.
- Notes: sidebar click opens a file; right-click menu has Open in Notes / Copy File / Duplicate / Copy Absolute Path / Reveal in Files / Delete; Copy Absolute Path put `/Users/danielbaker/notes/hello.md` on the clipboard; Reveal in Files switched to Files with hello.md selected.
- Prose/nvim pills toggle; Ctrl+B W opens the view switcher; Esc closes only it; `2` jumps to Files; Down+Down+Enter jumps to Jira.

## Test residue left in the user's data (needs the owner's call)

- `~/notes/hello.md`: first line now ` Spasted-namAVEMARKER SAVEMARKER SAVEMARKER`. Original first line was `SAVEMARKER SAVEMARKER SAVEMARKER`
  (the rest of the file was already SAVEMARKER test junk from earlier sessions). Fix = remove the leading space and `pasted-nam`.
- `~/notes/Untitled-4.md`: created by the `+` click (template only, 109 B); safe to delete.

---

## Pass 2 (same day): remaining areas tested, fixes applied (uncommitted)

Also verified working: Text to Table (prefills from clipboard, live Markdown preview, Esc), Health Checks
(runs, streams, Esc), Find Recent Files (opens, filters, Esc), Cmd+F in the nvim pane (vim `/` search),
Document-style menu (bottom-right icon), terminal drawer (Ctrl+J focus, typing, Enter), Ctrl+H -> sidebar focus ring.

### New findings
| id | sev | finding |
|---|---|---|
| U9 | S2 | Ctrl+Shift+H/J/K/L moved pane focus instead of resizing the window (`PrefixState::pane_dir` ignored Shift). |
| U10 | S4 | Terminal drawer: the top border overlaps the first text row. |
| U11 | S3 | Right-click in the terminal drawer shows no menu (rule 2 wants copy file / copy path / open in notes / reveal). No terminal menu exists in the Rust port. |
| U12 | S3 | `state.tools.*.shown` stayed `true` after Esc closed filefast / prettyprint / health (and `paths` kept `key:true`). |
| U13 | S4 | Dragging the drawer's top edge does nothing (no resize handle found); drawer height is fixed at `drawerInset` 200. |
| env | - | Window-resize shortcuts (Ctrl+Shift+HJKL, Cmd+Opt+±) cannot be checked live here: AeroSpace tiles the window and snaps it back. |

### Fixed
| id | fix |
|---|---|
| U1 | `views/notes.rs new_note_from`: `append()` already selects the tab, so `select_row()` saw "no change" and never loaded it. Now calls `open_note`. Verified live: `+` in Prose shows the new note and a paste lands in it. |
| U2 | `app/host.rs` palette key arm: n/p consumed only with Ctrl; Left/Right only on an empty query (they now move the caret). Verified live ("nonet pretty" types; Ctrl+N still navigates). |
| U3 | A click-pick left the palette window on screen under the tool (AppKit said `isVisible == false`, the window server still drew it). `PalettePanel::hide_panel` now also sets alpha 0 + ignoresMouseEvents (restored in `present`), and the click path hides it via a deferred `on_ui`. Verified live with `CGWindowList` + screenshots. |
| U5 (part) | Palette filter placeholder: "Search commands and windows". No-results row NOT added. |
| U6 | Prose body override now also sets `margin: 0 auto; padding: 28px 40px 56px; box-sizing: border-box`. Verified live on an unstyled note. |
| U9 | `pane_dir` requires `!shift`; regression test `ctrl_shift_hjkl_is_window_resize_not_a_pane_move`. Verified live: Ctrl+Shift+H no longer moves focus, Ctrl+H still does. |
| U12 | `ToolPanel::poll` resets `shown` when the window is no longer visible. Verified live (`prettyprint:false` after Esc). |

### Not fixed
U4 (badge column is the workspace pane by design), U5 no-results row, U7 (nvim gutter, rendered by the terminal), U8 (AppKit menu tracking), U10, U11 (needs a SwiftTerm-shim context menu), U13.

### Test residue to clean up
- `~/notes/hello.md` first line: ` Spasted-namAVEMARKER SAVEMARKER SAVEMARKER` (from the U1 repro; original `SAVEMARKER SAVEMARKER SAVEMARKER`).
- `~/notes/Untitled-4.md` and `samples/Untitled-1.md`: template-only notes created by clicking `+`.
- `samples/graphviz-showcase.md`: a stray paste was reverted (git diff is clean).
