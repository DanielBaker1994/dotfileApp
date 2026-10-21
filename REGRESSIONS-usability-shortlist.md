# Swift → Rust usability regressions — shortlist

Condensed from `REGRESSIONS-swift-to-rust.md` (213 DeepSeek claims, unverified; removed 2026-10-20, see `git log -- REGRESSIONS-swift-to-rust.md`). Kept only things a
daily user would notice losing. Dropped: wording/toast text, tooltips, animations, cursors, internal
plumbing, socket/CLI details, multi-display edge cases, theme-menu polish, and anything that is
"differs" without a user-visible loss.

IDs refer to the big file (full Swift/Rust file:line refs live there).
**Spot-check warning:** I grepped a few claims and some are wrong or overstated — marked ⚠ below.
Verify each before implementing.

## Status (2026-10-13)

Fixed (verified with real input unless noted):
- A-01/I-03 drag files **into** the browser (folder row or list; ring highlight; Option=copy, Cmd=move, same-volume=move)
- A-12/A-13 New Folder / **New File** (menu + ⌘⇧N) now select the new item and start inline rename
- A-17/A-18/I-08 row right-click menu (Open, Open in Notes, Quick Look, Copy/Cut/Duplicate, Compare for 2 rows, Rename, Trash, New Folder/File, Paste, hidden toggle) and an empty-space menu
- A-06/I-28 click a selected row to rename in any folder (guarded so the default selection doesn't arm it)
- A-30 paging/Home/End clear stale multi-selection
- G-07/G-09 screenshot Save-as: stem preselected, ⌘V/⌃V/⌘A/⌘C/⌘X work (cargo-tested)
- C-24 closed note tabs stay closed across restarts (persisted dismissed set)

Checked, not a bug / not worth changing:
- G-01 Esc closes the overlay: deliberate (commented in code)
- A-09 cut-then-copy: the cut flag is cleared on copy; only a Finder-side copy after a cut could still move
- A-21/I-13 type-ahead: the list runs vim keys (NORMAL mode consumes letters; `i` / `/` focus the filter). A fall-through is added for non-vim mode, untested.
- D-17 external note edits: nvim has `autoread` + `checktime`
- D-16 deleted note: saving recreates the file at the same path, nothing lost unless the folder is gone

## Tier 1 — core file browsing (highest daily value)

| ID | Gap | Notes |
|---|---|---|
| A-01 / I-03 | Drag files **into** the browser (onto a folder row or the list) to move/copy; Option=copy, Cmd=move | Confirmed by a code comment: `views/files.rs:11` "Not yet ported: drag-and-drop". Real. |
| A-06 / I-28 | Slow double-click on a selected name to rename, in **any** folder (Rust: Recent only) | Rename still on F2/⌘R. |
| A-12 / A-13 | New Folder → inline rename prompt; **New File** (create empty file) | ⚠ `Action::NewFile` exists in `views/files.rs`; check whether it's reachable from UI/menu, not whether it exists. |
| A-17 / A-18 / I-08 | Right-click menus are thin: row menu lacks Open, Open With, Duplicate, Rename, Compare, New File, Cut/Paste; empty-space right-click does nothing (no New Folder/Paste) | Same root issue for the Paths shelf menu (I-09). One fix covers all. |
| A-21 / I-13 | Type-ahead: typing a letter with the list focused jumps into the filter | |
| A-20 | Tab-completes the path in the filter field | |
| I-10 / I-01 / I-02 | Shift+Arrow range select in the browser list; multi-select (Cmd/Shift-click, Shift+Arrow) in the Paths shelf | ⚠ browser list has `extend_selection` + shift-click (`files.rs:542`, `files_view.rs:289`). I-10 may only be the keyboard path. Shelf claim unchecked. |
| A-30 | PageUp/Down/Home/End don't clear multi-selection → later action hits stale rows | Small but can cause wrong-file actions. |
| A-09 / A-31 | After cut then copy, ⌘V may still *move* the old files | Correctness bug, not just wording. |
| A-16 | ⌘↑ from Recent/search should reveal the selected file's enclosing folder | |
| I-12 | ⌘⇧R "Reveal in Finder" opens the app's own browser instead | Decide: intended or regression? |
| I-17 | ⌘O/Return on image/text opens external app instead of in-app preview | |
| A-25 | Space pages the preview; copy preview feedback | |
| A-22 / A-26 | Drag-resize list/preview splitter and sidebar width | |

## Tier 2 — file-adjacent features

| ID | Gap |
|---|---|
| I-04 / D-15 / I-05 / I-06 | Paths shelf no longer auto-fed by: drag-out, clipboard copy ("copied"), filefast ("filefast") |
| I-07 | Up/Down in the shelf while Quick Look is open doesn't walk files |
| C-17 / C-19 | Filefast: ⌘V/A/C/X/Z and Shift+Return in the name field; bad input focuses the offending field |
| G-09 | Screenshot Save-as field: ⌘V/edit shortcuts don't work (violates `rule.md` rule 1) |
| G-07 | Save-as should select filename stem so typing replaces it |
| G-01 | Esc in capture overlay closes everything instead of just deselecting / disarming a tool first |
| G-08 | Annotation text editing is append/backspace only (no caret, selection, paste) |
| G-19 / G-20 | Layers list can't reach older layers; single-click on a text layer starts editing |
| G-12 / G-13 | Side-panel font-family picker, brightness slider |
| G-05 | Post-capture preview/confirmation window |

## Tier 3 — notes

| ID | Gap |
|---|---|
| D-17 | External edits to a note (or new files in the notes dir) are not picked up; editor keeps stale text |
| D-16 | Note deleted on disk → unsaved edit is lost instead of parked in default.md |
| C-24 | Dismissed notes come back after restart |
| D-18 / D-19 | "+" chooser (Open Existing / New) ; click selected tab to copy path |
| B-11 | Custom in-note find bar with match counter (Rust shows system bar) |
| B-28 | Persistent editor status line for save / commands.toml errors |
| H-04 / H-03 | Prose pop-out: no live reload; no close affordance / Esc |
| H-01 | Prose: single-click on image opens preview (should need double-click) |
| H-06 | Prose zoom not remembered; pinch unbounded |
| D-05 / D-04 | No UI to toggle vim/voice/sticky; no font family picker |

## Tier 4 — Compare (35 claims, mostly "mouse does nothing")

⚠ **Verify first.** `views/compare_view.rs` has `mouseDown:` and `scrollWheel:` handlers, so E-01 / E-09
("clicking does nothing", "wheel doesn't scroll") look wrong or at least partial. The rest are
unchecked. If they hold, the ones that matter most:

- E-19 session persistence + crash recovery (closing loses all open comparisons)
- E-25 discard prompt before replacing a dirty side (silent data loss)
- E-20 changed-on-disk watcher
- E-24 Save As for a pasted/unbacked side
- E-15 in-pane Find / go-to-line; E-16 ⌘C/⌘V on the diff
- E-03 gutter click copies section; E-17 single-line copy chord copies whole hunk (wrong behaviour)
- E-07 minimap; E-11/E-12 recent-list and path-field keyboard nav
- E-27–E-34 folder compare: mouse selection, double-click, disclosure triangle, drag in/out, type-to-filter, Space Quick Look
- E-35 Esc should unwind scan/selection/find before closing

## Tier 5 — Jira

Known-unported pieces (some are deliberate scope cuts — decide which you still want):

- F-01 / F-07 / F-08 live search panel (⌘F), run search, copy JQL/curl — currently a "not ported" toast
- F-05 / B-06 per-column filter + quick filter (chevron currently sorts)
- B-05 / D-14 / B-07 column drag-resize/reorder and persisted widths
- D-13 group-by with collapse
- D-12 "load N more…" pagination
- D-10 favorite star doesn't persist or reach Jira
- D-09 / F-13 / F-14 double-click opens browser instead of in-app ticket; comments not shown
- D-20 ⌘K picker can't favorite/blacklist releases or pin labels
- D-11 Esc doesn't step back out of pinned board / Jira view
- F-23 Force/Stop poll, rebuild cache; F-16–F-20 dashboard column/definitions editor
- F-10 / F-11 board bar buttons + kanban view

## Tier 6 — window / app level

| ID | Gap |
|---|---|
| J-04 | Shared window forgets size/position across launches — ⚠ `host.rs:9353` mirrors frame into `state.views.<view>.frame`; likely partially/wholly present. Test by resizing, quitting, relaunching. |
| C-05 | Hiding the window doesn't hand focus back to the previous app |
| C-08 | Windows stranded on a phantom workspace after monitor unplug / wake |
| C-09 | Shared views not pre-warmed → slow first switch |
| C-01–C-03 / D-06 | commands.toml `.bak` fallback / self-heal / "Config Issues" warning — a bad hand-edit silently misbehaves |
| D-08 | Bare launch pops Notes instead of staying hidden |
| D-07 / J-15 / J-16 | `kitchen-sink setup` / first-run setup window; Esc + keyboard in it |
| B-01 / C-10 / C-11 | Unread badges on workspace chips (need an Accessibility prompt, C-07) |
| B-03 | Header "copy N" button for marked rows |
| J-01 / J-02 / J-03 | Terminal: clear/zoom/hide shortcuts + right-click Copy/Paste; resize grip; hotkey should raise a visible-but-unfocused panel |
| B-08 / B-15 | ⌘⌥± and ⌘0 should scale content, not just window |
| J-07 / J-17 | AI rules folder auto-reload; open rule in default editor |
| J-09–J-14 | Confluence: contributor list, import saved pages, right-click row selection, auto load-more, preview debounce, trimmed context menu |

## Deliberately dropped (examples)

Toast/status wording (A-08, I-16, A-31 text part), tooltips/hints (A-23, G-11, F-28, I-15, I-27),
animations (G-18), cursors (G-10), header ellipsis (B-13), multi-display overlay key/help-card
placement (G-14, G-17), theme/transparency/header-style menu polish (C-14–C-16, B-10), socket length
limit (C-23), HTML board/ticket builders and web zoom (F-12, F-27), empty-state copy (E-22, F-29),
page-size config, Services menu (D-01) and status-menu live-refresh (D-02/D-03) — all low usability
impact or already covered by another entry.

## Suggested order

1. Tier 1 right-click menus + drag-in + rename-in-any-folder + type-ahead + cut/copy bug.
2. Compare data-loss items (E-25, E-19, E-20), after confirming the mouse claims.
3. Notes staleness/loss (D-16, D-17, C-24).
4. Screenshot Save-as edit shortcuts (G-09) and Esc layering (G-01).
5. Jira / Confluence only for the pieces you actually still use.
