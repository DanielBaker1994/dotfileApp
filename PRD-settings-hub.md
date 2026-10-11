# PRD: `ws-settings`, one place to see and change every shortcut and setting

Status: approved, building · Written: 2026-10-03 · Target: a standalone Python tool beside kitchen-sink (macOS, arm64, python3 ≥ 3.11)

> **For the implementing agent.** Read `AGENT_CONTEXT.md` (= `CLAUDE.md`) and
> `rule.md` first. They are binding. This PRD says *what* to build and which
> files it reads and writes. Section 7 is the spec. Section 9 lists the checks
> that decide whether you are done.

---

## 1. Summary

Build `ws-settings`, a small standalone Python tool. It shows **every keyboard
shortcut and every setting** of the kitchen-sink setup in one place,
and lets you **change them** from that same place. It has a CLI for scripts and
quick lookups, and a fast keyboard-driven picker that opens on a hotkey, like
[KeyMinder](https://keyminder.app/) does for menu shortcuts. It works even
when the app is not running.

## 2. Contacts

| Name | Role | Comment |
|---|---|---|
| Daniel Baker | Owner, only user, reviewer | Decides scope, approves the look. Uses AeroSpace, herdr, Ghostty, nvim daily. |
| Implementing agent | Builds the tool | Follows `AGENT_CONTEXT.md` + `rule.md`. Does not commit unless asked. |
| PRD author (Claude) | Research + spec | Studied keyminder.app and this repo's config sources (§3, §7.3). |

## 3. Background

**What exists today.** The shortcuts and settings of this setup are spread
over many places, and each one has its own format:

| Source | What lives there | Format | Who reads it |
|---|---|---|---|
| `commands.toml` `[shortcuts]` | ~60 in-app keys, by view (`"files: Cmd+D" = "duplicate"`) | one-line TOML, a **label only** | the app's Cmd+/ card |
| `commands.toml` other sections | ~20 sections, hundreds of settings (`[app]`, `[theme]`, `[screenshot]`, `[jira]`…), documented in `#` comments above each section | one-line TOML (`configEntry` / `config_entry` codec) | the app + python side |
| `config/aerospace/aerospace.toml` | the global hotkeys: Hyper+S/N/T/X (they start the app), `alt-*` focus / move / workspace keys, service mode | real TOML | AeroSpace |
| `~/.config/herdr/config.toml` (→ dotfiles) | terminal multiplexer keys (`[keys]`, `[[keys.command]]`, incl. the pane-shot binding) | real TOML | herdr |
| `vim/init.lua` | the notes vim pane's own maps | lua | nvim |
| Swift code (`handleKey`, `PathsWindow`, `Confluence`, …) | keys that are real but **not** listed in `[shortcuts]` (e.g. /paths Cmd+Shift+C, Confluence Cmd+D / Cmd+G) | code | the app |

**The problem.** To answer "what key does X?" or "what does Hyper+T do?" you
need to know which of five files to open. The in-app Cmd+/ card only shows the
app's own keys, only for one view, and only while the app runs. Settings are
documented well, but only as comments in an 840-line file. Nothing warns when
two layers grab the same key (AeroSpace takes `alt-h` before any app sees it).

**What KeyMinder shows us** (keyminder.app, studied 2026-10-03). KeyMinder is a
free macOS menu-bar app. It reads the frontmost app's menus over Accessibility
and pops up all their shortcuts, grouped by menu. Ideas worth copying:

- Opens on a global hotkey **or a double-tap of a modifier** (Option by default).
- Type to search; **modifier filter toggles** (⌃ ⌥ ⇧ ⌘) to show only one combo.
- **Favorites** (★) and a favorites-only filter.
- Ignore lists (commands, menus, whole apps) with wildcards.
- **Markdown export** of a cheat sheet; settings export / import as JSON.
- A **system shortcuts** section (Spotlight, screenshots…).
- Quiz mode (flash cards) to learn shortcuts.
- Its gap: it **cannot edit** shortcuts ("planned"). It also can't see our
  shortcuts, since most of them are not menu items.

**Why now.** The setup has grown to 5+ key layers and ~20 config sections,
and the `[shortcuts]` list already drifts from the code. Python 3.14 with
`tomllib` and `curses` is on the machine. The python side already has the
`commands.toml` codec (`jira/jira_config.py` `config_entry` / `config_line`),
so the tool can reuse it instead of inventing one.

## 4. Objective

**Objective.** One fast, trusted view of "every key and every knob", plus safe
editing. You stop opening config files to look things up, and you change a
setting without breaking the file's comments or its one-line rules.

**Why it matters.** Fewer forgotten features (the app has a lot of them),
fewer key clashes, and config changes that can't corrupt `commands.toml`.

**Key results (SMART)**

| # | Key result | Target | How we measure |
|---|---|---|---|
| KR1 | Coverage: every shortcut source in §3 rows 1-5 shows up | 100% of entries in those files listed | test compares tool output to each source file |
| KR2 | Speed: hotkey → picker shown | ≤ 300 ms (warm) | `ws-settings tui --time` log line |
| KR3 | Safe writes: an edited `commands.toml` changes ONLY the edited line | byte-identical elsewhere, 100% of test edits | `Tests/test_settings_hub.py` diff check |
| KR4 | Clash report finds real conflicts across layers | ≥ every same-chord pair between AeroSpace and app keys | test with seeded conflicts |
| KR5 | Owner uses it instead of opening the files | owner's call after 2 weeks | owner review |

## 5. Market segment

One user: the owner, a keyboard-first developer on macOS with a heavily tuned
setup (AeroSpace tiling, herdr in Ghostty, nvim, a custom menu-bar app). The
job: *"remember and change how my desktop behaves without digging through
config files."*

**Constraints**

- Must run with **no daemon** (the app may be broken, building, or quit).
- Python stdlib only (no pip installs: the DMG install has no venv). `tomllib`
  (read) + `curses` (TUI) are both stdlib.
- Must follow `commands.toml`'s rules: one-line entries, read and written only
  through `config_entry` / `config_line`. Never split on `=` by hand.
- Must work in both install modes (repo and DMG): find files through "the
  home" `~/.config/kitchen-sink` and `$WS_COMMANDS_CONF`.

## 6. Value proposition

| Job | Today | With `ws-settings` |
|---|---|---|
| "What does Hyper+T do?" | grep aerospace.toml, then guess the app side | type `hyper t` → AeroSpace binding + the app's meaning, one row |
| "How do I rename a file in Files?" | open the app, Cmd+/ | `ws-settings keys files rename` or the picker |
| "Change the screenshot save path" | find `[screenshot]`, read comments, edit by hand | pick the setting, see its doc + current value, type the new one |
| "Is alt-h free?" | read 3 files | `ws-settings conflicts` / `ws-settings keys alt-h` |
| "Print me a cheat sheet" | none | `ws-settings export md` |

**Better than KeyMinder for us:** it knows *our* layers (the global hotkey
layer, the app's views, herdr), and it can **edit**. **Better than the Cmd+/
card:** it covers every view and every layer, works without the app, and
includes settings.

## 7. Solution

### 7.1 UX

**A. CLI** (`bin/ws-settings`, also installed as `kitchen-sink settings` later)

```
ws-settings keys [QUERY] [--layer aerospace|app|herdr|vim] [--view files] [--mods cmd,shift] [--json]
ws-settings settings [QUERY] [--section screenshot] [--json]
ws-settings get SECTION.KEY
ws-settings set SECTION.KEY VALUE      # validates, writes one line, applies
ws-settings bind LAYER CHORD ACTION    # editable layers only (§7.2 F5)
ws-settings conflicts
ws-settings export md|json [--out FILE]
ws-settings doctor                     # sources found, parse warnings, drift
ws-settings tui                        # the picker
```

`keys` and `settings` print an aligned table (or `--json`). Exit code 0 / 1
(error) / 2 (bad input), errors on stderr.

**B. The picker (`ws-settings tui`)**, a KeyMinder-style overlay in a terminal:

```
┌ ws-settings ─────────────────────────────── ⌃ ⌥ ⇧ ⌘  ★ ─┐
│ > copy_                                                    │
│ KEYS                                                       │
│ ★files      Cmd+K                copy the file's path app  │
│  jira       Cmd+K                actions: copy, URL…  app  │
│  screenshot Cmd+C / Return       copy and close       app  │
│ SETTINGS                                                   │
│  [app]      copy-toast = "Copied {} to clipboard"          │
│────────────────────────────────────────────────────────────│
│ Return edit · Tab section · Ctrl+F fav · ⌃⌥⇧⌘ filter · Esc │
└────────────────────────────────────────────────────────────┘
```

- Opens with focus in the search box. Type = fuzzy filter over chord, action
  text, view, section, key name and doc comment.
- Two groups, **Keys** and **Settings**; Tab jumps between them. Keys are
  grouped by layer, then view (`all`, `notes`, `files`, `jira`, `ai`,
  `screenshot`, …).
- Modifier filter: Alt+1..4 toggle ⌃ ⌥ ⇧ ⌘ (exact combo, as in KeyMinder; Ctrl+digits are not distinct keys in a terminal).
  Hyper is shown as **Hyper**, not ⌃⌥⇧⌘.
- ★ Ctrl+F marks a favorite; Alt+F shows favorites only.
- Return on a setting = edit it in place: the doc comment and allowed values
  are shown, Return saves, Esc cancels. Booleans flip on Space.
- Return on an editable key = rebind (press the new chord or type it).
  Read-only keys show *why* (e.g. "built into the app, label only").
- Esc closes the editor first, then clears the search, then quits
  (rule.md rule 6: Esc closes only the innermost thing).
- Edit shortcuts (Cmd+C/V/X/A/Z, Ctrl+C/V) work in every text field (rule.md).
  In a terminal Cmd+V arrives as a paste, and Ctrl+C must not quit while editing.
- Colors from `[theme]` (text / dim / accent / highlight) mapped to the
  terminal's 256 / truecolor palette, so it matches the app.

**C. Hotkey.** `Hyper+/` in aerospace.toml runs `ws-settings open`: it focuses
the picker window if it is already up, else runs `[settings-hub]
terminal-command` (Ghostty with `--title=ws-settings
--quit-after-last-window-closed=true --confirm-close-surface=false` and keybind
remaps so Cmd+A/C/X/Z reach the picker). An aerospace rule, placed BEFORE the
Ghostty → workspace 1 rule, floats that window by title. Picker quits → window
closes.
(A double-tap-modifier trigger needs a native event tap, so it moves to v2.)

### 7.2 Key features

**F1. One catalog of keys.** Each source has a small reader that returns rows of
`{layer, view, chord, action, source_file, line, editable, raw}`:

| Reader | Source | Notes |
|---|---|---|
| `aerospace` | `config/aerospace/aerospace.toml` (`tomllib`) `[mode.main.binding]`, `[mode.service.binding]` | `alt-cmd-ctrl-shift-x` → **Hyper+X**. `exec-and-forget …kitchen-sink window` → action text from a small map (`window` → "show / hide the window"), the rest shown raw. Service-mode keys get view `service`. |
| `app` | `commands.toml` `[shortcuts]` via `config_entry` | `"view: keys" = "what"`, in file order. Read-only binding (it's a label); the **text** is editable. |
| `herdr` | `~/.dotfiles/herdr/config.toml` `[keys]` + `[[keys.command]]` | lists (`["prefix+h","ctrl+h"]`) → one row per chord. `prefix` shown as herdr's prefix key. Path from `[settings-hub] herdr-config` (default `~/.config/herdr/config.toml`). Only configured keys, not herdr's defaults. |
| `vim` | `vim/init.lua` `*map` lines | best effort, read-only, view `notes (vim)`. |

Chords are normalized to one form (`Cmd+Shift+K`) for search, filters and
clash checks. The original text is kept for display and writing back.

**F2. One catalog of settings.** Every `[section]` key of `commands.toml`, read
with `config_entry`. For each key:
- **value** (current), **type** inferred (bool / number / color hex / path /
  list / text);
- **doc** = the `#   key   text` lines in the comment block above the section
  (the file already documents keys in this style), plus the comment directly
  above the line;
- **range / allowed values** from the app's schema when present (F6), else
  from the doc text (`a | b | c`), else free text;
- commented-out keys (`# key = value`) listed as "default, not set".

**F3. Search, filter, favorites.** Fuzzy match as in §7.1; `--mods` and the
⌃⌥⇧⌘ toggles; favorites in `~/.config/kitchen-sink/settings-hub.json`
(user data, never in the repo or the app bundle).

**F4. Safe setting edits.** `set` / picker edits:
1. Re-read the file, find the exact line (section + key) with `config_entry`.
2. Validate: type, range, enum. Bad → refuse, explain, change nothing.
3. Write the line with `config_line`, keep any trailing `# comment`, keep
   every other byte. A key not in the file is added at the end of its section.
4. Write atomically (temp file in the same folder + `rename`). The real
   `commands.toml` may be a symlink: write through to its target, never
   replace the link.
5. Record the old line in `~/.cache/kitchen-sink/settings-undo.json`;
   `ws-settings undo` puts it back (last 20 edits).

**F5. Rebinding (editable layers only).**
- **AeroSpace**: change the left side of one binding line (`alt-h = …` →
  `alt-shift-y = …`), line-based like F4 (tomllib can't write and would lose
  the comments). Refuse if the new chord is already bound in the same mode
  (show the holder). Apply: `aerospace reload-config`.
- **herdr**: same, on `[keys]` lines. Apply: herdr's reload if it has one,
  else say "restart herdr".
- **App keys**: NOT rebindable in v1. The bindings live in Swift
  (`handleKey`), and `[shortcuts]` is only the label list. Editing a chord
  there would make the card lie. Show them read-only with that reason.

**F6. Schema from the app (optional, one source of truth).** Add a CLI verb
`kitchen-sink config-schema` that prints JSON: `configNumberKeys` ranges,
known enum keys, and which sections apply live vs on reload. Plus
`kitchen-sink config-check SECTION KEY VALUE` (and `--file PATH`), which
runs the app's own `configValueProblem` / `validateConfig`, so section rules are
exact too. Allowed values read from doc comments are only a warning (`--force`
writes anyway). The tool caches
it in `~/.cache/kitchen-sink/config-schema.json` and falls back to
inference (F2) when the binary is missing. No ranges are copied into Python.

**F7. Apply after a change.**

| What changed | Apply |
|---|---|
| `[screenshot]`, `[pane-shot]` | nothing (read on every trigger) |
| `[theme]`, launch-only `[app]` keys | new verb `kitchen-sink restart` |
| `[confluence]`, `[ai]`, `[setup]` | nothing (read when the view opens) |
| `[jira] enabled` | `kitchen-sink jira-poll on\|off` (never a raw write) |
| other `commands.toml` sections | new socket message `reload` → `SwitcherController.reloadConfig()` (today only the notes icon menu's "Reload Config" calls it). No daemon → "applies on next start". |
| `[notifications]`, sketchybar | `sketchybar --reload` |
| aerospace.toml | `aerospace reload-config --dry-run --no-gui`, then `aerospace reload-config` (failure → auto undo) |
| herdr | `herdr config check`, then `herdr server reload-config` (failure → auto undo) |

The tool prints what it ran and whether it worked.

**F8. Clash report** (`conflicts`, and a ⚠ in the picker). Same normalized
chord in two places where both can fire:
- AeroSpace main-mode binding vs any app / herdr / vim key (AeroSpace wins
  globally, the other never fires), e.g. `alt-h` focus vs herdr `alt+h` resize.
- Two bindings in one layer + view.
- Ghostty `global:` keybinds (e.g. Opt+Space, the quick terminal).
- Known macOS system shortcuts (a small built-in list: Cmd+Space, Cmd+Shift+3/4/5,
  Ctrl+arrows…), like KeyMinder's System Shortcuts section.

**F9. Drift report** (`doctor`). Keys the code handles but `[shortcuts]` does
not list must be found by **driving the app**, not by grepping Swift (repo
rule: no source-grep tests). v1: `doctor` lists sources found, parse
warnings, unknown keys in `commands.toml` (keys the schema doesn't know),
and dead bindings (AeroSpace `exec` paths that don't exist). Filling the
known gaps in `[shortcuts]` (/paths, Confluence, Cmd+K pickers, Space Quick
Look…) is a one-time task in this project's first release.

**F10. Export.** `export md` = a cheat sheet (layer → view → table), for notes
or printing. `export json` = the full catalog (for other tools / the app's
Cmd+/ card later).

### 7.3 Technology

- Location: `settings_hub/` (package) + `bin/ws-settings` (launcher). Added to
  `install.conf` `RESOURCE_*` so the DMG ships it; reached through the home's
  `bin` link in app mode.
- The codec moves to a new top-level `pylib/config_text.py`; `jira_config.py`
  re-exports `config_entry` / `config_line`, so there is still ONE python codec.
- Paths: `$WS_COMMANDS_CONF` › the home's `commands.toml`; aerospace via the
  home's `config/aerospace`; herdr path from `[settings-hub]`.
- New `[settings-hub]` section in `commands.toml` (config over code): `hotkey`
  label, `herdr-config`, `vim-init`, `layers` (which readers run), `colors`
  (`theme` = use `[theme]`).
- Python ≥ 3.11 (`tomllib`). Preflight (`bin/preflight.sh`) already warns
  when python3 is missing; the tool prints a clear error on an older python.
- Swift changes (small): socket + CLI `reload` (F7), CLI `config-schema` (F6).
  Nothing else in the app changes.
- Tests: `Tests/test_settings_hub.py` (stdlib `unittest`): fixtures for each
  source, write-back byte checks, symlink write-through, validation refusals,
  clash detection, export snapshot. Plus one live check: `ws-settings set` +
  `reload` against the running daemon, verified via the socket's `state`.

### 7.4 Assumptions (to check)

1. **A terminal picker is "light enough".** The owner lives in Ghostty, so a
   curses UI in a floating Ghostty window feels native. *If not:* v2 swaps
   the front end (local web page or a small AppKit panel). The catalog and
   writer stay the same.
2. Ghostty starts fast enough for KR2 (≤ 300 ms). *Check first;* if it is slow,
   keep one hidden Ghostty window warm, or run the picker inside herdr.
3. ~~herdr can reload without a restart~~ Confirmed: `herdr server reload-config`.
4. The `#   key   text` comment style is used consistently enough to parse
   docs. Sections that don't follow it show "no description" (and `doctor`
   lists them).
5. Rebinding app-internal keys is not needed in v1. Most edits are settings
   and global hotkeys.
6. The owner wants the tool to edit `aerospace.toml` and herdr config, not
   only show them.

## 8. Release

| Phase | Size | Contents |
|---|---|---|
| **v1: read** | ~2-3 days | Readers (F1, F2), `keys` / `settings` / `get` / `export` CLI, the picker (search, groups, mod filter, favorites), Hyper+/ hotkey, `doctor`. Fill the missing `[shortcuts]` entries. Tests. |
| **v1.1: write** | ~2 days | `set` + undo (F4), `reload` socket verb + apply table (F7), `config-schema` (F6), picker editing, clash report (F8). |
| **v1.2: rebind** | ~1-2 days | AeroSpace + herdr rebinding (F5) with clash refusal. |
| **v2 (later)** | open | Double-tap-modifier trigger; GUI front end if the TUI falls short; make the app's own keys configurable from `[shortcuts]` (real remapping, a Swift project); quiz mode; the app's Cmd+/ card reads `export json` so both show the same list. |

Out of scope: system-wide remapping of other apps (KeyMinder points to
CustomShortcuts for that), iCloud sync, localization.

## 9. Definition of done (v1 + v1.1)

1. `ws-settings keys` lists every binding in aerospace.toml (both modes), every
   `[shortcuts]` entry and every herdr key (test compares counts and items).
2. `ws-settings keys hyper` shows Hyper+S / N / T / X with readable actions.
3. `ws-settings settings screenshot` lists every `[screenshot]` key with its
   doc text and current value.
4. `ws-settings set screenshot.contrast-opacity 999` is refused (range from the
   schema); `… 150` writes ONE changed line, `undo` restores the file
   byte-for-byte.
5. A `set` on a symlinked `commands.toml` keeps the symlink.
6. `set app.hide-on-focus-loss true` with the daemon running → `reload` sent →
   the socket `state` reflects it, no restart.
7. `conflicts` reports the seeded AeroSpace-vs-app clash in the test fixture.
8. The picker opens on Hyper+/, filters while typing, Esc unwinds one step at
   a time, Cmd+V pastes into the search and edit fields.
9. Works with the daemon quit (everything but apply).
10. `python3 Tests/test_settings_hub.py` passes. No new pip dependencies.
