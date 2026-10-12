//! The Notes view — Wave 5 egui port.
//!
//! It drives the same pure [`crate::views::notes::NotesView`] model the AppKit
//! [`crate::views::notes::NotesSurface`] drives: pull a [`NotesSnapshot`] each
//! frame, draw it with egui widgets, collect the user's actions as
//! [`NotesEvent`]s, and hand them to [`NotesView::handle`] — the exact same
//! event contract the AppKit surface reports. Each section below is commented
//! with the `notes.rs` region it mirrors.

use std::path::Path;

use crate::ui::theme::PopupColors;
use crate::views::notes::{NotesConfig, NotesEvent, NotesSnapshot, NotesView};
use crate::views::notes_tree::{NotesTree, TreeRow};

use egui_commonmark::{CommonMarkCache, CommonMarkViewer};

use super::app::Pane;
use super::theme::c;

pub struct NotesPane {
    pub model: NotesView,
    /// The editor's live text buffer (`notes.rs` `NotesTextView`); follows
    /// `snapshot.text` except while it is being edited (typing must not be
    /// clobbered), exactly like `files.rs`'s filter field.
    editor: String,
    /// Whether the prose (rendered markdown) view is showing instead of the
    /// raw editor (`notes.rs` `set_prose`).
    prose: bool,
    /// `egui_commonmark`'s per-frame parse/layout cache.
    cache: CommonMarkCache,
}

impl NotesPane {
    /// Build the model like `NotesSurface::build_surface`'s pure half (the AppKit
    /// `NSView` wiring is the macOS-only half and is skipped here). Config is
    /// read with the pure `section_vars` scan (`files.rs`'s precedent) so the
    /// pane needs no Python helper.
    pub fn build() -> Self {
        let text = crate::app::config::read_config_text().unwrap_or_default();
        let section = crate::views::tools::section_vars(&text, "notes");
        let raw = section.get("paths").cloned().unwrap_or_default();

        let mut model = NotesView::new(NotesConfig::default());
        // Directories in `[notes] paths` become pinned tree roots; files stay tabs.
        let (roots, paths) = crate::views::notes_tree::split_roots(
            &raw,
            &|p| Path::new(p).is_dir(),
            &|p| Path::new(p).exists(),
        );
        for p in &paths {
            model.tabs.append(p);
        }
        let show_hidden = matches!(
            section.get("tree-show-hidden").map(String::as_str),
            Some("true") | Some("yes") | Some("on") | Some("1")
        );
        let mut expanded = crate::views::notes_tree::load_expanded();
        if expanded.is_empty() {
            expanded.extend(roots.iter().take(1).cloned());
        }
        let mut tree = NotesTree::new(roots, expanded, show_hidden);
        if let Some(p) = model.tabs.selected_path() {
            tree.reveal(p);
        }
        model.tree = tree;
        // Point the editor at the selected tab and read its file (`load_selected`).
        if !model.tabs.is_empty() {
            let sel = model.tabs.selected;
            model.handle(NotesEvent::SelectTab(sel));
        }
        let editor = model.text.clone();
        NotesPane { model, editor, prose: false, cache: CommonMarkCache::default() }
    }

    /// Draw one frame and apply the events it generated. `colors` are the
    /// `[notes]` section tokens (from [`crate::ui::theme::section_colors`]).
    pub fn ui(&mut self, ui: &mut egui::Ui, colors: &PopupColors) {
        let snap = self.model.snapshot();
        let mut events: Vec<NotesEvent> = Vec::new();

        // --- pinned-folder tree | (tabs + editor) split (`notes.rs` `build_surface`) ---
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(220.0, ui.available_height()),
                egui::Layout::top_down(egui::Align::Min),
                |ui| self.tree_sidebar(ui, &snap, colors, &mut events),
            );
            ui.separator();
            ui.vertical(|ui| {
                self.toolbar(ui, &snap, colors, &mut events);
                ui.separator();
                self.editor(ui, &snap, colors, &mut events);
                ui.separator();
                // --- status line (`notes.rs` `NotesSurface` bottom bar) ---
                ui.horizontal(|ui| {
                    let path = snap.path.as_deref().unwrap_or("no file open");
                    ui.label(egui::RichText::new(path).color(c(colors.dim)).small());
                    ui.label(
                        egui::RichText::new(format!("{} chars", snap.text.chars().count()))
                            .color(c(colors.dim))
                            .small(),
                    );
                });
            });
        });

        for ev in events {
            self.model.handle(ev);
        }
    }

    /// The left tree column (`notes.rs` `TreeView`): pinned roots + expanded
    /// children, folded by `▸`/`▾`. A click reports the same `TreeClick` the
    /// AppKit tree emits.
    fn tree_sidebar(
        &self,
        ui: &mut egui::Ui,
        snap: &NotesSnapshot,
        colors: &PopupColors,
        events: &mut Vec<NotesEvent>,
    ) {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            if snap.tree.is_empty() {
                ui.label(egui::RichText::new("No pinned folders").color(c(colors.dim)).small());
            }
            for row in &snap.tree {
                let label = egui::RichText::new(tree_label(row)).color(c(colors.text));
                let selected = snap.tree_selected.as_deref() == Some(row.path.as_str());
                if ui.selectable_label(selected, label).clicked() {
                    events.push(NotesEvent::TreeClick {
                        path: row.path.clone(),
                        is_dir: row.is_dir,
                    });
                }
            }
        });
    }

    /// The open-note tabs (`PopupTabsBar`) plus the Save / Prose controls.
    fn toolbar(
        &mut self,
        ui: &mut egui::Ui,
        snap: &NotesSnapshot,
        colors: &PopupColors,
        events: &mut Vec<NotesEvent>,
    ) {
        ui.horizontal_wrapped(|ui| {
            for (i, tab) in snap.tabs.iter().enumerate() {
                let label = egui::RichText::new(tab.title.as_str()).color(c(colors.text));
                if ui.selectable_label(i == snap.selected, label).clicked() {
                    events.push(NotesEvent::SelectTab(i));
                }
                if ui.small_button("×").on_hover_text("Close tab").clicked() {
                    events.push(NotesEvent::CloseTab(i));
                }
            }
        });
        ui.horizontal(|ui| {
            if ui.add_enabled(snap.path.is_some(), egui::Button::new("Save")).clicked() {
                self.save(events);
            }
            if ui.selectable_label(self.prose, "Prose").clicked() {
                self.prose = !self.prose;
            }
        });
    }

    /// The editor (`notes.rs` `NotesTextView`) or, in prose mode, the rendered
    /// markdown (`egui_commonmark`, mirroring the AppKit `prose` pane).
    fn editor(
        &mut self,
        ui: &mut egui::Ui,
        snap: &NotesSnapshot,
        colors: &PopupColors,
        events: &mut Vec<NotesEvent>,
    ) {
        if snap.path.is_none() {
            ui.label(egui::RichText::new("No note open").color(c(colors.dim)));
            return;
        }
        if self.prose {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                CommonMarkViewer::new().show(ui, &mut self.cache, &self.editor);
            });
            return;
        }
        // Mirror the files filter: the field follows `snap.text` except while it
        // is being edited (a tab switch with the editor unfocused reloads it).
        let id = ui.make_persistent_id("notes-editor");
        let focused = ui.memory(|m| m.has_focus(id));
        if !focused && self.editor != snap.text {
            self.editor = snap.text.clone();
        }
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let resp = ui.add(
                egui::TextEdit::multiline(&mut self.editor)
                    .id(id)
                    .desired_width(f32::INFINITY)
                    .desired_rows(20),
            );
            if resp.changed() {
                events.push(NotesEvent::Edit(self.editor.clone()));
            }
        });
    }

    /// `commitSave`: push the live buffer first so `Save` writes exactly what is
    /// on screen, then the same `Save` event the AppKit menu item sends.
    fn save(&mut self, events: &mut Vec<NotesEvent>) {
        events.push(NotesEvent::Edit(self.editor.clone()));
        events.push(NotesEvent::Save);
    }
}

/// One tree row's display label: indent by depth, a `▾`/`▸` marker for folders
/// and a blank marker for files, then the name (mirrors the AppKit tree text).
fn tree_label(row: &TreeRow) -> String {
    let marker = if row.is_dir {
        if row.expanded {
            "▾ "
        } else {
            "▸ "
        }
    } else {
        "  "
    };
    format!("{}{marker}{}", "    ".repeat(row.depth), row.name)
}

impl Pane for NotesPane {
    fn ui(&mut self, ui: &mut egui::Ui, colors: &PopupColors) {
        NotesPane::ui(self, ui, colors);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, depth: usize, is_dir: bool, expanded: bool) -> TreeRow {
        TreeRow {
            path: format!("/n/{name}"),
            name: name.to_string(),
            depth,
            is_dir,
            expanded,
            is_root: depth == 0,
        }
    }

    #[test]
    fn tree_label_indents_and_marks_folders() {
        assert_eq!(tree_label(&row("notes", 0, true, true)), "▾ notes");
        assert_eq!(tree_label(&row("work", 1, true, false)), "    ▸ work");
        assert_eq!(tree_label(&row("a.md", 1, false, false)), "      a.md");
        assert_eq!(tree_label(&row("deep.md", 3, false, false)), "              deep.md");
    }
}
