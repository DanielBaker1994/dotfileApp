//! The live Confluence view — the AppKit half of Swift's
//! `ConfluenceWindow` (Confluence.swift): `ConfSegmented`, `ConfToggle`,
//! `JiraChoiceButton` / `JiraMultiPicker` chips, `ConfResultCell` rows, the
//! footer (status · Load more · Copy CQL | Copy curl) and the preview pane
//! (header, hits bar, the WKWebView, the empty hint + "Set Up Confluence…").
//! The model stays in [`super::confluence::ConfluenceModel`]; the host
//! pushes a [`ConfSnapshot`] and handles [`ConfEvent`]s.

use super::confluence::{ConfluenceRow, StatusTone};
use crate::ui::sidebar::{SidebarEvent, SidebarModel};

pub const STRIP_ROW_H: f64 = 26.0;
pub const STRIP_H: f64 = 10.0 + STRIP_ROW_H * 2.0 + 8.0 + 10.0;
pub const RESULT_H: f64 = 78.0;
pub const RESULT_GAP: f64 = 2.0;
pub const FOOT_H: f64 = 32.0;
pub const STAR_GUTTER: f64 = 30.0;

pub const MODES: [&str; 3] = ["All words", "Phrase", "Any word"];
pub const MODE_TIPS: [&str; 3] = [
    "Every word somewhere in the page (AND)",
    "The words together, in this order",
    "At least one of the words (OR)",
];
pub const TYPES: [(&str, &str); 6] = [
    ("Pages + blogs", "page,blogpost"),
    ("Pages", "page"),
    ("Blog posts", "blogpost"),
    ("Attachments", "attachment"),
    ("Comments", "comment"),
    ("Everything", "page,blogpost,attachment,comment"),
];
pub const MODIFIED: [(&str, &str); 5] = [
    ("Any time", ""),
    ("Past 7 days", "7d"),
    ("Past 30 days", "30d"),
    ("Past 90 days", "90d"),
    ("Past year", "1y"),
];
pub const SORTS: [(&str, &str); 2] = [("Relevance", "relevance"), ("Recently modified", "recent")];

pub fn title_for(items: &[(&str, &str)], value: &str) -> String {
    items.iter().find(|(_, v)| *v == value).map(|(t, _)| t.to_string()).unwrap_or_else(|| value.to_string())
}

/// The preview pane's state (`showPreviewHint` / `showHeader` / hits).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConfPreviewState {
    pub hint: String,
    pub setup: bool,
    /// (title, meta, favorite) once a page header shows.
    pub header: Option<(String, String, bool)>,
    pub hits: Option<String>,
    pub web: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConfSnapshot {
    pub sidebar: SidebarModel,
    pub mode: usize,
    pub title_only: bool,
    pub query: String,
    pub spaces: String,
    pub spaces_all: bool,
    pub people: String,
    pub people_all: bool,
    pub type_label: String,
    pub mod_label: String,
    pub sort_label: String,
    pub rows: Vec<ConfluenceRow>,
    pub selection: Option<usize>,
    pub empty: String,
    pub status: String,
    pub tone: Option<StatusTone>,
    pub searching: bool,
    pub can_more: bool,
    pub has_cql: bool,
    pub has_curl: bool,
    pub preview: ConfPreviewState,
    pub favorites_scope: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ConfEvent {
    Sidebar(SidebarEvent),
    Query(String),
    Enter,
    Move(i64),
    Escape,
    Mode(usize),
    TitleOnly(bool),
    Search,
    /// A chip menu: 0 spaces, 1 contributors, 2 type, 3 modified, 4 sort.
    Chip { which: usize, x: f64, y: f64 },
    Select { row: usize, double: bool },
    Star(usize),
    RowMenu { row: usize, x: f64, y: f64 },
    LoadMore,
    CopyCql,
    CopyCurl,
    Setup,
    PreviewStar,
    Open,
    CopyLink,
    PrevHit,
    NextHit,
}

// Control ids for the drawn widgets.
pub const C_MODE: i32 = 10; // + index
pub const C_TITLE: i32 = 20;
pub const C_SEARCH: i32 = 21;
pub const C_CHIP: i32 = 30; // + which
pub const C_MORE: i32 = 40;
pub const C_CQL: i32 = 41;
pub const C_CURL: i32 = 42;
pub const C_SETUP: i32 = 50;
pub const C_PSTAR: i32 = 51;
pub const C_POPEN: i32 = 52;
pub const C_PCOPY: i32 = 53;
pub const C_PREV: i32 = 54;
pub const C_NEXT: i32 = 55;

#[cfg(target_os = "macos")]
pub use appkit::ConfluenceView;

#[cfg(target_os = "macos")]
mod appkit {
    use super::*;
    use crate::ui::chrome::Rect;
    use crate::ui::pane_divider::{DividerRange, PaneDividerView};
    use crate::ui::sidebar::SidebarView;
    use crate::ui::style::{self, ButtonState, Weight};
    use crate::ui::theme::{PopupColors, PopupTone, Rgba};
    use crate::views::jira_view::draw_wrapped;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, ProtocolObject, Sel};
    use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSControlTextEditingDelegate, NSEvent, NSProgressIndicator, NSProgressIndicatorStyle,
        NSScrollView, NSTextField, NSTextFieldDelegate, NSTextView, NSTrackingArea,
        NSView, NSControlSize,
    };
    use objc2_foundation::{NSNotification, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use crate::ui::view_base::{self};

    fn ns(r: Rect) -> NSRect {
        style::ns(r)
    }

    type Sink = Rc<dyn Fn(ConfEvent)>;


    fn tone_rgba(c: &PopupColors, t: Option<StatusTone>) -> Rgba {
        match t {
            Some(StatusTone::Success) => c.tone(PopupTone::Success),
            Some(StatusTone::Warning) => c.tone(PopupTone::Warning),
            Some(StatusTone::Danger) => c.tone(PopupTone::Danger),
            _ => c.dim,
        }
    }

    // ------------------------------------------------------- result rows

    pub struct ResultsIvars {
        snap: Rc<RefCell<ConfSnapshot>>,
        colors: Cell<PopupColors>,
        hover: Cell<Option<usize>>,
        sink: RefCell<Option<Sink>>,
        tracking: RefCell<Option<Retained<NSTrackingArea>>>,
    }

    define_class!(
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSConfResultsView"]
        #[ivars = ResultsIvars]
        pub struct ConfResultsView;

        impl ConfResultsView {
            #[unsafe(method(isFlipped))]
            fn is_flipped(&self) -> bool { true }
            #[unsafe(method(acceptsFirstResponder))]
            fn afr(&self) -> bool { true }
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
                let ev = if p.x < STAR_GUTTER {
                    ConfEvent::Star(row)
                } else {
                    ConfEvent::Select { row, double: e.clickCount() >= 2 }
                };
                self.emit(ev);
            }

            #[unsafe(method(rightMouseDown:))]
            fn right_mouse_down(&self, e: &NSEvent) {
                let p = view_base::mouse_point(self, e);
                let Some(row) = self.row_at(p.y) else { return };
                let s = NSEvent::mouseLocation();
                self.emit(ConfEvent::RowMenu { row, x: s.x, y: s.y });
            }

            #[unsafe(method(drawRect:))]
            fn draw_rect(&self, _dirty: NSRect) {
                self.draw();
            }
        }

        unsafe impl NSObjectProtocol for ConfResultsView {}
    );

    impl ConfResultsView {
        fn new(mtm: MainThreadMarker, snap: Rc<RefCell<ConfSnapshot>>, colors: PopupColors) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(ResultsIvars {
                snap,
                colors: Cell::new(colors),
                hover: Cell::new(None),
                sink: RefCell::new(None),
                tracking: RefCell::new(None),
            });
            unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] }
        }

        fn emit(&self, ev: ConfEvent) {
            let s = self.ivars().sink.borrow().clone();
            if let Some(s) = s {
                s(ev);
            }
        }

        fn row_at(&self, y: f64) -> Option<usize> {
            let n = self.ivars().snap.borrow().rows.len();
            let i = (y / (RESULT_H + RESULT_GAP)).floor();
            (i >= 0.0 && (i as usize) < n).then_some(i as usize)
        }

        /// The visible result rows' rects (for the vim search highlight).
        pub fn vim_shown_rects(&self) -> Vec<(usize, Rect)> {
            let n = self.ivars().snap.borrow().rows.len();
            let vis = self.visibleRect();
            let row_h = RESULT_H + RESULT_GAP;
            if n == 0 || row_h <= 0.0 || vis.size.height <= 0.0 {
                return Vec::new();
            }
            let first = (vis.origin.y / row_h).floor().max(0.0) as usize;
            let last = (((vis.origin.y + vis.size.height) / row_h).floor() as usize).min(n - 1);
            if first > last {
                return Vec::new();
            }
            let w = self.bounds().size.width;
            (first..=last).map(|i| (i, Rect::new(0.0, i as f64 * row_h, w, RESULT_H))).collect()
        }

        fn draw(&self) {
            let c = self.ivars().colors.get();
            let snap = self.ivars().snap.borrow();
            let w = self.bounds().size.width;
            if snap.rows.is_empty() {
                if !snap.empty.is_empty() {
                    let f = style::system_font(12.5, Weight::Regular);
                    draw_wrapped(&snap.empty, Rect::new(24.0, 24.0, w - 48.0, 80.0), &f, c.dim);
                }
                return;
            }
            let x0 = STAR_GUTTER;
            let tw = w - x0 - 12.0;
            let hover = self.ivars().hover.get();
            for (i, r) in snap.rows.iter().enumerate() {
                let y = i as f64 * (RESULT_H + RESULT_GAP);
                let band = Rect::new(0.0, y, w, RESULT_H);
                if snap.selection == Some(i) {
                    style::fill_rounded(band.inset_by(4.0, 0.0), 6.0, c.highlight);
                } else if hover == Some(i) {
                    style::fill_rounded(band.inset_by(4.0, 0.0), 6.0, style::button_fill(ButtonState::Hover, &c));
                }
                let (star, sc) = if r.favorite {
                    ("★", c.tone(PopupTone::Warning))
                } else {
                    ("☆", c.dim.with_alpha(0.55))
                };
                style::draw_text(star, 10.0, y + 7.0, &style::system_font(14.0, Weight::Regular), sc);
                let tf = style::system_font(13.5, Weight::Semibold);
                let mut tx = x0;
                let label = r.type_label();
                if !label.is_empty() {
                    let lf = style::system_font(9.5, Weight::Bold);
                    let t = format!("{}  ", label.to_uppercase());
                    let (lw, lh) = style::text_size(&t, &lf);
                    style::draw_text(&t, tx, y + 6.0 + (19.0 - lh) / 2.0, &lf, c.tone(PopupTone::Accent2));
                    tx += lw;
                }
                let title = if r.title.is_empty() { "(untitled)".to_string() } else { r.title.clone() };
                style::draw_text_left(&title, Rect::new(tx, y + 6.0, (x0 + tw - tx).max(0.0), 19.0), &tf, if r.missing { c.dim } else { c.text });
                let mut meta: Vec<String> = vec![if r.space_name.is_empty() { r.space.clone() } else { r.space_name.clone() }];
                if !r.path.is_empty() {
                    meta.push(r.path.clone());
                } else if !r.container.is_empty() {
                    meta.push(format!("on {}", r.container));
                }
                if !r.author.is_empty() {
                    meta.push(r.author.clone());
                }
                if !r.modified_text.is_empty() {
                    meta.push(r.modified_text.clone());
                }
                let meta = meta.into_iter().filter(|m| !m.is_empty()).collect::<Vec<_>>().join("  ·  ");
                style::draw_text_left(&meta, Rect::new(x0, y + 26.0, tw, 16.0), &style::system_font(11.0, Weight::Regular), c.dim);
                draw_wrapped(&r.excerpt, Rect::new(x0, y + 43.0, tw, 32.0), &style::system_font(12.0, Weight::Regular), c.text.with_alpha(0.78));
            }
        }
    }

    // --------------------------------------------------------- container

    pub struct ConfIvars {
        snap: Rc<RefCell<ConfSnapshot>>,
        colors: Cell<PopupColors>,
        sidebar: Retained<SidebarView>,
        search: Retained<NSTextField>,
        search_box: Retained<NSView>,
        scroll: Retained<NSScrollView>,
        results: Retained<ConfResultsView>,
        spinner: Retained<NSProgressIndicator>,
        web: Retained<NSView>,
        sink: RefCell<Option<Sink>>,
        hover: Cell<Option<i32>>,
        pressed: Cell<Option<i32>>,
        tracking: RefCell<Option<Retained<NSTrackingArea>>>,
        sidebar_w: Cell<f64>,
        split: Cell<f64>,
        /// The draggable divider on the sidebar's right edge.
        divider: Retained<PaneDividerView>,
    }

    define_class!(
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSConfluenceView"]
        #[ivars = ConfIvars]
        pub struct ConfluenceView;

        impl ConfluenceView {

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
                    C_MODE..=12 => Some(MODE_TIPS[(id - C_MODE) as usize].to_string()),
                    C_TITLE => Some("Match page titles only".into()),
                    C_SEARCH => Some("Search (Return)".into()),
                    C_MORE => Some("Fetch the next page of results".into()),
                    C_CQL => Some("Copy the CQL of this search".into()),
                    C_CURL => Some("Copy this search as a runnable curl ($CONFLUENCE_TOKEN)".into()),
                    C_SETUP => Some("Site, email, token and the spaces to search".into()),
                    C_POPEN => Some("Open the page in your browser (Return)".into()),
                    C_PCOPY => Some("Copy the page link (Cmd+C)".into()),
                    C_PSTAR => Some("Star (Cmd+D)".into()),
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

            #[unsafe(method(mouseDown:))]
            fn mouse_down(&self, e: &NSEvent) {
                let p = view_base::mouse_point(self, e);
                self.ivars().pressed.set(self.hit(p.x, p.y));
                self.setNeedsDisplay(true);
            }

            #[unsafe(method(mouseUp:))]
            fn mouse_up(&self, e: &NSEvent) {
                let p = view_base::mouse_point(self, e);
                let h = self.hit(p.x, p.y);
                let was = self.ivars().pressed.replace(None);
                self.setNeedsDisplay(true);
                let Some(id) = h.filter(|_| h == was) else { return };
                let rect = self.controls().into_iter().find(|(i, _)| *i == id).map(|(_, r)| r).unwrap_or(Rect::ZERO);
                let title_only = self.ivars().snap.borrow().title_only;
                let ev = match id {
                    C_MODE..=12 => ConfEvent::Mode((id - C_MODE) as usize),
                    C_TITLE => ConfEvent::TitleOnly(!title_only),
                    C_SEARCH => ConfEvent::Search,
                    C_CHIP..=34 => ConfEvent::Chip { which: (id - C_CHIP) as usize, x: rect.x, y: rect.max_y() + 2.0 },
                    C_MORE => ConfEvent::LoadMore,
                    C_CQL => ConfEvent::CopyCql,
                    C_CURL => ConfEvent::CopyCurl,
                    C_SETUP => ConfEvent::Setup,
                    C_PSTAR => ConfEvent::PreviewStar,
                    C_POPEN => ConfEvent::Open,
                    C_PCOPY => ConfEvent::CopyLink,
                    C_PREV => ConfEvent::PrevHit,
                    C_NEXT => ConfEvent::NextHit,
                    _ => return,
                };
                self.emit(ev);
            }

            #[unsafe(method(drawRect:))]
            fn draw_rect(&self, _dirty: NSRect) {
                self.draw();
            }

            #[unsafe(method(controlTextDidChange:))]
            fn control_text_did_change(&self, _n: &NSNotification) {
                let q = self.ivars().search.stringValue().to_string();
                self.ivars().snap.borrow_mut().query = q.clone();
                self.emit(ConfEvent::Query(q));
            }

            #[unsafe(method(control:textView:doCommandBySelector:))]
            fn do_command(&self, _c: &AnyObject, _tv: &NSTextView, cmd: Sel) -> objc2::runtime::Bool {
                let ev = if cmd == sel!(insertNewline:) {
                    ConfEvent::Enter
                } else if cmd == sel!(moveDown:) {
                    ConfEvent::Move(1)
                } else if cmd == sel!(moveUp:) {
                    ConfEvent::Move(-1)
                } else if cmd == sel!(cancelOperation:) {
                    ConfEvent::Escape
                } else {
                    return objc2::runtime::Bool::NO;
                };
                self.emit(ev);
                objc2::runtime::Bool::YES
            }
        }

        unsafe impl NSObjectProtocol for ConfluenceView {}
        unsafe impl NSControlTextEditingDelegate for ConfluenceView {}
        unsafe impl NSTextFieldDelegate for ConfluenceView {}
    );

    impl ConfluenceView {
        /// `ConfluenceWindow.navPanes`: sidebar, search, results, preview.
        pub fn nav_panes(&self) -> Vec<crate::app::live_panes::LivePane> {
            use crate::app::live_panes::LivePane;
            let iv = self.ivars();
            vec![
                LivePane::new("sidebar", &iv.sidebar),
                LivePane::new("search", &iv.search_box).focusing(&iv.search),
                LivePane::new("results", &iv.scroll).focusing(&iv.results),
                LivePane::new("preview", &iv.web),
            ]
        }
        pub fn new(
            mtm: MainThreadMarker,
            colors: PopupColors,
            web: Retained<NSView>,
            sidebar_w: f64,
            sidebar_range: DividerRange,
            split: f64,
        ) -> Retained<Self> {
            let snap = Rc::new(RefCell::new(ConfSnapshot::default()));
            let sidebar = SidebarView::new(mtm, colors);
            sidebar.set_collapse_key("confluence");
            let (search_box, search) = crate::ui::field::search_box(
                mtm,
                &colors,
                13.0,
                "Search Confluence    \"exact phrase\"    prefix*",
                6.0,
                STRIP_ROW_H + 4.0,
            );
            let scroll = crate::ui::style::plain_scroll(mtm);
            let results = ConfResultsView::new(mtm, snap.clone(), colors);
            scroll.setDocumentView(Some(&results));
            let spinner = NSProgressIndicator::new(mtm);
            spinner.setStyle(NSProgressIndicatorStyle::Spinning);
            spinner.setControlSize(NSControlSize::Small);
            spinner.setDisplayedWhenStopped(false);
            let divider = PaneDividerView::new(mtm, sidebar_range);
            let this = Self::alloc(mtm).set_ivars(ConfIvars {
                snap,
                colors: Cell::new(colors),
                sidebar,
                search,
                search_box,
                scroll,
                results,
                spinner,
                web,
                sink: RefCell::new(None),
                hover: Cell::new(None),
                pressed: Cell::new(None),
                tracking: RefCell::new(None),
                sidebar_w: Cell::new(sidebar_w),
                split: Cell::new(split),
                divider: divider.clone(),
            });
            let v: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
            let iv = v.ivars();
            for sub in [&*iv.sidebar as &NSView, &iv.search_box, &iv.scroll, &iv.spinner, &iv.web] {
                v.addSubview(sub);
            }
            // Above every pane so the divider's strip never reads as a click.
            v.addSubview(&divider);
            unsafe { iv.search.setDelegate(Some(ProtocolObject::from_ref(&*v))) };
            v
        }

        pub fn set_on_event(&self, cb: Box<dyn Fn(ConfEvent)>) {
            let rc: Sink = Rc::from(cb);
            *self.ivars().sink.borrow_mut() = Some(rc.clone());
            *self.ivars().results.ivars().sink.borrow_mut() = Some(rc.clone());
            self.ivars().sidebar.set_on_event(Box::new(move |e| rc(ConfEvent::Sidebar(e))));
        }

        fn emit(&self, ev: ConfEvent) {
            let s = self.ivars().sink.borrow().clone();
            if let Some(s) = s {
                s(ev);
            }
        }

        pub fn search_field(&self) -> Retained<NSTextField> {
            self.ivars().search.clone()
        }

        /// The draggable divider on the sidebar's right edge.
        pub fn divider(&self) -> Retained<PaneDividerView> {
            self.ivars().divider.clone()
        }

        /// The expanded sidebar width (`[confluence] sidebar-width`).
        pub fn sidebar_width(&self) -> f64 {
            self.ivars().sidebar_w.get()
        }

        /// Apply a divider drag / `do:sidebar-width:confluence`.
        pub fn set_sidebar_width(&self, width: f64) {
            let w = self.ivars().divider.clamp(width);
            self.ivars().sidebar_w.set(w);
            self.layout_all();
        }

        /// Set the collapse-to-rail state from the host (Ctrl+B B /
        /// `do:rail:confluence`): flip the sidebar (which persists it),
        /// then re-lay-out exactly as the in-view `SidebarEvent::Rail` does.
        pub fn set_rail(&self, on: bool) {
            let sb = self.ivars().sidebar.clone();
            if sb.collapsed() != on {
                sb.set_collapsed(on);
            }
            self.layout_all();
        }

        pub fn results_view(&self) -> Retained<NSView> {
            Retained::into_super(self.ivars().results.clone())
        }

        /// The results list's document view plus each visible row's rect (the
        /// vim search highlight draws the overlay into it).
        pub fn vim_shown_rows(&self) -> (Retained<NSView>, Vec<(usize, Rect)>) {
            let r = self.ivars().results.clone();
            let rects = r.vim_shown_rects();
            (Retained::into_super(r), rects)
        }

        pub fn apply(&self, s: ConfSnapshot) {
            let iv = self.ivars();
            iv.sidebar.set_model(s.sidebar.clone());
            let editing = iv.search.currentEditor().is_some();
            if !editing && iv.search.stringValue().to_string() != s.query {
                iv.search.setStringValue(&NSString::from_str(&s.query));
            }
            if s.searching {
                unsafe { iv.spinner.startAnimation(None) };
            } else {
                unsafe { iv.spinner.stopAnimation(None) };
            }
            iv.web.setHidden(!s.preview.web);
            let moved = iv.snap.borrow().selection != s.selection;
            let sel = s.selection;
            *iv.snap.borrow_mut() = s;
            self.layout_all();
            // Keep a newly selected row on screen (arrowing past the fold).
            if let (true, Some(i)) = (moved, sel) {
                let row_h = RESULT_H + RESULT_GAP;
                let w = iv.results.bounds().size.width;
                iv.results.scrollRectToVisible(ns(Rect::new(0.0, i as f64 * row_h, w, row_h)));
            }
            iv.results.setNeedsDisplay(true);
            self.setNeedsDisplay(true);
        }

        fn left(&self) -> f64 {
            self.ivars().sidebar.width(self.ivars().sidebar_w.get()) + 4.0
        }

        /// (strip, table, footer y, preview) in view coordinates.
        fn regions(&self) -> (Rect, Rect, f64, Rect) {
            let b = self.bounds();
            let left = self.left();
            let w = (b.size.width - left).max(300.0);
            let h = b.size.height;
            let strip = Rect::new(left, 0.0, w, STRIP_H);
            let body_h = h - STRIP_H;
            let left_w = (w * self.ivars().split.get()).round();
            let table = Rect::new(left, STRIP_H + 4.0, left_w, (body_h - FOOT_H - 4.0).max(0.0));
            let fy = h - FOOT_H + 5.0;
            let preview = Rect::new(left + left_w + 6.0, STRIP_H, (w - left_w - 6.0).max(0.0), body_h.max(0.0));
            (strip, table, fy, preview)
        }

        pub fn layout_all(&self) {
            let iv = self.ivars();
            let b = self.bounds();
            let sw = iv.sidebar.width(iv.sidebar_w.get());
            iv.sidebar.setFrame(ns(Rect::new(0.0, 0.0, sw, b.size.height)));
            iv.divider.place(ns(Rect::new(sw - 4.0, 0.0, 8.0, b.size.height)));
            iv.divider.set_width(sw);
            iv.divider.setHidden(iv.sidebar.collapsed());
            let (strip, table, fy, preview) = self.regions();
            let seg_x = self.top_row_right_start(strip);
            iv.search_box.setFrame(ns(Rect::new(strip.x + 12.0, 8.0, (seg_x - 8.0 - strip.x - 12.0).max(80.0), STRIP_ROW_H + 4.0)));
            iv.scroll.setFrame(ns(table));
            let n = iv.snap.borrow().rows.len() as f64;
            iv.results.setFrame(NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(table.width, (n * (RESULT_H + RESULT_GAP)).max(table.height)),
            ));
            iv.spinner.setFrame(ns(Rect::new(table.x + 12.0, fy + 3.0, 16.0, 16.0)));
            iv.web.setFrame(ns(Rect::new(preview.x, preview.y + 84.0, preview.width, (preview.height - 84.0).max(0.0))));
        }

        fn seg_width(&self) -> f64 {
            let f = style::system_font(12.0, Weight::Medium);
            style::capsule_metrics(&MODES, &f).1
        }

        fn top_row_right_start(&self, strip: Rect) -> f64 {
            let f = style::system_font(12.0, Weight::Medium);
            let toggle = style::capsule_metrics(&["Title only"], &f).0[0];
            let search = style::text_size("Search", &style::system_font(12.0, Weight::Semibold)).0.ceil() + 30.0;
            strip.max_x() - 12.0 - search - 8.0 - toggle - 8.0 - self.seg_width()
        }

        /// Every drawn control's rect.
        fn controls(&self) -> Vec<(i32, Rect)> {
            let mut out = Vec::new();
            let snap = self.ivars().snap.borrow();
            let (strip, table, fy, preview) = self.regions();
            let f = style::system_font(12.0, Weight::Medium);
            // Top row, right to left: Search, Title only, mode segments.
            let mut x = strip.max_x() - 12.0;
            let sw = style::text_size("Search", &style::system_font(12.0, Weight::Semibold)).0.ceil() + 30.0;
            x -= sw;
            out.push((C_SEARCH, Rect::new(x, 10.0, sw, STRIP_ROW_H)));
            let tw = style::capsule_metrics(&["Title only"], &f).0[0];
            x -= 8.0 + tw;
            out.push((C_TITLE, Rect::new(x, 10.0, tw, STRIP_ROW_H)));
            let seg = self.seg_width();
            x -= 8.0 + seg;
            let geo = style::CAPSULE_GEO;
            let (ws, _) = style::capsule_metrics(&MODES, &f);
            let mut sx = x + geo.track_pad;
            for (i, w) in ws.iter().enumerate() {
                // Canonical chip: 20 tall inside the row (track_pad inset), so
                // the drawn pill is the same 26-pt capsule as `TabPillView`.
                out.push((
                    C_MODE + i as i32,
                    Rect::new(sx, 10.0 + geo.track_pad, *w, STRIP_ROW_H - 2.0 * geo.track_pad),
                ));
                sx += w + geo.gutter;
            }
            // Bottom row chips.
            let y2 = 10.0 + STRIP_ROW_H + 8.0;
            let mut cx = strip.x + 12.0;
            let labels = [
                (190.0, String::new()),
                (190.0, String::new()),
                (0.0, snap.type_label.clone()),
                (0.0, snap.mod_label.clone()),
                (0.0, snap.sort_label.clone()),
            ];
            for (i, (fixed, t)) in labels.iter().enumerate() {
                let w = if *fixed > 0.0 { *fixed } else { style::text_size(t, &f).0.ceil() + 34.0 };
                out.push((C_CHIP + i as i32, Rect::new(cx, y2, w, STRIP_ROW_H)));
                cx += w + 8.0;
            }
            // Footer, right to left inside the table column.
            let mut fx = table.max_x() - 10.0;
            let bf = style::system_font(12.0, Weight::Medium);
            let curl_w = style::text_size("Copy curl", &bf).0.ceil() + 22.0;
            fx -= curl_w;
            out.push((C_CURL, Rect::new(fx, fy, curl_w, 22.0)));
            let cql_w = style::text_size("Copy CQL", &bf).0.ceil() + 22.0;
            fx -= cql_w;
            out.push((C_CQL, Rect::new(fx, fy, cql_w, 22.0)));
            fx -= 10.0;
            let more_w = style::text_size("Load more", &bf).0.ceil() + 30.0;
            fx -= more_w;
            out.push((C_MORE, Rect::new(fx, fy, more_w, 22.0)));
            // Preview.
            let p = &snap.preview;
            if p.setup {
                let sf = style::system_font(13.0, Weight::Semibold);
                let w = style::text_size("Set Up Confluence…", &sf).0.ceil() + 48.0;
                out.push((C_SETUP, Rect::new(preview.x + (preview.width - w) / 2.0, preview.y + preview.height / 2.0 + 8.0, w, 30.0)));
            }
            if p.header.is_some() {
                let mut bx = preview.max_x() - 12.0;
                for (id, t) in [(C_PCOPY, "Copy Link"), (C_POPEN, "Open")] {
                    let w = style::text_size(t, &bf).0.ceil() + 28.0;
                    bx -= w;
                    out.push((id, Rect::new(bx, preview.y + 12.0, w, 22.0)));
                    bx -= 6.0;
                }
                bx -= 22.0;
                out.push((C_PSTAR, Rect::new(bx, preview.y + 12.0, 22.0, 22.0)));
            }
            if p.hits.is_some() {
                out.push((C_PREV, Rect::new(preview.x + 10.0, preview.y + 56.0 + 3.0, 26.0, 22.0)));
                out.push((C_NEXT, Rect::new(preview.x + 38.0, preview.y + 56.0 + 3.0, 26.0, 22.0)));
            }
            out
        }

        fn hit(&self, x: f64, y: f64) -> Option<i32> {
            self.controls().into_iter().find(|(_, r)| r.contains(x, y)).map(|(id, _)| id)
        }

        fn state_for(&self, id: i32, on: bool) -> ButtonState {
            if self.ivars().pressed.get() == Some(id) {
                return ButtonState::Pressed;
            }
            ButtonState::from_flags(on, self.ivars().hover.get() == Some(id), false)
        }

        fn draw(&self) {
            let c = self.ivars().colors.get();
            let (strip, table, fy, preview) = self.regions();
            let snap = self.ivars().snap.borrow();
            let controls = self.controls();
            let rect_of = |id: i32| controls.iter().find(|(i, _)| *i == id).map(|(_, r)| *r);
            let hover = self.ivars().hover.get();
            let pressed = self.ivars().pressed.get();
            // Strip + panes.
            style::fill_rect(strip, c.mantle().with_alpha(0.55));
            style::fill_rect(Rect::new(strip.x, STRIP_H, strip.width, preview.height), c.mantle().with_alpha(0.35));
            style::fill_rect(preview, c.mantle().with_alpha(0.15));
            style::fill_rect(Rect::new(table.max_x(), STRIP_H, 6.0, preview.height), c.mantle().with_alpha(0.2));
            let f = style::system_font(12.0, Weight::Medium);
            let geo = style::CAPSULE_GEO;
            // Mode segments (ConfSegmented).
            if let (Some(a), Some(b)) = (rect_of(C_MODE), rect_of(C_MODE + 2)) {
                style::capsule_track(
                    Rect::new(
                        a.x - geo.track_pad,
                        a.y - geo.track_pad,
                        b.max_x() - a.x + 2.0 * geo.track_pad,
                        a.height + 2.0 * geo.track_pad,
                    ),
                    &c,
                );
                for i in 0..3 {
                    let r = rect_of(C_MODE + i as i32).unwrap();
                    let on = snap.mode == i;
                    // Same press semantics as `TabPillView`: a press shows on
                    // any chip (it darkens even the selected one).
                    let hov = pressed == Some(C_MODE + i as i32) || (!on && hover == Some(C_MODE + i as i32));
                    style::capsule_chip(r, &c, on, hov);
                    let col = if on { c.text } else if hov { c.text.with_alpha(0.9) } else { c.dim };
                    style::draw_text_centered(MODES[i], r, &f, col);
                }
            }
            // Title only (ConfToggle).
            if let Some(r) = rect_of(C_TITLE) {
                let on = snap.title_only;
                let hov = pressed == Some(C_TITLE) || (!on && hover == Some(C_TITLE));
                style::capsule_track(r, &c);
                style::capsule_chip(r.inset_by(geo.track_pad, geo.track_pad), &c, on, hov);
                let col = if on { c.text } else if hov { c.text.with_alpha(0.9) } else { c.dim };
                style::draw_text_centered("Title only", r, &f, col);
            }
            // Search (primary).
            if let Some(r) = rect_of(C_SEARCH) {
                style::capsule_outline(r, &c, hover == Some(C_SEARCH));
                style::draw_text_centered("Search", r, &style::system_font(12.0, Weight::Semibold), c.accent);
            }
            // Pickers (JiraMultiPicker) + choices (JiraChoiceButton).
            for i in 0..5 {
                let Some(r) = rect_of(C_CHIP + i) else { continue };
                let hov = hover == Some(C_CHIP + i);
                if i < 2 {
                    style::fill_rounded(r, 6.0, if hov { style::input_fill(&c).blended(0.06, c.text) } else { style::input_fill(&c) });
                    style::stroke_rounded(r, 6.0, 1.0, style::input_stroke(&c));
                    style::chevron(Rect::new(r.max_x() - 20.0, r.y, 14.0, r.height), c.dim);
                    let (text, all) = if i == 0 { (&snap.spaces, snap.spaces_all) } else { (&snap.people, snap.people_all) };
                    if all {
                        let pf = style::system_font(12.0, Weight::Medium);
                        let (pw, _) = style::text_size(text, &pf);
                        let pill = Rect::new(r.x + 4.0, r.y + 4.0, (pw.ceil() + 14.0).min(r.width - 30.0), r.height - 8.0);
                        style::fill_rounded(pill, 4.0, c.text.with_alpha(0.13));
                        style::draw_text_left(text, Rect::new(pill.x + 7.0, pill.y, pill.width - 10.0, pill.height), &pf, c.text);
                    } else {
                        style::draw_text_left(text, Rect::new(r.x + 9.0, r.y, r.width - 34.0, r.height), &style::system_font(12.0, Weight::Regular), c.dim.with_alpha(0.8));
                    }
                } else {
                    let st = self.state_for(C_CHIP + i, false);
                    style::draw_button(r, st, &c, 6.0, false);
                    let t = match i {
                        2 => &snap.type_label,
                        3 => &snap.mod_label,
                        _ => &snap.sort_label,
                    };
                    style::draw_text_left(t, Rect::new(r.x + 10.0, r.y, r.width - 30.0, r.height), &f, style::button_text(ButtonState::Hover, &c));
                    style::chevron(Rect::new(r.max_x() - 20.0, r.y, 14.0, r.height), c.dim);
                }
            }
            // Footer: status, Load more, Copy CQL | Copy curl.
            let bf = style::system_font(12.0, Weight::Medium);
            if let Some(more) = rect_of(C_MORE) {
                let status_r = Rect::new(table.x + 32.0, fy + 3.0, (more.x - table.x - 36.0).max(40.0), 16.0);
                style::draw_text_left(&snap.status, status_r, &style::system_font(11.0, Weight::Regular), tone_rgba(&c, snap.tone));
                let enabled = snap.can_more;
                let hov = hover == Some(C_MORE) && enabled;
                let pres = pressed == Some(C_MORE) && enabled;
                style::stroke_rounded(more, more.height / 2.0, 1.0, c.text.with_alpha(if enabled { 0.25 } else { 0.12 }));
                if pres {
                    style::fill_rounded(more, more.height / 2.0, c.text.with_alpha(0.14));
                } else if hov {
                    style::fill_rounded(more, more.height / 2.0, c.text.with_alpha(0.08));
                }
                style::draw_text_centered("Load more", more, &bf, if enabled { c.text.with_alpha(0.85) } else { c.dim.with_alpha(0.6) });
            }
            if let (Some(cq), Some(cu)) = (rect_of(C_CQL), rect_of(C_CURL)) {
                let both = Rect::new(cq.x, cq.y, cu.max_x() - cq.x, cq.height);
                style::capsule_track(both, &c);
                for (id, r, t, en) in [(C_CQL, cq, "Copy CQL", snap.has_cql), (C_CURL, cu, "Copy curl", snap.has_curl)] {
                    if en && (hover == Some(id) || pressed == Some(id)) {
                        style::capsule_chip(r.inset_by(2.0, 2.0), &c, false, true);
                    }
                    style::draw_text_centered(t, r, &bf, if en { c.text.with_alpha(0.85) } else { c.dim.with_alpha(0.6) });
                }
                style::vline(cu.x, cu.y + 5.0, cu.max_y() - 5.0, c.text.with_alpha(0.14));
            }
            // Preview.
            let p = &snap.preview;
            if let Some((title, meta, fav)) = &p.header {
                let star = rect_of(C_PSTAR).unwrap_or(Rect::ZERO);
                style::draw_text_left(title, Rect::new(preview.x + 16.0, preview.y + 10.0, (star.x - preview.x - 22.0).max(40.0), 20.0), &style::system_font(15.0, Weight::Semibold), c.text);
                style::draw_text_left(meta, Rect::new(preview.x + 16.0, preview.y + 32.0, preview.width - 32.0, 16.0), &style::system_font(11.0, Weight::Regular), c.dim);
                style::draw_button(star, self.state_for(C_PSTAR, *fav), &c, 6.0, false);
                style::draw_symbol(if *fav { "star.fill" } else { "star" }, star, if *fav { c.tone(PopupTone::Warning) } else { c.dim }, 11.0);
                for (id, t) in [(C_POPEN, "Open"), (C_PCOPY, "Copy Link")] {
                    if let Some(r) = rect_of(id) {
                        style::capsule_track(r, &c);
                        if hover == Some(id) {
                            style::capsule_chip(r.inset_by(2.0, 2.0), &c, false, true);
                        }
                        style::draw_text_centered(t, r, &bf, c.text.with_alpha(0.9));
                    }
                }
            }
            if let Some(hits) = &p.hits {
                style::fill_rect(Rect::new(preview.x, preview.y + 56.0, preview.width, 28.0), c.mantle().with_alpha(0.7));
                for (id, sym) in [(C_PREV, "chevron.left"), (C_NEXT, "chevron.right")] {
                    if let Some(r) = rect_of(id) {
                        style::draw_button(r, self.state_for(id, false), &c, 6.0, false);
                        style::draw_symbol(sym, r, c.text.with_alpha(0.85), 10.0);
                    }
                }
                style::draw_text_left(hits, Rect::new(preview.x + 72.0, preview.y + 62.0, preview.width - 84.0, 16.0), &style::system_font(12.0, Weight::Regular), c.text.with_alpha(0.85));
            }
            if !p.hint.is_empty() {
                let hf = style::system_font(13.0, Weight::Regular);
                let r = Rect::new(preview.x + 40.0, preview.y + preview.height / 2.0 - 30.0, (preview.width - 80.0).max(0.0), 40.0);
                let lines: Vec<&str> = p.hint.lines().collect();
                for (i, l) in lines.iter().enumerate() {
                    style::draw_text_centered(l, Rect::new(r.x, r.y + i as f64 * 18.0, r.width, 18.0), &hf, c.text.with_alpha(0.85));
                }
            }
            if let Some(r) = rect_of(C_SETUP) {
                style::capsule_outline(r, &c, hover == Some(C_SETUP));
                style::draw_text_centered("Set Up Confluence…", r, &style::system_font(13.0, Weight::Semibold), c.accent);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choice_titles_map_values() {
        assert_eq!(title_for(&TYPES, "page,blogpost"), "Pages + blogs");
        assert_eq!(title_for(&MODIFIED, "30d"), "Past 30 days");
        // The default (empty wire value) is the label the chip shows at rest.
        assert_eq!(title_for(&MODIFIED, ""), "Any time");
        assert_eq!(title_for(&SORTS, "relevance"), "Relevance");
        assert_eq!(title_for(&SORTS, "recent"), "Recently modified");
        // An unknown value falls back to the raw value.
        assert_eq!(title_for(&SORTS, "x"), "x");
    }

    #[test]
    fn strip_height_matches_swift() {
        assert_eq!(STRIP_H, 10.0 + 26.0 * 2.0 + 8.0 + 10.0);
    }
}
