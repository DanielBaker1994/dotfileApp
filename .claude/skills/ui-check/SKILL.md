---
name: ui-check
description: Verify a kitchen-sink UI change in seconds by querying the running daemon's state over its socket (views, frames, drawers, focus, tabs) instead of running the slow bin/ui-test.sh suite. Use after ./build.sh when a change affects the shared window, views, drawers, or focus.
---

# ui-check — verify UI behavior through the daemon socket

The daemon answers two test messages on `$TMPDIR/ws-notes.sock`
(`[app] notes-socket`), implemented in `SwitcherController.testQuery`
(kitchen_sink.swift) + `PopupWindow.testState` (PopupWindow.swift):

- `state` → one JSON line:
  `view` (visible shared-window view, "" = hidden), `visible`, `active`,
  `keyWindow`, `windows` (visible NSWindows), `palette` (switcher shown),
  `views.<notes|files|jira|detail|releases|config|output|confluence|ai>` →
  `shown`, `key`, `frame` [x,y,w,h], and for PopupWindow views also
  `drawerInset`, `terminal` / `browser` (drawer OPEN), `findBar`,
  `accessory`, `pane`, `tabs`, `selectedTab`, `responder` (class name).
- `do:ACTION` → runs it, answers the state afterwards. ACTION = `cycle`,
  `cycle-back`, `hide`, `back`, `home`, `toggle`, `toggle-terminal`,
  `reset-size`, `open:VIEW`, `header-style:STYLE` (live only),
  `esc-hides:VIEW:on|off` (writes the view's `esc-close`),
  `paths:show|hide|return|select:N` (the /paths shelf popup; state `paths`
  = {shown, key, level, rows[{path, why}], frame}).
  State also carries `hideOnFocusLoss`, `escHides` (per view), `pid` (the
  daemon), `paletteCommands` (what Hyper+S "/" lists), `headerStyle`; each view has `wid` (its window id — compare with
  `aerospace list-windows --all --format '%{window-id}|%{workspace}|%{window-layout}'`).

## Steps

1. `./build.sh` (builds + relaunches; exit 0 + no output = OK).
2. Query:
   ```bash
   S="$TMPDIR/ws-notes.sock"
   echo state | nc -U -w3 "$S" | jq '{view, visible, notes: .views.notes | {frame, drawerInset, terminal, browser, pane}}'
   echo do:toggle-terminal | nc -U -w3 "$S" | jq '.views.notes | {terminal, frame, drawerInset}'
   ```
3. Assert the specific thing the change should affect. Put things back
   afterwards (toggle drawers back, re-open the view the user was on) —
   this is the user's live app.
4. If state you need is missing, add it to `testState` / `testQuery`
   rather than reaching for osascript.

In bin/ui-test.sh the same is wrapped as `ws_state JQ`, `ws_do ACTION`,
`wait_state 'JQ_BOOL' [secs]` (50 ms polling — use instead of `sleep`).

Only real keyboard/mouse paths (the key routing in `handleKey`, clicks)
still need synthetic input; run the full suite at most once (it is slow).
cliclick's keystrokes may not reach apps from the agent's session —
`osascript -e 'tell application "System Events" to key code 53'` does.

Show / hide / focus / workspace-follow changes: `bin/ui-test-focus.py`
(or a subset: `show hide follow stranded swap focus esc single`) — timed,
checks AeroSpace's view of the window too, restores everything after.
