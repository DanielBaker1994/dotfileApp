# PLAN-regression-hunt.md — find subtle feature regressions from the Swift → Rust port

Written 2026-10-13. Run by DeepSeek v4.1-flash agents only (read-only, static analysis). Claude does
not validate findings; it only merges the agents' output into `REGRESSIONS-swift-to-rust.md` for a
new session to act on.

## Reference points

- **Swift removed in `29268ae`** ("remove swift code entirely": 48 `.swift` files, ~47k lines).
- **Last full Swift tree: `209f34e`** (= `29268ae^`, Oct 10 2026), checked out read-only at
  `/Users/danielbaker/.config/kitchen-sink/.build/regress/swift` (`git worktree add --detach /Users/danielbaker/.config/kitchen-sink/.build/regress/swift 29268ae^`).
- **Rust port:** current `main`, `rust/ws-rs/src/**` (plus `bin/`, `pylib/`, `commands.toml`).

## What we are looking for

Not broad features ("there is a notes view"). **Subtle interaction behaviour** the Swift code
implemented that the Rust port lacks or does differently, for example:
- drag and drop: dragging images/files/rows in or out, drop targets, drag images, drop on a folder;
- multi-item selection: Cmd-click, Shift-click ranges, Shift+arrows, select all, marked rows surviving a
  reload or filter, acting on a multi-selection (copy, delete, open, drag);
- mouse details: double-click, right-click/context menus, hover states and tooltips, scroll-wheel
  behaviour, click-through, drag to resize or reorder;
- keyboard details: modifier variants of a key, repeat behaviour, Esc layering, Tab order, type-ahead;
- clipboard: copy/paste of images, file URLs, rich text, paste-to-create;
- persistence: remembered sizes, positions, selections, scroll offsets, sort order, last view;
- focus and window behaviour: which control gets focus after an action, activation rules;
- edge handling: empty states, long names, unicode, missing files, error messages shown to the user.

## Pipeline (per lane)

1. **Reader** (`--read-only`): reads the lane's Swift files in full, lists every subtle behaviour
   (with Swift `file:line`), then searches the Rust port for each and classifies it
   `present` | `missing` | `differs` with Rust `file:line` evidence (or the searches it ran when missing).
   Output: a JSON array in its final message.
2. **Validator** (`--read-only`, a separate agent, sees only the reader's claims): for each claim that
   is `missing` or `differs`, re-reads the cited Swift and independently searches the Rust tree, trying
   to **refute** it: `confirmed` | `refuted` (the Rust port does have it — cite where) | `partial` |
   `unclear`, with evidence.
3. **Merge:** a script joins reader + validator output per lane into `REGRESSIONS-swift-to-rust.md`.
   No Claude judgement is applied; disagreements are shown as disagreements.

## Lanes (Swift files at `209f34e`)

| Lane | Swift files | Area |
|---|---|---|
| A | PopupWindow.swift lines 1–5800 | the main popup/notes/files window (first half) |
| B | PopupWindow.swift lines 5800–end | same (second half) |
| C | kitchen_sink.swift lines 1–5100 | app core, routing, shared window behaviours |
| D | kitchen_sink.swift lines 5100–end, main.swift | app core (second half), startup |
| E | CompareWindow, CompareFolderView, ComparePane, CompareText, CompareFolder | compare |
| F | JiraDashboard, JiraSearch, JiraBoard, JiraTicket | jira |
| G | ScreenshotOverlay, Screenshot, ScreenshotAnnotations, ScreenshotPin, ScreenshotText, PaneShot | screenshot |
| H | NotesProse, VimKeys, VimSearch, NvimRPC, NoteFindWindow, TextEditKeys, InlineRename | notes editing |
| I | PathsWindow, PathShelf, FilePopup, FileOps, RecentFiles, ListFilter, SidebarJump, PopupInspector | files, paths, lists |
| J | SharedWindow, PaneNav, PaneGeometry, TerminalPanel, AnsiRender, ViewSwitcher, SwitcherStatus, ConfigText, CardWindow, SetupWindow, Confluence, AIWindow, AIFormat, PythonHelper, ProcessRun | window chrome, panes, terminal, misc views |

## Mechanics

- Briefs: `.build/regress/briefs/<lane>-{reader,validator}.txt` from templates in
  `.build/regress/templates/`. Runner: `.build/regress/run.sh` (readers in parallel, then validators in
  parallel), `node relay.mjs --read-only --model opencode-go/deepseek-v4.1-flash`.
- Never `deepseek-v4-pro`. Read-only agents cannot edit the tree.
- Merge: `.build/regress/merge.py` → `REGRESSIONS-swift-to-rust.md`.
