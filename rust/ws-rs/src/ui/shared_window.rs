//! `SharedWindow.swift`, ported.
//!
//! The `SharedWindow` state machine (`current` / `previousNav` / the back
//! `stack` / `escViews`) is a pure, generic model over a [`SharedWindowHost`]
//! (the `SwitcherController` surface), so every transition is unit-testable
//! without AppKit. The real host (`SlotHostWindow`, a `PopupPlainWindow`-style
//! `NSWindow` subclass) is macOS-gated and only built on demand.
//!
//! First cut: state transitions, the pure geometry (`place` /
//! `screen_containing` / `default_frame`), the `test_state` document and the
//! `SlotHostWindow` class (construction, guest tracking, the "empty hides one
//! turn later" behavior, `take` / `give`). Focus-loss observation, the header
//! decoration wiring and sheet handling land in a later cut.

use crate::app::hotkey::{eval_true_args, AeroCall, AeroIpc};
use crate::app::registry::{nav, RectI, SlotMember, SlotView};
use serde_json::{json, Value};
use std::rc::Rc;
use std::time::Instant;

#[cfg(target_os = "macos")]
use objc2::rc::Retained;
#[cfg(target_os = "macos")]
use objc2::MainThreadMarker;
#[cfg(target_os = "macos")]
use std::cell::RefCell;

// ---------------------------------------------------------------------------
// Nav ids + the Esc-close view set (mirrors `SharedWindow.nav*` / `escViews`).
// ---------------------------------------------------------------------------

/// `PopupChrome.workspaceBase` — workspace-switch header buttons start here.
pub const WORKSPACE_BASE: i64 = 1000;

// ---------------------------------------------------------------------------
// Pure geometry (mirrors `SharedWindow.frame` / `screen(of:)` / `place`).
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// The controller surface the state machine drives.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// The state machine.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// The AppKit host window (`SlotHostWindow`, mirrors `SharedWindow.swift`).
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
pub use appkit::SlotHostWindow;

#[cfg(target_os = "macos")]
mod appkit {
    use objc2::rc::{Retained, Weak};
    use objc2::runtime::AnyObject;
    use objc2::{
        define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly,
    };
    use objc2_app_kit::{
        NSAppearanceCustomization, NSBackingStoreType, NSColor, NSView, NSWindow, NSWindowAnimationBehavior,
        NSWindowButton, NSWindowStyleMask, NSWindowTitleVisibility,
    };
    use objc2_foundation::{NSObjectProtocol, NSPoint, NSRect, NSSize};
    use std::cell::{Cell, RefCell};

    use crate::ui::card::CardNSWindow;

    use crate::ui::view_base::as_any;

    pub struct SlotHostWindowIvars {
        corner_radius: Cell<f64>,
        guest: RefCell<Option<Weak<AnyObject>>>,
        on_empty: RefCell<Option<Box<dyn Fn()>>>,
        on_close_request: RefCell<Option<Box<dyn Fn()>>>,
    }

    define_class!(
        #[unsafe(super(NSWindow))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSSlotHostWindow"]
        #[ivars = SlotHostWindowIvars]
        pub struct SlotHostWindow;

        impl SlotHostWindow {
            #[unsafe(method(_cornerRadius))]
            fn _corner_radius(&self) -> f64 {
                self.ivars().corner_radius.get()
            }

            #[unsafe(method(performClose:))]
            fn perform_close(&self, _sender: Option<&AnyObject>) {
                if let Some(f) = self.ivars().on_close_request.borrow().as_ref() {
                    f();
                }
            }

            #[unsafe(method(close))]
            fn close(&self) {
                if let Some(f) = self.ivars().on_close_request.borrow().as_ref() {
                    f();
                }
            }

            #[unsafe(method(fireHostEmpty))]
            fn fire_host_empty(&self) {
                if self.ivars().guest.borrow().is_none() && self.isVisible() {
                    if let Some(f) = self.ivars().on_empty.borrow().as_ref() {
                        f();
                    }
                }
            }
        }

        unsafe impl NSObjectProtocol for SlotHostWindow {}
    );

    impl SlotHostWindow {
        pub fn create(mtm: MainThreadMarker) -> Retained<SlotHostWindow> {
            let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(900.0, 600.0));
            let mask = NSWindowStyleMask::Titled
                | NSWindowStyleMask::Closable
                | NSWindowStyleMask::Resizable
                | NSWindowStyleMask::FullSizeContentView;
            let this = SlotHostWindow::alloc(mtm).set_ivars(SlotHostWindowIvars {
                corner_radius: Cell::new(10.0),
                guest: RefCell::new(None),
                on_empty: RefCell::new(None),
                on_close_request: RefCell::new(None),
            });
            let window: Retained<SlotHostWindow> = unsafe {
                msg_send![
                    super(this),
                    initWithContentRect: frame,
                    styleMask: mask,
                    backing: NSBackingStoreType::Buffered,
                    defer: false
                ]
            };
            window.setTitlebarAppearsTransparent(true);
            window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
            for b in [
                NSWindowButton::CloseButton,
                NSWindowButton::MiniaturizeButton,
                NSWindowButton::ZoomButton,
            ] {
                if let Some(btn) = window.standardWindowButton(b) {
                    btn.setHidden(true);
                }
            }
            window.setOpaque(false);
            window.setBackgroundColor(Some(&NSColor::clearColor()));
            window.setHasShadow(true);
            unsafe { window.setReleasedWhenClosed(false) };
            window.setAnimationBehavior(NSWindowAnimationBehavior::None);
            window.setAcceptsMouseMovedEvents(true);
            let content = NSView::initWithFrame(NSView::alloc(mtm), frame);
            window.setContentView(Some(&content));
            window
        }

        pub fn set_corner_radius(&self, r: f64) {
            self.ivars().corner_radius.set(r);
            self.invalidateShadow();
        }

        pub fn set_on_close_request(&self, cb: Box<dyn Fn()>) {
            *self.ivars().on_close_request.borrow_mut() = Some(cb);
        }

    }
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

