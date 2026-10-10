//! Port of `ViewSwitcher.swift` — the `ViewSwitcherPanel` that lists the
//! header views (Ctrl+B W), previous view pre-selected, 1-9 / Return to jump.
//!
//! The list model, fuzzy filter and the digit-jump keys are complete and
//! tested; [`ViewSwitcherPanel::build`] constructs the floating `PopupPanel`
//! with its search field and row buttons on macOS.

use crate::engines::list_filter::FuzzyIndex;
use crate::ui::chrome::Rect;
use serde_json::{json, Value};

#[cfg(target_os = "macos")]
use objc2::rc::Retained;
#[cfg(target_os = "macos")]
use objc2_app_kit::NSView;
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
pub const SEARCH_PLACEHOLDER: &str = "switch view — type to filter · ↩ open · 1–9 jump";

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

#[derive(Default)]
pub struct ViewSwitcherPanel {
    pub rows: Vec<SwitchRow>,
    pub current_query: String,
    pub selection: usize,
    pub shown: bool,
    /// Palette commands for `/` command mode as `(name, title)` pairs.
    pub commands: Vec<(String, String)>,
    pub on_pick: Option<Box<dyn FnMut(i64)>>,
    /// Called with the command name when a command row is picked (`/` mode).
    pub on_command: Option<Box<dyn FnMut(String)>>,
    /// Called (alongside hiding) when Esc cancels the panel.
    pub on_close: Option<Box<dyn FnMut()>>,
    #[cfg(target_os = "macos")]
    pub window: Option<Retained<PopupPanel>>,
    #[cfg(target_os = "macos")]
    pub rows_view: Option<Retained<NSView>>,
    #[cfg(target_os = "macos")]
    handler: Option<Retained<macos::SwitcherHandler>>,
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
        ViewSwitcherPanel::default()
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
        self.selection = self
            .rows
            .iter()
            .position(|r| preselect == Some(r.nav_id))
            .unwrap_or(0);
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
        if cmd || ctrl || opt || !self.current_query.is_empty() {
            return None;
        }
        let n = digit_for(code)?;
        self.rows
            .iter()
            .find(|r| r.number == n)
            .map(|r| r.nav_id)
    }

    /// Hide the panel: `orderOut` the window (if built) and clear `shown`.
    pub fn hide_panel(&mut self) {
        self.shown = false;
        #[cfg(target_os = "macos")]
        if let Some(w) = &self.window {
            w.orderOut(None);
        }
    }

    /// Whether the panel window is currently ordered front (`NSWindow.isVisible`);
    /// falls back to the `shown` flag when the AppKit window is not built.
    pub fn is_visible(&self) -> bool {
        #[cfg(target_os = "macos")]
        if let Some(w) = &self.window {
            return w.isVisible();
        }
        self.shown
    }

    /// The live query: the AppKit search field's text when built, else
    /// [`Self::current_query`].
    pub fn live_query(&self) -> String {
        #[cfg(target_os = "macos")]
        if let Some(h) = &self.handler {
            return h.query();
        }
        self.current_query.clone()
    }

    /// True when the current search text starts with `/` (palette command mode).
    pub fn command_mode(&self) -> bool {
        is_command_query(&self.live_query())
    }

    /// Store the palette commands (`(name, title)` pairs) used in `/` mode and
    /// refresh the AppKit rows when the panel is already built.
    pub fn set_commands(&mut self, commands: Vec<(String, String)>) {
        self.commands = commands;
        #[cfg(target_os = "macos")]
        if let Some(h) = &self.handler {
            h.set_commands(self.commands.clone());
            let q = h.query();
            h.rebuild(&q);
        }
    }

    /// Wire the pick and command callbacks. Moves them into the live handler
    /// when the panel is already built, otherwise stores them for [`Self::build`].
    pub fn set_callbacks(
        &mut self,
        on_pick: Box<dyn FnMut(i64)>,
        on_command: Box<dyn FnMut(String)>,
    ) {
        let mut pick: Option<Box<dyn FnMut(i64)>> = Some(on_pick);
        let mut command: Option<Box<dyn FnMut(String)>> = Some(on_command);
        #[cfg(target_os = "macos")]
        if let Some(h) = &self.handler {
            h.set_on_pick(pick.take());
            h.set_on_command(command.take());
        }
        if let Some(p) = pick {
            self.on_pick = Some(p);
        }
        if let Some(c) = command {
            self.on_command = Some(c);
        }
    }

    /// Wire the Esc/close callback (moved into the live handler when built).
    pub fn set_on_close(&mut self, on_close: Box<dyn FnMut()>) {
        let mut close: Option<Box<dyn FnMut()>> = Some(on_close);
        #[cfg(target_os = "macos")]
        if let Some(h) = &self.handler {
            h.set_on_close(close.take());
        }
        if let Some(c) = close {
            self.on_close = Some(c);
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
        json!({
            "shown": self.shown,
            "selection": self.selection,
            "rows": self.display_rows(),
            "query": self.live_query(),
        })
    }

    /// Build the AppKit panel, its search field and the row buttons.
    pub fn build(&mut self, mtm: objc2::MainThreadMarker) {
        #[cfg(target_os = "macos")]
        self.build_macos(mtm);
        #[cfg(not(target_os = "macos"))]
        let _ = mtm;
    }

    /// `show` over a host window: rebuild the rows and place the panel.
    pub fn present(&mut self, host: Option<SwitchFrame>) {
        self.shown = true;
        #[cfg(target_os = "macos")]
        self.present_macos(host);
        #[cfg(not(target_os = "macos"))]
        let _ = host;
    }

    #[cfg(target_os = "macos")]
    fn build_macos(&mut self, mtm: objc2::MainThreadMarker) {
        if self.window.is_some() {
            return;
        }
        use crate::ui::popup::{CardFrame, PopupConfig};

        let (w, h) = panel_size(self.rows.len());
        let config = PopupConfig {
            name: "view-switcher".to_string(),
            frame: CardFrame { x: 0.0, y: 0.0, width: w, height: h },
            tool_panel: true,
            floating: true,
            ..PopupConfig::default()
        };
        let panel = PopupPanel::create(mtm, &config);

        let root = macos::SwitchFlippedView::new(mtm);
        let search = macos::make_search(mtm);
        search.setFrame(objc2_foundation::NSRect::new(
            objc2_foundation::NSPoint::new(PANEL_MARGIN, PANEL_MARGIN),
            objc2_foundation::NSSize::new(w - PANEL_MARGIN * 2.0, SEARCH_HEIGHT - PANEL_MARGIN),
        ));
        search.setAutoresizingMask(objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable);
        let rows_view = macos::SwitchFlippedView::new(mtm);
        rows_view.setAutoresizingMask(objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable);
        root.addSubview(&search);
        root.addSubview(&rows_view);
        panel.setContentView(Some(&root));

        let handler = macos::SwitcherHandler::new(
            mtm,
            self.rows.clone(),
            rows_view.clone().into_super(),
            search.clone(),
            panel.clone(),
            self.commands.clone(),
            self.on_pick.take(),
            self.on_command.take(),
            self.on_close.take(),
        );
        unsafe { search.setDelegate(Some(objc2::runtime::ProtocolObject::from_ref(&*handler))) };
        macos::retain_handler(&handler);
        handler.rebuild(&self.current_query);

        self.window = Some(panel);
        self.rows_view = Some(rows_view.into_super());
        self.handler = Some(handler);
    }

    #[cfg(target_os = "macos")]
    fn present_macos(&mut self, host: Option<SwitchFrame>) {
        let mtm = match objc2::MainThreadMarker::new() {
            Some(m) => m,
            None => return,
        };
        if self.window.is_none() {
            self.build_macos(mtm);
        }
        if let Some(handler) = &self.handler {
            handler.set_rows(self.rows.clone());
        }
        if let Some(panel) = &self.window {
            let count = self.filter(&self.current_query).len();
            let (w, h) = panel_size(count);
            panel.setContentSize(objc2_foundation::NSSize::new(w, h));
            if let Some(host) = host {
                let (x, y) = panel_origin(host, (w, h));
                panel.setFrameOrigin(objc2_foundation::NSPoint::new(x, y));
            }
            panel.makeKeyAndOrderFront(None);
        }
        if let Some(handler) = &self.handler {
            handler.rebuild(&self.current_query);
        }
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObject};
    use objc2::{
        define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly, Message,
    };
    use objc2_app_kit::{
        NSButton, NSFont, NSTextField, NSTextFieldDelegate, NSView, NSControlTextEditingDelegate,
    };
    use objc2_foundation::{
        NSNotification, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
    };
    use std::cell::RefCell;

    fn as_any<T: Message + ?Sized>(obj: &T) -> &AnyObject {
        unsafe { &*(obj as *const T as *const AnyObject) }
    }

    pub struct SwitchFlippedViewIvars;

    define_class!(
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSSwitchFlippedView"]
        #[ivars = SwitchFlippedViewIvars]
        pub struct SwitchFlippedView;

        impl SwitchFlippedView {
            #[unsafe(method(isFlipped))]
            fn is_flipped(&self) -> bool {
                true
            }
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

    pub fn make_search(mtm: MainThreadMarker) -> Retained<NSTextField> {
        let field = NSTextField::textFieldWithString(&NSString::from_str(""), mtm);
        field.setPlaceholderString(Some(&NSString::from_str(SEARCH_PLACEHOLDER)));
        field.setFont(Some(&NSFont::systemFontOfSize(13.0)));
        field.setBezeled(true);
        field.setEditable(true);
        field.setSelectable(true);
        field
    }
}

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
}
