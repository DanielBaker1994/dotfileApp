//! The Notes view — Wave 5 egui port.
//!
//! It drives the same pure [`crate::views::notes::NotesView`] model the AppKit
//! [`crate::views::notes::NotesSurface`] drives: pull a [`NotesSnapshot`] each
//! frame, draw it with egui widgets, collect the user's actions as
//! [`NotesEvent`]s, and hand them to [`NotesView::handle`] — the exact same
//! event contract the AppKit surface reports. Each section below is commented
//! with the `notes.rs` region it mirrors.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::ui::theme::PopupColors;
use crate::views::notes::{NotesConfig, NotesEvent, NotesSnapshot, NotesView};
use crate::views::notes_tree::{NotesTree, TreeRow};

use egui_commonmark::{CommonMarkCache, CommonMarkViewer};

use super::app::Pane;
use super::terminal::TerminalView;
use super::theme::c;
use super::widgets;

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
    /// The embedded `nvim` editor (the egui counterpart of the AppKit notes vim
    /// pane): built on the first "Edit in nvim" click and dropped — killing the
    /// child — when the mode is turned back off.
    nvim: Option<TerminalView>,
    /// The note the running `nvim` was started on (a tab switch restarts it).
    nvim_path: Option<String>,
    /// Whether `[notes] vim-bin` resolves to an executable; when false the
    /// toggle is disabled and the plain editor stays.
    nvim_available: bool,
    /// `[notes] vim-bin` (default `"nvim"`), the AppKit setting name.
    nvim_bin: String,
    /// The asset dir the pure notes-vim arg builder needs (`…/vim/init.lua`).
    asset_dir: String,
    /// The sidebar collapse-to-rail state (`SidebarView`'s rail; persisted
    /// as `sidebarRail.notes`, toggled by Ctrl+B B / `do:rail:notes`).
    rail: bool,
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
        let vim_bin = section
            .get("vim-bin")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "nvim".to_string());
        let asset_dir = crate::app::paths::Paths::from_env().asset_dir().to_string();
        let nvim_available =
            program_available(&vim_bin, &std::env::var("PATH").unwrap_or_default());
        let editor = model.text.clone();
        NotesPane {
            model,
            editor,
            prose: false,
            cache: CommonMarkCache::default(),
            nvim: None,
            nvim_path: None,
            nvim_available,
            nvim_bin: vim_bin,
            asset_dir,
            rail: crate::ui::sidebar::rail_default("notes"),
        }
    }

    /// Draw one frame and apply the events it generated. `colors` are the
    /// `[notes]` section tokens (from [`crate::ui::theme::section_colors`]).
    pub fn ui(&mut self, ui: &mut egui::Ui, colors: &PopupColors) {
        let snap = self.model.snapshot();
        let mut events: Vec<NotesEvent> = Vec::new();

        // --- pinned-folder tree | (tabs + editor) split (`notes.rs` `build_surface`) ---
        ui.horizontal_top(|ui| {
            if self.rail {
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
                    egui::vec2(220.0, ui.available_height()),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        self.tree_sidebar(ui, &snap, colors, &mut events);
                        if widgets::rail_button(ui, false, colors) {
                            self.set_rail(true);
                        }
                    },
                );
            }
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
                self.set_prose(!self.prose);
            }
            // The "Edit in nvim" toggle (the egui counterpart of the AppKit
            // notes vim pane; AppKit gates it on `[notes] vim-mode` and has no
            // toggle key, so this is a button only). With no `nvim` on PATH the
            // label stays and the plain editor does.
            if self.nvim_available {
                let label = if self.nvim.is_some() { "Exit nvim" } else { "Edit in nvim" };
                if ui.add_enabled(snap.path.is_some(), egui::Button::new(label)).clicked() {
                    self.toggle_vim(snap);
                }
            } else {
                ui.label(
                    egui::RichText::new("nvim not found").color(c(colors.dim)).small(),
                )
                .on_hover_text(format!("{} is not on PATH", self.nvim_bin));
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
            self.stop_vim();
            ui.label(egui::RichText::new("No note open").color(c(colors.dim)));
            return;
        }
        // The embedded `nvim` editor (a `TerminalView`) replaces the plain
        // editor; a tab switch restarts it on the newly selected note.
        if self.nvim.is_some() {
            if let Some(path) = snap.path.as_deref() {
                if self.nvim_path.as_deref() != Some(path) {
                    self.start_vim(path);
                }
            }
            if let Some(term) = self.nvim.as_mut() {
                term.ui(ui);
            }
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

    /// Toggle the prose view; nvim and prose are mutually exclusive (the AppKit
    /// `ProseModeSwitch`), so turning prose on shuts the editor down.
    fn set_prose(&mut self, on: bool) {
        self.prose = on;
        if on {
            self.stop_vim();
        }
    }

    /// The "Edit in nvim" toggle: start the editor on the selected note, or
    /// drop it (killing the child) when it is already running.
    fn toggle_vim(&mut self, snap: &NotesSnapshot) {
        if self.nvim.is_some() {
            self.stop_vim();
        } else if let Some(path) = snap.path.as_deref() {
            self.start_vim(path);
        }
    }

    /// Start `nvim` on `path` in a [`TerminalView`], using the pure notes-vim
    /// arg/env builder. Needs the `--listen` socket's directory, cleared first
    /// like `VimPane::restart`.
    fn start_vim(&mut self, path: &str) {
        let pid = std::process::id() as i32;
        let sock = crate::views::notes_vim::socket_path(pid);
        let _ = std::fs::remove_file(&sock);
        if let Some(dir) = Path::new(&sock).parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let (args, env, cwd) = vim_command_for(&self.asset_dir, path);
        self.prose = false;
        self.nvim = Some(TerminalView::new(self.nvim_bin.clone(), args, cwd, env));
        self.nvim_path = Some(path.to_string());
    }

    /// Drop the editor; [`TerminalView`]'s `Drop` sends `Msg::Shutdown`, so the
    /// `nvim` child is killed.
    fn stop_vim(&mut self) {
        self.nvim = None;
        self.nvim_path = None;
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

/// Whether `bin` is an executable: an absolute/relative path is checked
/// directly, a bare name is resolved through the `:`-separated `path_var`
/// (the same lookup the AppKit notes vim pane does). Pure (testable with an
/// explicit `path_var`), so the "nvim is not on PATH" case is deterministic.
fn program_available(bin: &str, path_var: &str) -> bool {
    if bin.contains('/') {
        return is_executable(Path::new(bin));
    }
    path_var
        .split(':')
        .filter(|d| !d.is_empty())
        .any(|d| is_executable(&Path::new(d).join(bin)))
}

/// A path is executable when it is a file with any execute bit set.
#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.is_file() && (m.permissions().mode() & 0o111) != 0)
        .unwrap_or(false)
}

/// Windows twin: no mode bits, so reuse the shared extension heuristic (a real
/// file whose extension is one of the Windows-executable ones).
#[cfg(windows)]
fn is_executable(path: &Path) -> bool {
    crate::app::python_helper::is_executable_windows_ext(path)
}

/// The directory to start `nvim` in for the note `file`: its parent, else
/// `$HOME` (mirrors `VimPane::note_dir`).
fn note_dir(file: &str) -> Option<String> {
    if let Some(parent) = Path::new(file).parent() {
        if !parent.as_os_str().is_empty() {
            return Some(parent.to_string_lossy().into_owned());
        }
    }
    std::env::var("HOME").ok()
}

/// The `nvim` args + env + working directory for `file`, built entirely by the
/// pure builders in [`crate::views::notes_vim`] ([`VimLaunch::args`] /
/// [`launch_env`]) — never reimplemented here. `asset_dir` supplies
/// `…/vim/init.lua`.
///
/// [`VimLaunch::args`]: crate::views::notes_vim::VimLaunch::args
fn vim_command_for(
    asset_dir: &str,
    file: &str,
) -> (Vec<String>, HashMap<String, String>, Option<PathBuf>) {
    let mut launch = crate::views::notes_vim::VimLaunch::for_pid(std::process::id() as i32);
    launch.cwd = note_dir(file);
    let args = launch.args(asset_dir, Some(file));
    let env: HashMap<String, String> = crate::views::notes_vim::launch_env().into_iter().collect();
    let cwd = launch.cwd.map(PathBuf::from);
    (args, env, cwd)
}

impl Pane for NotesPane {
    fn ui(&mut self, ui: &mut egui::Ui, colors: &PopupColors) {
        NotesPane::ui(self, ui, colors);
    }

    fn has_rail(&self) -> bool {
        true
    }

    fn rail(&self) -> bool {
        self.rail
    }

    /// Flip the sidebar rail; the pane owns its `sidebarRail.notes`
    /// persistence, like the AppKit `SidebarView`.
    fn set_rail(&mut self, on: bool) {
        self.rail = on;
        crate::ui::sidebar::set_rail_default("notes", on);
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

    #[cfg(unix)]
    #[test]
    fn program_available_probes_an_explicit_path() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("ws-notes-nvim-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let bin = dir.join("fakenvim");
        std::fs::write(&bin, b"#!/bin/sh\n").unwrap();

        // Present but not executable: not available.
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!program_available("fakenvim", dir.to_str().unwrap()));

        // Executable on the explicit PATH: available.
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(program_available("fakenvim", dir.to_str().unwrap()));

        // An absolute path is checked directly (no PATH needed).
        assert!(program_available(bin.to_str().unwrap(), ""));

        // A missing name is never available.
        assert!(!program_available("definitely-not-a-real-binary-xyz", dir.to_str().unwrap()));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn note_dir_is_the_parent_else_home() {
        assert_eq!(note_dir("/notes/a.md").as_deref(), Some("/notes"));
        assert_eq!(note_dir("/notes/deep/a.md").as_deref(), Some("/notes/deep"));
        // A bare file name falls back to `$HOME` (whatever it is).
        assert_eq!(note_dir("a.md"), std::env::var("HOME").ok());
    }

    #[test]
    fn vim_command_for_uses_the_notes_vim_builder() {
        let (args, env, cwd) = vim_command_for("/asset", "/notes/a.md");
        assert_eq!(args[0], "--listen");
        assert_eq!(&args[2..4], &["-u", "/asset/vim/init.lua"]);
        assert!(args.iter().any(|a| a.starts_with("let g:ws_sock=")));
        assert!(args.iter().any(|a| a.starts_with("let g:ws_img_file=")));
        assert_eq!(args.last().unwrap(), "/notes/a.md");
        // The env carries the AppKit notes-vim contract (PATH recovery, TERM).
        assert_eq!(env.get("TERM").map(String::as_str), Some("xterm-256color"));
        assert!(env.contains_key("PATH"));
        assert_eq!(cwd.as_deref(), Some(Path::new("/notes")));
    }
}
