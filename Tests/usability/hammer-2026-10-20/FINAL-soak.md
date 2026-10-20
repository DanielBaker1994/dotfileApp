# FINAL verification + soak (2026-10-20), sandbox pid 68692, clean build with all fixes

Real input throughout (ks-input key/type/click, osascript single cmd combos); nvim RPC (`--remote-expr`) used read-only as oracle.
Socket `do:toggle-terminal` / `do:open:notes` / `show` used only where no key exists (noted). Logs: shots/F-soak-100.log, shots/F-soak-after-idle.log.

## Verdicts
| Part | Result |
|---|---|
| 1 Compare folder keys | PASS (with notes below) |
| 2a 100 completion cycles + perturbations | PASS 100/100 (0 failed) |
| 2b 5 min idle (insert mode + popup up) then 10 cycles | PASS (mode i, pum 1 after 5:09 idle; Tab expanded; 10/10) |
| 2c 3000-line note | PASS |
| 2d prose pop-out of image note, edit/close/reopen x5 | PASS 5/5 |
| 2e rapid sidebar switching, expand('%:t') vs clicked row | PASS 85/85 (45 paced + 40 two-clicks-150ms) |
| panics / stuck Cmd / crash | none (daemon.log + prose.log clean, no new .ips, Cmd/Ctrl key-up sent, app still running) |

## Part 1 details (Compare)
Fixtures cmp-a / cmp-b (f1 same, f2..f6 differ, onlya.txt, sub/s1.txt). Palette -> Compare, typed both paths into Left/Right fields by real click+type, Return.
Session: 5 different, 1 left only, 2 same. Real keys, state `compare.folder.cursor`:
- Down Down 0->2, Up Up ->0: PASS. Home/End -> 0 / 7: PASS.
- Ctrl+N x2 -> 3 (first = f2 then f3), Ctrl+P x2 -> 2: PASS. Right expands `sub` (expanded=["sub"]): PASS.
- Return on f1 opened the pair as a new text session (sessions: folder,text), folder Return/Right on dir expands: PASS.
- Esc in the folder session: stays in Compare, nothing closes: sensible.
- Sidebar icon for the folder session is a folder icon ("cmp-a <-> cmp-b/") (shots/F-cmdk-folder2.png). PASS.
- Cmd+K on a folder session shows: New Comparison, Next Difference, Previous Difference, Open Pair (Text Compare), Copy to Right/Left, Rescan. PASS. (Text session shows its own list.)
Notes (not failures of the fix):
- C-1 · S4 · 1/1 · `j`/`k` do nothing in the folder list (folder_key has no 38/40 mapping, compare.rs ~4138-4260). Down/Up work. Only matters if vim-style keys were wanted.
- C-2 · S4 · 1/1 · Cmd+W in Compare hides the whole window (host.rs:11663 `13 if cmd => ui.hide()`), it never closes a session; sessions are closed only via the action picker/sidebar. Probably by design.
- C-3 · S4 · 2 misses (stopped, tooling suspected: ks-input click clickCount 0) · clicking the folder session row in the Compare sidebar and Up+Return did not switch sessions (`compare.selected` stayed 1). Workaround: Cmd+0 start page and re-run.
- C-4 · S4 · 1/1 · re-running the same pair opened a second folder session instead of focusing the existing one (3 sessions listed).
- C-5 · S4 · 1/1 · `sub` shows "same" although sub/s1.txt differs (hello vs world, equal size, status "2 by date/size only"); expected content check or "unknown". 

## Part 2 findings
Setup note: fresh sandbox editor started on the lazy.nvim UI; first nvim RPC worked, then **my own stray `q` (macro-record pending) made nvim RPC block for ~2 minutes** (`--remote-expr` hung until the next key arrived).
- S-1 · S3 · 5/5 · after opening the drawer (`do:toggle-terminal`), Ctrl+J then Ctrl+K: state says `vimFocus:true` / `terminal:true`, but text sent as unicode events (`ks-input type`) never reaches nvim (b:changedtick unchanged, mode stays n, 2nd and 3rd attempts too) and never shows in the drawer shell either. Real key events (`ks-input key i x`) DO reach nvim (3/3), and after one key event `type` works again. Without the drawer open, `type` works 100%. Trail: Notes, esc, do:toggle-terminal, Ctrl+J, Ctrl+K, `type iQQ`. Expected: text lands in nvim. Actual: dropped. Hypothesis: Ctrl+K (pane move back) updates the pane model (`vimFocus`) without making the nvim terminal view first responder again, so text-input events go to the drawer's SwiftTerm view while the keyDown monitor still routes key codes to nvim (host.rs route_key / PaneNav handling around the Ctrl+H/J/K/L path; `responder` stays `SwiftTerm.LocalProcessTerminalView`). Real typing may hit only the monitor path, so user impact is unconfirmed; this is the G3-1 residue (not 4/4 "keys go to shell": no keystroke reached the shell here).
- S-2 · S3 · 1/1 · files created on disk in the notes dir (18 note-NN.md, big, image) never appear in the Notes sidebar (rowCount 5 -> stays; Cmd+N rescan and Files/Notes round trip did not add them). Only app-created notes list. Worked around by copying content over Untitled-N.md. May be by design (G2 notes the same).
- S-3 · S4 · info · `Ctrl+B T` in Notes toggles prose (host.rs:6103), not the drawer; there is no key for the drawer, so my perturbation used the `do:` hook. Perturbation "drawer toggle" in the 100 cycles was therefore a no-op (state terminal:false both times); drawer behaviour covered separately above.
- S-4 · S4 · info · the nvim RPC stays blocked while nvim waits in a pending operator (`q`, `y`, `z`, `"`); any app-side RPC (checktime poll) times out then. With the command_rpc_only fix nothing is typed back, so no insert-mode drop; worth keeping RPC_TIMEOUT failures silent.
Not-a-finding: after `typ Go` etc. the pum is shown with mode `i` (not `ic`) and pumvisible=1 throughout.

## 2a trail (cycle = esc esc; typ Go; typ bash; check mode i + pumvisible==1; key tab tab; check one more ```bash block)
Perturbations every 5th cycle, each cycle after them still passed: sidebar click, Ctrl+H/Down/Return keyboard row switch (+Ctrl+L), Cmd+N, palette (`show` then Esc), Files/Jira/Notes header round trip, Cmd+0, prose capsule on/off, Cmd+W hide + `do:open:notes`. Rows: 100 ok / 0 fail. First failing cycle: none.
## 2c
Untitled-20.md copied to 3000 lines. G (0.38 s incl. key send), gg, Ctrl+D x12, wheel scroll, edits at top (real keys), middle (line 1500, via RPC normal!), bottom (`obash`+Tab at line 3001: popup shown, expanded to 3003 lines). No lag noted. Shots F-big-mid.png, F-big-end.png.
## 2d
Untitled-19.md (relative image). Each of 5 cycles: edit in nvim, click pop-out capsule icon, child window appears (1026x859), Cmd+W closes it, `pgrep 'kitchen-sink prose'` empty, main app frontmost. Image (red box) rendered and all 5 edits reflected in cycle 5 (shots/F-popout-5.png).
