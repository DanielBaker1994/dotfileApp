//! Cross-platform seams. Each function here replaces a direct AppKit call so
//! the rest of the app stays toolkit-neutral; migrate one entry at a time.

/// Open `url` in the user's default browser (was `NSWorkspace.openURL`).
/// Returns whether the OS accepted the request.
pub fn open_url(url: &str) -> bool {
    open::that_detached(url).is_ok()
}
