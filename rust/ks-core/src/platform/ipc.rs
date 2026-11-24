//! Windows named-pipe IPC seam for the command socket. The macOS/Linux body in
//! `app/socket.rs` talks over `std::os::unix` sockets directly; on Windows the
//! same path string maps to a named pipe via `interprocess`. The path→pipe-name
//! mapping is a pure function so it can be unit-tested on every platform.

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
