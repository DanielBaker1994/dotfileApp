//! The Files view — THE reference egui view (Wave 5 copies this shape).
//!
//! It drives the same pure [`crate::views::files::FileBrowser`] model the
//! AppKit [`crate::views::files_view::FilesView`] drives: pull a
//! [`FilesSnapshot`] each frame, draw it with egui widgets, collect the user's
//! actions as [`FilesEvent`]s, and hand them to [`FileBrowser::handle`] — the
//! exact same event contract the AppKit surface reports. Each section below is
//! commented with the `files_view.rs` region it mirrors.

use crate::ui::popup::KeyAction;
use crate::ui::sidebar::SidebarEvent;
use crate::ui::theme::PopupColors;
use crate::views::files::FileBrowser;
use crate::views::files_view::{FilesEvent, FilesSnapshot, Preview};

use super::app::Pane;
use super::theme::c;

pub struct FilesPane {
    pub model: FileBrowser,
    /// The filter field's editable buffer (`files_view.rs` `search` NSTextField).
    filter: String,
}

impl FilesPane {
    /// Build the model like `DaemonUi::build_files` (the pure half; `build()` /
    /// `content_view()` are the AppKit-only half and are skipped here).
    pub fn build() -> Self {
        let text = crate::app::config::read_config_text().unwrap_or_default();
        let section = crate::views::tools::section_vars(&text, "files");
        crate::views::files::start_recent(&section);
        let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
        let root = section
            .get("root")
            .map(|r| r.trim().to_string())
            .filter(|r| !r.is_empty())
            .map(|r| if let Some(rest) = r.strip_prefix('~') { format!("{home}{rest}") } else { r })
            .unwrap_or(home);
        let mut model = FileBrowser::new(&root);
        model.configure(&section);
        if section.get("start").map(|s| s.to_lowercase()) == Some("recent".into()) {
            model.show_virtual(0);
        } else {
            model.reload();
        }
        FilesPane { model, filter: String::new() }
    }

    /// Draw one frame and apply the events it generated. `colors` are the
    /// `[files]` section tokens (from [`crate::ui::theme::section_colors`]).
    pub fn ui(&mut self, ui: &mut egui::Ui, colors: &PopupColors) {
        let snap = self.model.snapshot();
        let mut events: Vec<FilesEvent> = Vec::new();

        // The click modifiers for this frame (constant within a frame).
        let (cmd, shift) = ui.input(|i| (i.modifiers.mac_cmd, i.modifiers.shift));

        // --- places sidebar | main split (`files_view.rs` `layoutPanes`) ---
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(200.0, ui.available_height()),
                egui::Layout::top_down(egui::Align::Min),
                |ui| self.places_sidebar(ui, &snap, colors, &mut events),
            );
            ui.separator();
            ui.vertical(|ui| {
                self.toolbar(ui, &snap, colors, &mut events);
                ui.separator();
                self.list_and_preview(ui, &snap, colors, cmd, shift, &mut events);
                ui.separator();
                // --- status line (`files_view.rs` `draw_status`) ---
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(snap.status.as_str()).color(c(colors.dim)).small());
                });
            });
        });

        for ev in events {
            self.model.handle(ev);
        }
    }

    /// The left Places column (`files_view.rs` `SidebarView`: pinned favorites
    /// then the Places rows — Recent, Arrived, Home, Desktop, …).
    fn places_sidebar(
        &self,
        ui: &mut egui::Ui,
        snap: &FilesSnapshot,
        colors: &PopupColors,
        events: &mut Vec<FilesEvent>,
    ) {
        let m = &snap.sidebar;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for pin in &m.pinned {
                let sel = m.pinned_selected.as_deref() == Some(pin.key.as_str());
                let label = egui::RichText::new(pin.label.as_str()).color(c(colors.text));
                if ui.selectable_label(sel, label).clicked() {
                    events.push(FilesEvent::Sidebar(SidebarEvent::Pinned(pin.key.clone())));
                }
            }
            if !m.pinned.is_empty() {
                ui.separator();
            }
            if !m.section_title.is_empty() {
                ui.label(
                    egui::RichText::new(m.section_title.to_uppercase())
                        .color(c(colors.dim))
                        .small(),
                );
            }
            for (i, row) in m.rows.iter().enumerate() {
                let label = egui::RichText::new(row.title.as_str()).color(c(colors.text));
                if ui.selectable_label(m.selected == Some(i), label).clicked() {
                    events.push(FilesEvent::Sidebar(SidebarEvent::Select(i)));
                }
            }
        });
    }

    /// The toolbar row (`files_view.rs` `draw_toolbar` / `toolbar_rects`).
    fn toolbar(
        &mut self,
        ui: &mut egui::Ui,
        snap: &FilesSnapshot,
        _colors: &PopupColors,
        events: &mut Vec<FilesEvent>,
    ) {
        ui.horizontal(|ui| {
            if ui.button("↑").on_hover_text("Up").clicked() {
                events.push(FilesEvent::Parent);
            }
            if snap.can_pin {
                let pin = if snap.pinned_on { "★ Pinned" } else { "☆ Pin" };
                if ui.button(pin).clicked() {
                    events.push(FilesEvent::TogglePin);
                }
            }
            // The sort popup is the frontend's job (AppKit shows a menu here);
            // the skeleton renders the current key as a label.
            ui.label(snap.sort_short.clone());
            let order = if snap.descending { "Desc" } else { "Asc" };
            if ui.button(order).clicked() {
                events.push(FilesEvent::ToggleOrder);
            }

            // --- filter field (`files_view.rs` `search_box` + `apply`) ---
            // Mirror `apply`: the field follows `snap.query` except while it is
            // being edited (typing must not be clobbered).
            let id = ui.make_persistent_id("files-filter");
            let focused = ui.memory(|m| m.has_focus(id));
            if !focused && self.filter != snap.query {
                self.filter = snap.query.clone();
            }
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .hint_text(&snap.placeholder)
                    .id(id),
            );
            if resp.changed() {
                events.push(FilesEvent::Query(self.filter.clone()));
            }
        });
    }

    /// The list (left) | preview (right) split (`files_view.rs` `layoutPanes`).
    fn list_and_preview(
        &self,
        ui: &mut egui::Ui,
        snap: &FilesSnapshot,
        colors: &PopupColors,
        cmd: bool,
        shift: bool,
        events: &mut Vec<FilesEvent>,
    ) {
        ui.columns(2, |cols| {
            // List column (`files_view.rs` `FileListView` / `draw`).
            let rows = snap.rows.clone();
            let selection = snap.selection;
            cols[0].push_id("files-list", |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    for (i, e) in rows.iter().enumerate() {
                        let label = if e.trailing_text.is_empty() {
                            e.name.clone()
                        } else {
                            format!("{}   {}", e.name, e.trailing_text)
                        };
                        let resp = ui.selectable_label(i == selection, label);
                        if resp.clicked() {
                            events.push(FilesEvent::Click {
                                row: i,
                                cmd,
                                shift,
                                double: resp.double_clicked(),
                            });
                        }
                    }
                });
            });

            // Preview column (`files_view.rs` `show_preview`).
            let preview = snap.preview.clone();
            cols[1].push_id("files-preview", |ui| match preview {
                Preview::None => {}
                Preview::Hint(h) => {
                    ui.label(egui::RichText::new(h).color(c(colors.dim)));
                }
                Preview::Text(t) => {
                    egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                        ui.monospace(t);
                    });
                }
                Preview::Image(path) => {
                    // Image decode is not ported yet; the skeleton shows the path.
                    ui.label(egui::RichText::new(path).color(c(colors.dim)));
                }
                Preview::Folder(rows) => {
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                        for (i, e) in rows.iter().enumerate() {
                            let resp = ui.selectable_label(false, e.name.clone());
                            if resp.double_clicked() {
                                events.push(FilesEvent::PreviewOpen(i));
                            }
                        }
                    });
                }
            });
        });
    }
}

impl Pane for FilesPane {
    fn ui(&mut self, ui: &mut egui::Ui, colors: &PopupColors) {
        FilesPane::ui(self, ui, colors);
    }

    /// The resolved key actions the Files pane's model consumes (the same
    /// three `handle_action` routed to `self.model` before the registry
    /// refactor: Escape, list up/down, and Enter).
    fn handle_key(&mut self, action: KeyAction) {
        match action {
            KeyAction::Escape(_) => self.model.handle(FilesEvent::Escape),
            KeyAction::ListMove(delta) => self.model.handle(FilesEvent::Move(delta as i64)),
            KeyAction::ListAccept => self.model.handle(FilesEvent::Enter),
            _ => {}
        }
    }
}
