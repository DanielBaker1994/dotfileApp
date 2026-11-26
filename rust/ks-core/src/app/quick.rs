//! The quick agent (`kitchen-sink quick`): a small resident process that owns
//! Hyper+S (the palette) and Hyper+X (screenshot) so both work with the main
//! app not running.
//!
//! It is a mode of the SAME signed bundle (so the Screen Recording grant
//! applies), an Accessory app with no status item, no shared window, no
//! `SwitcherController` and no Python worker pool. Its own socket
//! (`$TMPDIR/ks-quick.sock`, lock beside it) takes `ping`, `toggle`, `show`,
//! `hide`, `quit`, `state`, `screenshot[\tWORDS]` and `screenshot-permission`.
//! Event driven: every request is one main-queue dispatch; the only timer is a
//! 30 ms poll that exists while a tool panel is open.
//!
//! Palette picks: the tools it owns (Find Recent Files, Create File, Text to
//! Table, Pretty Print, Find Window) open locally, workspaces go through
//! AeroSpace IPC, everything else is forwarded to the main daemon's socket
//! (cold-starting it through `bin/kitchen_sink.sh` when nothing answers).
//!
//! The pure half (verbs, pick routing, palette rows, the socket handler) is
//! platform independent and unit tested; the AppKit half is `appkit`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::app::socket::{CommandHandler, Reply};

// ---------------------------------------------------------------------------
// Verbs
// ---------------------------------------------------------------------------

/// What a plain launch message does on the quick socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchVerb {
    /// Show the palette, or hide it when it is already up.
    Toggle,
    /// Show the palette (a no-op when it is up).
    Show,
    Hide,
    Quit,
}

/// `toggle` / `show` / `hide` / `quit`; anything else is ignored.
pub fn launch_verb(message: &str) -> Option<LaunchVerb> {
    match message.trim() {
        "toggle" => Some(LaunchVerb::Toggle),
        "show" => Some(LaunchVerb::Show),
        "hide" => Some(LaunchVerb::Hide),
        "quit" => Some(LaunchVerb::Quit),
        _ => None,
    }
}

/// The startup action of `kitchen-sink quick [ACTION]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartAction {
    None,
    Show,
    Screenshot,
}

pub fn start_action(arg: Option<&str>) -> StartAction {
    match arg {
        Some("show") | Some("toggle") => StartAction::Show,
        Some("screenshot") => StartAction::Screenshot,
        _ => StartAction::None,
    }
}

/// The words of a `screenshot\tWORDS` request and whether the caller wants
/// the controller's reply bytes (`--raw`, `--print-geometry`, …) — same
/// semantics as the main daemon's `screenshot_message`.
pub fn screenshot_words(rest: &str) -> (Vec<String>, bool) {
    let words: Vec<String> = rest.split('\t').filter(|w| !w.is_empty()).map(str::to_string).collect();
    let wants = crate::engines::screenshot_annotations::ShotArgs::parse(&words)
        .map(|a| a.wants_reply())
        .unwrap_or(false);
    (words, wants)
}

/// A command for the agent's main thread.
pub enum QuickCmd {
    Verb(LaunchVerb),
    Screenshot { words: Vec<String>, reply: Option<Reply> },
}

// ---------------------------------------------------------------------------
// Palette pick routing
// ---------------------------------------------------------------------------

/// Where a palette command id runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickRoute {
    /// "Find Recent Files" (the paths shelf window).
    Paths,
    /// A `ToolPanel` the agent owns (`prettyprint`, `filefast`, `text-table`).
    Tool(&'static str),
    FindWindow,
    Screenshot,
    /// Forward this launch message to the main daemon.
    Forward(String),
}

/// Route a `[app] palette` command id: the agent-owned tools run here,
/// everything else (views, "window"/Bansho, terminal, jira-config, health)
/// belongs to the main app.
pub fn route_command(id: &str) -> PickRoute {
    match id {
        "paths" => PickRoute::Paths,
        "prettyprint" => PickRoute::Tool("prettyprint"),
        "filefast" => PickRoute::Tool("filefast"),
        "text-table" => PickRoute::Tool("text-table"),
        "find-window" => PickRoute::FindWindow,
        "screenshot" => PickRoute::Screenshot,
        other => PickRoute::Forward(forward_message(other)),
    }
}

/// The main daemon's launch message for a command id (its socket's
/// `toggle_command` names): `jira-config` is `jira-dashboard` there.
pub fn forward_message(id: &str) -> String {
    match id {
        "jira-config" => "jira-dashboard".to_string(),
        other => other.to_string(),
    }
}

/// Picks that bring the main app forward (it activates itself): the agent
/// must not hand focus back to the previous app for these.
pub fn pick_keeps_app(id: &str) -> bool {
    matches!(id, "window" | "files" | "jira" | "confluence" | "ai" | "jira-config")
}

/// The `bin/kitchen_sink.sh` mode that cold-starts the main app when a
/// forwarded message finds nothing listening.
pub fn cold_start_mode(message: &str) -> &'static str {
    match message {
        "files" => "files",
        "jira" | "jira-dashboard" | "jira-setup" => "jira",
        "confluence" => "confluence",
        "ai" => "ai",
        _ => "window",
    }
}

/// Deliver `message` to the main daemon; with no listener, cold-start it
/// (`script` = `bin/kitchen_sink.sh`, LaunchServices for TCC) in the mode that
/// matches. Returns `true` when the daemon took the message.
pub fn forward_to_main(notes_sock: &str, script: &str, message: &str) -> bool {
    if crate::app::cli::send_launch_message(notes_sock, message) {
        return true;
    }
    let mode = cold_start_mode(message);
    match std::process::Command::new(script).arg(mode).spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(e) => crate::app::logging::ws_log(&format!("quick: cannot cold-start the main app via {script}: {e}")),
    }
    false
}

// ---------------------------------------------------------------------------
// Palette rows (pure scan of commands.toml)
// ---------------------------------------------------------------------------

fn id_list(v: &str) -> Vec<String> {
    v.split(',').map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect()
}

/// The Hyper+S palette's `(id, title)` rows from the config text, the same
/// rows the main daemon builds (`build_palette` + `hyper_palette`) without a
/// registry: enabled `in-palette` command sections in file order, the
/// enabled confluence / ai views and "Bansho", ordered by `[app]
/// palette-first`, then narrowed to `[app] palette` (empty = all).
pub fn palette_rows(text: &str) -> Vec<(String, String)> {
    use crate::app::sections::{label_of, parse_sections, parse_sections_ordered, section_flag, tri_pure};
    use crate::app::sections::COMMAND_SECTION_SKIPS;
    let sections = parse_sections(text);
    let label = label_of(&sections);
    let app = sections.get("app").cloned().unwrap_or_default();

    let mut all: Vec<(String, String)> = Vec::new();
    for (name, vars) in parse_sections_ordered(text) {
        if COMMAND_SECTION_SKIPS.contains(&name.as_str()) {
            continue;
        }
        if vars.get("enabled").map(String::as_str) != Some("true") {
            continue;
        }
        let in_palette = vars.get("in-palette").map(|v| tri_pure(v).unwrap_or(true)).unwrap_or(true);
        let jira = section_flag(&sections, "jira", false);
        if !in_palette || (name == "jira-config" && !jira) {
            continue;
        }
        let title = vars.get("label").map(|l| l.trim().to_string()).filter(|l| !l.is_empty());
        all.push((name.clone(), title.unwrap_or_else(|| name.clone())));
    }
    let listed = |section: &str| -> bool {
        sections
            .get(section)
            .and_then(|m| m.get("in-palette"))
            .map(|v| tri_pure(v).unwrap_or(true))
            .unwrap_or(true)
    };
    if section_flag(&sections, "confluence", false) && listed("confluence") {
        all.push(("confluence".into(), label("confluence", "Confluence")));
    }
    if section_flag(&sections, "ai", false) && listed("ai") {
        all.push(("ai".into(), label("ai", "AI")));
    }
    let shared_window = app.get("shared-window").and_then(|v| tri_pure(v)).unwrap_or(true);
    if shared_window {
        all.push(("window".into(), "Bansho".into()));
    }

    let first: Vec<usize> = app
        .get("palette-first")
        .map(|v| id_list(v))
        .unwrap_or_default()
        .iter()
        .filter_map(|n| all.iter().position(|c| c.0.to_lowercase() == *n))
        .collect();
    let mut ordered: Vec<(String, String)> = first.iter().map(|i| all[*i].clone()).collect();
    for (i, c) in all.into_iter().enumerate() {
        if !first.contains(&i) {
            ordered.push(c);
        }
    }

    let ids = app.get("palette").map(|v| id_list(v)).unwrap_or_default();
    if ids.is_empty() {
        return ordered;
    }
    ids.iter()
        .filter_map(|id| ordered.iter().find(|c| c.0.to_lowercase() == *id).cloned())
        .collect()
}

// ---------------------------------------------------------------------------
// The socket handler (platform independent; the sink is the agent's main queue)
// ---------------------------------------------------------------------------

/// What the socket thread shares with the main thread.
pub struct QuickShared {
    /// Mirror of the UI state, republished by the main thread on every change.
    pub state: Mutex<Value>,
}

impl QuickShared {
    pub fn new() -> Arc<QuickShared> {
        Arc::new(QuickShared { state: Mutex::new(json!({})) })
    }
}

pub struct QuickHandler {
    pub sink: Box<dyn Fn(QuickCmd) + Send + Sync>,
    pub shared: Arc<QuickShared>,
    /// Whether a capture session is up (read from the socket thread).
    pub screenshot_busy: Box<dyn Fn() -> bool + Send + Sync>,
}

impl CommandHandler for QuickHandler {
    fn state_json(&self) -> Value {
        let mut v = self.shared.state.lock().map(|g| g.clone()).unwrap_or_else(|_| json!({}));
        if let Some(o) = v.as_object_mut() {
            o.insert("agent".into(), json!("quick"));
            o.insert("pid".into(), json!(std::process::id()));
            o.insert("screenshotBusy".into(), json!((self.screenshot_busy)()));
        }
        v
    }

    fn do_action(&self, action: &str) -> Option<Value> {
        Some(json!({ "error": format!("unknown action {action} (quick agent: use the main socket)") }))
    }

    fn raw_request(&self, verb: &str, _rest: &str) -> Option<String> {
        Some(format!("error: {verb} is not served by the quick agent"))
    }

    fn raw_deferred(&self, verb: &str, rest: &str, reply: Reply) -> Option<Reply> {
        match verb {
            "screenshot" => {
                let (words, wants) = screenshot_words(rest);
                let reply = if wants { Some(reply) } else { None };
                (self.sink)(QuickCmd::Screenshot { words, reply });
                None
            }
            "screenshot-permission" => {
                #[cfg(target_os = "macos")]
                let ok = crate::views::screenshot::screen_capture_permitted();
                reply.send(if ok { "granted" } else { "denied" });
                None
            }
            // reload / restart / pane-shot / window-shot: main daemon verbs.
            _ => Some(reply),
        }
    }

    fn launch(&self, message: &str) {
        if let Some(v) = launch_verb(message) {
            (self.sink)(QuickCmd::Verb(v));
        }
    }
}

/// `{"tools": {name: {"shown": bool}}}` helper for the state mirror.
pub fn tools_state(tools: &HashMap<String, bool>) -> Value {
    let mut m = serde_json::Map::new();
    let mut names: Vec<&String> = tools.keys().collect();
    names.sort();
    for n in names {
        m.insert(n.clone(), json!({ "shown": tools[n] }));
    }
    Value::Object(m)
}

#[cfg(target_os = "macos")]
pub use appkit::run;

// ---------------------------------------------------------------------------
// AppKit half
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod appkit {
    use super::*;
    use crate::app::paths::Paths;
    use crate::app::socket::{CommandServer, DaemonLock};
    use crate::engines::path_shelf;
    use crate::ui::popup::{self, MonitorHandle};
    use crate::views::files::FileBrowser;
    use crate::views::files_actions::FilesHost;
    use crate::views::paths::{PathsEvent, PathsWindow, ReturnAction as PathsReturn};
    use crate::views::screenshot::{ScreenshotConfig, ScreenshotController};
    use crate::views::switcher::{
        cached_workspaces, refresh_workspaces_in_background, PaletteAction, PalettePanel, SwitchFrame, WorkspaceRow,
    };
    use crate::views::tools::{ToolKind, ToolPanel};
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSEvent};
    use objc2_foundation::{NSPoint, NSRect, NSSize};
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    fn log(msg: &str) {
        crate::app::logging::ws_log(&format!("quick: {msg}"));
    }

    thread_local! {
        static QUICK: RefCell<Option<Rc<QuickUi>>> = const { RefCell::new(None) };
    }

    /// Run `f` against the agent UI (main thread; a no-op before it exists).
    fn with_ui(f: impl FnOnce(&Rc<QuickUi>)) {
        let ui = QUICK.with(|q| q.borrow().clone());
        if let Some(ui) = ui {
            f(&ui);
        }
    }

    /// [`with_ui`] returning a value (`None` before the UI exists).
    fn with_ui_ret<R>(f: impl FnOnce(&Rc<QuickUi>) -> R) -> Option<R> {
        QUICK.with(|q| q.borrow().clone()).map(|ui| f(&ui))
    }

    /// Defer `f` to a fresh main-queue turn (out of an AppKit callback).
    fn defer(f: impl FnOnce(&Rc<QuickUi>) + Send + 'static) {
        crate::platform::run_on_main(move || with_ui(f));
    }

    /// Find Recent Files: the Files view torn down (no sidebar / Pin) in a
    /// half-screen panel, fed by the Recent engine.
    struct FilesWin {
        browser: FileBrowser,
        panel: Option<objc2::rc::Retained<popup::PopupPanel>>,
    }

    struct QuickUi {
        paths: Paths,
        files_win: RefCell<Option<FilesWin>>,
        /// Ctrl+H/J/K/L focus ring + spatial pane moves for the Recent window.
        files_nav: RefCell<crate::app::live_panes::LiveNav>,
        /// Vim key state for the Recent window's sidebar / list / filter.
        files_vim: RefCell<crate::panes::vim_keys::VimKeys>,
        shared: Arc<QuickShared>,
        server: RefCell<Option<CommandServer>>,
        palette: RefCell<Option<PalettePanel>>,
        tools: RefCell<HashMap<String, ToolPanel>>,
        opened_at: RefCell<HashMap<String, Instant>>,
        paths_win: RefCell<Option<PathsWindow>>,
        shot: Arc<Mutex<ScreenshotController>>,
        /// The app the palette was opened over (`None` = nothing to hand back to).
        palette_return: Cell<Option<i32>>,
        /// That app + when, kept after a pick for the tool panel it opens.
        pick_return: Cell<Option<(i32, Instant)>>,
        polling: Cell<bool>,
        monitor: RefCell<Option<MonitorHandle>>,
        /// Raycast-style navigation stack: the surface a palette pick opened
        /// (`"paths"` or a tool name) and the app to hand back to. Esc on that
        /// surface returns to the palette; only Esc on the palette closes.
        back_to: RefCell<Option<(String, Option<i32>)>>,
    }

    impl QuickUi {
        // -- config ---------------------------------------------------------

        /// Re-read commands.toml (pure scan, no helper) and re-install the
        /// theme, so edits apply on the next press without a restart.
        fn refresh_config(&self) -> String {
            let text = crate::app::config::read_config_text_pure().unwrap_or_default();
            let sections = crate::app::sections::parse_sections(&text);
            crate::ui::theme::install_theme(
                &sections.get("theme").cloned().unwrap_or_default(),
                sections.clone(),
            );
            text
        }

        fn app_settings(text: &str) -> crate::app::config::AppSettings {
            let sections = crate::app::sections::parse_sections(text);
            let mut s = crate::app::config::AppSettings::default();
            if let Some(f) = sections.get("app").and_then(|a| a.get("terminal-font")) {
                if !f.trim().is_empty() {
                    s.terminal_font = f.trim().to_string();
                }
            }
            s
        }

        fn shot_config(text: &str) -> ScreenshotConfig {
            let sections = crate::app::sections::parse_sections(text);
            ScreenshotConfig::from_entries(&sections.get("screenshot").cloned().unwrap_or_default())
        }

        // -- state mirror ---------------------------------------------------

        fn publish_state(&self) {
            let palette = self.palette.try_borrow().map(|p| p.as_ref().is_some_and(|x| x.is_visible())).unwrap_or(true);
            let tools: HashMap<String, bool> = self
                .tools
                .try_borrow()
                .map(|t| t.iter().map(|(n, p)| (n.clone(), p.is_shown())).collect())
                .unwrap_or_default();
            let paths = self
                .paths_win
                .try_borrow()
                .map(|p| p.as_ref().is_some_and(|w| w.is_window_visible()))
                .unwrap_or(true)
                || self
                    .files_win
                    .try_borrow()
                    .map(|g| g.as_ref().and_then(|w| w.panel.as_ref()).is_some_and(|p| p.isVisible()))
                    .unwrap_or(true);
            if let Ok(mut s) = self.shared.state.lock() {
                *s = json!({
                    "palette": palette,
                    "paths": paths,
                    "tools": tools_state(&tools),
                    "openTools": tools.iter().filter(|(_, v)| **v).map(|(n, _)| n.clone()).collect::<Vec<_>>(),
                    "socket": self.paths.quick_socket_path(),
                    "mainSocket": self.paths.notes_socket_path(),
                });
            }
        }

        fn palette_visible(&self) -> bool {
            self.palette.try_borrow().map(|p| p.as_ref().is_some_and(|x| x.is_visible())).unwrap_or(false)
        }

        // -- commands -------------------------------------------------------

        fn run(&self, cmd: QuickCmd) {
            match cmd {
                QuickCmd::Verb(LaunchVerb::Toggle) => {
                    if self.palette_visible() {
                        self.cancel_palette();
                    } else {
                        self.show_palette();
                    }
                }
                QuickCmd::Verb(LaunchVerb::Show) => {
                    if !self.palette_visible() {
                        self.show_palette();
                    }
                }
                QuickCmd::Verb(LaunchVerb::Hide) => self.cancel_palette(),
                QuickCmd::Verb(LaunchVerb::Quit) => self.quit(),
                QuickCmd::Screenshot { words, reply } => self.screenshot(words, reply),
            }
        }

        fn quit(&self) {
            log("quit requested");
            if let Some(mut s) = self.server.borrow_mut().take() {
                s.stop();
            }
            std::process::exit(0);
        }

        fn screenshot(&self, words: Vec<String>, reply: Option<Reply>) {
            let text = self.refresh_config();
            let cfg = Self::shot_config(&text);
            if !cfg.enabled {
                log("screenshot: [screenshot] is not enabled");
                return;
            }
            // Never swap the config under a live capture session.
            if let Ok(mut c) = self.shot.try_lock() {
                if c.is_idle() {
                    c.set_config(cfg);
                }
            }
            crate::views::screenshot::run_screenshot_message(self.shot.clone(), words, reply);
        }

        // -- palette --------------------------------------------------------

        fn prewarm_palette(&self) {
            if let (Ok(mut p), Some(mtm)) = (self.palette.try_borrow_mut(), MainThreadMarker::new()) {
                if p.is_none() {
                    let mut panel = PalettePanel::new();
                    panel.prewarm(mtm);
                    *p = Some(panel);
                }
            }
        }

        fn show_palette(&self) {
            let Some(mtm) = MainThreadMarker::new() else { return };
            let text = self.refresh_config();
            let mut panel = self.palette.borrow_mut().take().unwrap_or_else(PalettePanel::new);
            // `aerospace` can stall for seconds: paint the cached rows and
            // refresh off-main.
            panel.set_rows(palette_rows(&text), cached_workspaces());
            refresh_workspaces_in_background(|fresh| defer(move |ui| ui.refresh_palette_workspaces(fresh)));
            panel.set_callbacks(
                Box::new(|action: PaletteAction| {
                    // Runs before the click path orders the palette out.
                    with_ui(|ui| ui.pick_focus(&action));
                    // Deferred: a click-pick must leave the click handler
                    // before the panel comes down.
                    defer(|ui| ui.hide_palette_panel());
                    with_ui(|ui| ui.run_pick(action));
                }),
                Box::new(|| {
                    // Esc / click-off inside the palette.
                    with_ui(|ui| {
                        ui.hand_back();
                    });
                    defer(|ui| ui.cancel_palette());
                }),
            );
            // Centre on the screen under the mouse.
            let m = NSEvent::mouseLocation();
            let host = crate::platform::screen_containing(m.x, m.y)
                .or_else(crate::platform::main_screen)
                .map(|s| s.visible)
                .map(|v| NSRect::new(NSPoint::new(v.x, v.y), NSSize::new(v.w, v.h)))
                .unwrap_or_else(|| NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1440.0, 900.0)));
            panel.present(
                mtm,
                Some(SwitchFrame::new(host.origin.x, host.origin.y, host.size.width, host.size.height)),
            );
            *self.palette.borrow_mut() = Some(panel);
            crate::app::logging::log_hotkey_shown(None);
            // The app Hyper+S was pressed over, for the cancel hand-back.
            let own = std::process::id() as i32;
            let front = crate::platform::frontmost_app().map(|a| a.pid).filter(|p| *p != own);
            self.palette_return.set(front);
            self.publish_state();
        }

        fn refresh_palette_workspaces(&self, rows: Vec<WorkspaceRow>) {
            if let Ok(mut p) = self.palette.try_borrow_mut() {
                if let Some(panel) = p.as_mut().filter(|x| x.is_visible()) {
                    panel.refresh_workspaces(rows);
                }
            }
        }

        /// Re-activate the app Hyper+S was pressed over, *before* the palette
        /// is ordered out (else macOS activates us and raises something).
        fn hand_back(&self) -> bool {
            let Some(pid) = self.palette_return.take() else { return false };
            self.pick_return.set(Some((pid, Instant::now())));
            if !crate::platform::activate_pid(pid) {
                if let Some(mtm) = MainThreadMarker::new() {
                    NSApplication::sharedApplication(mtm).deactivate();
                }
            }
            true
        }

        fn pick_focus(&self, action: &PaletteAction) {
            let keeps = match action {
                PaletteAction::Command(id) => pick_keeps_app(id),
                PaletteAction::Workspace(_) => false,
            };
            if keeps {
                if let Some(pid) = self.palette_return.take() {
                    self.pick_return.set(Some((pid, Instant::now())));
                }
            } else {
                self.hand_back();
            }
        }

        fn hide_palette_panel(&self) {
            if let Ok(mut p) = self.palette.try_borrow_mut() {
                if let Some(panel) = p.as_mut() {
                    panel.hide_panel();
                }
            }
            self.publish_state();
        }

        /// Esc on an empty query / the toggle verb while it is up.
        fn cancel_palette(&self) {
            let handed = self.hand_back();
            self.hide_palette_panel();
            if !handed {
                if let Some(mtm) = MainThreadMarker::new() {
                    NSApplication::sharedApplication(mtm).deactivate();
                }
            }
        }

        fn run_pick(&self, action: PaletteAction) {
            match action {
                PaletteAction::Workspace(id) => switch_workspace(&id),
                PaletteAction::Command(id) => match route_command(&id) {
                    PickRoute::Paths => {
                        self.set_back_to("files");
                        self.show_files();
                    }
                    PickRoute::Tool(name) => {
                        if let Some(kind) = ToolKind::from_name(name) {
                            self.set_back_to(name);
                            self.open_tool(kind);
                        }
                    }
                    PickRoute::FindWindow => {
                        let text = self.refresh_config();
                        crate::app::find_window::run_find_window(
                            &text,
                            Arc::new(|t, sym| {
                                crate::platform::run_on_main(move || crate::ui::toast::show(&t, sym))
                            }),
                        );
                    }
                    PickRoute::Screenshot => self.screenshot(Vec::new(), None),
                    PickRoute::Forward(msg) => self.forward(&msg),
                },
            }
        }

        /// A palette pick is about to open `surface`: remember it so Esc there
        /// comes back to the palette.
        fn set_back_to(&self, surface: &str) {
            let pid = self.pick_return.get().map(|(pid, _)| pid);
            *self.back_to.borrow_mut() = Some((surface.to_string(), pid));
        }

        /// `surface` just closed. If a palette pick opened it and Esc closed it,
        /// show the palette again (the previous view) instead of leaving.
        fn surface_closed(&self, surface: &str, by_esc: bool) {
            let pid = {
                let mut b = self.back_to.borrow_mut();
                match b.as_ref() {
                    Some((s, _)) if s == surface => b.take().and_then(|(_, pid)| pid),
                    _ => return,
                }
            };
            if !by_esc {
                return;
            }
            self.show_palette();
            if self.palette_return.get().is_none() {
                self.palette_return.set(pid);
            }
        }

        fn forward(&self, msg: &str) {
            let sock = self.paths.notes_socket_path();
            let script = format!("{}/bin/kitchen_sink.sh", self.paths.asset_dir());
            let msg = msg.to_string();
            std::thread::spawn(move || {
                let delivered = forward_to_main(&sock, &script, &msg);
                log(&format!("forward '{msg}' to the main app: {}", if delivered { "delivered" } else { "cold-starting it" }));
            });
        }

        // -- tool panels ------------------------------------------------------

        fn open_tool(&self, kind: ToolKind) {
            let Some(mtm) = MainThreadMarker::new() else { return };
            let text = self.refresh_config();
            {
                let Ok(mut tools) = self.tools.try_borrow_mut() else { return };
                let mut panel = tools
                    .remove(kind.name())
                    .unwrap_or_else(|| ToolPanel::new(kind, &Self::app_settings(&text)));
                // Opened by a palette pick just now: its close hands focus
                // back to the app Hyper+S was pressed over.
                let picked = self
                    .pick_return
                    .take()
                    .filter(|(_, at)| at.elapsed().as_secs_f64() < 2.0)
                    .map(|(pid, _)| pid);
                if picked.is_some() {
                    panel.set_return_pid(picked);
                }
                panel.show(mtm);
                tools.insert(kind.name().to_string(), panel);
            }
            self.opened_at.borrow_mut().insert(kind.name().to_string(), Instant::now());
            self.publish_state();
            self.ensure_poll();
        }

        /// While a tool panel is up: apply background results and keep the
        /// keyboard on a just-opened panel (`reclaimToolKey`). Reschedules
        /// itself only while something is visible.
        fn ensure_poll(&self) {
            if self.polling.replace(true) {
                return;
            }
            schedule_poll();
        }

        fn poll_tick(&self) -> bool {
            let Some(mtm) = MainThreadMarker::new() else { return false };
            let mut any = false;
            let mut closed: Vec<String> = Vec::new();
            if let Ok(mut tools) = self.tools.try_borrow_mut() {
                let key_wid: i64 = NSApplication::sharedApplication(mtm)
                    .keyWindow()
                    .map(|w| w.windowNumber() as i64)
                    .unwrap_or(0);
                let mut opened = self.opened_at.borrow_mut();
                for (name, panel) in tools.iter_mut() {
                    if let Some(at) = opened.get(name).copied() {
                        if at.elapsed().as_secs_f64() >= 0.25 {
                            opened.remove(name);
                            // `key_wid == 0`: nothing of ours is key (the pick's
                            // activation hand-back landed after the panel took key).
                            if !panel.has_key() && key_wid != panel.window_number() {
                                panel.show(mtm);
                            }
                        }
                    }
                    let was_shown = panel.is_shown();
                    panel.poll();
                    if was_shown && !panel.is_shown() {
                        closed.push(name.clone());
                    }
                    any |= panel.is_shown();
                }
            } else {
                any = true;
            }
            if !closed.is_empty() {
                // Esc is the last event the app saw (a timer does not replace it).
                let by_esc = esc_just_pressed(mtm);
                for name in closed {
                    self.surface_closed(&name, by_esc);
                }
            }
            self.publish_state();
            any
        }

        // -- paths shelf --------------------------------------------------------

        fn shelf_entries() -> Vec<path_shelf::Item> {
            path_shelf::pure_load(&path_shelf::default_store(), path_shelf::MAX_LIMIT)
                .into_iter()
                .filter(|i| std::path::Path::new(&i.path).exists())
                .collect()
        }

        // -- Find Recent Files (the torn-down Files view) ------------------------
        // The file actions themselves are shared with the main app's Files
        // view: `views::files_actions::FilesHost` (impl below).

        fn show_files(&self) {
            use crate::ui::popup::{CardFrame, PopupConfig, PopupPanel};
            let Some(mtm) = MainThreadMarker::new() else { return };
            let text = self.refresh_config();
            let section = crate::app::sections::parse_sections(&text).get("files").cloned().unwrap_or_default();
            // The agent runs its own Recent engine so the list is live without the main app.
            crate::views::files::start_recent(&section);
            let mut win = self.files_win.borrow_mut().take().unwrap_or_else(|| {
                let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
                let mut browser = FileBrowser::new(&home);
                browser.configure(&section);
                browser.build(mtm);
                if let Some(v) = &browser.view {
                    v.set_sidebar_width(150.0);
                    v.set_on_event(Box::new(|ev| with_ui(|ui| ui.files_event(ev))));
                }
                FilesWin { browser, panel: None }
            });
            win.browser.show_virtual(0);
            win.browser.sync_view();
            // Half the size of the full-screen app window, centred on the screen under the mouse.
            let m = NSEvent::mouseLocation();
            let vf = crate::platform::screen_containing(m.x, m.y)
                .or_else(crate::platform::main_screen)
                .map(|s| s.visible)
                .unwrap_or(crate::platform::screens::Rect::new(0.0, 0.0, 1440.0, 900.0));
            let (w, h) = (vf.w * 0.72, vf.h * 0.55);
            let frame = NSRect::new(NSPoint::new(vf.x + (vf.w - w) / 2.0, vf.y + (vf.h - h) / 2.0), NSSize::new(w, h));
            if win.panel.is_none() {
                let cfg = PopupConfig {
                    name: "files-quick".to_string(),
                    frame: CardFrame { x: frame.origin.x, y: frame.origin.y, width: w, height: h },
                    tool_panel: true,
                    floating: true,
                    ..PopupConfig::default()
                };
                let panel = PopupPanel::create(mtm, &cfg);
                // The Files view draws no background of its own: wrap it in the
                // shared translucent card (as the palette does) so it reads over any app.
                let colors = crate::ui::theme::section_colors("files");
                let card = crate::ui::panel_frame::build(
                    mtm,
                    w,
                    h,
                    crate::ui::panel_frame::PanelSpec {
                        colors,
                        header_color: None,
                        title: None,
                        meta: "",
                        buttons: Vec::new(),
                        icon: None,
                        tint_alpha: 0.92,
                    },
                );
                card.chrome.setHidden(true);
                card.content.setHidden(true);
                if let Some(root) = win.browser.content_view() {
                    root.setFrame(card.root.bounds());
                    card.root.addSubview(&root);
                }
                panel.setOpaque(false);
                panel.setBackgroundColor(Some(&objc2_app_kit::NSColor::clearColor()));
                panel.setHasShadow(true);
                panel.setContentView(Some(&card.root));
                panel.setTitle(&objc2_foundation::NSString::from_str("Recent files"));
                // Click-off closes it (no palette), unless the preview popup
                // or one of its own sheets (Get Info, batch rename, the
                // delete / empty-trash confirms) took key — that closed the
                // panel with the sheet still attached.
                panel.set_on_resign(Box::new(|| {
                    defer(|ui| {
                        if crate::views::file_popup::is_live()
                            || ui.files_window().is_some_and(|p| p.attachedSheet().is_some())
                        {
                            return;
                        }
                        ui.hide_files();
                        ui.surface_closed("files", false);
                    });
                }));
                win.panel = Some(panel);
            }
            if let Some(panel) = &win.panel {
                panel.setFrame_display(frame, true);
                panel.makeKeyAndOrderFront(None);
                if let Some(v) = &win.browser.view {
                    let field = v.search_field();
                    let r: &objc2_app_kit::NSResponder = &field;
                    panel.makeFirstResponder(Some(r));
                }
            }
            *self.files_win.borrow_mut() = Some(win);
            self.publish_state();
        }

        /// Find Recent Files opened on `paths`: a directory becomes the
        /// folder shown; files open their folder with every one of them
        /// marked and the first selected (the main Files view's reveal).
        fn show_files_at(&self, paths: &[String]) {
            let wanted: Vec<String> = paths.iter().map(|p| crate::views::files::standardize(p)).collect();
            let Some(first) = wanted.first() else { return };
            let dir = if std::path::Path::new(first).is_dir() {
                first.clone()
            } else {
                std::path::Path::new(first)
                    .parent()
                    .map(|d| d.to_string_lossy().into_owned())
                    .unwrap_or_else(|| first.clone())
            };
            self.show_files();
            self.files_surface(|b| {
                b.cd(&dir);
                b.list.marked.clear();
                for (i, e) in b.list.rows.iter().enumerate() {
                    if wanted.iter().any(|w| w == &e.path) {
                        b.list.marked.insert(i);
                    }
                }
                if let Some(i) = b.list.rows.iter().position(|e| wanted.iter().any(|w| w == &e.path)) {
                    b.list.selection = i;
                }
            });
            // The list, not the filter field, has the keyboard: the
            // selection is what you act on (Return / Space / Cmd+C).
            if let (Some(w), Some(v)) = (
                self.files_window(),
                self.files_win.try_borrow().ok().and_then(|g| g.as_ref().and_then(|w| w.browser.view.clone())),
            ) {
                let list = v.list_view();
                let r: &objc2_app_kit::NSResponder = &list;
                w.makeFirstResponder(Some(r));
            }
        }

        fn hide_files(&self) {
            if let Ok(g) = self.files_win.try_borrow() {
                if let Some(p) = g.as_ref().and_then(|w| w.panel.as_ref()) {
                    p.orderOut(None);
                }
            }
            self.publish_state();
        }

        /// Open in the lightweight preview over the Recent window (Esc returns to
        /// it); only files the preview can't show go to their default app.
        fn files_open(&self, path: &str) {
            use crate::views::file_popup::{FilePopup, FilePopupRoute};
            if FilePopup::route(path) == FilePopupRoute::System {
                crate::platform::open_path(std::path::Path::new(path));
                self.hide_files();
                return;
            }
            let panel = self.files_win.try_borrow().ok().and_then(|g| g.as_ref().and_then(|w| w.panel.clone()));
            crate::views::file_popup::show(path, panel.as_deref().map(|p| &***p as &objc2_app_kit::NSWindow));
        }

        // -- Find Recent Files: the Files view's operations -----------------------

        fn files_window(&self) -> Option<objc2::rc::Retained<popup::PopupPanel>> {
            self.files_win.try_borrow().ok().and_then(|g| g.as_ref().and_then(|w| w.panel.clone()))
        }

        fn files_nav_panes(&self) -> Vec<crate::app::live_panes::LivePane> {
            self.files_surface(|b| b.view.as_ref().map(|v| v.nav_panes())).flatten().unwrap_or_default()
        }

        fn files_pane_id(&self) -> Option<String> {
            let w = self.files_window()?;
            let panes = self.files_nav_panes();
            crate::app::live_panes::current(&w, &panes).map(|p| p.id.clone())
        }

        fn files_sidebar(&self) -> Option<objc2::rc::Retained<crate::ui::sidebar::SidebarView>> {
            self.files_surface(|b| b.view.as_ref().map(|v| v.sidebar())).flatten()
        }

        /// Up / Down / Ctrl+N / Ctrl+P while the sidebar has the keyboard.
        fn files_sidebar_move(&self, d: i64) {
            let Some(bar) = self.files_sidebar() else { return };
            let (texts, cursor, _) = bar.vim_rows();
            if texts.is_empty() {
                return;
            }
            let to = (cursor as i64 + d).clamp(0, texts.len() as i64 - 1) as usize;
            bar.vim_move(to);
        }

        /// Keys while the Recent window is key: this window's extras (pane
        /// moves, sidebar, vim, Esc / Return / arrows, zoom), then the Finder
        /// keys shared with the main Files view (`FilesHost::files_finder_key`).
        fn files_key(&self, event: &NSEvent) -> bool {
            let key = self
                .files_win
                .try_borrow()
                .ok()
                .map(|g| g.as_ref().and_then(|w| w.panel.as_ref()).is_some_and(|p| p.isKeyWindow()))
                .unwrap_or(false);
            if !key {
                return false;
            }
            let k = popup::key_input_from_event(event);
            let (code, cmd, ctrl, shift, opt) = (k.key_code, k.cmd, k.ctrl, k.shift, k.opt);
            let plain = !cmd && !ctrl && !shift && !opt;
            let cmd_only = cmd && !shift && !opt && !ctrl;
            let (in_list, _) = self.files_focus();
            // Ctrl+H/J/K/L: move the focus between the sidebar, filter, list and preview.
            if ctrl && !cmd && !opt && !shift {
                use crate::panes::pane_geometry::PaneDir;
                let dir = match code {
                    4 => Some(PaneDir::Left),
                    38 => Some(PaneDir::Down),
                    40 => Some(PaneDir::Up),
                    37 => Some(PaneDir::Right),
                    _ => None,
                };
                if let Some(dir) = dir {
                    self.files_pane_move(dir);
                    return true;
                }
            }
            // The sidebar owns Up/Down/Ctrl+N/P/Return while it has the keyboard.
            if self.files_pane_id().as_deref() == Some("files-sidebar") {
                let down = (plain && code == popup::KEY_DOWN) || (ctrl && !cmd && !opt && code == 45);
                let up = (plain && code == popup::KEY_UP) || (ctrl && !cmd && !opt && code == 35);
                if down || up {
                    self.files_sidebar_move(if down { 1 } else { -1 });
                    return true;
                }
                if plain && matches!(code, popup::KEY_RETURN | 76) {
                    if let Some(bar) = self.files_sidebar() {
                        let (_, cursor, _) = bar.vim_rows();
                        bar.activate_row(cursor);
                    }
                    return true;
                }
            }
            if !cmd && self.files_vim_key(event) {
                return true;
            }
            match code {
                popup::KEY_ESC if plain => self.files_escape(),
                popup::KEY_RETURN | 76 if plain => {
                    self.files_open_selected();
                }
                popup::KEY_DOWN if plain => {
                    self.files_surface(|b| b.list.move_selection(1));
                }
                popup::KEY_UP if plain => {
                    self.files_surface(|b| b.list.move_selection(-1));
                }
                // Shift+Up / Down extend the selection (list focus; the filter keeps its own).
                popup::KEY_DOWN | popup::KEY_UP if shift && !cmd && !ctrl && !opt && in_list => {
                    let d: i64 = if code == popup::KEY_DOWN { 1 } else { -1 };
                    self.files_surface(|b| {
                        let to = (b.list.selection as i64 + d).clamp(0, b.list.rows.len() as i64 - 1).max(0) as usize;
                        b.list.extend_selection(to);
                    });
                }
                45 if ctrl && !cmd && !opt => {
                    self.files_surface(|b| b.list.move_selection(1));
                }
                35 if ctrl && !cmd && !opt => {
                    self.files_surface(|b| b.list.move_selection(-1));
                }
                0 if cmd_only && in_list => {
                    self.files_surface(|b| b.list.select_all());
                }
                // Text zoom
                24 if cmd_only || (cmd && shift && !opt && !ctrl) => self.files_text_zoom(1),
                27 if cmd_only => self.files_text_zoom(-1),
                _ => return self.files_finder_key(&k),
            }
            true
        }

        /// `VimKeys` on the sidebar / list / filter (rows only; `/` jumps to the filter).
        fn files_vim_key(&self, event: &NSEvent) -> bool {
            use crate::panes::vim_keys::{VimAction, VimContext, VimRows, VimTarget};
            let Some(w) = self.files_window() else { return false };
            let panes = self.files_nav_panes();
            let Some(cur) = crate::app::live_panes::current(&w, &panes) else { return false };
            let Some(fr) = w.firstResponder() else { return false };
            let pane_id = cur.id.clone();
            let query_empty = self.files_surface(|b| b.query.is_empty()).unwrap_or(true);
            // Esc in an empty filter closes the window (back to the palette), not just leaves the field.
            if event.keyCode() == popup::KEY_ESC && pane_id == "files-filter" && query_empty {
                return false;
            }
            let mut key = popup::key_input_from_event(event);
            if let Some(c) = event.characters().and_then(|s| s.to_string().chars().next()) {
                if !key.ctrl && !key.cmd {
                    key.chars = Some(c);
                }
            }
            let is_text_input = objc2_foundation::NSObjectProtocol::isKindOfClass(
                &*fr,
                <objc2_app_kit::NSTextView as objc2::ClassType>::class(),
            ) && fr
                .downcast_ref::<objc2_app_kit::NSTextView>()
                .map(|t| t.isEditable())
                .unwrap_or(true);
            let mut target: Option<VimTarget> = if is_text_input {
                None
            } else {
                match pane_id.as_str() {
                    "files-sidebar" => self.files_sidebar().map(|bar| {
                        let (texts, cursor, page) = bar.vim_rows();
                        VimTarget::Rows(VimRows { texts, cursor, page })
                    }),
                    "files-list" => self
                        .files_surface(|b| {
                            let texts: Vec<String> = b.visible_rows().iter().map(|e| e.name.clone()).collect();
                            VimRows { texts, cursor: b.selection(), page: 20 }
                        })
                        .map(VimTarget::Rows),
                    _ => None,
                }
            };
            let before = target.clone();
            let has_insert = pane_id == "files-list";
            let has_normal = pane_id == "files-filter";
            let clip = crate::platform::get_text();
            let action = {
                let mut ctx = VimContext {
                    pane_id: &pane_id,
                    target: target.as_mut(),
                    is_text_input,
                    owns_vim: false,
                    has_insert,
                    has_normal,
                    clipboard: clip.as_deref(),
                };
                self.files_vim.borrow_mut().handle(key, &mut ctx)
            };
            if let (Some(VimTarget::Rows(b)), Some(VimTarget::Rows(a))) = (&before, &target) {
                if a.cursor != b.cursor {
                    match pane_id.as_str() {
                        "files-sidebar" => {
                            if let Some(bar) = self.files_sidebar() {
                                bar.vim_move(a.cursor);
                            }
                        }
                        _ => {
                            self.files_surface(|br| {
                                br.list.marked.clear();
                                br.list.selection = a.cursor;
                            });
                        }
                    }
                }
            }
            match action {
                VimAction::Pass => false,
                VimAction::EnterInsert | VimAction::OpenSearch { .. } => {
                    self.files_focus_filter(false);
                    true
                }
                VimAction::ExitToNormal => {
                    self.files_surface(|b| {
                        if !b.query.is_empty() {
                            b.set_query("");
                        }
                        if let Some(v) = &b.view {
                            v.search_field().setStringValue(&objc2_foundation::NSString::from_str(""));
                        }
                    });
                    self.focus_files_list();
                    true
                }
                _ => true,
            }
        }

        fn files_pane_move(&self, dir: crate::panes::pane_geometry::PaneDir) {
            let Some(panel) = self.files_window() else { return };
            let panes = self.files_nav_panes();
            self.files_nav.borrow_mut().move_dir(&panel, &panes, dir, &|_| false);
        }

        fn show_paths(&self) {
            let Some(mtm) = MainThreadMarker::new() else { return };
            self.refresh_config();
            let mut win = self
                .paths_win
                .borrow_mut()
                .take()
                .unwrap_or_else(|| PathsWindow::new(PathsReturn::File));
            win.set_items(Self::shelf_entries());
            win.show();
            win.build(mtm);
            if win.on_event.is_none() {
                win.set_on_event(Rc::new(|ev| with_ui(|ui| ui.paths_event(ev))));
            }
            win.show_window(mtm);
            // Click-off hides the window, unless the preview popup took key.
            if let Some(panel) = &win.window {
                panel.set_on_resign(Box::new(|| {
                    defer(|ui| {
                        if crate::views::file_popup::is_live() {
                            return;
                        }
                        ui.paths_surface(|w| w.hide_window());
                        ui.publish_state();
                        ui.surface_closed("paths", false);
                    });
                }));
            }
            *self.paths_win.borrow_mut() = Some(win);
            self.publish_state();
        }

        fn paths_surface<R>(&self, f: impl FnOnce(&mut PathsWindow) -> R) -> Option<R> {
            let mut g = self.paths_win.try_borrow_mut().ok()?;
            let w = g.as_mut()?;
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
                PathsReturn::Path => copy_text(&text),
                PathsReturn::File => {
                    copy_files_toast(&files);
                }
                PathsReturn::Open => {
                    for p in open {
                        crate::platform::open_path(std::path::Path::new(&p));
                    }
                }
            }
            self.paths_surface(|w| w.hide_window());
            self.publish_state();
        }

        fn paths_event(&self, ev: PathsEvent) {
            match ev {
                PathsEvent::Query(q) => {
                    self.paths_surface(|w| w.set_query(&q));
                }
                PathsEvent::Move(d) => {
                    self.paths_surface(|w| w.move_by(d));
                }
                PathsEvent::Escape => {
                    self.paths_surface(|w| w.hide_window());
                    self.publish_state();
                    self.surface_closed("paths", true);
                }
                PathsEvent::Enter => self.paths_return(),
                PathsEvent::Click { row, double } => {
                    self.paths_surface(|w| {
                        w.selection = row;
                        w.last_clicked.replace(row);
                    });
                    if double {
                        if let Some(Some(path)) = self.paths_surface(|w| w.rows.get(row).map(|r| r.path.clone())) {
                            crate::views::file_popup::show(&path, None);
                        }
                    }
                }
                PathsEvent::RowMenu { row, x, y } => {
                    let Some(Some(path)) = self.paths_surface(|w| {
                        w.selection = row;
                        w.rows.get(row).map(|r| r.path.clone())
                    }) else {
                        return;
                    };
                    let (po, pr, pcf, pcp) = (path.clone(), path.clone(), path.clone(), path.clone());
                    // Rename / compress / trash need the main app's Files
                    // backends; the agent offers the read-only rows.
                    let actions = crate::views::files::FileMenuActions {
                        open: Some(Box::new(move || {
                            crate::platform::open_path(std::path::Path::new(&po));
                        })),
                        reveal: Some(Box::new(move || crate::app::hooks::reveal_in_files(&[pr]))),
                        copy_file: Some(Box::new(move || {
                            copy_files_toast(&[pcf]);
                        })),
                        copy_path: Some(Box::new(move || copy_text(&pcp))),
                        ..Default::default()
                    };
                    let entries = crate::views::files::file_context_menu(&path, actions);
                    crate::ui::menu_kit::popup(entries, None, x, y);
                }
            }
        }

        /// Keys for the paths shelf while it has the keyboard (copy / open /
        /// reveal / forget / Quick Look); Return, Esc and arrows ride the
        /// field delegate's `PathsEvent`s.
        fn paths_key(&self, event: &NSEvent) -> bool {
            use crate::views::paths::{key_action, PathsKey};
            let info = self.paths_win.try_borrow().ok().and_then(|g| {
                let w = g.as_ref()?;
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
                PathsKey::Hide | PathsKey::Move(..) | PathsKey::RunDefault => return false,
                PathsKey::CopyFiles => {
                    copy_files_toast(&paths);
                }
                PathsKey::CopyPaths => {
                    if !paths.is_empty() {
                        copy_text(&paths.join("\n"));
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
                        crate::platform::open_path(std::path::Path::new(p));
                    }
                }
                PathsKey::Reveal => crate::app::hooks::reveal_in_files(&paths),
                // Inline rename needs the main app's Files backends.
                PathsKey::BeginRename => return false,
                PathsKey::Forget => {
                    let store = path_shelf::default_store();
                    let mut items = path_shelf::pure_load(&store, path_shelf::MAX_LIMIT);
                    items.retain(|i| !paths.contains(&i.path));
                    let _ = path_shelf::pure_save(&store, &items);
                    self.paths_surface(|w| {
                        w.set_items(Self::shelf_entries());
                        w.reload(true);
                    });
                }
            }
            true
        }

        // -- keys ---------------------------------------------------------------

        fn route_key_event(&self, event: &NSEvent) -> bool {
            // Ctrl+W deletes the previous word; Cmd+C/V/X/A/Z reach the field
            // editor of a non-activating popup (rule 1).
            if crate::ui::popup_keys::kill_word_in_text_input(event) {
                return true;
            }
            if crate::ui::popup_keys::cmd_edit_in_popup(event) {
                return true;
            }
            if crate::ui::inline_rename::active() || crate::ui::multi_picker::appkit::is_open() {
                return false;
            }
            self.route_overlay_key(event)
        }

        /// Keys for the palette (and the paths shelf); only navigation /
        /// accept keys are consumed so typing still reaches the field.
        fn route_overlay_key(&self, event: &NSEvent) -> bool {
            use objc2_app_kit::NSEventModifierFlags as F;
            if let Some(mtm) = MainThreadMarker::new() {
                // The Create File panel owns its keys (Tab flips name <-> paste).
                if event.window(mtm).is_some_and(|w| w.title().to_string() == "filefast") {
                    return false;
                }
            }
            if self.files_key(event) {
                return true;
            }
            if self.paths_key(event) {
                return true;
            }
            enum Out {
                NotHandled,
                Consumed,
                Pick(PaletteAction),
                Close,
            }
            let code = event.keyCode();
            let mods = event.modifierFlags();
            let (cmd, ctrl, shift, opt) = (
                mods.contains(F::Command),
                mods.contains(F::Control),
                mods.contains(F::Shift),
                mods.contains(F::Option),
            );
            let out = {
                let Ok(mut g) = self.palette.try_borrow_mut() else { return false };
                match g.as_mut() {
                    Some(p) if p.has_key() => {
                        use crate::views::switcher::{palette_key, PaletteKey};
                        match palette_key(code, ctrl, shift, cmd, opt, p.live_query().trim().is_empty()) {
                            PaletteKey::Close => Out::Close,
                            PaletteKey::ClearQuery => {
                                p.clear_query();
                                Out::Consumed
                            }
                            PaletteKey::Accept => match p.key(code, ctrl, shift, cmd, opt) {
                                Some(a) => Out::Pick(a),
                                None => Out::Consumed,
                            },
                            PaletteKey::Navigate => {
                                p.key(code, ctrl, shift, cmd, opt);
                                Out::Consumed
                            }
                            PaletteKey::Ignore => Out::NotHandled,
                        }
                    }
                    _ => Out::NotHandled,
                }
            };
            match out {
                Out::NotHandled => false,
                Out::Consumed => true,
                Out::Pick(action) => {
                    // Accepted from the key monitor: decide focus while the
                    // palette is still up, open the pick, and only then close
                    // the palette (like a click does). Hiding first handed the
                    // keyboard back to the app underneath while a tool panel
                    // was still being built (its first open), and the first
                    // keystrokes typed after Return landed in that app.
                    self.pick_focus(&action);
                    self.run_pick(action);
                    self.hide_palette_panel();
                    true
                }
                Out::Close => {
                    self.cancel_palette();
                    true
                }
            }
        }
    }

    /// Find Recent Files gets every file action from the shared implementation.
    impl FilesHost for QuickUi {
        fn files_surface<R>(&self, f: impl FnOnce(&mut FileBrowser) -> R) -> Option<R> {
            let mut g = self.files_win.try_borrow_mut().ok()?;
            let w = g.as_mut()?;
            let r = f(&mut w.browser);
            w.browser.sync_view();
            Some(r)
        }
        fn files_ns_window(&self) -> Option<objc2::rc::Retained<objc2_app_kit::NSWindow>> {
            self.files_window().map(|p| objc2::rc::Retained::into_super(objc2::rc::Retained::into_super(p)))
        }
        fn focus_files_list(&self) {
            let list = self.files_surface(|b| b.view.as_ref().map(|v| v.list_view())).flatten();
            if let (Some(l), Some(w)) = (list, self.files_window()) {
                let r: &objc2_app_kit::NSResponder = &l;
                w.makeFirstResponder(Some(r));
            }
        }
        fn files_open_file(&self, path: &str) {
            self.files_open(path)
        }
        fn files_focus(&self) -> (bool, bool) {
            let list = self.files_surface(|b| b.view.as_ref().map(|v| v.list_view())).flatten();
            let Some(w) = self.files_window() else { return (false, false) };
            let Some(fr) = w.firstResponder() else { return (false, false) };
            let in_list = list.is_some_and(|l| {
                std::ptr::eq(
                    &*fr as *const objc2_app_kit::NSResponder as *const u8,
                    &*l as *const crate::views::files_view::FileListView as *const u8,
                )
            });
            let text = objc2_foundation::NSObjectProtocol::isKindOfClass(&*fr, <objc2_app_kit::NSTextView as objc2::ClassType>::class());
            (in_list, text)
        }
        fn files_focus_filter(&self, select_all: bool) {
            let field = self.files_surface(|b| b.view.as_ref().map(|v| v.search_field())).flatten();
            let (Some(f), Some(w)) = (field, self.files_window()) else { return };
            let r: &objc2_app_kit::NSResponder = &f;
            w.makeFirstResponder(Some(r));
            if select_all {
                if let Some(ed) = f.currentEditor() {
                    let nil: Option<&objc2::runtime::AnyObject> = None;
                    let _: () = unsafe { objc2::msg_send![&*ed, selectAll: nil] };
                }
            }
        }
        /// Focus the filter and let the field take the typed character.
        fn files_type_ahead(&self, _c: char) -> bool {
            self.files_focus_filter(false);
            false
        }
        /// Esc clears the filter first, then closes the window.
        fn files_escape(&self) {
            let had_query = self
                .files_surface(|b| {
                    if b.query.is_empty() {
                        return false;
                    }
                    b.set_query("");
                    if let Some(v) = &b.view {
                        v.search_field().setStringValue(&objc2_foundation::NSString::from_str(""));
                    }
                    true
                })
                .unwrap_or(false);
            if had_query {
                return;
            }
            self.hide_files();
            self.surface_closed("files", true);
        }
        fn files_screenshot(&self) -> Option<Arc<Mutex<ScreenshotController>>> {
            Some(self.shot.clone())
        }
        fn with_host(f: impl FnOnce(&Self)) {
            with_ui(|ui| f(ui))
        }
        fn later(f: impl FnOnce(&Self) + Send + 'static) {
            defer(move |ui| f(ui))
        }
    }

    /// True when the app's current event is an Esc key-down from the last half second.
    fn esc_just_pressed(mtm: MainThreadMarker) -> bool {
        let Some(event) = NSApplication::sharedApplication(mtm).currentEvent() else { return false };
        let uptime = objc2_foundation::NSProcessInfo::processInfo().systemUptime();
        event.r#type() == objc2_app_kit::NSEventType::KeyDown
            && event.keyCode() == popup::KEY_ESC
            && uptime - event.timestamp() < 0.5
    }

    /// One 30 ms poll turn; reschedules only while a tool panel is up.
    fn schedule_poll() {
        let Ok(when) = dispatch2::DispatchTime::try_from(Duration::from_millis(30)) else { return };
        let _ = dispatch2::DispatchQueue::main().after(when, || {
            let mut keep = false;
            with_ui(|ui| {
                keep = ui.poll_tick();
                if !keep {
                    ui.polling.set(false);
                }
            });
            if keep {
                schedule_poll();
            }
        });
    }

    /// `switchWorkspace(cell:)` — `aerospace workspace <id>` off-main.
    fn switch_workspace(id: &str) {
        let id = id.to_string();
        std::thread::spawn(move || {
            let ipc = crate::app::hotkey::AeroIpc::discover();
            let _ = crate::app::hotkey::AeroCall::call(&ipc, &["workspace".to_string(), id]);
        });
    }

    fn copy_text(text: &str) {
        let _ = crate::platform::set_text(text);
    }

    fn copy_files_toast(paths: &[String]) -> bool {
        crate::app::hooks::copy_files_toast(paths)
    }

    /// Run the agent: bind the socket, build the UI, run the main loop.
    pub fn run(paths: &Paths, action: StartAction, lock: DaemonLock) {
        let _lock = lock; // held for the whole process
        let sock = paths.quick_socket_path();
        let mtm = MainThreadMarker::new().expect("main thread");
        let app = NSApplication::sharedApplication(mtm);
        // No Dock icon, no menu-bar presence; only a minimal main menu so the
        // Edit shortcuts work if the agent is ever the active app.
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        crate::app::menu::install_main_menu(mtm, None);

        let text = crate::app::config::read_config_text_pure().unwrap_or_default();
        let sections = crate::app::sections::parse_sections(&text);
        crate::ui::theme::install_theme(&sections.get("theme").cloned().unwrap_or_default(), sections.clone());
        // `shared` wires the weak self-ref the overlay's key monitor and mouse
        // callbacks need; a bare `Arc::new` gives an overlay that ignores input.
        let shot = ScreenshotController::shared(QuickUi::shot_config(&text));

        // "Reveal in Files" (screenshot Recents ▸ Open Folder / Reveal, the
        // paths shelf) opens the agent's own Find Recent Files panel on the
        // screen in use — never the main app's window (AeroSpace pins that
        // to workspace 2, and a cold start of it is slow).
        crate::app::hooks::set_reveal_hook(move |p: &[String]| {
            let p = p.to_vec();
            defer(move |ui| ui.show_files_at(&p));
        });

        let shared = QuickShared::new();
        let ui = Rc::new(QuickUi {
            paths: paths.clone(),
            shared: shared.clone(),
            server: RefCell::new(None),
            palette: RefCell::new(None),
            tools: RefCell::new(HashMap::new()),
            opened_at: RefCell::new(HashMap::new()),
            paths_win: RefCell::new(None),
            files_win: RefCell::new(None),
            files_nav: RefCell::new(crate::app::live_panes::LiveNav::default()),
            files_vim: RefCell::new(crate::panes::vim_keys::VimKeys::default()),
            shot: shot.clone(),
            palette_return: Cell::new(None),
            pick_return: Cell::new(None),
            polling: Cell::new(false),
            monitor: RefCell::new(None),
            back_to: RefCell::new(None),
        });
        QUICK.with(|q| *q.borrow_mut() = Some(ui.clone()));

        let busy_shot = shot.clone();
        let handler = Arc::new(QuickHandler {
            sink: Box::new(|cmd| {
                // One main-queue block per request: wake-on-enqueue, no timer.
                crate::platform::run_on_main(move || with_ui(|ui| ui.run(cmd)));
            }),
            shared: shared.clone(),
            screenshot_busy: Box::new(move || busy_shot.try_lock().map(|c| !c.is_idle()).unwrap_or(true)),
        });
        match CommandServer::start(&sock, handler) {
            Ok(s) => *ui.server.borrow_mut() = Some(s),
            Err(e) => {
                log(&format!("cannot bind {sock}: {e}"));
                std::process::exit(1);
            }
        }

        let weak = Rc::downgrade(&ui);
        let monitor = popup::install_local_monitor(mtm, move |event| {
            weak.upgrade().map(|ui| ui.route_key_event(event)).unwrap_or(false)
        });
        *ui.monitor.borrow_mut() = Some(monitor);

        // Palette prewarm (hidden), then SCK + OCR warm-up off the critical path.
        // A cold start *for* a capture skips it: the real capture warms SCK
        // itself, and a second request in flight only competes with it.
        if action != StartAction::Screenshot {
            crate::views::screenshot::prewarm_capture();
        }
        let _ = dispatch2::DispatchQueue::main().after(dispatch2::DispatchTime::NOW.time(300_000_000), || {
            with_ui(|ui| ui.prewarm_palette());
        });
        let _ = dispatch2::DispatchQueue::main().after(dispatch2::DispatchTime::NOW.time(1_500_000_000), move || {
            let plan = shot.lock().unwrap().prewarm_plan();
            if let Some(cfg) = plan {
                crate::engines::screenshot_text::ShotOCR::warm_up_in_background(cfg);
            }
        });

        log(&format!("quick agent: pid {} socket {sock}", std::process::id()));
        app_log_start_action(&action);
        match action {
            StartAction::None => {}
            StartAction::Show => crate::platform::run_on_main(|| with_ui(|ui| ui.run(QuickCmd::Verb(LaunchVerb::Show)))),
            StartAction::Screenshot => crate::platform::run_on_main(|| {
                with_ui(|ui| ui.run(QuickCmd::Screenshot { words: Vec::new(), reply: None }))
            }),
        }
        app.run();
    }

    fn app_log_start_action(a: &StartAction) {
        if *a != StartAction::None {
            log(&format!("start action {a:?}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_verbs_parse() {
        assert_eq!(launch_verb("toggle"), Some(LaunchVerb::Toggle));
        assert_eq!(launch_verb(" show\n"), Some(LaunchVerb::Show));
        assert_eq!(launch_verb("hide"), Some(LaunchVerb::Hide));
        assert_eq!(launch_verb("quit"), Some(LaunchVerb::Quit));
        assert_eq!(launch_verb("files"), None, "views belong to the main app");
        assert_eq!(launch_verb(""), None);
    }

    #[test]
    fn start_actions_parse() {
        assert_eq!(start_action(None), StartAction::None);
        assert_eq!(start_action(Some("show")), StartAction::Show);
        assert_eq!(start_action(Some("screenshot")), StartAction::Screenshot);
        assert_eq!(start_action(Some("bogus")), StartAction::None);
    }

    #[test]
    fn screenshot_words_follow_the_main_daemons_semantics() {
        let (w, wants) = screenshot_words("");
        assert!(w.is_empty() && !wants, "bare Hyper+X: interactive, no reply");
        let (w, wants) = screenshot_words("--raw");
        assert_eq!(w, vec!["--raw".to_string()]);
        assert!(wants, "--raw blocks on the reply bytes");
        let (w, _) = screenshot_words("-r\t\t-s");
        assert_eq!(w, vec!["-r".to_string(), "-s".to_string()], "empty words are dropped");
    }

    #[test]
    fn picks_route_local_or_forward() {
        assert_eq!(route_command("paths"), PickRoute::Paths);
        assert_eq!(route_command("filefast"), PickRoute::Tool("filefast"));
        assert_eq!(route_command("text-table"), PickRoute::Tool("text-table"));
        assert_eq!(route_command("prettyprint"), PickRoute::Tool("prettyprint"));
        assert_eq!(route_command("find-window"), PickRoute::FindWindow);
        assert_eq!(route_command("screenshot"), PickRoute::Screenshot);
        // Everything else is the main app's.
        assert_eq!(route_command("window"), PickRoute::Forward("window".into()));
        assert_eq!(route_command("files"), PickRoute::Forward("files".into()));
        assert_eq!(route_command("jira-config"), PickRoute::Forward("jira-dashboard".into()));
        assert_eq!(route_command("health"), PickRoute::Forward("health".into()));
        assert!(pick_keeps_app("window") && pick_keeps_app("jira-config"));
        assert!(!pick_keeps_app("paths"));
    }

    #[test]
    fn cold_start_modes_cover_forwarded_messages() {
        assert_eq!(cold_start_mode("window"), "window");
        assert_eq!(cold_start_mode("jira-dashboard"), "jira");
        assert_eq!(cold_start_mode("health"), "window");
        assert_eq!(cold_start_mode("reveal\t/a"), "window");
    }

    const CFG: &str = r#"
[app]
palette-first = "paths, filefast, text-table, prettyprint"
palette = "paths, filefast, text-table, prettyprint, find-window"

[paths]
enabled = true
  label = "Find Recent Files"

[text-table]
enabled = true
label = "Text to Table"

[filefast]
enabled = true
label = "Create File"

[find-window]
enabled = true
label = "Find Window PID"

[prettyprint]
enabled = true
label = "Pretty Print"

[hidden]
enabled = true
in-palette = false
label = "Hidden"

[ai]
enabled = true
label = "AI"
"#;

    #[test]
    fn palette_rows_follow_app_palette_in_order() {
        let rows = palette_rows(CFG);
        assert_eq!(
            rows,
            vec![
                ("paths".to_string(), "Find Recent Files".to_string()),
                ("filefast".to_string(), "Create File".to_string()),
                ("text-table".to_string(), "Text to Table".to_string()),
                ("prettyprint".to_string(), "Pretty Print".to_string()),
                ("find-window".to_string(), "Find Window PID".to_string()),
            ]
        );
    }

    #[test]
    fn empty_palette_setting_lists_every_in_palette_command() {
        let text = CFG.replace("palette = \"paths, filefast, text-table, prettyprint, find-window\"", "palette = \"\"");
        let ids: Vec<String> = palette_rows(&text).into_iter().map(|r| r.0).collect();
        assert!(ids.starts_with(&["paths".to_string(), "filefast".to_string(), "text-table".to_string(), "prettyprint".to_string()]));
        assert!(ids.contains(&"ai".to_string()) && ids.contains(&"window".to_string()));
        assert!(!ids.contains(&"hidden".to_string()), "in-palette = false stays out");
    }

    #[test]
    fn handler_maps_launch_messages_and_reports_state() {
        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let s2 = seen.clone();
        let shared = QuickShared::new();
        *shared.state.lock().unwrap() = json!({ "palette": true });
        let h = QuickHandler {
            sink: Box::new(move |c| {
                if let QuickCmd::Verb(v) = c {
                    s2.lock().unwrap().push(format!("{v:?}"));
                }
            }),
            shared,
            screenshot_busy: Box::new(|| true),
        };
        h.launch("toggle");
        h.launch("files");
        h.launch("quit");
        assert_eq!(*seen.lock().unwrap(), vec!["Toggle".to_string(), "Quit".to_string()]);
        let st = h.state_json();
        assert_eq!(st["agent"], json!("quick"));
        assert_eq!(st["pid"], json!(std::process::id()));
        assert_eq!(st["palette"], json!(true));
        assert_eq!(st["screenshotBusy"], json!(true));
        assert!(h.do_action("open:files").unwrap()["error"].is_string());
    }

    #[test]
    fn tools_state_lists_shown_flags() {
        let mut m = HashMap::new();
        m.insert("prettyprint".to_string(), true);
        m.insert("filefast".to_string(), false);
        assert_eq!(
            tools_state(&m),
            json!({ "filefast": { "shown": false }, "prettyprint": { "shown": true } })
        );
    }

    #[test]
    #[cfg(unix)]
    fn socket_round_trip_serves_every_verb() {
        use crate::app::cli::{send_launch_message, send_request};
        use crate::app::socket::CommandServer;
        let dir = std::env::temp_dir().join(format!("ks-quick-rt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sock = format!("{}/q.sock", dir.display());
        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let s2 = seen.clone();
        let shared = QuickShared::new();
        let handler = Arc::new(QuickHandler {
            sink: Box::new(move |c| match c {
                QuickCmd::Verb(v) => s2.lock().unwrap().push(format!("{v:?}")),
                QuickCmd::Screenshot { words, reply } => {
                    s2.lock().unwrap().push(format!("shot {words:?} reply={}", reply.is_some()));
                    if let Some(r) = reply {
                        r.send_bytes(b"PNGDATA");
                    }
                }
            }),
            shared,
            screenshot_busy: Box::new(|| false),
        });
        let mut server = CommandServer::start(&sock, handler).expect("bind");
        let wait = std::time::Duration::from_secs(5);
        // ping closes with no reply; toggle / show are launch messages.
        assert_eq!(send_request(&sock, "ping", wait).unwrap(), b"");
        assert!(send_launch_message(&sock, "toggle"));
        assert!(send_launch_message(&sock, "show"));
        // state answers one JSON line.
        let st = send_request(&sock, "state", wait).unwrap();
        let v: Value = serde_json::from_slice(&st).unwrap();
        assert_eq!(v["agent"], json!("quick"));
        // A plain screenshot gets no reply bytes; --raw blocks for them.
        assert_eq!(send_request(&sock, "screenshot", wait).unwrap(), b"");
        assert_eq!(send_request(&sock, "screenshot\t--raw", wait).unwrap(), b"PNGDATA");
        // Main-daemon verbs are refused, not silently dropped.
        let r = send_request(&sock, "pane-shot\tx", wait).unwrap();
        assert!(String::from_utf8_lossy(&r).starts_with("error:") || r.is_empty());
        let log = seen.lock().unwrap().clone();
        assert_eq!(log[0], "Toggle");
        assert_eq!(log[1], "Show");
        assert!(log.contains(&"shot [] reply=false".to_string()), "{log:?}");
        assert!(log.contains(&"shot [\"--raw\"] reply=true".to_string()), "{log:?}");
        server.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[cfg(unix)]
    fn quick_socket_and_lock_are_separate_from_the_main_daemons() {
        use crate::app::socket::DaemonLock;
        let dir = std::env::temp_dir().join(format!("ks-quick-lock-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tmp = format!("{}/", dir.display());
        let p = crate::app::paths::Paths::resolve_at("/x/kitchen-sink", None, Some(&tmp), Some("/h"));
        // Two locks (main + quick) coexist; each excludes only a second of itself.
        let main_lock = DaemonLock::try_acquire(format!("{}.lock", p.notes_socket_path())).expect("main lock");
        let quick_lock = DaemonLock::try_acquire(p.quick_lock_path()).expect("quick lock beside the main one");
        assert!(DaemonLock::try_acquire(p.quick_lock_path()).is_none(), "a second agent is refused");
        drop((main_lock, quick_lock));
        // The default $TMPDIR path fits sockaddr_un's 103 bytes.
        let long = crate::app::paths::Paths::resolve_at("/x/kitchen-sink", None, Some("/var/folders/zz/abcdefghijklmnopqrstuvwxyz012345/T/"), Some("/h"));
        assert!(long.quick_socket_path().len() < 104, "{}", long.quick_socket_path());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
