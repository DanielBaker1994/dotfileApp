# Rules for this repo

Non-negotiable UX rules. Any change that touches a text input or keyboard
handling must honor these.

## 1. Obvious edit shortcuts always work in every text input

Copy / Paste / Cut / Select All / Undo must work in **every** editable text
field: the search/filter bars, tool panels, and
**every sheet prompt** (Rename…, path prompts).

- `Cmd+C` / `Cmd+V` / `Cmd+X` / `Cmd+A` / `Cmd+Z` — native macOS.
- `Ctrl+V` (and `Ctrl+C`) — must paste/copy too; users expect the same
  everywhere.
- Do NOT rely on AppKit key equivalents alone (the popup windows' focus is
  non-standard); route edit keys to the active field editor explicitly.
- The key router (`route_key_event` in `rust/ws-rs/src/app/host.rs`, deciding
  through `ui::popup::route_key`) must never swallow an edit shortcut without
  forwarding it to the focused text editor. Sheets (NSAlert) get paste routed
  straight to their field editor; every sheet text field sets
  `alert.window.initialFirstResponder`.

## 2. Right-click menus must offer obvious actions

File browsers must have right-click menus with the
actions users expect: copy the file, copy the absolute path, and reveal it in
the app's own Files view (`Reveal in Files`). (The notes app was removed;
markdown editing lives in nvim — `~/.dotfiles/nvim`.)

## 3. Config over code

User-facing strings, window sizes, and paths live in `commands.toml` — prefer
changing the config over hard-coding.

## 4. NEVER make git changes in a backup workspace

When working on this repo, ALWAYS verify you are in the real repo directory
(`/Users/danielbaker/.config/kitchen-sink`) before running any git command
(`git add`, `git commit`, `git push`, `git checkout`, etc.).

- NEVER stage, commit, or modify `.git/` in a `.bak` copy, backup directory,
  or duplicate of this repo.
- ALWAYS run `git status` and check `git rev-parse --show-toplevel` before
  committing to confirm you are in the correct repo.
- If a backup copy exists (e.g. `kitchen-sink.bak/`, `kitchen-sink.repo-<date>/`), it
  MUST NOT be touched by any git operation. Changes belong ONLY in the real
  repo at `/Users/danielbaker/.config/kitchen-sink`.
- Running `./INSTALL.sh` or any install script must NEVER make git changes
in a different repo or workspace. The install script is for installing the
app — not for modifying git history anywhere.

## 5. Transient overlays own the keyboard, and Esc closes only them

Every popover / picker / dropdown opened from a window (the jira column
filters, `JiraMultiPicker`, the search pickers, …):

- takes keyboard focus the moment it opens — its window is made KEY (not
  just a first responder), so typing filters and `Ctrl+N` / `Ctrl+P` / ↑↓
  move without a click first;
- closes on `Esc` WITHOUT closing the window under it. While one is up its
  Esc handler takes precedence (the "transient Esc" the key router consults
  before the window's own Esc handling); clear it when the overlay closes.

## 6. One feature, every surface: quick agent and main app stay in step

A feature that exists in more than one place must work the same in all of
them. The usual pairs:

- the **quick agent** (Hyper+S palette, Hyper+X screenshot, Find Recent
  Files — `ks-core/src/app/quick.rs`) and the **main app** (the shared
  window: Files view, Recents, tool panels — `ws-rs/src/app/host.rs`);
- the different ways into one tool (e.g. the screenshot editor from the
  Hyper+X Recents merge *and* from right-click ▸ "Merge N Photos…" in Find
  Recent Files *and* in the Files view).

How:

- Build the feature once, in `ks-core`, at the point every entry path
  shares (e.g. `ScreenshotController::open_merged_in_editor`, the shared
  `editor_menu_entry`), never in one caller. If only the host can do part
  of it, add a slot to `app/hooks.rs` and give both processes an
  implementation.
- Before calling a change done, grep for every caller / entry point and say
  in the summary which surfaces get it. A surface that cannot have it (e.g.
  the quick agent has no rename / trash) is named, with the reason.
- A test or the socket state should prove the shared path, not one caller.
