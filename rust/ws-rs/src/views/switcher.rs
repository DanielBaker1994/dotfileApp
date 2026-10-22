//! Port of `ViewSwitcher.swift` — the `ViewSwitcherPanel` that lists the
//! header views (Ctrl+B W), previous view pre-selected, 1-9 / Return to jump.
//!
//! The list model, fuzzy filter and the digit-jump keys are complete and
//! tested; [`ViewSwitcherPanel::build`] constructs the floating `PopupPanel`
//! with its search field and row buttons on macOS.

use crate::engines::links::{self, Entry, LinkGroup, LinkRow, LinkStatus};
use crate::engines::list_filter::FuzzyIndex;
use crate::engines::switcher_status::SwitcherStatus;
use crate::ui::chrome::Rect;
use serde_json::{json, Value};

#[cfg(target_os = "macos")]
use objc2::rc::Retained;
#[cfg(target_os = "macos")]
use crate::ui::popup::PopupPanel;

/// One header view offered by the switcher (`ViewSwitcherPanel.all` rows).
#[derive(Clone, Debug, PartialEq)]
pub struct SwitchRow {
    pub nav_id: i64,
    pub name: String,
    pub location: String,
    pub aliases: Vec<String>,
    /// 1-9 for the first nine rows, else 0 (`number: i < 9 ? i + 1 : 0`).
    pub number: i64,
    pub is_current: bool,
}

impl SwitchRow {
    pub fn title(&self) -> &str {
        &self.name
    }

    /// The `testState()` row string the Swift panel emits: `"Name|location"`.
    pub fn summary(&self) -> String {
        format!("{}|{}", self.name, self.location)
    }
}

/// The `(id, name, icon, location, aliases)` tuple `show(_:current:preselect:)`
/// takes (the icon is AppKit-only and omitted).
#[derive(Clone, Debug, PartialEq)]
pub struct SwitchView {
    pub id: i64,
    pub name: String,
    pub location: String,
    pub aliases: Vec<String>,
}

impl SwitchView {
    pub fn new(id: i64, name: impl Into<String>, location: impl Into<String>) -> Self {
        SwitchView {
            id,
            name: name.into(),
            location: location.into(),
            aliases: Vec::new(),
        }
    }
}

/// macOS `NSEvent.keyCode` for the top-row digits 1-9.
pub const DIGIT_KEYCODES: [(u16, i64); 9] = [
    (18, 1),
    (19, 2),
    (20, 3),
    (21, 4),
    (23, 5),
    (22, 6),
    (26, 7),
    (28, 8),
    (25, 9),
];

pub fn digit_for(code: u16) -> Option<i64> {
    DIGIT_KEYCODES.iter().find(|(c, _)| *c == code).map(|(_, n)| *n)
}

// Panel geometry mirroring the Swift `PopupConfig` (width 520, row 32).
pub const PANEL_WIDTH: f64 = 520.0;
pub const ROW_HEIGHT: f64 = 32.0;
pub const SEARCH_HEIGHT: f64 = 34.0;
pub const PANEL_MARGIN: f64 = 6.0;
pub const MAX_ROWS: usize = 12;
pub const SEARCH_PLACEHOLDER: &str = "";

/// A plain (bottom-left) frame, matching `NSWindow.frame` in the placement math.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SwitchFrame {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl SwitchFrame {
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        SwitchFrame { x, y, width, height }
    }

    pub fn mid_x(&self) -> f64 {
        self.x + self.width / 2.0
    }

    pub fn mid_y(&self) -> f64 {
        self.y + self.height / 2.0
    }
}

/// The panel's size for a given number of rows (`dynamicHeight`).
pub fn panel_size(row_count: usize) -> (f64, f64) {
    let rows = row_count.clamp(1, MAX_ROWS);
    (
        PANEL_WIDTH,
        SEARCH_HEIGHT + PANEL_MARGIN * 2.0 + rows as f64 * ROW_HEIGHT,
    )
}

/// `show(_:over:)`'s placement: centered over the host, nudged down 12%.
pub fn panel_origin(host: SwitchFrame, size: (f64, f64)) -> (f64, f64) {
    (
        host.mid_x() - size.0 / 2.0,
        host.mid_y() - size.1 / 2.0 + host.height * 0.12,
    )
}

/// The stacked row rects (flipped, top-left origin) inside the rows view.
pub fn row_frames(count: usize) -> Vec<Rect> {
    (0..count)
        .map(|i| Rect::new(0.0, i as f64 * ROW_HEIGHT, PANEL_WIDTH, ROW_HEIGHT))
        .collect()
}

/// The visible label for a row (the Swift `draw` text, minus the icon).
pub fn row_summary(row: &SwitchRow) -> String {
    let number = if row.number > 0 {
        row.number.to_string()
    } else {
        " ".to_string()
    };
    let location = if row.location.is_empty() {
        String::new()
    } else {
        format!("   {}", row.location)
    };
    let current = if row.is_current { "   ● here" } else { "" };
    format!("{number}   {}{location}{current}", row.name)
}

/// True when a query enters palette command mode — i.e. it starts with `/`
/// (leading whitespace ignored), mirroring the Swift `/` split (`q.hasPrefix("/")`).
pub fn is_command_query(query: &str) -> bool {
    query.trim_start().starts_with('/')
}

/// Indices into `commands` (`(name, title)` pairs) matching the `/` query, in
/// fuzzy-ranked order. An empty `/` (or `/ `) lists every command.
pub fn filter_commands(commands: &[(String, String)], query: &str) -> Vec<usize> {
    let q = query.trim_start();
    let q = q.strip_prefix('/').unwrap_or(q).trim();
    if q.is_empty() {
        return (0..commands.len()).collect();
    }
    let texts: Vec<String> = commands
        .iter()
        .map(|(name, title)| format!("{title} {name}"))
        .collect();
    FuzzyIndex::new(&texts).ranked(q)
}

/// The pick / command / close callbacks, shared with the AppKit rows (a row
/// click picks without going through the host's key router).
#[derive(Default)]
pub struct SwitchCallbacks {
    pub on_pick: Option<Box<dyn FnMut(i64)>>,
    /// Called with the command name when a command row is picked (`/` mode).
    pub on_command: Option<Box<dyn FnMut(String)>>,
    /// Called (alongside hiding) when Esc cancels the panel.
    pub on_close: Option<Box<dyn FnMut()>>,
    /// A click on the link group (its header, or a row to jump to).
    pub on_link: Option<Box<dyn FnMut(LinkPick)>>,
    /// The search text changed (the host re-reads the group's state).
    pub on_query: Option<Box<dyn FnMut()>>,
}

/// What the link group asks the host to do (PLAN-link-bank §3/§4).
#[derive(Clone, Debug, PartialEq)]
pub enum LinkPick {
    /// Jump to the other app's place (herdr: `herdr-pick go-id <id>`).
    Go(LinkRow),
    /// Return on the header: just bring the other app forward (Ghostty),
    /// no particular place.
    Focus,
    /// The header of an offline / unreachable provider: start it.
    Launch,
    /// The header was clicked (the host asks the model what that means).
    HeaderClick,
}

#[derive(Default)]
pub struct ViewSwitcherPanel {
    pub rows: Vec<SwitchRow>,
    pub current_query: String,
    pub selection: usize,
    pub shown: bool,
    /// Palette commands for `/` command mode as `(name, title)` pairs.
    pub commands: Vec<(String, String)>,
    pub callbacks: std::rc::Rc<std::cell::RefCell<SwitchCallbacks>>,
    /// The other app's places, collapsed under the views (herdr workspaces).
    pub links: LinkGroup,
    /// The cursor when it is on the group (`Header` / `Link(i)`); `None` =
    /// on the local row at `selection`.
    pub link_cursor: Option<Entry>,
    /// The query the cursor was last placed for (a change resets it).
    seen_query: String,
    #[cfg(target_os = "macos")]
    ui: Option<std::rc::Rc<switch_ui::SwitchUi>>,
}

impl std::fmt::Debug for ViewSwitcherPanel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ViewSwitcherPanel")
            .field("rows", &self.rows)
            .field("current_query", &self.current_query)
            .field("selection", &self.selection)
            .field("shown", &self.shown)
            .finish()
    }
}

impl ViewSwitcherPanel {
    pub fn new() -> Self {
        let mut p = ViewSwitcherPanel::default();
        // No group until a host with a link provider turns it on
        // (`set_links`); the egui switcher has none.
        p.links.enabled = false;
        p
    }

    /// `show(_:current:preselect:)` — build the rows, then select the
    /// pre-selected view (or the first).
    pub fn show(&mut self, views: &[SwitchView], current: Option<i64>, preselect: Option<i64>) {
        self.rows = views
            .iter()
            .enumerate()
            .map(|(i, v)| SwitchRow {
                nav_id: v.id,
                name: v.name.clone(),
                location: v.location.clone(),
                aliases: v.aliases.clone(),
                number: if i < 9 { i as i64 + 1 } else { 0 },
                is_current: current == Some(v.id),
            })
            .collect();
        self.current_query.clear();
        #[cfg(target_os = "macos")]
        if let Some(ui) = &self.ui {
            ui.clear_query();
        }
        self.selection = self
            .rows
            .iter()
            .position(|r| preselect == Some(r.nav_id))
            .unwrap_or(0);
        // Every fresh open starts expanded or collapsed per `[links]
        // start-expanded` (the last rows stay, stale), the cursor on the
        // pinned header so Return switches apps.
        self.links.reopen();
        self.seen_query.clear();
        self.link_cursor = (self.links.enabled && self.links.opts.start_on_group).then_some(Entry::Header);
        self.shown = true;
    }

    fn search_text(r: &SwitchRow) -> String {
        format!("{} {} {}", r.name, r.location, r.aliases.join(" "))
    }

    /// `filter(_:)` — indices into [`Self::rows`], ranked by the shared fuzzy
    /// matcher; an empty query keeps source order.
    pub fn filter(&self, query: &str) -> Vec<usize> {
        let q = query.trim();
        if q.is_empty() {
            return (0..self.rows.len()).collect();
        }
        let texts: Vec<String> = self.rows.iter().map(Self::search_text).collect();
        let mut idx = FuzzyIndex::new(&texts);
        idx.ranked(q)
    }

    /// `key(_:_:)` — a bare digit jumps straight to that numbered view. Returns
    /// the picked nav id, or `None` if the key is not consumed.
    pub fn key(&self, code: u16, cmd: bool, ctrl: bool, opt: bool) -> Option<i64> {
        if cmd || ctrl || opt || !self.live_query().is_empty() {
            return None;
        }
        let n = digit_for(code)?;
        self.rows
            .iter()
            .find(|r| r.number == n)
            .map(|r| r.nav_id)
    }

    /// Up/Down (`ViewSwitcher.key`): move the selection inside the filtered
    /// list, wrapping.
    pub fn move_selection(&mut self, delta: i64) {
        self.reconcile_query();
        let list = self.entries();
        if list.is_empty() {
            return;
        }
        let pos = self.cursor_pos(&list);
        let next = links::step(&list, pos, delta);
        self.set_cursor(list[next]);
        self.sync_ui();
    }

    /// Return: the selected row's nav id (the filtered list's selection
    /// index); `None` while the cursor is on the link group.
    pub fn accept(&self) -> Option<i64> {
        if self.link_cursor.is_some() && !self.command_mode() {
            return None;
        }
        let idx = self.filter(&self.live_query());
        let i = idx
            .iter()
            .position(|j| *j == self.selection)
            .unwrap_or(0);
        idx.get(i).map(|j| self.rows[*j].nav_id)
    }

    /// The full list in view mode: the matching views, then the link
    /// group's header and visible rows (`/` command mode has no group).
    pub fn entries(&self) -> Vec<Entry> {
        let q = self.live_query();
        if is_command_query(&q) {
            return Vec::new();
        }
        links::entries(&self.filter(&q), &self.links, &q)
    }

    /// The cursor's position in `list`.
    fn cursor_pos(&self, list: &[Entry]) -> usize {
        let want = self.link_cursor.unwrap_or(Entry::Local(self.selection));
        list.iter().position(|e| *e == want).unwrap_or(0)
    }

    fn set_cursor(&mut self, e: Entry) {
        match e {
            Entry::Local(i) => {
                self.selection = i;
                self.link_cursor = None;
            }
            other => self.link_cursor = Some(other),
        }
    }

    /// Catch the model up with the search field (after typing).
    pub fn sync_query(&mut self) {
        self.reconcile_query();
    }

    /// Wire the link group's click / typing callbacks.
    pub fn set_link_callbacks(&mut self, on_link: Box<dyn FnMut(LinkPick)>, on_query: Box<dyn FnMut()>) {
        let mut cb = self.callbacks.borrow_mut();
        cb.on_link = Some(on_link);
        cb.on_query = Some(on_query);
    }

    /// Typing moved on: clearing the query restores the group's default, and the
    /// cursor goes to the top like the drawn list (`SwitchUi::refilter`).
    fn reconcile_query(&mut self) {
        let q = self.live_query();
        if q == self.seen_query {
            return;
        }
        if q.trim().is_empty() && !self.seen_query.trim().is_empty() {
            self.links.expanded = self.links.opts.start_expanded;
        }
        self.seen_query = q;
        self.place_cursor();
        self.sync_ui();
    }

    /// The cursor's start for the current query (`links::start_pos`).
    fn place_cursor(&mut self) {
        let list = self.entries();
        match list.get(links::start_pos(&list, &self.links, &self.live_query())) {
            Some(e) => self.set_cursor(*e),
            None => self.link_cursor = None,
        }
    }

    /// Return while the cursor is on the group: the header switches to the
    /// other app (or starts it), a row jumps there. `None` = the cursor is
    /// on a local row.
    pub fn link_action(&mut self) -> Option<LinkPick> {
        self.reconcile_query();
        if self.command_mode() {
            return None;
        }
        match self.link_cursor? {
            Entry::Header => Some(self.header_action()),
            Entry::Link(i) => self.links.rows.get(i).cloned().map(LinkPick::Go),
            Entry::Local(_) => None,
        }
    }

    fn header_action(&mut self) -> LinkPick {
        if self.links.status.dim() {
            LinkPick::Launch
        } else {
            LinkPick::Focus
        }
    }

    /// A click on the header: put the cursor there and act like Return.
    pub fn header_click(&mut self) -> LinkPick {
        self.reconcile_query();
        self.link_cursor = Some(Entry::Header);
        self.header_action()
    }

    /// → / l open the group from its header; ← / h close it from the header
    /// or one of its rows (the cursor lands on the header). False = not on
    /// the group (the key goes to the search field).
    pub fn group_open(&mut self, open: bool) -> bool {
        self.reconcile_query();
        let ok = match self.link_cursor {
            Some(Entry::Header) => true,
            Some(Entry::Link(_)) => !open,
            _ => false,
        };
        if ok && !self.command_mode() {
            self.links.expanded = open;
            self.link_cursor = Some(Entry::Header);
            self.sync_ui();
        }
        ok
    }

    /// Ctrl+K: jump to the header and open the group.
    pub fn links_jump(&mut self) -> bool {
        self.reconcile_query();
        if self.command_mode() || !self.links.header_shown(&self.live_query()) {
            return false;
        }
        self.links.expanded = true;
        self.link_cursor = Some(Entry::Header);
        self.sync_ui();
        true
    }

    /// The provider answered (in the background): new rows / status, the
    /// header texts, the `[links] enabled` gate and how the group opens.
    pub fn set_links(
        &mut self,
        result: Result<Vec<LinkRow>, links::LinkError>,
        texts: links::LinkTexts,
        enabled: bool,
        opts: links::LinkOpts,
    ) {
        let was_on = self.links.enabled;
        self.links.apply(result);
        self.links.texts = texts;
        self.links.enabled = enabled;
        if opts.start_expanded != self.links.opts.start_expanded && self.live_query().trim().is_empty() {
            self.links.expanded = opts.start_expanded;
        }
        self.links.opts = opts;
        if enabled && !was_on && self.live_query().trim().is_empty() {
            self.place_cursor();
        }
        // A row the cursor sat on may be gone: fall back to the header.
        if let Some(Entry::Link(i)) = self.link_cursor {
            if i >= self.links.rows.len() {
                self.link_cursor = Some(Entry::Header);
            }
        }
        if !self.entries().contains(&self.link_cursor.unwrap_or(Entry::Local(self.selection))) {
            self.link_cursor = None;
        }
        self.sync_ui();
    }

    /// Hide the panel: `orderOut` the window (if built) and clear `shown`.
    pub fn hide_panel(&mut self) {
        self.shown = false;
        #[cfg(target_os = "macos")]
        if let Some(ui) = &self.ui {
            ui.panel.orderOut(None);
        }
    }

    /// Whether the panel window is currently ordered front (`NSWindow.isVisible`);
    /// falls back to the `shown` flag when the AppKit window is not built.
    pub fn is_visible(&self) -> bool {
        #[cfg(target_os = "macos")]
        if let Some(ui) = &self.ui {
            return ui.panel.isVisible();
        }
        self.shown
    }

    /// Whether the panel window currently has the keyboard.
    pub fn has_key(&self) -> bool {
        #[cfg(target_os = "macos")]
        if let Some(ui) = &self.ui {
            return ui.panel.isKeyWindow();
        }
        false
    }

    /// The live query: the AppKit search field's text when built, else
    /// [`Self::current_query`].
    pub fn live_query(&self) -> String {
        #[cfg(target_os = "macos")]
        if let Some(ui) = &self.ui {
            return ui.query();
        }
        self.current_query.clone()
    }

    /// True when the current search text starts with `/` (palette command mode).
    pub fn command_mode(&self) -> bool {
        is_command_query(&self.live_query())
    }

    /// Store the palette commands (`(name, title)` pairs) used in `/` mode.
    pub fn set_commands(&mut self, commands: Vec<(String, String)>) {
        self.commands = commands;
        self.sync_ui();
    }

    /// Wire the pick and command callbacks.
    pub fn set_callbacks(
        &mut self,
        on_pick: Box<dyn FnMut(i64)>,
        on_command: Box<dyn FnMut(String)>,
    ) {
        let mut cb = self.callbacks.borrow_mut();
        cb.on_pick = Some(on_pick);
        cb.on_command = Some(on_command);
    }

    /// Wire the Esc/close callback.
    pub fn set_on_close(&mut self, on_close: Box<dyn FnMut()>) {
        self.callbacks.borrow_mut().on_close = Some(on_close);
    }

    /// The row icons (`SharedWindow.navIcons` images) by nav id.
    #[cfg(target_os = "macos")]
    pub fn set_icons(&mut self, icons: Vec<(i64, Retained<objc2_app_kit::NSImage>)>) {
        if let Some(mtm) = objc2::MainThreadMarker::new() {
            self.ensure_ui(mtm);
        }
        if let Some(ui) = &self.ui {
            *ui.icons.borrow_mut() = icons.into_iter().collect();
        }
    }

    /// The currently displayed rows as strings: filtered view summaries
    /// (`"Name|location"`) in view mode, or the command titles in `/` mode.
    pub fn display_rows(&self) -> Vec<String> {
        let query = self.live_query();
        if is_command_query(&query) {
            filter_commands(&self.commands, &query)
                .into_iter()
                .map(|i| self.commands[i].1.clone())
                .collect()
        } else {
            self.filter(&query)
                .into_iter()
                .map(|i| self.rows[i].summary())
                .collect()
        }
    }

    /// `testState()` shape for the socket, keeping the Swift form
    /// (`shown` / `selection` / `rows` of `"Name|location"` strings) and adding
    /// the live `query`.
    pub fn test_state(&self) -> Value {
        let q = self.live_query();
        let cursor = match self.link_cursor {
            Some(Entry::Header) => "header".to_string(),
            Some(Entry::Link(i)) => format!("link:{}", self.links.rows.get(i).map(|r| r.id.as_str()).unwrap_or("")),
            _ => "local".to_string(),
        };
        let mut group = self.links.test_state(&q);
        group["cursor"] = json!(cursor);
        json!({
            "shown": self.shown,
            "selection": self.selection,
            "rows": self.display_rows(),
            "query": q,
            "linkGroup": group,
        })
    }

    /// Build the AppKit panel (search line + drawn rows).
    #[cfg(target_os = "macos")]
    pub fn build(&mut self, mtm: objc2::MainThreadMarker) {
        self.ensure_ui(mtm);
    }

    #[cfg(target_os = "macos")]
    fn ensure_ui(&mut self, mtm: objc2::MainThreadMarker) {
        if self.ui.is_none() {
            self.ui = Some(switch_ui::SwitchUi::new(mtm, self.callbacks.clone()));
        }
    }

    /// Push rows / filter / selection into the drawn list and size the panel.
    fn sync_ui(&self) {
        #[cfg(target_os = "macos")]
        if let Some(ui) = &self.ui {
            ui.sync(self);
        }
    }

    /// `show` over a host window: rebuild the rows and place the panel.
    pub fn present(&mut self, host: Option<SwitchFrame>) {
        self.shown = true;
        #[cfg(target_os = "macos")]
        {
            let Some(mtm) = objc2::MainThreadMarker::new() else { return };
            self.ensure_ui(mtm);
            let ui = self.ui.clone().unwrap();
            ui.sync(self);
            let size = ui.panel.frame().size;
            if let Some(host) = host {
                let (x, y) = panel_origin(host, (size.width, size.height));
                ui.panel.setFrameOrigin(objc2_foundation::NSPoint::new(x, y));
            }
            ui.panel.makeKeyAndOrderFront(None);
            ui.panel.makeFirstResponder(Some(&ui.list.search));
        }
        #[cfg(not(target_os = "macos"))]
        let _ = host;
    }
}

/// The AppKit side of the view switcher: `ViewSwitcherPanel`'s
/// `PopupWindow(toolPanel)` look via [`crate::ui::list_panel::ListPanel`] and
/// `ViewSwitcherPanel.draw(_:_:_:)` for each row.
#[cfg(target_os = "macos")]
mod switch_ui {
    use super::*;
    use crate::ui::list_panel::ListPanel;
    use crate::ui::style::{self, Weight};
    use crate::ui::theme::PopupColors;
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSImage;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::{Rc, Weak};

    /// What the row drawer reads.
    #[derive(Default)]
    struct DrawModel {
        rows: Vec<SwitchRow>,
        commands: Vec<(String, String)>,
        /// Indices into `commands` (`/` mode).
        visible: Vec<usize>,
        /// View mode: the views, then the link group's header + rows.
        entries: Vec<Entry>,
        command_mode: bool,
        links: LinkGroup,
        /// The query `entries` was built for (clearing it re-collapses).
        seen: String,
    }

    impl DrawModel {
        fn len(&self) -> usize {
            if self.command_mode { self.visible.len() } else { self.entries.len() }
        }
    }

    pub struct SwitchUi {
        pub panel: Retained<PopupPanel>,
        pub list: ListPanel,
        pub icons: RefCell<HashMap<i64, Retained<NSImage>>>,
        model: RefCell<DrawModel>,
        callbacks: Rc<RefCell<SwitchCallbacks>>,
        /// Typing re-filters through the owning panel's model; the panel
        /// lives in the host state, so the field change re-syncs from here.
        colors: PopupColors,
    }

    impl SwitchUi {
        pub fn new(mtm: MainThreadMarker, callbacks: Rc<RefCell<SwitchCallbacks>>) -> Rc<SwitchUi> {
            use crate::ui::popup::{CardFrame, PopupConfig};
            let colors = crate::ui::theme::PopupThemeDefaults::colors();
            let list = ListPanel::new(mtm, colors, PANEL_WIDTH, ROW_HEIGHT, SEARCH_PLACEHOLDER);
            let h = list.height(1, MAX_ROWS);
            let config = PopupConfig {
                name: "view-switcher".to_string(),
                frame: CardFrame { x: 0.0, y: 0.0, width: PANEL_WIDTH, height: h },
                tool_panel: true,
                floating: true,
                ..PopupConfig::default()
            };
            let panel = PopupPanel::create(mtm, &config);
            panel.setOpaque(false);
            panel.setBackgroundColor(Some(&objc2_app_kit::NSColor::clearColor()));
            panel.setHasShadow(true);
            panel.setContentView(Some(&list.root));
            let ui = Rc::new(SwitchUi {
                panel,
                list,
                icons: RefCell::new(HashMap::new()),
                model: RefCell::new(DrawModel::default()),
                callbacks,
                colors,
            });
            // Click-off closes the switcher (like Esc).
            let weak: Weak<SwitchUi> = Rc::downgrade(&ui);
            ui.panel.set_on_resign(Box::new(move || {
                let Some(ui) = weak.upgrade() else { return };
                if !ui.panel.isVisible() {
                    return;
                }
                ui.panel.orderOut(None);
                // `on_close` may already be borrowed (a pick in flight).
                let guard = ui.callbacks.try_borrow_mut();
                if let Ok(mut cb) = guard {
                    if let Some(f) = cb.on_close.as_mut() {
                        f();
                    }
                }
            }));
            let weak: Weak<SwitchUi> = Rc::downgrade(&ui);
            ui.list.set_drawer(Box::new(move |r, i, sel, hover| {
                if let Some(ui) = weak.upgrade() {
                    ui.draw_row(r, i, sel, hover);
                }
            }));
            let weak: Weak<SwitchUi> = Rc::downgrade(&ui);
            ui.list.set_on_click(Box::new(move |row, _x, _double| {
                if let Some(ui) = weak.upgrade() {
                    ui.pick_visible(row);
                }
            }));
            let weak: Weak<SwitchUi> = Rc::downgrade(&ui);
            let weak_cmd: Weak<SwitchUi> = Rc::downgrade(&ui);
            ui.list.set_on_field(
                Box::new(move |q: String| {
                    if let Some(ui) = weak.upgrade() {
                        ui.refilter(&q);
                        if let Ok(mut cb) = ui.callbacks.try_borrow_mut() {
                            if let Some(f) = cb.on_query.as_mut() {
                                f();
                            }
                        }
                    }
                }),
                Box::new(move |cmd| {
                    // Arrows / Return / Esc arrive through the host's key
                    // monitor first; this covers a click-focused field.
                    let Some(ui) = weak_cmd.upgrade() else { return false };
                    match cmd {
                        "cancelOperation:" => {
                            ui.panel.orderOut(None);
                            if let Some(cb) = ui.callbacks.borrow_mut().on_close.as_mut() {
                                cb();
                            }
                            true
                        }
                        "insertNewline:" => {
                            let sel = ui.list.rows.selection();
                            ui.pick_visible(sel);
                            true
                        }
                        _ => false,
                    }
                }),
            );
            ui
        }

        pub fn query(&self) -> String {
            self.list.search.stringValue().to_string()
        }

        pub fn clear_query(&self) {
            self.list.search.setStringValue(&objc2_foundation::NSString::from_str(""));
        }

        /// Mirror the panel model into the drawn rows and resize to fit.
        pub fn sync(&self, p: &ViewSwitcherPanel) {
            let q = self.query();
            let command_mode = is_command_query(&q);
            let visible = if command_mode { filter_commands(&p.commands, &q) } else { Vec::new() };
            let entries = p.entries();
            let sel = if command_mode { 0 } else { p.cursor_pos(&entries) };
            *self.model.borrow_mut() = DrawModel {
                rows: p.rows.clone(),
                commands: p.commands.clone(),
                visible,
                entries,
                command_mode,
                links: p.links.clone(),
                seen: q,
            };
            let n = self.model.borrow().len();
            self.list.set_rows(n, sel);
            self.resize(n);
        }

        /// Typing in the field: re-filter with the stored rows, cursor on top.
        fn refilter(&self, q: &str) {
            let n = {
                let mut m = self.model.borrow_mut();
                m.command_mode = is_command_query(q);
                if m.command_mode {
                    m.visible = filter_commands(&m.commands, q);
                } else {
                    if q.trim().is_empty() && !m.seen.trim().is_empty() {
                        m.links.expanded = m.links.opts.start_expanded;
                    }
                    let tmp = ViewSwitcherPanel { rows: m.rows.clone(), ..ViewSwitcherPanel::new() };
                    m.entries = links::entries(&tmp.filter(q), &m.links, q);
                }
                m.seen = q.to_string();
                let sel = if m.command_mode { 0 } else { links::start_pos(&m.entries, &m.links, q) };
                (m.len(), sel)
            };
            let (n, sel) = n;
            self.list.set_rows(n, sel);
            self.resize(n);
        }

        /// Minimalist width: the longest view name + the number column and
        /// padding (no icon / location / "here" mark).
        fn fit_width(&self) -> f64 {
            let font = style::system_font(13.0, Weight::Medium);
            let small = style::system_font(12.0, Weight::Regular);
            let m = self.model.borrow();
            let longest = m.rows.iter().map(|r| style::text_size(&r.name, &font).0).fold(0.0, f64::max);
            // The link group widens the panel to its header / rows, capped.
            let mut group: f64 = 0.0;
            if m.entries.contains(&Entry::Header) {
                group = style::text_size(&format!("▾ {}", m.links.header_text()), &font).0;
            }
            for e in &m.entries {
                if let Entry::Link(i) = e {
                    if let Some(r) = m.links.rows.get(*i) {
                        let w = style::text_size(&r.title, &font).0 + 14.0 + style::text_size(&r.subtitle, &small).0 + 16.0;
                        group = group.max(w);
                    }
                }
            }
            (longest.max(group).ceil() + 64.0).clamp(150.0, PANEL_WIDTH)
        }

        fn resize(&self, n: usize) {
            let w = self.fit_width();
            let h = self.list.height(n, MAX_ROWS);
            let f = self.panel.frame();
            // Keep the top edge put (AppKit frames grow from the bottom).
            let top = f.origin.y + f.size.height;
            self.panel.setFrame_display(
                objc2_foundation::NSRect::new(
                    objc2_foundation::NSPoint::new(f.origin.x, top - h),
                    objc2_foundation::NSSize::new(w, h),
                ),
                true,
            );
        }

        /// The row at visible position `pos` was clicked / accepted.
        fn pick_visible(&self, pos: usize) {
            let (command, nav, link) = {
                let m = self.model.borrow();
                if m.command_mode {
                    let Some(&i) = m.visible.get(pos) else { return };
                    (m.commands.get(i).map(|c| c.0.clone()), None, None)
                } else {
                    match m.entries.get(pos) {
                        Some(Entry::Local(i)) => (None, m.rows.get(*i).map(|r| r.nav_id), None),
                        Some(Entry::Header) => (None, None, Some(LinkPick::HeaderClick)),
                        Some(Entry::Link(j)) => (None, None, m.links.rows.get(*j).cloned().map(LinkPick::Go)),
                        None => return,
                    }
                }
            };
            // A header click opens / closes the group in place.
            if link == Some(LinkPick::HeaderClick) {
                if let Ok(mut cb) = self.callbacks.try_borrow_mut() {
                    if let Some(f) = cb.on_link.as_mut() {
                        f(LinkPick::HeaderClick);
                    }
                }
                return;
            }
            self.panel.orderOut(None);
            let mut cb = self.callbacks.borrow_mut();
            if let Some(pick) = link {
                if let Some(f) = cb.on_link.as_mut() {
                    f(pick);
                }
            } else if let Some(name) = command {
                if let Some(f) = cb.on_command.as_mut() {
                    f(name);
                }
            } else if let Some(id) = nav {
                if let Some(f) = cb.on_pick.as_mut() {
                    f(id);
                }
            }
            if let Some(f) = cb.on_close.as_mut() {
                f();
            }
        }

        /// `ViewSwitcherPanel.draw(_:_:_:)`.
        fn draw_row(&self, rect: crate::ui::chrome::Rect, pos: usize, selected: bool, hovered: bool) {
            use crate::ui::chrome::Rect;
            let c = &self.colors;
            let m = self.model.borrow();
            let i = if m.command_mode {
                let Some(&i) = m.visible.get(pos) else { return };
                i
            } else {
                match m.entries.get(pos) {
                    Some(Entry::Local(i)) => *i,
                    Some(Entry::Header) => {
                        let sep = matches!(m.entries.get(pos + 1), Some(Entry::Local(_)));
                        drop(m);
                        self.draw_header(rect, selected, hovered);
                        return self.draw_group_rule(rect, sep);
                    }
                    Some(Entry::Link(j)) => {
                        let j = *j;
                        let sep = matches!(m.entries.get(pos + 1), Some(Entry::Local(_)));
                        drop(m);
                        self.draw_link(rect, j, selected, hovered);
                        return self.draw_group_rule(rect, sep);
                    }
                    None => return,
                }
            };
            let inner = Rect::new(rect.x + 6.0, rect.y + 2.0, rect.width - 12.0, rect.height - 4.0);
            // Mouse feedback parity with the files/jira lists.
            style::row_pill(inner, 7.0, selected, selected, hovered, c, style::RowAccent::Rounded(4.0));
            let mono = style::mono_font(11.0, Weight::Regular);
            let mut x = inner.x + 12.0;
            let band = |x: f64, w: f64| Rect::new(x, inner.y, w.max(0.0), inner.height);
            if m.command_mode {
                let (name, title) = &m.commands[i];
                let nw = style::draw_text_left(title, band(x, 260.0), &style::system_font(13.0, Weight::Medium), c.text);
                x += nw.max(160.0) + 10.0;
                style::draw_text_left(&format!("/{name}"), band(x, inner.max_x() - x - 8.0), &style::system_font(12.0, Weight::Regular), c.dim);
                return;
            }
            let r = &m.rows[i];
            let num = if r.number > 0 { r.number.to_string() } else { " ".into() };
            style::draw_text_left(&num, band(x, 14.0), &mono, c.dim);
            x += 22.0;
            style::draw_text_left(
                &r.name,
                band(x, inner.max_x() - x - 6.0),
                &style::system_font(13.0, Weight::Medium),
                c.text,
            );
        }

        /// The hairline under the pinned group, separating it from the views.
        fn draw_group_rule(&self, rect: crate::ui::chrome::Rect, on: bool) {
            use crate::ui::chrome::Rect;
            if on {
                let c = &self.colors;
                let y = rect.y + rect.height - 1.0;
                style::fill_rect(Rect::new(rect.x + 14.0, y, rect.width - 28.0, 1.0), c.over(c.dim, 0.25, c.background));
            }
        }

        /// `▸ Terminal spaces · 6` — the pinned group's header, dim.
        fn draw_header(&self, rect: crate::ui::chrome::Rect, selected: bool, hovered: bool) {
            use crate::ui::chrome::Rect;
            let c = &self.colors;
            let m = self.model.borrow();
            let inner = Rect::new(rect.x + 6.0, rect.y + 2.0, rect.width - 12.0, rect.height - 4.0);
            style::row_pill(inner, 7.0, selected, selected, hovered, c, style::RowAccent::Rounded(4.0));
            let open = m.links.open(&m.seen);
            let x = inner.x + 12.0;
            let band = Rect::new(x, inner.y, (inner.max_x() - x - 6.0).max(0.0), inner.height);
            let text = format!("{} {}", if open { "▾" } else { "▸" }, m.links.header_text());
            let color = if m.links.status.dim() { c.over(c.dim, 0.6, c.background) } else { c.dim };
            style::draw_text_left(&text, band, &style::system_font(12.0, Weight::Medium), color);
        }

        /// One place of the other app: status dot (theme status tokens),
        /// title, dim subtitle (`5 tabs · working`).
        fn draw_link(&self, rect: crate::ui::chrome::Rect, j: usize, selected: bool, hovered: bool) {
            use crate::ui::chrome::Rect;
            use crate::ui::theme::PopupTone;
            let c = &self.colors;
            let m = self.model.borrow();
            let Some(r) = m.links.rows.get(j) else { return };
            let inner = Rect::new(rect.x + 6.0, rect.y + 2.0, rect.width - 12.0, rect.height - 4.0);
            style::row_pill(inner, 7.0, selected, selected, hovered, c, style::RowAccent::Rounded(4.0));
            let mut x = inner.x + 26.0;
            let tone = match r.status.as_deref() {
                Some("working") => Some(PopupTone::Warning),
                Some("waiting") => Some(PopupTone::Danger),
                Some("idle") => Some(PopupTone::Success),
                _ => None,
            };
            if let Some(t) = tone {
                let d = 7.0;
                style::fill_rounded(Rect::new(x, inner.mid_y() - d / 2.0, d, d), d / 2.0, c.tone(t));
            }
            x += 14.0;
            let font = style::system_font(13.0, Weight::Medium);
            let band = |x: f64, w: f64| Rect::new(x, inner.y, w.max(0.0), inner.height);
            let tw = style::draw_text_left(&r.title, band(x, inner.max_x() - x - 6.0), &font, c.text);
            x += tw + 10.0;
            if !r.subtitle.is_empty() && x < inner.max_x() - 20.0 {
                style::draw_text_left(&r.subtitle, band(x, inner.max_x() - x - 6.0), &style::system_font(12.0, Weight::Regular), c.dim);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The `show` command palette (Hyper+S: the `[app] palette` commands)
// ---------------------------------------------------------------------------

/// What accepting a row does (`accept(_:)`).
#[derive(Clone, Debug, PartialEq)]
pub enum PaletteAction {
    Command(String),
}

/// The Hyper+S palette model: one list of commands (`[app] palette`), the
/// fuzzy filter, the cursor and the accept decision. Pure; the panel draws
/// it. (It used to pair a second, Tab-switched pane of AeroSpace
/// workspaces; that pane is gone — one list, nothing to gather on show.)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PaletteModel {
    pub commands: Vec<(String, String)>,
    pub query: String,
    pub selection: usize,
    pub shown: bool,
    /// The status row's data (`SwitcherController.status`; gathered off-main).
    pub status: SwitcherStatus,
    /// The last filtered query (a change resets the selection).
    last_query: String,
}

impl PaletteModel {
    pub fn new(commands: Vec<(String, String)>) -> Self {
        PaletteModel {
            commands,
            ..Default::default()
        }
    }

    /// Update the query; a change resets the selection.
    pub fn set_query(&mut self, query: &str) {
        let q = query.trim().to_lowercase();
        if q != self.last_query {
            self.last_query = q;
            self.selection = 0;
        }
        self.query = query.to_string();
    }

    /// The commands the query keeps, best match first (a leading `/` is
    /// ignored: the palette is all commands).
    pub fn rows(&self) -> Vec<(String, String)> {
        let q = self.query.trim().to_lowercase();
        let t = q.trim_start_matches('/').trim();
        if t.is_empty() {
            return self.commands.clone();
        }
        let texts: Vec<String> = self
            .commands
            .iter()
            .map(|(name, label)| format!("{label} {name}"))
            .collect();
        let mut idx = FuzzyIndex::new(&texts);
        idx.ranked(t)
            .into_iter()
            .map(|i| self.commands[i].clone())
            .collect()
    }

    /// `moveCursor(_:)` — wrap around the list.
    pub fn move_cursor(&mut self, d: i64) {
        let n = self.rows().len();
        if n == 0 {
            return;
        }
        let cur = self.selection.min(n - 1) as i64;
        self.selection = (cur + d).rem_euclid(n as i64) as usize;
    }

    /// `accept(_:)` — the selected command.
    pub fn accept(&self) -> Option<PaletteAction> {
        self.rows()
            .get(self.selection)
            .map(|(name, _)| PaletteAction::Command(name.clone()))
    }

    /// `switcherKey(_:_:)`: ↑ ↓ / Ctrl+N,P / Tab, Shift+Tab / Return.
    /// Returns the accept action when Return fires.
    pub fn key(&mut self, code: u16, ctrl: bool, shift: bool, cmd: bool, opt: bool) -> Option<PaletteAction> {
        if cmd || opt {
            return None;
        }
        match (code, ctrl) {
            (125, false) | (45, true) => {
                self.move_cursor(1);
                None
            }
            (126, false) | (35, true) => {
                self.move_cursor(-1);
                None
            }
            // One list: Tab just moves down (Shift+Tab up).
            (48, false) => {
                self.move_cursor(if shift { -1 } else { 1 });
                None
            }
            (36, _) | (76, _) => self.accept(),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Snapshot / Event contract (Wave 3 frontend contract; pure, no cfg)
// ---------------------------------------------------------------------------

/// One row a switcher / palette panel draws.
#[derive(Clone, Debug, PartialEq)]
pub enum SwitcherRow {
    /// A view row (the Ctrl+B W switcher, view mode).
    View(SwitchRow),
    /// A command row (the Hyper+S palette, and the Ctrl+B W switcher's `/`
    /// command mode).
    Command { name: String, title: String },
}

/// Everything a switcher / palette panel draws, pushed by the model.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SwitcherSnapshot {
    /// The rows to draw, in display order.
    pub rows: Vec<SwitcherRow>,
    /// The selected row index into [`Self::rows`].
    pub selection: usize,
    pub query: String,
    pub shown: bool,
    /// True in the switcher's `/` command mode.
    pub command_mode: bool,
    /// The palette's status row (the Ctrl+B W switcher has none).
    pub status: Option<SwitcherStatus>,
}

/// Clicks / typing / keys reported to the owner.
#[derive(Clone, Debug, PartialEq)]
pub enum SwitcherEvent {
    /// A row was clicked (`row` is a visible index).
    Click { row: usize },
    Query(String),
    /// Return: accept the selected row.
    Enter,
    /// Up / Down (or Ctrl+N / Ctrl+P).
    Move(i64),
    /// Esc: close (a live palette query clears instead).
    Escape,
    /// The status row's fresh data arrived (`SwitcherController.onShow`).
    Status(SwitcherStatus),
}

impl ViewSwitcherPanel {
    /// Everything the panel draws: the filtered rows (view or `/` command
    /// mode) plus the live query and selection as the lone visible index.
    pub fn snapshot(&self) -> SwitcherSnapshot {
        let query = self.live_query();
        let command_mode = is_command_query(&query);
        let visible = self.filter(&query);
        let mut rows: Vec<SwitcherRow> = Vec::new();
        let mut selection = 0;
        if command_mode {
            for i in filter_commands(&self.commands, &query) {
                let (name, title) = &self.commands[i];
                rows.push(SwitcherRow::Command { name: name.clone(), title: title.clone() });
            }
        } else {
            selection = visible.iter().position(|i| *i == self.selection).unwrap_or(0);
            for i in &visible {
                rows.push(SwitcherRow::View(self.rows[*i].clone()));
            }
        }
        SwitcherSnapshot {
            rows,
            selection,
            query,
            shown: self.shown,
            command_mode,
            status: None,
        }
    }

    /// Dispatch one frontend event through the panel's own methods (the same
    /// ones the AppKit half calls today). Accepts are read back through
    /// [`Self::accept`]; a click selects, it does not pick.
    pub fn handle(&mut self, ev: SwitcherEvent) {
        match ev {
            SwitcherEvent::Query(q) => self.current_query = q,
            SwitcherEvent::Move(d) => self.move_selection(d),
            SwitcherEvent::Enter => {
                if self.accept().is_some() {
                    self.hide_panel();
                }
            }
            SwitcherEvent::Escape => self.hide_panel(),
            SwitcherEvent::Click { row } => {
                if let Some(&i) = self.filter(&self.live_query()).get(row) {
                    self.selection = i;
                }
            }
            SwitcherEvent::Status(_) => {}
        }
    }
}

impl PaletteModel {
    /// Everything the palette draws: the command rows plus the query, the
    /// selection and the status row.
    pub fn snapshot(&self) -> SwitcherSnapshot {
        SwitcherSnapshot {
            rows: self
                .rows()
                .into_iter()
                .map(|(name, title)| SwitcherRow::Command { name, title })
                .collect(),
            selection: self.selection,
            query: self.query.clone(),
            shown: self.shown,
            command_mode: is_command_query(&self.query),
            status: (!self.status.is_empty()).then(|| self.status.clone()),
        }
    }

    /// Dispatch one frontend event through the palette's own methods (the same
    /// ones the AppKit half calls today). Accepts are read back through
    /// [`Self::accept`].
    pub fn handle(&mut self, ev: SwitcherEvent) {
        match ev {
            SwitcherEvent::Query(q) => self.set_query(&q),
            SwitcherEvent::Move(d) => self.move_cursor(d),
            SwitcherEvent::Click { row } => {
                if row < self.rows().len() {
                    self.selection = row;
                }
            }
            SwitcherEvent::Enter => {
                if self.accept().is_some() {
                    self.shown = false;
                }
            }
            SwitcherEvent::Escape => {
                if self.query.trim().is_empty() {
                    self.shown = false;
                } else {
                    self.set_query("");
                    self.selection = 0;
                }
            }
            SwitcherEvent::Status(st) => self.status = st,
        }
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObject};
    use objc2::{
        define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly,
    };
    use objc2_app_kit::{
        NSButton, NSFont, NSTextField, NSTextFieldDelegate, NSView, NSControlTextEditingDelegate,
    };
    use objc2_foundation::{
        NSNotification, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
    };
    use std::cell::RefCell;

    use crate::ui::view_base::as_any;

    pub struct SwitchFlippedViewIvars;

    define_class!(
        #[unsafe(super(crate::ui::view_base::WSFlippedView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSSwitchFlippedView"]
        #[ivars = SwitchFlippedViewIvars]
        pub struct SwitchFlippedView;

        impl SwitchFlippedView {
        }

        unsafe impl NSObjectProtocol for SwitchFlippedView {}
    );

    impl SwitchFlippedView {
        pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(SwitchFlippedViewIvars);
            unsafe {
                msg_send![
                    super(this),
                    initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0))
                ]
            }
        }
    }

    pub struct SwitcherHandlerIvars {
        pub rows: RefCell<Vec<SwitchRow>>,
        pub commands: RefCell<Vec<(String, String)>>,
        pub rows_view: Retained<NSView>,
        pub search: Retained<NSTextField>,
        pub panel: Retained<crate::ui::popup::PopupPanel>,
        pub on_pick: RefCell<Option<Box<dyn FnMut(i64)>>>,
        pub on_command: RefCell<Option<Box<dyn FnMut(String)>>>,
        pub on_close: RefCell<Option<Box<dyn FnMut()>>>,
    }

    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSSwitcherHandler"]
        #[ivars = SwitcherHandlerIvars]
        pub struct SwitcherHandler;

        impl SwitcherHandler {
            #[unsafe(method(pick:))]
            fn pick(&self, sender: Option<&AnyObject>) {
                let tag = sender.map(|s| unsafe {
                    let tag: objc2_foundation::NSInteger =
                        msg_send![as_any(s), tag];
                    tag
                });
                let Some(tag) = tag else { return };
                if tag < 0 {
                    return;
                }
                let query = self.ivars().search.stringValue().to_string();
                if is_command_query(&query) {
                    let name = {
                        let cmds = self.ivars().commands.borrow();
                        cmds.get(tag as usize).map(|(n, _)| n.clone())
                    };
                    if let Some(name) = name {
                        self.ivars().panel.orderOut(None);
                        if let Some(cb) = self.ivars().on_command.borrow_mut().as_mut() {
                            cb(name);
                        }
                    }
                } else {
                    self.ivars().panel.orderOut(None);
                    if let Some(cb) = self.ivars().on_pick.borrow_mut().as_mut() {
                        cb(tag as i64);
                    }
                }
            }
        }

        impl SwitcherHandler {
            #[unsafe(method(controlTextDidChange:))]
            fn control_text_did_change(&self, _obj: &NSNotification) {
                let query = self.ivars().search.stringValue().to_string();
                self.rebuild(&query);
            }
        }

        impl SwitcherHandler {
            /// AppKit sends `cancelOperation:` up the responder chain on Esc.
            #[unsafe(method(cancelOperation:))]
            fn cancel(&self, _sender: Option<&AnyObject>) {
                self.ivars().panel.orderOut(None);
                if let Some(cb) = self.ivars().on_close.borrow_mut().as_mut() {
                    cb();
                }
            }
        }

        unsafe impl NSObjectProtocol for SwitcherHandler {}
        unsafe impl NSControlTextEditingDelegate for SwitcherHandler {}
        unsafe impl NSTextFieldDelegate for SwitcherHandler {}
    );

    impl SwitcherHandler {
        #[allow(clippy::too_many_arguments)]
        pub fn new(
            mtm: MainThreadMarker,
            rows: Vec<SwitchRow>,
            rows_view: Retained<NSView>,
            search: Retained<NSTextField>,
            panel: Retained<crate::ui::popup::PopupPanel>,
            commands: Vec<(String, String)>,
            on_pick: Option<Box<dyn FnMut(i64)>>,
            on_command: Option<Box<dyn FnMut(String)>>,
            on_close: Option<Box<dyn FnMut()>>,
        ) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(SwitcherHandlerIvars {
                rows: RefCell::new(rows),
                commands: RefCell::new(commands),
                rows_view,
                search,
                panel,
                on_pick: RefCell::new(on_pick),
                on_command: RefCell::new(on_command),
                on_close: RefCell::new(on_close),
            });
            unsafe { msg_send![super(this), init] }
        }

        pub fn set_rows(&self, rows: Vec<SwitchRow>) {
            *self.ivars().rows.borrow_mut() = rows;
        }

        pub fn set_commands(&self, commands: Vec<(String, String)>) {
            *self.ivars().commands.borrow_mut() = commands;
        }

        pub fn set_on_pick(&self, on_pick: Option<Box<dyn FnMut(i64)>>) {
            *self.ivars().on_pick.borrow_mut() = on_pick;
        }

        pub fn set_on_command(&self, on_command: Option<Box<dyn FnMut(String)>>) {
            *self.ivars().on_command.borrow_mut() = on_command;
        }

        pub fn set_on_close(&self, on_close: Option<Box<dyn FnMut()>>) {
            *self.ivars().on_close.borrow_mut() = on_close;
        }

        /// The search field's current text.
        pub fn query(&self) -> String {
            self.ivars().search.stringValue().to_string()
        }

        /// `filter(_:)` + `draw` — replace the row buttons under the query. A
        /// `/`-prefixed query switches to the palette command rows.
        pub fn rebuild(&self, query: &str) {
            let rows_view = &self.ivars().rows_view;
            let subs = rows_view.subviews();
            for s in &subs {
                s.removeFromSuperview();
            }
            let mtm = match MainThreadMarker::new() {
                Some(m) => m,
                None => return,
            };
            let count = if is_command_query(query) {
                let cmds = self.ivars().commands.borrow();
                let indices = filter_commands(&cmds, query);
                let frames = row_frames(indices.len());
                for (slot, idx) in indices.iter().enumerate() {
                    let button = unsafe {
                        NSButton::buttonWithTitle_target_action(
                            &NSString::from_str(&cmds[*idx].1),
                            Some(as_any(self)),
                            Some(objc2::sel!(pick:)),
                            mtm,
                        )
                    };
                    button.setTag(*idx as objc2_foundation::NSInteger);
                    button.setFrame(NSRect::new(
                        NSPoint::new(frames[slot].x, frames[slot].y),
                        NSSize::new(frames[slot].width, frames[slot].height),
                    ));
                    button.setBordered(false);
                    rows_view.addSubview(&button);
                }
                indices.len()
            } else {
                let rows = self.ivars().rows.borrow();
                let panel = ViewSwitcherPanel {
                    rows: rows.clone(),
                    ..ViewSwitcherPanel::new()
                };
                let indices = panel.filter(query);
                let frames = row_frames(indices.len());
                for (slot, idx) in indices.iter().enumerate() {
                    let row = &rows[*idx];
                    let button = unsafe {
                        NSButton::buttonWithTitle_target_action(
                            &NSString::from_str(&row_summary(row)),
                            Some(as_any(self)),
                            Some(objc2::sel!(pick:)),
                            mtm,
                        )
                    };
                    button.setTag(row.nav_id as objc2_foundation::NSInteger);
                    button.setFrame(NSRect::new(
                        NSPoint::new(frames[slot].x, frames[slot].y),
                        NSSize::new(frames[slot].width, frames[slot].height),
                    ));
                    button.setBordered(false);
                    rows_view.addSubview(&button);
                }
                indices.len()
            };
            let (_, total) = panel_size(count);
            rows_view.setFrame(NSRect::new(
                NSPoint::new(0.0, SEARCH_HEIGHT),
                NSSize::new(PANEL_WIDTH, (total - SEARCH_HEIGHT).max(0.0)),
            ));
            self.ivars()
                .panel
                .setContentSize(NSSize::new(PANEL_WIDTH, total));
        }
    }

    thread_local! {
        static LIVE_HANDLERS: RefCell<Vec<Retained<SwitcherHandler>>>
            = const { RefCell::new(Vec::new()) };
    }

    pub fn retain_handler(h: &Retained<SwitcherHandler>) {
        LIVE_HANDLERS.with(|v| v.borrow_mut().push(h.clone()));
    }

    // -- the `show` command palette panel -----------------------------------

    /// The palette is wider than the switcher: two panes side by side.
    pub const PALETTE_WIDTH: f64 = 480.0;

    /// The palette's own row cap (the shared `MAX_ROWS` is the view
    /// switcher's).
    pub const PALETTE_MAX_ROWS: usize = 16;

    /// Shown while the filter is empty (was blank: an unlabeled box).
    pub const PALETTE_SEARCH_PLACEHOLDER: &str = "Search commands";

    pub struct PaletteHandlerIvars {
        pub model: RefCell<PaletteModel>,
        pub rows_view: Retained<crate::ui::list_panel::ListRows>,
        pub search: Retained<NSTextField>,
        pub panel: Retained<crate::ui::popup::PopupPanel>,
        pub on_action: RefCell<Option<Box<dyn FnMut(PaletteAction)>>>,
        pub on_close: RefCell<Option<Box<dyn FnMut()>>>,
    }

    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSPaletteHandler"]
        #[ivars = PaletteHandlerIvars]
        pub struct PaletteHandler;

        impl PaletteHandler {
            #[unsafe(method(pick:))]
            fn pick(&self, sender: Option<&AnyObject>) {
                let tag: objc2_foundation::NSInteger = sender
                    .map(|s| unsafe { msg_send![as_any(s), tag] })
                    .unwrap_or(-1);
                if tag < 0 {
                    return;
                }
                let action = {
                    let mut model = self.ivars().model.borrow_mut();
                    model.selection = tag as usize;
                    model.accept()
                };
                if let Some(action) = action {
                    self.ivars().panel.orderOut(None);
                    if let Some(cb) = self.ivars().on_action.borrow_mut().as_mut() {
                        cb(action);
                    }
                }
            }

            #[unsafe(method(controlTextDidChange:))]
            fn control_text_did_change(&self, _obj: &NSNotification) {
                let query = self.ivars().search.stringValue().to_string();
                self.ivars().model.borrow_mut().set_query(&query);
                self.rebuild();
            }

            #[unsafe(method(cancelOperation:))]
            fn cancel(&self, _sender: Option<&AnyObject>) {
                self.ivars().panel.orderOut(None);
                if let Some(cb) = self.ivars().on_close.borrow_mut().as_mut() {
                    cb();
                }
            }
        }

        unsafe impl NSObjectProtocol for PaletteHandler {}
        unsafe impl NSControlTextEditingDelegate for PaletteHandler {}
        unsafe impl NSTextFieldDelegate for PaletteHandler {}
    );

    impl PaletteHandler {
        pub fn new(
            mtm: MainThreadMarker,
            model: PaletteModel,
            rows_view: Retained<crate::ui::list_panel::ListRows>,
            search: Retained<NSTextField>,
            panel: Retained<crate::ui::popup::PopupPanel>,
            on_action: Option<Box<dyn FnMut(PaletteAction)>>,
            on_close: Option<Box<dyn FnMut()>>,
        ) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(PaletteHandlerIvars {
                model: RefCell::new(model),
                rows_view,
                search,
                panel,
                on_action: RefCell::new(on_action),
                on_close: RefCell::new(on_close),
            });
            unsafe { msg_send![super(this), init] }
        }

        pub fn query(&self) -> String {
            self.ivars().search.stringValue().to_string()
        }

        pub fn model(&self) -> std::cell::Ref<'_, PaletteModel> {
            self.ivars().model.borrow()
        }

        pub fn model_mut(&self) -> std::cell::RefMut<'_, PaletteModel> {
            self.ivars().model.borrow_mut()
        }

        /// Push the model's rows / selection into the drawn list and size the
        /// panel to them (top edge kept).
        pub fn rebuild(&self) {
            let (n, sel) = {
                let model = self.ivars().model.borrow();
                (model.rows().len(), model.selection)
            };
            self.ivars().rows_view.set_count(n, sel);
            let h = crate::ui::list_panel::panel_height(n, ROW_HEIGHT, PALETTE_MAX_ROWS);
            let panel = &self.ivars().panel;
            let f = panel.frame();
            let top = f.origin.y + f.size.height;
            panel.setFrame_display(
                NSRect::new(NSPoint::new(f.origin.x, top - h), NSSize::new(PALETTE_WIDTH, h)),
                true,
            );
        }

        /// A click: select the row, then accept.
        pub fn click(&self, row: usize) {
            let action = {
                let mut m = self.ivars().model.borrow_mut();
                m.selection = row;
                m.accept()
            };
            if let Some(action) = action {
                // The callback first: it hands focus back to the user's app
                // while the palette is still up (ordering out first left us
                // active with no window, and macOS raised the shared window).
                if let Some(cb) = self.ivars().on_action.borrow_mut().as_mut() {
                    cb(action);
                }
                self.ivars().panel.orderOut(None);
            }
        }

        /// One command row: a full-width pill with the label.
        pub fn draw_row(&self, rect: crate::ui::chrome::Rect, i: usize, selected: bool, hover: bool) {
            use crate::ui::chrome::Rect;
            use crate::ui::style::{self, Weight};
            let c = crate::ui::theme::PopupThemeDefaults::colors();
            let model = self.ivars().model.borrow();
            let rows = model.rows();
            let Some((_, label)) = rows.get(i) else { return };
            let cell = Rect::new(8.0, rect.mid_y() - 13.0, rect.width - 16.0, 26.0);
            style::row_pill(cell, 6.0, selected, selected, hover, &c, style::RowAccent::Rounded(5.0));
            let f = style::system_font(13.0, if selected { Weight::Semibold } else { Weight::Regular });
            style::draw_text_left(
                label,
                Rect::new(cell.x + 14.0, cell.y, cell.width - 22.0, cell.height),
                &f,
                if selected { c.text } else { c.text.with_alpha(0.88) },
            );
        }
    }

    thread_local! {
        static LIVE_PALETTES: RefCell<Vec<Retained<PaletteHandler>>>
            = const { RefCell::new(Vec::new()) };
    }

    pub fn retain_palette_handler(h: &Retained<PaletteHandler>) {
        LIVE_PALETTES.with(|v| v.borrow_mut().push(h.clone()));
    }

    pub fn make_search(mtm: MainThreadMarker) -> Retained<NSTextField> {
        let field = NSTextField::textFieldWithString(&NSString::from_str(""), mtm);
        field.setPlaceholderString(Some(&NSString::from_str(SEARCH_PLACEHOLDER)));
        field.setFont(Some(&NSFont::systemFontOfSize(13.0)));
        field.setBezeled(true);
        field.setEditable(true);
        field.setSelectable(true);
        field
    }

    /// The `show` command palette's floating panel (a `PaletteModel` renderer).
    pub struct PalettePanel {
        pub model: PaletteModel,
        pub shown: bool,
        pub on_action: Option<Box<dyn FnMut(PaletteAction)>>,
        pub on_close: Option<Box<dyn FnMut()>>,
        pub window: Option<Retained<crate::ui::popup::PopupPanel>>,
        pub handler: Option<Retained<PaletteHandler>>,
    }

    impl std::fmt::Debug for PalettePanel {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("PalettePanel")
                .field("shown", &self.shown)
                .field("query", &self.model.query)
                .field("selection", &self.model.selection)
                .finish()
        }
    }

    impl Default for PalettePanel {
        fn default() -> Self {
            PalettePanel {
                model: PaletteModel::default(),
                shown: false,
                on_action: None,
                on_close: None,
                window: None,
                handler: None,
            }
        }
    }

    impl PalettePanel {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn set_rows(&mut self, commands: Vec<(String, String)>) {
            self.model = PaletteModel::new(commands);
            if let Some(h) = &self.handler {
                // A fresh model means a fresh query — empty the field too, or
                // the stale text from the last open lingers over the
                // unfiltered rows until the next keystroke (the view switcher
                // clears on every show; parity).
                h.ivars().search.setStringValue(&NSString::from_str(""));
                *h.model_mut() = self.model.clone();
                h.rebuild();
            }
        }

        pub fn set_callbacks(
            &mut self,
            on_action: Box<dyn FnMut(PaletteAction)>,
            on_close: Box<dyn FnMut()>,
        ) {
            let mut action: Option<Box<dyn FnMut(PaletteAction)>> = Some(on_action);
            let mut close: Option<Box<dyn FnMut()>> = Some(on_close);
            if let Some(h) = &self.handler {
                *h.ivars().on_action.borrow_mut() = action.take();
                *h.ivars().on_close.borrow_mut() = close.take();
            }
            if let Some(a) = action {
                self.on_action = Some(a);
            }
            if let Some(c) = close {
                self.on_close = Some(c);
            }
        }

        /// Route one key through the model; returns the accept action when
        /// Return fires (the host executes it).
        pub fn key(
            &mut self,
            code: u16,
            ctrl: bool,
            shift: bool,
            cmd: bool,
            opt: bool,
        ) -> Option<PaletteAction> {
            let q = self.live_query();
            // The drawn list reads the handler's model (typing and clicks
            // update that one), so keys must drive it, not the stale copy.
            if let Some(h) = &self.handler {
                let action = {
                    let mut m = h.model_mut();
                    m.set_query(&q);
                    m.key(code, ctrl, shift, cmd, opt)
                };
                h.rebuild();
                return action;
            }
            self.model.set_query(&q);
            self.model.key(code, ctrl, shift, cmd, opt)
        }

        pub fn live_query(&self) -> String {
            self.handler.as_ref().map(|h| h.query()).unwrap_or_else(|| self.model.query.clone())
        }

        /// `SwitcherController.handleEscape`'s first branch: clear the query
        /// and stay up (the field is emptied and the rows reset).
        pub fn clear_query(&mut self) {
            if let Some(h) = &self.handler {
                h.ivars()
                    .search
                    .setStringValue(&NSString::from_str(""));
                let mut model = h.model_mut();
                model.set_query("");
                model.selection = 0;
                drop(model);
                h.rebuild();
            }
            self.model.set_query("");
            self.model.selection = 0;
        }

        pub fn hide_panel(&mut self) {
            self.shown = false;
            if let Some(w) = &self.window {
                // A click-pick runs `orderOut` inside the click's mouse-down;
                // the window server kept drawing the panel (AppKit already
                // said `isVisible == false`, so a second `orderOut` is a
                // no-op) — under the tool it opened, back when that closed.
                // Make it invisible and click-through as well.
                w.setAlphaValue(0.0);
                w.setIgnoresMouseEvents(true);
                w.orderOut(None);
            }
        }

        pub fn is_visible(&self) -> bool {
            self.window.as_ref().map(|w| w.isVisible()).unwrap_or(self.shown)
        }

        /// Whether the palette window currently has the keyboard.
        pub fn has_key(&self) -> bool {
            self.window.as_ref().map(|w| w.isKeyWindow()).unwrap_or(self.shown)
        }

        /// Build (once) and present over the host frame.
        pub fn present(&mut self, mtm: MainThreadMarker, host: Option<SwitchFrame>) {
            if self.window.is_none() {
                self.build_macos(mtm);
            }
            if let Some(h) = &self.handler {
                *h.model_mut() = self.model.clone();
                h.rebuild();
            }
            if let Some(panel) = &self.window {
                let count = self.model.rows().len();
                let (w, h) = (PALETTE_WIDTH, crate::ui::list_panel::panel_height(count, ROW_HEIGHT, PALETTE_MAX_ROWS));
                panel.setContentSize(NSSize::new(w, h));
                if let Some(host) = host {
                    let (x, y) = panel_origin(host, (w, h));
                    panel.setFrameOrigin(NSPoint::new(x, y));
                }
                panel.setAlphaValue(1.0);
                panel.setIgnoresMouseEvents(false);
                panel.makeKeyAndOrderFront(None);
                if let Some(handler) = &self.handler {
                    let responder: &objc2_app_kit::NSResponder = &handler.ivars().search;
                    panel.makeFirstResponder(Some(responder));
                }
            }
            self.shown = true;
        }

        fn build_macos(&mut self, mtm: MainThreadMarker) {
            use crate::ui::list_panel::ListPanel;
            use crate::ui::popup::PopupConfig;

            let colors = crate::ui::theme::PopupThemeDefaults::colors();
            let list = ListPanel::new(mtm, colors, PALETTE_WIDTH, ROW_HEIGHT, PALETTE_SEARCH_PLACEHOLDER);
            let h = list.height(self.model.rows().len(), PALETTE_MAX_ROWS);
            let config = PopupConfig {
                name: "palette".to_string(),
                frame: crate::ui::popup::CardFrame { x: 0.0, y: 0.0, width: PALETTE_WIDTH, height: h },
                tool_panel: true,
                floating: true,
                ..PopupConfig::default()
            };
            let panel = crate::ui::popup::PopupPanel::create(mtm, &config);
            panel.setOpaque(false);
            panel.setBackgroundColor(Some(&objc2_app_kit::NSColor::clearColor()));
            panel.setHasShadow(true);
            panel.setContentView(Some(&list.root));

            let handler = PaletteHandler::new(
                mtm,
                self.model.clone(),
                list.rows.clone(),
                list.search.clone(),
                panel.clone(),
                self.on_action.take(),
                self.on_close.take(),
            );
            retain_palette_handler(&handler);
            let weak = objc2::rc::Weak::from_retained(&handler);
            list.set_drawer(Box::new(move |r, i, sel, hover| {
                if let Some(h) = weak.load() {
                    h.draw_row(r, i, sel, hover);
                }
            }));
            let weak = objc2::rc::Weak::from_retained(&handler);
            list.set_on_click(Box::new(move |row, _x, _double| {
                if let Some(h) = weak.load() {
                    h.click(row);
                }
            }));
            let (w1, w2) = (objc2::rc::Weak::from_retained(&handler), objc2::rc::Weak::from_retained(&handler));
            list.set_on_field(
                Box::new(move |q: String| {
                    if let Some(h) = w1.load() {
                        h.ivars().model.borrow_mut().set_query(&q);
                        h.rebuild();
                    }
                }),
                Box::new(move |cmd| {
                    // Arrows / Return / Esc come through the host's key
                    // monitor first; this covers a click-focused field.
                    let Some(h) = w2.load() else { return false };
                    if cmd == "cancelOperation:" {
                        // Callback (focus hand-back) before ordering out.
                        if let Some(cb) = h.ivars().on_close.borrow_mut().as_mut() {
                            cb();
                        }
                        h.ivars().panel.orderOut(None);
                        return true;
                    }
                    false
                }),
            );
            handler.rebuild();

            self.window = Some(panel);
            self.handler = Some(handler);
            PALETTE_LISTS.with(|l| l.borrow_mut().push(list));
        }
    }

    thread_local! {
        /// The palettes' list pieces (the field delegate lives on them).
        static PALETTE_LISTS: RefCell<Vec<crate::ui::list_panel::ListPanel>> = const { RefCell::new(Vec::new()) };
    }
}

#[cfg(target_os = "macos")]
pub use macos::PalettePanel;

#[cfg(test)]
mod tests {
    use super::*;

    fn views() -> Vec<SwitchView> {
        vec![
            SwitchView::new(64, "Files", "~/src"),
            SwitchView::new(60, "Notes", "todo.md"),
            SwitchView::new(61, "Jira", "PROJ"),
        ]
    }

    #[test]
    fn numbers_first_nine_then_zero() {
        let all: Vec<SwitchView> = (0..11)
            .map(|i| SwitchView::new(100 + i, format!("v{i}"), ""))
            .collect();
        let mut p = ViewSwitcherPanel::new();
        p.show(&all, None, None);
        assert_eq!(p.rows[0].number, 1);
        assert_eq!(p.rows[8].number, 9);
        assert_eq!(p.rows[9].number, 0);
        assert_eq!(p.rows[10].number, 0);
    }

    #[test]
    fn show_marks_current_and_preselects() {
        let mut p = ViewSwitcherPanel::new();
        p.show(&views(), Some(60), Some(61));
        assert!(p.rows.iter().find(|r| r.nav_id == 60).unwrap().is_current);
        assert!(!p.rows.iter().find(|r| r.nav_id == 64).unwrap().is_current);
        assert_eq!(p.selection, 2, "preselect wins");
        assert!(p.shown);

        p.show(&views(), None, Some(999));
        assert_eq!(p.selection, 0, "unknown preselect falls back to 0");
    }

    #[test]
    fn filter_is_empty_returns_source_order() {
        let mut p = ViewSwitcherPanel::new();
        p.show(&views(), None, None);
        assert_eq!(p.filter("   "), vec![0, 1, 2]);
    }

    #[test]
    fn filter_matches_name_location_and_aliases() {
        let mut p = ViewSwitcherPanel::new();
        let mut v = views();
        v[1].aliases = vec!["markdown".into()];
        p.show(&v, None, None);
        assert_eq!(p.filter("jira"), vec![2]);
        assert_eq!(p.filter("todo"), vec![1], "location match");
        assert_eq!(p.filter("markdown"), vec![1], "alias match");
        assert!(p.filter("zzz").is_empty());
        assert_eq!(p.filter("files"), vec![0]);
    }

    #[test]
    fn digit_key_jumps_when_query_empty() {
        let mut p = ViewSwitcherPanel::new();
        p.show(&views(), None, None);
        assert_eq!(p.key(19, false, false, false), Some(60), "digit 2 -> Notes");
        assert_eq!(p.key(20, false, false, false), Some(61));
        assert_eq!(p.key(18, false, false, false), Some(64));
        assert_eq!(p.key(21, false, false, false), None, "no 4th row");
        assert_eq!(p.key(19, true, false, false), None, "modifier blocks");
        p.current_query = "note".into();
        assert_eq!(p.key(19, false, false, false), None, "query blocks");
    }

    #[test]
    fn panel_size_grows_and_clamps() {
        // A dynamic panel keeps at least one row.
        assert_eq!(panel_size(0), (PANEL_WIDTH, SEARCH_HEIGHT + 2.0 * PANEL_MARGIN + ROW_HEIGHT));
        assert_eq!(panel_size(3).1, SEARCH_HEIGHT + 2.0 * PANEL_MARGIN + 3.0 * ROW_HEIGHT);
        assert_eq!(panel_size(100).1, panel_size(MAX_ROWS).1);
    }

    #[test]
    fn panel_origin_centers_over_host() {
        let host = SwitchFrame::new(0.0, 0.0, 1000.0, 800.0);
        let (x, y) = panel_origin(host, (520.0, 200.0));
        assert_eq!(x, 500.0 - 260.0);
        assert_eq!(y, 400.0 - 100.0 + 96.0);
    }

    #[test]
    fn row_frames_stack() {
        let f = row_frames(3);
        assert_eq!(f.len(), 3);
        assert_eq!(f[0], Rect::new(0.0, 0.0, PANEL_WIDTH, ROW_HEIGHT));
        assert_eq!(f[2].y, 2.0 * ROW_HEIGHT);
    }

    #[test]
    fn row_summary_includes_markers() {
        let mut p = ViewSwitcherPanel::new();
        p.show(&views(), Some(60), None);
        let notes = p.rows.iter().find(|r| r.nav_id == 60).unwrap();
        assert!(row_summary(notes).contains("2   Notes"));
        assert!(row_summary(notes).contains("● here"));
        let files = p.rows.iter().find(|r| r.nav_id == 64).unwrap();
        assert!(!row_summary(files).contains("here"));
        assert!(row_summary(files).contains("~/src"));
    }

    #[test]
    fn command_mode_detects_slash_prefix() {
        assert!(is_command_query("/"));
        assert!(is_command_query("/notes"));
        assert!(is_command_query("  /notes"), "leading whitespace ignored");
        assert!(!is_command_query(""));
        assert!(!is_command_query("notes"));
        assert!(!is_command_query("a/b"), "slash must lead");

        let mut p = ViewSwitcherPanel::new();
        p.show(&views(), None, None);
        assert!(!p.command_mode(), "empty query is view mode");
        p.current_query = "/ji".into();
        assert!(p.command_mode());
        p.current_query = "files".into();
        assert!(!p.command_mode());
    }

    fn commands() -> Vec<(String, String)> {
        vec![
            ("notes".to_string(), "Open Notes".to_string()),
            ("files".to_string(), "File Browser".to_string()),
            ("jira".to_string(), "Jira Board".to_string()),
            ("window".to_string(), "Kitchen Sink".to_string()),
        ]
    }

    #[test]
    fn command_filter_ranks_title_and_name() {
        let cmds = commands();
        assert_eq!(filter_commands(&cmds, "/"), vec![0, 1, 2, 3], "empty lists all");
        assert_eq!(filter_commands(&cmds, "/ "), vec![0, 1, 2, 3]);
        assert_eq!(filter_commands(&cmds, "/jira"), vec![2]);
        assert_eq!(filter_commands(&cmds, "/open"), vec![0], "title match");
        assert_eq!(filter_commands(&cmds, "/file"), vec![1]);
        assert!(filter_commands(&cmds, "/zzz").is_empty());
        // Leading whitespace before the slash is tolerated.
        assert_eq!(filter_commands(&cmds, "  /jira"), vec![2]);
    }

    #[test]
    fn command_rows_replace_view_rows_in_test_state() {
        let mut p = ViewSwitcherPanel::new();
        p.show(&views(), Some(60), None);
        p.set_commands(commands());
        // View mode still reports view summaries.
        assert_eq!(
            p.display_rows(),
            vec!["Files|~/src", "Notes|todo.md", "Jira|PROJ"]
        );
        // `/` mode reports the command titles.
        p.current_query = "/ji".into();
        assert!(p.command_mode());
        assert_eq!(p.display_rows(), vec!["Jira Board"]);
        p.current_query = "/".into();
        assert_eq!(
            p.display_rows(),
            vec!["Open Notes", "File Browser", "Jira Board", "Kitchen Sink"]
        );
    }

    #[test]
    fn show_hide_visibility_and_test_state_shape() {
        let mut p = ViewSwitcherPanel::new();
        p.show(&views(), Some(60), None);
        assert!(p.shown);
        assert!(p.is_visible(), "falls back to shown when unbuilt");

        let state = p.test_state();
        assert_eq!(state["shown"], json!(true));
        assert_eq!(state["selection"], json!(0));
        assert_eq!(state["query"], json!(""));
        assert_eq!(
            state["rows"],
            json!(["Files|~/src", "Notes|todo.md", "Jira|PROJ"])
        );

        p.hide_panel();
        assert!(!p.shown);
        assert!(!p.is_visible());
        assert_eq!(p.test_state()["shown"], json!(false));

        // present() re-shows.
        p.present(None);
        assert!(p.shown);
        assert!(p.is_visible());
    }

    #[test]
    fn test_state_reflects_live_query_filtering() {
        let mut p = ViewSwitcherPanel::new();
        p.show(&views(), None, None);
        p.current_query = "jira".into();
        let state = p.test_state();
        assert_eq!(state["query"], json!("jira"));
        assert_eq!(state["rows"], json!(["Jira|PROJ"]));
    }

    // -- the command palette model ------------------------------------------

    fn palette() -> PaletteModel {
        PaletteModel::new(vec![
            ("notes".to_string(), "Notes".to_string()),
            ("jira".to_string(), "Jira Board".to_string()),
        ])
    }

    #[test]
    fn palette_is_one_list_of_commands() {
        let p = palette();
        assert_eq!(
            p.rows(),
            vec![
                ("notes".to_string(), "Notes".to_string()),
                ("jira".to_string(), "Jira Board".to_string()),
            ]
        );
    }

    #[test]
    fn palette_fuzzy_filters_with_or_without_a_slash() {
        let mut p = palette();
        p.set_query("ji");
        assert_eq!(p.rows().len(), 1);
        assert_eq!(p.rows()[0].0, "jira");
        p.set_query("/ji");
        assert_eq!(p.rows()[0].0, "jira");
    }

    #[test]
    fn palette_query_change_resets_selection() {
        let mut p = palette();
        p.selection = 1;
        p.set_query("no");
        assert_eq!(p.selection, 0);
    }

    #[test]
    fn palette_move_cursor_wraps() {
        let mut p = palette();
        p.move_cursor(1);
        assert_eq!(p.selection, 1);
        p.move_cursor(1);
        assert_eq!(p.selection, 0, "wraps");
        p.move_cursor(-1);
        assert_eq!(p.selection, 1, "backwards wraps");
    }

    #[test]
    fn palette_accept_runs_the_selected_command() {
        let mut p = palette();
        assert_eq!(p.accept(), Some(PaletteAction::Command("notes".into())));
        p.set_query("zzz");
        assert_eq!(p.accept(), None, "nothing matches, nothing runs");
    }

    #[test]
    fn palette_keys_move_and_accept() {
        let mut p = palette();
        assert_eq!(p.key(125, false, false, false, false), None); // down
        assert_eq!(p.selection, 1);
        assert_eq!(p.key(45, true, false, false, false), None); // ctrl+n
        assert_eq!(p.selection, 0);
        p.key(48, false, false, false, false); // tab moves down
        assert_eq!(p.selection, 1);
        p.key(48, false, true, false, false); // shift+tab moves up
        assert_eq!(p.selection, 0);
        assert_eq!(
            p.key(36, false, false, false, false),
            Some(PaletteAction::Command("notes".into()))
        );
    }

    // -- the switcher / palette snapshot contract ---------------------------

    fn herdr(id: &str, title: &str, status: &str) -> LinkRow {
        LinkRow {
            status: Some(status.into()),
            subtitle: "2 tabs".into(),
            ..LinkRow::new("herdr", "workspace", id, title)
        }
    }

    /// A shown switcher (3 views) with two herdr rows loaded.
    fn linked() -> ViewSwitcherPanel {
        let mut p = ViewSwitcherPanel::new();
        p.show(&views(), None, None);
        p.set_links(
            Ok(vec![herdr("wB", "kitchen-sink", "working"), herdr("wR", "dotfiles", "idle")]),
            links::LinkTexts::default(),
            true,
            links::LinkOpts { start_expanded: false, ..links::LinkOpts::default() },
        );
        p
    }

    #[test]
    fn link_group_pinned_on_top_with_the_cursor_on_it() {
        let p = linked();
        assert_eq!(p.entries(), vec![Entry::Header, Entry::Local(0), Entry::Local(1), Entry::Local(2)]);
        let st = p.test_state();
        assert_eq!(st["linkGroup"]["expanded"], json!(false));
        assert_eq!(st["linkGroup"]["header"], json!("Terminal spaces · 2"));
        assert_eq!(st["linkGroup"]["cursor"], json!("header"));
        assert_eq!(p.accept(), None, "Return on the header is not a view pick");
        // Rows still list the views only (the Swift shape).
        assert_eq!(st["rows"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn link_group_return_on_header_focuses_the_terminal() {
        let mut p = linked();
        assert_eq!(p.link_action(), Some(LinkPick::Focus), "collapsed: just Ghostty");
        assert!(p.group_open(true));
        assert_eq!(p.link_action(), Some(LinkPick::Focus), "expanded too");
        p.set_links(Err(links::LinkError::Offline), links::LinkTexts::default(), true, p.links.opts.clone());
        assert_eq!(p.link_action(), Some(LinkPick::Launch));
        p.set_links(Err(links::LinkError::Unavailable("x".into())), links::LinkTexts::default(), true, p.links.opts.clone());
        assert_eq!(p.link_action(), Some(LinkPick::Launch));
        assert_eq!(p.header_click(), LinkPick::Launch);
    }

    #[test]
    fn link_group_navigation_and_go() {
        let mut p = linked();
        assert!(p.group_open(true), "→ on the header expands");
        p.move_selection(1);
        assert_eq!(p.link_action(), Some(LinkPick::Go(herdr("wB", "kitchen-sink", "working"))));
        p.move_selection(1);
        p.move_selection(1);
        assert_eq!((p.link_cursor, p.selection), (None, 0), "↓ past the group reaches the views");
        assert_eq!(p.accept(), Some(64));
        p.move_selection(-1);
        p.move_selection(-1);
        p.move_selection(-1);
        assert_eq!(p.link_cursor, Some(Entry::Header));
        p.move_selection(-1);
        assert_eq!((p.link_cursor, p.selection), (None, 2), "↑ from the top wraps to the last view");
        assert!(!p.group_open(true), "→ on a view row is the field's");
        // ← on a link row closes onto the header.
        p.links_jump();
        p.move_selection(1);
        assert!(p.group_open(false));
        assert_eq!((p.link_cursor, p.links.expanded), (Some(Entry::Header), false));
        // Digits jump views only.
        assert_eq!(p.key(19, false, false, false), Some(60));
    }

    #[test]
    fn link_group_query_prefers_views_and_clearing_collapses() {
        let mut p = linked();
        p.current_query = "dotf".into();
        p.move_selection(0);
        assert_eq!(p.entries(), vec![Entry::Header, Entry::Link(1)], "only the group matches");
        assert_eq!(p.link_cursor, Some(Entry::Link(1)), "the match, not the header");
        assert_eq!(p.test_state()["linkGroup"]["expanded"], json!(true));
        p.current_query = "fil".into();
        p.move_selection(0);
        assert_eq!((p.link_cursor, p.selection), (None, 0), "a view match wins the cursor");
        p.current_query.clear();
        p.move_selection(0);
        assert!(!p.links.expanded);
        assert_eq!(p.link_cursor, Some(Entry::Header));
        p.current_query = "/".into();
        assert!(p.entries().is_empty());
        assert_eq!(p.link_action(), None);
    }

    #[test]
    fn link_group_opens_expanded_and_its_name_keeps_it() {
        let mut p = linked();
        p.set_links(
            Ok(vec![herdr("wB", "kitchen-sink", "working"), herdr("wR", "dotfiles", "idle")]),
            links::LinkTexts::default(),
            true,
            links::LinkOpts::default(),
        );
        assert!(p.links.expanded, "start-expanded applies at once with no query");
        p.show(&views(), None, None);
        assert_eq!(p.entries()[..3], [Entry::Header, Entry::Link(0), Entry::Link(1)]);
        assert_eq!(p.link_cursor, Some(Entry::Header));
        p.current_query = "terminal".into();
        p.move_selection(0);
        assert_eq!(p.entries(), vec![Entry::Header, Entry::Link(0), Entry::Link(1)]);
        assert_eq!(p.link_cursor, Some(Entry::Header), "Return raises the terminal");
        p.current_query = "herdr dot".into();
        p.move_selection(0);
        assert_eq!(p.link_cursor, Some(Entry::Link(1)));
        p.current_query.clear();
        p.move_selection(0);
        assert!(p.links.expanded, "clearing the query keeps it open");
    }

    #[test]
    fn link_group_reopen_and_disable() {
        let mut p = linked();
        p.links.expanded = true;
        p.move_selection(1);
        p.show(&views(), None, None);
        assert!(!p.links.expanded);
        assert_eq!(p.link_cursor, Some(Entry::Header));
        p.set_links(
            Ok(vec![]),
            links::LinkTexts::default(),
            true,
            links::LinkOpts { start_on_group: false, start_expanded: false, ..links::LinkOpts::default() },
        );
        p.show(&views(), None, None);
        assert_eq!(p.link_cursor, None, "start-on-group = false starts on the views");
        p.set_links(Ok(vec![]), links::LinkTexts::default(), false, links::LinkOpts::default());
        assert_eq!(p.entries().len(), 3);
        assert!(!p.links_jump());
    }

    #[test]
    fn switcher_snapshot_and_events() {
        let mut p = ViewSwitcherPanel::new();
        p.show(&views(), Some(60), None);
        let before = p.snapshot();
        assert!(before.shown && !before.command_mode && before.rows.len() == 3);

        p.handle(SwitcherEvent::Move(1));
        let moved = p.snapshot();
        assert_ne!(moved, before, "Move changes the selection");
        assert_eq!(moved.selection, 1);

        p.handle(SwitcherEvent::Click { row: 2 });
        assert_eq!(p.snapshot().selection, 2, "a click selects the visible row");

        p.handle(SwitcherEvent::Query("jira".into()));
        assert_eq!(p.snapshot().rows.len(), 1, "typing filters");

        p.set_commands(commands());
        p.handle(SwitcherEvent::Query("/ji".into()));
        assert_eq!(
            p.snapshot().rows,
            vec![SwitcherRow::Command { name: "jira".into(), title: "Jira Board".into() }],
            "`/` mode draws command rows"
        );

        p.handle(SwitcherEvent::Escape);
        assert!(!p.snapshot().shown);
    }

    #[test]
    fn palette_snapshot_and_events() {
        let mut p = palette();
        let before = p.snapshot();
        assert_eq!(before.rows.len(), 2);
        assert!(before.status.is_none());

        p.handle(SwitcherEvent::Move(1));
        assert_eq!(p.snapshot().selection, 1, "Move changes the cursor");

        p.handle(SwitcherEvent::Status(SwitcherStatus { cpu: Some(12), ..Default::default() }));
        assert_eq!(p.snapshot().status.and_then(|s| s.cpu), Some(12));

        assert_eq!(
            p.snapshot().rows[0],
            SwitcherRow::Command { name: "notes".into(), title: "Notes".into() }
        );

        p.handle(SwitcherEvent::Query("/ji".into()));
        assert!(p.snapshot().command_mode);

        p.shown = true;
        p.handle(SwitcherEvent::Escape);
        assert_eq!(p.snapshot().query, "", "a live query clears first");
        p.handle(SwitcherEvent::Escape);
        assert!(!p.snapshot().shown, "Esc closes an empty-query palette");
    }
}
