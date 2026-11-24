//! `wsLog` (kitchen_sink.swift) — the app's own log sink.
//!
//! The Swift original teed every log line to stderr AND a file
//! (`appendToFile(debugLogPath, …)`, `debugLogPath = "/tmp/ws-debug.log"`).
//! The Rust port kept only the in-memory `Vec` and stderr, so `controller.log`
//! and the daemon's `eprintln!` never reached disk. This restores the file half
//! and lands it in the date-based log dir: `logs/YYYY_MM_DD/daemon.log`
//! (`WS_LOG_DIR` aware), the same place pylib/log_hub.py writes.
//!
//! Behaviour, matching Swift: every line goes to stderr *and* the file. stderr
//! is still what `log stream --predicate 'process == "kitchen-sink"'` shows.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::{Mutex, OnceLock};

/// (day bucket, open file). Opening lazily and only on a day change keeps the
/// hot path free of `date` subprocesses while still rolling at midnight.
static SINK: OnceLock<Mutex<Option<(u64, File)>>> = OnceLock::new();

fn day_bucket() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() / 86400)
        .unwrap_or(0)
}

/// Append `line` to the date-based daemon log (best effort; a failure is
/// silent — logging must never take the app down, same as Swift's `try?`).
fn append_to_file(line: &str) {
    let cell = SINK.get_or_init(|| Mutex::new(None));
    let mut guard = match cell.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    let bucket = day_bucket();
    if guard.as_ref().map(|(b, _)| *b) != Some(bucket) {
        let path = crate::app::paths::Paths::from_env().log_path("daemon");
        let file = OpenOptions::new().create(true).append(true).open(&path).ok();
        *guard = file.map(|f| (bucket, f));
    }
    if let Some((_, file)) = guard.as_mut() {
        let _ = writeln!(file, "{line}");
    }
}

/// `wsLog(_:)`: write to stderr and to `logs/YYYY_MM_DD/daemon.log`.
pub fn ws_log(msg: &str) {
    let line = format!("ws: {msg}");
    eprintln!("{line}");
    append_to_file(&line);
}

/// The last socket request's receive time (`hotkey: … recv->shown` latency).
static LAST_RECV: Mutex<Option<(std::time::Instant, String)>> = Mutex::new(None);

/// Stamp a socket request as just received (called from `socket.rs`).
pub fn note_recv(msg: &str) {
    if let Ok(mut g) = LAST_RECV.lock() {
        *g = Some((std::time::Instant::now(), msg.split('\t').next().unwrap_or("").to_string()));
    }
}

/// Take the pending receive stamp if it is fresh (< 2 s) and, when `verb`
/// is given, belongs to that request verb. Returns (verb, elapsed ms).
pub fn take_recv_ms(verb: Option<&str>) -> Option<(String, u128)> {
    let mut g = LAST_RECV.lock().ok()?;
    let (at, msg) = g.as_ref()?.clone();
    if at.elapsed().as_secs_f64() >= 2.0 || verb.is_some_and(|v| v != msg) {
        return None;
    }
    *g = None;
    Some((msg, at.elapsed().as_millis()))
}

/// Log `hotkey: <msg> recv->shown <N>ms` for the request that opened a panel.
pub fn log_hotkey_shown(verb: Option<&str>) {
    if let Some((msg, ms)) = take_recv_ms(verb) {
        ws_log(&format!("hotkey: {msg} recv->shown {ms}ms"));
    }
}

/// Send panics to `daemon.log` too. The app is launched by LaunchServices, so
/// its stderr goes nowhere: without this a panic on the main thread quits the
/// app (exit 101, no crash report) and leaves no trace of where it happened.
/// The default hook still runs afterwards, so stderr output is unchanged.
pub fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("<unnamed>");
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let bt = std::backtrace::Backtrace::force_capture();
        append_to_file(&format!(
            "ws: PANIC (unix time {secs}, pid {}, thread '{name}'): {info}\n{bt}",
            std::process::id()
        ));
        default_hook(info);
    }));
}

/// Main-thread stall watchdog (macOS). Every 500ms a background thread asks
/// the main queue to bump a counter; if the main thread has not answered for
/// 1s it logs "MAIN THREAD STALLED", runs `/usr/bin/sample` on ourselves into
/// `logs/YYYY_MM_DD/stall-<unix>.txt` (shows exactly what the main thread is
/// blocked on), and logs when it recovers and how long it was stuck. This is
/// what a "menu-bar icon does nothing" report looks like from the inside.
#[cfg(target_os = "macos")]
pub fn start_main_thread_watchdog() {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    let acked = Arc::new(AtomicU64::new(0));
    let a2 = acked.clone();
    let _ = std::thread::Builder::new().name("main-watchdog".into()).spawn(move || {
        let mut seq = 0u64;
        let mut stalled_since: Option<Instant> = None;
        loop {
            seq += 1;
            let sent = Instant::now();
            let ack = a2.clone();
            let s = seq;
            dispatch2::DispatchQueue::main().exec_async(move || ack.store(s, Ordering::SeqCst));
            std::thread::sleep(Duration::from_millis(500));
            if a2.load(Ordering::SeqCst) >= seq {
                if let Some(t0) = stalled_since.take() {
                    ws_log(&format!("main thread recovered after {:.1}s stall", t0.elapsed().as_secs_f64()));
                }
                continue;
            }
            // Not answered within 500ms: give it a second, then report.
            std::thread::sleep(Duration::from_millis(500));
            if a2.load(Ordering::SeqCst) >= seq {
                continue;
            }
            let t0 = *stalled_since.get_or_insert(sent);
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let pid = std::process::id();
            let out = crate::app::paths::Paths::from_env().log_path(&format!("stall-{stamp}")).replace(".log", ".txt");
            ws_log(&format!(
                "MAIN THREAD STALLED {:.1}s (menu-bar clicks/UI frozen) — sampling to {out}",
                t0.elapsed().as_secs_f64()
            ));
            let _ = std::process::Command::new("/usr/bin/sample")
                .args([&pid.to_string(), "2", "-file", &out])
                .output();
            // Wait for recovery without re-sampling every second.
            while a2.load(Ordering::SeqCst) < seq {
                std::thread::sleep(Duration::from_millis(250));
                if t0.elapsed() > Duration::from_secs(600) {
                    break;
                }
            }
            if let Some(t0) = stalled_since.take() {
                ws_log(&format!("main thread recovered after {:.1}s stall", t0.elapsed().as_secs_f64()));
            }
        }
    });
}

/// Unix time in seconds (fractional).
pub fn now_secs_f64() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Unix time in whole seconds.
pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

