//! A thin draggable vertical divider between a left pane (sidebar / rail /
//! tree) and the content to its right.
//!
//! The owner places one at the divider x (a few points wide, full height),
//! gives it the pane's `[min, max]` width range and the current width, and
//! receives a [`DividerDrag`] through its sink on every mouse move and on
//! mouse-up. The owner re-lays-out live and persists the width on `done`.
//!
//! The divider measures the pointer in *window* coordinates: the owner moves
//! the divider's frame while dragging, so view-relative points would drift.

/// Clamp a pane width into `[min, max]`; `max` never loses to `min`.
pub fn clamp_width(width: f64, min: f64, max: f64) -> f64 {
    let min = min.max(0.0);
    let max = max.max(min);
    width.max(min).min(max)
}

/// The drag bounds of one pane (`sidebar-min-width` / `sidebar-max-width`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DividerRange {
    pub min: f64,
    pub max: f64,
}

impl DividerRange {
    pub fn new(min: f64, max: f64) -> Self {
        let min = min.max(0.0);
        DividerRange { min, max: max.max(min) }
    }

    pub fn clamp(&self, width: f64) -> f64 {
        clamp_width(width, self.min, self.max)
    }
}

/// What a divider drag reports to the owner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DividerDrag {
    /// The pane width the drag wants (already clamped to the range).
    pub width: f64,
    /// `true` on mouse-up (persist), `false` while dragging.
    pub done: bool,
}

#[cfg(target_os = "macos")]
pub use appkit::PaneDividerView;

#[cfg(target_os = "macos")]
mod appkit {
    use super::*;
    use objc2::rc::Retained;
    use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{NSCursor, NSEvent, NSView};
    use objc2_foundation::{NSObjectProtocol, NSRect};
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    type Sink = Rc<dyn Fn(DividerDrag)>;

    pub struct DividerIvars {
        min: Cell<f64>,
        max: Cell<f64>,
        /// The pane width the owner last laid out (read at mouse-down).
        current: Cell<f64>,
        /// The pointer x (window coords) and pane width at mouse-down.
        start_x: Cell<f64>,
        start_width: Cell<f64>,
        dragging: Cell<bool>,
        /// Set once the pointer actually moved; a plain click persists nothing.
        moved: Cell<bool>,
        sink: RefCell<Option<Sink>>,
    }

    define_class!(
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSPaneDividerView"]
        #[ivars = DividerIvars]
        pub struct PaneDividerView;

        impl PaneDividerView {
            #[unsafe(method(isFlipped))]
            fn is_flipped(&self) -> bool {
                true
            }

            #[unsafe(method(mouseDownCanMoveWindow))]
            fn mouse_down_can_move_window(&self) -> bool {
                false
            }

            #[unsafe(method(acceptsFirstMouse:))]
            fn accepts_first_mouse(&self, _e: Option<&NSEvent>) -> bool {
                true
            }

            #[unsafe(method(mouseDown:))]
            fn mouse_down(&self, e: &NSEvent) {
                let iv = self.ivars();
                iv.dragging.set(true);
                iv.moved.set(false);
                iv.start_x.set(e.locationInWindow().x);
                iv.start_width.set(iv.current.get());
                NSCursor::resizeLeftRightCursor().set();
            }

            #[unsafe(method(mouseDragged:))]
            fn mouse_dragged(&self, e: &NSEvent) {
                let iv = self.ivars();
                if !iv.dragging.get() {
                    return;
                }
                let w = clamp_width(
                    iv.start_width.get() + (e.locationInWindow().x - iv.start_x.get()),
                    iv.min.get(),
                    iv.max.get(),
                );
                iv.moved.set(true);
                self.emit(DividerDrag { width: w, done: false });
                NSCursor::resizeLeftRightCursor().set();
            }

            #[unsafe(method(mouseUp:))]
            fn mouse_up(&self, e: &NSEvent) {
                let iv = self.ivars();
                if !iv.dragging.get() {
                    return;
                }
                iv.dragging.set(false);
                NSCursor::arrowCursor().set();
                let w = clamp_width(
                    iv.start_width.get() + (e.locationInWindow().x - iv.start_x.get()),
                    iv.min.get(),
                    iv.max.get(),
                );
                if iv.moved.get() || (w - iv.start_width.get()).abs() >= 2.0 {
                    self.emit(DividerDrag { width: w, done: true });
                }
            }

            #[unsafe(method(resetCursorRects))]
            fn reset_cursor_rects(&self) {
                self.addCursorRect_cursor(self.bounds(), &NSCursor::resizeLeftRightCursor());
            }

            #[unsafe(method(drawRect:))]
            fn draw_rect(&self, _dirty: NSRect) {}
        }

        unsafe impl NSObjectProtocol for PaneDividerView {}
    );

    impl PaneDividerView {
        pub fn new(mtm: MainThreadMarker, range: DividerRange) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(DividerIvars {
                min: Cell::new(range.min),
                max: Cell::new(range.max),
                current: Cell::new(range.min),
                start_x: Cell::new(0.0),
                start_width: Cell::new(range.min),
                dragging: Cell::new(false),
                moved: Cell::new(false),
                sink: RefCell::new(None),
            });
            unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] }
        }

        /// Install (or replace) the drag sink.
        pub fn set_on_drag(&self, cb: Box<dyn Fn(DividerDrag)>) {
            *self.ivars().sink.borrow_mut() = Some(Rc::from(cb));
        }

        pub fn clamp(&self, width: f64) -> f64 {
            clamp_width(width, self.ivars().min.get(), self.ivars().max.get())
        }

        /// Move the divider and refresh its cursor rect (the frame changes as
        /// the pane resizes, and AppKit does not re-run `resetCursorRects` on
        /// its own).
        pub fn place(&self, frame: NSRect) {
            self.setFrame(frame);
            if let Some(w) = self.window() {
                w.invalidateCursorRectsForView(self);
            }
        }

        /// The pane width the owner last laid out. Ignored mid-drag so the
        /// drag's start width stays fixed while the frame moves under it.
        pub fn set_width(&self, width: f64) {
            if !self.ivars().dragging.get() {
                self.ivars().current.set(width);
            }
        }

        fn emit(&self, drag: DividerDrag) {
            let cb = self.ivars().sink.borrow().clone();
            if let Some(cb) = cb {
                cb(drag);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_keeps_width_in_range() {
        assert_eq!(clamp_width(200.0, 140.0, 480.0), 200.0);
        assert_eq!(clamp_width(10.0, 140.0, 480.0), 140.0);
        assert_eq!(clamp_width(900.0, 140.0, 480.0), 480.0);
    }

    #[test]
    fn clamp_never_lets_max_lose_to_min() {
        assert_eq!(clamp_width(200.0, 300.0, 100.0), 300.0);
        assert_eq!(clamp_width(50.0, 300.0, 100.0), 300.0);
    }

    #[test]
    fn range_clamps_and_normalizes() {
        let r = DividerRange::new(140.0, 480.0);
        assert_eq!(r.clamp(250.0), 250.0);
        assert_eq!(r.clamp(-5.0), 140.0);
        let flipped = DividerRange::new(480.0, 140.0);
        assert_eq!(flipped.min, 480.0);
        assert_eq!(flipped.max, 480.0);
        assert_eq!(flipped.clamp(200.0), 480.0);
    }
}
