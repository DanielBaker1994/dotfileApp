//! `ProseModeSwitch` (NotesProse.swift): the notes window's bottom-right
//! capsule — "≡ Prose | </> nvim" segments, then ◐ (document style) and ⤢
//! (pop the reading view out into a floating window).

#[cfg(target_os = "macos")]
use crate::ui::chrome::Rect;

/// Which segment was clicked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProseSwitchEvent {
    Mode(bool),
    PopOut,
    Style,
}

/// The pill's natural height — canonical `capsule_track` height, matching
/// `pill_selector::HEIGHT` (chips inset by `CAPSULE_GEO.track_pad` each side).
pub const HEIGHT: f64 = 26.0;

#[cfg(target_os = "macos")]
pub use appkit::ProseSwitch;

#[cfg(target_os = "macos")]
mod appkit {
    use super::*;
    use crate::ui::style::{self, Weight};
    use crate::ui::theme::PopupColors;
    use objc2::rc::Retained;
    
    use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{NSCursor, NSEvent, NSTrackingArea, NSView};
    use objc2_foundation::{NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};
    use std::cell::{Cell, RefCell};

    use crate::ui::view_base::{self};

    pub struct SwitchIvars {
        colors: Cell<PopupColors>,
        prose: Cell<bool>,
        edit_label: RefCell<String>,
        hover: Cell<Option<usize>>,
        on_event: RefCell<Option<Box<dyn Fn(ProseSwitchEvent)>>>,
        tracking: RefCell<Option<Retained<NSTrackingArea>>>,
    }

    define_class!(
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSProseSwitch"]
        #[ivars = SwitchIvars]
        pub struct ProseSwitch;

        impl ProseSwitch {
            #[unsafe(method(isFlipped))]
            fn is_flipped(&self) -> bool { true }

            #[unsafe(method(mouseDownCanMoveWindow))]
            fn mdcmw(&self) -> bool { false }

            #[unsafe(method(acceptsFirstMouse:))]
            fn afm(&self, _e: Option<&NSEvent>) -> bool { true }

            /// Every segment is clickable, so the whole capsule is a
            /// pointing-hand target (Swift `PopupWindow.resetCursorRects`
            /// gives its clickable controls `.pointingHand` the same way).
            #[allow(deprecated)]
            #[unsafe(method(resetCursorRects))]
            fn reset_cursor_rects(&self) {
                self.addCursorRect_cursor(self.bounds(), &NSCursor::pointingHandCursor());
            }

            #[unsafe(method(updateTrackingAreas))]
            fn uta(&self) {
                let _: () = unsafe { msg_send![super(self), updateTrackingAreas] };
                view_base::reset_tracking(self, &self.ivars().tracking, view_base::hover_opts());
            }

            #[unsafe(method(mouseMoved:))]
            fn mouse_moved(&self, e: &NSEvent) {
                let p = view_base::mouse_point(self, e);
                let h = (0..4).find(|i| self.seg(*i).contains(p.x, p.y));
                view_base::set_hover(self, &self.ivars().hover, h);
            }

            #[unsafe(method(mouseExited:))]
            fn mouse_exited(&self, _e: &NSEvent) {
                view_base::set_hover(self, &self.ivars().hover, None);
            }

            #[unsafe(method(mouseDown:))]
            fn mouse_down(&self, e: &NSEvent) {
                let p = view_base::mouse_point(self, e);
                let Some(i) = (0..4).find(|i| self.seg(*i).contains(p.x, p.y)) else { return };
                let ev = match i {
                    2 => ProseSwitchEvent::PopOut,
                    3 => ProseSwitchEvent::Style,
                    i => {
                        let want = i == 0;
                        // Always report the click: the notes surface owns the
                        // reading-view state, so a cached copy here must never
                        // swallow the event (it goes stale after the view is
                        // re-rendered / toggled outside this control).
                        self.ivars().prose.set(want);
                        self.setNeedsDisplay(true);
                        ProseSwitchEvent::Mode(want)
                    }
                };
                let cb = self.ivars().on_event.borrow_mut().take();
                if let Some(cb) = cb {
                    cb(ev);
                    let mut slot = self.ivars().on_event.borrow_mut();
                    if slot.is_none() {
                        *slot = Some(cb);
                    }
                }
            }

            #[unsafe(method(drawRect:))]
            fn draw_rect(&self, _dirty: NSRect) {
                self.draw();
            }
        }

        unsafe impl NSObjectProtocol for ProseSwitch {}
    );

    impl ProseSwitch {
        pub fn new(mtm: MainThreadMarker, colors: PopupColors, edit_label: &str) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(SwitchIvars {
                colors: Cell::new(colors),
                prose: Cell::new(false),
                edit_label: RefCell::new(edit_label.to_string()),
                hover: Cell::new(None),
                on_event: RefCell::new(None),
                tracking: RefCell::new(None),
            });
            let v: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
            let w = v.seg_widths().iter().sum::<f64>() + 6.0 + 34.0 + 32.0;
            v.setFrameSize(NSSize::new(w, HEIGHT));
            v.setToolTip(Some(&NSString::from_str(
                "Reading view ⌘⇧P — Esc goes back to the editor; ◐ picks a document style; ⤢ opens it in a floating window",
            )));
            v
        }

        pub fn set_on_event(&self, cb: Box<dyn Fn(ProseSwitchEvent)>) {
            *self.ivars().on_event.borrow_mut() = Some(cb);
        }

        pub fn set_colors(&self, c: PopupColors) {
            self.ivars().colors.set(c);
            self.setNeedsDisplay(true);
        }

        pub fn set_prose(&self, on: bool) {
            self.ivars().prose.set(on);
            self.setNeedsDisplay(true);
        }

        /// Pin to the editor's bottom-right (`layoutEditorScroll`).
        pub fn place(&self, editor: NSRect) {
            let s = self.frame().size;
            self.setFrameOrigin(NSPoint::new(
                editor.origin.x + editor.size.width - s.width - 16.0,
                editor.origin.y + editor.size.height - s.height - 10.0,
            ));
        }

        fn labels(&self) -> [(&'static str, String); 2] {
            [
                ("text.alignleft", "Prose".to_string()),
                ("chevron.left.forwardslash.chevron.right", self.ivars().edit_label.borrow().clone()),
            ]
        }

        fn seg_widths(&self) -> Vec<f64> {
            let f = style::system_font(12.0, Weight::Medium);
            // Canonical text chip (CAPSULE_GEO chip pad 22) + a 16 pt symbol
            // allowance: the icon sits at x + 8 (14 pt wide) and the label at
            // x + 26, so the segment is text + 38 = metrics width + 16.
            let labels: Vec<String> = self.labels().iter().map(|(_, t)| t.clone()).collect();
            let (ws, _) = style::capsule_metrics(&labels, &f);
            ws.iter().map(|w| w + 16.0).collect()
        }

        fn seg(&self, i: usize) -> Rect {
            let b = self.bounds();
            let h = b.size.height - 6.0;
            match i {
                2 => Rect::new(b.size.width - 3.0 - 30.0, 3.0, 30.0, h),
                3 => Rect::new(b.size.width - 3.0 - 30.0 - 32.0, 3.0, 30.0, h),
                i => {
                    let w = self.seg_widths();
                    let x = 3.0 + w[..i].iter().sum::<f64>();
                    Rect::new(x, 3.0, w[i], h)
                }
            }
        }

        fn draw(&self) {
            let c = self.ivars().colors.get();
            let b = self.bounds();
            // One track, theme's `track_fill` — no under-fill, so the pill
            // reads identical to `TabPillView` at every preset.
            style::capsule_track(Rect::new(0.0, 0.0, b.size.width, b.size.height), &c);
            let hover = self.ivars().hover.get();
            for (i, sym) in [(3usize, "paintpalette"), (2usize, "rectangle.portrait.and.arrow.right")] {
                let r = self.seg(i);
                style::capsule_chip(r, &c, false, hover == Some(i));
                style::draw_symbol(sym, r, if hover == Some(i) { c.text } else { c.dim }, 11.0);
            }
            let f = style::system_font(12.0, Weight::Medium);
            let prose = self.ivars().prose.get();
            for (i, (sym, label)) in self.labels().iter().enumerate() {
                let r = self.seg(i);
                let on = (i == 0) == prose;
                style::capsule_chip(r, &c, on, hover == Some(i));
                let fg = if on { c.text } else { c.dim };
                style::draw_symbol(sym, Rect::new(r.x + 8.0, r.y, 14.0, r.height), if on { c.accent_on() } else { fg }, 10.0);
                let (_, th) = style::text_size(label, &f);
                style::draw_text(label, r.x + 26.0, r.mid_y() - th / 2.0, &f, fg);
            }
        }
    }
}
