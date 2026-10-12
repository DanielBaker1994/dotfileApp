//! Cross-platform seams. Each function here replaces a direct AppKit call so
//! the rest of the app stays toolkit-neutral; migrate one entry at a time.

pub mod app;
pub use app::*;

pub mod clipboard;
pub use clipboard::*;

pub mod screens;
pub use screens::*;

pub mod capture;
#[allow(unused_imports)] // the seam has no caller yet (W6-C lands the API only).
pub use capture::*;

pub mod settings;
pub use settings::*;

pub mod dialog;
pub use dialog::*;

pub mod dispatch;
pub use dispatch::*;

/// Open `url` in the user's default browser (was `NSWorkspace.openURL`).
/// Returns whether the OS accepted the request.
pub fn open_url(url: &str) -> bool {
    open::that_detached(url).is_ok()
}

/// Open `path` with its default application (was `NSWorkspace.openURL` with a
/// `fileURLWithPath` URL). Returns whether the OS accepted the request.
pub fn open_path(path: &std::path::Path) -> bool {
    open::that_detached(path).is_ok()
}

/// Ring the system bell (was `NSBeep`). Best effort; nothing to report.
/// Callable from any thread (same as the old direct `NSBeep` calls).
#[cfg(target_os = "macos")]
pub fn bell() {
    objc2_app_kit::NSBeep();
}

/// Ring the system bell. Non-macOS fallback: a plain BEL byte on stderr.
#[cfg(not(target_os = "macos"))]
pub fn bell() {
    use std::io::Write;
    let _ = std::io::stderr().write_all(b"\x07");
}
