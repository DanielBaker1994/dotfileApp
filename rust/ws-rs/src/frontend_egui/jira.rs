//! The Jira view — Wave 5 egui port.
//!
//! It drives the same pure [`crate::views::jira_view::JiraList`] model the
//! AppKit [`crate::views::jira_view::JiraView`] drives: pull a [`JiraSnapshot`]
//! each frame, draw it with egui widgets, collect the user's actions as
//! [`JiraEvent`]s, and hand them to [`JiraList::handle`] — the exact same event
//! contract the AppKit surface reports. Each section below is commented with the
//! `jira_view.rs` region it mirrors. `files.rs` is the reference shape.

use crate::app::host::UiCommand;
use crate::ui::popup::KeyAction;
use crate::ui::sidebar::SidebarEvent;
use crate::ui::theme::PopupColors;
use crate::views::jira_view::{
    cell_style, display_cell, tone_of, JiraEvent, JiraList, JiraSnapshot, BOARD_PREFIX, HEADER_H, ROW_H,
};

use egui_extras::{Column, TableBuilder};

use super::app::Pane;
use super::theme::c;
use super::widgets;

/// The lead gutter width (check + star), mirroring `jira_view.rs` `LEAD`'s
/// checkbox/star lead-in.
const GUTTER: f32 = 54.0;

pub struct JiraPane {
    pub model: JiraList,
    /// The search field's editable buffer (`JiraView.search` NSTextField).
    filter: String,
    /// The sidebar collapse-to-rail state (`SidebarView`'s rail; persisted
    /// as `sidebarRail.jira`, toggled by Ctrl+B B / `do:rail:jira`).
    rail: bool,
    /// The ticket panel's width (`inspector-width`; `do:inspector-width`).
    inspector_w: f32,
}

/// The header cell label (`PopupTableHeaderView.draw`): the column title
/// upper-cased, with ` ↑` / ` ↓` appended when that column is the sort key.
fn header_label(title: &str, sorted: Option<bool>) -> String {
    let mut s = title.to_uppercase();
    match sorted {
        Some(true) => s.push_str(" ↑"),
        Some(false) => s.push_str(" ↓"),
        None => {}
    }
    s
}

impl JiraPane {
    /// Build the model like `DaemonUi::build_jira` (the pure half; the
    /// `Retained<JiraView>` construction is the AppKit-only half, skipped here).
    pub fn build() -> Self {
        let text = crate::app::config::read_config_text().unwrap_or_default();
        let section = crate::views::tools::section_vars(&text, "jira");
        let paths = match crate::app::paths::JiraPaths::resolve() {
            crate::app::paths::JiraPathsState::Resolved(p) => Some(p),
            crate::app::paths::JiraPathsState::Failed(_) => None,
        };
        let mut model = JiraList::new(section, paths);
        model.start(None);
        // `build_jira`'s restore: the rail state persists, don't re-write.
        model.inspector = !crate::platform::get_bool("jira.inspectorRail");
        let inspector_w = model
            .section
            .get("inspector-width")
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(widgets::INSPECTOR_W)
            .clamp(widgets::RAIL_W, 640.0);
        JiraPane {
            model,
            filter: String::new(),
            rail: crate::ui::sidebar::rail_default("jira"),
            inspector_w,
        }
    }

    /// Draw one frame and apply the events it generated. `colors` are the
    /// `[jira]` section tokens (from [`crate::ui::theme::section_colors`]).
    pub fn ui(&mut self, ui: &mut egui::Ui, colors: &PopupColors) {
        let snap = self.model.snapshot();
        let mut events: Vec<JiraEvent> = Vec::new();

        // --- sidebar | list | inspector split (`JiraView.layoutPanes`) ---
        ui.horizontal_top(|ui| {
            if self.rail {
                // The collapsed rail (`SidebarView` icons): egui has no SF
                // Symbols, so it's the expand strip — same state, same key.
                ui.allocate_ui_with_layout(
                    egui::vec2(widgets::RAIL_W, ui.available_height()),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        if widgets::rail_button(ui, true, colors) {
                            self.set_rail(false);
                        }
                    },
                );
            } else {
                ui.allocate_ui_with_layout(
                    egui::vec2(widgets::SIDEBAR_W, ui.available_height()),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        self.sidebar(ui, &snap, colors, &mut events);
                        if widgets::rail_button(ui, false, colors) {
                            self.set_rail(true);
                        }
                    },
                );
            }
            ui.separator();
            ui.vertical(|ui| {
                // --- search box (`search_box` + `controlTextDidChange:`) ---
                self.search(ui, &snap, &mut events);
                ui.separator();
                // --- issue table (header + `JiraTableView`) ---
                self.table(ui, &snap, colors, &mut events);
            });
            // The ticket panel, or — collapsed (⌘I / its toggle) — the thin
            // rail strip with the expand button (`INSPECTOR_RAIL_W` mirror).
            if snap.show_inspector {
                ui.separator();
                let w = self.inspector_w;
                ui.allocate_ui_with_layout(
                    egui::vec2(w, ui.available_height()),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| self.inspector(ui, &snap, colors, &mut events),
                );
            } else {
                ui.separator();
                ui.allocate_ui_with_layout(
                    egui::vec2(widgets::RAIL_W, ui.available_height()),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        if widgets::rail_button(ui, true, colors) {
                            self.model.set_inspector_shown(Some(true));
                        }
                    },
                );
            }
        });

        for ev in events {
            self.model.handle(ev);
        }
    }

    /// The left rail (`SidebarModel`: MY WORK / PINNED / BOARDS / SYNCED),
    /// mirroring `jira_view.rs` `sidebar_model()`.
    fn sidebar(
        &self,
        ui: &mut egui::Ui,
        snap: &JiraSnapshot,
        colors: &PopupColors,
        events: &mut Vec<JiraEvent>,
    ) {
        let m = &snap.sidebar;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let mut last_section = "";
            for pin in &m.pinned {
                if pin.section != last_section {
                    widgets::section_label(ui, &pin.section, colors);
                    last_section = pin.section.as_str();
                }
                let sel = m.pinned_selected.as_deref() == Some(pin.key.as_str());
                // Tree rows ("All boards" ▾ + its indented starred boards),
                // mirroring the AppKit sidebar's chevron + depth indent.
                ui.horizontal(|ui| {
                    if pin.collapsible {
                        let chev = if pin.expanded { "▾" } else { "▸" };
                        let cr = ui.selectable_label(false, egui::RichText::new(chev).color(c(colors.dim)).small());
                        if cr.clicked() {
                            events.push(JiraEvent::Sidebar(SidebarEvent::TogglePin(pin.key.clone())));
                        }
                    } else if pin.depth > 0 {
                        ui.add_space(18.0);
                    }
                    let label = egui::RichText::new(pin.label.as_str()).color(c(colors.text));
                    let resp = ui.selectable_label(sel, label);
                    let resp = if pin.tip.is_empty() { resp } else { resp.on_hover_text(&pin.tip) };
                    if resp.clicked() {
                        events.push(JiraEvent::Sidebar(SidebarEvent::Pinned(pin.key.clone())));
                    }
                });
            }
            if !m.section_title.is_empty() {
                widgets::section_label(ui, &m.section_title, colors);
            }
            for (i, row) in m.rows.iter().enumerate() {
                let label = egui::RichText::new(row.title.as_str()).color(c(colors.text));
                let resp = ui.selectable_label(m.selected == Some(i), label);
                if resp.clicked() {
                    events.push(JiraEvent::Sidebar(SidebarEvent::Select(i)));
                }
            }
            // --- status line (`syncSummary` status dot) ---
            if let Some((text, tone)) = &m.status {
                ui.label(egui::RichText::new(text.as_str()).color(c(colors.tone(*tone))).small());
            }
        });
    }

    /// The search field (`JiraView.controlTextDidChange:`). The field follows
    /// `snap.query` except while it is being edited (typing must not be
    /// clobbered), mirroring `apply`.
    fn search(&mut self, ui: &mut egui::Ui, snap: &JiraSnapshot, events: &mut Vec<JiraEvent>) {
        let id = ui.make_persistent_id("jira-filter");
        let focused = ui.memory(|m| m.has_focus(id));
        if !focused && self.filter != snap.query {
            self.filter = snap.query.clone();
        }
        let resp = ui.add(
            egui::TextEdit::singleline(&mut self.filter)
                .hint_text("search...")
                .desired_width(f32::INFINITY)
                .id(id),
        );
        if resp.changed() {
            events.push(JiraEvent::Query(self.filter.clone()));
        }
    }

    /// The header + issue rows (`PopupTableHeaderView` + `JiraTableView.draw`).
    fn table(
        &self,
        ui: &mut egui::Ui,
        snap: &JiraSnapshot,
        colors: &PopupColors,
        events: &mut Vec<JiraEvent>,
    ) {
        if snap.rows.is_empty() {
            // `JiraTableView.draw`: the empty-state line.
            ui.add_space(12.0);
            ui.label(egui::RichText::new(snap.empty.as_str()).color(c(colors.dim)).small());
            return;
        }
        let rules = snap.rules.clone();
        let mut builder = TableBuilder::new(ui)
            .striped(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .sense(egui::Sense::click())
            .column(Column::exact(GUTTER));
        for _ in &snap.columns {
            builder = builder.column(Column::remainder().at_least(40.0).clip(true));
        }

        builder
            .header(HEADER_H as f32, |mut header| {
                // --- lead gutter: the "fit columns" glyph (`PopupTableHeaderView`) ---
                header.col(|ui| {
                    ui.label(egui::RichText::new("↔").color(c(colors.dim.with_alpha(0.9))).small());
                });
                for (i, col) in snap.columns.iter().enumerate() {
                    header.col(|ui| {
                        let sorted = snap.sort.filter(|(si, _)| *si == i).map(|(_, asc)| asc);
                        let color = if sorted.is_some() { colors.accent_on() } else { colors.dim };
                        let label = egui::RichText::new(header_label(&col.title, sorted))
                            .color(c(color))
                            .small()
                            .strong();
                        // `PopupTableHeaderView.mouseDown:` — sort by the column.
                        if ui.selectable_label(false, label).clicked() {
                            events.push(JiraEvent::Sort(i));
                        }
                    });
                }
            })
            .body(|body| {
                body.rows(ROW_H as f32, snap.rows.len(), |mut row| {
                    let i = row.index();
                    let r = &snap.rows[i];
                    let key = r.get("key").cloned().unwrap_or_default();
                    row.set_selected(i == snap.selection);

                    // --- check + star lead-in (`JiraTableView.draw` / `mouseDown:`) ---
                    let mut lead_consumed = false;
                    row.col(|ui| {
                        ui.horizontal(|ui| {
                            let mut on = snap.checked.contains(&key);
                            if ui.checkbox(&mut on, "").changed() {
                                events.push(JiraEvent::Check(i));
                                lead_consumed = true;
                            }
                            // Board rows: the ☆ is the sidebar pin, not a favourite.
                            let on = key
                                .strip_prefix(BOARD_PREFIX)
                                .map(|id| snap.pinned_boards.contains(id))
                                .unwrap_or_else(|| snap.favs.contains(&key));
                            let star = if on { "★" } else { "☆" };
                            if ui.selectable_label(on, star).clicked() {
                                events.push(JiraEvent::Star(i));
                                lead_consumed = true;
                            }
                        });
                    });

                    // --- cells (`JiraTableView.draw` column loop) ---
                    for col in &snap.columns {
                        let flat = display_cell(
                            &col.field,
                            r.get(&col.field).map(String::as_str).unwrap_or(""),
                        );
                        row.col(|ui| {
                            if flat.is_empty() {
                                return;
                            }
                            let st = rules.as_ref().and_then(|r| cell_style(r, &col.field, &flat));
                            let mut rt = egui::RichText::new(flat.clone()).color(c(colors.text));
                            if let Some(s) = &st {
                                if s.bold == Some(true) {
                                    rt = rt.strong();
                                }
                                if let Some(tone) = &s.tone {
                                    rt = rt.color(c(colors.tone(tone_of(Some(tone)))));
                                }
                            }
                            match col.align.as_str() {
                                "right" => {
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.label(rt);
                                        },
                                    );
                                }
                                "center" => {
                                    ui.with_layout(
                                        egui::Layout::centered_and_justified(
                                            egui::Direction::LeftToRight,
                                        ),
                                        |ui| {
                                            ui.label(rt);
                                        },
                                    );
                                }
                                _ => {
                                    ui.label(rt);
                                }
                            }
                        });
                    }

                    // `mouseDown:` on the row body selects it; a double-click
                    // opens the detail (`OpenDetail`).
                    if !lead_consumed {
                        let resp = row.response();
                        if resp.clicked() {
                            events.push(JiraEvent::Click { row: i, double: resp.double_clicked() });
                        }
                    }
                });
            });
    }

    /// The right inspector (`JiraInspectorView.draw`): key, title, chips,
    /// body, fields, then the "Open full detail" button.
    fn inspector(
        &mut self,
        ui: &mut egui::Ui,
        snap: &JiraSnapshot,
        colors: &PopupColors,
        events: &mut Vec<JiraEvent>,
    ) {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let Some(ic) = &snap.inspector else {
                ui.add_space(12.0);
                ui.label(egui::RichText::new("Select an issue").color(c(colors.dim)));
                return;
            };
            ui.label(egui::RichText::new(ic.key.as_str()).color(c(colors.accent)).strong());
            ui.label(egui::RichText::new(ic.title.as_str()).color(c(colors.text)).heading());
            if !ic.chips.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    for (text, tone) in &ic.chips {
                        widgets::badge(ui, text, colors.tone(*tone));
                    }
                });
            }
            if !ic.body.is_empty() {
                let body = if ic.body.chars().count() > 700 {
                    ic.body.chars().take(700).collect::<String>() + "…"
                } else {
                    ic.body.clone()
                };
                ui.label(egui::RichText::new(body).color(c(colors.text)));
            }
            for (label, value) in &ic.fields {
                if value.is_empty() {
                    continue;
                }
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(label.as_str()).color(c(colors.dim)).small());
                    ui.label(egui::RichText::new(value.as_str()).color(c(colors.text)));
                });
            }
            if widgets::capsule_accent(ui, "Open full detail ⏎", colors).clicked() {
                events.push(JiraEvent::OpenDetail);
            }
            // The rail collapse (AppKit's rail button at the panel's foot;
            // the strip in `ui()` reopens it). The egui pane is its own
            // "host", so it lands on the model seam directly.
            if widgets::rail_button(ui, false, colors) {
                self.model.set_inspector_shown(Some(false));
            }
        });
    }
}

impl Pane for JiraPane {
    fn ui(&mut self, ui: &mut egui::Ui, colors: &PopupColors) {
        JiraPane::ui(self, ui, colors);
    }

    fn has_rail(&self) -> bool {
        true
    }

    fn rail(&self) -> bool {
        self.rail
    }

    /// Flip the sidebar rail (the `SidebarView` collapse mirror). The pane
    /// owns its `sidebarRail.<view>` persistence, like the AppKit sidebar.
    fn set_rail(&mut self, on: bool) {
        self.rail = on;
        crate::ui::sidebar::set_rail_default("jira", on);
    }

    fn apply_command(&mut self, cmd: &UiCommand) {
        match cmd {
            UiCommand::JiraInspector(on) => self.model.set_inspector_shown(*on),
            UiCommand::InspectorWidth(w) => {
                self.inspector_w = (*w as f32).clamp(widgets::RAIL_W, 640.0)
            }
            _ => {}
        }
    }

    /// The resolved key actions the Jira model consumes (the same three the
    /// AppKit `JiraView` delegate routes: Escape, list up/down, Enter).
    fn handle_key(&mut self, action: KeyAction) {
        match action {
            KeyAction::Escape(_) => self.model.handle(JiraEvent::Escape),
            KeyAction::ListMove(delta) => self.model.handle(JiraEvent::Move(delta as i64)),
            KeyAction::ListAccept => self.model.handle(JiraEvent::Enter),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_label_appends_the_sort_arrow() {
        assert_eq!(header_label("status", None), "STATUS");
        assert_eq!(header_label("status", Some(true)), "STATUS ↑");
        assert_eq!(header_label("status", Some(false)), "STATUS ↓");
        // The sort arrow rides outside the title's own case folding.
        assert_eq!(header_label("Fix Version", Some(true)), "FIX VERSION ↑");
    }
}
