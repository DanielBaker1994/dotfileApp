//! Windows daemon-lock seam. The macOS/Linux body in `app/socket.rs` holds an
//! exclusive `flock` on `<socket>.lock`; on Windows the same lock file is held
//! with `LockFileEx` via `fs4`.

#[cfg(windows)]
use std::path::Path;

#[cfg(windows)]
use fs4::fs_std::FileExt;

/// An exclusive daemon lock on `<socket>.lock`, held until dropped.
#[cfg(windows)]
pub struct DaemonLock {
    file: std::fs::File,
}

#[cfg(windows)]
impl DaemonLock {
    /// `LockFileEx(LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY)`;
    /// `None` if the lock is held elsewhere (or the lock file can't be opened).
    /// Released when the lock is dropped (closing the handle releases the lock).
    pub fn try_acquire<P: AsRef<Path>>(path: P) -> Option<DaemonLock> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(path)
            .ok()?;
        match file.try_lock_exclusive() {
            Ok(true) => Some(DaemonLock { file }),
            _ => None,
        }
    }
}
