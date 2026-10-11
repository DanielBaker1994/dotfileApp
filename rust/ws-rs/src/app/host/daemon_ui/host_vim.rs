//! `VimKeys.swift`, AppKit half: vim-style keys on the shared window's
//! non-nvim panes — `j`/`k`, `gg`/`G`, `Ctrl+D`/`Ctrl+U`, `/` `?` `n` `N`,
//! `i`/`a` into a pane's text input and Esc back out — plus the search bar
//! and the NORMAL/INSERT badge.
//!
//! The key state machine is [`VimKeys`] (pure, unit-tested). Each key builds
//! a [`VimTarget`] snapshot of the focused pane (rows from the view model, an
//! `NSTextView`, or a `WKWebView`), runs it, and writes what moved back.

use super::*;
use crate::app::live_panes::{self, LivePane};
use crate::panes::vim_keys::{
    VimAction, VimContext, VimKeys, VimMode, VimModeBadge, VimRows, VimTarget, VimText, VimWeb,
};
use crate::ui::popup::KeyInput;
use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString, NSTextView, NSWindow, NSWindowOrderingMode};
use objc2_web_kit::WKWebView;

/// Where a pane's rows live (`VimRows` conformers).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum RowsSource {
    Sidebar,
    Files,
    Jira,
    Confluence,
}

/// The live target behind a snapshot.
enum Live {
    Rows(RowsSource, Option<Retained<crate::ui::sidebar::SidebarView>>),
    Text(Retained<NSTextView>),
    Web(Retained<WKWebView>),
}

/// The vim host state kept on the daemon UI.
#[derive(Default)]
pub(super) struct VimHost {
    pub keys: VimKeys,
    bar: Option<Retained<VimSearchBarView>>,
    badge: Option<Retained<VimModeBadge>>,
}

/// `VimTarget.find(in:)`: the first web / non-field text view under `v`.
fn find_live(v: &NSView) -> Option<Live> {
    if v.isHidden() {
        return None;
    }
    let any = as_any(v);
    if let Some(w) = any.downcast_ref::<WKWebView>() {
        return Some(Live::Web(w.retain()));
    }
    if let Some(t) = any.downcast_ref::<NSTextView>() {
        return (!t.isFieldEditor()).then(|| Live::Text(t.retain()));
    }
    for sub in v.subviews().iter() {
        if let Some(t) = find_live(&sub) {
            return Some(t);
        }
    }
    None
}

fn text_line_height(t: &NSTextView) -> f64 {
    let f = t.font().unwrap_or_else(|| NSFont::systemFontOfSize(13.0));
    (f.ascender() - f.descender() + f.leading()).ceil() + 2.0
}

fn text_scroll(t: &NSTextView) -> (f64, f64) {
    match t.enclosingScrollView() {
        Some(sv) => {
            let b = sv.contentView().bounds();
            (b.origin.y, b.size.height)
        }
        None => (0.0, t.bounds().size.height),
    }
}

fn text_scroll_to(t: &NSTextView, y: f64) {
    let Some(sv) = t.enclosingScrollView() else { return };
    let clip = sv.contentView();
    let max_y = (t.frame().size.height - clip.bounds().size.height).max(0.0);
    let x = clip.bounds().origin.x;
    clip.scrollToPoint(NSPoint::new(x, y.clamp(0.0, max_y)));
    sv.reflectScrolledClipView(&clip);
}

fn web_js(w: &WKWebView, js: &str) {
    unsafe { w.evaluateJavaScript_completionHandler(&NSString::from_str(js), None) };
}

fn js_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// A web view's virtual scroll model: the real height is unknown here, so
/// park the model mid-page and translate its movement into JS.
const WEB_VIRTUAL_H: f64 = 10_000_000.0;

impl DaemonUi {
    /// The pane the keyboard is in, with every pane (for focus moves).
    fn vim_pane(&self) -> Option<(Retained<SlotHostWindow>, Vec<LivePane>, usize)> {
        let (w, panes) = self.live_panes()?;
        let cur = live_panes::current(&w, &panes)?;
        let idx = panes.iter().position(|p| std::ptr::eq(p, cur))?;
        Some((w, panes, idx))
    }

    /// `ownsVim`: nvim (SwiftTerm), the prose page and the terminal drawer
    /// keep every key.
    fn vim_owned(&self, pane: &LivePane, fr: &objc2_app_kit::NSResponder) -> bool {
        let class = as_any(fr).class().name().to_string_lossy().into_owned();
        if class.contains("TerminalView") {
            return true;
        }
        let notes = self.ivars().controller.current() == Some(SlotView::Notes);
        notes && (pane.id == "editor" || pane.id == "terminal")
    }

    /// `pane.vim?() ?? VimTarget.find(in: pane.view)`.
    fn vim_live(&self, pane: &LivePane) -> Option<Live> {
        let current = self.ivars().controller.current();
        match pane.id.as_str() {
            "sidebar" | "files-sidebar" => {
                let bar = as_any(&*pane.area).downcast_ref::<crate::ui::sidebar::SidebarView>()?;
                Some(Live::Rows(RowsSource::Sidebar, Some(bar.retain())))
            }
            "files-list" => Some(Live::Rows(RowsSource::Files, None)),
            "list" if current.is_some_and(|v| v.is_jira()) => Some(Live::Rows(RowsSource::Jira, None)),
            "results" => Some(Live::Rows(RowsSource::Confluence, None)),
            "search" | "files-filter" | "recent-filter" | "left-path" | "right-path" | "input" => None,
            _ => find_live(&pane.area),
        }
    }

    /// Rows snapshot: texts, cursor, page.
    fn vim_rows(&self, src: RowsSource, bar: Option<&crate::ui::sidebar::SidebarView>) -> Option<VimRows> {
        let (texts, cursor, page) = match src {
            RowsSource::Sidebar => bar?.vim_rows(),
            RowsSource::Files => self.files_surface(|b| {
                let t: Vec<String> = b.visible_rows().iter().map(|e| e.name.clone()).collect();
                (t, b.selection(), 20)
            })?,
            RowsSource::Jira => self.jira_surface(|l| {
                let t: Vec<String> = l
                    .rows
                    .iter()
                    .map(|&i| {
                        let r = &l.all[i];
                        let key = r.get("key").cloned().unwrap_or_default();
                        let sum = r.get("summary").cloned().unwrap_or_default();
                        format!("{key} {sum}")
                    })
                    .collect();
                (t, l.selection, 20)
            })?,
            RowsSource::Confluence => self.conf_surface(|cs| {
                let t: Vec<String> = cs.model.rows.iter().map(|r| r.title.clone()).collect();
                (t, cs.model.selection, 12)
            })?,
        };
        Some(VimRows { texts, cursor, page })
    }

    /// `vimMove(to:)` for each rows source.
    fn vim_rows_move(&self, src: RowsSource, bar: Option<&crate::ui::sidebar::SidebarView>, row: usize) {
        match src {
            RowsSource::Sidebar => {
                if let Some(b) = bar {
                    b.vim_move(row);
                }
            }
            RowsSource::Files => {
                self.files_surface(|b| b.list.selection = row);
            }
            RowsSource::Jira => {
                self.jira_surface(|l| l.selection = row);
            }
            RowsSource::Confluence => self.conf_load_preview(row),
        }
    }

    /// `pane.normal` / `pane.insert`: the field ↔ list partner of a pane.
    fn vim_partner(pane: &str, insert: bool) -> Option<&'static str> {
        match (pane, insert) {
            ("files-filter", false) => Some("files-list"),
            ("files-list", true) => Some("files-filter"),
            ("search", false) => Some("list"),
            ("list", true) => Some("search"),
            ("results", true) => Some("search"),
            ("recent-filter", false) => Some("diff"),
            _ => None,
        }
    }

    fn vim_partner_exists(panes: &[LivePane], pane: &str, insert: bool) -> Option<&'static str> {
        let p = Self::vim_partner(pane, insert)?;
        // Confluence's search pairs with `results`, Jira's with `list`.
        let p = if p == "list" && panes.iter().any(|x| x.id == "results") { "results" } else { p };
        panes.iter().any(|x| x.id == p).then_some(p)
    }

    /// `VimKeys.shared.searching(in:)`.
    pub(super) fn vim_searching(&self) -> bool {
        self.ivars().vim.borrow().keys.search.as_ref().is_some_and(|s| s.open)
    }

    /// `VimKeys.shared.handle(_:in:)`: `true` = consumed.
    pub(super) fn vim_key(&self, event: &NSEvent) -> bool {
        let Some((w, panes, idx)) = self.vim_pane() else { return false };
        let pane = &panes[idx];
        let Some(fr) = w.firstResponder() else { return false };
        if self.vim_owned(pane, &fr) {
            return false;
        }
        let mut key: KeyInput = popup::key_input_from_event(event);
        // The search bar types what the key produces (`e.characters`).
        if let Some(c) = event.characters().and_then(|s| s.to_string().chars().next()) {
            if !key.ctrl && !key.cmd {
                key.chars = Some(c);
            }
        }
        let is_text_input = as_any(&*fr).downcast_ref::<NSTextView>().is_some_and(|t| t.isEditable());
        let insert_to = Self::vim_partner_exists(&panes, &pane.id, true);
        let normal_to = Self::vim_partner_exists(&panes, &pane.id, false);
        let live = if is_text_input { None } else { self.vim_live(pane) };

        // Snapshot the target.
        let mut text_scroll0 = (0.0, 0.0);
        let mut target = match &live {
            Some(Live::Rows(src, bar)) => self.vim_rows(*src, bar.as_deref()).map(VimTarget::Rows),
            Some(Live::Text(t)) => {
                let (y, h) = text_scroll(t);
                text_scroll0 = (y, h);
                let mut vt = VimText::new(t.string().to_string(), t.selectedRange().location, h, text_line_height(t));
                vt.scroll_y = y;
                Some(VimTarget::Text(vt))
            }
            Some(Live::Web(wv)) => {
                let mut vw = VimWeb::new(wv.bounds().size.height, WEB_VIRTUAL_H);
                vw.scroll_y = WEB_VIRTUAL_H / 2.0;
                Some(VimTarget::Web(vw))
            }
            None => None,
        };
        let before = target.clone();
        let clip = NSPasteboard::generalPasteboard()
            .stringForType(unsafe { NSPasteboardTypeString })
            .map(|s| s.to_string());
        let pane_id = pane.id.clone();
        let action = {
            let mut ctx = VimContext {
                pane_id: &pane_id,
                target: target.as_mut(),
                is_text_input,
                owns_vim: false,
                has_insert: insert_to.is_some(),
                has_normal: normal_to.is_some(),
                clipboard: clip.as_deref(),
            };
            self.ivars().vim.borrow_mut().keys.handle(key, &mut ctx)
        };

        // Write back what moved.
        if let (Some(live), Some(after), Some(before)) = (&live, &target, &before) {
            self.vim_apply(live, before, after, text_scroll0);
        }
        let query = self.ivars().vim.borrow().keys.search.as_ref().map(|s| (s.query.clone(), s.back));
        let consumed = match &action {
            VimAction::Pass => false,
            VimAction::EnterInsert => {
                if let Some(p) = insert_to.and_then(|id| panes.iter().find(|p| p.id == id)) {
                    live_panes::take_focus(&w, p);
                }
                true
            }
            VimAction::ExitToNormal => {
                if let Some(p) = normal_to.and_then(|id| panes.iter().find(|p| p.id == id)) {
                    live_panes::take_focus(&w, p);
                }
                true
            }
            VimAction::OpenSearch { back } => {
                self.vim_open_bar(&w, pane, *back);
                true
            }
            VimAction::RepeatSearch { reverse } => {
                if let Some(Live::Web(wv)) = &live {
                    let k = self.ivars().vim.borrow().keys.clone();
                    if !k.last_query.is_empty() {
                        let back = k.last_back != *reverse;
                        web_js(wv, &format!("window.find({}, false, {back}, true)", js_str(&k.last_query)));
                    }
                } else if self.ivars().vim.borrow().keys.last_query.is_empty() {
                    // Nothing searched yet: let the key through like Swift's guard.
                }
                true
            }
            VimAction::SearchCopy(s) => {
                if !s.is_empty() {
                    copy_text_to_pasteboard(s);
                }
                true
            }
            VimAction::Handled | VimAction::ClearHighlight => true,
        };
        // Web search runs in the page (`WKWebView.find`).
        if let (Some(Live::Web(wv)), Some((q, back))) = (&live, &query) {
            if !q.is_empty() && matches!(action, VimAction::Handled) {
                web_js(wv, "window.getSelection().removeAllRanges()");
                web_js(wv, &format!("window.find({}, false, {back}, true)", js_str(q)));
            }
        }
        self.vim_sync_bar();
        self.live_pane_refresh();
        consumed
    }

    /// Carry a target's movement back to AppKit.
    fn vim_apply(&self, live: &Live, before: &VimTarget, after: &VimTarget, scroll0: (f64, f64)) {
        match (live, before, after) {
            (Live::Rows(src, bar), VimTarget::Rows(b), VimTarget::Rows(a)) => {
                if a.cursor != b.cursor {
                    self.vim_rows_move(*src, bar.as_deref(), a.cursor);
                }
            }
            (Live::Text(t), VimTarget::Text(b), VimTarget::Text(a)) => {
                if a.cursor != b.cursor {
                    let q = self.ivars().vim.borrow().keys.search.as_ref().map(|s| s.query.clone())
                        .unwrap_or_else(|| self.ivars().vim.borrow().keys.last_query.clone());
                    let len = q.encode_utf16().count();
                    let r = objc2_foundation::NSRange::new(a.cursor, len);
                    t.setSelectedRange(r);
                    t.scrollRangeToVisible(r);
                    if len > 0 {
                        t.showFindIndicatorForRange(r);
                    }
                } else if (a.scroll_y - b.scroll_y).abs() > 0.5 {
                    let max_model = (a.content.split('\n').count() as f64 * a.line_height).max(a.viewport_h) - a.viewport_h;
                    let y = if a.scroll_y <= 0.0 {
                        0.0
                    } else if a.scroll_y >= max_model - 0.5 {
                        f64::MAX / 4.0
                    } else {
                        scroll0.0 + (a.scroll_y - b.scroll_y)
                    };
                    text_scroll_to(t, y);
                }
            }
            (Live::Web(wv), VimTarget::Web(b), VimTarget::Web(a)) => {
                let dy = a.scroll_y - b.scroll_y;
                if a.scroll_y <= 0.0 {
                    web_js(wv, "window.scrollTo(0, 0)");
                } else if a.scroll_y >= WEB_VIRTUAL_H - a.viewport_h - 0.5 {
                    web_js(wv, "window.scrollTo(0, document.documentElement.scrollHeight)");
                } else if dy.abs() > 0.5 {
                    web_js(wv, &format!("window.scrollBy(0, {})", dy.round()));
                }
            }
            _ => {}
        }
    }

    /// `openSearch`: the bar sits along the bottom of the pane.
    fn vim_open_bar(&self, w: &NSWindow, pane: &LivePane, back: bool) {
        let Some(root) = w.contentView() else { return };
        let mtm = MainThreadMarker::new().expect("main thread");
        let bar = VimSearchBarView::create(mtm, back);
        let wr = pane.area.convertRect_toView(pane.area.bounds(), None);
        let r = root.convertRect_fromView(wr, None);
        let h = 26.0;
        let y = if root.isFlipped() { r.origin.y + r.size.height - h - 6.0 } else { r.origin.y + 6.0 };
        bar.setFrame(NSRect::new(NSPoint::new(r.origin.x + 6.0, y), NSSize::new((r.size.width - 12.0).max(160.0), h)));
        root.addSubview_positioned_relativeTo(&bar, NSWindowOrderingMode::Above, None);
        let mut v = self.ivars().vim.borrow_mut();
        if let Some(old) = v.bar.replace(bar) {
            old.removeFromSuperview();
        }
    }

    /// Mirror the model's search state into the bar (and drop it once closed).
    fn vim_sync_bar(&self) {
        let mut v = self.ivars().vim.borrow_mut();
        let open = v.keys.search.as_ref().filter(|s| s.open).map(|s| (s.query.clone(), s.status.clone()));
        match (open, v.bar.as_ref()) {
            (Some((q, st)), Some(bar)) => bar.set_text(&q, &st),
            (None, Some(_)) => {
                if let Some(bar) = v.bar.take() {
                    bar.removeFromSuperview();
                }
            }
            _ => {}
        }
    }

    /// `paneChanged` + the badge half of `PaneNav.refresh`: close a search
    /// whose pane lost the keyboard; show NORMAL / INSERT on the pane.
    pub(super) fn vim_refresh(&self, w: &NSWindow, panes: &[LivePane]) {
        let cur = live_panes::current(w, panes);
        let key = w.isKeyWindow();
        {
            let mut v = self.ivars().vim.borrow_mut();
            let stale = v.keys.search.as_ref().is_some_and(|s| !key || cur.map(|c| c.id.as_str()) != Some(s.pane.as_str()));
            if stale {
                if let Some(s) = v.keys.search.take() {
                    if !s.query.is_empty() {
                        v.keys.last_query = s.query;
                        v.keys.last_back = s.back;
                    }
                }
                v.keys.mode = VimMode::Normal;
            }
            v.keys.pane = cur.map(|c| c.id.clone()).unwrap_or_default();
        }
        self.vim_sync_bar();
        let mode = match (cur, w.firstResponder()) {
            (Some(p), Some(fr)) if key && !self.vim_owned(p, &fr) => {
                let searching = self.vim_searching();
                let text = as_any(&*fr).downcast_ref::<NSTextView>().is_some_and(|t| t.isEditable());
                if searching {
                    None
                } else if text {
                    Self::vim_partner_exists(panes, &p.id, false).map(|_| VimMode::Insert)
                } else if matches!(p.id.as_str(), "sidebar" | "files-sidebar" | "files-list" | "results")
                    || (p.id == "list" && self.ivars().controller.current().is_some_and(|v| v.is_jira()))
                    || find_live(&p.area).is_some()
                {
                    Some(VimMode::Normal)
                } else {
                    None
                }
            }
            _ => None,
        };
        let Some(root) = w.contentView() else { return };
        let mtm = MainThreadMarker::new().expect("main thread");
        let mut v = self.ivars().vim.borrow_mut();
        if let Some(m) = mode {
            if !self.ivars().controller.settings().vim_mode_badge {
                return;
            }
            let badge = v.badge.get_or_insert_with(|| VimModeBadge::create(mtm)).clone();
            let p = cur.unwrap();
            let r = root.convertRect_fromView(p.area.convertRect_toView(p.area.bounds(), None), None);
            let (bw, bh) = VimModeBadge::size_for_mode(m);
            let y = if root.isFlipped() { r.origin.y + r.size.height - bh - 6.0 } else { r.origin.y + 6.0 };
            let f = NSRect::new(NSPoint::new(r.origin.x + r.size.width - bw - 8.0, y), NSSize::new(bw, bh));
            if badge.mode() != m {
                badge.set_mode(m);
            }
            if badge.frame() != f {
                badge.setFrame(f);
            }
            let on_top = root.subviews().lastObject().is_some_and(|l| std::ptr::eq(&*l as *const NSView as *const u8, &*badge as *const VimModeBadge as *const u8));
            if !on_top {
                root.addSubview_positioned_relativeTo(&badge, NSWindowOrderingMode::Above, None);
            }
            badge.setHidden(false);
        } else if let Some(b) = &v.badge {
            b.setHidden(true);
        }
        v.keys.mode = mode.unwrap_or(if v.keys.search.is_some() { VimMode::Search } else { VimMode::Normal });
    }

    /// `pane.vimMode` / `pane.vimSearch` for `state.pane`.
    pub(super) fn vim_test_state(&self) -> serde_json::Value {
        let v = self.ivars().vim.borrow();
        let ts = v.keys.test_state();
        let shown = v.badge.as_ref().is_some_and(|b| !b.isHidden());
        json!({
            "vimMode": if v.keys.search.is_some() { "SEARCH".to_string() } else if shown { ts.mode } else { String::new() },
            "vimSearch": ts.search,
        })
    }
}

// ---------------------------------------------------------------- search bar

pub(super) struct VimSearchBarIvars {
    back: bool,
    query: RefCell<String>,
    status: RefCell<String>,
}

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "WSVimSearchBar"]
    #[ivars = VimSearchBarIvars]
    /// `VimSearchBar`: `/query▏` on the left, `n/m` (or the key hint) right.
    pub(super) struct VimSearchBarView;

    impl VimSearchBarView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(hitTest:))]
        fn hit_test(&self, _p: NSPoint) -> *mut NSView {
            std::ptr::null_mut()
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            use crate::ui::style;
            use crate::ui::theme::{PopupThemeDefaults, PopupTone};
            let c = PopupThemeDefaults::colors();
            let b = self.bounds();
            let r = crate::ui::chrome::Rect::new(0.5, 0.5, b.size.width - 1.0, b.size.height - 1.0);
            style::fill_rounded(r, 6.0, c.background.blended(0.12, c.text).with_alpha(0.97));
            style::stroke_rounded(r, 6.0, 1.0, crate::panes::pane_nav::RingStyle::default().rgba());
            let font = NSFont::monospacedSystemFontOfSize_weight(12.0, unsafe { objc2_app_kit::NSFontWeightRegular });
            let lead = if self.ivars().back { "?" } else { "/" };
            let q = self.ivars().query.borrow().clone();
            let (lw, h) = style::text_size(lead, &font);
            let y = (b.size.height - h) / 2.0;
            style::draw_text(lead, 9.0, y, &font, c.accent);
            let (qw, _) = style::text_size(&q, &font);
            style::draw_text(&q, 9.0 + lw, y, &font, c.text);
            style::draw_text("▏", 9.0 + lw + qw, y, &font, c.accent);
            let st = self.ivars().status.borrow().clone();
            let hint = if st.is_empty() { "⏎ keep · esc back · ↑↓ prev / next".to_string() } else { st.clone() };
            let sf = NSFont::monospacedDigitSystemFontOfSize_weight(11.0, unsafe { objc2_app_kit::NSFontWeightRegular });
            let (sw, sh) = style::text_size(&hint, &sf);
            let col = if st == "no match" { c.tone(PopupTone::Danger) } else { c.dim };
            style::draw_text(&hint, b.size.width - sw - 10.0, (b.size.height - sh) / 2.0, &sf, col);
        }
    }
);

impl VimSearchBarView {
    fn create(mtm: MainThreadMarker, back: bool) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(VimSearchBarIvars {
            back,
            query: RefCell::new(String::new()),
            status: RefCell::new(String::new()),
        });
        let v: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
        v.setWantsLayer(true);
        v
    }

    fn set_text(&self, q: &str, status: &str) {
        if *self.ivars().query.borrow() != q || *self.ivars().status.borrow() != status {
            *self.ivars().query.borrow_mut() = q.to_string();
            *self.ivars().status.borrow_mut() = status.to_string();
            self.setNeedsDisplay(true);
        }
    }
}
