# kitchen-sink — menu & context-menu inventory

Read-only survey of **every user-facing menu, pop-up and picker** in the app, plus a
first pass at what looks **redundant or out of place**.

- **Method:** source reading only. No build, no app run, no socket probes.
- **Truth:** the code, not the docs. Line numbers are today's (`Oct 14 2026`) and drift —
  **grep the symbol**, don't trust the number. Every row below also names the builder
  function so it can be found after a shift.
- **Scope:** the macOS AppKit app (`rust/ws-rs`). The Linux/egui frontend gets a short
  section because it has its own (much smaller) context menu.
- **Paths** are relative to the repo root; `host.rs` = `rust/ws-rs/src/app/host.rs`.
- **Not covered:** the `do:`/raw socket verbs (an agent API, not a menu) and the
  command-line modes.

---

## 0. The five ways a "menu" exists here

Everything user-facing falls into one of these:

| # | Mechanism | Where | Opened by |
|---|---|---|---|
| 1 | **Main menu bar** (About / File / Edit / Window) | `app/menu.rs:600` `install_main_menu` | installed once at `main.rs:191` (daemon) and `main.rs:71` (prose child) |
| 2 | **Status-item menu** (the ☰ menu-bar icon) | `app/menu.rs:300` `build_menu_spec` | `main.rs:242` `install_status_item`; **also re-popped** inside the header menu as "App Menu…" (`host.rs:6107`) |
| 3 | **Closure pop-up menus** (every right-click / toolbar menu) | `ui/menu_kit.rs` — `popup` `:279`, `popup_at` `:303`, `popup_context` `:313` | rows are `MenuEntry::item/checked/disabled/submenu/Header/Separator` (`ui/menu_kit.rs:9-87`) |
| 4 | **Custom cards / pickers** (not `NSMenu`) | palette `views/switcher.rs`, view switcher `views/switcher.rs:215`, Cmd+K `ui/action_picker.rs`, column filter `ui/multi_picker`, shortcuts sheet `ui/shortcuts.rs`, sprint `NSPopUpButton` `views/jira_view.rs:3737`, AI rules `NSPopUpButton` `views/ai.rs:2142` | shortcuts, header buttons, `do:` hooks |
| 5 | **Web-content native menu filter** | `ui/quiet_web.rs:26` `willOpenMenu:` | right-click in any WKWebView (AI, Confluence, Jira detail) |

Linux adds a sixth: **egui `context_menu`** in `frontend_egui/files.rs:77` (reused by
`frontend_egui/paths.rs:113`).

The vast majority of right-click menus are **not** defined in the view files — the view
files emit events (`FilesEvent::RowMenu`, `JiraEvent::RowMenu`, `ConfEvent::RowMenu`,
`TreeEvent::Menu`, `SidebarEvent::MenuRow`) and `host.rs` builds the `menu_kit` menu.
When hunting a menu, start in `host.rs`.

---

## 1. Main menu bar — `install_main_menu`, `app/menu.rs:600-652`

Installed with **`target: None`** (`main.rs:191`), so nothing here has a live handler —
items either ride the responder chain or are dead.

| Menu | Row(s) | Key | Action | Line |
|---|---|---|---|---|
| app (kitchen-sink) | About kitchen-sink | — | `orderFrontStandardAboutPanel:` | `menu.rs:606` |
| | Hide | ⌘H | `hide:` | `menu.rs:608` |
| | Hide Others | ⌥⌘H | `hideOtherApplications:` | `menu.rs:609` |
| | Show All | — | `unhideAllApplications:` | `menu.rs:611` |
| | Quit | ⌘Q | `terminate:` | `menu.rs:613` |
| File | Close Window | ⌘W | `performClose:` — responder chain; the shared window funnels it to its close request (hide) | `menu.rs:607` |
| | Reset Window Size | ⌘0 | `menuAction:` → `MenuAction::ResetWindowSize` → `controller.do_action("reset-size")` — **daemon only** | `menu.rs:606`, `main.rs:278` |
| Edit | Undo / Redo / Cut / Copy / Paste / Select All | ⌘Z ⇧⌘Z ⌘X ⌘C ⌘V ⌘A | responder chain | `menu.rs:629-635` |
| Window | Minimize | ⌘M | `performMiniaturize:` | `menu.rs:642` |
| | Zoom | — | `performZoom:` | `menu.rs:643` |
| | Bring All to Front | — | `arrangeInFront:` | `menu.rs:645` |
| | *(open windows are appended by AppKit)* | | `setWindowsMenu` | `menu.rs:651` |

`MenuAction::ResetWindowSize` (`menu.rs:42`) is **wired now**: the daemon installs the main menu
with a `MenuTarget` (`main.rs:191`), so File ▸ "Reset Window Size" calls
`controller.do_action("reset-size")`. The `prose` child passes `None`, so it gets no reset row
and there is no dead item anywhere. (Fixed — see §8 remediation log.)

---

## 2. Status-item menu (☰) — `build_menu_spec`, `app/menu.rs:300-340`

Spec lives in `MenuSettings` (`menu.rs`); the click handler is `status_menu_action`
(`main.rs`). After round 2 the status menu is deliberately minimal — the views and the
"Kitchen Sink" window row live in the palette instead (§8b).

```
Header Bar Style ▸   (7 rows: Quiet / Flat / Accent Edge / Accent Stripe / Tinted / Glow / Aurora)
──────────
Close Window         (no key; ⌘W lives on File ▸ Close Window)
──────────
Quit                 ⌘Q
```

History: the Swift menu's dead placeholder rows were dropped because *"the palette covers those
features"*, and the "Hide When Focus Is Lost" row (id 1) was removed because the feature is not
ported (id retired — §8b). Round 2 then removed the three view toggles, the "Kitchen Sink" row and
the single-child "Global Window Options" wrapper, and renamed "Header Style" → "Header Bar Style"
(§8b). The status menu is no longer nested inside the header icon menu (F2).

---

## 3. Header app-icon menu — `header_icon_menu`, `host.rs:5995-6113`

Opened by clicking the header app icon (`chrome::HEADER_ICON_ID` → `host.rs:11693-11699`).
If no view is current it falls back to the raw status menu (`host.rs:5999`).
Composition: **per-view rows** (`view_icon_entries` `host.rs:9363`) + a trailing
separator (`host.rs:9488`) + the common block.

### 3a. Per-view rows — `view_icon_entries`, `host.rs:9363-9491`

Only three views add rows today. Files, Notes, Paths, Terminal, Compare add **nothing**
(the file also has no "Files"/"Notes" block, so those windows get the common block only).

**Jira** (`host.rs:9366-9387`)
```
Search Jira…                        host.rs  (the ⌘F hint was dropped in round 2 — §8b)
Group By ▸                          host.rs  (see §5, reused)
──────────
Import Favourite Filters            host.rs:9378
Open Jira Config Window             host.rs:9384
```

**AI** (`host.rs:9388-9448`)
```
New Rule…                           host.rs:9393
Edit “{rule}” in Notes              host.rs:9401  (only when a rule is selected)
Open Rules Folder                   host.rs:9407
Reload Rules                         host.rs:9412
──────────
Copy for ▸                          host.rs  (Outlook / Webex — one submenu since round 2)
Copy as Markdown                    host.rs
Copy Command                        host.rs:9443
```

**Confluence** (`host.rs:9449-9486`)
```
Confluence Setup…                   host.rs:9451
Import My Saved Pages as Favorites  host.rs:9457
Refresh Contributor List            host.rs:9463
──────────
Open config.json                    host.rs:9469
Open debug.log                      host.rs:9478
```

### 3b. Theme submenu — `host.rs:6012-6077`

```
<Header: tone-group name>           host.rs:6017   (one per group, separator between groups :6015)
  <preset name>  [swatch, checked]  host.rs:6046   (plain item for most views)
  <preset name> ▸                   host.rs:6042   (**Notes only** — a submenu of scopes)
      Whole Window                  host.rs:6024
      ──────────
      Notepad Only                  host.rs:6025
      Terminal Only                 host.rs:6026
──────────
Reset to Defaults                   host.rs:6064   → reset_theme(section)
```

### 3c. Common block — `host.rs:6076-6111`

```
Theme ▸                             host.rs:6077
──────────
Reset Default Size                  host.rs:6086
──────────
Open Config                         host.rs:6097   (opens commands.toml in Notes)
Keyboard Shortcuts  ⌘/              host.rs:6101   → shortcuts sheet
```

---

## 4. Right-click / toolbar context menus (all `menu_kit`, built in `host.rs`)

Format: trigger → builder → rows.

### 4a. Paths shelf
Right-click a row (`PathsEvent::RowMenu`) → `pathsed_event` `host.rs:4522` → `popup` `:4532`
```
Copy File / Copy Absolute Path / ── / Open in Notes / Reveal in Files   host.rs:4534-4542
```

### 4b. Files
- **Sort button** (`FilesEvent::SortMenu` `files_view.rs:767`) → `files_event` `host.rs:5865`,
  labels from `SortKey::label` `views/files.rs:105`:
  `Name / Date Modified / Date Created / Size / Kind` (checked = current) `host.rs:5876`.
- **Sidebar pinned row** (`SidebarEvent::MenuPinned` → `host.rs:5948`):
  `Open / Reveal in Files / Copy Absolute Path / ── / Unpin` `host.rs:5958-5968`.
- **List row** (`FilesEvent::RowMenu` `files_view.rs:339` → `files_row_menu` `host.rs:10256`,
  `popup` `:10384`):
  ```
  Open                              host.rs:10291  (single selection)
  Open in Notes                     host.rs:10303  (single, non-dir)
  Quick Look                        host.rs:10312
  ──────────
  Copy Absolute Path(s)             host.rs:10321
  Copy {n} Items / Copy             host.rs:10327
  Cut {n} Items / Cut               host.rs:10333
  Duplicate                         host.rs:10339
  Compress {n} Items / Compress “x” host.rs:10344
  ──────────
  Reveal in Files                   host.rs:10351
  ──────────                       (2 same-type selected)
  Compare                           host.rs:10362
  ──────────
  Rename…                           host.rs:10377
  Move {n} Items to Trash / Move to Trash  host.rs:10379
  ```
  plus `files_append_folder_items` (`host.rs:10389`, `sep=true` → `:10422`):
  `New Folder / New File / Paste / Show|Hide Hidden Files` `host.rs:10403-10412`.
- **Blank space** (`FilesEvent::BlankMenu` → `files_blank_menu` `host.rs:10428`): same four
  folder rows, `sep=false`.

### 4c. Notes
- **Pinned-folder tree** (`TreeEvent::Menu` → `notes_tree_event` `host.rs:6426`, `popup` `:6510`):
  ```
  New Note Here (dir) / Collapse·Expand (dir) / ── / Open in Notes (file) / ──
  Copy File / Duplicate / Copy Absolute Path / Reveal in Files
  ── / Rename…  F2 / Delete…        (non-root)          host.rs:6432-6481
  ```
- **Sidebar "Open" row** (`SidebarEvent::MenuRow` → `host.rs:6642`):
  `Rename…  F2 / ── / Copy Absolute Path / Reveal in Files / ── / Close` `host.rs:6649-6658`.
- **Plain editor** (`set_editor_menu` `notes.rs:4015`; builder `notes_editor_menu` `host.rs:10783`,
  `popup_context` `:10818`):
  ```
  [image under click] Open Full Size / Copy Image Path / ──
  [note] Copy Absolute Path / Open file at path…            host.rs:10795-10813
  ```
- **Prose reading view** (`ProseWebView.rightMouseDown` → `show_context_menu`
  `views/notes_prose.rs:472`, also wired for the pop-out `notes.rs:3061`):
  `Copy Absolute Path / Copy PDF Path` `notes_prose.rs` (renamed from "Copy File Path" in round 2 — §8b).
- **Vim image overlay** (`vim_image_menu` `views/notes.rs:2705`):
  `Open Full Size / ── / Copy Image Path / Copy Image / Reveal in Files` `notes.rs:2716-2727`.
- **Notes-drawer terminal** (`terminal_right_click` `host.rs:10846` → `terminal_menu`
  `host.rs:10860`):
  ```
  Copy (disabled w/o selection) / Paste / ──
  [path selected] Open in Notes / Copy File / Copy Absolute Path / Reveal in Files
  [no path]  the same 4, disabled, + disabled "Select a file path to use these"
  ── / Select All                                              host.rs:10874-10905
  ```

### 4d. Compare
- **Sidebar session row** (`host.rs:6711`): `Close / ── / Copy {Left|Right} Path / Reveal {Left|Right} in Files`
  `host.rs:6724-6735`.
- **Recents row** (`CompareEvent::RecentMenu` `compare_view.rs:303` → `host.rs:6817`):
  `Open / Fill the Path Fields / Reveal Left in Files / Reveal Right in Files / Copy Absolute Paths /
  ── / Remove from Recent / Remove All Missing (n) / Clear All Recents` `host.rs:6845-6877`.
- **Folder tree** (`compare_folder_menu` `host.rs:6952`, `popup` `:7095`):
  ```
  Collapse·Expand / Open in Text Compare                    host.rs  (one stable label since round 2)
  ──────────
  Copy to Right  ⌥→ / Copy to Left  ⌥← / Move to Right  ⌃⌥R / Move to Left  ⌃⌥L   host.rs:6995-6998
  ──────────
  Rename…  F2 / New Folder…  ⌘⇧N / Move to Trash  ⌘⌫         host.rs:7013-7051
  ──────────
  Copy Absolute Path / Reveal in Files / Open in Notes / Quick Look  Space   host.rs:7053-7084
  ──────────  (folders)
  Set as Base Folder  ⌘↓ / Set as Left Base Folder / Set as Right Base Folder host.rs:7088-7090
  ──────────
  Exclude “{name}”                                          host.rs:7094
  ```
- **Text pane** (`compare_pane_menu` `host.rs:7137`, `popup` `:7302`):
  ```
  Copy / Copy to {other} (⌃R|⌃L) / Copy Line to {other} (⌃⌥R|⌃⌥L) / Paste Clipboard Here /
  Select Section / ── / Align With… (or Align With Picked… / Cancel Align With) /
  Remove This Alignment / Clear All Alignments (n) / Convert {side} Side ▸
       Trim Trailing Whitespace / ── / Line Endings → LF · CRLF · CR
  ── / Copy Absolute Path / Reveal in Files / Open in Notes /
  Open File into This Side…  (⌘O) / Save This Side  (⌘S)      host.rs:7195-7293
  ```

### 4e. Jira
- **Table header** (`JiraEvent::HeaderMenu` `jira_view.rs:3283` → `host.rs:9551`):
  `Fit Columns to Content / ── / Group By ▸` `host.rs:9556-9563`.
- **Issue row** (`JiraEvent::RowMenu` `jira_view.rs:2806` → `host.rs:9605`):
  `Open in Browser / ── / Copy Key / Copy Link` `host.rs:9616-9619`.
- **Search bar** (`jira_search_event` `host.rs:9038-9111`): add-filter menu (`:9042`), date-range
  menu (`:9068`), max-results menu (`:9090`). Titles are config/data-driven.
- **Sidebar pin** (`SidebarEvent::MenuPinned` → `jira_sidebar_menu` `host.rs:9687`):
  `Open / ── / Unpin` — "Open" reuses the plain pin-click dispatch (`jira_open_pin_key`), "Unpin"
  appears only for a board view currently in `pinned_views`. (Was a silent no-op — see §8.)

### 4f. Confluence
- **Chip menus** (`ConfEvent::Chip` → `host.rs:8317`): spaces/contributors checkbox lists
  (`:8363-8382`), type (`:8346`, from `TYPES` `confluence_view.rs:25`), modified
  (`:8346`, `MODIFIED` `:33`), sort (`:8346`, `SORTS` `:40`).
- **Result row** (`ConfEvent::RowMenu` → `host.rs:8418`):
  `Open in Browser / Copy Link / Copy Title + Link / Copy as Markdown Link / ── /
  Add|Remove Favorites / Open Space {n} in Browser` `host.rs:8437-8450`.

### 4g. AI
- **Rule row** (`SidebarEvent::MenuRow` → `ai_rule_menu` `host.rs:7518`):
  `Open in Notes / Reveal in Files / Copy Absolute Path / ── / Duplicate… / Delete…`
  `host.rs:7527-7540`.

### 4h. Notes document-style menu (the ◐ segment)
`notes_doc_style_menu` `host.rs:6311`:
`Header "Document style" / Plain (no style) / ── / <template names>` `host.rs:6351-6363`;
helper-down fallback row `:6348`.

### 4i. Screenshot (defined in the view, not the host)
- **Pin window** (`pin_menu` `views/screenshot.rs:6956`):
  `Copy to clipboard / Save to file / ── / Rotate Right / Rotate Left / Increase Opacity /
  Decrease Opacity / ── / Close` `screenshot.rs:6969-6985`.
- **Recents grid** (`screenshot.rs:7709`): `Reopen as Pin / Copy / Open in Popup / Reveal in Files /
  ── / Delete` `screenshot.rs:7720-7743`.

---

## 5. Pickers & non-NSMenu surfaces

| Surface | Reach it | Rows / source | Code |
|---|---|---|---|
| **Command palette** | global **Hyper+S**, CLI `show` | the six views (**Notes, Files, Jira, Confluence, AI, Compare** — listed since round 2) plus the tool rows (**Create File, Find Recent Files, Text to Table, Pretty Print, Health Checks, Jira Config Window**) and **Kitchen Sink**. Built from `commands.toml` rows with `enabled=true` + `in-palette≠false`, minus `COMMAND_SECTION_SKIPS` (`host.rs:36`) | `show_palette` `host.rs`; model `views/switcher.rs`; `build_palette` `host.rs` |
| **View switcher** | **Ctrl+B W** (also `do:switcher`) | view names only, `host.rs:4267-4272` (Files/Notes/Confluence/Compare/AI/Jira); commands are explicitly emptied (`host.rs:4288`) | `switcher.rs:215`, `SwitchUi` `switcher.rs:482` |
| **Cmd+K action picker** — Compare | ⌘K in Compare | "Compare actions": New Comparison / Next·Prev Difference / Copy Right·Left / Swap Sides / Reload / Save Left·Right / Undo (`host.rs:9992-10013`) | `compare_actions` `host.rs:9984`; card `ui/action_picker.rs` |
| **Cmd+K action picker** — Jira | ⌘K in Jira | "Actions for {what}": Open in browser / Copy URL and title / Copy to clipboard / Add·Remove favorites / Pin labels to sidebar / Show release issues / Favorite·Unfavorite release / Blacklist·Restore release (`host.rs:10116-10153`) | `jira_actions` `host.rs:10062` |
| **Column filter multi-picker** | chevron in a Jira column header | value rows + `Sort ↑` / `Sort ↓` / `Pin to Sidebar` (`host.rs:8725`, `:8741`) | `jira_filter_picker` `host.rs:8657`; `ui/multi_picker` |
| **Jira "Group By"** | header menu + table-header menu | None / ── / choices / Collapse All / Expand All | `jira_group_menu` `host.rs:8763-8795` |
| **Keyboard shortcuts sheet** | header menu "Keyboard Shortcuts ⌘/" or **⌘/** | `[shortcuts]` in `commands.toml:406-580`, grouped per view; `⌘/` itself is documented once under `all:` (a shared `all:` row must not be copied into a view) | `ui/shortcuts.rs` (`groups_for` `:91`, `sheet_key` `:124`) |
| **Sprint picker** | board bar pop-up | from `JiraBoardBarModel.choices` (dynamic) | `views/jira_view.rs:3737-3779` |
| **AI rules picker** | AI pane top bar | rule names, or "No rules" | `views/ai.rs:2142-2148` |

---

## 6. Web-content native menu — `ui/quiet_web.rs:26`

Every `QuietWebView` (AI preview, Confluence preview, Jira detail) filters the system
context menu down to `Copy / Copy Link / Open Link / Copy Image` (`quiet_web.rs:11-16`); if
nothing survives it inserts `Select All` (`quiet_web.rs:38`).

---

## 7. Linux / egui frontend

One context menu: `row_context_menu` `frontend_egui/files.rs:77` (Files, and Paths via
`frontend_egui/paths.rs:113`) → `Copy File / Copy Absolute Path / Open in Notes /
Reveal in Files`. The AppKit version pluralises ("Copy Files", "Copy Absolute Paths") for
multi-select (`frontend_egui/files.rs:48,56`) but the egui caller always passes `count == 1`.
No menus exist in `frontend_egui/{jira,notes}.rs`.

---

## 8. Redundancy / out-of-place findings

Ordered by how clearly they look wrong. Each names the seam for a fix.

### F1 — "Kitchen Sink" as a row inside kitchen-sink's own menu
`menu.rs:332` (status menu) and `host.rs:892` (palette `window` command) both exist purely
to raise the app's own shared window (`controller.toggle()` `main.rs:265` / `host.rs:1553`).
The app already has an `About kitchen-sink` item (`menu.rs:606`) and a Dock/menu-bar
presence. A row literally named after the app, in that app's menu, reads as a bug.
**Seam:** drop the `OpenWindow` node (`menu.rs:331-333`) and/or the palette `window` row
(`host.rs:891-893`), or rename to "Show Window"/"Bring Window Forward".

### F2 — The whole status menu is exposed twice
The identical `NSMenu` is the ☰ status item (`main.rs:242-255`) **and** nested inside the
header icon menu as "App Menu…" (`host.rs:6107-6110`) — an item whose action pops another
menu, with an ellipsis that implies a dialog.
**Seam:** if it must appear in the header menu, make it a real `MenuEntry::submenu`
(`ui/menu_kit.rs:48`) rather than a re-pop; otherwise delete the row.

### F3 — "Toggle Notes" / "Toggle Jira Window" / "Toggle Terminal" are an arbitrary subset
The status menu exposes toggles for Notes, Jira and Terminal only (`menu.rs:306-323`) —
not Files, Confluence, Compare or AI, which are equally first-class views. Each duplicates
an existing path: the header nav icons (`registry.rs:96` `nav_id`), the launch/CLI modes
`notes|jira|terminal` (`bin/kitchen_sink.sh:82`, consumed by `toggle_command`
`host.rs:1543-1587`), and the `do:` verbs (`do:open:notes`, `toggle-terminal`
`host.rs:1967`). Note the checked-in AeroSpace config binds only Hyper+S / Hyper+X / Hyper+/
(`config/aerospace/aerospace.toml:85-89`) and the palette excludes views
(`in-palette = false`), so these three rows are effectively the *only* menu entry point for
Notes/Jira/Terminal — which is precisely why the arbitrary subset is a smell. The file's
own doc-comment (`menu.rs:294-299`) says the palette is the intended surface for features.
**Seam:** `build_menu_spec` `menu.rs:300-340`; delete the three rows or derive them from the
enabled views so the set is complete and consistent.

### F4 — ⌘W is claimed by two different "Close Window" items with two different behaviours
- Status menu `Close Window` ⌘W → `controller.hide("menu")` (`menu.rs:327`, `main.rs:269`).
- File menu `Close Window` ⌘W → responder-chain `performClose:` (`menu.rs:620`).
Two menu items in one app share the same key equivalent and do different things; the
in-window key router has a third path. Fragile and confusing.
**Seam:** `menu.rs:326-328` and `menu.rs:620`.

### F5 — Quit appears twice
App menu `Quit` ⌘Q (`menu.rs:613`, `terminate:`) and status menu `Quit` ⌘Q
(`menu.rs:336` → `MenuAction::Quit` → `terminate` `main.rs:278-282`). Same action, two rows.
**Seam:** `menu.rs:335-337`.

### F6 — File ▸ "Reset Window Size" is dead code
`install_main_menu` is always called with `target: None` (`main.rs:191`, `main.rs:71`), so
`add_reset_size` builds the item with **`action: None`** (`menu.rs:583-589`), and
`MenuAction::ResetWindowSize` has no arm in `status_menu_action` (falls into `_ => {}`
`main.rs:283`). Meanwhile the header menu has a working "Reset Default Size"
(`host.rs:6086`). The dead item even carries the ⌘0 key equivalent (`menu.rs:588`).
**Seam:** `menu.rs:582-595` + `main.rs:259-285`.

### F7 — "Reset Default Colors" duplicates Theme ▸ "Reset to Defaults"
Both sit in the same header menu and both call `reset_theme(section)`
(`host.rs:6091-6095` vs `host.rs:6064-6068`).
**Seam:** keep one (`host.rs:6091-6095` is the redundant one).

### F8 — "Open Config" naming collides with view config items
The header menu's `Open Config` opens `commands.toml` (`host.rs:6097-6100`), directly above
the Confluence-only `Open config.json` / `Open debug.log` (`host.rs:9469`, `:9478`) and next
to `Open Jira Config Window` (`host.rs:9384`) and `Confluence Setup…` (`host.rs:9451`).
Four "config"-ish rows opening four different things.
**Seam:** disambiguate titles (e.g. "Open commands.toml").

### F9 — "Hide When Focus Is Lost" toggles a feature the Rust port does not implement
`menu.rs:281-287` builds a real checkbox over `set_hide_on_focus_loss` (`main.rs:270-273`),
whose tooltip describes behaviour. Per `AGENTS.md` "Known gaps", the hide-on-focus-loss
focus bridge is **not ported**. A working-looking menu item for a non-working feature is
worse than absent.
**Seam:** gate the row on readiness, or remove until the watcher lands.

### F10 — The ⌘/ sheet is a documentation copy of the bindings, not the bindings
`ui/shortcuts.rs` renders `[shortcuts]` from `commands.toml:406-580`; the actual behaviour
lives in the key router (`route_key_event` / `ui/popup::route_key`) and
`config/aerospace/aerospace.toml`. Nothing keeps them in sync, and rows are self-duplicated
(`"all: Cmd+/"`, `"ai: Cmd+/"`, `"compare: Cmd+/"` all document the same key). The menu
title also hardcodes the key ("Keyboard Shortcuts  ⌘/" `host.rs:6101`) instead of using a
key equivalent.
**Seam:** `ui/shortcuts.rs`, `commands.toml:406`.

### F11 — The "file action" trio is respelled on every surface
The same concepts get different labels per menu:
`Copy File` (`host.rs:4534`, `:6454`, `:10888`) vs `Copy` (`screenshot.rs:7721`,
`host.rs:10874`) vs `Copy {n} Items` (`host.rs:10327`) vs `Copy Absolute Path(s)`
(`host.rs:4535` vs `:10321`); `Reveal in Files` vs `Show in Enclosing Folder`
(`host.rs:10351` vs `:10355`). Nine menu builders each hand-roll it.
**Seam:** factor a shared `file_entries(ctx)` helper next to `files_append_folder_items`
(`host.rs:10389`).

### F12 — Disabled-row spam when no path is selected (terminal)
`host.rs:10898-10901` pushes four disabled rows plus a fifth disabled hint. A menu of
nothing but disabled rows should not open, or should be one row.
**Seam:** `terminal_menu` `host.rs:10860`.

### F13 — Three different note context menus
`notes_editor_menu` (`host.rs:10783`), the prose menu (`notes_prose.rs:472`) and the vim
image menu (`notes.rs:2705`) offer overlapping image/path actions with three spellings
("Copy Image Path" / "Copy File Path" / "Copy Image"). Two of the three live outside
`host.rs`, so they drift independently.
**Seam:** unify on one builder.

### F14 — ~~The `/terminal` tool panel has no context menu at all~~ (withdrawn — not a defect)
**Correction made in round 2:** there is no `/terminal` tool panel in the Rust port. `ToolKind`
(`views/tools.rs:36`) has only PrettyPrint / Filefast / HealthChecks / TextTable; the `[terminal]`
section in `commands.toml` is a leftover, and `terminal` routes to the notes **drawer** terminal
(`toggle_terminal_panel` → `flip_terminal_panel`). There is one terminal surface and it already has
the menu, so this finding was wrong and is withdrawn. (Adding a menu to a second panel would first
mean building that panel.)

### F15 — Two independent appearance systems in two different menus — **fixed in round 2**
"Header Style" (7 options, status menu `menu.rs:257-272`) and "Theme" (~30 presets, header
menu `host.rs:6077`) both change the app's look and live in different entry points. Users
will not know which to reach for.
**Seam:** `menu.rs:257`, `host.rs:6012-6077`.
**Round 2:** the submenu is renamed **"Header Bar Style ▸"** with a tooltip distinguishing it from
the colour Theme presets, and the single-child "Global Window Options" wrapper is gone. Full
consolidation (one appearance home) was deliberately not attempted.

### F16 — The palette is a tools launcher, the status menu is a view switcher — **fixed in round 2**
Views set `in-palette = false` (`commands.toml:196`, `:232`, `:260`, `:314`, `:323`, `:347`)
so Hyper+S lists Create File / Find Recent Files / Text to Table / Pretty Print / Jira Config
Window — not Notes/Files/Jira. The view toggles live in the status menu instead. The name
"command palette" promises the union.
**Seam:** `build_palette` `host.rs:829-908`, `commands.toml` `in-palette` flags.
**Round 2:** the six views are now listed (with `label`s), and the status menu's view toggles were
removed (F3) — so the palette is the one command surface. `[screenshot]`/`[terminal]` stay gated.

### F17 — Jira header rows duplicate the palette and the shortcut — **partly fixed in round 2**
`Search Jira…  ⌘F` (`host.rs:9368`) duplicates the ⌘F router; `Open Jira Config Window`
(`host.rs:9384`) duplicates the palette `jira-config` row (`commands.toml:610`,
`host.rs:1191`). Small, but they widen the "where do I change this?" surface.
**Seam:** `view_icon_entries` `host.rs:9366-9387`.
**Round 2:** the embedded `⌘F` was dropped from the title (the key belongs to the router; the row
has no real key equivalent). The two actions themselves were kept for discoverability — deleting
them was judged too aggressive.

### F18 — Confluence chip values are lower-case against a Title-Case app — **fixed in round 2**
`any time`, `past 7 days`, `relevance`, `recently modified` (`confluence_view.rs:33-40`)
next to `Pages + blogs`, `All spaces` (`:26`, `host.rs:8363`).
**Seam:** `views/confluence_view.rs:25-40`.
**Round 2:** the `MODIFIED`/`SORTS` display titles are now "Any time / Past 7 days / … / Relevance /
Recently modified" (wire values untouched), and the strip's seeded labels
(`views/confluence.rs:2143-2144`) were brought in line.

### F19 — "Open (Compare)" vs "Open in Text Compare" changes with state — **fixed in round 2**
One row whose label flips on whether both sides exist (`host.rs:6990`). A row that is
sometimes one command and sometimes another is a low-grade surprise; two rows (one
disabled) would be clearer.
**Seam:** `compare_folder_menu` `host.rs:6990`.
**Round 2:** the row is always titled "Open in Text Compare" (the closure was already identical in
both cases).

### F20 — Jira sidebar right-click is a silent no-op
`JiraEvent::Sidebar(_) => {}` (`host.rs:9682`) swallows the event the sidebar emits
(`SidebarEvent::MenuRow`/`MenuPinned`), so the Jira sidebar looks right-clickable but
isn't — unlike Files, Notes, Compare and AI.
**Seam:** `host.rs:9682`.

### F21 — `pane-shot` preview window has no menu / surface (deferred in round 2)
`[pane-shot]` exists in `commands.toml:179` and `paneShot` in state, but there is no
preview window at all in the Rust port (`pane_shot_message` renders to a PNG and delivers it).
Attaching a menu means building the window first — a feature, not a menu cleanup. **Deferred.**

### F22 — Minor: `App Menu…`, `No rules`, casing, ellipses — **fixed in rounds 1–2**
`App Menu…` was removed in round 1 (F2), and the AI pane's four copy rows are now `Copy for ▸`
(Outlook / Webex) + "Copy as Markdown" + "Copy Command" (round 2). Remaining: ellipsis styles vary
(`Rename…  F2` vs `Duplicate…` vs `Open Full Size`) — cosmetic, left alone.

---

## 8b. Remediation log — round 1 (Oct 14 2026)

Fixes landed for the eight findings above, implemented by delegated agents and cross-reviewed.
**Fixed:**

| Finding | Change | Where |
|---|---|---|
| F2 | deleted the `App Menu…` row (+ its separator): the header menu no longer re-pops the status menu | `app/host.rs` `header_icon_menu` |
| F4 | exactly one ⌘W owner: File ▸ "Close Window" keeps ⌘W on the responder chain (`performClose:`); the status row loses its key equivalent. Deliberately **not** re-routed to `MenuAction::CloseWindow` — that would have hidden the shared window behind transient overlays (rule 6) | `app/menu.rs` |
| F6 | File ▸ "Reset Window Size" is live: `install_main_menu` takes the `MenuTarget`; the row is omitted for the target-less `prose` child; `MenuAction::ResetWindowSize` → `do_action("reset-size")` | `app/menu.rs`, `main.rs` |
| F7 | deleted "Reset Default Colors" (Theme ▸ "Reset to Defaults" is the single reset) | `app/host.rs` |
| F9 | deleted the "Hide When Focus Is Lost" row + variant + ids + settings field; id 1 retired | `app/menu.rs`, `main.rs` |
| F10 | the duplicate `⌘/` "this list" rows were removed; the module now states the sheet is descriptive, and a test pins "a shared `all:` row must not be copied into a view" against the real `commands.toml` | `ui/shortcuts.rs`, `commands.toml` |
| F11 | deleted the duplicate "Show in Enclosing Folder" row (it called the same `reveal_in_files` as "Reveal in Files") | `app/host.rs` `files_row_menu` |
| F20 | the Jira sidebar right-click now opens `Open / ── / Unpin`; the pin dispatch was extracted so the menu and a plain click cannot diverge | `app/host.rs` `jira_event` |

### Round 2 (Oct 14 2026) — Tier 1 + 2

| Finding | Change | Where |
|---|---|---|
| F1 | dropped the "Kitchen Sink" row from the status menu (the palette keeps its `window` entry) | `app/menu.rs`, `main.rs` |
| F3 | dropped "Toggle Jira Window" / "Toggle Terminal" / "Toggle Notes" and their variants/ids/arms | `app/menu.rs`, `main.rs` |
| F15 | flattened the single-child "Global Window Options" wrapper; renamed "Header Style" → "Header Bar Style" (+ tooltip); `GLOBAL_GROUP_TAG` removed | `app/menu.rs` |
| F16 | the palette now lists the six views (`in-palette = true` + `label`) | `commands.toml`, `app/host.rs` (test) |
| F12 | the terminal menu no longer opens four disabled rows when no path is selected | `app/host.rs` `terminal_menu` |
| F17 | dropped the embedded `⌘F` from the Jira menu row title | `app/host.rs` `view_icon_entries` |
| F19 | one stable label, "Open in Text Compare" | `app/host.rs` `compare_folder_menu` |
| F18 | `MODIFIED`/`SORTS` display titles Title-cased (+ the strip's seeded labels) | `views/confluence_view.rs`, `views/confluence.rs` |
| F22 | AI copy rows folded into `Copy for ▸` | `app/host.rs` `view_icon_entries` |
| F13 | the prose menu's "Copy File Path" → "Copy Absolute Path" | `views/notes_prose.rs` |
| F14 | **withdrawn** — no `/terminal` tool panel exists | — |
| F21 | **deferred** — needs the preview window built first | — |

One review-driven rework: the prose menu's added "Reveal in Files" row was **removed** — it is a
silent no-op in the prose pop-out, which runs in a separate process with no daemon UI.

**Still open (as of round 2):** F5 (Quit twice — judged *not* a defect: a status-item Quit beside the
app-menu Quit is normal macOS), F8 (`Open Config` naming), F21 (deferred), plus the deeper half of F10
(generating the ⌘/ sheet from the key router) and the wider label unification behind F11.
(Round 3 below fixes F8 and the F11 labels; F10's deeper half and F21 remain open.)

### Round 3 (Oct 14 2026) — `PLAN-menu-cleanup-remaining.md` R1 / R2 / R4

| Item | Change | Where | Commit |
|---|---|---|---|
| R1 / F8 | header row "Open Config" → "Open commands.toml" (it opens `commands.toml`) | `app/host.rs` `header_icon_menu` | `0419f4b` |
| R2 / F11 labels | extracted one `label_of` helper; `build_nav` + `build_palette` read the view `[section] label` (falls back per view); `prepare_panel` uses the nav tip, so header tooltip / pane name / palette row share one source. Palette fallbacks aligned to the shipped labels ("Confluence"/"AI") | `app/host.rs` | `80fc3ba`, `5b8d432` |
| R4 / F11 actions | shared `file_action_open_in_notes` / `_copy_path` / `_copy_file` / `_reveal` helpers; `ai_rule_menu` / `notes_editor_menu` / `terminal_menu` reordered to the canonical open → copy → edit → navigation order; `vim_image_menu` "Copy Image Path" → "Copy Absolute Path" | `app/host.rs`, `views/notes.rs` | `f4911b1`, `5b8d432` |

Review-driven rework (`5b8d432`): the palette's no-label fallback still said "Confluence Search" /
"AI View" while the nav said "Confluence" / "AI" — changed the palette to match. And
`notes_editor_menu` showed two identically-labelled "Copy Absolute Path" rows when an image was
right-clicked (image + note); the note row now appears only when the click is not on an image.

**Known gap from R4:** the plan asked for the shared builder to be used by ≥5 surfaces; it is used by
3 (`ai_rule_menu`, `notes_editor_menu`, `terminal_menu`). `files_row_menu` and `compare_folder_menu`
are multi-path / disabled-row surfaces that the single-path helpers do not fit;
`notes_prose.rs show_context_menu` runs in the prose pop-out, where a reveal row is a no-op
(§0.4a). Left for a future round if the helpers grow multi-path support.

---

## 9. Where to change what (agent map)

| Change | File / symbol |
|---|---|
| Main menu bar rows | `app/menu.rs:600` `install_main_menu` (and pass a target from `main.rs:191`) |
| Status-item menu rows | `app/menu.rs:300` `build_menu_spec` + `MenuAction` `:22` + `main.rs:259` `status_menu_action` |
| Which views add header rows | `host.rs:9363` `view_icon_entries` |
| Theme / header menu block | `host.rs:5995` `header_icon_menu` |
| Any right-click menu | the `menu_kit` builder in `host.rs` (grep the literal title) |
| Popup framework / row kinds | `ui/menu_kit.rs:9` `MenuEntry`, `:279` `popup` |
| Palette rows | `commands.toml` (`enabled`, `in-palette`, `label`) + `host.rs:829` `build_palette` |
| ⌘/ sheet contents | `commands.toml:406` `[shortcuts]` + `ui/shortcuts.rs:91` `groups_for` |
| ⌘K action list | `host.rs:9984` `compare_actions`, `host.rs:10062` `jira_actions`, `ui/action_picker.rs` |
| Web context menu | `ui/quiet_web.rs:11-43` |
| Linux context menu | `frontend_egui/files.rs:77` `row_context_menu` |

**Verification:** menu/state changes can be checked over the socket without a UI
suite (`state` / `do:` — see the `ui-check` skill in `.claude/skills/ui-check/`); rendering
still needs `pane-shot` / `window-shot` per `AGENTS.md`.
