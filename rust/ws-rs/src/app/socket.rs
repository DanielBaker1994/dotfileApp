//! Port of `startCommandServer` / `testQuery` / the daemon lock from
//! `kitchen_sink.swift` (plus `sendLaunchMessage` / `sendRequest` as client
//! helpers). One Unix stream socket; one request per connection.
//!
//! Wire behavior (parity):
//! - `ping` -> close, no reply.
//! - `state` -> state JSON + `\n`.
//! - `do:ACTION` -> the action's JSON reply, or (no error) the full state JSON.
//! - `reload` / `restart` / `screenshot\t…` / `compare\t…` / `pane-shot\t…`
//!   -> one raw line when requested.
//! - anything else -> `launch`, no reply.

use std::ffi::CString;
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::io::RawFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::Value;

const RECV_TIMEOUT_SECS: i64 = 2;
const SEND_TIMEOUT_SECS: i64 = 5;
const READ_MAX: usize = 2048;
const LISTEN_BACKLOG: i32 = 4;

const RAW_VERBS: [&str; 6] = [
    "reload",
    "restart",
    "screenshot",
    "screenshot-permission",
    "compare",
    "pane-shot",
];

/// The reply side of one raw-verb connection: a dup of the client fd that the
/// handler may answer now or later, from any thread (Swift keeps `cfd` open
/// for `compare --wait`, screenshot and pane-shot replies). Dropping it
/// closes the connection.
pub struct Reply {
    fd: RawFd,
}

impl Reply {
    fn dup(fd: RawFd) -> Option<Reply> {
        let d = unsafe { libc::dup(fd) };
        (d >= 0).then_some(Reply { fd: d })
    }

    /// Write one newline-terminated line, then close.
    pub fn send(self, line: &str) {
        write_line(self.fd, line);
    }

    /// Write raw bytes (screenshot `--raw` PNG / geometry), then close.
    pub fn send_bytes(self, data: &[u8]) {
        write_all(self.fd, data);
    }
}

impl Drop for Reply {
    fn drop(&mut self) {
        unsafe { libc::close(self.fd) };
    }
}

/// The app side of the command protocol. The server calls these from its
/// accept thread; implementors must be `Send + Sync`.
pub trait CommandHandler: Send + Sync {
    /// Full state document for `state` and for a `do:` that reports no error.
    fn state_json(&self) -> Value;
    /// Handle a `do:` verb. `None` = no error: fall through to `state_json`.
    fn do_action(&self, action: &str) -> Option<Value>;
    /// Handle a raw-verb request (`reload`, `screenshot`, `compare`, …).
    /// `None` = the request wants no reply (close silently).
    fn raw_request(&self, verb: &str, rest: &str) -> Option<String>;
    /// Take a raw verb asynchronously: keep `reply` to answer later (or drop
    /// it to close with no reply) and return `None`; hand it back to fall
    /// through to [`Self::raw_request`].
    fn raw_deferred(&self, _verb: &str, _rest: &str, reply: Reply) -> Option<Reply> {
        Some(reply)
    }
    /// Anything else is a launch/hotkey message, delivered with no reply.
    fn launch(&self, message: &str);
}

/// A bound listener + its accept loop on a background thread.
pub struct CommandServer {
    path: PathBuf,
    fd: Arc<AtomicI32>,
    handle: Mutex<Option<JoinHandle<()>>>,
}

impl CommandServer {
    /// Bind `path` (a stale socket file is removed first) and start accepting.
    pub fn start<P: AsRef<Path>>(
        path: P,
        handler: Arc<dyn CommandHandler>,
    ) -> io::Result<CommandServer> {
        let path = path.as_ref().to_path_buf();
        let fd = bind_listener(&path)?;
        let shared = Arc::new(AtomicI32::new(fd));
        let thread_fd = Arc::clone(&shared);
        let handle = thread::spawn(move || accept_loop(thread_fd, handler));
        Ok(CommandServer {
            path,
            fd: shared,
            handle: Mutex::new(Some(handle)),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Stop accepting, join the thread and unlink the socket file.
    pub fn stop(&mut self) {
        let fd = self.fd.swap(-1, Ordering::SeqCst);
        if fd >= 0 {
            unsafe { libc::close(fd) };
        }
        if let Some(h) = self.handle.lock().unwrap().take() {
            let _ = h.join();
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

impl Drop for CommandServer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn bind_listener(path: &Path) -> io::Result<RawFd> {
    // A crashed daemon leaves the socket file behind; unlink before bind.
    let _ = std::fs::remove_file(path);
    let cpath = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "socket path has NUL"))?;
    let bytes = cpath.as_bytes();
    if bytes.len() >= 104 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "socket path too long for sun_path",
        ));
    }
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (i, b) in bytes.iter().enumerate() {
        addr.sun_path[i] = *b as libc::c_char;
    }
    let len = std::mem::size_of::<libc::sockaddr_un>() as libc::socklen_t;
    let rc = unsafe { libc::bind(fd, &addr as *const _ as *const libc::sockaddr, len) };
    if rc != 0 {
        let e = io::Error::last_os_error();
        unsafe { libc::close(fd) };
        return Err(e);
    }
    if unsafe { libc::listen(fd, LISTEN_BACKLOG) } != 0 {
        let e = io::Error::last_os_error();
        unsafe { libc::close(fd) };
        return Err(e);
    }
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL, 0) };
    unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
    Ok(fd)
}

fn accept_loop(listener: Arc<AtomicI32>, handler: Arc<dyn CommandHandler>) {
    loop {
        let fd = listener.load(Ordering::SeqCst);
        if fd < 0 {
            break;
        }
        let cfd = unsafe { libc::accept(fd, std::ptr::null_mut(), std::ptr::null_mut()) };
        if cfd < 0 {
            if listener.load(Ordering::SeqCst) < 0 {
                break;
            }
            let e = io::Error::last_os_error();
            if matches!(e.raw_os_error(), Some(libc::EAGAIN) | Some(libc::EINTR)) {
                thread::sleep(Duration::from_millis(5));
            }
            continue;
        }
        configure_conn(cfd);
        handle_conn(cfd, handler.as_ref());
        unsafe { libc::close(cfd) };
    }
}

fn configure_conn(fd: RawFd) {
    // On macOS an accepted socket inherits the listener's O_NONBLOCK: a reply
    // larger than the socket buffer (~8 KB — a `state` with compare rows) then
    // hits EAGAIN and is cut short. Replies are written blocking, bounded by
    // SO_SNDTIMEO so a stalled client cannot wedge the accept loop.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL, 0) };
    if flags >= 0 {
        unsafe { libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK) };
    }
    let send_tv = libc::timeval {
        tv_sec: SEND_TIMEOUT_SECS as libc::time_t,
        tv_usec: 0,
    };
    unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_SNDTIMEO,
            &send_tv as *const _ as *const libc::c_void,
            std::mem::size_of::<libc::timeval>() as libc::socklen_t,
        )
    };
    let one: libc::c_int = 1;
    unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_NOSIGPIPE,
            &one as *const _ as *const libc::c_void,
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        )
    };
    let tv = libc::timeval {
        tv_sec: RECV_TIMEOUT_SECS as libc::time_t,
        tv_usec: 0,
    };
    unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_RCVTIMEO,
            &tv as *const _ as *const libc::c_void,
            std::mem::size_of::<libc::timeval>() as libc::socklen_t,
        )
    };
}

fn handle_conn(fd: RawFd, handler: &dyn CommandHandler) {
    let mut buf = [0u8; READ_MAX];
    let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
    let had_bytes = n > 0;
    let query = if had_bytes {
        String::from_utf8_lossy(&buf[..n as usize])
            .trim()
            .to_string()
    } else {
        String::new()
    };

    if query == "ping" {
        return;
    }

    if query == "state" {
        write_json(fd, &handler.state_json());
        return;
    }

    if let Some(action) = query.strip_prefix("do:") {
        let reply = handler
            .do_action(action)
            .unwrap_or_else(|| handler.state_json());
        write_json(fd, &reply);
        return;
    }

    let (verb, rest) = match query.split_once('\t') {
        Some((v, r)) => (v, r),
        None => (query.as_str(), ""),
    };
    if RAW_VERBS.contains(&verb) {
        if let Some(reply) = Reply::dup(fd) {
            match handler.raw_deferred(verb, rest, reply) {
                None => return,
                Some(unused) => drop(unused),
            }
        }
        if let Some(line) = handler.raw_request(verb, rest) {
            write_line(fd, &line);
        }
        return;
    }

    if had_bytes {
        handler.launch(&query);
    }
}

fn write_json(fd: RawFd, value: &Value) {
    let s = serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string());
    write_line(fd, &s);
}

fn write_line(fd: RawFd, s: &str) {
    let mut data = Vec::with_capacity(s.len() + 1);
    data.extend_from_slice(s.as_bytes());
    data.push(b'\n');
    write_all(fd, &data);
}

fn write_all(fd: RawFd, mut data: &[u8]) {
    while !data.is_empty() {
        let n = unsafe { libc::write(fd, data.as_ptr() as *const libc::c_void, data.len()) };
        if n < 0 {
            if io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return;
        }
        if n == 0 {
            return;
        }
        data = &data[n as usize..];
    }
}

/// An exclusive daemon lock on `<socket>.lock`, held until dropped.
pub struct DaemonLock {
    fd: RawFd,
}

impl DaemonLock {
    /// `flock(LOCK_EX | LOCK_NB)`; `None` if the lock is held elsewhere.
    pub fn try_acquire<P: AsRef<Path>>(path: P) -> Option<DaemonLock> {
        let cpath = CString::new(path.as_ref().as_os_str().as_bytes()).ok()?;
        let fd = unsafe {
            libc::open(
                cpath.as_ptr(),
                libc::O_RDWR | libc::O_CREAT | libc::O_CLOEXEC,
                0o600,
            )
        };
        if fd < 0 {
            return None;
        }
        if unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            unsafe { libc::close(fd) };
            return None;
        }
        Some(DaemonLock { fd })
    }
}

impl Drop for DaemonLock {
    fn drop(&mut self) {
        unsafe { libc::close(self.fd) };
    }
}

/// Connect to a command server.
pub fn connect<P: AsRef<Path>>(path: P) -> io::Result<UnixStream> {
    UnixStream::connect(path)
}

/// Send one request line (the caller's message plus `\n`). Written in a single
/// syscall so a server that closes right after reading can't race our newline.
pub fn send_line(stream: &mut UnixStream, line: &str) -> io::Result<()> {
    let mut data = Vec::with_capacity(line.len() + 1);
    data.extend_from_slice(line.as_bytes());
    data.push(b'\n');
    stream.write_all(&data)
}

/// Read one reply line. `None` = EOF with no bytes (e.g. `ping`).
pub fn read_line(stream: &mut UnixStream) -> io::Result<Option<String>> {
    let mut out = Vec::new();
    let mut saw = false;
    let mut byte = [0u8; 1];
    loop {
        match stream.read(&mut byte) {
            Ok(0) => break,
            Ok(_) => {
                saw = true;
                if byte[0] == b'\n' {
                    break;
                }
                out.push(byte[0]);
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    if !saw {
        Ok(None)
    } else {
        Ok(Some(String::from_utf8_lossy(&out).into_owned()))
    }
}

/// One-shot client: connect, send `msg`, read one reply line (or `None`).
pub fn request<P: AsRef<Path>>(path: P, msg: &str) -> io::Result<Option<String>> {
    let mut stream = connect(path)?;
    send_line(&mut stream, msg)?;
    read_line(&mut stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::atomic::AtomicU64;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[derive(Default)]
    struct FakeHandler {
        state: Value,
        actions: HashMap<String, Value>,
        raw: HashMap<String, String>,
        launched: Mutex<Vec<String>>,
    }

    impl CommandHandler for FakeHandler {
        fn state_json(&self) -> Value {
            self.state.clone()
        }
        fn do_action(&self, action: &str) -> Option<Value> {
            self.actions.get(action).cloned()
        }
        fn raw_request(&self, verb: &str, _rest: &str) -> Option<String> {
            self.raw.get(verb).cloned()
        }
        fn launch(&self, message: &str) {
            self.launched.lock().unwrap().push(message.to_string());
        }
    }

    fn temp_path(tag: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "ws-rs-{tag}-{}-{n}-{nanos}.sock",
            std::process::id()
        ))
    }

    fn start(handler: Arc<FakeHandler>, tag: &str) -> (CommandServer, PathBuf) {
        let path = temp_path(tag);
        let server =
            CommandServer::start(&path, handler as Arc<dyn CommandHandler>).expect("start server");
        (server, path)
    }

    #[test]
    fn ping_yields_eof_with_no_reply() {
        let h = Arc::new(FakeHandler::default());
        let (_s, path) = start(h, "ping");
        assert_eq!(request(&path, "ping").unwrap(), None);
    }

    #[test]
    fn state_returns_json() {
        let h = Arc::new(FakeHandler {
            state: json!({"view": "notes", "visible": true}),
            ..Default::default()
        });
        let (_s, path) = start(h, "state");
        let line = request(&path, "state").unwrap().unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v, json!({"view": "notes", "visible": true}));
    }

    #[test]
    fn do_known_returns_action_json() {
        let mut h = FakeHandler {
            state: json!({"view": "notes"}),
            ..Default::default()
        };
        h.actions
            .insert("cycle".into(), json!({"error": "unknown view"}));
        let h = Arc::new(h);
        let (_s, path) = start(h, "do-known");
        let line = request(&path, "do:cycle").unwrap().unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v, json!({"error": "unknown view"}));
    }

    #[test]
    fn do_unknown_falls_through_to_state() {
        let h = Arc::new(FakeHandler {
            state: json!({"view": "files", "n": 7}),
            ..Default::default()
        });
        let (_s, path) = start(h, "do-unknown");
        let line = request(&path, "do:nonexistent").unwrap().unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v, json!({"view": "files", "n": 7}));
    }

    #[test]
    fn raw_verb_returns_its_line() {
        let mut h = FakeHandler::default();
        h.raw.insert("reload".into(), "{\"ok\":true}".into());
        let h = Arc::new(h);
        let (_s, path) = start(h, "raw");
        assert_eq!(
            request(&path, "reload").unwrap().as_deref(),
            Some("{\"ok\":true}")
        );
    }

    #[test]
    fn raw_verb_with_args_gets_rest() {
        #[derive(Default)]
        struct EchoRaw {
            seen: Mutex<Vec<(String, String)>>,
        }
        impl CommandHandler for EchoRaw {
            fn state_json(&self) -> Value {
                json!({})
            }
            fn do_action(&self, _action: &str) -> Option<Value> {
                None
            }
            fn raw_request(&self, verb: &str, rest: &str) -> Option<String> {
                self.seen
                    .lock()
                    .unwrap()
                    .push((verb.to_string(), rest.to_string()));
                Some(format!("{verb}:{rest}"))
            }
            fn launch(&self, _message: &str) {}
        }
        let h = Arc::new(EchoRaw::default());
        let path = temp_path("raw-args");
        let _s = CommandServer::start(&path, h.clone() as Arc<dyn CommandHandler>).unwrap();
        let reply = request(&path, "compare\t--wait /a /b").unwrap().unwrap();
        assert_eq!(reply, "compare:--wait /a /b");
        assert_eq!(
            h.seen.lock().unwrap().as_slice(),
            &[("compare".to_string(), "--wait /a /b".to_string())]
        );
    }

    #[test]
    fn large_state_reply_is_not_truncated() {
        // > the ~8 KB unix socket buffer: must arrive whole (accepted fds
        // inherit the listener's O_NONBLOCK on macOS).
        let big = "x".repeat(200_000);
        let h = Arc::new(FakeHandler { state: json!({ "big": big }), ..Default::default() });
        let (_s, path) = start(h, "big-state");
        let reply = request(&path, "state").unwrap().unwrap();
        let v: Value = serde_json::from_str(&reply).expect("whole JSON");
        assert_eq!(v["big"].as_str().unwrap().len(), 200_000);
    }

    #[test]
    fn deferred_raw_reply_answers_from_another_thread() {
        struct Later;
        impl CommandHandler for Later {
            fn state_json(&self) -> Value {
                json!({})
            }
            fn do_action(&self, _action: &str) -> Option<Value> {
                None
            }
            fn raw_request(&self, _verb: &str, _rest: &str) -> Option<String> {
                Some("sync".to_string())
            }
            fn raw_deferred(&self, verb: &str, _rest: &str, reply: Reply) -> Option<Reply> {
                if verb != "compare" {
                    return Some(reply);
                }
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    reply.send("done");
                });
                None
            }
            fn launch(&self, _message: &str) {}
        }
        let path = temp_path("raw-deferred");
        let _s = CommandServer::start(&path, Arc::new(Later) as Arc<dyn CommandHandler>).unwrap();
        assert_eq!(request(&path, "compare\t--wait\t/a").unwrap().as_deref(), Some("done"));
        assert_eq!(request(&path, "reload").unwrap().as_deref(), Some("sync"), "handed back: sync path");
    }

    #[test]
    fn raw_verb_without_reply_closes_silently() {
        let h = Arc::new(FakeHandler::default());
        let (_s, path) = start(h, "raw-none");
        assert_eq!(request(&path, "screenshot").unwrap(), None);
    }

    #[test]
    fn launch_message_is_delivered() {
        let h = Arc::new(FakeHandler::default());
        let (_s, path) = start(h.clone(), "launch");
        assert_eq!(request(&path, "notes").unwrap(), None);
        assert_eq!(h.launched.lock().unwrap().as_slice(), &["notes".to_string()]);
    }

    #[test]
    fn daemon_lock_is_exclusive() {
        let path = temp_path("lock");
        let lock = DaemonLock::try_acquire(&path).expect("first acquire");
        assert!(DaemonLock::try_acquire(&path).is_none(), "second must fail");
        drop(lock);
        // Other tests spawn child processes in parallel; a fork/exec window can
        // briefly inherit the lock fd, delaying the release, so retry a moment.
        let mut reacquired = false;
        for _ in 0..200 {
            if DaemonLock::try_acquire(&path).is_some() {
                reacquired = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(reacquired, "released lock re-acquirable");
        let _ = std::fs::remove_file(&path);
    }
}
