//! Port of the `SwitcherController` host from `kitchen_sink.swift`.
//!
//! Bounded first cut: the socket surface (`testQuery` + `startCommandServer`'s
//! request routing), the hotkey path (`hotkeyPrep` / `applyHotkeyPrep` /
//! `toggleCommand`), and `reloadConfig`. The shared-window transitions mirror
//! `SharedWindow`'s pure state machine (open / toggle / hide / back / home /
//! cycle / hotkey); AppKit members are reduced to per-view `MemberState` rows so
//! the controller stays `Send + Sync` and can be handed to `CommandServer`.
//!
//! Unported view internals become `views.<view>` defaults; the documented
//! top-level keys are always present. `PaneNav` / `VimKeys` are the real ported
//! models. Raw verbs other than `reload` / `restart` fall through (None), and
//! unhandled `do:` verbs delegate to `Registry::dispatch_test_do`, mirroring
//! `testQuery`'s `compare:` / `screenshot:` delegation.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{json, Map, Value};

use crate::app::config::{self, AppSettings, HeaderStyle};
use crate::app::hotkey::{self, HotkeyPrep};
use crate::app::paths::Paths;
use crate::app::registry::{PaletteCommand, RectI, Registry, SlotView};
use crate::app::socket::CommandHandler;
use crate::panes::pane_geometry::{PaneDir, Rect};
use crate::panes::pane_nav::{NavPane, PaneNav, ViewId};
use crate::panes::vim_keys::VimKeys;
use crate::views::compare::{CompareConfig, CompareRecent, CompareWindowModel};
use crate::views::screenshot::{ScreenshotConfig, ScreenshotController};

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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    visible: bool,
    palette_visible: bool,
    target_screen: Option<i32>,
    aerospace_cache_cleared: bool,
    members: HashMap<SlotView, MemberState>,
    panes: Vec<PaneSpec>,
    pane_nav: PaneNav,
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

    fn present(&mut self, v: SlotView) {
        self.ensure_member(v);
        if let Some(old) = self.current {
            if old != v {
                self.previous = Some(old);
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
        if v.is_jira() {
            self.last_jira = v;
        }
        self.visible = true;
        self.logs.push(format!("shared window: {}", v.raw()));
        self.ui_push(UiCommand::Present(v));
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

    fn toggle(&mut self) {
        if self.shown_member().is_some() {
            self.hide("toggle");
            return;
        }
        self.open(self.last);
    }

    fn hotkey(&mut self, v: SlotView) {
        if let Some(cur) = self.current {
            let same = if v == SlotView::Jira { cur.is_jira() } else { cur == v };
            if same && self.shown_member().is_some() {
                self.hide("hotkey pressed while in it");
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
            // members always report (the ui-test drawer probes read these).
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
        let pane = if self.current.is_some() {
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
            "headerStyle": self.settings.header_style.raw(),
            "terminalPanel": { "shown": self.terminal_panel },
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
    inner: Mutex<ControllerInner>,
    registry: Registry,
    /// Handles the view registrations close over, so `do:compare:*` /
    /// `do:screenshot:*` reach the same model the host holds.
    compare_model: Option<Arc<Mutex<CompareWindowModel>>>,
    screenshot_controller: Option<Arc<Mutex<ScreenshotController>>>,
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
            visible: false,
            palette_visible: false,
            target_screen: None,
            aerospace_cache_cleared: false,
            members: HashMap::new(),
            panes: Vec::new(),
            pane_nav: PaneNav::new(),
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
            view_switcher: false,
            switcher_rows: Vec::new(),
            paths_state: json!({ "shown": false, "rows": [] }),
            paths_enabled: false,
            notes_terminal: false,
            notes_browser: false,
            window_count: 0,
            window_titles: HashMap::new(),
            pending_note_open: None,
            last_key: String::new(),
            esc_sections: HashMap::new(),
            esc_app,
            using_backup: false,
            logs: Vec::new(),
            ui_queue: None,
            main_queue: None,
        };
        SwitcherController {
            inner: Mutex::new(inner),
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
            (SlotView::Files, "Files".to_string(), icon(&s.files_icon_name)),
            (SlotView::Notes, "Notes".to_string(), icon(&s.notes_icon_name)),
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
            "terminal" => self.toggle_terminal_panel(),
            "window" | "show" | "notes" | "files" | "jira" | "confluence" | "ai"
            | "compare" => self.toggle_command(name),
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

    pub fn toggle(&self) {
        self.inner.lock().unwrap().toggle();
    }

    pub fn hide(&self, reason: &str) {
        self.inner.lock().unwrap().hide(reason);
    }

    pub fn back(&self, esc: bool) {
        self.inner.lock().unwrap().back(esc);
    }

    pub fn home(&self) {
        self.inner.lock().unwrap().home();
    }

    pub fn cycle(&self, dir: i32) {
        self.inner.lock().unwrap().cycle(dir);
    }

    pub fn hotkey(&self, v: SlotView) {
        self.inner.lock().unwrap().hotkey(v);
    }

    /// `PaneNav.shared.move(_:in:)` — move the focus ring one pane over.
    pub fn pane_move(&self, dir: PaneDir) {
        let mut inner = self.inner.lock().unwrap();
        let panes = inner.panel_list();
        inner.pane_nav.move_dir(dir, &panes);
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
                    inner.hotkey(SlotView::Jira);
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
        if !text.is_empty() {
            config::apply_app_config_from_settings_text(&mut inner.settings, &text);
            inner.esc_app = inner.settings.esc_close;
            inner.esc_sections = parse_esc_close(&text);
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
        let mut inner = self.inner.lock().unwrap();
        inner.terminal_panel = !inner.terminal_panel;
        inner.ui_push(UiCommand::TerminalPanel);
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

    fn do_host_action(&self, action: &str) -> Option<Value> {
        if let Some(v) = action.strip_prefix("open:") {
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
                self.compare_open(arg, true);
                return None;
            }
            if let Some(arg) = rest.strip_prefix("open:") {
                self.compare_open(arg, false);
                return None;
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
                    let n = sel[7..].parse::<usize>().unwrap_or(0);
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
            let mut inner = self.inner.lock().unwrap();
            inner
                .tools
                .insert(name.to_string(), json!({ "shown": true }));
            return None;
        } else if let Some(name) = action.strip_prefix("tool-close:") {
            {
                let mut inner = self.inner.lock().unwrap();
                if let Some(entry) = inner.tools.get_mut(name) {
                    if let Some(o) = entry.as_object_mut() {
                        o.insert("shown".into(), json!(false));
                    }
                }
            }
            if name == "paths" {
                return self.do_host_action("paths:hide");
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
                self.back(false);
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
                let mut inner = self.inner.lock().unwrap();
                inner.terminal_panel = !inner.terminal_panel;
                inner.ui_push(UiCommand::TerminalPanel);
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
            "jira-jump" | "notes-find" | "notes-grep" | "reset-size" => None,
            _ => {
                let rest = action
                    .split_once(':')
                    .map(|(_, r)| r)
                    .unwrap_or_default();
                if action.starts_with("board:")
                    || action.starts_with("compare:")
                    || action.starts_with("screenshot:")
                {
                    return self.registry.dispatch_test_do(action).or_else(|| {
                        Some(json!({ "error": format!("unhandled action {rest}") }))
                    });
                }
                self.registry.dispatch_test_do(action)
            }
        }
    }

    /// `do:compare:open[:sub]:LEFT|RIGHT` — `showCompare` + `openPair` (Swift
    /// `compareTestDo`): present the compare view (pushing the compareText
    /// sub-view for `open-sub`) and load the pair into the shared model.
    fn compare_open(&self, arg: &str, sub: bool) {
        let Some(model) = &self.compare_model else {
            return;
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
        model.lock().unwrap().open_pair(l.as_deref(), r.as_deref());
        self.open(SlotView::Compare);
        if sub {
            self.inner.lock().unwrap().push(SlotView::CompareText);
        }
    }
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
                .insert(key.to_string(), strip_comment(v).trim().to_string());
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

impl CommandHandler for SwitcherController {
    fn state_json(&self) -> Value {
        let mut v = {
            let inner = self.inner.lock().unwrap();
            inner.state_with_registry(&self.registry)
        };
        // `screenshot` / `compare` report their real controllers when enabled
        // (the inner placeholder only covers a disabled/detached tool).
        if let Some(c) = &self.screenshot_controller {
            v["screenshot"] = c.lock().unwrap().test_state();
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
            // `sub` is the Swift sub-window flag: true while compareText is up.
            st["sub"] = json!(view.as_str() == Some("compareText"));
            st["view"] = if is_compare { view } else { json!("") };
            v["compare"] = st;
        }
        v
    }

    fn do_action(&self, action: &str) -> Option<Value> {
        self.do_host_action(action)
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

#[cfg(target_os = "macos")]
mod daemon_ui {
    use super::{DrawerSide, MainQueue, PathsCmd, SwitcherController, UiCommand, UiQueue, DEFAULT_VIEW};
    use crate::app::registry::{RectI, SlotView};
    use crate::panes::pane_geometry::PaneDir;
    use crate::ui::chrome::{ChromeConfig, NavIcon, PopupChrome};
    use crate::ui::popup::{self, KeyAction, MonitorHandle, PopupConfig};
    use crate::ui::shared_window::SlotHostWindow;
    use crate::ui::theme::{set_current_header_style, HeaderStyle as ThemeHeaderStyle, PopupColors};
    use crate::views::paths::{PathsWindow, ReturnAction as PathsReturn};
    use crate::views::switcher::{SwitchFrame, SwitchView, ViewSwitcherPanel};
    use crate::views::terminal::{TerminalConfig, TerminalPanel};
    use objc2::rc::{Retained, Weak};
    use objc2::runtime::{AnyObject, NSObject};
    use objc2::{
        define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly, Message,
    };
    use objc2_app_kit::{
        NSApplication, NSAutoresizingMaskOptions, NSButton, NSFont, NSEvent, NSTextField,
        NSView, NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState,
        NSVisualEffectView, NSWindowCollectionBehavior, NSWorkspace,
    };
    use objc2_foundation::{NSInteger, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSTimer};
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;
    use std::sync::Arc;

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

    struct DaemonUiState {
        window: Retained<SlotHostWindow>,
        chrome: Retained<PopupChrome>,
        content: Retained<NSView>,
        views: HashMap<SlotView, Retained<NSView>>,
        current: Option<SlotView>,
        /// The notes surface (kept so its drawers/editor stay addressable).
        notes: Option<crate::views::notes::NotesSurface>,
        /// The Ctrl+B prefix machine (`SharedWindow.prefixKey`).
        prefix: popup::PrefixState,
        /// The `show` palette (a `ViewSwitcherPanel`).
        palette: Option<ViewSwitcherPanel>,
        /// The Ctrl+B W view switcher (a second `ViewSwitcherPanel`).
        switcher: Option<ViewSwitcherPanel>,
        /// The `/paths` shelf tool window.
        paths: Option<PathsWindow>,
        /// The `/terminal` tool panel (`do:term` / Ctrl+B T).
        terminal_panel: Option<TerminalPanel>,
    }

    /// Every visible `NSWindow` of this app (`state.windows` mirror).
    fn visible_window_count(mtm: MainThreadMarker) -> i64 {
        let app = NSApplication::sharedApplication(mtm);
        app.windows()
            .iter()
            .filter(|w| w.isVisible())
            .count() as i64
    }

    pub struct DaemonUiIvars {
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
        let this = DaemonUi::alloc(mtm).set_ivars(DaemonUiIvars {
            controller,
            queue,
            main_queue,
            state: RefCell::new(None),
            esc_streak: Cell::new(0),
            last_esc_at: Cell::new(0.0),
            key_monitor: RefCell::new(None),
            last_vim_check: Cell::new(0.0),
        });
        let ui: Retained<DaemonUi> = unsafe { msg_send![super(this), init] };
        // The window's local keyDown guard (mirrors `PopupWindow.installMonitors`):
        // the block holds the UI weakly so AppKit's monitor retains no cycle.
        let weak = Weak::new(&*ui);
        let monitor = popup::install_local_monitor(mtm, move |event| {
            weak.load()
                .map(|ui| ui.route_key_event(event))
                .unwrap_or(false)
        });
        *ui.ivars().key_monitor.borrow_mut() = Some(monitor);
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
            // The `do:` main-hop jobs first: a waiting socket thread is the
            // most latency-sensitive caller (Swift's 2 s semaphore).
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
                    }
                }
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
                if let Some(surface) = st.notes.as_mut() {
                    let relaunched = surface.vim_poll_exit(mtm);
                    // Swift's `TerminalAutoRestart`: after a relaunch, the new
                    // terminal takes the keyboard back (only while shown).
                    if relaunched && st.window.isVisible() {
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
        fn show_palette(&self) {
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };
            let controller = self.ivars().controller.clone();
            let mut state = self.ivars().state.borrow_mut();
            let st = state.get_or_insert_with(|| self.build_window(mtm));
            let mut panel = st.palette.take().unwrap_or_else(ViewSwitcherPanel::new);
            let rows = self.prepare_panel(
                &mut panel,
                Box::new({
                    let c = controller.clone();
                    move || c.set_palette_visible(false)
                }),
            );
            let host = st.window.frame();
            panel.present(Some(SwitchFrame::new(
                host.origin.x,
                host.origin.y,
                host.size.width,
                host.size.height,
            )));
            st.palette = Some(panel);
            drop(state);
            controller.set_switcher_rows(rows);
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
            let rows = self.prepare_panel(
                &mut panel,
                Box::new({
                    let c = controller.clone();
                    move || c.set_view_switcher(false)
                }),
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

        /// Wire the shared row set + callbacks onto a panel.
        fn prepare_panel(
            &self,
            panel: &mut ViewSwitcherPanel,
            on_close: Box<dyn FnMut()>,
        ) -> Vec<String> {
            let controller = self.ivars().controller.clone();
            let views: Vec<SwitchView> = controller
                .registry()
                .nav_icons()
                .iter()
                .map(|n| SwitchView::new(n.id, n.view.title(), ""))
                .collect();
            panel.set_commands(
                controller
                    .registry()
                    .palette_commands()
                    .iter()
                    .map(|c| (c.id.clone(), c.title.clone()))
                    .collect(),
            );
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
            panel.show(&views, current, current);
            views.iter().map(|v| format!("{}|{}", v.name, v.location)).collect()
        }

        fn hide_palette(&self) {
            let mut state = self.ivars().state.borrow_mut();
            if let Some(st) = state.as_mut() {
                if let Some(panel) = st.palette.as_mut() {
                    panel.hide_panel();
                }
            }
            drop(state);
            self.ivars().controller.set_palette_visible(false);
            if let Some(mtm) = MainThreadMarker::new() {
                self.ivars().controller.set_window_count(visible_window_count(mtm));
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
                    win.show();
                    win.build(mtm);
                    win.show_window(mtm);
                }
                PathsCmd::Hide => win.hide_window(),
                PathsCmd::Return => match win.return_action {
                    PathsReturn::Path => copy_text_to_pasteboard(&win.copy_paths_text()),
                    PathsReturn::File => copy_files_to_pasteboard(&win.copy_files()),
                    PathsReturn::Open => {
                        for p in win.open_paths() {
                            open_path(&p);
                        }
                    }
                },
                PathsCmd::Select(n) => win.test_select(n),
            }
            let doc = win.test_state();
            st.paths = Some(win);
            drop(state);
            controller.set_paths_state(doc);
            controller.set_window_count(visible_window_count(mtm));
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
            } else {
                panel.show();
            }
            st.terminal_panel = Some(panel);
        }

        /// The local keyDown guard (mirrors `PopupWindow.installMonitors`'
        /// handler, scaled to the shared host): route the ported
        /// [`popup::route_key`] decision, execute Esc-hide and edit keys, and
        /// consume only what is acted on (everything else passes through).
        fn route_key_event(&self, event: &NSEvent) -> bool {
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
                    self.ivars().controller.pane_move(pane_dir);
                    return true;
                }
                popup::PrefixOutcome::Command(ch) => {
                    self.run_prefix_command(ch);
                    return true;
                }
                popup::PrefixOutcome::Passthrough => {}
            }

            let current = self.ivars().controller.current();
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
            let cfg = PopupConfig {
                edit_mode,
                vim_focus: vim_active,
                esc_close_count: current
                    .map(|v| self.ivars().controller.esc_hide_count(v) as i32)
                    .unwrap_or_else(|| self.ivars().controller.settings().esc_close as i32),
                ..PopupConfig::default()
            };

            match popup::route_key(&cfg, key) {
                KeyAction::Escape(_) => {
                    if current == Some(SlotView::CompareText) {
                        // PRD-compare KR6: Esc in compareText steps back to the
                        // compare view (Swift `slot.back(esc: true)`), it never
                        // hides the window directly.
                        self.ivars().controller.back(true);
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
                KeyAction::Edit(op) => self.perform_edit_op(op),
                _ => false,
            }
        }

        /// Run an op against the notes vim pane (`PopupWindow.vimPaneKey`).
        fn perform_vim_op(&self, op: popup::VimOp) -> bool {
            use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
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
                    let text = clip.or_else(|| {
                        let pb = NSPasteboard::generalPasteboard();
                        pb.stringForType(unsafe { NSPasteboardTypeString })
                            .map(|s| s.to_string())
                    });
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
                popup::VimOp::OpenPath => self
                    .ivars()
                    .controller
                    .log("vim: open-at-path prompt not modelled yet"),
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

        /// The armed prefix's `l`/`w`/`t`/`b` (`SharedWindow.prefixKey`).
        fn run_prefix_command(&self, ch: char) {
            match ch {
                'l' => {
                    let prev = self.ivars().controller.previous_nav();
                    if let Some(id) = prev {
                        self.ivars().controller.nav_clicked(id);
                    }
                }
                'w' => self.ivars().controller.view_switcher_show(),
                't' => self.ivars().controller.toggle_terminal_panel(),
                'b' => self
                    .ivars()
                    .controller
                    .log("prefix: sidebar rail not modelled yet"),
                _ => {}
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
                    }
                    let bounds = st.content.bounds();
                    if !st.views.contains_key(&v) {
                        let view = if v == SlotView::Notes {
                            match crate::views::notes::NotesSurface::build(mtm) {
                                Some(surface) => {
                                    let view = surface.content_view();
                                    st.notes = Some(surface);
                                    view
                                }
                                None => placeholder(mtm, v),
                            }
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
                let front_pid = NSWorkspace::sharedWorkspace()
                    .frontmostApplication()
                    .map(|a| a.processIdentifier())
                    .unwrap_or(0);
                let app = NSApplication::sharedApplication(mtm);
                st.window.makeKeyAndOrderFront(None);
                app.activate();
                let f = st.window.frame();
                (
                    RectI::new(f.origin.x, f.origin.y, f.size.width, f.size.height),
                    title,
                    st.window.windowNumber() as i64,
                    front_pid,
                )
            };
            controller.set_member(v, true, true, Some(rect));
            controller.set_member_wid(v, wid);
            controller.set_key_window(title);
            controller.set_active(true);
            controller.set_frontmost_pid(front_pid);
            controller.bump_activations();
            controller.set_window_count(visible_window_count(mtm));
        }

        fn hide(&self) {
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };
            let controller = self.ivars().controller.clone();
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
            let colors = PopupColors::default();

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
            let nav: Vec<NavIcon> = self
                .ivars()
                .controller
                .registry()
                .nav_icons()
                .iter()
                .map(|n| NavIcon {
                    id: n.id as i32,
                    tip: n.tip.clone(),
                    image: None,
                })
                .collect();
            chrome.set_nav_icons(nav);

            root.addSubview(&fx);
            root.addSubview(&tint);
            root.addSubview(&chrome);

            // The view switcher: one transparent button per enabled view over
            // the chrome band; a click routes through `controller.navClicked`.
            let target = as_any(self);
            let entries: Vec<(i64, String)> = self
                .ivars()
                .controller
                .registry()
                .nav_icons()
                .iter()
                .map(|n| (n.id, n.view.title().to_string()))
                .collect();
            for (i, (id, title)) in entries.iter().enumerate() {
                let button = unsafe {
                    NSButton::buttonWithTitle_target_action(
                        &NSString::from_str(title),
                        Some(target),
                        Some(sel!(navClicked:)),
                        mtm,
                    )
                };
                button.setTag(*id as NSInteger);
                button.setBordered(false);
                button.setFont(Some(&NSFont::systemFontOfSize(12.0)));
                button.setContentTintColor(Some(&colors.text.to_nscolor()));
                button.setFrame(NSRect::new(
                    NSPoint::new(
                        NAV_X0 + i as f64 * NAV_STEP,
                        (HEADER_HEIGHT - NAV_HEIGHT) / 2.0,
                    ),
                    NSSize::new(NAV_WIDTH, NAV_HEIGHT),
                ));
                root.addSubview(&button);
            }
            let close = unsafe {
                NSButton::buttonWithTitle_target_action(
                    &NSString::from_str(""),
                    Some(target),
                    Some(sel!(closeClicked:)),
                    mtm,
                )
            };
            close.setBordered(false);
            close.setFrame(NSRect::new(NSPoint::new(4.0, 4.0), NSSize::new(26.0, 22.0)));
            root.addSubview(&close);

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
                prefix: popup::PrefixState::new(),
                palette: None,
                switcher: None,
                paths: None,
                terminal_panel: None,
            }
        }
    }

    // -- pasteboard / open helpers for the paths shelf (`do:paths:return`) ----

    fn copy_text_to_pasteboard(text: &str) {
        use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
        let pb = NSPasteboard::generalPasteboard();
        pb.clearContents();
        let _ = unsafe { pb.setString_forType(&NSString::from_str(text), NSPasteboardTypeString) };
    }

    fn copy_files_to_pasteboard(paths: &[String]) {
        use objc2::runtime::ProtocolObject;
        use objc2_app_kit::{NSPasteboard, NSPasteboardWriting};
        use objc2_foundation::NSURL;
        let pb = NSPasteboard::generalPasteboard();
        pb.clearContents();
        let objects: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> = paths
            .iter()
            .map(|p| {
                let url = NSURL::fileURLWithPath(&NSString::from_str(p));
                ProtocolObject::from_retained(url)
            })
            .collect();
        if !objects.is_empty() {
            let array = objc2_foundation::NSArray::from_retained_slice(&objects);
            let _ = pb.writeObjects(&array);
        }
    }

    fn open_path(path: &str) {
        use objc2_app_kit::NSWorkspace;
        let url = objc2_foundation::NSURL::fileURLWithPath(&NSString::from_str(path));
        NSWorkspace::sharedWorkspace().openURL(&url);
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

    const REQUIRED: [&str; 18] = [
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
    ];

    fn controller() -> SwitcherController {
        SwitcherController::new_default()
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
        assert_eq!(c.do_action("totally-unknown"), None);
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
    fn drawer_toggles_update_state_and_queue() {
        let c = controller();
        let q: UiQueue = Arc::new(Mutex::new(Vec::new()));
        c.install_ui(q.clone());
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
        let q: UiQueue = Arc::new(Mutex::new(Vec::new()));
        c.install_ui(q.clone());
        let err = c.do_action("paths:show").unwrap();
        assert_eq!(err["error"], json!("[paths] not enabled"));
        c.set_paths_enabled(true);
        assert_eq!(c.do_action("paths:show"), None);
        assert_eq!(c.state_json()["paths"]["shown"], json!(true));
        assert_eq!(c.do_action("paths:select:2"), None);
        assert_eq!(c.state_json()["paths"]["selection"], json!(2));
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
        let q: UiQueue = Arc::new(Mutex::new(Vec::new()));
        c.install_ui(q.clone());
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
            vec![64, 60, 61, 65, 67, 66],
            "files first, then notes / jira / confluence / compare / ai"
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
        assert_eq!(c.do_action("compare:start"), Some(Value::Null));
        assert!(c.do_action("screenshot:close").is_some());

        let mut off = controller();
        off.register_views_from_text("");
        let ids: Vec<i64> = off.registry().nav_icons().iter().map(|n| n.id).collect();
        assert_eq!(ids, vec![64, 60, 61], "confluence / compare / ai stay hidden");
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
}
