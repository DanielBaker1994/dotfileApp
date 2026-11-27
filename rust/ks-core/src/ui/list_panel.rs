//! A list popup in Swift's `PopupWindow` look (the palette, the view
//! switcher): the themed card, a bare search line at the top (large caret, no
//! box), then custom-drawn rows with hover + click. The owner draws each row
//! through a closure and handles typing / commands / clicks.

use crate::ui::chrome::Rect;

pub const SEARCH_H: f64 = 44.0;
pub const PAD_BOTTOM: f64 = 6.0;

/// Height for `n` rows of `row_h`.
pub fn panel_height(n: usize, row_h: f64, max_rows: usize) -> f64 {
    SEARCH_H + n.clamp(1, max_rows.max(1)) as f64 * row_h + PAD_BOTTOM
}

pub fn row_rect(i: usize, row_h: f64, width: f64) -> Rect {
    Rect::new(0.0, i as f64 * row_h, width, row_h)
}

/// The first row to draw so `selection` stays in a window of `visible` rows
/// (scrolling as little as possible from `top`). The panel caps its height
/// at a row limit; past it the selection used to move off the bottom unseen.
pub fn scroll_top(top: usize, selection: usize, visible: usize, count: usize) -> usize {
    let visible = visible.max(1);
    let top = if selection < top {
        selection
    } else if selection >= top + visible {
        selection + 1 - visible
    } else {
        top
    };
    top.min(count.saturating_sub(visible))
}

#[cfg(target_os = "macos")]
pub use appkit::{ListPanel, ListRows};

#[cfg(target_os = "macos")]
mod appkit {
    use super::*;
    use crate::ui::theme::PopupColors;
    use objc2::rc::Retained;
    
    use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{NSEvent, NSFocusRingType, NSFont, NSTextField, NSTrackingArea, NSView};
    use objc2_foundation::{NSObjectProtocol, NSPoint, NSRect, NSSize};
    use std::cell::{Cell, RefCell};

    use crate::ui::view_base::{self};

    type Drawer = Box<dyn Fn(Rect, usize, bool, bool)>;

    pub struct RowsIvars {
        count: Cell<usize>,
        selection: Cell<usize>,
        row_h: Cell<f64>,
        hover: Cell<Option<usize>>,
        drawer: RefCell<Option<Drawer>>,
        on_click: RefCell<Option<Box<dyn Fn(usize, f64, bool)>>>,
        tracking: RefCell<Option<Retained<NSTrackingArea>>>,
        /// The first row drawn ([`scroll_top`]).
        top: Cell<usize>,
    }

    define_class!(
        #[unsafe(super(crate::ui::view_base::WSFlippedView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSListRows"]
        #[ivars = RowsIvars]
        pub struct ListRows;

        impl ListRows {
            #[unsafe(method(mouseDownCanMoveWindow))]
            fn mdcmw(&self) -> bool { false }
            #[unsafe(method(acceptsFirstMouse:))]
            fn afm(&self, _e: Option<&NSEvent>) -> bool { true }

            #[unsafe(method(updateTrackingAreas))]
            fn uta(&self) {
                let _: () = unsafe { msg_send![super(self), updateTrackingAreas] };
                view_base::reset_tracking(self, &self.ivars().tracking, view_base::hover_opts());
            }

            #[unsafe(method(mouseMoved:))]
            fn mouse_moved(&self, e: &NSEvent) {
                let p = view_base::mouse_point(self, e);
                let h = self.row_at(p.y);
                view_base::set_hover(self, &self.ivars().hover, h);
            }

            #[unsafe(method(mouseExited:))]
            fn mouse_exited(&self, _e: &NSEvent) {
                view_base::set_hover(self, &self.ivars().hover, None);
            }

            #[unsafe(method(mouseDown:))]
            fn mouse_down(&self, e: &NSEvent) {
                let p = view_base::mouse_point(self, e);
                let Some(row) = self.row_at(p.y) else { return };
                if let Some(cb) = self.ivars().on_click.borrow().as_ref() {
                    cb(row, p.x, e.clickCount() >= 2);
                }
            }

            #[unsafe(method(drawRect:))]
            fn draw_rect(&self, dirty: NSRect) {
                let bounds = self.bounds();
                let w = bounds.size.width;
                let rh = self.ivars().row_h.get();
                let count = self.ivars().count.get();
                if rh <= 0.0 || count == 0 {
                    return;
                }
                let sel = self.ivars().selection.get();
                let hov = self.ivars().hover.get();
                let top = self.ivars().top.get();
                let vis_start = (dirty.origin.y / rh).floor().max(0.0) as usize;
                let vis_end = (((dirty.origin.y + dirty.size.height) / rh).ceil().max(0.0) as usize)
                    .min((bounds.size.height / rh).ceil().max(1.0) as usize);
                let first = (top + vis_start).min(count);
                let last = (top + vis_end).min(count);
                if let Some(d) = self.ivars().drawer.borrow().as_ref() {
                    for i in first..last {
                        d(row_rect(i - top, rh, w), i, i == sel, hov == Some(i));
                    }
                }
            }

            #[unsafe(method(setFrameSize:))]
            fn set_frame_size(&self, size: NSSize) {
                let _: () = unsafe { msg_send![super(self), setFrameSize: size] };
                self.reveal_selection();
            }
        }

        unsafe impl NSObjectProtocol for ListRows {}
    );

    impl ListRows {
        /// The highlighted row.
        pub fn selection(&self) -> usize {
            self.ivars().selection.get()
        }

        fn row_at(&self, y: f64) -> Option<usize> {
            let i = (y / self.ivars().row_h.get()).floor();
            let i = (i >= 0.0).then_some(i as usize + self.ivars().top.get())?;
            (i < self.ivars().count.get()).then_some(i)
        }

        /// Scroll so the selection is drawn (rows that fit the current height).
        fn reveal_selection(&self) {
            let rh = self.ivars().row_h.get();
            let visible = if rh > 0.0 { (self.bounds().size.height / rh).floor() as usize } else { 1 };
            let count = self.ivars().count.get();
            let top = scroll_top(self.ivars().top.get(), self.ivars().selection.get(), visible, count);
            if top != self.ivars().top.get() {
                self.ivars().top.set(top);
                self.setNeedsDisplay(true);
            }
        }
    }

    impl ListRows {
        /// New row count / selection (for owners that keep only the rows).
        pub fn set_count(&self, count: usize, selection: usize) {
            self.ivars().count.set(count);
            self.ivars().selection.set(selection);
            self.reveal_selection();
            self.setNeedsDisplay(true);
        }
    }

    /// The built panel pieces.
    pub struct ListPanel {
        pub root: Retained<NSView>,
        pub search: Retained<NSTextField>,
        pub rows: Retained<ListRows>,
        pub row_h: f64,
        pub delegate: RefCell<Option<Retained<crate::ui::field::FieldDelegate>>>,
    }

    impl ListPanel {
        pub fn new(mtm: MainThreadMarker, colors: PopupColors, width: f64, row_h: f64, placeholder: &str) -> ListPanel {
            let h = panel_height(1, row_h, 1);
            let frame = crate::ui::panel_frame::build(
                mtm,
                width,
                h,
                crate::ui::panel_frame::PanelSpec {
                    colors,
                    header_color: None,
                    title: None,
                    meta: "",
                    buttons: Vec::new(),
                    icon: None,
                    tint_alpha: 0.86,
                },
            );
            frame.chrome.setHidden(true);
            frame.content.setHidden(true);
            let search = NSTextField::new(mtm);
            search.setBezeled(false);
            search.setBordered(false);
            search.setDrawsBackground(false);
            search.setFocusRingType(NSFocusRingType::None);
            search.setFont(Some(&NSFont::systemFontOfSize(15.0)));
            search.setTextColor(Some(&colors.text.to_nscolor()));
            crate::ui::field::set_themed_placeholder(&search, &colors, 15.0, placeholder);
            search.setFrame(NSRect::new(NSPoint::new(18.0, 12.0), NSSize::new(width - 36.0, 22.0)));
            search.setAutoresizingMask(objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable);
            frame.root.addSubview(&search);
            let rows = ListRows::alloc(mtm).set_ivars(RowsIvars {
                count: Cell::new(0),
                selection: Cell::new(0),
                row_h: Cell::new(row_h),
                hover: Cell::new(None),
                drawer: RefCell::new(None),
                on_click: RefCell::new(None),
                tracking: RefCell::new(None),
                top: Cell::new(0),
            });
            let rows: Retained<ListRows> = unsafe {
                msg_send![super(rows), initWithFrame: NSRect::new(NSPoint::new(0.0, SEARCH_H), NSSize::new(width, row_h))]
            };
            rows.setAutoresizingMask(
                objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable | objc2_app_kit::NSAutoresizingMaskOptions::ViewHeightSizable,
            );
            frame.root.addSubview(&rows);
            ListPanel { root: frame.root, search, rows, row_h, delegate: RefCell::new(None) }
        }

        pub fn set_drawer(&self, d: Box<dyn Fn(Rect, usize, bool, bool)>) {
            *self.rows.ivars().drawer.borrow_mut() = Some(d);
        }

        pub fn set_on_click(&self, cb: Box<dyn Fn(usize, f64, bool)>) {
            *self.rows.ivars().on_click.borrow_mut() = Some(cb);
        }

        /// Typing + field commands (`insertNewline:` …; return true = handled).
        pub fn set_on_field(&self, on_change: Box<dyn Fn(String)>, on_command: Box<dyn Fn(&str) -> bool>) {
            let Some(mtm) = MainThreadMarker::new() else { return };
            *self.delegate.borrow_mut() =
                Some(crate::ui::field::FieldDelegate::attach(mtm, &self.search, on_change, on_command));
        }

        /// New rows / selection; the caller resizes the window to `height()`.
        pub fn set_rows(&self, count: usize, selection: usize) {
            self.rows.set_count(count, selection);
            self.rows.setNeedsDisplay(true);
        }

        pub fn height(&self, count: usize, max_rows: usize) -> f64 {
            panel_height(count, self.row_h, max_rows)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_top_keeps_the_selection_in_view() {
        // 15 rows, 12 fit: moving down past the 12th scrolls one row at a time.
        assert_eq!(scroll_top(0, 11, 12, 15), 0);
        assert_eq!(scroll_top(0, 12, 12, 15), 1);
        assert_eq!(scroll_top(1, 14, 12, 15), 3);
        // Back up above the window, and a shrunk list never leaves a gap.
        assert_eq!(scroll_top(3, 2, 12, 15), 2);
        assert_eq!(scroll_top(3, 3, 12, 5), 0);
        assert_eq!(scroll_top(0, 0, 0, 0), 0);
    }

    #[test]
    fn height_clamps_rows() {
        assert_eq!(panel_height(0, 30.0, 10), SEARCH_H + 30.0 + PAD_BOTTOM);
        assert_eq!(panel_height(50, 30.0, 10), SEARCH_H + 300.0 + PAD_BOTTOM);
    }
}
