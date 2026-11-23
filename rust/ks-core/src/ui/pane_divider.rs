//! A thin draggable vertical divider between two panes — most often the left
//! pane (sidebar / rail / tree) and the content to its right, but also a right
//! pane (Jira's ticket inspector) via [`PaneDividerView::set_right_pane`].
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

/// The pane width a drag wants: a left pane grows rightward with the pointer,
/// a right pane (`right_pane`) grows leftward — the delta is mirrored.
pub fn dragged_width(start_width: f64, pointer_x: f64, start_x: f64, right_pane: bool) -> f64 {
    let dx = pointer_x - start_x;
    if right_pane {
        start_width - dx
    } else {
        start_width + dx
    }
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
    use objc2_app_kit::{NSCursor, NSEvent};

    use objc2_foundation::{NSObjectProtocol, NSRect};
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    type Sink = Rc<dyn Fn(DividerDrag)>;

    pub struct DividerIvars {
        min: Cell<f64>,
        max: Cell<f64>,
        /// The pane width the owner last laid out (read at mouse-down).
        current: Cell<f64>,
        /// `true` for a RIGHT pane: the drag mirrors (pointer left grows it).
        right: Cell<bool>,
        /// The pointer x (window coords) and pane width at mouse-down.
        start_x: Cell<f64>,
        start_width: Cell<f64>,
        dragging: Cell<bool>,
        /// Set once the pointer actually moved; a plain click persists nothing.
        moved: Cell<bool>,
        sink: RefCell<Option<Sink>>,
    }

    define_class!(
        #[unsafe(super(crate::ui::view_base::WSFlippedView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSPaneDividerView"]
        #[ivars = DividerIvars]
        pub struct PaneDividerView;

        impl PaneDividerView {
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
                    dragged_width(iv.start_width.get(), e.locationInWindow().x, iv.start_x.get(), iv.right.get()),
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
                    dragged_width(iv.start_width.get(), e.locationInWindow().x, iv.start_x.get(), iv.right.get()),
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
                right: Cell::new(false),
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

        /// Mark this divider as the edge of a RIGHT pane: dragging the pointer
        /// LEFT grows the pane (the delta is mirrored vs. a left pane).
        pub fn set_right_pane(&self, on: bool) {
            self.ivars().right.set(on);
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
    fn dragged_width_grows_with_the_pointer_for_left_panes() {
        // Pointer moves right: the left pane gets wider by the same delta.
        assert_eq!(dragged_width(300.0, 360.0, 300.0, false), 360.0);
        assert_eq!(dragged_width(300.0, 240.0, 300.0, false), 240.0);
    }

    #[test]
    fn dragged_width_mirrors_for_right_panes() {
        // Pointer moves LEFT: the right pane (Jira's ticket panel) grows.
        assert_eq!(dragged_width(300.0, 240.0, 300.0, true), 360.0);
        assert_eq!(dragged_width(300.0, 360.0, 300.0, true), 240.0);
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
