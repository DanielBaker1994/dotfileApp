# kitchen-sink

A macOS app: AeroSpace switcher popup + notes / voice-to-text / Jira windows,
with a borders focused-window outline. Native AppKit app written in Rust
(objc2; `rust/`), no runtime dependencies beyond what the installer brings.

## Install (end user — pick one)

### The app (disk image)

Open `kitchen-sink-<version>.dmg`, drag the app to **Applications**,
open it. The first run shows **Setup & Health Check**: what this Mac has and
what is missing (no Apple on-device model → the AI view is off, everything
else works). Notes, files, Jira, Confluence and AI work straight away; the
Hyper hotkeys and window borders (AeroSpace, borders — Homebrew) are one optional button in that window. Your settings live in
`~/.config/kitchen-sink` (`commands.toml`, `rules/`, `config/`);
updating = dragging the new app over the old one.

Build the image: `./ws dmg` (→ `.build/dist/`). With `DEVELOPER_ID` +
`NOTARY_PROFILE` set in `install.conf` it is signed and notarized; without
them it is self-signed and other Macs need System Settings ▸ Privacy &
Security ▸ Open Anyway. Uninstall:
`/Applications/kitchen-sink.app/Contents/Resources/UNINSTALL.sh`.

### From source


**There is ONE command.** From the repo root:

```
./INSTALL.sh             install everything (direct, verbose)
./INSTALL.sh uninstall   remove everything
./INSTALL.sh help        show this
```

Not sure what to run? `./INSTALL.sh` — that's it.

### One script (self-cloning)

Download just the script (or `curl -fsSL …/INSTALL.sh | bash`). If it is not
running from a git checkout it asks where to put the app (default
`~/kitchen-sink`), clones the repo, and continues the install from the
clone:

```bash
bash INSTALL.sh
```

`INSTALL.sh` prints every step as it runs (checks your Mac, installs
Homebrew/deps, installs configs, compiles the app, grants mic + speech
permissions, starts borders, loads the jira poll agent, opens
the app). Re-running it is safe. Uninstall: `./UNINSTALL.sh`.

Repo: `https://github.com/DanielBaker1994/dotfileApp.git`

### What you get

| Thing | Where |
| --- | --- |
| Switcher popup | Hyper+S — workspaces (app icons per workspace), a command grid, and a status row (unread · CPU · RAM · battery · clock); type to narrow all of it |
| Every shortcut + setting (search, edit, rebind, clash report) | Hyper+/ — `bin/ws-settings` (`--help`: keys, settings, set, undo, bind, conflicts, export md, doctor) |
| Notes / Jira / Voice / Health windows | menu-bar **wrench** icon |
| Voice notes | red record button → dictation → text, pause/resume, live draft |
| Window borders | borders (brew service) |
| Jira window | Hyper+J (aerospace) — spreadsheet table, columns from `commands.toml [jira] columns` |
| AI view | Hyper+S → /ai — Apple's on-device model (`fm`) driven by rule files in `rules/*.md` (Grammar Check: grammar + Markdown formatting for Outlook/Webex); right pane = Diff / Markdown / Outlook / Webex preview, ⧉ Copy = rich text (HTML + RTF + Markdown via pandoc), Ctrl+Enter runs |
| Jira poll on/off + options | wrench menu → **Enable Jira** / **Disable Jira** (asks whether to keep polling in the background) / **Toggle Jira Window** / **Open Jira Config Window** (poll jobs, schedules, columns, Force Poll, Setup…) |
| Jira poll agent | launchd `com.jira.poll` (60s tick; `jira_poll.py` runs only due endpoints) |

## Development

`./ws` is the one entry point — run it with no arguments for an interactive
menu, or pass a command. `./ws help` lists everything.

```bash
./ws                     # interactive menu
./ws build               # compile kitchen-sink.app + TCC re-grant + relaunch
./ws build --build-only  # compile + grant, do NOT launch
./ws test config         # unit tests (any suite: compare, recent, …)
./ws test ui             # the UI suite
./ws doctor              # health check of the whole stack
./ws dmg                 # the distributable disk image
```

The old names (`bin/make-dmg.sh`, `bin/fix-permissions.sh`,
`bin/fake-*.sh`) still work as thin shims around the same functions.

- `rust/ws-rs` — the app (`kitchen-sink` binary): `app/` host, socket, CLI
  client, config, Python helper client; `ui/` popup framework + shared
  window; `views/` notes, files, jira, confluence, AI, compare, screenshot, …;
  `engines/` pure logic; `panes/` pane focus + vim keys
- `rust/swiftterm-shim` — the one Swift piece: a tiny `NSView` wrapper around
  the pinned SwiftTerm library for the embedded terminal
- `rust/ws-helpers` — the notify AX helpers (`dock_badges`, `webex_unread`)
- `bin/build-app.sh` — `cargo build` → the signed `kitchen-sink.app`
- `commands.toml` — every user-facing string + window definition (the app is
  config-driven; new windows need no code)
- `bin/kitchen_sink.sh` — hotkey launcher (pings the daemon, launches via
  LaunchServices so mic/speech TCC grants attach to the app bundle)
- `bin/grant-permissions.sh` — writes the mic + speech TCC grants
- `jira/` — python poller (stdlib only): `jira_api.py` (API client, JQL
  queries, `--sync`, curl logging), `jira_poll.py` (per-endpoint scheduler,
  lock, publish), `jira_config.py` (config.json load/migrate/validate +
  `[jira] columns` → API `fields=`), `jira_status.py` (status.json);
  `jira-doctor.sh`
  (health checks, `/health-checks` window). Tests: `python3 Tests/test_jira_poll.py`

### Jira poller at a glance

| What | Where |
| --- | --- |
| On/off switch | `commands.toml [jira] enabled` (menu **Enable Jira** / **Disable Jira**; `poll-when-disabled = true` keeps the poller running while disabled; or `bin/kitchen_sink.sh jira-poll on\|off\|toggle\|setup`) |
| Credentials + **poll jobs** (schedules) | `~/.config/jira/config.json` → `endpoints` (chmod 600; edited by the **Jira Config** window) |
| Team schema (custom fields, JQL templates, …) | `~/.config/jira/team.json` (optional; example `jira/team.example.json`) |
| Live state (last/next run, errors, lock) | `~/.cache/jira/status.json` · `jira/jira_status.py` · `jira-doctor.sh` |
| Every HTTP request, copy-pasteable | `~/.cache/jira/curl.log` (chmod 600 — contains the basic-auth token) |
| Window data | `~/.cache/kitchen-sink/jira_json/<endpoint file>.json` |

### Where the poll jobs (and their JSON files) come from

Every tab in the Jira window is a JSON file written by one **poll job**, and
every poll job is one entry of `endpoints` in `~/.config/jira/config.json`:

```jsonc
{
  "site": "https://you.atlassian.net", "email": "…", "token": "…",
  "outDir": "~/.cache/kitchen-sink/jira_json",
  "endpoints": [
    {"name": "all",      "type": "issues",   "projects": "*",     "window": "10m", "file": "all.json"},
    {"name": "KAN",      "type": "issues",   "projects": ["KAN"], "window": "30m", "file": "KAN.json"},
    {"name": "releases", "type": "releases", "projects": "*",     "window": "1h",  "file": "releases.json"},
    {"name": "directory","type": "directory","projects": "*",     "window": "1w"}
  ]
}
```

- `name` = the job (sidebar item in the Jira Config window), `file` = what it
  writes into `outDir` (= the Jira window tab), `window` = how often it runs,
  `projects` = `"*"` or a list of keys, `type` = `issues` / `releases` /
  `directory` (users + projects for the pickers; no tab). Each job also owns
  its `columns`.
- **Who creates them:** `jira/jira_config.py`. A fresh config gets the
  defaults (`all`, `releases`, `directory` — `DEFAULT_ENDPOINTS`). Configs
  migrated from the old env-style `~/.config/jira/config` got one extra job
  per project (`KAN`, `SAM1`, …). After that the file is yours: add / edit /
  delete jobs in **Jira Config** (each job page shows *Defined in
  `config.json › endpoints › NAME`* and an **Open config.json** button), or
  edit the file directly.
- **Who runs them:** launchd `com.jira.poll` wakes `jira/jira_poll.py` every
  60 s; it polls only the jobs whose `window` is due and writes their files.
- CLI: `python3 jira/jira_config.py --show` (token masked), `--path`,
  `python3 jira/jira_poll.py --describe` (every job's schedule, JQL and curl).


### SwiftTerm (fetched at build time for the embedded terminal)

The embedded terminal drawer (`config.terminal` in commands.toml) uses
[SwiftTerm](https://github.com/migueldeicaza/SwiftTerm) (MIT). It is **not
committed** — `bin/ensure-swiftterm.sh` shallow-clones the pinned upstream
commit from `install.conf` (`SWIFTTERM_PIN`), applies
`patches/swiftterm-cellstorage-cache.patch` (the perf fix the app was tested
with) and generates the two `Generated/*.swift` files upstream builds. It
fetches into `install.conf`'s `SWIFTTERM_DIR` — by default **`../SwiftTerm`, a
sibling of this repo, so the checkout never lives in the git tree**. It runs
automatically before the SwiftTerm compile in `bin/build-app.sh`, so a normal
build just works:

```bash
./ws build            # first run fetches + precompiles SwiftTerm, then builds
bin/ensure-swiftterm.sh   # fetch/refresh it alone (no-op when already pinned)
```

Needs network only when the pinned checkout is missing or the pin changed.
`PopupWindow.swift` imports SwiftTerm; `bin/build-app.sh` compiles
`$SWIFTTERM_DIR/Sources` + `Generated` into a static lib (see `build_term_lib`).

### Nerd font

The terminal renders with **Hack Nerd Font** (installed by the installer via
`font-hack-nerd-font` cask). Override in `commands.toml`:

```conf
[app]
terminal-font = Hack Nerd Font
```

### Shell in the terminal drawer

```conf
[app]
shell = /opt/homebrew/bin/bash
shell-args = --login -i      # login + interactive: sources profile AND rc
```

The shell auto-restarts if you `exit` it (poll + delegate), and the drawer
grows with the window.

## Uninstall

```bash
./UNINSTALL.sh
```