//! Read-only queries about the frontmost application (was direct `NSWorkspace`
//! calls). Plain-data API: no `NSRunningApplication` ever crosses the seam.
//!
//! No main-thread requirement: `NSWorkspace` is not main-thread-only in
//! objc2-app-kit, and the replaced call sites already ran on whichever thread
//! reached them (the AppKit-adjacent callers keep their own `MainThreadMarker`
//! guards). Non-macOS targets report "nothing" (`None`).

/// The frontmost application, as plain data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrontApp {
    /// Process id (`NSRunningApplication.processIdentifier`).
    pub pid: i32,
    /// Bundle id (`NSRunningApplication.bundleIdentifier`), if it has one.
    pub bundle_id: Option<String>,
}

/// The application that is frontmost right now
/// (`NSWorkspace.sharedWorkspace().frontmostApplication()`).
///
/// `None` when nothing is frontmost and on non-macOS targets.
#[cfg(target_os = "macos")]
pub fn frontmost_app() -> Option<FrontApp> {
    use objc2_app_kit::NSWorkspace;
    let a = NSWorkspace::sharedWorkspace().frontmostApplication()?;
    Some(FrontApp {
        pid: a.processIdentifier(),
        bundle_id: a.bundleIdentifier().map(|b| b.to_string()),
    })
}

/// The frontmost application. Non-macOS: always `None`.
#[cfg(not(target_os = "macos"))]
pub fn frontmost_app() -> Option<FrontApp> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs a live GUI session (a frontmost app exists once logged in). Run
    /// with `cargo test -p ws-rs platform -- --ignored`.
    #[test]
    #[ignore = "needs a live GUI session"]
    fn frontmost_app_has_a_pid() {
        let app = frontmost_app().expect("a GUI session has a frontmost app");
        assert!(app.pid > 0);
    }
}
