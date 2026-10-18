//! Shared AppKit view scaffolding — the pieces every `define_class!` view in
//! `views/` and `ui/` used to copy-paste: the `&AnyObject` up-cast, tracking
//! areas, the hover-cell redraw dance, and the event sink slot.
//!
//! Unlike the old private copies these are plain free functions, so they work
//! with any ivars type and add no Objective-C classes.

use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AllocAnyThread, Message};
use objc2_app_kit::{NSEvent, NSView, NSTrackingArea, NSTrackingAreaOptions};
use objc2_foundation::NSPoint;

/// Up-cast any message-receiving reference to `&AnyObject` (the `owner:` of a
/// tracking area, the receiver of a `performSelector:`, …).
pub fn as_any<T: Message + ?Sized>(obj: &T) -> &AnyObject {
    unsafe { &*(obj as *const T as *const AnyObject) }
}

/// Event point in `view`'s own coordinates — the one-liner pasted ~70×.
pub fn mouse_point(view: &NSView, e: &NSEvent) -> NSPoint {
    view.convertPoint_fromView(e.locationInWindow(), None)
}

/// The mask almost every hover view uses (`MouseMoved | MouseEnteredAndExited
/// | ActiveAlways | InVisibleRect`).
pub fn hover_opts() -> NSTrackingAreaOptions {
    NSTrackingAreaOptions::MouseMoved
        | NSTrackingAreaOptions::MouseEnteredAndExited
        | NSTrackingAreaOptions::ActiveAlways
        | NSTrackingAreaOptions::InVisibleRect
}

/// Drop the tracking area stored in `slot` and install a fresh one covering
/// `view`'s current bounds with `opts` (owner = `view`), storing it back.
/// The body of `updateTrackingAreas` is the super call plus this.
pub fn reset_tracking(
    view: &NSView,
    slot: &RefCell<Option<Retained<NSTrackingArea>>>,
    opts: NSTrackingAreaOptions,
) {
    if let Some(old) = slot.borrow_mut().take() {
        view.removeTrackingArea(&old);
    }
    let t = unsafe {
        NSTrackingArea::initWithRect_options_owner_userInfo(
            NSTrackingArea::alloc(),
            view.bounds(),
            opts,
            Some(as_any(view)),
            None,
        )
    };
    view.addTrackingArea(&t);
    *slot.borrow_mut() = Some(t);
}

/// Write `new` into a hover cell and redraw only when it actually changed —
/// the `mouseMoved:` / `mouseExited:` body, once.
pub fn set_hover<T: Copy + PartialEq>(view: &NSView, cell: &Cell<T>, new: T) {
    if cell.get() != new {
        cell.set(new);
        view.setNeedsDisplay(true);
    }
}
