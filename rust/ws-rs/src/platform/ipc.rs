//! Windows named-pipe IPC seam for the command socket. The macOS/Linux body in
//! `app/socket.rs` talks over `std::os::unix` sockets directly; on Windows the
//! same path string maps to a named pipe via `interprocess`. The path→pipe-name
//! mapping is a pure function so it can be unit-tested on every platform.

#[cfg(windows)]
use std::io;

/// Map the Unix-domain socket path the macOS/Linux daemon uses (e.g.
/// `$TMPDIR/ws-notes.sock`) to a Windows named-pipe name in the `\\.\pipe\`
/// namespace. Named pipes are a flat, machine-wide namespace with no path
/// separators, so the full path is hashed (FNV-1a 64) into a stable, unique
/// name. The daemon and every client call this with the same path string, so
/// they always agree on the pipe.
///
/// `cfg`-free (pure) so the tests below run on macOS/Linux too.
pub fn pipe_name(path: &str) -> String {
    format!("\\\\.\\pipe\\ws-{:016x}", fnv1a64(path.as_bytes()))
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[cfg(windows)]
pub use interprocess::local_socket::{Listener, Stream};

#[cfg(windows)]
use interprocess::local_socket::traits::Stream as _;

#[cfg(windows)]
use interprocess::local_socket::{ConnectOptions, GenericFilePath, ListenerOptions, ToFsName};

#[cfg(windows)]
use std::time::Duration;

/// Bind a named-pipe listener for the mapped path.
#[cfg(windows)]
pub fn bind(path: &str) -> io::Result<Listener> {
    let pipe = pipe_name(path);
    let name = pipe.to_fs_name::<GenericFilePath>()?;
    ListenerOptions::new().name(name).create_sync()
}

/// Connect a named-pipe stream for the mapped path.
#[cfg(windows)]
pub fn connect(path: &str) -> io::Result<Stream> {
    let pipe = pipe_name(path);
    let name = pipe.to_fs_name::<GenericFilePath>()?;
    ConnectOptions::new().name(name).connect_sync()
}

/// Connect and set the read/write timeouts (the client counterpart of the Unix
/// `SO_RCVTIMEO`/`SO_SNDTIMEO`). `None` leaves a direction blocking.
#[cfg(windows)]
pub fn connect_with_timeouts(
    path: &str,
    read: Option<Duration>,
    write: Option<Duration>,
) -> io::Result<Stream> {
    let stream = connect(path)?;
    let _ = stream.set_recv_timeout(read);
    let _ = stream.set_send_timeout(write);
    Ok(stream)
}

/// Apply the same send/recv timeouts the Unix body sets on accepted sockets
/// (`SO_SNDTIMEO` 5 s, `SO_RCVTIMEO` 2 s). Named-pipe streams block by default.
#[cfg(windows)]
pub fn configure(stream: &Stream) {
    let _ = stream.set_send_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_recv_timeout(Some(Duration::from_secs(2)));
}

#[cfg(test)]
mod tests {
    use super::pipe_name;

    #[test]
    fn pipe_name_is_stable_and_unique() {
        let a = pipe_name("/tmp/ws-notes.sock");
        assert_eq!(a, pipe_name("/tmp/ws-notes.sock"), "same path -> same pipe");
        assert_ne!(
            a,
            pipe_name("/var/folders/x/ws-notes.sock"),
            "different path -> different pipe"
        );
        assert!(a.starts_with("\\\\.\\pipe\\ws-"), "pipe-namespace prefix: {a}");
        assert_eq!(a.len(), "\\\\.\\pipe\\ws-".len() + 16, "16 hex hash chars: {a}");
    }

    #[test]
    fn pipe_name_is_always_a_valid_pipe_name() {
        // Slashes/dots in the path must never leak into the pipe name: the
        // hash portion is the only variable part.
        let p = pipe_name("/some/dir.with.dots/ws-notes.sock");
        let tail: String = p.chars().skip("\\\\.\\pipe\\ws-".len()).collect();
        assert_eq!(tail.len(), 16);
        assert!(tail.chars().all(|c| c.is_ascii_hexdigit()), "hex only: {tail}");
    }
}
