# PORTS-STATUS.md — Linux & Windows ports: what works, what was verified

Start here if you are a new agent picking up cross-platform work. Keep this file current
(update it in the same commit as any wave that changes the status). Deep history:
`PLAN-cross-platform.md` (Linux, §8 log), `PLAN-windows.md` + `PLAN-windows-scouting.md` (Windows).
macOS is the product; AppKit stays and is untouched by both ports (rule: Unix/macOS bodies are
never rewritten, other platforms get separate `#[cfg]` bodies).

## Status at a glance (last updated 2026-10-12)

| | macOS | Linux | Windows |
|---|---|---|---|
| Compiles | yes (AppKit, default) | **yes** (`bin/linux-check.sh check`, Docker arm64) | **no yet** — compile-only phase in progress |
| Rust tests | 750 pass / 4 ignored (791 with `--features egui`) | 784 pass / 5 ignored in Docker | not run (cannot run on the Mac) |
| Daemon / socket API | yes | **runs headless in Docker** (`state`, `do:open:files` verified) | untested; Unix sockets must be ported first |
| GUI | AppKit (the real app) | **egui/eframe frontend written, never run on Linux** (see below) | none yet |
| Real desktop run | daily-driven | **never** (no Linux box available) | never |

### Linux GUI — exact state
- A second frontend, `rust/ws-rs/src/frontend_egui/` (eframe/egui 0.32.3), drives the same
  toolkit-neutral core as AppKit (`SwitcherController`, view models with `snapshot()`/`handle()`).
- Panes exist for: Files (reference impl), Jira, Confluence, AI, Compare, Notes, Paths, Tools+Setup,
  switcher/palette overlay, toasts, right-click row menus, embedded terminal
  (`alacritty_terminal` 0.26 — `egui_term` has no egui-0.32 release), Notes "edit in nvim" mode.
- **Only verified by:** compiling in Docker, unit tests, and one live run of the egui window on the
  *owner's Mac* (`--features egui` + `WS_FRONTEND=egui`, view switching via the socket worked, no panics).
  **Nobody has seen it on X11/Wayland.** Expect rough edges; it is "functional-first", not pixel parity.
- Known gaps: no screenshot capture/annotation UI (only the `platform::capture` seam, `xcap =0.4.1`;
  >=0.5 needs pipewire); OCR is the `tesseract` CLI if installed; "open in notes" context action is a
  no-op off macOS; `ToolsPane` cannot pre-select a tool tab; prose/Confluence use plain text/markdown
  (`egui_commonmark`), not a web view; no tray icon/global menu; no QuickLook; `status.py` is macOS-only;
  Python `Tests/` still mostly macOS-flavoured. Global hotkeys = your WM binds keys to the CLI
  (`kitchen-sink notes|files|jira|...`, forwards over the socket).
- Build a dist: `bin/build-linux.sh [--docker]` -> `dist/kitchen-sink/run.sh` (verified in Docker: answers `state`).

### Windows — exact state
- **Goal of the current phase: `cargo check` passes for `x86_64-pc-windows-gnu`. Compile-only.**
  No GUI, no daemon, no run has been or can be verified from the Mac. Every Windows body written is
  "type-checks, untested at runtime" and must be listed under *Untested Windows bodies* below.
- Third-party deps (eframe, arboard, rfd, ...) already compile for Windows. Remaining errors are our
  Unix-only core: `UnixListener`/raw `libc` sockets (`app/socket.rs`, `cli.rs`, `engines/nvim_rpc.rs`),
  `flock` daemon lock, `PermissionsExt::mode`, `pre_exec`/`setsid`, `libc::kill/fnmatch/localtime_r`, `/bin/bash`.
- Proposed seams (see scouting doc): `platform::ipc` (`interprocess`), `platform::lock` (`fs4`).
- Not started: any runtime verification (needs a Windows machine/CI), Python portability, packaging.

#### Untested Windows bodies
_(none yet — append here as Windows waves land: file, what, the assumption most likely wrong)_

## How to verify (all from the Mac)
```bash
bin/linux-check.sh check            # Linux compile, Docker via Colima (colima start --arch aarch64 --cpu 4 --memory 8)
bin/linux-check.sh test [filter]    # Linux cargo tests + plain-script Python suites
bin/run-tests.sh linux-headless     # headless daemon smoke in Docker
bin/windows-check.sh check          # Windows compile (WW-0, when landed)
cd rust && CARGO_TARGET_DIR=../.build/rust-target cargo build -p ws-rs   # macOS must stay at 20 warnings
cd rust && CARGO_TARGET_DIR=../.build/rust-target cargo test -p ws-rs --features egui   # includes egui tests
```
Egui on the Mac for dev: `WS_FRONTEND=egui` + `--features egui`, ALWAYS with a private
`TMPDIR=$(mktemp -d) WS_HOME=$(mktemp -d)` — the socket is `$TMPDIR/ws-notes.sock`, shared with the live app.

## Gotchas learned
- Homebrew `cargo` shadows rustup's; Windows std only exists in rustup's: use `rustup run stable cargo`.
- Docker Desktop was absent; Colima provides the daemon.
- `rfd` needs features `xdg-portal` + `async-std` on Linux (else `zbus` fails). `xcap` pinned `=0.4.1`.
- Linux Python bridge needs `/usr/bin/python3` >= 3.11 (macOS-only skip of `/usr/bin` in `find_python`).
- Orchestration recipe used (OpenCode workers via `opencode-delegate`, parallel waves with disjoint files, orchestrator re-runs gates and commits with a `Worker:` trailer): `PLAN-cross-platform.md` §O.
