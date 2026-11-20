# GAPS-shortcuts.md — our shortcuts vs polished Mac apps

**What this is:** a feature-level comparison of the keys in our Cmd+/ sheet
(`[shortcuts]` in `commands.toml`) with what users already know from Finder,
macOS text fields, Apple Notes / Bear / Obsidian, Raycast / Alfred, Linear /
Jira web and CleanShot X / Shottr. It compares only **what the user can do**,
not how those apps are built. Written 2026-11-19.

**Goal (owner):** the Files view should behave like Finder, with all the
same shortcuts "and then some". Every other view should follow the
conventions of the best-known app of its kind.

Priority: **P1** = muscle memory, you'll hit it daily, and its absence feels
broken. **P2** = clearly useful, and polished apps all have it. **P3** = nice
to have.

## Status (2026-11-19): what was built, what is left

**Built** (in the Cmd+/ sheet, unit-tested; 12 `sc-*` flows in
`Tests/app_flows.json` drive the Files keys with real input in the VM — all
pass, run vm-20261119-201859. Palette, Jira, screenshot, Settings and find-bar
keys are unit-tested only: the VM has no Jira data or Screen Recording grant,
and no socket verb opens the palette):

- §0 bug fixed: in the Files filter bar Cmd+A/C/X/V/Z edit the text;
  Ctrl+A is line-start everywhere; Cmd+K still copies the path from the bar.
- Files: Cmd+O, Cmd+Y, Cmd+Opt+Y, Cmd+F, Cmd+Shift+G, Cmd+Shift+Z (file
  redo, in `pylib/file_ops.py`), Cmd+Opt+A, Cmd+I (Get Info sheet),
  Cmd+Shift+H/D/O + Cmd+Opt+L, Cmd+Opt+C, Cmd+Ctrl+N, Cmd+Opt+Delete,
  Cmd+Shift+Delete, Cmd+Ctrl+T (pins the selected folder), Cmd+Ctrl+Opt+1/2/5/6/7
  sort, Cmd+Shift+P preview pane (remembered), Cmd+Opt+S sidebar, Cmd+E eject,
  "N of M selected" in the status line.
- Text: Cmd+F / Cmd+G / Cmd+Shift+G find bar in the AI text boxes and the
  Files text preview.
- Palette: Cmd+1…9, Cmd+↑ / Cmd+↓.
- Jira: Cmd+C, Cmd+Shift+C, Cmd+Return / Cmd+O, Cmd+R (kicks the poller),
  ticking rows (Ctrl+Space — listed before but never wired — Shift+↑↓, Cmd+A,
  Cmd+Opt+A) and Cmd+K acting on the ticked rows.
- Confluence: Cmd+O. Paths: Cmd+K.
- Screenshot: W = select the window under the pointer, Cmd+P = pin.
  (Save As, delay, pixelate and Recent already existed; the sheet now says so.)
- App: Settings… on Cmd+, (opens ws-settings); Cmd+0 listed.

**Already fine, no change:** typing in the Files list already jumps (it
starts the filter, which narrows to the match); rename selects the name
without the extension; Opt/Cmd drag modifiers; Opt/Cmd+arrows and
Opt+Delete in fields (the router passes them through); Cmd+M / Cmd+H (menu);
Cmd+Shift+V (our text boxes are plain text, so every paste is plain).

**Built in the second pass:** Files tabs (Cmd+T, Cmd+W closes a tab,
Cmd+Shift+[ / ], a tab bar with two or more tabs), Opt+→ / Opt+← expand /
collapse folders in place, Rename N Items (Cmd+R / F2 / right-click with
several selected: find & replace, add before / after, number; one undo
step), screenshot V = screen recording of the selection (`screencapture -v`;
Hyper+X stops it, the .mov path is copied and lands in Recent Files).
Also fixed on the way: Return in a form sheet's text field (Rename N Items,
Jira / Confluence setup, New Rule) now presses OK; it used to do nothing.

**Left, by size (not done):** Files column / gallery views (Cmd+1–4);
screenshot scrolling capture; Jira write actions
(assign to me, change status, comment: need Jira API write support);
Confluence Cmd+[ / ] page history; palette Cmd+K / Cmd+Return / Cmd+C (palette rows are commands and
workspaces, not files, so Raycast's file actions don't map).

---

## 0. Confirmed bug found while checking: edit keys in the Files filter bar act on files

Cmd+A in the Files filter / address bar selects every **file** instead of the
**text**. Cause: `app/host.rs:5139` sets `browser_has_focus` whenever the Files
view is showing (`current == Some(SlotView::Files)`), even when the cursor is in
the filter field. `ui/popup.rs` `file_browser_key` then catches Cmd+A/C/X/V/Z
before the text field ever sees them. So while you type in the filter bar:

| Key in the filter bar | Expected (Finder search field) | What happens |
|---|---|---|
| Cmd+A | select the typed text | selects all files |
| Cmd+C | copy the typed text | copies the selected **files** |
| Cmd+V | paste text into the field | pastes **files** into the folder |
| Cmd+X | cut the text | cuts files |
| Cmd+Z | undo typing | undoes the last file operation |

Fix: `browser_has_focus` must be false when the first responder is the
filter field's editor (rule.md §1). This applies to every P1 row in §2 too:
**while a text field has focus, text editing always wins.**

---

## 1. Files view vs Finder

### Already have (matches Finder)
Cmd+C / X / V, Cmd+Opt+V (move), Cmd+D, Cmd+Delete, Cmd+Z, Cmd+Shift+N,
Cmd+[ / ], Cmd+Up / Down, Space (Quick Look), Cmd+Shift+. (hidden files),
Shift+↑↓, Shift/Cmd+click, Cmd+A (in the list), Home/End/PgUp/PgDn, Cmd+W.

### Deliberately different (keep, but add the Finder key as well)
| Finder | Ours | Suggestion |
|---|---|---|
| Return = rename | Return = open | Keep ours; Cmd+R / F2 already rename. |
| Cmd+K = Connect to Server | Cmd+K = copy path | Keep, **and** add Finder's Cmd+Opt+C "Copy as Pathname". |
| Cmd+L = Make Alias | Cmd+L = focus filter | Keep. |
| Cmd+R = Show Original | Cmd+R = rename | Keep. |
| Tab = next file | Tab = complete path | Keep (only applies while typing a path). |

### Gaps
| P | Finder key | What it does | Ours today |
|---|---|---|---|
| P1 | Cmd+O | open the selection | missing (Return / Cmd+Down only; Paths has Cmd+O) |
| P1 | Cmd+Y | Quick Look | missing in Files (Paths has it) |
| P1 | Cmd+F | search | not listed for Files (Cmd+L does it); make Cmd+F focus the filter |
| P1 | Cmd+Shift+G | Go to Folder (type a path) | missing; alias to the address bar with the text selected |
| P1 | Cmd+Shift+Z | redo | missing (undo only) |
| P1 | Cmd+Opt+A | deselect all | missing |
| P1 | Cmd+I | Get Info: size, created/modified dates, kind, permissions, folder item count | missing |
| P1 | Cmd+Shift+H / D / O / Cmd+Opt+L | go to Home / Desktop / Documents / Downloads | missing |
| P1 | typing letters in the list | jump to the first file starting with them | sidebar has it, list doesn't (typing goes to the filter) |
| P2 | Cmd+Opt+C | copy as pathname | Cmd+K does it; add the Finder key too |
| P2 | Cmd+Ctrl+N | new folder containing the selection | missing |
| P2 | Cmd+Opt+Delete | delete immediately (with confirmation) | missing |
| P2 | Cmd+Shift+Delete | empty the Trash | missing |
| P2 | Cmd+T / Cmd+Shift+[ ] | tabs: new tab, previous / next tab | missing |
| P2 | Cmd+Ctrl+T | add folder to sidebar favourites | missing (no way to pin a folder) |
| P2 | Cmd+Ctrl+Opt+1…7 | sort by name / kind / date / size… | no sort keys (check whether a sort exists at all) |
| P2 | Opt+→ / Opt+← (list view) | expand / collapse a folder in place | missing (flat list) |
| P2 | Cmd+Opt+P / Cmd+/ | show path bar / status bar ("12 items, 3 selected, 40 GB free") | no item-count or selection-count readout |
| P2 | Cmd+Shift+P | show / hide the preview pane | missing (Jira has Cmd+I for this) |
| P2 | Cmd+Opt+S | show / hide the sidebar | ours is Cmd+\; add Finder's key as well |
| P2 | rename several files | Finder's "Rename N Items…" (replace / add text / number) | missing |
| P2 | while renaming: only the name selected, not the extension | Finder default | check |
| P2 | Opt+drag = copy, Cmd+drag = move, Cmd+Opt+drag = alias | drag modifiers | check |
| P3 | Cmd+1 / 2 / 3 / 4 | icon / list / column / gallery view | list only |
| P3 | Cmd+Opt+Y | full-screen Quick Look | missing |
| P3 | Cmd+E | eject the selected volume | missing |
| P3 | right-click → Open With, Compress, Tags | context-menu actions | check what the menu has |

---

## 2. Every text field vs standard macOS text editing

This covers the filter / address bars, rename, sheet prompts, the palette
input, Jira search, the Confluence search and the AI text. Apple Notes, Bear
and Obsidian all honour these, because they come free with macOS text views.
Our key router can swallow them, as §0 shows. Mark each one as working or
broken per field.

| P | Key | Expected |
|---|---|---|
| P1 | Cmd+A / C / X / V / Z / Shift+Z | select all / copy / cut / paste / undo / redo **the text** |
| P1 | Opt+← / → | jump a word |
| P1 | Cmd+← / → | start / end of the line |
| P1 | Shift + any of the moves above | extend the selection |
| P1 | Opt+Delete | delete the word before the cursor (we have Ctrl+W, the terminal habit) |
| P1 | Cmd+Delete | delete to the start of the line (**conflict:** in Files it's Trash; must not fire while typing) |
| P1 | Ctrl+A / E / K | start of line / end of line / delete to end (emacs keys macOS gives every field) |
| P1 | Esc | cancel / clear (already the convention) |
| P2 | Cmd+Up / Down | start / end of the whole text (AI text) |
| P2 | Fn+Delete | delete forward |
| P2 | Cmd+Shift+V / Cmd+Opt+Shift+V | paste as plain text (Notes, Bear, Obsidian) — AI text |
| P2 | Cmd+F / Cmd+G / Cmd+Shift+G | find in the text, next / previous (AI text) |
| P2 | Cmd+Ctrl+Space | emoji & symbols picker |
| P3 | Opt+↑ / ↓ | move the line up / down (Obsidian, VS Code) — AI text |
| P3 | Cmd+Shift+K or Cmd+Shift+Delete | delete the line (Obsidian) — AI text |

---

## 3. Palette (Hyper+S) vs Raycast / Alfred / Spotlight

Have: ↑↓, Ctrl+N/P, Tab, Return, Esc (clear, then close).

| P | Raycast / Alfred key | What it does | Ours |
|---|---|---|---|
| P1 | Cmd+K | actions on the highlighted item (open with, reveal, copy path, copy file) | missing |
| P1 | Cmd+Return | the item's second action (Raycast: reveal in Finder / open in…) | missing |
| P1 | Cmd+C | copy the highlighted item (file / path / name) | missing |
| P2 | Cmd+1…9 | run the Nth result | missing |
| P2 | Cmd+↑ / Cmd+↓ | first / last result | missing |
| P2 | Cmd+Y / Space-to-preview | Quick Look the highlighted file | missing |
| P2 | Cmd+, | open settings | missing (Hyper+/ only) |
| P3 | Cmd+Delete (on a recent) | remove it from Recents | missing (Paths has it) |

---

## 4. Jira vs Jira web / Linear

Have: Cmd+F, Cmd+L, Cmd+I, Cmd+K actions menu, Return, j/k, Ctrl+Space tick,
1–9 in the menu.

| P | Their key | What it does | Ours |
|---|---|---|---|
| P1 | Cmd+C (Linear Cmd+.) | copy the issue key | inside Cmd+K only; make it direct |
| P1 | Cmd+Shift+C (Linear Cmd+Shift+,) | copy the issue URL | inside Cmd+K only |
| P1 | Cmd+Return / o | open in the browser | inside Cmd+K only (Confluence uses Cmd+Return; match it) |
| P1 | Cmd+R | refresh | missing (Confluence has Cmd+R) |
| P2 | i (Jira) / a (Linear) | assign to me / assign | missing |
| P2 | s (Linear) / . (Jira) | change status / transition | missing |
| P2 | m (Jira) / c (Linear) | comment / create an issue | missing |
| P2 | x / Shift+click / Shift+↑↓ / Cmd+A | multi-select rows | Ctrl+Space only |
| P2 | Cmd+[ / Cmd+] | back / forward between views | Esc only |
| P3 | g then i / b / m | go to inbox / board / my issues | Space s f does this |

---

## 5. Confluence vs a browser / Confluence web

Close to complete. Gaps:

| P | Key | What it does | Ours |
|---|---|---|---|
| P1 | Cmd+F (page open) | find **in the page** (browser habit) | Cmd+F focuses the search box; Cmd+G already steps matches |
| P2 | Cmd+[ / Cmd+] | back / forward through opened pages | missing |
| P2 | Cmd+Shift+C → Cmd+K | copy title + link: matches Jira's Cmd+K menu | inconsistent with Jira |

---

## 6. Screenshot (Hyper+X) vs CleanShot X / Shottr / macOS Cmd+Shift+4

Keys are strong (tools, undo/redo, copy, save, OCR). Gaps are mostly features:

| P | Their feature / key | Ours |
|---|---|---|
| P1 | Space (macOS) = capture a single window | missing (our Space = side panel) |
| P2 | Pin the shot on screen as a floating window (CleanShot, Shottr) | missing |
| P2 | Cmd+Shift+S = Save As… (choose folder / name) | missing (Cmd+S saves to the default) |
| P2 | Self-timer / delay capture | missing |
| P2 | Drag the finished shot straight into another app (CleanShot) | check |
| P2 | Blur (not just pixelate), crop after capture | check |
| P3 | Scrolling capture, screen recording | missing |

---

## 7. App-wide vs every Mac app

| P | Key | Expected | Ours |
|---|---|---|---|
| P1 | Cmd+, | Settings | missing (Hyper+/ opens ws-settings) |
| P1 | Cmd+F | find in the current view | Files and Paths don't list it |
| P2 | Cmd+0 | actual size | **exists in code** (`ResizeReset`) but missing from the Cmd+/ sheet |
| P2 | Cmd+M / Cmd+H | minimise / hide | check |
| P2 | Help-menu search (Cmd+Shift+/) | find any menu command | the palette covers this; check |

---

## Consistency problems inside our own app

The same action uses different keys in different views. Pick one key and add
it everywhere:

| Action | Files | Paths | Jira | Confluence |
|---|---|---|---|---|
| Open | Return / Cmd+Down | Return / **Cmd+O** | Return | Return / **Cmd+Return** |
| Copy path / link | **Cmd+K** | **Cmd+C** | Cmd+K menu | **Cmd+C** |
| Quick Look | Space | Space / **Cmd+Y** | — | — |
| Rename | Cmd+R / F2 | Cmd+R | — | — |
| Refresh | — | — | — | Cmd+R |
| Find | Cmd+L | — | Cmd+F / Cmd+L | Cmd+F / Cmd+L |
| Copy title + link | — | — | Cmd+K menu | Cmd+Shift+C |
