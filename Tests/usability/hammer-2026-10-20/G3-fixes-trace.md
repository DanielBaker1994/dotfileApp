# G3 fixes trace (2026-10-20, instrumented sandbox pid 59539)

## Task 1: sidebar deafness fix (G3-2): PASS
8 notes via Cmd+N (osascript key code 45), then ctrl+h (sidebar focus: `take_focus pane=sidebar target=WSSidebarView accepted=true`), Up/Down, Return.
Every keyboard Return logged the full chain and nvim switched (expand('%:t') over RPC), five times on different rows:
```
host Select(5) / select_row(5) tabs=8 selected=7 / vim open Untitled-6.md rpc_ok=true
host Select(1) / select_row(1) ... vim open Untitled-2.md rpc_ok=true
host Select(4) -> Untitled-5.md ; Select(0) -> Untitled-1.md ; Select(4) -> Untitled-5.md   (all rpc_ok=true)
```
After the 5 Returns the callback was still alive (previously dead forever):
- mouse click row: `sidebar mouseDown hit=Some((1,false))` -> host Select(1) -> select_row -> vim open Untitled-2.md rpc_ok=true; second click row 5 -> Untitled-6.md.
- "+" click: `mouseDown hit=Some((-2,false))` -> new note Untitled-9.md (rows 8 -> 9, file created, nvim on it).
- Delete (close) via sidebar keyboard: rows 9 -> 8, selection moved to Untitled-9, `vim open Untitled-9.md rpc_ok=true` (note file stays on disk; it is a tab close).
- Space: no observable effect and no log line (not conclusive; no crash, later Return/clicks still fine).
- Tree pane: none in Notes, not tested.
Caveats: the first ctrl+h right after 8 Cmd+N only logged `after move Left ... current=editor`, a 2nd ctrl+h was needed to reach the sidebar (ks-input drops/merges; not investigated). At that point nvim's current buffer was [No Name] (`:ls`: buffer 1 %a, Untitled-8 loaded as buffer 10) although sidebar selection = Untitled-8 and DBG showed `vim open Untitled-8 rpc_ok=true` earlier; Return on a row fixed it. Possibly ctrl+h/ctrl+k/Up reached nvim. Unexplained, watch for it.

## Task 2: G3-1 (Ctrl+J / Ctrl+K with drawer open): NOT REPRODUCED here
Drawer opened with the `toggle-terminal` action (state views.notes.terminal=true, panes sidebar/editor/terminal).
Ctrl+J then Ctrl+K, log:
```
take_focus pane=terminal target=SwiftTerm.LocalProcessTerminalView@0x760fea1c00 accepted=true focusView=WSShim@0x76102a2800
after move Down: firstResponder=SwiftTerm.LocalProcessTerminalView@0x760fea1c00 current=terminal
take_focus pane=editor   target=SwiftTerm.LocalProcessTerminalView@0x760fea1500 accepted=true focusView=WSShim@0x76102a2080
after move Up:   firstResponder=SwiftTerm.LocalProcessTerminalView@0x760fea1500 current=editor
```
Answers: the editor pane's `focus` view is a `WSShim` container; `first_focusable` (live_panes.rs:73) descends to the real nvim SwiftTerm view (@...1500) and makeFirstResponder is accepted; the drawer shell is a different instance (@...1c00). First responder after Ctrl+K equals the editor target, and equals the initial-state responder (@...1500 also after ctrl+h/ctrl+l hops). Typed keys landed in nvim, not the shell (checked via RPC getline/mode and screenshot: shell prompt empty):
- nvim normal mode: Ctrl+J, Ctrl+K, `o`,`a`,`bc` -> line 2 = "abc", mode i. PASS
- nvim insert mode: Ctrl+J, Ctrl+K, `xy` -> "abcxy" in insert. PASS
- view round trip (socket verbs files/notes, and ctrl+tab x2): typing reached nvim. PASS
- click Notes header icon, Ctrl+J/K, ctrl+k again, `G o q w`: "recording @w" appeared in nvim (keys reached nvim). PASS
Hypothesis for the original report: the failing runs (G3 soak) had state responder = LocalProcessTerminalView for both panes, i.e. ambiguity only in `state`. The correct pointer can be stale if the editor pane's SwiftTerm view is REPLACED (nvim restart / reload via notes_vim.rs focus ~548 or a Files round trip that rebuilds the embedded terminal) while `focus`/`current` still resolve through the old WSShim: then makeFirstResponder is accepted on a detached/old view and the window falls back to the other terminal. Not observable here: pointer @0x760fea1500 stayed identical across every variant, so a rebuild did not happen. Next step to catch it: log the pointer in notes_vim.rs focus and compare with take_focus target after a nvim restart (kill nvim via RPC `:qa!` is not allowed here); G3 saw it after ~20 cycles, so also run a longer soak with the drawer open.

## Side findings
- A hit-enter prompt in nvim (`E5107: Error loading lua [string ":lua"]:1: unexpected symbol near '|'`, shown after toggling the drawer) blocked all nvim RPC (every --remote-expr hung) until Return was typed. Source: the app typed a `:lua ... | ...` command as keys (resize/drawer path, VimResized lazy.nvim float error also in :messages). Worth grepping for `:lua` + `|` in notes_vim.rs.
- Pending operators (`z`, `q`) also make --remote-expr hang; use esc before RPC probes.
