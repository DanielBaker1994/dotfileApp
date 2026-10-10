# Agent prompt: continue the Swift→Rust port (kitchen-sink) — next wave

You are continuing an in-flight port. Everything is on disk; read the current
state before writing anything. Repo: `/Users/danielbaker/.config/kitchen-sink`
(the REAL repo — `git rev-parse --show-toplevel` first; `rule.md` forbids git in
backups). The previous session's history is gone — the facts below are the
handoff.

## Read first (in order)
1. `PLAN-rust-port.md` — the plan and the **Progress** section (the source of
   truth; updated at the end of the last wave, including the "Wave notes").
2. `AGENTS.md` (repo root, freshly refreshed) — repo map, rules, build/test
   commands. `AGENT_CONTEXT.md`/`CLAUDE.md` are legacy.
3. `rust/ws-rs/src/app/host.rs` — the controller **and** the new daemon UI
   bridge (`UiCommand`/`UiQueue`, `install_ui`, `nav_clicked`, `build_palette`,
   the macOS `daemon_ui` module with `install_daemon_ui`, `WSDaemonUi`,
   `HostRootView`, and `content_for`).
4. `rust/ws-rs/src/main.rs` — daemon startup: helper config, menu/status item,
   UI bridge install, default view, the `prose` CLI path.

## Where things stand (end of the last wave)
- **The daemon shows the window.** On daemon start (and on a launch message /
  `do:open:*`) it builds a real `SlotHostWindow` with a `PopupChrome` header,
  the view-switcher buttons (routed through `SwitcherController::nav_clicked`),
  the ✕ / Cmd+W close path, the AppKit main menu, a status item, and
  `.regular` activation policy. `DEFAULT_VIEW = SlotView::Notes`; the window is
  presented on a bare start **by design** (so fresh-start `state.view/visible`
  differ from the hidden Swift daemon — the parity tool shows only this).
- **Real content surfaces are wired** in `host.rs::content_for`:
  `views::notes::build_content`, `views::files::build_content`,
  `views::jira::build_content`, `views::compare::build_content` (all
  `#[cfg(target_os = "macos")] pub fn build_content(mtm) -> Option<Retained<NSView>>`).
  Confluence / AI still get the themed placeholder (their modules build their
  own windows instead of an embeddable surface). `views/paths.rs` has a
  `build_content` too but paths is a tool panel, not a `SlotView`.
- **Landed last wave** (all tested): jira NSView tree (`JiraViewController::build`),
  the compare drawing layer (`ComparePaneView`, `CompareEditor`,
  `CompareThumbnail`, `FolderTreeView`), notes/files/paths surfaces +
  `ProseWindow`/`ProseProcess`, AI `runPart` streaming (`fm respond --stream`,
  `poll_stream`, cancel/kill), screenshot overlay panels (`ShotOverlayPanel`)
  + JPEG encode + pasteboard copy. `config-schema`, palette-command population,
  `__info_plist` (new `rust/ws-rs/build.rs`), the `prose` one-shot, and
  `reload`'s `loadCommands().count` are all done.
- **Verification:** `cd rust && cargo test` → **540 green** (535 ws-rs + 5
  helpers); `cargo build` **warning-free**; `bin/build-rust-app.sh --force`
  builds a signed `.app` at `.build/rust-app/` with `__TEXT,__info_plist`
  embedded. `bin/rs-parity.py --rust-sock … --actions state` vs the live Swift
  daemon: **0 missing keys**; `paletteCommands` matches exactly; `escHides` /
  `hideOnFocusLoss` / `headerStyle` match.
- **Everything is UNCOMMITTED** (the whole Rust tree is staged in the index +
  working-tree changes; `rust/ws-rs/build.rs` is new/untracked). Do NOT commit.

## Commands
```bash
cd rust && cargo test                 # workspace tests (must stay green)
cd rust && cargo build                # must stay warning-free
./bin/build-rust-app.sh --force       # -> .build/rust-app/kitchen-sink.app (signed)
# run the daemon (dev: point the python helper at the checkout's pylib):
D=$(mktemp -d); TMPDIR="$D" WS_RS_ASSET_DIR="$PWD" \
  .build/rust-app/kitchen-sink.app/Contents/MacOS/kitchen-sink &
printf 'state\n' | nc -U "$D/ws-notes.sock"
python3 bin/rs-parity.py --rust-sock "$D/ws-notes.sock" --actions state  # read-only
```
- **NEVER run `bin/rs-parity.py`'s default `do:` actions against the real
  `$TMPDIR/ws-notes.sock` while the owner's Swift app is up** — it changes their
  UI. `--actions state` only, or use an isolated `TMPDIR` for the Rust daemon.
- **Do not `pkill -f kitchen-sink`** — the owner's live Swift app matches.
  Kill the Rust one by path: `pkill -f "target/debug/kitchen-sink"` or
  `pkill -f "rust-app/kitchen-sink.app"`.
- `cargo test` does **not** rebuild the plain binary — run
  `cargo build -p ws-rs` before launching `target/debug/kitchen-sink`.

## Remaining work (each item = one file/area)
1. **`ui/popup.rs` `install_local_monitor`** — the last big spine seam: wire the
   `NSEvent::addLocalMonitorForEvents` keyDown monitor to route keys into
   `route_key`/`handleKey` (mirror `PopupWindow.installMonitors`). The shared
   window currently has no keyboard routing (Esc, Cmd+W via menu works, but no
   in-window keys). This is the key blocker for the UI suites.
2. **Confluence / AI embeddable surfaces** — `views/confluence.rs`,
   `views/ai.rs`: add `build_content(mtm)` (the WKWebView surfaces exist; make
   an embeddable tree) and wire them in `host.rs::content_for`.
3. **`engines/ai_format.rs` `RichText::rtf` / `RichText::copy`** — AppKit
   `NSAttributedString` + `NSPasteboard` (two documented `todo!()`s).
4. **Screenshot overlay key monitor** (`views/screenshot.rs`) — the overlay
   panels exist; key routing (Esc/tools) still goes only through `do:key:`.
5. **Small / optional**: wire `ProseProcess::launch` from a notes UI control;
   `views/paths.rs` `PathsWindow::build` is implemented but nothing shows it
   (paths is opened via `do:paths:*` / the `[paths]` command — check the Swift
   `showPaths` path before wiring).
6. **Then the Phase 4 gate** (do NOT start until the UI suites pass against the
   Rust binary — see `PLAN-rust-port.md`): run `bin/ui-test.sh`,
   `bin/ui-test-vim.sh`, `bin/ui-test-focus.py`, the `run-tests.sh` suites and
   `test_install.sh` against the Rust binary; run the `code-review` + `deslop`
   skills over the branch; flip `./ws` / `bin/build-app.sh` to cargo; migrate
   `notify/notify_poll.py` to the Rust AX helpers (`rust/ws-helpers`); delete
   the Swift tree.

## How to work (the owner's directives)
- Spawn **cheap `general` subagents** (`opencode-go/deepseek-v4.1-flash` worked
  well last wave), one per file/area, **in parallel** where files are disjoint.
  Each agent owns a fixed list of files and must NOT edit anything else
  (`main.rs`, any `mod.rs`, `Cargo.toml`, `host.rs`, or another agent's module).
  Prefer to own `main.rs`/`host.rs`/`registry.rs`/`Cargo.toml` yourself.
- Every agent must keep `cargo test` green, run `cargo build`, and add tests for
  pure logic. AppKit calls are `#[cfg(target_os = "macos")]`; guard
  `MainThreadMarker::new()` and never panic in ObjC callbacks.
- Keep the socket contract byte-compatible (`state` keys, `do:` grammar);
  Python is frozen; `commands.toml`'s format is frozen.
- Config over code; user-facing strings live in `commands.toml`.
- Do NOT commit; leave changes uncommitted for review. Update the **Progress**
  section of `PLAN-rust-port.md` when you finish.

## Gotchas learned last wave
- The off-screen 500×500 `TUINSWindow` per `NSTextView` is AppKit's text-input
  services window — benign, invisible, not counted in `state.windows`. When
  screenshotting, pick the 1100×640 window from `CGWindowListCopyWindowInfo`
  (filter by the daemon PID; get the PID via `lsof -U | grep <sock>`).
- `config_text::tri` calls the Python helper — do not use it for cold-start
  parsing. `host.rs` has `tri_pure` for raw-text scans (palette gating).
- The daemon UI is a 30 ms `NSTimer` draining the `UiCommand` queue on the main
  thread; tests stay headless (no `install_ui` -> no UI commands).
- `install_status_item`'s handle is intentionally leaked; `menu.rs` builds the
  status/main menus.

## Definition of done for a wave
`cd rust && cargo test` green, `cargo build` warning-free, the Rust `.app`
visibly shows the shared window and answers `state`/`do:` identically to the
schema in `PLAN-rust-port.md`'s Progress (fresh-start `view`/`visible` differ
by design — the Rust daemon presents the default view).
