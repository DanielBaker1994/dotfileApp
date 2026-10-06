# PRD — Compare: Beyond Compare-style text + folder compare inside workspace-switcher

Status: phases 0–2 built (Text + Folder Compare), phase 3 open · Written: 2026-10-03 · Reviewed: 2026-10-03 · Target: workspace-switcher (Swift/AppKit, macOS ≥ 26.7, arm64)

> **For the implementing agent.** Read `AGENT_CONTEXT.md` (= `CLAUDE.md`) and
> `rule.md` first. They are binding. This PRD says *what* to build and which
> parts of the codebase it plugs into. Section 7 is the spec, section 9 the
> checks that decide whether you are done. Do not read the big Swift files
> whole: grep the symbols named here and read about 60 lines around each.

> **Review, 2026-10-03.** Phase 1 shipped without Folder Compare, and pasted
> text (§7.1 "Clipboard") was only reachable by pressing Compare on an empty
> start page and then pasting into each pane. Done in this pass: the start
> page has a **Compare Pasted Text…** button (clipboard = left side, focus the
> right); ⌘V works in any pane (an empty side takes it, a side with text is
> replaced by an undoable edit); a pair with pasted text becomes a Recent row
> ("pasted text ⇆ …", snapshots in `~/.cache/workspace-switcher/compare-pasted/`);
> Folder Compare (phase 2) is built — see "Phase 2 as built" in §8. Still open:
> the phase 3 list, plus the gaps named there.

---

## 1. Summary

Add a **Compare** view to the shared window. It does the two Beyond Compare
jobs the owner actually uses: **Text Compare** (two files or two pasted texts,
side by side, lined up, differences colored, copy a change across, edit, save)
and **Folder Compare** (two folder trees side by side, which files differ, are
newer, or exist on one side only; open a pair in Text Compare; copy files
across). The view is shown, hidden and closed exactly like the other views
(files, notes, AI, jira, confluence). Hex, media, picture and table compare
are out of scope.

## 2. Contacts

| Name | Role | Comment |
|---|---|---|
| Daniel Baker | Owner, only user, reviewer | Decides scope, approves the look. Has Beyond Compare 5.2.5 (build 32528) installed at `/Applications/Beyond Compare.app`, on a trial key. |
| Implementing agent | Builds the feature | Follows `AGENT_CONTEXT.md` and `rule.md`. Does not commit unless asked. |
| PRD author (Claude) | Research + spec | Read the Beyond Compare 5 help (Text / Folder Compare command lists, importance and comparison settings, see §10) and this repo's shared-window code. |

## 3. Background

**What this is.** workspace-switcher is the owner's menu-bar app. One shared
window holds the files, notes, AI, Jira and Confluence views. You switch views
with the header icons or Ctrl+Tab and show or hide the window with Hyper+N.
Every view hides the same way (✕, Cmd+W, Hyper+N, focus loss, and Esc where
the view's "Esc Hides Window" switch is on).

**The problem.** Comparing things is a daily job: a config before and after,
two log dumps, an AI answer against the draft, a Jira description against a
pasted copy, a backup folder against the working folder, the output of
`git difftool`. Today that means opening Beyond Compare, a separate app that:

- costs money (the copy on this Mac is a **trial**: `trial-arm64.key`, a nag
  dialog in `BCState.xml`);
- doesn't follow the app's theme, `commands.toml`, the AeroSpace layout, the
  `/paths` shelf or the file browser's right-click menu;
- opens as its own window with its own close rules. The owner wants compare to
  disappear and come back the way every other view does.

**Why now.**

- **The building blocks are already here.** The shared window takes new views
  through `SlotMember` / `SlotView`. The Jira detail view gives a back stack
  where Esc means back. The file browser already has `FileListPane` (rows,
  multi-select, drag, right-click), `FileOps` (copy / move / trash with undo),
  `InlineRename`, Quick Look and `IgnoreRules` (gitignore matching). The AI
  view already draws word-level diffs (`WordDiff` in `AIWindow.swift`).
  The tested `FileOps.swift` / `PathShelf.swift` pattern (Foundation only,
  `bin/run-tests.sh`) suits a diff engine.
- **Swift's standard library ships a Myers diff** (`CollectionDifference`),
  so a correct line diff is a small amount of code.
  Better alignment (histogram / patience) is an add-on, not a research
  project.

## 4. Objective

**Objective.** Replace Beyond Compare for everyday text and folder compares,
inside the app, keyboard-first, themed, and dismissed like every other view.

**Why it matters.** One less paid, foreign app. Compares start from where the
owner already is: the file browser, `/paths`, the palette, the terminal and
git. They also look and close like the rest of the app.

**Key results (SMART).**

| # | Key result | Target | Measured by |
|---|---|---|---|
| KR1 | Beyond Compare not opened for text or folder compares | 0 launches in the 4 weeks after the full release (§8 phase 3) | owner; `BCState.xml` mtime unchanged |
| KR2 | Text Compare is fast | two 10k-line files: palette Return → painted ≤ 150 ms; 100k lines ≤ 1 s; an edit re-compared ≤ 50 ms (10k lines) | `/tmp/ws-debug.log` `compare text: N lines, diff M ms, paint P ms` |
| KR3 | Folder Compare is fast | 20k files per side: first rows ≤ 200 ms; quick test (size + time) complete ≤ 2 s; UI never blocks > 16 ms | log `compare folder: N items, scan M ms` |
| KR4 | Diffs are right | on a corpus of ≥ 50 file pairs, hunk count and ranges match `git diff --no-index --histogram` for ≥ 95 % of pairs; **never** "identical" for files that differ | `bin/run-tests.sh compare` |
| KR5 | No data loss | every copy / move / delete / sync undoable with Cmd+Z; deletes go to the Trash; unsaved edits survive hide, focus loss and view switches | tests + `do:` hooks |
| KR6 | Dismissal conforms | the compare view passes the same show / hide / focus checks as the other views | `bin/ui-test-focus.py compare` |

## 5. Market segment

Built for one person: the owner, a developer on macOS with AeroSpace and a
keyboard-first workflow. Jobs to be done (problems, not demographics):

1. **"What changed between these two files?"** Config, logs, generated
   output, two copies of the same script.
2. **"What changed between these two texts?"** Pasted from Jira, Webex, an
   AI answer or a terminal. There's no file to point at.
3. **"Are these two folders the same, and if not, where do they differ?"**
   A backup against the working folder, a deploy against a build output, two
   checkouts.
4. **"Make the other side match."** Copy one change across (a line, a
   section, a file, a folder) and save, without a second tool.
5. **"Show me this git diff properly."** `git difftool` / `git difftool
   --dir-diff` open the same view.

**Constraints.** macOS ≥ 26.7, arm64, accessory app (no Dock icon, no Edit
menu: rule 1 applies), no paid or network dependencies, AeroSpace decides
float vs tile, one daemon, config in `commands.toml` (rule 3).

## 6. Value propositions

**Gains.** Compare from anywhere in the app in one or two keys. It uses the app's
theme and fonts, and the same edit keys as every other text input. Recent
comparisons are one Return away.

**Pains avoided.** No paid trial, no second app to find in the window cycle,
no separate close and hotkey rules, no "which window was that in".

**Value curve** (● strong · ◐ partial · ○ none):

| Factor | Beyond Compare 5 | FileMerge (`opendiff`, ships with Xcode) | VS Code diff | **Compare view** |
|---|---|---|---|---|
| Cost | ◐ paid | ● free | ● free | ● free |
| Text compare quality (alignment, char-level, importance) | ● | ◐ | ● | ● (v1: no grammar rules) |
| Edit + copy changes across | ● | ◐ (merge only) | ◐ | ● |
| Folder compare + copy across | ● | ◐ | ○ | ● |
| Folder sync (update / mirror) | ● | ○ | ○ | ◐ (phase 3, with preview) |
| Hex / media / picture / table / 3-way merge | ● | ○ | ○ | ○ (out of scope) |
| Opens from the file browser / `/paths` / palette | ○ | ○ | ○ | ● |
| Theme, `commands.toml`, AeroSpace, dismissal like the rest | ○ | ○ | ○ | ● |
| Start-up (window up) | ◐ (own app, cold launch; not measured) | ◐ | ○ | ● (prewarmed view) |

We win where it matters to the owner: it lives where the owner works. We knowingly
give up BC's breadth (formats, remotes, archives, scripting, merge).

## 7. Solution

### 7.1 UX and user flows

#### Entry points (no new global hotkey)

AGENT_CONTEXT keeps global hotkeys to Hyper+N / S / X / /. Compare is
reached like the other views:

| From | How | Opens |
|---|---|---|
| Header | the Compare nav icon (after confluence), Ctrl+Tab | last session, or the start page |
| Palette (Hyper+S) | `/compare` | start page (Return on a recent pair opens it) |
| File browser + `/paths` | right-click "Select for Compare" on one row, then "Compare to 'NAME'" on another; with exactly 2 rows marked, "Compare" | Text or Folder Compare (by row type) |
| Drag & drop | drop a file / folder on the left or right path field or pane | that side |
| CLI | `workspace-switcher compare [--wait] [--title1 T] [--title2 T] LEFT [RIGHT]` | Text or Folder Compare |
| git | `git difftool` / `git difftool --dir-diff` (config snippet in §7.4) | same, with `--wait` |
| Clipboard | an empty pane takes Cmd+V / Ctrl+V; right-click "Paste Clipboard Here" | Text Compare of pasted text |

#### Main flow: text

1. Open a pair (any entry point). The view shows both files side by side,
   lined up, scrolled to the first difference.
2. Ctrl+N / Ctrl+P jump between differences. The thumbnail on the left
   shows where they all are.
3. Opt+→ / Opt+← (or the gutter arrows; Ctrl+R = right too) copy the current
   section to the right / left. Or type in either pane.
4. Cmd+S saves the focused side. A dot on the session pill marks unsaved edits.
5. Hide the window (Cmd+W, ✕, Hyper+N, focus loss). Nothing is lost. Hyper+N
   brings it back exactly as it was.

#### Main flow: folder

1. Open two folders. Rows stream in as the scan runs. Differences are
   colored and folders with differences inside are marked.
2. The filter bar narrows it: **All · Differences · Same · Orphans · Left
   newer · Right newer**.
3. Return on a file pair opens Text Compare **inside the view** (a back
   step). Esc or the header "back" returns to the folder, with the cursor
   on the same row.
4. Copy files or folders to the other side (Opt+→ / Opt+←, right-click, or
   drag across). Cmd+Z undoes.

#### Wireframe A: start page (empty session)

```
┌ ✕ ☰ [files][notes][AI][jira][conf][⇆] ────────────────────────────────┐
│ ( + )                                                                  │
├────────────────────────────────────────────────────────────────────────┤
│  Left   [ ~/work/app/config.toml                                ] 📂   │
│  Right  [ ~/backup/app/config.toml                              ] 📂   │
│                               [ Compare ⏎ ]                            │
│                                                                        │
│  RECENT                                         filter: [          ]   │
│  ▸ config.toml        ~/work/app  ⇆  ~/backup/app        2 h ago       │
│  ▸ src/               ~/a/src     ⇆  ~/b/src             yesterday     │
│  ▸ (clipboard)        pasted text                        Mon           │
└────────────────────────────────────────────────────────────────────────┘
```
Path fields complete with Tab (the file browser's completion). Files on both
sides → Text Compare, folders on both sides → Folder Compare (a folder and a
file, or one folder alone, is refused with a hint). Only one file filled → the
other pane is empty and takes a paste or a drop. The **Compare Pasted Text…**
button next to Compare opens a text session with the clipboard as the left
side; ⌘V pastes the other side.

#### Wireframe B: Text Compare

```
┌ ✕ ☰ [icons…][⇆] ─────────────────────────────────────────────── back ┐
│ ( config.toml ⇆ config.toml ●  ✕ ) ( src/ ⇆ src/ ✕ ) ( + )             │
├────────────────────────────────────────────────────────────────────────┤
│ [All|Diffs|Same|Context]  ≠ 4 sections   ↑↓ ⇄swap  ⟳  ⚙ importance    │
│ ~/work/app/config.toml   ✎      │ │ ~/backup/app/config.toml          │
├──┬─────────────────────────────┬─┼─┬──────────────────────────────────┤
│▮ │ 12  name = "app"            │ │ │ 12  name = "app"                 │
│▮ │ 13  port = 8080             │→│←│ 13  port = 9090                  │  ← red bg, "8080"/"9090" char-marked
│▬ │ 14  debug = true            │→│ │ ░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░ │  ← filler: line only on the left
│  │ 15  host  = "x"             │ │ │ 14  host = "x"                   │  ← blue: whitespace only (unimportant)
├──┴─────────────────────────────┴─┴─┴──────────────────────────────────┤
│ L  13  port = 8080                                                     │  line details: current line,
│ R  13  port = 9090                                                     │  left over right, char diffs
├────────────────────────────────────────────────────────────────────────┤
│ 3 important · 1 unimportant · line 13:9 · UTF-8 · LF   ‖  LF · edited  │
└────────────────────────────────────────────────────────────────────────┘
```
- Far left: the **thumbnail**. Each line is a colored pixel row. A rectangle
  shows the visible part and a click jumps there.
- Center gutter: **→ / ←** on each difference section copy it across. Hidden
  until hover / cursor in the section (`[compare] gutter-arrows = hover`).
- Filler rows (hatched) keep both sides lined up.

#### Wireframe C: Folder Compare

```
┌ ✕ ☰ [icons…][⇆] ───────────────────────────────────────────────────── ┐
│ ( src/ ⇆ src/  ✕ ) ( + )                                               │
├────────────────────────────────────────────────────────────────────────┤
│ [All|Diffs|Same|Orphans|L newer|R newer]  ☐ flatten   filter: [*.swift] │
│ ~/a/src                    ⟳  ⇄  ↑                ~/b/src               │
├───────────────────────────┬──────┬──────────┬─┬──────────────────────────┤
│ Name                      │ Size │ Modified │ │ Name           Size  Modified│
│ ▾ 📁 lib            (red) │      │          │≠│ ▾ 📁 lib              …      │
│     util.swift      (red) │ 4 KB │ 10-02    │>│     util.swift  4 KB  09-30  │  left newer
│     old.swift    (purple) │ 1 KB │ 09-01    │ │                              │  left orphan
│     api.swift     (black) │ 8 KB │ 09-20    │=│     api.swift   8 KB  09-20  │
│     log.swift      (blue) │ 2 KB │ 10-01    │≈│     log.swift   2 KB  09-28  │  differs only in unimportant ways
│                           │      │          │ │     new.swift (purple) …     │  right orphan
├───────────────────────────┴──────┴──────────┴─┴──────────────────────────┤
│ 2 differ · 1 left only · 1 right only · 41 same · scanning content 63 %  │
└────────────────────────────────────────────────────────────────────────┘
```

Colors follow Beyond Compare's legend, drawn with theme tokens (§7.3.5):
**text** = same, **danger (red)** = different (newer side), **dim (gray)** =
the older side of a different pair, **accent2 (purple)** = orphan (one side
only), **info (blue)** = differs only in unimportant ways. A folder shows red when
something inside it differs.

#### Dismissal (the owner's explicit requirement)

The Compare view is a **shared-window view** (`SlotView.compare`, plus
`.compareText` for a Text Compare opened from a folder, the way Jira detail
sits on Jira). It is **not** a tool panel. It is a long-lived, editable
workspace that belongs in the shared frame and AeroSpace's layout, like notes.
It reuses the shared window's code for every way of closing:

| Action | Behavior | Code path (existing) |
|---|---|---|
| ✕ in the header, Cmd+W | hide the window; the view is parked, sessions + unsaved edits kept | `SharedWindow.hide` → `park()` |
| Hyper+N while in it | hide; Hyper+N again → back on Compare (it's `last`) | `SharedWindow.toggle` |
| Hyper+N from elsewhere | focus / show on the current workspace's monitor | `hotkeyPrep`, `targetScreen` |
| Focus loss | hide after `[app] focus-loss-delay` when Hide When Focus Is Lost is on | `checkFocusLoss`, `hostHandlesFocusLoss = true` |
| Ctrl+Tab / Ctrl+Shift+Tab | next / previous view (Compare's icon is in the cycle) | `SharedWindow.cycle`, `onCycleView` |
| Esc | chain, first match wins: (1) a popover / picker / find bar / path field / inline rename / section editor closes (rule 6, `transientEscape`); (2) a running scan or diff stops; (3) in `.compareText` opened from a folder → **back** to the folder, same row (`slot.back(esc: true)`); (4) at the top: hide **only** if the kitchen sink's "Esc Hides Window" is on (`[compare] esc-close`, default `[app] esc-close` = 0 = never) | `escapeAtTop`, add `.compare` to `SharedWindow.escViews` |
| Session pill ✕ / right-click Close | closes that session (not the window); unsaved → a sheet (Save · Discard · Cancel) via `jiraFormSheet`-style sheet, **never** an app-modal NSAlert | `PopupTabsBar` closable pill |
| Last session closed | the start page, window stays up | — |
| Daemon restart / quit | sessions (paths, filters, scroll spot) restored from `compare-sessions.json`; unsaved text kept as recovery copies (§7.3.4) | `restartDaemon` |

Cmd+W never closes a session. It hides, as it does in every view. That
matches the `[shortcuts]` row `"all: Cmd+W" = "hide the window"`.

### 7.2 Key features

#### 7.2.1 Feature table (scope)

Beyond Compare 5 command names in the left column, so scope can be checked
against BC's own menus (§10).

| BC command / feature | v1 | Notes |
|---|---|---|
| **Text Compare: view** | | |
| Side-by-side layout, synced scroll, aligned filler lines | ✅ | |
| Line-level + character-level difference marks | ✅ | char-level = `WordDiff`-style tokens within a changed line pair |
| Important (red) vs unimportant (blue) differences | ✅ | importance rules below |
| Show All / Differences / Same / Context (N lines) | ✅ | `[compare] context-lines = 3`; Cmd+1…4 |
| Ignore Unimportant Differences | ✅ | toggle; unimportant then counts as same |
| Thumbnail (overview bar) | ✅ | click / drag to scroll |
| Line details pane (current line, left over right) | ✅ | toggle; read-only in v1 |
| Line numbers, word wrap, visible whitespace | ✅ | toggles in the kitchen sink |
| Syntax highlighting | ⏭ | later; not needed to spot differences |
| Over-under layout, webpages, hex / alignment details | ❌ | |
| **Text Compare: edit** | | |
| Copy to Right / Left (section or selected lines) | ✅ | Opt+→ / Opt+← (Ctrl+R) + gutter arrows |
| Copy Line to Right / Left | ✅ | Ctrl+Opt+R / Ctrl+Opt+L |
| Edit either side, Undo / Redo | ✅ | rule 1 edit keys; one undo stack per side |
| Save / Save As (per side), Save both | ✅ | keeps encoding, BOM, line endings, permissions |
| Swap Sides, Reload, Recompare | ✅ | |
| Open File into a side, Open Clipboard into a side | ✅ | Cmd+O; paste into an empty pane |
| Find / Find next / prev, Go To line | ✅ | Cmd+F, Cmd+G, Cmd+Shift+G, Ctrl+G |
| Next / Previous Difference (section) | ✅ | Ctrl+N / Ctrl+P (BC's own Windows keys) |
| Align With / Isolate | ⏭ | manual alignment fix-ups, later |
| Convert: trim trailing whitespace, line endings | ⏭ | |
| Bookmarks, replacements, text reports, sessions by name | ❌ | the recent list replaces named sessions |
| **Text Compare: importance (settings)** | | |
| Leading / embedded / trailing whitespace unimportant | ✅ | each a switch; default: leading + trailing on, embedded off |
| Character case unimportant | ✅ | default off |
| Line endings (CRLF vs LF) unimportant | ✅ | default on (= BC default) |
| Blank-line-only differences unimportant | ✅ | default off |
| Grammar elements (comments, strings per language) | ⏭ | needs per-language rules |
| **Folder Compare: view** | | |
| Two trees side by side, aligned by name | ✅ | case per volume (APFS default: insensitive) |
| Status colors + center glyph (= ≠ ≈ < > orphan) | ✅ | legend in Cmd+/ card |
| Show All / Differences / Same / Orphans / Left newer / Right newer | ✅ | Cmd+1…6 |
| Expand / Collapse, Expand All / Collapse All | ✅ | → / ←, Shift+→ recursive |
| Ignore Folder Structure (flatten) | ✅ | |
| Name filter (include / exclude globs) + gitignore rules | ✅ | reuses `IgnoreRules`; `[compare] ignore-file`, `use-gitignore` |
| Hidden files toggle | ✅ | Cmd+Shift+. (file browser key) |
| Back / Forward, Up One Level (both), Swap Sides, Refresh | ✅ | Cmd+[ / ], Cmd+Up, Cmd+Opt+X, F5 |
| Columns: name, size, modified (per side) | ✅ | |
| **Folder Compare: comparison criteria** | | |
| Quick test: size + timestamp (± tolerance) | ✅ | `[compare] time-tolerance = 2` s (FAT / network copies) |
| Content compare (byte-for-byte), on demand or auto | ✅ | `[compare] content = auto` (same size + different time) `| always | never`; background, early exit |
| Rules-based (text files equal after importance rules → blue) | ✅ | same normalizer as Text Compare |
| CRC, attributes / permissions / owner | ⏭ | |
| **Folder Compare: actions** | | |
| Open (pair → Text Compare in-view) | ✅ | Return / double-click |
| Copy to Right / Left / Other Side | ✅ | `FileOps.transfer`, undoable; clash = sheet (Replace · Keep both · Skip, "apply to all") |
| Move to Other Side | ✅ | Cmd+Opt+R / L |
| Delete (to the Trash), Rename, New Folder | ✅ | `FileOps.trash`, `InlineRename`, file browser keys |
| Quick Look, Reveal in Finder, Copy Path, Open in Notes, Open in Default App | ✅ | rule 2 right-click menu |
| Touch, Attributes, Exchange, Copy / Move to Folder | ⏭ | |
| Synchronize ▸ Update Right / Left / Both, Mirror to Right / Left | phase 3 | always through a preview sheet (counts + list); mirror deletes = Trash |
| **Out of scope** | ❌ | Hex, Media / MP3, Picture, Table / Data, Registry, Version compare; Text / Folder **Merge** (3-way); remote folders (FTP / SFTP / cloud); archives as folders; snapshots; reports; scripting; source control commands |

#### 7.2.2 Diff engine behavior

- **Lines first, then characters.** (1) Normalize each line for comparison
  per the importance rules (the shown text never changes). (2) Line diff:
  anchor on lines unique to both sides (patience / histogram), Myers
  (`CollectionDifference`) between anchors. (3) Pair up changed lines inside a section
  (a "changed" line = similar enough; else delete + insert). (4) Character
  diff on each pair: tokens = words / whitespace runs / single punctuation
  (the `WordDiff.tokens` rule).
- **Important vs unimportant.** A line pair that's equal after normalizing
  but differs raw = unimportant (blue). Any other change = important (red).
- **Sections.** A run of adjacent changed rows = one section: the unit for
  Ctrl+N / P, the gutter arrows and Opt+→ / Opt+←.
- **Live.** Edits re-diff only the touched window (the section ± 50
  unchanged lines). A full re-diff runs off main. A generation counter drops
  stale results (the `runGen` pattern from the AI view).
- **Input.** UTF-8 (with or without BOM), UTF-16 LE / BE (BOM), else
  Latin-1. Each side keeps its own encoding + line endings on save. A file
  with NUL bytes in the first 8 KB = binary: shown as "Binary files are
  identical / differ (size, first difference at byte N)". No hex view.
- **Limits.** Up to 200k lines or 50 MB per side. Past that: a status line
  and a quick byte compare only.
- **Changed on disk.** A `DispatchSource` per open file. Not edited here →
  reload + re-diff silently. Edited here → banner "Changed on disk:
  Reload · Keep mine".

#### 7.2.3 Folder engine behavior

- **Scan** both trees off main with `FileManager.enumerator`, prefetching
  size / mtime / isDirectory / isSymbolicLink. Rows stream to the UI every
  100 ms. Ignore rules apply during the scan, so an ignored folder is
  never walked (`.git/`, `node_modules/` by default via `[compare] exclude`).
- **Pairing** by relative path. Name case follows the volume
  (`volumeSupportsCaseSensitiveNames`). Unicode NFC / NFD treated as equal
  (BC's "filename alignment").
- **Status** per pair: `same`, `different` (+ which side is newer),
  `unimportant` (blue: text equal after rules), `leftOrphan`, `rightOrphan`,
  `unknown` (content not checked yet), `error` (unreadable). A folder's status =
  the worst status below it.
- **Content** check (per `[compare] content`) on a background queue: byte
  compare in 1 MB chunks with early exit. Text files that differ get one
  pass of the normalizer to tell red from blue.
- **Symlinks** are compared as links (target path), never followed (the
  `/tmp` lesson from RecentFiles).
- **Copy / move / delete** go through `FileOps` (undo stack, Trash, " 2"
  clash names on "Keep both"), report to `FileDrag.onFileOp` (Recent and
  /paths stay right), then re-check only the touched rows.

#### 7.2.4 Keyboard map (Compare view)

Ctrl+N / Ctrl+P keep their app-wide meaning ("next / previous item"). In
Compare the items are differences, which is also Beyond Compare's own
default.

| Keys | Text Compare | Folder Compare |
|---|---|---|
| Ctrl+N / Ctrl+P | next / previous difference section | next / previous differing row |
| ↑ ↓ / PgUp PgDn / Home End | move the cursor | move the cursor |
| Tab | focus the other side | focus the other side |
| Opt+→ / Opt+← (Ctrl+R = right) | copy section / selection to right / left | copy selected to right / left (Ctrl+Opt+→ / ← = move) |
| Ctrl+H / Ctrl+L | the left / right side as a pane (every view's pane keys) | same |
| Ctrl+Opt+R / Ctrl+Opt+L | copy the current line | move selected to right / left |
| Cmd+Z / Cmd+Shift+Z | undo / redo (edits, copies) | undo the last copy / move / trash / rename |
| Cmd+S / Cmd+Opt+S | save the focused side / both | — |
| Cmd+O | open a file into the focused side | open a folder into the focused side |
| Cmd+L | focus the focused side's path field | same |
| Cmd+1…4 / Cmd+1…6 | All / Diffs / Same / Context | All / Diffs / Same / Orphans / L newer / R newer |
| Cmd+F, Cmd+G, Cmd+Shift+G | find, next, previous | filter box (name) |
| Ctrl+G | go to line | — |
| Cmd+Opt+X | swap sides | swap sides |
| F5 | reload + recompare | refresh |
| Return / double-click | — | open the pair (Text Compare, back step) / go into a folder |
| → / ← (Opt = recursive) | — | expand / collapse |
| Space | — | Quick Look |
| Cmd+R / F2 | — | rename (file browser keys) |
| Cmd+Delete | — | move to the Trash |
| Cmd+[ / Cmd+] / Cmd+Up | — | back / forward / both up one level |
| Cmd+K | action picker (`showActionPicker`): every action of the view by name | same |
| Cmd+/ | shortcuts card (`[shortcuts] "compare: …"` rows) | same |
| Esc | the dismissal chain in §7.1 | same |
| Cmd+W, Hyper+N, Ctrl+Tab | app-wide (hide / toggle / next view) | same |

Every text input in the view (path fields, filter box, find bar, the panes,
sheets) takes Cmd+C / V / X / A / Z and Ctrl+C / V (rule 1). Route them
through `handleKey` / `JiraEditKeys.route`.

#### 7.2.5 Right-click menus (rule 2)

- **Text pane:** Copy · Copy to Other Side · Copy Line to Other Side · Paste
  Clipboard Here · Select Section · Find… · Copy Path · Reveal in Finder ·
  Open in Notes · Open in Default App.
- **Folder row:** Open (Compare) · Copy to Other Side · Move to Other Side ·
  Quick Look · Rename… · Move to Trash · New Folder · Copy Path · Reveal in
  Finder · Open in Notes · Open in Default App · Set as Left / Right Base
  Folder · Exclude "NAME" (adds to the session's filter).
- **File browser / `/paths` rows (new items):** Select for Compare ·
  Compare to "NAME" (after a pick) · Compare (exactly two marked).

### 7.3 Technology

#### 7.3.1 Where it lives

| File | Contents | Depends on |
|---|---|---|
| `CompareText.swift` (new) | `TextSide` (decode / encode / line endings), `Importance` (normalizer), `LineDiff` (anchors + Myers), `CharDiff`, `AlignedRows` (rows with fillers, sections), `CopyAcross` | Foundation only → `bin/run-tests.sh compare` |
| `CompareFolder.swift` (new) | `FolderScan`, `FolderPair`, `PairStatus`, `ContentCheck`, `SyncPlan` (phase 3) | Foundation + `IgnoreRules`, `FileOps` |
| `CompareWindow.swift` (new) | `CompareWindow` (`CardWindowController`, `SlotMember`, views `.compare` / `.compareText`), `ComparePaneView` (drawn rows), `CompareThumbnail`, `FolderTreeView`, start page | AppKit |
| `SharedWindow.swift` | `SlotView.compare`, `.compareText` (`isSub`), nav id **67** (`navCompare`, appended after confluence in `navIcons`), `navOn`, `escViews`, `navClicked` | — |
| `workspace_switcher.swift` | `[compare]` parsing (`makeCommand` / `configNumberKeys` / `configValueProblem`), `ensureSlotMember(.compare)`, prewarm, socket `compare` + `do:compare:*`, palette entry | — |
| `main.swift` | CLI `compare` (`--wait` = `sendRequest`, reply on close, like `screenshot -r`) | — |
| `PopupWindow.swift` | `FileListPane.Action` + menu items "Select for Compare" / "Compare to…" | — |

`WordDiff` moves out of `AIWindow.swift` into `CompareText.swift` (`CharDiff`),
and the AI view calls it from there. That gives one diff in the app.

#### 7.3.2 Rendering and editing (spike S1 decides)

Panes need thousands of aligned rows, filler rows, per-character colors and
editing. Two options:

- **(a) Drawn rows + section editor (recommended).** `ComparePaneView` draws
  visible rows only (culls to the dirty rect like `PopupRowView`),
  monospaced, both panes scrolling together. Typing, or Return on a row,
  puts an `NSTextView` over the current section of that side (the
  `InlineRename` overlay idea, multi-line). Clicking away or moving off
  with Ctrl+N / P commits it, and it gets re-diffed. Esc commits and
  leaves edit mode (it never discards silently). Fast and simple, and alignment
  stays in our hands.
- **(b) Two NSTextViews, TextKit 2**, with filler gaps through custom layout
  fragment heights. It edits like a native text editor, but synced scroll +
  fillers + 100k lines are risky.

S1: build (a) with 100k lines. Pass = KR2 timings + edit round trip ≤ 50 ms.
If (a) feels wrong while typing, try (b) before going further.

#### 7.3.3 Config: `[compare]` in `commands.toml`

```toml
# /compare: Beyond Compare-style text + folder compare (CompareWindow.swift).
#   enabled          true | false: the view, its nav icon, palette entry
#   esc-close        0 | 1 | 2: "Esc Hides Window" (0 = never; default [app] esc-close)
#   font / font-size the panes' monospaced font (default: the notes font)
#   context-lines    lines around a difference in the Context filter
#   tab-width        spaces per tab when drawing
#   ignore-leading-ws / ignore-trailing-ws / ignore-embedded-ws / ignore-case /
#   ignore-line-endings / ignore-blank-lines   importance switches (true = unimportant)
#   content          auto | always | never: folder content check
#   time-tolerance   seconds two timestamps may differ and still be "same"
#   exclude          names / globs never scanned (comma list)
#   use-gitignore    true | false: honor .gitignore / .ignore like /paths
#   ignore-file      extra gitignore-syntax file
#   gutter-arrows    hover | always | off
#   max-lines        past this a text pair gets a byte compare only
#   recent           how many recent pairs the start page keeps
[compare]
enabled = true
esc-close = 0
context-lines = 3
tab-width = 4
ignore-leading-ws = true
ignore-trailing-ws = true
ignore-embedded-ws = false
ignore-case = false
ignore-line-endings = true
ignore-blank-lines = false
content = auto
time-tolerance = 2
exclude = .git, node_modules, .DS_Store, .build
use-gitignore = true
ignore-file = ~/.config/workspace-switcher/config/compare.ignore
gutter-arrows = hover
max-lines = 200000
recent = 30
same-label = Identical
binary-label = Binary files {}
```

All user-facing strings (status words, sheet buttons, empty-page hints) are
`*-label` keys (rule 3). Numeric keys go into `configNumberKeys`. Per-session
changes to importance or filters live in the session and don't write the
config. "Save as Default" in the ⚙ popover writes it.

#### 7.3.4 State on disk

- `~/.cache/workspace-switcher/compare-recent.json`: recent pairs (paths,
  kind, last used).
- `~/.cache/workspace-switcher/compare-sessions.json`: open sessions (pair,
  filter, importance overrides, scroll row, cursor) for restore after a
  restart.
- `~/.cache/workspace-switcher/compare-recovery/`: unsaved text of a dirty
  side, written 2 s after the last edit. Restored as "unsaved (recovered)"
  and deleted on save or discard.

#### 7.3.5 Theme

Draw with `PopupColors` tokens only (AGENT_CONTEXT "Theme system"):
important = `palette.danger`, unimportant = `palette.info`, orphan =
`palette.accent2`, older = `dim`, same = `text`. Line backgrounds = that
color at 12–18 % over `base`. Character marks = 35 % plus an underline for
color-blind safety. Filler = hatched `hairline` on `mantle`. Thumbnail = full
color. Pills, segmented filter (`ConfSegmented`), path fields
(`JiraInputBox`), buttons (`ThemeButton`). Live theme changes go through
`PopupThemeable`.

#### 7.3.6 Test hooks (required: "No source-grep tests")

- Socket `do:compare:open:LEFT|RIGHT`, `do:compare:paste:left|right:TEXT`,
  `next`, `prev`, `copy-right`, `copy-left`, `filter:NAME`, `swap`,
  `save:left|right`, `back`, `close-session`, `key:SPEC`.
- State `compare` {view, sessions[{kind, left, right, dirtyL, dirtyR}],
  current {sections, important, unimportant, cursorRow, filter, rows[0..50]
  {l, r, status}}, folder {counts, scanning, rows[0..50]}}.
- `bin/run-tests.sh compare`: engine against fixtures + the `git diff
  --no-index --histogram` parity corpus (KR4), encodings round trip
  byte-for-byte, folder scan vs `diff -rq` on temp trees, ignore rules,
  copy + undo.
- `bin/ui-test-focus.py compare`: show / hide via ✕, Cmd+W, Hyper+N, focus
  loss, Esc (back from `.compareText`, then hide only with the switch on),
  Ctrl+Tab in and out. Frame unchanged, AeroSpace view as for the other
  views (KR6).

### 7.4 CLI and git

```
workspace-switcher compare LEFT [RIGHT]          # files → text, folders → folder
workspace-switcher compare --wait LEFT RIGHT     # block until that session closes or the window hides
workspace-switcher compare --title1 "ours" --title2 "theirs" A B
```

`--wait` uses `sendRequest` (the reply comes when the session is done, as
with `screenshot -r`). It's for git, which deletes its temp files once the
tool returns:

```ini
[diff]      tool = ws
[difftool "ws"]
    cmd = ~/.config/workspace-switcher/workspace-switcher.app/Contents/MacOS/workspace-switcher compare --wait --title1 \"$BASE\" \"$LOCAL\" \"$REMOTE\"
[difftool]  prompt = false
```

`git difftool --dir-diff` passes two temp folders → Folder Compare. "Done" =
the session closed or the window hidden. Temp-file sessions are marked
"(git)" and are not added to Recent.

### 7.5 Assumptions (check during the build)

| # | Assumption | How to check |
|---|---|---|
| A1 | The owner uses BC for text + folder compare only (no merge, hex, remotes). | Owner confirms. `BCState.xml` holds only `TTextCompareState`. |
| A2 | Drawn rows + section editor (7.3.2 a) feels good enough for editing. | Spike S1, owner tries it. |
| A3 | Patience / histogram anchoring + Myers gets ≥ 95 % parity with git histogram. | KR4 corpus. |
| A4 | Hiding the window is an acceptable "done" for `git difftool --wait`. | Owner tries a `--dir-diff`. |
| A5 | Shared-window view (not a tool panel) is the right home: it should be hidden with the other views, not float on its own. | Owner confirms (this PRD's §7.1 dismissal table). |
| A6 | Ctrl+R / Ctrl+N / Ctrl+P / Opt+→ / Opt+← don't clash with anything the owner relies on inside the view (Ctrl+L became the pane key, 2026-10-06: PRD-keyboard.md). | `ws-settings` clash report (`conflicts.py`) after adding the `[shortcuts]` rows. |
| A7 | A 2 s timestamp tolerance + content `auto` gives no false "same" in practice. | Folder tests with same-size, same-second edits (content check covers it when times differ; equal times + equal size = "same" without content, as in BC's quick test). |

## 8. Release

Relative sizes. Each phase ships on its own and is usable.

| Phase | Scope | Rough size |
|---|---|---|
| **0: spike** | S1 (rendering + editing, 100k lines), diff engine + parity corpus | a few days |
| **1: Text Compare** | view + dismissal wiring (`.compare`, nav icon, Esc / hide / focus loss, Ctrl+Tab), start page + recent, CLI + `--wait` + git difftool, file browser / `/paths` "Compare to…", clipboard sides, importance, filters, thumbnail, copy across, edit, save, find, shortcuts card, test hooks | ~1–2 weeks |
| **2: Folder Compare** (built) | scan + pairing + status colors + filters + flatten + ignore rules, content check, open a pair as `.compareText` (Esc = back), copy / move / trash / rename with undo, right-click menus, `--dir-diff` | ~1–2 weeks |
| **3: polish + sync** (built, see below) | Synchronize (Update / Mirror) with preview sheet, Align With, trim-whitespace / line-ending convert, session restore + recovery copies, syntax highlighting (if wanted) | ~1 week |
| later | grammar rules per language, CRC / permissions criteria, over-under layout | — |
| never (out of scope) | Hex, Media, Picture, Table / Data, Registry, Version compare, 3-way merge, remotes, archives, reports, scripting | — |

#### Phase 2 as built

- Files: `CompareFolder.swift` (engine, Foundation only), `CompareFolderView.swift`
  (view + actions), `FileOps.place` (copy / move to a mirrored path, ONE undo
  record). Tests: `bin/run-tests.sh compare` (`Tests/test_compare_folder.swift`),
  `bin/ui-test-focus.py compare` (folder scan, filter, copy + undo, open pair, back).
- Done: scan + pairing (case per volume, NFC = NFD, symlinks as links), quick
  test with `time-tolerance`, background content check (`content`), the
  Text Compare importance rules for blue, status colors + center glyphs, the
  six filters (⌘1–6), flatten, name filter (`*.swift, !*.o`), hidden files
  (⌘⇧.), exclude + gitignore rules, expand / collapse (⌥ = all below), open a
  pair in Text Compare on the view (Esc = back, cursor kept), copy / move (a
  folder pair is merged; a clash asks Replace · Keep Both · Skip), trash,
  rename, new folder, Cmd+Z, right-click menu, Cmd+K actions, CLI / file
  browser / `/paths` entry for two folders, `[shortcuts]` rows.
- Changed from the spec: rename and new folder use a sheet, not an inline
  field; "Replace" sends the old file to the Trash (undoable) instead of a
  separate undo for the delete; Cmd+Z uses the app-wide `FileOps` stack, so it
  can also undo a file browser operation; the content check runs on pairs
  with equal size and different time (`auto`), plus one text pass over pairs
  that differ, ≤ 4 MB each, to find blue ones.
- Built later (with phase 3): Quick Look, Set as (Left / Right) Base Folder,
  drag across to copy, Back / Forward / Up One Level, a real
  `git difftool --dir-diff` run, and a per-session undo (the deviation above
  is gone: a folder session's ⌘Z only undoes its own operations).

#### Phase 3 as built

- Synchronize ▸ Update Right / Left / Both (newer + orphan items; pairs that
  differ with neither side newer are skipped and counted), Mirror to Right /
  Left (every difference copied, the far side's orphans to the Trash). Always
  a preview sheet (counts + the first items); the whole run is ONE ⌘Z.
  `SyncPlan` in CompareFolder.swift, tested in `bin/run-tests.sh compare`.
- Align With: right-click a line ▸ Align With…, then a line on the other side ▸
  Align With Picked Line. The two share a row; the diff runs between anchors.
  Remove This Alignment / Clear All Alignments. Not undoable (it changes no
  text); an edit that deletes an anchored line drops that anchor.
- Convert ▸ Trim Trailing Whitespace, Line Endings → LF / CRLF / CR (per side,
  one undo step each, endings kept byte-exact otherwise).
- Visible whitespace (kitchen sink): spaces as ·, tabs as →.
- Session restore + recovery copies as in §7.3.4 (git sessions excluded). A
  recovered side opens dirty with "unsaved changes recovered"; ⌘Z returns to
  the file on disk.
- git `--dir-diff`: git symlinks the work tree's files into its right-hand
  folder. A link facing a regular file is compared by its target, and git
  sessions always check content (git writes the left copies at run time, so
  times say nothing). Edits save through the link into the work tree. Hiding
  the window, also from a pushed Text Compare, returns control to git.
- Not built: word wrap, syntax highlighting, Isolate.

## 9. Definition of done (acceptance checks)

1. `./build.sh --build-only` passes; `bin/run-tests.sh compare` passes,
   including the KR4 parity corpus and byte-exact save round trips (UTF-8,
   UTF-8 BOM, UTF-16, CRLF, no final newline).
2. `bin/ui-test-focus.py compare` passes (KR6): ✕ / Cmd+W / Hyper+N /
   focus loss hide and park; Hyper+N restores the same session + scroll;
   Esc steps back from `.compareText`, then hides only with "Esc Hides
   Window" on; Ctrl+Tab cycles through Compare.
3. Unsaved edits survive hide, focus loss, view switches and a daemon
   restart (recovery). Closing a dirty session asks with a sheet.
4. Every folder action (copy, move, trash, rename) is undone by Cmd+Z, and
   Recent / `/paths` reflect it.
5. Timings in `/tmp/ws-debug.log` meet KR2 / KR3 on the owner's Mac.
6. Every text input passes the rule 1 edit keys; every right-click menu
   lists §7.2.5's actions (rule 2); strings + sizes are in `[compare]`
   (rule 3).
7. `[shortcuts]` has the `compare: …` rows, and `ws-settings` reports no new
   clashes.
8. AGENT_CONTEXT.md gets a "Compare" section (files, symbols, hooks,
   config), like the other views.

## 10. Research notes and references

- Beyond Compare 5 help, Text Compare commands (the full menu list behind
  §7.2.1): https://scootersoftware.com/v5help/commandstext.html
- Text Compare view (side-by-side panes, thumbnail, line details, red =
  important / blue = unimportant): https://www.scootersoftware.com/v5help/viewtext.html
- Text importance settings (leading / embedded / trailing whitespace, case,
  line endings ignored by default): https://scootersoftware.com/v5help/sessiontextimportance.html
- Folder Compare commands (Show All / Differences / Same / Orphans / Newer,
  Ignore Folder Structure, Copy / Move to side, Synchronize Update / Mirror):
  https://www.scootersoftware.com/v5help/commandsdir.html
- Folder comparison criteria (size, timestamp tolerance, CRC, binary,
  rules-based, "skip if quick tests say same"):
  https://www.scootersoftware.com/v5help/sessiondircomparison.html
- Text Merge commands (out of scope, for reference):
  https://scootersoftware.com/v5help/commandstextmerge.html
- Local install: Beyond Compare 5.2.5 build 32528, `bcomp` CLI in
  `Contents/MacOS`, trial keys in `Contents/Resources`, state in
  `~/Library/Application Support/Beyond Compare 5/BCState.xml` (text
  compare state only, no saved sessions).
- Repo building blocks: `SharedWindow.push` / `back(esc:)` /
  `escapeAtTop` / `escViews` / `navIcons`; `CardWindowController`;
  `FileListPane.Action`; `FileOps`; `InlineRename`; `IgnoreRules`;
  `WordDiff` (`AIWindow.swift`); `showActionPicker`; `sendRequest`
  (screenshot `-r`).
