# Rules for this repo

Non-negotiable UX rules. Any change that touches a text input or keyboard
handling must honor these.

## 1. Obvious edit shortcuts always work in every text input

Copy / Paste / Cut / Select All / Undo must work in **every** editable text
field: the notes editor, the search/filter bars, the embedded terminal, and
**every sheet prompt** (New Note, Open Existing…).

- `Cmd+C` / `Cmd+V` / `Cmd+X` / `Cmd+A` / `Cmd+Z` — native macOS.
- `Ctrl+V` (and `Ctrl+C`) — must paste/copy too, because the embedded
  terminal honors them and users expect the same everywhere.
- The app is an accessory app with no Edit menu, so do NOT rely on AppKit key
  equivalents; route edit keys to the active field editor explicitly.
- `PopupWindow.handleKey` must never swallow an edit shortcut without
  forwarding it to the focused text editor. Sheets (NSAlert) get paste routed
  straight to their field editor; every sheet text field sets
  `alert.window.initialFirstResponder`.

## 2. Right-click menus must offer obvious actions

File browsers and the embedded terminal must have right-click menus with the
actions users expect: copy the absolute path, open the file in the notes app,
open in default app, reveal in Finder.

## 3. Config over code

User-facing strings, window sizes, and paths live in `commands.conf` — prefer
changing the config over hard-coding.