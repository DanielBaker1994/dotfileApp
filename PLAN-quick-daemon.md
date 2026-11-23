# PLAN — Hyper+S / Hyper+X independent of the main app, plus responsiveness

Status: Phase 0 landed (e503871, 63f038f); Phase 1 implemented 2026-11-23
(quick agent, `app/quick.rs`; awaiting live test + login-item install); Phases 2-3
not started. Proposed 2026-11-23. Owner request: Hyper+S (palette) and Hyper+X
(screenshot) must work with the main kitchen-sink (Bansho) app not running;
treat them as a shared module; make the app feel faster.

## What exists today (measured / read from code)

- Hyper+S → `bin/kitchen_sink.sh` → `kitchen-sink toggle` → socket →
  `ControllerInner` → `ui_push(UiCommand::ShowPalette)` → **30 ms NSTimer**
  `DaemonUi::drain` → `apply_pending` → `show_palette`.
- Hyper+X → `kitchen-sink screenshot` → socket → `screenshot_message` → same queue.
- Process spawn + socket round trip is **10–20 ms** (measured with
  `WS_PING_ONLY=1`). The bash wrapper is ~0. **Delivery is not the problem.**
- Latency is inside the daemon:
  1. Every UI command waits for the next 30 ms tick (avg 15 ms, worst 30 ms),
     *plus* whatever that tick runs first.
  2. Each tick runs ~10 polls on main before/after commands:
     `poll_files_dir`, `sync_tools`, `sync_member_states`, `watch_focus`,
     `live_pane_refresh`, `refresh_workspace_strip`, `refresh_files_recent`,
     `refresh_jira`, `poll_conf_hits`, `poll_ai`, `visible_window_count`. A
     slow one delays the palette; all of them run 33×/s even when hidden.
  3. `show_palette` / `open_tool` call `state.get_or_insert_with(build_window)`:
     the **first** palette or tool after launch builds the whole shared window
     (files/jira host) although neither shows it. A cold Hyper+S pays for it.
  4. Palette/screenshot share the main thread with Jira, Confluence, AI,
     terminal: any main-thread stall there stalls the hotkey.
- Coupling: palette + tool panels + screenshot live in `DaemonUi`
  (`app/host.rs`, 13k lines) and `SwitcherController`. Screenshot reaches
  `app::host::screenshot_saved_to_path_shelf`, `app::registry`,
  `views::file_popup`, `engines::config_text` (Python helper). Tools reach
  `python_helper`, `process_run`, `menu::pop_status_menu`. Switcher reaches
  `app::hotkey` (AeroSpace). UI framework (`ui::popup/theme/chrome/style/
  list_panel/view_base/toast`) and `platform::*` are shared.

## Pushback / constraints

1. **"Without the main app running" still needs *a* process.** A hotkey that
   cold-starts AppKit via LaunchServices costs ~0.3–1 s every press. So the
   design is a small **separate resident agent** ("quick agent") that owns
   Hyper+S and Hyper+X, started at login / on first press, and has its own
   lifecycle. The main app can be quit, crash or be rebuilt and the hotkeys
   keep working.
2. **TCC (Screen Recording).** A new bundle ID needs its own grant (a manual
   click). Phase 1 therefore runs the agent as **a mode of the same signed
   bundle** (`open -n -g kitchen-sink.app --args quick`), so it inherits the
   existing grant. A separate smaller binary/nested helper bundle is a later,
   optional step (Phase 3) and costs one grant click.
3. **Shared lib first is the wrong order.** Moving 10k-line `screenshot.rs` +
   the UI framework into a lib crate before the runtime split works has no
   user-visible payoff and guesses the seams. Split the runtime first (Phase
   1); the compiler + the agent's actual imports then show the real seam for
   the crate split (Phase 2).
4. **Picks that need the main app** (Kitchen Sink, Files/Jira/Confluence/AI
   views, terminal, jira-config) are forwarded to the main socket; if no main
   daemon answers, the agent cold-starts it (`bin/kitchen_sink.sh <mode>`).
   That pick is slow on a cold main app; that is expected.
5. **No Python pool in the agent.** `PythonHelper::shared()` spawns workers;
   the agent must read config with the pure-Rust scans
   (`views::tools::section_vars`, `pathfmt::section_value`) and only spawn
   Python for the text-table / prettyprint tool runs that already shell out.
6. **Two writers of `~/.cache/kitchen-sink/paths.json`** (shelf: screenshot
   adds in the agent, Paths view in the main app). `engines::path_shelf` has
   `own_write` + reload; verify reload-on-mtime in both processes and use
   atomic write (tmp + rename).
7. **Hyper+S never activates** (AGENTS.md rule) — the agent is an Accessory
   app with non-activating panels; same `PALETTE_RETURN` hand-back.

## Phases

### Phase 0 — main-app responsiveness (independent, do first)

- **Wake on enqueue.** In `ControllerInner::ui_push` (and `post_main` /
  `run_on_main`'s queue push) also `dispatch2` an async main-queue block that
  calls the UI's `apply_pending` *commands-only* path immediately (coalesce
  with an `AtomicBool` "wake pending" so a burst schedules one block). The
  timer stays for polls.
- **Split polls from commands.** `apply_pending` → `apply_commands()` (UI
  queue + main jobs, cheap) and `poll_tick()` (the ~10 polls). Timer: 30 ms
  while the shared window or any panel is visible, 250 ms when everything is
  hidden (re-arm on visibility edges). Keep `watch_focus` semantics (it
  samples edges; verify hide-on-focus-loss / self-activation still work).
- **No `build_window` on palette/tool paths.** Palette, tool panels, terminal
  panel, paths must not construct the shared window; make
  `DaemonUiState.window` lazy (or move palette/tools/paths into a separate
  `PanelState` not tied to `build_window`).
- **Prewarm** the palette panel (build `PalettePanel` + icons once, hidden)
  shortly after launch, off the critical path.
- **Instrument.** Log `hotkey: <msg> recv→shown <ms>` to daemon.log (socket
  receive timestamp carried in the command; stamp when the panel is
  ordered front). This is how we measure real presses without driving UI.
- Accept: `cargo test` green; daemon.log shows palette recv→shown; idle CPU
  of the hidden daemon drops (check `ps -o %cpu` over 30 s before/after).

### Phase 1 — the quick agent (runtime independence)

- New argv mode `quick` in `main.rs`: own socket `$TMPDIR/ks-quick.sock` +
  lock, `NSApplicationActivationPolicy::Accessory`, no status item, no main
  menu (or a minimal one for edit shortcuts — rule 1), **no
  `SwitcherController`, no shared window, no Python helper pool**.
- New module `app/quick.rs`: `QuickUi` owning `PalettePanel`, `ToolPanel`s
  (paths, filefast, text-table, prettyprint), find-window, and the
  `ScreenshotController` + overlay; its own key monitor routing (reuse
  `ui::popup::route_key`); its own wake-on-enqueue main queue (no 30 ms timer
  at all — event driven).
- Socket verbs: `toggle` / `show` (palette), `screenshot\t…` (same words as
  today), `state` (JSON: palette/tools/screenshot visible), `ping`.
- Palette picks:
  - agent-owned tools → open locally;
  - workspaces → AeroSpace IPC locally;
  - views / Kitchen Sink / terminal / jira-config → forward the launch message
    to `ws-notes.sock`; if no answer, exec `bin/kitchen_sink.sh <mode>`.
- Seams to cut (replace with small traits/fns the agent and main app both
  implement): `screenshot_saved_to_path_shelf` (→ `engines::path_shelf`
  direct add), `views::file_popup::show` (agent forwards "reveal" to main),
  `app::registry` use in screenshot (screenshot `do:` hooks register only in
  the main app), `engines::config_text` (→ pure-Rust reads in the agent).
- Launch: `bin/kitchen_sink.sh` and the AeroSpace bindings for Hyper+S /
  Hyper+X talk to `ks-quick.sock` first; cold start = `open -n -g
  kitchen-sink.app --args quick` (LaunchServices → TCC). Login: a
  LaunchAgent / login item that runs the same `open` (verify TCC attribution).
  `kitchen-sink quit` keeps the agent; `kitchen-sink quick-quit` stops it.
- Main app: Hyper+S/X no longer route to it. Keep its in-window entry points
  (Ctrl+B W, Cmd+K, `do:screenshot:*`, socket `screenshot` for tests) working
  — either keep the code paths or forward to the agent; decide per verb.
- Accept: with the main daemon killed, Hyper+S shows the palette and Hyper+X
  starts a capture; picks of agent tools work; a view pick cold-starts the
  main app. With both running, no double palettes. TCC grant reused.

### Phase 2 — shared crate (`rust/ks-quick-core`)

Move what the agent actually imports (from Phase 1's `use` list):
`platform::*`, `ui::{popup, theme, chrome, style, list_panel, view_base,
toast, menu_kit, preview_ring}`, `views::{switcher (palette half), tools,
paths, screenshot}`, `engines::{screenshot_*, pane_shot, path_shelf,
list_filter, switcher_status, recent_files}`, `app::{hotkey (AeroIpc),
paths, config (pure parts)}` into a lib crate; `ws-rs` depends on it.
Enforced boundary: the lib crate cannot see `host.rs`. Optional Phase 3: a
separate `ks-quick` binary in a nested helper bundle (smaller, faster cold
start; costs one Screen Recording grant).

## Execution

Sequential (every phase touches `host.rs` / `main.rs`), committed on `main`.
Implementers: Sonnet agents, one phase at a time, orchestrator reviews each
diff. DeepSeek flash is a poor fit for subtle AppKit/threading edits in a 13k
line file; it fits the post-phase exercise/test runs (TESTING.md playbook).
