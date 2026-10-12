# PORTS-STATUS.md — Linux & Windows ports: what works, what was verified

Start here if you are a new agent picking up cross-platform work. Keep this file current
(update it in the same commit as any wave that changes the status). Deep history:
`PLAN-cross-platform.md` (Linux, §8 log), `PLAN-windows.md` + `PLAN-windows-scouting.md` (Windows).
macOS is the product; AppKit stays and is untouched by both ports (rule: Unix/macOS bodies are
never rewritten, other platforms get separate `#[cfg]` bodies).

## Status at a glance (last updated 2026-10-12, after Windows compile-only phase)

| | macOS | Linux | Windows |
|---|---|---|---|
| Compiles | yes (AppKit, default) | **yes** (`bin/linux-check.sh check`, Docker arm64) | **yes** (`bin/windows-check.sh check`, `x86_64-pc-windows-gnu`, cross-check only) |
| Rust tests | 753 pass / 4 ignored (796 with `--features egui`) | 789 pass / 5 ignored in Docker | **not run** (cannot run on the Mac) |
| Daemon / socket API | yes | **runs headless in Docker** (`state`, `do:open:files` verified) | **compiles; never executed.** Windows IPC = named pipes via `interprocess`, lock via `fs4` |
| GUI | AppKit (the real app) | **egui/eframe frontend written, never run on Linux** (see below) | egui frontend **compiles** for Windows (same code as Linux); never shown on a Windows desktop |
| Real desktop run | daily-driven | **never** (no Linux box available) | **never** (no Windows machine available) |

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
- **Phase done: compile-only.** `bin/windows-check.sh check` passes (`cargo check --workspace --all-targets
  --target x86_64-pc-windows-gnu`, run on the Mac with rustup + `brew install mingw-w64`). Nothing has ever been
  executed on Windows; **do not claim any Windows feature works.** The app *should* behave like the Linux egui
  build (same frontend), but the transport/process/time layers below are new, untested code.
- Seams: `platform/ipc.rs` (pure `pipe_name` fn + Windows `interprocess` client/server used by `app/socket.rs`,
  `app/cli.rs`, `engines/nvim_rpc.rs`), `platform/lock.rs` (`fs4` daemon lock). Unix bodies are untouched under `#[cfg(unix)]`.
- **Known runtime blockers (compile fine, will not work yet):** PATH is still split on `':'` in
  `views/notes_vim.rs` (`launch_env_from`) and `frontend_egui/notes.rs` (`program_available`) so `nvim.exe` is never
  found; `jira_poll.rs` `python()` is Homebrew-only and `sync_launch_agent` is a silent no-op; `views/tools.rs`
  health scripts are POSIX shell run via `cmd /C`; Windows `is_executable` needs an extension (`.exe/.cmd/.bat/.com`)
  so bare-name tools are rejected; Python side (`fcntl`, `start_new_session`, macOS CLIs, `os.pathsep`) untouched;
  `HOME`/`TMPDIR`/`/tmp` assumptions in paths/config; screens seam is empty; no packaging.
- Next steps if Windows matters: get a Windows box or a GitHub Actions `windows-latest` job, run
  `cargo test -p ws-rs` + a headless smoke test there first; then fix the blockers above (cards WB/WC in `PLAN-windows.md`).

#### Untested Windows bodies (compile only; one assumption most likely wrong each)
1. `socket.rs` `accept_loop`: `ListenerNonblockingMode::Accept` returns `WouldBlock` when idle so the stop flag is polled.
2. `socket.rs` `Reply = Arc<Stream>`: a deferred reply written after the accept loop drops its handle still delivers.
3. `nvim_rpc.rs` `WinTransport::shutdown` is a no-op; `Client::shutdown`'s `join` may hang if nvim never closes the pipe.
4. `platform/ipc.rs` `pipe_name`: `\\.\pipe\ws-<16hex>` is valid/stable and daemon and client feed the identical path string.
5. `platform/lock.rs` `DaemonLock`: closing the handle releases `LockFileEx` like `flock` drop.
6. `python_helper.rs`/`hotkey.rs`/`ai_format.rs`/`pane_shot.rs` `is_executable` (extension-based) and `Child::kill()` reaping a wedged Python without leaking a grandchild.
7. `recent_files.rs` `glob_match` (POSIX `fnmatch` semantics; equivalence-tested against libc on Unix only).
8. `screenshot_annotations.rs`, `jira_view.rs` local-time twins: `UtcOffset::current_local_offset()` succeeds on Windows (falls back to UTC).
9. `jira_poll.rs` silent no-op; `frontend_egui/notes.rs` executable check; `tools.rs` `cmd /C` twin.

## How to verify (all from the Mac)
```bash
bin/linux-check.sh check            # Linux compile, Docker via Colima (colima start --arch aarch64 --cpu 4 --memory 8)
bin/linux-check.sh test [filter]    # Linux cargo tests + plain-script Python suites
bin/run-tests.sh linux-headless     # headless daemon smoke in Docker
bin/windows-check.sh check|errors   # Windows compile cross-check (errors = per-file table)
cd rust && CARGO_TARGET_DIR=../.build/rust-target cargo build -p ws-rs   # macOS must stay at 20 warnings
cd rust && CARGO_TARGET_DIR=../.build/rust-target cargo test -p ws-rs --features egui   # includes egui tests
```
Egui on the Mac for dev: `WS_FRONTEND=egui` + `--features egui`, ALWAYS with a private
`TMPDIR=$(mktemp -d) WS_HOME=$(mktemp -d)` — the socket is `$TMPDIR/ws-notes.sock`, shared with the live app.

## Gotchas learned
- Homebrew `cargo` shadows rustup's; Windows std only exists in rustup's: use `rustup run stable cargo`.
- Docker Desktop was absent; Colima provides the daemon.
- `bin/windows-check.sh` prepends rustup's toolchain bin to PATH: `rustup run stable cargo` alone still spawns Homebrew's `rustc` (E0463). Windows pins: `interprocess =2.4.5`, `fs4 =0.13.1` (1.x renamed `try_lock_exclusive`).
- `rfd` needs features `xdg-portal` + `async-std` on Linux (else `zbus` fails). `xcap` pinned `=0.4.1`.
- Linux Python bridge needs `/usr/bin/python3` >= 3.11 (macOS-only skip of `/usr/bin` in `find_python`).
- Orchestration recipe used (OpenCode workers via `opencode-delegate`, parallel waves with disjoint files, orchestrator re-runs gates and commits with a `Worker:` trailer): `PLAN-cross-platform.md` §O.
