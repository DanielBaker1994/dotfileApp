//! The egui app: `KsApp` mirrors the AppKit `daemon_ui`'s queue-draining and
//! key-routing, but renders with egui instead of AppKit. It installs its queues
//! on the controller exactly like `daemon_ui::install_daemon_ui` does, so the
//! socket thread's `UiCommand`s and main-thread jobs flow into `update()`.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use crate::app::host::{MainQueue, PathsCmd, SwitcherController, UiCommand, UiQueue, DEFAULT_VIEW};
use crate::app::registry::SlotView;
use crate::ui::popup::{self, KeyAction, PrefixOutcome, PrefixState};
use crate::ui::theme::{section_colors, PopupColors};
use crate::views::paths::ReturnAction;
use crate::views::tools::ToolKind;

use super::ai::AiPane;
use super::compare::ComparePane;
use super::confluence::ConfluencePane;
use super::files::FilesPane;
use super::jira::JiraPane;
use super::keys::key_input;
use super::notes::NotesPane;
use super::paths::PathsPane;
use super::switcher::SwitcherOverlay;
use super::terminal::TerminalView;
use super::theme::c;
use super::tools::ToolsPane;

/// The shared window's default size, used at launch and by `do:reset-size`.
const DEFAULT_WINDOW_SIZE: [f32; 2] = [1000.0, 700.0];

/// How long a toast pill stays up, and how long its final fade lasts.
const TOAST_TTL: Duration = Duration::from_millis(2500);
const TOAST_FADE: Duration = Duration::from_millis(500);

/// The views the sidebar lists (header nav order, minus the unported sub-views).
const SIDEBAR_VIEWS: [SlotView; 6] = [
    SlotView::Notes,
    SlotView::Files,
    SlotView::Jira,
    SlotView::Confluence,
    SlotView::Ai,
    SlotView::Compare,
];

/// One content pane. Each view (`SlotView`) renders through one of these; the
/// host builds them lazily in [`KsApp::ensure_panes`] and routes resolved key
/// actions back through [`Pane::handle_key`] so the pane's model sees the same
/// events the AppKit surface reports (kept object-safe so the host can hold a
/// `Box<dyn Pane>` registry).
pub(super) trait Pane {
    /// Draw one frame and apply any events the widgets generated.
    fn ui(&mut self, ui: &mut egui::Ui, colors: &PopupColors);
    /// Called when this pane becomes the current view.
    fn on_enter(&mut self) {}
    /// Route a resolved key action into the pane's model; default ignores it.
    fn handle_key(&mut self, _action: KeyAction) {}
    /// Whether this pane's left column collapses to a rail (the mirror of
    /// the AppKit `SidebarView` collapse). Default: no rail.
    fn has_rail(&self) -> bool {
        false
    }
    /// The current rail (collapsed-to-icons) state.
    fn rail(&self) -> bool {
        false
    }
    /// Set the rail state (the caller persists `sidebarRail.<key>`).
    fn set_rail(&mut self, _on: bool) {}
    /// A drained `UiCommand` aimed at this pane (Jira's inspector toggle /
    /// width). Default: ignore.
    fn apply_command(&mut self, _cmd: &UiCommand) {}
}

/// A stand-in for views with no egui port yet (the sub-views: Detail, Releases,
/// Config, Output, CompareText). Renders the same "not ported" label the stub
/// panes do.
struct Placeholder {
    view: SlotView,
}

impl Placeholder {
    fn new(view: SlotView) -> Self {
        Placeholder { view }
    }
}

impl Pane for Placeholder {
    fn ui(&mut self, ui: &mut egui::Ui, _colors: &PopupColors) {
        ui.label(format!("{} — not ported yet", self.view.title()));
    }
}

pub struct KsApp {
    controller: Arc<SwitcherController>,
    ui_queue: UiQueue,
    main_queue: MainQueue,
    current: SlotView,
    /// The ported panes, built lazily per view (Wave 4: Files; the rest are
    /// stubs or the shared [`Placeholder`]).
    panes: HashMap<SlotView, Box<dyn Pane>>,
    /// The switcher / command-palette overlay, drawn last (stub for now).
    palette: SwitcherOverlay,
    /// The Paths shelf tool window (`do:paths:*`), built lazily on first show.
    paths: Option<PathsPane>,
    paths_open: bool,
    /// The Tools panel window (`do:tool:*`), built lazily on first open.
    tools: Option<ToolsPane>,
    tools_open: bool,
    /// The Terminal tool panel (`do:term` / `toggle-terminal` / Ctrl+B T), built
    /// lazily on first open and hosting a reusable [`TerminalView`].
    terminal: Option<TerminalView>,
    terminal_open: bool,
    /// Live toast pills `(text, shown-at)`, oldest first (`do:toast` verbs).
    toasts: Vec<(String, Instant)>,
    /// The Ctrl+B prefix state (`SharedWindow.prefixKey`).
    prefix: PrefixState,
    /// Consecutive-Esc count + timestamp (the `escStreakCloses` 0.6 s window).
    esc_streak: u32,
    last_esc_at: f64,
    /// `UiCommand` / `KeyAction` variants already reported (log once, ignore).
    logged: BTreeSet<&'static str>,
}

/// Entry point: build the app and run eframe's event loop on the current
/// (main) thread. Never returns while the window is open.
pub fn run(controller: Arc<SwitcherController>, ui_queue: UiQueue, main_queue: MainQueue) {
    let app = KsApp::new(controller, ui_queue, main_queue);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(DEFAULT_WINDOW_SIZE)
            .with_title("Kitchen Sink"),
        ..Default::default()
    };
    if let Err(e) = eframe::run_native("kitchen-sink", options, Box::new(|_cc| Ok(Box::new(app)))) {
        eprintln!("kitchen-sink egui: {e}");
    }
}

impl KsApp {
    fn new(controller: Arc<SwitcherController>, ui_queue: UiQueue, main_queue: MainQueue) -> Self {
        let current = controller.current().unwrap_or(DEFAULT_VIEW);
        KsApp {
            controller,
            ui_queue,
            main_queue,
            current,
            panes: HashMap::new(),
            palette: SwitcherOverlay::build(),
            paths: None,
            paths_open: false,
            tools: None,
            tools_open: false,
            terminal: None,
            terminal_open: false,
            toasts: Vec::new(),
            prefix: PrefixState::new(),
            esc_streak: 0,
            last_esc_at: 0.0,
            logged: BTreeSet::new(),
        }
    }

    fn log_once(&mut self, name: &'static str) {
        if self.logged.insert(name) {
            eprintln!("kitchen-sink egui: {name} not implemented yet (ignored)");
        }
    }

    /// Ctrl+B B / the ToggleSidebarRail key: flip the current pane's rail
    /// (the mirror of `DaemonUi::route_rail_toggle`). The pane persists
    /// `sidebarRail.<view>` in its own `set_rail` (like `SidebarView`).
    fn toggle_rail(&mut self) {
        let view = self.current;
        if let Some(p) = self.panes.get_mut(&view) {
            if p.has_rail() {
                let next = !p.rail();
                p.set_rail(next);
            }
        }
    }

    /// Build a view's pane lazily, like `daemon_ui::content_for`.
    fn ensure_panes(&mut self) {
        if self.panes.contains_key(&self.current) {
            return;
        }
        let pane: Box<dyn Pane> = match self.current {
            SlotView::Notes => Box::new(NotesPane::build()),
            SlotView::Files => Box::new(FilesPane::build()),
            SlotView::Jira => Box::new(JiraPane::build()),
            SlotView::Confluence => Box::new(ConfluencePane::build()),
            SlotView::Ai => Box::new(AiPane::build()),
            SlotView::Compare => Box::new(ComparePane::build()),
            // Sub-views without an egui port yet.
            SlotView::Detail
            | SlotView::Releases
            | SlotView::Config
            | SlotView::Output
            | SlotView::CompareText => Box::new(Placeholder::new(self.current)),
        };
        // No rail seed here: each pane's `build()` reads
        // `sidebarRail.<view>` itself, like the AppKit `set_collapse_key`.
        self.panes.insert(self.current, pane);
    }

    /// The `PopupConfig` `route_key` reads (mirrors the daemon UI's per-frame
    /// config: cycle-view hook, selectable rows, file-browser focus, esc count).
    fn popup_config(&self) -> popup::PopupConfig {
        popup::PopupConfig {
            has_cycle_view_hook: true,
            enable_navigation: true,
            selectable_rows: self.current == SlotView::Files,
            has_file_browser: self.current == SlotView::Files,
            browser_has_focus: self.current == SlotView::Files,
            esc_close_count: self.controller.esc_hide_count(self.current) as i32,
            ..popup::PopupConfig::default()
        }
    }

    /// Drain the `UiCommand` queue (the `Present`/`Hide`/… the socket thread
    /// enqueues) and the main-thread job queues, then repaint. `ctx` is only
    /// needed for the commands that drive the viewport itself (`ResetSize`).
    fn drain_queues(&mut self, ctx: &egui::Context) {
        for _ in 0..100 {
            let cmds: Vec<UiCommand> = {
                let mut q = self.ui_queue.lock().unwrap();
                if q.is_empty() {
                    break;
                }
                q.drain(..).collect()
            };
            for cmd in cmds {
                self.handle_command(cmd, ctx);
            }
        }
        for _ in 0..100 {
            let jobs: Vec<Box<dyn FnOnce() + Send>> = {
                let mut q = self.main_queue.lock().unwrap();
                if q.is_empty() {
                    break;
                }
                q.drain(..).collect()
            };
            for job in jobs {
                job();
            }
        }
        // The platform seam's FIFO (non-macOS `run_on_main` defers into it).
        // On macOS it doesn't exist (dispatch2 handles the main queue there).
        #[cfg(not(target_os = "macos"))]
        crate::platform::drain_main_queue();
    }

    /// Apply one drained [`UiCommand`] (the egui half of `daemon_ui`'s drain
    /// match). `ctx` is only used by `ResetSize`'s viewport command; the rest
    /// just mutate the app / controller. Anything not ported logs once.
    fn handle_command(&mut self, cmd: UiCommand, ctx: &egui::Context) {
        match cmd {
            UiCommand::Present(v) => {
                self.current = v;
                self.ensure_panes();
            }
            // The switcher overlay reads the controller's flags each frame
            // (`SwitcherOverlay::ui`), so showing/hiding is a flag flip.
            UiCommand::ShowPalette => self.controller.set_palette_visible(true),
            UiCommand::HidePalette => self.controller.set_palette_visible(false),
            UiCommand::ShowSwitcher => self.controller.set_view_switcher(true),
            UiCommand::HideSwitcher => self.controller.set_view_switcher(false),
            UiCommand::Paths(cmd) => self.handle_paths(cmd),
            UiCommand::Tool(kind) => self.open_tool(kind),
            UiCommand::ToolClose(name) => self.close_tool(&name),
            UiCommand::TerminalPanel => self.toggle_terminal(),
            UiCommand::ResetSize => ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(
                DEFAULT_WINDOW_SIZE.into(),
            )),
            UiCommand::Toast(text, _icon) => self.toasts.push((text, Instant::now())),
            // Collapse the left pane to its rail (`do:rail:<view>` / Ctrl+B
            // B). The `do:` verb already persisted `sidebarRail.<view>`;
            // the key path persists in `toggle_rail`.
            UiCommand::SidebarRail(v, on) => {
                if let Some(p) = self.panes.get_mut(&v) {
                    p.set_rail(on);
                }
            }
            // The ticket panel (`do:jira-inspector:*`, ⌘I): the model owns
            // the state + its persistence (`JiraList::set_inspector_shown`).
            UiCommand::JiraInspector(on) => {
                if let Some(p) = self.panes.get_mut(&SlotView::Jira) {
                    p.apply_command(&UiCommand::JiraInspector(on));
                }
            }
            UiCommand::InspectorWidth(w) => {
                if let Some(p) = self.panes.get_mut(&SlotView::Jira) {
                    p.apply_command(&UiCommand::InspectorWidth(w));
                }
            }
            other => self.log_unhandled(other),
        }
    }

    /// The `do:paths:*` commands (`run_paths_cmd`): show/hide the shelf window,
    /// run its Return action, or move its selection.
    fn handle_paths(&mut self, cmd: PathsCmd) {
        match cmd {
            PathsCmd::Show => {
                if self.paths.is_none() {
                    self.paths = Some(PathsPane::build());
                }
                self.paths_open = true;
            }
            PathsCmd::Hide => self.paths_open = false,
            PathsCmd::Return => self.paths_return(),
            PathsCmd::Select(n) => {
                if let Some(pane) = self.paths.as_mut() {
                    pane.model.test_select(n);
                }
            }
        }
    }

    /// `do:paths:return` — the shelf's Return action (`run_paths_cmd`'s Return
    /// arm). `PathsPane` keeps its side-effecting `run_return` private, so the
    /// frontend-side pasteboard / open follow-ups run here against the model's
    /// public methods — the same split `views/paths.rs` documents.
    fn paths_return(&mut self) {
        let Some(pane) = self.paths.as_mut() else {
            return;
        };
        match pane.model.return_action {
            ReturnAction::Open => {
                for p in pane.model.open_paths() {
                    let _ = crate::platform::open_path(std::path::Path::new(&p));
                }
            }
            ReturnAction::File | ReturnAction::Path => {
                let _ = crate::platform::clipboard::set_text(&pane.model.copy_paths_text());
            }
        }
    }

    /// `do:tool:<name>` (`open_tool`): raise the Tools window. `ToolsPane`
    /// exposes no kind selector (all three panels are tabs it owns), so `kind`
    /// only tells the window to appear — the user picks the tab.
    fn open_tool(&mut self, _kind: ToolKind) {
        if self.tools.is_none() {
            self.tools = Some(ToolsPane::build());
        }
        self.tools_open = true;
    }

    /// `do:tool-close:<name>` (`close_tool`): order the Tools window out when
    /// `name` is one of its panels.
    fn close_tool(&mut self, name: &str) {
        if ToolKind::is_panel(name) {
            self.tools_open = false;
        }
    }

    /// `do:term` / `toggle-terminal` / Ctrl+B T: toggle the Terminal tool panel,
    /// building the [`TerminalView`] (spawning the shell) on first open.
    fn toggle_terminal(&mut self) {
        if self.terminal.is_none() {
            self.terminal = Some(self.build_terminal());
        }
        self.terminal_open = !self.terminal_open;
    }

    /// Build the terminal from the same `[app]` shell/font config the AppKit
    /// `TerminalPanel` reads (`TerminalConfig::from_settings`).
    fn build_terminal(&self) -> TerminalView {
        let mut settings = crate::app::config::AppSettings::default();
        crate::app::config::apply_app_config_from_disk(&mut settings);
        let cfg = crate::views::terminal::TerminalConfig::from_settings(&settings);
        let mut term = TerminalView::new(cfg.shell, cfg.args, None, std::collections::HashMap::new());
        term.set_font_size(cfg.font_size as f32);
        term
    }

    /// Draw the Paths shelf window when it is open (`paths_open`), building it
    /// on the first `Show`. The window's close button clears `paths_open`.
    fn paths_ui(&mut self, ctx: &egui::Context) {
        if !self.paths_open {
            return;
        }
        let Some(pane) = self.paths.as_mut() else {
            return;
        };
        let colors = section_colors("paths");
        let mut open = true;
        egui::Window::new("Paths")
            .id(egui::Id::new("Paths"))
            .resizable(true)
            .default_size([680.0, 360.0])
            .open(&mut open)
            .show(ctx, |ui| pane.ui(ui, &colors));
        if !open {
            self.paths_open = false;
        }
    }

    /// Draw the Tools window when it is open (`tools_open`), building it on the
    /// first `Tool`. The window's close button clears `tools_open`.
    fn tools_ui(&mut self, ctx: &egui::Context) {
        if !self.tools_open {
            return;
        }
        let Some(pane) = self.tools.as_mut() else {
            return;
        };
        let colors = section_colors(self.current.raw());
        let mut open = true;
        egui::Window::new("Tools")
            .id(egui::Id::new("Tools"))
            .resizable(true)
            .default_size([760.0, 520.0])
            .open(&mut open)
            .show(ctx, |ui| pane.ui(ui, &colors));
        if !open {
            self.tools_open = false;
        }
    }

    /// Draw the Terminal window when it is open (`terminal_open`). The window's
    /// close button clears `terminal_open`.
    fn terminal_ui(&mut self, ctx: &egui::Context) {
        if !self.terminal_open {
            return;
        }
        let Some(term) = self.terminal.as_mut() else {
            return;
        };
        let mut open = true;
        egui::Window::new("Terminal")
            .id(egui::Id::new("Terminal"))
            .resizable(true)
            .default_size([760.0, 460.0])
            .open(&mut open)
            .show(ctx, |ui| term.ui(ui));
        if !open {
            self.terminal_open = false;
        }
    }

    /// Draw the toast pills (bottom-centre), dropping expired ones first. The
    /// 30 ms repaint timer animates the fade.
    fn toasts_ui(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        prune_toasts(&mut self.toasts, now);
        if self.toasts.is_empty() {
            return;
        }
        let colors = section_colors(self.current.raw());
        let toasts = &self.toasts;
        egui::Area::new(egui::Id::new("toasts"))
            .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -24.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                for (text, at) in toasts {
                    let alpha = toast_alpha(now.saturating_duration_since(*at));
                    egui::Frame::popup(&ui.style())
                        .fill(c(colors.background).gamma_multiply(alpha))
                        .stroke(egui::Stroke::new(1.0, c(colors.border).gamma_multiply(alpha)))
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new(text).color(c(colors.text).gamma_multiply(alpha)),
                            );
                        });
                    ui.add_space(6.0);
                }
            });
    }

    fn log_unhandled(&mut self, cmd: UiCommand) {
        let name = match cmd {
            UiCommand::Present(_) => "Present",
            UiCommand::Hide => "Hide",
            UiCommand::ShowPalette => "ShowPalette",
            UiCommand::HidePalette => "HidePalette",
            UiCommand::ShowSwitcher => "ShowSwitcher",
            UiCommand::HideSwitcher => "HideSwitcher",
            UiCommand::ToggleDrawer(_) => "ToggleDrawer",
            UiCommand::Paths(_) => "Paths",
            UiCommand::TerminalPanel => "TerminalPanel",
            UiCommand::Tool(_) => "Tool",
            UiCommand::ToolClose(_) => "ToolClose",
            UiCommand::ResetSize => "ResetSize",
            UiCommand::Toast(..) => "Toast",
            UiCommand::PaneMove(_) => "PaneMove",
            UiCommand::PaneFocus(_) => "PaneFocus",
            UiCommand::NoteFind(_) => "NoteFind",
            UiCommand::JiraJump => "JiraJump",
            UiCommand::JiraEnable(..) => "JiraEnable",
            UiCommand::JiraSetup(_) => "JiraSetup",
            UiCommand::Board(_) => "Board",
            UiCommand::JiraTab(_) => "JiraTab",
            UiCommand::SidebarWidth(_, _) => "SidebarWidth",
            UiCommand::InspectorWidth(_) => "InspectorWidth",
            UiCommand::JiraInspector(_) => "JiraInspector",
            UiCommand::SidebarRail(..) => "SidebarRail",
            UiCommand::Input(_) => "Input",
        };
        self.log_once(name);
    }

    /// Feed egui key events through the existing [`popup::route_key`] router.
    fn handle_keys(&mut self, ctx: &egui::Context) {
        // While a text widget (the filter field) owns the keyboard, let egui
        // handle it; otherwise route navigation/shortcut keys like AppKit does.
        if ctx.wants_keyboard_input() {
            return;
        }
        let events: Vec<egui::Event> = ctx.input(|i| i.events.clone());
        let now = now_secs();
        for ev in events {
            let egui::Event::Key { key, pressed: true, repeat: false, modifiers, .. } = ev else {
                continue;
            };
            let Some(mut ki) = key_input(key, modifiers, None) else {
                continue;
            };
            // Consecutive-Esc streak (`route_key_event`'s `escStreakCloses`).
            if ki.key_code == popup::KEY_ESC {
                ki.esc_streak = if now - self.last_esc_at <= 0.6 {
                    self.esc_streak.saturating_add(1)
                } else {
                    1
                };
                self.esc_streak = ki.esc_streak;
                self.last_esc_at = now;
            } else {
                self.esc_streak = 0;
            }

            // The Ctrl+B prefix / Ctrl+HJKL pane moves run first (as in
            // `route_key_event`), then the ordinary router.
            let pk = popup::PrefixKey {
                key_code: ki.key_code,
                chars: ki.chars,
                cmd: ki.cmd,
                ctrl: ki.ctrl,
                opt: ki.opt,
                shift: ki.shift,
                is_repeat: false,
            };
            match self.prefix.handle(&pk, now) {
                PrefixOutcome::Consumed => continue,
                PrefixOutcome::PaneMove(_) => {
                    self.log_once("pane move");
                    continue;
                }
                PrefixOutcome::Command(ch) => {
                    self.run_prefix_command(ch);
                    continue;
                }
                PrefixOutcome::Passthrough => {}
            }

            self.handle_action(popup::route_key(&self.popup_config(), ki));
        }
    }

    fn handle_action(&mut self, action: KeyAction) {
        match action {
            KeyAction::Escape(outcome) => {
                let cfg = self.popup_config();
                if popup::esc_streak_closes(self.esc_streak, cfg.esc_close_count) {
                    self.controller.hide("Esc");
                } else if let Some(pane) = self.panes.get_mut(&self.current) {
                    pane.handle_key(KeyAction::Escape(outcome));
                }
            }
            KeyAction::CycleView(dir) => self.controller.cycle(dir),
            KeyAction::ListMove(delta) => {
                if let Some(pane) = self.panes.get_mut(&self.current) {
                    pane.handle_key(KeyAction::ListMove(delta));
                }
            }
            KeyAction::ListAccept => {
                if let Some(pane) = self.panes.get_mut(&self.current) {
                    pane.handle_key(KeyAction::ListAccept);
                }
            }
            KeyAction::CycleTabs(_) => self.log_once("CycleTabs"),
            KeyAction::FileBrowser(_) => self.log_once("FileBrowser"),
            KeyAction::ToggleSidebarRail => self.toggle_rail(),
            KeyAction::Vim(_) => self.log_once("Vim"),
            KeyAction::Edit(_) | KeyAction::SheetEdit(_) | KeyAction::Terminal(_) => {
                self.log_once("Edit")
            }
            KeyAction::CommandK => self.log_once("CommandK"),
            KeyAction::CommandF => self.log_once("CommandF"),
            KeyAction::ShowShortcuts => self.log_once("ShowShortcuts"),
            KeyAction::FontStep(_) => self.log_once("FontStep"),
            KeyAction::ResizeStep(_) | KeyAction::ResizeReset => self.log_once("Resize"),
            KeyAction::FindNext(_) => self.log_once("FindNext"),
            KeyAction::EditorCommit | KeyAction::EditorOpenPath => self.log_once("Editor"),
            KeyAction::ListMoveWrap(_)
            | KeyAction::ListToggleSelection
            | KeyAction::ListPage(_) => self.log_once("List"),
            KeyAction::PaneResize(_) => self.log_once("PaneResize"),
            KeyAction::Pass => {}
        }
    }

    /// The Ctrl+B commands (mirrors `DaemonUi::run_prefix_command`; only the
    /// ones reachable without AppKit are wired, the rest log once).
    fn run_prefix_command(&mut self, ch: char) {
        match ch {
            'l' => {
                use crate::app::registry::nav;
                let id = self.controller.previous_nav().unwrap_or(if self.current == SlotView::Files {
                    nav::NOTES
                } else {
                    nav::FILES
                });
                self.controller.nav_clicked(id);
            }
            'w' => {
                if self.controller.view_switcher_shown() {
                    self.controller.view_switcher_hide();
                } else {
                    self.controller.view_switcher_show();
                }
            }
            't' => self.toggle_terminal(),
            'b' => self.toggle_rail(),
            _ => {}
        }
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.heading("kitchen-sink");
        ui.separator();
        for v in SIDEBAR_VIEWS {
            let colors = section_colors(v.raw());
            let selected = self.current == v;
            let label = egui::RichText::new(v.title()).color(c(colors.text));
            if ui.selectable_label(selected, label).clicked() {
                self.controller.open(v);
            }
        }
    }

    fn content(&mut self, ui: &mut egui::Ui) {
        let colors = section_colors(self.current.raw());
        if let Some(pane) = self.panes.get_mut(&self.current) {
            pane.ui(ui, &colors);
        } else {
            ui.label(format!("{} view is not ported yet", self.current.title()));
        }
    }
}

impl eframe::App for KsApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_queues(ctx);
        self.handle_keys(ctx);

        let bg = c(section_colors(self.current.raw()).background);
        egui::SidePanel::left("sidebar")
            .frame(egui::Frame::NONE.fill(bg))
            .resizable(false)
            .default_width(180.0)
            .show(ctx, |ui| self.sidebar(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(bg))
            .show(ctx, |ui| self.content(ui));

        // Tool windows float over the active pane; the switcher / palette
        // overlay draws over them, and toasts on top of everything.
        self.paths_ui(ctx);
        self.tools_ui(ctx);
        self.terminal_ui(ctx);
        self.palette.ui(ctx, &self.controller);
        self.toasts_ui(ctx);

        // Mirror the 30 ms AppKit drain timer.
        ctx.request_repaint_after(Duration::from_millis(30));
    }
}

/// Drop toast pills older than [`TOAST_TTL`]. Pure; the test drives `now`.
fn prune_toasts(toasts: &mut Vec<(String, Instant)>, now: Instant) {
    toasts.retain(|(_, at)| now.saturating_duration_since(*at) < TOAST_TTL);
}

/// A toast's opacity for its age: full until the fade window, then linear to 0.
fn toast_alpha(age: Duration) -> f32 {
    if age >= TOAST_TTL {
        return 0.0;
    }
    let fade_start = TOAST_TTL - TOAST_FADE;
    if age <= fade_start {
        1.0
    } else {
        (TOAST_TTL - age).as_secs_f32() / TOAST_FADE.as_secs_f32()
    }
}

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn app() -> KsApp {
        KsApp::new(
            Arc::new(SwitcherController::new_default()),
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(Vec::new())),
        )
    }

    #[test]
    fn prune_toasts_drops_expired_pills() {
        let now = Instant::now();
        let mut toasts = vec![
            ("fresh".to_string(), now),
            ("stale".to_string(), now - TOAST_TTL - Duration::from_millis(1)),
        ];
        prune_toasts(&mut toasts, now);
        assert_eq!(toasts.len(), 1);
        assert_eq!(toasts[0].0, "fresh");
    }

    #[test]
    fn toast_alpha_fades_only_at_the_end() {
        assert_eq!(toast_alpha(Duration::ZERO), 1.0);
        assert_eq!(toast_alpha(TOAST_TTL), 0.0);
        let mid = toast_alpha(TOAST_TTL - TOAST_FADE / 2);
        assert!(mid > 0.0 && mid < 1.0, "mid-fade alpha {mid}");
    }

    /// Feed commands through the pure drain path and assert the state they set
    /// (no live window / GUI).
    #[test]
    fn drains_commands_into_egui_state() {
        let ctx = egui::Context::default();
        let mut app = app();

        app.handle_command(UiCommand::Toast("saved".into(), "checkmark"), &ctx);
        assert_eq!(app.toasts.len(), 1, "toast queued");

        app.handle_command(UiCommand::Paths(PathsCmd::Show), &ctx);
        assert!(app.paths_open && app.paths.is_some(), "paths shown + built");
        app.handle_command(UiCommand::Paths(PathsCmd::Select(0)), &ctx);
        app.handle_command(UiCommand::Paths(PathsCmd::Hide), &ctx);
        assert!(!app.paths_open, "paths hidden");

        app.handle_command(UiCommand::Tool(ToolKind::Filefast), &ctx);
        assert!(app.tools_open && app.tools.is_some(), "tools shown + built");
        app.handle_command(UiCommand::ToolClose(ToolKind::Filefast.name().to_string()), &ctx);
        assert!(!app.tools_open, "tools closed");
        // A non-panel name leaves the window up.
        app.handle_command(UiCommand::Tool(ToolKind::PrettyPrint), &ctx);
        app.handle_command(UiCommand::ToolClose("bogus".to_string()), &ctx);
        assert!(app.tools_open, "an unknown tool name is a no-op");

        app.handle_command(UiCommand::ShowSwitcher, &ctx);
        assert!(app.controller.view_switcher_shown(), "switcher flag set");
        app.handle_command(UiCommand::HideSwitcher, &ctx);
        assert!(!app.controller.view_switcher_shown(), "switcher flag cleared");
        app.handle_command(UiCommand::ShowPalette, &ctx);
        assert!(app.controller.is_palette_visible(), "palette flag set");
        app.handle_command(UiCommand::HidePalette, &ctx);
        assert!(!app.controller.is_palette_visible(), "palette flag cleared");

        // ResetSize only queues a viewport command; must not panic.
        app.handle_command(UiCommand::ResetSize, &ctx);
    }
}
