# Usability cases (reusable catalogue)
Setup: see PLAN-usability-click-test.md §1 (GUI sandbox). Screen is 1440x932 pt; menu bar y≈12.
Menu bar x (pt): kitchen-sink 94 · File 165 · Edit 207 · Window 263. Drop-down rows ≈24 pt apart starting y≈52.
Do not screencapture regions of the screen outside the app window (owner's private apps show through).

### MB-APP-01 · kitchen-sink menu opens
- Start: sandbox app frontmost. Steps: click "kitchen-sink" in menu bar.
- Expect: About (disabled), Hide ⌘H, Hide Others, Show All, Quit ⌘Q.
### MB-APP-HIDE · Hide
- Steps: kitchen-sink ▸ Hide (x80 y87). Expect: app windows vanish (`ks-input windows` = []). Re-show: `ks-input activate`.
### MB-APP-HIDEOTHERS / SHOWALL · Hide Others (y111), Show All (y135)
- Expect: other apps hidden (visible process count drops to 1), Show All restores them (count back).
### MB-FILE-01 · File menu: Close Window ⌘W, Reset Window Size ⌘0
### MB-EDIT-01 · Edit menu: Undo/Redo/Cut/Copy/Paste/Select All (+ system Writing Tools, AutoFill)
### MB-WINDOW-01 · Window menu opens a dropdown
- Expect: Minimize etc. (plan). Actual: nothing opens.

### HDR-ICON-01 · App-icon menu (header, x≈52 y≈15 in window)
- Rows (window y): Theme ▸ 42 · Reset Default Size 78 · Reset Default Colors 101 · Open Config 136 · Keyboard Shortcuts ⌘/ 160 · App Menu… 195.
### HDR-ICON-SHORTCUTS · Keyboard Shortcuts row opens the sheet; Esc closes only it. PASS.
### KEY-all-cmd-slash · press Cmd+/ in Notes: expect the sheet. FAIL (nothing).
### HDR-TABS · header tabs (window x): Notes 97 · Files 131 · Jira 165 · Confluence 199 · Compare 233 · AI 267 (y 15). Check `state | jq .view`. AI tab crashes the app (see RUN).
NOTE for next run: use `ks-input key a b c` (real key codes), NOT `ks-input type`, for text entry in panels/fields.
