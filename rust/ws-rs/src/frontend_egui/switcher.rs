//! The switcher / command-palette overlay — Wave 5 egui view (W5-A).
//!
//! A modal drawn last, over the active pane (`app.rs` `self.palette.ui(ctx)`).
//! It drives the two pure models the AppKit panels drive behind the shared
//! [`crate::views::switcher::SwitcherSnapshot`] / `SwitcherEvent` contract:
//! [`crate::views::switcher::ViewSwitcherPanel`] (the Ctrl+B W switcher) and
//! [`crate::views::switcher::PaletteModel`] (the `show` palette). Each frame it
//! pulls a snapshot, draws a filter field + list, collects the user's actions
//! as events, and hands them back through `handle()` — with accepts read back
//! through `accept()` exactly like the AppKit handlers' `pick:` (a click/Return
//! selects, then the owner runs the action). Colours go through
//! [`super::theme::c`] only.

use crate::app::host::{SwitcherController, DEFAULT_VIEW};
use crate::engines::switcher_status::SwitcherStatus;
use crate::ui::theme::section_colors;
use crate::views::switcher::{
    row_summary, PaletteAction, PaletteModel, SwitcherEvent, SwitcherRow, SwitchView,
    ViewSwitcherPanel,
};

use super::theme::c;

/// Which of the two overlay models is up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OverlayKind {
    /// The Ctrl+B W view switcher (`ViewSwitcherPanel`).
    Switcher,
    /// The `show` command palette (`PaletteModel`).
    Palette,
}

/// The switcher / palette modal overlay. Not a `SlotView` pane: the host draws
/// it last, on top of the active pane.
pub struct SwitcherOverlay {
    switcher: ViewSwitcherPanel,
    palette: PaletteModel,
    /// The filter field's editable buffer (only one overlay is up at a time).
    filter: String,
    /// The model currently showing, or `None` when both are hidden.
    active: Option<OverlayKind>,
}

impl SwitcherOverlay {
    /// Build the empty overlay; each model is filled when it is shown (`open`),
    /// mirroring the AppKit `show_palette` / `show_switcher`.
    pub fn build() -> Self {
        SwitcherOverlay {
            switcher: ViewSwitcherPanel::new(),
            palette: PaletteModel::default(),
            filter: String::new(),
            active: None,
        }
    }

    /// Draw the overlay (if one is showing) and route its events.
    ///
    /// `controller` is the source of truth for which panel is up
    /// (`view_switcher_shown` / `is_palette_visible`) and the target of an
    /// accept (`nav_clicked` / `run_palette_command`).
    pub fn ui(&mut self, ctx: &egui::Context, controller: &SwitcherController) {
        // Show/hide is read from the controller flags each frame, so the
        // socket's `ShowPalette` / `ShowSwitcher` verbs (logged and ignored in
        // `app.rs::drain_queues`) still surface here.
        let kind = overlay_kind(controller);
        if kind != self.active {
            self.active = kind;
            self.filter.clear();
            if let Some(k) = kind {
                self.open(k, controller);
            }
        }
        let Some(kind) = self.active else {
            return;
        };

        let colors = section_colors(controller.current().unwrap_or(DEFAULT_VIEW).raw());
        let snap = match kind {
            OverlayKind::Switcher => self.switcher.snapshot(),
            OverlayKind::Palette => self.palette.snapshot(),
        };

        // --- keys (`route_key_event`'s switcher / palette handling) --------
        // Read before the widgets so the focused field cannot eat navigation:
        // a single-line field uses none of ↑ / ↓ / Esc.
        let (up, down, esc, enter, tab, shift) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::ArrowDown),
                i.key_pressed(egui::Key::Escape),
                i.key_pressed(egui::Key::Enter),
                i.key_pressed(egui::Key::Tab),
                i.modifiers.shift,
            )
        });
        let mut events: Vec<SwitcherEvent> = Vec::new();
        if up {
            events.push(SwitcherEvent::Move(-1));
        }
        if down {
            events.push(SwitcherEvent::Move(1));
        }
        if esc {
            events.push(SwitcherEvent::Escape);
        }
        if enter {
            events.push(SwitcherEvent::Enter);
        }
        // The palette is one list: Tab moves down, Shift+Tab up.
        if tab && kind == OverlayKind::Palette {
            events.push(SwitcherEvent::Move(if shift { -1 } else { 1 }));
        }

        // --- the filter field (`ViewSwitcherPanel.search` / the palette's) --
        // A modal owns the keyboard (rule 6): keep the field focused so typing
        // filters immediately and the window's own Esc handling stays out.
        let filter_id = egui::Id::new("switcher-overlay-filter");
        if !ctx.memory(|m| m.has_focus(filter_id)) {
            ctx.memory_mut(|m| m.request_focus(filter_id));
        }
        let title = match kind {
            OverlayKind::Switcher => "Switcher",
            OverlayKind::Palette => "Command Palette",
        };

        // --- the modal window (the floating `PopupPanel`) ------------------
        egui::Window::new(title)
            .title_bar(false)
            .resizable(false)
            .collapsible(false)
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 90.0))
            .frame(egui::Frame::popup(&ctx.style()).fill(c(colors.background)))
            .show(ctx, |ui| {
                ui.set_min_width(420.0);
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.filter)
                        .hint_text("Filter")
                        .id(filter_id)
                        .desired_width(400.0),
                );
                if resp.changed() {
                    events.push(SwitcherEvent::Query(self.filter.clone()));
                }
                ui.separator();
                // --- the rows (`ListRows`) ---------------------------------
                egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                    for (i, row) in snap.rows.iter().enumerate() {
                        let label = egui::RichText::new(row_label(row)).color(c(colors.text));
                        if ui.selectable_label(i == snap.selection, label).clicked() {
                            events.push(SwitcherEvent::Click { row: i });
                        }
                    }
                });
                // --- the palette status row (`SwitcherStatus`) -------------
                if let Some(status) = &snap.status {
                    ui.separator();
                    ui.label(
                        egui::RichText::new(status_line(status))
                            .color(c(colors.dim))
                            .small(),
                    );
                }
            });

        self.apply(kind, events, controller);
    }

    /// Fill the model for a freshly shown panel (`show_palette` /
    /// `show_switcher`): commands from the registry, the switcher's views from
    /// the nav icons, and the current view pre-selected.
    fn open(&mut self, kind: OverlayKind, controller: &SwitcherController) {
        match kind {
            OverlayKind::Switcher => {
                let views: Vec<SwitchView> = controller
                    .registry()
                    .nav_icons()
                    .iter()
                    .map(|n| SwitchView::new(n.id, n.view.title(), String::new()))
                    .collect();
                let current = controller.current().map(|v| v.nav_id());
                self.switcher.show(&views, current, None);
                self.switcher.set_commands(palette_commands(controller));
            }
            OverlayKind::Palette => {
                self.palette = PaletteModel::new(controller.hyper_palette_commands());
                self.palette.shown = true;
            }
        }
    }

    /// Dispatch the queued events through the active model, running the accept
    /// action through the controller (the AppKit `pick:` callbacks).
    fn apply(
        &mut self,
        kind: OverlayKind,
        events: Vec<SwitcherEvent>,
        controller: &SwitcherController,
    ) {
        for ev in events {
            // An accept is read back before the model hides: `handle` only
            // hides on Return, the action runs through the owner.
            if ev == SwitcherEvent::Enter {
                self.dispatch_accept(kind, controller);
            }
            match kind {
                OverlayKind::Switcher => self.switcher.handle(ev),
                OverlayKind::Palette => self.palette.handle(ev),
            }
        }
        // A model that hid itself (Esc / accept) closes the overlay for real by
        // clearing the controller flag, so the next frame sees both hidden.
        let shown = match kind {
            OverlayKind::Switcher => self.switcher.shown,
            OverlayKind::Palette => self.palette.shown,
        };
        if !shown {
            match kind {
                OverlayKind::Switcher => controller.set_view_switcher(false),
                OverlayKind::Palette => controller.set_palette_visible(false),
            }
            self.active = None;
        }
        // Mirror model-side query changes (Esc clears a live palette query)
        // into the field; typing already produced a matching Query event.
        self.filter = match kind {
            OverlayKind::Switcher => self.switcher.live_query(),
            OverlayKind::Palette => self.palette.query.clone(),
        };
    }

    /// The accept decision (`ViewSwitcherPanel.accept` / `PaletteModel.accept`)
    /// run against the controller.
    fn dispatch_accept(&mut self, kind: OverlayKind, controller: &SwitcherController) {
        match kind {
            OverlayKind::Switcher => {
                if let Some(id) = self.switcher.accept() {
                    controller.nav_clicked(id);
                }
            }
            OverlayKind::Palette => {
                if let Some(PaletteAction::Command(name)) = self.palette.accept() {
                    controller.run_palette_command(&name);
                }
            }
        }
    }
}

/// Which overlay the controller says is up. Only one is ever shown; the
/// Ctrl+B W switcher takes precedence if both flags are somehow set.
fn overlay_kind(controller: &SwitcherController) -> Option<OverlayKind> {
    if controller.view_switcher_shown() {
        Some(OverlayKind::Switcher)
    } else if controller.is_palette_visible() {
        Some(OverlayKind::Palette)
    } else {
        None
    }
}

/// The registry's palette commands as the models' `(name, title)` pairs
/// (the Ctrl+B W `/` commands; the Hyper+S palette uses
/// `hyper_palette_commands`).
fn palette_commands(controller: &SwitcherController) -> Vec<(String, String)> {
    controller
        .registry()
        .palette_commands()
        .iter()
        .map(|c| (c.id.clone(), c.title.clone()))
        .collect()
}

/// The label one row draws (`SwitchRow.summary` / the command title).
fn row_label(row: &SwitcherRow) -> String {
    match row {
        SwitcherRow::View(r) => row_summary(r),
        SwitcherRow::Command { name, title } => {
            if title.is_empty() {
                name.clone()
            } else {
                title.clone()
            }
        }
    }
}

/// The palette status row's text (`SwitcherStatus`: CPU / RAM / battery /
/// unread chips).
fn status_line(status: &SwitcherStatus) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(cpu) = status.cpu {
        parts.push(format!("CPU {cpu}%"));
    }
    if let Some(ram) = status.ram {
        parts.push(format!("RAM {ram}%"));
    }
    if let Some((pct, charging)) = status.battery {
        parts.push(format!("{}{pct}%", if charging { "⚡" } else { "" }));
    }
    for chip in &status.chips {
        if chip.count.is_empty() {
            parts.push(chip.app.clone());
        } else {
            parts.push(format!("{} {}", chip.app, chip.count));
        }
    }
    parts.join("   ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::switcher::SwitchRow;

    fn view_row(name: &str, number: i64, current: bool) -> SwitchRow {
        SwitchRow {
            nav_id: number,
            name: name.into(),
            location: String::new(),
            aliases: Vec::new(),
            number,
            is_current: current,
        }
    }

    #[test]
    fn labels_the_row_kinds() {
        assert_eq!(
            row_label(&SwitcherRow::View(view_row("Files", 3, true))),
            "3   Files   ● here"
        );
        assert_eq!(
            row_label(&SwitcherRow::Command { name: "notes".into(), title: "Notes".into() }),
            "Notes"
        );
        // A command with no title falls back to its name.
        assert_eq!(
            row_label(&SwitcherRow::Command { name: "notes".into(), title: String::new() }),
            "notes"
        );
    }

    #[test]
    fn renders_the_status_row() {
        let s = SwitcherStatus {
            cpu: Some(12),
            ram: Some(55),
            battery: Some((88, true)),
            chips: Vec::new(),
        };
        assert_eq!(status_line(&s), "CPU 12%   RAM 55%   ⚡88%");
        assert_eq!(status_line(&SwitcherStatus::default()), "");
    }
}
