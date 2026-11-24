//! The CLI client side of `main.swift`: everything a `kitchen-sink MODE …`
//! invocation does *before* (or instead of) becoming the daemon — forward to a
//! running daemon, print replies, hand cold hotkey starts to
//! `bin/kitchen_sink.sh` so TCC attaches to the bundle, and the lock-retry
//! loop that keeps a second daemon from starting.

use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

#[cfg(windows)]
use crate::platform::ipc::{self, Stream};

use crate::app::hotkey;
use crate::app::paths::Paths;
use crate::app::socket::DaemonLock;
use crate::engines::pane_shot::PaneShotArgs;
use crate::engines::screenshot_annotations::ShotArgs;

/// `ipcSocketTimeout`: how long a fire-and-forget launch message may take to connect.
const IPC_TIMEOUT: Duration = Duration::from_secs(1);


/// What `main` does after the client pass.
pub enum Outcome {
    /// Done: exit with this code.
    Exit(i32),
    /// Become the daemon, applying `mode` once it is up (`openCommand`).
    Daemon { mode: Option<String>, lock: DaemonLock },
    /// Become the quick agent (`kitchen-sink quick [show|screenshot]`), which
    /// owns Hyper+S / Hyper+X on its own socket and lock.
    Quick { action: crate::app::quick::StartAction, lock: DaemonLock },
}

/// `sendLaunchMessage`: connect (bounded), write `name\n`, no reply.
pub fn send_launch_message(sock: &str, name: &str) -> bool {
    let Ok(mut s) = connect_bounded(sock) else {
        return false;
    };
    let mut line = name.as_bytes().to_vec();
    line.push(b'\n');
    s.write_all(&line).is_ok()
}

/// `sendRequest`: write `msg\n` and read the whole reply until EOF (or the
/// timeout). `None` when no daemon answers the connect.
#[cfg(unix)]
pub fn send_request(sock: &str, msg: &str, timeout: Duration) -> Option<Vec<u8>> {
    let mut s = UnixStream::connect(sock).ok()?;
    let _ = s.set_read_timeout(Some(timeout));
    let mut line = msg.as_bytes().to_vec();
    line.push(b'\n');
    s.write_all(&line).ok()?;
    let mut out = Vec::new();
    let mut buf = [0u8; 65536];
    loop {
        match s.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => out.extend_from_slice(&buf[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    Some(out)
}

/// Windows: same shape, over a named pipe (recv timeout bounds the read).
#[cfg(windows)]
pub fn send_request(sock: &str, msg: &str, timeout: Duration) -> Option<Vec<u8>> {
    let mut s = ipc::connect_with_timeouts(sock, Some(timeout), None).ok()?;
    let mut line = msg.as_bytes().to_vec();
    line.push(b'\n');
    s.write_all(&line).ok()?;
    let mut out = Vec::new();
    let mut buf = [0u8; 65536];
    loop {
        match s.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => out.extend_from_slice(&buf[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    Some(out)
}

#[cfg(unix)]
fn connect_bounded(sock: &str) -> std::io::Result<UnixStream> {
    // A unix-socket connect either succeeds or fails at once; the only wait is
    // a full backlog, which the write timeout bounds.
    let s = UnixStream::connect(sock)?;
    let _ = s.set_write_timeout(Some(IPC_TIMEOUT));
    Ok(s)
}

#[cfg(windows)]
fn connect_bounded(sock: &str) -> std::io::Result<Stream> {
    // A named-pipe connect either succeeds or fails at once; the only wait is
    // a full backlog, which the send timeout bounds.
    ipc::connect_with_timeouts(sock, None, Some(IPC_TIMEOUT))
}

fn not_running() -> i32 {
    eprintln!("kitchen-sink is not running");
    1
}

/// Hand a cold start to `bin/kitchen_sink.sh MODE` (LaunchServices `open`, so
/// TCC attaches to the bundle) unless launchd started us. Returns only when it
/// cannot exec.
fn exec_launcher(paths: &Paths, mode: &str) {
    #[cfg(target_os = "macos")]
    {
        if unsafe { libc::getppid() } == 1 {
            return;
        }
        let script = format!("{}/bin/kitchen_sink.sh", paths.asset_dir());
        if is_executable(&script) {
            use std::os::unix::process::CommandExt;
            let _ = std::process::Command::new(&script).arg(mode).exec();
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        // No LaunchServices/`open` off macOS: the app re-launches itself,
        // detached (`setsid`), and becomes the daemon.
        let _ = paths;
        let _ = spawn_self_detached(Some(mode));
    }
}

/// Off macOS a cold start is this binary re-executing itself in a new session
/// (`setsid`) so it outlives the caller. `mode` is the launch/hotkey argument.
#[cfg(all(unix, not(target_os = "macos")))]
fn spawn_self_detached(mode: Option<&str>) -> bool {
    use std::os::unix::process::CommandExt;
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let mut cmd = std::process::Command::new(exe);
    if let Some(mode) = mode {
        cmd.arg(mode);
    }
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    cmd.spawn().is_ok()
}

/// Windows: the same detached re-exec, via `DETACHED_PROCESS |
/// CREATE_NEW_PROCESS_GROUP` (the `setsid` counterpart — no console, outlives
/// the caller). `mode` is the launch/hotkey argument.
#[cfg(windows)]
fn spawn_self_detached(mode: Option<&str>) -> bool {
    use std::os::windows::process::CommandExt;
    // winbase.h: DETACHED_PROCESS = 0x00000008, CREATE_NEW_PROCESS_GROUP = 0x00000200.
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let mut cmd = std::process::Command::new(exe);
    if let Some(mode) = mode {
        cmd.arg(mode);
    }
    cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    cmd.spawn().is_ok()
}

#[cfg(target_os = "macos")]
fn is_executable(path: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// The client pass. `args` is the full argv.
pub fn run(args: &[String], paths: &Paths) -> Outcome {
    let sock = paths.notes_socket_path();
    let arg1 = args.get(1).map(String::as_str);

    if std::env::var_os("WS_PING_ONLY").is_some() {
        let name = arg1.unwrap_or(&paths.switcher_window_name);
        return Outcome::Exit(if send_launch_message(&sock, name) { 0 } else { 1 });
    }

    let mut open_command: Option<String> = None;
    match arg1 {
        // Hyper+S: the quick agent owns the palette. No agent = exit 1, and
        // `bin/kitchen_sink.sh` cold-starts it (`quick show`).
        Some("toggle") | Some("quick-toggle") => {
            return Outcome::Exit(if send_launch_message(&paths.quick_socket_path(), "toggle") { 0 } else { 1 });
        }
        Some("quick-show") => {
            return Outcome::Exit(if send_launch_message(&paths.quick_socket_path(), "show") { 0 } else { 1 });
        }
        Some("quick-quit") => {
            return Outcome::Exit(if send_launch_message(&paths.quick_socket_path(), "quit") { 0 } else { not_running() });
        }
        Some("quick-state") => {
            let Some(data) = send_request(&paths.quick_socket_path(), "state", Duration::from_secs(3)) else {
                return Outcome::Exit(not_running());
            };
            let _ = std::io::stdout().write_all(&data);
            return Outcome::Exit(0);
        }
        Some("quick") => {
            // The agent: take its own lock, or hand the start action to the
            // one already running (`open -n` always starts a second process).
            let action = crate::app::quick::start_action(args.get(2).map(String::as_str));
            if let Some(lock) = DaemonLock::try_acquire(paths.quick_lock_path()) {
                return Outcome::Quick { action, lock };
            }
            let qsock = paths.quick_socket_path();
            match action {
                crate::app::quick::StartAction::Show => {
                    send_launch_message(&qsock, "show");
                }
                crate::app::quick::StartAction::Screenshot => {
                    send_launch_message(&qsock, "screenshot");
                }
                crate::app::quick::StartAction::None => {}
            }
            return Outcome::Exit(0);
        }
        Some("jira-poll") => {
            let action = args.get(2).map(String::as_str).unwrap_or("toggle");
            if !["on", "off", "toggle", "setup", "dashboard"].contains(&action) {
                eprintln!("usage: kitchen-sink jira-poll on|off|toggle|setup|dashboard");
                return Outcome::Exit(2);
            }
            let msg = match action {
                "setup" => "jira-setup".to_string(),
                "dashboard" => "jira-dashboard".to_string(),
                a => format!("jira-poll-{a}"),
            };
            return Outcome::Exit(if send_launch_message(&sock, &msg) { 0 } else { not_running() });
        }
        Some(verb @ ("reload" | "restart")) => {
            let Some(data) = send_request(&sock, verb, Duration::from_secs(15)) else {
                return Outcome::Exit(not_running());
            };
            let reply = String::from_utf8_lossy(&data).trim().to_string();
            if reply.is_empty() {
                eprintln!("the running kitchen-sink is too old for '{verb}': restart it once");
                return Outcome::Exit(1);
            }
            println!("{reply}");
            return Outcome::Exit(if reply.contains("\"ok\":true") { 0 } else { 1 });
        }
        Some("setup") => {
            if send_launch_message(&sock, "setup") {
                return Outcome::Exit(0);
            }
            open_command = Some("setup".to_string());
        }
        Some(verb @ ("screenshot" | "quick-screenshot")) => {
            let words: Vec<String> = args[2..].to_vec();
            let parsed = match ShotArgs::parse(&words) {
                Ok(a) => a,
                Err(p) => {
                    eprintln!("kitchen-sink {verb}: {}", p.message);
                    return Outcome::Exit(2);
                }
            };
            let msg = std::iter::once("screenshot".to_string())
                .chain(words.iter().cloned())
                .collect::<Vec<_>>()
                .join("\t");
            // The quick agent first (it owns Hyper+X); `screenshot` then falls
            // back to a running main daemon, `quick-screenshot` does not.
            let mut targets = vec![paths.quick_socket_path()];
            if verb == "screenshot" {
                targets.push(sock.clone());
            }
            if parsed.wants_reply() {
                for t in &targets {
                    let Some(data) = send_request(t, &msg, Duration::from_secs(3600)) else { continue };
                    if data.is_empty() {
                        return Outcome::Exit(1);
                    }
                    let _ = std::io::stdout().write_all(&data);
                    return Outcome::Exit(0);
                }
                return Outcome::Exit(not_running());
            }
            if targets.iter().any(|t| send_launch_message(t, &msg)) {
                return Outcome::Exit(0);
            }
            if verb == "quick-screenshot" {
                // No agent: exit 1. `bin/kitchen_sink.sh screenshot` (which
                // calls this) cold-starts it; exec'ing the launcher from here
                // would loop.
                return Outcome::Exit(1);
            }
            exec_launcher(paths, "screenshot");
            open_command = Some("screenshot".to_string());
        }
        Some("pane-shot") => return Outcome::Exit(pane_shot(&sock, &args[2..])),
        Some(mode) if hotkey::is_hotkey_mode(mode) => {
            if send_launch_message(&sock, mode) {
                return Outcome::Exit(0);
            }
            exec_launcher(paths, mode);
            open_command = Some(if mode == "window" { "files".to_string() } else { mode.to_string() });
        }
        _ => {}
    }
    let arg = arg1.unwrap_or("");
    if arg == "show" {
        open_command = Some("show".to_string());
    }

    // `acquireDaemonLock` + the hand-over loop: a second launch delivers its
    // message to the daemon that holds the lock; a lock holder that never
    // answers is reported, never doubled.
    let lock_path = format!("{sock}.lock");
    if let Some(lock) = DaemonLock::try_acquire(&lock_path) {
        return Outcome::Daemon { mode: open_command, lock };
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let msg = if open_command.is_none() && arg != "setup" { "ping" } else { arg };
        if send_launch_message(&sock, msg) {
            return Outcome::Exit(0);
        }
        let wait_until = Instant::now() + Duration::from_millis(250);
        while Instant::now() < wait_until {
            if let Some(lock) = DaemonLock::try_acquire(&lock_path) {
                return Outcome::Daemon { mode: open_command, lock };
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        if Instant::now() >= deadline {
            eprintln!("kitchen-sink: another daemon holds the lock but does not answer — not starting a second one");
            return Outcome::Exit(1);
        }
    }
}

/// `kitchen-sink pane-shot [ARGS]` (`-` reads the ANSI from stdin).
fn pane_shot(sock: &str, rest: &[String]) -> i32 {
    let mut words: Vec<String> = rest.to_vec();
    let mut tmp: Option<String> = None;
    match PaneShotArgs::parse(&words) {
        Ok(a) => {
            if a.file.as_deref() == Some("-") {
                if let Some(i) = words.iter().position(|w| w == "-") {
                    let path = std::env::temp_dir()
                        .join(format!("pane-shot-{}.ansi", std::process::id()))
                        .to_string_lossy()
                        .into_owned();
                    let mut input = Vec::new();
                    let _ = std::io::stdin().read_to_end(&mut input);
                    let _ = std::fs::write(&path, &input);
                    words[i] = path.clone();
                    tmp = Some(path);
                }
            }
        }
        Err(p) => {
            eprintln!("kitchen-sink pane-shot: {}", p.message);
            return 2;
        }
    }
    let msg = std::iter::once("pane-shot".to_string()).chain(words).collect::<Vec<_>>().join("\t");
    let reply = send_request(sock, &msg, Duration::from_secs(60));
    if let Some(t) = tmp {
        let _ = std::fs::remove_file(t);
    }
    let Some(data) = reply else {
        return not_running();
    };
    let line = String::from_utf8_lossy(&data).trim().to_string();
    if line.is_empty() || line.starts_with("error: ") {
        let why = if line.is_empty() { "no answer" } else { &line[7..] };
        eprintln!("kitchen-sink pane-shot: {why}");
        return 1;
    }
    println!("{line}");
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(words: &[&str]) -> Vec<String> {
        words.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    #[cfg(unix)]
    fn launch_and_request_against_a_live_socket() {
        use std::os::unix::net::UnixListener;
        let path = std::env::temp_dir().join(format!("ws-cli-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for _ in 0..2 {
                let (mut c, _) = listener.accept().unwrap();
                let mut buf = [0u8; 256];
                let n = c.read(&mut buf).unwrap();
                let line = String::from_utf8_lossy(&buf[..n]).to_string();
                if line.starts_with("reload") {
                    c.write_all(b"{\"ok\":true}\n").unwrap();
                }
                seen.push(line);
            }
            seen
        });
        let sock = path.to_string_lossy().into_owned();
        assert!(send_launch_message(&sock, "notes"));
        let reply = send_request(&sock, "reload", Duration::from_secs(5)).unwrap();
        assert_eq!(String::from_utf8_lossy(&reply), "{\"ok\":true}\n");
        assert_eq!(server.join().unwrap(), vec!["notes\n".to_string(), "reload\n".to_string()]);
        let _ = std::fs::remove_file(&path);
        assert!(!send_launch_message(&sock, "notes"), "no daemon: false");
    }
}
