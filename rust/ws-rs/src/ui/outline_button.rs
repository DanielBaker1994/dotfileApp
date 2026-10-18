//! `OutlineButton` — the canonical reusable capsule for **one-shot commands**
//! (the counterpart to [`crate::ui::pill_selector::TabPillView`], which owns
//! *state selection* per that module's rule). Draws the two `style.rs`
//! one-shot dialects — [`crate::ui::style::capsule_outline`] (accent, the
//! "Run" / "Set Up…" / "Open full detail" role) and the neutral text-tinted
//! outline (the "Load more" / "Copy CQL" role) — with the shared hover **and**
//! press feedback the hand-rolled copies kept missing.
//!
//! Native `NSButton` bezels (`BezelStyle::Push`, `MomentaryPushIn`) render
//! system chrome the `[theme]` cannot reach — the same reason
//! `NSSegmentedControl` is banned (see `pill_selector.rs`). Dialog and bar
//! one-shots route through this view instead; the NSControl `target` /
//! `action` pairing is preserved via [`OutlineButton::set_target_action`] so
//! existing handlers need no rewrite. Selection (an "on" state) belongs to a
//! pill, not here.

#[cfg(target_os = "macos")]
pub use appkit::{capsule_width, draw_chrome, OutlineButton};

/// The canonical one-shot capsule height (same 26 pt track as the pills).
pub const HEIGHT: f64 = 26.0;

/// Which of the two one-shot dialects to draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// Accent-tinted capsule outline + accent text (`capsule_outline`).
    Accent,
    /// Neutral text-tinted outline + text (the "Load more" role).
    Neutral,
}

#[cfg(target_os = "macos")]
mod appkit {
    use super::{Tone, HEIGHT};
    use crate::ui::chrome::Rect;
    use crate::ui::style::{self, Weight};
    use crate::ui::theme::{PopupColors, Rgba};
    use objc2::rc::{Retained, Weak};
    use objc2::runtime::{AnyObject, Sel};
    use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{NSEvent, NSFont, NSTrackingArea, NSView};
    use objc2_foundation::NSObjectProtocol;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use crate::ui::view_base::{self, as_any};

    /// Chip width allowance on top of the title text (7 pt/side + breathing).
    const SIDE_PAD: f64 = 14.0;

    /// The canonical capsule chrome — the two one-shot dialects (accent /
    /// neutral outline) plus the filled `toggled` chip — returning the text
    /// colour that pairs with what it drew. Shared by [`OutlineButton`]'s
    /// own drawing and by hand-drawn strips (the Files toolbar) so the
    /// geometry and state language cannot drift per surface.
    pub fn draw_chrome(
        r: Rect,
        c: &PopupColors,
        tone: Tone,
        toggled: bool,
        enabled: bool,
        hover: bool,
        pressed: bool,
    ) -> Rgba {
        let hot = hover || pressed;
        if toggled {
            // Toggle ON: the selected-chip fill (press darkens it), same
            // language as the `capsule_chip` pills.
            style::capsule_track(r, c);
            style::capsule_chip(r.inset_by(3.0, 3.0), c, true, enabled && pressed);
            return if enabled { c.text } else { c.dim.with_alpha(0.6) };
        }
        match tone {
            Tone::Accent => {
                style::capsule_outline(r, c, hot && enabled);
                if enabled {
                    c.accent
                } else {
                    c.dim.with_alpha(0.6)
                }
            }
            Tone::Neutral => {
                style::stroke_rounded(
                    r,
                    r.height / 2.0,
                    1.0,
                    c.text.with_alpha(if enabled { 0.25 } else { 0.12 }),
                );
                if enabled && pressed {
                    style::fill_rounded(r, r.height / 2.0, c.text.with_alpha(0.14));
                } else if enabled && hover {
                    style::fill_rounded(r, r.height / 2.0, c.text.with_alpha(0.08));
                }
                if enabled {
                    c.text.with_alpha(0.85)
                } else {
                    c.dim.with_alpha(0.6)
                }
            }
        }
    }

    /// Capsule width for content at font `f`: the title + [`SIDE_PAD`] per
    /// side, plus the strip allowances for a leading SF symbol (12 pt + the
    /// 4 pt gap) and a trailing chevron (12 pt). [`OutlineButton::width_for`]
    /// is the title-only case of this.
    pub fn capsule_width(f: &NSFont, title: &str, has_sym: bool, has_chev: bool) -> f64 {
        let tw = if title.is_empty() { 0.0 } else { style::text_size(title, f).0.ceil() };
        let sym = if has_sym { 12.0 + if title.is_empty() { 0.0 } else { 4.0 } } else { 0.0 };
        let chev = if has_chev { 12.0 } else { 0.0 };
        (tw + sym + chev + 2.0 * SIDE_PAD).ceil()
    }

    pub struct OutlineButtonIvars {
        colors: Cell<PopupColors>,
        title: RefCell<String>,
        tone: Cell<Tone>,
        enabled: Cell<bool>,
        /// The filled "on" look for toggle commands (the checkbox role);
        /// selection groups belong to a pill, this is the single-chip state.
        on: Cell<bool>,
        hover: Cell<bool>,
        pressed: Cell<bool>,
        sink: RefCell<Option<Rc<dyn Fn()>>>,
        target: RefCell<Option<Weak<AnyObject>>>,
        action: Cell<Option<Sel>>,
        /// NSControl-compatible `tag` (the row-scoped actions read it off the
        /// sender, exactly as with `NSButton`).
        tag: Cell<objc2_foundation::NSInteger>,
        tracking: RefCell<Option<Retained<NSTrackingArea>>>,
    }

    define_class!(
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSOutlineButton"]
        #[ivars = OutlineButtonIvars]
        pub struct OutlineButton;

        impl OutlineButton {
            #[unsafe(method(isFlipped))]
            fn is_flipped(&self) -> bool {
                true
            }

            #[unsafe(method(drawRect:))]
            fn draw_rect(&self, _r: objc2_foundation::NSRect) {
                let iv = self.ivars();
                let c = iv.colors.get();
                let b = self.bounds();
                let r = Rect::new(b.origin.x, b.origin.y, b.size.width, b.size.height);
                let f = style::system_font(12.5, Weight::Semibold);
                let (enabled, hov, pres) = (iv.enabled.get(), iv.hover.get(), iv.pressed.get());
                let fg = draw_chrome(r, &c, iv.tone.get(), iv.on.get(), enabled, hov, pres);
                style::draw_text_centered(&iv.title.borrow(), r, &f, fg);
            }

            #[unsafe(method(mouseDown:))]
            fn mouse_down(&self, _e: &NSEvent) {
                if !self.ivars().enabled.get() {
                    return;
                }
                self.ivars().pressed.set(true);
                self.setNeedsDisplay(true);
            }

            #[unsafe(method(mouseUp:))]
            fn mouse_up(&self, e: &NSEvent) {
                let iv = self.ivars();
                let was = iv.pressed.replace(false);
                self.setNeedsDisplay(true);
                if !was || !iv.enabled.get() {
                    return;
                }
                let p = self.convertPoint_fromView(e.locationInWindow(), None);
                let b = self.bounds();
                if !Rect::new(b.origin.x, b.origin.y, b.size.width, b.size.height).contains(p.x, p.y) {
                    return;
                }
                if let Some(s) = iv.sink.borrow().as_ref() {
                    s();
                    return;
                }
                // Classic target/action: the button sends itself, mirroring
                // `NSButton` (sender-less actions ignore it; `tag`-reading
                // ones, like the setup fix rows, still work).
                if let (Some(sel), Some(target)) = (iv.action.get(), iv.target.borrow().as_ref().and_then(|w| w.load())) {
                    let t = as_any(&*target);
                    let me = as_any(self);
                    let _: () = unsafe { msg_send![t, performSelector: sel, withObject: me] };
                }
            }

            #[unsafe(method(mouseEntered:))]
            fn mouse_entered(&self, e: &NSEvent) {
                self.track_hover(e);
            }

            #[unsafe(method(mouseMoved:))]
            fn mouse_moved(&self, e: &NSEvent) {
                self.track_hover(e);
            }

            #[unsafe(method(mouseExited:))]
            fn mouse_exited(&self, _e: &NSEvent) {
                if self.ivars().hover.get() {
                    self.ivars().hover.set(false);
                    self.setNeedsDisplay(true);
                }
            }

            #[unsafe(method(setFrameSize:))]
            fn set_frame_size(&self, size: objc2_foundation::NSSize) {
                let _: () = unsafe { msg_send![super(self), setFrameSize: size] };
                self.reset_tracking();
            }

            #[unsafe(method(tag))]
            fn tag_get(&self) -> objc2_foundation::NSInteger {
                self.ivars().tag.get()
            }

            #[unsafe(method(setTag:))]
            fn tag_set(&self, t: objc2_foundation::NSInteger) {
                self.ivars().tag.set(t);
            }
        }

        unsafe impl NSObjectProtocol for OutlineButton {}
    );

    impl OutlineButton {
        fn track_hover(&self, e: &NSEvent) {
            let p = self.convertPoint_fromView(e.locationInWindow(), None);
            let b = self.bounds();
            let on = Rect::new(b.origin.x, b.origin.y, b.size.width, b.size.height).contains(p.x, p.y);
            if self.ivars().hover.get() != on {
                self.ivars().hover.set(on);
                self.setNeedsDisplay(true);
            }
        }

        fn reset_tracking(&self) {
            view_base::reset_tracking(self, &self.ivars().tracking, view_base::hover_opts());
        }

        pub fn new(mtm: MainThreadMarker, colors: PopupColors, title: &str) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(OutlineButtonIvars {
                colors: Cell::new(colors),
                title: RefCell::new(title.to_string()),
                tone: Cell::new(Tone::Accent),
                enabled: Cell::new(true),
                on: Cell::new(false),
                hover: Cell::new(false),
                pressed: Cell::new(false),
                sink: RefCell::new(None),
                target: RefCell::new(None),
                action: Cell::new(None),
                tag: Cell::new(0),
                tracking: RefCell::new(None),
            });
            let w = Self::width_for(title);
            let v: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRectZero(w)] };
            v.reset_tracking();
            v
        }

        /// The snug width for a title: text + side padding. Layout callers
        /// pair this with [`HEIGHT`].
        pub fn width_for(title: &str) -> f64 {
            let f = style::system_font(12.5, Weight::Semibold);
            capsule_width(&f, title, false, false)
        }

        pub fn set_colors(&self, c: PopupColors) {
            self.ivars().colors.set(c);
            self.setNeedsDisplay(true);
        }

        pub fn set_tone(&self, t: Tone) {
            self.ivars().tone.set(t);
            self.setNeedsDisplay(true);
        }

        pub fn title(&self) -> String {
            self.ivars().title.borrow().clone()
        }

        /// Swap the label (keeps the click wiring); resize the frame to the
        /// new [`Self::width_for`].
        pub fn set_title(&self, t: &str) {
            *self.ivars().title.borrow_mut() = t.to_string();
            self.setNeedsDisplay(true);
        }

        pub fn set_enabled(&self, on: bool) {
            if self.ivars().enabled.get() != on {
                self.ivars().enabled.set(on);
                if !on {
                    self.ivars().hover.set(false);
                    self.ivars().pressed.set(false);
                }
                self.setNeedsDisplay(true);
            }
        }

        pub fn enabled(&self) -> bool {
            self.ivars().enabled.get()
        }

        /// The toggle state (the checkbox role): `on` draws the filled chip.
        pub fn set_on(&self, on: bool) {
            if self.ivars().on.get() != on {
                self.ivars().on.set(on);
                self.setNeedsDisplay(true);
            }
        }

        pub fn is_on(&self) -> bool {
            self.ivars().on.get()
        }

        /// NSControl-style association id for sender-less routing (the
        /// `tag`/`setTag:` ObjC selectors read the same cell).
        pub fn set_tag(&self, t: objc2_foundation::NSInteger) {
            self.ivars().tag.set(t);
        }

        pub fn set_on_click(&self, cb: impl Fn() + 'static) {
            *self.ivars().sink.borrow_mut() = Some(Rc::new(cb));
        }

        /// NSControl-compatible wiring: on click the target receives
        /// `[target action:nil]` (sender-less — the converted actions ignore
        /// it). The target is held weakly, so a released handler makes clicks
        /// no-ops instead of wild pointers.
        pub fn set_target_action(&self, target: &AnyObject, action: Sel) {
            let raw = target as *const AnyObject as *mut AnyObject;
            let weak = unsafe { Retained::retain(raw) }.as_ref().map(Weak::from_retained);
            *self.ivars().target.borrow_mut() = weak;
            self.ivars().action.set(Some(action));
        }
    }

    /// `NSZeroRect` is not exposed; build it around the computed width.
    #[allow(non_snake_case)]
    fn NSRectZero(w: f64) -> objc2_foundation::NSRect {
        objc2_foundation::NSRect::new(
            objc2_foundation::NSPoint::new(0.0, 0.0),
            objc2_foundation::NSSize::new(w, HEIGHT),
        )
    }
}
