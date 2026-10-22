# App bugs found by the automated testing

The **single ledger** of real bugs in app features (not MCP/socket plumbing). Test workers read it
before a run so they don't re-report known items, and the orchestrator appends to it after triage
(`TESTING.md` §8). One row per bug.

- **Status:** `open` · `fixed <commit>` (the fix landed) · `verified <date>` (a replay or re-run after the
  fix passed) · `by-design?` (waiting on an owner decision) · `re-check` (code changed since the report, so
  the status is unknown; a worker may try it again and report "still reproduces" or "gone").
- **Evidence:** the run report that holds the repro trail. Reports live in `Tests/usability/`.

| # | Sev | Feature | Bug | Evidence | Status |
|---|---|---|---|---|---|
| 1 | S2 | Files | `**` in the Files filter ("search every folder below", `commands.toml:477`) shows nothing: the query parses as `QueryMode::Recursive` but the listing returned an empty `Vec` | flow `files-recursive-search`; `views/files.rs` (`QueryMode::Recursive`) | fixed 60b7e62 (night run, DeepSeek) + 400 ms search budget · verified 2026-10-21 (VM flow) |
| 2 | S2 | Notes / nvim | The 1 s `checktime` poll fell back to typed keys `<C-\><C-N>:…` when nvim RPC failed, which dropped insert mode and closed the completion popup ("intellisense randomly stops") | `hammer-2026-10-20/G1.md` | fixed e9ab3d5 · verified 2026-10-20 (`FINAL-soak.md` 2a, 100/100) |
| 3 | S2 | Prose pop-out | Pop-out showed alt text instead of images (loaded HTML with no base URL) | `hammer-2026-10-20/G2.md` | fixed e9ab3d5 · verified 2026-10-20 (`FINAL-soak.md` 2d, 5/5) |
| 4 | S2 | Notes sidebar | With ~10+ notes, clicking a sidebar row stopped switching notes (Return read `clickCount` on a key event) | `hammer-2026-10-20/G3-2-bisect.md` | fixed ceb8a53 · verified 2026-10-20 (`FINAL-soak.md` 2e, 85/85) |
| 5 | S3 | Notes drawer focus | After opening the drawer, Ctrl+J then Ctrl+K: state says `vimFocus:true`, but unicode text events reach neither nvim nor the shell until one real key event arrives; the responder stays the drawer's terminal view | `FINAL-soak.md` S-1 (5/5), `G3.md` G3-1 | open (impact on real typing unconfirmed) |
| 6 | S3 | Notes sidebar | `.md` files created on disk in the notes folder never appear in the Notes sidebar; Cmd+N and a view round trip don't rescan | `FINAL-soak.md` S-2, `G2.md` | fixed 89dca62: rescan on Notes re-open / note open · unit test |
| 7 | S3 | Notes / nvim | Cmd+= / Cmd+- (zoom) leaves nvim in Visual-line mode | `RUN-2026-10-20.md` item 5 (3/3) | fixed 9a96fec: a text event's key code 0 is no longer Cmd+A in the vim pane (`ui/popup.rs` `vim_pane_key` + `char_is`) · unit test `typed_text_is_not_select_all_in_the_vim_pane` |
| 8 | S3 | Prose pop-out | The pop-out never refreshes when the note changes on disk; clicking pop-out again is a no-op | `RUN-2026-10-20.md` item 5 | fixed c895e7d: `ProseProcess::launch` retires a live child for the note instead of returning early, so a re-click respawns a child that re-reads the file (`views/notes.rs` `replace_live_child`/`retire_child`) · tests `a_live_popout_for_the_same_note_is_replaced_on_relaunch`, `retiring_a_tracked_child_kills_it_so_a_relaunch_replaces_the_popout` |
| 9 | S3 | Notes | Renaming a note to an existing name silently does nothing (no error shown) | `RUN-2026-10-20.md` item 5 | fixed 8f30b1e: the "already exists" toast was shown but held only 1.4 s in the success tint, so it read as a no-op; error toasts (`exclamationmark…`) now hold 4 s in the warning tint (`ui/toast.rs` `hold_secs`) · unit test `error_toasts_hold_longer_than_info_toasts` · not yet re-checked on screen |
| 10 | S3 | Notes | There is no way to delete a note from the app | `RUN-2026-10-20.md` item 5 | by-design? |
| 11 | S1 | AI view | Clicking the AI header tab killed the app (`RefCell already borrowed`, `host.rs` `ai_surface`) | `RUN-2026-10-13.md` HDR-TAB-AI (2/2) | re-check: `do:open:ai` no longer crashes (flow `ai-open` PASS 2026-10-20); the header-tab *click* is untested, so lanes still must not click it |
| 12 | S3 | Menu bar | The Window menu opens no dropdown | `RUN-2026-10-13.md` MB-WINDOW-01 (2/2) | re-check |
| 13 | S4 | Compare | Re-running the same folder pair opens a second session instead of focusing the existing one | `FINAL-soak.md` C-4 | open |
| 14 | S4 | Compare | A sub-folder shows "same" although a file inside differs (equal size, compared by date/size only) | `FINAL-soak.md` C-5 | open |
| 15 | S4 | Compare | `j`/`k` don't move the folder-compare list (Down/Up do) | `FINAL-soak.md` C-1 | by-design? |
| 16 | S3 | Compare | Right after a text compare opens, Ctrl+N / Ctrl+P ("next / previous difference", `commands.toml` compare shortcuts) do nothing: focus is on the sidebar (`WSSidebarView`), not the compare panes. Real HID keys behave the same | flow `compare-next-diff` (run_flows 2026-10-20, 2/2 runs); real `ks-input key ctrl+n` too | fixed 5a55b0b (night run, DeepSeek) · verified 2026-10-21 (VM flow) |
| 17 | S3 | Socket | The first `do:` verb after a GUI launch replies at once but keeps the connection open ~25 s (later verbs close immediately); a client that waits for EOF stalls | `nc -w25` timing 2026-10-20; `Tests/run_flows.py` reads one line to work around it | fixed 8cf6fe4: the client fd leaked into children (nvim/helper) spawned mid-request; now FD_CLOEXEC · unit test |
| 18 | S3 | Compare | Ctrl+Opt+R / Ctrl+Opt+L ("copy line") copied the whole difference section | flow `n-compare-copy-line-only` (night 2026-10-20) | fixed 153c5d1 · verified (VM flow) |
| 19 | S2 | Compare | Return on a folder-compare file row opened the sub session without pushing the text sub-view, so Esc/back could not return to the folder | flow `n-compare-folder-return-sub` | fixed 4d51e9d · verified (VM flow) |
| 20 | S2 | Notes | Cmd+Z did nothing in the plain Notes editor (undo stack never enabled) | flow `n-notes-undo-after-save` | fixed d6c9974 · verified (VM flow) |
| 21 | S2 | Notes | With the Cmd+F find bar focused, Cmd+S then Esc hangs the app: every socket verb times out (20 s) afterwards, so the main loop looks blocked (a modal?) | flow `n-notes-find-bar-not-in-note`, step 12 `key esc`, VM 2026-10-21 (2 of 3 runs; the 3rd failed to activate) | fixed d77209a: `state` reads the screenshot/compare controllers with `try_lock`, OCR prewarm runs outside the mutex · unit test · VM flow PASS |
| 22 | S2 | Files | Typing at the list starts the filter one character short: `i` / `a` are the vim engine's insert command (`panes/vim_keys.rs`), so they focused the filter and were swallowed before the list's type-ahead (`host.rs` `files_key`) could insert them — typed `abc` gave query `bc` | flow `n-files-new-folder-esc-cancel` (VM, `views.files.query = "bc"`); probe: list focused + one `a` -> `pane.focused = files-filter`, `query = ""` | fixed bc1dcdc: `panes/vim_keys.rs` `typeahead_before_vim` + `route_key_event` gives the Files type-ahead `i` / `a` first (`j` / `k` / `G` and every other vim key unchanged) · unit test `typeahead_beats_vim_insert_letters` · verified 2026-10-21 (VM flow PASS) |
| 23 | S4 | Paths shelf | `state.paths` / `state.tools.paths` kept `shown:true` after a key-driven close (Esc / Return / click-off) | flow `n-shell-paths-esc-chain` | fixed 4f85ac6: republished every drain · unit test |
| 24 | S2 | Config | A setting write against a broken `commands.toml` replaced it with a stub and clobbered the last-good `.bak` | night lead (h-cfg, 2026-10-20) | fixed 77a42f0: writes refuse a broken live file · unit test |
| 25 | S2 | Config | `reload` returned ok for invalid configs, partly applied fatal errors, kept deleted keys' stale values | night lead (h-cfg, 2026-10-20) | fixed 349a2dc: fatal/non-UTF-8 config rejected + parked as `.broken-<epoch>`, reload parses into fresh defaults · unit test |
| 26 | S2 | Compare | Folder rows below the top level had case-folded ancestor paths, so select-by-real-path failed | night lead (h-cmp, 2026-10-20) | fixed 8b3b743 + b2378b4 (keys stay folded) · unit test |
| 27 | S2 | Compare | A pair past `[compare] max-bytes` was reported identical even when the bytes differ | night lead (h-cmp, 2026-10-20) | fixed e492a88: streamed byte compare from disk · unit test |
| 28 | S2 | Shell | Ctrl+Tab only toggled Files <-> Notes (`host.rs` `cycle()` ignored `dir`); PRODUCT.md:42 says it cycles the views | `.build/haiku/20261021-2242/HAIKU.md` (2/2) | fixed 675bf70: `cycle_target` walks the header nav icons both ways · unit test `cycle_walks_the_header_views_both_ways` · flow `cycle-views-all-header-views` · verified 2026-10-22 (VM flows + ks-drive replay) |
| 29 | S2 | Jira | After Esc (focus on the list), Cmd+L did not focus the search box and typed text was dropped: Jira never bound Cmd+L | `.build/haiku/20261021-2242/HAIKU.md` (2/2) | fixed 6c24cbb: `host.rs` `jira_key` Cmd+L -> `live_pane_focus("search")` · flow `jira-cmd-l-after-esc` · verified 2026-10-22 (VM flow + ks-drive replay) |
| 30 | S2 | Compare | Start page: Tab in the Left path box did not move to the Right one (no key-view loop), so the second path replaced the first | `.build/haiku/20261021-2242/HAIKU.md` (2/2) | fixed e172144: `compare_view.rs` `do_command` moves Left <-> Right on Tab / Shift+Tab · flow `compare-start-tab-moves-to-right` · verified 2026-10-22 (VM flow + ks-drive replay) |

## Leads (unverified: headless hunters, night 2026-10-20, each 3/3 by the worker)

Full repros: `.build/night/20261020-2246/find/h-cmp|h-cfg/findings.jsonl`. Confirm before fixing.

- S2 compare: folder entries undecidable by size+time stay "unknown" forever (the content check is dead code).
- S3 config: a UTF-8 BOM drops the whole `[app]` section; TOML arrays accepted as scalars; palette-first with a repeated id shows the command twice.
- S3 compare: `max-lines`, `exclude` / `use-gitignore` / `ignore-file` are not enforced.

## Leads (unverified: Haiku agents driving the real app in the VM, 2026-10-21, each 2/2+ by the agent)

Full steps (`bin/ks-drive` commands) and screenshots: `.build/haiku/20261021-2242/<area>/`. Confirm before fixing.

- S2 shell: the Ctrl+B W switcher list wraps after 4 entries; Compare and AI can't be reached from it.
- S2 compare: folder compare's Diffs / Same filter pills highlight but don't filter the rows.
- S2 compare: clicking a folder-compare row doesn't select it (Return still acts on the old row); clicking an OPEN session in the sidebar doesn't switch to it.
- S2 compare: the start page's path fields keep the previous text, so retyping appends and compares a nonexistent path as "(no differences)" with no error.
- S2 files: after Return into an empty folder, state says cwd=empty/rowCount 0 but the window still draws the parent's list and path (may be a stale redraw or a capture artifact: check on the real screen).
