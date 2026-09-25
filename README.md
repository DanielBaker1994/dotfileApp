# workspace-switcher

A macOS app: AeroSpace switcher popup + notes / voice-to-text / Jira windows,
with sketchybar + borders menu-bar stack. Native AppKit (Swift), no runtime
dependencies beyond what the installer brings.

## Install (end user — pick one)

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
`~/workspace-switcher`), clones the repo, and continues the install from the
clone:

```bash
bash INSTALL.sh
```

`INSTALL.sh` prints every step as it runs (checks your Mac, installs
Homebrew/deps, installs configs, compiles the app, grants mic + speech
permissions, starts sketchybar + borders, loads the jira poll agent, opens
the app). Re-running it is safe. Uninstall: `./UNINSTALL.sh`.

Repo: `https://github.com/DanielBaker1994/dotfileApp.git`

### What you get

| Thing | Where |
| --- | --- |
| Switcher popup | Hyper+S (Karabiner) |
| Notes / Jira / Voice / Health windows | menu-bar **wrench** icon |
| Voice notes | red record button → dictation → text, pause/resume, live draft |
| Menu-bar stack | sketchybar + borders (brew services) |
| Jira window | Hyper+J (aerospace) — spreadsheet table, columns from `commands.conf [jira] columns` |
| Jira poll on/off + options | wrench menu → **Enable Jira** / **Disable Jira** (asks whether to keep polling in the background) / **Toggle Jira Window** / **Open Jira Config Window** (poll jobs, schedules, columns, Force Poll, Setup…) |
| Jira poll agent | launchd `com.jira.poll` (60s tick; `jira_poll.py` runs only due endpoints) |

## Development

```bash
./INSTALL.sh help       # the one command (install / uninstall)
./build.sh              # dev: compile workspace-switcher.app + TCC re-grant
./build.sh --build-only # dev: compile + grant, do NOT launch
```

- `PopupWindow.swift` — reusable AppKit popup framework (windows, chrome,
  rows, filters, editor, embedded terminal, record meter)
- `workspace_switcher.swift` — host app: commands.conf parsing, aerospace IPC,
  icons, voice recorder (AVAudioEngine → SFSpeechRecognizer)
- `main.swift` — entry point
- `commands.conf` — every user-facing string + window definition (the app is
  config-driven; new windows need no code)
- `bin/workspace_switcher.sh` — hotkey launcher (pings the daemon, launches via
  LaunchServices so mic/speech TCC grants attach to the app bundle)
- `bin/voice-permissions.sh` — writes the mic + speech TCC grants
- `jira/` — python poller (stdlib only): `jira_api.py` (API client, JQL
  queries, `--sync`, curl logging), `jira_poll.py` (per-endpoint scheduler,
  lock, publish), `jira_config.py` (config.json load/migrate/validate +
  `[jira] columns` → API `fields=`), `jira_status.py` (status.json);
  `jira-api.sh` / `jira-poll.sh` are thin compat wrappers; `jira-doctor.sh`
  (health checks, `/health-checks` window). Tests: `python3 Tests/test_jira_poll.py`

### Jira poller at a glance

| What | Where |
| --- | --- |
| On/off switch | `commands.conf [jira] enabled` (menu **Enable Jira** / **Disable Jira**; `poll-when-disabled = true` keeps the poller running while disabled; or `bin/workspace_switcher.sh jira-poll on\|off\|toggle\|setup`) |
| Credentials + **poll jobs** (schedules) | `~/.config/jira/config.json` → `endpoints` (chmod 600; edited by the **Jira Config** window) |
| Team schema (custom fields, JQL templates, …) | `~/.config/jira/team.json` (optional; example `jira/team.example.json`) |
| Live state (last/next run, errors, lock) | `~/.cache/jira/status.json` · `jira/jira_status.py` · `jira-doctor.sh` |
| Every HTTP request, copy-pasteable | `~/.cache/jira/curl.log` (chmod 600 — contains the basic-auth token) |
| Window data | `~/.cache/workspace-switcher/jira_json/<endpoint file>.json` |

### Where the poll jobs (and their JSON files) come from

Every tab in the Jira window is a JSON file written by one **poll job**, and
every poll job is one entry of `endpoints` in `~/.config/jira/config.json`:

```jsonc
{
  "site": "https://you.atlassian.net", "email": "…", "token": "…",
  "outDir": "~/.cache/workspace-switcher/jira_json",
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


### Vendored SwiftTerm (one-time pull for the embedded terminal)

The embedded terminal drawer (`config.terminal` in commands.conf) uses
[SwiftTerm](https://github.com/migueldeicaza/SwiftTerm) (MIT). The tested copy
is vendored at `Vendor/SwiftTerm` — the one-time pull:

```bash
git clone https://github.com/migueldeicaza/SwiftTerm.git Vendor/SwiftTerm
# pin to the commit the vendored copy was tested with (see git log there if
# re-vendoring), or keep the checked-in copy — it builds as-is via:
./build.sh
```

`PopupWindow.swift` imports SwiftTerm; the build script compiles
`Vendor/SwiftTerm/Sources` implicitly through the `@_spi`/module import
(see `bin/workspace_switcher.sh` for the exact swiftc invocation).

### Nerd font

The terminal renders with **Hack Nerd Font** (installed by the installer via
`font-hack-nerd-font` cask). Override in `commands.conf`:

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