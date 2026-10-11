//! The vertical sidebar — a port of Swift's `PopupTabsBar` with
//! `vertical = true` (PopupWindow.swift): the rounded mantle card, an
//! uppercase section label, optional pinned rows (grouped by section), the
//! icon rows with a right-aligned meta / badge, the collapse-to-rail toggle
//! at the bottom-left with an optional status dot + line, hover pills,
//! wheel scrolling and an "+" add button. Files (PINNED / PLACES), Jira (MY
//! WORK / PINNED / BOARDS / SYNCED), Confluence, Compare, AI (RULES) and
//! notes all draw their left column through this one view.

use crate::ui::chrome::Rect;
use crate::ui::theme::PopupTone;

/// `PopupTabsBar.railWidth`.
pub const RAIL_WIDTH: f64 = 50.0;
const ROW_H: f64 = 34.0;
const PIN_ROW_H: f64 = 30.0;
const HEAD_H: f64 = 24.0;

/// One pinned row (`pinned` + `pinnedLabel` / `pinnedIconFor` /
/// `pinnedSection` / `pinnedMeta` / `pinnedTip`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SidebarPin {
    pub key: String,
    pub label: String,
    pub icon: String,
    pub section: String,
    pub meta: String,
    pub tip: String,
}

/// One list row (`titles[i]` + `rowIcon` / `vMeta` / `badges[i]` / `pathTip`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SidebarRow {
    pub title: String,
    pub icon: String,
    pub meta: String,
    pub badge: Option<(String, PopupTone)>,
    pub tip: String,
}

impl SidebarRow {
    pub fn new(title: &str, icon: &str) -> Self {
        SidebarRow { title: title.to_string(), icon: icon.to_string(), ..Default::default() }
    }

    pub fn meta(mut self, m: &str) -> Self {
        self.meta = m.to_string();
        self
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SidebarModel {
    pub section_title: String,
    pub rows: Vec<SidebarRow>,
    pub selected: Option<usize>,
    pub pinned: Vec<SidebarPin>,
    pub pinned_selected: Option<String>,
    pub add_button: bool,
    pub closable: bool,
    /// The status dot + text beside the rail toggle (`statusLine`).
    pub status: Option<(String, PopupTone)>,
}

/// What a click asks the owner to do.
#[derive(Clone, Debug, PartialEq)]
pub enum SidebarEvent {
    Select(usize),
    Pinned(String),
    Add,
    Close(usize),
    /// The rail toggle (the owner relayouts; the view already flipped).
    Rail(bool),
    /// Right-click on a row / pinned key (screen-space point for a menu).
    MenuRow(usize, f64, f64),
    MenuPinned(String, f64, f64),
}

/// `handleNavKey`'s outcome for a key while the rail has the keyboard.
#[derive(Clone, Debug, PartialEq)]
pub enum NavKeyResult {
    /// Not a rail key — let it pass.
    Pass,
    /// Cursor moved (or the key was swallowed); just redraw.
    Moved,
    /// Activate the row under the cursor, then optionally leave right.
    Activate { row: usize, leave: bool },
    /// Close the (non-pinned) row under the cursor.
    Close(usize),
    Leave,
}

/// `PopupTabsBar.handleNavKey` over `n` rows (pinned first), cursor `cur`,
/// `page` rows per page. `titles` gives each row's title for letter jumps.
/// Returns the outcome and the new cursor.
pub fn nav_key(
    key_code: u16,
    ctrl: bool,
    cmd: bool,
    opt: bool,
    shift: bool,
    ch: Option<char>,
    cur: usize,
    n: usize,
    page: usize,
    titles: &dyn Fn(usize) -> String,
) -> (NavKeyResult, usize) {
    let clamp = |r: i64| -> usize { r.clamp(0, n.saturating_sub(1) as i64) as usize };
    let plain = !ctrl && !cmd && !opt && !shift;
    let c = cur as i64;
    let page = page.max(1) as i64;
    let moved = |to: i64| (NavKeyResult::Moved, if n == 0 { 0 } else { clamp(to) });
    match key_code {
        125 if plain => moved(c + 1),
        45 if ctrl && !cmd && !opt && !shift => moved(c + 1),
        126 if plain => moved(c - 1),
        35 if ctrl && !cmd && !opt && !shift => moved(c - 1),
        115 if plain => moved(0),
        126 if cmd && !ctrl && !opt && !shift => moved(0),
        119 if plain => moved(n as i64 - 1),
        125 if cmd && !ctrl && !opt && !shift => moved(n as i64 - 1),
        116 if plain => moved(c - page),
        121 if plain => moved(c + page),
        36 | 76 if plain => {
            if n == 0 {
                (NavKeyResult::Leave, cur)
            } else {
                (NavKeyResult::Activate { row: cur, leave: true }, cur)
            }
        }
        49 if plain => {
            if n == 0 {
                (NavKeyResult::Moved, cur)
            } else {
                (NavKeyResult::Activate { row: cur, leave: false }, cur)
            }
        }
        53 if plain => (NavKeyResult::Leave, cur),
        51 | 117 if !ctrl && !opt && !shift => (NavKeyResult::Close(cur), cur),
        _ => {
            if cmd || ctrl || opt || key_code == 48 {
                return (NavKeyResult::Pass, cur);
            }
            let Some(ch) = ch.map(|c| c.to_ascii_lowercase()).filter(|c| c.is_alphanumeric()) else {
                return (NavKeyResult::Moved, cur);
            };
            for step in 1..=n {
                let r = (cur + step) % n;
                if titles(r).to_lowercase().starts_with(ch) {
                    return (NavKeyResult::Moved, r);
                }
            }
            (NavKeyResult::Moved, cur)
        }
    }
}

/// Hit ids: row index (>= 0), -2 add, -4 toggle, -10-k pinned k.
pub const HIT_ADD: i64 = -2;
pub const HIT_TOGGLE: i64 = -4;

/// The `PopupTabsBar` vertical geometry for a given bounds / model / scroll.
#[derive(Clone, Debug)]
pub struct SidebarGeometry {
    pub card: Rect,
    pub pinned_heads: Vec<(Rect, String)>,
    pub pinned_rows: Vec<Rect>,
    pub head: Rect,
    pub add: Rect,
    pub list: Rect,
    pub toggle: Rect,
    pub collapsed: bool,
}

impl SidebarGeometry {
    pub fn new(width: f64, height: f64, m: &SidebarModel, collapsed: bool) -> Self {
        let card = Rect::new(8.0, 6.0, (width - 8.0).max(0.0), (height - 14.0).max(0.0));
        let mut pinned_heads = Vec::new();
        let mut pinned_rows = Vec::new();
        if !m.pinned.is_empty() {
            let h = Rect::new(card.x, card.y + 6.0, card.width, HEAD_H);
            let mut y = h.max_y() + 2.0;
            let mut last: Option<&str> = None;
            for p in &m.pinned {
                match last {
                    None => pinned_heads.push((h, p.section.clone())),
                    Some(l) if l != p.section => {
                        y += 6.0;
                        pinned_heads.push((Rect::new(card.x, y, card.width, HEAD_H), p.section.clone()));
                        y += HEAD_H + 2.0;
                    }
                    _ => {}
                }
                last = Some(&p.section);
                pinned_rows.push(Rect::new(card.x + 6.0, y, (card.width - 12.0).max(0.0), PIN_ROW_H));
                y += PIN_ROW_H + 1.0;
            }
        }
        let top = pinned_rows.last().map_or(card.y + 6.0, |r| r.max_y() + 10.0);
        let head = Rect::new(card.x, top, card.width, HEAD_H);
        let add = if collapsed {
            Rect::new(head.mid_x() - 11.0, head.mid_y() - 11.0, 22.0, 22.0)
        } else {
            Rect::new(head.max_x() - 8.0 - 22.0, head.mid_y() - 11.0, 22.0, 22.0)
        };
        let sz = 24.0;
        let toggle = Rect::new(
            if collapsed { card.mid_x() - sz / 2.0 } else { card.x + 8.0 },
            card.max_y() - 6.0 - sz,
            sz,
            sz,
        );
        let ltop = head.max_y() + 4.0;
        let list = Rect::new(
            card.x + 6.0,
            ltop,
            (card.width - 12.0).max(0.0),
            (toggle.y - 4.0 - ltop).max(0.0),
        );
        SidebarGeometry { card, pinned_heads, pinned_rows, head, add, list, toggle, collapsed }
    }

    pub fn content_height(n: usize) -> f64 {
        n as f64 * (ROW_H + 1.0)
    }

    pub fn row_rect(&self, i: usize, scroll: f64) -> Rect {
        Rect::new(self.list.x, self.list.y + i as f64 * (ROW_H + 1.0) - scroll, self.list.width, ROW_H)
    }

    pub fn close_rect(row: Rect) -> Rect {
        Rect::new(row.max_x() - 16.0 - 6.0, row.mid_y() - 8.0, 16.0, 16.0)
    }

    pub fn clamp_scroll(&self, scroll: f64, n: usize) -> f64 {
        scroll.min(Self::content_height(n) - self.list.height).max(0.0)
    }

    /// `vHit(_:)`.
    pub fn hit(&self, m: &SidebarModel, scroll: f64, x: f64, y: f64) -> Option<(i64, bool)> {
        if self.toggle.contains(x, y) {
            return Some((HIT_TOGGLE, false));
        }
        if m.add_button && self.add.contains(x, y) {
            return Some((HIT_ADD, false));
        }
        for (k, r) in self.pinned_rows.iter().enumerate() {
            if r.contains(x, y) {
                return Some((-10 - k as i64, false));
            }
        }
        if !self.list.contains(x, y) {
            return None;
        }
        for i in 0..m.rows.len() {
            let r = self.row_rect(i, scroll);
            if r.contains(x, y) {
                if self.collapsed {
                    return Some((i as i64, false));
                }
                let c = Self::close_rect(r).inset_by(-2.0, -2.0);
                return Some((i as i64, m.closable && c.contains(x, y)));
            }
        }
        None
    }

    /// Scroll so row `i` is fully visible (`revealSelected`).
    pub fn reveal(&self, i: usize, scroll: f64, n: usize) -> f64 {
        let top = i as f64 * (ROW_H + 1.0);
        let mut s = scroll;
        if top < s {
            s = top;
        } else if top + ROW_H > s + self.list.height {
            s = top + ROW_H - self.list.height;
        }
        self.clamp_scroll(s, n)
    }
}

/// `vIcon(_:)` — an SF Symbol from a file name's extension.
pub fn icon_for_name(name: &str) -> &'static str {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "md" | "markdown" | "txt" | "" => "doc.text",
        "json" | "toml" | "yaml" | "yml" | "cfg" | "conf" | "ini" | "plist" => "gearshape",
        "log" => "list.bullet.rectangle",
        "swift" | "py" | "js" | "ts" | "sh" | "rb" | "go" | "rs" | "c" | "h" => {
            "chevron.left.forwardslash.chevron.right"
        }
        _ => "doc",
    }
}

#[cfg(target_os = "macos")]
pub use appkit::{set_leave_right, SidebarView};

#[cfg(target_os = "macos")]
mod appkit {
    use super::*;
    use crate::ui::style::{self, Weight};
    use crate::ui::theme::{PopupColors, Rgba};
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{define_class, msg_send, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, Message};
    use objc2_app_kit::{NSEvent, NSGraphicsContext, NSBezierPath, NSTrackingArea, NSTrackingAreaOptions, NSView};
    use objc2_foundation::{NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};
    use std::cell::{Cell, RefCell};

    thread_local! {
        /// The host's `PaneNav.shared.move(.right)` (Return / Esc on the rail).
        static LEAVE_RIGHT: RefCell<Option<Box<dyn Fn()>>> = const { RefCell::new(None) };
    }

    /// Install the host's "focus the pane right of the rail" action.
    pub fn set_leave_right(f: Box<dyn Fn()>) {
        LEAVE_RIGHT.with(|l| *l.borrow_mut() = Some(f));
    }

    fn leave_right() {
        // Deferred: the host re-enters AppKit focus, never while a view borrow is held.
        dispatch2::DispatchQueue::main().exec_async(|| {
            LEAVE_RIGHT.with(|l| {
                if let Some(f) = l.borrow().as_ref() {
                    f();
                }
            })
        });
    }

    fn as_any<T: Message + ?Sized>(obj: &T) -> &AnyObject {
        unsafe { &*(obj as *const T as *const AnyObject) }
    }

    pub struct SidebarIvars {
        model: RefCell<SidebarModel>,
        colors: Cell<PopupColors>,
        collapsed: Cell<bool>,
        scroll: Cell<f64>,
        hover: Cell<Option<i64>>,
        hover_close: Cell<Option<usize>>,
        on_event: RefCell<Option<Box<dyn Fn(SidebarEvent)>>>,
        tracking: RefCell<Option<Retained<NSTrackingArea>>>,
        collapse_key: RefCell<String>,
        /// Whether the rail holds the keyboard (`keyFocused`).
        key_focused: Cell<bool>,
        /// The keyboard cursor over pinned rows then list rows (`cursor`).
        cursor: Cell<usize>,
    }

    define_class!(
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSSidebarView"]
        #[ivars = SidebarIvars]
        pub struct SidebarView;

        impl SidebarView {
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

            #[unsafe(method(drawRect:))]
            fn draw_rect(&self, _dirty: NSRect) {
                self.draw();
            }

            #[unsafe(method(acceptsFirstResponder))]
            fn accepts_first_responder(&self) -> bool {
                true
            }

            #[unsafe(method(becomeFirstResponder))]
            fn become_first_responder(&self) -> bool {
                let start = {
                    let m = self.ivars().model.borrow();
                    match (&m.pinned_selected, m.selected) {
                        (Some(k), _) => m.pinned.iter().position(|p| &p.key == k).unwrap_or(0),
                        (None, Some(i)) => m.pinned.len() + i,
                        _ => 0,
                    }
                };
                self.ivars().cursor.set(start);
                self.ivars().key_focused.set(true);
                self.setNeedsDisplay(true);
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

            #[unsafe(method(setFrameSize:))]
            fn set_frame_size(&self, size: NSSize) {
                let _: () = unsafe { msg_send![super(self), setFrameSize: size] };
                let (n, sel) = {
                    let m = self.ivars().model.borrow();
                    (m.rows.len(), m.selected)
                };
                let g = self.geometry();
                let mut s = g.clamp_scroll(self.ivars().scroll.get(), n);
                if let Some(i) = sel {
                    s = g.reveal(i, s, n);
                }
                self.ivars().scroll.set(s);
            }

            #[unsafe(method(updateTrackingAreas))]
            fn update_tracking_areas(&self) {
                let _: () = unsafe { msg_send![super(self), updateTrackingAreas] };
                if let Some(t) = self.ivars().tracking.borrow_mut().take() {
                    self.removeTrackingArea(&t);
                }
                let opts = NSTrackingAreaOptions::MouseMoved
                    | NSTrackingAreaOptions::MouseEnteredAndExited
                    | NSTrackingAreaOptions::ActiveAlways
                    | NSTrackingAreaOptions::InVisibleRect;
                let t = unsafe {
                    NSTrackingArea::initWithRect_options_owner_userInfo(
                        NSTrackingArea::alloc(),
                        self.bounds(),
                        opts,
                        Some(as_any(self)),
                        None,
                    )
                };
                self.addTrackingArea(&t);
                *self.ivars().tracking.borrow_mut() = Some(t);
            }

            #[unsafe(method(mouseMoved:))]
            fn mouse_moved(&self, e: &NSEvent) {
                let p = self.convertPoint_fromView(e.locationInWindow(), None);
                let g = self.geometry();
                let hit = {
                    let m = self.ivars().model.borrow();
                    g.hit(&m, self.ivars().scroll.get(), p.x, p.y)
                };
                let over = hit.map(|h| h.0);
                let over_close = hit.and_then(|h| (h.1 && h.0 >= 0).then_some(h.0 as usize));
                let tip = over.and_then(|i| self.tip_for(i));
                let cur: Option<String> = self.toolTip().map(|t| t.to_string());
                if cur != tip {
                    self.setToolTip(tip.as_deref().map(NSString::from_str).as_deref());
                }
                if over != self.ivars().hover.get() || over_close != self.ivars().hover_close.get() {
                    self.ivars().hover.set(over);
                    self.ivars().hover_close.set(over_close);
                    self.setNeedsDisplay(true);
                }
            }

            #[unsafe(method(mouseExited:))]
            fn mouse_exited(&self, _e: &NSEvent) {
                self.ivars().hover.set(None);
                self.ivars().hover_close.set(None);
                self.setNeedsDisplay(true);
            }

            #[unsafe(method(scrollWheel:))]
            fn scroll_wheel(&self, e: &NSEvent) {
                let dy = if e.hasPreciseScrollingDeltas() {
                    e.scrollingDeltaY()
                } else {
                    e.scrollingDeltaY() * 10.0
                };
                let n = self.ivars().model.borrow().rows.len();
                let g = self.geometry();
                self.ivars().scroll.set(g.clamp_scroll(self.ivars().scroll.get() - dy, n));
                self.setNeedsDisplay(true);
            }

            #[unsafe(method(mouseDown:))]
            fn mouse_down(&self, e: &NSEvent) {
                let p = self.convertPoint_fromView(e.locationInWindow(), None);
                let g = self.geometry();
                let (hit, pinned_key) = {
                    let m = self.ivars().model.borrow();
                    let hit = g.hit(&m, self.ivars().scroll.get(), p.x, p.y);
                    let key = hit.and_then(|(i, _)| {
                        (i <= -10).then(|| m.pinned.get((-10 - i) as usize).map(|p| p.key.clone())).flatten()
                    });
                    (hit, key)
                };
                let Some((i, close)) = hit else {
                    return;
                };
                let ev = match i {
                    HIT_TOGGLE => {
                        let c = !self.ivars().collapsed.get();
                        self.set_collapsed(c);
                        SidebarEvent::Rail(c)
                    }
                    HIT_ADD => SidebarEvent::Add,
                    i if i <= -10 => match pinned_key {
                        Some(k) => SidebarEvent::Pinned(k),
                        None => return,
                    },
                    i if close => SidebarEvent::Close(i as usize),
                    i => {
                        let i = i as usize;
                        self.ivars().model.borrow_mut().selected = Some(i);
                        self.setNeedsDisplay(true);
                        SidebarEvent::Select(i)
                    }
                };
                self.emit(ev);
            }

            #[unsafe(method(rightMouseDown:))]
            fn right_mouse_down(&self, e: &NSEvent) {
                let p = self.convertPoint_fromView(e.locationInWindow(), None);
                let g = self.geometry();
                let ev = {
                    let m = self.ivars().model.borrow();
                    let s = NSEvent::mouseLocation();
                    match g.hit(&m, self.ivars().scroll.get(), p.x, p.y) {
                        Some((i, _)) if i >= 0 => Some(SidebarEvent::MenuRow(i as usize, s.x, s.y)),
                        Some((i, _)) if i <= -10 => m
                            .pinned
                            .get((-10 - i) as usize)
                            .map(|pin| SidebarEvent::MenuPinned(pin.key.clone(), s.x, s.y)),
                        _ => None,
                    }
                };
                if let Some(ev) = ev {
                    self.emit(ev);
                }
            }
        }

        unsafe impl NSObjectProtocol for SidebarView {}
    );

    impl SidebarView {
        pub fn new(mtm: MainThreadMarker, colors: PopupColors) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(SidebarIvars {
                model: RefCell::new(SidebarModel::default()),
                colors: Cell::new(colors),
                collapsed: Cell::new(false),
                scroll: Cell::new(0.0),
                hover: Cell::new(None),
                hover_close: Cell::new(None),
                on_event: RefCell::new(None),
                tracking: RefCell::new(None),
                collapse_key: RefCell::new(String::new()),
                key_focused: Cell::new(false),
                cursor: Cell::new(0),
            });
            unsafe {
                msg_send![super(this), initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(240.0, 400.0))]
            }
        }

        /// Whether the rail holds the keyboard (`keyFocused`).
        pub fn key_focused(&self) -> bool {
            self.ivars().key_focused.get()
        }

        /// `handleNavKey(_:)`: ↑↓ / Ctrl+N/P, Home/End, Page, Return (open +
        /// leave right), Space (open), Esc (leave right), Delete (close),
        /// letter jump. `false` = not a rail key.
        pub fn handle_nav_key(&self, e: &NSEvent) -> bool {
            use objc2_app_kit::NSEventModifierFlags as F;
            if !self.key_focused() {
                return false;
            }
            let mods = e.modifierFlags();
            let ch = e.charactersIgnoringModifiers().and_then(|s| s.to_string().chars().next());
            let (n, pins, page) = {
                let m = self.ivars().model.borrow();
                let g = self.geometry();
                (m.pinned.len() + m.rows.len(), m.pinned.len(), (g.list.height / (ROW_H + 1.0)) as usize)
            };
            let titles = |r: usize| -> String {
                let m = self.ivars().model.borrow();
                if r < m.pinned.len() {
                    m.pinned[r].label.clone()
                } else {
                    m.rows.get(r - m.pinned.len()).map(|x| x.title.clone()).unwrap_or_default()
                }
            };
            let (res, cur) = nav_key(
                e.keyCode(),
                mods.contains(F::Control),
                mods.contains(F::Command),
                mods.contains(F::Option),
                mods.contains(F::Shift),
                ch,
                self.ivars().cursor.get(),
                n,
                page.saturating_sub(1),
                &titles,
            );
            self.ivars().cursor.set(cur);
            self.reveal_cursor();
            match res {
                NavKeyResult::Pass => return false,
                NavKeyResult::Moved => {}
                NavKeyResult::Activate { row, leave } => {
                    self.activate(row);
                    if leave && self.key_focused() {
                        leave_right();
                    }
                }
                NavKeyResult::Close(row) => {
                    let closable = self.ivars().model.borrow().closable;
                    if closable && row >= pins {
                        self.emit(SidebarEvent::Close(row - pins));
                        let n = {
                            let m = self.ivars().model.borrow();
                            m.pinned.len() + m.rows.len()
                        };
                        self.ivars().cursor.set(cur.min(n.saturating_sub(1)));
                    }
                }
                NavKeyResult::Leave => leave_right(),
            }
            self.setNeedsDisplay(true);
            true
        }

        /// `activate(row:)`: pinned rows open their key, list rows select.
        fn activate(&self, row: usize) {
            let ev = {
                let mut m = self.ivars().model.borrow_mut();
                if row < m.pinned.len() {
                    SidebarEvent::Pinned(m.pinned[row].key.clone())
                } else {
                    let i = row - m.pinned.len();
                    if i >= m.rows.len() {
                        return;
                    }
                    m.selected = Some(i);
                    SidebarEvent::Select(i)
                }
            };
            self.emit(ev);
        }

        /// `revealCursor()`.
        fn reveal_cursor(&self) {
            let (pins, n) = {
                let m = self.ivars().model.borrow();
                (m.pinned.len(), m.rows.len())
            };
            let cur = self.ivars().cursor.get();
            if cur < pins {
                return;
            }
            let g = self.geometry();
            self.ivars().scroll.set(g.reveal(cur - pins, self.ivars().scroll.get(), n));
        }

        pub fn set_on_event(&self, cb: Box<dyn Fn(SidebarEvent)>) {
            *self.ivars().on_event.borrow_mut() = Some(cb);
        }

        fn emit(&self, ev: SidebarEvent) {
            // Clone-free: take the callback out while it runs so a re-entrant
            // `set_model` from inside it can't double-borrow.
            let cb = self.ivars().on_event.borrow_mut().take();
            if let Some(cb) = cb {
                cb(ev);
                let mut slot = self.ivars().on_event.borrow_mut();
                if slot.is_none() {
                    *slot = Some(cb);
                }
            }
        }

        pub fn set_model(&self, m: SidebarModel) {
            let changed_sel = self.ivars().model.borrow().selected != m.selected;
            let n = m.rows.len();
            let sel = m.selected;
            *self.ivars().model.borrow_mut() = m;
            let g = self.geometry();
            let mut s = g.clamp_scroll(self.ivars().scroll.get(), n);
            if changed_sel {
                if let Some(i) = sel {
                    s = g.reveal(i, s, n);
                }
            }
            self.ivars().scroll.set(s);
            self.setNeedsDisplay(true);
        }

        pub fn model(&self) -> SidebarModel {
            self.ivars().model.borrow().clone()
        }

        pub fn set_colors(&self, c: PopupColors) {
            self.ivars().colors.set(c);
            self.setNeedsDisplay(true);
        }

        /// `collapseKey`: restore / persist the rail state per view.
        pub fn set_collapse_key(&self, key: &str) {
            *self.ivars().collapse_key.borrow_mut() = key.to_string();
            let on = crate::ui::sidebar::rail_default(key);
            self.ivars().collapsed.set(on);
        }

        pub fn collapsed(&self) -> bool {
            self.ivars().collapsed.get()
        }

        pub fn set_collapsed(&self, on: bool) {
            self.ivars().collapsed.set(on);
            self.ivars().scroll.set(0.0);
            let key = self.ivars().collapse_key.borrow().clone();
            if !key.is_empty() {
                crate::ui::sidebar::set_rail_default(&key, on);
            }
            self.setNeedsDisplay(true);
        }

        /// `width(expanded:)`.
        pub fn width(&self, expanded: f64) -> f64 {
            if self.collapsed() {
                RAIL_WIDTH
            } else {
                expanded
            }
        }

        fn geometry(&self) -> SidebarGeometry {
            let b = self.bounds();
            let m = self.ivars().model.borrow();
            SidebarGeometry::new(b.size.width, b.size.height, &m, self.collapsed())
        }

        fn tip_for(&self, i: i64) -> Option<String> {
            let m = self.ivars().model.borrow();
            let collapsed = self.collapsed();
            match i {
                HIT_ADD => Some("New".into()),
                HIT_TOGGLE => Some(if collapsed {
                    "Show the sidebar  ⌘\\".into()
                } else {
                    "Collapse the sidebar  ⌘\\".into()
                }),
                i if i <= -10 => m.pinned.get((-10 - i) as usize).map(|p| {
                    if collapsed || p.tip.is_empty() { p.label.clone() } else { p.tip.clone() }
                }),
                i => m.rows.get(i as usize).and_then(|r| {
                    if collapsed {
                        Some(r.title.clone())
                    } else if r.tip.is_empty() {
                        None
                    } else {
                        Some(r.tip.clone())
                    }
                }),
            }
        }

        fn section_label(&self, text: &str, head: Rect, c: &PopupColors) {
            let f = style::system_font(10.0, Weight::Semibold);
            let t = text.to_uppercase();
            // `.kern: 0.8` — approximate by spacing the letters.
            let spaced: String = t.chars().collect();
            let (_, h) = style::text_size(&spaced, &f);
            draw_kerned(&spaced, head.x + 14.0, head.mid_y() - h / 2.0, &f, c.dim, 0.8);
        }

        fn rail_rule(&self, head: Rect, c: &PopupColors) {
            style::fill_rect(Rect::new(head.mid_x() - 10.0, head.mid_y(), 20.0, 1.0), c.dim.with_alpha(0.25));
        }

        fn draw(&self) {
            let c = self.ivars().colors.get();
            let g = self.geometry();
            let card = g.card;
            if card.width <= 0.0 || card.height <= 0.0 {
                return;
            }
            let collapsed = g.collapsed;
            style::fill_rounded(card, 12.0, c.mantle());
            let radius = 4.0;
            let m = self.ivars().model.borrow();
            let hover = self.ivars().hover.get();
            for (r, title) in &g.pinned_heads {
                if collapsed { self.rail_rule(*r, &c) } else { self.section_label(title, *r, &c) }
            }
            for (k, (r, pin)) in g.pinned_rows.iter().zip(m.pinned.iter()).enumerate() {
                let sel = m.pinned_selected.as_deref() == Some(pin.key.as_str());
                if sel || hover == Some(-10 - k as i64) {
                    style::fill_rounded(*r, radius, if sel { c.surface1() } else { c.surface0() });
                }
                let icon_x = if collapsed { r.mid_x() - 8.0 } else { r.x + 8.0 };
                style::draw_symbol(
                    if pin.icon.is_empty() { "folder" } else { &pin.icon },
                    Rect::new(icon_x, r.y, 16.0, r.height),
                    if sel { c.accent_on() } else { c.dim },
                    if collapsed { 13.0 } else { 11.0 },
                );
                if collapsed {
                    continue;
                }
                let f = style::system_font(12.5, if sel { Weight::Semibold } else { Weight::Regular });
                let mut tag_w = 0.0;
                if !pin.meta.is_empty() {
                    let tf = style::system_font(10.0, Weight::Regular);
                    let tw = style::draw_text_right(&pin.meta, Rect::new(r.x, r.y, r.width - 8.0, r.height), &tf, c.dim);
                    tag_w = tw + 8.0;
                }
                style::draw_text_left(
                    &pin.label,
                    Rect::new(r.x + 30.0, r.y, (r.width - 38.0 - tag_w).max(0.0), r.height),
                    &f,
                    c.text,
                );
            }
            if collapsed {
                self.rail_rule(g.head, &c);
            } else {
                self.section_label(&m.section_title, g.head, &c);
            }
            if m.add_button {
                let st = if hover == Some(HIT_ADD) { style::ButtonState::Hover } else { style::ButtonState::Idle };
                style::draw_button(g.add, st, &c, radius, true);
                style::plus(g.add, style::button_text(st, &c), 4.5);
            }
            let scroll = self.ivars().scroll.get();
            NSGraphicsContext::saveGraphicsState_class();
            NSBezierPath::clipRect(style::ns(g.list));
            let hover_close = self.ivars().hover_close.get();
            for (i, row) in m.rows.iter().enumerate() {
                let r = g.row_rect(i, scroll);
                if r.max_y() <= g.list.y || r.y >= g.list.max_y() {
                    continue;
                }
                let sel = m.selected == Some(i);
                let hov = hover == Some(i as i64);
                if sel || hov {
                    style::fill_rounded(r, radius, if sel { c.surface1() } else { c.surface0() });
                }
                let icon = if row.icon.is_empty() { icon_for_name(&row.title) } else { row.icon.as_str() };
                if collapsed {
                    style::draw_symbol(
                        icon,
                        Rect::new(r.mid_x() - 8.0, r.y, 16.0, r.height),
                        if sel { c.accent_on() } else { c.dim },
                        13.0,
                    );
                    if let Some((_, tone)) = &row.badge {
                        let dot = Rect::new(r.mid_x() + 5.0, r.mid_y() - 9.0, 7.0, 7.0);
                        c.tone(*tone).to_nscolor().setFill();
                        NSBezierPath::bezierPathWithOvalInRect(style::ns(dot)).fill();
                    }
                    continue;
                }
                style::draw_symbol(
                    icon,
                    Rect::new(r.x + 8.0, r.y, 16.0, r.height),
                    if sel { c.accent_on() } else { c.dim },
                    11.0,
                );
                let show_close = m.closable && hov;
                let close = SidebarGeometry::close_rect(r);
                let meta = if show_close {
                    String::new()
                } else {
                    row.badge.as_ref().map(|b| b.0.clone()).unwrap_or_else(|| row.meta.clone())
                };
                let mf = objc2_app_kit::NSFont::monospacedDigitSystemFontOfSize_weight(10.0, unsafe {
                    objc2_app_kit::NSFontWeightRegular
                });
                let mut right = if show_close { close.x - 4.0 } else { r.max_x() - 8.0 };
                if !meta.is_empty() {
                    let mw = style::draw_text_right(&meta, Rect::new(r.x, r.y, r.width - 8.0, r.height), &mf, c.dim);
                    right -= mw + 8.0;
                }
                if let (Some((_, tone)), false) = (&row.badge, show_close) {
                    let dot = Rect::new(right - 7.0, r.mid_y() - 3.5, 7.0, 7.0);
                    c.tone(*tone).to_nscolor().setFill();
                    NSBezierPath::bezierPathWithOvalInRect(style::ns(dot)).fill();
                    right = dot.x - 6.0;
                }
                let tf = style::system_font(12.5, if sel { Weight::Semibold } else { Weight::Regular });
                let tx = r.x + 30.0;
                style::draw_text_left(&row.title, Rect::new(tx, r.y, (right - tx).max(0.0), r.height), &tf, c.text);
                if show_close {
                    let hc = hover_close == Some(i);
                    if hc {
                        c.tone(PopupTone::Danger).with_alpha(0.22).to_nscolor().setFill();
                        NSBezierPath::bezierPathWithOvalInRect(style::ns(close)).fill();
                    }
                    style::cross(close, if hc { c.tone(PopupTone::Danger) } else { c.dim }, 3.0);
                }
            }
            if self.key_focused() {
                let cur = self.ivars().cursor.get();
                let pins = m.pinned.len();
                let r = if cur < pins { g.pinned_rows.get(cur).copied() } else { (cur - pins < m.rows.len()).then(|| g.row_rect(cur - pins, scroll)) };
                if let Some(r) = r {
                    // `drawCursor`: the pane ring color, a touch stronger.
                    let ring = crate::panes::pane_nav::RingStyle::default().rgba();
                    let a = (ring.a + 0.25).min(1.0);
                    style::stroke_rounded(r.inset_by(0.5, 0.5), radius, 1.0, ring.with_alpha(a));
                }
            }
            NSGraphicsContext::restoreGraphicsState_class();
            {
                let st = if hover == Some(HIT_TOGGLE) { style::ButtonState::Hover } else { style::ButtonState::Idle };
                style::draw_button(g.toggle, st, &c, radius, true);
                style::draw_symbol(
                    if collapsed { "sidebar.right" } else { "sidebar.left" },
                    g.toggle,
                    if hover == Some(HIT_TOGGLE) { c.text } else { c.dim },
                    12.0,
                );
            }
            if let (Some((text, tone)), false) = (&m.status, collapsed) {
                let r = g.toggle;
                let dot = Rect::new(r.max_x() + 8.0, r.mid_y() - 3.5, 7.0, 7.0);
                c.tone(*tone).to_nscolor().setFill();
                NSBezierPath::bezierPathWithOvalInRect(style::ns(dot)).fill();
                let sf = style::system_font(10.5, Weight::Regular);
                let tx = dot.max_x() + 6.0;
                style::draw_text_left(text, Rect::new(tx, r.y, (card.max_x() - 10.0 - tx).max(0.0), r.height), &sf, c.dim);
            }
            let content = SidebarGeometry::content_height(m.rows.len());
            if content > g.list.height + 1.0 {
                let h = (g.list.height * g.list.height / content).max(20.0);
                let y = g.list.y + (g.list.height - h) * (scroll / (content - g.list.height).max(1.0));
                style::fill_rounded(Rect::new(card.max_x() - 4.0, y, 2.5, h), 1.25, c.dim.with_alpha(0.35));
            }
        }
    }

    /// Draw with per-glyph tracking (`.kern`).
    fn draw_kerned(s: &str, x: f64, y: f64, f: &objc2_app_kit::NSFont, color: Rgba, kern: f64) {
        let mut cx = x;
        for ch in s.chars() {
            let t = ch.to_string();
            style::draw_text(&t, cx, y, f, color);
            cx += style::text_size(&t, f).0 + kern;
        }
    }
}

/// The rail state per `collapseKey` (`UserDefaults "sidebarRail.<key>"`).
pub fn rail_default(key: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        use objc2_foundation::{NSString, NSUserDefaults};
        let d = NSUserDefaults::standardUserDefaults();
        return d.boolForKey(&NSString::from_str(&format!("sidebarRail.{key}")));
    }
    #[allow(unreachable_code)]
    {
        let _ = key;
        false
    }
}

pub fn set_rail_default(key: &str, on: bool) {
    #[cfg(target_os = "macos")]
    {
        use objc2_foundation::{NSString, NSUserDefaults};
        let d = NSUserDefaults::standardUserDefaults();
        d.setBool_forKey(on, &NSString::from_str(&format!("sidebarRail.{key}")));
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (key, on);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(n: usize, pins: usize) -> SidebarModel {
        SidebarModel {
            section_title: "Places".into(),
            rows: (0..n).map(|i| SidebarRow::new(&format!("r{i}"), "folder")).collect(),
            pinned: (0..pins)
                .map(|i| SidebarPin { key: format!("/p{i}"), label: format!("p{i}"), section: "Pinned".into(), ..Default::default() })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn head_sits_below_pinned_rows() {
        let m = model(3, 2);
        let g = SidebarGeometry::new(240.0, 600.0, &m, false);
        assert_eq!(g.pinned_rows.len(), 2);
        assert_eq!(g.head.y, g.pinned_rows[1].max_y() + 10.0);
        assert_eq!(g.list.y, g.head.max_y() + 4.0);
    }

    #[test]
    fn hits_rows_pins_and_toggle() {
        let m = model(3, 1);
        let g = SidebarGeometry::new(240.0, 600.0, &m, false);
        let r1 = g.row_rect(1, 0.0);
        assert_eq!(g.hit(&m, 0.0, r1.mid_x() - 40.0, r1.mid_y()), Some((1, false)));
        let p = g.pinned_rows[0];
        assert_eq!(g.hit(&m, 0.0, p.mid_x(), p.mid_y()), Some((-10, false)));
        assert_eq!(g.hit(&m, 0.0, g.toggle.mid_x(), g.toggle.mid_y()), Some((HIT_TOGGLE, false)));
    }

    #[test]
    fn section_change_adds_a_head() {
        let mut m = model(0, 3);
        m.pinned[2].section = "Release".into();
        let g = SidebarGeometry::new(240.0, 600.0, &m, false);
        assert_eq!(g.pinned_heads.len(), 2);
    }

    #[test]
    fn reveal_clamps() {
        let m = model(40, 0);
        let g = SidebarGeometry::new(240.0, 300.0, &m, false);
        let s = g.reveal(39, 0.0, 40);
        assert!(s > 0.0);
        assert!(g.row_rect(39, s).max_y() <= g.list.max_y() + 0.001);
    }

    #[test]
    fn icons_by_extension() {
        assert_eq!(icon_for_name("a.md"), "doc.text");
        assert_eq!(icon_for_name("a.toml"), "gearshape");
        assert_eq!(icon_for_name("a.rs"), "chevron.left.forwardslash.chevron.right");
        assert_eq!(icon_for_name("a.pdf"), "doc");
    }
}

#[cfg(test)]
mod nav_tests {
    use super::*;

    fn key(code: u16, ctrl: bool, ch: Option<char>, cur: usize) -> (NavKeyResult, usize) {
        let titles = |r: usize| ["alpha", "beta", "bravo", "gamma"][r].to_string();
        nav_key(code, ctrl, false, false, false, ch, cur, 4, 2, &titles)
    }

    #[test]
    fn arrows_and_ctrl_np_move_and_clamp() {
        assert_eq!(key(125, false, None, 0), (NavKeyResult::Moved, 1));
        assert_eq!(key(45, true, None, 3), (NavKeyResult::Moved, 3));
        assert_eq!(key(126, false, None, 0), (NavKeyResult::Moved, 0));
        assert_eq!(key(35, true, None, 2), (NavKeyResult::Moved, 1));
        assert_eq!(key(119, false, None, 0), (NavKeyResult::Moved, 3));
    }

    #[test]
    fn return_activates_and_leaves_space_stays_esc_leaves() {
        assert_eq!(key(36, false, None, 2), (NavKeyResult::Activate { row: 2, leave: true }, 2));
        assert_eq!(key(49, false, None, 1), (NavKeyResult::Activate { row: 1, leave: false }, 1));
        assert_eq!(key(53, false, None, 1), (NavKeyResult::Leave, 1));
        assert_eq!(key(51, false, None, 1), (NavKeyResult::Close(1), 1));
    }

    #[test]
    fn letters_jump_cyclically_and_ctrl_keys_pass() {
        assert_eq!(key(11, false, Some('b'), 0), (NavKeyResult::Moved, 1));
        assert_eq!(key(11, false, Some('b'), 1), (NavKeyResult::Moved, 2));
        assert_eq!(key(11, false, Some('b'), 2), (NavKeyResult::Moved, 1));
        assert_eq!(key(4, true, Some('h'), 2).0, NavKeyResult::Pass);
        assert_eq!(key(48, false, None, 2).0, NavKeyResult::Pass);
    }
}
