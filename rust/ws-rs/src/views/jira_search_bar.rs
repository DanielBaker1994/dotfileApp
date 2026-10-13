//! The Jira live-search bar (`JiraSearchPanel`, JiraSearch.swift): Cmd+F
//! attaches it above the issue list. Top row: search text, Projects picker,
//! "+ Filter", Search, ×. One row per added criterion (people / list
//! pickers, a date range choice or a contains-text field, ⊖ removes). Footer:
//! spinner + status, "Fetch from Jira" (empty directory), "Max N", Copy JQL,
//! Copy curl. The host owns the search itself; this view owns the controls
//! and reports [`SearchEvent`]s.

use crate::views::jira::{FilterKind, SearchRow, ValueKind};

/// `JiraSearchPanel.dateRanges` — `[jira] search-date-ranges`, else the
/// defaults.
pub fn date_ranges(cfg: Option<&str>) -> Vec<String> {
    let r: Vec<String> = cfg
        .unwrap_or("")
        .split(',')
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty())
        .collect();
    if r.is_empty() {
        ["today", "2d", "7d", "14d", "30d", "90d"].iter().map(|s| s.to_string()).collect()
    } else {
        r
    }
}

/// `JiraSearchPanel.maxChoices` — `[jira] search-max-choices`, else defaults.
pub fn max_choices(cfg: Option<&str>) -> Vec<String> {
    let r: Vec<String> = cfg
        .unwrap_or("")
        .split(',')
        .map(|x| x.trim().to_string())
        .filter(|x| x.parse::<i64>().is_ok())
        .collect();
    if r.is_empty() {
        ["25", "50", "100", "250", "500"].iter().map(|s| s.to_string()).collect()
    } else {
        r
    }
}

/// `dateTitle`: "today" → Today, "7d" → Last 7 days.
pub fn date_title(r: &str) -> String {
    if r == "today" {
        return "Today".into();
    }
    let (n, u) = r.split_at(r.len().saturating_sub(1));
    let Ok(n) = n.parse::<i64>() else { return r.to_string() };
    let unit = match u {
        "h" => "hour",
        "d" => "day",
        "w" => "week",
        other => other,
    };
    format!("Last {n} {unit}{}", if n == 1 { "" } else { "s" })
}

/// What the bar reports to the host.
#[derive(Clone, Debug, PartialEq)]
pub enum SearchEvent {
    Run,
    Close,
    /// "+ Filter" at this screen point.
    AddFilter { x: f64, y: f64 },
    RemoveRow(String),
    /// A date row's choice button.
    DateMenu { key: String, x: f64, y: f64 },
    MaxMenu { x: f64, y: f64 },
    Fetch,
    CopyJql,
    CopyCurl,
    /// Anything the persisted state covers changed.
    Changed,
    /// The Projects picker changed (labels / releases re-scope).
    ProjectsChanged,
}

/// One criterion row's value, as the bar holds it.
#[derive(Clone, Debug, PartialEq)]
pub enum RowValue {
    Picks(Vec<String>),
    Date(String),
    Text(String),
}

/// `searchCriteriaParams()` / `saveState()` rows from the bar's values.
pub fn search_rows(rows: &[(FilterKind, RowValue)]) -> Vec<SearchRow> {
    rows.iter()
        .map(|(k, v)| {
            let mut r = SearchRow::new(k.key.clone(), k.kind);
            match v {
                RowValue::Picks(p) => r.selected = p.clone(),
                RowValue::Date(d) => r.value = Some(d.clone()),
                RowValue::Text(t) => r.text = t.clone(),
            }
            r
        })
        .collect()
}

/// A saved row's kind when the catalog no longer lists it (`field:X` →
/// "X contains").
pub fn fallback_kind(key: &str) -> Option<FilterKind> {
    key.strip_prefix("field:").map(|f| FilterKind { key: key.to_string(), title: format!("{f} contains"), kind: ValueKind::Text })
}

#[cfg(target_os = "macos")]
pub use appkit::JiraSearchBar;

#[cfg(target_os = "macos")]
mod appkit {
    use super::{date_title, RowValue, SearchEvent};
    use crate::ui::chrome::Rect;
    use crate::ui::multi_picker::appkit::{PickerChip, Shared};
    use crate::ui::multi_picker::MultiPickerModel;
    use crate::ui::style::{self, ns, ButtonState, Weight};
    use crate::ui::theme::{PopupColors, PopupTone};
    use crate::views::jira::{FilterKind, ValueKind};
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, ProtocolObject, Sel};
    use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly, Message};
    use objc2_app_kit::{
        NSControlTextEditingDelegate, NSEvent, NSProgressIndicator, NSProgressIndicatorStyle, NSTextField,
        NSTextFieldDelegate, NSTextView, NSView,
    };
    use objc2_foundation::{NSNotification, NSObjectProtocol, NSRect, NSString};
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    const PAD: f64 = 12.0;
    const ROW: f64 = 28.0;
    const GAP: f64 = 8.0;
    const LABEL_W: f64 = 150.0;

    type Sink = Rc<dyn Fn(SearchEvent)>;

    struct BarRow {
        kind: FilterKind,
        chip: Option<Retained<PickerChip>>,
        field_box: Option<Retained<NSView>>,
        field: Option<Retained<NSTextField>>,
        date: RefCell<String>,
    }

    /// The drawn buttons, by id.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum Btn {
        AddFilter,
        Search,
        Close,
        Remove(usize),
        Date(usize),
        Fetch,
        Max,
        CopyJql,
        CopyCurl,
    }

    pub struct BarIvars {
        colors: Cell<PopupColors>,
        sink: RefCell<Option<Sink>>,
        text_box: Retained<NSView>,
        text: Retained<NSTextField>,
        projects: Retained<PickerChip>,
        rows: RefCell<Vec<BarRow>>,
        spinner: Retained<NSProgressIndicator>,
        status: RefCell<(String, PopupTone)>,
        searching: Cell<bool>,
        fetch_shown: Cell<bool>,
        max: RefCell<String>,
        hover: Cell<Option<usize>>,
    }

    define_class!(
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSJiraSearchBar"]
        #[ivars = BarIvars]
        pub struct JiraSearchBar;

        impl JiraSearchBar {
            #[unsafe(method(isFlipped))]
            fn is_flipped(&self) -> bool { true }

            #[unsafe(method(mouseDownCanMoveWindow))]
            fn mdcmw(&self) -> bool { false }

            #[unsafe(method(setFrameSize:))]
            fn set_frame_size(&self, size: objc2_foundation::NSSize) {
                let _: () = unsafe { msg_send![super(self), setFrameSize: size] };
                self.layout_controls();
            }

            #[unsafe(method(mouseDown:))]
            fn mouse_down(&self, e: &NSEvent) {
                let p = self.convertPoint_fromView(e.locationInWindow(), None);
                let hit = self.buttons().into_iter().find(|(_, r, _)| r.contains(p.x, p.y));
                let Some((b, r, _)) = hit else { return };
                let below = self.screen_point(r.x, r.max_y());
                let ev = match b {
                    Btn::AddFilter => SearchEvent::AddFilter { x: below.0, y: below.1 },
                    Btn::Search => SearchEvent::Run,
                    Btn::Close => SearchEvent::Close,
                    Btn::Remove(i) => match self.ivars().rows.borrow().get(i) {
                        Some(row) => SearchEvent::RemoveRow(row.kind.key.clone()),
                        None => return,
                    },
                    Btn::Date(i) => match self.ivars().rows.borrow().get(i) {
                        Some(row) => SearchEvent::DateMenu { key: row.kind.key.clone(), x: below.0, y: below.1 },
                        None => return,
                    },
                    Btn::Fetch => SearchEvent::Fetch,
                    Btn::Max => SearchEvent::MaxMenu { x: below.0, y: below.1 },
                    Btn::CopyJql => SearchEvent::CopyJql,
                    Btn::CopyCurl => SearchEvent::CopyCurl,
                };
                self.emit(ev);
            }

            #[unsafe(method(drawRect:))]
            fn draw_rect(&self, _dirty: NSRect) {
                self.draw();
            }

            // The search text and contains-text fields: Return searches.
            #[unsafe(method(control:textView:doCommandBySelector:))]
            fn do_command(&self, _c: &AnyObject, _tv: &NSTextView, cmd: Sel) -> objc2::runtime::Bool {
                if cmd == sel!(insertNewline:) {
                    self.emit(SearchEvent::Run);
                    return objc2::runtime::Bool::YES;
                }
                if cmd == sel!(cancelOperation:) {
                    self.emit(SearchEvent::Close);
                    return objc2::runtime::Bool::YES;
                }
                objc2::runtime::Bool::NO
            }

            #[unsafe(method(controlTextDidEndEditing:))]
            fn did_end(&self, _n: &NSNotification) {
                self.emit(SearchEvent::Changed);
            }
        }

        unsafe impl NSObjectProtocol for JiraSearchBar {}
        unsafe impl NSControlTextEditingDelegate for JiraSearchBar {}
        unsafe impl NSTextFieldDelegate for JiraSearchBar {}
    );

    impl JiraSearchBar {
        pub fn new(mtm: MainThreadMarker, colors: PopupColors, projects: Shared) -> Retained<Self> {
            let (text_box, text) = crate::ui::field::search_box(
                mtm,
                &colors,
                14.0,
                "Search text — matches title, description and comments · Return searches",
                6.0,
                ROW,
            );
            let chip = PickerChip::new(mtm, projects, colors);
            let spinner = NSProgressIndicator::new(mtm);
            spinner.setStyle(NSProgressIndicatorStyle::Spinning);
            spinner.setControlSize(objc2_app_kit::NSControlSize::Small);
            spinner.setDisplayedWhenStopped(false);
            let this = Self::alloc(mtm).set_ivars(BarIvars {
                colors: Cell::new(colors),
                sink: RefCell::new(None),
                text_box,
                text,
                projects: chip,
                rows: RefCell::new(Vec::new()),
                spinner,
                status: RefCell::new((String::new(), PopupTone::Dim)),
                searching: Cell::new(false),
                fetch_shown: Cell::new(false),
                max: RefCell::new(String::new()),
                hover: Cell::new(None),
            });
            let v: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
            let iv = v.ivars();
            for sub in [&*iv.text_box, &*iv.projects as &NSView, &*iv.spinner as &NSView] {
                v.addSubview(sub);
            }
            unsafe { iv.text.setDelegate(Some(ProtocolObject::from_ref(&*v))) };
            let weak = objc2::rc::Weak::from_retained(&v);
            iv.projects.set_on_change(Rc::new(move || {
                if let Some(b) = weak.load() {
                    b.emit(SearchEvent::ProjectsChanged);
                }
            }));
            v
        }

        pub fn set_on_event(&self, cb: Box<dyn Fn(SearchEvent)>) {
            *self.ivars().sink.borrow_mut() = Some(Rc::from(cb));
        }

        fn emit(&self, ev: SearchEvent) {
            let s = self.ivars().sink.borrow().clone();
            if let Some(s) = s {
                s(ev);
            }
        }

        pub fn text_field(&self) -> Retained<NSTextField> {
            self.ivars().text.clone()
        }

        pub fn text(&self) -> String {
            self.ivars().text.stringValue().to_string()
        }

        pub fn set_text(&self, t: &str) {
            self.ivars().text.setStringValue(&NSString::from_str(t));
        }

        pub fn projects(&self) -> Retained<PickerChip> {
            self.ivars().projects.clone()
        }

        /// Is keyboard focus inside the bar (`topAccessoryHasFocus`)?
        pub fn has_focus(&self) -> bool {
            let Some(w) = self.window() else { return false };
            let Some(r) = w.firstResponder() else { return false };
            // The field editor is an NSTextView, itself an NSView.
            let r: &AnyObject = &r;
            r.downcast_ref::<NSView>().is_some_and(|v: &NSView| v.isDescendantOf(self))
        }

        pub fn set_colors(&self, c: PopupColors) {
            self.ivars().colors.set(c);
            self.ivars().projects.set_colors(c);
            for r in self.ivars().rows.borrow().iter() {
                if let Some(ch) = &r.chip {
                    ch.set_colors(c);
                }
            }
            self.setNeedsDisplay(true);
        }

        pub fn set_status(&self, s: &str, tone: PopupTone) {
            *self.ivars().status.borrow_mut() = (s.to_string(), tone);
            self.setToolTip(Some(&NSString::from_str(s)));
            self.setNeedsDisplay(true);
        }

        pub fn set_searching(&self, on: bool) {
            self.ivars().searching.set(on);
            unsafe {
                if on {
                    self.ivars().spinner.startAnimation(None);
                } else {
                    self.ivars().spinner.stopAnimation(None);
                }
            }
            self.setNeedsDisplay(true);
        }

        pub fn set_fetch_shown(&self, on: bool) {
            self.ivars().fetch_shown.set(on);
            self.setNeedsDisplay(true);
        }

        pub fn set_max(&self, v: &str) {
            *self.ivars().max.borrow_mut() = v.to_string();
            self.setNeedsDisplay(true);
        }

        pub fn max(&self) -> String {
            self.ivars().max.borrow().clone()
        }

        pub fn row_keys(&self) -> Vec<String> {
            self.ivars().rows.borrow().iter().map(|r| r.kind.key.clone()).collect()
        }

        /// `addRow`: a picker (users / list), a date choice or a text field.
        /// Returns the picker's model for a picker row (the host fills it).
        pub fn add_row(&self, kind: FilterKind, default_date: &str) -> Option<Shared> {
            let mtm = MainThreadMarker::from(self);
            let c = self.ivars().colors.get();
            let mut row = BarRow { kind: kind.clone(), chip: None, field_box: None, field: None, date: RefCell::new(String::new()) };
            let mut model = None;
            match kind.kind {
                ValueKind::Users | ValueKind::List => {
                    // The picker pluralises the noun ("Filter labels"), so keep it singular.
                    let noun = if kind.kind == ValueKind::Users {
                        "user".to_string()
                    } else {
                        let t = kind.title.to_lowercase();
                        t.strip_suffix('s').map(str::to_string).unwrap_or(t)
                    };
                    let m: Shared = Rc::new(RefCell::new(MultiPickerModel::new(&noun, None)));
                    let chip = PickerChip::new(mtm, m.clone(), c);
                    let weak = objc2::rc::Weak::from_retained(&self.retain());
                    chip.set_on_change(Rc::new(move || {
                        if let Some(b) = weak.load() {
                            b.emit(SearchEvent::Changed);
                        }
                    }));
                    self.addSubview(&chip);
                    row.chip = Some(chip);
                    model = Some(m);
                }
                ValueKind::Date => {
                    *row.date.borrow_mut() = default_date.to_string();
                }
                ValueKind::Text => {
                    let (b, f) = crate::ui::field::search_box(mtm, &c, 12.5, "text — Return searches", 6.0, ROW);
                    unsafe { f.setDelegate(Some(ProtocolObject::from_ref(self))) };
                    self.addSubview(&b);
                    row.field_box = Some(b);
                    row.field = Some(f);
                }
            }
            self.ivars().rows.borrow_mut().push(row);
            self.layout_controls();
            self.setNeedsDisplay(true);
            model
        }

        pub fn remove_row(&self, key: &str) {
            let mut rows = self.ivars().rows.borrow_mut();
            if let Some(i) = rows.iter().position(|r| r.kind.key == key) {
                let r = rows.remove(i);
                if let Some(c) = &r.chip {
                    c.removeFromSuperview();
                }
                if let Some(b) = &r.field_box {
                    b.removeFromSuperview();
                }
            }
            drop(rows);
            self.layout_controls();
            self.setNeedsDisplay(true);
        }

        pub fn row_chip(&self, key: &str) -> Option<Retained<PickerChip>> {
            self.ivars().rows.borrow().iter().find(|r| r.kind.key == key).and_then(|r| r.chip.clone())
        }

        pub fn row_field(&self, key: &str) -> Option<Retained<NSTextField>> {
            self.ivars().rows.borrow().iter().find(|r| r.kind.key == key).and_then(|r| r.field.clone())
        }

        pub fn set_row_date(&self, key: &str, v: &str) {
            if let Some(r) = self.ivars().rows.borrow().iter().find(|r| r.kind.key == key) {
                *r.date.borrow_mut() = v.to_string();
            }
            self.setNeedsDisplay(true);
        }

        pub fn set_row_text(&self, key: &str, v: &str) {
            if let Some(f) = self.row_field(key) {
                f.setStringValue(&NSString::from_str(v));
            }
        }

        pub fn relabel(&self, kinds: &[FilterKind]) {
            for r in self.ivars().rows.borrow_mut().iter_mut() {
                if let Some(k) = kinds.iter().find(|k| k.key == r.kind.key) {
                    r.kind.title = k.title.clone();
                }
            }
            self.setNeedsDisplay(true);
        }

        /// The rows' current values (`searchCriteriaParams` / `saveState`).
        pub fn values(&self) -> Vec<(FilterKind, RowValue)> {
            self.ivars()
                .rows
                .borrow()
                .iter()
                .map(|r| {
                    let v = if let Some(c) = &r.chip {
                        RowValue::Picks(c.model().borrow().selected.clone())
                    } else if let Some(f) = &r.field {
                        RowValue::Text(f.stringValue().to_string())
                    } else {
                        RowValue::Date(r.date.borrow().clone())
                    };
                    (r.kind.clone(), v)
                })
                .collect()
        }

        /// The bar's height for its rows (the host lays the list out under it).
        pub fn wanted_height(&self, window_h: f64) -> f64 {
            let n = self.ivars().rows.borrow().len() as f64;
            let rows = n * (ROW + 6.0);
            let chrome = PAD + ROW + GAP + ROW + PAD + if n > 0.0 { GAP } else { 0.0 };
            chrome + rows.min((window_h * 0.4 - chrome).max(ROW + 6.0))
        }

        fn screen_point(&self, x: f64, y: f64) -> (f64, f64) {
            let p = self.convertPoint_toView(objc2_foundation::NSPoint::new(x, y), None);
            let s = self.window().map(|w| w.convertPointToScreen(p)).unwrap_or(p);
            (s.x, s.y)
        }

        fn layout_controls(&self) {
            let iv = self.ivars();
            let w = self.bounds().size.width;
            let (top, rows_y) = (PAD, PAD + ROW + GAP);
            // Top row: [text][projects 230][+ Filter][Search][×]
            let right = w - PAD - 28.0 - GAP - 92.0 - GAP - 78.0 - GAP;
            let pw = 230.0;
            iv.text_box.setFrame(ns(Rect::new(PAD, top, (right - pw - GAP - PAD).max(120.0), ROW)));
            iv.projects.setFrame(ns(Rect::new(right - pw, top, pw, ROW)));
            for (i, r) in iv.rows.borrow().iter().enumerate() {
                let y = rows_y + i as f64 * (ROW + 6.0);
                let ctl = Rect::new(PAD + LABEL_W + GAP, y, (w - PAD * 2.0 - LABEL_W - GAP * 2.0 - 28.0).max(80.0), ROW);
                if let Some(c) = &r.chip {
                    c.setFrame(ns(ctl));
                }
                if let Some(b) = &r.field_box {
                    b.setFrame(ns(ctl));
                }
            }
            let fy = self.footer_y();
            iv.spinner.setFrame(ns(Rect::new(PAD, fy + (ROW - 16.0) / 2.0, 16.0, 16.0)));
        }

        fn footer_y(&self) -> f64 {
            self.bounds().size.height - PAD - ROW
        }

        /// Every drawn button with its rect and title.
        fn buttons(&self) -> Vec<(Btn, Rect, String)> {
            let iv = self.ivars();
            let w = self.bounds().size.width;
            let f = style::system_font(12.0, Weight::Medium);
            let tw = |t: &str| style::text_size(t, &f).0 + 24.0;
            let mut out = Vec::new();
            let mut x = w - PAD - 28.0;
            out.push((Btn::Close, Rect::new(x, PAD, 28.0, ROW), "×".to_string()));
            x -= GAP + 92.0;
            out.push((Btn::Search, Rect::new(x, PAD, 92.0, ROW), "Search".to_string()));
            x -= GAP + 78.0;
            out.push((Btn::AddFilter, Rect::new(x, PAD, 78.0, ROW), "+ Filter".to_string()));
            let rows_y = PAD + ROW + GAP;
            for (i, r) in iv.rows.borrow().iter().enumerate() {
                let y = rows_y + i as f64 * (ROW + 6.0);
                out.push((Btn::Remove(i), Rect::new(w - PAD - 28.0, y, 28.0, ROW), "−".to_string()));
                if r.kind.kind == ValueKind::Date {
                    let ctl = Rect::new(PAD + LABEL_W + GAP, y, 180.0, ROW);
                    out.push((Btn::Date(i), ctl, date_title(&r.date.borrow())));
                }
            }
            let fy = self.footer_y();
            let mut x = w - PAD;
            for (b, t) in [
                (Btn::CopyCurl, "Copy curl".to_string()),
                (Btn::CopyJql, "Copy JQL".to_string()),
                (Btn::Max, format!("Max {} ▾", if iv.max.borrow().is_empty() { "—".to_string() } else { iv.max.borrow().clone() })),
            ] {
                let bw = tw(&t);
                x -= bw;
                out.push((b, Rect::new(x, fy, bw, ROW), t));
                x -= GAP;
            }
            if iv.fetch_shown.get() {
                let t = "Fetch from Jira".to_string();
                let bw = tw(&t) + 16.0;
                x -= bw;
                out.push((Btn::Fetch, Rect::new(x, fy, bw, ROW), t));
            }
            out
        }

        fn draw(&self) {
            let iv = self.ivars();
            let c = iv.colors.get();
            let b = crate::ui::style::rect_of(self.bounds());
            style::fill_rect(b, c.mantle().with_alpha(0.6));
            style::hline(0.0, b.width, b.height - 0.5, c.hairline());
            let label_font = style::system_font(12.0, Weight::Regular);
            let rows_y = PAD + ROW + GAP;
            for (i, r) in iv.rows.borrow().iter().enumerate() {
                let y = rows_y + i as f64 * (ROW + 6.0);
                style::draw_text_right(&r.kind.title, Rect::new(PAD, y, LABEL_W, ROW), &label_font, c.dim);
            }
            let f = style::system_font(12.0, Weight::Medium);
            for (btn, r, t) in self.buttons() {
                match btn {
                    Btn::Search => {
                        style::capsule_primary(r, &c, false, false);
                        let alpha = if iv.searching.get() { 0.5 } else { 1.0 };
                        style::draw_symbol("magnifyingglass", Rect::new(r.x + 10.0, r.y, 16.0, r.height), c.on_accent().with_alpha(alpha), 12.0);
                        style::draw_text_left(&t, Rect::new(r.x + 30.0, r.y, r.width - 34.0, r.height), &f, c.on_accent().with_alpha(alpha));
                    }
                    Btn::Close | Btn::Remove(_) => {
                        let sym = if btn == Btn::Close { "xmark" } else { "minus.circle" };
                        style::draw_symbol(sym, r, c.dim, 12.0);
                    }
                    Btn::Date(_) => {
                        style::fill_rounded(r.inset_by(0.5, 0.5), 6.0, style::input_fill(&c));
                        style::stroke_rounded(r.inset_by(0.5, 0.5), 6.0, 1.0, style::input_stroke(&c));
                        style::draw_text_left(&t, Rect::new(r.x + 9.0, r.y, r.width - 30.0, r.height), &label_font, c.text);
                        style::chevron(Rect::new(r.max_x() - 18.0, r.mid_y() - 4.0, 8.0, 8.0), c.dim);
                    }
                    _ => {
                        style::draw_button(r, ButtonState::Idle, &c, 6.0, false);
                        style::draw_text_centered(&t, r, &f, style::button_text(ButtonState::Idle, &c));
                    }
                }
            }
            let (s, tone) = iv.status.borrow().clone();
            if !s.is_empty() {
                let fy = self.footer_y();
                let left = if iv.searching.get() { PAD + 22.0 } else { PAD };
                let first_btn = self.buttons().iter().filter(|(_, r, _)| (r.y - fy).abs() < 1.0).map(|(_, r, _)| r.x).fold(b.width, f64::min);
                let small = style::system_font(11.0, Weight::Regular);
                let col = if tone == PopupTone::Dim { c.dim } else { c.tone(tone) };
                style::draw_text_left(&s, Rect::new(left, fy, (first_btn - left - GAP).max(0.0), ROW), &small, col);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_lists_and_titles() {
        assert_eq!(date_ranges(None)[2], "7d");
        assert_eq!(date_ranges(Some(" 1d, 3w ")), vec!["1d", "3w"]);
        assert_eq!(max_choices(Some("10, x, 40")), vec!["10", "40"]);
        assert_eq!(max_choices(Some("")), vec!["25", "50", "100", "250", "500"]);
        assert_eq!(date_title("today"), "Today");
        assert_eq!(date_title("1d"), "Last 1 day");
        assert_eq!(date_title("14d"), "Last 14 days");
        assert_eq!(date_title("3w"), "Last 3 weeks");
    }

    #[test]
    fn rows_to_search_rows() {
        let k = |key: &str, kind| FilterKind { key: key.into(), title: key.into(), kind };
        let rows = search_rows(&[
            (k("assignee", ValueKind::Users), RowValue::Picks(vec!["u1".into()])),
            (k("updated", ValueKind::Date), RowValue::Date("7d".into())),
            (k("field:title", ValueKind::Text), RowValue::Text("crash".into())),
        ]);
        assert_eq!(rows[0].selected, vec!["u1"]);
        assert_eq!(rows[1].value.as_deref(), Some("7d"));
        assert_eq!(rows[2].text, "crash");
        assert_eq!(fallback_kind("field:env").unwrap().title, "env contains");
        assert!(fallback_kind("status").is_none());
    }
}
