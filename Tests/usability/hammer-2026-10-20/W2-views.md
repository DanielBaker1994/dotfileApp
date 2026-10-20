# W2 - non-Notes views hammer (Files, Jira, Confluence, Compare), 2026-10-20

Sandbox pid 62192, real keys/mouse via ks-input + osascript + a paced CGEvent tool. Oracles: socket `state`, disk, daemon logs, curl to fake servers. Shots: shots/W2-*.png.
Fixtures (all under /private/tmp/kssb/home): w2/files, w2/huge (5000 files), w2/cmp/{a,b}; Jira: fake_jira_tab.py 150 issues -> .cache/kitchen-sink/jira_json/all.json plus the board fixture; Confluence: fake_confluence.py on 127.0.0.1:8765 + .config/confluence/config.json. No real network or credentials.
Cleanup note: fake Confluence server (pid in home/.cache/confluence/fake.pid) is still running; left alone (never kill rule).

## Findings

### W2-1 · S2 · 4/4 · Compare folder session: keyboard does nothing (j/k/Down/Up/Return)
Steps: Compare, Left=/private/tmp/kssb/home/w2/cmp/a, Right=.../b, Return -> folder list (4 rows). Click the list (focus ring appears), press j j / Down Down / Return.
Expected: cursor moves, Return opens the f.txt diff (PARITY says keys exist). Actual: state.compare.folder.cursor stays 0, nothing opens. Double-click (real clickCount 2) on the f.txt row also did not open it 1/1.
Socket `do:compare:folder-open` opens the diff correctly, so the model works; the real key/mouse routing to `CompareWindow::handle_key`/`folder_key` (views/compare.rs:4078,4136) is not reached from the AppKit event path.
Evidence: D04, D06, D07.

### W2-2 · S3 · 2/2 · Compare file diff: j / n keys do not move (cursorRow stayed 1); Cmd+W does not close the session
After `do:compare:folder-open` (f.txt diff shown), j j and n n left cursorRow at 1. Cmd+W left 2 sessions. `do:compare:close-all` works and returns to the start page, recent=2. Same routing suspicion as W2-1.

### W2-3 · S3 · 2/2 · Files: Esc/Esc does not clear the filter while the list has focus
Type `beta` in the filter, Esc, Esc (also a real key code 53): query stays 'beta', focus moves to the list. Esc only clears while the filter field has focus (files.rs:1630 handles Escape in the field delegate only). Expected per the Files keys: Esc clears the query. Needed Cmd+A + Delete to clear.

### W2-4 · S3 · 3/3 · Quick Look opens via context menu but Space does not toggle it from the list
Space on a selected row: nothing, 3/3 (once typed into the filter instead). Context menu Quick Look opens the panel (816x849, keyWindow ""); Esc closes it 2/2 (FIXED vs 10-13), state.quickLook empties. Space close while up: not exercised cleanly.

### W2-5 · S4 · 1/1 · Jira: Return / "Open full detail" in the list does nothing without a site
Selecting YOUR-3 and pressing Return or clicking Open full detail: no window, no toast (host.rs:10308 only opens a URL when site configured). No feedback explaining why.

### W2-6 · S4 · 1/1 · Jira board with no project scope shows "sprint filter failed - showing all: no projects in scope" truncated in header; ok but cut off
Cosmetic, header text truncated with ellipsis (B12).

### W2-7 · S4 · 2/2 · Zoom-key/Cmd+0 in Files: Cmd+= zoom works (rows grow, A37) but Cmd+0 (key 29) leaves zoom (A38), known G3-8 reconfirmed 1/1.

### W2-8 · S4 · harness · ks-input activate sometimes reports failure while app is frontmost; a macOS "Keychain Not Found" SecurityAgent dialog (pid 63899) appeared mid-run (triggered by Confluence token handling in the sandbox HOME) and made ks-input refuse key events for a while. Not an app bug per se, but the app tries the macOS keychain when saving the Confluence token (confluence setup Test & Save) in a sandbox HOME with no keychain; worth checking that it falls back to the file.

## Cases (PASS/FAIL)
Files
1. Arrows/j/k nav in sidebar+list, Return opens folder: PASS
2. Filter typing narrows (alpha, beta, f049 -> 100 of 5000): PASS
3. Return on file/folder navigation incl. path typed in filter: PASS (typed literal path did not navigate until list Return; ok)
4. Space Quick Look from list: FAIL (W2-4)
5. Quick Look Esc close (retest of 10-13): PASS 2/2
6. F2 rename (x.txt, disk verified): PASS
7. Cmd+Shift+N new folder (retest): PASS (creates "untitled folder" in rename field; typed newdir + Return, disk verified)
8. Cmd+Delete trash (retest): PASS (beta.txt moved to home/.Trash? found only 'beta copy.txt' in .Trash; beta.txt removed from folder, Cmd+Z restored it, disk verified)
9. Cmd+K copy path: PASS (pasteboard = /private/tmp/kssb/home/w2/files/beta.txt)
10. Cmd+D duplicate: PASS ("beta copy.txt" appeared <2.5 s)
11. Cmd+Z undo: PASS (duplicate removed; trash restored)
12. Context menu: right-click appears in ~0.4 s (retest lag): PASS; Rename... via menu: PASS (name field opens; retest of 10-13 fail); keyboard in menu ok
13. Sort asc/desc toggle, Pin toggle: PASS (files-favorites.json updated, sidebar shows pin)
14. 5000-file dir: open instant, filter f049 shows 100 items, scroll ok, CPU 5-9%: PASS
15. Unicode name "Café 日本.txt" lists/previews: PASS
16. Cmd+= zoom: PASS; Cmd+0 reset: FAIL (W2-7)
17. Esc clears filter from list focus: FAIL (W2-3)
Jira
18. List tab loads 150 fixture issues, sidebar click, row nav j/k/Down, preview pane follows: PASS
19. Search box typing filters (blocke -> 26, matches ground truth count of 'blocked'): PASS
20. Cmd+F opens the search/filter panel (Assignee, Labels, Search/Fetch buttons): PASS
21. Boards list, open Demo board, Columns and Table modes, counts 2/1/1/1 match fixture: PASS
22. Open full detail / Return: FAIL-ish S4 (W2-5)
23. Header/inspector divider drag (paced tool): PASS (inspector 273.8 -> 393.8 and back to 273.8)
24. Rapid view switching 20x header clicks across Files/Jira/Confluence/Compare: PASS 0 misses; plus 35 do:open cycles: PASS, process alive
Confluence
25. Unconfigured state + Set Up dialog (Enter/Test & Save) against fake: PASS (connected)
26. Search "deploy": 25 results, matches API ground truth (25), preview with highlights, "1 of 4" match counter; Down moves selection; Cmd+G next match: PASS
27. Star (Cmd+D) persists to config.json favorites, Starred list shows it: PASS
28. Cmd+C copy link: not verified (pasteboard unchanged by shell check); skipped
29. Search "eng" returns 0 though 8+ ENG pages exist; fake server's text~ search returns 0 for "eng" (server semantics), not an app bug: n/a
Compare
30. Start page, typing two folder paths, Tab, Compare button: PASS; folder rows match ground truth (1 different, 1 left only, 1 right only, 1 same)
31. Diff view vs difflib: PASS (line 2 two/2 changed, line 7 seven added; 2 sections)
32. Keyboard in folder list: FAIL (W2-1); double-click open: FAIL
33. Diff nav keys j/n, Cmd+W close: FAIL (W2-2); close-all and recent (2) OK via socket: PASS
Cross
34. daemon.log and home/logs: 0 panicked / already borrowed; process alive at end; no stuck Cmd (typing after osascript combos behaved): PASS. One ks-input refusal episode due to SecurityAgent (W2-8).

## Coverage
Files A: nav, filter, Return, QL, F2, Cmd+Shift+N, Cmd+Delete, Cmd+K, Cmd+D, Cmd+Z, context menu, sort, pin, 5000 files, unicode, zoom. Not done: favorites reorder, Cmd+Y.
Jira B: list, search, Cmd+F panel, boards, columns/table, divider drag, switching. Not done: filters dropdowns, keyboard-only board card nav (h/l/j in columns did not change anything visible, unverified), issue detail (no site).
Confluence C: setup, search, preview, match nav, star, starred list. Not done: back/forward (no such UI found), Cmd+Return open in browser (would open a browser).
Compare D: start page, folder compare, file diff, close, recent. Not done: text compare, edit/sync actions.
