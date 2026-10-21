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
| 1 | S2 | Files | `**` in the Files filter ("search every folder below", `commands.toml:477`) shows nothing: the query parses as `QueryMode::Recursive` but the listing returned an empty `Vec` | flow `files-recursive-search`; `views/files.rs` (`QueryMode::Recursive`) | re-check (files.rs changed since 2026-10-13) |
| 2 | S2 | Notes / nvim | The 1 s `checktime` poll fell back to typed keys `<C-\><C-N>:…` when nvim RPC failed, which dropped insert mode and closed the completion popup ("intellisense randomly stops") | `hammer-2026-10-20/G1.md` | fixed e9ab3d5 · verified 2026-10-20 (`FINAL-soak.md` 2a, 100/100) |
| 3 | S2 | Prose pop-out | Pop-out showed alt text instead of images (loaded HTML with no base URL) | `hammer-2026-10-20/G2.md` | fixed e9ab3d5 · verified 2026-10-20 (`FINAL-soak.md` 2d, 5/5) |
| 4 | S2 | Notes sidebar | With ~10+ notes, clicking a sidebar row stopped switching notes (Return read `clickCount` on a key event) | `hammer-2026-10-20/G3-2-bisect.md` | fixed ceb8a53 · verified 2026-10-20 (`FINAL-soak.md` 2e, 85/85) |
| 5 | S3 | Notes drawer focus | After opening the drawer, Ctrl+J then Ctrl+K: state says `vimFocus:true`, but unicode text events reach neither nvim nor the shell until one real key event arrives; the responder stays the drawer's terminal view | `FINAL-soak.md` S-1 (5/5), `G3.md` G3-1 | open (impact on real typing unconfirmed) |
| 6 | S3 | Notes sidebar | `.md` files created on disk in the notes folder never appear in the Notes sidebar; Cmd+N and a view round trip don't rescan | `FINAL-soak.md` S-2, `G2.md` | open (owner 2026-10-20: they should appear) |
| 7 | S3 | Notes / nvim | Cmd+= / Cmd+- (zoom) leaves nvim in Visual-line mode | `RUN-2026-10-20.md` item 5 (3/3) | open |
| 8 | S3 | Prose pop-out | The pop-out never refreshes when the note changes on disk; clicking pop-out again is a no-op | `RUN-2026-10-20.md` item 5 | open |
| 9 | S3 | Notes | Renaming a note to an existing name silently does nothing (no error shown) | `RUN-2026-10-20.md` item 5 | open |
| 10 | S3 | Notes | There is no way to delete a note from the app | `RUN-2026-10-20.md` item 5 | by-design? |
| 11 | S1 | AI view | Clicking the AI header tab killed the app (`RefCell already borrowed`, `host.rs` `ai_surface`) | `RUN-2026-10-13.md` HDR-TAB-AI (2/2) | re-check (AI code changed in 7e86a00); until re-checked, lanes must not click the AI tab |
| 12 | S3 | Menu bar | The Window menu opens no dropdown | `RUN-2026-10-13.md` MB-WINDOW-01 (2/2) | re-check |
| 13 | S4 | Compare | Re-running the same folder pair opens a second session instead of focusing the existing one | `FINAL-soak.md` C-4 | open |
| 14 | S4 | Compare | A sub-folder shows "same" although a file inside differs (equal size, compared by date/size only) | `FINAL-soak.md` C-5 | open |
| 15 | S4 | Compare | `j`/`k` don't move the folder-compare list (Down/Up do) | `FINAL-soak.md` C-1 | by-design? |
