# Product

<!-- impeccable:product-schema 1 -->

## Platform

macos

Native macOS app: Swift / AppKit, an accessory (menu-bar) app with no Dock
icon and no Edit menu. Minimum macOS from `install.conf` `MACOS_MIN`, Apple
silicon only for the app install. None of the `web` / `ios` / `android`
values fit; treat platform guidance as macOS desktop (AppKit, Human
Interface Guidelines for macOS), never web or mobile.

## Users

- **Primary: the owner.** A developer who works all day in a tiling-window
  setup (AeroSpace, sketchybar, borders, Ghostty, herdr, nvim) and wants
  notes, files, Jira, Confluence, an AI writing helper, diffing and
  screenshots a hotkey away. They choose what gets built, based on what they
  use every day.
- **Later: other people on other Macs.** Colleagues or anyone who installs
  the DMG. The notarized DMG, `commands.default.toml` (personal settings
  stripped, Jira off) and the Setup & Health Check window exist for them.
  They matter, but they do not set priorities yet.

## Product Purpose

workspace-switcher puts the tools a keyboard-driven developer reaches for
all day into one hotkey-summoned window. It sits beside a tiling window
manager and does not fight it. Success: the owner reaches for it before any
separate app (browser Jira or Confluence, Beyond Compare, Flameshot,
Raycast-style launchers, a notes app) because it is faster, never loses
focus or context, and always does what the keyboard expects.

## Positioning

All three of these together; no single one leads:

1. **One window replaces many.** Notes (nvim pane), files, Jira, Confluence,
   AI, Compare (text and folder) are views of ONE shared window, switched
   with Ctrl+Tab and header icons. /screenshot, /paths, /filefast and
   /pane-shot are tool panels next to it.
2. **Keyboard speed and reliability.** Hyper+N shows or hides it in tens of
   milliseconds. Esc, edit shortcuts and focus hand-back behave the same way
   in every view.
3. **Native to a tiling workflow.** Built around AeroSpace's model (focus
   files, closed-windows cache, workspace placement) and sketchybar, with
   workspace awareness and a notifications pill.

## Operating Context

- Summoned by global Hyper hotkeys (caps lock via Karabiner) bound in
  `aerospace.toml`: Hyper+N window, Hyper+S palette, Hyper+X screenshot,
  Hyper+/ ws-settings. It is used in short bursts between other work, and
  hidden again on Esc (when enabled), the hotkey, or focus loss.
- Lives alongside Ghostty plus herdr panes, nvim, Webex, Outlook, and a
  corporate Jira and Confluence (Server/DC or Cloud).
- Configured through `commands.toml` (hand-edited or through the ws-settings
  TUI). Themes, presets and header styles switch live.
- Installed two ways: from the repo checkout (`./build.sh`, the owner) or
  from the DMG (everyone else).

## Capabilities and Constraints

- Views of the shared window: notes, files (Recent / Arrived), AI, Jira
  (lists, detail, releases, Jira Config), Confluence search, Compare.
  Tool panels: /screenshot (Flameshot-style, OCR Copy Text), /paths,
  /filefast, /prettyprint, /health-checks, /pane-shot.
- Tool panels must never activate the app or bring the shared window with
  them. AeroSpace decides float or tile; the app never changes the layout.
- Every text input honors Cmd+C/V/X/A/Z and Ctrl+C/V (`rule.md` rule 1).
  Right-click menus offer the obvious actions (rule 2).
- The AI view depends on Apple's on-device model (`fm`). Without it the
  view is off and everything else still works.
- Jira and Confluence never query outside the user-typed project or space
  scope.
- Tests must drive the app or unit-test code (socket `state` / `do:` hooks).
  Source-grep "tests" don't count.

## Evidence on Hand

- Specs: `PRD-compare.md`, `PRD-screenshot.md`, `PRD-settings-hub.md`,
  `PLAN-settings-hub.md`. Agent and system docs: `AGENT_CONTEXT.md`.
- Test suites under `Tests/` and `bin/run-tests.sh`, plus UI drivers
  `bin/ui-test.sh` and `bin/ui-test-focus.py`.
- App icons and view icons (`[app] *-icon`, `confluence_icon.png`).
- Missing (do not invent any of these): user counts, testimonials, external
  users, pricing, license terms, public marketing site.

## Product Principles

1. **Keyboard-first.** Every action has a key, and the mouse is optional.
   Each view lists its keys (Cmd+/, `[shortcuts]`), and Esc follows one
   predictable chain: close the innermost thing first.
2. **Never steal focus.** Showing, hiding and tool panels leave the user
   where they were: no surprise activation, no workspace jumps, no focus
   left behind on a workspace the user has left.
3. **Config over code.** User-facing strings, sizes, paths and switches
   live in `commands.toml` and are reachable from ws-settings. Validation is
   the app's own (`config-check`).
4. **One of everything.** One window, one opener, one codec, one theme path.
   A new feature joins the shared system instead of growing a parallel one.
5. **Owner's daily use decides; others get sane defaults.** Build for the
   owner's real workflow, and keep the DMG defaults and first run working
   for someone who has none of the owner's setup.
