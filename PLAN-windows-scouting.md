# PLAN-windows-scouting.md — Windows blockers (W7-D, docs only)

Scouting only: **no code changed.** Every `file:line` below was read/grepped in this
checkout (`/Users/danielbaker/.config/kitchen-sink`, `main`, 2026-10-12) against
`rust/ws-rs/src`, `pylib/`, `settings_hub/`, `notify/`, `jira/`, `confluence/`.

Context: the Linux port (`PLAN-cross-platform.md`, Waves 0–5 landed) already put the
whole app behind `#[cfg(target_os = "macos")]` and added a second **egui/eframe**
frontend. Windows therefore inherits nearly all of that: the AppKit halves are compiled
out, `frontend_egui/` and the pure controller/engines are portable, and the known Linux
branches (`platform/clipboard.rs` arboard, `platform/dialog.rs` rfd, `platform/mod.rs`
`open`, `platform/dispatch.rs`, `platform/app.rs`, `platform/screens.rs`,
`hotkey_prep` no-op) compile for any non-macOS target. What remains is the **Unix-only
core** listed below — it is a shorter list than Linux's was, because the seam work is
done.

The card's "proposed seam" holds up: **`platform::ipc` (interprocess) + `platform::lock`
(fs4)**, plus three small helpers (`::process`, `::fs`, `::time`).

## Summary counts (verified)

| Category | Sites | Non-test | Blocking Windows build? |
|---|---:|---:|---|
| Unix-domain socket / AF_UNIX IPC (`UnixStream`/`UnixListener`) | 3 files (+2 test) | 3 files | **yes** — daemon + client won't compile |
| Raw `libc` socket/fd calls (`socket/bind/listen/accept/read/write/dup/fcntl/setsockopt/close`) | `socket.rs` ×47 | 47 | **yes** (inside the ipc seam) |
| `flock` daemon lock | 1 (`socket.rs`) | 1 | **yes** |
| `libc::kill` (kill wedged Python) | 1 | 1 | yes (1-liner; `Child::kill`) |
| `CommandExt::pre_exec` + `setsid` (detached spawn) | 1 | 1 | yes |
| `CommandExt::exec` / `getppid()==1` (macOS-gated already) | 2 | 2 | no (macOS-only) |
| `launchctl` + `getuid` jira poller | 1 module | 1 module | yes (gate the module) |
| `libc::localtime_r` / `mktime` | 2 files | 2 | yes (`localtime_r`) |
| `PermissionsExt` (`mode() & 0o111`) | 9 (5 non-test) | 5 | **yes** (no `mode` on Windows) |
| `libc::fnmatch` | 1 | 1 | yes |
| `libc::stat` (+`st_birthtime` already gated) | 1 | 1 | verify fields |
| `libc::getxattr` + `XATTR_NOFOLLOW` | 1 (already `#[cfg(macos)]` + non-macOS `None` twin) | 0 | no |
| Hard-coded shell `/bin/bash` | 1 | 1 | yes |
| `/tmp/` + `HOME`/`USER`/`TMPDIR` + `starts_with('/')` | 5 files | 5 | yes |
| PATH split on `':'` / `python3` names | 1 file | 1 | yes |
| Python `fcntl` (`flock`, `termios` ioctl) | 4 files / 17 refs | 4 files | yes |
| Python `start_new_session=True` | 2 | 2 | silently ignored on Windows |
| Python macOS CLIs (`osascript`/`sysctl`/`vm_stat`/`pmset`/`lsappinfo`/`pbcopy`/`open`) | 5 files | 5 | degrades (fallbacks exist) |
| Python PATH sep `":"` | 1 (`script_bridge.py:37`) | 1 | yes |
| AX helpers (`rust/ws-helpers`) | 2 | 2 | already stubbed off macOS |

## Rust blockers (file:line → proposed seam)

### 1. Unix-domain socket IPC — the daemon's whole transport (`platform::ipc`)

- `app/socket.rs:15-17` `use std::os::unix::{ffi::OsStrExt, io::RawFd, net::UnixStream}`.
- Server: `bind_listener` `socket.rs:139-175` (`libc::socket(AF_UNIX,…)` :151,
  `sockaddr_un` :155-161, `bind` :161, `listen` :167, `fcntl(O_NONBLOCK)` :172-173);
  `accept_loop` :177-198 (`libc::accept` :183, `EAGAIN|EINTR` :189);
  `configure_conn` :200-249 (`fcntl` :205-207, `setsockopt` `SO_SNDTIMEO` :213,
  `SO_NOSIGPIPE` :224 (macOS-gated), `SO_RCVTIMEO` :241).
- `Reply` holds a raw fd (`dup` :50, `close` :67) and may write later from any thread
  (`read` :254, `write` :317).
- `DaemonLock` :332-362 (see §2); `connect` :365; `send_line` :371; `read_line` :379.
- Client: `app/cli.rs:8` `UnixStream`; `send_launch_message` :33-40,
  `send_request` :44-61, `connect_bounded` :63-69. Test `UnixListener` :413.
- nvim: `engines/nvim_rpc.rs:8` `UnixStream`; `UnixTransport` :69-79. **Easiest** —
  the `Transport` trait (`:57-61`) already abstracts read/write, so only
  `UnixTransport::connect` + the `Connector` impl need a Windows body.
- Tests (macOS-gated/ignore): `engines/path_shelf.rs:729,986` `UnixListener`.

Seam: **`interprocess`** (`LocalSocketListener`/`LocalSocketStream` — Unix socket on
Unix, named pipe on Windows). Two real design constraints to call out up front:
(a) `Reply` is a **duplicated fd written later from arbitrary threads**; a
`LocalSocketStream` is not `dup`-able, so it must become a cloned stream or an mpsc
back to the accept thread; (b) the `SO_*TIMEO` timeouts are Unix-only —
`LocalSocketStream::set_read_timeout`/`set_write_timeout` covers them cross-platform.

### 2. Daemon lock — `app/socket.rs:338-362` (`platform::lock`)

`DaemonLock::try_acquire`: `libc::open(O_RDWR|O_CREAT|O_CLOEXEC, 0o600)` :341,
`libc::flock(LOCK_EX|LOCK_NB)` :350, `close` :360. Seam: **`fs4`**
(`FileExt::try_lock_exclusive`) — Windows maps to `LockFileEx`. (Pin the exact version
at implementation time.)

### 3. Process — `libc::kill`, `setsid`, `getppid`, `exec` (`platform::process`)

- `app/python_helper.rs:288` `libc::kill(pid as i32, libc::SIGKILL)` (kill a wedged
  interpreter after a 5 s timeout). Portable fix: keep the `Child` and call
  `child.kill()`, or a `platform::process::kill(pid)`.
- `app/cli.rs:104,113-116` `CommandExt::pre_exec(|| { libc::setsid(); })` in
  `spawn_self_detached` (the non-macOS cold start). Windows:
  `std::os::windows::process::CommandExt::creation_flags(DETACHED_PROCESS |
  CREATE_NEW_PROCESS_GROUP)`.
- `app/cli.rs:82` `libc::getppid()==1` and `:87` `CommandExt::exec` are already inside
  `#[cfg(target_os = "macos")]` — no Windows work.

### 4. launchd jira poller — `app/jira_poll.rs`

`libc::getuid` :180; `Command::new("/bin/launchctl")` :168; verbs :184,199,204,206.
Whole module is macOS-shaped. Windows: drop, or a Task Scheduler (`schtasks`) twin.

### 5. Time — `localtime_r` / `mktime` (`platform::time`)

- `engines/screenshot_annotations.rs:1466-1479` `ShotDate::from_unix_local` uses
  `libc::localtime_r` :1470. Not available on Windows MSVC (there it is `localtime_s`
  with a different signature). Test helper :2331-2339 uses `libc::mktime`.
- `views/jira_view.rs:244` `libc::mktime` in the **non-macOS** `parse_stamp_local`
  (already `#[cfg(not(target_os = "macos"))]`; the `time` crate parses the fields).
  Wizard: `mktime` exists in the `libc` crate on Windows; keep, or route both through
  the `time` crate (already a non-macOS dep).

### 6. Permission bits — `platform::fs::is_executable` (5 non-test sites)

`use std::os::unix::fs::PermissionsExt` + `permissions().mode() & 0o111`:
- `app/hotkey.rs:38` (`fn executable`, :37)
- `app/python_helper.rs:222` (`fn is_executable`, :221)
- `engines/ai_format.rs:158` (`fn is_executable`, :157)
- `engines/pane_shot.rs:348` (`fn is_executable_file`, :347)
- `app/cli.rs:123` (already `#[cfg(target_os = "macos")]`, :121)

Test-only (leave): `engines/nvim_rpc.rs:903`, `views/notes_vim.rs:1003`,
`engines/pane_shot.rs:489`, `views/ai.rs:2715` (`#[cfg(unix)]`).

Windows: no mode bits — `is_executable` becomes "is a file" (optionally check
`.exe`/`.bat`/`.cmd`). One seam function replaces all non-test sites.

### 7. `libc::fnmatch` — `engines/recent_files.rs:404`

`fnmatch_str` (`libc::fnmatch`, :404) backs the ignore-rule matcher (`keep`, used by
`in_scope`/`scan_dir`). Windows MSVC CRT has no `fnmatch`. Seam:
`platform::glob::match(pattern, name)` — a tiny matcher or a crate (`wildmatch`).

### 8. `libc::stat` / xattr — `engines/recent_files.rs`

`stamp` :503-514 uses `libc::stat` :505-506; `st_birthtime` is **already gated**
(:509-512 uses `st_mtime` elsewhere). `libc::stat` exists on Windows in the `libc`
crate — verify field names (`st_mtime` present; confirm on the Windows target).
`getxattr`+`XATTR_NOFOLLOW` :553-585 is already macOS-gated with a non-macOS `None`
twin (:587) — done.

### 9. Hard-coded shell — `views/tools.rs:761`

`Command::new("/bin/bash").arg("-c").arg(&script)` for the health-check scripts.
Seam: `platform::shell_command()` → `/bin/sh -c` on Unix, `cmd.exe /C` on Windows.
Tests only (leave/ignore): `app/process_run.rs:81,89,96,110` (`/bin/echo`,`/bin/cat`,
`/bin/sh`).

### 10. Paths & env

- `app/paths.rs:20` `popup_tmp_dir` falls back to `"/tmp/"` when `TMPDIR` is unset;
  `TMPDIR` :146 and `HOME` :147 are read in `from_env` (`config.rs:203` reads `USER`).
  Windows: `TEMP`/`TMP`, `USERPROFILE`, `USERNAME`; `popup_tmp_dir` should use
  `std::env::temp_dir()`.
- `app/paths.rs:41` `resolve_symlinks` tests `path.starts_with('/')` for "absolute" —
  misses `C:\…`/`\\…`. Use `Path::is_absolute()`.
- `app/config.rs:330` `aerospace_socket_path: "/tmp/bobko.aerospace-…"`, `:855`
  `save_dir: "/tmp/"` — config-over-code (rule 3); change the shipped default, not code.
- `app/python_helper.rs:245` splits `PATH` on `':'` and appends `python3`; candidate
  list (:246-256) is Homebrew-only. Windows: `os.pathsep` (`;`), `python.exe` / `py -3`.
  (This is also the open Linux gap: `skip_path_dir` :236.)
- `engines/recent_files.rs:411,450,788` rewrite `/tmp`→`/private/tmp` (macOS). Already a
  known Linux gap (tests ignored, :1379); Windows needs the same treatment.
- `views/note_find.rs:242` runs the configured `rg` — fine where ripgrep is on PATH.

## Python blockers (file:line → replacement)

- **`fcntl` flock singleton/locking** — `notify/notify_poll.py:20,194`;
  `jira/jira_poll.py:137,355,397`; `jira/jira_status.py:29,90,94`. Windows:
  `msvcrt.locking` / lock-file, or `portalocker`.
- **`fcntl`/`termios` terminal size** — `settings_hub/tui/app.py:751,754`
  (`TIOCGWINSZ`). Windows: `shutil.get_terminal_size()`.
- **`start_new_session=True`** — `settings_hub/cli.py:358`, `notify/notify_poll.py:238`
  (ignored on Windows). Use `creationflags=DETACHED_PROCESS|CREATE_NEW_PROCESS_GROUP`.
- **`os.chmod` mode bits** — `settings_hub/writer.py:36`, `notify/webex_api.py:76`
  (limited/no-op on Windows; acceptable, note it).
- **macOS CLIs** — `settings_hub/cli.py:401` `osascript` (JXA `NSScreen`; already has a
  `{"vw":1440,"vh":870,"scale":2}` fallback); `pylib/status.py:32` `sysctl`, `:42`
  `vm_stat`, `:62` `pmset`; `notify/notify_poll.py:163` `lsappinfo`;
  `settings_hub/tui/lineedit.py:12,19` `pbcopy`/`pbpaste`;
  `notify/webex_api.py:260` `subprocess.run(["open", url])`.
  Windows: `webbrowser`/`os.startfile`, `ctypes` (`GlobalMemoryStatusEx`,
  `GetSystemPowerStatus`), `clip`/PowerShell; `lsappinfo`/AX have no equivalent
  (fall back).
- **PATH separator** — `pylib/helper/script_bridge.py:37` joins with `":"`. Use
  `os.pathsep`.
- **`/`-joined paths** — 19 `f"…/{…}"` sites measured (the card's 22; mine counts only
  f-string `/{`). Spot-check shows the sampled ones are **jira/confluence API URLs**
  (`jira/jira_api.py:361,951,…`, `confluence/confluence_api.py:267,340`), which are
  fine; the filesystem ones should use `os.path.join` (112 such calls already exist).
- **`~` / Trash** — `os.path.expanduser("~")` is portable; `pylib/file_ops.py:130`
  `~/.Trash` has no Windows equivalent (same as the Linux gap).
- **Shebangs** — only `#!/usr/bin/env python3` (`notify/notify_poll.py:1`,
  `notify/webex_api.py:1`) — portable. (`settings_hub` is entered via the `ws-settings`
  wrapper, no homebrew shebang to fix.)
- **AX helpers** — `rust/ws-helpers/src/dock_badges.rs:146-150`,
  `webex_unread.rs` last lines: already `#[cfg(not(target_os = "macos"))] fn main() {
  eprintln!("macOS only"); exit(1) }`. Windows: keep the stub; `notify` falls back.

## Already portable (verified — no Windows work)

`platform/clipboard.rs:30-42` (arboard), `platform/dialog.rs:47-60` (rfd),
`platform/mod.rs:24-31` (`open` crate `open_url`/`open_path`),
`platform/dispatch.rs:31-57` (main-queue FIFO), `platform/app.rs:33,50`
(frontmost `None` / crate version), `platform/screens.rs:105-120` (empty — see below),
`app/hotkey.rs` non-macOS `hotkey_prep` no-op, `main.rs` non-macOS = egui/headless.
arboard and rfd are gated `cfg(not(target_os = "macos"))`, so they already resolve on
Windows (verify rfd's `xdg-portal`+`async-std` feature doesn't force a Linux backend
on Windows at implementation time).

## Proposed seams (name → crate → replaces)

| Seam | Crate / API | Replaces |
|---|---|---|
| `platform::ipc` | `interprocess` `LocalSocket*` | `socket.rs`, `cli.rs`, `nvim_rpc.rs` Unix sockets |
| `platform::lock` | `fs4` `FileExt::try_lock_exclusive` | `DaemonLock` `flock` |
| `platform::process` | std `Child::kill`; Windows `creation_flags` | `libc::kill`, `setsid`/`pre_exec` |
| `platform::fs` | mode bits (Unix) vs `is_file` (Windows) | 5 `PermissionsExt` sites |
| `platform::time` | `time` crate or `localtime_s` | `localtime_r` / `mktime` |
| `platform::glob` | tiny matcher / `wildmatch` | `libc::fnmatch` |
| `platform::shell` | `/bin/sh -c` vs `cmd /C` | `views/tools.rs:761` |
| `platform::screens` | winit monitor API (populate empty stub) | multi-monitor on Windows |

macOS code paths stay byte-for-byte identical (rule 3): each seam adds a
`#[cfg(windows)]`/`#[cfg(not(unix))]` body and leaves the existing macOS branch alone.

## Recommended order (Windows phase, after Linux lands)

1. **`platform::ipc` + `platform::lock`** — nothing runs without them; largest and it
   establishes the pattern (incl. the `Reply`-dup redesign). Do nvim last within this
   step (its `Transport` trait already isolates it).
2. **`platform::process` + `platform::fs`** — small, mechanical (kill/detach + 5 sites).
3. **`platform::time` + `platform::glob` + `platform::shell`** — small, isolated.
4. **Gate/retire macOS-only Rust modules**: `app/jira_poll.rs` (launchd), the
   `/tmp`→`/private/tmp` path rewrite, `app/cli.rs` macOS exec branch (already gated).
5. **Paths/env normalization**: `TEMP`/`USERPROFILE`/`USERNAME`, `popup_tmp_dir`,
   `Path::is_absolute`, PATH `os.pathsep`, `python3`→`py -3`/`python` (also fixes the
   known Debian `find_python` gap).
6. **Python portability** (opportunistic, mirrors W2-PY): fcntl→locks,
   `start_new_session`→`creationflags`, `osascript`/status/`lsappinfo`/`pbcopy`/`open`
   → Windows, PATH sep.
7. **Polish**: `platform::screens` from winit, `frontmost_app` best-effort, packaging/docs.

## Notes / corrections to the card

- The card's "`PermissionsExt` (5 sites)" is exactly right **counting non-test sites**
  (hotkey, python_helper, ai_format, pane_shot, cli); there are 9 `use` lines total,
  4 test-only.
- The card's "`/`-joins (22)" — I measured **19** f-string `f"…/{…}"` sites (and 112
  `os.path.join`). The sampled f-string joins are jira/confluence **URLs**, not
  filesystem paths, so the count is not a Windows risk by itself; the risk is the
  `script_bridge.py:37` PATH join and `os.path.join`-vs-string building.
- `app/jira_poll.rs` (launchd) and `rust/ws-helpers` (AX) are macOS-only modules that
  simply need gating; they are not "port me" work.
