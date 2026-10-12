//! The system clipboard, plain text only (was direct `NSPasteboard` calls).
//!
//! macOS keeps the app's existing `NSPasteboard` implementation verbatim;
//! non-macOS targets use `arboard` (text-only build). No main-thread
//! requirement: like the calls they replace, these may run on any thread
//! (sites that deliberately no-op off the main thread keep their own guard).

/// The general pasteboard's plain string, if it holds one
/// (`NSPasteboardTypeString`).
#[cfg(target_os = "macos")]
pub fn get_text() -> Option<String> {
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
    NSPasteboard::generalPasteboard()
        .stringForType(unsafe { NSPasteboardTypeString })
        .map(|s| s.to_string())
}

/// Replace the general pasteboard with plain `text` (clearing it first).
/// Returns whether the write was accepted.
#[cfg(target_os = "macos")]
pub fn set_text(text: &str) -> bool {
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
    use objc2_foundation::NSString;
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    unsafe { pb.setString_forType(&NSString::from_str(text), NSPasteboardTypeString) }
}

/// The system clipboard's text, if it holds any.
#[cfg(not(target_os = "macos"))]
pub fn get_text() -> Option<String> {
    arboard::Clipboard::new().ok()?.get_text().ok()
}

/// Replace the system clipboard with plain `text`.
/// Returns whether the write was accepted.
#[cfg(not(target_os = "macos"))]
pub fn set_text(text: &str) -> bool {
    arboard::Clipboard::new()
        .and_then(|mut c| c.set_text(text.to_owned()))
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs a live clipboard session (GUI login). Run with
    /// `cargo test -p ws-rs clipboard -- --ignored`.
    #[test]
    #[ignore = "needs a live clipboard session"]
    fn clipboard_text_round_trip() {
        let text = "kitchen-sink platform::clipboard round-trip";
        assert!(set_text(text));
        assert_eq!(get_text().as_deref(), Some(text));
    }
}
