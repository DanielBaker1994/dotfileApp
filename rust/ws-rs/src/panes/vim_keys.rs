//! Port of `VimKeys.swift` — the NORMAL/INSERT/SEARCH key state machine, the
//! [`VimTarget`] models it drives, the `/`/`?` search bar (query editing with
//! undo, incremental matching, `n`/`N` stepping with wrap, and origin
//! restore), and the `pane.vimMode` / `pane.vimSearch` test state.
//!
//! The row/text matching engine is the real port in [`vim_search`]. AppKit
//! (`NSView`, `NSTextView`, `WKWebView`, nvim RPC) is modelled, not linked: a
//! target is a plain scrollable model, clipboard paste arrives through
//! [`VimContext::clipboard`], and copy is reported as [`VimAction::SearchCopy`]
//! for the host to place on the pasteboard. The `NORMAL`/`INSERT` mode badge is
//! the `NSView` subclass [`VimModeBadge`] below — on macOS it draws via
//! `drawRect:` with the same geometry/colors as the pure [`badge_size`] /
//! [`badge_fill`] / [`badge_origin`] helpers (which stay testable off-AppKit).

use std::time::{Duration, Instant};

use serde::Serialize;

use crate::panes::pane_geometry::Rect;
use crate::panes::pane_nav::RingStyle;
use crate::panes::vim_search;
use crate::ui::popup::{
    KeyInput, KEY_A, KEY_C, KEY_DOWN, KEY_ESC, KEY_H, KEY_N, KEY_P, KEY_RETURN, KEY_UP, KEY_V,
    KEY_W, KEY_X, KEY_Z,
};
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

/// The open `/` / `?` search bar (`VimSearchBar` + the socket-facing fields).
#[derive(Clone, Debug, PartialEq)]
pub struct SearchState {
    pub pane: String,
    pub query: String,
    pub status: String,
    pub back: bool,
    pub open: bool,
    /// Where the bar opened: a row index (rows target) or a UTF-16 location
    /// (text target). Esc restores the target to here.
    pub origin: i64,
    /// `VimSearchBar.undo`: the query history (for `Ctrl+Z` / `Cmd+Z`).
    pub undo: Vec<String>,
}

impl SearchState {
    /// `searchKey`'s `set(_:)`: remember the previous query, then replace it.
    fn set(&mut self, query: String) {
        self.undo.push(std::mem::take(&mut self.query));
        self.query = query;
    }

    /// `set(String(query.dropLast()))` from Delete / `Ctrl+H`.
    fn backspace(&mut self) {
        if self.query.is_empty() {
            return;
        }
        self.undo.push(self.query.clone());
        self.query.pop();
    }
}

/// `handle(_:in:)`'s side effects the host must carry out.
#[derive(Clone, Debug, PartialEq, Eq)]
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
    /// Repeat the last search; `reverse` = `N` vs `n`. The model has already
    /// moved the target; the host repaints the highlight.
    RepeatSearch { reverse: bool },
    /// The host should copy this text to the pasteboard (`Ctrl+C` / `Cmd+C` /
    /// `Cmd+X` in the search bar).
    SearchCopy(String),
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
    /// The pasteboard string, read on demand for `Ctrl+V` / `Cmd+V` in the
    /// search bar. `None` means the host has no clipboard to offer.
    pub clipboard: Option<&'a str>,
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
    /// When the pending `g` was pressed; `gg` only fires within 0.8 s.
    pending_g_at: Option<Instant>,
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
            pending_g_at: None,
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

    /// `handle(_:in:)`: seeds the `gg` timeout from the current wall clock.
    pub fn handle(&mut self, key: KeyInput, ctx: &mut VimContext) -> VimAction {
        self.handle_at(key, ctx, Instant::now())
    }

    /// `handle(_:in:)` with an explicit clock: the Swift `pendingG` expires
    /// 0.8 s after the first `g`, so `gg` only fires within that window.
    pub fn handle_at(&mut self, key: KeyInput, ctx: &mut VimContext, now: Instant) -> VimAction {
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
            let clipboard = ctx.clipboard;
            return self.search_key(key, ctx.t(), clipboard);
        }

        let mods_empty = !key.cmd && !key.ctrl && !key.opt && !key.shift;
        let mods_shift_only = !key.cmd && !key.ctrl && !key.opt;

        if ctx.is_text_input && !self.field_normal {
            if key.key_code == KEY_ESC && mods_empty && ctx.has_normal {
                self.reset_pending_g();
                self.mode = VimMode::Normal;
                return VimAction::ExitToNormal;
            }
            return VimAction::Pass;
        }

        if ctx.target.is_none() {
            return VimAction::Pass;
        }

        if key.key_code == KEY_ESC && mods_empty && self.highlighting {
            self.reset_pending_g();
            self.highlighting = false;
            return VimAction::ClearHighlight;
        }

        if key.ctrl && !key.cmd && !key.opt && !key.shift
            && (key.key_code == KEY_D_CODE || key.key_code == KEY_U_CODE)
        {
            self.reset_pending_g();
            let down = key.key_code == KEY_D_CODE;
            if let Some(t) = ctx.t() {
                t.half_page(down);
            }
            return VimAction::Handled;
        }

        let ch = if mods_shift_only { key.chars } else { None };

        if ch == Some('g') {
            if self.pending_g_fresh(now) {
                self.reset_pending_g();
                if let Some(t) = ctx.t() {
                    t.edge(false);
                }
            } else {
                self.pending_g = true;
                self.pending_g_at = Some(now);
            }
            return VimAction::Handled;
        }
        self.reset_pending_g();

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
            Some('n') => self.repeat_search(ctx.t(), false),
            Some('N') => self.repeat_search(ctx.t(), true),
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

    fn reset_pending_g(&mut self) {
        self.pending_g = false;
        self.pending_g_at = None;
    }

    fn pending_g_fresh(&self, now: Instant) -> bool {
        self.pending_g
            && self
                .pending_g_at
                .map(|t| now.saturating_duration_since(t) < Duration::from_millis(800))
                .unwrap_or(false)
    }

    fn open_search(&mut self, ctx: &VimContext, back: bool) {
        let origin = match ctx.target.as_deref() {
            Some(VimTarget::Rows(r)) => r.cursor as i64,
            Some(VimTarget::Text(t)) => t.cursor as i64,
            _ => 0,
        };
        self.search = Some(SearchState {
            pane: ctx.pane_id.to_string(),
            query: String::new(),
            status: String::new(),
            back,
            open: true,
            origin,
            undo: Vec::new(),
        });
        self.mode = VimMode::Search;
    }

    /// `searchKey(_:_:in:)`: edit the query (with undo + `Ctrl+W` word drop),
    /// step matches with the arrows, and accept / cancel. Esc restores the
    /// target to where the bar opened. Clipboard paste comes from
    /// [`VimContext::clipboard`]; copy is reported as [`VimAction::SearchCopy`].
    fn search_key(
        &mut self,
        key: KeyInput,
        target: Option<&mut VimTarget>,
        clipboard: Option<&str>,
    ) -> VimAction {
        let mut s = match self.search.take() {
            Some(s) => s,
            None => return VimAction::Pass,
        };
        let cmd = key.cmd;
        let ctrl = key.ctrl;
        let opt = key.opt;
        let mut action = VimAction::Handled;

        match key.key_code {
            KEY_ESC => {
                self.restore_origin(&s, target);
                self.highlighting = false;
                self.mode = VimMode::Normal;
                return VimAction::Handled;
            }
            KEY_RETURN | KEY_ENTER_NUMPAD => {
                self.last_query = s.query.clone();
                self.last_back = s.back;
                self.highlighting = !s.query.is_empty();
                self.mode = VimMode::Normal;
                return VimAction::Handled;
            }
            KEY_DELETE if cmd || opt => {
                let q = vim_search::drop_word(&s.query);
                s.set(q);
                self.incremental(&mut s, target);
            }
            KEY_DELETE => {
                if s.query.is_empty() {
                    self.restore_origin(&s, target);
                    self.highlighting = false;
                    self.mode = VimMode::Normal;
                    return VimAction::Handled;
                }
                s.backspace();
                self.incremental(&mut s, target);
            }
            KEY_DOWN => self.step_search(&mut s, target, false),
            KEY_UP => self.step_search(&mut s, target, true),
            _ => {
                if ctrl && !cmd {
                    match key.key_code {
                        KEY_N => self.step_search(&mut s, target, false),
                        KEY_P => self.step_search(&mut s, target, true),
                        KEY_W => {
                            let q = vim_search::drop_word(&s.query);
                            s.set(q);
                            self.incremental(&mut s, target);
                        }
                        KEY_U_CODE => {
                            s.set(String::new());
                            self.incremental(&mut s, target);
                        }
                        KEY_H => {
                            s.backspace();
                            self.incremental(&mut s, target);
                        }
                        KEY_V => {
                            if let Some(c) = clipboard {
                                let q = format!("{}{}", s.query, vim_search::one_line(c));
                                s.set(q);
                                self.incremental(&mut s, target);
                            }
                        }
                        KEY_C => action = VimAction::SearchCopy(s.query.clone()),
                        _ => {}
                    }
                } else if cmd {
                    match key.key_code {
                        KEY_V => {
                            if let Some(c) = clipboard {
                                let q = format!("{}{}", s.query, vim_search::one_line(c));
                                s.set(q);
                                self.incremental(&mut s, target);
                            }
                        }
                        KEY_C => action = VimAction::SearchCopy(s.query.clone()),
                        KEY_X => {
                            action = VimAction::SearchCopy(s.query.clone());
                            s.set(String::new());
                            self.incremental(&mut s, target);
                        }
                        KEY_Z => {
                            if let Some(prev) = s.undo.pop() {
                                s.query = prev;
                                self.incremental(&mut s, target);
                            }
                        }
                        KEY_A => {}
                        _ => {
                            self.search = Some(s);
                            return VimAction::Pass;
                        }
                    }
                } else if let Some(c) = key.chars {
                    if (c as u32) >= 0x20 && (c as u32) < 0xF700 {
                        let q = format!("{}{}", s.query, c);
                        s.set(q);
                        self.incremental(&mut s, target);
                    }
                }
            }
        }
        self.search = Some(s);
        action
    }

    /// `incremental(_:)`: match from the origin as the query is edited.
    fn incremental(&mut self, s: &mut SearchState, mut target: Option<&mut VimTarget>) {
        if s.query.is_empty() {
            s.status.clear();
            self.highlighting = false;
            if let Some(VimTarget::Rows(r)) = target.as_deref_mut() {
                if !r.texts.is_empty() {
                    let last = r.texts.len() - 1;
                    r.move_to((s.origin.max(0) as usize).min(last));
                }
            }
            return;
        }
        self.highlighting = find_in(s, target, Some(s.origin), s.back, false);
    }

    /// `find(_:from:reverse:step:)` with `from == nil` (the arrow / `Ctrl+N|P`
    /// step path): continues from the current target position, skipping it.
    fn step_search(&mut self, s: &mut SearchState, target: Option<&mut VimTarget>, reverse: bool) {
        if s.query.is_empty() {
            return;
        }
        self.highlighting = find_in(s, target, None, reverse, true);
    }

    /// `repeatSearch(_:reverse:pane:in:)`: `n` / `N` after a completed search.
    fn repeat_search(&mut self, target: Option<&mut VimTarget>, reverse: bool) -> VimAction {
        if self.last_query.is_empty() {
            return VimAction::RepeatSearch { reverse };
        }
        let query = self.last_query.clone();
        let back = self.last_back != reverse;
        match target {
            Some(VimTarget::Rows(r)) => {
                let hit = vim_search::rows(&r.texts, &query, r.cursor as i64, back, true);
                if let Some(i) = hit.0 {
                    r.move_to(i);
                }
            }
            Some(VimTarget::Text(t)) => {
                let hit = vim_search::text(&t.content, &query, t.cursor as i64, back, true);
                if let Some(rg) = hit.0 {
                    t.cursor = rg.location;
                }
            }
            _ => {}
        }
        self.highlighting = true;
        VimAction::RepeatSearch { reverse }
    }

    /// `closeSearch(accept: false, ...)`: put the target back where the bar
    /// opened (rows cursor, or text selection location).
    fn restore_origin(&self, s: &SearchState, target: Option<&mut VimTarget>) {
        match target {
            Some(VimTarget::Rows(r)) => {
                if !r.texts.is_empty() {
                    let last = r.texts.len() - 1;
                    r.move_to((s.origin.max(0) as usize).min(last));
                }
            }
            Some(VimTarget::Text(t)) => t.cursor = s.origin.max(0) as usize,
            _ => {}
        }
    }
}

/// One `VimSearch` step against a [`VimTarget`]: move the target to the hit and
/// record the `status`. Returns whether the host should paint a highlight (any
/// non-empty query does, matching Swift's `defer { highlight(...) }`).
fn find_in(
    s: &mut SearchState,
    target: Option<&mut VimTarget>,
    from: Option<i64>,
    reverse: bool,
    step: bool,
) -> bool {
    let query = s.query.clone();
    if query.is_empty() {
        s.status.clear();
        return false;
    }
    let skip = step || from.is_none();
    match target {
        Some(VimTarget::Rows(r)) => {
            let start = from.unwrap_or(r.cursor as i64);
            let hit = vim_search::rows(&r.texts, &query, start, reverse, skip);
            if let Some(i) = hit.0 {
                r.move_to(i);
            }
            s.status = vim_search::status(hit.0.is_some(), hit.1, hit.2);
        }
        Some(VimTarget::Text(t)) => {
            let start = from.unwrap_or(t.cursor as i64);
            let hit = vim_search::text(&t.content, &query, start, reverse, skip);
            if let Some(rg) = hit.0 {
                t.cursor = rg.location;
            }
            s.status = vim_search::status(hit.0.is_some(), hit.1, hit.2);
        }
        Some(VimTarget::Web(_)) => s.status = "match".to_string(),
        None => {}
    }
    true
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
            clipboard: None,
        }
    }

    fn apply(vim: &mut VimKeys, target: &mut VimTarget, pane: &str, text_input: bool, k: KeyInput) -> VimAction {
        let mut c = ctx(target, pane, text_input);
        vim.handle(k, &mut c)
    }

    fn apply_at(
        vim: &mut VimKeys,
        target: &mut VimTarget,
        pane: &str,
        k: KeyInput,
        now: Instant,
    ) -> VimAction {
        let mut c = ctx(target, pane, false);
        vim.handle_at(k, &mut c, now)
    }

    fn apply_clip(vim: &mut VimKeys, target: &mut VimTarget, pane: &str, k: KeyInput, clip: &str) -> VimAction {
        let mut c = VimContext {
            pane_id: pane,
            target: Some(target),
            is_text_input: false,
            owns_vim: false,
            has_insert: true,
            has_normal: true,
            clipboard: Some(clip),
        };
        vim.handle(k, &mut c)
    }

    fn rows(n: usize) -> VimTarget {
        let texts = (0..n).map(|i| format!("row {i}")).collect();
        let mut r = VimRows::new(texts);
        r.page = 10;
        VimTarget::Rows(r)
    }

    fn matching_rows() -> VimTarget {
        let texts = ["alpha", "beta", "gamma", "beta two", "delta"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        VimTarget::Rows(VimRows::new(texts))
    }

    fn cursor(t: &VimTarget) -> usize {
        match t {
            VimTarget::Rows(r) => r.cursor,
            VimTarget::Text(x) => x.cursor,
            _ => 0,
        }
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
            clipboard: None,
        };
        assert_eq!(vim.mode(&mut c), None);

        let mut c2 = VimContext {
            pane_id: "list",
            target: None,
            is_text_input: false,
            owns_vim: false,
            has_insert: false,
            has_normal: false,
            clipboard: None,
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

    #[test]
    fn gg_requires_a_fresh_pending_g() {
        let mut vim = VimKeys::new();
        let mut t = VimTarget::Rows(VimRows::new(vec!["a".into(), "b".into(), "c".into()]));
        apply(&mut vim, &mut t, "list", false, key(5, Some('G')));
        assert_eq!(cursor(&t), 2, "G to the bottom");
        let t0 = Instant::now();
        apply_at(&mut vim, &mut t, "list", key(5, Some('g')), t0);
        // A second `g` a second later is a *new* `g`, not `gg`.
        apply_at(&mut vim, &mut t, "list", key(5, Some('g')), t0 + Duration::from_secs(1));
        assert_eq!(cursor(&t), 2, "a stale `g` does not fire gg");
        // Within 0.8 s of that second `g`, `gg` fires.
        apply_at(&mut vim, &mut t, "list", key(5, Some('g')), t0 + Duration::from_millis(1100));
        assert_eq!(cursor(&t), 0, "a fresh `g` pair fires gg");
    }

    #[test]
    fn search_bar_edits_and_undo() {
        let mut vim = VimKeys::new();
        let mut t = matching_rows();
        assert_eq!(apply(&mut vim, &mut t, "list", false, key(44, Some('/'))), VimAction::OpenSearch { back: false });
        // open at row 0, origin captured
        assert_eq!(vim.search.as_ref().unwrap().origin, 0);
        for c in ['b', 'e', 't', 'a'] {
            apply(&mut vim, &mut t, "list", false, key(0, Some(c)));
        }
        assert_eq!(vim.search.as_ref().unwrap().query, "beta");
        assert_eq!(vim.search.as_ref().unwrap().status, "1/2", "typing advances to the first hit");
        assert_eq!(cursor(&t), 1, "incremental search moved the cursor to 'beta'");
        // backspace
        apply(&mut vim, &mut t, "list", false, key(KEY_DELETE, None));
        assert_eq!(vim.search.as_ref().unwrap().query, "bet");
        // Ctrl+W drops the whole (only) word
        apply(&mut vim, &mut t, "list", false, ctrl(KEY_W));
        assert_eq!(vim.search.as_ref().unwrap().query, "");
        assert_eq!(vim.search.as_ref().unwrap().status, "");
        assert_eq!(cursor(&t), 0, "an empty query snaps back to the origin");
        // type then Cmd+Z restores the prior query
        apply(&mut vim, &mut t, "list", false, key(0, Some('b')));
        apply(&mut vim, &mut t, "list", false, key(0, Some('e')));
        let cmd_z = KeyInput { key_code: KEY_Z, chars: Some('z'), cmd: true, ctrl: false, opt: false, shift: false, esc_streak: 0 };
        apply(&mut vim, &mut t, "list", false, cmd_z);
        assert_eq!(vim.search.as_ref().unwrap().query, "b");
    }

    #[test]
    fn search_bar_steps_and_wraps() {
        let mut vim = VimKeys::new();
        let mut t = matching_rows();
        apply(&mut vim, &mut t, "list", false, key(44, Some('/')));
        for c in ['b', 'e', 't', 'a'] {
            apply(&mut vim, &mut t, "list", false, key(0, Some(c)));
        }
        assert_eq!(cursor(&t), 1);
        assert_eq!(vim.search.as_ref().unwrap().status, "1/2");
        // Down steps to the second hit
        apply(&mut vim, &mut t, "list", false, key(KEY_DOWN, None));
        assert_eq!(cursor(&t), 3);
        assert_eq!(vim.search.as_ref().unwrap().status, "2/2");
        // Down again wraps back to the first
        apply(&mut vim, &mut t, "list", false, key(KEY_DOWN, None));
        assert_eq!(cursor(&t), 1);
        // Up wraps to the last
        apply(&mut vim, &mut t, "list", false, key(KEY_UP, None));
        assert_eq!(cursor(&t), 3);
    }

    #[test]
    fn search_bar_esc_restores_origin() {
        let mut vim = VimKeys::new();
        let mut t = matching_rows();
        // move to row 4, then open search and type; Esc puts us back at row 4
        apply(&mut vim, &mut t, "list", false, key(KEY_J, Some('j')));
        apply(&mut vim, &mut t, "list", false, key(KEY_J, Some('j')));
        apply(&mut vim, &mut t, "list", false, key(KEY_J, Some('j')));
        apply(&mut vim, &mut t, "list", false, key(KEY_J, Some('j')));
        assert_eq!(cursor(&t), 4);
        apply(&mut vim, &mut t, "list", false, key(44, Some('/')));
        for c in ['b', 'e', 't', 'a'] {
            apply(&mut vim, &mut t, "list", false, key(0, Some(c)));
        }
        assert_eq!(cursor(&t), 1);
        assert_eq!(apply(&mut vim, &mut t, "list", false, key(KEY_ESC, None)), VimAction::Handled);
        assert_eq!(cursor(&t), 4, "Esc restores the cursor to where / opened");
        assert!(vim.search.is_none());
        assert!(!vim.highlighting);
    }

    #[test]
    fn search_bar_accept_remembers_the_query() {
        let mut vim = VimKeys::new();
        let mut t = matching_rows();
        apply(&mut vim, &mut t, "list", false, key(44, Some('/')));
        for c in ['b', 'e', 't', 'a'] {
            apply(&mut vim, &mut t, "list", false, key(0, Some(c)));
        }
        assert_eq!(apply(&mut vim, &mut t, "list", false, key(KEY_RETURN, None)), VimAction::Handled);
        assert_eq!(vim.last_query, "beta");
        assert!(!vim.last_back);
        // n / N now move over the real list
        assert_eq!(apply(&mut vim, &mut t, "list", false, key(KEY_N, Some('n'))), VimAction::RepeatSearch { reverse: false });
        assert_eq!(cursor(&t), 3, "n skips to the next match");
        assert_eq!(apply(&mut vim, &mut t, "list", false, key(KEY_N, Some('N'))), VimAction::RepeatSearch { reverse: true });
        assert_eq!(cursor(&t), 1, "N steps back");
        assert!(vim.highlighting, "repeat marks the highlight for repaint");
    }

    #[test]
    fn search_bar_paste_and_copy_actions() {
        let mut vim = VimKeys::new();
        let mut t = matching_rows();
        apply(&mut vim, &mut t, "list", false, key(44, Some('/')));
        let cmd_v = KeyInput { key_code: KEY_V, chars: Some('v'), cmd: true, ctrl: false, opt: false, shift: false, esc_streak: 0 };
        assert_eq!(apply_clip(&mut vim, &mut t, "list", cmd_v, "al\npha"), VimAction::Handled);
        assert_eq!(vim.search.as_ref().unwrap().query, "al pha", "paste joins lines with spaces");
        let cmd_c = KeyInput { key_code: KEY_C, chars: Some('c'), cmd: true, ctrl: false, opt: false, shift: false, esc_streak: 0 };
        assert_eq!(
            apply_clip(&mut vim, &mut t, "list", cmd_c, ""),
            VimAction::SearchCopy("al pha".to_string())
        );
    }

    #[test]
    fn search_bar_text_target_uses_utf16_origin() {
        let mut vim = VimKeys::new();
        let mut t = VimTarget::Text(VimText::new("beta one two beta", 0, 40.0, 20.0));
        apply(&mut vim, &mut t, "editor", false, key(44, Some('/')));
        for c in ['b', 'e', 't', 'a'] {
            apply(&mut vim, &mut t, "editor", false, key(0, Some(c)));
        }
        assert_eq!(cursor(&t), 0, "incremental match at the origin");
        apply(&mut vim, &mut t, "editor", false, key(KEY_DOWN, None));
        assert_eq!(cursor(&t), 13, "stepping to the second 'beta'");
        assert_eq!(apply(&mut vim, &mut t, "editor", false, key(KEY_ESC, None)), VimAction::Handled);
        assert_eq!(cursor(&t), 0, "Esc restores the text origin");
    }

    #[test]
    fn visual_counts_and_edit_ops_are_not_in_vim_keys() {
        // VimKeys.swift implements no visual mode, numeric counts, or
        // delete/change/put ops; those live in nvim (the notes vim pane).
        // Guard the surface so a partial port is noticed: `w`/`d`/`c`/`p`/digits
        // simply fall through to `Pass` in a non-field normal target.
        let mut vim = VimKeys::new();
        let mut t = matching_rows();
        for (code, c) in [(13u16, 'w'), (2, 'd'), (8, 'c'), (35, 'p'), (29, '0'), (18, '1')] {
            assert_eq!(
                apply(&mut vim, &mut t, "list", false, key(code, Some(c))),
                VimAction::Pass,
                "'{c}' is not a VimKeys normal-mode motion"
            );
        }
    }
}
