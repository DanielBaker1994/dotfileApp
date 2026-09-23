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

## 4. NEVER make git changes in a backup workspace

When working on this repo, ALWAYS verify you are in the real repo directory
(`/Users/danielbaker/.config/dotfileApp`) before running any git command
(`git add`, `git commit`, `git push`, `git checkout`, etc.).

- NEVER stage, commit, or modify `.git/` in a `.bak` copy, backup directory,
  or duplicate of this repo.
- ALWAYS run `git status` and check `git rev-parse --show-toplevel` before
  committing to confirm you are in the correct repo.
- If a backup copy exists (e.g. `dotfileApp.bak/`, `dotfileApp.old/`), it
  MUST NOT be touched by any git operation. Changes belong ONLY in the real
  repo at `/Users/danielbaker/.config/dotfileApp`.
- Running `./INSTALL.sh` or any install script must NEVER make git changes
in a different repo or workspace. The install script is for installing the
app — not for modifying git history anywhere.

## 5. NEVER rebuild SwiftTerm (Vendor/SwiftTerm/) unless explicitly told to

SwiftTerm in `Vendor/SwiftTerm/` is a third-party embedded terminal library.
It is precompiled once into a static lib (`.build/SwiftTerm/libSwiftTerm.a`).

- NEVER modify, edit, or refactor any code under `Vendor/SwiftTerm/`.
- NEVER rebuild it (`build_term_lib`) unless the user explicitly requests it.
- The build script auto-rebuilds it only when a SwiftTerm source file changes —
  do not touch those files, and it won't trigger.
- If SwiftTerm is missing or broken, ask the user before rebuilding.