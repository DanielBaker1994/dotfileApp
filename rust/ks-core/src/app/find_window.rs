//! `[find-window]` palette command, shared by the main daemon and the quick agent.

use std::sync::Arc;

/// How a find-window run reports to the user (a toast on the daemon UI).
pub type ToastFn = Arc<dyn Fn(String, &'static str) + Send + Sync>;

/// `[find-window]` palette command, shared by the main daemon and the quick
/// agent: wait `delay` seconds (so the user can point at or focus the window
/// in question), run `script` (winwatch --pids2 candidate rows), then offer a
/// kill picker. `text` is the config text; progress goes through `toast`.
/// Never activates the app; the work is off the main thread.
pub fn run_find_window(text: &str, toast: ToastFn) {
    let vars = crate::views::tools::section_vars(text, "find-window");
    let delay: f64 = vars.get("delay").and_then(|v| v.parse().ok()).unwrap_or(2.0);
    let script = vars.get("script").cloned().unwrap_or_default();
    let script = match script.strip_prefix("~/") {
        Some(rest) => format!("{}/{rest}", std::env::var("HOME").unwrap_or_default()),
        None => script,
    };
    toast(format!("Point at (or focus) the window — reading in {delay}s"), "scope");
    crate::app::logging::ws_log(&format!("find-window: sampling in {delay}s via {script}"));
    let secs: u64 = vars.get("dialog-seconds").and_then(|v| v.parse().ok()).unwrap_or(30);
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs_f64(delay.max(0.0)));
        let me = std::process::id() as i64;
        let o = match std::process::Command::new("/bin/bash")
            .arg("-c")
            .arg(&script)
            .env("KS_EXCLUDE_PID", me.to_string())
            .output()
        {
            Ok(o) if o.status.success() => o,
            Ok(o) => {
                let err = String::from_utf8_lossy(&o.stderr);
                let why = err.lines().last().unwrap_or("non-zero exit").trim().to_string();
                toast(format!("find-window failed: {why}"), "exclamationmark.triangle.fill");
                return;
            }
            Err(e) => {
                toast(format!("find-window failed: {e}"), "exclamationmark.triangle.fill");
                return;
            }
        };
        // winwatch --pids2: kind<TAB>pid<TAB>owner<TAB>WxH<TAB>note, kinds
        // P (under pointer), A (AX focused), F (frontmost app), T (small window).
        // One picker row per pid, labelled by the first (best) kind it showed up as.
        let text = String::from_utf8_lossy(&o.stdout).to_string();
        let mut rows: Vec<(i64, String)> = Vec::new();
        for line in text.lines() {
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() < 3 {
                continue;
            }
            let Ok(p) = f[1].parse::<i64>() else { continue };
            if p == me || p <= 1 || rows.iter().any(|r| r.0 == p) {
                continue;
            }
            let kind = match f[0] {
                "P" => "Under pointer",
                "A" => "Focused window",
                "F" => "Frontmost app",
                "T" => "Small window",
                _ => continue,
            };
            let size = f.get(3).filter(|s| **s != "-").map(|s| format!(" {s}")).unwrap_or_default();
            let note = f.get(4).filter(|s| !s.is_empty()).map(|s| format!(" [{s}]")).unwrap_or_default();
            rows.push((p, format!("{kind}: {} (pid {p}){size}{note}", f[2])));
        }
        if rows.is_empty() {
            toast("find-window: no candidate windows".into(), "scope");
            return;
        }
        let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
        let items = rows.iter().map(|r| format!("\"{}\"", esc(&r.1))).collect::<Vec<_>>().join(", ");
        // `choose from list` takes any number of rows (display dialog caps at
        // 3 buttons) but has no timeout, so the osascript is killed after `secs`.
        let first = format!("\"{}\"", esc(&rows[0].1));
        let osa = format!(
            "choose from list {{{items}}} with title \"Find Window PID\" \
             with prompt \"Kill which process? (SIGTERM, then SIGKILL)\" default items {{{first}}} \
             OK button name \"Kill\" cancel button name \"Cancel\""
        );
        let Ok(child) = std::process::Command::new("/usr/bin/osascript")
            .args(["-e", &osa])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
        else {
            return;
        };
        let child_pid = child.id();
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let done2 = done.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(secs));
            // Only while still waiting on it, so a recycled pid is never hit.
            if !done2.load(std::sync::atomic::Ordering::SeqCst) {
                let _ = std::process::Command::new("/bin/kill").arg(child_pid.to_string()).status();
            }
        });
        let r = child.wait_with_output();
        done.store(true, std::sync::atomic::Ordering::SeqCst);
        let Ok(r) = r else { return };
        // Cancel prints "false"; a pick prints the row text.
        let reply = String::from_utf8_lossy(&r.stdout).trim().to_string();
        let pid = rows.iter().find(|row| row.1 == reply).map(|row| row.0);
        let Some(pid) = pid else { return };
        if pid == me || pid <= 1 {
            return;
        }
        // SIGTERM first; SIGKILL if it is still there a second later.
        let ok = std::process::Command::new("/bin/bash")
            .args(["-c", &format!("kill {pid} && sleep 1; kill -0 {pid} 2>/dev/null && kill -9 {pid}; ! kill -0 {pid} 2>/dev/null")])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            toast(format!("Killed pid {pid}"), "checkmark");
        } else {
            toast(format!("Couldn't kill pid {pid}"), "exclamationmark.triangle.fill");
        }
    });
}
