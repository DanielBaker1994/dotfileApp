//! The CLI client side of `main.swift`: everything a `kitchen-sink MODE …`
//! invocation does *before* (or instead of) becoming the daemon — forward to a
//! running daemon, print replies, hand cold hotkey starts to
//! `bin/kitchen_sink.sh` so TCC attaches to the bundle, and the lock-retry
//! loop that keeps a second daemon from starting.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::app::hotkey;
use crate::app::paths::Paths;
use crate::app::socket::DaemonLock;
use crate::engines::pane_shot::PaneShotArgs;
use crate::engines::screenshot_annotations::ShotArgs;

/// `ipcSocketTimeout`: how long a fire-and-forget launch message may take to connect.
const IPC_TIMEOUT: Duration = Duration::from_secs(1);

const COMPARE_USAGE: &str = "usage: kitchen-sink compare [--wait] [--title1 T] [--title2 T] LEFT [RIGHT]";

/// What `main` does after the client pass.
pub enum Outcome {
    /// Done: exit with this code.
    Exit(i32),
    /// Become the daemon, applying `mode` once it is up (`openCommand`).
    Daemon { mode: Option<String>, lock: DaemonLock },
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

fn connect_bounded(sock: &str) -> std::io::Result<UnixStream> {
    // A unix-socket connect either succeeds or fails at once; the only wait is
    // a full backlog, which the write timeout bounds.
    let s = UnixStream::connect(sock)?;
    let _ = s.set_write_timeout(Some(IPC_TIMEOUT));
    Ok(s)
}

fn not_running() -> i32 {
    eprintln!("kitchen-sink is not running");
    1
}

/// Hand a cold start to `bin/kitchen_sink.sh MODE` (LaunchServices `open`, so
/// TCC attaches to the bundle) unless launchd started us. Returns only when it
/// cannot exec.
fn exec_launcher(paths: &Paths, mode: &str) {
    if unsafe { libc::getppid() } == 1 {
        return;
    }
    let script = format!("{}/bin/kitchen_sink.sh", paths.asset_dir());
    if is_executable(&script) {
        use std::os::unix::process::CommandExt;
        let _ = std::process::Command::new(&script).arg(mode).exec();
    }
}

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
        Some("toggle") => {
            return Outcome::Exit(if send_launch_message(&sock, "show") { 0 } else { 1 });
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
        Some("screenshot") => {
            let words: Vec<String> = args[2..].to_vec();
            let parsed = match ShotArgs::parse(&words) {
                Ok(a) => a,
                Err(p) => {
                    eprintln!("kitchen-sink screenshot: {}", p.message);
                    return Outcome::Exit(2);
                }
            };
            let msg = std::iter::once("screenshot".to_string())
                .chain(words.iter().cloned())
                .collect::<Vec<_>>()
                .join("\t");
            if parsed.wants_reply() {
                let Some(data) = send_request(&sock, &msg, Duration::from_secs(3600)) else {
                    return Outcome::Exit(not_running());
                };
                if data.is_empty() {
                    return Outcome::Exit(1);
                }
                let _ = std::io::stdout().write_all(&data);
                return Outcome::Exit(0);
            }
            if send_launch_message(&sock, &msg) {
                return Outcome::Exit(0);
            }
            exec_launcher(paths, "screenshot");
            open_command = Some("screenshot".to_string());
        }
        Some("pane-shot") => return Outcome::Exit(pane_shot(&sock, &args[2..])),
        Some("compare") if args.len() > 2 => return Outcome::Exit(compare(&sock, paths, &args[2..])),
        Some("term") => {
            return Outcome::Exit(if send_launch_message(&sock, "term") { 0 } else { not_running() });
        }
        Some(mode) if hotkey::is_hotkey_mode(mode) => {
            if send_launch_message(&sock, mode) {
                return Outcome::Exit(0);
            }
            exec_launcher(paths, mode);
            open_command = Some(if mode == "window" { "notes".to_string() } else { mode.to_string() });
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

/// The `compare\t…` message for `kitchen-sink compare` argv (paths made
/// absolute and standardized, tabs in titles flattened); `None` = usage error.
pub fn compare_message(rest: &[String], cwd: &str) -> Option<(String, bool)> {
    let mut words: Vec<String> = Vec::new();
    let mut wait = false;
    let mut paths = 0;
    let mut i = 0;
    while i < rest.len() {
        let w = &rest[i];
        if w == "--wait" {
            wait = true;
        } else if (w == "--title1" || w == "--title2") && i + 1 < rest.len() {
            words.push(w.clone());
            words.push(rest[i + 1].replace('\t', " "));
            i += 1;
        } else if w.starts_with('-') && w.len() > 1 {
            return None;
        } else {
            let abs = if w.starts_with('/') { w.clone() } else { format!("{cwd}/{w}") };
            words.push(standardize_path(&abs));
            paths += 1;
        }
        i += 1;
    }
    if !(1..=2).contains(&paths) {
        return None;
    }
    let mut parts = vec!["compare".to_string()];
    if wait {
        parts.push("--wait".to_string());
    }
    parts.extend(words);
    Some((parts.join("\t"), wait))
}

/// `NSString.standardizingPath` for absolute paths: drop `.`, resolve `..`.
fn standardize_path(p: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for part in p.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            x => out.push(x),
        }
    }
    format!("/{}", out.join("/"))
}

fn compare(sock: &str, paths: &Paths, rest: &[String]) -> i32 {
    let cwd = std::env::current_dir()
        .map(|d| d.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "/".to_string());
    let Some((msg, wait)) = compare_message(rest, &cwd) else {
        eprintln!("{COMPARE_USAGE}");
        return 2;
    };
    let deliver = || -> bool {
        if wait {
            send_request(sock, &msg, Duration::from_secs(7 * 86400)).is_some()
        } else {
            send_launch_message(sock, &msg)
        }
    };
    let mut ok = deliver();
    if !ok {
        // Cold start the bundle in the background, then retry for 15 s.
        if let Some(bundle) = &paths.app_bundle_path {
            if Path::new(bundle).exists() {
                let _ = std::process::Command::new("/usr/bin/open").args(["-g", bundle]).status();
            }
        }
        let deadline = Instant::now() + Duration::from_secs(15);
        while !ok && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(200));
            if send_launch_message(sock, "ping") {
                ok = deliver();
            }
        }
    }
    if ok {
        0
    } else {
        not_running()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(words: &[&str]) -> Vec<String> {
        words.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn compare_message_matches_main_swift() {
        let (msg, wait) =
            compare_message(&v(&["--wait", "--title1", "a\tb", "x/../a.txt", "/abs/./b.txt"]), "/cwd").unwrap();
        assert!(wait);
        assert_eq!(msg, "compare\t--wait\t--title1\ta b\t/cwd/a.txt\t/abs/b.txt");
        let (msg, wait) = compare_message(&v(&["one.txt"]), "/w").unwrap();
        assert!(!wait);
        assert_eq!(msg, "compare\t/w/one.txt");
        assert!(compare_message(&v(&["--bogus", "a"]), "/w").is_none(), "unknown flag");
        assert!(compare_message(&v(&["a", "b", "c"]), "/w").is_none(), "three paths");
        assert!(compare_message(&v(&["--wait"]), "/w").is_none(), "no paths");
    }

    #[test]
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
