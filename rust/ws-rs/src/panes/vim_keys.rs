//! Port of `VimKeys.swift` — the pure half: the NORMAL/INSERT key state
//! machine, the [`VimTarget`] models it drives, and the `pane.vimMode` /
//! `pane.vimSearch` test state.
//!
//! AppKit (`NSView`, `NSTextView`, `WKWebView`, nvim RPC) is out of scope: a
//! target is a plain scrollable model, and the mode badge's drawing is a
//! documented `todo!()`. Search *matching* (`VimSearch.swift`) is also out of
//! scope; `openSearch` / `n` / `N` are reported as actions for the host.

use serde::Serialize;

use crate::panes::pane_geometry::Rect;
use crate::panes::pane_nav::RingStyle;
use crate::ui::popup::{KeyInput, KEY_ESC, KEY_RETURN};
use crate::ui::theme::{PopupColors, Rgba};

/// macOS key codes not surfaced by `ui::popup`'s table.
pub const KEY_D_CODE: u16 = 2;
pub const KEY_U_CODE: u16 = 32;
const KEY_DELETE: u16 = 51;
const KEY_FORWARD_DELETE: u16 = 117;
const KEY_ENTER_NUMPAD: u16 = 76;

/// `VimMode` (`"NORMAL"` / `"INSERT"` / `"SEARCH"`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
pub enum VimMode {
    Normal,
    Insert,
    Search,
}

impl Default for VimMode {
    fn default() -> Self {
        VimMode::Normal
    }
}

impl VimMode {
    pub fn raw_value(self) -> &'static str {
        match self {
            VimMode::Normal => "NORMAL",
            VimMode::Insert => "INSERT",
            VimMode::Search => "SEARCH",
        }
    }
}

/// `VimRows`: the row-list target model (the Swift `VimRows` protocol
/// approximated as concrete state).
#[derive(Clone, Debug, PartialEq)]
pub struct VimRows {
    pub texts: Vec<String>,
    pub cursor: usize,
    pub page: usize,
}

impl VimRows {
    pub fn new(texts: Vec<String>) -> VimRows {
        VimRows { texts, cursor: 0, page: 1 }
    }

    pub fn count(&self) -> usize {
        self.texts.len()
    }

    pub fn text(&self, row: usize) -> &str {
        self.texts.get(row).map(|s| s.as_str()).unwrap_or("")
    }

    pub fn move_to(&mut self, row: usize) {
        if self.texts.is_empty() {
            return;
        }
        self.cursor = row.min(self.texts.len() - 1);
    }

    /// `step(_:n:)`: move `n` rows, clamped to the list.
    pub fn step(&mut self, n: i64) {
        if self.texts.is_empty() {
            return;
        }
        let last = self.texts.len() as i64 - 1;
        self.cursor = (self.cursor as i64 + n).clamp(0, last) as usize;
    }

    fn half_page_step(&self) -> i64 {
        (self.page as i64 / 2).max(1)
    }
}

/// The text target model: a scroll position over laid-out lines.
#[derive(Clone, Debug, PartialEq)]
pub struct VimText {
    pub content: String,
    pub cursor: usize,
    pub scroll_y: f64,
    pub viewport_h: f64,
    pub line_height: f64,
}

impl VimText {
    pub fn new(content: impl Into<String>, cursor: usize, viewport_h: f64, line_height: f64) -> VimText {
        VimText { content: content.into(), cursor, scroll_y: 0.0, viewport_h, line_height }
    }

    fn content_height(&self) -> f64 {
        let lines = self.content.split('\n').count() as f64;
        (lines * self.line_height).max(self.viewport_h)
    }

    fn scroll_by(&mut self, dy: f64) {
        let max_y = (self.content_height() - self.viewport_h).max(0.0);
        self.scroll_y = (self.scroll_y + dy).clamp(0.0, max_y);
    }

    fn scroll_to(&mut self, bottom: bool) {
        let max_y = (self.content_height() - self.viewport_h).max(0.0);
        self.scroll_y = if bottom { max_y } else { 0.0 };
    }
}

/// The web target model (a WKWebView's scroll position, driven by JS).
#[derive(Clone, Debug, PartialEq)]
pub struct VimWeb {
    pub scroll_y: f64,
    pub viewport_h: f64,
    pub content_h: f64,
}

impl VimWeb {
    pub fn new(viewport_h: f64, content_h: f64) -> VimWeb {
        VimWeb { scroll_y: 0.0, viewport_h, content_h }
    }

    fn scroll_by(&mut self, dy: f64) {
        let max_y = (self.content_h - self.viewport_h).max(0.0);
        self.scroll_y = (self.scroll_y + dy).clamp(0.0, max_y);
    }

    fn scroll_to(&mut self, bottom: bool) {
        let max_y = (self.content_h - self.viewport_h).max(0.0);
        self.scroll_y = if bottom { max_y } else { 0.0 };
    }
}

/// `VimTarget` (`.rows` / `.text` / `.web`).
#[derive(Clone, Debug, PartialEq)]
pub enum VimTarget {
    Rows(VimRows),
    Text(VimText),
    Web(VimWeb),
}

impl VimTarget {
    /// `step(_:n:)`: j/k — a row (rows), `lineHeight * 2` (text), 48 pt (web).
    pub fn step(&mut self, n: i64) {
        match self {
            VimTarget::Rows(r) => r.step(n),
            VimTarget::Text(t) => t.scroll_by(n as f64 * t.line_height * 2.0),
            VimTarget::Web(w) => w.scroll_by(n as f64 * 48.0),
        }
    }

    /// `halfPage(_:down:)`: Ctrl+D / Ctrl+U.
    pub fn half_page(&mut self, down: bool) {
        match self {
            VimTarget::Rows(r) => {
                let n = r.half_page_step();
                r.step(if down { n } else { -n });
            }
            VimTarget::Text(t) => t.scroll_by(if down { 1.0 } else { -1.0 } * t.viewport_h / 2.0),
            VimTarget::Web(w) => w.scroll_by(if down { 1.0 } else { -1.0 } * w.viewport_h / 2.0),
        }
    }

    /// `edge(_:bottom:)`: gg / G.
    pub fn edge(&mut self, bottom: bool) {
        match self {
            VimTarget::Rows(r) => r.move_to(if bottom { r.count().saturating_sub(1) } else { 0 }),
            VimTarget::Text(t) => t.scroll_to(bottom),
            VimTarget::Web(w) => w.scroll_to(bottom),
        }
    }
}

/// The open `/` / `?` search bar.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchState {
    pub pane: String,
    pub query: String,
    pub status: String,
    pub back: bool,
    pub open: bool,
}

/// `handle(_:in:)`'s side effects the host must carry out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VimAction {
    /// Not consumed: the key propagates to the text view / terminal.
    Pass,
    /// Consumed with no side effect (the target already moved).
    Handled,
    /// `pane.insert()` — enter the editable text input.
    EnterInsert,
    /// `pane.normal()` — Esc left the text input.
    ExitToNormal,
    /// Open the `/` (back = false) or `?` (back = true) search bar.
    OpenSearch { back: bool },
    /// Repeat the last search; `reverse` = `N` vs `n`.
    RepeatSearch { reverse: bool },
    /// Esc cleared the active highlight.
    ClearHighlight,
}

/// Per-key context: the focused pane and everything `handle` needs from it.
pub struct VimContext<'a> {
    pub pane_id: &'a str,
    pub target: Option<&'a mut VimTarget>,
    pub is_text_input: bool,
    pub owns_vim: bool,
    pub has_insert: bool,
    pub has_normal: bool,
}

impl<'a> VimContext<'a> {
    fn t(&mut self) -> Option<&mut VimTarget> {
        self.target.as_deref_mut()
    }
}

/// The socket `pane.vimSearch` shape.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct VimSearchTestState {
    pub open: bool,
    pub query: String,
    pub status: String,
    pub pane: String,
    pub last: String,
    pub field_normal: bool,
}

/// The socket `pane.vimMode` shape: `{ mode, pane, search }`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct VimKeysTestState {
    pub mode: String,
    pub pane: String,
    pub search: VimSearchTestState,
}

/// `VimKeys`: the shared NORMAL/INSERT state machine.
#[derive(Clone, Debug)]
pub struct VimKeys {
    pub enabled: bool,
    pub show_badge: bool,
    /// A `g` was pressed and may be the first half of `gg`.
    pub pending_g: bool,
    /// An active highlight exists (set by the host); Esc clears it.
    pub highlighting: bool,
    /// A jira-list-style field kept in normal mode (caret hidden).
    pub field_normal: bool,
    pub last_query: String,
    pub last_back: bool,
    /// The focused pane id (empty = none; `vimMode` reads `""`).
    pub pane: String,
    pub mode: VimMode,
    pub search: Option<SearchState>,
}

impl Default for VimKeys {
    fn default() -> Self {
        VimKeys::new()
    }
}

impl VimKeys {
    pub fn new() -> VimKeys {
        VimKeys {
            enabled: true,
            show_badge: true,
            pending_g: false,
            highlighting: false,
            field_normal: false,
            last_query: String::new(),
            last_back: false,
            pane: String::new(),
            mode: VimMode::Normal,
            search: None,
        }
    }

    /// `mode(_:in:)` for the focused pane.
    pub fn mode(&self, ctx: &VimContext) -> Option<VimMode> {
        if !self.enabled || ctx.owns_vim {
            return None;
        }
        if let Some(s) = &self.search {
            return if s.open && s.pane == ctx.pane_id { Some(VimMode::Search) } else { None };
        }
        if ctx.is_text_input {
            if self.field_normal {
                return Some(VimMode::Normal);
            }
            return if ctx.has_normal { Some(VimMode::Insert) } else { None };
        }
        if ctx.target.is_some() {
            Some(VimMode::Normal)
        } else {
            None
        }
    }

    /// `testState(_:)` as `{ mode, pane, search }`.
    pub fn test_state(&self) -> VimKeysTestState {
        let search = match &self.search {
            Some(s) if s.open => VimSearchTestState {
                open: true,
                query: s.query.clone(),
                status: s.status.clone(),
                pane: s.pane.clone(),
                last: self.last_query.clone(),
                field_normal: self.field_normal,
            },
            _ => VimSearchTestState {
                open: false,
                query: String::new(),
                status: String::new(),
                pane: String::new(),
                last: self.last_query.clone(),
                field_normal: self.field_normal,
            },
        };
        let mode = if self.pane.is_empty() {
            String::new()
        } else {
            self.mode.raw_value().to_string()
        };
        VimKeysTestState { mode, pane: self.pane.clone(), search }
    }

    /// `handle(_:in:)`.
    pub fn handle(&mut self, key: KeyInput, ctx: &mut VimContext) -> VimAction {
        if !self.enabled || ctx.owns_vim {
            return VimAction::Pass;
        }
        self.pane = ctx.pane_id.to_string();

        let search_open = self
            .search
            .as_ref()
            .map(|s| s.open && s.pane == ctx.pane_id)
            .unwrap_or(false);
        if search_open {
            return self.search_key(key);
        }

        let mods_empty = !key.cmd && !key.ctrl && !key.opt && !key.shift;
        let mods_shift_only = !key.cmd && !key.ctrl && !key.opt;

        if ctx.is_text_input && !self.field_normal {
            if key.key_code == KEY_ESC && mods_empty && ctx.has_normal {
                self.pending_g = false;
                self.mode = VimMode::Normal;
                return VimAction::ExitToNormal;
            }
            return VimAction::Pass;
        }

        if ctx.target.is_none() {
            return VimAction::Pass;
        }

        if key.key_code == KEY_ESC && mods_empty && self.highlighting {
            self.pending_g = false;
            self.highlighting = false;
            return VimAction::ClearHighlight;
        }

        if key.ctrl && !key.cmd && !key.opt && !key.shift
            && (key.key_code == KEY_D_CODE || key.key_code == KEY_U_CODE)
        {
            self.pending_g = false;
            let down = key.key_code == KEY_D_CODE;
            if let Some(t) = ctx.t() {
                t.half_page(down);
            }
            return VimAction::Handled;
        }

        let ch = if mods_shift_only { key.chars } else { None };

        if ch == Some('g') {
            if self.pending_g {
                self.pending_g = false;
                if let Some(t) = ctx.t() {
                    t.edge(false);
                }
            } else {
                self.pending_g = true;
            }
            return VimAction::Handled;
        }
        self.pending_g = false;

        match ch {
            Some('j') => {
                if let Some(t) = ctx.t() {
                    t.step(1);
                }
                VimAction::Handled
            }
            Some('k') => {
                if let Some(t) = ctx.t() {
                    t.step(-1);
                }
                VimAction::Handled
            }
            Some('G') => {
                if let Some(t) = ctx.t() {
                    t.edge(true);
                }
                VimAction::Handled
            }
            Some('/') => {
                self.open_search(ctx, false);
                VimAction::OpenSearch { back: false }
            }
            Some('?') => {
                self.open_search(ctx, true);
                VimAction::OpenSearch { back: true }
            }
            Some('n') => VimAction::RepeatSearch { reverse: false },
            Some('N') => VimAction::RepeatSearch { reverse: true },
            Some('i') | Some('a') => {
                if ctx.has_insert {
                    self.mode = VimMode::Insert;
                    VimAction::EnterInsert
                } else if self.field_normal {
                    VimAction::Handled
                } else {
                    VimAction::Pass
                }
            }
            _ => {
                if !self.field_normal || !mods_shift_only {
                    return VimAction::Pass;
                }
                if key.key_code == KEY_DELETE || key.key_code == KEY_FORWARD_DELETE {
                    return VimAction::Handled;
                }
                match key.chars {
                    Some(c) if (c as u32) > 0x20 && (c as u32) < 0xF700 => VimAction::Handled,
                    _ => VimAction::Pass,
                }
            }
        }
    }

    fn open_search(&mut self, ctx: &VimContext, back: bool) {
        self.search = Some(SearchState {
            pane: ctx.pane_id.to_string(),
            query: String::new(),
            status: String::new(),
            back,
            open: true,
        });
        self.mode = VimMode::Search;
    }

    /// Reduced `searchKey`: edit the query, Esc cancels, Return accepts.
    /// Matching/highlighting (`VimSearch`) is out of scope for this cut.
    fn search_key(&mut self, key: KeyInput) -> VimAction {
        let mut s = match self.search.take() {
            Some(s) => s,
            None => return VimAction::Pass,
        };
        let mut keep = true;
        if key.key_code == KEY_ESC && !key.cmd && !key.ctrl && !key.opt {
            keep = false;
            self.mode = VimMode::Normal;
        } else if key.key_code == KEY_RETURN || key.key_code == KEY_ENTER_NUMPAD {
            keep = false;
            self.last_query = s.query.clone();
            self.last_back = s.back;
            self.mode = VimMode::Normal;
        } else if key.key_code == KEY_DELETE && !key.cmd && !key.opt {
            if s.query.is_empty() {
                keep = false;
                self.mode = VimMode::Normal;
            } else {
                s.query.pop();
            }
        } else if !key.cmd && !key.ctrl && !key.opt {
            if let Some(c) = key.chars {
                if (c as u32) >= 0x20 && (c as u32) < 0xF700 {
                    s.query.push(c);
                }
            }
        }
        if keep {
            self.search = Some(s);
        }
        VimAction::Handled
    }
}

/// `VimModeBadge` (`VimKeys.swift:643`): the bottom-right NORMAL/INSERT chip.
/// The `NSView` subclass (`drawRect:`) is macOS-only; its geometry and colors
/// are pure and tested here.
pub const BADGE_FONT_SIZE: f64 = 9.0;
pub const BADGE_HEIGHT: f64 = 15.0;
pub const BADGE_H_PADDING: f64 = 12.0;

/// `VimModeBadge.size(_:)`: `ceil(textWidth) + 12` × 15. `text_width` is the
/// measured run width (AppKit measures; the geometry is pure).
pub fn badge_size(text_width: f64) -> (f64, f64) {
    (text_width.ceil() + BADGE_H_PADDING, BADGE_HEIGHT)
}

/// `VimModeBadge.draw` fill: INSERT = accent @ 0.85, else the ring @ 0.35.
pub fn badge_fill(mode: VimMode, colors: &PopupColors, ring: Rgba) -> Rgba {
    match mode {
        VimMode::Insert => colors.accent.with_alpha(0.85),
        _ => ring.with_alpha(0.35),
    }
}

/// `VimModeBadge.draw` text color.
pub fn badge_text_color(mode: VimMode, colors: &PopupColors) -> Rgba {
    match mode {
        VimMode::Insert => colors.on_accent(),
        _ => colors.text,
    }
}

/// `PaneNav.refresh` places the chip `pane.maxX - w - 8` and either
/// `pane.maxY - h - 6` (flipped root) or `pane.minY + 6`.
pub fn badge_origin(pane: Rect, width: f64, height: f64, root_flipped: bool) -> (f64, f64) {
    let x = pane.max_x() - width - 8.0;
    let y = if root_flipped {
        pane.max_y() - height - 6.0
    } else {
        pane.min_y() + 6.0
    };
    (x, y)
}

/// The ring color the badge falls back to (`PaneNav.ringColor`).
pub fn badge_default_ring() -> Rgba {
    RingStyle::default().rgba()
}

/// `VimModeBadge`, as an `NSView` subclass on macOS.
#[cfg(target_os = "macos")]
mod appkit {
    use super::*;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSBezierPath, NSColor, NSFont, NSFontAttributeName, NSFontWeightSemibold,
        NSForegroundColorAttributeName, NSStringDrawing, NSView,
    };
    use objc2_foundation::{
        NSAttributedStringKey, NSDictionary, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
    };
    use std::cell::{Cell, RefCell};

    fn badge_font() -> Retained<NSFont> {
        NSFont::monospacedSystemFontOfSize_weight(BADGE_FONT_SIZE, unsafe { NSFontWeightSemibold })
    }

    /// `[.font: font, .foregroundColor: color]` as an attribute dictionary.
    fn attr_dict(
        font: &NSFont,
        color: &NSColor,
    ) -> Retained<NSDictionary<NSAttributedStringKey, AnyObject>> {
        let font_obj: &AnyObject = unsafe { &*(font as *const NSFont as *const AnyObject) };
        let color_obj: &AnyObject = unsafe { &*(color as *const NSColor as *const AnyObject) };
        let keys: [&NSAttributedStringKey; 2] =
            unsafe { [NSFontAttributeName, NSForegroundColorAttributeName] };
        let objs: [&AnyObject; 2] = [font_obj, color_obj];
        NSDictionary::from_slices(&keys, &objs)
    }

    pub struct VimModeBadgeIvars {
        mode: Cell<VimMode>,
        colors: RefCell<PopupColors>,
        ring: Cell<Rgba>,
    }

    define_class!(
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSVimModeBadge"]
        #[ivars = VimModeBadgeIvars]
        pub struct VimModeBadge;

        impl VimModeBadge {
            #[unsafe(method(hitTest:))]
            fn hit_test(&self, _point: NSPoint) -> *mut NSView {
                std::ptr::null_mut()
            }

            #[unsafe(method(drawRect:))]
            fn draw_rect(&self, _dirty: NSRect) {
                let b = self.bounds();
                let mode = self.ivars().mode.get();
                let colors = *self.ivars().colors.borrow();
                let ring = self.ivars().ring.get();

                let radius = b.size.height / 2.0;
                let path =
                    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(b, radius, radius);
                badge_fill(mode, &colors, ring).to_nscolor().setFill();
                path.fill();

                let font = badge_font();
                let color = badge_text_color(mode, &colors).to_nscolor();
                let text = NSString::from_str(mode.raw_value());
                let attrs = attr_dict(&font, &color);
                let s = unsafe { text.sizeWithAttributes(Some(&attrs)) };
                let p = NSPoint::new(
                    (b.size.width - s.width) / 2.0,
                    (b.size.height - s.height) / 2.0,
                );
                unsafe { text.drawAtPoint_withAttributes(p, Some(&attrs)) };
            }
        }

        unsafe impl NSObjectProtocol for VimModeBadge {}
    );

    impl VimModeBadge {
        pub fn create(mtm: MainThreadMarker) -> Retained<VimModeBadge> {
            let this = VimModeBadge::alloc(mtm).set_ivars(VimModeBadgeIvars {
                mode: Cell::new(VimMode::Normal),
                colors: RefCell::new(PopupColors::default()),
                ring: Cell::new(badge_default_ring()),
            });
            let view: Retained<VimModeBadge> = unsafe {
                msg_send![
                    super(this),
                    initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0))
                ]
            };
            view.setHidden(true);
            view
        }

        pub fn mode(&self) -> VimMode {
            self.ivars().mode.get()
        }

        pub fn set_mode(&self, mode: VimMode) {
            self.ivars().mode.set(mode);
            self.setNeedsDisplay(true);
        }

        pub fn set_style(&self, colors: PopupColors, ring: Rgba) {
            *self.ivars().colors.borrow_mut() = colors;
            self.ivars().ring.set(ring);
            self.setNeedsDisplay(true);
        }

        /// `VimModeBadge.size(_:)` (measures the mode text with the chip font).
        pub fn size_for_mode(mode: VimMode) -> (f64, f64) {
            let font = badge_font();
            let attrs = attr_dict(&font, &NSColor::blackColor());
            let text = NSString::from_str(mode.raw_value());
            let s = unsafe { text.sizeWithAttributes(Some(&attrs)) };
            badge_size(s.width)
        }

        /// Mark the chip for redraw (`drawRect:` paints it).
        pub fn draw(&self) {
            self.setNeedsDisplay(true);
        }
    }
}

#[cfg(target_os = "macos")]
#[allow(unused_imports)]
pub use appkit::VimModeBadge;

/// Non-AppKit build: the chip cannot draw.
#[cfg(not(target_os = "macos"))]
pub struct VimModeBadge;

#[cfg(not(target_os = "macos"))]
impl VimModeBadge {
    pub fn draw(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::popup::{KEY_J, KEY_K};

    fn key(code: u16, ch: Option<char>) -> KeyInput {
        KeyInput { key_code: code, chars: ch, cmd: false, ctrl: false, opt: false, shift: false, esc_streak: 0 }
    }

    fn ctrl(code: u16) -> KeyInput {
        KeyInput { key_code: code, chars: None, cmd: false, ctrl: true, opt: false, shift: false, esc_streak: 0 }
    }

    fn ctx<'a>(target: &'a mut VimTarget, pane: &'a str, text_input: bool) -> VimContext<'a> {
        VimContext {
            pane_id: pane,
            target: Some(target),
            is_text_input: text_input,
            owns_vim: false,
            has_insert: true,
            has_normal: true,
        }
    }

    fn apply(vim: &mut VimKeys, target: &mut VimTarget, pane: &str, text_input: bool, k: KeyInput) -> VimAction {
        let mut c = ctx(target, pane, text_input);
        vim.handle(k, &mut c)
    }

    fn rows(n: usize) -> VimTarget {
        let texts = (0..n).map(|i| format!("row {i}")).collect();
        let mut r = VimRows::new(texts);
        r.page = 10;
        VimTarget::Rows(r)
    }

    #[test]
    fn normal_motions_j_k() {
        let mut vim = VimKeys::new();
        let mut t = rows(20);
        assert_eq!(apply(&mut vim, &mut t, "list", false, key(KEY_J, Some('j'))), VimAction::Handled);
        assert_eq!(apply(&mut vim, &mut t, "list", false, key(KEY_J, Some('j'))), VimAction::Handled);
        assert_eq!(apply(&mut vim, &mut t, "list", false, key(KEY_K, Some('k'))), VimAction::Handled);
        match &t {
            VimTarget::Rows(r) => assert_eq!(r.cursor, 1),
            _ => panic!("expected rows"),
        }
    }

    #[test]
    fn normal_gg_and_g() {
        let mut vim = VimKeys::new();
        let mut t = rows(20);
        assert_eq!(apply(&mut vim, &mut t, "list", false, key(KEY_J, Some('j'))), VimAction::Handled);
        apply(&mut vim, &mut t, "list", false, key(5, Some('G')));
        match &t {
            VimTarget::Rows(r) => assert_eq!(r.cursor, 19, "G jumps to the last row"),
            _ => panic!(),
        }
        apply(&mut vim, &mut t, "list", false, key(5, Some('g')));
        apply(&mut vim, &mut t, "list", false, key(5, Some('g')));
        match &t {
            VimTarget::Rows(r) => assert_eq!(r.cursor, 0, "gg jumps to the first row"),
            _ => panic!(),
        }
    }

    #[test]
    fn normal_ctrl_half_page() {
        let mut vim = VimKeys::new();
        let mut t = rows(20);
        apply(&mut vim, &mut t, "list", false, key(KEY_J, Some('j')));
        apply(&mut vim, &mut t, "list", false, key(KEY_J, Some('j')));
        assert_eq!(apply(&mut vim, &mut t, "list", false, ctrl(KEY_D_CODE)), VimAction::Handled);
        match &t {
            VimTarget::Rows(r) => assert_eq!(r.cursor, 7, "Ctrl+D moves half a page (2 + 5)"),
            _ => panic!(),
        }
        assert_eq!(apply(&mut vim, &mut t, "list", false, ctrl(KEY_U_CODE)), VimAction::Handled);
        match &t {
            VimTarget::Rows(r) => assert_eq!(r.cursor, 2, "Ctrl+U moves back half a page"),
            _ => panic!(),
        }
    }

    #[test]
    fn insert_and_escape_transitions() {
        let mut vim = VimKeys::new();
        let mut t = rows(20);
        assert_eq!(apply(&mut vim, &mut t, "list", false, key(0, Some('i'))), VimAction::EnterInsert);
        assert_eq!(vim.mode, VimMode::Insert);
        let st = vim.test_state();
        assert_eq!(st.mode, "INSERT");
        assert_eq!(st.pane, "list");
        assert!(!st.search.open);

        let mut text = VimTarget::Text(VimText::new("a\nb", 0, 300.0, 20.0));
        assert_eq!(
            apply(&mut vim, &mut text, "list", true, key(KEY_ESC, None)),
            VimAction::ExitToNormal
        );
        assert_eq!(vim.mode, VimMode::Normal);
        assert_eq!(vim.test_state().mode, "NORMAL");

        assert_eq!(apply(&mut vim, &mut t, "list", false, key(0, Some('a'))), VimAction::EnterInsert);
        assert_eq!(vim.mode, VimMode::Insert);
    }

    #[test]
    fn insert_other_keys_pass_through() {
        let mut vim = VimKeys::new();
        vim.mode = VimMode::Insert;
        let mut text = VimTarget::Text(VimText::new("a\nb", 0, 300.0, 20.0));
        assert_eq!(apply(&mut vim, &mut text, "list", true, key(KEY_J, Some('j'))), VimAction::Pass);
    }

    #[test]
    fn search_open_type_close() {
        let mut vim = VimKeys::new();
        let mut t = rows(20);
        assert_eq!(
            apply(&mut vim, &mut t, "list", false, key(44, Some('/'))),
            VimAction::OpenSearch { back: false }
        );
        let st = vim.test_state();
        assert_eq!(st.mode, "SEARCH");
        assert!(st.search.open);
        assert_eq!(st.search.pane, "list");

        apply(&mut vim, &mut t, "list", false, key(0, Some('a')));
        apply(&mut vim, &mut t, "list", false, key(0, Some('b')));
        assert_eq!(vim.search.as_ref().unwrap().query, "ab");

        assert_eq!(apply(&mut vim, &mut t, "list", false, key(KEY_ESC, None)), VimAction::Handled);
        assert_eq!(vim.mode, VimMode::Normal);
        assert!(vim.search.is_none());
    }

    #[test]
    fn search_back_and_repeat_actions() {
        let mut vim = VimKeys::new();
        let mut t = rows(20);
        assert_eq!(
            apply(&mut vim, &mut t, "list", false, key(44, Some('?'))),
            VimAction::OpenSearch { back: true }
        );
        assert_eq!(
            apply(&mut vim, &mut t, "list", false, key(KEY_RETURN, None)),
            VimAction::Handled
        );
        assert_eq!(vim.last_query, "");
        assert!(vim.last_back);

        assert_eq!(apply(&mut vim, &mut t, "list", false, key(45, Some('n'))), VimAction::RepeatSearch { reverse: false });
        assert_eq!(apply(&mut vim, &mut t, "list", false, key(45, Some('N'))), VimAction::RepeatSearch { reverse: true });
    }

    #[test]
    fn esc_clears_highlight() {
        let mut vim = VimKeys::new();
        vim.highlighting = true;
        let mut t = rows(20);
        assert_eq!(apply(&mut vim, &mut t, "list", false, key(KEY_ESC, None)), VimAction::ClearHighlight);
        assert!(!vim.highlighting);
    }

    #[test]
    fn command_keys_pass_through() {
        let mut vim = VimKeys::new();
        let mut t = rows(20);
        let cmd_c = KeyInput { key_code: 8, chars: Some('c'), cmd: true, ctrl: false, opt: false, shift: false, esc_streak: 0 };
        assert_eq!(apply(&mut vim, &mut t, "list", false, cmd_c), VimAction::Pass);
    }

    #[test]
    fn text_target_scrolls() {
        let mut vim = VimKeys::new();
        let mut t = VimTarget::Text(VimText::new("a\nb\nc\nd\ne", 0, 40.0, 20.0));
        apply(&mut vim, &mut t, "editor", false, key(KEY_J, Some('j')));
        match &t {
            VimTarget::Text(x) => assert_eq!(x.scroll_y, 40.0, "j scrolls 2 lines"),
            _ => panic!(),
        }
        apply(&mut vim, &mut t, "editor", false, key(5, Some('G')));
        match &t {
            VimTarget::Text(x) => assert_eq!(x.scroll_y, 60.0, "G goes to the bottom"),
            _ => panic!(),
        }
        apply(&mut vim, &mut t, "editor", false, key(5, Some('g')));
        apply(&mut vim, &mut t, "editor", false, key(5, Some('g')));
        match &t {
            VimTarget::Text(x) => assert_eq!(x.scroll_y, 0.0, "gg goes to the top"),
            _ => panic!(),
        }
    }

    #[test]
    fn mode_is_none_without_target_or_vim_owner() {
        let vim = VimKeys::new();
        let mut target = rows(3);
        let mut c = VimContext {
            pane_id: "terminal",
            target: Some(&mut target),
            is_text_input: false,
            owns_vim: true,
            has_insert: false,
            has_normal: false,
        };
        assert_eq!(vim.mode(&mut c), None);

        let mut c2 = VimContext {
            pane_id: "list",
            target: None,
            is_text_input: false,
            owns_vim: false,
            has_insert: false,
            has_normal: false,
        };
        assert_eq!(vim.mode(&mut c2), None);
    }

    #[test]
    fn badge_size_rounds_up_and_pads() {
        assert_eq!(badge_size(30.0), (42.0, 15.0));
        assert_eq!(badge_size(30.4), (43.0, 15.0));
        assert_eq!(badge_size(0.0), (12.0, 15.0));
    }

    #[test]
    fn badge_fill_and_text_by_mode() {
        let colors = PopupColors::default();
        let ring = badge_default_ring();
        assert_eq!(
            badge_fill(VimMode::Insert, &colors, ring),
            colors.accent.with_alpha(0.85)
        );
        assert_eq!(
            badge_fill(VimMode::Normal, &colors, ring),
            ring.with_alpha(0.35)
        );
        assert_eq!(
            badge_fill(VimMode::Search, &colors, ring),
            ring.with_alpha(0.35)
        );
        assert_eq!(
            badge_text_color(VimMode::Insert, &colors),
            colors.on_accent()
        );
        assert_eq!(badge_text_color(VimMode::Normal, &colors), colors.text);
    }

    #[test]
    fn badge_origin_bottom_right() {
        let pane = Rect::new(100.0, 200.0, 400.0, 300.0);
        // 8 from the right edge, height 15.
        assert_eq!(badge_origin(pane, 40.0, 15.0, false), (452.0, 206.0));
        // Flipped root: 6 from the top edge (max Y).
        assert_eq!(badge_origin(pane, 40.0, 15.0, true), (452.0, 479.0));
    }

    #[test]
    fn badge_default_ring_is_ring_style_default() {
        let c = badge_default_ring();
        assert!((c.r - 200.0 / 255.0).abs() < 1e-9);
        assert!((c.a - 140.0 / 255.0).abs() < 1e-9);
    }
}
