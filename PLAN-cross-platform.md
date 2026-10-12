# PLAN-cross-platform.md — kitchen-sink on Linux (orchestrator brief for DeepSeek workers)

> **Orchestrator (Claude Code / Sonnet):** you run this plan. Save this text into the
> repo as `PLAN-cross-platform.md` and commit it on `main` (`docs: cross-platform plan`).
> Workers get **§2 (rules) + §3 (the canonical example their WP names) + their one WP
> card** — nothing else. Never hand a worker the whole doc; weaker models drift when
> given choices.

## O. Orchestration mechanics (orchestrator only — never part of a brief)

You dispatch; you never implement. Each WP goes out through the bundled relay
(`opencode-delegate` skill, already installed at `~/.claude/skills/opencode-delegate`),
and you review and commit what comes back. Workers never commit (the relay never does
either) — the commit is the orchestrator's.

**Setup, once:** `command -v opencode` and `opencode auth list` show a stored provider
(`opencode-go/…` is the one to use). Docker Desktop must be running from Wave 0 on
(`docker info` must succeed; otherwise `open -a Docker` and wait) — `bin/linux-check.sh`
runs in Docker.

**Per WP:**
1. Write the brief to `.build/xplat-briefs/<WP>.txt`: §2 verbatim + the §3 example(s)
   the card names + the card verbatim — nothing else. (`.build/` is gitignored.)
2. Dispatch it as a backgrounded Bash call (`run_in_background: true`; you are notified
   when it exits):

   ```bash
   node ~/.claude/skills/opencode-delegate/scripts/relay.mjs \
     --brief .build/xplat-briefs/<WP>.txt \
     --model opencode-go/deepseek-v4.1-flash \
     --cd /Users/danielbaker/.config/kitchen-sink \
     --timeout 2h --out-dir .build/xplat-runs/<WP>
   ```

   A resumed run keeps its session's model: use `--resume-last` for rework, with only
   the delta brief on stdin.
3. On exit, read `.build/xplat-runs/<WP>/result.json` (`status`, `finalMessage`,
   `touchedFiles`, `sessionId`, `cost`) — then **re-run the card's gates yourself and
   read the diff**. The self-report is a claim, not evidence.
4. Land it: stage only that WP's files; commit on `main`, message
   `xplat: <WP id> <summary>`, ending with a trailer line
   `Worker: opencode-go/deepseek-v4.1-flash via opencode-delegate (session <sessionId>)`.
   If the review fails, send a delta brief with `--resume-last` and review again.
5. Retries: default model is `opencode-go/deepseek-v4.1-flash`; a second attempt on a
   failed card may use `opencode-go/deepseek-v4-pro` (Wave 4 may start on it — it is the
   reference implementation everything later copies). Max 2 attempts per card, then
   report to the owner.

**Wave rule:** within a wave, cards own disjoint files — dispatch them in parallel, one
backgrounded relay call each (separate `--out-dir`s), and serialize review + commit.
W0 is a single worker. After a wave's cards have all landed, run the wave gates (§5) and
append results to §8.

## 0. Context

**Why.** kitchen-sink is a macOS-only Rust app (`rust/ws-rs`, about 98k lines, AppKit
via objc2). The owner wants it cross-platform, with **Linux first**. Earlier
work (`PLAN-appkit-seams.md` WP1–WP8, all landed) moved leaf AppKit calls behind
`rust/ws-rs/src/platform/`. The **non-macOS branches of those seams have never been
compiled**, so nobody knows whether the crate builds anywhere except macOS.

**Owner decisions (2026-10-12).**
1. Linux first. Windows is a later phase (§7).
2. **Keep AppKit on macOS, pixel-identical.** Linux gets a **second frontend** over
   the same toolkit-neutral core: controller, socket API, Python helper, engines,
   view models.
3. Toolkit: this plan recommends **egui/eframe** (§1.3).
4. Python is mostly portable. The focus is Rust. Python fixes are opportunistic (WP-PY).

**What exploration found (makes this tractable).**
- `app/host.rs` already splits into the AppKit-free controller (lines 1–2597) and
  `#[cfg(target_os = "macos")] mod daemon_ui` (2598–9826). The headless daemon
  (`kitchen-sink headless`) already runs the full socket API with no AppKit.
- `app/registry.rs`, `cli.rs`, `config.rs`, `hotkey.rs`, `paths.rs`,
  `python_helper.rs`, `process_run.rs` and `socket.rs` are pure Rust. So are 11 of the
  14 engines.
- Almost every `ui/` and `views/` file is already split the same way: a **pure top
  half** (models, layout math, `Snapshot`/`Event` types) and a gated
  `#[cfg(target_os = "macos")] mod appkit|macos { … }` bottom half. Theme data
  (`Rgba`, `PopupColors`, 30 presets) is already plain Rust.
- Four views already have the ideal frontend contract, a pushed `XxxSnapshot` plus a
  reported `XxxEvent` enum: `views/files_view.rs`, `jira_view.rs`,
  `confluence_view.rs`, `ai_view.rs`.
- Seven concrete compile blockers are known (§4 Wave 1), plus ungated imports and
  `MainThreadMarker` signatures in `ui/`/`views/`.

**Consequence: much of `PLAN-appkit-seams-findings.md`'s next wave is NOT needed.**
Because macOS keeps AppKit, the egui frontend reads `Rgba`/`PopupColors` directly
and draws with its own primitives. **Do not do** FontSpec/`platform::fonts`,
`NSBezierPath` geometry extraction, attributed-string seams, or a `WebPane` trait
for macOS. Those only matter if AppKit were being replaced.

## 1. Strategy in one page

### 1.1 Order: easiest first, each wave unblocks the next

| Wave | What | Size | Parallel? | Unblocks |
|---|---|---|---|---|
| 0 | Linux check harness (Docker) + target-gate macOS deps in `Cargo.toml` | S | 1 worker | everything |
| 1 | Make `cargo check` pass on Linux. Gating only, no logic change | M total, many S WPs | yes, 8–10 workers (one per file group) | Wave 2 |
| 2 | Headless daemon + `cargo test` green on Linux; opportunistic Python fixes | S–M | yes, 4–6 workers | Waves 3–4 |
| 3 | Frontend contract: add `snapshot()`/`Event` to the views that lack them | M | yes, one worker per view | Wave 5 |
| 4 | egui frontend skeleton + the **reference view** (Files) | M–L | **1 worker, serial** (or the orchestrator) | Wave 5 |
| 5 | egui views, one per worker, all copying the Files reference | M each | yes, 6–10 workers | Wave 6 |
| 6 | Hard platform features: terminal, screen capture, notes nvim, previews | L | yes, 3–4 workers | — |
| 7 | Linux packaging + docs; Windows scouting | S–M | yes | — |

### 1.2 Cross-platform answer per macOS-only piece

| macOS piece | Linux replacement | Wave | Difficulty |
|---|---|---|---|
| SwiftTerm terminal (`views/terminal.rs`, `notes_vim.rs`) | **`egui_term`** (alacritty_terminal backend). nvim runs inside it, as it does in SwiftTerm today | 6 | M |
| WKWebView (notes prose, confluence, AI preview) | **Defer wry** (on Linux it needs GTK/webkit2gtk and fights eframe). Render markdown with **`egui_commonmark`**. Confluence and the full prose view use "Open in browser" (`platform::open_url` on a temp HTML file) | 5/6 | S |
| ScreenCaptureKit capture | **`xcap`** crate (X11 + Wayland portal). Annotation engine (`engines/screenshot_annotations.rs`) is already pure | 6 | M |
| Vision OCR (`engines/screenshot_text.rs`) | optional `ocrs` (pure Rust) or tesseract CLI. Otherwise `None` (already the fallback) | 6, optional | M |
| QuickLook (`files.rs`, `paths.rs`) | none. Reuse the app's own `Preview` enum (Text/Image/Folder) rendered by egui | 5 | S |
| NSPasteboard text / NSOpenPanel / NSUserDefaults / NSScreen / open | **already seamed** (`platform/`): arboard, rfd, JSON prefs, `open` | done | verify only |
| Status item / main menu | `tray-icon` + `muda`. **Defer**: needs GTK on Linux; the egui window gets an in-window menu bar instead | 7, optional | M |
| Global hotkeys (AeroSpace) | The window manager binds keys to the existing CLI (`kitchen-sink notes`, …), which already forwards over the socket. `hotkey_prep` becomes a no-op off macOS | 2 (docs) | S |
| AX helpers (`rust/ws-helpers`: Dock badges, Webex) | none. Gate the binaries to macOS; `notify/` falls back to API sources | 1 | S |
| launchd jira poller | systemd `--user` timer. Python/shell only | 7, optional | S |
| TCC / codesign / dmg | not needed on Linux | — | — |

### 1.3 Why egui (recommendation)

- The macOS UI is **hand-drawn rows** (28 `drawRect` overrides, no NSTableView). It
  is fed by snapshot structs, which matches immediate mode exactly:
  `fn ui(&mut self, ui, &Snapshot) -> Vec<Event>`. There is no retained widget tree
  to keep in sync, which is the main place weaker agents go wrong.
- egui has the most training data of any Rust GUI, so DeepSeek writes it reliably.
- It has cross-platform answers for the hard parts: `egui_term` (terminal),
  `egui_commonmark` (markdown), `egui_extras` (images, tables).
- Only 4 views use a web view, and their HTML comes from Python/pandoc, so
  Tauri would mean rewriting about 25 custom-drawn views as web UI. Not worth it.
- **It runs on macOS too**: behind `--features egui` plus `WS_FRONTEND=egui`, workers
  can run and screenshot the Linux frontend on the owner's Mac. Docker remains the
  truth for compiling and testing.

## 2. Rules for every worker (paste verbatim into every worker prompt)

1. Read `AGENTS.md` and `rule.md` first. Work **directly on `main`**: no branches, no
   worktrees, never push, **never commit** — leave all changes in the working tree; the
   orchestrator reviews and commits. Do not run `git add`, `git commit`, or `git stash`.
2. **Touch only the files listed in your WP card.** If a fix needs another file,
   stop and report (name the file and the reason). Do not "tidy while you're there".
3. **macOS behaviour must not change.** Every macOS code path stays byte-for-byte
   the same except for moving it under `#[cfg(target_os = "macos")]`. Never replace
   an objc2 macOS body with a crate "because it's simpler".
4. Copy the canonical example (§3) your card names. If your site doesn't match
   the example's "before" shape, **stop and report** rather than improvising.
5. Never use `unwrap()`/`expect()` where the old code returned `None`/`false`/`()`.
   No `tokio`/async. No new dependency unless your card names it, with the exact
   version and `[target…]` table.
6. Verify with **all** of these, in this order. Paste the last 5 lines of each into your report:
   ```bash
   cd rust && CARGO_TARGET_DIR=../.build/rust-target cargo build -p ws-rs   # macOS still builds
   cd rust && CARGO_TARGET_DIR=../.build/rust-target cargo test -p ws-rs <module>  # if your card says so
   bin/linux-check.sh check        # Linux compile (from Wave 0)
   bin/linux-check.sh test <filter>   # Linux tests (Wave 2+ cards)
   ```
   No new warnings on macOS (baseline is 20). Never run `./ws build`, never click through the app,
   never relaunch the owner's live app unless your card says to (memory: no UI retry loops; at
   most one live check, then report).
7. If the WP grows past about 150 changed lines, or needs an AppKit type in a public
   signature: **commit nothing**, report, stop.
8. Report in 15 lines or fewer: files changed, sites changed (count), sites deliberately left (with
   file:line and reason), verify output tails, surprises.

## 3. Canonical examples (each WP card names which one to copy)

### X0. Target-gating dependencies (`rust/ws-rs/Cargo.toml`)
```toml
[dependencies]            # portable deps only
serde = { version = "1", features = ["derive"] }
serde_json = "1"
anyhow = "1"
libc = "0.2"
rmpv = "1"
open = "5"

[target.'cfg(target_os = "macos")'.dependencies]
objc2 = "0.6.5"
block2 = "0.6.2"
dispatch2 = "0.3.1"
objc2-foundation = "0.3.2"
objc2-app-kit = "0.3.2"
objc2-quartz-core = "0.3.2"
objc2-core-graphics = "0.3.2"
objc2-core-text = "0.3.2"
objc2-web-kit = "0.3.2"
objc2-vision = "0.3.2"
objc2-screen-capture-kit = "0.3.2"
objc2-quick-look-ui = "0.3.2"
swiftterm-shim = { path = "../swiftterm-shim" }

[target.'cfg(not(target_os = "macos"))'.dependencies]
arboard = { version = "3", default-features = false }
rfd = { version = "0.15", default-features = false, features = ["xdg-portal"] }
```
Same idea for `ws-helpers/Cargo.toml`. Each of its `src/*.rs` binaries gets the X2
"whole file" treatment, with a non-macOS `fn main() { eprintln!("macOS only"); std::process::exit(1) }`.

### X1. Gate imports + the AppKit half of a file (the most common fix, ~60% of Wave 1)
Existing precedent: `ui/style.rs:116` (`#[cfg(target_os = "macos")] mod appkit { … }`) and
`views/files.rs:23-34` (each `use objc2…` line individually gated).
```rust
// BEFORE (top of file)
use objc2::rc::Retained;
use objc2_app_kit::{NSView, NSColor};
// AFTER
#[cfg(target_os = "macos")]
use objc2::rc::Retained;
#[cfg(target_os = "macos")]
use objc2_app_kit::{NSView, NSColor};
```
Then gate every item (fn, impl, struct, `define_class!`, `thread_local!`) that mentions
an objc2/AppKit type, **as a whole item**: `#[cfg(target_os = "macos")]` on the item.
If a pure item calls a gated one, gate the call statement in place, as `files.rs:1413` does:
```rust
pub fn sync_view(&self) {
    #[cfg(target_os = "macos")]
    if let Some(v) = &self.view { v.apply(self.snapshot()); }
}
```
Struct fields holding AppKit types get `#[cfg(target_os = "macos")]` on the field **and** on
every initialiser line that sets it.

### X2. Whole-module gating (when nothing in the file is pure)
Precedent `app/mod.rs:8`:
```rust
#[cfg(target_os = "macos")]
pub mod live_panes;
```
Use this only when a grep shows no non-macOS caller of anything in the module.
Otherwise use X1.

### X3. `MainThreadMarker` in a public signature (blocker in `views/files.rs` ×7, `ui/popup.rs`)
```rust
// BEFORE (views/files.rs:1715)
pub fn build(&mut self, mtm: objc2::MainThreadMarker) {
    #[cfg(target_os = "macos")]
    self.build_macos(mtm);
    #[cfg(not(target_os = "macos"))]
    let _ = mtm;
}
// AFTER: the fn only exists on macOS (its callers live in gated daemon_ui code)
#[cfg(target_os = "macos")]
pub fn build(&mut self, mtm: objc2::MainThreadMarker) {
    self.build_macos(mtm);
}
```
Check callers with `grep -rn "\.build(" rust/ws-rs/src`. If a caller is NOT already gated,
gate that call statement too (X1). If the caller is in a file you don't own, stop and report.

### X4. libc / POSIX differences (macOS-only struct fields and flags)
```rust
// socket.rs:224 — SO_NOSIGPIPE does not exist on Linux. Rust std already ignores
// SIGPIPE in binaries, so the option is simply skipped elsewhere.
#[cfg(target_os = "macos")]
unsafe { libc::setsockopt(fd, libc::SOL_SOCKET, libc::SO_NOSIGPIPE, /* unchanged */) };

// recent_files.rs:486 — st_birthtime is macOS-only.
#[cfg(target_os = "macos")]
let t = (st.st_birthtime as f64).max(st.st_mtime as f64);
#[cfg(not(target_os = "macos"))]
let t = st.st_mtime as f64;

// recent_files.rs:530-550 — 6-argument getxattr + XATTR_NOFOLLOW is the macOS API.
// Gate the whole helper fn; the non-macOS twin returns None (macOS-specific xattrs).
```

### X5. A new seam for a tiny AppKit/Foundation leak in core (follow `platform/clipboard.rs`)
Example: `main.rs::ensure_home` reads the bundle version via `NSBundle`.
```rust
// platform/app.rs  (add beside frontmost_app)
/// CFBundleShortVersionString of the running bundle; "" if none.
#[cfg(target_os = "macos")]
pub fn bundle_version() -> String {
    use objc2_foundation::{NSBundle, NSString};
    NSBundle::mainBundle()
        .objectForInfoDictionaryKey(&NSString::from_str("CFBundleShortVersionString"))
        .and_then(|v| v.downcast::<NSString>().ok())
        .map(|v| v.to_string())
        .unwrap_or_default()
}
#[cfg(not(target_os = "macos"))]
pub fn bundle_version() -> String { env!("CARGO_PKG_VERSION").to_string() }
```
The call site becomes `let version = crate::platform::bundle_version();`.

### X6. macOS-only tests
```rust
#[test]
#[cfg_attr(not(target_os = "macos"), ignore = "macOS-only: <reason>")]
fn needs_appkit() { … }
```
Use `#[cfg(all(test, target_os = "macos"))]` on the whole `mod tests` only if it won't compile
off macOS (precedent `platform/dialog.rs:62`).

### X7. Snapshot + Event contract for a view (Wave 3). Copy `views/files_view.rs:17-63`.
```rust
/// Everything the view draws, pushed by the model.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PathsSnapshot { pub rows: Vec<PathRow>, pub selection: usize, pub query: String, pub status: String }

/// Clicks / typing reported to the owner.
#[derive(Clone, Debug, PartialEq)]
pub enum PathsEvent { Click { row: usize, double: bool }, Query(String), Enter, Move(i64), Escape }

impl PathShelfModel {                     // the existing model type
    pub fn snapshot(&self) -> PathsSnapshot { /* read existing fields only */ }
    pub fn handle(&mut self, ev: PathsEvent) { /* call the SAME model methods the AppKit
                                                 half calls today for each user action */ }
}
```
Rules: pure Rust, no cfg. Put it in the existing pure half of the view's file (or in a
new `xxx_view_model.rs` if the file has no pure half). Find each event's behaviour by
reading the macOS half's action handlers (`sel!(...)` methods, `mouseDown`, `keyDown`)
and calling the same model methods. **Do not modify the macOS half.** Add a unit test:
build model → `snapshot()` → `handle(event)` → assert the snapshot changed.

### X8. egui frontend patterns (Wave 4 writes these; Wave 5 copies them)
```rust
// frontend_egui/theme.rs: the ONE colour conversion (mirror of Rgba::to_nscolor)
pub fn c(rgba: crate::ui::theme::Rgba) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(
        (rgba.r * 255.0).round() as u8, (rgba.g * 255.0).round() as u8,
        (rgba.b * 255.0).round() as u8, (rgba.a * 255.0).round() as u8)
}

// frontend_egui/keys.rs: the ONE key adapter. Maps egui keys to the macOS virtual
// key codes route_key already understands (ui/popup.rs KEY_* consts), so the existing
// pure routing (ui::popup::route_key, engines/text_edit_keys) is reused unchanged.
pub fn key_input(key: egui::Key, m: egui::Modifiers, text: Option<char>) -> Option<crate::ui::popup::KeyInput> { … }

// frontend_egui/files.rs: THE reference view. Every other view copies this shape.
pub struct FilesPane { model: crate::views::files::FileBrowser }
impl FilesPane {
    pub fn ui(&mut self, ui: &mut egui::Ui, colors: &PopupColors) {
        let snap = self.model.snapshot();                    // pull each frame
        let mut events = Vec::new();
        // ... draw snap.rows with ui.selectable_label / painter, push FilesEvent::Click{..}
        for ev in events { self.model.handle(ev); }          // same events AppKit sends
    }
}
```

## 4. Work-package cards

Card format: **Files** (owned) · **Example** · **Steps** · **Done when**. Line numbers are
approximate; grep the symbol.

### Wave 0 (serial, one worker)

**W0-1 Linux check harness + dependency gating.** Files: new `bin/linux-check.sh`, new
`rust/docker/linux.Dockerfile`, `rust/ws-rs/Cargo.toml`, `rust/ws-helpers/Cargo.toml`,
`rust/ws-helpers/src/*.rs`, `bin/run-tests.sh` (one new `linux` case), `.gitignore`. Example: X0, X2.
- Dockerfile: `FROM rust:1.92-bookworm` (match local `rustc 1.92`), plus `python3 python3-pip`
  and, for later waves, `libxkbcommon-dev libgtk-3-dev libxcb1-dev libwayland-dev pkg-config`.
- `bin/linux-check.sh check|test [filter]|shell`: `docker build` (cached) then
  `docker run --rm -v "$REPO":/repo -w /repo/rust -e CARGO_TARGET_DIR=/repo/.build/linux-target <img> cargo check --workspace --all-targets`
  (`test` runs `cargo test -p ws-rs -- <filter>`, then the plain-script Python suites:
  `for t in Tests/test_{config,fileops,jsonmgr,snippet_render}.py; do python3 $t; done`).
  Use `docker run --platform linux/arm64` (native on Apple Silicon; fast). `.build/` is already ignored.
- `swiftterm-shim` is already `#![cfg(target_os="macos")]` in `src/lib.rs`; check its `build.rs`
  skips `swiftc` off macOS (add `if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") { return; }`
  at the top if not).
- Run `bin/linux-check.sh check`, save the full error list to `.build/linux-errors-wave0.txt`
  and report a **summary table** (errors per file) in your final message; the orchestrator
  appends it to the end of `PLAN-cross-platform.md` §8.
  **The orchestrator uses that table to confirm or re-split the Wave 1 file groups.**
- Done when: macOS `cargo build -p ws-rs` is unchanged (20 warnings); the script runs; the error table is reported.

### Wave 1: `cargo check` on Linux (parallel; each card owns disjoint files)
Every card: Example X1 (plus the extras named). Done when **your files** produce zero errors in
`bin/linux-check.sh check 2>&1 | grep -E '^(error|warning: unused).*' -A3 | grep 'src/<your file>'`,
and macOS still builds with no new warnings. Errors in others' files are expected until everyone lands.

| Card | Files | Known blockers / notes | Extra example |
|---|---|---|---|
| W1-A core libc | `app/socket.rs`, `engines/recent_files.rs` | `SO_NOSIGPIPE` (socket.rs:227); `st_birthtime` (recent_files:486); 6-arg `getxattr` (530–550) | X4 |
| W1-B entry point | `main.rs`, `platform/app.rs` | `prose` branch (55–64) → gate the whole match arm; `ensure_home` NSBundle → `platform::bundle_version()`; `run_daemon` AppKit part after the headless `park` (138–), `install_status_item`, `status_menu_action` Quit arm → gate. **Non-macOS `run_daemon` = the headless path for now** (Wave 4 swaps in egui) | X5, X1 |
| W1-C keys/panes | `ui/popup.rs`, `panes/vim_keys.rs`, `panes/pane_nav.rs` | ungated `use objc2::MainThreadMarker` (popup.rs:14); `key_input_from_event` (730) must be gated; keep `KeyInput`/`route_key`/`KEY_*` pure | X1, X3 |
| W1-D engines | `engines/ansi_render.rs`, `engines/ai_format.rs`, `engines/screenshot_text.rs` | ansi_render has **no gates**: imports 13–24, CF externs 27–38, `AnsiCoreFont`/`AnsiCoreImage`/`font_named`/`font`/`metrics`/`image` (≈590–925) → gate; parse/SGR/theme stay pure | X1 |
| W1-E ui (pure-heavy) | `ui/theme.rs`, `ui/style.rs`, `ui/chrome.rs`, `ui/shared_window.rs`, `ui/sidebar.rs`, `ui/tree_view.rs`, `ui/ansi_text.rs` | keep `Rgba`, `PopupColors`, `HeaderStyle`, `chrome::Rect`, `SidebarModel`, `nav_key`, shared-window index math pure; `style.rs` `Weight` (≈146) is inside `mod appkit`; if a pure caller needs it, report | X1 |
| W1-F ui (AppKit-only) | `ui/action_picker.rs`, `card.rs`, `field.rs`, `inline_rename.rs`, `jira_setup.rs`, `list_panel.rs`, `menu_kit.rs`, `panel_frame.rs`, `prose_switch.rs`, `sheet.rs`, `shortcuts.rs`, `toast.rs`, `ui/mod.rs` | first `grep` for non-macOS callers of each; if none → X2 in `ui/mod.rs`; else X1 | X2/X1 |
| W1-G views: files/paths/tools | `views/files.rs`, `files_view.rs`, `file_popup.rs`, `paths.rs`, `tools.rs`, `inspector.rs` | files.rs: 7 `MainThreadMarker` signatures (`is_up` 845, `toggle` 873, `refresh` 917, `dismiss` 938, `toggle_quick_look` 1657, `refresh_quick_look` 1663, `build` 1715); `QuickLookSource` class (971–1070) and `root`/`view` fields (1076–1079) outside `mod macos` | X3, X1 |
| W1-H views: jira/confluence/ai | `views/jira.rs`, `jira_view.rs`, `confluence.rs`, `confluence_view.rs`, `ai.rs`, `ai_view.rs` | jira.rs `content` field (2972), `build()` (3018); ai.rs `preview` field (948) and `render_answer`/`preview_view`/`show_preview_macos` (1322–1355); confluence `ConfluenceImageLoader.handler` (1166). **`jira_view.rs::parse_stamp` (212) uses NSDateFormatter and returns `None` off macOS**: keep the macOS body; for non-macOS add a pure parser for the same input format (read the formatter's `dateFormat` string; use the `time = "0.3"` crate under the non-macOS target table only) + unit test | X1, X3 |
| W1-I views: notes | `views/notes.rs`, `notes_prose.rs`, `notes_tree.rs`, `notes_vim.rs`, `note_find.rs` | note_find is already whole-file `#![cfg]`; notes.rs 5,439 lines with WK + SwiftTerm: gate every item touching them | X1 |
| W1-J views: compare/screenshot/switcher/setup/terminal | `views/compare.rs`, `compare_view.rs`, `screenshot.rs`, `switcher.rs`, `setup.rs`, `terminal.rs` | compare `CompareSurface` (4931–5090) + `build_content` (4917) outside `mod macos`; screenshot.rs is 8k lines with 43 objc2 imports: if it exceeds the 150-line rule, **split the card in two** (screenshot alone) | X1 |
| W1-K app shell | `app/host.rs` (outside `daemon_ui` only), `app/menu.rs`, `app/mod.rs` | host.rs non-UI part already gates its AppKit calls; fix whatever the error table shows. **Never edit inside `mod daemon_ui`** | X1 |

Orchestrator: after all W1 cards land, run `bin/linux-check.sh check` yourself. Leftover
errors → one follow-up card per file. **Exit criterion:** zero errors on Linux;
macOS `cargo build` + `cargo test -p ws-rs` unchanged (727 pass / 4 ignored).

### Wave 2: Linux headless runtime + tests (parallel)

| Card | Files | Task |
|---|---|---|
| W2-A Linux tests | test modules only (the files named by failures) | `bin/linux-check.sh test`; for each failing test decide: macOS-only → X6; real Linux bug → report, don't fix. Target: all non-ignored tests pass |
| W2-B headless smoke | new `Tests/test_linux_headless.sh` | in Docker: build, `WS_HOME=$(mktemp -d) kitchen-sink headless &`, then `printf 'state\n' \| nc -U "$TMPDIR/ws-notes.sock"` returns JSON with `"view"`; `do:open:files` returns state. Add a `linux-headless` case to `bin/run-tests.sh` |
| W2-C CLI cold start | `app/cli.rs` | off macOS the cold start must not call `/usr/bin/open` / `kitchen_sink.sh`: spawn `std::env::current_exe()` detached with the same mode arg (`Command::new(exe).arg(mode).spawn()`, plus `setsid` via `pre_exec`) under `#[cfg(not(target_os="macos"))]`; macOS branch untouched; `getppid()==1` launchd check gated macOS |
| W2-D hotkey no-op | `app/hotkey.rs` | `hotkey_prep` returns the empty/default `HotkeyPrep` off macOS (AeroSpace absent). The `AeroIpc` trait is already injectable; add a test |
| W2-PY Python (optional, owner said low priority) | `settings_hub/tui/lineedit.py`, `pylib/status.py`, `notify/webex_api.py`, `bin/no_stray_workspaces.sh` | lineedit `pbcopy/pbpaste` → try `wl-copy`/`xclip`/`pbcopy` in order (one helper fn); `status.py` `pmset/sysctl/vm_stat` → `sys.platform` branch reading `/sys/class/power_supply`, `/proc/meminfo`; `webex_api.py:260` `open` → `webbrowser.open`; shebang `#!/opt/homebrew/bin/bash` → `#!/usr/bin/env bash`. Keep macOS output byte-identical (rule 7 in AGENTS.md); run the matching `Tests/test_*.py` |

Exit: Linux `cargo test` green; the headless smoke test passes in Docker.

### Wave 3: frontend contract (parallel, one worker per view; pure Rust, add-only)
Example: X7. Files that already have Snapshot/Event (files, jira, confluence, ai) only need
`snapshot()` to be callable without a view (check `FileBrowser::snapshot` exists and is
pure) and a pure `handle(Event)` on the model. Write `handle` only if the macOS half dispatches
events inline today.

| Card | View | Files |
|---|---|---|
| W3-A | files: `handle(FilesEvent)` extracted from the macOS event dispatch | `views/files.rs` (pure half only) |
| W3-B | jira: `handle(JiraEvent)` | `views/jira.rs` (pure half) |
| W3-C | confluence + ai: `handle` | `views/confluence.rs`, `views/ai.rs` (pure halves) |
| W3-D | paths shelf: new `PathsSnapshot`/`PathsEvent` | `views/paths.rs`, `engines/path_shelf.rs` |
| W3-E | switcher + palette: `SwitcherSnapshot`/`SwitcherEvent` | `views/switcher.rs`, `engines/switcher_status.rs` |
| W3-F | compare: `CompareSnapshot` (rows of the existing diff model)/`CompareEvent` | `views/compare.rs` (pure half), `compare_view.rs` (pure half) |
| W3-G | notes: `NotesSnapshot` (sidebar tree from `notes_tree.rs`, current file path + text)/`NotesEvent` | `views/notes.rs` (pure half), `notes_tree.rs` |
| W3-H | tools (prettyprint, filefast, health) + setup checks | `views/tools.rs`, `views/setup.rs` (pure halves) |
| W3-I | screenshot recent grid + annotation state (capture comes in Wave 6) | `views/screenshot.rs` pure half |

Each card: unit test per X7; macOS untouched; `bin/linux-check.sh test <module>` green.

### Wave 4: egui skeleton + reference view (ONE worker, serial; stronger model if available)
Files: new `rust/ws-rs/src/frontend_egui/{mod,app,theme,keys,files}.rs`, `main.rs`
(`run_daemon` non-macOS branch + the macOS `WS_FRONTEND=egui` dev switch), `Cargo.toml`.
- Deps: `eframe` + `egui_extras` (pin one minor version that builds on rustc 1.92, e.g. `0.32`; record it),
  in `[target.'cfg(not(target_os = "macos"))'.dependencies]` **and** as `optional = true` under
  `[dependencies]` behind `[features] egui = ["dep:eframe", "dep:egui_extras"]` for macOS dev runs.
  Module gate: `#[cfg(any(not(target_os = "macos"), feature = "egui"))] mod frontend_egui;`.
- `app.rs`: `struct KsApp { controller: Arc<SwitcherController>, ui_queue: UiQueue, main_queue: MainQueue, current: SlotView, panes: Panes }`.
  `update()`: drain `ui_queue` (handle `Present`, `Hide`, `Toast`, `ShowPalette`, …; unimplemented
  variants log once and are ignored), drain `main_queue` and `platform::drain_main_queue()`, then
  `ctx.request_repaint_after(30ms)` (mirrors the 30 ms AppKit drain timer). It installs queues with
  `controller.install_ui(q)` / `install_main_queue(q)` exactly like `daemon_ui::install_daemon_ui`.
- Layout: left sidebar of `SlotView`s (the colours from `ui::theme::section_colors`) and a central panel that
  dispatches to the pane for `current`. Placeholder label for views not yet ported.
- Keys: `keys.rs` adapter (X8). Feed `route_key` and handle the resulting `KeyAction`s that
  `daemon_ui` handles: Esc, cycle, view switch, Ctrl+B prefix.
- `files.rs`: the full Files view (sidebar places, toolbar, list, preview Text/Image/Folder,
  status line) using `FilesSnapshot`/`FilesEvent`. **This is the template for Wave 5**: comment
  each section with the `files_view.rs` region it mirrors.
- Verify: `bin/linux-check.sh check`; on macOS `cargo run -p ws-rs --features egui` with
  `WS_FRONTEND=egui WS_HOME=$(mktemp -d)` and one screenshot of the window; `state` over the socket
  shows `view: files` after `do:open:files`.
- Then the orchestrator appends the final `frontend_egui/files.rs` path + key excerpts to §3 X8.

### Wave 5: egui views (parallel; each card = one new file `frontend_egui/<view>.rs` + one registration line in `frontend_egui/app.rs`)
Rule for all: copy `frontend_egui/files.rs` structure; use only the view's Snapshot/Event (from Wave 3);
colours only via `theme::c`; no AppKit-parity pixel hunting. Linux is functional-first. The
`app.rs` registration line is the only shared-file edit: **orchestrator serialises those commits**
(or the worker re-reads `app.rs` and appends a line just before finishing).

| Card | View | Notes |
|---|---|---|
| W5-A | switcher / palette | modal overlay, filter field + list |
| W5-B | paths shelf | list + actions (copy path via `platform::clipboard::set_text`) |
| W5-C | jira | table: `egui_extras::TableBuilder`; inspector panel |
| W5-D | confluence | results list; preview = plain text + "Open in browser" (write HTML to temp, `platform::open_url`) |
| W5-E | ai | rules list + answer rendered by `egui_commonmark` (pin version compatible with chosen egui) |
| W5-F | compare | two-column diff rows from the snapshot; folder compare list |
| W5-G | notes (plain editor) | tree sidebar + `egui::TextEdit::multiline` editor; save through the existing model; prose = `egui_commonmark` of the markdown |
| W5-H | tools + setup | prettyprint/filefast/health as simple panels |
| W5-I | toast + context menus | `ui.response.context_menu` for rule 2 (copy file, copy path, open in notes, reveal in Files) on files/paths rows |

Each card verifies: Linux check; macOS `--features egui` dev run + one screenshot; a `state` probe.

### Wave 6: hard platform features (parallel, each optional, each self-contained)
| Card | Feature | Approach |
|---|---|---|
| W6-A | terminal panel | `egui_term` (alacritty_terminal) in `frontend_egui/terminal.rs`; reuse `[terminal]` config (shell, font size) |
| W6-B | notes nvim pane | same widget running `nvim` with the args `notes_vim.rs` builds (reuse its pure arg builder; `engines/nvim_rpc.rs` already speaks msgpack over a Unix socket) |
| W6-C | screenshot capture | `platform/capture.rs` seam: `capture_screen(display) -> Option<RgbaImage>`, macOS = existing ScreenCaptureKit path moved verbatim (only if trivially movable; else leave macOS alone and add the non-macOS fn only), non-macOS = `xcap`. Feed the pure annotation engine; egui overlay for annotate |
| W6-D | OCR (optional) | non-macOS `screenshot_text::recognize` via `tesseract` CLI if present, else `None` |

### Wave 7: packaging, docs, Windows scouting
- W7-A `bin/build-linux.sh`: `cargo build --release -p ws-rs`, lay out `dist/kitchen-sink/{kitchen-sink, pylib/, vim/, commands.default.toml}` and set `WS_RS_ASSET_DIR` in a wrapper script. `./ws build` branches on `uname`.
- W7-B docs: `AGENTS.md` "Linux" section (build, Docker check, WM keybindings → CLI modes, what's unsupported); README.
- W7-C (optional) systemd `--user` timer for the jira poller, mirroring `jira/com.jira.poll.plist`.
- W7-D Windows scouting (no code): table of blockers. Known so far: `std::os::unix` + raw `libc` AF_UNIX in `socket.rs`, `cli.rs`, `nvim_rpc.rs`; `flock` (`socket.rs:345` DaemonLock); `libc::kill` (`python_helper.rs:276`); `localtime_r` (`screenshot_annotations.rs:1468`); `PermissionsExt` (5 sites); `fnmatch`; Python `fcntl`, `/`-joins (22), `start_new_session`. Proposed seam: `platform::ipc` (interprocess crate) + `platform::lock` (fs4).

## 5. Parallelisation and conflict control
- **File ownership is the lock.** Within a wave no two cards share a file. The only shared files
  are `rust/ws-rs/Cargo.toml`, `platform/mod.rs`, `frontend_egui/app.rs`: one-line appends,
  commit serialised by the orchestrator (the worker re-reads the file immediately before editing
  and never overwrites another worker's line).
- Dependency graph: `W0 → W1(all) → {W2(all), W3(all)} → W4 → W5(all) → W6 → W7`.
  W3 can start as soon as W1 lands because it is pure model code; it doesn't need W2.
  W7-D can run anytime.
- After each wave the orchestrator runs: macOS `cargo build -p ws-rs` + `cargo test -p ws-rs`
  (must stay 727 pass / 4 ignored or more), `bin/linux-check.sh check`, and from Wave 2 on
  `bin/linux-check.sh test`. Record the results in §8.
- Worker failure handling: a worker that reports "stop" gets its card re-scoped by the
  orchestrator (split by file or by item), never retried verbatim. Max 2 attempts per card,
  then escalate to the owner.

## 6. Verification (end-to-end)
1. macOS unchanged: `./ws build` (orchestrator only, once at the end of each wave), then the `ui-check`
   skill probes (`echo state | nc -U "$TMPDIR/ws-notes.sock" | jq .view`) and a
   `bin/ks-mcp-sandbox.sh start --gui` `pane-shot` of notes/files compared with the previous wave.
2. Linux compile + tests: `bin/linux-check.sh check` and `bin/linux-check.sh test` (Docker, arm64).
3. Linux headless runtime: `bin/run-tests.sh linux-headless` (Wave 2).
4. Linux GUI: macOS dev run `WS_FRONTEND=egui cargo run -p ws-rs --features egui` + screenshot per
   view (Wave 4–5). Real Linux desktop check (Docker can't show a window; use an owner-provided
   Linux VM/box): launch, `kitchen-sink files` from a shell to verify CLI → socket → window.

## 7. Out of scope / deferred
macOS AppKit rewrites of any kind; FontSpec/colour/bezier seams; wry/WebKitGTK; tray icon and global
menu; AX notification helpers on Linux; Windows implementation (scouting only); pixel parity of the
Linux frontend with macOS.

## 8. Progress log (orchestrator appends)
- Wave 0 error table: _pending_
- Wave 0 (landed): `bin/linux-check.sh` + `rust/docker/linux.Dockerfile` (Colima, arm64). Extra gating beyond card: `swiftterm-shim/Cargo.toml` + `build.rs`; X0 `rfd` needs `features = ["xdg-portal", "async-std"]` (zbus hard-errors otherwise). macOS: 20 warnings. Linux: 64 ws-rs errors (resolution phase; later phases likely masked, so re-derive after W1).
  | errors | file |
  |---:|---|
  | 14 | main.rs |
  | 12 | views/files.rs |
  | 9 | ui/shared_window.rs |
  | 7 | engines/recent_files.rs |
  | 6 | engines/ansi_render.rs |
  | 5 | views/paths.rs |
  | 2 | ui/chrome.rs |
  | 1 each | ui/card.rs, ui/popup.rs, ui/tree_view.rs, app/socket.rs, views/{terminal,notes_vim,inspector,setup,switcher,tools,screenshot}.rs |
- Wave 1 (landed, commits be7eb60..d70b43c): W1-A..K + W1-L follow-up (recent_files FSEvents/CF FFI gating). Many cards were no-ops (gating already existed from the seams work); W1-K needed no edits. Gates: `bin/linux-check.sh check` exit 0 (0 errors; 6 bin / 8 test warnings); macOS build 20 warnings; macOS `cargo test -p ws-rs` 727 passed / 4 ignored. Added `time = "0.3"` (non-macOS) for `jira_view::parse_stamp`. Process note: orchestrator once ran `git stash` by mistake and popped it immediately (no loss).
- Wave 2+3 (landed, cb8191f..bc13027 + W2-A): W3-A..I add `snapshot()`/`handle()` + tests to files, jira(jira_view.rs), confluence, ai, paths, switcher, compare, notes, tools, setup, screenshot. W2-B headless smoke (`bin/run-tests.sh linux-headless`, python socket client; image has no `nc`), W2-C cli cold start, W2-D hotkey no-op, W2-A Linux tests. Gates: Linux check 0; `linux-check.sh test` exit 0 (730 pass / 10 ignored); macOS 743 pass / 4 ignored, 20 warnings; headless smoke OK (state + `do:open:files` -> view files).
  Known Linux leftovers (not fixed, from W2-A): `recent_files.rs:787` unconditional `/tmp`->`/private/tmp` rewrite (8 tests X6-ignored); `python_helper.rs:240` `find_python` skips `/usr/bin` so the Python bridge can't start on Debian images (**needed before the Linux app is useful**); `status.py` pmset/sysctl (W2-PY); `file_ops._trash` now creates `~/.Trash` (no-op on macOS).
- Wave 4 + 5 (landed, 1d56c32..W5-J): egui 0.32.3 frontend; `frontend_egui/` = app (pane registry, toasts, tool/paths windows), files (reference), switcher overlay, paths, jira, confluence, ai (egui_commonmark 0.21.1), compare, notes, tools+setup, row context menus. Gates: Linux `check` 0; Linux `test` 769 pass / 5 ignored; macOS default build 20 warnings; macOS `--features egui` tests 777 pass / 4 ignored. Live macOS egui run (private TMPDIR/WS_HOME): do:open:{files,jira,ai,compare,notes} each switch view, no panics.
  Known gaps: `ToolsPane` has no kind selector (`Tool(kind)` opens window but can't pre-select tab); `PathsPane::run_return` private; "open in notes" context action is a documented no-op off macOS (no cross-platform seam); Wave 6 (terminal, nvim, capture, OCR) and Wave 7 (packaging/docs) not started.
