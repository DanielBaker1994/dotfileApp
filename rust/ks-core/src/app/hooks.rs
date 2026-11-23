//! Seams between the shared code and whichever process hosts it.
//!
//! The main daemon (`ws-rs` `host.rs`) and the quick agent both run the same
//! views (screenshot, paths shelf, file popup). A few of their actions end in
//! something only the host knows how to do ("reveal in Files", "add to the live
//! path shelf", "cancel the pending click-to-rename"). Each is a slot here:
//! the host installs its implementation once at startup, and the shared code
//! calls the free function. With nothing installed the defaults are the
//! behaviour of a process without those surfaces.

use std::sync::OnceLock;

type RevealFn = Box<dyn Fn(&[String]) + Send + Sync>;
static REVEAL_HOOK: OnceLock<RevealFn> = OnceLock::new();

/// Where "reveal in Files" goes (the main daemon shows its Files view; the
/// quick agent forwards it to the main daemon's socket). Set once at startup;
/// the first installer wins.
pub fn set_reveal_hook(f: impl Fn(&[String]) + Send + Sync + 'static) {
    let _ = REVEAL_HOOK.set(Box::new(f));
}

/// Show `paths` in the app's own file browser (`SlotView::Files`) — the uniform
/// entry point for every "reveal / show in Finder" action, callable from any
/// module. Never opens macOS Finder. A no-op before a host installed a hook.
pub fn reveal_in_files(paths: &[String]) {
    if let Some(hook) = REVEAL_HOOK.get() {
        hook(paths);
    }
}

type ShelfAddFn = Box<dyn Fn(&[String], crate::engines::path_shelf::Why) -> bool + Send + Sync>;
static SHELF_ADD_HOOK: OnceLock<ShelfAddFn> = OnceLock::new();

/// Install the live path-shelf adder (the main daemon's `PathShelf`); returns
/// `true` from the closure when a live shelf took the paths.
pub fn set_shelf_add_hook(
    f: impl Fn(&[String], crate::engines::path_shelf::Why) -> bool + Send + Sync + 'static,
) {
    let _ = SHELF_ADD_HOOK.set(Box::new(f));
}

/// `PathShelf.add([path], why: .screenshot)` after a successful overlay save.
/// Called on the main thread from `views::screenshot`'s save path.
///
/// With no live shelf in this process (the quick agent) the store file is
/// updated directly — pure Rust, atomic, merged with whatever the other
/// process wrote ([`crate::engines::path_shelf::pure_add`]).
pub fn screenshot_saved_to_path_shelf(path: &str) {
    #[cfg(target_os = "macos")]
    {
        let why = crate::engines::path_shelf::Why::Screenshot;
        let live = SHELF_ADD_HOOK.get().map(|h| h(&[path.to_string()], why)).unwrap_or(false);
        if !live {
            crate::engines::path_shelf::pure_add(
                &crate::engines::path_shelf::default_store(),
                &[path.to_string()],
                why,
            );
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = path;
}

#[cfg(target_os = "macos")]
type VoidFn = Box<dyn Fn() + Send + Sync>;
#[cfg(target_os = "macos")]
static CANCEL_CLICK_RENAME_HOOK: OnceLock<VoidFn> = OnceLock::new();

/// Install the main daemon's "drop the pending click-to-rename" action.
#[cfg(target_os = "macos")]
pub fn set_cancel_click_rename_hook(f: impl Fn() + Send + Sync + 'static) {
    let _ = CANCEL_CLICK_RENAME_HOOK.set(Box::new(f));
}

/// Drop a pending click-to-rename (a row drag started: it was never a click).
/// A no-op in a process without the daemon's Files list rename.
#[cfg(target_os = "macos")]
pub fn cancel_click_rename() {
    if let Some(h) = CANCEL_CLICK_RENAME_HOOK.get() {
        h();
    }
}

/// Swift `copyFiles`: one pasteboard item per path carrying both a
/// `public.file-url` (Finder pastes the file) and a plain string (text
/// fields / terminals paste the path). Returns whether anything was written.
#[cfg(target_os = "macos")]
pub fn copy_files_to_pasteboard(paths: &[String]) -> bool {
    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2_app_kit::{
        NSPasteboard, NSPasteboardItem, NSPasteboardTypeFileURL, NSPasteboardTypeString,
        NSPasteboardWriting,
    };
    use objc2_foundation::{NSString, NSURL};
    if paths.is_empty() {
        return false;
    }
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    let objects: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> = paths
        .iter()
        .map(|p| {
            let item = NSPasteboardItem::new();
            let url = NSURL::fileURLWithPath(&NSString::from_str(p));
            if let Some(abs) = url.absoluteString() {
                let _ = item.setString_forType(&abs, unsafe { NSPasteboardTypeFileURL });
            }
            let _ = item.setString_forType(&NSString::from_str(p), unsafe { NSPasteboardTypeString });
            ProtocolObject::from_retained(item)
        })
        .collect();
    let array = objc2_foundation::NSArray::from_retained_slice(&objects);
    pb.writeObjects(&array)
}

/// Copy files and toast Swift's wording; only toasts when something landed.
/// The quick agent's paths shelf uses it too.
#[cfg(target_os = "macos")]
pub fn copy_files_toast(paths: &[String]) -> bool {
    let ok = copy_files_to_pasteboard(paths);
    if ok {
        crate::ui::toast::show(&format!("Copied {}", copied_what(paths)), "doc.on.doc");
    }
    ok
}

/// Swift `items(n, leaf)`: “name” for one path, "N items" otherwise.
#[cfg(target_os = "macos")]
pub fn copied_what(paths: &[String]) -> String {
    match paths {
        [one] => format!(
            "“{}”",
            std::path::Path::new(one)
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_default()
        ),
        _ => format!("{} items", paths.len()),
    }
}
