# Agent prompt: continue the Swift→Rust port (kitchen-sink)

You are continuing an in-flight port. Everything is on disk; read the current
state before writing anything. Repo: `/Users/danielbaker/.config/kitchen-sink`
(the REAL repo — `git rev-parse --show-toplevel` first; `rule.md` forbids git in
backups).

## Read first (in order)
1. `PLAN-rust-port.md` — the plan, decisions, and **Progress** section (the
   source of truth for what is done).
2. `PROMPT-agents-md-refresh.md` — if `AGENTS.md` / `AGENT_CONTEXT.md` exists,
   read it; it describes the Swift app + Python boundary.
3. `rust/ws-rs/src/main.rs` — the daemon entry (socket + `SwitcherController`).

## Where things stand
- The Rust workspace `rust/` (`ws-rs`, `ws-helpers`, `swiftterm-shim`) mirrors
  the Swift files module-for-module. `cargo test` from `rust/` is green
  (~495 tests). `bin/build-rust-app.sh` builds a signed `.app` to
  `.build/rust-app/` (isolated — never touches the live Swift app).
- The Rust binary runs a **daemon** that answers the Unix-socket contract
  (`$TMPDIR/ws-notes.sock`): `state`, `do:ACTION`, `reload`, `ping`, launch
  messages. `bin/rs-parity.py` compares its `state` to the Swift daemon.
- **But it shows no window yet**, and several view builders are `todo!()`.

## Commands
```bash
cd rust && cargo test                 # Rust unit tests (must stay green)
cd rust && cargo build                # must stay warning-free
./bin/build-rust-app.sh --force       # -> .build/rust-app/kitchen-sink.app
# run the daemon (dev: point the python helper at the checkout's pylib):
WS_RS_ASSET_DIR="$PWD" .build/rust-app/kitchen-sink.app/Contents/MacOS/kitchen-sink
# verify over a socket (use an isolated TMPDIR so you don't drive the live app):
D=$(mktemp -d); TMPDIR="$D" WS_RS_ASSET_DIR="$PWD" <bin> &
printf 'state\n' | nc -U "$D/ws-notes.sock"
python3 bin/rs-parity.py --rust-sock "$D/ws-notes.sock"   # needs the Swift daemon for a diff
```
NOTE: never run the parity tool`s `do:` actions against the real
`$TMPDIR/ws-notes.sock` while the owner's Swift app is up — it changes their UI.

## Remaining work (each item = one file/area; keep edits within the listed files)
1. **Show the window.** `main.rs` + `app/host.rs`: on daemon start and on a
   launch/`do:open:*`, build a `SlotHostWindow` (`ui/shared_window.rs`) and
   `present()` the default view (files/notes). Minimal is fine: a real visible
   window with a `PopupChrome` header + the view switcher. This is the key
   milestone that makes the Rust app *launchable*.
2. **`views/jira.rs` `build`** — AppKit NSView tree for the jira list/table +
   detail (models are already ported/tested).
3. **`views/compare.rs` drawing layer** — `ComparePaneView`, `FolderTreeView`,
   `CompareEditor`, `CompareThumbnail` (models ported; only rendering is
   `todo!()`).
4. **`views/notes.rs`, `views/files.rs`, `views/paths.rs` window surfaces** —
   the NSView trees (models ported).
5. **`views/ai.rs` `runPart`** — the streaming `fm respond --stream` spawn
   (std process + reader thread; no new dep needed).
6. **`views/screenshot.rs` overlay panels + the CG encode pass** — models +
   `ShotSession::render` exist; the AppKit overlay panels remain.
7. **Small:** `config-schema` JSON (`main.rs`); full palette-command population
   (`app/host.rs`); `__info_plist` embed via a `ws-rs/build.rs`
   `cargo:rustc-link-arg=-Wl,-sectcreate,__TEXT,__info_plist,...`.
8. **Then Phase 4 cutover** (see `PLAN-rust-port.md`): flip `./ws` /
   `bin/build-app.sh` to cargo; make `notify/notify_poll.py` call the Rust AX
   helpers; delete the Swift tree. Do NOT start this until the window is shown
   and the UI suites pass against the Rust binary.

## How to work (the owner's directives)
- Spawn **cheap `general` subagents** (avoid expensive models), one per
  file/area, **in parallel** where the files are disjoint — each agent owns a
  fixed list of files and must NOT edit anything else (`main.rs`, any `mod.rs`,
  `Cargo.toml`, or another agent's module). Prefer to own `Cargo.toml`/`main.rs`
  yourself and hand them out one at a time.
- Every agent must keep `cargo test` green and add tests for pure logic; run
  `cargo build` too. AppKit calls are `#[cfg(target_os = "macos")]`.
- Keep the socket contract byte-compatible (`state` keys, `do:` grammar); do not
  change the Python side (frozen) or `commands.toml`'s format.
- Config over code; user-facing strings live in `commands.toml`.
- Do NOT commit; leave changes uncommitted for review.

## Definition of done for a wave
`cd rust && cargo test` green, `cargo build` warning-free, and (once item 1
lands) the Rust `.app` visibly shows the shared window and answers
`state`/`do:` identically to the schema in `PLAN-rust-port.md`'s Progress.
