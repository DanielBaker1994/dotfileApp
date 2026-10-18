//! The Compare window chrome — the AppKit half of Swift's `CompareWindow`
//! around the ported `ComparePaneView`: the vertical sidebar (COMPARE: Home /
//! New text compare, OPEN: sessions), the start page (`buildStart` /
//! `layoutStart`: Left / Right path boxes, the Compare ⏎ | New Text Compare
//! capsule, RECENT with Clear Missing / Clear All + filter and the
//! `CompareRecentList` rows) and the text toolbar (`buildText`: the filter
//! segments, the summary, the tool buttons) + status line.

use super::compare::{CompareFilter, CompareRecentEntry, PaneModel};
#[cfg(target_os = "macos")]
use crate::ui::chrome::Rect;
use crate::ui::sidebar::{SidebarEvent, SidebarModel};

pub const TOOLBAR_H: f64 = 36.0;
pub const STATUS_H: f64 = 24.0;
pub const RECENT_ROW_H: f64 = 28.0;
pub const FIELD_H: f64 = 28.0;

pub const FILTERS: [&str; 4] = ["All", "Diffs", "Same", "Context"];
pub const FILTER_TIPS: [&str; 4] = [
    "Every line (⌘1)",
    "Only differences (⌘2)",
    "Only lines that are the same (⌘3)",
    "Differences with [compare] context-lines around them (⌘4)",
];
pub const TOOLS: [(&str, &str); 6] = [
    ("chevron.up", "Previous difference (⌃P)"),
    ("chevron.down", "Next difference (⌃N)"),
    ("arrow.left.arrow.right", "Swap sides (⌘⌥X)"),
    ("arrow.clockwise", "Reload both files and compare again (F5)"),
    ("slider.horizontal.3", "Importance: what counts as a difference"),
    ("magnifyingglass", "Find (⌘F)"),
];

/// One recent row with its missing flags.
#[derive(Clone, Debug, PartialEq)]
pub struct RecentRow {
    pub entry: CompareRecentEntry,
    pub missing_left: bool,
    pub missing_right: bool,
    pub pasted_left: bool,
    pub pasted_right: bool,
}

impl RecentRow {
    pub fn missing(&self) -> bool {
        self.missing_left || self.missing_right
    }
}

#[derive(Clone, Debug, Default)]
pub struct CompareSnap {
    pub sidebar: SidebarModel,
    pub on_start: bool,
    pub folder: bool,
    pub recent: Vec<RecentRow>,
    pub recent_sel: usize,
    pub filter: usize,
    pub summary: String,
    pub identical: bool,
    pub status: String,
}

/// Everything the Compare view draws, pushed by the model: the diff body
/// (rows of the existing diff model — [`PaneModel`], built from the current
/// `TextCompare`), the chrome around it, and whether a folder session's tree
/// is showing instead.
#[derive(Clone, Debug, PartialEq)]
pub struct CompareSnapshot {
    /// The diff pane (rows + both sides' line text); empty on the start page
    /// and for a folder session.
    pub pane: PaneModel,
    pub on_start: bool,
    pub folder: bool,
    pub filter: CompareFilter,
    pub summary: String,
    pub status: String,
}

impl Default for CompareSnapshot {
    fn default() -> Self {
        CompareSnapshot {
            pane: PaneModel::empty(),
            on_start: true,
            folder: false,
            filter: CompareFilter::All,
            summary: String::new(),
            status: String::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum CompareEvent {
    Sidebar(SidebarEvent),
    Compare { left: String, right: String },
    NewText,
    Browse { right: bool },
    OpenRecent(usize),
    RemoveRecent(usize),
    RecentMenu { row: usize, x: f64, y: f64 },
    ClearMissing,
    ClearAll,
    RecentFilter(String),
    Filter(usize),
    Tool(usize),
    /// Right-click in the text pane (`ComparePane.rightMouseDown`).
    PaneMenu { right: bool, row: usize, x: f64, y: f64 },
    /// Right-click on a folder tree row (node `id`).
    FolderMenu { id: usize, x: f64, y: f64 },
}

/// `CompareRecent.age(_:)`.
pub fn recent_age(used: f64, now: f64) -> String {
    let s = now - used;
    if s < 60.0 {
        return "just now".into();
    }
    if s < 3600.0 {
        return format!("{} min ago", (s / 60.0) as i64);
    }
    if s < 86400.0 {
        return format!("{} h ago", (s / 3600.0) as i64);
    }
    if s < 172_800.0 {
        return "yesterday".into();
    }
    let days = (s / 86400.0) as i64;
    format!("{days} d ago")
}

// Control ids.
const C_COMPARE: i32 = 1;
const C_NEWTEXT: i32 = 2;
const C_BROWSE_L: i32 = 3;
const C_BROWSE_R: i32 = 4;
const C_CLEAR_MISSING: i32 = 5;
const C_CLEAR_ALL: i32 = 6;
const C_FILTER: i32 = 10;
const C_TOOL: i32 = 20;
const C_RECENT: i32 = 100; // + row (x hit: + 10000)

#[cfg(target_os = "macos")]
pub use appkit::CompareView;

#[cfg(target_os = "macos")]
mod appkit {
    use super::*;
    use crate::ui::pane_divider::{DividerRange, PaneDividerView};
    use crate::ui::sidebar::SidebarView;
    use crate::ui::style::{self, ButtonState, Weight};
    use crate::ui::theme::{PopupColors, PopupTone};
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, ProtocolObject, Sel};
    use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSControlTextEditingDelegate, NSEvent, NSTextField, NSTextFieldDelegate, NSTextView, NSTrackingArea, NSView,
    };
    use objc2_foundation::{NSNotification, NSObjectProtocol, NSRect, NSSize, NSString};
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use crate::ui::view_base::{self, as_any};

    fn ns(r: Rect) -> NSRect {
        style::ns(r)
    }

    type Sink = Rc<dyn Fn(CompareEvent)>;

    pub struct CompareIvars {
        snap: RefCell<CompareSnap>,
        colors: Cell<PopupColors>,
        sidebar: Retained<SidebarView>,
        left_box: Retained<NSView>,
        left: Retained<NSTextField>,
        right_box: Retained<NSView>,
        right: Retained<NSTextField>,
        filter_box: Retained<NSView>,
        filter: Retained<NSTextField>,
        body: RefCell<Vec<Retained<NSView>>>,
        sink: RefCell<Option<Sink>>,
        hover: Cell<Option<i32>>,
        pressed: Cell<Option<i32>>,
        tracking: RefCell<Option<Retained<NSTrackingArea>>>,
        sidebar_w: Cell<f64>,
        /// The draggable divider on the sidebar's right edge.
        divider: Retained<PaneDividerView>,
        recent_scroll: Cell<f64>,
    }

    define_class!(
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSCompareView"]
        #[ivars = CompareIvars]
        pub struct CompareView;

        impl CompareView {
            #[unsafe(method(isFlipped))]
            fn is_flipped(&self) -> bool { true }

            #[unsafe(method(setFrameSize:))]
            fn set_frame_size(&self, size: NSSize) {
                let _: () = unsafe { msg_send![super(self), setFrameSize: size] };
                self.layout_all();
            }

            #[unsafe(method(updateTrackingAreas))]
            fn uta(&self) {
                let _: () = unsafe { msg_send![super(self), updateTrackingAreas] };
                view_base::reset_tracking(self, &self.ivars().tracking, view_base::hover_opts());
            }

            #[unsafe(method(mouseMoved:))]
            fn mouse_moved(&self, e: &NSEvent) {
                let p = view_base::mouse_point(self, e);
                let h = self.hit(p.x, p.y);
                let tip = h.and_then(|id| match id {
                    C_FILTER..=13 => Some(FILTER_TIPS[(id - C_FILTER) as usize].to_string()),
                    C_TOOL..=25 => Some(TOOLS[(id - C_TOOL) as usize].1.to_string()),
                    C_BROWSE_L => Some("Choose the left file or folder…".into()),
                    C_BROWSE_R => Some("Choose the right file or folder…".into()),
                    C_COMPARE | C_NEWTEXT => Some("Compare: files on both sides → Text Compare, folders → Folder Compare. New Text Compare: two empty sides — click one and paste (⌘V).".into()),
                    id if id >= C_RECENT && id < C_RECENT + 10000 => {
                        let snap = self.ivars().snap.borrow();
                        snap.recent.get((id - C_RECENT) as usize).filter(|r| r.missing()).map(|r| {
                            let mut gone = Vec::new();
                            if r.missing_left {
                                gone.push(r.entry.left.clone());
                            }
                            if r.missing_right && r.entry.right != r.entry.left {
                                gone.push(r.entry.right.clone());
                            }
                            format!("Can't open: no longer exists\n{}", gone.join("\n"))
                        })
                    }
                    _ => None,
                });
                let cur: Option<String> = self.toolTip().map(|t| t.to_string());
                if cur != tip {
                    self.setToolTip(tip.as_deref().map(NSString::from_str).as_deref());
                }
                view_base::set_hover(self, &self.ivars().hover, h);
            }

            #[unsafe(method(mouseExited:))]
            fn mouse_exited(&self, _e: &NSEvent) {
                view_base::set_hover(self, &self.ivars().hover, None);
            }

            #[unsafe(method(scrollWheel:))]
            fn scroll_wheel(&self, e: &NSEvent) {
                if !self.ivars().snap.borrow().on_start {
                    return;
                }
                let (_, list) = self.recent_regions();
                let n = self.ivars().snap.borrow().recent.len() as f64;
                let max = (n * RECENT_ROW_H - list.height).max(0.0);
                let dy = if e.hasPreciseScrollingDeltas() { e.scrollingDeltaY() } else { e.scrollingDeltaY() * 10.0 };
                let s = (self.ivars().recent_scroll.get() - dy).clamp(0.0, max);
                self.ivars().recent_scroll.set(s);
                self.setNeedsDisplay(true);
            }

            #[unsafe(method(mouseDown:))]
            fn mouse_down(&self, e: &NSEvent) {
                let p = view_base::mouse_point(self, e);
                let h = self.hit(p.x, p.y);
                if let Some(id) = h {
                    if id >= C_RECENT {
                        let row = ((id - C_RECENT) % 10000) as usize;
                        let ev = if id >= C_RECENT + 10000 { CompareEvent::RemoveRecent(row) } else { CompareEvent::OpenRecent(row) };
                        self.ivars().snap.borrow_mut().recent_sel = row;
                        self.emit(ev);
                        return;
                    }
                }
                self.ivars().pressed.set(h);
                self.setNeedsDisplay(true);
            }

            #[unsafe(method(rightMouseDown:))]
            fn right_mouse_down(&self, e: &NSEvent) {
                let p = view_base::mouse_point(self, e);
                if let Some(id) = self.hit(p.x, p.y).filter(|id| *id >= C_RECENT) {
                    let s = NSEvent::mouseLocation();
                    self.emit(CompareEvent::RecentMenu { row: ((id - C_RECENT) % 10000) as usize, x: s.x, y: s.y });
                }
            }

            #[unsafe(method(mouseUp:))]
            fn mouse_up(&self, e: &NSEvent) {
                let p = view_base::mouse_point(self, e);
                let h = self.hit(p.x, p.y);
                let was = self.ivars().pressed.replace(None);
                self.setNeedsDisplay(true);
                let Some(id) = h.filter(|_| h == was) else { return };
                let ev = match id {
                    C_COMPARE => CompareEvent::Compare {
                        left: self.ivars().left.stringValue().to_string(),
                        right: self.ivars().right.stringValue().to_string(),
                    },
                    C_NEWTEXT => CompareEvent::NewText,
                    C_BROWSE_L => CompareEvent::Browse { right: false },
                    C_BROWSE_R => CompareEvent::Browse { right: true },
                    C_CLEAR_MISSING => CompareEvent::ClearMissing,
                    C_CLEAR_ALL => CompareEvent::ClearAll,
                    C_FILTER..=13 => CompareEvent::Filter((id - C_FILTER) as usize),
                    C_TOOL..=25 => CompareEvent::Tool((id - C_TOOL) as usize),
                    _ => return,
                };
                self.emit(ev);
            }

            #[unsafe(method(drawRect:))]
            fn draw_rect(&self, _dirty: NSRect) {
                self.draw();
            }

            #[unsafe(method(controlTextDidChange:))]
            fn control_text_did_change(&self, n: &NSNotification) {
                let obj = n.object();
                let is_filter = obj.as_deref().is_some_and(|o| std::ptr::eq(o as *const AnyObject, as_any(&*self.ivars().filter)));
                if is_filter {
                    self.ivars().recent_scroll.set(0.0);
                    self.emit(CompareEvent::RecentFilter(self.ivars().filter.stringValue().to_string()));
                }
            }

            #[unsafe(method(control:textView:doCommandBySelector:))]
            fn do_command(&self, _c: &AnyObject, _tv: &NSTextView, cmd: Sel) -> objc2::runtime::Bool {
                if cmd == sel!(insertNewline:) {
                    self.emit(CompareEvent::Compare {
                        left: self.ivars().left.stringValue().to_string(),
                        right: self.ivars().right.stringValue().to_string(),
                    });
                    return objc2::runtime::Bool::YES;
                }
                objc2::runtime::Bool::NO
            }
        }

        unsafe impl NSObjectProtocol for CompareView {}
        unsafe impl NSControlTextEditingDelegate for CompareView {}
        unsafe impl NSTextFieldDelegate for CompareView {}
    );

    impl CompareView {
        /// `CompareWindow.navPanes` (chrome half): sidebar, both path boxes,
        /// the recent filter.
        pub fn nav_panes(&self) -> Vec<crate::app::live_panes::LivePane> {
            use crate::app::live_panes::LivePane;
            let iv = self.ivars();
            vec![
                LivePane::new("sidebar", &iv.sidebar),
                LivePane::new("left-path", &iv.left_box).focusing(&iv.left),
                LivePane::new("right-path", &iv.right_box).focusing(&iv.right),
                LivePane::new("recent-filter", &iv.filter_box).focusing(&iv.filter),
            ]
        }

        pub fn new(mtm: MainThreadMarker, colors: PopupColors, sidebar_w: f64, sidebar_range: DividerRange) -> Retained<Self> {
            let sidebar = SidebarView::new(mtm, colors);
            sidebar.set_collapse_key("compare");
            let (left_box, left) = crate::ui::field::search_box(mtm, &colors, 13.0, "Left: a file or folder path (Tab completes)", 6.0, FIELD_H);
            let (right_box, right) = crate::ui::field::search_box(mtm, &colors, 13.0, "Right: a file or folder path", 6.0, FIELD_H);
            let (filter_box, filter) = crate::ui::field::search_box(mtm, &colors, 12.0, "filter", 6.0, 26.0);
            let divider = PaneDividerView::new(mtm, sidebar_range);
            let this = Self::alloc(mtm).set_ivars(CompareIvars {
                snap: RefCell::new(CompareSnap::default()),
                colors: Cell::new(colors),
                sidebar,
                left_box,
                left,
                right_box,
                right,
                filter_box,
                filter,
                body: RefCell::new(Vec::new()),
                sink: RefCell::new(None),
                hover: Cell::new(None),
                pressed: Cell::new(None),
                tracking: RefCell::new(None),
                sidebar_w: Cell::new(sidebar_w),
                divider: divider.clone(),
                recent_scroll: Cell::new(0.0),
            });
            let v: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
            let iv = v.ivars();
            for sub in [&*iv.sidebar as &NSView, &iv.left_box, &iv.right_box, &iv.filter_box] {
                v.addSubview(sub);
            }
            v.addSubview(&divider);
            for f in [&iv.left, &iv.right, &iv.filter] {
                unsafe { f.setDelegate(Some(ProtocolObject::from_ref(&*v))) };
            }
            v
        }

        /// The diff pane / folder tree views laid out in the body.
        pub fn set_body(&self, views: Vec<Retained<NSView>>) {
            for b in &views {
                self.addSubview(b);
            }
            // The divider must stay above the body panes so its strip takes the
            // drag rather than a body click.
            if let Some(last) = views.last() {
                let div = self.ivars().divider.clone();
                self.addSubview_positioned_relativeTo(
                    &div,
                    objc2_app_kit::NSWindowOrderingMode::Above,
                    Some(last),
                );
            }
            *self.ivars().body.borrow_mut() = views;
            self.layout_all();
        }

        pub fn set_on_event(&self, cb: Box<dyn Fn(CompareEvent)>) {
            let rc: Sink = Rc::from(cb);
            *self.ivars().sink.borrow_mut() = Some(rc.clone());
            self.ivars().sidebar.set_on_event(Box::new(move |e| rc(CompareEvent::Sidebar(e))));
        }

        fn emit(&self, ev: CompareEvent) {
            let s = self.ivars().sink.borrow().clone();
            if let Some(s) = s {
                s(ev);
            }
        }

        pub fn set_side_text(&self, right: bool, text: &str) {
            let f = if right { &self.ivars().right } else { &self.ivars().left };
            f.setStringValue(&NSString::from_str(text));
        }

        pub fn left_field(&self) -> Retained<NSTextField> {
            self.ivars().left.clone()
        }

        /// The draggable divider on the sidebar's right edge.
        pub fn divider(&self) -> Retained<PaneDividerView> {
            self.ivars().divider.clone()
        }

        /// The expanded sidebar width (`[compare] sidebar-width`).
        pub fn sidebar_width(&self) -> f64 {
            self.ivars().sidebar_w.get()
        }

        /// Apply a divider drag / `do:sidebar-width:compare`.
        pub fn set_sidebar_width(&self, width: f64) {
            let w = self.ivars().divider.clamp(width);
            self.ivars().sidebar_w.set(w);
            self.layout_all();
        }

        /// Set the collapse-to-rail state from the host (Ctrl+B B /
        /// `do:rail:compare`): flip the sidebar (which persists it), then
        /// re-lay-out exactly as the in-view `SidebarEvent::Rail` does.
        pub fn set_rail(&self, on: bool) {
            let sb = self.ivars().sidebar.clone();
            if sb.collapsed() != on {
                sb.set_collapsed(on);
            }
            self.layout_all();
        }

        pub fn apply(&self, s: CompareSnap) {
            let iv = self.ivars();
            iv.sidebar.set_model(s.sidebar.clone());
            let start = s.on_start;
            for v in [&*iv.left_box, &iv.right_box, &iv.filter_box] {
                v.setHidden(!start);
            }
            for b in iv.body.borrow().iter() {
                b.setHidden(start);
            }
            *iv.snap.borrow_mut() = s;
            self.layout_all();
            self.setNeedsDisplay(true);
        }

        fn left(&self) -> f64 {
            self.ivars().sidebar.width(self.ivars().sidebar_w.get()) + 4.0
        }

        /// The start page column (x, width) — `layoutStart`.
        fn start_col(&self) -> (f64, f64) {
            let b = self.bounds();
            let left = self.left();
            let avail = b.size.width - left;
            let w = (avail - 48.0).min(900.0).max(200.0);
            (left + (avail - w) / 2.0, w)
        }

        /// (recent header y, recent list rect).
        fn recent_regions(&self) -> (f64, Rect) {
            let (x, w) = self.start_col();
            let mut y = 26.0;
            y += (FIELD_H + 12.0) * 2.0;
            y += 42.0 + 6.0;
            let head = y;
            y += 26.0 + 8.0;
            let h = (self.bounds().size.height - y - 14.0).max(40.0);
            (head, Rect::new(x, y, w, h))
        }

        pub fn layout_all(&self) {
            let iv = self.ivars();
            let b = self.bounds();
            let left = self.left();
            let sw = iv.sidebar.width(iv.sidebar_w.get());
            iv.sidebar.setFrame(ns(Rect::new(0.0, 0.0, sw, b.size.height)));
            iv.divider.place(ns(Rect::new(sw - 4.0, 0.0, 8.0, b.size.height)));
            iv.divider.set_width(sw);
            iv.divider.setHidden(iv.sidebar.collapsed());
            let (x, w) = self.start_col();
            let lw = 46.0;
            let bw = 30.0;
            let mut y = 26.0;
            for bx in [&iv.left_box, &iv.right_box] {
                bx.setFrame(ns(Rect::new(x + lw + bw + 6.0, y, w - lw - bw - 6.0, FIELD_H)));
                y += FIELD_H + 12.0;
            }
            let (head, _) = self.recent_regions();
            iv.filter_box.setFrame(ns(Rect::new(x + w - 220.0, head, 220.0, 26.0)));
            let body = Rect::new(left, TOOLBAR_H, (b.size.width - left).max(0.0), (b.size.height - TOOLBAR_H - STATUS_H).max(0.0));
            for v in iv.body.borrow().iter() {
                v.setFrame(ns(body));
            }
        }

        fn controls(&self) -> Vec<(i32, Rect)> {
            let mut out = Vec::new();
            let snap = self.ivars().snap.borrow();
            let b = self.bounds();
            let left = self.left();
            if snap.on_start {
                let (x, w) = self.start_col();
                let lw = 46.0;
                let mut y = 26.0;
                for id in [C_BROWSE_L, C_BROWSE_R] {
                    out.push((id, Rect::new(x + lw, y, 30.0, FIELD_H)));
                    y += FIELD_H + 12.0;
                }
                // The capsule (CapsuleButtons): Compare ⏎ (primary) | New Text Compare.
                let f = style::system_font(12.5, Weight::Semibold);
                let geo = style::CAPSULE_GEO;
                // Icon + label commands: the +48 is the symbol allowance on
                // top of the canonical text chip; gutters/track padding stay
                // the shared CAPSULE_GEO numbers.
                let w1 = style::text_size("Compare  ⏎", &f).0.ceil() + 48.0;
                let w2 = style::text_size("New Text Compare", &f).0.ceil() + 48.0;
                let total = 2.0 * geo.track_pad + w1 + w2 + geo.gutter;
                let cx = x + (w - total) / 2.0 + lw / 2.0;
                out.push((C_COMPARE, Rect::new(cx + geo.track_pad, y + geo.track_pad, w1, 26.0)));
                out.push((C_NEWTEXT, Rect::new(cx + geo.track_pad + w1 + geo.gutter, y + geo.track_pad, w2, 26.0)));
                let (head, list) = self.recent_regions();
                let missing = snap.recent.iter().filter(|r| r.missing()).count();
                let mut ax = x + 66.0 + geo.track_pad;
                if missing > 0 {
                    let t = format!("Clear Missing ({missing})");
                    let mw = style::text_size(&t, &f).0.ceil() + 48.0;
                    out.push((C_CLEAR_MISSING, Rect::new(ax, head, mw, 22.0)));
                    ax += mw + geo.gutter;
                }
                if !snap.recent.is_empty() {
                    let cw = style::text_size("Clear All", &f).0.ceil() + 48.0;
                    out.push((C_CLEAR_ALL, Rect::new(ax, head - 4.0 + 4.0, cw, 22.0)));
                }
                let scroll = self.ivars().recent_scroll.get();
                for (i, _) in snap.recent.iter().enumerate() {
                    let r = Rect::new(list.x, list.y + i as f64 * RECENT_ROW_H - scroll, list.width, RECENT_ROW_H);
                    if r.max_y() < list.y || r.y > list.max_y() {
                        continue;
                    }
                    out.push((C_RECENT + 10000 + i as i32, Rect::new(r.max_x() - 30.0, r.y + 4.0, 20.0, 20.0)));
                    out.push((C_RECENT + i as i32, r));
                }
            } else {
                let f = style::system_font(12.0, Weight::Medium);
                let geo = style::CAPSULE_GEO;
                let (ws, _) = style::capsule_metrics(&FILTERS, &f);
                let mut sx = left + 12.0 + geo.track_pad;
                for (i, w) in ws.iter().enumerate() {
                    out.push((C_FILTER + i as i32, Rect::new(sx, (TOOLBAR_H - 26.0) / 2.0 + geo.track_pad, *w, 20.0)));
                    sx += w + geo.gutter;
                }
                let mut bx = b.size.width - 10.0;
                for i in (0..TOOLS.len()).rev() {
                    bx -= 28.0;
                    out.push((C_TOOL + i as i32, Rect::new(bx, (TOOLBAR_H - 26.0) / 2.0, 26.0, 26.0)));
                    bx -= 2.0;
                }
            }
            out
        }

        fn hit(&self, x: f64, y: f64) -> Option<i32> {
            self.controls().into_iter().find(|(_, r)| r.contains(x, y)).map(|(id, _)| id)
        }

        fn draw(&self) {
            let c = self.ivars().colors.get();
            let snap = self.ivars().snap.borrow();
            let b = self.bounds();
            let left = self.left();
            let controls = self.controls();
            let rect_of = |id: i32| controls.iter().find(|(i, _)| *i == id).map(|(_, r)| *r);
            let hover = self.ivars().hover.get();
            let pressed = self.ivars().pressed.get();
            if snap.on_start {
                let (x, w) = self.start_col();
                let lf = style::system_font(12.0, Weight::Semibold);
                let mut y = 26.0;
                for (label, id) in [("Left", C_BROWSE_L), ("Right", C_BROWSE_R)] {
                    style::draw_text(label, x, y + 5.0, &lf, c.text);
                    if let Some(r) = rect_of(id) {
                        style::draw_button(r, ButtonState::from_flags(false, hover == Some(id), pressed == Some(id)), &c, 4.0, true);
                        style::draw_symbol("folder", r, style::button_text(ButtonState::Idle, &c), 11.0);
                    }
                    y += FIELD_H + 12.0;
                }
                if let (Some(a), Some(bb)) = (rect_of(C_COMPARE), rect_of(C_NEWTEXT)) {
                    let geo = style::CAPSULE_GEO;
                    let track = Rect::new(
                        a.x - geo.track_pad,
                        a.y - geo.track_pad,
                        bb.max_x() - a.x + 2.0 * geo.track_pad,
                        a.height + 2.0 * geo.track_pad,
                    );
                    style::capsule_track(track, &c);
                    style::capsule_primary(a, &c, hover == Some(C_COMPARE), pressed == Some(C_COMPARE));
                    style::capsule_chip(bb, &c, false, hover == Some(C_NEWTEXT) || pressed == Some(C_NEWTEXT));
                    let f = style::system_font(12.5, Weight::Semibold);
                    for (r, sym, t, fg) in [
                        (a, "arrow.left.arrow.right", "Compare  ⏎", c.on_accent()),
                        (bb, "square.and.pencil", "New Text Compare", c.text.with_alpha(0.85)),
                    ] {
                        let (tw, th) = style::text_size(t, &f);
                        let x0 = r.mid_x() - (tw + 18.0) / 2.0;
                        style::draw_symbol(sym, Rect::new(x0, r.y, 14.0, r.height), fg, 11.0);
                        style::draw_text(t, x0 + 18.0, r.mid_y() - th / 2.0, &f, fg);
                    }
                }
                let (head, list) = self.recent_regions();
                style::draw_text("RECENT", x, head + 5.0, &style::system_font(11.0, Weight::Bold), c.dim);
                let af = style::system_font(12.5, Weight::Semibold);
                let actions: Vec<(i32, &str, String)> = [
                    (C_CLEAR_MISSING, "exclamationmark.triangle", format!("Clear Missing ({})", snap.recent.iter().filter(|r| r.missing()).count())),
                    (C_CLEAR_ALL, "trash", "Clear All".to_string()),
                ]
                .into_iter()
                .filter(|(id, _, _)| rect_of(*id).is_some())
                .collect();
                if let (Some(first), Some(last)) = (actions.first().and_then(|a| rect_of(a.0)), actions.last().and_then(|a| rect_of(a.0))) {
                    let geo = style::CAPSULE_GEO;
                    style::capsule_track(
                        Rect::new(
                            first.x - geo.track_pad,
                            first.y - geo.track_pad,
                            last.max_x() - first.x + 2.0 * geo.track_pad,
                            first.height + 2.0 * geo.track_pad,
                        ),
                        &c,
                    );
                    for (id, sym, t) in &actions {
                        let r = rect_of(*id).unwrap();
                        let hov = hover == Some(*id) || pressed == Some(*id);
                        style::capsule_chip(r, &c, false, hov);
                        let fg = if hov { c.text } else { c.text.with_alpha(0.85) };
                        let (tw, th) = style::text_size(t, &af);
                        let x0 = r.mid_x() - (tw + 18.0) / 2.0;
                        style::draw_symbol(sym, Rect::new(x0, r.y, 14.0, r.height), fg, 11.0);
                        style::draw_text(t, x0 + 18.0, r.mid_y() - th / 2.0, &af, fg);
                    }
                }
                // Recent list (CompareRecentList).
                let font = style::system_font(12.5, Weight::Medium);
                let small = style::system_font(11.5, Weight::Regular);
                let warn = style::system_font(11.0, Weight::Semibold);
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
                let home = std::env::var("HOME").unwrap_or_default();
                let tilde = |p: &str| -> String {
                    let dir = std::path::Path::new(p).parent().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
                    if !home.is_empty() && dir.starts_with(&home) { format!("~{}", &dir[home.len()..]) } else { dir }
                };
                let scroll = self.ivars().recent_scroll.get();
                objc2_app_kit::NSGraphicsContext::saveGraphicsState_class();
                objc2_app_kit::NSBezierPath::clipRect(ns(list));
                for (i, row) in snap.recent.iter().enumerate() {
                    let r = Rect::new(list.x, list.y + i as f64 * RECENT_ROW_H - scroll, list.width, RECENT_ROW_H);
                    if r.max_y() < list.y || r.y > list.max_y() {
                        continue;
                    }
                    let missing = row.missing();
                    if i == snap.recent_sel {
                        style::fill_rounded(r.inset_by(2.0, 2.0), 6.0, c.highlight.with_alpha(0.6));
                        style::fill_rect(Rect::new(r.x + 2.0, r.y + 5.0, 3.0, r.height - 10.0), c.accent_on());
                    }
                    let name_of = |p: &str, pasted: bool| if pasted { "pasted text".to_string() } else { std::path::Path::new(p).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default() };
                    let ln = name_of(&row.entry.left, row.pasted_left);
                    let rn = name_of(&row.entry.right, row.pasted_right);
                    let name = if ln == rn { ln } else { format!("{ln} ⇆ {rn}") };
                    let mut nx = r.x + 14.0;
                    if missing {
                        style::draw_symbol("exclamationmark.triangle.fill", Rect::new(r.x + 12.0, r.y, 16.0, r.height), c.tone(PopupTone::Danger), 11.0);
                        nx = r.x + 32.0;
                    }
                    style::draw_text_left(&name, Rect::new(nx, r.y, r.x + 234.0 - nx, r.height), &font, if missing { c.dim } else { c.text });
                    let hov = hover.is_some_and(|h| h == C_RECENT + i as i32 || h == C_RECENT + 10000 + i as i32);
                    let right_edge = r.max_x() - 12.0 - if hov { 28.0 } else { 0.0 };
                    let age = recent_age(row.entry.used, now);
                    let (agw, _) = style::text_size(&age, &small);
                    let mut age_x = right_edge - agw;
                    if missing {
                        let (tw, _) = style::text_size("missing", &warn);
                        style::draw_text_right("missing", Rect::new(r.x, r.y, right_edge - r.x, r.height), &warn, c.tone(PopupTone::Danger));
                        age_x -= tw + 10.0;
                    }
                    let dl = if row.pasted_left { "(pasted)".to_string() } else { tilde(&row.entry.left) };
                    let dr = if row.pasted_right { "(pasted)".to_string() } else { tilde(&row.entry.right) };
                    let dirs = if dl == dr { dl } else { format!("{dl}  ⇆  {dr}") };
                    let dw = (age_x - 14.0 - (r.x + 240.0)).max(40.0);
                    let drect = Rect::new(r.x + 240.0, r.y, dw, r.height);
                    let shown = style::draw_text_left(&dirs, drect, &small, if missing { c.dim.with_alpha(0.6) } else { c.dim });
                    if missing {
                        style::fill_rect(Rect::new(drect.x, r.mid_y() + 0.5, shown, 1.0), c.tone(PopupTone::Danger).with_alpha(0.7));
                    }
                    style::draw_text(&age, age_x, r.mid_y() - 7.0, &small, c.dim);
                    if hov {
                        let xr = Rect::new(r.max_x() - 30.0, r.y + 4.0, 20.0, 20.0);
                        let on_x = hover == Some(C_RECENT + 10000 + i as i32);
                        if on_x {
                            c.tone(PopupTone::Danger).with_alpha(0.2).to_nscolor().setFill();
                            objc2_app_kit::NSBezierPath::bezierPathWithOvalInRect(ns(xr)).fill();
                        }
                        style::cross(xr, if on_x { c.tone(PopupTone::Danger) } else { c.dim }, 3.5);
                    }
                }
                objc2_app_kit::NSGraphicsContext::restoreGraphicsState_class();
                if snap.recent.is_empty() {
                    style::draw_text("Nothing compared yet — pick two files or folders above.", list.x + 14.0, list.y + 6.0, &small, c.dim);
                }
                let _ = w;
                return;
            }
            // Toolbar.
            style::fill_rect(Rect::new(left, 0.0, b.size.width - left, TOOLBAR_H), c.mantle());
            let f = style::system_font(12.0, Weight::Medium);
            if let (Some(a), Some(z)) = (rect_of(C_FILTER), rect_of(C_FILTER + 3)) {
                let geo = style::CAPSULE_GEO;
                style::capsule_track(
                    Rect::new(
                        a.x - geo.track_pad,
                        a.y - geo.track_pad,
                        z.max_x() - a.x + 2.0 * geo.track_pad,
                        a.height + 2.0 * geo.track_pad,
                    ),
                    &c,
                );
                for i in 0..4 {
                    let r = rect_of(C_FILTER + i).unwrap();
                    let on = snap.filter == i as usize;
                    // Press shows on any chip (`TabPillView` semantics).
                    let hov = pressed == Some(C_FILTER + i) || (!on && hover == Some(C_FILTER + i));
                    style::capsule_chip(r, &c, on, hov);
                    style::draw_text_centered(FILTERS[i as usize], r, &f, if on { c.text } else if hov { c.text.with_alpha(0.9) } else { c.dim });
                }
                let first_tool = rect_of(C_TOOL).map(|r| r.x).unwrap_or(b.size.width);
                style::draw_text_left(
                    &snap.summary,
                    Rect::new(z.max_x() + 17.0, 0.0, (first_tool - z.max_x() - 24.0).max(40.0), TOOLBAR_H),
                    &style::system_font(12.0, Weight::Semibold),
                    if snap.identical { c.tone(PopupTone::Success) } else { c.text },
                );
            }
            for (i, (sym, _)) in TOOLS.iter().enumerate() {
                let id = C_TOOL + i as i32;
                if let Some(r) = rect_of(id) {
                    let st = ButtonState::from_flags(false, hover == Some(id), pressed == Some(id));
                    style::draw_button(r, st, &c, 4.0, true);
                    style::draw_symbol(sym, r, style::button_text(st, &c), 11.0);
                }
            }
            // Status line.
            style::draw_text_left(
                &snap.status,
                Rect::new(left + 12.0, b.size.height - STATUS_H + 4.0, b.size.width - left - 24.0, 16.0),
                &style::system_font(11.5, Weight::Regular),
                c.dim,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ages_read_like_swift() {
        assert_eq!(recent_age(100.0, 110.0), "just now");
        assert_eq!(recent_age(0.0, 600.0), "10 min ago");
        assert_eq!(recent_age(0.0, 7200.0), "2 h ago");
        assert_eq!(recent_age(0.0, 100_000.0), "yesterday");
    }
}
