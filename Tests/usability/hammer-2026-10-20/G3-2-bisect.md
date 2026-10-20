# G3-2 bisect: Notes sidebar rows stop switching the note (2026-10-20, instrumented sandbox pid 56451)

## Trigger
NOT the note count. Trigger = the first keyboard Return in the notes sidebar (ctrl+h, Up/Down, Return).
After that ONE keypress the sidebar's `on_event` callback is permanently gone: mouse clicks and the "+" button
all stop working. Bisect with mouse only, Cmd+N (osascript key 45) to 20 rows (sidebar scrolling from 16 rows,
scroll=171 at 20) and clicking an older row after every step: every click that reached the view worked
(host Select -> select_row -> vim open rpc_ok=true; `&modified`=0 throughout, notes empty/unmodified).
Count, scrolling, scroll-then-click: all fine. Modified-state not tested (typing skipped).

## Chain break
Keyboard Return: link (c). Log shows only `DBG host Select(5)`, then NOTHING (no select_row, no vim open); nvim
ends in an unnamed [No Name] buffer (:ls: buffer 2 wiped). No panic/borrowed text in daemon.log.
Afterwards every mouse click: link (b): `DBG sidebar mouseDown ... hit=Some((8,false))` / `(10,false)` hit-test is
right, but no `host Select` ever again. "+" click (mouseDown hit=-2) creates no note either.

Evidence:
```
ws: DBG sidebar mouseDown p=(120,125) hit=Some((7, false)) scroll=171 ...   <- last good click
ws: DBG host Select(7) / select_row(7) tabs=20 selected=19 / vim open Untitled-8 rpc_ok=true
ws: DBG host Select(5)                  <- keyboard Return; chain dies here
ws: DBG sidebar mouseDown p=(120,159) hit=Some((8, false)) scroll=171       <- no Select after
ws: DBG sidebar mouseDown p=(120,227) hit=Some((10, false)) scroll=171      <- no Select after
ws: DBG sidebar mouseDown p=(229,24) hit=Some((-2, false))  ("+")           <- no note created
```
Cursor state: ctrl+h focus sidebar OK; `state.sidebarCursor` never changes (as in G3-9).

## Hypothesis (file:line)
1. rust/ws-rs/src/app/host.rs:7008-7013 `notes_click_arms_rename` calls `NSApplication.currentEvent().clickCount()`.
   For a keyboard activation currentEvent is a KeyDown; `-[NSEvent clickCount]` raises
   NSInternalInconsistencyException on key events. Called from the `E::Select` arm (host.rs:7113-7120) after the
   `host Select` log and before `click_row` -> exactly where the log stops.
2. rust/ws-rs/src/ui/sidebar.rs:766-777 `emit` does `on_event.take()`, runs `cb(ev)`, restores only after it
   returns. When the callback unwinds/throws, the slot stays `None` forever, so every later
   `emit` (click, "+", Return) is a silent no-op: the permanent break. (Mouse clicks pass clickCount 1 so the
   first mouse path is safe; this is why mouse-only runs never failed.)
Fix ideas: only call clickCount when `event.type == LeftMouseDown`; restore the callback with a drop guard.
Unverified: not checked G3's original session history (they used ctrl+h/Up/Return early, consistent with this).

## Other observations
- ks-input `key cmd+n` dropped 3/3 here; osascript `key code 45 using {command down}` worked every time.
- 2 of ~30 clicks right after Cmd+N produced no mouseDown (event dropped by input tool/focus, not the chain).
- Return pressed with sidebar focus moves focus to editor (leave_right) even though Select was lost.
