//! `TabPillView` — the canonical reusable capsule for **mutually exclusive
//! button groups**: hand-drawn `capsule_track` + `capsule_chip` on the theme
//! tokens, the same look the Confluence mode chips use. A native
//! `NSSegmentStyle::Capsule` control renders system colours the theme cannot
//! reach (selected chips went system-blue under every preset), so the pattern
//! stays hand-drawn — and `NSSegmentedControl` usage must stay at zero
//! (`grep -rn NSSegmentedControl rust/ws-rs/src`).
//!
//! Extracted from `views/jira_view.rs` (PLAN-capsule-toggle-consistency §3.1)
//! so surfaces outside Jira can reuse it. Geometry comes from
//! [`crate::ui::style::capsule_metrics`] (chip pad 22, gutters 2, track
//! padding 3) — the track hugs the chips (intrinsic [`TabPillView::desired_width`]),
//! only squeezing to equal-share chips when the container is narrower.
//!
//! Reserve this vocabulary for *state selection* (exactly-one-of). One-shot
//! commands are push buttons / [`crate::ui::style::capsule_outline`].

#[cfg(target_os = "macos")]
pub use appkit::TabPillView;

/// The pill's natural height (track height; chips inset by the track padding).
pub const HEIGHT: f64 = 26.0;

#[cfg(target_os = "macos")]
mod appkit {
    use crate::ui::chrome::Rect;
    use crate::ui::style::{self, Weight};
    use crate::ui::theme::PopupColors;
    use objc2::rc::Retained;
    
    use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{NSEvent, NSFont, NSTrackingArea, NSView};
    use objc2_foundation::{NSObjectProtocol, NSRect, NSSize};
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use crate::ui::view_base::{self};

    pub struct TabPillIvars {
        colors: Cell<PopupColors>,
        labels: RefCell<Vec<String>>,
        tab: Cell<usize>,
        hover: Cell<Option<usize>>,
        pressed: Cell<Option<usize>>,
        sink: RefCell<Option<Rc<dyn Fn(usize)>>>,
        tracking: RefCell<Option<Retained<NSTrackingArea>>>,
    }

    define_class!(
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSTabPillView"]
        #[ivars = TabPillIvars]
        pub struct TabPillView;

        impl TabPillView {
            #[unsafe(method(isFlipped))]
            fn is_flipped(&self) -> bool {
                true
            }

            #[unsafe(method(drawRect:))]
            fn draw_rect(&self, _r: NSRect) {
                let iv = self.ivars();
                let c = iv.colors.get();
                let b = self.bounds();
                style::capsule_track(Rect::new(b.origin.x, b.origin.y, b.size.width, b.size.height), &c);
                let f = style::system_font(12.0, Weight::Medium);
                let rects = Self::chip_rects(&iv.labels.borrow(), &f, b.size.width, b.size.height);
                for (i, r) in rects.iter().enumerate() {
                    let on = iv.tab.get() == i;
                    let hov = iv.pressed.get() == Some(i) || (!on && iv.hover.get() == Some(i));
                    style::capsule_chip(*r, &c, on, hov);
                    let col = if on { c.text } else if hov { c.text.with_alpha(0.9) } else { c.dim };
                    style::draw_text_centered(&iv.labels.borrow()[i], *r, &f, col);
                }
            }

            #[unsafe(method(mouseDown:))]
            fn mouse_down(&self, e: &NSEvent) {
                let p = self.convertPoint_fromView(e.locationInWindow(), None);
                let idx = self.chip_at(p.x, p.y);
                if idx != self.ivars().tab.get() {
                    self.ivars().pressed.set(Some(idx));
                    self.setNeedsDisplay(true);
                }
            }

            #[unsafe(method(mouseUp:))]
            fn mouse_up(&self, e: &NSEvent) {
                let iv = self.ivars();
                let was = iv.pressed.take();
                let p = self.convertPoint_fromView(e.locationInWindow(), None);
                self.setNeedsDisplay(true);
                if was.is_some() && Some(self.chip_at(p.x, p.y)) == was {
                    if let Some(s) = iv.sink.borrow().as_ref() {
                        s(was.unwrap());
                    }
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
                if self.ivars().hover.get().is_some() {
                    self.ivars().hover.set(None);
                    self.setNeedsDisplay(true);
                }
            }

            #[unsafe(method(setFrameSize:))]
            fn set_frame_size(&self, size: NSSize) {
                let _: () = unsafe { msg_send![super(self), setFrameSize: size] };
                self.reset_tracking();
            }
        }

        unsafe impl NSObjectProtocol for TabPillView {}
    );

    impl TabPillView {
        fn track_hover(&self, e: &NSEvent) {
            let p = self.convertPoint_fromView(e.locationInWindow(), None);
            let idx = self.chip_at(p.x, p.y);
            if self.ivars().hover.get() != Some(idx) {
                self.ivars().hover.set(Some(idx));
                self.setNeedsDisplay(true);
            }
        }

        fn chip_at(&self, x: f64, y: f64) -> usize {
            let iv = self.ivars();
            let b = self.bounds();
            let f = style::system_font(12.0, Weight::Medium);
            Self::chip_rects(&iv.labels.borrow(), &f, b.size.width, b.size.height)
                .iter()
                .position(|r| r.contains(x, y))
                .unwrap_or_else(|| iv.tab.get())
        }

        /// Chips share the track: intrinsic widths from [`style::capsule_metrics`]
        /// with 3 pt track padding and 2 pt gutters. When the view is squeezed
        /// narrower than that, the chips share the track equally.
        fn chip_rects(labels: &[String], f: &NSFont, w: f64, h: f64) -> Vec<Rect> {
            if labels.is_empty() {
                return Vec::new();
            }
            let geo = style::CAPSULE_GEO;
            let (base, total) = style::capsule_metrics(labels, f);
            let ws: Vec<f64> = if total <= w {
                base
            } else {
                let even =
                    ((w - 2.0 * geo.track_pad - (labels.len() - 1) as f64 * geo.gutter) / labels.len() as f64).max(36.0);
                vec![even; labels.len()]
            };
            let mut x = geo.track_pad + (w - total).max(0.0) / 2.0;
            let mut out = Vec::new();
            for wv in ws {
                out.push(Rect::new(x, geo.track_pad, wv, (h - 2.0 * geo.track_pad).max(1.0)));
                x += wv + geo.gutter;
            }
            out
        }

        /// The snug width of the whole pill (chips + track padding) so the
        /// container can be sized to the buttons instead of the column.
        pub fn desired_width(&self) -> f64 {
            let iv = self.ivars();
            let labels = iv.labels.borrow();
            if labels.is_empty() {
                return 0.0;
            }
            let f = style::system_font(12.0, Weight::Medium);
            style::capsule_metrics(&labels, &f).1
        }

        fn reset_tracking(&self) {
            view_base::reset_tracking(self, &self.ivars().tracking, view_base::hover_opts());
        }

        pub fn new(mtm: MainThreadMarker, colors: PopupColors, labels: &[&str]) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(TabPillIvars {
                colors: Cell::new(colors),
                labels: RefCell::new(labels.iter().map(|s| s.to_string()).collect()),
                tab: Cell::new(0),
                hover: Cell::new(None),
                pressed: Cell::new(None),
                sink: RefCell::new(None),
                tracking: RefCell::new(None),
            });
            let v: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
            v.reset_tracking();
            v
        }

        /// Reflect the selected surface (keeps the pill in step with
        /// `set_tab`, including host-driven switches).
        pub fn set_selected(&self, i: usize) {
            if self.ivars().tab.get() != i {
                self.ivars().tab.set(i);
                self.setNeedsDisplay(true);
            }
        }

        pub fn set_on_pick(&self, cb: Box<dyn Fn(usize)>) {
            *self.ivars().sink.borrow_mut() = Some(Rc::from(cb));
        }
    }
}
