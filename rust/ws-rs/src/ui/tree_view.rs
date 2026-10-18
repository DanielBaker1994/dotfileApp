//! A compact, scrolling file-tree card: disclosure triangles, indent per depth,
//! hover pills, a selected row, wheel scrolling and a right-click hook. The
//! notes window stacks it above the "Open" sidebar in the left rail
//! (`views::notes_tree` is its model).

use crate::ui::chrome::Rect;
use crate::views::notes_tree::TreeRow;

const ROW_H: f64 = 22.0;
const HEAD_H: f64 = 24.0;

/// What a click asks the owner to do.
#[derive(Clone, Debug, PartialEq)]
pub enum TreeEvent {
    /// The PINNED header: collapse / expand the whole tree card.
    ToggleSection,
    /// Left click on a row (the owner toggles a folder / opens a file).
    Click { path: String, is_dir: bool },
    /// Right click (screen-space point for a menu).
    Menu { path: String, is_dir: bool, is_root: bool, x: f64, y: f64 },
}

/// Geometry for a card of `width` x `height` holding `n` rows.
#[derive(Clone, Copy, Debug)]
pub struct TreeGeometry {
    pub card: Rect,
    pub head: Rect,
    pub list: Rect,
}

impl TreeGeometry {
    pub fn new(width: f64, height: f64) -> Self {
        let card = Rect::new(8.0, 6.0, (width - 8.0).max(0.0), (height - 8.0).max(0.0));
        let head = Rect::new(card.x, card.y + 6.0, card.width, HEAD_H);
        let top = head.max_y() + 2.0;
        let list = Rect::new(card.x + 6.0, top, (card.width - 12.0).max(0.0), (card.max_y() - 6.0 - top).max(0.0));
        TreeGeometry { card, head, list }
    }

    /// Card height with the tree collapsed to its header.
    pub fn collapsed_height() -> f64 {
        let g = Self::new(100.0, 0.0);
        g.head.max_y() + 6.0
    }

    /// Card height that fits `n` rows exactly (no empty space below).
    pub fn height_for(n: usize) -> f64 {
        let g = Self::new(100.0, 0.0);
        g.list.y + n as f64 * ROW_H + 8.0
    }

    pub fn content_height(n: usize) -> f64 {
        n as f64 * ROW_H
    }

    pub fn row_rect(&self, i: usize, scroll: f64) -> Rect {
        Rect::new(self.list.x, self.list.y + i as f64 * ROW_H - scroll, self.list.width, ROW_H)
    }

    pub fn clamp_scroll(&self, scroll: f64, n: usize) -> f64 {
        scroll.min(Self::content_height(n) - self.list.height).max(0.0)
    }

    /// Row under a point in view coordinates.
    pub fn hit(&self, n: usize, scroll: f64, x: f64, y: f64) -> Option<usize> {
        if !self.list.contains(x, y) {
            return None;
        }
        let i = ((y - self.list.y + scroll) / ROW_H).floor();
        (i >= 0.0 && (i as usize) < n).then_some(i as usize)
    }

    /// Scroll so row `i` is fully visible.
    pub fn reveal(&self, i: usize, scroll: f64, n: usize) -> f64 {
        let top = i as f64 * ROW_H;
        let mut s = scroll;
        if top < s {
            s = top;
        } else if top + ROW_H > s + self.list.height {
            s = top + ROW_H - self.list.height;
        }
        self.clamp_scroll(s, n)
    }
}

#[cfg(target_os = "macos")]
pub use appkit::TreeView;

#[cfg(target_os = "macos")]
mod appkit {
    use super::*;
    use crate::ui::sidebar::icon_for_name;
    use crate::ui::style::{self, Weight};
    use crate::ui::theme::PopupColors;
    use objc2::rc::Retained;
    
    use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{NSBezierPath, NSEvent, NSGraphicsContext, NSTrackingArea, NSView};
    use objc2_foundation::{NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};
    use std::cell::{Cell, RefCell};

    use crate::ui::view_base::{self};

    pub struct TreeIvars {
        rows: RefCell<Vec<TreeRow>>,
        selected: RefCell<Option<String>>,
        colors: Cell<PopupColors>,
        scroll: Cell<f64>,
        hover: Cell<Option<usize>>,
        indent: Cell<f64>,
        on_event: RefCell<Option<Box<dyn Fn(TreeEvent)>>>,
        tracking: RefCell<Option<Retained<NSTrackingArea>>>,
        collapsed: Cell<bool>,
        /// Whether the tree holds the keyboard (Ctrl+H focus), and its cursor row.
        key_focused: Cell<bool>,
        cursor: Cell<usize>,
    }

    define_class!(
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSTreeView"]
        #[ivars = TreeIvars]
        pub struct TreeView;

        impl TreeView {
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

            #[unsafe(method(acceptsFirstResponder))]
            fn accepts_first_responder(&self) -> bool {
                true
            }

            #[unsafe(method(becomeFirstResponder))]
            fn become_first_responder(&self) -> bool {
                let start = {
                    let rows = self.ivars().rows.borrow();
                    let sel = self.ivars().selected.borrow();
                    sel.as_deref().and_then(|p| rows.iter().position(|r| r.path == p)).unwrap_or(0)
                };
                self.ivars().cursor.set(start);
                self.ivars().key_focused.set(true);
                self.reveal_row(start);
                true
            }

            #[unsafe(method(resignFirstResponder))]
            fn resign_first_responder(&self) -> bool {
                self.ivars().key_focused.set(false);
                self.setNeedsDisplay(true);
                true
            }

            #[unsafe(method(keyDown:))]
            fn key_down(&self, e: &NSEvent) {
                if !self.handle_nav_key(e) {
                    let _: () = unsafe { msg_send![super(self), keyDown: e] };
                }
            }

            #[unsafe(method(drawRect:))]
            fn draw_rect(&self, _dirty: NSRect) {
                self.draw();
            }

            #[unsafe(method(setFrameSize:))]
            fn set_frame_size(&self, size: NSSize) {
                let _: () = unsafe { msg_send![super(self), setFrameSize: size] };
                let n = self.ivars().rows.borrow().len();
                let g = self.geometry();
                self.ivars().scroll.set(g.clamp_scroll(self.ivars().scroll.get(), n));
            }

            #[unsafe(method(updateTrackingAreas))]
            fn update_tracking_areas(&self) {
                let _: () = unsafe { msg_send![super(self), updateTrackingAreas] };
                view_base::reset_tracking(self, &self.ivars().tracking, view_base::hover_opts());
            }

            #[unsafe(method(mouseMoved:))]
            fn mouse_moved(&self, e: &NSEvent) {
                let over = self.row_at(e);
                let tip = over.and_then(|i| self.ivars().rows.borrow().get(i).map(|r| r.path.clone()));
                let cur: Option<String> = self.toolTip().map(|t| t.to_string());
                if cur != tip {
                    self.setToolTip(tip.as_deref().map(NSString::from_str).as_deref());
                }
                view_base::set_hover(self, &self.ivars().hover, over);
            }

            #[unsafe(method(mouseExited:))]
            fn mouse_exited(&self, _e: &NSEvent) {
                view_base::set_hover(self, &self.ivars().hover, None);
            }

            #[unsafe(method(scrollWheel:))]
            fn scroll_wheel(&self, e: &NSEvent) {
                let dy = if e.hasPreciseScrollingDeltas() { e.scrollingDeltaY() } else { e.scrollingDeltaY() * 10.0 };
                let n = self.ivars().rows.borrow().len();
                let g = self.geometry();
                self.ivars().scroll.set(g.clamp_scroll(self.ivars().scroll.get() - dy, n));
                self.setNeedsDisplay(true);
            }

            #[unsafe(method(mouseDown:))]
            fn mouse_down(&self, e: &NSEvent) {
                let p = self.convertPoint_fromView(e.locationInWindow(), None);
                if self.geometry().head.contains(p.x, p.y) {
                    self.emit(TreeEvent::ToggleSection);
                    return;
                }
                let Some(i) = self.row_at(e) else { return };
                let ev = self.ivars().rows.borrow().get(i).map(|r| TreeEvent::Click { path: r.path.clone(), is_dir: r.is_dir });
                if let Some(ev) = ev {
                    self.emit(ev);
                }
            }

            #[unsafe(method(rightMouseDown:))]
            fn right_mouse_down(&self, e: &NSEvent) {
                let Some(i) = self.row_at(e) else { return };
                let s = NSEvent::mouseLocation();
                let ev = self.ivars().rows.borrow().get(i).map(|r| TreeEvent::Menu {
                    path: r.path.clone(),
                    is_dir: r.is_dir,
                    is_root: r.is_root,
                    x: s.x,
                    y: s.y,
                });
                if let Some(ev) = ev {
                    self.emit(ev);
                }
            }
        }

        unsafe impl NSObjectProtocol for TreeView {}
    );

    impl TreeView {
        pub fn new(mtm: MainThreadMarker, colors: PopupColors) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(TreeIvars {
                rows: RefCell::new(Vec::new()),
                selected: RefCell::new(None),
                colors: Cell::new(colors),
                scroll: Cell::new(0.0),
                hover: Cell::new(None),
                indent: Cell::new(14.0),
                on_event: RefCell::new(None),
                tracking: RefCell::new(None),
                collapsed: Cell::new(crate::views::notes_tree::load_tree_collapsed()),
                key_focused: Cell::new(false),
                cursor: Cell::new(0),
            });
            unsafe {
                msg_send![super(this), initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(240.0, 300.0))]
            }
        }

        pub fn set_on_event(&self, cb: Box<dyn Fn(TreeEvent)>) {
            *self.ivars().on_event.borrow_mut() = Some(cb);
        }

        fn emit(&self, ev: TreeEvent) {
            // Take the callback out while it runs so the owner can call
            // `set_rows` back from inside it without a double borrow.
            let cb = self.ivars().on_event.borrow_mut().take();
            if let Some(cb) = cb {
                cb(ev);
                let mut slot = self.ivars().on_event.borrow_mut();
                if slot.is_none() {
                    *slot = Some(cb);
                }
            }
        }

        pub fn set_colors(&self, c: PopupColors) {
            self.ivars().colors.set(c);
            self.setNeedsDisplay(true);
        }

        pub fn set_indent(&self, indent: f64) {
            self.ivars().indent.set(indent.clamp(6.0, 40.0));
            self.setNeedsDisplay(true);
        }

        /// Replace the rows, keeping the scroll position (clamped).
        pub fn set_rows(&self, rows: Vec<TreeRow>, selected: Option<String>) {
            let n = rows.len();
            *self.ivars().rows.borrow_mut() = rows;
            *self.ivars().selected.borrow_mut() = selected;
            let g = self.geometry();
            self.ivars().scroll.set(g.clamp_scroll(self.ivars().scroll.get(), n));
            self.setNeedsDisplay(true);
        }

        /// Scroll row `i` into view.
        pub fn reveal_row(&self, i: usize) {
            let n = self.ivars().rows.borrow().len();
            let g = self.geometry();
            self.ivars().scroll.set(g.reveal(i, self.ivars().scroll.get(), n));
            self.setNeedsDisplay(true);
        }

        pub fn key_focused(&self) -> bool {
            self.ivars().key_focused.get()
        }

        /// The row under the keyboard cursor: (path, is_dir, is_root).
        pub fn cursor_row(&self) -> Option<(String, bool, bool)> {
            let rows = self.ivars().rows.borrow();
            rows.get(self.ivars().cursor.get()).map(|r| (r.path.clone(), r.is_dir, r.is_root))
        }

        /// Scroll `path`'s row into view and return the rect its name is drawn
        /// in (view coordinates) — where an inline rename field goes.
        pub fn name_rect(&self, path: &str) -> Option<NSRect> {
            if self.collapsed() {
                return None;
            }
            let (i, depth) = {
                let rows = self.ivars().rows.borrow();
                let i = rows.iter().position(|r| r.path == path)?;
                (i, rows[i].depth)
            };
            self.reveal_row(i);
            let r = self.geometry().row_rect(i, self.ivars().scroll.get());
            let tx = r.x + 4.0 + depth as f64 * self.ivars().indent.get() + 34.0;
            Some(NSRect::new(NSPoint::new(tx, r.y), NSSize::new((r.max_x() - 6.0 - tx).max(80.0), r.height)))
        }

        /// Keys while the tree has the keyboard: the rail keys (↑↓ / Ctrl+N/P /
        /// Home / End / Page / Return / Space / Esc) plus ← → to collapse or
        /// expand a folder. Return on a file opens it and moves focus right.
        pub fn handle_nav_key(&self, e: &NSEvent) -> bool {
            use crate::ui::sidebar::{leave_right, nav_key, NavKeyResult};
            use objc2_app_kit::NSEventModifierFlags as F;
            if !self.key_focused() {
                return false;
            }
            let mods = e.modifierFlags();
            let plain = !mods.contains(F::Control) && !mods.contains(F::Command) && !mods.contains(F::Option) && !mods.contains(F::Shift);
            let n = self.ivars().rows.borrow().len();
            if self.collapsed() {
                // Only the header is left: Return / Space / → opens it again.
                return match e.keyCode() {
                    36 | 76 | 49 | 124 if plain => {
                        self.emit(TreeEvent::ToggleSection);
                        true
                    }
                    53 if plain => {
                        leave_right();
                        true
                    }
                    _ => false,
                };
            }
            let cur = self.ivars().cursor.get().min(n.saturating_sub(1));
            let row = self.ivars().rows.borrow().get(cur).cloned();
            if plain && (e.keyCode() == 123 || e.keyCode() == 124) {
                if let Some(r) = row.filter(|r| r.is_dir) {
                    if r.expanded == (e.keyCode() == 123) {
                        self.emit(TreeEvent::Click { path: r.path, is_dir: true });
                    }
                }
                return true;
            }
            let ch = e.charactersIgnoringModifiers().and_then(|s| s.to_string().chars().next());
            let page = (self.geometry().list.height / ROW_H) as usize;
            let titles = |i: usize| -> String { self.ivars().rows.borrow().get(i).map(|r| r.name.clone()).unwrap_or_default() };
            let (res, next) = nav_key(
                e.keyCode(),
                mods.contains(F::Control),
                mods.contains(F::Command),
                mods.contains(F::Option),
                mods.contains(F::Shift),
                ch,
                cur,
                n,
                page.saturating_sub(1),
                &titles,
            );
            self.ivars().cursor.set(next);
            match res {
                NavKeyResult::Pass => return false,
                NavKeyResult::Moved => self.reveal_row(next),
                NavKeyResult::Activate { row: i, leave } => {
                    let r = self.ivars().rows.borrow().get(i).cloned();
                    if let Some(r) = r {
                        let is_dir = r.is_dir;
                        self.emit(TreeEvent::Click { path: r.path, is_dir });
                        if leave && !is_dir && self.key_focused() {
                            leave_right();
                        }
                    }
                }
                // Rows are not closable; Backspace / Delete are ignored.
                NavKeyResult::Close(_) => {}
                NavKeyResult::Leave => leave_right(),
            }
            self.setNeedsDisplay(true);
            true
        }

        pub fn collapsed(&self) -> bool {
            self.ivars().collapsed.get()
        }

        pub fn set_collapsed(&self, on: bool) {
            self.ivars().collapsed.set(on);
            self.ivars().hover.set(None);
            crate::views::notes_tree::save_tree_collapsed(on);
            self.setNeedsDisplay(true);
        }

        /// The card height this tree wants: header only when collapsed, else
        /// exactly its rows.
        pub fn wanted_height(&self) -> f64 {
            if self.collapsed() {
                TreeGeometry::collapsed_height()
            } else {
                TreeGeometry::height_for(self.row_count())
            }
        }

        pub fn row_count(&self) -> usize {
            self.ivars().rows.borrow().len()
        }

        fn geometry(&self) -> TreeGeometry {
            let b = self.bounds();
            TreeGeometry::new(b.size.width, b.size.height)
        }

        fn row_at(&self, e: &NSEvent) -> Option<usize> {
            let p = self.convertPoint_fromView(e.locationInWindow(), None);
            if self.collapsed() {
                return None;
            }
            let n = self.ivars().rows.borrow().len();
            self.geometry().hit(n, self.ivars().scroll.get(), p.x, p.y)
        }

        fn draw(&self) {
            let c = self.ivars().colors.get();
            let g = self.geometry();
            if g.card.width <= 0.0 || g.card.height <= 0.0 {
                return;
            }
            // One pane with the Open list below: the fill runs past the view so
            // the clipped bottom corners are square, then a hairline divides them.
            let bounds = self.bounds();
            NSGraphicsContext::saveGraphicsState_class();
            NSBezierPath::clipRect(bounds);
            style::fill_rounded(Rect::new(g.card.x, g.card.y, g.card.width, g.card.height + 20.0), 12.0, c.mantle());
            style::fill_rect(Rect::new(g.card.x + 8.0, bounds.size.height - 1.0, (g.card.width - 16.0).max(0.0), 1.0), c.surface0());
            NSGraphicsContext::restoreGraphicsState_class();
            let hf = style::system_font(10.0, Weight::Semibold);
            let collapsed = self.ivars().collapsed.get();
            style::draw_symbol(
                if collapsed { "chevron.right" } else { "chevron.down" },
                Rect::new(g.head.x + 10.0, g.head.y, 12.0, g.head.height),
                c.dim,
                8.0,
            );
            style::draw_text_left("PINNED", Rect::new(g.head.x + 26.0, g.head.y, g.head.width - 32.0, g.head.height), &hf, c.dim);
            if collapsed {
                return;
            }
            let rows = self.ivars().rows.borrow();
            let selected = self.ivars().selected.borrow();
            let hover = self.ivars().hover.get();
            let scroll = self.ivars().scroll.get();
            let indent = self.ivars().indent.get();
            NSGraphicsContext::saveGraphicsState_class();
            NSBezierPath::clipRect(style::ns(g.list));
            for (i, row) in rows.iter().enumerate() {
                let r = g.row_rect(i, scroll);
                if r.max_y() <= g.list.y || r.y >= g.list.max_y() {
                    continue;
                }
                let sel = selected.as_deref() == Some(row.path.as_str());
                if sel || hover == Some(i) {
                    style::fill_rounded(r, 4.0, if sel { c.surface1() } else { c.surface0() });
                }
                let x0 = r.x + 4.0 + row.depth as f64 * indent;
                if row.is_dir {
                    style::draw_symbol(
                        if row.expanded { "chevron.down" } else { "chevron.right" },
                        Rect::new(x0, r.y, 12.0, r.height),
                        c.dim,
                        8.0,
                    );
                }
                let icon = if row.is_dir { "folder" } else { icon_for_name(&row.name) };
                style::draw_symbol(icon, Rect::new(x0 + 14.0, r.y, 16.0, r.height), if sel { c.accent_on() } else { c.dim }, 11.0);
                let w = if sel || row.is_root { Weight::Semibold } else { Weight::Regular };
                let f = style::system_font(12.0, w);
                let tx = x0 + 34.0;
                style::draw_text_left(&row.name, Rect::new(tx, r.y, (r.max_x() - 6.0 - tx).max(0.0), r.height), &f, c.text);
            }
            if self.key_focused() {
                let r = g.row_rect(self.ivars().cursor.get(), scroll);
                if r.y >= g.list.y - 0.5 && r.max_y() <= g.list.max_y() + 0.5 {
                    let ring = crate::panes::pane_nav::RingStyle::default().rgba();
                    style::stroke_rounded(r.inset_by(0.5, 0.5), 4.0, 1.0, ring.with_alpha((ring.a + 0.25).min(1.0)));
                }
            }
            NSGraphicsContext::restoreGraphicsState_class();
            let content = TreeGeometry::content_height(rows.len());
            if content > g.list.height + 1.0 {
                let h = (g.list.height * g.list.height / content).max(20.0);
                let y = g.list.y + (g.list.height - h) * (scroll / (content - g.list.height).max(1.0));
                style::fill_rounded(Rect::new(g.card.max_x() - 4.0, y, 2.5, h), 1.25, c.dim.with_alpha(0.35));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_maps_points_to_rows_with_scroll() {
        let g = TreeGeometry::new(240.0, 300.0);
        let y0 = g.list.y;
        assert_eq!(g.hit(10, 0.0, g.list.x + 5.0, y0 + 1.0), Some(0));
        assert_eq!(g.hit(10, 0.0, g.list.x + 5.0, y0 + ROW_H * 2.5), Some(2));
        assert_eq!(g.hit(10, ROW_H, g.list.x + 5.0, y0 + 1.0), Some(1));
        assert_eq!(g.hit(2, 0.0, g.list.x + 5.0, y0 + ROW_H * 5.0), None);
        assert_eq!(g.hit(10, 0.0, 0.5, y0 + 1.0), None);
    }

    #[test]
    fn height_for_fits_rows_exactly() {
        let h = TreeGeometry::height_for(10);
        let g = TreeGeometry::new(240.0, h);
        assert!(g.list.height >= 10.0 * ROW_H - 0.001);
        assert!(g.list.height < 10.0 * ROW_H + 8.0);
    }

    #[test]
    fn reveal_keeps_row_in_view() {
        let g = TreeGeometry::new(240.0, 200.0);
        let s = g.reveal(39, 0.0, 40);
        assert!(g.row_rect(39, s).max_y() <= g.list.max_y() + 0.001);
        assert_eq!(g.reveal(0, s, 40), 0.0);
    }
}
