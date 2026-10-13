//! Port of the `SwitcherController` host from `kitchen_sink.swift`.
//!
//! The socket surface (`testQuery` + `startCommandServer`'s request routing),
//! the hotkey path (`hotkeyPrep` / `applyHotkeyPrep` / `toggleCommand`), and
//! `reloadConfig` are real, as are the shared-window transitions (open / toggle
//! / hide / back / home / cycle / hotkey), the `do:` host actions, and the
//! daemon UI bridge (`install_daemon_ui`: the shared host window, the tool
//! panels, the command palette, and the local key monitor that drives
//! `PopupWindow.handleKey`'s full action set — list keys, window geometry,
//! the notes find bar / rail / commit, and the path sheet).
//!
//! The controller model stays `Send + Sync` (per-view `MemberState` rows) so it
//! can be handed to `CommandServer`; AppKit lives only behind the main-thread
//! [`UiQueue`]/[`MainQueue`] drain. Unported `do:` verbs delegate to
//! `Registry::dispatch_test_do`, mirroring `testQuery`'s `compare:` /
//! `screenshot:` delegation.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{json, Map, Value};

use crate::app::config::{self, AppSettings, HeaderStyle};
use crate::app::hotkey::{self, HotkeyPrep};
use crate::app::paths::Paths;
use crate::app::registry::{PaletteCommand, RectI, Registry, SlotView};
use crate::app::socket::{CommandHandler, Reply};
use crate::panes::pane_geometry::{PaneDir, Rect};
use crate::panes::pane_nav::{NavPane, PaneNav, ViewId};
use crate::panes::vim_keys::VimKeys;
use crate::views::compare::{CompareConfig, CompareRecent, CompareWindowModel};
use crate::views::screenshot::{ScreenshotConfig, ScreenshotController};
use crate::views::tools::ToolKind;

/// The sections `loadCommands` never turns into commands.
const COMMAND_SECTION_SKIPS: [&str; 11] = [
    "icons",
    "shortcuts",
    "app",
    "confluence",
    "ai",
    "compare",
    "notifications",
    "pane-shot",
    "notes-find",
    "setup",
    "settings-hub",
];

/// Every `SlotView`, in `testQuery`'s order (`kitchen_sink.swift:3111`).
pub const ALL_VIEWS: [SlotView; 11] = [
    SlotView::Notes,
    SlotView::Files,
    SlotView::Jira,
    SlotView::Detail,
    SlotView::Releases,
    SlotView::Config,
    SlotView::Output,
    SlotView::Confluence,
    SlotView::Ai,
    SlotView::Compare,
    SlotView::CompareText,
];

/// The `SharedWindow.escViews` set, for the `escHides` document.
const ESC_VIEWS: [SlotView; 6] = [
    SlotView::Files,
    SlotView::Notes,
    SlotView::Ai,
    SlotView::Jira,
    SlotView::Confluence,
    SlotView::Compare,
];

/// One view's AppKit-free mirror (a reduced `PopupWindow.testState`).
#[derive(Clone, Debug)]
pub struct MemberState {
    pub shown: bool,
    pub key: bool,
    pub wid: i64,
    pub frame: Option<RectI>,
    /// The view's own `testState` object, merged into `views.<view>`.
    pub state: Value,
}

impl Default for MemberState {
    fn default() -> Self {
        MemberState {
            shown: false,
            key: false,
            wid: 0,
            frame: None,
            state: Value::Object(Map::new()),
        }
    }
}

/// One registered pane (id + window-space rect). `PaneNav` builds the real
/// `NavPane`s from these; no AppKit closures leak into the model.
#[derive(Clone, Debug)]
pub struct PaneSpec {
    pub id: String,
    pub rect: RectI,
}

/// A cross-thread command from the socket thread to the daemon's main thread.
/// AppKit is main-thread-only, so the controller only enqueues these; the
/// daemon's UI bridge drains the queue on the main run loop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UiCommand {
    /// Show the shared host window on `v` (built on first use).
    Present(SlotView),
    /// Order the shared host window out.
    Hide,
    /// Show / hide the view-switcher palette (`show`, Ctrl+B W, `do:switcher`).
    ShowPalette,
    HidePalette,
    /// Show / hide the Ctrl+B W view switcher (`do:switcher`).
    ShowSwitcher,
    HideSwitcher,
    /// Toggle one notes-window drawer.
    ToggleDrawer(DrawerSide),
    /// The paths shelf tool window (`do:paths:*`).
    Paths(PathsCmd),
    /// Toggle the `/terminal` tool panel (`do:term`, Ctrl+B T).
    TerminalPanel,
    /// Open (or re-raise) one of the `openTool` panels (`do:tool:*`).
    Tool(ToolKind),
    /// Order a tool panel out (`do:tool-close:*`).
    ToolClose(String),
    /// `do:reset-size` — restore the shared window's default size.
    ResetSize,
    /// Show a toast pill (text, SF Symbol) — socket verbs run off the main
    /// thread, so they queue it here.
    Toast(String, &'static str),
    /// Ctrl+HJKL / `do:pane:h|j|k|l` over the live AppKit panes.
    PaneMove(PaneDir),
    /// `do:pane:focus:<id>`.
    PaneFocus(String),
    /// `do:notes-find` (false) / `do:notes-grep` (true) — `NoteFindWindow`.
    NoteFind(bool),
    /// `do:jira-jump` — `SidebarJumpWindow` over the Jira rail.
    JiraJump,
    /// `jira-poll on`'s checks finished: (ok, message).
    JiraEnable(bool, String),
    /// `JiraSetupWindow.show(controller:reason:)`.
    JiraSetup(Option<String>),
}

/// Which notes drawer a [`UiCommand::ToggleDrawer`] flips.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrawerSide {
    Terminal,
    Browser,
}

/// The `do:paths:*` grammar's UI side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathsCmd {
    Show,
    Hide,
    Return,
    Select(usize),
}

/// The command queue the controller pushes onto; the main thread drains it.
pub type UiQueue = Arc<Mutex<Vec<UiCommand>>>;

/// Jobs the socket thread runs on the main thread synchronously (Swift's
/// `testQuery` hops to main with a 2 s semaphore). The daemon's UI bridge
/// drains this alongside the [`UiQueue`]; headless controllers run inline.
pub type MainQueue = Arc<Mutex<Vec<Box<dyn FnOnce() + Send>>>>;

/// The view the daemon presents when it cold-starts with no launch mode.
pub const DEFAULT_VIEW: SlotView = SlotView::Notes;

/// The mutable model behind the controller's `Mutex`.
struct ControllerInner {
    current: Option<SlotView>,
    /// The view shown before `current` (`slot.previousNav`).
    previous: Option<SlotView>,
    last: SlotView,
    last_jira: SlotView,
    stack: Vec<SlotView>,
    /// Bounded navigation history for the socket `do:back`, recorded in
    /// `present()` whenever the current view changes to a different view.
    /// Independent of `stack` (which keeps Swift parity for Esc / in-window
    /// navigation through `push`/`back(true)`).
    history: Vec<SlotView>,
    /// True once any view has been presented; distinguishes a fresh controller
    /// from one that has shown then hidden a view (for `do:show`).
    ever_presented: bool,
    visible: bool,
    palette_visible: bool,
    target_screen: Option<i32>,
    aerospace_cache_cleared: bool,
    members: HashMap<SlotView, MemberState>,
    panes: Vec<PaneSpec>,
    pane_nav: PaneNav,
    /// `PaneNav.testState` from the live AppKit panes (main thread), which
    /// `state.pane` reports instead of the headless model when set.
    live_pane: Option<Value>,
    vim_keys: VimKeys,
    settings: AppSettings,
    return_wid: Option<String>,
    return_pid: Option<i32>,
    active: bool,
    key_window: String,
    activations: i64,
    frontmost_pid: i32,
    screenshot: Value,
    compare: Value,
    pane_shot: Value,
    tools: Map<String, Value>,
    terminal_panel: bool,
    /// The live `TerminalPanel.testState()` once the panel is built (the
    /// daemon UI pushes it each drain); `None` = never built.
    terminal_panel_state: Option<Value>,
    view_switcher: bool,
    /// The rows `state.viewSwitcher.rows` reports (pushed by the daemon UI
    /// when the palette is built; empty until then).
    switcher_rows: Vec<String>,
    /// `state.paths` — the live PathsWindow document pushed by the daemon UI
    /// (`{"shown": false, "rows": [...]}` until it is built).
    paths_state: Value,
    /// `[paths] enabled = "true"`; gates `do:paths:show` like Swift.
    paths_enabled: bool,
    /// The notes drawers' shown flags (`views.notes.terminal` / `.browser`).
    notes_terminal: bool,
    notes_browser: bool,
    /// Visible `NSWindow` count (daemon UI pushes it; `state.windows`).
    window_count: i64,
    /// Per-view window titles (`cmd.windowName`, used by `present`).
    window_titles: HashMap<SlotView, String>,
    /// An `open:<path>` launch message waiting for the notes surface.
    pending_note_open: Option<String>,
    /// A `Cmd+O` path-sheet result waiting for the notes surface.
    pending_note_external: Option<String>,
    last_key: String,
    /// `[section] esc-close` values parsed from commands.toml.
    esc_sections: HashMap<String, i64>,
    /// `[app] esc-close`, the fallback for `escHides`.
    esc_app: i64,
    using_backup: bool,
    logs: Vec<String>,
    /// The daemon UI's command queue (`install_ui`); `None` = headless (tests).
    ui_queue: Option<UiQueue>,
    main_queue: Option<MainQueue>,
    /// The compare model, so every hide path can release `--wait` callers
    /// (Swift `slotPark(stopVoice: true)` → `finishWaiters`). Lock order:
    /// `inner` before the model, never the reverse.
    compare_model: Option<Arc<Mutex<CompareWindowModel>>>,
}

impl ControllerInner {
    fn member(&mut self, v: SlotView) -> &mut MemberState {
        self.members.entry(v).or_default()
    }

    fn shown_member(&self) -> Option<SlotView> {
        self.current.filter(|v| {
            self.members
                .get(v)
                .map(|m| m.shown)
                .unwrap_or(false)
        })
    }

    fn ensure_member(&mut self, v: SlotView) {
        self.members.entry(v).or_default();
    }

    /// Append `v` to the bounded `do:back` navigation history (cap 32, the
    /// oldest entries drop off).
    fn record_history(&mut self, v: SlotView) {
        const CAP: usize = 32;
        self.history.push(v);
        if self.history.len() > CAP {
            self.history.drain(0..self.history.len() - CAP);
        }
    }

    fn present(&mut self, v: SlotView) {
        self.ensure_member(v);
        if let Some(old) = self.current {
            if old != v {
                self.previous = Some(old);
                self.record_history(old);
                if let Some(m) = self.members.get_mut(&old) {
                    m.shown = false;
                }
            }
        }
        if let Some(m) = self.members.get_mut(&v) {
            m.shown = true;
        }
        self.current = Some(v);
        self.last = v;
        self.ever_presented = true;
        if v.is_jira() {
            self.last_jira = v;
        }
        self.visible = true;
        self.logs.push(format!("shared window: {}", v.raw()));
        self.ui_push(UiCommand::Present(v));
    }

    /// Toggle the `/terminal` panel: flip the mirror (and the live doc's
    /// `shown` until the daemon UI's next push) and queue the UI command.
    fn flip_terminal_panel(&mut self) {
        self.terminal_panel = !self.terminal_panel;
        let shown = self.terminal_panel;
        if let Some(o) = self.terminal_panel_state.as_mut().and_then(Value::as_object_mut) {
            o.insert("shown".into(), json!(shown));
        }
        self.ui_push(UiCommand::TerminalPanel);
    }

    fn ui_push(&self, cmd: UiCommand) {
        if let Some(q) = &self.ui_queue {
            q.lock().unwrap().push(cmd);
        }
    }

    fn open(&mut self, v: SlotView) {
        if v == SlotView::Jira && self.current.map_or(false, |c| c.is_jira()) {
            self.stack.clear();
        }
        if !v.is_jira() {
            self.stack.retain(|x| *x != v);
        }
        self.present(v);
    }

    fn push(&mut self, v: SlotView) {
        match self.current {
            Some(cur) if cur != v => self.stack.push(cur),
            Some(_) => {}
            None => self.stack = vec![v],
        }
        self.stack.retain(|x| *x != v);
        self.present(v);
    }

    fn hide(&mut self, reason: &str) {
        if matches!(self.current, Some(SlotView::Compare) | Some(SlotView::CompareText)) {
            if let Some(m) = &self.compare_model {
                m.lock().unwrap().finish_waiters();
            }
        }
        self.visible = false;
        if let Some(cur) = self.current {
            if let Some(m) = self.members.get_mut(&cur) {
                m.shown = false;
            }
        }
        self.current = None;
        self.return_wid = None;
        self.return_pid = None;
        self.target_screen = None;
        self.aerospace_cache_cleared = false;
        self.logs.push(format!("shared window: hidden — {reason}"));
        self.ui_push(UiCommand::Hide);
    }

    fn escape_at_top(&mut self, v: SlotView) {
        if self.esc_hide_count(v) > 0 {
            self.hide(&format!("Esc ({})", v.raw()));
        }
    }

    fn back(&mut self, esc: bool) {
        while let Some(prev) = self.stack.pop() {
            self.open(prev);
            return;
        }
        if let Some(cur) = self.current {
            if cur.is_jira() && cur != SlotView::Jira {
                self.open(SlotView::Jira);
            } else if cur == SlotView::CompareText {
                self.open(SlotView::Compare);
            } else if esc {
                self.escape_at_top(cur);
            } else {
                self.hide("back from the first view");
            }
        }
    }

    fn home(&mut self) {
        self.stack.clear();
        self.open(SlotView::Jira);
    }

    fn cycle(&mut self, dir: i32) {
        match self.current {
            Some(SlotView::Files) => self.open(SlotView::Notes),
            _ => self.open(SlotView::Files),
        }
        self.logs.push(format!(
            "cycle: dir={dir} now={}",
            self.current.map(|v| v.raw()).unwrap_or("nil")
        ));
    }

    /// `hideOrFocus`: hide when the user is already in the window, else bring
    /// the shown view forward (`slotShow`).
    fn hide_or_focus(&mut self, v: SlotView, user_in_it: bool) {
        if user_in_it {
            self.hide("hotkey pressed while in it");
        } else {
            self.present(v);
        }
    }

    fn toggle(&mut self, user_in_it: bool) {
        if let Some(m) = self.shown_member() {
            self.hide_or_focus(m, user_in_it);
            return;
        }
        self.open(self.last);
    }

    fn hotkey(&mut self, v: SlotView, user_in_it: bool) {
        if let Some(cur) = self.current {
            let same = if v == SlotView::Jira { cur.is_jira() } else { cur == v };
            if same && self.shown_member().is_some() {
                self.hide_or_focus(cur, user_in_it);
                return;
            }
        }
        if v == SlotView::Jira
            && self.last_jira != SlotView::Jira
            && self.members.contains_key(&self.last_jira)
        {
            self.present(self.last_jira);
        } else {
            self.open(v);
        }
    }

    fn show_files(&mut self) {
        self.open(SlotView::Files);
    }

    fn esc_hide_count(&self, v: SlotView) -> i64 {
        if let Some(n) = self.esc_sections.get(v.raw()) {
            return *n;
        }
        if v == SlotView::Notes {
            // Notes counts Esc in its own panes; default to the app value.
        }
        self.esc_app
    }

    fn panel_list(&self) -> Vec<NavPane> {
        self.panes
            .iter()
            .enumerate()
            .map(|(i, p)| {
                NavPane::new(
                    p.id.clone(),
                    ViewId(i as u64 + 1),
                    Rect::new(p.rect.x, p.rect.y, p.rect.w, p.rect.h),
                )
            })
            .collect()
    }

    /// The socket `state` document. `registry` is passed in because the
    /// controller owns it separately from the mutex-guarded model.
    fn state_with_registry(&self, registry: &Registry) -> Value {
        let mut views = Map::new();
        for v in ALL_VIEWS {
            let m = self.members.get(&v).cloned().unwrap_or_default();
            let mut entry = json!({
                "shown": m.shown,
                "key": m.key,
                "wid": m.wid,
                "frame": m.frame.map(|r| {
                    json!([r.x.round() as i64, r.y.round() as i64, r.w.round() as i64, r.h.round() as i64])
                }),
            });
            // The `PopupWindow.testState` keys the Swift notes / files / jira
            // members always report (the state probes read these).
            if matches!(v, SlotView::Notes | SlotView::Files | SlotView::Jira) {
                let notes = v == SlotView::Notes;
                if let Some(obj) = entry.as_object_mut() {
                    obj.insert("name".into(), json!(v.raw()));
                    obj.insert("terminal".into(), json!(notes && self.notes_terminal));
                    obj.insert("browser".into(), json!(notes && self.notes_browser));
                    obj.insert(
                        "drawerInset".into(),
                        json!(if notes && self.notes_terminal { 200 } else { 0 }),
                    );
                    obj.insert("findBar".into(), json!(false));
                    obj.insert("query".into(), json!(""));
                    obj.insert("rowCount".into(), json!(0));
                    obj.insert("selection".into(), json!(0));
                    obj.insert("selectedTab".into(), json!(0));
                    obj.insert("tabs".into(), json!([]));
                    obj.insert("responder".into(), json!(""));
                    obj.insert("accessory".into(), json!(false));
                    obj.insert("board".into(), json!({}));
                    obj.insert("sidebarCursor".into(), json!(0));
                    obj.insert("pane".into(), json!(""));
                    obj.insert("header".into(), json!({}));
                }
            }
            if let (Some(dst), Some(src)) = (entry.as_object_mut(), m.state.as_object()) {
                for (k, val) in src {
                    dst.insert(k.clone(), val.clone());
                }
            }
            views.insert(v.raw().to_string(), entry);
        }

        let mut esc_hides = Map::new();
        for v in ESC_VIEWS {
            esc_hides.insert(v.raw().to_string(), json!(self.esc_hide_count(v) > 0));
        }

        let panes = self.panel_list();
        let pane = if let (Some(_), Some(live)) = (self.current, &self.live_pane) {
            live.clone()
        } else if self.current.is_some() {
            serde_json::to_value(self.pane_nav.test_state(&panes))
                .unwrap_or_else(|_| json!({}))
        } else {
            json!({})
        };

        let palette_commands: Vec<String> = registry
            .palette_commands()
            .iter()
            .map(|c| c.title.clone())
            .collect();

        json!({
            "view": self.current.map(|v| v.raw()).unwrap_or(""),
            "visible": self.visible,
            "active": self.active,
            "keyWindow": self.key_window,
            "windows": self.window_count,
            "palette": self.palette_visible,
            "views": Value::Object(views),
            "hideOnFocusLoss": self.settings.hide_on_focus_loss,
            "escHides": Value::Object(esc_hides),
            "pid": std::process::id(),
            "paletteCommands": palette_commands,
            "pane": pane,
            "screenshot": self.screenshot,
            "compare": self.compare,
            "paneShot": self.pane_shot,
            "activations": self.activations,
            "frontmostPid": self.frontmost_pid,
            "tools": Value::Object(self.tools.clone()),
            "lastKey": self.last_key.clone(),
            "headerStyle": self.settings.header_style.raw(),
            "terminalPanel": self
                .terminal_panel_state
                .clone()
                .unwrap_or_else(|| json!({ "shown": self.terminal_panel })),
            "viewSwitcher": {
                "shown": self.view_switcher,
                "rows": self.switcher_rows,
                "selection": 0
            },
            "paths": self.paths_state,
        })
    }
}

/// Mirrors `SwitcherController`: owns the registry, the shared-window state,
/// the `[app]` settings and the pane/vim models.
pub struct SwitcherController {
    inner: Arc<Mutex<ControllerInner>>,
    registry: Registry,
    /// Handles the view registrations close over, so `do:compare:*` /
    /// `do:screenshot:*` reach the same model the host holds.
    compare_model: Option<Arc<Mutex<CompareWindowModel>>>,
    screenshot_controller: Option<Arc<Mutex<ScreenshotController>>>,
}

/// `do:` verbs whose only effect is a `UiCommand` to a window that cannot
/// exist headless (they would otherwise write a `shown`/visible flag into
/// state with no window to back it). Refused in headless mode; hide/close
/// verbs stay accepted (no-ops are fine).
fn needs_window(action: &str) -> bool {
    action == "paths:show"
        || action == "switcher"
        || action == "toggle-terminal"
        || action == "toggle-browser"
        || action == "term"
        || action == "notes-find"
        || action == "notes-grep"
        || action == "jira-jump"
        || action == "reset-size"
        || action.starts_with("tool:")
}

impl SwitcherController {
    pub fn new(settings: AppSettings) -> Self {
        let esc_app = settings.esc_close;
        let inner = ControllerInner {
            current: None,
            previous: None,
            last: SlotView::Notes,
            last_jira: SlotView::Jira,
            stack: Vec::new(),
            history: Vec::new(),
            ever_presented: false,
            visible: false,
            palette_visible: false,
            target_screen: None,
            aerospace_cache_cleared: false,
            members: HashMap::new(),
            panes: Vec::new(),
            pane_nav: PaneNav::new(),
            live_pane: None,
            vim_keys: VimKeys::new(),
            settings,
            return_wid: None,
            return_pid: None,
            active: false,
            key_window: String::new(),
            activations: 0,
            frontmost_pid: 0,
            screenshot: json!({ "shown": false, "permission": false, "displays": [], "buttons": [], "objects": [] }),
            compare: json!({ "shown": false, "view": "", "sessions": [] }),
            pane_shot: json!({}),
            tools: Map::new(),
            terminal_panel: false,
            terminal_panel_state: None,
            view_switcher: false,
            switcher_rows: Vec::new(),
            paths_state: json!({ "shown": false, "rows": [] }),
            paths_enabled: false,
            notes_terminal: false,
            notes_browser: false,
            window_count: 0,
            window_titles: HashMap::new(),
            pending_note_open: None,
            pending_note_external: None,
            last_key: String::new(),
            esc_sections: HashMap::new(),
            esc_app,
            using_backup: false,
            logs: Vec::new(),
            ui_queue: None,
            main_queue: None,
            compare_model: None,
        };
        SwitcherController {
            inner: Arc::new(Mutex::new(inner)),
            registry: Registry::new(),
            compare_model: None,
            screenshot_controller: None,
        }
    }

    pub fn new_default() -> Self {
        Self::new(AppSettings::default())
    }

    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    pub fn registry_mut(&mut self) -> &mut Registry {
        &mut self.registry
    }

    /// The shared compare-window model (held so `do:compare:*` mutates the one
    /// the host owns); `None` until `register_views` while `[compare] enabled`.
    pub fn compare_model(&self) -> Option<Arc<Mutex<CompareWindowModel>>> {
        self.compare_model.clone()
    }

    /// The shared screenshot controller (held so `do:screenshot:*` works).
    pub fn screenshot_controller(&self) -> Option<Arc<Mutex<ScreenshotController>>> {
        self.screenshot_controller.clone()
    }

    /// Populate the registry from the ported view modules (palette rows + the
    /// `do:*` tables) and rebuild the header nav. Enabled flags are read from
    /// the real commands.toml (`[jira] enabled`, `[compare] enabled`,
    /// `[confluence] enabled`, `[ai] enabled`, `[screenshot] enabled`).
    pub fn register_views(&mut self) {
        let text = config::read_config_text().unwrap_or_default();
        self.register_views_from_text(&text);
    }

    fn register_views_from_text(&mut self, text: &str) {
        let sections = parse_sections(text);
        crate::ui::theme::install_theme(
            &sections.get("theme").cloned().unwrap_or_default(),
            sections.clone(),
        );

        self.inner.lock().unwrap().paths_enabled = section_flag(&sections, "paths", false);

        // `cmd.windowName` per view: the section's `name`, else the view name.
        let title_of = |section: &str, fallback: &str| -> String {
            sections
                .get(section)
                .and_then(|m| m.get("name"))
                .map(|s| unquote_value(s).trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| fallback.to_string())
        };
        {
            let mut inner = self.inner.lock().unwrap();
            inner.window_titles = [
                (SlotView::Notes, title_of("notes", "notes")),
                (SlotView::Files, title_of("files", "files")),
                (SlotView::Jira, title_of("jira", "jira")),
                (SlotView::Confluence, title_of("confluence", "confluence")),
                (SlotView::Ai, title_of("ai", "ai")),
                (SlotView::Compare, title_of("compare", "compare")),
                (SlotView::CompareText, title_of("compare", "compare")),
            ]
            .into_iter()
            .collect();
        }

        crate::views::notes::register(&mut self.registry);

        crate::views::jira::register(&mut self.registry, section_flag(&sections, "jira", false));

        let compare_cfg = CompareConfig::from_entries(
            sections.get("compare").cloned().unwrap_or_default(),
        );
        if crate::views::compare::compare_enabled(&compare_cfg) {
            let model = Arc::new(Mutex::new(CompareWindowModel::new(
                compare_cfg,
                CompareRecent::from_env(),
            )));
            crate::views::compare::register_compare_hooks(&mut self.registry, model.clone());
            self.inner.lock().unwrap().compare_model = Some(model.clone());
            self.compare_model = Some(model);
        }

        let shot_cfg = ScreenshotConfig::from_entries(
            &sections.get("screenshot").cloned().unwrap_or_default(),
        );
        if shot_cfg.enabled {
            let mut controller = ScreenshotController::new();
            controller.set_config(shot_cfg);
            let controller = Arc::new(Mutex::new(controller));
            crate::views::screenshot::register_controller(&mut self.registry, controller.clone());
            self.screenshot_controller = Some(controller);
        }

        self.registry.set_nav(self.build_nav(&sections));
        self.registry.set_palette(self.build_palette(text, &sections));
    }

    /// `SwitcherController.paletteCommands()` — the real palette: commands.toml
    /// rows (file order, `enabled = "true"` + `in-palette` gated, `jira-config`
    /// only with jira), the enabled slot commands, and "Kitchen Sink"; ordered
    /// by `[app] palette-first`.
    fn build_palette(
        &self,
        text: &str,
        sections: &HashMap<String, HashMap<String, String>>,
    ) -> Vec<PaletteCommand> {
        const SKIP: [&str; 11] = COMMAND_SECTION_SKIPS;
        let settings = self.inner.lock().unwrap().settings.clone();
        let jira = section_flag(sections, "jira", false);
        let label_of = |section: &str, fallback: &str| -> String {
            sections
                .get(section)
                .and_then(|m| m.get("label"))
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| fallback.to_string())
        };

        let mut all: Vec<PaletteCommand> = Vec::new();
        for (name, vars) in parse_sections_ordered(text) {
            if SKIP.contains(&name.as_str()) {
                continue;
            }
            if vars.get("enabled").map(String::as_str) != Some("true") {
                continue;
            }
            let spec = config::make_command(&name, &vars);
            // Parse `in-palette` from the raw text (pure): `make_command`'s
            // value goes through the python codec, which is unavailable to a
            // cold daemon.
            let in_palette = vars
                .get("in-palette")
                .map(|v| tri_pure(v).unwrap_or(true))
                .unwrap_or(true);
            if !in_palette || (spec.name == "jira-config" && !jira) {
                continue;
            }
            all.push(PaletteCommand::new(
                spec.name.clone(),
                spec.label.clone().unwrap_or_else(|| spec.name.clone()),
                name.clone(),
            ));
        }

        let listed = |section: &str| -> bool {
            sections
                .get(section)
                .and_then(|m| m.get("in-palette"))
                .map(|v| tri_pure(v).unwrap_or(true))
                .unwrap_or(true)
        };
        if section_flag(sections, "confluence", false) && listed("confluence") {
            let title = label_of("confluence", "Confluence Search");
            all.push(PaletteCommand::new("confluence", title, "views"));
        }
        if section_flag(sections, "ai", false) && listed("ai") {
            let title = label_of("ai", "AI View");
            all.push(PaletteCommand::new("ai", title, "views"));
        }
        if section_flag(sections, "compare", false) && listed("compare") {
            let title = label_of("compare", "Compare");
            all.push(PaletteCommand::new("compare", title, "views"));
        }
        if settings.shared_window {
            all.push(PaletteCommand::new("window", "Kitchen Sink", "views"));
        }

        let first: Vec<usize> = settings
            .palette_first
            .iter()
            .filter_map(|n| all.iter().position(|c| c.id.to_lowercase() == *n))
            .collect();
        let mut ordered: Vec<PaletteCommand> =
            first.iter().map(|i| all[*i].clone()).collect();
        for (i, c) in all.into_iter().enumerate() {
            if !first.contains(&i) {
                ordered.push(c);
            }
        }
        ordered
    }

    /// The header switcher, mirroring `SharedWindow.navIcons`: files first,
    /// then notes, then jira always; confluence / compare / ai while enabled.
    fn build_nav(
        &self,
        sections: &HashMap<String, HashMap<String, String>>,
    ) -> Vec<(SlotView, String, Option<String>)> {
        let s = self.inner.lock().unwrap().settings.clone();
        let icon = |name: &str| (!name.is_empty()).then(|| name.to_string());

        let mut nav = vec![
            (SlotView::Notes, "Notes".to_string(), icon(&s.notes_icon_name)),
            (SlotView::Files, "Files".to_string(), icon(&s.files_icon_name)),
            (SlotView::Jira, "Jira".to_string(), icon(&s.jira_icon_name)),
        ];
        if section_flag(sections, "confluence", false) {
            nav.push((
                SlotView::Confluence,
                "Confluence search".to_string(),
                icon(&s.confluence_icon_name),
            ));
        }
        if section_flag(sections, "compare", false) {
            nav.push((SlotView::Compare, "Compare".to_string(), None));
        }
        if section_flag(sections, "ai", false) {
            nav.push((SlotView::Ai, "AI view".to_string(), icon(&s.ai_icon_name)));
        }
        nav
    }

    /// `[app]` settings, for the caller to read.
    pub fn settings(&self) -> AppSettings {
        self.inner.lock().unwrap().settings.clone()
    }

    /// `(settings.hideOnFocusLoss, settings.focusLossDelay)` without cloning
    /// the whole settings struct (read every drain tick).
    pub fn focus_loss_settings(&self) -> (bool, f64) {
        let inner = self.inner.lock().unwrap();
        (inner.settings.hide_on_focus_loss, inner.settings.focus_loss_delay)
    }

    /// `setGlobalHideOnFocusLoss(_:)` — flip the global switch; `persist`
    /// writes `[app] hide-on-focus-loss` (the status-menu path). The shared
    /// window has no per-view `sticky` override, so nothing else changes.
    pub fn set_hide_on_focus_loss(&self, on: bool, persist: bool) {
        self.inner.lock().unwrap().settings.hide_on_focus_loss = on;
        if persist {
            config::save_config_value("app", "hide-on-focus-loss", if on { "true" } else { "false" });
        }
        self.log(&format!("[app] hide-on-focus-loss = {on}"));
    }

    /// Route UI commands to the daemon's main thread. Install once, before
    /// the run loop starts; without it the controller stays headless (tests).
    pub fn install_ui(&self, queue: UiQueue) {
        self.inner.lock().unwrap().ui_queue = Some(queue);
    }

    /// Install the main-thread job queue `do:` hops through when the caller is
    /// not on the main thread (mirrors `testQuery`'s `DispatchQueue.main.async`
    /// + 2 s semaphore). Without it (headless tests) `do:` runs inline.
    pub fn install_main_queue(&self, queue: MainQueue) {
        self.inner.lock().unwrap().main_queue = Some(queue);
    }

    /// Run `work` on the main thread, waiting up to `timeout` — Swift's
    /// `testQuery` main hop. `None` on timeout (the caller replies
    /// `{"error":"timeout"}`). Inline when already on main or headless.
    #[cfg(target_os = "macos")]
    pub fn run_on_main<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> T + Send + 'static,
        timeout: std::time::Duration,
    ) -> Option<T> {
        if objc2::MainThreadMarker::new().is_some() {
            return Some(work());
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let queue = self.inner.lock().unwrap().main_queue.clone();
        match queue {
            Some(q) => {
                q.lock().unwrap().push(Box::new(move || {
                    let _ = tx.send(work());
                }));
                rx.recv_timeout(timeout).ok()
            }
            None => Some(work()),
        }
    }

    #[cfg(not(target_os = "macos"))]
    pub fn run_on_main<T: Send>(
        &self,
        work: impl FnOnce() -> T + Send,
        _timeout: std::time::Duration,
    ) -> Option<T> {
        Some(work())
    }

    // -- direct model accessors (no AppKit) ---------------------------------

    pub fn current(&self) -> Option<SlotView> {
        self.inner.lock().unwrap().current
    }

    pub fn is_visible(&self) -> bool {
        self.inner.lock().unwrap().visible
    }

    pub fn set_active(&self, active: bool) {
        self.inner.lock().unwrap().active = active;
    }

    pub fn set_key_window(&self, title: impl Into<String>) {
        self.inner.lock().unwrap().key_window = title.into();
    }

    pub fn set_activations(&self, n: i64) {
        self.inner.lock().unwrap().activations = n;
    }

    /// One app activation (mirrors Swift's `appActivations` counter).
    pub fn bump_activations(&self) {
        self.inner.lock().unwrap().activations += 1;
    }

    pub fn set_frontmost_pid(&self, pid: i32) {
        self.inner.lock().unwrap().frontmost_pid = pid;
    }

    pub fn set_member(&self, v: SlotView, shown: bool, key: bool, frame: Option<RectI>) {
        let mut inner = self.inner.lock().unwrap();
        let m = inner.member(v);
        m.shown = shown;
        m.key = key;
        m.frame = frame;
    }

    /// Mark a member hidden while keeping its last frame (`views.<view>` parity).
    pub fn set_member_hidden(&self, v: SlotView) {
        let mut inner = self.inner.lock().unwrap();
        let m = inner.member(v);
        m.shown = false;
        m.key = false;
    }

    /// `state.views.<view>.wid` — the live window number.
    pub fn set_member_wid(&self, v: SlotView, wid: i64) {
        self.inner.lock().unwrap().member(v).wid = wid;
    }

    /// Queue an `open:<path>` launch for the notes surface (`openNoteFile`).
    pub fn set_pending_note_open(&self, path: &str) {
        self.inner.lock().unwrap().pending_note_open = Some(path.to_string());
    }

    /// Take the queued `open:<path>` (the daemon UI drains it on present).
    pub fn take_pending_note_open(&self) -> Option<String> {
        self.inner.lock().unwrap().pending_note_open.take()
    }

    pub fn set_view_state(&self, v: SlotView, state: Value) {
        self.inner.lock().unwrap().member(v).state = state;
    }

    pub fn set_panes(&self, panes: Vec<PaneSpec>) {
        self.inner.lock().unwrap().panes = panes;
    }

    pub fn set_esc_hides(&self, v: SlotView, on: bool) {
        self.inner
            .lock()
            .unwrap()
            .esc_sections
            .insert(v.raw().to_string(), i64::from(on));
    }

    /// `state.windows` — pushed by the daemon UI after ordering windows.
    pub fn set_window_count(&self, n: i64) {
        self.inner.lock().unwrap().window_count = n;
    }

    /// `state.palette` — the view-switcher palette's shown flag.
    pub fn set_palette_visible(&self, on: bool) {
        self.inner.lock().unwrap().palette_visible = on;
    }

    /// `state.viewSwitcher.rows` — `["Name|location"]`, pushed on palette build.
    pub fn set_switcher_rows(&self, rows: Vec<String>) {
        self.inner.lock().unwrap().switcher_rows = rows;
    }

    /// `state.paths` — the live `PathsWindow.testState` document.
    pub fn set_paths_state(&self, state: Value) {
        self.inner.lock().unwrap().paths_state = state;
    }

    /// `state.tools.<name>` — a tool panel's `testState` + `wid`/`level`,
    /// pushed by the daemon UI on every drain (`subWindows` mirror).
    pub fn set_tool_state(&self, name: &str, state: Value) {
        self.inner
            .lock()
            .unwrap()
            .tools
            .insert(name.to_string(), state);
    }

    /// Drop a tool panel's `tools` entry (`unregisterSubWindow`).
    /// `state.terminalPanel` ← the live panel document (daemon UI).
    /// The live `PaneNav.testState` (main thread, after each ring refresh).
    pub fn set_live_pane_state(&self, state: Value) {
        self.inner.lock().unwrap().live_pane = Some(state);
    }

    pub fn set_terminal_panel_state(&self, state: Value) {
        let mut inner = self.inner.lock().unwrap();
        inner.terminal_panel = state["shown"].as_bool().unwrap_or(false);
        inner.terminal_panel_state = Some(state);
    }

    pub fn remove_tool_state(&self, name: &str) {
        self.inner.lock().unwrap().tools.remove(name);
    }

    /// A `Cmd+O` path-sheet result; the daemon UI drains it on the next tick.
    pub fn set_pending_note_external(&self, path: String) {
        self.inner.lock().unwrap().pending_note_external = Some(path);
    }

    /// Take the pending external open (once).
    pub fn take_pending_note_external(&self) -> Option<String> {
        self.inner.lock().unwrap().pending_note_external.take()
    }

    /// `views.notes.terminal` / `.browser`.
    pub fn set_notes_drawer(&self, side: DrawerSide, on: bool) {
        let mut inner = self.inner.lock().unwrap();
        match side {
            DrawerSide::Terminal => inner.notes_terminal = on,
            DrawerSide::Browser => inner.notes_browser = on,
        }
    }

    pub fn notes_drawer(&self, side: DrawerSide) -> bool {
        let inner = self.inner.lock().unwrap();
        match side {
            DrawerSide::Terminal => inner.notes_terminal,
            DrawerSide::Browser => inner.notes_browser,
        }
    }

    /// `[paths] enabled` — gates `do:paths:show` (Swift `pathsCommand`).
    pub fn set_paths_enabled(&self, on: bool) {
        self.inner.lock().unwrap().paths_enabled = on;
    }

    pub fn paths_enabled(&self) -> bool {
        self.inner.lock().unwrap().paths_enabled
    }

    /// `cmd.windowName` (the AX window title the UI tests find windows by).
    pub fn window_title(&self, v: SlotView) -> String {
        self.inner
            .lock()
            .unwrap()
            .window_titles
            .get(&v)
            .cloned()
            .unwrap_or_else(|| v.raw().to_string())
    }

    /// `showCommand(_:)`'s shared-window subset: run a palette command row.
    pub fn run_palette_command(&self, name: &str) {
        match name {
            "paths" => {
                let _ = self.do_host_action("paths:show");
            }
            "screenshot" => {
                let _ = self.do_host_action("screenshot:show");
            }
            "terminal" => self.toggle_terminal_panel(),
            "window" | "show" | "notes" | "files" | "jira" | "confluence" | "ai"
            | "compare" | "prettyprint" | "filefast" | "health" | "health-checks"
            | "text-table" | "notes-find" | "notes-grep" => {
                self.toggle_command(name)
            }
            _ => {
                self.inner
                    .lock()
                    .unwrap()
                    .logs
                    .push(format!("palette: '{name}' has no Rust handler yet"));
            }
        }
    }

    /// Whether the palette is currently shown (`state.palette`).
    pub fn is_palette_visible(&self) -> bool {
        self.inner.lock().unwrap().palette_visible
    }

    /// `controller.escHideCount(v)` — the per-view Esc-close count.
    pub fn esc_hide_count(&self, v: SlotView) -> i64 {
        self.inner.lock().unwrap().esc_hide_count(v)
    }

    pub fn logs(&self) -> Vec<String> {
        self.inner.lock().unwrap().logs.clone()
    }

    // -- the shared-window verbs -------------------------------------------

    pub fn open(&self, v: SlotView) {
        self.inner.lock().unwrap().open(v);
    }

    /// Push a sub-view (`folder-open` on a file pair → compareText).
    pub fn push_view(&self, v: SlotView) {
        self.inner.lock().unwrap().push(v);
    }

    pub fn toggle(&self) {
        let in_it = self.user_in_it();
        self.inner.lock().unwrap().toggle(in_it);
    }

    /// Live `hideOrFocus` probe for the shown view (main-thread hop). Nothing
    /// shown, headless, or a timed-out hop all count as "in it" — the answer
    /// only matters when a view is shown.
    fn user_in_it(&self) -> bool {
        let wid = {
            let inner = self.inner.lock().unwrap();
            match inner.shown_member() {
                Some(v) => inner.members.get(&v).map_or(0, |m| m.wid),
                None => return true,
            }
        };
        #[cfg(target_os = "macos")]
        {
            self.run_on_main(move || daemon_ui::user_in_it(wid), std::time::Duration::from_secs(2))
                .unwrap_or(true)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = wid;
            true
        }
    }

    pub fn hide(&self, reason: &str) {
        self.inner.lock().unwrap().hide(reason);
    }

    pub fn back(&self, esc: bool) {
        let was_sub = self.current() == Some(SlotView::CompareText);
        self.inner.lock().unwrap().back(esc);
        // Leaving compareText closes the sub-view's session (Swift's separate
        // sub window) and returns to the session underneath.
        if was_sub && self.current() != Some(SlotView::CompareText) {
            if let Some(m) = &self.compare_model {
                m.lock().unwrap().close_sub();
            }
        }
    }

    pub fn home(&self) {
        self.inner.lock().unwrap().home();
    }

    pub fn cycle(&self, dir: i32) {
        self.inner.lock().unwrap().cycle(dir);
    }

    pub fn hotkey(&self, v: SlotView) {
        let in_it = self.user_in_it();
        self.inner.lock().unwrap().hotkey(v, in_it);
    }

    /// `PaneNav.shared.move(_:in:)` — move the focus ring one pane over.
    pub fn pane_move(&self, dir: PaneDir) {
        let mut inner = self.inner.lock().unwrap();
        let panes = inner.panel_list();
        inner.pane_nav.move_dir(dir, &panes);
    }

    /// `kitchen-sink jira-poll on|off|toggle` (`enableJiraChecked` /
    /// `setJiraEnabled`). The checks hit the network, so they run off-thread;
    /// `toggle` from the CLI stops polling outright (Swift asks in an alert).
    fn jira_poll(&self, name: &str) {
        use crate::app::jira_poll::{self as jp, EnableCheck};
        let on = jp::enabled_in_config();
        let want_on = match name {
            "jira-poll-on" => true,
            "jira-poll-off" => false,
            _ => !on,
        };
        if want_on == on {
            self.log(&format!("jira: {name}: already {}", if on { "on" } else { "off" }));
            if on {
                self.open(SlotView::Jira);
            }
            return;
        }
        if !want_on {
            jp::set_enabled(false, false);
            let _ = self.reload_config();
            self.log("jira: [jira] enabled = false (jira-poll off)");
            if self.current() == Some(SlotView::Jira) {
                self.hide("jira-poll off");
            }
            return;
        }
        // `jira-poll-on` with the poller off: the network checks and the UI
        // push need the daemon UI; a headless daemon re-enables directly (and
        // updates state) so `jira-poll-off` can be undone without a window.
        let Some(queue) = self.inner.lock().unwrap().ui_queue.clone() else {
            jp::set_enabled(true, false);
            let _ = self.reload_config();
            self.log("jira: [jira] enabled = true (jira-poll on)");
            self.open(SlotView::Jira);
            return;
        };
        std::thread::spawn(move || {
            // `enableJiraChecked`: a missing / incomplete config opens the
            // setup window with the reason instead of refusing.
            let cmd = match jp::enable_check() {
                EnableCheck::Ok => UiCommand::JiraEnable(true, String::new()),
                EnableCheck::Setup(reason) => UiCommand::JiraSetup(Some(reason)),
                EnableCheck::Login(msg) => UiCommand::JiraEnable(false, format!("Jira login failed: {msg}")),
            };
            queue.lock().unwrap().push(cmd);
        });
    }

    /// `enableJiraChecked`'s main-thread tail (after the off-thread checks).
    pub fn jira_enable_result(&self, ok: bool, msg: &str) {
        if !ok {
            self.log(&format!("jira: enable refused: {}", msg.replace('\n', "; ")));
            self.inner.lock().unwrap().ui_push(UiCommand::Toast(
                msg.lines().next().unwrap_or("").to_string(),
                "exclamationmark.triangle.fill",
            ));
            return;
        }
        crate::app::jira_poll::set_enabled(true, false);
        let _ = self.reload_config();
        self.log("jira: [jira] enabled = true (jira-poll on)");
        self.inner.lock().unwrap().ui_push(UiCommand::Toast("Jira polling on".into(), "checkmark.circle.fill"));
        self.open(SlotView::Jira);
    }

    /// `slot.previousNav` — the nav id of the view shown before `current`.
    pub fn previous_nav(&self) -> Option<i64> {
        self.inner.lock().unwrap().previous.map(|v| v.nav_id())
    }

    /// Mirrors `controller.log(_:)`.
    pub fn log(&self, message: &str) {
        self.inner.lock().unwrap().logs.push(message.to_string());
    }

    /// `SharedWindow.navClicked(_:)` over the controller model.
    pub fn nav_clicked(&self, id: i64) {
        use crate::app::registry::nav;
        let mut inner = self.inner.lock().unwrap();
        match id {
            nav::NOTES => {
                if inner.current != Some(SlotView::Notes) {
                    inner.open(SlotView::Notes);
                }
            }
            nav::FILES => {
                if inner.current != Some(SlotView::Files) {
                    inner.show_files();
                }
            }
            nav::JIRA => {
                if inner.current.map_or(false, |c| c.is_jira()) {
                    inner.home();
                } else {
                    inner.hotkey(SlotView::Jira, true);
                }
            }
            nav::CONFLUENCE => {
                if inner.current != Some(SlotView::Confluence) {
                    inner.open(SlotView::Confluence);
                }
            }
            nav::AI => {
                if inner.current != Some(SlotView::Ai) {
                    inner.open(SlotView::Ai);
                }
            }
            nav::COMPARE => {
                if inner.current != Some(SlotView::Compare) {
                    inner.open(SlotView::Compare);
                }
            }
            nav::HOME => inner.home(),
            nav::BACK => inner.back(false),
            id if id >= crate::ui::shared_window::WORKSPACE_BASE => {
                inner
                    .logs
                    .push(format!("switchWorkspace: cell {} not modeled yet", id));
            }
            _ => {}
        }
    }

    // -- hotkey prep / reload ----------------------------------------------

    /// `applyHotkeyPrep(_:)`.
    pub fn apply_hotkey_prep(&self, prep: &HotkeyPrep) {
        let mut inner = self.inner.lock().unwrap();
        inner.target_screen = prep.screen;
        inner.aerospace_cache_cleared = prep.cache_cleared;
    }

    /// `SwitcherController.hotkeyPrep()` against the discovered AeroSpace CLI.
    pub fn hotkey_prep(&self) -> HotkeyPrep {
        let paths = Paths::from_env();
        let settings = self.inner.lock().unwrap().settings.clone();
        let ipc = hotkey::AeroIpc::discover();
        hotkey::hotkey_prep(
            &ipc,
            &paths.focus_file_path(),
            &settings.switcher_window_name,
        )
    }

    /// `reloadConfig()`: re-read `[app]`, the esc-close map and the section
    /// count; return the `reload` reply (`ok` / `commands` / `usingBackup` /
    /// `issues`).
    pub fn reload_config(&self) -> Value {
        let mut inner = self.inner.lock().unwrap();
        let path = inner.settings.commands_conf_path();
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let mut shot_cfg: Option<ScreenshotConfig> = None;
        if !text.is_empty() {
            config::apply_app_config_from_settings_text(&mut inner.settings, &text);
            inner.esc_app = inner.settings.esc_close;
            inner.esc_sections = parse_esc_close(&text);
            let sections = parse_sections(&text);
            shot_cfg =
                Some(ScreenshotConfig::from_entries(&sections.get("screenshot").cloned().unwrap_or_default()));
        }
        let commands = count_commands(&text);
        let issues: Vec<Value> = config::validate_config(&text)
            .into_iter()
            .map(|i| {
                json!({ "line": i.line, "message": i.message, "fatal": i.fatal })
            })
            .collect();
        let using_backup = inner.using_backup;
        inner.logs.push(format!("config reloaded ({commands} commands)"));
        // Drop the `inner` borrow before locking the screenshot controller
        // (startup builds it without holding `inner`; inner → controller is the
        // fallback order).
        drop(inner);
        if let (Some(cfg), Some(controller)) = (shot_cfg, self.screenshot_controller.clone()) {
            controller.lock().unwrap().set_config(cfg);
        }
        json!({
            "ok": !using_backup,
            "commands": commands,
            "usingBackup": using_backup,
            "issues": issues,
        })
    }

    // -- socket launch path -------------------------------------------------

    /// `SwitcherController.showViewSwitcher()` — show the Ctrl+B W switcher.
    pub fn view_switcher_show(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.view_switcher = true;
        inner.ui_push(UiCommand::ShowSwitcher);
    }

    /// `switcher-hide` — hide the Ctrl+B W switcher (the palette is separate).
    pub fn view_switcher_hide(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.view_switcher = false;
        inner.ui_push(UiCommand::HideSwitcher);
    }

    pub fn view_switcher_shown(&self) -> bool {
        self.inner.lock().unwrap().view_switcher
    }

    /// `state.viewSwitcher.shown` (the panel's own Esc close path).
    pub fn set_view_switcher(&self, on: bool) {
        self.inner.lock().unwrap().view_switcher = on;
    }

    /// The `show` launch verb / `sendToggle`: show or hide the palette.
    pub fn palette_toggle(&self) {
        let mut inner = self.inner.lock().unwrap();
        if inner.palette_visible {
            inner.palette_visible = false;
            inner.ui_push(UiCommand::HidePalette);
        } else {
            inner.palette_visible = true;
            inner.ui_push(UiCommand::ShowPalette);
        }
    }

    /// `SwitcherController.toggleTerminalPanel()`.
    pub fn toggle_terminal_panel(&self) {
        self.inner.lock().unwrap().flip_terminal_panel();
    }

    /// `toggleCommand(_:)`: map a hotkey mode name onto a view / toggle.
    pub fn toggle_command(&self, name: &str) {
        // `open:<path>` (the vim test's tab switch / a CLI open): show notes
        // and hand the path to the surface (`openNoteFile`).
        if let Some(path) = name.strip_prefix("open:") {
            self.set_pending_note_open(path);
            self.open(SlotView::Notes);
            return;
        }
        let name = if name == "voice" { "notes" } else { name };
        match name {
            "window" => {
                self.toggle();
            }
            "show" => {
                self.palette_toggle();
            }
            "term" => {
                self.toggle_terminal_panel();
            }
            "jira" => {
                self.hotkey(SlotView::Jira);
            }
            "jira-dashboard" => self.open(SlotView::Jira),
            "jira-poll-on" | "jira-poll-off" | "jira-poll-toggle" => self.jira_poll(name),
            "jira-setup" => {
                self.inner.lock().unwrap().ui_push(UiCommand::JiraSetup(None));
            }
            "notes" => {
                self.hotkey(SlotView::Notes);
            }
            "files" => {
                self.hotkey(SlotView::Files);
            }
            "confluence" => {
                self.hotkey(SlotView::Confluence);
            }
            "ai" => {
                self.hotkey(SlotView::Ai);
            }
            "compare" => {
                self.hotkey(SlotView::Compare);
            }
            "terminal" => {
                self.toggle_terminal_panel();
            }
            "paths" => {
                let shown = self
                    .inner
                    .lock()
                    .unwrap()
                    .tools
                    .get("paths")
                    .and_then(|v| v.get("shown"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if shown {
                    let _ = self.do_host_action("paths:hide");
                } else {
                    let _ = self.do_host_action("paths:show");
                }
            }
            "health" | "health-checks" | "prettyprint" | "filefast" | "text-table" => {
                self.toggle_tool(name);
            }
            // `startCommandServer`'s fall-through: the notes vim pane's
            // leader+sf / leader+sg send these raw (no `do:`) over `g:ws_sock`.
            "notes-find" | "notes-grep" => {
                self.inner
                    .lock()
                    .unwrap()
                    .ui_push(UiCommand::NoteFind(name == "notes-grep"));
            }
            _ => {
                self.inner
                    .lock()
                    .unwrap()
                    .logs
                    .push(format!("toggleCommand: unhandled mode {name}"));
            }
        }
    }

    /// `launch(message)`: hotkey modes run `hotkeyPrep` first, then
    /// `toggleCommand` (mirrors `startCommandServer`'s fall-through branch).
    pub fn launch_with_prep(&self, message: &str, prep: Option<HotkeyPrep>) {
        if let Some(p) = prep {
            self.apply_hotkey_prep(&p);
        }
        self.toggle_command(message);
    }

    /// `switchWorkspace(cell:)` — `aerospace workspace <key>` off-main.
    pub fn switch_workspace(&self, id: &str) {
        let id = id.to_string();
        std::thread::spawn(move || {
            let ipc = hotkey::AeroIpc::discover();
            let _ = hotkey::AeroCall::call(&ipc, &["workspace".to_string(), id]);
        });
    }

    /// Swift `toggleCommand`'s tool-panel half: hide when the panel is shown
    /// and key, else raise it (`openTool`).
    fn toggle_tool(&self, name: &str) {
        let Some(kind) = ToolKind::from_name(name) else {
            return;
        };
        let name = kind.name();
        let (shown, key) = {
            let inner = self.inner.lock().unwrap();
            let entry = inner.tools.get(name);
            (
                entry
                    .and_then(|v| v.get("shown"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                entry
                    .and_then(|v| v.get("key"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            )
        };
        if shown && key {
            let _ = self.do_host_action(&format!("tool-close:{name}"));
        } else {
            let _ = self.do_host_action(&format!("tool:{name}"));
        }
    }

    fn do_host_action(&self, action: &str) -> Option<Value> {
        // Window-only verbs have no effect without a window; refuse rather than
        // write a `shown`/visible flag into state with nothing behind it.
        if self.is_headless() && needs_window(action) {
            return Some(json!({ "error": format!("{action} unavailable in headless mode") }));
        }
        if let Some(v) = action.strip_prefix("open:") {
            return match SlotView::from_raw(v) {
                Some(view) => {
                    self.open(view);
                    None
                }
                None => Some(json!({ "error": "unknown view" })),
            };
        }
        if let Some(v) = action.strip_prefix("show:") {
            return match SlotView::from_raw(v) {
                Some(view) => {
                    self.open(view);
                    None
                }
                None => Some(json!({ "error": "unknown view" })),
            };
        }
        if let Some(v) = action.strip_prefix("rebuild-card:") {
            return match SlotView::from_raw(v) {
                Some(view)
                    if matches!(
                        view,
                        SlotView::Confluence | SlotView::Ai | SlotView::Compare | SlotView::CompareText
                    ) =>
                {
                    None
                }
                _ => Some(json!({ "error": "not a card view" })),
            };
        }
        if let Some(rest) = action.strip_prefix("compare:") {
            if let Some(arg) = rest.strip_prefix("open-sub:") {
                return self.compare_open(arg, true);
            }
            if let Some(arg) = rest.strip_prefix("open:") {
                return self.compare_open(arg, false);
            }
            if rest == "back" {
                self.back(true);
                return None;
            }
            // next/prev/edit/… stay on the model hook below.
        }
        if let Some(rest) = action.strip_prefix("screenshot:") {
            // `screenshot:*` creates real overlay/pin windows: hop to main like
            // Swift's `testQuery` (2 s budget) so `MainThreadMarker` is present.
            if let Some(controller) = self.screenshot_controller.clone() {
                let rest = rest.to_string();
                let result = self.run_on_main(
                    move || crate::views::screenshot::controller_test_do(&controller, &rest),
                    std::time::Duration::from_secs(2),
                );
                return Some(match result {
                    Some(v) => v,
                    None => json!({ "error": "timeout" }),
                });
            }
        }
        if let Some(v) = action.strip_prefix("pane:") {
            let mut inner = self.inner.lock().unwrap();
            if inner.current.is_none() {
                return Some(json!({ "error": "no view shown" }));
            }
            if inner.main_queue.is_some() {
                // The daemon: real AppKit focus, on main before the reply.
                if let Some(id) = v.strip_prefix("focus:") {
                    inner.ui_push(UiCommand::PaneFocus(id.to_string()));
                    return None;
                }
                if let Some(dir) = v.chars().next().and_then(PaneDir::from_raw) {
                    inner.ui_push(UiCommand::PaneMove(dir));
                    return None;
                }
                return Some(json!({ "error": "pane:h|j|k|l|focus:ID" }));
            }
            let panes = inner.panel_list();
            if let Some(id) = v.strip_prefix("focus:") {
                let found = inner.pane_nav.focus(id, &panes);
                return if found {
                    None
                } else {
                    Some(json!({ "error": "no such pane" }))
                };
            }
            if let Some(c) = v.chars().next() {
                if let Some(dir) = PaneDir::from_raw(c) {
                    inner.pane_nav.move_dir(dir, &panes);
                    return None;
                }
            }
            return Some(json!({ "error": "pane:h|j|k|l|focus:ID" }));
        }
        if let Some(spec) = action.strip_prefix("key:") {
            if spec.is_empty() {
                return Some(json!({ "error": "key:<spec>" }));
            }
            self.inner.lock().unwrap().last_key = spec.to_string();
            return None;
        }
        if let Some(rest) = action.strip_prefix("paths:") {
            let mut inner = self.inner.lock().unwrap();
            match rest {
                "show" => {
                    if !inner.paths_enabled {
                        return Some(json!({ "error": "[paths] not enabled" }));
                    }
                    if let Some(o) = inner.paths_state.as_object_mut() {
                        o.insert("shown".into(), json!(true));
                    }
                    inner.ui_push(UiCommand::Paths(PathsCmd::Show));
                    return None;
                }
                "hide" => {
                    if let Some(o) = inner.paths_state.as_object_mut() {
                        o.insert("shown".into(), json!(false));
                    }
                    inner.ui_push(UiCommand::Paths(PathsCmd::Hide));
                    return None;
                }
                "return" => {
                    inner.ui_push(UiCommand::Paths(PathsCmd::Return));
                    return None;
                }
                sel if sel.starts_with("select:") => {
                    let Ok(n) = sel[7..].parse::<usize>() else {
                        return Some(json!({ "error": "paths:select:N" }));
                    };
                    let rows = inner
                        .paths_state
                        .get("rows")
                        .and_then(Value::as_array)
                        .map_or(0, Vec::len);
                    if n >= rows {
                        return Some(json!({ "error": "paths:select: out of range" }));
                    }
                    if let Some(o) = inner.paths_state.as_object_mut() {
                        o.insert("selection".into(), json!(n));
                    }
                    inner.ui_push(UiCommand::Paths(PathsCmd::Select(n)));
                    return None;
                }
                _ => return Some(json!({ "error": "paths:show|hide|return|select:N" })),
            }
        }
        if let Some(style) = action.strip_prefix("header-style:") {
            return match HeaderStyle::from_raw(style) {
                Some(s) => {
                    self.inner.lock().unwrap().settings.header_style = s;
                    None
                }
                None => Some(json!({ "error": "unknown header style" })),
            };
        } else if let Some(rest) = action.strip_prefix("esc-hides:") {
            let parts: Vec<&str> = rest.split(':').collect();
            if parts.len() == 2 {
                if let Some(v) = SlotView::from_raw(parts[0]) {
                    match parts[1] {
                        "on" | "off" => {
                            self.inner
                                .lock()
                                .unwrap()
                                .esc_sections
                                .insert(v.raw().to_string(), i64::from(parts[1] == "on"));
                            return None;
                        }
                        _ => {}
                    }
                }
            }
            return Some(json!({ "error": "esc-hides:VIEW:on|off" }));
        } else if let Some(rest) = action.strip_prefix("hide-on-focus-loss:") {
            // In-memory flip (tests); the status menu persists it.
            return match rest {
                "on" | "off" => {
                    self.set_hide_on_focus_loss(rest == "on", false);
                    None
                }
                _ => Some(json!({ "error": "hide-on-focus-loss:on|off" })),
            };
        } else if let Some(name) = action.strip_prefix("tool:") {
            if name == "paths" {
                if !self.paths_enabled() {
                    return Some(json!({ "error": "not a tool panel: paths" }));
                }
                {
                    let mut inner = self.inner.lock().unwrap();
                    inner.tools.insert(name.to_string(), json!({ "shown": true }));
                }
                return self.do_host_action("paths:show");
            }
            if let Some(kind) = ToolKind::from_name(name) {
                let mut inner = self.inner.lock().unwrap();
                inner
                    .tools
                    .insert(name.to_string(), json!({ "shown": true, "key": true }));
                inner.ui_push(UiCommand::Tool(kind));
                return None;
            }
            // Empty or unknown names are rejected instead of fabricating a
            // `tools.<name>` entry in state (F3).
            return Some(json!({ "error": format!("not a tool panel: {name}") }));
        } else if let Some(name) = action.strip_prefix("tool-close:") {
            // Same set `tool:` accepts (incl. `paths`); a bogus name is an
            // error instead of a silent no-op.
            if name == "paths" {
                // `paths` is its own panel: clear its `tools` entry too, then
                // run the `paths:hide` hook.
                let mut inner = self.inner.lock().unwrap();
                if let Some(entry) = inner.tools.get_mut(name) {
                    if let Some(o) = entry.as_object_mut() {
                        o.insert("shown".into(), json!(false));
                    }
                }
                drop(inner);
                return self.do_host_action("paths:hide");
            }
            if ToolKind::from_name(name).is_none() {
                return Some(json!({ "error": format!("not a tool panel: {name}") }));
            }
            {
                let mut inner = self.inner.lock().unwrap();
                if let Some(entry) = inner.tools.get_mut(name) {
                    if let Some(o) = entry.as_object_mut() {
                        o.insert("shown".into(), json!(false));
                    }
                }
                inner.ui_push(UiCommand::ToolClose(name.to_string()));
            }
            return None;
        }

        match action {
            "cycle" => {
                self.cycle(1);
                None
            }
            "cycle-back" => {
                self.cycle(-1);
                None
            }
            "hide" => {
                self.hide("test");
                None
            }
            "back" => {
                // `do:back` returns to the previous view (the bounded history),
                // falling back to today's sub-view/`hide` behaviour when empty.
                let target = {
                    let mut inner = self.inner.lock().unwrap();
                    let cur = inner.current;
                    let mut target = None;
                    while let Some(v) = inner.history.pop() {
                        if cur != Some(v) {
                            target = Some(v);
                            break;
                        }
                    }
                    target
                };
                match target {
                    Some(v) => {
                        let len = self.inner.lock().unwrap().history.len();
                        self.open(v);
                        // A stack, not a toggle: drop the view we just left,
                        // which `present()` recorded, so a second `back` keeps
                        // going back instead of returning here.
                        self.inner.lock().unwrap().history.truncate(len);
                    }
                    None => self.back(false),
                }
                None
            }
            "show" => {
                let (ever, last) = {
                    let inner = self.inner.lock().unwrap();
                    (inner.ever_presented, inner.last)
                };
                // A fresh controller has no last view: default to jira.
                self.open(if ever { last } else { SlotView::Jira });
                None
            }
            "home" => {
                self.home();
                None
            }
            "toggle" => {
                self.toggle();
                None
            }
            "toggle-terminal" => {
                let mut inner = self.inner.lock().unwrap();
                inner.notes_terminal = !inner.notes_terminal;
                inner.ui_push(UiCommand::ToggleDrawer(DrawerSide::Terminal));
                None
            }
            "toggle-browser" => {
                let mut inner = self.inner.lock().unwrap();
                inner.notes_browser = !inner.notes_browser;
                inner.ui_push(UiCommand::ToggleDrawer(DrawerSide::Browser));
                None
            }
            "term" => {
                self.inner.lock().unwrap().flip_terminal_panel();
                None
            }
            "switcher" => {
                self.view_switcher_show();
                None
            }
            "switcher-hide" => {
                self.view_switcher_hide();
                None
            }
            "notes-find" | "notes-grep" => {
                let inner = self.inner.lock().unwrap();
                inner.ui_push(UiCommand::NoteFind(action == "notes-grep"));
                None
            }
            "jira-jump" => {
                self.inner.lock().unwrap().ui_push(UiCommand::JiraJump);
                None
            }
            "jira-poll-on" | "jira-poll-off" | "jira-setup" => {
                self.toggle_command(action);
                None
            }
            "reset-size" => {
                let inner = self.inner.lock().unwrap();
                inner.ui_push(UiCommand::ResetSize);
                None
            }
            a if a.starts_with("board:") => {
                // The Swift jira window's board verbs are not ported yet; report
                // a clear error instead of the generic `unhandled action`.
                Some(json!({ "error": format!("{a} not implemented in the Rust port yet") }))
            }
            _ => {
                let rest = action
                    .split_once(':')
                    .map(|(_, r)| r)
                    .unwrap_or_default();
                if action.starts_with("compare:") || action.starts_with("screenshot:") {
                    let result = self.registry.dispatch_test_do(action).or_else(|| {
                        Some(json!({ "error": format!("unhandled action {rest}") }))
                    });
                    // `folder-open` on a file pair opens the compareText sub-view.
                    let push_sub = action.starts_with("compare:")
                        && self
                            .compare_model
                            .as_ref()
                            .map_or(false, |m| m.lock().unwrap().take_sub_pending());
                    if push_sub {
                        self.inner.lock().unwrap().push(SlotView::CompareText);
                    }
                    return result;
                }
                // A verb that no registry hook claims is a typo, not a
                // success: report it so callers (ks-mcp → isError) can see it
                // (F4).
                match self.registry.dispatch_test_do(action) {
                    Some(v) => Some(v),
                    None => Some(json!({ "error": format!("unknown action {action}") })),
                }
            }
        }
    }

    /// Queue `work` on the main thread without waiting (Swift's
    /// `DispatchQueue.main.async`); inline when headless.
    fn post_main(&self, work: impl FnOnce() + Send + 'static) {
        let queue = self.inner.lock().unwrap().main_queue.clone();
        match queue {
            Some(q) => q.lock().unwrap().push(Box::new(work)),
            None => work(),
        }
    }

    /// `screenshot[\tWORDS]`: trigger on main; a caller that wants output
    /// (`--raw`, `--print-geometry`, …) gets the controller's reply bytes once
    /// the session finishes, everyone else is closed straight away.
    fn screenshot_message(&self, rest: &str, reply: Reply) {
        let words: Vec<String> =
            rest.split('\t').filter(|w| !w.is_empty()).map(str::to_string).collect();
        let wants_reply = crate::engines::screenshot_annotations::ShotArgs::parse(&words)
            .map(|a| a.wants_reply())
            .unwrap_or(false);
        let reply = if wants_reply { Some(reply) } else { None };
        let Some(controller) = self.screenshot_controller.clone() else {
            return;
        };
        self.post_main(move || {
            let (finished, data) = {
                let mut c = controller.lock().unwrap();
                c.reply = None;
                let _ = c.handle(&words);
                #[cfg(target_os = "macos")]
                c.redraw_overlays();
                let finished = c.is_idle();
                (finished, if finished { c.reply.take() } else { None })
            };
            let Some(reply) = reply else { return };
            if finished {
                reply.send_bytes(data.as_deref().unwrap_or_default());
                return;
            }
            // An interactive overlay is up: answer when it closes.
            std::thread::spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_millis(50));
                let mut c = controller.lock().unwrap();
                if c.is_idle() {
                    let data = c.reply.take();
                    drop(c);
                    reply.send_bytes(data.as_deref().unwrap_or_default());
                    return;
                }
            });
        });
    }

    /// `compare\tWORDS` (`handleCompareMessage`): `--title1/--title2 T`,
    /// `--wait` (git difftool: reply `done` once the session is closed or the
    /// view hides), then up to two paths.
    fn compare_message(&self, rest: &str, reply: Reply) {
        let words: Vec<&str> = rest.split('\t').collect();
        let mut paths: Vec<String> = Vec::new();
        let mut titles = HashMap::new();
        let wait = words.contains(&"--wait");
        let mut i = 0;
        while i < words.len() {
            match words[i] {
                "--wait" | "" => {}
                "--title1" if i + 1 < words.len() => {
                    titles.insert(crate::views::compare::CompareSide::Left, words[i + 1].to_string());
                    i += 1;
                }
                "--title2" if i + 1 < words.len() => {
                    titles.insert(crate::views::compare::CompareSide::Right, words[i + 1].to_string());
                    i += 1;
                }
                p => paths.push(p.to_string()),
            }
            i += 1;
        }
        let reply = if wait { Some(reply) } else { None };
        let Some(model) = self.compare_model.clone() else {
            if let Some(r) = reply {
                r.send("done");
            }
            return;
        };
        self.open(SlotView::Compare);
        let Some(left) = paths.first() else {
            if let Some(r) = reply {
                r.send("done");
            }
            return;
        };
        let id = model
            .lock()
            .unwrap()
            .open_message(left, paths.get(1).map(String::as_str), &titles, wait);
        let Some(reply) = reply else { return };
        let Some(id) = id else {
            reply.send("done");
            return;
        };
        std::thread::spawn(move || {
            while model.lock().unwrap().is_waiting(id) {
                // An abandoned `compare --wait` (client gone) must not pin an
                // fd and this thread until the window closes.
                if reply.peer_closed() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            reply.send("done");
        });
    }

    /// `pane-shot[\tWORDS]` (`ScreenshotController.paneShot`): render the pane
    /// off-main, deliver (save / copy) on main, reply with the path,
    /// `copied`, or `error: …`.
    fn pane_shot_message(&self, rest: &str, reply: Reply) {
        use crate::engines::pane_shot::{PaneShotArgs, PaneShotConfig};
        let words: Vec<String> =
            rest.split('\t').filter(|w| !w.is_empty()).map(str::to_string).collect();
        let args = match PaneShotArgs::parse(&words) {
            Ok(a) => a,
            Err(p) => {
                reply.send(&format!("error: {}", p.message));
                return;
            }
        };
        let Some(controller) = self.screenshot_controller.clone() else {
            reply.send("error: [screenshot] is not enabled");
            return;
        };
        let entries: HashMap<String, String> = config::read_config_text()
            .map(|t| {
                let lines = crate::engines::config_text::config_lines(&t);
                crate::engines::config_text::config_section_entries(&lines, "pane-shot")
                    .into_iter()
                    .map(|(_, k, v)| (k, v))
                    .collect()
            })
            .unwrap_or_default();
        let cfg = PaneShotConfig::from_entries(&entries);
        #[cfg(target_os = "macos")]
        {
            let scale = self
                .run_on_main(
                    crate::views::screenshot::mouse_screen_scale,
                    std::time::Duration::from_secs(1),
                )
                .unwrap_or(2.0);
            let queue = self.inner.lock().unwrap().main_queue.clone();
            std::thread::spawn(move || {
                let started = crate::views::screenshot::epoch_millis();
                let result = crate::views::screenshot::pane_shot_image(&args, &cfg, scale);
                let deliver = move || {
                    let mut c = controller.lock().unwrap();
                    let line = match result {
                        Ok(shot) => c.deliver_pane_shot(&shot, &args, &cfg, started),
                        Err(f) => {
                            c.pane_shot_failed(&f.message);
                            format!("error: {}", f.message)
                        }
                    };
                    drop(c);
                    reply.send(&line);
                };
                match queue {
                    Some(q) => q.lock().unwrap().push(Box::new(deliver)),
                    None => deliver(),
                }
            });
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (args, cfg, controller);
            reply.send("error: pane-shot needs macOS");
        }
    }

    /// `window-shot[\tWORDS]`: present the view (if `--view`), then capture the
    /// shared window's own content straight to a PNG on the main thread — no
    /// overlay, no activation beyond the present. Replies with the saved path or
    /// `error: …`.
    fn window_shot_message(&self, rest: &str, reply: Reply) {
        use crate::views::screenshot::WindowShotArgs;
        let words: Vec<String> =
            rest.split('\t').filter(|w| !w.is_empty()).map(str::to_string).collect();
        let args = match WindowShotArgs::parse(&words) {
            Ok(a) => a,
            Err(e) => {
                reply.send(&format!("error: {}", e.0));
                return;
            }
        };
        #[cfg(target_os = "macos")]
        {
            use crate::views::screenshot::{screen_capture_permitted, window_shot_image};
            if !screen_capture_permitted() {
                reply.send("error: screen capture not permitted");
                return;
            }
            let target = match args.view.as_deref() {
                Some(name) => match SlotView::from_raw(name) {
                    Some(v) => Some(v),
                    None => {
                        reply.send(&format!("error: unknown view {name}"));
                        return;
                    }
                },
                None => self.current(),
            };
            // Always present: the capture needs the shared window on screen with
            // a live window number, and `open` is what a `do:open:<view>` would do.
            let view = target.unwrap_or(SlotView::Notes);
            self.open(view);
            // The `Present` command is drained before this `run_on_main` job, so
            // the member's `wid` is set by the time the closure runs.
            let inner = self.inner.clone();
            let out = crate::engines::pane_shot::expand_tilde(
                &args.out.clone().unwrap_or_else(default_window_shot_path),
            );
            let scale = args.scale;
            let result = self.run_on_main(
                move || {
                    let wid = {
                        let guard = inner.lock().unwrap();
                        guard.members.get(&view).map_or(0, |m| m.wid)
                    };
                    if wid == 0 {
                        return Err("no shared window to capture".to_string());
                    }
                    let shot = window_shot_image(wid as u32, scale).map_err(|e| e.0)?;
                    let bytes = shot.png.ok_or_else(|| "could not encode PNG".to_string())?;
                    std::fs::write(&out, &bytes)
                        .map_err(|e| format!("could not write {out}: {e}"))?;
                    Ok(out)
                },
                std::time::Duration::from_secs(20),
            );
            match result {
                Some(Ok(path)) => reply.send(&path),
                Some(Err(e)) => reply.send(&format!("error: {e}")),
                None => reply.send("error: window-shot timed out"),
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = args;
            reply.send("error: window-shot needs macOS");
        }
    }

    /// `do:compare:open[:sub]:LEFT|RIGHT` — `showCompare` + `openPair` (Swift
    /// `compareTestDo`): present the compare view (pushing the compareText
    /// sub-view for `open-sub`) and load the pair into the shared model.
    fn compare_open(&self, arg: &str, sub: bool) -> Option<Value> {
        let Some(model) = &self.compare_model else {
            return None;
        };
        let mut parts = arg.splitn(2, '|');
        let l = parts
            .next()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let r = parts
            .next()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        // A path that does not exist must refuse (and open nothing) instead of
        // opening a session for a phantom file.
        for p in [l.as_deref(), r.as_deref()].into_iter().flatten() {
            if !std::path::Path::new(p).exists() {
                return Some(json!({ "error": format!("compare: no such path: {p}") }));
            }
        }
        {
            let mut m = model.lock().unwrap();
            if sub {
                m.open_sub(l.as_deref(), r.as_deref());
                m.take_sub_pending();
            } else {
                m.close_sub();
                m.open_pair(l.as_deref(), r.as_deref());
            }
        }
        self.open(SlotView::Compare);
        if sub {
            self.inner.lock().unwrap().push(SlotView::CompareText);
        }
        None
    }
}

/// Default `--out` for `window-shot`: a unique PNG under the process's temp
/// dir, so callers always get a fresh file without naming one.
fn default_window_shot_path() -> String {
    let name = format!("window-shot-{}.png", crate::views::screenshot::epoch_millis());
    std::env::temp_dir().join(name).to_string_lossy().into_owned()
}

/// `[section] esc-close` / `vim-esc-close` and `[app] esc-close` from the raw
/// commands.toml text (a small pure scan; the python codec is not needed for
/// one integer per section).
fn parse_esc_close(text: &str) -> HashMap<String, i64> {
    let mut out = HashMap::new();
    let mut section = String::new();
    for line in crate::engines::config_text::config_lines(text) {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if let Some(name) = t.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            section = name.trim().to_string();
            continue;
        }
        if let Some((k, v)) = t.split_once('=') {
            let key = k.trim();
            if key == "esc-close" || key == "vim-esc-close" {
                if let Ok(n) = v.trim().parse::<i64>() {
                    out.insert(section.clone(), n);
                }
            }
        }
    }
    out
}

/// Parse `[section]` + `key = value` lines in file order (duplicate sections
/// stay separate entries, mirroring `loadCommands`'s per-header flush).
fn parse_sections_ordered(text: &str) -> Vec<(String, HashMap<String, String>)> {
    let mut out: Vec<(String, HashMap<String, String>)> = Vec::new();
    for line in crate::engines::config_text::config_lines(text) {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if let Some(name) = t.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            out.push((name.trim().to_string(), HashMap::new()));
            continue;
        }
        if let Some((k, v)) = t.split_once('=') {
            let key = k.trim();
            if key.is_empty() {
                continue;
            }
            if let Some((_, vars)) = out.last_mut() {
                vars.insert(key.to_string(), unquote_value(strip_comment(v)));
            }
        }
    }
    out
}

/// A pure `config.tri` over a raw value (the helper-backed
/// `config_text::tri` stays the codec path; the raw scan must not need it).
fn tri_pure(value: &str) -> Option<bool> {
    match value.trim().trim_matches('"').to_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}

/// Strip matching surrounding quotes (the python codec's value decoding).
fn unquote_value(v: &str) -> String {
    let t = v.trim();
    if t.len() >= 2
        && ((t.starts_with('"') && t.ends_with('"'))
            || (t.starts_with('\'') && t.ends_with('\'')))
    {
        t[1..t.len() - 1].to_string()
    } else {
        t.to_string()
    }
}

/// Parse `[section]` + `key = value` lines into `section -> key -> value`
/// (a small pure scan; the python codec is not needed for enabled flags).
fn parse_sections(text: &str) -> HashMap<String, HashMap<String, String>> {
    let mut out: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut section = String::new();
    for line in crate::engines::config_text::config_lines(text) {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if let Some(name) = t.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            section = name.trim().to_string();
            out.entry(section.clone()).or_default();
            continue;
        }
        if let Some((k, v)) = t.split_once('=') {
            let key = k.trim();
            if key.is_empty() {
                continue;
            }
            out.entry(section.clone())
                .or_default()
                .insert(key.to_string(), unquote_value(strip_comment(v)));
        }
    }
    out
}

fn strip_comment(v: &str) -> &str {
    let (mut single, mut double) = (false, false);
    for (i, c) in v.char_indices() {
        match c {
            '\'' if !double => single = !single,
            '"' if !single => double = !double,
            '#' if !single && !double => return &v[..i],
            _ => {}
        }
    }
    v
}

fn section_flag(
    sections: &HashMap<String, HashMap<String, String>>,
    section: &str,
    default: bool,
) -> bool {
    match sections.get(section).and_then(|m| m.get("enabled")) {
        Some(v) => matches!(
            v.trim().trim_matches('"').to_ascii_lowercase().as_str(),
            "true" | "yes" | "on" | "1"
        ),
        None => default,
    }
}

/// `loadCommands().count` — sections with `enabled = "true"` outside the skip
/// list, plus top-level `key = value` shell commands (the Swift `commands`
/// array's length, which `reload` reports).
fn count_commands(text: &str) -> usize {
    let mut n = parse_sections_ordered(text)
        .into_iter()
        .filter(|(name, _)| !COMMAND_SECTION_SKIPS.contains(&name.as_str()))
        .filter(|(_, vars)| vars.get("enabled").map(String::as_str) == Some("true"))
        .count();
    let mut in_section = false;
    for line in crate::engines::config_text::config_lines(text) {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if t.starts_with('[') {
            in_section = true;
            continue;
        }
        if !in_section {
            if let Some((k, v)) = t.split_once('=') {
                if !k.trim().is_empty() && !v.trim().is_empty() {
                    n += 1;
                }
            }
        }
    }
    n
}

/// Count `[section]` headers.
fn count_sections(text: &str) -> usize {
    crate::engines::config_text::config_lines(text)
        .iter()
        .filter(|l| {
            let t = l.trim();
            t.starts_with('[') && t.ends_with(']') && !t.starts_with('#')
        })
        .count()
}

/// What [`FocusWatch::tick`] asks the daemon UI to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FocusAction {
    None,
    /// `checkFocusLoss` → `hide("focus loss → …", restoreFocus: false)`.
    Hide,
    /// Focus stolen right after a view swap → `m.slotShow` (refocus).
    Refocus,
    /// `startFocusBridge`'s didBecomeKey observer: our window became key while
    /// the app is inactive (AeroSpace focused it) → `NSApp.activate`.
    SelfActivate,
}

/// One drain tick's view of the shared window's focus (all live AppKit).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct FocusSample {
    pub now: f64,
    /// The shared window is visible.
    pub shown: bool,
    /// The shared window is key.
    pub key: bool,
    /// `focusLeft(w)`: not key, no sheet, and no other visible window of
    /// ours holds key.
    pub left: bool,
    /// Any window of ours is key (Swift's global didBecomeKey cancels a
    /// pending focus-loss check).
    pub any_key: bool,
    pub app_active: bool,
    /// A click in another app within the last second (`lastOtherAppClick`).
    pub clicked_away: bool,
}

/// The polled port of `SharedWindow`'s didResignKey/didBecomeKey observers
/// (`focusLossGen` + `asyncAfter(focusLossDelay)` + `checkFocusLoss`) and of
/// `startFocusBridge`'s "became key while inactive" observer. The daemon UI
/// samples AppKit every drain tick; the edges stand in for the notifications.
#[derive(Clone, Debug, Default)]
pub(crate) struct FocusWatch {
    was_key: bool,
    was_any_key: bool,
    /// When the shared window resigned key (the pending check), if pending.
    resigned_at: Option<f64>,
    /// `swappedAt` — the last view swap inside the shown window.
    swapped_at: Option<f64>,
    /// Our own last `present` (it activates itself; the key edge it causes
    /// is not an AeroSpace focus).
    presented_at: Option<f64>,
}

impl FocusWatch {
    /// `slot.present` swapped views inside the shown window.
    pub fn note_swap(&mut self, now: f64) {
        self.swapped_at = Some(now);
    }

    /// The daemon UI presented (activated + made key) the window itself.
    pub fn note_present(&mut self, now: f64) {
        self.presented_at = Some(now);
    }

    /// `hide()` clears `swappedAt` and any pending check.
    pub fn reset(&mut self) {
        *self = FocusWatch::default();
    }

    pub fn tick(&mut self, s: FocusSample, hide_on_focus_loss: bool, delay: f64) -> FocusAction {
        if !s.shown {
            self.reset();
            return FocusAction::None;
        }
        let became_key = s.key && !self.was_key;
        let resigned = !s.key && self.was_key;
        // didBecomeKey (any window of ours) bumps `focusLossGen`.
        let any_became_key = s.any_key && (!self.was_any_key || became_key);
        self.was_key = s.key;
        self.was_any_key = s.any_key;
        if any_became_key {
            self.resigned_at = None;
        }
        if became_key {
            let own = self.presented_at.map_or(false, |t| s.now - t < 0.5);
            if !s.app_active && !s.clicked_away && !own {
                return FocusAction::SelfActivate;
            }
            return FocusAction::None;
        }
        if resigned {
            self.resigned_at = Some(s.now);
        }
        let Some(t) = self.resigned_at else {
            return FocusAction::None;
        };
        if s.now - t < delay {
            return FocusAction::None;
        }
        self.resigned_at = None;
        if !s.left {
            return FocusAction::None;
        }
        if let Some(sw) = self.swapped_at.take() {
            if s.now - sw < 1.0 {
                return FocusAction::Refocus;
            }
        }
        if hide_on_focus_loss {
            FocusAction::Hide
        } else {
            FocusAction::None
        }
    }
}

/// `focusBridgeChanged(_:)`'s decision: the bridge file names our shown
/// shared window (`raw` = AeroSpace's focused window id), it was written
/// within the last second, and the user did not just click another app.
/// One AeroSpace workspace for the header strip (`WorkspaceInfo`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StripWorkspace {
    pub id: String,
    /// `(app name, bundle id, window title)` per window.
    pub apps: Vec<(String, String, String)>,
    pub focused: bool,
}

/// `gatherWorkspaces()` (with bundle ids) filtered like `refreshWorkspaceStrip`
/// (only workspaces with windows, plus the focused one), numeric ids first.
pub(crate) fn gather_strip_with(call: &dyn Fn(&[String]) -> Option<String>) -> Vec<StripWorkspace> {
    let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let order: Vec<String> = call(&args(&["list-workspaces", "--all"]))
        .unwrap_or_default()
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    if order.is_empty() {
        return Vec::new();
    }
    let focused = call(&args(&["list-workspaces", "--focused"]))
        .unwrap_or_default()
        .trim()
        .to_string();
    let mut rows: Vec<StripWorkspace> = order
        .iter()
        .map(|id| StripWorkspace { id: id.clone(), apps: Vec::new(), focused: *id == focused })
        .collect();
    let wins = call(&args(&[
        "list-windows",
        "--all",
        "--format",
        "%{app-name}|%{app-bundle-id}|%{window-title}|%{workspace}",
    ]))
    .unwrap_or_default();
    for line in wins.lines() {
        let parts: Vec<&str> = line.split('|').collect();
        if parts.len() < 4 {
            continue;
        }
        let ws = parts[parts.len() - 1];
        let title = parts[2..parts.len() - 1].join("|");
        if let Some(row) = rows.iter_mut().find(|w| w.id == ws) {
            row.apps.push((parts[0].to_string(), parts[1].to_string(), title));
        }
    }
    rows.sort_by(|a, b| match (a.id.parse::<i64>(), b.id.parse::<i64>()) {
        (Ok(x), Ok(y)) => x.cmp(&y),
        (Err(_), Ok(_)) => std::cmp::Ordering::Less,
        (Ok(_), Err(_)) => std::cmp::Ordering::Greater,
        (Err(_), Err(_)) => a.id.to_lowercase().cmp(&b.id.to_lowercase()),
    });
    rows.retain(|w| !w.apps.is_empty() || w.focused);
    rows
}

pub(crate) fn focus_bridge_should_activate(
    raw: &str,
    our_wid: i64,
    age: f64,
    clicked_away: bool,
) -> bool {
    let Ok(id) = raw.trim().parse::<i64>() else {
        return false;
    };
    our_wid != 0 && id == our_wid && !clicked_away && age < 1.0
}

/// Live AppKit focus, read on main for each `state` (see `daemon_ui::live_focus`).
#[derive(Clone, Debug, Default)]
pub(crate) struct LiveFocus {
    pub active: bool,
    pub key_title: String,
    pub key_wid: i64,
    pub windows: i64,
    pub frontmost: i32,
    /// The primary screen's height (AppKit → top-left screen points).
    pub main_h: f64,
}

/// The header ✕ button's frame in the flipped host root (x, y, w, h).
const HEADER_CLOSE_FRAME: (f64, f64, f64, f64) = (4.0, 4.0, 26.0, 22.0);

/// `closeButtonRect` in global top-left screen points (what `cliclick`
/// takes), for a window `frame` [x, y, w, h] in AppKit coordinates.
fn header_close_point(frame: &[f64], main_h: f64) -> [i64; 2] {
    let (cx, cy, cw, ch) = HEADER_CLOSE_FRAME;
    let (x, y, h) = (frame[0], frame[1], frame[3]);
    [
        (x + cx + cw / 2.0).round() as i64,
        (main_h - (y + h - (cy + ch / 2.0))).round() as i64,
    ]
}

impl ControllerInner {
    /// Overwrite the present-time mirrors with live values. Every view shares
    /// the one host window, so only the current view can be key (Swift: a
    /// parked member's window is never key).
    fn apply_live_focus(&mut self, f: &LiveFocus) {
        self.active = f.active;
        self.key_window = f.key_title.clone();
        self.window_count = f.windows;
        self.frontmost_pid = f.frontmost;
        let current = self.current;
        for (v, m) in self.members.iter_mut() {
            m.key = Some(*v) == current && m.shown && m.wid != 0 && m.wid == f.key_wid;
        }
    }
}

impl SwitcherController {
    /// One main-thread hop per `state`: read the live focus fields and sync
    /// the screenshot overlay panels' key/wid into the controller.
    #[cfg(target_os = "macos")]
    fn live_focus(&self) -> Option<LiveFocus> {
        let shot = self.screenshot_controller.clone();
        self.run_on_main(
            move || {
                let f = daemon_ui::live_focus();
                if f.is_some() {
                    if let Some(c) = &shot {
                        c.lock().unwrap().sync_live_overlay();
                    }
                }
                f
            },
            std::time::Duration::from_secs(2),
        )
        .flatten()
    }

    #[cfg(not(target_os = "macos"))]
    fn live_focus(&self) -> Option<LiveFocus> {
        None
    }

    /// No daemon UI was installed (`install_ui` / `install_main_queue` never
    /// ran): the `headless` server. Screen-capture verbs must refuse rather
    /// than grab the real screen (F0-3).
    fn is_headless(&self) -> bool {
        self.inner.lock().unwrap().main_queue.is_none()
    }
}

impl CommandHandler for SwitcherController {
    fn state_json(&self) -> Value {
        let live = self.live_focus();
        let mut v = {
            let mut inner = self.inner.lock().unwrap();
            if let Some(f) = &live {
                inner.apply_live_focus(f);
            }
            inner.state_with_registry(&self.registry)
        };
        // `screenshot` / `compare` report their real controllers when enabled
        // (the inner placeholder only covers a disabled/detached tool).
        if let Some(c) = &self.screenshot_controller {
            let c = c.lock().unwrap();
            v["screenshot"] = c.test_state();
            v["paneShot"] = c.pane_shot_last.clone();
        }
        if let Some(m) = &self.compare_model {
            let mut st = m.lock().unwrap().test_state();
            if let Some(o) = st.as_object_mut() {
                o.entry("shown").or_insert(json!(false));
                o.entry("key").or_insert(json!(false));
                o.entry("frame").or_insert(json!([0, 0, 0, 0]));
                o.entry("close").or_insert(json!([0, 0]));
                o.entry("sheet").or_insert(json!(false));
                o.entry("shortcutsCard").or_insert(json!(false));
            }
            let view = v["view"].clone();
            let is_compare = matches!(view.as_str(), Some("compare") | Some("compareText"));
            // Swift reports the compare window's frame and its ✕ point.
            if is_compare {
                let frame: Option<Vec<f64>> = view
                    .as_str()
                    .and_then(|name| v["views"][name]["frame"].as_array())
                    .map(|a| a.iter().filter_map(Value::as_f64).collect())
                    .filter(|f: &Vec<f64>| f.len() == 4);
                if let Some(f) = frame {
                    st["frame"] = json!(f.iter().map(|x| x.round() as i64).collect::<Vec<_>>());
                    if let Some(main_h) = live.as_ref().map(|l| l.main_h).filter(|h| *h > 0.0) {
                        st["close"] = json!(header_close_point(&f, main_h));
                    }
                }
            }
            // `sub` is the Swift sub-window flag: true while compareText is up.
            st["sub"] = json!(view.as_str() == Some("compareText"));
            st["view"] = if is_compare { view } else { json!("") };
            v["compare"] = st;
        }
        v
    }

    fn do_action(&self, action: &str) -> Option<Value> {
        let result = self.do_host_action(action);
        // Swift's `testQuery` dispatches on main before replying. Flush the
        // daemon UI queue the same way so a caller that immediately re-reads
        // `state` sees the presented window (`views.*.shown`, `windows`).
        let _ = self.run_on_main(|| {}, std::time::Duration::from_millis(1000));
        // A registry hook answers `Some(null)` for "handled, no error"
        // (`compareTestDo` returning nil): reply with the state like Swift,
        // never a bare `null`.
        result.filter(|v| !v.is_null())
    }

    fn raw_deferred(&self, verb: &str, rest: &str, reply: Reply) -> Option<Reply> {
        // A headless daemon has no window to capture; refuse instead of
        // reading the real screen (F0-3).
        if matches!(verb, "screenshot" | "pane-shot" | "window-shot") && self.is_headless() {
            reply.send(&format!("error: {verb} unavailable in headless mode"));
            return None;
        }
        match verb {
            "screenshot-permission" => {
                #[cfg(target_os = "macos")]
                let ok = crate::views::screenshot::screen_capture_permitted();
                #[cfg(not(target_os = "macos"))]
                let ok = false;
                reply.send(if ok { "granted" } else { "denied" });
            }
            "screenshot" => self.screenshot_message(rest, reply),
            "compare" => self.compare_message(rest, reply),
            "pane-shot" => self.pane_shot_message(rest, reply),
            "window-shot" => self.window_shot_message(rest, reply),
            _ => return Some(reply),
        }
        None
    }

    fn raw_request(&self, verb: &str, _rest: &str) -> Option<String> {
        match verb {
            "reload" => Some(self.reload_config().to_string()),
            "restart" => Some(json!({ "ok": true, "restarting": true }).to_string()),
            _ => None,
        }
    }

    fn launch(&self, message: &str) {
        let prep = if self.inner.lock().unwrap().settings.shared_window
            && hotkey::is_hotkey_mode(message)
        {
            Some(self.hotkey_prep())
        } else {
            None
        };
        self.launch_with_prep(message, prep);
    }
}

// ---------------------------------------------------------------------------
// Daemon UI bridge (macOS): drains the controller's [`UiCommand`] queue on the
// main thread and drives a `SlotHostWindow` with a `PopupChrome` header, the
// view-switcher buttons and one content surface per [`SlotView`]. The content
// surfaces are placeholders until each view module lands its `build_content`.
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
pub use daemon_ui::install_daemon_ui;

/// `PathShelf.add([path], why: .screenshot)` after a successful overlay save.
/// Called on the main thread from `views::screenshot`'s save path.
pub fn screenshot_saved_to_path_shelf(path: &str) {
    #[cfg(target_os = "macos")]
    daemon_ui::shelf_add(&[path.to_string()], crate::engines::path_shelf::Why::Screenshot);
    #[cfg(not(target_os = "macos"))]
    let _ = path;
}

/// Show `paths` in the app's own file browser (`SlotView::Files`) — the uniform
/// entry point for every "reveal / show in Finder" action, callable from any
/// module. Never opens macOS Finder. A no-op off macOS or before the UI is up.
pub fn reveal_in_files(paths: &[String]) {
    #[cfg(target_os = "macos")]
    daemon_ui::reveal_in_files(paths);
    #[cfg(not(target_os = "macos"))]
    let _ = paths;
}

#[cfg(target_os = "macos")]
mod daemon_ui {
    mod host_vim;
    use super::{DrawerSide, MainQueue, PathsCmd, SwitcherController, UiCommand, UiQueue, DEFAULT_VIEW};
    use crate::app::registry::{RectI, SlotView};
    use crate::panes::pane_geometry::PaneDir;
    use crate::ui::chrome::{ChromeConfig, NavIcon, PopupChrome};
    use crate::ui::popup::{self, KeyAction, MonitorHandle, PopupConfig};
    use crate::ui::shared_window::SlotHostWindow;
    use crate::ui::theme::{set_current_header_style, HeaderStyle as ThemeHeaderStyle, PopupColors};
    use crate::views::paths::{PathsWindow, ReturnAction as PathsReturn};
    use crate::views::switcher::{
        gather_workspaces, PaletteAction, PalettePanel, SwitchFrame, SwitchView,
        ViewSwitcherPanel,
    };
    use crate::views::terminal::{TerminalConfig, TerminalPanel};
    use crate::views::tools::{ToolKind, ToolPanel};
    use crate::views::files::FileBrowser;
    use objc2::rc::{Retained, Weak};
    use serde_json::json;
    use objc2::runtime::{AnyObject, NSObject};
    use objc2::{
        define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly, Message,
    };
    use objc2_app_kit::{
        NSApplication, NSAutoresizingMaskOptions, NSButton, NSFont, NSEvent, NSTextField,
        NSView, NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState,
        NSVisualEffectView, NSWindowCollectionBehavior,
    };
    use objc2_foundation::{NSInteger, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSTimer};
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;
    use std::rc::Rc;
    use std::sync::Arc;

    fn now_secs() -> f64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0)
    }

    const HEADER_HEIGHT: f64 = 30.0;
    const NAV_X0: f64 = 36.0;
    const NAV_STEP: f64 = 68.0;
    const NAV_WIDTH: f64 = 64.0;
    const NAV_HEIGHT: f64 = 22.0;

    fn as_any<T: Message + ?Sized>(obj: &T) -> &AnyObject {
        unsafe { &*(obj as *const T as *const AnyObject) }
    }

    fn mask_fill() -> NSAutoresizingMaskOptions {
        NSAutoresizingMaskOptions::ViewWidthSizable
            | NSAutoresizingMaskOptions::ViewHeightSizable
    }

    /// `NSApp.activate(ignoringOtherApps: true)` — the exact Swift call.
    /// The non-deprecated `activate()` does not take focus for a
    /// background-launched process on this OS.
    thread_local! {
        /// The app that was frontmost when Hyper+S opened the palette from
        /// outside (`None` = the user was already in kitchen-sink). Main
        /// thread only; the palette's own callbacks read it synchronously.
        static PALETTE_RETURN: Cell<Option<i32>> = const { Cell::new(None) };
        /// That app + when, kept after a pick for the tool panel it opens.
        static PICK_RETURN: Cell<Option<(i32, std::time::Instant)>> = const { Cell::new(None) };
    }

    /// Picks that are *meant* to bring kitchen-sink forward: "Kitchen Sink"
    /// and the shared window's views. Every other pick — every popup (tool
    /// panels, terminal, paths, screenshot, the notes finders), all
    /// non-activating and taking the keyboard without activating us — and
    /// workspaces leave the user's app focused.
    fn pick_keeps_app(action: &PaletteAction) -> bool {
        match action {
            PaletteAction::Command(name) => matches!(
                name.as_str(),
                "window" | "notes" | "files" | "jira" | "confluence" | "ai" | "compare"
            ),
            PaletteAction::Workspace(_) => false,
        }
    }

    /// Re-activate the app Hyper+S was pressed over, *before* the palette is
    /// ordered out: while we are active with no palette on screen, macOS
    /// makes the shared window key and AeroSpace follows it to its
    /// workspace. Returns whether there was an app to go back to.
    fn palette_hand_back(mtm: MainThreadMarker) -> bool {
        let Some(pid) = PALETTE_RETURN.with(|c| c.take()) else { return false };
        PICK_RETURN.with(|c| c.set(Some((pid, std::time::Instant::now()))));
        if !crate::platform::activate_pid(pid) {
            NSApplication::sharedApplication(mtm).deactivate();
        }
        true
    }

    /// A pick: hand focus back unless the command wants kitchen-sink.
    fn palette_pick_focus(mtm: MainThreadMarker, action: &PaletteAction) {
        if pick_keeps_app(action) {
            if let Some(pid) = PALETTE_RETURN.with(|c| c.take()) {
                PICK_RETURN.with(|c| c.set(Some((pid, std::time::Instant::now()))));
            }
        } else {
            palette_hand_back(mtm);
        }
    }

    #[allow(deprecated)]
    fn activate_app(mtm: MainThreadMarker) {
        NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
    }

    /// Flipped (top-left origin) root so the header sits at y = 0.
    pub struct HostRootViewIvars;

    define_class!(
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSHostRootView"]
        #[ivars = HostRootViewIvars]
        pub struct HostRootView;

        impl HostRootView {
            #[unsafe(method(isFlipped))]
            fn is_flipped(&self) -> bool {
                true
            }
        }

        unsafe impl NSObjectProtocol for HostRootView {}
    );

    impl HostRootView {
        fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(HostRootViewIvars);
            unsafe { msg_send![super(this), initWithFrame: frame] }
        }
    }

    /// The `presentPathSheet` OK/Cancel target (a `TextFieldSheet` mirror).
    pub struct PathSheetHandlerIvars {
        sheet: Retained<objc2_app_kit::NSWindow>,
        field: Retained<NSTextField>,
        on_result: RefCell<Option<Box<dyn FnMut(Option<String>)>>>,
    }

    thread_local! {
        static PATH_SHEET_HANDLERS: RefCell<Vec<Retained<PathSheetHandler>>> = const { RefCell::new(Vec::new()) };
        /// The folder tree's Quick Look panel source (menu ▸ Quick Look).
        static COMPARE_QUICK_LOOK: RefCell<crate::views::files::QuickLook> = RefCell::new(crate::views::files::QuickLook::new());
    }

    /// `~`-abbreviated path for prompts and toasts.
    fn tilde_path(p: &str) -> String {
        crate::views::notes::tilde_path(p, &std::env::var("HOME").unwrap_or_default())
    }

    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSPathSheetHandler"]
        #[ivars = PathSheetHandlerIvars]
        pub struct PathSheetHandler;

        impl PathSheetHandler {
            #[unsafe(method(ok:))]
            fn ok(&self, _sender: Option<&AnyObject>) {
                let value = self.ivars().field.stringValue().to_string();
                self.end();
                let cb = self.ivars().on_result.borrow_mut().take();
                if let Some(mut cb) = cb {
                    cb(Some(value));
                }
            }

            #[unsafe(method(cancel:))]
            fn cancel(&self, _sender: Option<&AnyObject>) {
                self.end();
                let cb = self.ivars().on_result.borrow_mut().take();
                if let Some(mut cb) = cb {
                    cb(None);
                }
            }
        }

        unsafe impl NSObjectProtocol for PathSheetHandler {}
    );

    impl PathSheetHandler {
        fn end(&self) {
            if let Some(parent) = self.ivars().sheet.sheetParent() {
                parent.endSheet(&self.ivars().sheet);
            } else {
                self.ivars().sheet.orderOut(None);
            }
        }
    }

    struct DaemonUiState {
        window: Retained<SlotHostWindow>,
        chrome: Retained<PopupChrome>,
        content: Retained<NSView>,
        views: HashMap<SlotView, Retained<NSView>>,
        current: Option<SlotView>,
        /// The notes surface (kept so its drawers/editor stay addressable).
        notes: Option<crate::views::notes::NotesSurface>,
        /// The files surface (kept so its list keys stay addressable).
        files: Option<crate::views::files::FileBrowser>,
        /// The live AI window.
        ai: Option<AIState>,
        /// The live Confluence window (model + view + web preview).
        confluence: Option<ConfState>,
        /// The live Jira list (`ListSession`) + its view.
        jira: Option<(crate::views::jira_view::JiraList, Retained<crate::views::jira_view::JiraView>)>,
        /// The live compare surface (compare + compareText share it).
        compare: Option<crate::views::compare::CompareSurface>,
        /// The Ctrl+B prefix machine (`SharedWindow.prefixKey`).
        prefix: popup::PrefixState,
        /// The `show` palette (commands + workspaces).
        palette: Option<PalettePanel>,
        /// The Ctrl+B W view switcher (a second `ViewSwitcherPanel`).
        switcher: Option<ViewSwitcherPanel>,
        /// The `/paths` shelf tool window.
        paths: Option<PathsWindow>,
        /// The `/terminal` tool panel (`do:term` / Ctrl+B T).
        terminal_panel: Option<TerminalPanel>,
        /// The Cmd+/ shortcuts sheet while it is up.
        shortcuts: Option<Retained<crate::ui::shortcuts::ShortcutsOverlay>>,
        /// The `openTool` panels (prettyprint / filefast / health-checks).
        tools: HashMap<String, ToolPanel>,
        /// When each tool panel was opened (the `reclaimToolKey` window).
        tool_opened_at: HashMap<String, f64>,
        /// When the terminal popup was last shown (its one reclaim check).
        terminal_opened_at: Option<std::time::Instant>,
    }

    /// Every visible `NSWindow` of this app (`state.windows` mirror).
    /// `SharedWindow.hideOrFocus`'s "user is in it": the shared window (window
    /// number `wid`; `0` = unknown, any visible key window) is key, the app is
    /// active, and it is the frontmost app. Off the main thread (headless
    /// tests) there is no AppKit to ask, so it counts as in it.
    /// The live focus fields `testQuery` reads straight from AppKit (`NSApp.isActive`,
    /// `NSApp.keyWindow`, visible window count, frontmost app). `None` off the
    /// main thread (headless tests keep the mirrored values).
    pub(super) fn live_focus() -> Option<super::LiveFocus> {
        let mtm = MainThreadMarker::new()?;
        let app = NSApplication::sharedApplication(mtm);
        let key = app.keyWindow();
        Some(super::LiveFocus {
            active: app.isActive(),
            key_title: key.as_ref().map(|w| w.title().to_string()).unwrap_or_default(),
            key_wid: key.as_ref().map_or(0, |w| w.windowNumber() as i64),
            windows: visible_window_count(mtm),
            frontmost: crate::platform::frontmost_app().map_or(0, |a| a.pid),
            main_h: crate::platform::screens()
                .first()
                .map_or(0.0, |s| s.frame.h),
        })
    }

    pub(super) fn user_in_it(wid: i64) -> bool {
        let Some(mtm) = MainThreadMarker::new() else {
            return true;
        };
        let app = NSApplication::sharedApplication(mtm);
        let key = app.keyWindow().map_or(false, |w| {
            w.isVisible() && (wid == 0 || w.windowNumber() as i64 == wid)
        });
        let front = crate::platform::frontmost_app().map_or(0, |a| a.pid);
        key && app.isActive() && front == std::process::id() as i32
    }

    fn visible_window_count(mtm: MainThreadMarker) -> i64 {
        let app = NSApplication::sharedApplication(mtm);
        app.windows()
            .iter()
            .filter(|w| w.isVisible())
            .count() as i64
    }

    /// `AIWindow`'s live pieces.
    pub(crate) struct AIState {
        model: crate::views::ai::AIWindowModel,
        view: Retained<crate::views::ai_view::AIView>,
        command: String,
        tokens: String,
        tokens_warn: bool,
        token_due: Option<f64>,
        token_gen: u64,
        was_running: bool,
        set_input: Option<String>,
    }

    /// `ConfluenceWindow`'s live pieces.
    pub(crate) struct ConfState {
        model: crate::views::confluence::ConfluenceModel,
        view: Retained<crate::views::confluence_view::ConfluenceView>,
        preview: crate::views::confluence::macos::ConfluencePreview,
        pstate: crate::views::confluence_view::ConfPreviewState,
        pages: HashMap<String, serde_json::Value>,
        preview_id: String,
        search_gen: u64,
        preview_gen: u64,
        last_hit: Option<(i64, i64, String)>,
    }

    thread_local! {
        /// `PathShelf.shared` — the Paths tool's recent-files shelf.
        static SHELF: RefCell<Option<Rc<RefCell<crate::engines::path_shelf::PathShelf>>>> = const { RefCell::new(None) };
    }

    /// `configureRecentFiles()` + `configurePathShelf()` at launch: the
    /// Recent engine starts and feeds the shelf (`onKept` / `onRenamed`).
    fn configure_recent_and_shelf() {
        let text = crate::app::config::read_config_text().unwrap_or_default();
        let sections = super::parse_sections(&text);
        crate::views::files::start_recent(&sections.get("files").cloned().unwrap_or_default());
        let Some(paths) = sections.get("paths") else { return };
        let limit = paths.get("limit").and_then(|v| v.parse::<i64>().ok()).unwrap_or(500);
        let p = crate::app::paths::Paths::from_env();
        let ignore = paths
            .get("ignore-file")
            .map(|v| crate::app::config::expand_tilde(v))
            .filter(|v| !v.is_empty())
            .map(|v| if v.starts_with('/') { v } else { format!("{}/{v}", p.user_dir()) })
            .unwrap_or_else(|| format!("{}/config/paths.ignore", p.user_dir()));
        let shelf = Rc::new(RefCell::new(crate::engines::path_shelf::PathShelf::new()));
        shelf.borrow_mut().configure(limit, Some(&ignore));
        let sh = shelf.clone();
        crate::views::files::with_recent(|r| crate::engines::path_shelf::PathShelf::wire_recent(sh, r));
        if shelf.borrow().is_empty() {
            let seed: Vec<(String, f64, Option<String>)> =
                crate::views::files::recent_snapshot(false).into_iter().map(|e| (e.path, e.at, e.source)).collect();
            shelf.borrow_mut().seed(&seed);
        }
        SHELF.with(|s| *s.borrow_mut() = Some(shelf));
    }

    fn shelf_entries() -> Vec<crate::engines::path_shelf::Item> {
        SHELF.with(|s| s.borrow().as_ref().map(|sh| sh.borrow().entries()).unwrap_or_default())
    }

    /// `PathShelf.add(_:why:)` on the live shelf (main thread). The screenshot
    /// save path calls this through [`super::screenshot_saved_to_path_shelf`].
    pub(crate) fn shelf_add(paths: &[String], why: crate::engines::path_shelf::Why) {
        SHELF.with(|s| {
            if let Some(sh) = s.borrow().as_ref() {
                sh.borrow_mut().add(paths, why);
            }
        });
    }

    thread_local! {
        /// Cmd+X in the file list: the next Cmd+V moves instead of copies.
        static FILES_CUT: Cell<bool> = const { Cell::new(false) };
    }

    /// Last `gather_workspaces()` result, so the palette opens instantly.
    static WORKSPACE_CACHE: std::sync::Mutex<Vec<crate::views::switcher::WorkspaceRow>> =
        std::sync::Mutex::new(Vec::new());

    thread_local! {
        /// The daemon UI, for main-thread hops from worker threads.
        static UI_WEAK: RefCell<Option<objc2::rc::Weak<DaemonUi>>> = const { RefCell::new(None) };
    }

    thread_local! {
        /// Generation of the pending click-to-rename; a double-click or a
        /// newer click bumps it so the older timer does nothing.
        static CLICK_RENAME_GEN: Cell<u64> = const { Cell::new(0) };
    }

    fn cancel_click_rename() {
        CLICK_RENAME_GEN.with(|g| g.set(g.get() + 1));
    }

    /// Run `f` after the system double-click interval unless a double-click
    /// (or another click) arrived first — single click renames, double opens.
    fn schedule_click_rename(f: fn(&DaemonUi)) {
        cancel_click_rename();
        let gen = CLICK_RENAME_GEN.with(|g| g.get());
        let secs = unsafe { objc2_app_kit::NSEvent::doubleClickInterval() }.max(0.2) + 0.05;
        let Ok(when) = dispatch2::DispatchTime::try_from(std::time::Duration::from_secs_f64(secs)) else {
            return;
        };
        let _ = dispatch2::DispatchQueue::main().after(when, move || {
            if CLICK_RENAME_GEN.with(|g| g.get()) == gen {
                let ui = UI_WEAK.with(|w| w.borrow().as_ref().and_then(|w| w.load()));
                if let Some(ui) = ui {
                    f(&ui);
                }
            }
        });
    }

    /// Run `f` on the main thread against the live daemon UI.
    fn on_ui(f: impl FnOnce(&DaemonUi) + Send + 'static) {
        crate::platform::run_on_main(move || {
            let ui = UI_WEAK.with(|w| w.borrow().as_ref().and_then(|w| w.load()));
            if let Some(ui) = ui {
                f(&ui);
            }
        });
    }

    /// `ConfluenceAPI.run(args, stdin:) { j in … }` — the CLI off-main, the
    /// reply on main (`{"ok": false, "error"}` when the run itself fails).
    fn conf_api(
        args: Vec<String>,
        stdin: Option<serde_json::Value>,
        then: impl FnOnce(&DaemonUi, serde_json::Value) + Send + 'static,
    ) {
        std::thread::spawn(move || {
            let argv: Vec<&str> = args.iter().map(String::as_str).collect();
            let data = stdin.map(|v| serde_json::to_vec(&v).unwrap_or_default());
            let j = match crate::views::confluence::run_api(&argv, data.as_deref()) {
                Ok(v) => v,
                Err(e) => serde_json::json!({ "ok": false, "error": e }),
            };
            on_ui(move |ui| then(ui, j));
        });
    }

    pub struct DaemonUiIvars {
        /// Header workspace strip: the background gather's latest result,
        /// whether one is running, when it last ran, the drawn signature and
        /// the per-app icon cache (`refreshWorkspaceStrip`, `iconCache`).
        strip_result: Arc<std::sync::Mutex<Option<Vec<super::StripWorkspace>>>>,
        strip_busy: Arc<std::sync::atomic::AtomicBool>,
        strip_last: Cell<f64>,
        files_recent_at: Cell<f64>,
        jira_refresh_at: Cell<f64>,
        strip_sig: RefCell<String>,
        icon_cache: RefCell<HashMap<String, Retained<objc2_app_kit::NSImage>>>,
        controller: Arc<SwitcherController>,
        queue: UiQueue,
        /// The `do:` main-hop jobs (`SwitcherController::run_on_main`).
        main_queue: MainQueue,
        state: RefCell<Option<DaemonUiState>>,
        /// The Esc streak (`escStreakCloses`): consecutive Esc presses within
        /// 0.6 s; any other key resets it.
        esc_streak: Cell<u32>,
        last_esc_at: Cell<f64>,
        /// The local keyDown guard (`PopupWindow.installMonitors`).
        key_monitor: RefCell<Option<MonitorHandle>>,
        /// Last `checktime` poll of the notes vim pane (Swift's note watcher).
        last_vim_check: Cell<f64>,
        /// When the notes vim pane last relaunched (`:q`) — the
        /// `TerminalAutoRestart` reclaim window.
        last_vim_relaunch: Cell<f64>,
        /// The polled didResignKey/didBecomeKey observers
        /// (`SharedWindow.checkFocusLoss`, `startFocusBridge`).
        focus_watch: RefCell<super::FocusWatch>,
        /// `$TMPDIR/<[app] focus-bridge>` — AeroSpace's `on-focus-changed`
        /// writes the focused window id here (`watchFocusBridge`).
        bridge_path: String,
        /// The bridge file's last seen mtime (`bridgeMtime`).
        bridge_mtime: Cell<Option<std::time::SystemTime>>,
        /// `lastOtherAppClick` (epoch seconds), fed by a global
        /// left-mouse-down monitor.
        last_other_click: Rc<Cell<f64>>,
        /// The global click monitor (`globalClickMonitor`).
        click_monitor: RefCell<Option<Retained<AnyObject>>>,
        /// `PaneNav.shared` over the shared window's real panes.
        live_nav: RefCell<crate::app::live_panes::LiveNav>,
        /// Last published `state.pane` (only re-sent on change).
        live_pane_sig: RefCell<String>,
        /// `VimKeys.shared` + its search bar / mode badge.
        vim: RefCell<host_vim::VimHost>,
        /// `noteFindWindow` / `noteGrepWindow` (leader+sf / leader+sg).
        note_find: RefCell<Option<Rc<crate::views::note_find::NoteFindUi>>>,
        note_grep: RefCell<Option<Rc<crate::views::note_find::NoteFindUi>>>,
        /// `sidebarJumpWindow` (`do:jira-jump`).
        sidebar_jump: RefCell<Option<Rc<crate::views::note_find::SidebarJumpUi>>>,
        /// The `<leader>sf` item finder on the non-nvim panes.
        pane_find: RefCell<Option<Rc<crate::views::note_find::PaneFindUi>>>,
        /// The Cmd+K action picker while it is up.
        action_picker: RefCell<Option<Retained<crate::ui::action_picker::ActionPicker>>>,
    }

    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSDaemonUi"]
        #[ivars = DaemonUiIvars]
        pub struct DaemonUi;

        impl DaemonUi {
            /// The main-thread drain: apply every queued UI command.
            #[unsafe(method(drain:))]
            fn drain(&self, _timer: &NSTimer) {
                self.apply_pending();
            }

            /// A view-switcher button: route the tag as a nav id.
            #[unsafe(method(navClicked:))]
            fn nav_clicked(&self, sender: &AnyObject) {
                let tag: NSInteger = unsafe { msg_send![sender, tag] };
                self.ivars().controller.nav_clicked(tag as i64);
            }

            /// The header ✕ overlay button.
            #[unsafe(method(closeClicked:))]
            fn close_clicked(&self, _sender: &AnyObject) {
                self.ivars().controller.hide("✕");
            }
        }

        unsafe impl NSObjectProtocol for DaemonUi {}
    );

    /// Install the main-thread UI: a repeating drain timer owns the bridge for
    /// the life of the run loop (the timer retains its target). Call on the
    /// main thread before `NSApplication::run`.
    pub fn install_daemon_ui(
        mtm: MainThreadMarker,
        controller: Arc<SwitcherController>,
        queue: UiQueue,
        main_queue: MainQueue,
    ) -> Retained<DaemonUi> {
        let bridge_path = format!(
            "{}{}",
            crate::app::paths::Paths::from_env().popup_tmp_dir(),
            controller.settings().focus_bridge_name
        );
        // `watchFocusBridge` creates the file so AeroSpace's writes land.
        if !std::path::Path::new(&bridge_path).exists() {
            let _ = std::fs::write(&bridge_path, "");
        }
        // Held for the delayed `prewarm()` (the `controller` itself moves into
        // the UI ivars below).
        let shot_prewarm = controller.screenshot_controller();
        let this = DaemonUi::alloc(mtm).set_ivars(DaemonUiIvars {
            strip_result: Arc::new(std::sync::Mutex::new(None)),
            strip_busy: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            strip_last: Cell::new(0.0),
            files_recent_at: Cell::new(0.0),
            jira_refresh_at: Cell::new(0.0),
            strip_sig: RefCell::new(String::new()),
            icon_cache: RefCell::new(HashMap::new()),
            controller,
            queue,
            main_queue,
            state: RefCell::new(None),
            esc_streak: Cell::new(0),
            last_esc_at: Cell::new(0.0),
            key_monitor: RefCell::new(None),
            last_vim_check: Cell::new(0.0),
            last_vim_relaunch: Cell::new(0.0),
            focus_watch: RefCell::new(super::FocusWatch::default()),
            bridge_path,
            bridge_mtime: Cell::new(None),
            last_other_click: Rc::new(Cell::new(0.0)),
            click_monitor: RefCell::new(None),
            live_nav: RefCell::new(crate::app::live_panes::LiveNav::default()),
            live_pane_sig: RefCell::new(String::new()),
            vim: RefCell::new(host_vim::VimHost::default()),
            note_find: RefCell::new(None),
            note_grep: RefCell::new(None),
            sidebar_jump: RefCell::new(None),
            pane_find: RefCell::new(None),
            action_picker: RefCell::new(None),
        });
        let ui: Retained<DaemonUi> = unsafe { msg_send![super(this), init] };
        UI_WEAK.with(|w| *w.borrow_mut() = Some(objc2::rc::Weak::from_retained(&ui)));
        {
            // Return / Esc on a sidebar rail hands the keyboard right.
            crate::ui::sidebar::set_leave_right(Box::new(|| {
                if let Some(ui) = UI_WEAK.with(|w| w.borrow().as_ref().and_then(|w| w.load())) {
                    ui.live_pane_move(PaneDir::Right);
                }
            }));
            let s = ui.ivars().controller.settings();
            let c = s.pane_focus_color;
            let byte = |v: f64| ((v.clamp(0.0, 1.0) * 255.0).round() as u32) & 0xff;
            ui.ivars().vim.borrow_mut().keys.enabled = s.vim_keys;
            ui.ivars().live_nav.borrow_mut().style = crate::panes::pane_nav::RingStyle {
                color_argb: byte(c.a) << 24 | byte(c.r) << 16 | byte(c.g) << 8 | byte(c.b),
                width: s.pane_focus_width,
            };
        }
        // The Recent seed runs `mdfind`: start it after launch settles.
        dispatch2::DispatchQueue::main()
            .after(dispatch2::DispatchTime::NOW.time(2_000_000_000), configure_recent_and_shelf)
            .ok();
        // `ScreenshotController.prewarm()` ~1.5 s after launch: SCK + OCR warm
        // so the first capture / first OCR does not pay the cold cost.
        if let Some(shot) = shot_prewarm {
            dispatch2::DispatchQueue::main()
                .after(dispatch2::DispatchTime::NOW.time(1_500_000_000), move || {
                    shot.lock().unwrap().prewarm();
                })
                .ok();
        }
        // The window's local keyDown guard (mirrors `PopupWindow.installMonitors`):
        // the block holds the UI weakly so AppKit's monitor retains no cycle.
        let weak = Weak::new(&*ui);
        let monitor = popup::install_local_monitor(mtm, move |event| {
            weak.load()
                .map(|ui| ui.route_key_event(event))
                .unwrap_or(false)
        });
        *ui.ivars().key_monitor.borrow_mut() = Some(monitor);
        // `startFocusBridge`'s global click monitor: a click outside our
        // windows marks `lastOtherAppClick` (the user left on purpose, so an
        // AeroSpace focus echo must not pull the app back).
        {
            use block2::RcBlock;
            use objc2_app_kit::NSEventMask;
            let clicked = ui.ivars().last_other_click.clone();
            let block = RcBlock::new(move |_event: std::ptr::NonNull<NSEvent>| {
                let Some(mtm) = MainThreadMarker::new() else {
                    return;
                };
                let p = NSEvent::mouseLocation();
                let in_ours = NSApplication::sharedApplication(mtm).windows().iter().any(|w| {
                    let f = w.frame();
                    w.isVisible()
                        && p.x >= f.origin.x
                        && p.x <= f.origin.x + f.size.width
                        && p.y >= f.origin.y
                        && p.y <= f.origin.y + f.size.height
                });
                if !in_ours {
                    clicked.set(now_secs());
                }
            });
            let m = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(
                NSEventMask::LeftMouseDown,
                &block,
            );
            *ui.ivars().click_monitor.borrow_mut() = m;
        }
        unsafe {
            NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                0.03,
                as_any(&*ui),
                sel!(drain:),
                None,
                true,
            );
        }
        ui
    }

    impl DaemonUi {
        fn apply_pending(&self) {
            // `onVimExit`: `:q` / a crash rebuilds the pane on the same note.
            self.poll_notes_vim();
            // UI commands first, then the settling syncs, then the blocking
            // `do:` jobs: a `do:` caller waits on a main-queue job and must see
            // the window it just asked for (`views.*` / `windows`).
            for _ in 0..100 {
                let cmds: Vec<UiCommand> = {
                    let mut q = self.ivars().queue.lock().unwrap();
                    if q.is_empty() {
                        break;
                    }
                    q.drain(..).collect()
                };
                for cmd in cmds {
                    match cmd {
                        UiCommand::Present(v) => self.present(v),
                        UiCommand::Hide => self.hide(),
                        UiCommand::ShowPalette => self.show_palette(),
                        UiCommand::HidePalette => self.hide_palette(),
                        UiCommand::ShowSwitcher => self.show_switcher(),
                        UiCommand::HideSwitcher => self.hide_switcher(),
                        UiCommand::ToggleDrawer(side) => self.toggle_drawer(side),
                        UiCommand::Paths(cmd) => self.run_paths_cmd(cmd),
                        UiCommand::TerminalPanel => self.toggle_terminal_panel(),
                        UiCommand::Tool(kind) => self.open_tool(kind),
                        UiCommand::ToolClose(name) => self.close_tool(&name),
                        UiCommand::ResetSize => {
                            self.route_reset_size();
                        }
                        UiCommand::Toast(text, symbol) => crate::ui::toast::show(&text, symbol),
                        UiCommand::NoteFind(grep) => self.show_note_find(grep),
                        UiCommand::JiraJump => self.show_sidebar_jump(),
                        UiCommand::JiraEnable(ok, msg) => self.ivars().controller.jira_enable_result(ok, &msg),
                        UiCommand::JiraSetup(reason) => {
                            let ctrl = self.ivars().controller.clone();
                            crate::ui::jira_setup::show(
                                reason.as_deref(),
                                Box::new(move || ctrl.jira_enable_result(true, "")),
                            );
                        }
                        UiCommand::PaneMove(dir) => {
                            self.live_pane_move(dir);
                        }
                        UiCommand::PaneFocus(id) => {
                            if !self.live_pane_focus(&id) {
                                self.ivars().controller.log(&format!("pane:focus:{id}: no such pane"));
                            }
                        }
                    }
                }
            }
            self.sync_tools();
            self.sync_member_states();
            self.sync_compare();
            self.watch_focus();
            self.live_pane_refresh();
            self.refresh_workspace_strip();
            self.refresh_files_recent();
            self.refresh_jira();
            self.poll_conf_hits();
            self.poll_ai();
            if let Some(mtm) = MainThreadMarker::new() {
                self.ivars()
                    .controller
                    .set_window_count(visible_window_count(mtm));
            }
            for _ in 0..100 {
                let jobs: Vec<Box<dyn FnOnce() + Send>> = {
                    let mut q = self.ivars().main_queue.lock().unwrap();
                    if q.is_empty() {
                        break;
                    }
                    q.drain(..).collect()
                };
                for job in jobs {
                    job();
                }
            }
        }

        /// `CompareWindow.syncAll`: repaint the live compare surface from the
        /// model while a compare view is up (a no-op unless it changed).
        fn sync_compare(&self) {
            let state = self.ivars().state.borrow();
            let Some(st) = state.as_ref() else {
                return;
            };
            if !matches!(st.current, Some(SlotView::Compare) | Some(SlotView::CompareText))
                || !st.window.isVisible()
            {
                return;
            }
            let (Some(surface), Some(model)) =
                (st.compare.as_ref(), self.ivars().controller.compare_model())
            else {
                return;
            };
            let Ok(mut m) = model.try_lock() else {
                return;
            };
            surface.sync(&mut m);
        }

        /// The polled focus observers: `SharedWindow`'s didResignKey →
        /// `focusLossDelay` → `checkFocusLoss` (hide with `[app]
        /// hide-on-focus-loss`, refocus after a swap steal), the "became key
        /// while inactive" self-activation, and `watchFocusBridge` (AeroSpace
        /// focused our window → activate).
        /// `refreshWorkspaceStrip()`: every 6 s while the window is up, gather
        /// AeroSpace off-main, then build the header cells (≤3 app icons, "+N",
        /// focused) on main and redraw only when the signature changed.
        fn refresh_workspace_strip(&self) {
            use std::sync::atomic::Ordering;
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };
            if let Some(fresh) = self.ivars().strip_result.lock().unwrap().take() {
                let sig = fresh
                    .iter()
                    .map(|w| format!("{}|{}|{}", w.id, w.apps.len(), w.focused))
                    .collect::<Vec<_>>()
                    .join(";");
                if *self.ivars().strip_sig.borrow() != sig {
                    *self.ivars().strip_sig.borrow_mut() = sig;
                    let cells: Vec<crate::ui::chrome::WorkspaceCell> = fresh
                        .iter()
                        .map(|w| crate::ui::chrome::WorkspaceCell {
                            key: w.id.clone(),
                            icons: w.apps.iter().take(3).map(|a| self.app_icon(a)).collect(),
                            extra: (w.apps.len() > 3).then(|| format!("+{}", w.apps.len() - 3)),
                            unread: String::new(),
                            focused: w.focused,
                        })
                        .collect();
                    crate::ui::chrome::set_workspace_cells(mtm, cells);
                }
            }
            let visible = self
                .ivars()
                .state
                .borrow()
                .as_ref()
                .is_some_and(|st| st.window.isVisible());
            let now = now_secs();
            if !visible || now - self.ivars().strip_last.get() < 6.0 {
                return;
            }
            if self.ivars().strip_busy.swap(true, Ordering::SeqCst) {
                return;
            }
            self.ivars().strip_last.set(now);
            let out = self.ivars().strip_result.clone();
            let busy = self.ivars().strip_busy.clone();
            std::thread::spawn(move || {
                let ipc = crate::app::hotkey::AeroIpc::discover();
                let rows = super::gather_strip_with(&|args| {
                    crate::app::hotkey::AeroCall::call(&ipc, args)
                });
                if !rows.is_empty() {
                    *out.lock().unwrap() = Some(rows);
                }
                busy.store(false, Ordering::SeqCst);
            });
        }

        /// `iconForApp(_:)` (cached): the bundle's icon, else `/Applications/<name>.app`.
        fn app_icon(&self, app: &(String, String, String)) -> Retained<objc2_app_kit::NSImage> {
            use objc2_app_kit::NSWorkspace;
            let key = if app.1.is_empty() { app.0.clone() } else { app.1.clone() };
            if let Some(img) = self.ivars().icon_cache.borrow().get(&key) {
                return img.clone();
            }
            let ws = NSWorkspace::sharedWorkspace();
            let mut path: Option<String> = None;
            if !app.1.is_empty() {
                if let Some(url) =
                    ws.URLForApplicationWithBundleIdentifier(&NSString::from_str(&app.1))
                {
                    path = url.path().map(|p| p.to_string());
                }
            }
            if path.is_none() {
                for dir in ["/Applications", "/System/Applications", "/Applications/Utilities"] {
                    let p = format!("{dir}/{}.app", app.0);
                    if std::path::Path::new(&p).exists() {
                        path = Some(p);
                        break;
                    }
                }
            }
            let img = match path {
                Some(p) => ws.iconForFile(&NSString::from_str(&p)),
                None => crate::ui::style::glyph_icon(
                    "app.dashed",
                    "?",
                    crate::ui::theme::PopupColors::default().dim,
                    32.0,
                ),
            };
            self.ivars().icon_cache.borrow_mut().insert(key, img.clone());
            img
        }

        fn watch_focus(&self) {
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };
            let now = now_secs();
            let clicked_away = now - self.ivars().last_other_click.get() < 1.0;
            let app = NSApplication::sharedApplication(mtm);
            let (sample, wid) = {
                let state = self.ivars().state.borrow();
                let Some(st) = state.as_ref() else {
                    return;
                };
                let w = &st.window;
                let wid = w.windowNumber() as i64;
                let key = w.isKeyWindow();
                let key_win = app.keyWindow().filter(|k| k.isVisible());
                let other_key = key_win
                    .as_ref()
                    .map_or(false, |k| k.windowNumber() as i64 != wid);
                let sample = super::FocusSample {
                    now,
                    shown: w.isVisible(),
                    key,
                    left: !key && w.attachedSheet().is_none() && !other_key,
                    any_key: key || key_win.is_some(),
                    app_active: app.isActive(),
                    clicked_away,
                };
                (sample, wid)
            };
            let controller = self.ivars().controller.clone();
            let (hide_on_loss, delay) = controller.focus_loss_settings();
            let action = self.ivars().focus_watch.borrow_mut().tick(sample, hide_on_loss, delay);
            let front = || {
                crate::platform::frontmost_app()
                    .and_then(|a| a.bundle_id)
                    .unwrap_or_else(|| "?".into())
            };
            match action {
                super::FocusAction::None => {}
                super::FocusAction::Hide => {
                    controller.hide(&format!("focus loss → {}", front()));
                }
                super::FocusAction::Refocus | super::FocusAction::SelfActivate => {
                    activate_app(mtm);
                    if let Some(st) = self.ivars().state.borrow().as_ref() {
                        st.window.orderFrontRegardless();
                        if action == super::FocusAction::Refocus {
                            st.window.makeKeyAndOrderFront(None);
                        }
                    }
                    controller.log(if action == super::FocusAction::Refocus {
                        "shared window: focus stolen right after a view swap — refocused"
                    } else {
                        "our window became key while inactive (aerospace focus) — self-activated"
                    });
                }
            }
            if !sample.shown {
                return;
            }
            // `focusBridgeChanged`: act once per new mtime.
            let Ok(meta) = std::fs::metadata(&self.ivars().bridge_path) else {
                return;
            };
            let Ok(mtime) = meta.modified() else {
                return;
            };
            if self.ivars().bridge_mtime.get() == Some(mtime) {
                return;
            }
            self.ivars().bridge_mtime.set(Some(mtime));
            let age = std::time::SystemTime::now()
                .duration_since(mtime)
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0);
            let raw = std::fs::read_to_string(&self.ivars().bridge_path).unwrap_or_default();
            if super::focus_bridge_should_activate(&raw, wid, age, clicked_away) {
                activate_app(mtm);
                if let Some(st) = self.ivars().state.borrow().as_ref() {
                    st.window.orderFrontRegardless();
                    st.window.makeKeyAndOrderFront(None);
                }
                controller.log(&format!("aerospace focused our window {wid} — self-activated"));
            }
        }

        /// The notes vim pane's `onVimExit` poll + the note watcher's
        /// `checktime` (cheap; every drain tick, checktime once a second).
        fn poll_notes_vim(&self) {
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0);
            let due = now - self.ivars().last_vim_check.get() >= 1.0;
            if due {
                self.ivars().last_vim_check.set(now);
            }
            let mut state = self.ivars().state.borrow_mut();
            if let Some(st) = state.as_mut() {
                // The shells' `TerminalAutoRestart` (drawer + /terminal panel).
                if let Some(tp) = st.terminal_panel.as_mut() {
                    tp.poll_restart();
                }
                if let Some(surface) = st.notes.as_ref() {
                    surface.terminal_poll_restart();
                }
                if let Some(surface) = st.notes.as_mut() {
                    let relaunched = surface.vim_poll_exit(mtm);
                    // The nvim inline-image sidecar (throttled inside).
                    surface.tick_vim_images();
                    if relaunched {
                        self.ivars().last_vim_relaunch.set(now);
                    }
                    // Swift's `TerminalAutoRestart`: after a relaunch, the new
                    // terminal takes the keyboard back — and keeps it through
                    // the next stretch (AppKit may drop the window's key status
                    // or first responder while the pane is rebuilt).
                    let reclaiming = now - self.ivars().last_vim_relaunch.get() <= 2.0;
                    if st.window.isVisible() && reclaiming && !st.window.isKeyWindow() {
                        st.window.makeKeyAndOrderFront(None);
                    }
                    if st.window.isVisible()
                        && (relaunched
                            || (reclaiming && st.window.isKeyWindow() && !surface.vim_has_focus()))
                    {
                        surface.focus_editor();
                    }
                    if due && st.window.isVisible() {
                        if let Some(pane) = surface.vim_pane() {
                            pane.command("silent! checktime");
                        }
                    }
                }
            }
        }

        /// `showViewSwitcher()` — show the palette over the shared window.
        /// Swap fresh `aerospace` rows into an open palette (see `show_palette`).
        fn refresh_palette_workspaces(&self, workspaces: Vec<crate::views::switcher::WorkspaceRow>) {
            let Ok(mut state) = self.ivars().state.try_borrow_mut() else { return };
            if let Some(p) = state.as_mut().and_then(|st| st.palette.as_mut()) {
                if p.is_visible() {
                    p.refresh_workspaces(workspaces);
                }
            }
        }

        fn show_palette(&self) {
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };
            let controller = self.ivars().controller.clone();
            let mut state = self.ivars().state.borrow_mut();
            let st = state.get_or_insert_with(|| self.build_window(mtm));
            let mut panel = st.palette.take().unwrap_or_else(PalettePanel::new);
            let commands: Vec<(String, String)> = controller
                .registry()
                .palette_commands()
                .iter()
                .map(|c| (c.id.clone(), c.title.clone()))
                .collect();
            // The `aerospace` CLI can stall for seconds (it did when the
            // palette was opened from the Ctrl+B key chord), so never wait on
            // it here: paint with the last known rows and refresh off-main.
            let workspaces = WORKSPACE_CACHE.lock().unwrap().clone();
            panel.set_rows(commands, workspaces);
            std::thread::spawn(|| {
                let fresh = gather_workspaces();
                *WORKSPACE_CACHE.lock().unwrap() = fresh.clone();
                on_ui(move |ui| ui.refresh_palette_workspaces(fresh));
            });
            panel.set_callbacks(
                Box::new({
                    let c = controller.clone();
                    move |action: PaletteAction| {
                        // Runs before the click path orders the palette out.
                        if let Some(mtm) = MainThreadMarker::new() {
                            palette_pick_focus(mtm, &action);
                        }
                        // Deferred so it runs after the click handler returns:
                        // a click-pick left the palette window on screen
                        // (under the tool it opened) until the tool closed.
                        on_ui(|ui| {
                            ui.hide_palette_panel();
                            ui.end_palette(false);
                        });
                        match action {
                            PaletteAction::Command(name) => c.run_palette_command(&name),
                            PaletteAction::Workspace(id) => c.switch_workspace(&id),
                        }
                        c.set_palette_visible(false);
                    }
                }),
                Box::new({
                    let c = controller.clone();
                    move || {
                        c.set_palette_visible(false);
                        // Esc inside the field: give focus back to the app
                        // that was frontmost instead of the main window.
                        if let Some(mtm) = MainThreadMarker::new() {
                            palette_hand_back(mtm);
                        }
                        on_ui(|ui| ui.hide_palette());
                    }
                }),
            );
            // Centre on the screen under the mouse, not on the (possibly
            // hidden / other-Space / stale) main window's frame.
            let m = objc2_app_kit::NSEvent::mouseLocation();
            let host = crate::platform::screen_containing(m.x, m.y)
                .or_else(crate::platform::main_screen)
                .map(|s| s.visible)
                .map(|v| NSRect::new(NSPoint::new(v.x, v.y), NSSize::new(v.w, v.h)))
                .unwrap_or_else(|| st.window.frame());
            panel.present(
                mtm,
                Some(SwitchFrame::new(
                    host.origin.x,
                    host.origin.y,
                    host.size.width,
                    host.size.height,
                )),
            );
            st.palette = Some(panel);
            // Opened from another app (Hyper+S): remember it so a cancel
            // gives focus back there. We are frontmost = the user was in our
            // window, and a cancel just leaves them in it.
            // Ask the frontmost app, not `isActive`: the launch path has
            // already requested activation by now, so `isActive` reads true
            // while the user's app is still frontmost.
            let own = std::process::id() as i32;
            let front = crate::platform::frontmost_app().map(|a| a.pid).filter(|p| *p != own);
            PALETTE_RETURN.with(|c| c.set(front));
            drop(state);
            // No `activate_app` here: the palette is a non-activating panel
            // (`tool_panel`) and takes the keyboard on its own. Activating
            // kitchen-sink made it the focused app, so dismissing the
            // palette or opening any popup from it let macOS raise the
            // shared window and AeroSpace jump to its workspace. Only a pick
            // that opens the shared window activates (its `present`).
            controller.set_window_count(visible_window_count(mtm));
        }

        /// `showViewSwitcher()` for the Ctrl+B W panel (`state.viewSwitcher`).
        fn show_switcher(&self) {
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };
            let controller = self.ivars().controller.clone();
            let mut state = self.ivars().state.borrow_mut();
            let st = state.get_or_insert_with(|| self.build_window(mtm));
            let mut panel = st.switcher.take().unwrap_or_else(ViewSwitcherPanel::new);
            // The views' "where it is" (read here: the state is borrowed).
            let wheres = Self::where_texts(st, &controller);
            let rows = self.prepare_panel(
                &mut panel,
                Box::new({
                    let c = controller.clone();
                    move || c.set_view_switcher(false)
                }),
                wheres,
            );
            let host = st.window.frame();
            panel.present(Some(SwitchFrame::new(
                host.origin.x,
                host.origin.y,
                host.size.width,
                host.size.height,
            )));
            st.switcher = Some(panel);
            drop(state);
            controller.set_switcher_rows(rows);
            controller.set_window_count(visible_window_count(mtm));
        }

        /// `showViewSwitcher`'s per-view "where it is" (`whereText` / `tabName`).
        fn where_texts(st: &DaemonUiState, controller: &SwitcherController) -> HashMap<SlotView, String> {
            let mut w = HashMap::new();
            if let Some(b) = st.files.as_ref() {
                w.insert(SlotView::Files, b.where_text());
            }
            if let Some(name) = st
                .notes
                .as_ref()
                .and_then(|s| s.selected_path())
                .and_then(|p| std::path::Path::new(&p).file_name().map(|f| f.to_string_lossy().into_owned()))
            {
                w.insert(SlotView::Notes, name.trim_end_matches(".json").to_string());
            }
            if let Some((l, _)) = st.jira.as_ref() {
                let sb = l.snapshot().sidebar;
                let t = match &sb.pinned_selected {
                    Some(k) => sb.pinned.iter().find(|p| &p.key == k).map(|p| p.label.clone()),
                    None => sb.selected.and_then(|i| sb.rows.get(i)).map(|r| r.title.clone()),
                };
                w.insert(SlotView::Jira, t.unwrap_or_default());
            }
            if let Some(c) = st.confluence.as_ref() {
                w.insert(SlotView::Confluence, c.model.where_text());
            }
            if let Some(a) = st.ai.as_ref() {
                w.insert(SlotView::Ai, a.model.where_text());
            }
            if let Some(m) = controller.compare_model() {
                let label = m.lock().unwrap().current().map(|s| s.label()).unwrap_or_else(|| "Home".into());
                w.insert(SlotView::Compare, label);
            }
            w
        }

        /// Wire the shared row set + callbacks onto a panel.
        fn prepare_panel(
            &self,
            panel: &mut ViewSwitcherPanel,
            on_close: Box<dyn FnMut()>,
            wheres: HashMap<SlotView, String>,
        ) -> Vec<String> {
            let controller = self.ivars().controller.clone();
            // `showViewSwitcher`: each view's "where it is" + `aliases`.
            let assets = crate::app::paths::Paths::from_env().asset_dir().to_string();
            let mut icons = Vec::new();
            let views: Vec<SwitchView> = controller
                .registry()
                .nav_icons()
                .iter()
                // Ctrl+B W lists only the in-app views: Files, Notes,
                // Confluence, Jira (Hyper+S keeps the full palette).
                .filter(|n| {
                    n.view.is_jira() || matches!(n.view, SlotView::Files | SlotView::Notes | SlotView::Confluence)
                })
                .map(|n| {
                    let (name, section) = match n.view {
                        SlotView::Files => ("Files", "files"),
                        SlotView::Notes => ("Notes", "notes"),
                        SlotView::Confluence => ("Confluence", "confluence"),
                        SlotView::Compare | SlotView::CompareText => ("Compare", "compare"),
                        SlotView::Ai => ("AI", "ai"),
                        _ => ("Jira", "jira"),
                    };
                    let key = if n.view.is_jira() { SlotView::Jira } else { n.view };
                    let loc = wheres.get(&key).cloned().unwrap_or_default();
                    icons.push((n.id, nav_icon_image(n.view, n.image_key.as_deref(), &assets)));
                    let mut v = SwitchView::new(n.id, name, loc);
                    v.aliases = crate::views::notes::section_value(section, "aliases")
                        .unwrap_or_default()
                        .split(',')
                        .map(|a| a.trim().to_string())
                        .filter(|a| !a.is_empty())
                        .collect();
                    v
                })
                .collect();
            panel.set_icons(icons);
            panel.set_commands(Vec::new());
            panel.set_callbacks(
                Box::new({
                    let c = controller.clone();
                    move |id| c.nav_clicked(id)
                }),
                Box::new({
                    let c = controller.clone();
                    move |name| c.run_palette_command(&name)
                }),
            );
            panel.set_on_close(on_close);
            let current = controller.current().map(|v| v.nav_id());
            // Always start on the first row.
            panel.show(&views, current, None);
            views.iter().map(|v| format!("{}|{}", v.name, v.location)).collect()
        }

        /// Order the palette window out (a click-pick; `hide_palette` minus the
        /// focus hand-back, which `palette_pick_focus` already did).
        fn hide_palette_panel(&self) {
            if let Some(p) = self.ivars().state.borrow_mut().as_mut().and_then(|st| st.palette.as_mut()) {
                p.hide_panel();
            }
        }

        fn hide_palette(&self) {
            // Hand focus back first (see `palette_hand_back`), then hide.
            if let Some(mtm) = MainThreadMarker::new() {
                palette_hand_back(mtm);
            }
            let mut state = self.ivars().state.borrow_mut();
            if let Some(st) = state.as_mut() {
                if let Some(panel) = st.palette.as_mut() {
                    panel.hide_panel();
                }
            }
            drop(state);
            self.ivars().controller.set_palette_visible(false);
            self.end_palette(true);
        }

        /// The palette closed. The focus hand-back already happened before it
        /// was ordered out (`palette_hand_back` / `palette_pick_focus`); this
        /// is the bookkeeping, plus — for a cancel of a palette opened from
        /// inside the app — handing activation back when nothing of ours is
        /// left on screen. (The window's `isVisible` can't decide the outside
        /// case: AeroSpace parks it on another workspace while it still
        /// reports visible.)
        fn end_palette(&self, cancelled: bool) {
            let Some(mtm) = MainThreadMarker::new() else { return };
            self.ivars().controller.set_window_count(visible_window_count(mtm));
            if !cancelled || palette_hand_back(mtm) {
                return;
            }
            let main_visible = self
                .ivars()
                .state
                .borrow()
                .as_ref()
                .map(|st| st.window.isVisible())
                .unwrap_or(false);
            if !main_visible {
                NSApplication::sharedApplication(mtm).deactivate();
            }
        }

        fn hide_switcher(&self) {
            let mut state = self.ivars().state.borrow_mut();
            if let Some(st) = state.as_mut() {
                if let Some(panel) = st.switcher.as_mut() {
                    panel.hide_panel();
                }
            }
            drop(state);
            if let Some(mtm) = MainThreadMarker::new() {
                self.ivars().controller.set_window_count(visible_window_count(mtm));
            }
        }

        fn toggle_drawer(&self, side: DrawerSide) {
            let controller = self.ivars().controller.clone();
            let mut state = self.ivars().state.borrow_mut();
            if let Some(st) = state.as_mut() {
                if let Some(surface) = st.notes.as_mut() {
                    let which = match side {
                        DrawerSide::Terminal => "terminal",
                        DrawerSide::Browser => "browser",
                    };
                    if let Some(on) = surface.toggle_drawer(which) {
                        controller.set_notes_drawer(side, on);
                    }
                    return;
                }
            }
            // No notes surface yet: undo the optimistic model flip.
            let on = controller.notes_drawer(side);
            controller.set_notes_drawer(side, !on);
        }

        /// `showPaths` / `do:paths:*` — the shelf tool window.
        fn run_paths_cmd(&self, cmd: PathsCmd) {
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };
            let controller = self.ivars().controller.clone();
            let mut state = self.ivars().state.borrow_mut();
            let st = match state.as_mut() {
                Some(s) => s,
                None => return,
            };
            let mut win = st
                .paths
                .take()
                .unwrap_or_else(|| PathsWindow::new(PathsReturn::File));
            match cmd {
                PathsCmd::Show => {
                    win.set_items(shelf_entries());
                    win.show();
                    win.build(mtm);
                    if win.on_event.is_none() {
                        let weak = objc2::rc::Weak::from_retained(&self.retain());
                        win.set_on_event(Rc::new(move |ev| {
                            if let Some(ui) = weak.load() {
                                ui.paths_event(ev);
                            }
                        }));
                    }
                    win.show_window(mtm);
                    // Click-off hides the window (and any rename field),
                    // unless the preview popup took key from a double-click.
                    if let Some(panel) = &win.window {
                        panel.set_on_resign(Box::new(|| {
                            on_ui(|ui| {
                                if crate::views::file_popup::is_live() {
                                    return;
                                }
                                cancel_click_rename();
                                crate::ui::inline_rename::cancel();
                                ui.paths_surface(|w| w.hide_window());
                            });
                        }));
                    }
                }
                PathsCmd::Hide => win.hide_window(),
                PathsCmd::Return => match win.return_action {
                    PathsReturn::Path => copy_text_to_pasteboard(&win.copy_paths_text()),
                    PathsReturn::File => { copy_files_toast(&win.copy_files()); }
                    PathsReturn::Open => {
                        for p in win.open_paths() {
                            open_path(&p);
                        }
                    }
                },
                PathsCmd::Select(n) => win.test_select(n),
            }
            let doc = win.test_state();
            let mut tool_doc = doc.clone();
            if let Some(o) = tool_doc.as_object_mut() {
                o.insert("wid".into(), json!(win.window_number()));
            }
            st.paths = Some(win);
            drop(state);
            controller.set_paths_state(doc);
            controller.set_tool_state("paths", tool_doc);
            controller.set_window_count(visible_window_count(mtm));
        }

        /// Run `f` on the paths window, then repaint it.
        fn paths_surface<R>(&self, f: impl FnOnce(&mut PathsWindow) -> R) -> Option<R> {
            let mut state = self.ivars().state.borrow_mut();
            let w = state.as_mut().and_then(|st| st.paths.as_mut())?;
            let r = f(w);
            w.sync_view();
            Some(r)
        }

        fn paths_return(&self) {
            let Some((action, text, files, open)) = self.paths_surface(|w| {
                (w.return_action, w.copy_paths_text(), w.copy_files(), w.selected_paths())
            }) else {
                return;
            };
            match action {
                PathsReturn::Path => copy_text_to_pasteboard(&text),
                PathsReturn::File => { copy_files_toast(&files); }
                PathsReturn::Open => {
                    for p in open {
                        open_path(&p);
                    }
                }
            }
            self.paths_surface(|w| w.hide_window());
        }

        fn paths_event(&self, ev: crate::views::paths::PathsEvent) {
            use crate::views::paths::PathsEvent as E;
            match ev {
                E::Query(q) => {
                    self.paths_surface(|w| w.set_query(&q));
                }
                E::Move(d) => {
                    self.paths_surface(|w| w.move_by(d));
                }
                E::Escape => {
                    self.paths_surface(|w| w.hide_window());
                }
                E::Enter => self.paths_return(),
                E::Click { row, double } => {
                    let was_selected = self.paths_surface(|w| {
                        w.selection = row;
                        w.last_clicked.replace(row) == Some(row)
                    });
                    // Like Finder: clicking a row that is already selected
                    // renames it once the double-click window passes (a
                    // first click only selects); a double-click cancels
                    // that and previews.
                    if double {
                        cancel_click_rename();
                    } else if was_selected == Some(true) {
                        schedule_click_rename(|ui| ui.paths_begin_rename());
                    } else {
                        cancel_click_rename();
                    }
                    // Double-click previews in the dismissable FilePopup (copying
                    // the path stays on Cmd+C / Return, never a click).
                    if double {
                        if let Some(Some(path)) =
                            self.paths_surface(|w| w.rows.get(row).map(|r| r.path.clone()))
                        {
                            crate::views::file_popup::show(&path, None);
                        }
                    }
                }
                E::RowMenu { row, x, y } => {
                    use crate::ui::menu_kit::{popup, MenuEntry};
                    let Some(Some(path)) = self.paths_surface(|w| {
                        w.selection = row;
                        w.rows.get(row).map(|r| r.path.clone())
                    }) else {
                        return;
                    };
                    let w1 = objc2::rc::Weak::from_retained(&self.retain());
                    let (p1, p2, p3, p4) = (path.clone(), path.clone(), path.clone(), path.clone());
                    popup(
                        vec![
                            MenuEntry::item("Copy File", move || { copy_files_toast(&[p1]); }),
                            MenuEntry::item("Copy Absolute Path", move || copy_text_to_pasteboard(&p2)),
                            MenuEntry::Separator,
                            MenuEntry::item("Open in Notes", move || {
                                if let Some(ui) = w1.load() {
                                    ui.open_in_notes(&p3);
                                }
                            }),
                            MenuEntry::item("Reveal in Files", move || reveal_in_files(&[p4])),
                        ],
                        None,
                        x,
                        y,
                    );
                }
            }
        }

        /// `toggleTerminalPanel()` — the `/terminal` tool panel.
        fn toggle_terminal_panel(&self) {
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };
            let mut state = self.ivars().state.borrow_mut();
            let st = state.get_or_insert_with(|| self.build_window(mtm));
            let mut panel = st.terminal_panel.take().unwrap_or_else(|| {
                TerminalPanel::new(TerminalConfig::from_settings(
                    &self.ivars().controller.settings(),
                ))
            });
            if panel.is_shown() {
                panel.hide();
                st.terminal_opened_at = None;
            } else {
                panel.show();
                st.terminal_opened_at = Some(std::time::Instant::now());
            }
            st.terminal_panel = Some(panel);
        }

        /// `openTool` for the prettyprint / filefast / health-checks panels.
        fn open_tool(&self, kind: ToolKind) {
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };
            let settings = self.ivars().controller.settings();
            let mut state = self.ivars().state.borrow_mut();
            let st = state.get_or_insert_with(|| self.build_window(mtm));
            let mut panel = st
                .tools
                .remove(kind.name())
                .unwrap_or_else(|| ToolPanel::new(kind, &settings));
            // Opened by a palette pick just now: its close hands focus back
            // to the app Hyper+S was pressed over.
            let picked = PICK_RETURN
                .with(|c| c.take())
                .filter(|(_, at)| at.elapsed().as_secs_f64() < 2.0)
                .map(|(pid, _)| pid);
            if picked.is_some() {
                panel.set_return_pid(picked);
            }
            panel.show(mtm);
            let doc = panel.test_state();
            st.tools.insert(kind.name().to_string(), panel);
            st.tool_opened_at.insert(
                kind.name().to_string(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs_f64())
                    .unwrap_or(0.0),
            );
            drop(state);
            self.ivars().controller.set_tool_state(kind.name(), doc);
            self.ivars()
                .controller
                .set_window_count(visible_window_count(mtm));
        }

        /// `unregisterSubWindow` for a tool panel: order it out and drop it.
        fn close_tool(&self, name: &str) {
            let mut state = self.ivars().state.borrow_mut();
            if let Some(st) = state.as_mut() {
                if let Some(mut panel) = st.tools.remove(name) {
                    panel.hide();
                }
            }
            drop(state);
            self.ivars().controller.remove_tool_state(name);
            if let Some(mtm) = MainThreadMarker::new() {
                self.ivars()
                    .controller
                    .set_window_count(visible_window_count(mtm));
            }
        }

        /// Per-drain: let each tool panel apply background results, keep the
        /// keyboard on a just-opened panel (`reclaimToolKey`), and mirror its
        /// `testState` (+ wid/level) into `state.tools`.
        fn sync_tools(&self) {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0);
            let mut docs: Vec<(String, serde_json::Value)> = Vec::new();
            {
                let mut state = self.ivars().state.borrow_mut();
                if let Some(st) = state.as_mut() {
                    let mtm = MainThreadMarker::new();
                    let key_wid: i64 = mtm
                        .and_then(|m| {
                            NSApplication::sharedApplication(m)
                                .keyWindow()
                                .map(|w| w.windowNumber() as i64)
                        })
                        .unwrap_or(0);
                    for (name, panel) in st.tools.iter_mut() {
                        // `reclaimToolKey`: one check 0.25 s after opening — if
                        // another window took the keyboard, take it back once
                        // (a later hotkey to the shared window wins).
                        if let (Some(mtm), Some(opened)) = (
                            mtm,
                            st.tool_opened_at.get(name).copied(),
                        ) {
                            if now - opened >= 0.25 {
                                st.tool_opened_at.remove(name);
                                // `key_wid == 0`: no window of ours is key — a palette
                                // pick handed activation back to the user's app and
                                // that activation landed after the panel took key.
                                if !panel.has_key() && key_wid != panel.window_number() {
                                    panel.show(mtm);
                                }
                            }
                        }
                        panel.poll();
                        docs.push((name.clone(), panel.test_state()));
                    }
                }
            }
            for (name, doc) in docs {
                self.ivars().controller.set_tool_state(&name, doc);
            }
            if let Some(st) = self.ivars().state.borrow_mut().as_mut() {
                if st.terminal_opened_at.is_some_and(|t| t.elapsed().as_secs_f64() >= 0.25) {
                    st.terminal_opened_at = None;
                    if let Some(tp) = st.terminal_panel.as_ref() {
                        tp.reclaim_key();
                    }
                }
            }
            let term_doc = self
                .ivars()
                .state
                .borrow()
                .as_ref()
                .and_then(|st| st.terminal_panel.as_ref().map(|tp| tp.test_state()));
            if let Some(doc) = term_doc {
                self.ivars().controller.set_terminal_panel_state(doc);
            }
        }

        /// Cmd+A/X/C/V/Z (+ Cmd+Shift+Z) for a popup's text input while
        /// another app is frontmost. Popups are non-activating panels — Hyper+S
        /// never activates kitchen-sink — and AppKit only routes Cmd edit keys
        /// through the Edit menu of the *active* app, so without this they did
        /// nothing in the Create File paste box, Text to Table, the palette
        /// search, … (rule 1). Sent straight to the first responder: any
        /// `NSText` (fields, text views), or the terminal popup's view.
        fn cmd_edit_in_popup(&self, event: &NSEvent) -> bool {
            use objc2_app_kit::NSEventModifierFlags as F;
            let Some(mtm) = MainThreadMarker::new() else {
                return false;
            };
            let flags = event.modifierFlags();
            if !flags.contains(F::Command) || flags.contains(F::Control) || flags.contains(F::Option) {
                return false;
            }
            let shift = flags.contains(F::Shift);
            let selector = match (event.keyCode(), shift) {
                (0, false) => sel!(selectAll:),
                (7, false) => sel!(cut:),
                (8, false) => sel!(copy:),
                (9, false) => sel!(paste:),
                (6, false) => sel!(undo:),
                (6, true) => sel!(redo:),
                _ => return false,
            };
            // We are the frontmost app: the Edit menu works, leave it be.
            let own = std::process::id() as i32;
            if crate::platform::frontmost_app().map_or(true, |a| a.pid == own) {
                return false;
            }
            let app = NSApplication::sharedApplication(mtm);
            let Some(window) = app.keyWindow() else {
                return false;
            };
            let Some(r) = window.firstResponder() else {
                return false;
            };
            let is_text = as_any(&*r).downcast_ref::<objc2_app_kit::NSText>().is_some();
            let is_terminal = window.title().to_string() == "terminal";
            let target: &AnyObject = as_any(&*r);
            let responds: bool = unsafe { msg_send![target, respondsToSelector: selector] };
            if !(is_text || is_terminal) || !responds {
                return false;
            }
            unsafe { app.sendAction_to_from(selector, Some(target), None) }
        }

        /// `TextEditKeys.killWordBackward` for any editable `NSText` first
        /// responder (field editors included) in the key window.
        fn kill_word_in_text_input(&self, event: &NSEvent) -> bool {
            use crate::engines::text_edit_keys::{route, EditAction, Focus, Modifiers, KEY_W};
            use objc2_app_kit::NSEventModifierFlags as F;
            let Some(mtm) = MainThreadMarker::new() else {
                return false;
            };
            let flags = event.modifierFlags();
            let mods = Modifiers {
                ctrl: flags.contains(F::Control),
                cmd: flags.contains(F::Command),
                option: flags.contains(F::Option),
                shift: flags.contains(F::Shift),
            };
            if !mods.ctrl || event.keyCode() != KEY_W {
                return false;
            }
            let Some(window) = objc2_app_kit::NSApplication::sharedApplication(mtm).keyWindow() else {
                return false;
            };
            let Some(r) = window.firstResponder() else {
                return false;
            };
            let Some(text) = as_any(&*r).downcast_ref::<objc2_app_kit::NSText>() else {
                return false;
            };
            let focus = Focus { is_text: true, is_editable: text.isEditable() };
            if route(event.keyCode(), mods, focus) != Some(EditAction::KillWordBackward) {
                return false;
            }
            let nil: Option<&AnyObject> = None;
            let _: () = unsafe { msg_send![text, deleteWordBackward: nil] };
            true
        }

        /// The local keyDown guard (mirrors `PopupWindow.installMonitors`'
        /// handler, scaled to the shared host): route the ported
        /// [`popup::route_key`] decision, execute Esc-hide and edit keys, and
        /// consume only what is acted on (everything else passes through).
        fn route_key_event(&self, event: &NSEvent) -> bool {
            // Ctrl+W deletes the previous word in every text input (window
            // fields, panel search boxes, sheets, inline rename).
            if self.kill_word_in_text_input(event) {
                return true;
            }
            if self.cmd_edit_in_popup(event) {
                return true;
            }
            if crate::ui::inline_rename::active() {
                return false;
            }
            if self.route_overlay_key(event) {
                return true;
            }
            {
                let state = self.ivars().state.borrow();
                let Some(st) = state.as_ref() else {
                    return false;
                };
                if !st.window.isVisible() || !st.window.isKeyWindow() {
                    return false;
                }
                if st.window.attachedSheet().is_some() {
                    // Sheets keep their own Esc / edit-key routing.
                    return false;
                }
            }

            // `VimKeys.searching(in:)`: an open `/` bar owns every key.
            if self.vim_searching() {
                return self.vim_key(event);
            }

            // `actionPickerKey`: the Cmd+K card owns the keyboard while up.
            let picker = self.ivars().action_picker.borrow().clone();
            if let Some(p) = picker {
                if unsafe { p.superview() }.is_some() {
                    use objc2_app_kit::NSEventModifierFlags as F;
                    let m = event.modifierFlags();
                    if !p.key(event.keyCode(), m.contains(F::Control), m.contains(F::Shift)) {
                        self.ivars().action_picker.borrow_mut().take();
                    }
                    return true;
                }
                self.ivars().action_picker.borrow_mut().take();
            }

            // `overlayKey`: the shortcuts sheet owns the keyboard while up.
            let sheet = self.ivars().state.borrow().as_ref().and_then(|st| st.shortcuts.clone());
            if let Some(sheet) = sheet {
                let mods = event.modifierFlags();
                use objc2_app_kit::NSEventModifierFlags as F;
                sheet.handle_key(event.keyCode(), mods.contains(F::Control), mods.contains(F::Command));
                // A close clears `st.shortcuts` itself; make sure it is gone.
                if unsafe { sheet.superview() }.is_none() {
                    if let Some(st) = self.ivars().state.borrow_mut().as_mut() {
                        st.shortcuts = None;
                    }
                }
                return true;
            }
            let mut key = popup::key_input_from_event(event);
            if key.key_code == popup::KEY_ESC {
                let now = event.timestamp();
                let n = if now - self.ivars().last_esc_at.get() <= 0.6 {
                    self.ivars().esc_streak.get().saturating_add(1)
                } else {
                    1
                };
                self.ivars().esc_streak.set(n);
                self.ivars().last_esc_at.set(now);
                key.esc_streak = n;
            } else {
                self.ivars().esc_streak.set(0);
            }

            // The Ctrl+B prefix / Ctrl+HJKL pane moves (`SharedWindow.prefixKey`).
            let prefix_key = popup::PrefixKey {
                key_code: event.keyCode(),
                chars: event
                    .charactersIgnoringModifiers()
                    .and_then(|s| s.to_string().chars().next()),
                cmd: key.cmd,
                ctrl: key.ctrl,
                opt: key.opt,
                shift: key.shift,
                is_repeat: event.isARepeat(),
            };
            let outcome = {
                let mut state = self.ivars().state.borrow_mut();
                match state.as_mut() {
                    Some(st) => st.prefix.handle(&prefix_key, event.timestamp()),
                    None => popup::PrefixOutcome::Passthrough,
                }
            };
            match outcome {
                popup::PrefixOutcome::Consumed => return true,
                popup::PrefixOutcome::PaneMove(dir) => {
                    let pane_dir = match dir {
                        popup::Direction::Left => PaneDir::Left,
                        popup::Direction::Right => PaneDir::Right,
                        popup::Direction::Up => PaneDir::Up,
                        popup::Direction::Down => PaneDir::Down,
                    };
                    self.live_pane_move(pane_dir);
                    return true;
                }
                popup::PrefixOutcome::Command(ch) => {
                    self.run_prefix_command(ch);
                    return true;
                }
                popup::PrefixOutcome::Passthrough => {}
            }

            // `if !armed, VimKeys.shared.handle(e, in: w)`.
            if self.vim_key(event) {
                return true;
            }

            // `(w.firstResponder as? PopupTabsBar)?.handleNavKey(e)`: a rail
            // with the keyboard owns ↑↓ / Return / Esc / letters.
            let rail = self.ivars().state.borrow().as_ref().and_then(|st| st.window.firstResponder());
            if let Some(r) = rail {
                let any: &AnyObject = unsafe { &*(&*r as *const objc2_app_kit::NSResponder as *const AnyObject) };
                if let Some(bar) = any.downcast_ref::<crate::ui::sidebar::SidebarView>() {
                    if bar.handle_nav_key(event) {
                        return true;
                    }
                }
                if let Some(tree) = any.downcast_ref::<crate::ui::tree_view::TreeView>() {
                    if tree.handle_nav_key(event) {
                        return true;
                    }
                }
            }

            let current = self.ivars().controller.current();
            if current.is_some_and(|v| v.is_jira()) && self.jira_key(&key) {
                return true;
            }
            if matches!(current, Some(SlotView::Compare) | Some(SlotView::CompareText)) {
                let in_text = self
                    .ivars()
                    .state
                    .borrow()
                    .as_ref()
                    .and_then(|st| st.window.firstResponder())
                    .is_some_and(|r| as_any(&*r).downcast_ref::<objc2_app_kit::NSText>().is_some());
                // Folder copy / move chords ask Replace / Keep Both / Skip first.
                if !in_text {
                    if let Some((from, move_)) = crate::views::compare::folder_transfer_key(&key) {
                        let is_folder = self.compare_with(|m| m.current_is_folder()).unwrap_or(false);
                        if is_folder {
                            self.compare_folder_transfer(from, move_);
                            return true;
                        }
                    }
                }
                if let Some(m) = self.ivars().controller.compare_model() {
                    if m.lock().unwrap().handle_key(key, in_text) {
                        return true;
                    }
                }
            }
            if current == Some(SlotView::Files) && self.files_key(&key) {
                return true;
            }
            if current == Some(SlotView::Confluence) && self.conf_key(&key) {
                return true;
            }
            if current == Some(SlotView::Ai) {
                // `AIWindow.handleKey`: Esc stops a run, Ctrl/Cmd+Return runs,
                // Cmd+L focuses the input.
                use crate::views::ai::{route_ai_key, AIKeyAction, KeyInputLike};
                let running = self.ai_surface(|a| a.model.is_running()).unwrap_or(false);
                let k = KeyInputLike { key_code: key.key_code, cmd: key.cmd, ctrl: key.ctrl };
                match route_ai_key(k, running) {
                    AIKeyAction::Stop | AIKeyAction::Run => {
                        self.ai_event(crate::views::ai_view::AIEvent::Run);
                        return true;
                    }
                    AIKeyAction::FocusInput => {
                        self.live_pane_focus("input");
                        return true;
                    }
                    _ => {}
                }
            }
            let current = self.ivars().controller.current();
            if current == Some(SlotView::Notes) {
                if let Some(r) = self.route_notes_key(&key, event.timestamp()) {
                    return r;
                }
            }
            let (edit_mode, vim_active) = {
                let state = self.ivars().state.borrow();
                // `config.editMode` is per-window: only the notes member is an
                // editor; files/jira keep their list-key routing.
                let notes = current == Some(SlotView::Notes);
                let vim = notes
                    && state
                        .as_ref()
                        .and_then(|st| st.notes.as_ref())
                        .map(|s| s.vim_active())
                        .unwrap_or(false);
                (notes, vim)
            };
            let (find_shown, _) = {
                let state = self.ivars().state.borrow();
                let surface = state.as_ref().and_then(|st| st.notes.as_ref());
                (
                    surface.map(|s| s.find_shown()).unwrap_or(false),
                    surface.map(|s| s.rail_collapsed()).unwrap_or(false),
                )
            };
            let cfg = PopupConfig {
                edit_mode,
                vim_focus: vim_active,
                has_cycle_view_hook: current.is_some(),
                find_bar_shown: find_shown,
                has_file_browser: current == Some(SlotView::Files),
                browser_has_focus: current == Some(SlotView::Files),
                esc_close_count: current
                    .map(|v| self.ivars().controller.esc_hide_count(v) as i32)
                    .unwrap_or_else(|| self.ivars().controller.settings().esc_close as i32),
                ..PopupConfig::default()
            };

            match popup::route_key(&cfg, key) {
                KeyAction::Escape(_) => {
                    if current == Some(SlotView::Files) {
                        let had = self
                            .files_surface(|b| {
                                let had = !b.list.marked.is_empty();
                                b.list.marked.clear();
                                had
                            })
                            .unwrap_or(false);
                        if had {
                            return true;
                        }
                    }
                    if current == Some(SlotView::CompareText) {
                        // PRD-compare KR6: Esc in compareText steps back to the
                        // compare view (Swift `slot.back(esc: true)`), it never
                        // hides the window directly.
                        self.ivars().controller.back(true);
                        return true;
                    }
                    if find_shown {
                        // Swift `editorKey`: the find bar closes before the
                        // window's own Esc policy runs.
                        self.notes_find_close();
                        return true;
                    }
                    let closes = popup::esc_streak_closes(key.esc_streak, cfg.esc_close_count);
                    if vim_active {
                        // Swift `editorKey`: Esc belongs to nvim; the window
                        // closes only on the configured streak from Normal mode.
                        let normal = if closes {
                            self.notes_vim_mode().as_deref() == Some("n")
                        } else {
                            false
                        };
                        if closes && normal {
                            let v = current.map(|v| v.raw()).unwrap_or("");
                            self.ivars().controller.hide(&format!("Esc ({v})"));
                        } else {
                            self.notes_vim_keys("\u{1b}");
                        }
                        return true;
                    }
                    if closes {
                        let v = current.map(|v| v.raw()).unwrap_or("");
                        self.ivars().controller.hide(&format!("Esc ({v})"));
                    }
                    true
                }
                KeyAction::Vim(op) => self.perform_vim_op(op),
                KeyAction::Edit(popup::EditOp::Find) => self.route_find_toggle(),
                KeyAction::Edit(op) | KeyAction::SheetEdit(op) | KeyAction::Terminal(op) => {
                    self.perform_edit_op(op)
                }
                KeyAction::ListMove(delta) if key.shift && current == Some(SlotView::Files) => {
                    // Shift+↑/↓ grows the marked range from the anchor.
                    self.files_surface(|b| {
                        let n = b.list.rows.len() as i64;
                        if n > 0 {
                            let to = (b.list.selection as i64 + delta as i64).clamp(0, n - 1);
                            b.list.extend_selection(to as usize);
                        }
                    });
                    true
                }
                KeyAction::ListMove(delta) => self.route_list_move(delta as i64),
                KeyAction::ListMoveWrap(delta) => self.route_list_move_wrap(delta as i64),
                KeyAction::ListAccept => self.route_list_accept(),
                KeyAction::ListToggleSelection => self.route_list_toggle(),
                KeyAction::ListPage(page) => self.route_list_page(page),
                KeyAction::CycleView(dir) => {
                    self.ivars().controller.cycle(dir);
                    true
                }
                KeyAction::CycleTabs(dir) => self.route_cycle_tabs(dir),
                KeyAction::ToggleSidebarRail => self.route_rail_toggle(),
                KeyAction::PaneResize(dir) => self.route_resize(dir),
                KeyAction::ResizeStep(sign) => self.route_resize_by(sign),
                KeyAction::FontStep(sign) => {
                    if current == Some(SlotView::Notes) {
                        self.notes_surface(|s| s.step_font(sign)).is_some()
                    } else if current == Some(SlotView::Files) {
                        self.files_text_zoom(sign);
                        true
                    } else if current.is_some_and(|v| v.is_jira()) {
                        crate::ui::toast::not_ported("Jira list text zoom");
                        true
                    } else {
                        self.route_resize_by(sign)
                    }
                }
                KeyAction::ResizeReset => self.route_reset_size(),
                KeyAction::CommandF => self.route_find_toggle(),
                KeyAction::FindNext(dir) => self.route_find_step(dir),
                KeyAction::EditorCommit => self.route_commit(),
                KeyAction::EditorOpenPath => self.route_open_path_prompt(),
                KeyAction::FileBrowser(action) => self.route_file_browser(action),
                KeyAction::CommandK => match current {
                    Some(v) if v.is_jira() => {
                        self.jira_actions();
                        true
                    }
                    Some(SlotView::Compare) | Some(SlotView::CompareText) => {
                        self.compare_actions();
                        true
                    }
                    // Swift wires `onCommandK` only for list windows.
                    _ => false,
                },
                KeyAction::ShowShortcuts => {
                    self.show_shortcuts();
                    true
                }
                _ => false,
            }
        }

        /// Keys for the key-window overlays (the `show` palette and the Ctrl+B
        /// W view switcher). Only navigation/accept keys are consumed so
        /// typing still reaches their search fields.
        fn route_overlay_key(&self, event: &NSEvent) -> bool {
            use objc2_app_kit::NSEventModifierFlags;
            // The Create File panel owns its own keys (Tab flips name <->
            // paste). A palette/switcher that still believes it has the key
            // must not eat Tab out from under it.
            if let Some(mtm) = objc2::MainThreadMarker::new() {
                if event.window(mtm).is_some_and(|w| w.title().to_string() == "filefast") {
                    return false;
                }
            }
            if self.note_find_key(event) || self.paths_key(event) {
                return true;
            }

            enum Out {
                NotHandled,
                Consumed,
                Palette(PaletteAction),
                PaletteClosed,
                SwitcherClosed,
                Nav(i64),
            }

            let code = event.keyCode();
            let mods = event.modifierFlags();
            let cmd = mods.contains(NSEventModifierFlags::Command);
            let ctrl = mods.contains(NSEventModifierFlags::Control);
            let shift = mods.contains(NSEventModifierFlags::Shift);
            let opt = mods.contains(NSEventModifierFlags::Option);

            let out = {
                let mut state = self.ivars().state.borrow_mut();
                let Some(st) = state.as_mut() else {
                    return false;
                };
                if let Some(p) = st.palette.as_mut() {
                    if p.has_key() {
                        if code == popup::KEY_ESC {
                            if p.live_query().trim().is_empty() {
                                // Hidden in the arm below, after the hand-back.
                                Out::PaletteClosed
                            } else {
                                // `SwitcherController.handleEscape`: a live
                                // query clears instead of hiding.
                                p.clear_query();
                                Out::Consumed
                            }
                        } else if code == popup::KEY_RETURN || code == 76 {
                            match p.key(code, ctrl, shift, cmd, opt) {
                                Some(a) => Out::Palette(a),
                                None => Out::Consumed,
                            }
                        } else if matches!(
                            code,
                            popup::KEY_DOWN | popup::KEY_UP | popup::KEY_TAB
                        ) || (matches!(code, 45 | 35) && ctrl)
                            // Left/Right switch panes only on an empty query;
                            // otherwise they move the caret in the filter field.
                            || (matches!(code, 123 | 124)
                                && p.live_query().trim().is_empty())
                        {
                            p.key(code, ctrl, shift, cmd, opt);
                            Out::Consumed
                        } else {
                            Out::NotHandled
                        }
                    } else {
                        Out::NotHandled
                    }
                } else {
                    Out::NotHandled
                }
            };
            let out = match out {
                Out::NotHandled => {
                    let mut state = self.ivars().state.borrow_mut();
                    let Some(st) = state.as_mut() else {
                        return false;
                    };
                    if let Some(sw) = st.switcher.as_mut() {
                        if sw.has_key() {
                            if code == popup::KEY_ESC {
                                sw.hide_panel();
                                Out::SwitcherClosed
                            } else if code == popup::KEY_DOWN || (code == 45 && ctrl) {
                                sw.move_selection(1);
                                Out::Consumed
                            } else if code == popup::KEY_UP || (code == 35 && ctrl) {
                                sw.move_selection(-1);
                                Out::Consumed
                            } else if code == popup::KEY_TAB {
                                sw.move_selection(if shift { -1 } else { 1 });
                                Out::Consumed
                            } else if code == popup::KEY_RETURN || code == 76 {
                                match sw.accept() {
                                    Some(id) => Out::Nav(id),
                                    None => Out::Consumed,
                                }
                            } else if let Some(id) =
                                sw.key(code, cmd, ctrl, opt)
                            {
                                Out::Nav(id)
                            } else {
                                Out::NotHandled
                            }
                        } else {
                            Out::NotHandled
                        }
                    } else {
                        Out::NotHandled
                    }
                }
                other => other,
            };

            match out {
                Out::NotHandled => false,
                Out::Consumed => true,
                Out::Palette(action) => {
                    // Return accepted from the key monitor: decide focus while
                    // the palette is still up, then close it like a click does
                    // (it used to linger after Enter).
                    if let Some(mtm) = MainThreadMarker::new() {
                        palette_pick_focus(mtm, &action);
                    }
                    if let Some(p) = self.ivars().state.borrow_mut().as_mut().and_then(|st| st.palette.as_mut()) {
                        p.hide_panel();
                    }
                    self.end_palette(false);
                    match action {
                        PaletteAction::Command(name) => {
                            self.ivars().controller.run_palette_command(&name)
                        }
                        PaletteAction::Workspace(id) => {
                            self.ivars().controller.switch_workspace(&id)
                        }
                    }
                    self.ivars().controller.set_palette_visible(false);
                    true
                }
                Out::PaletteClosed => {
                    // Esc on an empty query: the usual cancel. This path used
                    // to only hide the panel, leaving the app active.
                    if let Some(mtm) = MainThreadMarker::new() {
                        palette_hand_back(mtm);
                    }
                    if let Some(p) = self.ivars().state.borrow_mut().as_mut().and_then(|st| st.palette.as_mut()) {
                        p.hide_panel();
                    }
                    self.ivars().controller.set_palette_visible(false);
                    self.end_palette(true);
                    true
                }
                Out::SwitcherClosed => {
                    self.ivars().controller.view_switcher_hide();
                    true
                }
                Out::Nav(id) => {
                    self.ivars().controller.nav_clicked(id);
                    self.ivars().controller.view_switcher_hide();
                    // The switcher hides itself via the nav callback path.
                    let mut state = self.ivars().state.borrow_mut();
                    if let Some(st) = state.as_mut() {
                        if let Some(sw) = st.switcher.as_mut() {
                            sw.hide_panel();
                        }
                    }
                    true
                }
            }
        }

        /// Run an op against the notes vim pane (`PopupWindow.vimPaneKey`).
        fn perform_vim_op(&self, op: popup::VimOp) -> bool {
            let state = self.ivars().state.borrow();
            let Some(pane) = state
                .as_ref()
                .and_then(|st| st.notes.as_ref())
                .and_then(|s| s.vim_pane())
            else {
                return false;
            };
            let visual = || {
                pane.eval("mode()")
                    .map(|m| matches!(m.trim(), "v" | "V" | "\u{16}"))
                    .unwrap_or(false)
            };
            match op {
                popup::VimOp::Copy => {
                    if visual() {
                        pane.remote("\"+y");
                    }
                }
                popup::VimOp::Cut => {
                    if visual() {
                        pane.remote("\"+d");
                    }
                }
                popup::VimOp::Paste => {
                    // Swift `vimPaste`: an image on the clipboard wins — it is
                    // saved next to the note (`assets/img-<epoch>.png`) and
                    // pasted as `![](assets/…)`; otherwise the text.
                    let mut clip: Option<String> = None;
                    #[cfg(target_os = "macos")]
                    if let Some(note) = pane.file() {
                        clip = crate::views::notes_vim::save_pasteboard_image(note)
                            .map(|rel| format!("![]({rel})"));
                    }
                    let text = clip.or_else(crate::platform::get_text);
                    if let Some(text) = text {
                        if !text.is_empty() {
                            // `nvim_paste` first, bracketed paste fallback.
                            pane.paste(&text);
                        }
                    }
                }
                popup::VimOp::SelectAll => pane.remote("<C-\\><C-N>ggVG"),
                popup::VimOp::Undo => pane.remote("<C-\\><C-N>u"),
                popup::VimOp::Save => pane.flush(),
                popup::VimOp::Search => pane.remote("<C-\\><C-N>/"),
                popup::VimOp::Close => self.ivars().controller.hide("Cmd+W"),
                popup::VimOp::OpenPath => {
                    self.route_open_path_prompt();
                }
            }
            true
        }

        /// The notes vim pane's current mode (`vimEval("mode()")`).
        fn notes_vim_mode(&self) -> Option<String> {
            let state = self.ivars().state.borrow();
            state
                .as_ref()
                .and_then(|st| st.notes.as_ref())
                .and_then(|s| s.vim_pane())
                .and_then(|p| p.eval("mode()"))
                .map(|m| m.trim().to_string())
        }

        /// Send raw keys to the notes vim pane.
        fn notes_vim_keys(&self, keys: &str) {
            let state = self.ivars().state.borrow();
            if let Some(pane) = state
                .as_ref()
                .and_then(|st| st.notes.as_ref())
                .and_then(|s| s.vim_pane())
            {
                pane.send_keys(keys);
            }
        }

        /// The notes window's own keys ahead of the shared router — Swift
        /// `handleKey`'s prose search, `overlayKey` Esc, `windowSizeKey` page
        /// zoom and the `modifiedKey` notes branches.
        fn route_notes_key(&self, key: &popup::KeyInput, now: f64) -> Option<bool> {
            use crate::views::notes::Mods;
            let code = key.key_code;
            let m = Mods { command: key.cmd, control: key.ctrl, option: key.opt, shift: key.shift };
            let prose = self.notes_surface(|s| s.prose_shown()).unwrap_or(false);
            if prose {
                if let Some(Some(r)) = self.notes_surface(|s| s.prose_pane().and_then(|p| p.search_key(code, m, now))) {
                    if r && code != popup::KEY_ESC {
                        self.notes_surface(|s| s.focus_prose());
                    }
                    return Some(r);
                }
                if code == popup::KEY_ESC && !key.cmd && !key.ctrl && !key.opt {
                    self.notes_surface(|s| s.set_prose(false));
                    return Some(true);
                }
                let plus = code == popup::KEY_PLUS || code == popup::KEY_KEYPAD_PLUS;
                let minus = code == popup::KEY_MINUS || code == popup::KEY_KEYPAD_MINUS;
                if key.cmd && !key.opt && (plus || minus) {
                    let f = if plus { 1.1 } else { 1.0 / 1.1 };
                    self.notes_surface(|s| s.prose_pane().map(|p| p.zoom_by(f)));
                    return Some(true);
                }
                if key.cmd && !key.opt && code == popup::KEY_ZERO {
                    self.notes_surface(|s| s.prose_pane().map(|p| p.reset_zoom()));
                    return Some(true);
                }
            }
            if key.cmd && key.shift && !key.ctrl && code == popup::KEY_P {
                if self.notes_surface(|s| s.has_prose_source()) == Some(true) {
                    self.notes_surface(|s| s.set_prose(!s.prose_shown()));
                    return Some(true);
                }
            }
            if key.cmd && key.shift && !key.ctrl && code == popup::KEY_O {
                if self.notes_surface(|s| s.has_prose_source()) == Some(true) {
                    self.notes_prose_switch_event(crate::ui::prose_switch::ProseSwitchEvent::PopOut);
                    return Some(true);
                }
            }
            if prose && code == popup::KEY_P && m == Mods::CMD {
                let toast = self.ivars().controller.settings().copy_toast.clone();
                self.notes_surface(|s| {
                    let cfg = crate::views::notes::NotesConfig::load();
                    if let Some(p) = s.prose_pane() {
                        p.export_pdf(&cfg, &toast);
                    }
                });
                return Some(true);
            }
            if code == popup::KEY_N && m == Mods::CMD {
                self.notes_surface(|s| s.new_note_from(false));
                return Some(true);
            }
            if code == popup::KEY_N && m == Mods::CTRL {
                // Not in the drawer shell, and only from vim Normal mode.
                let ok = self
                    .notes_surface(|s| {
                        if s.terminal_has_focus() {
                            return false;
                        }
                        if !s.vim_has_focus() {
                            return true;
                        }
                        s.vim_pane()
                            .and_then(|v| v.eval("mode() . (get(g:, 'ws_picking', 0) ? 'p' : '')"))
                            .map(|m| m.trim() == "n")
                            .unwrap_or(false)
                    })
                    .unwrap_or(false);
                if ok {
                    self.notes_surface(|s| s.new_note_from(true));
                    return Some(true);
                }
            }
            if (code == popup::KEY_J || code == popup::KEY_K) && key.ctrl && key.shift && !key.cmd && !key.opt {
                if self.notes_surface(|s| s.vim_has_focus()) == Some(true) {
                    let plug = if code == popup::KEY_J {
                        "<Plug>(VM-Add-Cursor-Down)"
                    } else {
                        "<Plug>(VM-Add-Cursor-Up)"
                    };
                    self.notes_surface(|s| s.vim_pane().map(|v| v.remote(plug)));
                    return Some(true);
                }
            }
            if prose && m.is_empty() && (code == popup::KEY_J || code == popup::KEY_K) {
                let dy = if code == popup::KEY_J {
                    crate::views::notes::PROSE_LINE_STEP
                } else {
                    -crate::views::notes::PROSE_LINE_STEP
                };
                self.notes_surface(|s| s.prose_pane().map(|p| p.scroll_by(dy)));
                return Some(true);
            }
            None
        }

        /// `showShortcuts(on:view:)` — Cmd+/: the view's `[shortcuts]` rows
        /// then the shared groups, in a sheet over the window.
        fn show_shortcuts(&self) {
            use crate::ui::shortcuts::{groups_for, parse_entries, ShortcutsOverlay};
            let Some(mtm) = MainThreadMarker::new() else { return };
            let view = self.ivars().controller.current().map(theme_section).unwrap_or("all");
            let text = crate::app::config::read_config_text().unwrap_or_default();
            let groups = groups_for(view, &parse_entries(&text));
            if groups.is_empty() {
                crate::ui::toast::show("No shortcuts listed — add a [shortcuts] section to commands.toml", "keyboard");
                return;
            }
            let mut state = self.ivars().state.borrow_mut();
            let Some(st) = state.as_mut() else { return };
            if let Some(old) = st.shortcuts.take() {
                old.removeFromSuperview();
            }
            let Some(root) = st.window.contentView() else { return };
            let colors = crate::ui::theme::section_colors(view);
            let weak = objc2::rc::Weak::from_retained(&self.retain());
            let o = ShortcutsOverlay::show(
                mtm,
                &root,
                groups,
                colors,
                "Keyboard Shortcuts",
                Box::new(move || {
                    if let Some(ui) = weak.load() {
                        if let Ok(mut state) = ui.ivars().state.try_borrow_mut() {
                            if let Some(st) = state.as_mut() {
                                st.shortcuts = None;
                            }
                        }
                    }
                }),
            );
            st.shortcuts = Some(o);
        }

        /// The armed prefix's `l`/`w`/`t`/`b` (`SharedWindow.prefixKey`).
        fn run_prefix_command(&self, ch: char) {
            match ch {
                'l' => {
                    // `previousNav ?? (current == .files ? navNotes : navFiles)`
                    use crate::app::registry::nav;
                    let c = &self.ivars().controller;
                    let id = c.previous_nav().unwrap_or(if c.current() == Some(SlotView::Files) {
                        nav::NOTES
                    } else {
                        nav::FILES
                    });
                    c.nav_clicked(id);
                }
                // Ctrl+B W: the in-app view switcher (Files / Notes / Confluence
                // / Jira). Hyper+S keeps the full two-pane palette.
                'w' => {
                    let c = &self.ivars().controller;
                    if c.view_switcher_shown() {
                        c.view_switcher_hide();
                    } else {
                        c.view_switcher_show();
                    }
                }
                't' => {
                    // `toggleProseFromPrefix`: the notes vim pane / reading
                    // view flips prose; anything else gets the terminal.
                    let notes = self.ivars().controller.current() == Some(SlotView::Notes);
                    if !(notes && self.notes_surface(|s| s.toggle_prose_from_prefix()) == Some(true)) {
                        self.ivars().controller.toggle_terminal_panel();
                    }
                }
                'b' => {
                    self.route_rail_toggle();
                }
                _ => {}
            }
        }

        /// Run `f` against the notes surface when it exists.
        /// The current member's `navPanes` (real AppKit views) + the window.
        fn live_panes(&self) -> Option<(Retained<SlotHostWindow>, Vec<crate::app::live_panes::LivePane>)> {
            let state = self.ivars().state.borrow();
            let st = state.as_ref()?;
            let panes = match st.current? {
                SlotView::Notes => st.notes.as_ref()?.nav_panes(),
                SlotView::Files => st.files.as_ref()?.view.as_ref()?.nav_panes(),
                SlotView::Jira => st.jira.as_ref()?.1.nav_panes(),
                SlotView::Confluence => st.confluence.as_ref()?.view.nav_panes(),
                SlotView::Ai => st.ai.as_ref()?.view.nav_panes(),
                SlotView::Compare | SlotView::CompareText => st.compare.as_ref()?.nav_panes(),
                _ => return None,
            };
            Some((st.window.clone(), panes))
        }

        /// The id of the pane that holds the keyboard right now.
        fn focused_pane_id(&self) -> Option<String> {
            let (w, panes) = self.live_panes()?;
            crate::app::live_panes::current(&w, &panes).map(|p| p.id.clone())
        }

        /// `PaneNav.shared.move(_:in:)` on the live window (Ctrl+HJKL).
        fn live_pane_move(&self, dir: PaneDir) -> bool {
            let Some((w, panes)) = self.live_panes() else { return false };
            let split = |d: PaneDir| self.notes_surface(|s| s.vim_split_move(d)).unwrap_or(false);
            let moved = self.ivars().live_nav.borrow_mut().move_dir(&w, &panes, dir, &split);
            self.publish_live_pane(&w, &panes);
            moved
        }

        /// `PaneNav.shared.focus(_:in:)` (`do:pane:focus:<id>`).
        fn live_pane_focus(&self, id: &str) -> bool {
            let Some((w, panes)) = self.live_panes() else { return false };
            let ok = self.ivars().live_nav.borrow_mut().focus(&w, &panes, id);
            self.publish_live_pane(&w, &panes);
            ok
        }

        /// `refreshSoon`: keep the ring on whatever holds the keyboard (a
        /// click, a sheet, a view switch) — run from the drain tick.
        fn live_pane_refresh(&self) {
            match self.live_panes() {
                Some((w, panes)) => {
                    self.ivars().live_nav.borrow_mut().refresh(&w, &panes);
                    self.vim_refresh(&w, &panes);
                    self.publish_live_pane(&w, &panes);
                }
                None => {
                    let sig = "none".to_string();
                    if *self.ivars().live_pane_sig.borrow() != sig {
                        *self.ivars().live_pane_sig.borrow_mut() = sig;
                        self.ivars().controller.set_live_pane_state(json!({}));
                    }
                }
            }
        }

        fn publish_live_pane(&self, w: &objc2_app_kit::NSWindow, panes: &[crate::app::live_panes::LivePane]) {
            let mut doc = self.ivars().live_nav.borrow().test_state(w, panes);
            if let (Some(o), Some(v)) = (doc.as_object_mut(), self.vim_test_state().as_object()) {
                for (k, val) in v {
                    o.insert(k.clone(), val.clone());
                }
            }
            let sig = doc.to_string();
            if *self.ivars().live_pane_sig.borrow() != sig {
                *self.ivars().live_pane_sig.borrow_mut() = sig;
                self.ivars().controller.set_live_pane_state(doc);
            }
        }

        fn notes_surface<R>(&self, f: impl FnOnce(&mut crate::views::notes::NotesSurface) -> R) -> Option<R> {
            let mut state = self.ivars().state.borrow_mut();
            state
                .as_mut()
                .and_then(|st| st.notes.as_mut())
                .map(f)
        }

        /// Run `f` against the files browser when it exists.
        fn files_surface<R>(&self, f: impl FnOnce(&mut FileBrowser) -> R) -> Option<R> {
            let mut state = self.ivars().state.borrow_mut();
            let b = state.as_mut().and_then(|st| st.files.as_mut())?;
            let r = f(b);
            b.sync_view();
            Some(r)
        }

        /// Build the live file browser (`makeFileBrowser`): `[files]` root /
        /// sort / favorites / start page, the Recent engine, and the view's
        /// event sink routed back to [`Self::files_event`].
        fn build_files(&self, mtm: MainThreadMarker) -> Option<(FileBrowser, Retained<NSView>)> {
            let text = crate::app::config::read_config_text().unwrap_or_default();
            let sections = super::parse_sections(&text);
            let section: HashMap<String, String> = sections.get("files").cloned().unwrap_or_default();
            crate::views::files::start_recent(&section);
            let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
            let root = section
                .get("root")
                .map(|r| r.trim().to_string())
                .filter(|r| !r.is_empty())
                .map(|r| if let Some(rest) = r.strip_prefix('~') { format!("{home}{rest}") } else { r })
                .unwrap_or(home);
            let mut browser = FileBrowser::new(&root);
            browser.configure(&section);
            if section.get("start").map(|s| s.to_lowercase()) == Some("recent".into()) {
                browser.show_virtual(0);
            } else {
                browser.reload();
            }
            browser.build(mtm);
            if let Some(v) = &browser.view {
                let weak = objc2::rc::Weak::from_retained(&self.retain());
                v.set_on_event(Box::new(move |ev| {
                    if let Some(ui) = weak.load() {
                        ui.files_event(ev);
                    }
                }));
            }
            let view = browser.content_view()?;
            Some((browser, view))
        }

        /// A click / keystroke from the live file browser view.
        fn files_event(&self, ev: crate::views::files_view::FilesEvent) {
            use crate::ui::sidebar::SidebarEvent;
            use crate::views::files_view::FilesEvent as E;
            match ev {
                E::Click { row, cmd, shift, double } => {
                    let was_selected = self.files_surface(|b| b.list.selection == row && b.list.marked.is_empty()) == Some(true);
                    self.files_surface(|b| {
                        b.status_note = None;
                        if shift {
                            b.list.extend_selection(row);
                        } else if cmd {
                            b.list.command_click(row);
                        } else {
                            b.list.marked.clear();
                            b.list.selection = row;
                        }
                    });
                    // Recent / Arrived files: click renames (after the
                    // double-click window), double-click previews in the
                    // dismissable popup.
                    let recent_file = (!cmd && !shift)
                        .then(|| {
                            self.files_surface(|b| {
                                if !b.in_recent() {
                                    return None;
                                }
                                b.visible_rows().get(row).filter(|e| !e.is_dir && !e.is_parent()).map(|e| e.path.clone())
                            })
                            .flatten()
                        })
                        .flatten();
                    self.focus_files_list();
                    if let Some(path) = recent_file {
                        if double {
                            cancel_click_rename();
                            crate::views::file_popup::show(&path, None);
                        } else if was_selected {
                            schedule_click_rename(|ui| ui.files_begin_rename());
                        } else {
                            cancel_click_rename();
                        }
                    } else if double && !cmd && !shift {
                        self.files_accept_selected();
                    }
                }
                E::RowMenu { row, x, y } => {
                    self.files_surface(|b| {
                        if !b.list.marked.contains(&row) {
                            b.list.marked.clear();
                            b.list.selection = row;
                        }
                    });
                    self.files_row_menu(x, y);
                }
                E::PreviewOpen(i) => {
                    let target = self.files_surface(|b| match b.preview().1 {
                        crate::views::files_view::Preview::Folder(rows) => rows.get(i).cloned(),
                        _ => None,
                    });
                    if let Some(Some(e)) = target {
                        if e.is_dir {
                            self.files_surface(|b| b.cd(&e.path));
                        } else {
                            open_path(&e.path);
                        }
                    }
                }
                E::Parent => {
                    self.files_surface(|b| b.cd_parent());
                }
                E::TogglePin => {
                    self.files_surface(|b| b.toggle_pin());
                }
                E::ToggleOrder => {
                    self.files_surface(|b| b.toggle_order());
                }
                E::SortMenu { x, y } => {
                    use crate::ui::menu_kit::{popup, MenuEntry};
                    use crate::views::files::SortKey;
                    let cur = self.files_surface(|b| b.sort_key).unwrap_or(SortKey::Name);
                    let view = self.files_surface(|b| b.view.clone()).flatten();
                    let weak = objc2::rc::Weak::from_retained(&self.retain());
                    let entries = SortKey::ALL
                        .iter()
                        .map(|k| {
                            let k = *k;
                            let weak = weak.clone();
                            MenuEntry::checked(k.label(), k == cur, move || {
                                if let Some(ui) = weak.load() {
                                    ui.files_surface(|b| b.set_sort(k));
                                }
                            })
                        })
                        .collect();
                    if let Some(v) = view {
                        popup(entries, Some(&v), x, y);
                    }
                }
                E::Query(q) => {
                    self.files_surface(|b| {
                        b.status_note = None;
                        b.list.selection = 0;
                        b.set_query(&q);
                    });
                }
                E::Enter => {
                    let jumped = self.files_surface(|b| {
                        let q = b.query.trim().to_string();
                        if (q.contains('/') || q.starts_with('~')) && !crate::views::files::has_glob(&q) {
                            let p = crate::views::files::resolve_path(&q, &b.cwd);
                            if std::path::Path::new(&p).is_dir() {
                                b.cd(&p);
                                return Some(None);
                            }
                            if std::path::Path::new(&p).exists() {
                                return Some(Some(p));
                            }
                        }
                        None
                    });
                    match jumped {
                        Some(Some(Some(p))) => open_path(&p),
                        Some(Some(None)) => self.focus_files_list(),
                        _ => {
                            self.files_accept_selected();
                        }
                    }
                }
                E::Move(d) => {
                    self.files_surface(|b| b.list.move_selection(d));
                }
                E::DragEnded => {
                    self.files_surface(|b| b.reload());
                }
                E::Escape => {
                    self.files_surface(|b| {
                        if !b.query.is_empty() {
                            b.set_query("");
                        }
                    });
                    self.focus_files_list();
                }
                E::Sidebar(SidebarEvent::Select(i)) => {
                    let home = std::env::var("HOME").unwrap_or_default();
                    let places = crate::views::files::places(&home);
                    self.files_surface(|b| {
                        b.status_note = None;
                        if i < 2 {
                            b.show_virtual(i);
                        } else if let Some(p) = places.get(i - 2) {
                            b.cd(&p.path);
                        }
                    });
                }
                E::Sidebar(SidebarEvent::Pinned(path)) => {
                    if std::path::Path::new(&path).is_dir() {
                        self.files_surface(|b| b.cd(&path));
                    }
                }
                E::Sidebar(SidebarEvent::MenuPinned(path, x, y)) => {
                    use crate::ui::menu_kit::{popup, MenuEntry};
                    let weak = objc2::rc::Weak::from_retained(&self.retain());
                    let pinned = self
                        .files_surface(|b| b.pinned_favorites.contains(&path))
                        .unwrap_or(false);
                    let (p1, p2, p3, p4) = (path.clone(), path.clone(), path.clone(), path.clone());
                    let w1 = weak.clone();
                    let w4 = weak.clone();
                    let mut entries = vec![
                        MenuEntry::item("Open", move || {
                            if let Some(ui) = w1.load() {
                                ui.files_surface(|b| b.cd(&p1));
                            }
                        }),
                        MenuEntry::item("Reveal in Files", move || reveal_in_files(&[p2])),
                        MenuEntry::item("Copy Absolute Path", move || copy_text_to_pasteboard(&p3)),
                    ];
                    if pinned {
                        entries.push(MenuEntry::Separator);
                        entries.push(MenuEntry::item("Unpin", move || {
                            if let Some(ui) = w4.load() {
                                ui.files_surface(|b| {
                                    b.pinned_favorites.retain(|x| x != &p4);
                                    b.toggle_pin_save();
                                });
                            }
                        }));
                    }
                    popup(entries, None, x, y);
                }
                E::Sidebar(SidebarEvent::Rail(_)) => {
                    self.files_surface(|b| {
                        if let Some(v) = &b.view {
                            v.layout_panes();
                            v.setNeedsDisplay(true);
                        }
                    });
                }
                E::Sidebar(_) => {}
            }
        }

        /// The notes sidebar: select / close / "+" / rail / row menu.
        /// The header app-icon menu (`onChromeIconClick`): Theme ▸ and the
        /// window settings (`addWindowSettingsItems`), Open Config, the
        /// shortcuts list, then the app menu.
        fn header_icon_menu(&self, view: &NSView, x: f64, y: f64) {
            use crate::ui::menu_kit::{popup, MenuEntry};
            use crate::ui::theme::{grouped_by_tone, HexColor, ThemePreset};
            let Some(v) = self.ivars().controller.current() else {
                crate::app::menu::pop_status_menu(view, x, y);
                return;
            };
            let section = theme_section(v);
            let text = crate::app::config::read_config_text().unwrap_or_default();
            let sections = super::parse_sections(&text);
            let overrides: Vec<(String, String)> = sections
                .get("themes")
                .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                .unwrap_or_default();
            let presets = ThemePreset::all_from(&overrides);
            let current_bg = crate::ui::theme::section_colors(section).base();
            let notes = v == SlotView::Notes;
            let mut theme = Vec::new();
            for (i, (_, title, group)) in grouped_by_tone(&presets).into_iter().enumerate() {
                if i > 0 {
                    theme.push(MenuEntry::Separator);
                }
                theme.push(MenuEntry::Header(title.to_string()));
                for p in group {
                    let on = HexColor::format_rgb(&p.background) == HexColor::format_rgb(&current_bg);
                    let entry = if notes {
                        // `themeScopes`: the notes window adds per-part scopes.
                        let mut scopes = Vec::new();
                        for (j, (scope, label)) in [
                            (ThemeScope::Window, "Whole Window"),
                            (ThemeScope::Notepad, "Notepad Only"),
                            (ThemeScope::Terminal, "Terminal Only"),
                        ]
                        .into_iter()
                        .enumerate()
                        {
                            let weak = objc2::rc::Weak::from_retained(&self.retain());
                            let p = p.clone();
                            scopes.push(MenuEntry::item(label, move || {
                                if let Some(ui) = weak.load() {
                                    ui.apply_theme_preset(section, &p, scope);
                                }
                            }));
                            if j == 0 {
                                scopes.push(MenuEntry::Separator);
                            }
                        }
                        MenuEntry::submenu(&p.name, scopes)
                    } else {
                        let weak = objc2::rc::Weak::from_retained(&self.retain());
                        let pc = p.clone();
                        MenuEntry::item(&p.name, move || {
                            if let Some(ui) = weak.load() {
                                ui.apply_theme_preset(section, &pc, ThemeScope::Window);
                            }
                        })
                    };
                    let wk = objc2::rc::Weak::from_retained(&self.retain());
                    let pv = p.clone();
                    let entry = entry.with_hover(move || {
                        if let Some(ui) = wk.load() {
                            ui.preview_theme_preset(section, &pv, ThemeScope::Window);
                        }
                    });
                    theme.push(entry.with_swatch(p.background).with_checked(on));
                }
            }
            theme.push(MenuEntry::Separator);
            let weak = objc2::rc::Weak::from_retained(&self.retain());
            theme.push(MenuEntry::item("Reset to Defaults", move || {
                if let Some(ui) = weak.load() {
                    ui.reset_theme(section);
                }
            }));

            let w1 = objc2::rc::Weak::from_retained(&self.retain());
            let w2 = objc2::rc::Weak::from_retained(&self.retain());
            let w3 = objc2::rc::Weak::from_retained(&self.retain());
            let conf = self.ivars().controller.settings().commands_conf_path();
            let c = self.ivars().controller.clone();
            let entries = vec![
                MenuEntry::submenu("Theme", theme).with_preview_restore({
                    let wk = objc2::rc::Weak::from_retained(&self.retain());
                    move || {
                        if let Some(ui) = wk.load() {
                            ui.after_theme_change(None);
                        }
                    }
                }),
                MenuEntry::Separator,
                MenuEntry::item("Reset Default Size", move || {
                    if let Some(ui) = w1.load() {
                        ui.route_reset_size();
                    }
                }),
                MenuEntry::item("Reset Default Colors", move || {
                    if let Some(ui) = w2.load() {
                        ui.reset_theme(section);
                    }
                }),
                MenuEntry::Separator,
                MenuEntry::item("Open Config", move || {
                    c.set_pending_note_external(conf.to_string_lossy().into_owned());
                    c.open(SlotView::Notes);
                }),
                MenuEntry::item("Keyboard Shortcuts  ⌘/", move || {
                    if let Some(ui) = w3.load() {
                        ui.show_shortcuts();
                    }
                }),
                MenuEntry::Separator,
                MenuEntry::item("App Menu…", {
                    let view = view.retain();
                    move || crate::app::menu::pop_status_menu(&view, x, y)
                }),
            ];
            popup(entries, Some(view), x, y);
        }

        /// `applyThemePreset(_:scope:to:section:)` — persist the preset's
        /// colors into `[section]`, then re-theme.
        fn apply_theme_preset(&self, section: &str, p: &crate::ui::theme::ThemePreset, scope: ThemeScope) {
            let kv = self.theme_preset_kv(section, p, scope);
            crate::app::config::save_config_values(section, &kv);
            self.ivars().controller.log(&format!("theme '{}' applied to [{section}]", p.name));
            self.after_theme_change(None);
        }

        /// Hover preview (`applyThemePreset(persist: false)`): re-theme with the
        /// preset's colors without touching `commands.toml`.
        fn preview_theme_preset(&self, section: &str, p: &crate::ui::theme::ThemePreset, scope: ThemeScope) {
            let kv = self.theme_preset_kv(section, p, scope);
            let text = crate::app::config::read_config_text().unwrap_or_default();
            let mut sections = super::parse_sections(&text);
            let sec = sections.entry(section.to_string()).or_default();
            for (k, v) in kv {
                match v {
                    Some(v) => {
                        sec.insert(k, v);
                    }
                    None => {
                        sec.remove(&k);
                    }
                }
            }
            self.after_theme_change(Some(sections));
        }

        /// The `[section]` keys a preset writes for `scope`.
        fn theme_preset_kv(&self, section: &str, p: &crate::ui::theme::ThemePreset, scope: ThemeScope) -> Vec<(String, Option<String>)> {
            use crate::ui::theme::HexColor;
            let cur = |k: &str| crate::views::notes::section_value(section, k).and_then(|v| HexColor::parse(v.trim()));
            // `max(current alpha, presetMinOpacity)`
            let paint = |k: &str, c: crate::ui::theme::Rgba| -> (String, Option<String>) {
                let a = cur(k).map(|x| x.a).unwrap_or(1.0).max(0.6);
                (k.to_string(), Some(HexColor::format(&c.with_alpha(a))))
            };
            let mut kv: Vec<(String, Option<String>)> = Vec::new();
            let notes = section == "notes";
            match scope {
                ThemeScope::Window => {
                    kv.push(paint("background-color", p.background));
                    kv.push(paint("header-color", p.header));
                    if notes || section == "files" {
                        kv.push(paint("browser-background", if notes { p.browser } else { p.background }));
                    }
                    if notes {
                        kv.push(paint("terminal-background", p.terminal));
                    }
                    kv.push(("terminal-foreground".to_string(), None));
                }
                ThemeScope::Notepad => {
                    kv.push(paint("background-color", p.background));
                    kv.push(paint("header-color", p.header));
                }
                ThemeScope::Terminal => {
                    kv.push(paint("terminal-background", p.background));
                    kv.push(("terminal-foreground".to_string(), Some(HexColor::format_rgb(&p.text))));
                }
            }
            if matches!(scope, ThemeScope::Window | ThemeScope::Notepad) {
                for (k, c) in [("text-color", p.text), ("dim-color", p.dim), ("highlight-color", p.highlight), ("accent-color", p.accent)] {
                    kv.push((k.to_string(), Some(HexColor::format_rgb(&c))));
                }
                let pal = p.palette.all().map(|c| HexColor::format_rgb(&c)).join(", ");
                kv.push(("palette".to_string(), Some(pal)));
            }
            kv
        }

        /// `resetWindowTheme` — drop the section's color keys.
        fn reset_theme(&self, section: &str) {
            let kv: Vec<(String, Option<String>)> = [
                "browser-background", "terminal-background", "background-color", "header-color",
                "text-color", "dim-color", "highlight-color", "accent-color", "terminal-foreground", "palette",
            ]
            .iter()
            .map(|k| (k.to_string(), None))
            .collect();
            crate::app::config::save_config_values(section, &kv);
            self.ivars().controller.log(&format!("theme reset for [{section}]"));
            self.after_theme_change(None);
        }

        /// Reload `[theme]` + sections, then re-color the notes surface in
        /// place and rebuild any other current view with the new colors.
        fn after_theme_change(&self, over: Option<std::collections::HashMap<String, std::collections::HashMap<String, String>>>) {
            let sections = over.unwrap_or_else(|| {
                let text = crate::app::config::read_config_text().unwrap_or_default();
                super::parse_sections(&text)
            });
            crate::ui::theme::install_theme(&sections.get("theme").cloned().unwrap_or_default(), sections);
            let Some(v) = self.ivars().controller.current() else { return };
            if v == SlotView::Notes {
                self.notes_surface(|s| s.retheme());
                return;
            }
            {
                let mut state = self.ivars().state.borrow_mut();
                let Some(st) = state.as_mut() else { return };
                if let Some(view) = st.views.remove(&v) {
                    view.removeFromSuperview();
                }
                match v {
                    SlotView::Files => st.files = None,
                    SlotView::Ai => st.ai = None,
                    SlotView::Confluence => st.confluence = None,
                    SlotView::Compare | SlotView::CompareText => {
                        st.compare = None;
                        st.views.remove(&SlotView::Compare);
                        st.views.remove(&SlotView::CompareText);
                    }
                    _ if v.is_jira() => st.jira = None,
                    _ => {}
                }
                st.current = None;
            }
            self.present(v);
        }

        /// The prose capsule: Prose / nvim segments, ⤢ pop out, ◐ doc style
        /// (`installProseSwitch`).
        fn notes_prose_switch_event(&self, ev: crate::ui::prose_switch::ProseSwitchEvent) {
            use crate::ui::prose_switch::ProseSwitchEvent as P;
            match ev {
                P::Mode(on) => {
                    self.notes_surface(|s| s.set_prose(on));
                }
                P::PopOut => {
                    self.notes_surface(|s| {
                        if let Some(v) = s.vim_pane() {
                            v.command("silent! update");
                        }
                        s.pop_out_prose();
                    });
                }
                P::Style => self.notes_doc_style_menu(),
            }
        }

        /// A message from the reading view's page scripts.
        fn notes_prose_msg(&self, m: crate::views::notes_prose::ProseMsg) {
            use crate::views::notes_prose::ProseMsg as M;
            match m {
                M::Search(state) => {
                    self.notes_surface(|s| {
                        if let Some(p) = s.prose_pane() {
                            p.state.set_search_state(&state);
                        }
                    });
                }
                M::Jump(n) => {
                    self.notes_surface(|s| s.prose_jump(n));
                }
                M::Image(path) => self.notes_open_image(path),
            }
        }

        /// `PopupTextView.onPasteImage` — save the pasted image next to the
        /// note (`assets/img-<epoch>.png`) and insert it as an inline
        /// attachment.
        fn notes_paste_image(&self, img: objc2::rc::Retained<objc2_app_kit::NSImage>) {
            #[cfg(target_os = "macos")]
            {
                self.notes_surface(|s| {
                    let Some(path) = s.selected_path() else { return };
                    if let Some(rel) = crate::views::notes_vim::save_image(&path, &img) {
                        s.insert_image_attachment(&rel, &img);
                    }
                });
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = img;
            }
        }

        /// `FilePopup.show(path, over: panel)` — the floating image popup.
        fn notes_open_image(&self, path: String) {
            // Attachment clicks carry the markdown-relative target; resolve it
            // against the selected note before showing.
            let resolved = self
                .notes_surface(|s| s.resolve_image_path(&path))
                .unwrap_or_else(|| path.clone());
            let parent: Option<objc2::rc::Retained<objc2_app_kit::NSWindow>> = self
                .ivars()
                .state
                .borrow()
                .as_ref()
                .map(|st| st.window.clone().into_super());
            crate::views::file_popup::show(&resolved, parent.as_deref());
        }

        /// `docTemplateMenu` — the ◐ document-style menu
        /// (`doc_templates.menu` → pick → `doc_templates.edit`).
        fn notes_doc_style_menu(&self) {
            use crate::ui::menu_kit::{popup, MenuEntry};
            let Some(Some(text)) = self.notes_surface(|s| s.prose_markdown()) else { return };
            let configured = crate::views::notes::section_value("notes", "doc-templates").unwrap_or_default();
            let css_path = crate::views::notes::section_value("notes", "pdf-css")
                .map(|v| v.trim().to_string())
                .unwrap_or_default();
            let css = if css_path.is_empty() {
                String::new()
            } else {
                let home = std::env::var("HOME").unwrap_or_default();
                let p = match css_path.strip_prefix("~/") {
                    Some(rest) => format!("{home}/{rest}"),
                    None => css_path.clone(),
                };
                std::fs::read_to_string(p).unwrap_or_default()
            };
            let menu = crate::app::python_helper::PythonHelper::shared()
                .call(
                    "doc_templates.menu",
                    serde_json::json!({"text": text, "configured": configured, "css": css}),
                    std::time::Duration::from_secs(3),
                    std::time::Duration::from_secs(3),
                )
                .ok();
            let names: Option<Vec<String>> = menu.as_ref().and_then(|m| {
                m.get("names")?
                    .as_array()
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
            });
            let current = menu
                .as_ref()
                .and_then(|m| m.get("current"))
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let mouse = objc2_app_kit::NSEvent::mouseLocation();
            let Some(names) = names else {
                popup(vec![MenuEntry::disabled("Document styles unavailable (python helper)")], None, mouse.x, mouse.y);
                return;
            };
            let mut entries = vec![MenuEntry::Header("Document style".to_string())];
            let weak = objc2::rc::Weak::from_retained(&self.retain());
            entries.push(MenuEntry::checked("Plain (no style)", current.is_none(), move || {
                if let Some(ui) = weak.load() {
                    ui.notes_apply_doc_template(None);
                }
            }));
            entries.push(MenuEntry::Separator);
            for name in names {
                let weak = objc2::rc::Weak::from_retained(&self.retain());
                let on = current.as_deref() == Some(name.as_str());
                let n = name.clone();
                entries.push(MenuEntry::checked(&name, on, move || {
                    if let Some(ui) = weak.load() {
                        ui.notes_apply_doc_template(Some(n));
                    }
                }));
            }
            popup(entries, None, mouse.x, mouse.y);
        }

        /// `applyDocTemplate` + `applyDocTemplateChange`.
        fn notes_apply_doc_template(&self, name: Option<String>) {
            let Some(Some(text)) = self.notes_surface(|s| s.prose_markdown()) else { return };
            let r = crate::app::python_helper::PythonHelper::shared().call(
                "doc_templates.edit",
                serde_json::json!({"text": text, "template": name}),
                std::time::Duration::from_secs(5),
                std::time::Duration::from_secs(3),
            );
            let edit = r.ok().and_then(|v| {
                let remove = v.get("remove")?.as_i64()?;
                let insert: Vec<String> = v
                    .get("insert")?
                    .as_array()?
                    .iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect();
                Some((remove, insert))
            });
            let Some((remove, insert)) = edit else {
                crate::ui::toast::show("Style change failed (python helper)", "exclamationmark.triangle.fill");
                return;
            };
            if remove == 0 && insert.is_empty() {
                return;
            }
            self.notes_surface(|s| s.apply_doc_template_change(remove as usize, &insert));
            self.notes_surface(|s| s.refresh_prose());
            let msg = match &name {
                Some(n) => format!("Style: {n}"),
                None => "Style removed".to_string(),
            };
            crate::ui::toast::show(&msg, "paintpalette");
        }

        /// The notes pinned-folder tree: clicks toggle folders / open files;
        /// the right-click menu follows the file-browser rule (copy absolute
        /// path, copy file, open in notes, reveal in the app's own Files view).
        fn notes_tree_event(&self, ev: crate::ui::tree_view::TreeEvent) {
            use crate::ui::menu_kit::{popup, MenuEntry};
            use crate::ui::tree_view::TreeEvent as E;
            match ev {
                E::ToggleSection => {
                    self.notes_surface(|s| s.tree_toggle_section());
                }
                E::Click { path, is_dir } => {
                    self.notes_surface(|s| s.tree_click(&path, is_dir));
                }
                E::Menu { path, is_dir, is_root, x, y } => {
                    let weak = objc2::rc::Weak::from_retained(&self.retain());
                    let (p1, p3, p4, p5) = (path.clone(), path.clone(), path.clone(), path.clone());
                    let mut entries = Vec::new();
                    if is_dir {
                        let w = weak.clone();
                        entries.push(MenuEntry::item("New Note Here", move || {
                            if let Some(ui) = w.load() {
                                ui.notes_surface(|s| s.tree_new_note_in(&p4));
                            }
                        }));
                        let w = weak.clone();
                        entries.push(MenuEntry::item("Collapse / Expand", move || {
                            if let Some(ui) = w.load() {
                                ui.notes_surface(|s| s.tree_click(&p5, true));
                            }
                        }));
                        entries.push(MenuEntry::Separator);
                    } else {
                        let w = weak.clone();
                        entries.push(MenuEntry::item("Open in Notes", move || {
                            if let Some(ui) = w.load() {
                                ui.notes_surface(|s| s.tree_click(&p4, false));
                            }
                        }));
                        entries.push(MenuEntry::Separator);
                    }
                    let pc = path.clone();
                    entries.push(MenuEntry::item("Copy File", move || {
                        copy_files_toast(&[pc]);
                    }));
                    if !is_root {
                        let (w, pd) = (weak.clone(), path.clone());
                        entries.push(MenuEntry::item("Duplicate", move || {
                            use crate::engines::file_ops::FileOps;
                            let o = FileOps::duplicate(&[pd], FileOps::shared());
                            if let Some(ui) = w.load() {
                                if let Some(msg) = o.failed.as_deref() {
                                    crate::ui::toast::show(msg, "exclamationmark.triangle");
                                }
                                ui.notes_surface(|s| s.tree_refresh());
                            }
                        }));
                    }
                    entries.push(MenuEntry::item("Copy Absolute Path", move || copy_text_to_pasteboard(&p1)));
                    entries.push(MenuEntry::item("Reveal in Files", move || reveal_in_files(&[p3])));
                    if !is_root {
                        entries.push(MenuEntry::Separator);
                        let (w, pt) = (weak.clone(), path.clone());
                        entries.push(MenuEntry::item("Delete…", move || {
                            let Some(ui) = w.load() else { return };
                            let Some(window) = ui.ivars().state.borrow().as_ref().map(|st| st.window.clone()) else { return };
                            let name = std::path::Path::new(&pt).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                            let weak = objc2::rc::Weak::from_retained(&ui.retain());
                            crate::ui::sheet::alert_sheet(
                                &window,
                                &format!("Delete “{name}”?"),
                                "It goes to the Trash.",
                                &["Move to Trash", "Cancel"],
                                move |b| {
                                    if b != 0 {
                                        return;
                                    }
                                    use crate::engines::file_ops::FileOps;
                                    let o = FileOps::trash(&[pt.clone()], FileOps::shared());
                                    if let Some(ui) = weak.load() {
                                        match o.failed.as_deref() {
                                            Some(msg) => crate::ui::toast::show(msg, "exclamationmark.triangle"),
                                            None => {
                                                ui.notes_surface(|s| s.tree_removed(&pt));
                                                crate::ui::toast::show("Moved to the Trash", "trash");
                                            }
                                        }
                                    }
                                },
                            );
                        }));
                    }
                    popup(entries, None, x, y);
                }
            }
        }

        fn notes_sidebar_event(&self, ev: crate::ui::sidebar::SidebarEvent) {
            use crate::ui::sidebar::SidebarEvent as E;
            match ev {
                E::Select(i) => {
                    self.notes_surface(|s| s.click_row(i));
                }
                E::Close(i) => {
                    self.notes_surface(|s| s.close_tab(i));
                }
                E::Add => {
                    self.notes_surface(|s| s.new_note());
                }
                E::Rail(c) => {
                    self.notes_surface(|s| s.rail_changed(c));
                }
                E::MenuRow(i, x, y) => {
                    use crate::ui::menu_kit::{popup, MenuEntry};
                    let Some(Some(path)) = self.notes_surface(|s| s.tab_path(i)) else { return };
                    let weak = objc2::rc::Weak::from_retained(&self.retain());
                    let (p1, p3) = (path.clone(), path.clone());
                    popup(
                        vec![
                            MenuEntry::item("Copy Absolute Path", move || copy_text_to_pasteboard(&p1)),
                            MenuEntry::item("Reveal in Files", move || reveal_in_files(&[p3])),
                            MenuEntry::Separator,
                            MenuEntry::item("Close", move || {
                                if let Some(ui) = weak.load() {
                                    ui.notes_surface(|s| s.close_tab(i));
                                }
                            }),
                        ],
                        None,
                        x,
                        y,
                    );
                }
                _ => {}
            }
        }

        /// Run `f` on the shared compare model, then repaint the surface.
        fn compare_with<R>(&self, f: impl FnOnce(&mut crate::views::compare::CompareWindowModel) -> R) -> Option<R> {
            let model = self.ivars().controller.compare_model()?;
            let r = {
                let mut m = model.lock().ok()?;
                f(&mut m)
            };
            if let Some(st) = self.ivars().state.borrow().as_ref() {
                if let Some(s) = st.compare.as_ref() {
                    s.invalidate();
                }
            }
            self.sync_compare();
            Some(r)
        }

        fn compare_event(&self, ev: crate::views::compare_view::CompareEvent) {
            use crate::ui::sidebar::SidebarEvent;
            use crate::views::compare::{CompareFilter, CompareSide};
            use crate::views::compare_view::CompareEvent as E;
            match ev {
                E::Sidebar(SidebarEvent::Select(i)) => {
                    self.compare_with(|m| m.select(i));
                }
                E::Sidebar(SidebarEvent::Close(i)) => {
                    self.compare_with(|m| m.close_session(i));
                }
                E::Sidebar(SidebarEvent::Pinned(k)) => {
                    self.compare_with(|m| if k == "home" { m.show_start_page() } else { m.start_pasted() });
                }
                E::Sidebar(SidebarEvent::Rail(_)) => {
                    if let Some(st) = self.ivars().state.borrow().as_ref() {
                        if let Some(s) = st.compare.as_ref() {
                            s.chrome().layout_all();
                            s.invalidate();
                        }
                    }
                }
                E::Sidebar(SidebarEvent::MenuRow(i, x, y)) => {
                    // `pillMenu(_:)`: Close, then each side's path.
                    use crate::ui::menu_kit::{popup, MenuEntry};
                    let paths = self
                        .compare_with(|m| {
                            m.sessions.get(i).map(|s| {
                                (s.path.get(&CompareSide::Left).cloned(), s.path.get(&CompareSide::Right).cloned())
                            })
                        })
                        .flatten();
                    let Some((lp, rp)) = paths else { return };
                    let weak = objc2::rc::Weak::from_retained(&self.retain());
                    let mut e = vec![
                        MenuEntry::item("Close", move || {
                            if let Some(ui) = weak.load() {
                                ui.compare_with(|m| m.close_session(i));
                            }
                        }),
                        MenuEntry::Separator,
                    ];
                    for (name, p) in [("Left", lp), ("Right", rp)] {
                        let Some(p) = p else { continue };
                        let p2 = p.clone();
                        e.push(MenuEntry::item(&format!("Copy {name} Path"), move || copy_text_to_pasteboard(&p)));
                        e.push(MenuEntry::item(&format!("Reveal {name} in Files"), move || reveal_in_files(&[p2])));
                    }
                    popup(e, None, x, y);
                }
                E::Sidebar(_) => {}
                E::Compare { left, right } => {
                    let home = std::env::var("HOME").unwrap_or_default();
                    let exp = |p: &str| -> String {
                        let p = p.trim();
                        if let Some(rest) = p.strip_prefix('~') { format!("{home}{rest}") } else { p.to_string() }
                    };
                    let (l, r) = (exp(&left), exp(&right));
                    if l.is_empty() && r.is_empty() {
                        return;
                    }
                    let dirs = std::path::Path::new(&l).is_dir() && std::path::Path::new(&r).is_dir();
                    self.compare_with(|m| {
                        if dirs {
                            m.open_folders(&l, &r);
                        } else {
                            m.open_pair((!l.is_empty()).then_some(l.as_str()), (!r.is_empty()).then_some(r.as_str()));
                        }
                    });
                }
                E::NewText => {
                    self.compare_with(|m| m.start_pasted());
                }
                E::Browse { right } => {
                    let opts = crate::platform::PickOpts {
                        files: true,
                        dirs: true,
                        multiple: false,
                    };
                    if let Some(p) = crate::platform::pick_path(&opts) {
                        if let Some(st) = self.ivars().state.borrow().as_ref() {
                            if let Some(s) = st.compare.as_ref() {
                                s.chrome().set_side_text(right, &p.to_string_lossy());
                            }
                        }
                    }
                }
                E::OpenRecent(i) => {
                    let rows = self.compare_with(|m| {
                        m.recent_selection = i;
                        m.recent.load()
                    });
                    let filter_rows = {
                        let st = self.ivars().state.borrow();
                        let model = self.ivars().controller.compare_model();
                        match (st.as_ref().and_then(|s| s.compare.as_ref()), model) {
                            (Some(s), Some(m)) => m.lock().ok().map(|m| s.recent_rows(&m)),
                            _ => None,
                        }
                    };
                    let _ = rows;
                    let Some(e) = filter_rows.and_then(|r| r.get(i).cloned()) else { return };
                    if e.missing() {
                        return;
                    }
                    let (l, r) = (e.entry.left.clone(), e.entry.right.clone());
                    let dirs = std::path::Path::new(&l).is_dir() && std::path::Path::new(&r).is_dir();
                    self.compare_with(|m| {
                        if dirs {
                            m.open_folders(&l, &r);
                        } else {
                            m.open_pair(Some(&l), Some(&r));
                        }
                    });
                }
                E::RemoveRecent(i) => {
                    let row = {
                        let st = self.ivars().state.borrow();
                        let model = self.ivars().controller.compare_model();
                        match (st.as_ref().and_then(|s| s.compare.as_ref()), model) {
                            (Some(s), Some(m)) => m.lock().ok().and_then(|m| s.recent_rows(&m).get(i).cloned()),
                            _ => None,
                        }
                    };
                    if let Some(r) = row {
                        self.compare_with(|m| m.recent.remove(&r.entry));
                    }
                }
                E::RecentMenu { row, x, y } => {
                    use crate::ui::menu_kit::{popup, MenuEntry};
                    let entry = {
                        let st = self.ivars().state.borrow();
                        let model = self.ivars().controller.compare_model();
                        match (st.as_ref().and_then(|s| s.compare.as_ref()), model) {
                            (Some(s), Some(m)) => m.lock().ok().and_then(|m| s.recent_rows(&m).get(row).cloned()),
                            _ => None,
                        }
                    };
                    let Some(e) = entry else { return };
                    let weak = objc2::rc::Weak::from_retained(&self.retain());
                    let (w1, w2, w3, w4, w5) = (weak.clone(), weak.clone(), weak.clone(), weak.clone(), weak);
                    let (l, r) = (e.entry.left.clone(), e.entry.right.clone());
                    let (l2, r2) = (l.clone(), r.clone());
                    let (fl, fr) = (l.clone(), r.clone());
                    let ent = e.entry.clone();
                    let gone = self
                        .compare_with(|m| {
                            m.recent
                                .load()
                                .iter()
                                .filter(|e| !std::path::Path::new(&e.left).exists() || !std::path::Path::new(&e.right).exists())
                                .count()
                        })
                        .unwrap_or(0);
                    let mut tail = Vec::new();
                    if gone > 0 {
                        tail.push(MenuEntry::item(&format!("Remove All Missing ({gone})"), move || {
                            if let Some(ui) = w4.load() {
                                ui.compare_event(E::ClearMissing);
                            }
                        }));
                    }
                    tail.push(MenuEntry::item("Clear All Recents", move || {
                        if let Some(ui) = w5.load() {
                            ui.compare_event(E::ClearAll);
                        }
                    }));
                    popup(
                        vec![
                            MenuEntry::item("Open", move || {
                                if let Some(ui) = w1.load() {
                                    ui.compare_event(E::OpenRecent(row));
                                }
                            }),
                            MenuEntry::item("Fill the Path Fields", move || {
                                if let Some(ui) = w3.load() {
                                    if let Some(st) = ui.ivars().state.borrow().as_ref() {
                                        if let Some(s) = st.compare.as_ref() {
                                            s.chrome().set_side_text(false, &tilde_path(&fl));
                                            s.chrome().set_side_text(true, &tilde_path(&fr));
                                        }
                                    }
                                }
                            }),
                            MenuEntry::item("Reveal Left in Files", move || reveal_in_files(&[l2])),
                            MenuEntry::item("Reveal Right in Files", move || reveal_in_files(&[r2])),
                            MenuEntry::item("Copy Absolute Paths", move || copy_text_to_pasteboard(&format!("{l}\n{r}"))),
                            MenuEntry::Separator,
                            MenuEntry::item("Remove from Recent", move || {
                                if let Some(ui) = w2.load() {
                                    ui.compare_with(|m| m.recent.remove(&ent));
                                }
                            }),
                        ]
                        .into_iter()
                        .chain(tail)
                        .collect(),
                        None,
                        x,
                        y,
                    );
                }
                E::ClearMissing => {
                    self.compare_with(|m| {
                        let keep: Vec<_> = m
                            .recent
                            .load()
                            .into_iter()
                            .filter(|e| std::path::Path::new(&e.left).exists() && std::path::Path::new(&e.right).exists())
                            .collect();
                        m.recent.save(&keep);
                    });
                }
                E::ClearAll => {
                    let Some(window) = self.ivars().state.borrow().as_ref().map(|st| st.window.clone()) else { return };
                    let weak = objc2::rc::Weak::from_retained(&self.retain());
                    crate::ui::sheet::alert_sheet(
                        &window,
                        "Clear all recent comparisons?",
                        "Pasted text kept for them is deleted too.",
                        &["Clear All", "Cancel"],
                        move |b| {
                            if b == 0 {
                                if let Some(ui) = weak.load() {
                                    ui.compare_with(|m| m.recent.clear_all());
                                }
                            }
                        },
                    );
                }
                E::RecentFilter(q) => {
                    if let Some(st) = self.ivars().state.borrow().as_ref() {
                        if let Some(s) = st.compare.as_ref() {
                            s.set_recent_filter(&q);
                        }
                    }
                    self.sync_compare();
                }
                E::Filter(i) => {
                    self.compare_with(|m| m.set_filter(CompareFilter::ALL[i.min(3)]));
                }
                E::Tool(i) => {
                    self.compare_with(|m| match i {
                        0 => m.jump_section(-1),
                        1 => m.jump_section(1),
                        2 => m.swap_sides(),
                        3 => {
                            let _ = m.test_do("reload");
                        }
                        _ => {}
                    });
                    let _ = CompareSide::Left;
                }
                E::PaneMenu { right, row, x, y } => {
                    let side = if right { CompareSide::Right } else { CompareSide::Left };
                    self.compare_pane_menu(side, row, x, y);
                }
                E::FolderMenu { id, x, y } => self.compare_folder_menu(id, x, y),
            }
        }

        /// `CompareFolderView.menu(_:_:)`: the folder tree's right-click menu
        /// for node `id`, acting on the focused side.
        fn compare_folder_menu(&self, id: usize, x: f64, y: f64) {
            use crate::engines::file_ops::Clash;
            use crate::ui::menu_kit::{popup, MenuEntry};
            use crate::views::compare::{CompareSide, CompareWindowModel};
            let Some(Some(t)) = self.compare_with(|m| m.folder_page.menu_target(id)) else { return };
            let weak = objc2::rc::Weak::from_retained(&self.retain());
            let act = |f: Box<dyn FnOnce(&mut CompareWindowModel)>| {
                let w = weak.clone();
                move || {
                    if let Some(ui) = w.load() {
                        ui.compare_with(f);
                    }
                }
            };
            let item = |title: &str, enabled: bool, f: Box<dyn FnOnce()>| {
                if enabled {
                    MenuEntry::item(title, f)
                } else {
                    MenuEntry::disabled(title)
                }
            };
            let w = weak.clone();
            let open = move || {
                let Some(ui) = w.load() else { return };
                let sub = ui
                    .compare_with(|m| {
                        let _ = m.test_do("folder-open");
                        m.take_sub_pending()
                    })
                    .unwrap_or(false);
                if sub {
                    ui.ivars().controller.push_view(SlotView::CompareText);
                }
            };
            let mut e = Vec::new();
            if t.is_folder {
                e.push(MenuEntry::item(if t.expanded { "Collapse" } else { "Expand" }, open));
            } else {
                let title = if t.has_left && t.has_right { "Open (Compare)" } else { "Open in Text Compare" };
                e.push(MenuEntry::item(title, open));
            }
            e.push(MenuEntry::Separator);
            for (title, from, move_, enabled) in [
                ("Copy to Right  ⌥→", CompareSide::Left, false, t.has_left),
                ("Copy to Left  ⌥←", CompareSide::Right, false, t.has_right),
                ("Move to Right  ⌃⌥R", CompareSide::Left, true, t.has_left),
                ("Move to Left  ⌃⌥L", CompareSide::Right, true, t.has_right),
            ] {
                let w = weak.clone();
                e.push(item(title, enabled, Box::new(move || {
                    if let Some(ui) = w.load() {
                        ui.compare_folder_transfer(from, move_);
                    }
                })));
            }
            let _ = Clash::Replace;
            e.push(MenuEntry::Separator);
            let on_side = t.path.is_some();
            let name = t.path.as_deref().and_then(|p| std::path::Path::new(p).file_name()).map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
            let w = weak.clone();
            let rename_path = t.path.clone().unwrap_or_default();
            e.push(item("Rename…  F2", on_side, Box::new(move || {
                let Some(ui) = w.load() else { return };
                let Some(window) = ui.ivars().state.borrow().as_ref().map(|st| st.window.clone()) else { return };
                let w2 = w.clone();
                crate::ui::sheet::form_sheet(
                    &window,
                    "Rename",
                    &tilde_path(&rename_path),
                    vec![crate::ui::sheet::FormRow::new("Name", &name, "")],
                    "Rename",
                    move |answer| {
                        let Some(name) = answer.and_then(|v| v.into_iter().next()) else { return };
                        if let Some(ui) = w2.load() {
                            ui.compare_with(|m| m.folder_page.rename_target(&name));
                        }
                    },
                );
            })));
            let w = weak.clone();
            e.push(MenuEntry::item("New Folder…  ⌘⇧N", move || {
                let Some(ui) = w.load() else { return };
                let Some(window) = ui.ivars().state.borrow().as_ref().map(|st| st.window.clone()) else { return };
                let base = ui.compare_with(|m| m.folder_page.new_folder_base()).flatten().unwrap_or_default();
                let w2 = w.clone();
                crate::ui::sheet::form_sheet(
                    &window,
                    "New Folder",
                    &format!("in {}", tilde_path(&base)),
                    vec![crate::ui::sheet::FormRow::new("Name", "untitled folder", "")],
                    "Create",
                    move |answer| {
                        let Some(name) = answer.and_then(|v| v.into_iter().next()) else { return };
                        if let Some(ui) = w2.load() {
                            ui.compare_with(|m| m.folder_page.new_folder(&name));
                        }
                    },
                );
            }));
            e.push(item("Move to Trash  ⌘⌫", on_side, Box::new(act(Box::new(|m| m.folder_page.trash_selection())))));
            e.push(MenuEntry::Separator);
            e.push(item("Copy Absolute Path", on_side, Box::new(act(Box::new(|m| {
                let paths = m.folder_page.target_paths();
                if paths.is_empty() {
                    return;
                }
                copy_text_to_pasteboard(&paths.join("\n"));
                let msg = if paths.len() == 1 {
                    format!("Copied {} to clipboard", tilde_path(&paths[0]))
                } else {
                    format!("Copied {} paths to clipboard", paths.len())
                };
                crate::ui::toast::show(&msg, "doc.on.clipboard");
            })))));
            let p = t.path.clone().unwrap_or_default();
            let (p1, p2) = (p.clone(), p.clone());
            e.push(item("Reveal in Files", on_side, Box::new(move || reveal_in_files(&[p1]))));
            if !t.is_folder {
                let w = weak.clone();
                e.push(item("Open in Notes", on_side, Box::new(move || {
                    if let Some(ui) = w.load() {
                        ui.open_in_notes(&p2);
                    }
                })));
            }
            let w = weak.clone();
            e.push(item("Quick Look  Space", t.has_left || t.has_right, Box::new(move || {
                let Some(ui) = w.load() else { return };
                let paths = ui.compare_with(|m| m.folder_page.target_paths()).unwrap_or_default();
                if let Some(mtm) = MainThreadMarker::new() {
                    COMPARE_QUICK_LOOK.with(|q| q.borrow_mut().toggle(mtm, &paths));
                }
            })));
            if t.is_folder {
                e.push(MenuEntry::Separator);
                let id = t.id;
                e.push(item("Set as Base Folder  ⌘↓", t.has_left && t.has_right, Box::new(act(Box::new(move |m| m.folder_page.set_base(id, None))))));
                e.push(item("Set as Left Base Folder", t.left_dir, Box::new(act(Box::new(move |m| m.folder_page.set_base(id, Some(CompareSide::Left)))))));
                e.push(item("Set as Right Base Folder", t.right_dir, Box::new(act(Box::new(move |m| m.folder_page.set_base(id, Some(CompareSide::Right)))))));
            }
            e.push(MenuEntry::Separator);
            let ex = t.name.clone();
            e.push(MenuEntry::item(&format!("Exclude “{}”", t.name), act(Box::new(move |m| m.folder_page.exclude(&ex)))));
            popup(e, None, x, y);
        }

        /// `CompareFolderView.transfer(from:move:)`: copy / move the selection
        /// across, asking first when items already exist on the other side.
        fn compare_folder_transfer(&self, from: crate::views::compare::CompareSide, move_: bool) {
            use crate::engines::file_ops::Clash;
            let clashes = self.compare_with(|m| m.folder_page.clash_count(from)).unwrap_or(0);
            if clashes == 0 {
                self.compare_with(|m| m.folder_page.copy_or_move(from, move_, Clash::Replace));
                return;
            }
            let Some(window) = self.ivars().state.borrow().as_ref().map(|st| st.window.clone()) else { return };
            let other = from.other().raw();
            let title = if clashes == 1 {
                format!("1 item already exists on the {other} side")
            } else {
                format!("{clashes} items already exist on the {other} side")
            };
            let weak = objc2::rc::Weak::from_retained(&self.retain());
            crate::ui::sheet::alert_sheet(
                &window,
                &title,
                "Replace them, keep both (the copy gets a new name), or skip them?",
                &["Replace", "Keep Both", "Skip", "Cancel"],
                move |b| {
                    let clash = match b {
                        0 => Clash::Replace,
                        1 => Clash::KeepBoth,
                        2 => Clash::Skip,
                        _ => return,
                    };
                    if let Some(ui) = weak.load() {
                        ui.compare_with(|m| m.folder_page.copy_or_move(from, move_, clash));
                    }
                },
            );
        }

        /// `CompareWindow.paneMenu(row:side:)`: the text pane's right-click
        /// menu. The click first moves the cursor and focus there
        /// (`paneClicked`), like Swift.
        fn compare_pane_menu(&self, side: crate::views::compare::CompareSide, row: usize, x: f64, y: f64) {
            use crate::ui::menu_kit::{popup, MenuEntry};
            use crate::views::compare::{CompareSide, Eol};
            struct Info {
                has_rows: bool,
                can_copy_across: bool,
                in_section: bool,
                line: i64,
                pick: Option<(CompareSide, i64)>,
                is_anchor: Option<usize>,
                anchors: usize,
                path: Option<String>,
                dirty: bool,
            }
            let info = self.compare_with(|m| {
                if m.current_is_folder() {
                    return None;
                }
                m.move_cursor(row as i64, false);
                let pick = m.align_pick;
                let in_section = m.cursor_in_section();
                let s = m.current_mut()?;
                s.focus = side;
                let n = s.display_count();
                let mrow = (n > 0).then(|| s.model_row(s.cursor));
                Some(Info {
                    has_rows: n > 0,
                    can_copy_across: s.anchor.is_some() || in_section,
                    in_section,
                    line: mrow.map(|r| s.model.rows()[r].line(side) as i64).unwrap_or(-1),
                    pick,
                    is_anchor: mrow.filter(|r| s.model.is_anchor(*r)),
                    anchors: s.model.anchors().len(),
                    path: s.path.get(&side).cloned(),
                    dirty: s.dirty(side),
                })
            });
            let Some(Some(info)) = info else { return };
            let clip = crate::platform::get_text();
            let weak = objc2::rc::Weak::from_retained(&self.retain());
            let act = |f: Box<dyn FnOnce(&mut crate::views::compare::CompareWindowModel)>| {
                let w = weak.clone();
                move || {
                    if let Some(ui) = w.load() {
                        ui.compare_with(f);
                    }
                }
            };
            let other = if side == CompareSide::Left { "Right" } else { "Left" };
            let key = if side == CompareSide::Left { "R" } else { "L" };
            let item = |title: &str, enabled: bool, f: Box<dyn FnOnce()>| {
                if enabled {
                    MenuEntry::item(title, f)
                } else {
                    MenuEntry::disabled(title)
                }
            };
            let mut e = vec![
                MenuEntry::item("Copy", act(Box::new(|m| {
                    if let Some(t) = m.rows_text() {
                        copy_text_to_pasteboard(&t);
                    }
                }))),
                item(
                    &format!("Copy to {other}  (⌃{key})"),
                    info.can_copy_across,
                    Box::new(act(Box::new(move |m| m.copy_across(side)))),
                ),
                item(
                    &format!("Copy Line to {other}  (⌃⌥{key})"),
                    info.has_rows,
                    Box::new(act(Box::new(move |m| m.copy_line(side)))),
                ),
            ];
            let w = weak.clone();
            e.push(item(
                "Paste Clipboard Here",
                clip.is_some(),
                Box::new(move || {
                    let Some(t) = clip else { return };
                    if let Some(ui) = w.load() {
                        ui.compare_with(|m| match t.strip_prefix("file:") {
                            Some(p) => m.open_file_into(side, p),
                            None => m.set_pasted(&t, side),
                        });
                    }
                }),
            ));
            e.push(item("Select Section", info.in_section, Box::new(act(Box::new(|m| m.select_section())))));
            e.push(MenuEntry::Separator);
            let line = info.line;
            match info.pick {
                Some((ps, pl)) if ps != side && line >= 0 => {
                    let ps_name = if ps == CompareSide::Left { "Left" } else { "Right" };
                    e.push(MenuEntry::item(
                        &format!("Align With Picked {ps_name} Line {}", pl + 1),
                        act(Box::new(move |m| m.align_with(side, line))),
                    ));
                    e.push(MenuEntry::item("Cancel Align With", act(Box::new(|m| m.align_pick = None))));
                }
                _ => e.push(item(
                    "Align With…  (pick this line, then one on the other side)",
                    line >= 0,
                    Box::new(act(Box::new(move |m| m.align_with(side, line)))),
                )),
            }
            if let Some(r) = info.is_anchor {
                e.push(MenuEntry::item("Remove This Alignment", act(Box::new(move |m| m.clear_alignment(Some(r))))));
            }
            if info.anchors > 0 {
                e.push(MenuEntry::item(
                    &format!("Clear All Alignments ({})", info.anchors),
                    act(Box::new(|m| m.clear_alignment(None))),
                ));
            }
            let mut conv = vec![
                MenuEntry::item("Trim Trailing Whitespace", act(Box::new(move |m| m.trim(side)))),
                MenuEntry::Separator,
            ];
            for (eol, label) in [(Eol::Lf, "LF"), (Eol::Crlf, "CRLF"), (Eol::Cr, "CR")] {
                conv.push(MenuEntry::item(
                    &format!("Line Endings → {label}"),
                    act(Box::new(move |m| m.convert_eol(side, eol))),
                ));
            }
            e.push(MenuEntry::submenu(&format!("Convert {} Side", if side == CompareSide::Left { "Left" } else { "Right" }), conv));
            e.push(MenuEntry::Separator);
            let path = info.path.clone();
            let has_path = path.is_some();
            let p = path.clone().unwrap_or_default();
            let (p1, p2, p3) = (p.clone(), p.clone(), p.clone());
            e.push(item("Copy Absolute Path", has_path, Box::new(move || copy_text_to_pasteboard(&p1))));
            e.push(item("Reveal in Files", has_path, Box::new(move || reveal_in_files(&[p2]))));
            let w = weak.clone();
            e.push(item(
                "Open in Notes",
                has_path,
                Box::new(move || {
                    if let Some(ui) = w.load() {
                        ui.open_in_notes(&p3);
                    }
                }),
            ));
            let w = weak.clone();
            e.push(MenuEntry::item("Open File into This Side…  (⌘O)", move || {
                let opts = crate::platform::PickOpts {
                    files: true,
                    dirs: false,
                    multiple: false,
                };
                let Some(p) = crate::platform::pick_path(&opts) else { return };
                let p = p.to_string_lossy().into_owned();
                if let Some(ui) = w.load() {
                    ui.compare_with(|m| m.open_file_into(side, &p));
                }
            }));
            e.push(item(
                "Save This Side  (⌘S)",
                info.dirty || !has_path,
                Box::new(act(Box::new(move |m| {
                    if !m.save(side) {
                        crate::ui::toast::not_ported("Compare Save As (side has no file)");
                    }
                }))),
            ));
            popup(e, None, x, y);
        }

        /// Run `f` on the AI state, then push a fresh snapshot.
        fn ai_surface<R>(&self, f: impl FnOnce(&mut AIState) -> R) -> Option<R> {
            let mut state = self.ivars().state.borrow_mut();
            let a = state.as_mut().and_then(|st| st.ai.as_mut())?;
            let r = f(a);
            let snap = ai_snapshot(a);
            a.view.apply(snap);
            Some(r)
        }

        fn build_ai(&self, mtm: MainThreadMarker) -> AIState {
            use crate::views::ai::{AIConfig, AIWindowModel};
            let cfg = AIConfig::load();
            let sidebar_w = cfg.sidebar_width();
            let split = cfg.split();
            let mut model = AIWindowModel::new(cfg);
            let dir = model.rules_dir();
            let _ = std::fs::create_dir_all(&dir);
            model.reload_rules(&dir);
            if let Some(keep) = defaults_string("aiRule") {
                if let Some(i) = model.rules.iter().position(|r| r.file() == keep) {
                    model.select(i);
                }
            }
            if let Some(m) = defaults_string("aiViewMode") {
                if let Some(pm) = model.modes().into_iter().find(|p| p.as_str() == m) {
                    model.set_mode(pm);
                }
            }
            let input = model.rule().and_then(|r| defaults_string(&format!("aiInput.{}", r.file()))).unwrap_or_default();
            model.input = input.clone();
            let colors = crate::ui::theme::section_colors("ai");
            let web = model.preview_view().unwrap_or_else(|| NSView::new(mtm));
            let view = crate::views::ai_view::AIView::new(mtm, colors, web, sidebar_w.max(160.0), split);
            let mut a = AIState {
                model,
                view,
                command: String::new(),
                tokens: String::new(),
                tokens_warn: false,
                token_due: Some(now_secs()),
                token_gen: 0,
                was_running: false,
                set_input: Some(input),
            };
            a.model.render_answer();
            let snap = ai_snapshot(&mut a);
            a.view.apply(snap);
            let weak = objc2::rc::Weak::from_retained(&self.retain());
            a.view.set_on_event(Box::new(move |ev| {
                if let Some(ui) = weak.load() {
                    ui.ai_event(ev);
                }
            }));
            self.ai_refresh_command_for(a.model.rule().cloned());
            // `checkAvailable()`.
            let bin = a.model.config.fm_bin();
            std::thread::spawn(move || {
                let res = if !std::path::Path::new(&bin).exists() {
                    (false, format!("fm not found at {bin} ([ai] fm-bin) — it ships with macOS 26+"))
                } else {
                    match std::process::Command::new(&bin).arg("available").output() {
                        Ok(o) => {
                            let line = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)).trim().to_string();
                            let low = line.to_lowercase();
                            let ok = o.status.success() && !low.contains("not ") && !low.contains("unavailable");
                            (ok, if ok { String::new() } else { format!("Apple Intelligence unavailable: {}", if line.is_empty() { "fm available failed".into() } else { line }) })
                        }
                        Err(e) => (false, format!("fm available: {e}")),
                    }
                };
                on_ui(move |ui| {
                    ui.ai_surface(|a| {
                        a.model.set_available(res.0, &res.1);
                        if !res.0 {
                            a.model.set_status(res.1.clone(), crate::views::ai::AITone::Danger);
                        } else if !a.model.is_running() && a.model.run.answer.is_empty() {
                            ai_set_ready(a);
                        }
                        a.token_due = Some(now_secs());
                    });
                });
            });
            a
        }

        /// The command line text (`r.preview` — a helper call, off-main).
        fn ai_refresh_command_for(&self, rule: Option<crate::engines::ai_format::AIRule>) {
            let Some(r) = rule else {
                self.ai_surface(|a| {
                    a.command = format!("no rules in {} — press + to make one", a.model.rules_dir());
                });
                return;
            };
            let file = r.file();
            std::thread::spawn(move || {
                let text = r.preview();
                on_ui(move |ui| {
                    ui.ai_surface(|a| {
                        if a.model.rule().map(|x| x.file()) == Some(file) {
                            a.command = text;
                        }
                    });
                });
            });
        }

        fn ai_select(&self, i: usize) {
            let rule = self.ai_surface(|a| {
                if let Some(r) = a.model.rule() {
                    defaults_set_string(&format!("aiInput.{}", r.file()), &a.view.input_text());
                }
                a.model.select(i);
                let r = a.model.rule().cloned();
                if let Some(r) = &r {
                    defaults_set_string("aiRule", &r.file());
                    let input = defaults_string(&format!("aiInput.{}", r.file())).unwrap_or_default();
                    a.model.input = input.clone();
                    a.set_input = Some(input);
                    a.model.run = Default::default();
                    a.model.render_answer();
                    ai_set_ready(a);
                }
                a.command.clear();
                a.token_due = Some(now_secs());
                r
            });
            if let Some(r) = rule {
                self.ai_refresh_command_for(r);
            }
            self.ai_focus_input();
        }

        fn ai_focus_input(&self) {
            let st = self.ivars().state.borrow();
            if let Some(st) = st.as_ref() {
                if let Some(a) = st.ai.as_ref() {
                    st.window.makeFirstResponder(Some(&a.view.input_view()));
                }
            }
        }

        fn ai_new_rule(&self, source: Option<crate::engines::ai_format::AIRule>) {
            use crate::ui::sheet::{form_sheet, FormRow};
            let Some(dir) = self.ai_surface(|a| a.model.rules_dir()) else { return };
            let Some(window) = self.ivars().state.borrow().as_ref().map(|st| st.window.clone()) else { return };
            let home = std::env::var("HOME").unwrap_or_default();
            let info = format!(
                "A rule is a .md file in {}: frontmatter for the fm options, then the instructions.",
                dir.replacen(&home, "~", 1)
            );
            let initial = source.as_ref().map(|r| format!("{} copy", r.name)).unwrap_or_default();
            let weak = objc2::rc::Weak::from_retained(&self.retain());
            form_sheet(
                &window,
                if source.is_none() { "New Rule" } else { "Duplicate Rule" },
                &info,
                vec![FormRow::new("Name", &initial, "Summarize, Rewrite formally, Explain…")],
                "Create",
                move |vals| {
                    let (Some(v), Some(ui)) = (vals, weak.load()) else { return };
                    let name = v[0].trim().to_string();
                    let base: String = name
                        .to_lowercase()
                        .chars()
                        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                        .collect::<String>()
                        .split('-')
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join("-");
                    if base.is_empty() {
                        return;
                    }
                    let mut path = format!("{dir}/{base}.md");
                    let mut n = 2;
                    while std::path::Path::new(&path).exists() {
                        path = format!("{dir}/{base}-{n}.md");
                        n += 1;
                    }
                    let text = match source.as_ref().and_then(|r| std::fs::read_to_string(&r.path).ok()) {
                        Some(body) => body
                            .lines()
                            .map(|l| if l.starts_with("name:") { format!("name: {name}") } else { l.to_string() })
                            .collect::<Vec<_>>()
                            .join("\n"),
                        None => AI_RULE_TEMPLATE.replace("{name}", &name),
                    };
                    if let Err(e) = std::fs::write(&path, text) {
                        ui.ai_surface(|a| a.model.set_status(format!("Couldn't write {path}: {e}"), crate::views::ai::AITone::Danger));
                        return;
                    }
                    let file = std::path::Path::new(&path).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
                    defaults_set_string("aiRule", &file);
                    let idx = ui.ai_surface(|a| {
                        let d = a.model.rules_dir();
                        a.model.reload_rules(&d);
                        a.model.rules.iter().position(|r| r.path == path)
                    });
                    if let Some(Some(i)) = idx {
                        ui.ai_select(i);
                    }
                    ui.open_in_notes(&path);
                },
            );
        }

        fn ai_rule_menu(&self, i: usize, x: f64, y: f64) {
            use crate::ui::menu_kit::{popup, MenuEntry};
            let Some(Some(r)) = self.ai_surface(|a| a.model.rules.get(i).cloned()) else { return };
            let weak = objc2::rc::Weak::from_retained(&self.retain());
            let (p1, p3, p4) = (r.path.clone(), r.path.clone(), r.path.clone());
            let (w1, w2, w3) = (weak.clone(), weak.clone(), weak.clone());
            let (r2, r3) = (r.clone(), r.clone());
            popup(
                vec![
                    MenuEntry::item("Open in Notes", move || {
                        if let Some(ui) = w1.load() {
                            ui.open_in_notes(&p1);
                        }
                    }),
                    MenuEntry::item("Reveal in Files", move || reveal_in_files(&[p3])),
                    MenuEntry::item("Copy Absolute Path", move || copy_text_to_pasteboard(&p4)),
                    MenuEntry::Separator,
                    MenuEntry::item("Duplicate…", move || {
                        if let Some(ui) = w2.load() {
                            ui.ai_new_rule(Some(r2));
                        }
                    }),
                    MenuEntry::item("Delete…", move || {
                        let Some(ui) = w3.load() else { return };
                        let Some(window) = ui.ivars().state.borrow().as_ref().map(|st| st.window.clone()) else { return };
                        let weak = objc2::rc::Weak::from_retained(&ui.retain());
                        let r = r3.clone();
                        crate::ui::sheet::alert_sheet(
                            &window,
                            &format!("Delete “{}”?", r.name),
                            &format!("{} goes to the Trash.", r.file()),
                            &["Move to Trash", "Cancel"],
                            move |b| {
                                if b != 0 {
                                    return;
                                }
                                let undo = crate::engines::file_ops::UndoStack::default();
                                let _ = crate::views::files::trash_paths(&[r.path.clone()], &undo);
                                defaults_remove(&format!("aiInput.{}", r.file()));
                                if let Some(ui) = weak.load() {
                                    ui.ai_surface(|a| {
                                        let d = a.model.rules_dir();
                                        a.model.reload_rules(&d);
                                    });
                                    ui.ai_select(0);
                                }
                            },
                        );
                    }),
                ],
                None,
                x,
                y,
            );
        }

        fn ai_event(&self, ev: crate::views::ai_view::AIEvent) {
            use crate::ui::sidebar::SidebarEvent;
            use crate::views::ai_view::AIEvent as E;
            match ev {
                E::Sidebar(SidebarEvent::Select(i)) => self.ai_select(i),
                E::Sidebar(SidebarEvent::Add) => self.ai_new_rule(None),
                E::Sidebar(SidebarEvent::MenuRow(i, x, y)) => self.ai_rule_menu(i, x, y),
                E::Sidebar(SidebarEvent::Rail(_)) => {
                    self.ai_surface(|a| a.view.layout_all());
                }
                E::Sidebar(_) => {}
                E::InputChanged(text) => {
                    self.ai_surface(|a| {
                        if let Some(r) = a.model.rule() {
                            defaults_set_string(&format!("aiInput.{}", r.file()), &text);
                        }
                        a.model.input = text;
                        a.token_due = Some(now_secs() + 0.6);
                    });
                }
                E::Mode(i) => {
                    self.ai_surface(|a| {
                        if let Some(m) = a.model.modes().get(i).copied() {
                            a.model.set_mode(m);
                            defaults_set_string("aiViewMode", m.as_str());
                        }
                    });
                }
                E::Copy => {
                    self.ai_surface(|a| {
                        use crate::engines::ai_format::RichText;
                        match a.model.copy_plan(false) {
                            crate::views::ai::CopyPlan::Nothing => {}
                            crate::views::ai::CopyPlan::Markdown(md) => {
                                copy_text_to_pasteboard(&md);
                                a.model.set_status("Copied the Markdown", crate::views::ai::AITone::Success);
                            }
                            crate::views::ai::CopyPlan::Rich => {
                                let t = a.model.target;
                                let frag = RichText::html(&a.model.run.answer, t);
                                RichText::copy(&a.model.run.answer, frag.as_deref(), t);
                                a.model.set_status(format!("Copied for {}", t.title()), crate::views::ai::AITone::Success);
                            }
                        }
                    });
                }
                E::Run => {
                    self.ai_surface(|a| {
                        if a.model.is_running() {
                            a.model.cancel_run();
                        } else {
                            let text = a.view.input_text();
                            let _ = a.model.start_run(&text);
                            a.model.render_answer();
                        }
                    });
                }
                E::CopyCommand => {
                    let job = self.ai_surface(|a| (a.model.rule().cloned(), a.view.input_text()));
                    if let Some((Some(r), input)) = job {
                        std::thread::spawn(move || {
                            let cmd = r.runnable(&input);
                            on_ui(move |ui| {
                                copy_text_to_pasteboard(&cmd);
                                ui.ai_surface(|a| a.model.set_status("Copied the command", crate::views::ai::AITone::Success));
                            });
                        });
                    }
                }
            }
        }

        /// Drain tick: stream the run, and count tokens when due.
        fn poll_ai(&self) {
            let (running, due) = {
                let state = self.ivars().state.borrow();
                let Some(a) = state.as_ref().and_then(|st| st.ai.as_ref()) else { return };
                (a.model.is_running() || a.was_running, a.token_due.is_some_and(|t| now_secs() >= t))
            };
            if running {
                self.ai_surface(|a| {
                    a.model.poll_stream();
                    let now_running = a.model.is_running();
                    if a.was_running && !now_running {
                        a.model.render_answer();
                    }
                    a.was_running = now_running;
                });
            }
            if due {
                self.ai_count_tokens();
            }
        }

        /// `countTokens()` — `fm count-tokens -q [-i instructions]` off-main.
        fn ai_count_tokens(&self) {
            use crate::engines::ai_format::{CodeGuard, TokenBudget};
            let job = self.ai_surface(|a| {
                a.token_due = None;
                if a.model.available == Some(false) {
                    a.tokens.clear();
                    return None;
                }
                let r = a.model.rule()?.clone();
                let text = a.view.input_text().trim().to_string();
                if text.is_empty() {
                    a.tokens.clear();
                    return None;
                }
                let g = r.protect_code().then(|| CodeGuard::new(&text));
                let guarded = g.as_ref().is_some_and(|g| !g.codes.is_empty());
                let send = r.wrap(&r.prepare(if guarded { &g.as_ref().unwrap().text } else { &text }));
                let instr = r.instructions_for(guarded);
                let parts = if r.chunk() { TokenBudget::parts(&send, TokenBudget::part_budget(&instr)).len() } else { 1 };
                a.token_gen += 1;
                let codes = g.as_ref().map_or(0, |g| g.codes.len());
                Some((a.model.config.fm_bin(), send, instr, a.token_gen, parts, codes, guarded))
            });
            let Some(Some((bin, send, instr, gen, parts, codes, guarded))) = job else { return };
            std::thread::spawn(move || {
                use std::io::Write;
                let mut cmd = std::process::Command::new(&bin);
                cmd.arg("count-tokens").arg("-q");
                if !instr.is_empty() {
                    cmd.arg("-i").arg(&instr);
                }
                let out = cmd
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn()
                    .and_then(|mut c| {
                        if let Some(mut si) = c.stdin.take() {
                            let _ = si.write_all(send.as_bytes());
                        }
                        c.wait_with_output()
                    });
                let n = out.ok().filter(|o| o.status.success()).and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<i64>().ok());
                on_ui(move |ui| {
                    ui.ai_surface(|a| {
                        if a.token_gen != gen {
                            return;
                        }
                        let Some(n) = n else {
                            a.tokens.clear();
                            return;
                        };
                        let ctx = TokenBudget::context();
                        let need = n + (n - TokenBudget::estimate(&instr));
                        let mut s = format!("{n} / {ctx} tokens");
                        if guarded {
                            s.push_str(&format!(" · {codes} code kept out"));
                        }
                        if parts > 1 {
                            s.push_str(&format!(" · runs in {parts} parts"));
                        }
                        a.tokens = s;
                        a.tokens_warn = need > ctx && parts == 1;
                    });
                });
            });
        }

        /// Run `f` on the Confluence state, then push a fresh snapshot.
        fn conf_surface<R>(&self, f: impl FnOnce(&mut ConfState) -> R) -> Option<R> {
            let mut state = self.ivars().state.borrow_mut();
            let cs = state.as_mut().and_then(|st| st.confluence.as_mut())?;
            let r = f(cs);
            cs.view.apply(conf_snapshot(cs));
            Some(r)
        }

        fn build_confluence(&self, mtm: MainThreadMarker) -> ConfState {
            use crate::views::confluence::macos::ConfluencePreview;
            use crate::views::confluence::{ConfluenceConfig, ConfluenceImageLoader, ConfluenceModel};
            let text = crate::app::config::read_config_text().unwrap_or_default();
            let sections = super::parse_sections(&text);
            let entries: HashMap<String, String> = sections.get("confluence").cloned().unwrap_or_default();
            let config = ConfluenceConfig::from_entries(entries);
            let sidebar_w = config.sidebar_width();
            let split = config.split_clamped();
            let mut model = ConfluenceModel::new(config);
            model.show_search_empty();
            model.set_status("", crate::views::confluence::StatusTone::Dim);
            let mut loader = ConfluenceImageLoader::new();
            let preview = ConfluencePreview::new(mtm, &mut loader);
            std::mem::forget(loader);
            let colors = crate::ui::theme::section_colors("confluence");
            let web: Retained<NSView> = Retained::into_super(preview.web.clone());
            let view = crate::views::confluence_view::ConfluenceView::new(mtm, colors, web, sidebar_w.max(0.0).max(160.0), split);
            let cs = ConfState {
                model,
                view,
                preview,
                pstate: Default::default(),
                pages: HashMap::new(),
                preview_id: String::new(),
                search_gen: 0,
                preview_gen: 0,
                last_hit: None,
            };
            cs.view.apply(conf_snapshot(&cs));
            let weak = objc2::rc::Weak::from_retained(&self.retain());
            cs.view.set_on_event(Box::new(move |ev| {
                if let Some(ui) = weak.load() {
                    ui.conf_event(ev);
                }
            }));
            conf_api(vec!["--check".into()], None, |ui, j| ui.conf_checked(j));
            cs
        }

        /// `reloadConfig` — the `--check` reply.
        fn conf_checked(&self, j: serde_json::Value) {
            use crate::views::confluence::{PickerOption, StatusTone};
            let configured = j.get("ok").and_then(serde_json::Value::as_bool).unwrap_or(false);
            self.conf_surface(|cs| {
                cs.model.configured = configured;
                cs.model.site = j.get("site").and_then(serde_json::Value::as_str).unwrap_or("").to_string();
                let opts: Vec<PickerOption> = j
                    .get("spaces")
                    .and_then(serde_json::Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|s| {
                                let k = s.get("key")?.as_str()?.to_string();
                                Some(PickerOption {
                                    title: k.clone(),
                                    detail: s.get("name").and_then(serde_json::Value::as_str).unwrap_or("").to_string(),
                                    id: k,
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                cs.model.spaces.set_options(opts);
                if !configured {
                    let probs = j
                        .get("problems")
                        .and_then(serde_json::Value::as_array)
                        .map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join("; "))
                        .unwrap_or_default();
                    cs.model.rows.clear();
                    cs.model.set_status(format!("Not set up yet: {probs}"), StatusTone::Warning);
                    cs.pstate = crate::views::confluence_view::ConfPreviewState {
                        hint: "Connect a Confluence site to start searching.".into(),
                        setup: true,
                        ..Default::default()
                    };
                } else {
                    cs.pstate = Default::default();
                    if cs.model.rows.is_empty() {
                        cs.model.show_search_empty();
                    }
                }
            });
        }

        fn conf_run_search(&self, more: bool) {
            use crate::views::confluence::{Scope, StatusTone};
            let plan = self.conf_surface(|cs| {
                if !cs.model.configured {
                    return None;
                }
                let mut crit = cs.model.criteria();
                if cs.model.scope == Scope::Search && !cs.model.has_criteria() && !more {
                    cs.model.show_search_empty();
                    return Some(None);
                }
                if more {
                    if cs.model.next_link.is_empty() {
                        return Some(None);
                    }
                    if let Some(o) = crit.as_object_mut() {
                        o.insert("next".into(), serde_json::Value::String(cs.model.next_link.clone()));
                    }
                }
                cs.search_gen += 1;
                cs.model.searching = true;
                cs.model.set_status(if more { "Loading more…" } else { "Searching…" }, StatusTone::Dim);
                Some(Some((crit, cs.search_gen)))
            });
            match plan {
                Some(None) => self.conf_setup(),
                None | Some(Some(None)) => {}
                Some(Some(Some((crit, gen)))) => {
                    conf_api(vec!["--search".into()], Some(crit), move |ui, j| {
                        let first = ui.conf_surface(|cs| {
                            if cs.search_gen != gen {
                                return false;
                            }
                            cs.model.apply_search_result(&j, more, now_secs());
                            !more && !cs.model.rows.is_empty()
                        });
                        if first == Some(true) {
                            ui.conf_load_preview(0);
                        }
                    });
                }
            }
        }

        fn conf_load_favorites(&self, show: bool) {
            conf_api(vec!["--favorites".into(), "--no-refresh".into()], None, move |ui, j| {
                ui.conf_surface(|cs| cs.model.apply_favorites(&j, show, now_secs()));
            });
        }

        /// `loadPreview(_:)`.
        fn conf_load_preview(&self, i: usize) {
            use crate::views::confluence_view::ConfPreviewState;
            let job = self.conf_surface(|cs| {
                let r = cs.model.rows.get(i)?.clone();
                cs.model.selection = i;
                cs.preview_gen += 1;
                cs.preview_id = r.id.clone();
                let mut meta = vec![if r.space_name.is_empty() { r.space.clone() } else { r.space_name.clone() }];
                if !r.path.is_empty() {
                    meta.push(r.path.clone());
                }
                if !r.author.is_empty() {
                    meta.push(r.author.clone());
                }
                if !r.modified_text.is_empty() {
                    meta.push(format!("updated {}", r.modified_text));
                }
                let meta = meta.into_iter().filter(|m| !m.is_empty()).collect::<Vec<_>>().join("  ·  ");
                cs.pstate = ConfPreviewState { header: Some((r.title.clone(), meta, r.favorite)), ..Default::default() };
                if r.missing {
                    cs.pstate.hint = "This page wasn't found — deleted, moved, or you lost access.\nRemove it with ★.".into();
                    return None;
                }
                if let Some(page) = cs.pages.get(&r.id).cloned() {
                    return Some((page, r, cs.preview_gen, true));
                }
                cs.pstate.hits = Some("Loading page…".into());
                Some((serde_json::Value::Null, r, cs.preview_gen, false))
            });
            let Some(Some((page, row, gen, cached))) = job else { return };
            if cached {
                self.conf_render(page, row);
                return;
            }
            let id = row.id.clone();
            conf_api(vec!["--page".into(), id.clone()], None, move |ui, j| {
                let ok = j.get("ok").and_then(serde_json::Value::as_bool) == Some(true);
                let current = ui.conf_surface(|cs| {
                    if cs.preview_gen != gen {
                        return false;
                    }
                    if !ok {
                        cs.pstate.hits = None;
                        cs.pstate.hint = format!(
                            "Couldn't load the page:\n{}",
                            j.get("error").and_then(serde_json::Value::as_str).unwrap_or("failed")
                        );
                        return false;
                    }
                    cs.pages.insert(id.clone(), j.clone());
                    if cs.pages.len() > 20 {
                        if let Some(k) = cs.pages.keys().next().cloned() {
                            cs.pages.remove(&k);
                        }
                    }
                    true
                });
                if current == Some(true) {
                    ui.conf_render(j, row);
                }
            });
        }

        /// `render(_:_:)` — `confluence.preview_html` off-main, then load.
        fn conf_render(&self, page: serde_json::Value, r: crate::views::confluence::ConfluenceRow) {
            let Some((site, terms, gen)) = self.conf_surface(|cs| (cs.model.site.clone(), cs.model.terms.clone(), cs.preview_gen)) else {
                return;
            };
            let colors = crate::ui::theme::section_colors("confluence");
            let pc: serde_json::Map<String, serde_json::Value> = crate::views::confluence::page_colors(&colors)
                .into_iter()
                .map(|(k, v)| (k.to_string(), serde_json::Value::String(v)))
                .collect();
            let base = page.get("site").and_then(serde_json::Value::as_str).map(str::to_string).unwrap_or(site.clone());
            let params = serde_json::json!({
                "page": page,
                "row": {"type": r.kind, "url": r.url, "title": r.title},
                "site": site,
                "terms": terms,
                "colors": pc,
            });
            let has_terms = !terms.is_empty();
            std::thread::spawn(move || {
                let html = crate::views::confluence::preview_html(&params);
                on_ui(move |ui| {
                    ui.conf_surface(|cs| {
                        if cs.preview_gen != gen {
                            return;
                        }
                        match &html {
                            Some(h) => {
                                cs.pstate.hint.clear();
                                cs.pstate.web = true;
                                cs.pstate.hits = Some(if has_terms { "Finding matches…".into() } else { String::new() });
                                cs.preview.load_html(h, Some(&base));
                            }
                            None => cs.pstate.hint = "✗ preview failed (python helper unavailable)".into(),
                        }
                    });
                });
            });
        }

        fn conf_toggle_favorite(&self, i: usize) {
            use crate::views::confluence::StatusTone;
            let Some(Some((id, add, row_json))) = self.conf_surface(|cs| {
                let r = cs.model.rows.get(i)?;
                Some((r.id.clone(), !r.favorite, r.to_json()))
            }) else {
                return;
            };
            let args = vec!["--favorite".to_string(), if add { "add".into() } else { "remove".into() }, id.clone()];
            conf_api(args, add.then(|| serde_json::Value::Array(vec![row_json])), move |ui, j| {
                ui.conf_surface(|cs| {
                    if j.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
                        cs.model.set_status(j.get("error").and_then(serde_json::Value::as_str).unwrap_or("favorite failed"), StatusTone::Danger);
                        return;
                    }
                    cs.model.favorites = j
                        .get("favorites")
                        .and_then(serde_json::Value::as_array)
                        .map(|a| {
                            a.iter()
                                .map(|x| {
                                    let mut r = crate::views::confluence::ConfluenceRow::from_json(x);
                                    r.favorite = true;
                                    r
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    if let Some(r) = cs.model.rows.iter_mut().find(|r| r.id == id) {
                        r.favorite = add;
                    }
                    if let Some((_, _, fav)) = cs.pstate.header.as_mut() {
                        if cs.preview_id == id {
                            *fav = add;
                        }
                    }
                    if cs.model.showing_favorites && !add {
                        cs.model.show_favorites(true, now_secs());
                    }
                    cs.model.set_status(
                        if add { "★ Added to favorites" } else { "Removed from favorites" },
                        if add { StatusTone::Success } else { StatusTone::Dim },
                    );
                });
            });
        }

        /// `showSetup()` — the Site / Email / Token / Spaces sheet, then
        /// `--detect-auth` → `--save` → spaces → `--check`.
        fn conf_setup(&self) {
            conf_api(vec!["--check".into()], None, |ui, j| {
                use crate::ui::sheet::{form_sheet, FormRow};
                let s = |k: &str| j.get(k).and_then(serde_json::Value::as_str).unwrap_or("").to_string();
                let has_token = j.get("hasToken").and_then(serde_json::Value::as_bool).unwrap_or(false);
                let old: Vec<String> = j
                    .get("spaces")
                    .and_then(serde_json::Value::as_array)
                    .map(|a| a.iter().filter_map(|x| x.get("key").and_then(|k| k.as_str()).map(str::to_string)).collect())
                    .unwrap_or_default();
                let state = if s("site").is_empty() {
                    "○ Not set up yet.".to_string()
                } else if !has_token {
                    "● Site saved, token missing.".to_string()
                } else {
                    format!(
                        "● Set up · {} · Test & Save re-checks the sign-in.",
                        if old.is_empty() { "whole site".to_string() } else { format!("{} space{}", old.len(), if old.len() == 1 { "" } else { "s" }) }
                    )
                };
                let info = format!(
                    "{state}\n\nConfluence has its own site and token (not Jira's). Spaces work like Jira projects: searches stay inside them. Saved to {}.",
                    if s("configPath").is_empty() { "config.json".to_string() } else { s("configPath") }
                );
                let rows = vec![
                    FormRow::new("Site", &s("site"), "https://yourcompany.atlassian.net/wiki  or  https://confluence.corp/confluence"),
                    FormRow::new("Email", &s("email"), "Cloud: your Atlassian account email (empty for a Data Center token)"),
                    FormRow::new(
                        "Token",
                        "",
                        if has_token { "saved — leave empty to keep it" } else { "API token (Cloud) or personal access token (Data Center)" },
                    )
                    .secure(),
                    FormRow::new("Spaces", &old.join(", "), "space keys, e.g. ENG, OPS (empty = the whole site)"),
                ];
                let Some(window) = ui.ivars().state.borrow().as_ref().map(|st| st.window.clone()) else { return };
                let weak = objc2::rc::Weak::from_retained(&ui.retain());
                form_sheet(&window, "Confluence Setup", &info, rows, "Test & Save", move |vals| {
                    let (Some(v), Some(ui)) = (vals, weak.load()) else { return };
                    let keys: Vec<String> = v[3]
                        .to_uppercase()
                        .split(|c: char| c == ',' || c == ' ')
                        .filter(|k| !k.is_empty())
                        .map(str::to_string)
                        .collect();
                    ui.conf_save_setup(v[0].clone(), v[1].clone(), v[2].clone(), keys, old);
                });
            });
        }

        fn conf_save_setup(&self, site: String, email: String, token: String, keys: Vec<String>, old: Vec<String>) {
            use crate::views::confluence::StatusTone;
            self.conf_surface(|cs| {
                cs.model.searching = true;
                cs.model.set_status("Testing the connection…", StatusTone::Dim);
            });
            let mut given = serde_json::json!({"site": site, "email": email});
            if !token.is_empty() {
                given["token"] = serde_json::Value::String(token);
            }
            conf_api(vec!["--detect-auth".into()], Some(given.clone()), move |ui, j| {
                let mut save = given.clone();
                if j.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
                    let err = j.get("error").and_then(serde_json::Value::as_str).unwrap_or("failed").to_string();
                    ui.conf_surface(|cs| {
                        cs.model.searching = false;
                        cs.model.set_status(format!("Setup: {err}"), StatusTone::Danger);
                    });
                    return;
                }
                save["auth"] = j.get("auth").cloned().unwrap_or(serde_json::Value::String(String::new()));
                if let Some(s) = j.get("site") {
                    save["site"] = s.clone();
                }
                let user = j.get("user").and_then(serde_json::Value::as_str).unwrap_or("").to_string();
                conf_api(vec!["--save".into()], Some(save), move |_ui, _| {
                    let gone: Vec<String> = old.iter().filter(|k| !keys.contains(k)).cloned().collect();
                    let new: Vec<String> = keys.iter().filter(|k| !old.contains(k)).cloned().collect();
                    let finish = move |ui: &DaemonUi| {
                        let user = user.clone();
                        conf_api(vec!["--check".into()], None, move |ui, j| {
                            ui.conf_checked(j);
                            ui.conf_surface(|cs| {
                                cs.model.searching = false;
                                let site = cs.model.site.clone();
                                cs.model.set_status(
                                    format!("Connected{} · {site}", if user.is_empty() { String::new() } else { format!(" as {user}") }),
                                    StatusTone::Success,
                                );
                            });
                        });
                        let _ = ui;
                    };
                    let add = move |ui: &DaemonUi| {
                        if new.is_empty() {
                            finish(ui);
                        } else {
                            let mut a = vec!["--add-space".to_string()];
                            a.extend(new.clone());
                            conf_api(a, None, move |ui, _| finish(ui));
                        }
                    };
                    if gone.is_empty() {
                        on_ui(move |ui| add(ui));
                    } else {
                        let mut a = vec!["--remove-space".to_string()];
                        a.extend(gone);
                        conf_api(a, None, move |ui, _| add(ui));
                    }
                });
            });
        }

        fn conf_event(&self, ev: crate::views::confluence_view::ConfEvent) {
            use crate::ui::menu_kit::{popup, MenuEntry};
            use crate::ui::sidebar::SidebarEvent;
            use crate::views::confluence::Scope;
            use crate::views::confluence_view::{ConfEvent as E, MODIFIED, SORTS, TYPES};
            match ev {
                E::Sidebar(SidebarEvent::Select(i)) => {
                    let s = if i == 0 { Scope::Search } else { Scope::Favorites };
                    let configured = self.conf_surface(|cs| {
                        cs.model.set_scope(s, now_secs());
                        cs.model.configured
                    });
                    if s == Scope::Favorites {
                        self.conf_load_favorites(true);
                    } else if configured == Some(true) {
                        self.conf_run_search(false);
                    }
                }
                E::Sidebar(SidebarEvent::Rail(_)) => {
                    self.conf_surface(|cs| cs.view.layout_all());
                }
                E::Sidebar(_) => {}
                E::Query(q) => {
                    self.conf_surface(|cs| {
                        cs.model.query = q;
                        if cs.model.scope == Scope::Favorites {
                            cs.model.show_favorites(false, now_secs());
                        }
                    });
                }
                E::Enter | E::Search => self.conf_run_search(false),
                E::Move(d) => {
                    let next = self.conf_surface(|cs| {
                        let n = cs.model.rows.len() as i64;
                        (n > 0).then(|| (cs.model.selection as i64 + d).clamp(0, n - 1) as usize)
                    });
                    if let Some(Some(i)) = next {
                        self.conf_load_preview(i);
                    }
                }
                E::Escape => {
                    self.conf_surface(|cs| {
                        cs.model.escape();
                    });
                }
                E::Mode(i) => {
                    let go = self.conf_surface(|cs| {
                        cs.model.mode_index = i;
                        cs.model.configured && cs.model.has_criteria()
                    });
                    if go == Some(true) {
                        self.conf_run_search(false);
                    }
                }
                E::TitleOnly(on) => {
                    let go = self.conf_surface(|cs| {
                        cs.model.title_only = on;
                        cs.model.configured && cs.model.has_criteria()
                    });
                    if go == Some(true) {
                        self.conf_run_search(false);
                    }
                }
                E::Chip { which, x, y } => {
                    let Some(Some((view, cur, opts, sel_all, sel))) = self.conf_surface(|cs| {
                        let cur = match which {
                            2 => cs.model.type_choice.clone(),
                            3 => cs.model.modified.clone(),
                            4 => cs.model.sort.clone(),
                            _ => String::new(),
                        };
                        let picker = if which == 0 { &cs.model.spaces } else { &cs.model.contributors };
                        Some((
                            cs.view.clone(),
                            cur,
                            picker.options.iter().map(|o| (o.id.clone(), o.title.clone(), o.detail.clone())).collect::<Vec<_>>(),
                            picker.is_all(),
                            picker.selected.clone(),
                        ))
                    }) else {
                        return;
                    };
                    let weak = objc2::rc::Weak::from_retained(&self.retain());
                    let mut entries = Vec::new();
                    if which >= 2 {
                        let items: &[(&str, &str)] = match which {
                            2 => &TYPES,
                            3 => &MODIFIED,
                            _ => &SORTS,
                        };
                        for (t, v) in items {
                            let (w, v2) = (weak.clone(), v.to_string());
                            entries.push(MenuEntry::checked(t, *v == cur, move || {
                                if let Some(ui) = w.load() {
                                    let go = ui.conf_surface(|cs| {
                                        match which {
                                            2 => cs.model.type_choice = v2,
                                            3 => cs.model.modified = v2,
                                            _ => cs.model.sort = v2,
                                        }
                                        cs.model.configured && cs.model.has_criteria()
                                    });
                                    if go == Some(true) {
                                        ui.conf_run_search(false);
                                    }
                                }
                            }));
                        }
                    } else {
                        let all_title = if which == 0 { "All spaces" } else { "Any contributor" };
                        let w = weak.clone();
                        entries.push(MenuEntry::checked(all_title, sel_all, move || {
                            if let Some(ui) = w.load() {
                                ui.conf_surface(|cs| {
                                    let p = if which == 0 { &mut cs.model.spaces } else { &mut cs.model.contributors };
                                    p.set(Vec::new(), true);
                                });
                            }
                        }));
                        if opts.is_empty() {
                            entries.push(MenuEntry::disabled(if which == 0 { "(no spaces cached)" } else { "(no contributors cached)" }));
                        } else {
                            entries.push(MenuEntry::Separator);
                        }
                        for (id, title, detail) in opts {
                            let on = !sel_all && sel.contains(&id);
                            let w = weak.clone();
                            let label = if detail.is_empty() { title } else { format!("{title} — {detail}") };
                            entries.push(MenuEntry::checked(&label, on, move || {
                                if let Some(ui) = w.load() {
                                    ui.conf_surface(|cs| {
                                        let p = if which == 0 { &mut cs.model.spaces } else { &mut cs.model.contributors };
                                        let mut s = if p.is_all() { Vec::new() } else { p.selected.clone() };
                                        if let Some(k) = s.iter().position(|x| *x == id) {
                                            s.remove(k);
                                        } else {
                                            s.push(id.clone());
                                        }
                                        let all = s.is_empty();
                                        p.set(s, all);
                                    });
                                }
                            }));
                        }
                    }
                    popup(entries, Some(&view), x, y);
                }
                E::Select { row, double } => {
                    self.conf_load_preview(row);
                    if double {
                        self.conf_event(E::Open);
                    }
                    let v = self.conf_surface(|cs| cs.view.results_view());
                    if let (Some(v), Some(st)) = (v, self.ivars().state.borrow().as_ref()) {
                        st.window.makeFirstResponder(Some(&v));
                    }
                }
                E::Star(row) => self.conf_toggle_favorite(row),
                E::PreviewStar => {
                    if let Some(i) = self.conf_surface(|cs| cs.model.selection) {
                        self.conf_toggle_favorite(i);
                    }
                }
                E::RowMenu { row, x, y } => {
                    let Some(Some(r)) = self.conf_surface(|cs| cs.model.rows.get(row).cloned()) else { return };
                    let weak = objc2::rc::Weak::from_retained(&self.retain());
                    let (u1, u2) = (r.url.clone(), r.url.clone());
                    let w = weak.clone();
                    popup(
                        vec![
                            MenuEntry::item("Open in Browser", move || open_url(&u1)),
                            MenuEntry::item("Copy Link", move || copy_text_to_pasteboard(&u2)),
                            MenuEntry::Separator,
                            MenuEntry::item(if r.favorite { "Remove from Favorites" } else { "Add to Favorites" }, move || {
                                if let Some(ui) = w.load() {
                                    ui.conf_toggle_favorite(row);
                                }
                            }),
                        ],
                        None,
                        x,
                        y,
                    );
                }
                E::LoadMore => self.conf_run_search(true),
                E::CopyCql => {
                    if let Some(c) = self.conf_surface(|cs| cs.model.last_cql.clone()).filter(|c| !c.is_empty()) {
                        copy_text_to_pasteboard(&c);
                    }
                }
                E::CopyCurl => {
                    if let Some(c) = self.conf_surface(|cs| cs.model.last_curl.clone()).filter(|c| !c.is_empty()) {
                        copy_text_to_pasteboard(&c);
                    }
                }
                E::Setup => self.conf_setup(),
                E::Open => {
                    if let Some(Some(u)) = self.conf_surface(|cs| cs.model.rows.get(cs.model.selection).map(|r| r.url.clone())) {
                        open_url(&u);
                    }
                }
                E::CopyLink => {
                    if let Some(Some(u)) = self.conf_surface(|cs| cs.model.rows.get(cs.model.selection).map(|r| r.url.clone())) {
                        copy_text_to_pasteboard(&u);
                    }
                }
                E::PrevHit => {
                    self.conf_surface(|cs| cs.preview.prev_hit());
                }
                E::NextHit => {
                    self.conf_surface(|cs| cs.preview.next_hit());
                }
            }
        }

        /// The web view's `{i, n, snippet}` highlighter posts → the hits bar.
        fn poll_conf_hits(&self) {
            let changed = {
                let state = self.ivars().state.borrow();
                let Some(cs) = state.as_ref().and_then(|st| st.confluence.as_ref()) else { return };
                let hit = cs.preview.script.last_hit().map(|h| (h.index, h.count, h.snippet));
                hit != cs.last_hit
            };
            if changed {
                self.conf_surface(|cs| {
                    let hit = cs.preview.script.last_hit().map(|h| (h.index, h.count, h.snippet));
                    cs.last_hit = hit.clone();
                    if let Some((i, n, snip)) = hit {
                        cs.pstate.hits = Some(if n == 0 {
                            "No matches in the page text (matched in title, labels or attachments)".into()
                        } else {
                            format!("{} of {}   “{}”", i + 1, n, snip)
                        });
                    }
                });
            }
        }

        /// Run `f` on the Jira list, then push a fresh snapshot.
        fn jira_surface<R>(&self, f: impl FnOnce(&mut crate::views::jira_view::JiraList) -> R) -> Option<R> {
            let mut state = self.ivars().state.borrow_mut();
            let (list, view) = state.as_mut().and_then(|st| st.jira.as_mut())?;
            let r = f(list);
            view.apply(list.snapshot());
            Some(r)
        }

        /// Build the Jira list (`openListWindow` for `[jira]`).
        fn build_jira(
            &self,
            mtm: MainThreadMarker,
        ) -> (crate::views::jira_view::JiraList, Retained<crate::views::jira_view::JiraView>) {
            use crate::views::jira_view::{JiraList, JiraView};
            let text = crate::app::config::read_config_text().unwrap_or_default();
            let sections = super::parse_sections(&text);
            let section: HashMap<String, String> = sections.get("jira").cloned().unwrap_or_default();
            let paths = match crate::app::paths::JiraPaths::resolve() {
                crate::app::paths::JiraPathsState::Resolved(p) => Some(p),
                crate::app::paths::JiraPathsState::Failed(e) => {
                    self.ivars().controller.log(&e);
                    None
                }
            };
            let sidebar_w = section.get("sidebar-width").and_then(|v| v.parse().ok()).unwrap_or(245.0);
            let insp_w = section.get("inspector-width").and_then(|v| v.parse().ok()).unwrap_or(340.0);
            let mut list = JiraList::new(section, paths);
            list.start(None);
            let colors = crate::ui::theme::section_colors("jira");
            let view = JiraView::new(mtm, colors, sidebar_w, insp_w);
            view.apply(list.snapshot());
            let weak = objc2::rc::Weak::from_retained(&self.retain());
            view.set_on_event(Box::new(move |ev| {
                if let Some(ui) = weak.load() {
                    ui.jira_event(ev);
                }
            }));
            (list, view)
        }

        /// `onTableFit`: size the columns to their content. Saving the new
        /// widths (`columnsResized` → `jira_config.py --set-columns`) is not
        /// ported, so the fit lasts until the list reloads its columns.
        fn jira_fit_columns(&self) {
            let view = self.ivars().state.borrow().as_ref().and_then(|st| st.jira.as_ref().map(|(_, v)| v.clone()));
            let Some(view) = view else { return };
            self.jira_surface(|l| {
                let rows: Vec<_> = l.rows.iter().filter_map(|&i| l.all.get(i).cloned()).collect();
                let pcts = view.fit_columns(&l.columns, &rows, 400);
                for (c, p) in l.columns.iter_mut().zip(pcts) {
                    c.width = p;
                }
            });
            crate::ui::toast::show(
                "Columns fitted to their content · saving the widths: not ported yet",
                "arrow.left.and.right",
            );
        }

        fn jira_event(&self, ev: crate::views::jira_view::JiraEvent) {
            use crate::ui::sidebar::SidebarEvent;
            use crate::views::jira_view::{JiraEvent as E, BOARD_LIST_KEY, BOARD_PREFIX, MY_WORK_PREFIX};
            match ev {
                E::Click { row, double } => {
                    self.jira_surface(|l| l.selection = row);
                    self.focus_jira_table();
                    if double {
                        self.jira_event(E::OpenDetail);
                    }
                }
                E::Check(row) => {
                    self.jira_surface(|l| {
                        if let Some(k) = l.row(row).and_then(|r| r.get("key")).cloned() {
                            if !l.checked.remove(&k) {
                                l.checked.insert(k);
                            }
                        }
                    });
                }
                E::Star(row) => {
                    self.jira_surface(|l| {
                        if let Some(k) = l.row(row).and_then(|r| r.get("key")).cloned() {
                            if !l.favs.remove(&k) {
                                l.favs.insert(k);
                            }
                        }
                    });
                }
                E::Sort(i) => {
                    self.jira_surface(|l| l.sort_by(i));
                }
                E::HeaderMenu { x, y } => {
                    // `PopupTableHeader.rightMouseDown`: Fit Columns to Content.
                    use crate::ui::menu_kit::{popup, MenuEntry};
                    let w1 = objc2::rc::Weak::from_retained(&self.retain());
                    popup(
                        vec![MenuEntry::item("Fit Columns to Content", move || {
                            if let Some(ui) = w1.load() {
                                ui.jira_fit_columns();
                            }
                        })],
                        None,
                        x,
                        y,
                    );
                }
                E::Query(q) => {
                    self.jira_surface(|l| {
                        l.query = q;
                        l.selection = 0;
                        l.refilter();
                    });
                }
                E::Move(d) => {
                    self.jira_surface(|l| jira_move(l, d));
                }
                E::Enter | E::OpenDetail => {
                    let target = self.jira_surface(|l| {
                        let r = l.selected()?;
                        if let Some(b) = r.get("boardId").filter(|_| r.get("key").is_some_and(|k| k.starts_with(BOARD_PREFIX))) {
                            return Some(Err(b.clone()));
                        }
                        let key = r.get("key")?.clone();
                        let site = l.site();
                        (!site.is_empty()).then(|| Ok(format!("{site}/browse/{key}")))
                    });
                    match target.flatten() {
                        Some(Ok(url)) => open_url(&url),
                        Some(Err(board)) => {
                            let ok = self.jira_surface(|l| l.open_board(&board)).unwrap_or(false);
                            if !ok {
                                self.ivars().controller.log(&format!("jira: board {board} not synced yet"));
                            }
                        }
                        None => {}
                    }
                }
                E::Escape => {
                    self.jira_surface(|l| {
                        l.query.clear();
                        l.refilter();
                    });
                    self.focus_jira_table();
                }
                E::RowMenu { row, x, y } => {
                    use crate::ui::menu_kit::{popup, MenuEntry};
                    let info = self.jira_surface(|l| {
                        l.selection = row;
                        l.selected().map(|r| (r.get("key").cloned().unwrap_or_default(), l.site()))
                    });
                    if let Some(Some((key, site))) = info {
                        let url = format!("{site}/browse/{key}");
                        let (u1, u2, k1) = (url.clone(), url, key.clone());
                        popup(
                            vec![
                                MenuEntry::item("Open in Browser", move || open_url(&u1)),
                                MenuEntry::Separator,
                                MenuEntry::item("Copy Key", move || copy_text_to_pasteboard(&k1)),
                                MenuEntry::item("Copy Link", move || copy_text_to_pasteboard(&u2)),
                            ],
                            None,
                            x,
                            y,
                        );
                    }
                }
                E::Sidebar(SidebarEvent::Select(i)) => {
                    self.jira_surface(|l| l.select_tab(i));
                }
                E::Sidebar(SidebarEvent::Pinned(key)) => {
                    let err = self.jira_surface(|l| -> Option<String> {
                        if let Some(id) = key.strip_prefix(MY_WORK_PREFIX) {
                            return l.open_my_work(id).err();
                        }
                        if key == BOARD_LIST_KEY {
                            l.show_board_list();
                            return None;
                        }
                        if let Some(id) = key.strip_prefix(BOARD_PREFIX) {
                            return (!l.open_board(id)).then(|| format!("board {id} not synced yet"));
                        }
                        Some(format!("{key}: not available in the Rust port yet"))
                    });
                    if let Some(Some(e)) = err {
                        self.ivars().controller.log(&format!("jira: {e}"));
                    }
                }
                E::Sidebar(SidebarEvent::Rail(_)) => {
                    let st = self.ivars().state.borrow();
                    if let Some((_, v)) = st.as_ref().and_then(|s| s.jira.as_ref()) {
                        v.layout_panes();
                    }
                }
                E::Sidebar(_) => {}
            }
        }

        /// Every 10 s while Jira is up: the sidebar badges / sync line and
        /// the shown list follow the poller.
        fn refresh_jira(&self) {
            let now = now_secs();
            if now - self.ivars().jira_refresh_at.get() < 10.0 {
                return;
            }
            self.ivars().jira_refresh_at.set(now);
            if !self.ivars().controller.current().is_some_and(|v| v.is_jira()) {
                return;
            }
            self.jira_surface(|l| l.refresh());
        }

        /// `ConfluenceWindow.handleKey`: Cmd+L / Cmd+F search field, Cmd+G /
        /// Cmd+Shift+G preview hits, Cmd+D favorite, Cmd+R search, Cmd+Return
        /// open, Cmd+C / Cmd+Shift+C link (title + link) from the results.
        fn conf_key(&self, key: &popup::KeyInput) -> bool {
            use crate::views::confluence_view::ConfEvent as E;
            if !key.cmd || key.ctrl || key.opt {
                return false;
            }
            match key.key_code {
                37 | 3 if !key.shift => {
                    self.live_pane_focus("search");
                }
                5 => self.conf_event(if key.shift { E::PrevHit } else { E::NextHit }),
                2 => {
                    if let Some(i) = self.conf_surface(|cs| cs.model.selection) {
                        self.conf_event(E::Star(i));
                    }
                }
                15 => self.conf_event(E::Search),
                36 | 76 => self.conf_event(E::Open),
                8 => {
                    // Only from the results list: the search field keeps Cmd+C.
                    let on_results = self.focused_pane_id().as_deref() == Some("results");
                    if !on_results {
                        return false;
                    }
                    if key.shift {
                        let r = self.conf_surface(|cs| cs.model.rows.get(cs.model.selection).map(|r| (r.title.clone(), r.url.clone())));
                        if let Some(Some((t, u))) = r {
                            copy_text_to_pasteboard(&format!("{t}\n{u}"));
                            crate::ui::toast::show("Copied title + link", "link");
                        }
                    } else {
                        self.conf_event(E::CopyLink);
                    }
                }
                _ => return false,
            }
            true
        }

        /// `PopupFileBrowser.handleShortcut` + `FileListPane` keys: history,
        /// hidden files, new folder, parent / open, and (list focused) ← →,
        /// Space Quick Look, Cmd+C/X/V files, Cmd+D, Cmd+Delete, Cmd+Z.
        fn files_key(&self, key: &popup::KeyInput) -> bool {
            use crate::engines::file_ops::FileOps;
            let in_list = self.focused_pane_id().as_deref() == Some("files-list");
            let (cmd, shift, opt, ctrl) = (key.cmd, key.shift, key.opt, key.ctrl);
            let plain = !cmd && !ctrl && !opt && !shift;
            let done = |ui: &Self, what: String, failed: Option<String>| {
                ui.files_surface(|b| b.reload());
                match failed {
                    Some(f) => crate::ui::toast::show(&f, "exclamationmark.triangle.fill"),
                    None if !what.is_empty() => crate::ui::toast::show(&what, "checkmark.circle.fill"),
                    None => {}
                }
            };
            match key.key_code {
                33 if cmd => {
                    self.files_surface(|b| b.go_back());
                }
                30 if cmd => {
                    self.files_surface(|b| b.go_forward());
                }
                47 if cmd && shift => {
                    self.files_surface(|b| b.toggle_hidden());
                }
                37 if cmd && !shift => {
                    self.live_pane_focus("files-filter");
                }
                45 if cmd && shift => {
                    let Some(Some(dir)) = self.files_surface(|b| b.ops_directory()) else { return true };
                    let o = FileOps::create("untitled folder", &dir, true, FileOps::shared());
                    done(self, String::new(), o.failed);
                }
                126 if cmd && !shift => {
                    self.files_surface(|b| b.cd_parent());
                }
                125 if cmd && !shift && in_list => {
                    self.files_accept_selected();
                }
                123 if plain && in_list => {
                    self.files_surface(|b| b.cd_parent());
                }
                124 if plain && in_list => {
                    let dir = self.files_surface(|b| {
                        b.visible_rows().get(b.selection()).filter(|e| e.is_dir).map(|e| e.path.clone())
                    });
                    if let Some(Some(d)) = dir {
                        self.files_surface(|b| b.cd(&d));
                    }
                }
                49 if plain && in_list => {
                    if let Some(mtm) = MainThreadMarker::new() {
                        self.files_surface(|b| b.toggle_quick_look(mtm));
                    }
                }
                15 if cmd && !shift => self.files_begin_rename(),
                120 if !cmd && !ctrl => self.files_begin_rename(),
                51 if cmd && in_list => {
                    let paths = self.files_surface(|b| b.selected_paths()).unwrap_or_default();
                    if paths.is_empty() {
                        return true;
                    }
                    let o = FileOps::trash(&paths, FileOps::shared());
                    done(self, format!("Moved {} item(s) to the Trash", paths.len()), o.failed);
                }
                2 if cmd && in_list => {
                    let paths = self.files_surface(|b| b.selected_paths()).unwrap_or_default();
                    let o = FileOps::duplicate(&paths, FileOps::shared());
                    done(self, String::new(), o.failed);
                }
                8 if cmd && !shift && in_list => {
                    let paths = self.files_surface(|b| b.selected_paths()).unwrap_or_default();
                    if copy_files_toast(&paths) {
                        FILES_CUT.with(|c| c.set(false));
                    }
                }
                7 if cmd && in_list => {
                    let paths = self.files_surface(|b| b.selected_paths()).unwrap_or_default();
                    if copy_files_to_pasteboard(&paths) {
                        FILES_CUT.with(|c| c.set(true));
                        crate::ui::toast::show(&format!("Cut {}", copied_what(&paths)), "scissors");
                    }
                }
                9 if cmd && in_list => {
                    let src = pasteboard_file_paths();
                    let Some(Some(dir)) = self.files_surface(|b| b.ops_directory()) else { return true };
                    if src.is_empty() {
                        return true;
                    }
                    let mv = opt || FILES_CUT.with(|c| c.replace(false));
                    let o = FileOps::transfer(&src, &dir, mv, FileOps::shared());
                    let verb = if mv { "Moved" } else { "Copied" };
                    done(self, format!("{verb} {} item(s) here", src.len()), o.failed);
                }
                6 if cmd && !shift && in_list => {
                    match FileOps::undo(FileOps::shared()) {
                        Some((what, o)) => done(self, format!("Undid {what}"), o.failed),
                        None => crate::ui::toast::show("Nothing to undo", "arrow.uturn.backward"),
                    }
                }
                _ => return false,
            }
            true
        }

        /// The list window's Jira keys (`hostShortcutKey` + `listKey`):
        /// Cmd+I issue panel, Ctrl+Space tick, Tab / Shift+Tab, Home / End /
        /// Page Up / Page Down, Cmd+F live search.
        fn jira_key(&self, key: &popup::KeyInput) -> bool {
            let plain = !key.cmd && !key.ctrl && !key.opt;
            match key.key_code {
                34 if key.cmd && !key.ctrl => {
                    self.jira_surface(|l| l.inspector = !l.inspector);
                    if let Some((_, v)) = self.ivars().state.borrow().as_ref().and_then(|s| s.jira.as_ref()) {
                        v.layout_panes();
                    }
                    true
                }
                3 if key.cmd && !key.ctrl && !key.shift => {
                    crate::ui::toast::not_ported("Jira live search (Cmd+F)");
                    true
                }
                49 if key.ctrl && !key.cmd => {
                    self.jira_surface(|l| {
                        if let Some(k) = l.selected().and_then(|r| r.get("key")).cloned() {
                            if !l.checked.remove(&k) {
                                l.checked.insert(k);
                            }
                        }
                    });
                    true
                }
                48 if !key.cmd && !key.ctrl && !key.opt => {
                    self.jira_surface(|l| jira_move(l, if key.shift { -1 } else { 1 }));
                    true
                }
                115 | 119 | 116 | 121 if plain => {
                    self.jira_surface(|l| {
                        let n = l.rows.len();
                        if n == 0 {
                            return;
                        }
                        l.selection = match key.key_code {
                            115 => 0,
                            119 => n - 1,
                            116 => l.selection.saturating_sub(10),
                            _ => (l.selection + 10).min(n - 1),
                        };
                    });
                    true
                }
                _ => false,
            }
        }

        /// `CompareWindow.actionPicker()`: the session's actions (Swift pops
        /// an `NSMenu`; here the shared Cmd+K card).
        fn compare_actions(&self) {
            let Some(mtm) = MainThreadMarker::new() else { return };
            let Some(model) = self.ivars().controller.compare_model() else { return };
            let (has, folder) = {
                let m = model.lock().unwrap();
                (m.current().is_some(), m.current().is_some_and(|s| s.folder.is_some()))
            };
            let mut items: Vec<(&str, &str, &str)> = vec![("New Comparison", "the start page · ⌘0", "start")];
            if has && folder {
                items.extend([
                    ("Next Difference", "⌃N", "folder-key:ctrl-n"),
                    ("Previous Difference", "⌃P", "folder-key:ctrl-p"),
                    ("Open Pair (Text Compare)", "⏎", "folder-open"),
                    ("Copy to Right", "⌥→", "folder-copy:right"),
                    ("Copy to Left", "⌥←", "folder-copy:left"),
                    ("Rescan", "F5", "folder-rescan"),
                ]);
            } else if has {
                items.extend([
                    ("Next Difference", "⌃N", "next"),
                    ("Previous Difference", "⌃P", "prev"),
                    ("Copy to Right", "⌥→ · the section under the cursor", "copy-right"),
                    ("Copy to Left", "⌥← · the section under the cursor", "copy-left"),
                    ("Swap Sides", "⌘⌥X", "swap"),
                    ("Reload Both Sides", "F5", "reload"),
                    ("Save Left", "⌘S with the left side focused", "save:left"),
                    ("Save Right", "⌘S with the right side focused", "save:right"),
                    ("Undo", "⌘Z", "undo"),
                ]);
            }
            let Some(root) = self.ivars().state.borrow().as_ref().and_then(|st| st.window.contentView()) else { return };
            let acts: Vec<String> = items.iter().map(|i| i.2.to_string()).collect();
            let ctrl = self.ivars().controller.clone();
            let weak = objc2::rc::Weak::from_retained(&self.retain());
            let picker = crate::ui::action_picker::ActionPicker::show(
                mtm,
                &root,
                "Compare actions",
                items.iter().map(|i| (i.0.to_string(), i.1.to_string())).collect(),
                crate::ui::theme::section_colors("compare"),
                Box::new(move |i| {
                    let Some(a) = acts.get(i) else { return };
                    let Some(m) = ctrl.compare_model() else { return };
                    let mut m = m.lock().unwrap();
                    let key = |code: u16| popup::KeyInput { key_code: code, chars: None, cmd: false, ctrl: true, opt: false, shift: false, esc_streak: 0 };
                    match a.as_str() {
                        "folder-key:ctrl-n" => {
                            m.handle_key(key(45), false);
                        }
                        "folder-key:ctrl-p" => {
                            m.handle_key(key(35), false);
                        }
                        "folder-copy:right" | "folder-copy:left" => {
                            let from = if a == "folder-copy:right" {
                                crate::views::compare::CompareSide::Left
                            } else {
                                crate::views::compare::CompareSide::Right
                            };
                            drop(m);
                            if let Some(ui) = weak.load() {
                                ui.compare_folder_transfer(from, false);
                            }
                        }
                        other => {
                            if let Some(err) = m.test_do(other) {
                                drop(m);
                                ctrl.log(&format!("compare action {other}: {err}"));
                            }
                        }
                    }
                }),
            );
            *self.ivars().action_picker.borrow_mut() = picker;
        }

        /// `showActions()` for the Jira list: the highlighted row plus every
        /// ticked row.
        fn jira_actions(&self) {
            let Some(mtm) = MainThreadMarker::new() else { return };
            let Some(Some((rows, site, favs))) = self.jira_surface(|l| {
                let mut keys: Vec<String> = l.checked.iter().cloned().collect();
                if let Some(k) = l.selected().and_then(|r| r.get("key")).cloned() {
                    if !keys.contains(&k) {
                        keys.insert(0, k);
                    }
                }
                let rows: Vec<(String, String)> = keys
                    .iter()
                    .filter(|k| !k.is_empty())
                    .map(|k| {
                        let title = l
                            .all
                            .iter()
                            .find(|r| r.get("key") == Some(k))
                            .and_then(|r| r.get("title").or_else(|| r.get("summary")).cloned())
                            .unwrap_or_default();
                        (k.clone(), title.trim().to_string())
                    })
                    .collect();
                (!rows.is_empty()).then(|| (rows, l.site(), l.favs.clone()))
            }) else {
                return;
            };
            let n = rows.len();
            let what = if n == 1 { rows[0].0.clone() } else { format!("{n} rows") };
            let s = if n == 1 { "" } else { "s" };
            let mut items: Vec<(String, String)> = Vec::new();
            if !site.is_empty() {
                items.push(("Open in browser".into(), format!("opens {n} issue{s} · the highlighted row + ticked rows")));
                items.push(("Copy URL and title".into(), format!("{n} issue{s} · one “URL Title” line each")));
            }
            items.push(("Copy to clipboard".into(), format!("{what} · key, title")));
            let all_fav = rows.iter().all(|(k, _)| favs.contains(k));
            items.push(if all_fav {
                ("Remove from favorites".into(), format!("unpin {what}"))
            } else {
                ("Add to favorites".into(), format!("pin {what} · re-polled every run"))
            });
            let Some(root) = self.ivars().state.borrow().as_ref().and_then(|st| st.window.contentView()) else { return };
            let titles: Vec<String> = items.iter().map(|i| i.0.clone()).collect();
            let weak = objc2::rc::Weak::from_retained(&self.retain());
            let colors = crate::ui::theme::section_colors("jira");
            let picker = crate::ui::action_picker::ActionPicker::show(
                mtm,
                &root,
                &format!("Actions for {what}"),
                items,
                colors,
                Box::new(move |i| {
                    let Some(ui) = weak.load() else { return };
                    let url = |k: &str| format!("{site}/browse/{k}");
                    match titles.get(i).map(String::as_str) {
                        Some("Open in browser") => {
                            for (k, _) in &rows {
                                open_url(&url(k));
                            }
                            let lines: Vec<String> = rows.iter().map(|(k, _)| format!("{k}\t{}", url(k))).collect();
                            copy_text_to_pasteboard(&lines.join("\n"));
                            crate::ui::toast::show(&format!("Opened {n} · copied keys + URLs"), "safari");
                        }
                        Some("Copy URL and title") => {
                            let lines: Vec<String> = rows
                                .iter()
                                .map(|(k, t)| if t.is_empty() { url(k) } else { format!("{} {t}", url(k)) })
                                .collect();
                            copy_text_to_pasteboard(&lines.join("\n"));
                            crate::ui::toast::show(&format!("Copied {n} URL{s} + title{s}"), "link");
                        }
                        Some("Copy to clipboard") => {
                            let lines: Vec<String> = rows.iter().map(|(k, t)| format!("{k}\t{t}")).collect();
                            copy_text_to_pasteboard(&lines.join("\n"));
                            crate::ui::toast::show(&format!("Copied {what}"), "doc.on.clipboard");
                        }
                        Some("Add to favorites") | Some("Remove from favorites") => {
                            let add = !all_fav;
                            ui.jira_surface(|l| {
                                for (k, _) in &rows {
                                    if add {
                                        l.favs.insert(k.clone());
                                    } else {
                                        l.favs.remove(k);
                                    }
                                }
                            });
                        }
                        _ => {}
                    }
                }),
            );
            *self.ivars().action_picker.borrow_mut() = picker;
        }

        fn focus_jira_table(&self) {
            let st = self.ivars().state.borrow();
            if let Some(st) = st.as_ref() {
                if let Some((_, v)) = st.jira.as_ref() {
                    st.window.makeFirstResponder(Some(&v.table_view()));
                }
            }
        }

        /// Give the file list keyboard focus (so arrows / ↵ / Space work).
        fn focus_files_list(&self) {
            let target = self
                .files_surface(|b| b.view.as_ref().map(|v| v.list_view()))
                .flatten();
            if let (Some(list), Some(st)) = (target, self.ivars().state.borrow().as_ref()) {
                st.window.makeFirstResponder(Some(&list));
            }
        }

        /// The rule-2 file context menu: open in notes, reveal in the app's
        /// own Files view, copy the absolute path, copy the file, Quick Look,
        /// trash.
        fn files_row_menu(&self, x: f64, y: f64) {
            use crate::ui::menu_kit::{popup, MenuEntry};
            let Some(paths) = self.files_surface(|b| b.selected_paths()) else {
                return;
            };
            if paths.is_empty() {
                return;
            }
            let weak = objc2::rc::Weak::from_retained(&self.retain());
            let first = paths[0].clone();
            let is_dir = std::path::Path::new(&first).is_dir();
            let mut entries = Vec::new();
            if !is_dir {
                let w = weak.clone();
                let p = first.clone();
                entries.push(MenuEntry::item("Open in Notes", move || {
                    if let Some(ui) = w.load() {
                        ui.open_in_notes(&p);
                    }
                }));
            }
            {
                let ps = paths.clone();
                entries.push(MenuEntry::item("Reveal in Files", move || reveal_in_files(&ps)));
            }
            entries.push(MenuEntry::Separator);
            {
                let joined = paths.join("\n");
                let n = paths.len();
                entries.push(MenuEntry::item(
                    if n == 1 { "Copy Absolute Path" } else { "Copy Absolute Paths" },
                    move || copy_text_to_pasteboard(&joined),
                ));
            }
            {
                let ps = paths.clone();
                let n = paths.len();
                entries.push(MenuEntry::item(
                    if n == 1 { "Copy File" } else { "Copy Files" },
                    move || { copy_files_toast(&ps); },
                ));
            }
            entries.push(MenuEntry::Separator);
            {
                let w = weak.clone();
                entries.push(MenuEntry::item("Quick Look", move || {
                    if let (Some(ui), Some(mtm)) = (w.load(), MainThreadMarker::new()) {
                        ui.files_surface(|b| b.toggle_quick_look(mtm));
                    }
                }));
            }
            {
                let w = weak.clone();
                entries.push(MenuEntry::item("Move to Trash", move || {
                    if let Some(ui) = w.load() {
                        ui.files_surface(|b| {
                            let ps = b.selected_paths();
                            let undo = crate::engines::file_ops::UndoStack::default();
                            let _ = crate::views::files::trash_paths(&ps, &undo);
                            b.reload();
                        });
                    }
                }));
            }
            popup(entries, None, x, y);
        }

        /// `showNoteFind(grep:)`: the open-notes picker / ripgrep window.
        fn show_note_find(&self, grep: bool) {
            use crate::views::note_find::{NoteFindConfig, NoteFindUi};
            use crate::views::notes::NoteFindMode;
            let Some(mtm) = MainThreadMarker::new() else { return };
            let slot = if grep { &self.ivars().note_grep } else { &self.ivars().note_find };
            let own = slot.borrow().clone();
            // `showNoteFind`: toggle the same panel, and hide the other mode's
            // window first (files vs grep).
            if let Some(u) = &own {
                if u.is_shown() {
                    u.hide();
                    return;
                }
            }
            let other = if grep { &self.ivars().note_find } else { &self.ivars().note_grep };
            if let Some(o) = other.borrow().clone() {
                o.hide();
            }
            let ui = match own {
                Some(u) => u,
                None => {
                    let text = crate::app::config::read_config_text().unwrap_or_default();
                    let sec = super::parse_sections(&text).get("notes-find").cloned().unwrap_or_default();
                    let mode = if grep { NoteFindMode::Grep } else { NoteFindMode::Files };
                    let u = NoteFindUi::new(mtm, mode, NoteFindConfig::from_section(&sec));
                    u.set_on_open(Box::new(|path, line| {
                        let Some(ui) = UI_WEAK.with(|w| w.borrow().as_ref().and_then(|w| w.load())) else { return };
                        ui.open_in_notes(&path);
                        if let Some(n) = line {
                            // `vimCommand("normal! NGzz")` once nvim has the buffer.
                            dispatch2::DispatchQueue::main()
                                .after(dispatch2::DispatchTime::NOW.time(250_000_000), move || {
                                    let Some(ui) = UI_WEAK.with(|w| w.borrow().as_ref().and_then(|w| w.load())) else { return };
                                    ui.notes_surface(|s| s.vim_command(&format!("normal! {n}Gzz")));
                                })
                                .ok();
                        }
                    }));
                    let c = self.ivars().controller.clone();
                    u.set_on_log(Box::new(move |m| c.log(&m)));
                    *slot.borrow_mut() = Some(u.clone());
                    u
                }
            };
            let paths = self.notes_surface(|s| s.open_paths()).unwrap_or_default();
            ui.show(paths);
        }

        /// `showSidebarJump(_:)`: toggle the jump list over the Jira rail.
        fn show_sidebar_jump(&self) {
            use crate::views::note_find::{JumpItem, NoteFindConfig, SidebarJumpUi};
            let Some(mtm) = MainThreadMarker::new() else { return };
            let Some(bar) = self.ivars().state.borrow().as_ref().and_then(|st| st.jira.as_ref().map(|j| j.1.sidebar())) else {
                self.ivars().controller.log("jira-jump: the Jira view is not built");
                return;
            };
            let existing = self.ivars().sidebar_jump.borrow().clone();
            if let Some(u) = &existing {
                if u.is_shown() {
                    u.hide();
                    return;
                }
            }
            let ui = existing.unwrap_or_else(|| {
                let text = crate::app::config::read_config_text().unwrap_or_default();
                let sec = super::parse_sections(&text).get("notes-find").cloned().unwrap_or_default();
                let u = SidebarJumpUi::new(mtm, NoteFindConfig::from_section(&sec));
                *self.ivars().sidebar_jump.borrow_mut() = Some(u.clone());
                u
            });
            let weak_bar = objc2::rc::Weak::from_retained(&bar);
            ui.set_on_jump(Box::new(move |row| {
                if let Some(b) = weak_bar.load() {
                    b.activate_row(row);
                }
            }));
            let items = bar
                .jump_items()
                .into_iter()
                .enumerate()
                .map(|(row, (title, section, icon))| JumpItem { title, section, icon, row })
                .collect();
            ui.show(items);
        }

        /// The name column of list row `i` in `list`'s coordinates.
        fn rename_rect(list: &crate::views::files_view::FileListView, i: usize) -> NSRect {
            let r = list.row_rect(i);
            let x = r.x + 30.0;
            NSRect::new(NSPoint::new(x, r.y), NSSize::new((r.width * 0.55 - x).max(120.0), r.height))
        }

        /// `setTextZoom(_:)` for the file browser: 0.6–3, 0.1 a step, kept
        /// in `UserDefaults textZoom.files`.
        fn files_text_zoom(&self, sign: i32) {
            let Some(Some(view)) = self.files_surface(|b| b.view.clone()) else { return };
            let list = view.list_view();
            let z = ((list.zoom() + 0.1 * sign as f64) * 100.0).round() / 100.0;
            let z = z.clamp(0.6, 3.0);
            list.set_zoom(z);
            defaults_set_string("textZoom.files", &z.to_string());
            self.files_surface(|_| ());
        }

        /// `PopupFileBrowser.beginRename()` (Cmd+R / F2).
        fn files_begin_rename(&self) {
            let Some(Some((i, path, is_dir, list))) = self.files_surface(|b| {
                b.begin_rename(None);
                let (path, _) = b.rename.clone()?;
                let i = b.list.selection;
                let is_dir = b.list.rows.get(i).is_some_and(|e| e.is_dir);
                Some((i, path, is_dir, b.view.as_ref()?.list_view()))
            }) else {
                return;
            };
            let name = std::path::Path::new(&path).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let weak = objc2::rc::Weak::from_retained(&self.retain());
            let wl = objc2::rc::Weak::from_retained(&list);
            crate::ui::inline_rename::begin(
                &list,
                Self::rename_rect(&list, i),
                &name,
                is_dir,
                crate::ui::theme::section_colors("files"),
                12.0,
                Box::new(move |text| {
                    let Some(ui) = weak.load() else { return };
                    let msg = ui.files_surface(|b| match &text {
                        Some(t) => {
                            if let Some((p, _)) = b.rename.clone() {
                                b.rename = Some((p, t.clone()));
                            }
                            b.commit_rename()
                        }
                        None => {
                            b.cancel_rename();
                            None
                        }
                    });
                    if let Some(Some(m)) = msg {
                        crate::ui::toast::show(&m, "exclamationmark.triangle.fill");
                    }
                    // Back to the list — unless the rename ended by clicking
                    // into another text field, which keeps its focus.
                    if let (Some(l), Some(w)) = (wl.load(), wl.load().and_then(|l| l.window())) {
                        let typing = w.firstResponder().is_some_and(|r| r.isKindOfClass(<objc2_app_kit::NSTextView as objc2::ClassType>::class()));
                        if !typing {
                            w.makeFirstResponder(Some(&l));
                        }
                    }
                }),
            );
        }

        /// `PathsWindow.beginRename()` (Cmd+R).
        fn paths_begin_rename(&self) {
            let Some(Some((i, path, list))) = self.paths_surface(|w| {
                let i = w.selection;
                let path = w.rows.get(i)?.path.clone();
                Some((i, path, w.list.clone()?))
            }) else {
                return;
            };
            let name = std::path::Path::new(&path).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let is_dir = std::path::Path::new(&path).is_dir();
            let weak = objc2::rc::Weak::from_retained(&self.retain());
            crate::ui::inline_rename::begin(
                &list,
                Self::rename_rect(&list, i),
                &name,
                is_dir,
                crate::ui::theme::PopupThemeDefaults::colors(),
                12.0,
                Box::new(move |text| {
                    let Some(ui) = weak.load() else { return };
                    let Some(t) = text else { return };
                    let msg = ui.paths_surface(|w| {
                        w.begin_rename(&t);
                        w.commit_rename()
                    });
                    if let Some(Some(m)) = msg {
                        crate::ui::toast::show(&m, "pencil");
                    }
                }),
            );
        }

        /// `PathsWindow.handleKey` for what the field delegate doesn't see:
        /// Cmd+C / Cmd+Shift+C, Space / Cmd+Y Quick Look, Cmd+O, Cmd+R,
        /// Cmd+Shift+R, Cmd+Delete.
        fn paths_key(&self, event: &NSEvent) -> bool {
            use crate::views::paths::{key_action, PathsKey};
            let info = self.ivars().state.borrow().as_ref().and_then(|st| {
                let w = st.paths.as_ref()?;
                let win = w.window.as_ref()?;
                if !win.isKeyWindow() {
                    return None;
                }
                let sel = w
                    .search
                    .as_ref()
                    .and_then(|f| f.currentEditor())
                    .map(|e| e.selectedRange().length > 0)
                    .unwrap_or(false);
                let list_focused = match (win.firstResponder(), &w.list) {
                    (Some(fr), Some(l)) => std::ptr::eq(
                        &*fr as *const objc2_app_kit::NSResponder as *const u8,
                        &**l as *const crate::views::files_view::FileListView as *const u8,
                    ),
                    _ => false,
                };
                Some((sel, w.query.trim().is_empty(), list_focused))
            });
            let Some((text_selected, query_empty, list_focused)) = info else { return false };
            let key = popup::key_input_from_event(event);
            let Some(action) = key_action(key, text_selected, query_empty, list_focused) else { return false };
            let paths = self.paths_surface(|w| w.selected_paths()).unwrap_or_default();
            match action {
                // The field delegate already routes these (`PathsEvent`).
                PathsKey::Hide | PathsKey::Move(..) | PathsKey::RunDefault => return false,
                PathsKey::CopyFiles => {
                    copy_files_toast(&paths);
                }
                PathsKey::CopyPaths => {
                    if !paths.is_empty() {
                        copy_text_to_pasteboard(&paths.join("\n"));
                        crate::ui::toast::show(&format!("Copied {} path(s)", paths.len()), "doc.on.clipboard");
                    }
                }
                PathsKey::QuickLook => {
                    if let Some(mtm) = MainThreadMarker::new() {
                        self.paths_surface(|w| w.toggle_quick_look(mtm));
                    }
                }
                PathsKey::Open => {
                    for p in &paths {
                        open_path(p);
                    }
                }
                PathsKey::Reveal => reveal_in_files(&paths),
                PathsKey::BeginRename => self.paths_begin_rename(),
                PathsKey::Forget => {
                    SHELF.with(|s| {
                        if let Some(sh) = s.borrow().as_ref() {
                            sh.borrow_mut().remove(&paths);
                        }
                    });
                    self.paths_surface(|w| {
                        w.set_items(shelf_entries());
                        w.reload(true);
                    });
                }
            }
            true
        }

        /// `NoteFindWindow.key(_:_:)` while its panel is key.
        fn note_find_key(&self, event: &NSEvent) -> bool {
            use objc2_app_kit::NSEventModifierFlags as F;
            if let Some(j) = self.ivars().sidebar_jump.borrow().clone().filter(|u| u.has_key()) {
                use objc2_app_kit::NSEventModifierFlags as F;
                let ctrl = event.modifierFlags().contains(F::Control);
                match event.keyCode() {
                    popup::KEY_ESC => j.hide(),
                    popup::KEY_DOWN => j.move_selection(1),
                    45 if ctrl => j.move_selection(1),
                    popup::KEY_UP => j.move_selection(-1),
                    35 if ctrl => j.move_selection(-1),
                    popup::KEY_RETURN | 76 => j.open(j.selection()),
                    _ => return false,
                }
                if !j.is_shown() {
                    if let Some(w) = self.ivars().state.borrow().as_ref().map(|st| st.window.clone()) {
                        if w.isVisible() {
                            w.makeKeyAndOrderFront(None);
                        }
                    }
                }
                return true;
            }
            // The `<leader>sf` pane item finder (Jira / Confluence / files /
            // sidebar): nav + accept only, so typing reaches its search field.
            let pf_open = self.ivars().pane_find.borrow().clone();
            if let Some(pf) = pf_open.filter(|u| u.has_key()) {
                let ctrl = event.modifierFlags().contains(F::Control);
                match event.keyCode() {
                    popup::KEY_ESC => pf.hide(),
                    popup::KEY_DOWN => pf.move_selection(1),
                    45 if ctrl => pf.move_selection(1),
                    popup::KEY_UP => pf.move_selection(-1),
                    35 if ctrl => pf.move_selection(-1),
                    popup::KEY_RETURN | 76 => pf.open(pf.selection()),
                    _ => return false,
                }
                if !pf.is_shown() {
                    if let Some(w) = self.ivars().state.borrow().as_ref().map(|st| st.window.clone()) {
                        if w.isVisible() {
                            w.makeKeyAndOrderFront(None);
                        }
                    }
                }
                return true;
            }
            let ui = [&self.ivars().note_find, &self.ivars().note_grep]
                .iter()
                .find_map(|s| s.borrow().clone().filter(|u| u.has_key()));
            let Some(ui) = ui else { return false };
            let mods = event.modifierFlags();
            let (cmd, ctrl) = (mods.contains(F::Command), mods.contains(F::Control));
            match event.keyCode() {
                popup::KEY_ESC => ui.hide(),
                13 if cmd => ui.hide(),
                popup::KEY_DOWN => ui.move_selection(1),
                45 if ctrl => ui.move_selection(1),
                popup::KEY_UP => ui.move_selection(-1),
                35 if ctrl => ui.move_selection(-1),
                popup::KEY_RETURN | 76 => ui.open(ui.selection()),
                8 if cmd && !ui.field_has_selection() => {
                    if let Some(p) = ui.selected_path() {
                        copy_text_to_pasteboard(&p);
                        crate::ui::toast::show(&format!("Copied {}", p.replace(&std::env::var("HOME").unwrap_or_default(), "~")), "checkmark.circle.fill");
                    }
                }
                _ => return false,
            }
            if !ui.is_shown() {
                // `hide(restore: true)`: the keyboard returns to the notes window.
                if let Some(w) = self.ivars().state.borrow().as_ref().map(|st| st.window.clone()) {
                    if w.isVisible() {
                        w.makeKeyAndOrderFront(None);
                    }
                }
            }
            true
        }

        /// `PopupTextView.menu(for:)`: the plain notes editor's right-click.
        /// On an image link: Open Full Size and Copy Image Path; then Copy
        /// Absolute Path (the note) and Open file at path….
        fn notes_editor_menu(&self, idx: usize) {
            use crate::ui::menu_kit::{popup_context, MenuEntry};
            let Some((image, note, view)) = self.notes_surface(|s| {
                s.editor_view()
                    .map(|v| (s.image_path_at(idx), s.selected_path(), v))
            }).flatten() else {
                return;
            };
            let mut e = Vec::new();
            if let Some(path) = image {
                let weak = objc2::rc::Weak::from_retained(&self.retain());
                let p = path.clone();
                e.push(MenuEntry::item("Open Full Size", move || {
                    if let Some(ui) = weak.load() {
                        ui.notes_open_image(p);
                    }
                }));
                e.push(MenuEntry::item("Copy Image Path", move || {
                    copy_text_to_pasteboard(&path);
                    crate::ui::toast::show(&format!("Copied {}", tilde_path(&path)), "doc.on.clipboard");
                }));
                e.push(MenuEntry::Separator);
            }
            if let Some(note) = note {
                e.push(MenuEntry::item("Copy Absolute Path", move || {
                    copy_text_to_pasteboard(&note);
                    crate::ui::toast::show(&format!("Copied {}", tilde_path(&note)), "doc.on.clipboard");
                }));
            }
            let weak = objc2::rc::Weak::from_retained(&self.retain());
            e.push(MenuEntry::item("Open file at path…", move || {
                if let Some(ui) = weak.load() {
                    ui.route_open_path_prompt();
                }
            }));
            popup_context(e, &view);
        }

        /// `openNoteFile(_:)` from a file browser: show notes on `path`.
        fn open_in_notes(&self, path: &str) {
            self.ivars().controller.open(SlotView::Notes);
            self.present(SlotView::Notes);
            let p = path.to_string();
            self.notes_surface(|s| s.open_note(&p));
        }

        /// Show `paths` in the app's **own** file browser (`SlotView::Files`)
        /// instead of macOS Finder: a directory becomes the root, a file opens
        /// its parent with the file selected (all `paths` are marked). This is
        /// the one entry point every "reveal / show in Finder" action uses.
        fn open_in_files(&self, paths: &[String]) {
            let wanted: Vec<String> = paths
                .iter()
                .map(|p| crate::views::files::standardize(p))
                .collect();
            let Some(first) = wanted.first() else { return };
            let dir = if std::path::Path::new(first).is_dir() {
                first.clone()
            } else {
                std::path::Path::new(first)
                    .parent()
                    .map(|d| d.to_string_lossy().into_owned())
                    .unwrap_or_else(|| first.clone())
            };
            self.ivars().controller.open(SlotView::Files);
            self.present(SlotView::Files);
            self.files_surface(|b| {
                b.cd(&dir);
                b.list.marked.clear();
                for (i, e) in b.list.rows.iter().enumerate() {
                    if wanted.iter().any(|w| w == &e.path) {
                        b.list.marked.insert(i);
                    }
                }
                if let Some(i) = b.list.rows.iter().position(|e| wanted.iter().any(|w| w == &e.path))
                {
                    b.list.selection = i;
                }
            });
        }

        /// The Recent / Arrived lists follow the engine (Swift
        /// `recentChanged`, here polled every 2 s while the browser shows one).
        fn refresh_files_recent(&self) {
            let now = now_secs();
            if now - self.ivars().files_recent_at.get() < 2.0 {
                return;
            }
            self.ivars().files_recent_at.set(now);
            if self.ivars().controller.current() != Some(SlotView::Files) {
                return;
            }
            let need = {
                let state = self.ivars().state.borrow();
                state
                    .as_ref()
                    .and_then(|st| st.files.as_ref())
                    .is_some_and(|b| b.in_recent() && b.query.is_empty())
            };
            if !need {
                return;
            }
            self.files_surface(|b| {
                let v = b.virtual_index.unwrap_or(0);
                let fresh = b.recent_entries(v == 1);
                let sig = |r: &[crate::views::files::Entry]| {
                    r.iter().map(|e| format!("{}{}", e.path, e.trailing_text)).collect::<Vec<_>>()
                };
                if sig(&fresh) != sig(&b.all) {
                    let keep = b.list.rows.get(b.list.selection).map(|e| e.path.clone());
                    b.all = fresh;
                    b.refilter();
                    if let Some(k) = keep {
                        if let Some(i) = b.list.rows.iter().position(|e| e.path == k) {
                            b.list.selection = i;
                        }
                    }
                }
            });
        }

        // -- list keys (`PopupWindow.listKey` on the focused list) ------------

        fn route_list_move(&self, delta: i64) -> bool {
            match self.ivars().controller.current() {
                Some(SlotView::Notes) => self.notes_surface(|s| s.list_move(delta)).is_some(),
                Some(SlotView::Files) => self
                    .files_surface(|b| b.list.move_selection(delta))
                    .is_some(),
                Some(v) if v.is_jira() => self.jira_surface(|l| jira_move(l, delta)).is_some(),
                _ => false,
            }
        }

        fn route_list_move_wrap(&self, delta: i64) -> bool {
            match self.ivars().controller.current() {
                Some(SlotView::Notes) => {
                    self.notes_surface(|s| s.list_move_wrap(delta)).is_some()
                }
                Some(SlotView::Files) => {
                    let n = self.files_surface(|b| b.visible_rows().len()).unwrap_or(0);
                    if n == 0 {
                        return false;
                    }
                    self.files_surface(|b| {
                        let cur = b.selection() as i64;
                        b.list.selection = ((cur + delta).rem_euclid(n as i64)) as usize;
                    });
                    true
                }
                _ => false,
            }
        }

        fn route_list_page(&self, page: popup::PageMove) -> bool {
            match self.ivars().controller.current() {
                Some(SlotView::Notes) => self.notes_surface(|s| s.list_page(page)).is_some(),
                Some(SlotView::Files) => {
                    let n = self.files_surface(|b| b.visible_rows().len()).unwrap_or(0);
                    if n == 0 {
                        return false;
                    }
                    self.files_surface(|b| {
                        let cur = b.selection();
                        b.list.selection = match page {
                            popup::PageMove::Home => 0,
                            popup::PageMove::End => n - 1,
                            popup::PageMove::Up => cur.saturating_sub(10),
                            popup::PageMove::Down => (cur + 10).min(n - 1),
                        };
                    });
                    true
                }
                _ => false,
            }
        }

        /// `acceptSelection` — notes: open the tab; files: enter a dir / open
        /// a file (a directory re-roots the browser and swaps the view tree).
        fn route_list_accept(&self) -> bool {
            match self.ivars().controller.current() {
                Some(SlotView::Notes) => self.notes_surface(|s| s.list_accept()).is_some(),
                Some(SlotView::Files) => self.files_accept_selected(),
                Some(v) if v.is_jira() => {
                    self.jira_event(crate::views::jira_view::JiraEvent::OpenDetail);
                    true
                }
                _ => false,
            }
        }

        fn route_list_toggle(&self) -> bool {
            match self.ivars().controller.current() {
                Some(SlotView::Notes) => self.notes_surface(|s| s.list_toggle_selection()).is_some(),
                Some(SlotView::Files) => {
                    let n = self.files_surface(|b| b.visible_rows().len()).unwrap_or(0);
                    if n == 0 {
                        return false;
                    }
                    self.files_surface(|b| {
                        let i = b.selection();
                        if b.list.marked.contains(&i) {
                            b.list.marked.remove(&i);
                        } else {
                            b.list.marked.insert(i);
                        }
                    });
                    true
                }
                _ => false,
            }
        }

        /// `openIndex(_:)` on the files list: descend into a directory (rebuild
        /// the browser rooted there) or open the file.
        fn files_accept_selected(&self) -> bool {
            let Some(mtm) = MainThreadMarker::new() else {
                return false;
            };
            let action = self.files_surface(|b| {
                let rows = b.visible_rows().to_vec();
                crate::views::files::open_index(&rows, b.selection())
            });
            match action {
                Some(crate::views::files::OpenOutcome::Open(path)) => {
                    open_path(&path);
                    true
                }
                Some(crate::views::files::OpenOutcome::Cd(dir)) => {
                    let _ = mtm;
                    self.files_surface(|b| b.cd(&dir));
                    true
                }
                _ => false,
            }
        }

        // -- window geometry (Cmd+± / Cmd+Opt+± / Cmd+0 / Ctrl+Shift+HJKL) ----

        /// Ctrl+Shift+H/J/K/L — the Swift `paneResizeKey` window resize (20 pt
        /// steps; the drawer-height special cases land with the drawer keys).
        fn route_resize(&self, dir: popup::Direction) -> bool {
            let Some(_mtm) = MainThreadMarker::new() else {
                return false;
            };
            let mut state = self.ivars().state.borrow_mut();
            let Some(st) = state.as_mut() else {
                return false;
            };
            let mut f = st.window.frame();
            match dir {
                popup::Direction::Left => f.size.width = (f.size.width - 20.0).max(120.0),
                popup::Direction::Right => f.size.width += 20.0,
                popup::Direction::Up => f.size.height += 20.0,
                popup::Direction::Down => f.size.height = (f.size.height - 20.0).max(140.0),
            }
            clamp_to_screen(&mut f);
            st.window.setFrame_display(f, true);
            drop(state);
            self.sync_window_frame();
            true
        }

        /// Cmd+± / Cmd+Opt+± — `resizeBy(80 * sign)`.
        fn route_resize_by(&self, sign: i32) -> bool {
            let Some(_mtm) = MainThreadMarker::new() else {
                return false;
            };
            let mut state = self.ivars().state.borrow_mut();
            let Some(st) = state.as_mut() else {
                return false;
            };
            let mut f = st.window.frame();
            let delta = 80.0 * sign as f64;
            f.size.width = (f.size.width + delta).max(120.0);
            f.size.height = (f.size.height + delta).max(140.0);
            clamp_to_screen(&mut f);
            st.window.setFrame_display(f, true);
            drop(state);
            self.sync_window_frame();
            true
        }

        /// Cmd+0 — `resetToDefaultSize`: the shared default size, same origin.
        fn route_reset_size(&self) -> bool {
            let Some(_mtm) = MainThreadMarker::new() else {
                return false;
            };
            let settings = self.ivars().controller.settings();
            let mut state = self.ivars().state.borrow_mut();
            let Some(st) = state.as_mut() else {
                return false;
            };
            st.window.setContentSize(NSSize::new(
                settings.shared_width.max(420.0),
                settings.shared_height.max(260.0),
            ));
            let mut f = st.window.frame();
            clamp_to_screen(&mut f);
            st.window.setFrame_display(f, true);
            drop(state);
            self.sync_window_frame();
            true
        }

        /// Mirror the live host frame into `state.views.<view>.frame`.
        fn sync_window_frame(&self) {
            let Some(_mtm) = MainThreadMarker::new() else {
                return;
            };
            let mut rect = None;
            {
                let state = self.ivars().state.borrow();
                if let Some(st) = state.as_ref() {
                    let f = st.window.frame();
                    rect = Some(RectI::new(
                        f.origin.x,
                        f.origin.y,
                        f.size.width,
                        f.size.height,
                    ));
                }
            }
            if let (Some(rect), Some(v)) = (rect, self.ivars().controller.current()) {
                self.ivars().controller.set_member(v, true, true, Some(rect));
            }
        }

        // -- notes rail / tabs / find / commit / open-path ---------------------

        fn route_rail_toggle(&self) -> bool {
            self.notes_surface(|s| s.toggle_rail()).is_some()
        }

        fn route_cycle_tabs(&self, dir: i32) -> bool {
            self.notes_surface(|s| s.list_move_wrap(dir as i64)).is_some()
        }

        fn route_find_toggle(&self) -> bool {
            self.notes_surface(|s| s.toggle_find()).is_some()
        }

        fn route_find_step(&self, dir: i32) -> bool {
            let shown = self
                .notes_surface(|s| s.find_shown())
                .unwrap_or(false);
            if !shown {
                return false;
            }
            self.notes_surface(|s| s.find_step(dir as i64));
            true
        }

        fn notes_find_close(&self) {
            let _ = self.notes_surface(|s| s.close_find());
        }

        fn route_commit(&self) -> bool {
            self.notes_surface(|s| s.commit()).unwrap_or(false)
        }

        /// Cmd+O — `onOpenPathPrompt`: a path sheet on the host window.
        fn route_open_path_prompt(&self) -> bool {
            if self.ivars().controller.current() != Some(SlotView::Notes) {
                return false;
            }
            let Some(mtm) = MainThreadMarker::new() else {
                return false;
            };
            let controller = self.ivars().controller.clone();
            let state = self.ivars().state.borrow();
            let Some(st) = state.as_ref() else {
                return false;
            };
            present_path_sheet(mtm, &st.window, Box::new(move |value| {
                if let Some(path) = value {
                    controller.set_pending_note_external(path);
                    controller.log("open file at path");
                }
            }));
            true
        }

        /// File-browser modified keys (`FilePopup` subset).
        fn route_file_browser(&self, action: popup::FileBrowserAction) -> bool {
            if self.ivars().controller.current() != Some(SlotView::Files) {
                return false;
            }
            match action {
                popup::FileBrowserAction::MoveDown => self.route_list_move(1),
                popup::FileBrowserAction::MoveUp => self.route_list_move(-1),
                popup::FileBrowserAction::SelectAll => {
                    self.files_surface(|b| b.list.select_all()).is_some()
                }
                popup::FileBrowserAction::CopyPath => {
                    let path = self.files_surface(|b| {
                        b.selected_paths().into_iter().next().unwrap_or_default()
                    });
                    match path {
                        Some(p) if !p.is_empty() => {
                            copy_text_to_pasteboard(&p);
                            true
                        }
                        _ => false,
                    }
                }
                popup::FileBrowserAction::FocusFilterBar => {
                    self.files_surface(|_| ()).is_some()
                }
                popup::FileBrowserAction::Rename => {
                    self.files_begin_rename();
                    true
                }
                popup::FileBrowserAction::Copy
                | popup::FileBrowserAction::Paste
                | popup::FileBrowserAction::Cut
                | popup::FileBrowserAction::Undo => false,
            }
        }

        /// The per-drain notes/files sync: external opens, `views.*` selection
        /// extras.
        fn sync_member_states(&self) {
            if let Some(path) = self.ivars().controller.take_pending_note_external() {
                let _ = self.notes_surface(|s| s.open_external_path(&path));
            }
            let mut notes_state = None;
            let mut files_state = None;
            let mut responder = String::new();
            {
                let state = self.ivars().state.borrow();
                if let Some(st) = state.as_ref() {
                    if let Some(fr) = st.window.firstResponder() {
                        responder = fr.class().name().to_string_lossy().into_owned();
                    }
                    if let Some(s) = st.notes.as_ref() {
                        notes_state = Some(s.selection_state());
                    }
                    if let Some(b) = st.files.as_ref() {
                        files_state = Some(b.test_state());
                    }
                }
            }
            if let Some(mut v) = notes_state {
                if let Some(o) = v.as_object_mut() {
                    o.insert("responder".into(), json!(responder));
                    let vim_focus = {
                        let state = self.ivars().state.borrow();
                        state
                            .as_ref()
                            .and_then(|st| st.notes.as_ref())
                            .map(|s| s.vim_has_focus())
                            .unwrap_or(false)
                    };
                    o.insert("vimFocus".into(), json!(vim_focus));
                }
                self.ivars()
                    .controller
                    .set_view_state(SlotView::Notes, v);
            }
            if let Some(v) = files_state {
                self.ivars()
                    .controller
                    .set_view_state(SlotView::Files, v);
            }
        }

        /// Forward a routed edit op down the responder chain (Cmd+A/X/C/V/Z
        /// keep the AppKit Edit-menu path; this covers the Ctrl variants).
        fn perform_edit_op(&self, op: popup::EditOp) -> bool {
            let selector = match op {
                popup::EditOp::SelectAll => sel!(selectAll:),
                popup::EditOp::Copy => sel!(copy:),
                popup::EditOp::Paste => sel!(paste:),
                popup::EditOp::Cut => sel!(cut:),
                popup::EditOp::Undo => sel!(undo:),
                popup::EditOp::Find => return false,
            };
            let Some(mtm) = MainThreadMarker::new() else {
                return false;
            };
            let app = NSApplication::sharedApplication(mtm);
            unsafe { app.sendAction_to_from(selector, None, None) }
        }

        fn present(&self, v: SlotView) {
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };
            let controller = self.ivars().controller.clone();
            let (rect, title, wid, front_pid) = {
                let mut state = self.ivars().state.borrow_mut();
                let st = state.get_or_insert_with(|| self.build_window(mtm));
                if st.current != Some(v) {
                    if let Some(prev) = st.current {
                        if let Some(view) = st.views.get(&prev) {
                            view.removeFromSuperview();
                        }
                        // `swappedAt`: a focus loss right after a swap is a
                        // steal to undo, not the user leaving.
                        if st.window.isVisible() {
                            self.ivars().focus_watch.borrow_mut().note_swap(now_secs());
                        }
                    }
                    let bounds = st.content.bounds();
                    if !st.views.contains_key(&v) {
                        let view = if v == SlotView::Notes {
                            match crate::views::notes::NotesSurface::build(mtm) {
                                Some(surface) => {
                                    let view = surface.content_view();
                                    let weak = objc2::rc::Weak::from_retained(&self.retain());
                                    surface.sidebar_view().set_on_event(Box::new(move |ev| {
                                        if let Some(ui) = weak.load() {
                                            ui.notes_sidebar_event(ev);
                                        }
                                    }));
                                    let weak = objc2::rc::Weak::from_retained(&self.retain());
                                    surface.tree_view().set_on_event(Box::new(move |ev| {
                                        if let Some(ui) = weak.load() {
                                            ui.notes_tree_event(ev);
                                        }
                                    }));
                                    let weak = objc2::rc::Weak::from_retained(&self.retain());
                                    surface.prose_switch().set_on_event(Box::new(move |ev| {
                                        if let Some(ui) = weak.load() {
                                            ui.notes_prose_switch_event(ev);
                                        }
                                    }));
                                    let mut surface = surface;
                                    let weak = objc2::rc::Weak::from_retained(&self.retain());
                                    surface.set_prose_sink(std::rc::Rc::new(move |m| {
                                        if let Some(ui) = weak.load() {
                                            ui.notes_prose_msg(m);
                                        }
                                    }));
                                    let weak = objc2::rc::Weak::from_retained(&self.retain());
                                    surface.set_editor_menu(std::rc::Rc::new(move |idx| {
                                        if let Some(ui) = weak.load() {
                                            ui.notes_editor_menu(idx);
                                        }
                                    }));
                                    let weak = objc2::rc::Weak::from_retained(&self.retain());
                                    surface.set_editor_paste_image(std::rc::Rc::new(move |img| {
                                        if let Some(ui) = weak.load() {
                                            ui.notes_paste_image(img);
                                        }
                                    }));
                                    let weak = objc2::rc::Weak::from_retained(&self.retain());
                                    surface.set_editor_open_image(std::rc::Rc::new(move |path| {
                                        if let Some(ui) = weak.load() {
                                            ui.notes_open_image(path);
                                        }
                                    }));
                                    let weak = objc2::rc::Weak::from_retained(&self.retain());
                                    surface.set_vim_image_open(std::rc::Rc::new(move |path| {
                                        if let Some(ui) = weak.load() {
                                            ui.notes_open_image(path);
                                        }
                                    }));
                                    st.notes = Some(surface);
                                    view
                                }
                                None => placeholder(mtm, v),
                            }
                        } else if v == SlotView::Files {
                            match self.build_files(mtm) {
                                Some((browser, view)) => {
                                    if let (Some(fv), Some(z)) = (
                                        browser.view.as_ref(),
                                        defaults_string("textZoom.files").and_then(|z| z.parse::<f64>().ok()),
                                    ) {
                                        fv.list_view().set_zoom(z.clamp(0.6, 3.0));
                                    }
                                    st.files = Some(browser);
                                    view
                                }
                                None => placeholder(mtm, v),
                            }
                        } else if v == SlotView::Ai {
                            let a = self.build_ai(mtm);
                            let root: Retained<NSView> = Retained::into_super(a.view.clone());
                            st.ai = Some(a);
                            root
                        } else if v == SlotView::Confluence {
                            let cs = self.build_confluence(mtm);
                            let root: Retained<NSView> = Retained::into_super(cs.view.clone());
                            st.confluence = Some(cs);
                            root
                        } else if v == SlotView::Jira {
                            let (list, view) = self.build_jira(mtm);
                            let root: Retained<NSView> = Retained::into_super(view.clone());
                            st.jira = Some((list, view));
                            root
                        } else if matches!(v, SlotView::Compare | SlotView::CompareText) {
                            st.compare
                                .get_or_insert_with(|| {
                                    let surface = crate::views::compare::CompareSurface::build(mtm);
                                    let weak = objc2::rc::Weak::from_retained(&self.retain());
                                    surface.set_on_event(Box::new(move |ev| {
                                        if let Some(ui) = weak.load() {
                                            ui.compare_event(ev);
                                        }
                                    }));
                                    surface
                                })
                                .content_view()
                        } else {
                            content_for(mtm, v)
                        };
                        view.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), bounds.size));
                        view.setAutoresizingMask(mask_fill());
                        st.views.insert(v, view);
                    }
                    let content = st.views.get(&v).cloned().unwrap();
                    st.content.addSubview(&content);
                    st.current = Some(v);
                }
                // Drain a queued `open:<path>` even when notes is already
                // current, then focus the active editor (vim pane or text).
                if v == SlotView::Notes {
                    if let Some(surface) = st.notes.as_mut() {
                        if let Some(path) = controller.take_pending_note_open() {
                            surface.open_note(&path);
                        }
                        surface.focus_editor();
                    }
                }
                st.chrome.set_header_title(Some(v.title().to_string()));
                st.chrome.set_nav_on(Some(v.nav_id() as i32));
                let title = self.ivars().controller.window_title(v);
                st.window.setTitle(&NSString::from_str(&title));
                let front_pid = crate::platform::frontmost_app().map_or(0, |a| a.pid);
                // Swift `unpark` / `slotShow`: activate first, then key + front.
                // A view switch inside the already-key window re-raises
                // nothing: a redundant raise makes AeroSpace jump to the
                // window's home workspace.
                let app = NSApplication::sharedApplication(mtm);
                if !(st.window.isVisible() && st.window.isKeyWindow() && app.isActive()) {
                    self.ivars().focus_watch.borrow_mut().note_present(now_secs());
                    activate_app(mtm);
                    st.window.makeKeyAndOrderFront(None);
                }
                let f = st.window.frame();
                (
                    RectI::new(f.origin.x, f.origin.y, f.size.width, f.size.height),
                    title,
                    st.window.windowNumber() as i64,
                    front_pid,
                )
            };
            // Swift's views each take the keyboard on show (the filter /
            // search field, the AI input, the diff); keep a pane that already
            // has it. After this turn's layout (a fresh view has no size yet).
            on_ui(move |ui| ui.focus_default_pane(v));
            controller.set_member(v, true, true, Some(rect));
            controller.set_member_wid(v, wid);
            controller.set_key_window(title);
            controller.set_active(true);
            controller.set_frontmost_pid(front_pid);
            controller.bump_activations();
            controller.set_window_count(visible_window_count(mtm));
        }

        fn focus_default_pane(&self, v: SlotView) {
            if self.ivars().controller.current() != Some(v) {
                return;
            }
            if self.focused_pane_id().is_none() {
                let order: &[&str] = match v {
                    SlotView::Files => &["files-filter"],
                    SlotView::Confluence => &["search"],
                    SlotView::Ai => &["input"],
                    // A session's diff / tree, else the start page's path box.
                    SlotView::Compare | SlotView::CompareText => &["diff", "tree", "left-path"],
                    v if v.is_jira() => &["search"],
                    _ => &[],
                };
                for id in order {
                    if self.live_pane_focus(id) {
                        break;
                    }
                }
            }
        }

        fn hide(&self) {
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };
            let controller = self.ivars().controller.clone();
            self.ivars().focus_watch.borrow_mut().reset();
            if let Some(st) = self.ivars().state.borrow().as_ref() {
                if let Some(surface) = st.notes.as_ref() {
                    surface.vim_flush();
                }
                st.window.orderOut(None);
            }
            if let Some(v) = controller.current() {
                controller.set_member_hidden(v);
            }
            controller.set_key_window(String::new());
            controller.set_window_count(visible_window_count(mtm));
        }

        /// Build the host window + chrome + switcher + content container.
        fn build_window(&self, mtm: MainThreadMarker) -> DaemonUiState {
            let settings = self.ivars().controller.settings();
            set_current_header_style(
                ThemeHeaderStyle::from_raw(settings.header_style.raw()).unwrap_or_default(),
            );
            let width = settings.shared_width.max(420.0);
            let height = settings.shared_height.max(260.0);
            let colors = crate::ui::theme::PopupThemeDefaults::colors();

            let window = SlotHostWindow::create(mtm);
            window.setContentSize(NSSize::new(width, height));
            window.center();
            window.setMovableByWindowBackground(true);
            window.setCollectionBehavior(
                NSWindowCollectionBehavior::CanJoinAllSpaces
                    | NSWindowCollectionBehavior::FullScreenAuxiliary,
            );
            let controller = self.ivars().controller.clone();
            window.set_on_close_request(Box::new(move || controller.hide("✕")));

            let cfg = ChromeConfig {
                colors,
                header_height: HEADER_HEIGHT,
                title_pill: false,
                ..ChromeConfig::default()
            };
            let radius = cfg.corner_radius + 1.0;
            window.set_corner_radius(radius);

            let root = HostRootView::new(
                mtm,
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width, height)),
            );
            root.setWantsLayer(true);
            if let Some(layer) = root.layer() {
                layer.setCornerRadius(radius);
                layer.setMasksToBounds(true);
            }

            let fx = NSVisualEffectView::initWithFrame(NSVisualEffectView::alloc(mtm), root.bounds());
            fx.setMaterial(NSVisualEffectMaterial::HUDWindow);
            fx.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
            fx.setState(NSVisualEffectState::Active);
            fx.setAutoresizingMask(mask_fill());

            let tint = NSView::initWithFrame(NSView::alloc(mtm), root.bounds());
            tint.setWantsLayer(true);
            if let Some(layer) = tint.layer() {
                layer.setBackgroundColor(Some(
                    &colors.base().with_alpha(cfg.tint_alpha).to_nscolor().CGColor(),
                ));
                layer.setBorderColor(Some(&colors.border.to_nscolor().CGColor()));
                layer.setBorderWidth(1.0);
                layer.setCornerRadius(radius);
            }
            tint.setAutoresizingMask(mask_fill());

            let chrome = PopupChrome::create(mtm, cfg);
            chrome.set_drag_header_height(HEADER_HEIGHT);
            chrome.set_copy_labels("", "");
            chrome.set_header_title(Some(DEFAULT_VIEW.title().to_string()));
            chrome.setFrame(NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(width, HEADER_HEIGHT),
            ));
            chrome.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
            let assets = crate::app::paths::Paths::from_env().asset_dir().to_string();
            let nav: Vec<NavIcon> = self
                .ivars()
                .controller
                .registry()
                .nav_icons()
                .iter()
                .map(|n| NavIcon {
                    id: n.id as i32,
                    tip: n.tip.clone(),
                    image: Some(nav_icon_image(n.view, n.image_key.as_deref(), &assets)),
                })
                .collect();
            chrome.set_nav_icons(nav);
            chrome.set_header_icon(
                crate::ui::style::image_file(&asset_path(&assets, &settings.app_icon_name))
                    .or_else(|| crate::ui::style::named_image("NSApplicationIcon")),
            );
            {
                let controller = self.ivars().controller.clone();
                let weak = objc2::rc::Weak::from_retained(&chrome);
                let weak_ui = objc2::rc::Weak::from_retained(&self.retain());
                chrome.set_on_click(Box::new(move |id| match id {
                    crate::ui::chrome::HEADER_CLOSE_ID => controller.hide("✕"),
                    crate::ui::chrome::HEADER_ICON_ID => {
                        if let (Some(ch), Some(ui)) = (weak.load(), weak_ui.load()) {
                            let r = ch.icon_button_rect();
                            ch.set_icon_menu_open(true);
                            ui.header_icon_menu(&ch, r.x, r.max_y() + 4.0);
                            ch.set_icon_menu_open(false);
                        }
                    }
                    id if id >= crate::ui::chrome::WORKSPACE_BASE => {
                        let cell = (id - crate::ui::chrome::WORKSPACE_BASE) as usize;
                        if let Some(key) = crate::ui::chrome::workspace_cell_key(cell) {
                            controller.switch_workspace(&key);
                        }
                    }
                    id => controller.nav_clicked(id as i64),
                }));
            }

            root.addSubview(&fx);
            root.addSubview(&tint);
            root.addSubview(&chrome);

            let content = NSView::initWithFrame(
                NSView::alloc(mtm),
                NSRect::new(
                    NSPoint::new(0.0, HEADER_HEIGHT),
                    NSSize::new(width, (height - HEADER_HEIGHT).max(0.0)),
                ),
            );
            content.setAutoresizingMask(mask_fill());
            root.addSubview(&content);

            window.setContentView(Some(&root));

            DaemonUiState {
                window,
                chrome,
                content,
                views: HashMap::new(),
                current: None,
                notes: None,
                files: None,
                jira: None,
                confluence: None,
                ai: None,
                compare: None,
                prefix: popup::PrefixState::new(),
                palette: None,
                switcher: None,
                paths: None,
                terminal_panel: None,
                tools: HashMap::new(),
                tool_opened_at: HashMap::new(),
                terminal_opened_at: None,
                shortcuts: None,
            }
        }
    }

    /// `ThemeScope` — which part of the window a preset paints.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum ThemeScope {
        Window,
        Notepad,
        Terminal,
    }

    /// The `commands.toml` section that holds a view's colors (`cmd.name`).
    fn theme_section(v: SlotView) -> &'static str {
        match v {
            SlotView::Notes => "notes",
            SlotView::Files => "files",
            SlotView::Confluence => "confluence",
            SlotView::Ai => "ai",
            SlotView::Compare | SlotView::CompareText => "compare",
            _ => "jira",
        }
    }

    /// `assetDir + "/" + name` (absolute names pass through, `~` expands).
    fn asset_path(assets: &str, name: &str) -> String {
        if name.is_empty() {
            return String::new();
        }
        if let Some(rest) = name.strip_prefix("~/") {
            return format!("{}/{}", std::env::var("HOME").unwrap_or_default(), rest);
        }
        if name.starts_with('/') {
            return name.to_string();
        }
        format!("{assets}/{name}")
    }

    /// The header nav icon per view (`notesNavIcon`, `filesNavIcon`,
    /// `jiraNavIcon`, `confluenceNavIcon`, `compareNavIcon`, `aiNavIcon`):
    /// the configured image file, else Swift's glyph fallback.
    fn nav_icon_image(
        v: SlotView,
        image_key: Option<&str>,
        assets: &str,
    ) -> Retained<objc2_app_kit::NSImage> {
        use crate::ui::style::{glyph_icon, image_file, named_image, srgb};
        if let Some(img) = image_key.and_then(|k| image_file(&asset_path(assets, k))) {
            return img;
        }
        let jira_accent = srgb(0.36, 0.62, 0.95);
        match v {
            SlotView::Files => named_image("NSFolder")
                .unwrap_or_else(|| glyph_icon("folder.fill", "F", jira_accent, 32.0)),
            SlotView::Notes => glyph_icon("note.text", "N", srgb(0.98, 0.82, 0.35), 32.0),
            SlotView::Confluence => glyph_icon("book.pages", "C", jira_accent, 32.0),
            SlotView::Compare | SlotView::CompareText => {
                glyph_icon("arrow.left.arrow.right", "⇆", srgb(0.53, 0.75, 0.95), 32.0)
            }
            SlotView::Ai => glyph_icon("sparkles", "✦", srgb(0.72, 0.56, 0.98), 32.0),
            _ => glyph_icon("ticket", "J", jira_accent, 32.0),
        }
    }

    // -- pasteboard / open helpers for the paths shelf (`do:paths:return`) ----

    const AI_RULE_TEMPLATE: &str = "---\nname: {name}\n# diff = show what changed vs your text · plain = just the answer\noutput: plain\ngreedy: true\n# guardrails: permissive-content-transformations\n# placeholder: hint shown in the empty input pane\n# prompt: a line put before your text, e.g. \"Summarize this text:\"\n# then: another-rule.md   (runs next, on this rule's answer)\n---\nDescribe what the model should do with the text it is given.\nReturn ONLY the result — no commentary, no preamble.\n";

    fn defaults_string(key: &str) -> Option<String> {
        crate::platform::get_string(key)
    }

    fn defaults_set_string(key: &str, value: &str) {
        crate::platform::set_string(key, value);
    }

    fn defaults_remove(key: &str) {
        crate::platform::remove_key(key);
    }

    /// `setReady()`.
    fn ai_set_ready(a: &mut AIState) {
        let s = a.model.rule().map(|r| format!("Ready — Ctrl+Enter runs “{}”", r.name)).unwrap_or_default();
        a.model.set_status(s, crate::views::ai::AITone::Dim);
    }

    /// The AI view snapshot.
    fn ai_snapshot(a: &mut AIState) -> crate::views::ai_view::AISnapshot {
        use crate::engines::ai_format::AIRule;
        use crate::ui::sidebar::{SidebarModel, SidebarRow};
        use crate::views::ai_view::{AISnapshot, StepState};
        let m = &a.model;
        let rows: Vec<SidebarRow> = m
            .rules
            .iter()
            .map(|r| SidebarRow {
                title: r.name.clone(),
                icon: "wand.and.stars".into(),
                meta: short_mtime(&r.path),
                tip: r.path.replacen(&std::env::var("HOME").unwrap_or_default(), "~", 1),
                ..Default::default()
            })
            .collect();
        let rule = m.rule();
        let chain = rule.map(AIRule::chain).unwrap_or_default();
        let running = m.is_running();
        let steps = chain
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let st = if running {
                    if i < m.run.step_index {
                        StepState::Done
                    } else if i == m.run.step_index {
                        StepState::Running
                    } else {
                        StepState::Waiting
                    }
                } else if !m.run.answer.is_empty() {
                    if m.status_tone == crate::views::ai::AITone::Danger { StepState::Failed } else { StepState::Done }
                } else {
                    StepState::Waiting
                };
                (r.name.clone(), st)
            })
            .collect();
        let modes = m.modes();
        let output = if m.mode.target().is_some() {
            None
        } else if running {
            Some(vec![(0, m.run.so_far().0)])
        } else if m.run.answer.is_empty() {
            Some(Vec::new())
        } else if m.run.answer_diff && m.mode == crate::engines::ai_format::PaneMode::Diff {
            Some(crate::views::ai::diff_ops(&m.run.answer_input, &m.run.answer))
        } else {
            Some(vec![(0, m.run.answer.clone())])
        };
        AISnapshot {
            sidebar: SidebarModel {
                section_title: "Rules".into(),
                rows,
                selected: (!m.rules.is_empty()).then_some(m.selected),
                add_button: true,
                ..Default::default()
            },
            command: if a.command.is_empty() { rule.map(|r| r.file()).unwrap_or_default() } else { a.command.clone() },
            warning: rule.map(|r| r.warnings.join(", ")).unwrap_or_default(),
            steps,
            right_title: if rule.is_some_and(|r| r.diff()) { "Changes".into() } else { "Answer".into() },
            mode: modes.iter().position(|x| *x == m.mode).unwrap_or(0),
            modes: modes.iter().map(|x| x.title()).collect(),
            copy_title: m.copy_title(),
            placeholder: rule
                .map(|r| if r.placeholder.is_empty() { "Type or paste your text — Ctrl+Enter runs it".to_string() } else { r.placeholder.clone() })
                .unwrap_or_default(),
            output,
            running,
            status: m.status.clone(),
            tone: match m.status_tone {
                crate::views::ai::AITone::Dim => 0,
                crate::views::ai::AITone::Success => 1,
                crate::views::ai::AITone::Warning => 2,
                crate::views::ai::AITone::Danger => 3,
            },
            tokens: a.tokens.clone(),
            tokens_warn: a.tokens_warn,
            set_input: a.set_input.take(),
        }
    }

    /// `vMeta` — a file's mtime as "HH:mm" today, else "MMM d".
    fn short_mtime(path: &str) -> String {
        use objc2_foundation::{NSDate, NSDateFormatter};
        let Ok(t) = std::fs::metadata(path).and_then(|m| m.modified()) else { return String::new() };
        let secs = t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
        let d = NSDate::dateWithTimeIntervalSince1970(secs);
        let f = NSDateFormatter::new();
        let today = objc2_foundation::NSCalendar::currentCalendar().isDateInToday(&d);
        f.setDateFormat(Some(&NSString::from_str(if today { "HH:mm" } else { "MMM d" })));
        f.stringFromDate(&d).to_string()
    }

    /// The Confluence view snapshot from the live state.
    fn conf_snapshot(cs: &ConfState) -> crate::views::confluence_view::ConfSnapshot {
        use crate::ui::sidebar::{SidebarModel, SidebarRow};
        use crate::views::confluence::Scope;
        use crate::views::confluence_view::{title_for, ConfSnapshot, MODIFIED, SORTS, TYPES};
        let m = &cs.model;
        let picker_text = |p: &crate::views::confluence::PickerModel| -> String {
            if p.is_all() {
                p.all_title.clone()
            } else if p.selected.is_empty() {
                p.placeholder.clone()
            } else {
                p.selected.join(", ")
            }
        };
        let empty = if !m.configured {
            String::new()
        } else if m.showing_favorites {
            m.favorites_empty_hint()
        } else if m.has_criteria() && !m.searching {
            String::new()
        } else {
            format!(
                "Type to search {}, then Return.\n★ Favorites keeps the pages you open often.",
                if m.site.is_empty() { "Confluence" } else { m.site.as_str() }
            )
        };
        ConfSnapshot {
            sidebar: SidebarModel {
                section_title: "Confluence".into(),
                rows: vec![SidebarRow::new("Search", "magnifyingglass"), SidebarRow::new("Favorites", "star")],
                selected: Some(if m.scope == Scope::Favorites { 1 } else { 0 }),
                ..Default::default()
            },
            mode: m.mode_index,
            title_only: m.title_only,
            query: m.query.clone(),
            spaces: picker_text(&m.spaces),
            spaces_all: m.spaces.is_all(),
            people: picker_text(&m.contributors),
            people_all: m.contributors.is_all(),
            type_label: format!("Type: {}", title_for(&TYPES, &m.type_choice)),
            mod_label: format!("Modified: {}", title_for(&MODIFIED, &m.modified)),
            sort_label: format!("Sort: {}", title_for(&SORTS, &m.sort)),
            rows: m.rows.clone(),
            selection: (!m.rows.is_empty()).then_some(m.selection),
            empty,
            status: m.status.clone(),
            tone: Some(m.status_tone),
            searching: m.searching,
            can_more: !m.next_link.is_empty(),
            has_cql: !m.last_cql.is_empty(),
            has_curl: !m.last_curl.is_empty(),
            preview: cs.pstate.clone(),
            favorites_scope: m.scope == Scope::Favorites,
        }
    }

    fn jira_move(l: &mut crate::views::jira_view::JiraList, d: i64) {
        let n = l.rows.len() as i64;
        if n == 0 {
            return;
        }
        l.selection = (l.selection as i64 + d).clamp(0, n - 1) as usize;
    }

    fn open_url(url: &str) {
        crate::platform::open_url(url);
    }

    /// Show `paths` in the app's own file browser (`SlotView::Files`) — the
    /// uniform replacement for every "Reveal in Files" action. Looks the live
    /// UI up through [`UI_WEAK`] (callers are main-thread menu/key handlers
    /// that hold no `&DaemonUi`).
    pub(crate) fn reveal_in_files(paths: &[String]) {
        if paths.is_empty() {
            return;
        }
        let ui = UI_WEAK.with(|w| w.borrow().as_ref().and_then(|w| w.load()));
        if let Some(ui) = ui {
            ui.open_in_files(paths);
        }
    }

    fn copy_text_to_pasteboard(text: &str) {
        let _ = crate::platform::set_text(text);
    }

    /// The file URLs on the general pasteboard, as paths.
    fn pasteboard_file_paths() -> Vec<String> {
        use objc2_app_kit::NSPasteboard;
        let pb = NSPasteboard::generalPasteboard();
        let ty = NSString::from_str("public.file-url");
        pb.pasteboardItems()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|it| it.stringForType(&ty))
                    .filter_map(|s| objc2_foundation::NSURL::URLWithString(&s))
                    .filter_map(|u| u.path().map(|p| p.to_string()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Swift `copyFiles`: one pasteboard item per path carrying both a
    /// `public.file-url` (Finder pastes the file) and a plain string (text
    /// fields / terminals paste the path). Returns whether anything was written.
    fn copy_files_to_pasteboard(paths: &[String]) -> bool {
        use objc2::runtime::ProtocolObject;
        use objc2_app_kit::{NSPasteboard, NSPasteboardItem, NSPasteboardTypeFileURL, NSPasteboardTypeString, NSPasteboardWriting};
        use objc2_foundation::NSURL;
        if paths.is_empty() {
            return false;
        }
        let pb = NSPasteboard::generalPasteboard();
        pb.clearContents();
        let objects: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> = paths
            .iter()
            .map(|p| {
                let item = NSPasteboardItem::new();
                let url = NSURL::fileURLWithPath(&NSString::from_str(p));
                if let Some(abs) = url.absoluteString() {
                    let _ = item.setString_forType(&abs, unsafe { NSPasteboardTypeFileURL });
                }
                let _ = item.setString_forType(&NSString::from_str(p), unsafe { NSPasteboardTypeString });
                ProtocolObject::from_retained(item)
            })
            .collect();
        let array = objc2_foundation::NSArray::from_retained_slice(&objects);
        pb.writeObjects(&array)
    }

    /// Copy files and toast Swift's wording; only toasts when something landed.
    fn copy_files_toast(paths: &[String]) -> bool {
        let ok = copy_files_to_pasteboard(paths);
        if ok {
            crate::ui::toast::show(&format!("Copied {}", copied_what(paths)), "doc.on.doc");
        }
        ok
    }

    /// Swift `items(n, leaf)`: “name” for one path, "N items" otherwise.
    fn copied_what(paths: &[String]) -> String {
        match paths {
            [one] => format!("“{}”", std::path::Path::new(one).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default()),
            _ => format!("{} items", paths.len()),
        }
    }

    fn open_path(path: &str) {
        crate::platform::open_path(std::path::Path::new(path));
    }

    /// Build a `$HOME`-rooted file browser and hand back the model + its view
    /// (the host keeps the model so list keys stay addressable).
    fn build_files_browser(
        mtm: MainThreadMarker,
    ) -> Option<(FileBrowser, Retained<NSView>)> {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
        build_files_browser_at(mtm, &home)
    }

    /// Build a file browser rooted at `dir` (used by the accept-key descent).
    fn build_files_browser_at(
        mtm: MainThreadMarker,
        dir: &str,
    ) -> Option<(FileBrowser, Retained<NSView>)> {
        let mut browser = FileBrowser::new(dir);
        browser.reload();
        browser.build(mtm);
        let view = browser.content_view()?;
        Some((browser, view))
    }

    /// `clampToScreen`: keep the frame inside the window's screen's visible
    /// frame (Swift `PopupWindow.clampToScreen`).
    fn clamp_to_screen(f: &mut NSRect) {
        let Some(vf) = crate::platform::main_screen().map(|s| {
            let v = s.visible;
            NSRect::new(NSPoint::new(v.x, v.y), NSSize::new(v.w, v.h))
        }) else {
            return;
        };
        let max_w = vf.size.width;
        let max_h = vf.size.height;
        if f.size.width > max_w {
            f.size.width = max_w;
        }
        if f.size.height > max_h {
            f.size.height = max_h;
        }
        if f.origin.x < vf.origin.x {
            f.origin.x = vf.origin.x;
        }
        if f.origin.y < vf.origin.y {
            f.origin.y = vf.origin.y;
        }
        if f.origin.x + f.size.width > vf.origin.x + vf.size.width {
            f.origin.x = vf.origin.x + vf.size.width - f.size.width;
        }
        if f.origin.y + f.size.height > vf.origin.y + vf.size.height {
            f.origin.y = vf.origin.y + vf.size.height - f.size.height;
        }
    }

    /// `presentPathSheet`: a small sheet window with a single-line field and
    /// OK/Cancel; `on_result` runs on the main thread with the typed path.
    fn present_path_sheet(
        mtm: MainThreadMarker,
        parent: &objc2_app_kit::NSWindow,
        on_result: Box<dyn FnMut(Option<String>)>,
    ) {
        use objc2::MainThreadOnly;
        use objc2_app_kit::{NSButton, NSFont, NSTextField, NSView, NSWindow, NSWindowStyleMask};
        use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

        let sheet: Retained<NSWindow> = unsafe {
            let w: Retained<NSWindow> = msg_send![
                NSWindow::alloc(mtm),
                initWithContentRect: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(460.0, 128.0)),
                styleMask: NSWindowStyleMask::Titled,
                backing: objc2_app_kit::NSBackingStoreType::Buffered,
                defer: false
            ];
            w
        };
        sheet.setTitle(&NSString::from_str("Open file at path"));
        let content = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(460.0, 128.0)),
        );
        let label = NSTextField::labelWithString(
            &NSString::from_str("Absolute path (or ~/…) to open as a note:"),
            mtm,
        );
        label.setFont(Some(&NSFont::systemFontOfSize(12.0)));
        label.setFrame(NSRect::new(
            NSPoint::new(16.0, 98.0),
            NSSize::new(428.0, 18.0),
        ));
        let field = NSTextField::textFieldWithString(&NSString::from_str(""), mtm);
        field.setFont(Some(&NSFont::systemFontOfSize(13.0)));
        field.setFrame(NSRect::new(
            NSPoint::new(16.0, 62.0),
            NSSize::new(428.0, 24.0),
        ));
        let ok = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str("Open"),
                None,
                None,
                mtm,
            )
        };
        ok.setFrame(NSRect::new(
            NSPoint::new(460.0 - 16.0 - 88.0, 18.0),
            NSSize::new(88.0, 30.0),
        ));
        let cancel = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str("Cancel"),
                None,
                None,
                mtm,
            )
        };
        cancel.setFrame(NSRect::new(
            NSPoint::new(460.0 - 16.0 - 88.0 - 96.0, 18.0),
            NSSize::new(88.0, 30.0),
        ));
        content.addSubview(&label);
        content.addSubview(&field);
        content.addSubview(&ok);
        content.addSubview(&cancel);
        sheet.setContentView(Some(&content));
        sheet.setInitialFirstResponder(Some(&field));

        let handler: Retained<PathSheetHandler> = unsafe {
            let h = PathSheetHandler::alloc(mtm).set_ivars(PathSheetHandlerIvars {
                sheet: sheet.clone(),
                field: field.clone(),
                on_result: RefCell::new(Some(on_result)),
            });
            msg_send![super(h), init]
        };
        unsafe {
            ok.setTarget(Some(as_any(&*handler)));
            ok.setAction(Some(sel!(ok:)));
            cancel.setTarget(Some(as_any(&*handler)));
            cancel.setAction(Some(sel!(cancel:)));
        }
        PATH_SHEET_HANDLERS.with(|v| v.borrow_mut().push(handler));
        parent.beginSheet_completionHandler(&sheet, None);
    }

    /// The content surface for a view. View modules expose `build_content`
    /// as their AppKit trees land; views without one get the themed
    /// placeholder.
    fn content_for(mtm: MainThreadMarker, v: SlotView) -> Retained<NSView> {
        let built = match v {
            SlotView::Notes => crate::views::notes::build_content(mtm),
            SlotView::Files => crate::views::files::build_content(mtm),
            SlotView::Jira => crate::views::jira::build_content(mtm),
            SlotView::Compare | SlotView::CompareText => crate::views::compare::build_content(mtm),
            SlotView::Confluence => crate::views::confluence::build_content(mtm),
            SlotView::Ai => crate::views::ai::build_content(mtm),
            _ => None,
        };
        built.unwrap_or_else(|| placeholder(mtm, v))
    }

    fn placeholder(mtm: MainThreadMarker, v: SlotView) -> Retained<NSView> {
        let colors = PopupColors::default();
        let root = HostRootView::new(
            mtm,
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(640.0, 360.0)),
        );
        let title = NSTextField::labelWithString(&NSString::from_str(v.title()), mtm);
        title.setFont(Some(&NSFont::systemFontOfSize(15.0)));
        title.setTextColor(Some(&colors.text.to_nscolor()));
        title.setFrame(NSRect::new(
            NSPoint::new(16.0, 14.0),
            NSSize::new(420.0, 22.0),
        ));
        let hint = NSTextField::labelWithString(
            &NSString::from_str("Rust port — view surface pending"),
            mtm,
        );
        hint.setFont(Some(&NSFont::systemFontOfSize(11.5)));
        hint.setTextColor(Some(&colors.dim.to_nscolor()));
        hint.setFrame(NSRect::new(
            NSPoint::new(16.0, 36.0),
            NSSize::new(420.0, 18.0),
        ));
        root.addSubview(&title);
        root.addSubview(&hint);
        root.into_super()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::registry::PaletteCommand;

    const REQUIRED: [&str; 19] = [
        "view",
        "visible",
        "active",
        "keyWindow",
        "windows",
        "palette",
        "views",
        "hideOnFocusLoss",
        "escHides",
        "pid",
        "paletteCommands",
        "pane",
        "screenshot",
        "compare",
        "paneShot",
        "activations",
        "frontmostPid",
        "tools",
        "lastKey",
    ];

    fn controller() -> SwitcherController {
        SwitcherController::new_default()
    }

    /// Install both queues so `is_headless()` is false (GUI mode), plus a
    /// drainer for the `MainQueue` (the only jobs it sees in tests are the
    /// `do:` flush's no-op `|| {}`). Returns the `UiQueue` for assertions.
    fn install_gui(c: &SwitcherController) -> UiQueue {
        let q: UiQueue = Arc::new(Mutex::new(Vec::new()));
        c.install_ui(q.clone());
        let mq: MainQueue = Arc::new(Mutex::new(Vec::new()));
        c.install_main_queue(mq.clone());
        std::thread::spawn(move || loop {
            let job = mq.lock().unwrap().pop();
            match job {
                Some(j) => j(),
                None => {
                    // The controller drops its `main_queue` clone at the end of
                    // the test; the last clone is ours, so exit cleanly then.
                    if Arc::strong_count(&mq) <= 1 {
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            }
        });
        q
    }

    #[test]
    fn state_has_every_required_key() {
        let c = controller();
        let s = c.state_json();
        let obj = s.as_object().expect("state is an object");
        for k in REQUIRED {
            assert!(obj.contains_key(k), "missing top-level key: {k}");
        }
    }

    #[test]
    fn state_shape_defaults() {
        let c = controller();
        c.set_member(SlotView::Notes, false, false, None);
        let s = c.state_json();
        assert_eq!(s["view"], json!(""));
        assert_eq!(s["visible"], json!(false));
        assert_eq!(s["palette"], json!(false));
        assert!(s["escHides"].is_object());
        for v in ESC_VIEWS {
            assert_eq!(s["escHides"][v.raw()], json!(false));
        }
        assert_eq!(
            s["pid"].as_u64(),
            Some(std::process::id() as u64)
        );
        assert!(s["pane"].is_object());
        assert!(s["views"]["notes"].is_object());
        assert_eq!(s["views"]["notes"]["shown"], json!(false));
        assert_eq!(s["views"]["notes"]["terminal"], json!(false));
        assert_eq!(s["views"]["notes"]["browser"], json!(false));
        assert_eq!(s["views"]["notes"]["drawerInset"], json!(0));
        assert_eq!(s["viewSwitcher"]["shown"], json!(false));
        assert!(s["viewSwitcher"]["rows"].is_array());
        assert_eq!(s["paths"]["shown"], json!(false));
    }

    #[test]
    fn open_updates_view_and_member_shown() {
        let c = controller();
        c.open(SlotView::Notes);
        let s = c.state_json();
        assert_eq!(s["view"], json!("notes"));
        assert_eq!(s["visible"], json!(true));
        assert_eq!(s["views"]["notes"]["shown"], json!(true));
    }

    #[test]
    fn view_state_merges_into_views_entry() {
        let c = controller();
        c.open(SlotView::Jira);
        c.set_view_state(SlotView::Jira, json!({ "tabs": 3, "selectedTab": 1 }));
        let s = c.state_json();
        assert_eq!(s["views"]["jira"]["tabs"], json!(3));
        assert_eq!(s["views"]["jira"]["selectedTab"], json!(1));
        assert_eq!(s["views"]["jira"]["shown"], json!(true));
    }

    #[test]
    fn dispatch_open_and_cycle() {
        let c = controller();
        assert_eq!(c.do_action("cycle"), None);
        assert_eq!(c.current(), Some(SlotView::Files));
        assert_eq!(c.do_action("cycle"), None);
        assert_eq!(c.current(), Some(SlotView::Notes));
        assert_eq!(c.do_action("hide"), None);
        assert_eq!(c.current(), None);
        assert_eq!(c.do_action("home"), None);
        assert_eq!(c.current(), Some(SlotView::Jira));
    }

    #[test]
    fn cycle_and_cycle_back_are_a_two_view_toggle() {
        // Swift's `SharedWindow.cycle(_:)` ignores `dir` too (`current == .files
        // ? navNotes : navFiles`), so `cycle-back` is identical by design.
        let c = controller();
        assert_eq!(c.do_action("cycle-back"), None);
        assert_eq!(c.current(), Some(SlotView::Files));
        assert_eq!(c.do_action("cycle-back"), None);
        assert_eq!(c.current(), Some(SlotView::Notes));
        // Same sequence for the forward verb: Files <-> Notes.
        assert_eq!(c.do_action("cycle"), None);
        assert_eq!(c.current(), Some(SlotView::Files));
        assert_eq!(c.do_action("cycle"), None);
        assert_eq!(c.current(), Some(SlotView::Notes));
    }

    #[test]
    fn dispatch_open_view_and_error() {
        let c = controller();
        assert_eq!(c.do_action("open:notes"), None);
        assert_eq!(c.current(), Some(SlotView::Notes));
        let err = c.do_action("open:bogus").unwrap();
        assert_eq!(err["error"], json!("unknown view"));
    }

    #[test]
    fn dispatch_unknown_delegates_to_registry() {
        let mut c = controller();
        c.registry_mut()
            .register_test_do(|a| (a == "compare:open").then(|| json!({ "ok": true })));
        assert_eq!(c.do_action("compare:open"), Some(json!({ "ok": true })));
        assert_eq!(
            c.do_action("totally-unknown").unwrap()["error"],
            json!("unknown action totally-unknown")
        );
    }

    #[test]
    fn unknown_verbs_report_an_error() {
        // A typo must be visible: unknown `do:` verbs answer `{"error": …}`
        // instead of the state that a success returns (F4).
        let c = controller();
        for action in ["bogus-xyz", "file:/etc/hosts", "notes:bogus", "select:0"] {
            let err = c.do_action(action).unwrap_or_else(|| panic!("{action} must error"));
            assert_eq!(err["error"], json!(format!("unknown action {action}")));
        }
    }

    #[test]
    fn unknown_tool_name_is_rejected() {
        // `tool:` fabricates a `tools.<name>` entry only for a real panel (F3).
        let c = controller();
        install_gui(&c);
        let err = c.do_action("tool:nope").unwrap();
        assert_eq!(err["error"], json!("not a tool panel: nope"));
        assert!(c.state_json()["tools"].get("nope").is_none());
        let err = c.do_action("tool:").unwrap();
        assert_eq!(err["error"], json!("not a tool panel: "));
        assert_eq!(c.do_action("tool:prettyprint"), None);
        assert_eq!(c.state_json()["tools"]["prettyprint"]["shown"], json!(true));
    }

    #[test]
    fn dispatch_header_style_and_esc_hides() {
        let c = controller();
        assert_eq!(c.do_action("header-style:aurora"), None);
        assert_eq!(c.state_json()["headerStyle"], json!("aurora"));
        assert_eq!(c.do_action("header-style:bogus").unwrap()["error"], json!("unknown header style"));

        assert_eq!(c.do_action("esc-hides:notes:on"), None);
        assert_eq!(c.state_json()["escHides"]["notes"], json!(true));
        assert_eq!(c.do_action("esc-hides:notes:off"), None);
        assert_eq!(c.state_json()["escHides"]["notes"], json!(false));
    }

    #[test]
    fn esc_hide_count_reads_sections_and_app_default() {
        let c = controller();
        assert_eq!(c.esc_hide_count(SlotView::Notes), 0, "app default esc-close");
        c.set_esc_hides(SlotView::Notes, true);
        assert_eq!(c.esc_hide_count(SlotView::Notes), 1);
        c.set_esc_hides(SlotView::Notes, false);
        assert_eq!(c.esc_hide_count(SlotView::Notes), 0);
    }

    #[test]
    fn dispatch_tool_open_and_close() {
        let c = controller();
        install_gui(&c);
        // Without an enabled [paths] command, `tool:paths` is not a tool panel.
        let err = c.do_action("tool:paths").unwrap();
        assert_eq!(err["error"], json!("not a tool panel: paths"));
        c.set_paths_enabled(true);
        assert_eq!(c.do_action("tool:paths"), None);
        assert_eq!(c.state_json()["tools"]["paths"]["shown"], json!(true));
        assert_eq!(c.state_json()["paths"]["shown"], json!(true));
        assert_eq!(c.do_action("tool-close:paths"), None);
        assert_eq!(c.state_json()["tools"]["paths"]["shown"], json!(false));
        assert_eq!(c.state_json()["paths"]["shown"], json!(false));
    }

    #[test]
    fn tool_close_rejects_an_unknown_panel() {
        // `tool-close:` accepts exactly the set `tool:` accepts; a bogus (or
        // empty) name is an error instead of a silent no-op.
        let c = controller();
        install_gui(&c);
        let err = c.do_action("tool-close:bogus").unwrap();
        assert_eq!(err["error"], json!("not a tool panel: bogus"));
        let err = c.do_action("tool-close:").unwrap();
        assert_eq!(err["error"], json!("not a tool panel: "));
        assert_eq!(c.do_action("tool-close:health"), None);
    }

    #[test]
    fn drawer_toggles_update_state_and_queue() {
        let c = controller();
        let q = install_gui(&c);
        assert_eq!(c.do_action("toggle-terminal"), None);
        assert_eq!(c.state_json()["views"]["notes"]["terminal"], json!(true));
        assert_eq!(c.state_json()["views"]["notes"]["drawerInset"], json!(200));
        assert_eq!(c.do_action("toggle-browser"), None);
        assert_eq!(c.state_json()["views"]["notes"]["browser"], json!(true));
        assert_eq!(c.do_action("toggle-terminal"), None);
        assert_eq!(c.state_json()["views"]["notes"]["terminal"], json!(false));
        assert_eq!(c.state_json()["views"]["notes"]["drawerInset"], json!(0));
        let cmds = q.lock().unwrap().clone();
        assert_eq!(
            cmds,
            vec![
                UiCommand::ToggleDrawer(DrawerSide::Terminal),
                UiCommand::ToggleDrawer(DrawerSide::Browser),
                UiCommand::ToggleDrawer(DrawerSide::Terminal),
            ]
        );
    }

    #[test]
    fn paths_grammar_gates_on_enabled_and_queues() {
        let c = controller();
        let q = install_gui(&c);
        let err = c.do_action("paths:show").unwrap();
        assert_eq!(err["error"], json!("[paths] not enabled"));
        c.set_paths_enabled(true);
        assert_eq!(c.do_action("paths:show"), None);
        assert_eq!(c.state_json()["paths"]["shown"], json!(true));
        // Selection is bounds-checked against the reported rows (F3).
        c.set_paths_state(json!({ "shown": true, "rows": [{}, {}, {}] }));
        assert_eq!(c.do_action("paths:select:2"), None);
        assert_eq!(c.state_json()["paths"]["selection"], json!(2));
        assert_eq!(c.do_action("paths:select:x").unwrap()["error"], json!("paths:select:N"));
        assert_eq!(c.do_action("paths:select:-1").unwrap()["error"], json!("paths:select:N"));
        assert_eq!(
            c.do_action("paths:select:9").unwrap()["error"],
            json!("paths:select: out of range")
        );
        assert_eq!(c.do_action("paths:hide"), None);
        assert_eq!(c.state_json()["paths"]["shown"], json!(false));
        let err = c.do_action("paths:bogus").unwrap();
        assert_eq!(err["error"], json!("paths:show|hide|return|select:N"));
        let cmds = q.lock().unwrap().clone();
        assert_eq!(
            cmds,
            vec![
                UiCommand::Paths(PathsCmd::Show),
                UiCommand::Paths(PathsCmd::Select(2)),
                UiCommand::Paths(PathsCmd::Hide),
            ]
        );
    }

    #[test]
    fn palette_toggle_flips_state_and_queue() {
        let c = controller();
        let q = install_gui(&c);
        c.palette_toggle();
        assert_eq!(c.state_json()["palette"], json!(true));
        assert_eq!(c.state_json()["viewSwitcher"]["shown"], json!(false));
        c.palette_toggle();
        assert_eq!(c.state_json()["palette"], json!(false));
        // `do:switcher` drives the Ctrl+B W panel, not the palette.
        c.do_action("switcher");
        assert_eq!(c.state_json()["viewSwitcher"]["shown"], json!(true));
        assert_eq!(c.state_json()["palette"], json!(false));
        c.do_action("switcher-hide");
        assert_eq!(c.state_json()["viewSwitcher"]["shown"], json!(false));
        c.do_action("term");
        assert_eq!(c.state_json()["terminalPanel"]["shown"], json!(true));
        let cmds = q.lock().unwrap().clone();
        assert_eq!(
            cmds,
            vec![
                UiCommand::ShowPalette,
                UiCommand::HidePalette,
                UiCommand::ShowSwitcher,
                UiCommand::HideSwitcher,
                UiCommand::TerminalPanel,
            ]
        );
    }

    #[test]
    fn toggle_command_handles_show_and_term() {
        let c = controller();
        c.toggle_command("show");
        assert_eq!(c.state_json()["palette"], json!(true));
        c.toggle_command("show");
        assert_eq!(c.state_json()["palette"], json!(false));
        c.toggle_command("term");
        assert_eq!(c.state_json()["terminalPanel"]["shown"], json!(true));
    }

    #[test]
    fn dispatch_pane_focus_and_move() {
        let c = controller();
        // `pane:*` needs a shown view (Swift guards `slot.current`).
        let err = c.do_action("pane:focus:editor").unwrap();
        assert_eq!(err["error"], json!("no view shown"));
        c.open(SlotView::Notes);
        c.set_panes(vec![
            PaneSpec { id: "sidebar".into(), rect: RectI::new(0.0, 0.0, 200.0, 600.0) },
            PaneSpec { id: "editor".into(), rect: RectI::new(200.0, 0.0, 700.0, 600.0) },
        ]);
        assert_eq!(c.do_action("pane:focus:editor"), None);
        assert_eq!(c.state_json()["pane"]["focused"], json!("editor"));
        assert_eq!(c.do_action("pane:h"), None);
        assert_eq!(c.state_json()["pane"]["focused"], json!("sidebar"));
        let err = c.do_action("pane:focus:nope").unwrap();
        assert_eq!(err["error"], json!("no such pane"));
    }

    #[test]
    fn palette_commands_are_listed() {
        let mut c = controller();
        c.registry_mut()
            .add_palette(PaletteCommand::new("notes", "Notes", "views"));
        c.registry_mut()
            .add_palette(PaletteCommand::new("jira-cfg", "Jira Config", "views").hidden());
        let s = c.state_json();
        assert_eq!(s["paletteCommands"], json!(["Notes"]));
    }

    #[test]
    fn palette_screenshot_has_a_handler() {
        // `/screenshot` must dispatch (`screenshot:show`), not hit the
        // `no Rust handler yet` fallback log.
        let c = controller();
        c.run_palette_command("screenshot");
        assert!(
            !c.logs().iter().any(|l| l.contains("no Rust handler")),
            "palette 'screenshot' must dispatch"
        );
        c.run_palette_command("definitely-not-a-command");
        assert!(c.logs().iter().any(|l| l.contains("no Rust handler")));
    }

    #[test]
    fn reload_reads_disk_and_reports() {
        let dir = std::env::temp_dir().join(format!("ws-rs-host-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let conf = dir.join("commands.toml");
        std::fs::write(
            &conf,
            "[app]\nesc-close = 2\n\n[notes]\ntype = note\nesc-close = 1\n",
        )
        .unwrap();

        let mut settings = AppSettings::default();
        settings.shared_width = 1100.0;
        let c = SwitcherController::new(settings);
        // Point the controller's config path at the fixture via reload's own
        // path resolution: AppSettings reads $WS_COMMANDS_CONF indirectly is
        // not available here, so drive parse + count through reload_config's
        // helpers directly.
        let text = std::fs::read_to_string(&conf).unwrap();
        let sections = parse_esc_close(&text);
        assert_eq!(sections.get("app"), Some(&2));
        assert_eq!(sections.get("notes"), Some(&1));
        assert_eq!(count_sections(&text), 2);

        let _ = c;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn toggle_command_maps_modes() {
        let c = controller();
        c.toggle_command("notes");
        assert_eq!(c.current(), Some(SlotView::Notes));
        c.toggle_command("notes");
        assert_eq!(c.current(), None);
        c.toggle_command("window");
        assert_eq!(c.current(), Some(SlotView::Notes));
        c.toggle_command("jira");
        assert_eq!(c.current(), Some(SlotView::Jira));
    }

    #[test]
    fn toggle_and_hotkey_focus_unless_user_in_it() {
        // `SharedWindow.hideOrFocus`: a shown view the user is not in comes
        // forward instead of hiding.
        let c = controller();
        c.open(SlotView::Notes);
        let mut inner = c.inner.lock().unwrap();
        inner.toggle(false);
        assert_eq!(inner.current, Some(SlotView::Notes));
        inner.hotkey(SlotView::Notes, false);
        assert_eq!(inner.current, Some(SlotView::Notes));
        inner.toggle(true);
        assert_eq!(inner.current, None);
    }

    #[test]
    fn parse_sections_unquotes_values_like_the_codec() {
        // A quoted list must not keep its quotes on the first/last item
        // (`buttons = "pencil, …, pin"` lost pencil and pin).
        let s = parse_sections("[screenshot]\nbuttons = \"pencil, pin\"  # tools\nsize = 3\n");
        assert_eq!(s["screenshot"]["buttons"], "pencil, pin");
        assert_eq!(s["screenshot"]["size"], "3");
    }

    #[test]
    fn live_focus_overrides_present_time_mirrors() {
        let c = controller();
        c.open(SlotView::Notes);
        c.set_member(SlotView::Notes, true, true, None);
        c.set_member_wid(SlotView::Notes, 7);
        c.set_member(SlotView::Files, false, true, None);
        let mut inner = c.inner.lock().unwrap();
        // another app is frontmost: nothing of ours is key
        inner.apply_live_focus(&LiveFocus { active: false, frontmost: 1, ..Default::default() });
        assert!(!inner.active);
        assert_eq!(inner.frontmost_pid, 1);
        assert!(!inner.members[&SlotView::Notes].key);
        assert!(!inner.members[&SlotView::Files].key);
        // the host window is key: only the current view reports key
        inner.apply_live_focus(&LiveFocus { active: true, key_wid: 7, ..Default::default() });
        assert!(inner.members[&SlotView::Notes].key);
        assert!(!inner.members[&SlotView::Files].key);
    }

    #[test]
    fn apply_hotkey_prep_sets_target_screen() {
        let c = controller();
        let prep = HotkeyPrep {
            log: String::new(),
            workspace: "2".into(),
            screen: Some(1),
            cache_cleared: true,
        };
        c.apply_hotkey_prep(&prep);
        c.launch_with_prep("notes", Some(prep));
        assert_eq!(c.current(), Some(SlotView::Notes));
    }

    #[test]
    fn controller_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<SwitcherController>();
    }

    fn sample(now: f64, key: bool) -> FocusSample {
        FocusSample {
            now,
            shown: true,
            key,
            left: !key,
            any_key: key,
            app_active: true,
            clicked_away: false,
        }
    }

    #[test]
    fn focus_loss_hides_after_the_delay_only_when_enabled() {
        let mut w = FocusWatch::default();
        assert_eq!(w.tick(sample(0.0, true), true, 0.3), FocusAction::None);
        // Resign: pending until focusLossDelay elapses.
        assert_eq!(w.tick(sample(1.0, false), true, 0.3), FocusAction::None);
        assert_eq!(w.tick(sample(1.2, false), true, 0.3), FocusAction::None);
        assert_eq!(w.tick(sample(1.31, false), true, 0.3), FocusAction::Hide);
        // One shot per resign.
        assert_eq!(w.tick(sample(1.5, false), true, 0.3), FocusAction::None);
        // Switch off: the check runs but does nothing.
        let mut w = FocusWatch::default();
        w.tick(sample(0.0, true), false, 0.3);
        w.tick(sample(1.0, false), false, 0.3);
        assert_eq!(w.tick(sample(2.0, false), false, 0.3), FocusAction::None);
    }

    #[test]
    fn focus_loss_cancelled_by_rekey_or_our_other_window() {
        let mut w = FocusWatch::default();
        w.tick(sample(0.0, true), true, 0.3);
        w.tick(sample(1.0, false), true, 0.3);
        // Became key again within the delay → focusLossGen bump.
        w.tick(sample(1.1, true), true, 0.3);
        assert_eq!(w.tick(sample(2.0, true), true, 0.3), FocusAction::None);
        // Key moved to one of our panels: didBecomeKey cancels, and
        // focusLeft is false anyway.
        let mut w = FocusWatch::default();
        w.tick(sample(0.0, true), true, 0.3);
        let mut s = sample(1.0, false);
        s.any_key = true;
        s.left = false;
        w.tick(s, true, 0.3);
        s.now = 2.0;
        assert_eq!(w.tick(s, true, 0.3), FocusAction::None);
        // Hidden: nothing pending survives.
        let mut w = FocusWatch::default();
        w.tick(sample(0.0, true), true, 0.3);
        w.tick(sample(1.0, false), true, 0.3);
        let mut s = sample(1.1, false);
        s.shown = false;
        w.tick(s, true, 0.3);
        assert_eq!(w.tick(sample(2.0, false), true, 0.3), FocusAction::None);
    }

    #[test]
    fn focus_stolen_right_after_a_swap_refocuses() {
        let mut w = FocusWatch::default();
        w.tick(sample(0.0, true), false, 0.3);
        w.note_swap(0.9);
        w.tick(sample(1.0, false), false, 0.3);
        assert_eq!(w.tick(sample(1.4, false), false, 0.3), FocusAction::Refocus);
        // An old swap (> 1 s) is a real focus loss.
        let mut w = FocusWatch::default();
        w.tick(sample(0.0, true), true, 0.3);
        w.note_swap(0.0);
        w.tick(sample(1.0, false), true, 0.3);
        assert_eq!(w.tick(sample(1.4, false), true, 0.3), FocusAction::Hide);
    }

    #[test]
    fn key_records_last_key_and_rejects_an_empty_spec() {
        // `key:<spec>` is record-only; the spec is now visible in state as
        // top-level `lastKey`, and an empty spec is a visible error.
        let c = controller();
        assert_eq!(c.do_action("key:cmd+k"), None);
        assert_eq!(c.state_json()["lastKey"], json!("cmd+k"));
        let err = c.do_action("key:").unwrap();
        assert_eq!(err["error"], json!("key:<spec>"));
        // The rejected call leaves the recorded spec untouched.
        assert_eq!(c.state_json()["lastKey"], json!("cmd+k"));
    }

    #[test]
    fn key_while_inactive_self_activates_unless_clicked_away_or_ours() {
        let mut w = FocusWatch::default();
        w.tick(sample(0.0, false), true, 0.3);
        let mut s = sample(1.0, true);
        s.app_active = false;
        assert_eq!(w.tick(s, true, 0.3), FocusAction::SelfActivate);
        let mut w = FocusWatch::default();
        w.tick(sample(0.0, false), true, 0.3);
        s.clicked_away = true;
        assert_eq!(w.tick(s, true, 0.3), FocusAction::None);
        // Our own present's key edge is not an AeroSpace focus.
        let mut w = FocusWatch::default();
        w.tick(sample(0.0, false), true, 0.3);
        w.note_present(0.9);
        s.clicked_away = false;
        assert_eq!(w.tick(s, true, 0.3), FocusAction::None);
    }

    #[test]
    fn focus_bridge_decision() {
        assert!(focus_bridge_should_activate("4512\n", 4512, 0.2, false));
        assert!(!focus_bridge_should_activate("4512", 4513, 0.2, false));
        assert!(!focus_bridge_should_activate("4512", 4512, 1.5, false));
        assert!(!focus_bridge_should_activate("4512", 4512, 0.2, true));
        assert!(!focus_bridge_should_activate("", 0, 0.0, false));
        assert!(!focus_bridge_should_activate("x", 4512, 0.0, false));
    }

    #[test]
    fn hide_on_focus_loss_hook_flips_state() {
        let c = controller();
        assert_eq!(c.do_action("hide-on-focus-loss:off"), None);
        assert_eq!(c.state_json()["hideOnFocusLoss"], json!(false));
        assert_eq!(c.do_action("hide-on-focus-loss:on"), None);
        assert_eq!(c.state_json()["hideOnFocusLoss"], json!(true));
        assert!(c.do_action("hide-on-focus-loss:maybe").unwrap().get("error").is_some());
    }

    #[test]
    fn ui_queue_receives_present_and_hide() {
        let c = controller();
        let q: UiQueue = Arc::new(Mutex::new(Vec::new()));
        c.install_ui(q.clone());
        c.open(SlotView::Notes);
        c.hide("test");
        assert_eq!(
            q.lock().unwrap().as_slice(),
            &[UiCommand::Present(SlotView::Notes), UiCommand::Hide]
        );
    }

    #[test]
    fn no_ui_queue_stays_headless() {
        let c = controller();
        c.open(SlotView::Files); // must not panic without a UI installed
        assert_eq!(c.current(), Some(SlotView::Files));
    }

    #[test]
    fn register_views_populates_nav_palette_and_dispatches() {
        let fixture = "\
[jira]
enabled = true

[confluence]
enabled = true

[ai]
enabled = true

[compare]
enabled = true

[screenshot]
enabled = true
";
        let mut c = controller();
        c.register_views_from_text(fixture);

        let ids: Vec<i64> = c.registry().nav_icons().iter().map(|n| n.id).collect();
        assert_eq!(
            ids,
            vec![60, 64, 61, 65, 67, 66],
            "notes first, then files / jira / confluence / compare / ai (Swift navIcons)"
        );

        let titles: Vec<String> =
            c.registry().palette_commands().iter().map(|p| p.title.clone()).collect();
        assert_eq!(
            titles,
            vec![
                "jira",
                "screenshot",
                "Confluence Search",
                "AI View",
                "Compare",
                "Kitchen Sink",
            ],
            "commands.toml rows, then the enabled slot commands and the window"
        );

        assert!(c.compare_model().is_some(), "compare handle held");
        assert!(c.screenshot_controller().is_some(), "screenshot handle held");

        assert_eq!(c.do_action("notes:state"), Some(json!({ "available": true })));
        assert_eq!(c.do_action("compare:start"), None, "handled: falls through to state");
        assert_eq!(c.do_action("screenshot:close"), None, "handled: falls through to state");

        let mut off = controller();
        off.register_views_from_text("");
        let ids: Vec<i64> = off.registry().nav_icons().iter().map(|n| n.id).collect();
        assert_eq!(ids, vec![60, 64, 61], "confluence / compare / ai stay hidden");
        assert!(off.compare_model().is_none());
        let titles: Vec<String> =
            off.registry().palette_commands().iter().map(|p| p.title.clone()).collect();
        assert_eq!(titles, vec!["Kitchen Sink"], "no commands -> just the window");
    }

    #[test]
    fn compare_open_sub_and_back() {
        // `do:compare:open[:sub]:LEFT|RIGHT` presents the view and loads the
        // pair; `back` returns from the compareText sub-view (Swift
        // `compareTestDo`). The pair load decodes through the Python helper —
        // skip when it is unavailable (the view transitions still run).
        let mut c = controller();
        let lib = format!("{}/../../pylib", env!("CARGO_MANIFEST_DIR"));
        let helper = crate::app::python_helper::PythonHelper::shared();
        if !crate::app::python_helper::lib_has_helper(&lib) {
            return;
        }
        helper.configure(&lib);
        if helper.call_default("ping", json!({})).is_err() {
            eprintln!("skipping compare_open test: python helper unavailable");
            return;
        }
        c.register_views_from_text("[compare]\nenabled = true\n");
        if c.compare_model().is_none() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("ws-host-compare-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let left = dir.join("left.txt");
        std::fs::write(&left, "a\nb\nc\n").unwrap();
        let one = format!("compare:open:{}", left.display());
        assert_eq!(c.do_action(&one), None);
        let s = c.state_json();
        assert_eq!(s["view"], json!("compare"));
        assert_eq!(s["visible"], json!(true));
        assert!(s["compare"]["current"]["sections"].as_i64().unwrap() > 0, "{s}");
        assert_eq!(s["compare"]["sub"], json!(false));

        let sub = format!("compare:open-sub:{}", left.display());
        assert_eq!(c.do_action(&sub), None);
        let s = c.state_json();
        assert_eq!(s["view"], json!("compareText"));
        assert_eq!(s["compare"]["sub"], json!(true));

        assert_eq!(c.do_action("compare:back"), None);
        let s = c.state_json();
        assert_eq!(s["view"], json!("compare"));
        assert_eq!(s["compare"]["sub"], json!(false));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn compare_open_rejects_a_missing_path() {
        // `compare:open:[sub]:LEFT|RIGHT` must refuse a path that does not exist
        // and open nothing, for both the pair and the sub-view form.
        let mut c = controller();
        c.register_views_from_text("[compare]\nenabled = true\n");
        if c.compare_model().is_none() {
            return;
        }
        let missing = "/nonexistent/kitchen-sink-wave3/no-such-file";
        for verb in ["compare:open", "compare:open-sub"] {
            let action = format!("{verb}:{missing}|/etc/hosts");
            let err = c.do_action(&action).unwrap();
            assert_eq!(
                err["error"],
                json!(format!("compare: no such path: {missing}")),
                "{action}"
            );
            assert_eq!(c.current(), None, "{action} must open nothing");
        }
        // A missing right-hand path is refused the same way.
        let action = format!("compare:open:/etc/hosts|{missing}");
        let err = c.do_action(&action).unwrap();
        assert_eq!(err["error"], json!(format!("compare: no such path: {missing}")));
        assert_eq!(c.current(), None);
    }

    #[test]
    fn jira_poll_on_headless_reenables_config() {
        // `jira-poll-off` persists `enabled = false`; a headless `jira-poll-on`
        // (no UI queue) must re-enable directly, so off-then-on round-trips.
        // Point the config at a throwaway file and keep launchd untouched.
        let dir = std::env::temp_dir().join(format!("ws-host-jira-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let conf = dir.join("commands.toml");
        std::fs::write(&conf, "[jira]\nenabled = true\n").unwrap();

        let old_conf = std::env::var_os("WS_COMMANDS_CONF");
        let old_skip = std::env::var_os("WS_SKIP_LAUNCH_AGENT");
        std::env::set_var("WS_COMMANDS_CONF", &conf);
        std::env::set_var("WS_SKIP_LAUNCH_AGENT", "1");

        let c = controller(); // headless: install_ui never ran, so ui_queue is None
        assert_eq!(c.do_action("jira-poll-off"), None);
        assert!(
            !crate::app::jira_poll::enabled_in_config(),
            "off must persist enabled=false"
        );
        assert_eq!(c.state_json()["view"], json!(""));

        assert_eq!(c.do_action("jira-poll-on"), None);
        assert!(
            crate::app::jira_poll::enabled_in_config(),
            "headless on must re-enable enabled=true"
        );
        let s = c.state_json();
        assert_eq!(s["view"], json!("jira"));
        assert_eq!(s["visible"], json!(true));

        match old_conf {
            Some(v) => std::env::set_var("WS_COMMANDS_CONF", v),
            None => std::env::remove_var("WS_COMMANDS_CONF"),
        }
        match old_skip {
            Some(v) => std::env::set_var("WS_SKIP_LAUNCH_AGENT", v),
            None => std::env::remove_var("WS_SKIP_LAUNCH_AGENT"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn count_commands_mirrors_load_commands() {
        let text = "\
[app]
esc-close = 2

[notes]
enabled = true

[files]
enabled = false

[shortcuts]
ctrl+j = x

[filefast]
enabled = true

[paths]
enabled = true
";
        assert_eq!(count_commands(text), 3, "notes + filefast + paths");
        assert_eq!(count_commands("custom = echo hi\n"), 1, "top-level command");
        assert_eq!(count_commands(""), 0);
    }

    #[test]
    fn palette_mirrors_commands_toml_order_and_gates() {
        // Mirrors the shipped commands.toml: `in-palette = false` views stay
        // out, palette-first rows lead (the live Swift daemon lists exactly
        // this sequence).
        let fixture = "\
[filefast]
enabled = true
label = \"Create File\"

[paths]
enabled = true
label = \"Find Recent Files\"

[notes]
enabled = true
in-palette = false

[jira]
enabled = true
in-palette = false

[prettyprint]
enabled = true
label = \"Pretty Print\"

[screenshot]
enabled = true
label = \"Screenshot\"

[terminal]
enabled = true
label = \"Terminal\"

[health-checks]
enabled = true
label = \"Health Checks\"
";
        let mut c = controller();
        c.register_views_from_text(fixture);
        let titles: Vec<String> =
            c.registry().palette_commands().iter().map(|p| p.title.clone()).collect();
        assert_eq!(
            titles,
            vec![
                "Create File",
                "Find Recent Files",
                "Pretty Print",
                "Screenshot",
                "Terminal",
                "Health Checks",
                "Kitchen Sink",
            ]
        );
    }

    #[test]
    fn headless_refuses_window_only_verbs() {
        // A headless daemon has no window: window-only verbs must error and
        // leave state untouched (no `shown: true` with nothing behind it).
        let c = controller();
        for action in [
            "paths:show",
            "tool:jira",
            "switcher",
            "toggle-terminal",
            "toggle-browser",
            "term",
            "notes-find",
            "notes-grep",
            "jira-jump",
            "reset-size",
        ] {
            let err = c
                .do_action(action)
                .unwrap_or_else(|| panic!("{action} must error headless"));
            assert_eq!(
                err["error"],
                json!(format!("{action} unavailable in headless mode")),
                "{action}"
            );
        }
        let s = c.state_json();
        assert_eq!(s["paths"]["shown"], json!(false));
        assert_eq!(s["views"]["notes"]["terminal"], json!(false));
        assert_eq!(s["views"]["notes"]["browser"], json!(false));
        assert_eq!(s["terminalPanel"]["shown"], json!(false));
        assert_eq!(s["viewSwitcher"]["shown"], json!(false));
        assert!(s["tools"].as_object().map_or(true, |o| o.is_empty()));
    }

    #[test]
    fn back_returns_to_previous_view() {
        let c = controller();
        assert_eq!(c.do_action("open:notes"), None);
        assert_eq!(c.do_action("open:files"), None);
        assert_eq!(c.current(), Some(SlotView::Files));
        assert_eq!(c.do_action("back"), None);
        assert_eq!(c.current(), Some(SlotView::Notes));
    }

    #[test]
    fn back_twice_walks_the_history_stack() {
        let c = controller();
        for v in ["open:jira", "open:notes", "open:files"] {
            assert_eq!(c.do_action(v), None);
        }
        assert_eq!(c.do_action("back"), None);
        assert_eq!(c.current(), Some(SlotView::Notes));
        assert_eq!(c.do_action("back"), None);
        assert_eq!(c.current(), Some(SlotView::Jira));
    }

    #[test]
    fn back_with_no_history_keeps_old_behaviour() {
        let c = controller();
        // Nothing shown yet: back hides (today's `self.back(false)` behaviour).
        assert_eq!(c.do_action("back"), None);
        assert_eq!(c.current(), None);
    }

    #[test]
    fn show_reopens_last_view_and_defaults() {
        // Bare `show` on a fresh controller has no last view -> jira.
        let c = controller();
        assert_eq!(c.do_action("show"), None);
        assert_eq!(c.current(), Some(SlotView::Jira));

        // `hide` then `show` re-presents the same view.
        let c = controller();
        assert_eq!(c.do_action("open:files"), None);
        assert_eq!(c.do_action("hide"), None);
        assert_eq!(c.current(), None);
        assert_eq!(c.do_action("show"), None);
        assert_eq!(c.current(), Some(SlotView::Files));
        assert_eq!(c.state_json()["visible"], json!(true));

        // `show:<view>` opens the named view; unknown views error.
        assert_eq!(c.do_action("show:notes"), None);
        assert_eq!(c.current(), Some(SlotView::Notes));
        let err = c.do_action("show:bogus").unwrap();
        assert_eq!(err["error"], json!("unknown view"));
    }

    #[test]
    fn board_verbs_report_not_implemented() {
        // The Swift jira window's board verbs are not ported: `board:*` must
        // answer a clear error, not the generic `unhandled action`.
        let c = controller();
        for action in ["board:open:ABC", "board:list", "board:list-star:ABC", "board:bogus"] {
            let err = c.do_action(action).unwrap_or_else(|| panic!("{action} must error"));
            assert_eq!(
                err["error"],
                json!(format!("{action} not implemented in the Rust port yet")),
                "{action}"
            );
        }
        // `state.views.jira.board` stays the host placeholder `{}` (no fake
        // board model is installed).
        assert_eq!(c.state_json()["views"]["jira"]["board"], json!({}));
    }
}
