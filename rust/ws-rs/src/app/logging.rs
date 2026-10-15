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
