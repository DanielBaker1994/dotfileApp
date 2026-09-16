# workspace-switcher

A macOS app: AeroSpace switcher popup + notes / voice-to-text / Jira windows,
with sketchybar + borders menu-bar stack. Native AppKit (Swift), no runtime
dependencies beyond what the installer brings.

## Install (end user — pick one)

### Option A — .pkg installer (true macOS click-through) ← recommended

```bash
./INSTALL.sh pkg
```

Builds and opens the package → **native macOS Installer wizard** (Continue →
Install → your password → Done). The package carries the whole repo and runs
the full install as your user.

### Option B — DMG (GUI wizard)

```bash
./INSTALL.sh dmg
```

Builds and opens the DMG → **double-click Installer.app** (SwiftUI wizard)
or run `./INSTALL.sh` in a terminal. The DMG carries the whole repo, so it
works from anywhere; INSTALL.sh clones a fresh copy to `~/workspace-switcher`
and installs from there.

### Option B — one script (self-cloning)

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
| Jira poll agent | launchd `com.jira.poll` |

## Development

```bash
./INSTALL.sh help       # the one command (install / pkg / dmg / uninstall)
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
- `jira/` — jira-api.sh (cache/sync), jira-poll.sh (publish window JSON),
  jira-doctor.sh (health checks, `/health-checks` window)

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