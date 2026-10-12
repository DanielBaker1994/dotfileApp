//! `ScreenToast` / `makeToastPill` (Screenshot.swift, PopupWindow.swift): a
//! borderless, click-through pill near the bottom of the screen — symbol +
//! text on `crust` — that fades out after 1.4 s.

#[cfg(target_os = "macos")]
pub use appkit::show;

/// How long the pill stays up before fading (`animateToastPill` `hold`).
pub const HOLD_SECS: f64 = 1.4;

/// Pill text for a shortcut / verb the port routes but doesn't implement yet,
/// so a gap is visible on screen instead of only in the log.
pub fn not_ported_text(what: &str) -> String {
    format!("{what}: not ported yet")
}

/// SF Symbol for [`not_ported`] pills.
pub const NOT_PORTED_SYMBOL: &str = "hammer";

/// Show the "not ported yet" pill for `what`. Main thread only (like [`show`]).
#[cfg(target_os = "macos")]
pub fn not_ported(what: &str) {
    show(&not_ported_text(what), NOT_PORTED_SYMBOL);
}

#[cfg(target_os = "macos")]
mod appkit {
    use super::HOLD_SECS;
    use crate::ui::style::{self, Weight};
    use crate::ui::theme::{PopupThemeDefaults, PopupTone};
    use objc2::rc::Retained;
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSBackingStoreType, NSColor, NSImageView, NSLineBreakMode, NSPanel, NSTextField, NSView,
        NSWindowCollectionBehavior, NSWindowStyleMask,
    };
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
    use std::cell::RefCell;

    thread_local! {
        static PANEL: RefCell<Option<Retained<NSPanel>>> = const { RefCell::new(None) };
    }

    /// `ScreenToast.show(_:on:symbol:)` on the main screen. Empty text is a
    /// no-op; a new toast replaces the one still showing.
    pub fn show(text: &str, symbol: &str) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        if text.is_empty() {
            return;
        }
        let Some(scr) = crate::platform::main_screen() else { return };
        PANEL.with(|p| {
            if let Some(old) = p.borrow_mut().take() {
                old.orderOut(None);
            }
        });
        let c = PopupThemeDefaults::colors();
        let f = scr.frame;
        let sf = NSRect::new(NSPoint::new(f.x, f.y), NSSize::new(f.w, f.h));

        let label = NSTextField::labelWithString(&NSString::from_str(text), mtm);
        label.setFont(Some(&style::system_font(12.5, Weight::Medium)));
        label.setTextColor(Some(&c.text.to_nscolor()));
        label.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
        label.sizeToFit();
        let icon = style::symbol_image(symbol, 12.5, Weight::Medium, c.tone(PopupTone::Success))
            .map(|img| NSImageView::imageViewWithImage(&img, mtm));

        let (pad_x, gap, h) = (16.0, 8.0, 32.0);
        let icon_size = icon.as_ref().map(|i| i.fittingSize()).unwrap_or(NSSize::new(0.0, 0.0));
        let extra = if icon.is_some() { icon_size.width + gap } else { 0.0 };
        let lf = label.frame().size;
        let w = (lf.width + extra + pad_x * 2.0).ceil().min(sf.size.width - 64.0);
        let label_w = (w - pad_x * 2.0 - extra).max(0.0);

        let pill = NSView::initWithFrame(NSView::alloc(mtm), NSRect::new(NSPoint::new(18.0, 18.0), NSSize::new(w, h)));
        pill.setWantsLayer(true);
        if let Some(layer) = pill.layer() {
            layer.setBackgroundColor(Some(&c.crust().with_alpha(0.94).to_nscolor().CGColor()));
            layer.setBorderColor(Some(&c.text.with_alpha(0.10).to_nscolor().CGColor()));
            layer.setBorderWidth(1.0);
            layer.setCornerRadius(h / 2.0);
        }
        let mut x = (w - (extra + label_w)) / 2.0;
        if let Some(iv) = &icon {
            iv.setFrame(NSRect::new(
                NSPoint::new(x, ((h - icon_size.height) / 2.0).round()),
                icon_size,
            ));
            pill.addSubview(iv);
            x += extra;
        }
        label.setFrame(NSRect::new(
            NSPoint::new(x, ((h - lf.height) / 2.0).round()),
            NSSize::new(label_w, lf.height),
        ));
        pill.addSubview(&label);

        let pad = 18.0;
        let (pw, ph) = (w + pad * 2.0, h + pad * 2.0 + 6.0);
        let rect = NSRect::new(
            NSPoint::new((sf.origin.x + sf.size.width / 2.0 - pw / 2.0).round(), sf.origin.y + 70.0),
            NSSize::new(pw, ph),
        );
        let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
            NSPanel::alloc(mtm),
            rect,
            NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
            NSBackingStoreType::Buffered,
            false,
        );
        // `.screenSaver + 1`
        panel.setLevel(1001);
        panel.setOpaque(false);
        panel.setBackgroundColor(Some(&NSColor::clearColor()));
        panel.setHasShadow(false);
        panel.setIgnoresMouseEvents(true);
        panel.setHidesOnDeactivate(false);
        unsafe { panel.setReleasedWhenClosed(false) };
        panel.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::IgnoresCycle
                | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );
        let root = NSView::initWithFrame(NSView::alloc(mtm), NSRect::new(NSPoint::new(0.0, 0.0), rect.size));
        root.addSubview(&pill);
        panel.setContentView(Some(&root));
        panel.orderFrontRegardless();
        PANEL.with(|p| *p.borrow_mut() = Some(panel.clone()));

        let bound = dispatch2::MainThreadBound::new(panel, mtm);
        dispatch2::DispatchQueue::main()
            .after(
                dispatch2::DispatchTime::NOW.time((HOLD_SECS * 1e9) as i64),
                move || {
                    let Some(mtm) = MainThreadMarker::new() else { return };
                    let panel = bound.get(mtm);
                    panel.orderOut(None);
                    PANEL.with(|p| {
                        let mut slot = p.borrow_mut();
                        if slot.as_ref().is_some_and(|cur| std::ptr::eq(&**cur, &**panel)) {
                            *slot = None;
                        }
                    });
                },
            )
            .ok();
    }
}
