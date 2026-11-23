//! `QuietWebView` (`CardWindow.swift`): a `WKWebView` whose context menu
//! keeps only Copy, Copy Link, Open Link and Copy Image (else Select All),
//! for the AI and Confluence previews.

use objc2::rc::Retained;
use objc2::{define_class, msg_send, sel, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSEvent, NSMenu, NSMenuItem, NSUserInterfaceItemIdentification};
use objc2_foundation::{NSRect, NSString};
use objc2_web_kit::{WKWebView, WKWebViewConfiguration};

const KEEP: &[&str] = &[
    "WKMenuItemIdentifierCopy",
    "WKMenuItemIdentifierCopyLink",
    "WKMenuItemIdentifierOpenLink",
    "WKMenuItemIdentifierCopyImage",
];

define_class!(
    #[unsafe(super(WKWebView))]
    #[thread_kind = MainThreadOnly]
    #[name = "WSQuietWebView"]
    pub struct QuietWebView;

    impl QuietWebView {
        #[unsafe(method(willOpenMenu:withEvent:))]
        fn will_open_menu(&self, menu: &NSMenu, event: &NSEvent) {
            for item in menu.itemArray().iter() {
                let id = item.identifier().map(|i| i.to_string()).unwrap_or_default();
                if !KEEP.contains(&id.as_str()) {
                    menu.removeItem(&item);
                }
            }
            if menu.numberOfItems() == 0 {
                let mtm = MainThreadMarker::from(self);
                let all = unsafe {
                    NSMenuItem::initWithTitle_action_keyEquivalent(
                        NSMenuItem::alloc(mtm),
                        &NSString::from_str("Select All"),
                        Some(sel!(selectAll:)),
                        &NSString::from_str(""),
                    )
                };
                menu.addItem(&all);
            }
            let _: () = unsafe { msg_send![super(self), willOpenMenu: menu, withEvent: event] };
        }
    }
);

/// A quiet preview web view, returned as a plain `WKWebView`.
pub fn new(mtm: MainThreadMarker, frame: NSRect, config: &WKWebViewConfiguration) -> Retained<WKWebView> {
    let this = QuietWebView::alloc(mtm).set_ivars(());
    let v: Retained<QuietWebView> = unsafe { msg_send![super(this), initWithFrame: frame, configuration: config] };
    Retained::into_super(v)
}
