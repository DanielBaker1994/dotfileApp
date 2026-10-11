//! A list popup in Swift's `PopupWindow` look (the palette, the view
//! switcher): the themed card, a bare search line at the top (large caret, no
//! box), then custom-drawn rows with hover + click. The owner draws each row
//! through a closure and handles typing / commands / clicks.

use crate::ui::chrome::Rect;

pub const SEARCH_H: f64 = 44.0;
pub const PAD_BOTTOM: f64 = 6.0;

/// What the panel reports.
#[derive(Clone, Debug, PartialEq)]
pub enum ListEvent {
    Query(String),
    /// A field command (`insertNewline:`, `moveUp:`, …) — return true when
    /// handled from the command closure instead.
    Click { row: usize, x: f64, double: bool },
}

/// Height for `n` rows of `row_h`.
pub fn panel_height(n: usize, row_h: f64, max_rows: usize) -> f64 {
    SEARCH_H + n.clamp(1, max_rows.max(1)) as f64 * row_h + PAD_BOTTOM
}

pub fn row_rect(i: usize, row_h: f64, width: f64) -> Rect {
    Rect::new(0.0, i as f64 * row_h, width, row_h)
}

#[cfg(target_os = "macos")]
pub use appkit::{ListPanel, ListRows};

#[cfg(target_os = "macos")]
mod appkit {
    use super::*;
    use crate::ui::theme::PopupColors;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{define_class, msg_send, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, Message};
    use objc2_app_kit::{NSEvent, NSFocusRingType, NSFont, NSTextField, NSTrackingArea, NSTrackingAreaOptions, NSView};
    use objc2_foundation::{NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};
    use std::cell::{Cell, RefCell};

    fn as_any<T: Message + ?Sized>(obj: &T) -> &AnyObject {
        unsafe { &*(obj as *const T as *const AnyObject) }
    }

    type Drawer = Box<dyn Fn(Rect, usize, bool, bool)>;

    pub struct RowsIvars {
        count: Cell<usize>,
        selection: Cell<usize>,
        row_h: Cell<f64>,
        hover: Cell<Option<usize>>,
        drawer: RefCell<Option<Drawer>>,
        on_click: RefCell<Option<Box<dyn Fn(usize, f64, bool)>>>,
        tracking: RefCell<Option<Retained<NSTrackingArea>>>,
    }

    define_class!(
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSListRows"]
        #[ivars = RowsIvars]
        pub struct ListRows;

        impl ListRows {
            #[unsafe(method(isFlipped))]
            fn is_flipped(&self) -> bool { true }
            #[unsafe(method(mouseDownCanMoveWindow))]
            fn mdcmw(&self) -> bool { false }
            #[unsafe(method(acceptsFirstMouse:))]
            fn afm(&self, _e: Option<&NSEvent>) -> bool { true }

            #[unsafe(method(updateTrackingAreas))]
            fn uta(&self) {
                let _: () = unsafe { msg_send![super(self), updateTrackingAreas] };
                if let Some(t) = self.ivars().tracking.borrow_mut().take() {
                    self.removeTrackingArea(&t);
                }
                let opts = NSTrackingAreaOptions::MouseMoved
                    | NSTrackingAreaOptions::MouseEnteredAndExited
                    | NSTrackingAreaOptions::ActiveAlways
                    | NSTrackingAreaOptions::InVisibleRect;
                let t = unsafe {
                    NSTrackingArea::initWithRect_options_owner_userInfo(NSTrackingArea::alloc(), self.bounds(), opts, Some(as_any(self)), None)
                };
                self.addTrackingArea(&t);
                *self.ivars().tracking.borrow_mut() = Some(t);
            }

            #[unsafe(method(mouseMoved:))]
            fn mouse_moved(&self, e: &NSEvent) {
                let p = self.convertPoint_fromView(e.locationInWindow(), None);
                let h = self.row_at(p.y);
                if h != self.ivars().hover.get() {
                    self.ivars().hover.set(h);
                    self.setNeedsDisplay(true);
                }
            }

            #[unsafe(method(mouseExited:))]
            fn mouse_exited(&self, _e: &NSEvent) {
                self.ivars().hover.set(None);
                self.setNeedsDisplay(true);
            }

            #[unsafe(method(mouseDown:))]
            fn mouse_down(&self, e: &NSEvent) {
                let p = self.convertPoint_fromView(e.locationInWindow(), None);
                let Some(row) = self.row_at(p.y) else { return };
                if let Some(cb) = self.ivars().on_click.borrow().as_ref() {
                    cb(row, p.x, e.clickCount() >= 2);
                }
            }

            #[unsafe(method(drawRect:))]
            fn draw_rect(&self, _d: NSRect) {
                let w = self.bounds().size.width;
                let rh = self.ivars().row_h.get();
                let sel = self.ivars().selection.get();
                let hov = self.ivars().hover.get();
                if let Some(d) = self.ivars().drawer.borrow().as_ref() {
                    for i in 0..self.ivars().count.get() {
                        d(row_rect(i, rh, w), i, i == sel, hov == Some(i));
                    }
                }
            }
        }

        unsafe impl NSObjectProtocol for ListRows {}
    );

    impl ListRows {
        fn row_at(&self, y: f64) -> Option<usize> {
            let i = (y / self.ivars().row_h.get()).floor();
            (i >= 0.0 && (i as usize) < self.ivars().count.get()).then_some(i as usize)
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
            search.setPlaceholderString(Some(&NSString::from_str(placeholder)));
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
            self.rows.ivars().count.set(count);
            self.rows.ivars().selection.set(selection);
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
    fn height_clamps_rows() {
        assert_eq!(panel_height(0, 30.0, 10), SEARCH_H + 30.0 + PAD_BOTTOM);
        assert_eq!(panel_height(50, 30.0, 10), SEARCH_H + 300.0 + PAD_BOTTOM);
    }
}
