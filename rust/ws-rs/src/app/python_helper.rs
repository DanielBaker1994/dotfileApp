//! Port of `PythonHelper.swift` — the persistent Python worker client.
//!
//! JSON-lines over stdin/stdout: request `{id, method, params}`, reply
//! `{id, ok, result}` or `{id, ok:false, error:{message}}`. The Python side
//! (`pylib/`) is frozen; this is the Rust mirror of the Swift client.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

#[derive(Debug)]
pub struct HelperFailure(pub String);

impl std::fmt::Display for HelperFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for HelperFailure {}

type PendingMap = Arc<Mutex<HashMap<u64, Sender<Result<Value, String>>>>>;

struct Inner {
    child: Option<Child>,
    stdin: Option<std::process::ChildStdin>,
    next_id: u64,
    lib_dir: String,
    pending: PendingMap,
    /// `None` = not looked up yet; `Some(None)` = looked up, none found.
    python: Option<Option<String>>,
}

pub struct PythonHelper {
    inner: Mutex<Inner>,
}

static HELPER: OnceLock<PythonHelper> = OnceLock::new();

impl PythonHelper {
    fn new() -> Self {
        PythonHelper {
            inner: Mutex::new(Inner {
                child: None,
                stdin: None,
                next_id: 1,
                lib_dir: String::new(),
                pending: Arc::new(Mutex::new(HashMap::new())),
                python: None,
            }),
        }
    }

    pub fn shared() -> &'static PythonHelper {
        HELPER.get_or_init(Self::new)
    }

    pub fn configure(&self, lib_dir: &str) {
        self.inner.lock().unwrap().lib_dir = lib_dir.to_string();
    }

    pub fn lib_dir(&self) -> String {
        self.inner.lock().unwrap().lib_dir.clone()
    }

    /// Blocking call with the default 10 s timeout.
    pub fn call_default(&self, method: &str, params: Value) -> Result<Value, HelperFailure> {
        self.call(method, params, Duration::from_secs(10), Duration::from_secs(5))
    }

    /// Block the calling thread until the worker answers. `timeout` bounds the
    /// wait for the reply; `grace` is added on top for poll bookkeeping.
    pub fn call(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
        grace: Duration,
    ) -> Result<Value, HelperFailure> {
        let (tx, rx): (Sender<Result<Value, String>>, Receiver<Result<Value, String>>) = channel();
        {
            let mut inner = self.inner.lock().unwrap();
            if inner.lib_dir.is_empty() {
                return Err(HelperFailure("python helper not configured".into()));
            }
            if !self.ensure_started(&mut inner) {
                return Err(HelperFailure(
                    "python 3.11+ not found — brew install python, or set WS_PYTHON".into(),
                ));
            }
            let id = inner.next_id;
            inner.next_id += 1;
            inner.pending.lock().unwrap().insert(id, tx);
            let line = format!("{}\n", json!({"id": id, "method": method, "params": params}));
            let wrote = inner
                .stdin
                .as_mut()
                .map(|si| si.write_all(line.as_bytes()).is_ok() && si.flush().is_ok())
                .unwrap_or(false);
            if !wrote {
                inner.pending.lock().unwrap().remove(&id);
                return Err(HelperFailure(format!("helper write failed for {method}")));
            }
        }
        match rx.recv_timeout(timeout + grace) {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(HelperFailure(e)),
            Err(RecvTimeoutError::Timeout) => {
                Err(HelperFailure(format!("helper timed out: {method}")))
            }
            Err(RecvTimeoutError::Disconnected) => Err(HelperFailure("helper exited".into())),
        }
    }

    #[cfg(test)]
    fn kill_for_testing(&self) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(child) = inner.child.as_mut() {
            let _ = child.kill();
        }
        inner.child = None;
        inner.stdin = None;
    }

    fn ensure_started(&self, inner: &mut Inner) -> bool {
        if let Some(child) = inner.child.as_mut() {
            match child.try_wait() {
                Ok(None) => return true,
                _ => {
                    inner.child = None;
                    inner.stdin = None;
                }
            }
        }
        let lib = inner.lib_dir.clone();
        let python = match &inner.python {
            Some(p) => p.clone(),
            None => {
                let found = find_python(&lib);
                inner.python = Some(found.clone());
                found
            }
        };
        let Some(python) = python else {
            return false;
        };
        let mut cmd = Command::new(&python);
        cmd.args(["-B", "-m", "helper"])
            .current_dir(&lib)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let base_path = std::env::var("PATH").unwrap_or_default();
        cmd.env(
            "PATH",
            format!("/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:{base_path}"),
        );
        let extra_py = std::env::var("PYTHONPATH")
            .map(|p| format!(":{p}"))
            .unwrap_or_default();
        cmd.env("PYTHONPATH", format!("{lib}{extra_py}"));
        cmd.env("PYTHONDONTWRITEBYTECODE", "1");
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(_) => return false,
        };
        let stdout = child.stdout.take();
        let stdin = child.stdin.take();
        let map: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        inner.pending = map.clone();
        inner.stdin = stdin;
        inner.child = Some(child);
        if let Some(stdout) = stdout {
            std::thread::spawn(move || reader_loop(stdout, map));
        }
        true
    }
}

fn reader_loop(stdout: ChildStdout, pending: PendingMap) {
    let reader = BufReader::new(stdout);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        let Ok(obj) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(id) = obj.get("id").and_then(Value::as_u64) else {
            continue;
        };
        let sender = pending.lock().unwrap().remove(&id);
        if let Some(tx) = sender {
            let ok = obj.get("ok").and_then(Value::as_bool).unwrap_or(false);
            let result = if ok {
                Ok(obj.get("result").cloned().unwrap_or(Value::Null))
            } else {
                let msg = obj
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("helper error")
                    .to_string();
                Err(msg)
            };
            let _ = tx.send(result);
        }
    }
    // EOF: fail everything still waiting on this generation.
    let mut map = pending.lock().unwrap();
    let drained: Vec<_> = map.drain().map(|(_, tx)| tx).collect();
    drop(map);
    for tx in drained {
        let _ = tx.send(Err("helper exited".into()));
    }
}

fn is_executable(path: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(path) {
        Ok(m) => m.is_file() && (m.permissions().mode() & 0o111) != 0,
        Err(_) => false,
    }
}

fn find_python(lib: &str) -> Option<String> {
    let mut candidates: Vec<String> = Vec::new();
    if let Ok(own) = std::env::var("WS_PYTHON") {
        if !own.is_empty() {
            candidates.push(own);
        }
    }
    candidates.push("/opt/homebrew/bin/python3".into());
    candidates.push("/usr/local/bin/python3".into());
    if let Ok(path) = std::env::var("PATH") {
        for dir in path.split(':') {
            if dir.is_empty() || dir == "/usr/bin" || dir == "/bin" {
                continue;
            }
            candidates.push(format!("{dir}/python3"));
        }
    }
    for c in candidates {
        if is_executable(&c) && python_runs_311(&c, lib) {
            return Some(c);
        }
    }
    None
}

fn python_runs_311(path: &str, lib: &str) -> bool {
    let Ok(mut child) = Command::new(path)
        .args(["-c", "import sys; sys.exit(0 if sys.version_info >= (3, 11) else 1)"])
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONPATH", lib)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    // Bounded wait so a wedged interpreter can't hang discovery.
    let (tx, rx) = channel();
    let pid = child.id();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait().map(|s| s.code()));
    });
    match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(Some(0))) => true,
        _ => {
            unsafe {
                libc::kill(pid as i32, libc::SIGKILL);
            }
            false
        }
    }
}

/// True when `lib` looks like a `pylib` directory with the helper package.
pub fn lib_has_helper(lib: &str) -> bool {
    Path::new(lib).join("helper").join("__main__.py").is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo_pylib() -> String {
        // rust/ws-rs/src/app/python_helper.rs -> repo root -> pylib
        let dir = env!("CARGO_MANIFEST_DIR");
        format!("{dir}/../../pylib")
    }

    #[test]
    fn discovery_is_bounded() {
        // Must not hang; returns Some on a dev machine with brew python.
        let _ = find_python(&repo_pylib());
    }

    #[test]
    fn ping_round_trip_when_python_present() {
        let lib = repo_pylib();
        if find_python(&lib).is_none() || !lib_has_helper(&lib) {
            eprintln!("skipping: no python 3.11+ or pylib/helper not found");
            return;
        }
        let h = PythonHelper::new();
        h.configure(&lib);
        let res = h.call("ping", json!({}), Duration::from_secs(15), Duration::from_secs(5));
        assert!(res.is_ok(), "ping failed: {res:?}");
    }

    #[test]
    fn unknown_method_is_an_error() {
        let lib = repo_pylib();
        if find_python(&lib).is_none() || !lib_has_helper(&lib) {
            return;
        }
        let h = PythonHelper::new();
        h.configure(&lib);
        let res = h.call(
            "definitely.not.a.method",
            json!({}),
            Duration::from_secs(15),
            Duration::from_secs(5),
        );
        assert!(res.is_err());
        let msg = res.unwrap_err().0;
        assert!(msg.contains("unknown method"), "fails with its name: {msg}");
    }

    fn live_helper() -> Option<PythonHelper> {
        let lib = repo_pylib();
        if find_python(&lib).is_none() || !lib_has_helper(&lib) {
            return None;
        }
        let h = PythonHelper::new();
        h.configure(&lib);
        Some(h)
    }

    #[test]
    fn ping_answers_with_a_version() {
        let Some(h) = live_helper() else { return };
        let v = h.call("ping", json!({}), Duration::from_secs(15), Duration::from_secs(5)).unwrap();
        assert_eq!(v["version"], json!(1), "{v}");
    }

    #[test]
    fn many_calls_in_flight_all_answer() {
        let Some(h) = live_helper() else { return };
        let h = Arc::new(h);
        let workers: Vec<_> = (0..25)
            .map(|_| {
                let h = h.clone();
                std::thread::spawn(move || {
                    h.call("ping", json!({}), Duration::from_secs(20), Duration::from_secs(5)).is_ok()
                })
            })
            .collect();
        let answered = workers.into_iter().filter_map(|w| w.join().ok()).filter(|ok| *ok).count();
        assert_eq!(answered, 25, "25 calls in flight all answer");
    }

    #[test]
    fn the_worker_restarts_after_being_killed() {
        let Some(h) = live_helper() else { return };
        assert!(h.call("ping", json!({}), Duration::from_secs(15), Duration::from_secs(5)).is_ok());
        h.kill_for_testing();
        let res = h.call("ping", json!({}), Duration::from_secs(20), Duration::from_secs(5));
        assert!(res.is_ok(), "recovered: {res:?}");
    }

    #[test]
    fn a_package_less_worker_fails_fast() {
        let lib = repo_pylib();
        if find_python(&lib).is_none() {
            return;
        }
        let empty = std::env::temp_dir().join(format!("ws-helper-empty-{}", std::process::id()));
        std::fs::create_dir_all(&empty).unwrap();
        let h = PythonHelper::new();
        h.configure(empty.to_str().unwrap());
        let started = std::time::Instant::now();
        let res = h.call("ping", json!({}), Duration::from_secs(8), Duration::from_secs(2));
        assert!(res.is_err(), "no helper package: must fail");
        assert!(started.elapsed() < Duration::from_secs(11), "fails within the timeout");
        let _ = std::fs::remove_dir_all(&empty);
    }
}
