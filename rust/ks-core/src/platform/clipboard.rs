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

/// Replace the general pasteboard with a rich `html` flavor (`public.html`),
/// the same content as RTF (`public.rtf`, converted from the HTML on the main
/// thread — what Outlook pastes most reliably) and its plain `text` twin for
/// plain-text targets. Returns whether the HTML and text writes were accepted
/// (RTF is best effort).
#[cfg(target_os = "macos")]
pub fn set_html(html: &str, text: &str) -> bool {
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypeHTML, NSPasteboardTypeRTF, NSPasteboardTypeString};
    use objc2_foundation::NSString;
    let rtf = html_to_rtf(html);
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    unsafe {
        let ok = pb.setString_forType(&NSString::from_str(html), NSPasteboardTypeHTML)
            & pb.setString_forType(&NSString::from_str(text), NSPasteboardTypeString);
        if let Some(rtf) = rtf {
            let _ = pb.setData_forType(Some(&rtf), NSPasteboardTypeRTF);
        }
        ok
    }
}

/// `html` rendered to RTF data through AppKit's HTML importer (main thread
/// only; `None` elsewhere or when the import fails).
#[cfg(target_os = "macos")]
fn html_to_rtf(html: &str) -> Option<objc2::rc::Retained<objc2_foundation::NSData>> {
    use objc2::AnyThread;
    use objc2_app_kit::NSAttributedStringAppKitDocumentFormats;
    use objc2_foundation::{NSAttributedString, NSData, NSDictionary, NSRange};
    objc2::MainThreadMarker::new()?;
    let data = NSData::with_bytes(html.as_bytes());
    let attr = unsafe {
        NSAttributedString::initWithHTML_documentAttributes(NSAttributedString::alloc(), &data, None)
    }?;
    let range = NSRange::new(0, attr.length());
    unsafe { attr.RTFFromRange_documentAttributes(range, &NSDictionary::new()) }
}

/// Replace the general pasteboard with one item carrying the file at `path`
/// (`public.file-url`: Finder / mail / chat paste the file) and `text`
/// (`public.utf8-plain-text`: editors and terminals paste the text).
#[cfg(target_os = "macos")]
pub fn set_file_and_text(path: &str, text: &str) -> bool {
    use objc2::runtime::ProtocolObject;
    use objc2_app_kit::{NSPasteboard, NSPasteboardItem, NSPasteboardTypeFileURL, NSPasteboardTypeString, NSPasteboardWriting};
    use objc2_foundation::{NSArray, NSString, NSURL};
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    let item = NSPasteboardItem::new();
    let url = NSURL::fileURLWithPath(&NSString::from_str(path));
    let mut ok = true;
    if let Some(abs) = url.absoluteString() {
        ok &= item.setString_forType(&abs, unsafe { NSPasteboardTypeFileURL });
    }
    ok &= item.setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString });
    let objects: Vec<objc2::rc::Retained<ProtocolObject<dyn NSPasteboardWriting>>> =
        vec![ProtocolObject::from_retained(item)];
    ok && pb.writeObjects(&NSArray::from_retained_slice(&objects))
}

/// No file flavor off macOS: the text alone.
#[cfg(not(target_os = "macos"))]
pub fn set_file_and_text(_path: &str, text: &str) -> bool {
    set_text(text)
}

/// Rich + plain clipboard write (`arboard`'s HTML flavor with a text twin).
#[cfg(not(target_os = "macos"))]
pub fn set_html(html: &str, text: &str) -> bool {
    arboard::Clipboard::new()
        .and_then(|mut c| c.set_html(html.to_owned(), Some(text.to_owned())))
        .is_ok()
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
