# VERIFY fixes (2026-10-20), sandbox pid 53105

## FIX A (insert-mode drop, host.rs command_rpc_only): PASS
- RPC working (sock nvim-notes-53105.sock, 64 chars): note Untitled-11, o + `bash`, idle 5s -> mode()=='i', pumvisible()==1 (shots/V-A1-popup-rpc-ok.png).
- Deleted the nvim socket file (rm only, nothing killed). Idle 10s in insert mode, then typed `JJKK`: landed in the buffer as text ("bashJJKK", still insert; shots/V-A2-after-sock-rm.png). Old build: 10/10 dropped to normal within ~1s.
- Fresh line, typed `bash` with RPC gone, idle 10s: popup `markdown_bash_block [snippet]` still showing (shots/V-A3-popup-after-10s-no-rpc.png).
- Tab expanded the snippet (```bash / echo ${HOME} / ```), then typed `x` went into the buffer (shots/V-A4-after-tab.png).
- Caveat: with RPC gone I could not read mode() directly; evidence is screenshots (visible text + popup).

## FIX B (pop-out images): PASS
- Fixture: img1.md copied over Untitled-12.md (relative img/local.png red, absolute abs.png blue).
- Inline prose renders both (shots/V-B1-inline.png). Pop-out (child pid, window 1026x859) now renders BOTH images, not alt text (shots/V-B2-popout.png). Old: 5/5 alt text.
- Right-click in pop-out -> menu (shots/V-B3-popout-menu.png) -> Copy PDF Path: clipboard = /private/tmp/kssb/home/Downloads/Untitled-12.pdf, file written 15:54 (13.9 KB).
- open/close(Cmd+W)/reopen x3: child pids 55009, 55073, 55156 each exited on close; `pgrep -fl 'kitchen-sink prose'` empty afterwards; daemon.log: 0 panics/"borrowed".
- Not re-shot images on reopen cycles 2-3 (only first open screenshotted).

## Odd / tooling
- `ctrl+u` via ks-input in insert mode produced repeated "E29: No inserted text yet" + hit-enter prompts (looks like ^A/^@-like mapping, not investigated; tooling or keymap).
- A second Cmd+W sent after the child closed hit the MAIN window and hid the whole app (view ''); recovered via `show` socket verb + palette "Notes". Do not send cmd+w twice.
- `ks-input activate` often reports "not frontmost within 2s" but succeeds ~3s later; checking `front` after a sleep works.
- `pbcopy </dev/null` hangs in this environment (use pbpaste only).
- Sandbox nvim now has no RPC socket file (deleted by me); Untitled-11/12 notes added. App left running.
