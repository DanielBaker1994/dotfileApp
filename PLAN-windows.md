# PLAN-windows.md — kitchen-sink compiles for Windows (compile-only phase)

Orchestrator mechanics are identical to `PLAN-cross-platform.md` §O (relay + `opencode-delegate`,
briefs in `.build/xplat-briefs/`, workers never commit, orchestrator re-runs gates and commits with a
`Worker:` trailer). Source of truth for blockers: `PLAN-windows-scouting.md`.

## 0. Scope
**Goal:** `cargo check --workspace --all-targets --target x86_64-pc-windows-gnu` passes, with real
(not stubbed) Windows implementations where the seam is clear. **No runtime verification** is possible on
the owner's Mac; every Windows body is "type-checks, untested" and is listed in §8 as such. A Windows
runtime/CI phase is separate and not started here.
**Toolchain:** Homebrew `cargo` shadows rustup's, and only rustup's has the Windows std. The check
script must run `rustup run stable cargo` (rustc 1.92.0; target `x86_64-pc-windows-gnu` and
`mingw-w64` already installed by the orchestrator). No Docker involved.

## 2. Rules for every worker (paste verbatim into every brief)
1. Read `AGENTS.md` and `rule.md` first. Work directly on `main`: no branches/worktrees, never push, **never commit**,
   no `git add/commit/stash`. Leave changes in the working tree.
2. **Touch only the files in your card.** If a fix needs another file, stop and report (file + reason).
3. **macOS and Linux behaviour must not change.** Keep the existing Unix body byte-for-byte under
   `#[cfg(unix)]` (or its current `#[cfg(target_os="macos")]`/`not(macos)` gate) and add the Windows body as a
   separate `#[cfg(windows)]` item/branch. Never rewrite a Unix body "because the crate is simpler".
4. Prefer the smallest correct Windows body. New dependency only if the card names it, exact version pin, in
   `[target.'cfg(windows)'.dependencies]` of `rust/ws-rs/Cargo.toml` (re-read the file right before editing; add only your lines).
5. No `unwrap()`/`expect()` where the old code returned `None`/`false`/`()`. No tokio/async.
6. Verify in this order and paste the last 5 lines of each:
   ```bash
   cd rust && CARGO_TARGET_DIR=../.build/rust-target cargo build -p ws-rs        # macOS: 20 warnings, no new
   cd rust && CARGO_TARGET_DIR=../.build/rust-target cargo test -p ws-rs <module> # if applicable
   bin/linux-check.sh check                                                       # Linux still compiles
   bin/windows-check.sh check                                                     # Windows compile (from WW-0)
   ```
   Never run `./ws build`, never launch the app.
7. If your card grows past ~150 changed lines (excluding tests) stop and report, unless the card says otherwise.
8. Report in <= 15 lines: files, sites changed, sites left (file:line + reason), verify tails, surprises, and a list of
   every Windows body you wrote that is **untested at runtime** with the one assumption most likely to be wrong.

## 4. Cards
- **WW-0** `bin/windows-check.sh check|shell-less` + `bin/run-tests.sh windows` + `.gitignore`; run it and report the per-file error table. (serial)
- **Wave WA (after table; parallel, disjoint files):** WA-1 ipc/lock (serial, strong model: `app/socket.rs`, `app/cli.rs`, `engines/nvim_rpc.rs`, new `platform/ipc.rs`, `platform/lock.rs`, `platform/mod.rs`); WA-2 `app/hotkey.rs`, `app/python_helper.rs`, `engines/ai_format.rs`, `engines/pane_shot.rs` (PermissionsExt + `libc::kill`); WA-3 `engines/recent_files.rs`, `engines/screenshot_annotations.rs`, `views/jira_view.rs`, `views/tools.rs` (`fnmatch`/`localtime_r`/`mktime`/`/bin/bash`); WA-4 gate macOS-only modules (`app/jira_poll.rs` launchd etc.) + paths/env (`app/paths.rs` and friends) per table.
- **Wave WB:** leftovers one card per file. **Exit:** `bin/windows-check.sh check` exit 0; macOS 20 warnings / tests unchanged; `bin/linux-check.sh check` and `test` unchanged.
- **Wave WC (optional, opportunistic):** Python portability (`fcntl`, `start_new_session`, `os.pathsep`, macOS CLIs) with `Tests/test_*.py` still green on macOS.
- **Wave WD:** docs (`AGENTS.md` "Windows (compile-only)" section), plan log.

## 8. Progress log
- WW-0 (landed): `bin/windows-check.sh check|errors` (needs `PATH` prepended with rustup's toolchain bin: `rustup run stable cargo` alone still spawns Homebrew rustc). Baseline errors (ws-rs only; swiftterm-shim/ws-helpers/deps compile): socket.rs 43, cli.rs 7, frontend_egui/notes.rs 5, path_shelf.rs 4, pane_shot.rs 4, python_helper.rs 4, recent_files.rs 3, nvim_rpc.rs 3, notes_vim.rs 2, screenshot_annotations.rs 2, ai_format.rs 2, hotkey.rs 2, screenshot.rs 1, jira_view.rs 1, jira_poll.rs 1.
