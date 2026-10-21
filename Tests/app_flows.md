# tests/app_flows — feature-flow oracle table

`Tests/app_flows.json` is a table of **feature flows** for the runner to drive
against a sandboxed **windowed** kitchen-sink (Rust port) with **real** keyboard
and mouse input through `bin/ks-input`. The point is to find real bugs in the
app's features, not in the MCP/socket plumbing: every expectation cites the
project's own docs or code as `file:line`, and every flow asserts at least one
**non-log** oracle (state, disk, or an independent recomputation).

This table validates the app, not the plumbing. See `TESTING.md`
for the test strategy and how workers run these flows; the UI suites were deleted by owner decision
(AGENTS.md "Tests").

## Runner contract

- **Sandbox HOME.** Flows assume the launcher's sandbox HOME is
  `$SANDBOX/home` (see `bin/ks-mcp-sandbox.sh:119-153`, which sets
  `HOME`/`WS_HOME`/`XDG_*` under the sandbox). Every `$HOME` in the JSON expands
  to that sandbox HOME, so `~/notes` is `$SANDBOX/home/notes`. Paths in `disk`
  oracles are relative to the sandbox HOME.
- **Fixtures.** Create everything in `fixtures` before the run. The one
  `"kind":"config"` fixture (`cfg-sandbox`, `"applies_to":"all"`) is
  **load-bearing**: apply it to `$SANDBOX/home/commands.toml` before start. It
  sets three known open notes (`[notes] paths`), a fixed Files root
  (`[files] root`/`start`), and `[notes] vim-mode = false`. Without it
  `notes-open-tabs` and `files-open-listing` will fail (no tabs / wrong root)
  and typing into the notes editor would require an external nvim.
- **`do:` verbs.** `setup` contains only socket `do:` verbs to reach the start
  state (`do:open:notes`, `do:pane:focus:…`, …). For features that have no
  key/mouse entry, a step may itself be `{"input":"do:…"}`; these are marked in
  the flow and are still real state transitions through `do_host_action`.
- **`raw` steps.** `config-reload` uses `{"input":"raw reload"}` (the raw socket
  verb `reload`), since reload has no key.
- **`needs`.** `"gui"` always; `"screen-capture"` for `screenshot-show-state`
  (the runner skips it when Screen Recording is unavailable). No flow needs
  `"network"`, `"herdr"` or `"jira-creds"` — the Jira/AI/Confluence flows are
  deliberately offline.
- **Absent retries.** A flow ends when its oracles are read; `wait` steps are
  the only synchronisation (there is no retry loop).
- **Checkpoints.** `after` on an oracle names a step's `checkpoint`; with no
  `after` the oracle reads the end state.

### Oracle kinds (schema is in the JSON `schema` block)

- `state` — read the daemon `state` JSON: `equals`, `not_equals`, `min`,
  `contains` (array/string), `contains_name` (a `rows[].name` match), `length`.
- `disk` — a path or `glob` relative to the sandbox HOME: `exists`, `contains`,
  `is_dir`, `count_min`.
- `independent` — a recomputation the runner performs itself (a direct `ls`, a
  Python `difflib` count, or the body of a `do:` reply). This is the strongest
  kind; used wherever `state` is a placeholder.
- `visual` — a screenshot at `after`; a vision model decides. Used only where no
  state/disk check can see the thing (overlays with no state, the focus ring).
- `log` — the sandbox `daemon.log` must contain none of a list; every flow ends
  with `{"kind":"log","none_of":["panic","ERROR"]}`.

### Known limitations the runner must expect

- `views.jira`, `views.confluence` and `views.ai` are **placeholder-only** over
  the socket (the live model `test_state`s are never pushed; only
  `views.notes`/`views.files` are). Jira/AI flows therefore assert `view`/
  `shown` plus a visual or independent check.
- `views.notes.query` is always `""` (seeded at `host.rs:497`, never updated), so
  note-find match counts are not assertable.
- `compare` exposes no per-session `differences` count; the runner recomputes one
  (`compare.current.sections` / `compare.current.important`) with `difflib`.
- The `Cmd+K` action picker and the `Cmd+/` shortcuts card have **no `state`
  representation** (host.rs:539-570) — visual oracles only.

## Flows

45 flows, by feature: notes 6 · files 7 · compare 6 · view-switcher 2 ·
view-cycling 1 · header-styles 1 · shortcuts-sheet 1 · action-picker 1 ·
esc-hides 1 · hide-on-focus-loss 1 · terminal-drawer 1 · browser-drawer 1 ·
pane-navigation 2 · vim-keys 1 · tools 4 · paths-shelf 2 · config-view 2 ·
output-view 1 · jira 2 · screenshot 1 · ai 1.

| id | feature | promise | needs |
| --- | --- | --- | --- |
| `notes-open-tabs` | notes | Opening Notes shows the configured open notes as sidebar tabs. | gui |
| `notes-new-note-file` | notes | Cmd+N creates a new empty note file on disk in the notes folder. | gui |
| `notes-rapid-new-notes` | notes | Repeated Cmd+N makes a fresh note each time and never reuses a name (no data corruption). | gui |
| `notes-type-save` | notes | Text typed into a new note is saved to disk on Cmd+S. | gui |
| `notes-unicode-save` | notes | Unicode text in a note survives the round-trip to disk (UTF-16 range math, not byte indices). | gui |
| `notes-find-toggle` | notes | Cmd+F opens the note find bar; Esc closes it before the window's own Esc policy runs. | gui |
| `files-open-listing` | files | The Files view lists the folder named by [files] root. | gui |
| `files-filter-local` | files | Typing in the Files filter narrows the list to names that match. | gui |
| `files-recursive-search` | files | In the Files filter, ** searches every folder below the current one and lists the matches. | gui |
| `files-open-dir` | files | Return opens the selected folder and the view re-roots there. | gui |
| `files-rename` | files | Cmd+R renames the selected file, and a name with a space and an accent is accepted. | gui |
| `files-new-folder` | files | Cmd+Shift+N creates a new folder in the current folder. | gui |
| `files-hidden-toggle` | files | Cmd+Shift+. shows hidden (dot) files in the list. | gui |
| `compare-text-open` | compare | Opening two text files shows a text compare with their differences. | gui |
| `compare-text-diff-count` | compare | The differences the app shows match an independent line diff of the two files. | gui |
| `compare-next-diff` | compare | Ctrl+N jumps to the next difference and Ctrl+P back to the previous one. | gui |
| `compare-filter-switch` | compare | Cmd+2 shows only differences; Cmd+1 shows all rows. | gui |
| `compare-swap-sides` | compare | Cmd+Opt+X swaps the left and right sides. | gui |
| `compare-folder-open` | compare | Two folders open as a folder compare with per-status counts. | gui |
| `switcher-open` | view-switcher | Ctrl+B W opens the view switcher listing the open windows. | gui |
| `switcher-close-esc` | view-switcher | Esc closes only the switcher overlay, leaving the window under it visible (rule 6). | gui |
| `cycle-views` | view-cycling | Ctrl+Tab cycles the shared window's views and back. | gui |
| `header-style-set` | header-styles | Header styles change live; an unknown style is refused. | gui |
| `shortcuts-sheet-visual` | shortcuts-sheet | Cmd+/ shows the shortcuts card for the current view. | gui |
| `action-picker-visual` | action-picker | Cmd+K opens the action menu in Jira (it is wired only for Jira and Compare). | gui |
| `esc-hides-on` | esc-hides | With Esc Hides on for notes, one Esc hides the window. | gui |
| `hide-on-focus-loss-state` | hide-on-focus-loss | Hide On Focus Loss reflects the setting and can be flipped at runtime. | gui |
| `terminal-drawer` | terminal-drawer | The notes terminal drawer toggles open and closed. | gui |
| `browser-drawer` | browser-drawer | The notes file-browser drawer toggles open and closed. | gui |
| `pane-focus` | pane-navigation | A pane can be focused by id and the state reports which pane has the keys. | gui |
| `pane-move` | pane-navigation | Ctrl+H/J/K/L moves the focus ring between panes and the opposite key returns. | gui |
| `vim-list-keys` | vim-keys | In normal mode j/k move the list selection and gg/G jump to the ends. | gui |
| `tool-prettyprint` | tools | The Pretty Print tool panel opens and reports itself shown. | gui |
| `tool-text-table` | tools | The Text to Table panel opens and reports itself shown. | gui |
| `tool-filefast` | tools | The Create File (filefast) panel opens and reports itself shown. | gui |
| `tool-health` | tools | The Health Checks output panel opens and reports itself shown. | gui |
| `paths-show` | paths-shelf | The paths shelf opens and reports itself shown. | gui |
| `paths-select-out-of-range` | paths-shelf | Selecting a paths row past the end is refused, not silently accepted (an edge case of the shelf's selection). | gui |
| `config-open` | config-view | The config view can be opened and the state reports it as the current view. | gui |
| `config-reload` | config-view | Reload re-reads commands.toml and resets a runtime-only change. | gui |
| `output-open` | output-view | The output view can be opened and the state reports it as the current view. | gui |
| `jira-open` | jira | The Jira view opens with a list surface, without any network access. | gui |
| `jira-board-error` | jira | Board verbs are advertised but must not silently succeed (known open bug). | gui |
| `screenshot-show-state` | screenshot | The screenshot overlay opens with the documented toolbar buttons. | gui, screen-capture |
| `ai-open` | ai | The AI view opens even when the on-device model is unavailable. | gui |

## Fixtures the flows rely on

All paths are relative to the sandbox HOME. The runner creates these before the
run (`bin/ks-mcp-sandbox.sh` reseeds a missing `commands.toml` from the repo, so
`cfg-sandbox` must be applied after that copy).

| fixture | path | used by |
| --- | --- | --- |
| notes folder + 3 notes | `notes/Alpha.md`, `notes/Beta.md`, `notes/Quill.md` | all `notes-*` |
| Files root tree | `files/` with `README.txt`, `spaced name.txt`, `.hidden.txt`, `docs/reading.md`, `notes/deep.txt`, `empty/` | all `files-*` |
| compare text pair | `compare/textL.txt`, `compare/textR.txt` | `compare-text-*`, `compare-next-diff`, `compare-filter-switch`, `compare-swap-sides` |
| compare folder pair | `compare/left/{a.txt,common.txt,only_left.txt}`, `compare/right/{a.txt,common.txt}` | `compare-folder-open` |
| identical pair | `compare/identicalL.txt`, `compare/identicalR.txt` | reserved (identical-file edge; not yet wired to a flow) |
| jira cache dir | `.cache/kitchen-sink/jira_json/` (empty is fine for `jira-open`) | `jira-*` |
| config tweak | `commands.toml` (`cfg-sandbox`) | every flow |

## Edge cases covered (one or more per feature)

| feature | edge | flow |
| --- | --- | --- |
| notes | rapid repeated Cmd+N (name reuse / selection corruption) | `notes-rapid-new-notes` |
| notes | unicode text round-trip | `notes-unicode-save` |
| files | filename with a space and an accent | `files-rename` |
| files | hidden (dot) files | `files-hidden-toggle` |
| files | recursive filter `**` across nested folders | `files-recursive-search` |
| compare | many-vs-few line changes grouped into hunks | `compare-text-diff-count` |
| header-styles | invalid style value refused | `header-style-set` |
| paths-shelf | out-of-range selection refused | `paths-select-out-of-range` |
| jira | unimplemented board verb must error, not no-op | `jira-board-error` |
| config-view | runtime-only change undone by reload | `config-reload` |

## Flows that intentionally hit a known open bug

These are expected to FAIL today and lock the documented gap (the runner should
report them as known, not new):

- `files-recursive-search` — `[shortcuts]` promises `**` searches every folder
  below (commands.toml:477) but the Rust model maps recursive queries to an empty
  list (`rust/ws-rs/src/views/files.rs:1592`, comment at :1569-1570).
- `jira-board-error` — `do:board:*` is advertised but always errors
  (BACKLOG.md:72-75, host.rs:1915-1918).
- `terminal-drawer` (visual half) — the drawer's top border clips the first text
  row and there is no resize handle (BACKLOG.md:21-24).

## Features found but not specified (and why)

- **Notes rename.** An inline-rename model exists for the notes tree
  (`rust/ws-rs/src/views/notes.rs:4836`, `:5740`), but no keyboard entry is wired
  for notes in `route_notes_key`; the only rename keys are the Files view's
  (commands.toml:462). Rename coverage lives in `files-rename` instead.
- **Notes grep / notes-find popup (`Space s g` / `Space s f`).** These are
  nvim-pane leader keys (vim/init.lua:69-72 → raw `notes-find`/`notes-grep`);
  they open floating panels with **no socket state** (BACKLOG.md:87-90) and need
  the nvim pane (`Space i` insert picker etc.). They conflict with the
  deterministic `vim-mode = false` config, so they are out of this table; a
  second run with `[notes] vim-mode = true` + nvim would be needed.
- **Notes vim pane (`:w`, multi-cursor Ctrl+Shift+J/K).** Same reason: needs the
  nvim pane, which the deterministic config disables.
- **Notes prose / reading view (`Cmd+Shift+P`) and PDF export (`Cmd+P`).** The
  toggle has no `state` field (`prose_shown` is internal, host.rs:5035); PDF
  export also needs pandoc. Visual-only and lower value than the flows above.
- **Notes very-long-text edge.** No size cap, test or documented behaviour was
  found in `notes.rs`/`notes_vim.rs`, so there is nothing to assert against.
- **Files Quick Look (`Space`), Trash (`Cmd+Delete`), Duplicate (`Cmd+D`),
  copy/cut/paste and undo, and the `term` filter.** Not specified to keep the
  table bounded; `quickLook`'s state shape was not confirmed. The `term` filter
  is a candidate bug: commands.toml:478 promises it opens a shell, but no
  `QueryMode::Terminal` handling exists outside the model.
- **Files Tab path completion.** commands.toml:459 promises Tab completes the
  typed path, but Tab is mapped to list-selection wrap (popup.rs:649); this looks
  unported — a candidate bug, listed rather than asserted because the expected
  behaviour is genuinely ambiguous.
- **Compare edit + save, find (`Cmd+F`/`Cmd+G`), go-to-line, folder
  next/prev/sync/copy-move chords, `Cmd+K` folder menu.** The compare key handler
  covers next/prev, filters, swap, save, copy and undo
  (`rust/ws-rs/src/views/compare.rs:4067-4255`); the remaining documented rows
  (commands.toml:495-536) were left for a follow-up. Compare find looks
  unported (`current.find` is hardcoded `false`, compare.rs:4039).
- **Palette (Hyper+S), `Cmd+K` picker rows, right-click menus.** Transient
  non-activating panels; the palette has a `state.palette` flag but the pickers
  and menus have no state (BACKLOG.md:87-90). Only `action-picker-visual`
  touches this area.
- **Jira `Cmd+F` live search, boards, releases, detail, Jira Config, Confluence
  search, AI runs.** `Cmd+F` in Jira currently shows a "not ported" toast
  (host.rs:8153-8156); Confluence search needs network/creds and
  `do:confluence:*` is dead code (BACKLOG.md:77-80); AI runs need Apple's
  on-device `fm`. All have placeholder-only socket state.
- **Screenshot annotation editing, OCR Copy Text, recents grid.** The recents
  grid has no state field or verb (BACKLOG.md:82-85); the annotation flows all
  capture the real screen.
- **Window-resize shortcuts (Ctrl+Shift+HJKL / Cmd+Opt+±).** Untestable under
  AeroSpace tiling (BACKLOG.md:41-46).
- **Focus-loss live hide.** The setting is assertable
  (`hide-on-focus-loss-state`) but the live behaviour is a documented unported
  gap (AGENTS.md:470-471).
- **Voice notes / dictation, ws-settings (`Hyper+/`), parity checker.** Out of
  scope for app-feature flows (separate CLIs / long-running jobs).
