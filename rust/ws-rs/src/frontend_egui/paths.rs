//! The Paths tool panel — the egui shelf view (`frontend_egui/files.rs` is the
//! reference; this copies its shape).
//!
//! It drives the same pure [`crate::views::paths::PathsWindow`] model the AppKit
//! shelf drives: pull a [`PathsSnapshot`] each frame, draw it with egui widgets,
//! collect the user's actions as [`PathsEvent`]s, and hand them to
//! [`PathsWindow::handle`] — the exact same event contract the AppKit surface
//! reports. Each section below is commented with the `views/paths.rs` region it
//! mirrors.

use std::path::Path;

use crate::engines::path_shelf::{Item, Why};
use crate::ui::popup::{EditOp, KeyAction};
use crate::ui::theme::PopupColors;
use crate::views::paths::{PathsEvent, PathsSnapshot, PathsWindow, ReturnAction};

use super::app::Pane;
use super::theme::c;

/// The Paths shelf panel.
pub struct PathsPane {
    pub model: PathsWindow,
    /// The query field's editable buffer (`views/paths.rs` `search` NSTextField).
    query: String,
}

impl PathsPane {
    /// Build the model like `run_paths_cmd`'s `PathsCmd::Show` arm: read the
    /// `[paths]` config for the Return action, then seed the model with the
    /// shelf store's items. The AppKit-only half (`build_macos` /
    /// `show_window`) is skipped here.
    pub fn build() -> Self {
        let text = crate::app::config::read_config_text().unwrap_or_default();
        let section = crate::views::tools::section_vars(&text, "paths");
        let action = section.get("return").map(|s| s.trim()).unwrap_or("file");
        let mut model = PathsWindow::from_action(action);
        model.set_items(load_shelf(&shelf_store_path()));
        PathsPane { model, query: String::new() }
    }

    /// Draw one frame and apply the events it generated. `colors` are the
    /// `[paths]` section tokens (from [`crate::ui::theme::section_colors`]).
    pub fn ui(&mut self, ui: &mut egui::Ui, colors: &PopupColors) {
        let snap = self.model.snapshot();
        let mut events: Vec<PathsEvent> = Vec::new();

        // --- query field | row list | actions (`views/paths.rs` `build_macos`) ---
        self.query_field(ui, &snap, &mut events);
        ui.separator();
        self.list(ui, &snap, colors, &mut events);
        ui.separator();
        self.toolbar(ui, &snap, colors);

        for ev in events {
            // Return runs the shelf's Return action (`paths_return`): the
            // pasteboard / open follow-ups are the frontend's job, done before
            // the model hands the event on (Enter then hides the shelf).
            if matches!(ev, PathsEvent::Enter) {
                self.run_return();
            }
            self.model.handle(ev);
        }
    }

    /// The query field (`views/paths.rs` `build_macos` search box + `set_query`).
    /// Mirror `set_query`: the field follows `snap.query` except while it is
    /// being edited (typing must not be clobbered).
    fn query_field(
        &mut self,
        ui: &mut egui::Ui,
        snap: &PathsSnapshot,
        events: &mut Vec<PathsEvent>,
    ) {
        let id = ui.make_persistent_id("paths-query");
        let focused = ui.memory(|m| m.has_focus(id));
        if !focused && self.query != snap.query {
            self.query = snap.query.clone();
        }
        let resp = ui.add(
            egui::TextEdit::singleline(&mut self.query)
                .hint_text("Filter recent files…")
                .id(id),
        );
        if resp.changed() {
            events.push(PathsEvent::Query(self.query.clone()));
        }
    }

    /// The scrollable row list (`views/paths.rs` `sync_view`'s `FileListView`
    /// rows). With no rows the empty hint (`is_empty_text`) is drawn instead.
    fn list(
        &self,
        ui: &mut egui::Ui,
        snap: &PathsSnapshot,
        colors: &PopupColors,
        events: &mut Vec<PathsEvent>,
    ) {
        if snap.rows.is_empty() {
            ui.label(egui::RichText::new(snap.status.as_str()).color(c(colors.dim)));
            return;
        }
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for (i, row) in snap.rows.iter().enumerate() {
                let label = row_label(&row.name, &row.trailing);
                let resp = ui.selectable_label(i == snap.selection, label);
                if resp.clicked() {
                    events.push(PathsEvent::Click { row: i, double: resp.double_clicked() });
                }
            }
        });
    }

    /// The actions the shelf's keys drive (`views/paths.rs` `PathsWindow.key`),
    /// as buttons: copy the selected path text (`copy_paths_text`) and open the
    /// selection (`open_paths`).
    fn toolbar(&self, ui: &mut egui::Ui, snap: &PathsSnapshot, colors: &PopupColors) {
        ui.horizontal(|ui| {
            if ui.button("Copy path").on_hover_text("Copy the selected path").clicked() {
                let _ = self.copy_selected();
            }
            if ui.button("Open").clicked() {
                self.open_selected();
            }
            if let Some(row) = snap.rows.get(snap.selection) {
                ui.label(egui::RichText::new(row.path.as_str()).color(c(colors.dim)).small());
            }
        });
    }

    /// `copyPaths()` → the newline-joined path text on the clipboard (the
    /// text-only [`crate::platform::clipboard`] seam).
    fn copy_selected(&self) -> bool {
        let text = self.model.copy_paths_text();
        if text.is_empty() {
            return false;
        }
        crate::platform::clipboard::set_text(&text)
    }

    /// `open_paths()` → hand each selected path to the OS.
    fn open_selected(&self) {
        for p in self.model.selected_paths() {
            let _ = crate::platform::open_path(Path::new(&p));
        }
    }

    /// `paths_return()`: the Return action decides what Enter does. Path/File
    /// copy the selected paths (text-only here); Open launches them.
    fn run_return(&mut self) {
        match self.model.return_action {
            ReturnAction::Open => self.open_selected(),
            ReturnAction::File | ReturnAction::Path => {
                let _ = self.copy_selected();
            }
        }
    }
}

impl Pane for PathsPane {
    fn ui(&mut self, ui: &mut egui::Ui, colors: &PopupColors) {
        PathsPane::ui(self, ui, colors);
    }

    /// The resolved key actions the shelf consumes (`views/paths.rs`
    /// `key_action` → `PathsWindow.key`): Escape, list up/down, Enter, and
    /// Cmd+C (copy the selected path text).
    fn handle_key(&mut self, action: KeyAction) {
        match action {
            KeyAction::Escape(_) => self.model.handle(PathsEvent::Escape),
            KeyAction::ListMove(delta) => self.model.handle(PathsEvent::Move(delta as i64)),
            KeyAction::ListAccept => {
                self.run_return();
                self.model.handle(PathsEvent::Enter);
            }
            KeyAction::Edit(EditOp::Copy) => {
                let _ = self.copy_selected();
            }
            _ => {}
        }
    }
}

/// One list row's label — the file name, then the `folder · age · why` text the
/// model built (`views/paths.rs` `trailing`).
fn row_label(name: &str, trailing: &str) -> String {
    if trailing.is_empty() {
        name.to_string()
    } else {
        format!("{name}   {trailing}")
    }
}

/// `PathShelf::new`'s store (`~/.cache/kitchen-sink/paths.json`).
fn shelf_store_path() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    format!("{home}/.cache/kitchen-sink/paths.json")
}

/// Read the shelf store and build the model's [`Item`]s. A missing or malformed
/// store yields no items — the shelf shows its empty hint.
fn load_shelf(store: &str) -> Vec<Item> {
    let Ok(bytes) = std::fs::read(store) else {
        return Vec::new();
    };
    let Ok(raw) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return Vec::new();
    };
    parse_items(&raw)
}

/// `items_from_value`: keep `{path, at, why}` rows with a known `why`.
fn parse_items(raw: &serde_json::Value) -> Vec<Item> {
    raw.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|d| {
                    let path = d.get("path")?.as_str()?.to_string();
                    let at = d
                        .get("at")
                        .and_then(serde_json::Value::as_f64)
                        .unwrap_or(0.0);
                    let why = Why::from_raw(
                        d.get("why").and_then(serde_json::Value::as_str).unwrap_or("modified"),
                    )?;
                    Some(Item { path, at, why })
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_label_joins_name_and_trailing() {
        assert_eq!(row_label("a.txt", "~/x · now · copied"), "a.txt   ~/x · now · copied");
        assert_eq!(row_label("a.txt", ""), "a.txt", "no trailing text → just the name");
    }

    #[test]
    fn parse_items_keeps_known_whys() {
        let raw = serde_json::json!([
            {"path": "/a.txt", "at": 2.0, "why": "clipboard"},
            {"path": "/b.txt", "at": 1.0, "why": "bogus"},
            {"path": 7, "at": 1.0, "why": "created"},
        ]);
        let items = parse_items(&raw);
        assert_eq!(items.len(), 1, "unknown why and non-string path are dropped");
        assert_eq!(items[0].path, "/a.txt");
        assert_eq!(items[0].at, 2.0);
        assert_eq!(items[0].why, Why::Clipboard);
    }

    #[test]
    fn load_shelf_reads_a_store_round_trip() {
        let dir = std::env::temp_dir().join(format!("ws-egui-paths-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = dir.join("paths.json");
        std::fs::write(&store, br#"[{"path":"/a.txt","at":1.0,"why":"copied"}]"#).unwrap();
        let items = load_shelf(store.to_str().unwrap());
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].why, Why::Copied);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_shelf_missing_store_is_empty() {
        assert!(load_shelf("/no/such/kitchen-sink-paths.json").is_empty());
    }

    #[test]
    fn build_reads_the_return_action() {
        // The config's `[paths] return = "file"` default; the model maps it.
        let mut model = PathsWindow::from_action("file");
        model.set_items(Vec::new());
        let pane = PathsPane { model, query: String::new() };
        assert_eq!(pane.model.return_action, ReturnAction::File);
        assert!(pane.model.snapshot().status.contains("Nothing yet"));
    }
}
