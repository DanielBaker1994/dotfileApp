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
        "terminal" => "term".to_string(),
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
        "term" | "terminal" => "terminal",
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
    use crate::app::host::{label_of, parse_sections, parse_sections_ordered, section_flag, tri_pure};
    use crate::app::host::COMMAND_SECTION_SKIPS;
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
                #[cfg(not(target_os = "macos"))]
                let ok = false;
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
    use crate::views::paths::{PathsEvent, PathsWindow, ReturnAction as PathsReturn};
    use crate::views::screenshot::{ScreenshotConfig, ScreenshotController};
    use crate::views::switcher::{gather_workspaces, PaletteAction, PalettePanel, SwitchFrame, WorkspaceRow};
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

    /// Last `gather_workspaces()` result so the palette opens instantly.
    static WORKSPACE_CACHE: Mutex<Vec<WorkspaceRow>> = Mutex::new(Vec::new());

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

    /// Defer `f` to a fresh main-queue turn (out of an AppKit callback).
    fn defer(f: impl FnOnce(&Rc<QuickUi>) + Send + 'static) {
        crate::platform::run_on_main(move || with_ui(f));
    }

    struct QuickUi {
        paths: Paths,
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
    }

    impl QuickUi {
        // -- config ---------------------------------------------------------

        /// Re-read commands.toml (pure scan, no helper) and re-install the
        /// theme, so edits apply on the next press without a restart.
        fn refresh_config(&self) -> String {
            let text = crate::app::config::read_config_text_pure().unwrap_or_default();
            let sections = crate::app::host::parse_sections(&text);
            crate::ui::theme::install_theme(
                &sections.get("theme").cloned().unwrap_or_default(),
                sections.clone(),
            );
            text
        }

        fn app_settings(text: &str) -> crate::app::config::AppSettings {
            let sections = crate::app::host::parse_sections(text);
            let mut s = crate::app::config::AppSettings::default();
            if let Some(f) = sections.get("app").and_then(|a| a.get("terminal-font")) {
                if !f.trim().is_empty() {
                    s.terminal_font = f.trim().to_string();
                }
            }
            s
        }

        fn shot_config(text: &str) -> ScreenshotConfig {
            let sections = crate::app::host::parse_sections(text);
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
            let workspaces = WORKSPACE_CACHE.lock().map(|w| w.clone()).unwrap_or_default();
            panel.set_rows(palette_rows(&text), workspaces);
            std::thread::spawn(|| {
                let fresh = gather_workspaces();
                if let Ok(mut c) = WORKSPACE_CACHE.lock() {
                    *c = fresh.clone();
                }
                defer(move |ui| ui.refresh_palette_workspaces(fresh));
            });
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
                    PickRoute::Paths => self.show_paths(),
                    PickRoute::Tool(name) => {
                        if let Some(kind) = ToolKind::from_name(name) {
                            self.open_tool(kind);
                        }
                    }
                    PickRoute::FindWindow => {
                        let text = self.refresh_config();
                        crate::app::host::run_find_window(
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
                    panel.poll();
                    any |= panel.is_shown();
                }
            } else {
                any = true;
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
                        reveal: Some(Box::new(move || crate::app::host::reveal_in_files(&[pr]))),
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
                PathsKey::Reveal => crate::app::host::reveal_in_files(&paths),
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
                        if code == popup::KEY_ESC {
                            if p.live_query().trim().is_empty() {
                                Out::Close
                            } else {
                                // A live query clears instead of hiding.
                                p.clear_query();
                                Out::Consumed
                            }
                        } else if code == popup::KEY_RETURN
                            || code == 76
                            || (cmd && !ctrl && !opt && !shift && crate::views::switcher::palette_digit(code).is_some())
                        {
                            // Return, or Cmd+1..9 (run the Nth row).
                            match p.key(code, ctrl, shift, cmd, opt) {
                                Some(a) => Out::Pick(a),
                                None => Out::Consumed,
                            }
                        } else if cmd && !ctrl && !opt && !shift && matches!(code, popup::KEY_DOWN | popup::KEY_UP) {
                            p.key(code, ctrl, shift, cmd, opt);
                            Out::Consumed
                        } else if matches!(code, popup::KEY_DOWN | popup::KEY_UP | popup::KEY_TAB)
                            || (matches!(code, 45 | 35) && ctrl)
                            || (matches!(code, 123 | 124) && p.live_query().trim().is_empty())
                        {
                            p.key(code, ctrl, shift, cmd, opt);
                            Out::Consumed
                        } else {
                            Out::NotHandled
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
                    // palette is still up, then close it like a click does.
                    self.pick_focus(&action);
                    self.hide_palette_panel();
                    self.run_pick(action);
                    true
                }
                Out::Close => {
                    self.cancel_palette();
                    true
                }
            }
        }
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
        crate::app::host::copy_files_toast(paths)
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
        let sections = crate::app::host::parse_sections(&text);
        crate::ui::theme::install_theme(&sections.get("theme").cloned().unwrap_or_default(), sections.clone());
        let mut shot = ScreenshotController::new();
        shot.set_config(QuickUi::shot_config(&text));
        let shot = Arc::new(Mutex::new(shot));

        // "Reveal in Files" has no Files view here: ask the main app.
        {
            let (sock, script) = (paths.notes_socket_path(), format!("{}/bin/kitchen_sink.sh", paths.asset_dir()));
            crate::app::host::set_reveal_hook(move |p: &[String]| {
                let msg = std::iter::once("reveal".to_string()).chain(p.iter().cloned()).collect::<Vec<_>>().join("\t");
                forward_to_main(&sock, &script, &msg);
            });
        }

        let shared = QuickShared::new();
        let ui = Rc::new(QuickUi {
            paths: paths.clone(),
            shared: shared.clone(),
            server: RefCell::new(None),
            palette: RefCell::new(None),
            tools: RefCell::new(HashMap::new()),
            opened_at: RefCell::new(HashMap::new()),
            paths_win: RefCell::new(None),
            shot: shot.clone(),
            palette_return: Cell::new(None),
            pick_return: Cell::new(None),
            polling: Cell::new(false),
            monitor: RefCell::new(None),
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
        let _ = dispatch2::DispatchQueue::main().after(dispatch2::DispatchTime::NOW.time(300_000_000), || {
            with_ui(|ui| ui.prewarm_palette());
        });
        let _ = dispatch2::DispatchQueue::main().after(dispatch2::DispatchTime::NOW.time(1_500_000_000), move || {
            let plan = shot.lock().unwrap().prewarm_plan();
            if let Some(cfg) = plan {
                let _ = crate::engines::screenshot_text::ShotOCR::warm_up(&cfg);
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
        assert_eq!(route_command("terminal"), PickRoute::Forward("term".into()));
        assert_eq!(route_command("health"), PickRoute::Forward("health".into()));
        assert!(pick_keeps_app("window") && pick_keeps_app("jira-config"));
        assert!(!pick_keeps_app("paths") && !pick_keeps_app("terminal"));
    }

    #[test]
    fn cold_start_modes_cover_forwarded_messages() {
        assert_eq!(cold_start_mode("window"), "window");
        assert_eq!(cold_start_mode("jira-dashboard"), "jira");
        assert_eq!(cold_start_mode("term"), "terminal");
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
