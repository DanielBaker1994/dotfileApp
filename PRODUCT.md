# Product

<!-- impeccable:product-schema 1 -->

## Platform

macos

Native macOS app: Rust + AppKit (objc2), Apple silicon, minimum macOS from
`install.conf` `MACOS_MIN`. A menu-bar app (status item, no Dock icon unless
`[app] dock-icon = true`). Linux and Windows were dropped in Nov 2026. None of
the `web` / `ios` / `android` values fit; treat platform guidance as macOS
desktop (AppKit, Human Interface Guidelines for macOS), never web or mobile.

## Users

- **Primary: the owner.** A developer who works all day in a tiling-window
  setup (a tiling window manager, borders, Ghostty, herdr, nvim) and wants a command
  palette, screenshots, recent files and small text tools a hotkey away, with
  files, Jira, Confluence and an AI writing helper one step further. They
  choose what gets built, based on what they use every day.
- **Later: other people on other Macs.** Colleagues or anyone who installs
  the DMG. The notarized DMG, `commands.default.toml` (personal settings
  stripped, Jira off) and the Setup & Health Check window exist for them.
  They matter, but they do not set priorities yet.

## Product Purpose

**Name.** The product is **Bansho** (canonical since Nov 21, 2026): every
name a user sees says Bansho (bundle display name, status menu, About,
palette row, tooltips). **kitchen-sink** is the technical name and stays on
purpose: the repo, the executable and bundle (`kitchen-sink.app`), the bundle
id `dev.danielbaker.kitchen-sink` (TCC grants hang off it), the CLI, the
config and cache paths (`~/.config/kitchen-sink`, `~/.cache/kitchen-sink`) and
the sockets. Write Bansho in user-facing text and kitchen-sink for code,
paths and commands.

Bansho puts the tools a keyboard-driven developer reaches for all day
behind two hotkeys. Hyper+S opens a palette of tools; Hyper+X takes a screenshot. Both must feel instant and never move
the user's focus. Success: the owner reaches for it before any separate app
(Raycast-style launchers, Flameshot, Finder's Recents, a JSON formatter,
browser Jira or Confluence) because it is faster, never loses focus or
context, and always does what the keyboard expects.

## Positioning

In priority order:

1. **Hyper+S and Hyper+X are the product.** They belong to the quick agent, a
   small resident process (`kitchen-sink quick`) that works with the main app
   not running and starts at login. A warm press is one socket write; speed
   and focus behaviour here come before anything else.
2. **Keyboard speed and reliability.** Esc, edit shortcuts and focus hand-back
   behave the same way in every panel and view.
3. **At home in a tiling workflow, tied to none.** No window-manager
   dependency (AeroSpace support was removed in Nov 2026): it never moves or
   lays out windows, so any tiling manager can place it.
4. **One shared window for the bigger tools.** Files, Jira, Confluence and AI
   are views of one window (Ctrl+Tab and header icons switch them; Ctrl+B W
   is the view switcher). It has no hotkey of its own: it is reached from the
   menu-bar item, the palette, or a "reveal" from another tool.

## Operating Context

- Summoned by global Hyper hotkeys (caps lock via Karabiner) that the app's
  resident quick agent registers itself (`[app] quick-hotkey-*`): Hyper+S
  palette, Hyper+X screenshot, Hyper+/ ws-settings. It is used in short bursts between other
  work and dismissed with Esc or the hotkey.
- Lives alongside Ghostty plus herdr panes, nvim, Webex, Outlook, and a
  corporate Jira and Confluence (Server/DC or Cloud).
- Configured through `commands.toml` plus a personal override in
  `~/.config/kitchen-sink-local/` (hand-edited or through the ws-settings
  TUI). Themes, presets and header styles switch live.
- Installed two ways: from the repo checkout (`./ws build`) or from the DMG
  (everyone else; `./ws dmg` notarizes unless `--no-notarize`). The hotkeys
  come with the app (the quick agent's login item), so a DMG install needs
  no hotkey daemon; "Hyper" itself is a Karabiner caps-lock mapping, or set
  `quick-hotkey-*` to plain combos.

## Capabilities and Constraints

- **Quick agent (Hyper+S / Hyper+X):** the palette (`[app] palette`: Find
  Recent Files, Create File, Text to Table, Pretty Print, Pane Shot (shown only
  with herdr installed), Find Window PID); the screenshot overlay and editor
  (Copy Text with a review card, QR codes, merge several pictures, record,
  retake, recents); pane-shot (a terminal pane with its scrollback as a
  picture); Find Recent Files (a file browser with the same actions as the
  main Files view).
- **Main app:** the shared window's views (Files with Recent / Arrived, Jira
  lists / detail / releases / config, Confluence search, AI), health checks,
  setup, and the same pane-shot code as the quick agent.
- Palette panels and tool panels never activate the app or bring the shared
  window with them. The window manager decides float or tile; the app
  never changes the layout.
- One feature, every surface: a feature reaches both the quick agent and the
  main app (and every entry into a tool) through one implementation in
  `ks-core`, never a copy (`rule.md` rule 7).
- Every text input honors Cmd+C/V/X/A/Z and Ctrl+C/V (rule 1). Right-click
  menus in file lists offer the obvious actions (rule 2).
- The AI view depends on Apple's on-device model (`[ai] fm-bin`). Without it
  the view is off and everything else still works.
- Jira and Confluence never query outside the user-typed project or space
  scope.
- Tests drive the app or unit-test code (socket `state` / `do:` hooks, the
  headless MCP sandbox, GUI flows in a macOS VM). Source-grep "tests" don't
  count.

## Evidence on Hand

- Specs (history): `git show 64bbd4a^:PRD-screenshot.md`, `PRD-settings-hub.md`.
  Agent and system docs: `AGENTS.md`; test strategy: `TESTING.md`; open
  items: `BACKLOG.md`.
- Tests: `cd rust && cargo test` (~700), Python suites under `Tests/` via
  `bin/run-tests.sh`, GUI flows `Tests/run_flows.py` in the VM (`bin/ks-vm`).
- Hotkey latency is logged per press (`hotkey: … recv->shown`, `screenshot:
  overlay shown in … capture landed …` in `logs/YYYY_MM_DD/daemon.log`).
- App icons and view icons (`[app] *-icon`, `confluence_icon.png`).
- Missing (do not invent any of these): user counts, testimonials, external
  users, pricing, license terms, public marketing site.

## Product Principles

1. **Instant, or it isn't used.** Hyper+S and Hyper+X are judged in
   milliseconds from the key press, cold starts included. Prewarm, cache and
   keep the agent resident; never add work to the press path.
2. **Keyboard-first.** Every action has a key, and the mouse is optional.
   Esc follows one predictable chain: close the innermost thing first.
3. **Never steal focus.** Showing, hiding and tool panels leave the user
   where they were: no surprise activation, no workspace jumps, no typed keys
   landing in the wrong app.
4. **Config over code.** User-facing strings, sizes, paths and switches live
   in `commands.toml` and are reachable from ws-settings. Validation is the
   app's own (`config-check`).
5. **One of everything, less of it.** One implementation per feature shared
   by every surface, one opener, one codec, one theme path. Delete before
   adding; a new feature joins the shared system instead of growing a
   parallel one.
6. **Owner's daily use decides; others get sane defaults.** Build for the
   owner's real workflow, and keep the DMG defaults and first run working
   for someone who has none of the owner's setup.
