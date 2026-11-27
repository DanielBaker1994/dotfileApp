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

/// Everything on the general pasteboard: per item, every type with its
/// bytes. Lets a copy be taken back ("Undo copy" on the OCR card).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub items: Vec<Vec<(String, Vec<u8>)>>,
    /// `changeCount` right after the snapshot's own copy landed (see
    /// [`restore_if_unchanged`]); 0 = not set.
    pub change_count: isize,
}

/// Read the whole general pasteboard (every item, every type).
#[cfg(target_os = "macos")]
pub fn snapshot() -> Snapshot {
    use objc2_app_kit::NSPasteboard;
    let pb = NSPasteboard::generalPasteboard();
    let mut items = Vec::new();
    if let Some(list) = pb.pasteboardItems() {
        for item in list.iter() {
            let mut kinds = Vec::new();
            for t in item.types().iter() {
                if let Some(d) = item.dataForType(&t) {
                    kinds.push((t.to_string(), d.to_vec()));
                }
            }
            if !kinds.is_empty() {
                items.push(kinds);
            }
        }
    }
    Snapshot { items, change_count: 0 }
}

/// The general pasteboard's change counter (bumps on every write).
#[cfg(target_os = "macos")]
pub fn change_count() -> isize {
    objc2_app_kit::NSPasteboard::generalPasteboard().changeCount()
}

/// Put `snap` back on the general pasteboard. An empty snapshot clears it.
#[cfg(target_os = "macos")]
pub fn restore(snap: &Snapshot) -> bool {
    use objc2::runtime::ProtocolObject;
    use objc2_app_kit::{NSPasteboard, NSPasteboardItem, NSPasteboardWriting};
    use objc2_foundation::{NSArray, NSData, NSString};
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    if snap.items.is_empty() {
        return true;
    }
    let objects: Vec<objc2::rc::Retained<ProtocolObject<dyn NSPasteboardWriting>>> = snap
        .items
        .iter()
        .map(|kinds| {
            let item = NSPasteboardItem::new();
            for (t, bytes) in kinds {
                let _ = item.setData_forType(&NSData::with_bytes(bytes), &NSString::from_str(t));
            }
            ProtocolObject::from_retained(item)
        })
        .collect();
    pb.writeObjects(&NSArray::from_retained_slice(&objects))
}

/// [`restore`], but only while nothing else was copied since our own copy
/// (`snap.change_count`): an undo must never clobber a newer copy.
#[cfg(target_os = "macos")]
pub fn restore_if_unchanged(snap: &Snapshot) -> bool {
    if snap.change_count != 0 && change_count() != snap.change_count {
        return false;
    }
    restore(snap)
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
