//! Port of `NvimRPC.swift` — a msgpack-RPC client over a Unix socket to a
//! `nvim --server` instance. The Swift original is synchronous behind one lock;
//! here a background reader thread matches response msgids to callers through
//! channels so several calls can be in flight at once (writes are serialised).

use std::collections::HashMap;
use std::io::{self, Cursor, Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use rmpv::Value;

/// Messaging channel of the msgpack-RPC spec.
const REQUEST: u64 = 0;
const RESPONSE: u64 = 1;
const NOTIFICATION: u64 = 2;

#[derive(Debug)]
pub enum RpcError {
    Io(io::Error),
    Timeout,
    Closed,
    /// The peer answered with a non-nil error object (`[1, id, err, result]`).
    Rpc(String),
    Decode(String),
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RpcError::Io(e) => write!(f, "io: {e}"),
            RpcError::Timeout => write!(f, "timeout"),
            RpcError::Closed => write!(f, "connection closed"),
            RpcError::Rpc(m) => write!(f, "rpc error: {m}"),
            RpcError::Decode(m) => write!(f, "decode: {m}"),
        }
    }
}

impl std::error::Error for RpcError {}

impl From<io::Error> for RpcError {
    fn from(e: io::Error) -> Self {
        RpcError::Io(e)
    }
}

pub type RpcResult<T> = Result<T, RpcError>;

/// A connected duplex byte stream. `&self` because the reader thread and the
/// write path share one handle; real `UnixStream` supports `Read`/`Write`
/// through shared references.
pub trait Transport: Send + Sync {
    fn write_all(&self, data: &[u8]) -> io::Result<()>;
    fn read(&self, buf: &mut [u8]) -> io::Result<usize>;
    fn shutdown(&self);
}

/// Opens a fresh transport for a socket path. Injectable so tests can supply a
/// scripted in-process peer.
pub trait Connector: Send + Sync {
    fn connect(&self, path: &str) -> io::Result<Arc<dyn Transport>>;
}

pub struct UnixTransport {
    stream: UnixStream,
}

impl UnixTransport {
    pub fn connect(path: &str) -> io::Result<Self> {
        Ok(Self {
            stream: UnixStream::connect(path)?,
        })
    }
}

impl Transport for UnixTransport {
    fn write_all(&self, data: &[u8]) -> io::Result<()> {
        (&self.stream).write_all(data)
    }

    fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        (&self.stream).read(buf)
    }

    fn shutdown(&self) {
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }
}

pub struct UnixConnector;

impl Connector for UnixConnector {
    fn connect(&self, path: &str) -> io::Result<Arc<dyn Transport>> {
        // `NvimRPC.connect` checks fileExists first so a missing socket fails
        // fast instead of waiting on connect().
        if !std::path::Path::new(path).exists() {
            return Err(io::Error::new(io::ErrorKind::NotFound, "socket not found"));
        }
        Ok(Arc::new(UnixTransport::connect(path)?))
    }
}

type Pending = Arc<Mutex<HashMap<u32, Sender<RpcResult<Value>>>>>;
type NotifyFn = Arc<dyn Fn(&str, &[Value]) + Send + Sync>;

struct Client {
    transport: Arc<dyn Transport>,
    pending: Pending,
    next_id: AtomicU32,
    alive: Arc<AtomicBool>,
    write_lock: Mutex<()>,
    reader: Mutex<Option<JoinHandle<()>>>,
}

impl Client {
    fn spawn(transport: Arc<dyn Transport>, notify: Option<NotifyFn>) -> Arc<Client> {
        let client = Arc::new(Client {
            transport: transport.clone(),
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: AtomicU32::new(0),
            alive: Arc::new(AtomicBool::new(true)),
            write_lock: Mutex::new(()),
            reader: Mutex::new(None),
        });
        let pending = client.pending.clone();
        let alive = client.alive.clone();
        let handle = thread::spawn(move || reader_loop(transport, pending, alive, notify));
        *client.reader.lock().unwrap() = Some(handle);
        client
    }

    fn next_msgid(&self) -> u32 {
        self.next_id.fetch_add(1, Ordering::SeqCst).wrapping_add(1)
    }

    fn shutdown(&self) {
        self.alive.store(false, Ordering::SeqCst);
        self.transport.shutdown();
        let handle = self.reader.lock().unwrap().take();
        if let Some(h) = handle {
            let _ = h.join();
        }
    }
}

fn reader_loop(
    transport: Arc<dyn Transport>,
    pending: Pending,
    alive: Arc<AtomicBool>,
    notify: Option<NotifyFn>,
) {
    let mut buf = vec![0u8; 65536];
    let mut acc: Vec<u8> = Vec::new();
    'outer: loop {
        match transport.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => acc.extend_from_slice(&buf[..n]),
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
        loop {
            match decode_one(&acc) {
                Ok(Some((value, used))) => {
                    acc.drain(..used);
                    dispatch(&value, &pending, &notify);
                }
                Ok(None) => break,
                Err(_) => break 'outer,
            }
        }
    }
    alive.store(false, Ordering::SeqCst);
    let senders: Vec<Sender<RpcResult<Value>>> = {
        let mut map = pending.lock().unwrap();
        map.drain().map(|(_, tx)| tx).collect()
    };
    for tx in senders {
        let _ = tx.send(Err(RpcError::Closed));
    }
}

fn dispatch(value: &Value, pending: &Pending, notify: &Option<NotifyFn>) {
    let Value::Array(a) = value else { return };
    let Some(kind) = a.first().and_then(Value::as_u64) else {
        return;
    };
    match kind {
        RESPONSE if a.len() >= 4 => {
            let Some(id) = a[1].as_u64() else { return };
            let tx = pending.lock().unwrap().remove(&(id as u32));
            if let Some(tx) = tx {
                let result = if matches!(a[2], Value::Nil) {
                    Ok(a[3].clone())
                } else {
                    Err(RpcError::Rpc(text(&a[2])))
                };
                let _ = tx.send(result);
            }
        }
        NOTIFICATION => {
            let (Some(nf), Some(method), Some(Value::Array(params))) =
                (notify.as_ref(), a.get(1).and_then(Value::as_str), a.get(2))
            else {
                return;
            };
            nf(method, params);
        }
        _ => {}
    }
}

/// Decode one value from the front of `buf`; `Ok(None)` = incomplete frame.
fn decode_one(buf: &[u8]) -> Result<Option<(Value, usize)>, rmpv::decode::Error> {
    let mut cursor = Cursor::new(buf);
    match rmpv::decode::read_value(&mut cursor) {
        Ok(v) => Ok(Some((v, cursor.position() as usize))),
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Ok(None),
        Err(e) => Err(e),
    }
}

fn text(v: &Value) -> String {
    match v {
        Value::Nil => "vim.NIL".to_string(),
        Value::Boolean(b) => if *b { "true" } else { "false" }.to_string(),
        Value::Integer(n) => n.to_string(),
        Value::F32(f) => format_double(*f as f64),
        Value::F64(f) => format_double(*f),
        Value::String(s) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
        Value::Binary(b) => String::from_utf8_lossy(b).into_owned(),
        Value::Array(_) | Value::Map(_) => "table".to_string(),
        Value::Ext(_, _) => "userdata".to_string(),
    }
}

fn format_double(d: f64) -> String {
    if d.fract() == 0.0 && d.abs() < 1e15 {
        format!("{}", d as i64)
    } else {
        format!("{d}")
    }
}

fn frame_request(id: u32, method: &str, params: Vec<Value>) -> Value {
    Value::Array(vec![
        Value::from(REQUEST),
        Value::from(id as u64),
        Value::from(method),
        Value::Array(params),
    ])
}

/// Single-quote a Vim string literal (`PopupWindow.vimString`).
pub fn vim_string(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Double-quote a Vim string literal (`PopupWindow.vimDQ`).
pub fn vim_dq(s: &str) -> String {
    format!(
        "\"{}\"",
        s.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
    )
}

pub struct NvimRpc {
    path: String,
    connector: Arc<dyn Connector>,
    client: Mutex<Option<Arc<Client>>>,
    connect_lock: Mutex<()>,
    notify: Mutex<Option<NotifyFn>>,
}

impl NvimRpc {
    pub fn new(path: impl Into<String>) -> Self {
        Self::with_connector(path, Arc::new(UnixConnector))
    }

    pub fn with_connector(path: impl Into<String>, connector: Arc<dyn Connector>) -> Self {
        NvimRpc {
            path: path.into(),
            connector,
            client: Mutex::new(None),
            connect_lock: Mutex::new(()),
            notify: Mutex::new(None),
        }
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    /// Install the notification handler used by connections opened afterwards.
    pub fn set_notification_handler<F>(&self, handler: F)
    where
        F: Fn(&str, &[Value]) + Send + Sync + 'static,
    {
        *self.notify.lock().unwrap() = Some(Arc::new(handler));
    }

    fn connection(&self) -> RpcResult<Arc<Client>> {
        let _connect = self.connect_lock.lock().unwrap();
        let existing = self.client.lock().unwrap().clone();
        if let Some(c) = existing {
            if c.alive.load(Ordering::SeqCst) {
                return Ok(c);
            }
            if let Some(cur) = self.client.lock().unwrap().take() {
                cur.shutdown();
            }
        }
        let transport = self.connector.connect(&self.path)?;
        let notify = self.notify.lock().unwrap().clone();
        let client = Client::spawn(transport, notify);
        *self.client.lock().unwrap() = Some(client.clone());
        Ok(client)
    }

    fn drop_client(&self, conn: &Arc<Client>) {
        let _connect = self.connect_lock.lock().unwrap();
        let taken = {
            let mut guard = self.client.lock().unwrap();
            match guard.as_ref() {
                Some(cur) if Arc::ptr_eq(cur, conn) => guard.take(),
                _ => None,
            }
        };
        if let Some(c) = taken {
            c.shutdown();
        }
    }

    /// Send a request and wait for its matching response.
    ///
    /// Mirrors `NvimRPC.call`: a write failure or a closed connection retries
    /// once on a fresh connection; a timeout tears down and gives up (a peer
    /// error does neither).
    pub fn call(&self, method: &str, params: Vec<Value>, timeout: Duration) -> RpcResult<Value> {
        let mut last: Option<RpcError> = None;
        for _ in 0..2 {
            let conn = match self.connection() {
                Ok(c) => c,
                Err(e) => return Err(e),
            };
            let id = conn.next_msgid();
            let (tx, rx) = mpsc::channel();
            conn.pending.lock().unwrap().insert(id, tx);
            let frame = frame_request(id, method, params.clone());
            let mut bytes = Vec::new();
            if let Err(e) = rmpv::encode::write_value(&mut bytes, &frame) {
                conn.pending.lock().unwrap().remove(&id);
                return Err(RpcError::Decode(e.to_string()));
            }
            {
                let _w = conn.write_lock.lock().unwrap();
                if let Err(e) = conn.transport.write_all(&bytes) {
                    drop(_w);
                    conn.pending.lock().unwrap().remove(&id);
                    self.drop_client(&conn);
                    last = Some(RpcError::Io(e));
                    continue;
                }
            }
            match rx.recv_timeout(timeout) {
                Ok(Ok(v)) => return Ok(v),
                Ok(Err(e)) => {
                    conn.pending.lock().unwrap().remove(&id);
                    if matches!(e, RpcError::Closed) {
                        self.drop_client(&conn);
                        last = Some(e);
                        continue;
                    }
                    return Err(e);
                }
                Err(RecvTimeoutError::Timeout) => {
                    conn.pending.lock().unwrap().remove(&id);
                    self.drop_client(&conn);
                    return Err(RpcError::Timeout);
                }
                Err(RecvTimeoutError::Disconnected) => {
                    self.drop_client(&conn);
                    last = Some(RpcError::Closed);
                    continue;
                }
            }
        }
        Err(last.unwrap_or(RpcError::Closed))
    }

    /// `nvim_eval`, mapped through `text` (the Swift `eval`).
    pub fn vim_eval(&self, expr: &str, timeout: Duration) -> RpcResult<String> {
        self.vim_expr(expr, timeout).map(|v| text(&v))
    }

    /// `nvim_eval`, raw value.
    pub fn vim_expr(&self, expr: &str, timeout: Duration) -> RpcResult<Value> {
        self.call("nvim_eval", vec![Value::from(expr)], timeout)
    }

    /// `nvim_command`.
    pub fn vim_command(&self, command: &str, timeout: Duration) -> RpcResult<()> {
        self.call("nvim_command", vec![Value::from(command)], timeout)
            .map(|_| ())
    }

    /// `nvim_call_function`.
    pub fn nvim_call_function(
        &self,
        name: &str,
        args: Vec<Value>,
        timeout: Duration,
    ) -> RpcResult<Value> {
        self.call(
            "nvim_call_function",
            vec![Value::from(name), Value::Array(args)],
            timeout,
        )
    }

    /// `nvim_get_keymap`.
    pub fn nvim_get_keymap(&self, mode: &str, timeout: Duration) -> RpcResult<Value> {
        self.call("nvim_get_keymap", vec![Value::from(mode)], timeout)
    }

    /// `luaeval` with no argument (`PopupWindow.vimVoiceBegin`-style).
    pub fn luaeval(&self, lua: &str, timeout: Duration) -> RpcResult<String> {
        self.vim_eval(&format!("luaeval({})", vim_string(lua)), timeout)
    }

    /// `luaeval` with a `_A` argument, double-quoted (`vimVoiceUpdate`).
    pub fn luaeval_with_arg(
        &self,
        lua: &str,
        arg: &str,
        timeout: Duration,
    ) -> RpcResult<String> {
        self.vim_eval(
            &format!("luaeval({}, {})", vim_string(lua), vim_dq(arg)),
            timeout,
        )
    }

    /// `nvim_input` (`NvimRPC.input`).
    pub fn send_keys(&self, keys: &str, timeout: Duration) -> RpcResult<()> {
        self.call("nvim_input", vec![Value::from(keys)], timeout)
            .map(|_| ())
    }

    pub fn disconnect(&self) {
        let taken = self.client.lock().unwrap().take();
        if let Some(c) = taken {
            c.shutdown();
        }
    }
}

impl Drop for NvimRpc {
    fn drop(&mut self) {
        let taken = self.client.lock().unwrap().take();
        if let Some(c) = taken {
            c.shutdown();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Condvar;

    // ---- scripted in-process peer -----------------------------------------

    struct FakeState {
        inbound: VecDeque<u8>,
        outbound: Vec<u8>,
        closed: bool,
    }

    struct FakeServer {
        state: Mutex<FakeState>,
        cond: Condvar,
        responder: Mutex<Option<Box<dyn Fn(&[u8], &FakeHandle) + Send>>>,
    }

    #[derive(Clone)]
    struct FakeHandle(Arc<FakeServer>);

    impl FakeHandle {
        fn push(&self, bytes: Vec<u8>) {
            let mut st = self.0.state.lock().unwrap();
            st.inbound.extend(bytes);
            self.0.cond.notify_all();
        }
        fn close(&self) {
            let mut st = self.0.state.lock().unwrap();
            st.closed = true;
            self.0.cond.notify_all();
        }
        fn outbound(&self) -> Vec<u8> {
            self.0.state.lock().unwrap().outbound.clone()
        }
        fn is_closed(&self) -> bool {
            self.0.state.lock().unwrap().closed
        }
    }

    struct FakeTransport(Arc<FakeServer>);

    impl Transport for FakeTransport {
        fn write_all(&self, data: &[u8]) -> io::Result<()> {
            {
                let mut st = self.0.state.lock().unwrap();
                if st.closed {
                    return Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed"));
                }
                st.outbound.extend_from_slice(data);
            }
            let handle = FakeHandle(self.0.clone());
            let responder = self.0.responder.lock().unwrap();
            if let Some(f) = responder.as_ref() {
                f(data, &handle);
            }
            Ok(())
        }

        fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
            let mut st = self.0.state.lock().unwrap();
            loop {
                if !st.inbound.is_empty() {
                    let n = buf.len().min(st.inbound.len());
                    for slot in buf.iter_mut().take(n) {
                        *slot = st.inbound.pop_front().unwrap();
                    }
                    return Ok(n);
                }
                if st.closed {
                    return Ok(0);
                }
                st = self.0.cond.wait(st).unwrap();
            }
        }

        fn shutdown(&self) {
            FakeHandle(self.0.clone()).close();
        }
    }

    struct FakeConnector {
        servers: Mutex<Vec<FakeHandle>>,
        responders: Mutex<Vec<Option<Box<dyn Fn(&[u8], &FakeHandle) + Send>>>>,
        fail: AtomicBool,
    }

    impl FakeConnector {
        fn new() -> Arc<Self> {
            Arc::new(FakeConnector {
                servers: Mutex::new(Vec::new()),
                responders: Mutex::new(Vec::new()),
                fail: AtomicBool::new(false),
            })
        }

        /// Queue a responder consumed by the next connect.
        fn on_connect(self: &Arc<Self>, f: Box<dyn Fn(&[u8], &FakeHandle) + Send>) {
            self.responders.lock().unwrap().push(Some(f));
        }

        fn server(&self, i: usize) -> FakeHandle {
            self.servers.lock().unwrap()[i].clone()
        }

        fn count(&self) -> usize {
            self.servers.lock().unwrap().len()
        }
    }

    impl Connector for FakeConnector {
        fn connect(&self, _path: &str) -> io::Result<Arc<dyn Transport>> {
            if self.fail.load(Ordering::SeqCst) {
                return Err(io::Error::new(io::ErrorKind::NotFound, "no socket"));
            }
            let mut responders = self.responders.lock().unwrap();
            let responder = if responders.is_empty() {
                None
            } else if responders.len() == 1 {
                responders[0].take()
            } else {
                responders.remove(0)
            };
            drop(responders);
            let server = Arc::new(FakeServer {
                state: Mutex::new(FakeState {
                    inbound: VecDeque::new(),
                    outbound: Vec::new(),
                    closed: false,
                }),
                cond: Condvar::new(),
                responder: Mutex::new(responder),
            });
            self.servers.lock().unwrap().push(FakeHandle(server.clone()));
            Ok(Arc::new(FakeTransport(server)))
        }
    }

    // ---- frame helpers -----------------------------------------------------

    fn encode(v: &Value) -> Vec<u8> {
        let mut out = Vec::new();
        rmpv::encode::write_value(&mut out, v).unwrap();
        out
    }

    fn parse_request(bytes: &[u8]) -> (u32, String, Vec<Value>) {
        let (v, _) = decode_one(bytes).unwrap().unwrap();
        let Value::Array(a) = v else { panic!("not an array") };
        assert_eq!(a[0].as_u64(), Some(REQUEST));
        let id = a[1].as_u64().unwrap() as u32;
        let method = a[2].as_str().unwrap().to_string();
        let Value::Array(params) = a[3].clone() else {
            panic!("params not an array")
        };
        (id, method, params)
    }

    fn response(id: u32, err: Value, result: Value) -> Vec<u8> {
        encode(&Value::Array(vec![
            Value::from(RESPONSE),
            Value::from(id as u64),
            err,
            result,
        ]))
    }

    fn notification(method: &str, params: Vec<Value>) -> Vec<u8> {
        encode(&Value::Array(vec![
            Value::from(NOTIFICATION),
            Value::from(method),
            Value::Array(params),
        ]))
    }

    const T: Duration = Duration::from_millis(500);

    // ---- codec / text ------------------------------------------------------

    #[test]
    fn frame_round_trip() {
        let req = frame_request(7, "nvim_eval", vec![Value::from("mode()")]);
        let bytes = encode(&req);
        let (id, method, params) = parse_request(&bytes);
        assert_eq!(id, 7);
        assert_eq!(method, "nvim_eval");
        assert_eq!(params[0].as_str(), Some("mode()"));

        let (back, used) = decode_one(&bytes).unwrap().unwrap();
        assert_eq!(used, bytes.len());
        assert_eq!(back, req);
    }

    #[test]
    fn incomplete_then_complete() {
        let bytes = encode(&Value::Array(vec![Value::from(1u64), Value::from(2u64)]));
        let split = bytes.len() - 3;
        assert!(decode_one(&bytes[..split]).unwrap().is_none());
        let (v, used) = decode_one(&bytes).unwrap().unwrap();
        assert_eq!(used, bytes.len());
        assert_eq!(v.as_array().unwrap().len(), 2);
    }

    #[test]
    fn text_matches_swift() {
        assert_eq!(text(&Value::Nil), "vim.NIL");
        assert_eq!(text(&Value::from(true)), "true");
        assert_eq!(text(&Value::from(false)), "false");
        assert_eq!(text(&Value::from(42i64)), "42");
        assert_eq!(text(&Value::from(-7i64)), "-7");
        assert_eq!(text(&Value::from(1.5f64)), "1.5");
        assert_eq!(text(&Value::from(3.0f64)), "3");
        assert_eq!(text(&Value::from("héllo ✓")), "héllo ✓");
        assert_eq!(
            text(&Value::Array(vec![Value::from(1)])),
            "table"
        );
        assert_eq!(
            text(&Value::Map(vec![(Value::from("a"), Value::from(1))])),
            "table"
        );
        assert_eq!(text(&Value::Ext(1, vec![0, 1])), "userdata");
        assert_eq!(text(&Value::Binary(vec![b'h', b'i'])), "hi");
    }

    #[test]
    fn vim_literals() {
        assert_eq!(vim_string("it's"), "'it''s'");
        assert_eq!(vim_dq("a\"b\\c\nd"), "\"a\\\"b\\\\c\\nd\"");
    }

    // ---- request/response, msgid matching, notifications -------------------

    #[test]
    fn auto_response_and_msgid_matching() {
        let conn = FakeConnector::new();
        conn.on_connect(Box::new(|bytes, h| {
            let (id, _m, _p) = parse_request(bytes);
            h.push(response(id, Value::Nil, Value::from("n")));
        }));
        let rpc = NvimRpc::with_connector("/fake", conn.clone());

        assert_eq!(rpc.vim_eval("mode()", T).unwrap(), "n");
        assert_eq!(rpc.vim_eval("1+1", T).unwrap(), "n");
        // two requests on ONE persistent connection, ids increment
        let out = conn.server(0).outbound();
        let mut cur = Cursor::new(&out[..]);
        let mut ids = Vec::new();
        while (cur.position() as usize) < out.len() {
            let v = rmpv::decode::read_value(&mut cur).unwrap();
            ids.push(v.as_array().unwrap()[1].as_u64().unwrap());
        }
        assert_eq!(ids, vec![1, 2]);
        assert_eq!(conn.count(), 1);
    }

    #[test]
    fn notification_and_stray_response_ignored() {
        let conn = FakeConnector::new();
        conn.on_connect(Box::new(|bytes, h| {
            let (id, _m, _p) = parse_request(bytes);
            h.push(notification("event", vec![Value::from("tick")]));
            // a response for an unknown msgid must not fool the caller
            h.push(response(9999, Value::Nil, Value::from("wrong")));
            h.push(response(id, Value::Nil, Value::from("right")));
        }));
        let rpc = NvimRpc::with_connector("/fake", conn.clone());
        let seen: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        rpc.set_notification_handler(move |method, params| {
            sink.lock()
                .unwrap()
                .push((method.to_string(), text(&params[0])));
        });

        assert_eq!(rpc.vim_eval("x", T).unwrap(), "right");
        assert_eq!(*seen.lock().unwrap(), vec![("event".into(), "tick".into())]);
    }

    #[test]
    fn rpc_error_is_returned_and_connection_survives() {
        let conn = FakeConnector::new();
        conn.on_connect(Box::new(|bytes, h| {
            let (id, method, _p) = parse_request(bytes);
            if method == "nvim_eval" && bytes.windows(4).any(|w| w == b"boom") {
                h.push(response(id, Value::from("E121: boom"), Value::Nil));
            } else {
                h.push(response(id, Value::Nil, Value::from(2)));
            }
        }));
        let rpc = NvimRpc::with_connector("/fake", conn.clone());

        assert!(matches!(rpc.vim_eval("boom", T), Err(RpcError::Rpc(_))));
        assert_eq!(rpc.vim_expr("1+1", T).unwrap().as_u64(), Some(2));
        assert_eq!(conn.count(), 1, "an error must not drop the connection");
    }

    // ---- concurrency -------------------------------------------------------

    #[test]
    fn concurrent_calls_are_matched() {
        let conn = FakeConnector::new();
        conn.on_connect(Box::new(|bytes, h| {
            let (id, _method, params) = parse_request(bytes);
            h.push(response(id, Value::Nil, params[0].clone()));
        }));
        let rpc = Arc::new(NvimRpc::with_connector("/fake", conn.clone()));

        let mut handles = Vec::new();
        for i in 0..8 {
            let rpc = rpc.clone();
            handles.push(thread::spawn(move || {
                let method = format!("m{i}");
                let v = rpc.vim_expr(&method, T).unwrap();
                assert_eq!(v.as_str(), Some(method.as_str()));
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(conn.count(), 1, "one shared connection");
    }

    // ---- timeout / restart / connect failure -------------------------------

    #[test]
    fn timeout_then_reconnect_gets_a_fresh_connection() {
        let conn = FakeConnector::new();
        conn.on_connect(Box::new(|_bytes, _h| {})); // never answers
        conn.on_connect(Box::new(|bytes, h| {
            let (id, _m, _p) = parse_request(bytes);
            h.push(response(id, Value::Nil, Value::from("fresh")));
        }));
        let rpc = NvimRpc::with_connector("/fake", conn.clone());

        let start = std::time::Instant::now();
        assert!(matches!(
            rpc.vim_eval("slow", Duration::from_millis(80)),
            Err(RpcError::Timeout)
        ));
        assert!(start.elapsed() < Duration::from_secs(2));
        assert_eq!(rpc.vim_eval("again", T).unwrap(), "fresh");
        assert_eq!(conn.count(), 2);
        assert!(conn.server(0).is_closed(), "timed-out connection is torn down");
    }

    #[test]
    fn closed_connection_retries_once() {
        let conn = FakeConnector::new();
        conn.on_connect(Box::new(|_bytes, h| h.close()));
        conn.on_connect(Box::new(|bytes, h| {
            let (id, _m, _p) = parse_request(bytes);
            h.push(response(id, Value::Nil, Value::from("3")));
        }));
        let rpc = NvimRpc::with_connector("/fake", conn.clone());

        assert_eq!(rpc.vim_eval("1+2", T).unwrap(), "3");
        assert_eq!(conn.count(), 2);
    }

    #[test]
    fn connect_failure_is_an_io_error() {
        let conn = FakeConnector::new();
        conn.fail.store(true, Ordering::SeqCst);
        let rpc = NvimRpc::with_connector("/fake", conn.clone());
        assert!(matches!(rpc.vim_eval("1", T), Err(RpcError::Io(_))));
    }

    #[test]
    fn missing_socket_fails_fast() {
        let rpc = NvimRpc::new("/nonexistent/ws-rs-nvim.sock");
        let start = std::time::Instant::now();
        assert!(rpc.vim_eval("1", T).is_err());
        assert!(start.elapsed() < Duration::from_millis(200));
    }

    // ---- convenience helpers build the right requests ----------------------

    #[test]
    fn helpers_use_the_right_methods() {
        let conn = FakeConnector::new();
        let methods = Arc::new(Mutex::new(Vec::<String>::new()));
        let seen = methods.clone();
        conn.on_connect(Box::new(move |bytes, h| {
            let (id, method, params) = parse_request(bytes);
            seen.lock().unwrap().push(method.clone());
            let result = match method.as_str() {
                "nvim_eval" => {
                    // luaeval(...) wrapper
                    let expr = params[0].as_str().unwrap();
                    assert!(expr.starts_with("luaeval("), "got {expr}");
                    Value::from("1")
                }
                "nvim_command" => Value::Nil,
                "nvim_input" => Value::from(4),
                "nvim_get_keymap" => {
                    assert_eq!(params[0].as_str(), Some("n"));
                    Value::Array(vec![])
                }
                _ => Value::Nil,
            };
            h.push(response(id, Value::Nil, result));
        }));
        let rpc = NvimRpc::with_connector("/fake", conn.clone());

        assert_eq!(rpc.luaeval("return 1", T).unwrap(), "1");
        assert!(rpc.vim_command("silent! wall", T).is_ok());
        assert!(rpc.send_keys("ihello<Esc>", T).is_ok());
        assert!(rpc.nvim_get_keymap("n", T).unwrap().is_array());
        assert_eq!(
            *methods.lock().unwrap(),
            vec!["nvim_eval", "nvim_command", "nvim_input", "nvim_get_keymap"]
        );
    }

    // ---- live, gated on a real nvim -----------------------------------------

    struct ChildGuard(Option<std::process::Child>);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            if let Some(c) = self.0.as_mut() {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
    }

    fn find_nvim() -> Option<String> {
        use std::os::unix::fs::PermissionsExt;
        let mut candidates = vec![
            "/opt/homebrew/bin/nvim".to_string(),
            "/usr/local/bin/nvim".to_string(),
        ];
        if let Ok(path) = std::env::var("PATH") {
            for dir in path.split(':') {
                if !dir.is_empty() {
                    candidates.push(format!("{dir}/nvim"));
                }
            }
        }
        candidates.into_iter().find(|c| {
            std::fs::metadata(c)
                .map(|m| m.is_file() && (m.permissions().mode() & 0o111) != 0)
                .unwrap_or(false)
        })
    }

    fn start_nvim(nvim: &str, sock: &str) -> Option<ChildGuard> {
        use std::process::{Command, Stdio};
        let child = Command::new(nvim)
            .args(["--headless", "--clean", "--listen", sock])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let guard = ChildGuard(Some(child));
        for _ in 0..300 {
            if std::path::Path::new(sock).exists() {
                return Some(guard);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        None
    }

    #[test]
    fn live_nvim_round_trip() {
        let Some(nvim) = find_nvim() else {
            eprintln!("skipping live nvim test: no nvim executable");
            return;
        };
        let dir = std::env::temp_dir().join(format!("ws-rs-nvimrpc-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let sock = dir.join("n.sock");
        let sock = sock.to_string_lossy().to_string();
        let _ = std::fs::remove_file(&sock);

        let Some(_child) = start_nvim(&nvim, &sock) else {
            eprintln!("skipping live nvim test: server never started");
            return;
        };
        let rpc = NvimRpc::new(&sock);

        assert_eq!(rpc.vim_eval("1+1", T).unwrap(), "2");
        assert_eq!(rpc.luaeval("1 + 1", T).unwrap(), "2");
        assert_eq!(rpc.vim_eval("luaeval('nil')", T).unwrap(), "vim.NIL");
        assert_eq!(rpc.vim_eval("'héllo ✓'", T).unwrap(), "héllo ✓");
        assert_eq!(rpc.vim_eval("{'a':1}", T).unwrap(), "table");

        // an nvim error is an Err, and the connection survives it
        assert!(rpc.vim_eval("nonexistent_fn()", T).is_err());
        assert_eq!(rpc.vim_eval("1+1", T).unwrap(), "2");

        assert!(rpc.vim_eval("repeat('x', 200000)", T).unwrap().len() == 200_000);

        rpc.send_keys("ihello<Esc>", T).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(rpc.vim_eval("getline(1)", T).unwrap(), "hello");
        assert_eq!(rpc.vim_eval("mode()", T).unwrap(), "n");

        // reconnect after the server restarts on the same path
        drop(_child);
        std::thread::sleep(Duration::from_millis(100));
        let _ = std::fs::remove_file(&sock);
        let _child2 = start_nvim(&nvim, &sock);
        assert_eq!(rpc.vim_eval("1+2", T).unwrap(), "3");

        drop(_child2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
