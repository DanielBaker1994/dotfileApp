//! The egui app: `KsApp` mirrors the AppKit `daemon_ui`'s queue-draining and
//! key-routing, but renders with egui instead of AppKit. It installs its queues
//! on the controller exactly like `daemon_ui::install_daemon_ui` does, so the
//! socket thread's `UiCommand`s and main-thread jobs flow into `update()`.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::app::host::{MainQueue, SwitcherController, UiCommand, UiQueue, DEFAULT_VIEW};
use crate::app::registry::SlotView;
use crate::ui::popup::{self, KeyAction, PrefixOutcome, PrefixState};
use crate::ui::theme::{section_colors, PopupColors};

use super::ai::AiPane;
use super::compare::ComparePane;
use super::confluence::ConfluencePane;
use super::files::FilesPane;
use super::jira::JiraPane;
use super::keys::key_input;
use super::notes::NotesPane;
use super::switcher::SwitcherOverlay;
use super::theme::c;

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
            .with_inner_size([1000.0, 700.0])
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
    /// enqueues) and the main-thread job queues, then repaint.
    fn drain_queues(&mut self) {
        for _ in 0..100 {
            let cmds: Vec<UiCommand> = {
                let mut q = self.ui_queue.lock().unwrap();
                if q.is_empty() {
                    break;
                }
                q.drain(..).collect()
            };
            for cmd in cmds {
                match cmd {
                    UiCommand::Present(v) => {
                        self.current = v;
                        self.ensure_panes();
                    }
                    other => self.log_unhandled(other),
                }
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
            KeyAction::ToggleSidebarRail => self.log_once("ToggleSidebarRail"),
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
            't' => self.log_once("terminal panel"),
            'b' => self.log_once("sidebar rail"),
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
        self.drain_queues();
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

        // The switcher / palette overlay draws last, over the active pane.
        self.palette.ui(ctx);

        // Mirror the 30 ms AppKit drain timer.
        ctx.request_repaint_after(Duration::from_millis(30));
    }
}

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}
