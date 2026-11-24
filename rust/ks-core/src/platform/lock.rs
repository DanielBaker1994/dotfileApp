//! Windows daemon-lock seam. The macOS/Linux body in `app/socket.rs` holds an
//! exclusive `flock` on `<socket>.lock`; on Windows the same lock file is held
//! with `LockFileEx` via `fs4`.

