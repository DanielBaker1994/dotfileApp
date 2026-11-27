//! The file actions every file-browser window shares: the main app's Files
//! view (`ws-rs` host) and the quick agent's Find Recent Files (`app/quick.rs`).
//!
//! ONE implementation. A window implements the handful of required
//! [`FilesHost`] methods (its browser, its window, how to get back to it on
//! the main thread, and the few actions that really differ) and gets every
//! other action from the provided methods. Never copy an action into a host:
//! a change here reaches both windows.

use std::cell::Cell;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use objc2::rc::Retained;
use objc2::MainThreadMarker;
use objc2_app_kit::NSWindow;

use crate::ui::menu_kit::MenuEntry;
use crate::views::files::{can_perform, Action, FileBrowser, PermContext};
use crate::views::screenshot::ScreenshotController;

thread_local! {
    /// Cmd+X in a file list: the next Cmd+V moves instead of copies.
    static FILES_CUT: Cell<bool> = const { Cell::new(false) };
    /// `(cwd, row)` of the last plain click: a second click on the same row
    /// (after the double-click window) renames it, like Finder.
    static LAST_CLICK: std::cell::RefCell<Option<(String, usize)>> = const { std::cell::RefCell::new(None) };
    /// Bumped to cancel a pending click-rename.
    static CLICK_RENAME_GEN: Cell<u64> = const { Cell::new(0) };
}

/// Cancel a pending click-to-rename (a double-click, a drag, a key).
pub fn cancel_click_rename() {
    CLICK_RENAME_GEN.with(|g| g.set(g.get() + 1));
}

/// Run `f` (start a rename) once the double-click window has passed, unless
/// cancelled meanwhile or the button is still down (a press-and-hold is a
/// drag; a rename field under the pointer would swallow it). Finder's
/// click-on-a-selected-row rename, for every list that has one.
pub fn schedule_click_rename(f: impl FnOnce() + Send + 'static) {
    cancel_click_rename();
    let gen = CLICK_RENAME_GEN.with(|g| g.get());
    let secs = objc2_app_kit::NSEvent::doubleClickInterval().max(0.2) + 0.05;
    let Ok(when) = dispatch2::DispatchTime::try_from(Duration::from_secs_f64(secs)) else { return };
    let _ = dispatch2::DispatchQueue::main().after(when, move || {
        let held = objc2_app_kit::NSEvent::pressedMouseButtons() & 1 != 0;
        if !held && CLICK_RENAME_GEN.with(|g| g.get()) == gen {
            f();
        }
    });
}

/// The file URLs on the general pasteboard, as paths.
pub fn pasteboard_file_paths() -> Vec<String> {
    use objc2_app_kit::NSPasteboard;
    let pb = NSPasteboard::generalPasteboard();
    let ty = objc2_foundation::NSString::from_str("public.file-url");
    pb.pasteboardItems()
        .map(|items| {
            items
                .iter()
                .filter_map(|it| it.stringForType(&ty))
                .filter_map(|s| objc2_foundation::NSURL::URLWithString(&s))
                .filter_map(|u| u.path().map(|p| p.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

fn file_name(p: &str) -> String {
    std::path::Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// A window that shows a [`FileBrowser`]. Implement the required methods; the
/// provided ones are the shared actions (menus, sheets, clipboard, trash, …).
pub trait FilesHost: Sized + 'static {
    // -- required: what differs per window ---------------------------------

    /// Run `f` against the live browser (and sync its view); `None` when the
    /// window has no browser.
    fn files_surface<R>(&self, f: impl FnOnce(&mut FileBrowser) -> R) -> Option<R>;
    /// The window sheets attach to.
    fn files_ns_window(&self) -> Option<Retained<NSWindow>>;
    /// Give the file list the keyboard.
    fn focus_files_list(&self);
    /// Open a file (not a folder) the way this window opens files.
    fn files_open_file(&self, path: &str);
    /// Esc in the list: what leaving means differs per window.
    fn files_escape(&self);
    /// `(the file list has the keyboard, a text field is being edited)`.
    fn files_focus(&self) -> (bool, bool);
    /// Put the keyboard in the filter / address field (`select_all`: its
    /// text selected, Go to Folder).
    fn files_focus_filter(&self, select_all: bool);
    /// A printable key typed in the list starts the filter with it; `true`
    /// when the window inserted it itself, `false` to let the field take it.
    fn files_type_ahead(&self, c: char) -> bool;
    /// The screenshot editor, for the pictures-only menu rows.
    fn files_screenshot(&self) -> Option<Arc<Mutex<ScreenshotController>>>;
    /// Run `f` against this window now, if it still exists (main thread:
    /// menu items and sheet callbacks).
    fn with_host(f: impl FnOnce(&Self));
    /// Run `f` against this window on a later main-queue turn (any thread).
    fn later(f: impl FnOnce(&Self) + Send + 'static);
    /// Remember a view preference (text zoom, preview shown). Default: no-op.
    fn files_remember(&self, _key: &str, _value: &str) {}

    // -- provided: the shared actions --------------------------------------

    fn files_selected(&self) -> Vec<String> {
        self.files_surface(|b| b.selected_paths()).unwrap_or_default()
    }

    /// Open the selected row: a folder (or `..`) opens in the window, a file
    /// the way the window opens files. `false` when nothing was opened.
    fn files_open_selected(&self) -> bool {
        use crate::views::files::{open_index, OpenOutcome};
        let outcome = self.files_surface(|b| {
            let rows = b.visible_rows().to_vec();
            open_index(&rows, b.selection())
        });
        match outcome {
            Some(OpenOutcome::Cd(dir)) => {
                self.files_surface(|b| b.cd(&dir));
                true
            }
            Some(OpenOutcome::Open(path)) => {
                self.files_open_file(&path);
                true
            }
            _ => false,
        }
    }

    /// Finder's keys for a file list (Go menu, Get Info, undo / redo, trash,
    /// tabs, sort, rename, clipboard, type-ahead). `true` = handled. Window
    /// extras (pane moves, vim keys, the sidebar, …) run before this.
    fn files_finder_key(&self, key: &crate::ui::popup::KeyInput) -> bool {
        use crate::engines::file_ops::FileOps;
        let (in_list, text) = self.files_focus();
        let (cmd, shift, opt, ctrl) = (key.cmd, key.shift, key.opt, key.ctrl);
        let plain = !cmd && !ctrl && !opt && !shift;
        // Exact chords, so a Finder key never fires for a neighbouring one.
        let cmd_only = cmd && !shift && !opt && !ctrl;
        let cmd_shift = cmd && shift && !opt && !ctrl;
        let cmd_opt = cmd && opt && !shift && !ctrl;
        let cmd_ctrl = cmd && ctrl && !shift && !opt;
        let home = std::env::var("HOME").unwrap_or_default();
        // Finder's Go menu and the other Finder keys first: several share a
        // key code with a looser arm below (Cmd+Opt+C vs Cmd+C).
        match key.key_code {
            // Cmd+Shift+H / D / O, Cmd+Opt+L: Home, Desktop, Documents, Downloads.
            4 if cmd_shift => return self.files_go(&home),
            2 if cmd_shift => return self.files_go(&format!("{home}/Desktop")),
            31 if cmd_shift => return self.files_go(&format!("{home}/Documents")),
            37 if cmd_opt => return self.files_go(&format!("{home}/Downloads")),
            // Cmd+Shift+G: Go to Folder — the address field, its text selected.
            5 if cmd_shift => {
                self.files_focus_filter(true);
                return true;
            }
            // Cmd+F: search — the same filter box as Cmd+L.
            3 if cmd_only => {
                self.files_focus_filter(false);
                return true;
            }
            // Cmd+O: open the selection (Cmd+Down's twin).
            31 if cmd_only => {
                self.files_open_selected();
                return true;
            }
            // Cmd+Y: Quick Look (Space's twin); Cmd+Opt+Y: full screen.
            16 if cmd_only || cmd_opt => {
                if let Some(mtm) = MainThreadMarker::new() {
                    self.files_quick_look(mtm);
                    if cmd_opt {
                        crate::views::files::quick_look_full_screen(mtm);
                    }
                }
                return true;
            }
            34 if cmd_only => {
                self.files_get_info();
                return true;
            }
            // Cmd+Opt+C: Copy as Pathname.
            8 if cmd_opt => {
                if let Some(p) = self.files_selected().into_iter().next() {
                    let _ = crate::platform::set_text(&p);
                }
                return true;
            }
            // Cmd+Opt+A: Deselect All.
            0 if cmd_opt => {
                self.files_surface(|b| b.list.deselect_all());
                return true;
            }
            // Cmd+Shift+Z: redo (a text field keeps it for its own typing).
            6 if cmd_shift && !text => {
                match FileOps::redo(FileOps::shared()) {
                    Some((what, o)) => self.files_op_done(format!("Redid {what}"), o.failed),
                    None => crate::ui::toast::show("Nothing to redo", "arrow.uturn.forward"),
                }
                return true;
            }
            45 if cmd_ctrl => {
                self.files_new_folder_with_selection();
                return true;
            }
            // Cmd+Opt+Delete: delete immediately (asks first).
            51 if cmd_opt && !text => {
                self.files_delete_now();
                return true;
            }
            // Cmd+Shift+Delete: Empty Trash (asks first).
            51 if cmd_shift && !text => {
                self.files_empty_trash();
                return true;
            }
            // Cmd+Ctrl+T: Add to Sidebar (pin the selected / current folder).
            17 if cmd_ctrl => {
                self.files_surface(|b| b.pin_selection_or_cwd());
                return true;
            }
            35 if cmd_shift => {
                self.files_toggle_preview();
                return true;
            }
            // Tabs: Cmd+T new, Cmd+W closes one while there are several,
            // Cmd+Shift+[ / ] cycle.
            17 if cmd_only => {
                self.files_surface(|b| b.new_tab());
                self.files_relayout();
                return true;
            }
            13 if cmd_only && self.files_surface(|b| b.tab_count()).unwrap_or(1) > 1 => {
                self.files_surface(|b| b.close_tab());
                self.files_relayout();
                return true;
            }
            30 | 33 if cmd_shift => {
                let d = if key.key_code == 30 { 1 } else { -1 };
                return self.files_surface(|b| b.cycle_tab(d)).unwrap_or(false);
            }
            // Opt+→ / Opt+←: expand / collapse the selected folder in place.
            124 if opt && !cmd && !ctrl && !shift && in_list => {
                self.files_surface(|b| b.expand_selected());
                return true;
            }
            123 if opt && !cmd && !ctrl && !shift && in_list => {
                self.files_surface(|b| b.collapse_selected());
                return true;
            }
            14 if cmd_only => {
                self.files_eject();
                return true;
            }
            // Cmd+Ctrl+Opt+1/2/5/6/7: sort by Name / Kind / Modified / Created / Size.
            18 | 19 | 23 | 22 | 26 if cmd && ctrl && opt && !shift => {
                use crate::views::files::SortKey;
                let k = match key.key_code {
                    18 => SortKey::Name,
                    19 => SortKey::Kind,
                    23 => SortKey::Modified,
                    22 => SortKey::Created,
                    _ => SortKey::Size,
                };
                self.files_surface(|b| b.set_sort(k));
                return true;
            }
            _ => {}
        }
        match key.key_code {
            33 if cmd => {
                self.files_surface(|b| b.go_back());
            }
            30 if cmd => {
                self.files_surface(|b| b.go_forward());
            }
            47 if cmd && shift => {
                self.files_surface(|b| b.toggle_hidden());
            }
            37 if cmd && !shift => self.files_focus_filter(false),
            45 if cmd && shift => self.files_new_item(true),
            126 if cmd && !shift => {
                self.files_surface(|b| b.cd_parent());
            }
            125 if cmd && !shift && in_list => {
                self.files_open_selected();
            }
            123 if plain && in_list => {
                self.files_surface(|b| b.cd_parent());
            }
            124 if plain && in_list => {
                let dir = self.files_surface(|b| b.visible_rows().get(b.selection()).filter(|e| e.is_dir).map(|e| e.path.clone()));
                if let Some(Some(d)) = dir {
                    self.files_surface(|b| b.cd(&d));
                }
            }
            49 if plain && in_list => {
                if let Some(mtm) = MainThreadMarker::new() {
                    self.files_quick_look(mtm);
                }
            }
            // Rename: Cmd+R / F2.
            15 if cmd && !shift => self.files_begin_rename(None),
            120 if !cmd && !ctrl => self.files_begin_rename(None),
            51 if cmd_only && in_list => self.files_trash(),
            2 if cmd_only && in_list => self.files_duplicate(),
            8 if cmd_only && in_list => self.files_copy(),
            7 if cmd_only && in_list => self.files_cut(),
            9 if cmd && !shift && !ctrl && in_list => self.files_paste(opt),
            6 if cmd && !shift && in_list => match FileOps::undo(FileOps::shared()) {
                Some((what, o)) => self.files_op_done(format!("Undid {what}"), o.failed),
                None => crate::ui::toast::show("Nothing to undo", "arrow.uturn.backward"),
            },
            // Type-ahead: a printable key with the list focused starts the filter.
            _ if in_list
                && !cmd
                && !ctrl
                && !opt
                && key.chars.is_some_and(|c| !c.is_control() && !c.is_whitespace() && (c.is_alphanumeric() || "._-~/*+".contains(c))) =>
            {
                return self.files_type_ahead(key.chars.unwrap_or_default());
            }
            _ => return false,
        }
        true
    }

    /// A click / keystroke from the file browser view. Pure model events go
    /// to `FileBrowser::handle`; the rest (menus, drops, opening, renaming)
    /// is here, once, for every window.
    fn files_event(&self, ev: crate::views::files_view::FilesEvent) {
        use crate::ui::sidebar::SidebarEvent;
        use crate::views::files_view::FilesEvent as E;
        match ev {
            E::Click { row, cmd, shift, double } => {
                // Only a row the user already clicked counts as "selected"
                // (the default selection of row 0 must not arm a rename).
                let click_key = self.files_surface(|b| (b.cwd.clone(), row));
                let was_selected = self.files_surface(|b| b.list.selection == row && b.list.marked.is_empty()) == Some(true)
                    && LAST_CLICK.with(|c| *c.borrow() == click_key);
                LAST_CLICK.with(|c| *c.borrow_mut() = click_key);
                self.files_surface(|b| b.handle(E::Click { row, cmd, shift, double: false }));
                let target = (!cmd && !shift)
                    .then(|| {
                        self.files_surface(|b| {
                            let in_recent = b.in_recent();
                            b.visible_rows().get(row).filter(|e| !e.is_parent()).map(|e| (e.path.clone(), in_recent && !e.is_dir))
                        })
                        .flatten()
                    })
                    .flatten();
                self.focus_files_list();
                match target {
                    // In Recent a double-click previews instead of descending.
                    Some((path, true)) if double => {
                        cancel_click_rename();
                        self.files_open_file(&path);
                    }
                    Some(_) if double => {
                        cancel_click_rename();
                        self.files_open_selected();
                    }
                    Some(_) if was_selected => schedule_click_rename(|| Self::with_host(|ui| ui.files_begin_rename(None))),
                    _ => {
                        cancel_click_rename();
                        if double && !cmd && !shift {
                            self.files_open_selected();
                        }
                    }
                }
            }
            E::RowMenu { row, x, y } => {
                self.files_surface(|b| b.handle(E::RowMenu { row, x, y }));
                self.files_row_menu(x, y);
            }
            E::BlankMenu { x, y } => self.files_blank_menu(x, y),
            E::Drop { row, paths, copy, force_move } => {
                use std::os::unix::fs::MetadataExt;
                let dest = self
                    .files_surface(|b| match row {
                        Some(i) => b.visible_rows().get(i).map(|e| e.path.clone()),
                        None => b.ops_directory(),
                    })
                    .flatten();
                let Some(dest) = dest else {
                    crate::ui::toast::show("Can't drop here \u{2014} open a folder first", "exclamationmark.triangle.fill");
                    return;
                };
                // Dropping a file back into its own folder (or a folder onto
                // itself) is a no-op.
                let src: Vec<String> = paths
                    .into_iter()
                    .filter(|p| {
                        p != &dest && std::path::Path::new(p).parent().map(|d| d.to_string_lossy() != dest.trim_end_matches('/')) != Some(false)
                    })
                    .collect();
                if src.is_empty() {
                    return;
                }
                // Finder rule: same volume = move, other volume = copy;
                // Option forces a copy, Command forces a move.
                let dev = |p: &str| std::fs::metadata(p).map(|m| m.dev()).ok();
                let same_volume = src.iter().all(|p| dev(p).is_some() && dev(p) == dev(&dest));
                let mv = force_move || (!copy && same_volume);
                let name = std::path::Path::new(&dest).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(dest.clone());
                self.files_transfer(src, dest, mv, format!("into {name}"));
            }
            E::PreviewOpen(i) => {
                let target = self.files_surface(|b| match b.preview().1 {
                    crate::views::files_view::Preview::Folder(rows) => rows.get(i).cloned(),
                    _ => None,
                });
                if let Some(Some(e)) = target {
                    if e.is_dir {
                        self.files_surface(|b| b.cd(&e.path));
                    } else {
                        self.files_open_file(&e.path);
                    }
                }
            }
            E::SortMenu { x, y } => {
                use crate::views::files::SortKey;
                let cur = self.files_surface(|b| b.sort_key).unwrap_or(SortKey::Name);
                let view = self.files_surface(|b| b.view.clone()).flatten();
                let entries = SortKey::ALL
                    .iter()
                    .map(|k| {
                        let k = *k;
                        MenuEntry::checked(k.label(), k == cur, move || {
                            Self::with_host(|ui| {
                                ui.files_surface(|b| b.set_sort(k));
                            })
                        })
                    })
                    .collect();
                if let Some(v) = view {
                    crate::ui::menu_kit::popup(entries, Some(&v), x, y);
                }
            }
            E::Enter => {
                // A typed path: jump to the folder, or open the file.
                let jumped = self.files_surface(|b| {
                    let q = b.query.trim().to_string();
                    if (q.contains('/') || q.starts_with('~')) && !crate::views::files::has_glob(&q) {
                        let p = crate::views::files::resolve_path(&q, &b.cwd);
                        if std::path::Path::new(&p).is_dir() {
                            b.cd(&p);
                            return Some(None);
                        }
                        if std::path::Path::new(&p).exists() {
                            return Some(Some(p));
                        }
                    }
                    None
                });
                match jumped {
                    Some(Some(Some(p))) => self.files_open_file(&p),
                    Some(Some(None)) => self.focus_files_list(),
                    _ => {
                        self.files_open_selected();
                    }
                }
            }
            E::Escape => self.files_escape(),
            E::Tab(i) => {
                self.files_surface(|b| b.handle(E::Tab(i)));
                self.files_relayout();
            }
            E::Sidebar(SidebarEvent::MenuPinned(path, x, y)) => {
                let pinned = self.files_surface(|b| b.pinned_favorites.contains(&path)).unwrap_or(false);
                let (p1, p2, p3, p4) = (path.clone(), path.clone(), path.clone(), path.clone());
                let mut entries = vec![
                    MenuEntry::item("Open", move || {
                        Self::with_host(|ui| {
                            ui.files_surface(|b| b.cd(&p1));
                        })
                    }),
                    MenuEntry::item("Reveal in Files", move || crate::app::hooks::reveal_in_files(&[p2.clone()])),
                    MenuEntry::item("Copy Absolute Path", move || {
                        let _ = crate::platform::set_text(&p3);
                    }),
                ];
                entries.extend(crate::views::files::copy_contents_entry(&path));
                if pinned {
                    entries.push(MenuEntry::Separator);
                    entries.push(MenuEntry::item("Unpin", move || {
                        Self::with_host(|ui| {
                            ui.files_surface(|b| {
                                b.pinned_favorites.retain(|x| x != &p4);
                                b.toggle_pin_save();
                            });
                        })
                    }));
                }
                crate::ui::menu_kit::popup(entries, None, x, y);
            }
            E::Sidebar(SidebarEvent::Rail(_)) => self.files_relayout(),
            // Parent, pin, order, query, move, drag end, sidebar navigation.
            other => {
                self.files_surface(|b| b.handle(other));
            }
        }
    }

    fn files_op_done(&self, what: String, failed: Option<String>) {
        self.files_surface(|b| b.reload());
        match failed {
            Some(f) => crate::ui::toast::show(&f, "exclamationmark.triangle.fill"),
            None if !what.is_empty() => crate::ui::toast::show(&what, "checkmark.circle.fill"),
            None => {}
        }
    }

    fn files_new_item(&self, folder: bool) {
        use crate::engines::file_ops::FileOps;
        let Some(Some(dir)) = self.files_surface(|b| b.ops_directory()) else {
            crate::ui::toast::show("Can't create here \u{2014} open a folder first", "exclamationmark.triangle.fill");
            return;
        };
        let o = FileOps::create(if folder { "untitled folder" } else { "untitled.txt" }, &dir, folder, FileOps::shared());
        let Some(path) = o.paths().into_iter().next() else {
            self.files_op_done(String::new(), o.failed.or(Some("Couldn't create".into())));
            return;
        };
        self.files_surface(|b| {
            if !b.query.is_empty() {
                b.set_query("");
            }
            b.reload();
            b.select_path(&path);
        });
        self.focus_files_list();
        self.files_begin_rename(None);
    }

    /// Finder's New Folder with Selection: move the selection into a new
    /// folder and start renaming it (no selection: a plain new folder).
    fn files_new_folder_with_selection(&self) {
        use crate::engines::file_ops::FileOps;
        let paths = self.files_selected();
        if paths.is_empty() {
            self.files_new_item(true);
            return;
        }
        let Some(Some(dir)) = self.files_surface(|b| b.ops_directory()) else {
            crate::ui::toast::show("Can't create here \u{2014} open a folder first", "exclamationmark.triangle.fill");
            return;
        };
        let stack = FileOps::shared();
        let since = stack.count();
        let made = FileOps::create("New Folder With Items", &dir, true, stack);
        let Some(folder) = made.paths().into_iter().next() else {
            self.files_op_done(String::new(), made.failed.or(Some("Couldn't create the folder".into())));
            return;
        };
        let moved = FileOps::transfer(&paths, &folder, true, stack);
        stack.collapse(since, "New Folder with Items");
        if let Some(f) = moved.failed {
            crate::ui::toast::show(&f, "exclamationmark.triangle.fill");
        }
        self.files_surface(|b| {
            if !b.query.is_empty() {
                b.set_query("");
            }
            b.reload();
            b.select_path(&folder);
        });
        self.focus_files_list();
        self.files_begin_rename(None);
    }

    /// Cmd+R / F2 / click-to-rename: an inline field over the row (`row`, or
    /// the selection). Several marked: the "Rename N Items" sheet instead.
    fn files_begin_rename(&self, row: Option<usize>) {
        if row.is_none() && self.files_surface(|b| b.list.marked.len()).unwrap_or(0) > 1 {
            self.files_batch_rename();
            return;
        }
        let Some(Some((i, path, is_dir, list))) = self.files_surface(|b| {
            b.begin_rename(row);
            let (path, _) = b.rename.clone()?;
            let i = b.list.selection;
            let is_dir = b.list.rows.get(i).is_some_and(|e| e.is_dir);
            Some((i, path, is_dir, b.view.as_ref()?.list_view()))
        }) else {
            return;
        };
        let name = file_name(&path);
        // A 22pt field centred on the row (rows grow with the list zoom).
        let r = list.row_rect(i);
        let x = r.x + 30.0 + list.row_indent(i);
        let rect = objc2_foundation::NSRect::new(
            objc2_foundation::NSPoint::new(x, (r.y + (r.height - 22.0) / 2.0).round()),
            objc2_foundation::NSSize::new((r.width * 0.55 - x).max(r.width - x - 280.0).max(160.0), 22.0),
        );
        let wl = objc2::rc::Weak::from_retained(&list);
        crate::ui::inline_rename::begin(
            &list,
            rect,
            &name,
            is_dir,
            crate::ui::theme::section_colors("files"),
            12.0,
            Box::new(move |text| {
                let old_path = path.clone();
                Self::with_host(|ui| {
                    let msg = ui.files_surface(|b| match &text {
                        Some(t) => {
                            if let Some((p, _)) = b.rename.clone() {
                                b.rename = Some((p, t.clone()));
                            }
                            let m = b.commit_rename();
                            if m.as_deref().is_some_and(|m| m.starts_with("renamed")) {
                                // Keep the Recent engine's entry on the new name.
                                let dir = std::path::Path::new(&old_path).parent().map(|d| d.to_path_buf()).unwrap_or_default();
                                let dst = dir.join(t.trim()).to_string_lossy().into_owned();
                                crate::views::files::with_recent(|r| r.own_change(Some(&old_path), &dst));
                                b.reload();
                                b.select_path(&dst);
                            }
                            m
                        }
                        None => {
                            b.cancel_rename();
                            None
                        }
                    });
                    if let Some(Some(m)) = msg {
                        let icon = if m.starts_with("renamed") { "pencil" } else { "exclamationmark.triangle.fill" };
                        crate::ui::toast::show(&m, icon);
                    }
                });
                // Back to the list, unless the rename ended by clicking into
                // another text field, which keeps its focus.
                if let Some(l) = wl.load() {
                    if let Some(w) = l.window() {
                        let typing = w.firstResponder().is_some_and(|r| {
                            objc2_foundation::NSObjectProtocol::isKindOfClass(&*r, <objc2_app_kit::NSTextView as objc2::ClassType>::class())
                        });
                        if !typing {
                            w.makeFirstResponder(Some(&l));
                        }
                    }
                }
            }),
        );
    }

    fn files_quick_look(&self, mtm: MainThreadMarker) {
        let Some((mut ql, paths)) = self.files_surface(|b| (b.quick_look.clone(), b.selected_paths())) else { return };
        ql.toggle(mtm, &paths);
        self.files_surface(|b| b.quick_look = ql);
    }

    fn files_go(&self, dir: &str) -> bool {
        if !std::path::Path::new(dir).is_dir() {
            crate::ui::toast::show(&format!("No folder at {}", crate::views::files::display_path(dir)), "exclamationmark.triangle.fill");
            return true;
        }
        self.files_surface(|b| {
            if !b.query.is_empty() {
                b.set_query("");
            }
            b.cd(dir);
        });
        true
    }

    /// Cmd+I: Finder's Get Info — kind, size (folders summed off the main
    /// thread), where, dates, permissions — as a sheet.
    fn files_get_info(&self) {
        let paths = match self.files_surface(|b| (b.selected_paths(), b.cwd.clone())) {
            Some((p, _)) if !p.is_empty() => p,
            Some((_, cwd)) if !cwd.is_empty() => vec![cwd],
            _ => return,
        };
        std::thread::spawn(move || {
            let (title, rows) = crate::views::files::file_info_rows(&paths, Duration::from_secs(3));
            let text = rows.iter().map(|(k, v)| format!("{k}:  {v}")).collect::<Vec<_>>().join("\n");
            Self::later(move |ui| {
                let Some(window) = ui.files_ns_window() else { return };
                let first = paths[0].clone();
                let all = paths.join("\n");
                crate::ui::sheet::alert_sheet(
                    &window,
                    &format!("{title} Info"),
                    &text,
                    &["OK", "Copy Path", "Show in Finder"],
                    move |b| match b {
                        1 => {
                            let _ = crate::platform::set_text(&all);
                        }
                        2 => {
                            let _ = std::process::Command::new("open").args(["-R", &first]).spawn();
                        }
                        _ => {}
                    },
                );
            });
        });
    }

    fn files_batch_rename(&self) {
        use crate::ui::sheet::{form_sheet, FormRow};
        let paths = self.files_selected();
        if paths.len() < 2 {
            return;
        }
        let Some(window) = self.files_ns_window() else { return };
        form_sheet(
            &window,
            &format!("Rename {} Items", paths.len()),
            "Replace text, add text before or after the name, or give a name to number them (\u{201c}Name 1\u{201d}, \u{201c}Name 2\u{201d}, \u{2026}). Extensions are kept.",
            vec![
                FormRow::new("Find", "", "text to replace"),
                FormRow::new("Replace", "", "replace it with"),
                FormRow::new("Before", "", "add before the name"),
                FormRow::new("After", "", "add after the name"),
                FormRow::new("Number", "", "name to number them, e.g. Photo"),
            ],
            "Rename",
            move |vals| {
                let Some(v) = vals else { return };
                use crate::engines::file_ops::FileOps;
                use crate::views::files::{batch_rename_plan, BatchRename};
                let o = BatchRename {
                    find: v[0].clone(),
                    replace: v[1].clone(),
                    prefix: v[2].clone(),
                    suffix: v[3].clone(),
                    base: v[4].clone(),
                };
                let is_dir = |p: &str| std::path::Path::new(p).is_dir();
                let exists = |p: &str| std::path::Path::new(p).symlink_metadata().is_ok();
                let plan = match batch_rename_plan(&paths, &o, &is_dir, &exists) {
                    Ok(p) => p,
                    Err(e) => {
                        crate::ui::toast::show(&format!("Not renamed: {e}"), "exclamationmark.triangle.fill");
                        return;
                    }
                };
                if plan.is_empty() {
                    crate::ui::toast::show("Nothing to rename", "pencil");
                    return;
                }
                let stack = FileOps::shared();
                let since = stack.count();
                let mut failed = None;
                let mut done = Vec::new();
                for (from, to) in &plan {
                    match std::fs::rename(from, to) {
                        Ok(()) => {
                            FileOps::record_rename(from, to, stack);
                            crate::views::files::with_recent(|r| r.own_change(Some(from), to));
                            done.push(to.clone());
                        }
                        Err(e) => {
                            failed.get_or_insert(format!("{}: {e}", file_name(from)));
                        }
                    }
                }
                stack.collapse(since, &format!("rename of {} items", done.len()));
                Self::with_host(|ui| {
                    ui.files_op_done(format!("Renamed {} item(s) \u{b7} \u{2318}Z undoes", done.len()), failed);
                    ui.files_surface(|b| {
                        let idx: Vec<usize> = done.iter().filter_map(|p| b.list.rows.iter().position(|e| &e.path == p)).collect();
                        if let Some(&first) = idx.first() {
                            b.list.selection = first;
                        }
                        b.list.marked = idx.into_iter().collect();
                    });
                });
            },
        );
    }

    fn files_delete_now(&self) {
        let paths = self.files_selected();
        if paths.is_empty() {
            return;
        }
        let Some(window) = self.files_ns_window() else { return };
        let what = if paths.len() == 1 {
            format!("\u{201c}{}\u{201d}", file_name(&paths[0]))
        } else {
            format!("these {} items", paths.len())
        };
        crate::ui::sheet::alert_sheet(
            &window,
            &format!("Are you sure you want to delete {what} immediately?"),
            "You can't undo this action.",
            &["Delete", "Cancel"],
            move |b| {
                if b != 0 {
                    return;
                }
                let mut failed = None;
                for p in &paths {
                    let path = std::path::Path::new(p);
                    let r = match path.symlink_metadata() {
                        Ok(m) if m.is_dir() => std::fs::remove_dir_all(path),
                        Ok(_) => std::fs::remove_file(path),
                        Err(e) => Err(e),
                    };
                    if let Err(e) = r {
                        failed.get_or_insert(format!("{}: {e}", file_name(p)));
                    }
                }
                Self::with_host(|ui| ui.files_op_done(format!("Deleted {} item(s)", paths.len()), failed));
            },
        );
    }

    fn files_empty_trash(&self) {
        let Some(window) = self.files_ns_window() else { return };
        crate::ui::sheet::alert_sheet(
            &window,
            "Are you sure you want to permanently erase the items in the Trash?",
            "You can't undo this action.",
            &["Empty Trash", "Cancel"],
            move |b| {
                if b != 0 {
                    return;
                }
                std::thread::spawn(|| {
                    let ok = std::process::Command::new("/usr/bin/osascript")
                        .args(["-e", "tell application \"Finder\" to empty trash"])
                        .status()
                        .map(|s| s.success())
                        .unwrap_or(false);
                    Self::later(move |ui| {
                        if ok {
                            ui.files_op_done(String::new(), None);
                            crate::ui::toast::show("Emptied the Trash", "trash");
                        } else {
                            ui.files_op_done(
                                String::new(),
                                Some("Couldn't empty the Trash (allow Bansho to control Finder in System Settings \u{203a} Privacy)".into()),
                            );
                        }
                    });
                });
            },
        );
    }

    fn files_eject(&self) {
        let Some((sel, cwd)) = self.files_surface(|b| (b.selected_paths(), b.cwd.clone())) else { return };
        let p = sel.into_iter().next().unwrap_or(cwd);
        let parts: Vec<&str> = p.trim_start_matches('/').split('/').collect();
        if parts.first() != Some(&"Volumes") || parts.len() < 2 || parts[1].is_empty() {
            crate::ui::toast::show("Not an ejectable volume (select something under /Volumes)", "eject");
            return;
        }
        let vol = format!("/Volumes/{}", parts[1]);
        let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
        // Step out of the volume first so the listing doesn't hold it busy.
        self.files_go(&home);
        std::thread::spawn(move || {
            let ok = std::process::Command::new("/usr/sbin/diskutil")
                .args(["eject", &vol])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            Self::later(move |_| {
                if ok {
                    crate::ui::toast::show(&format!("Ejected {vol}"), "eject");
                } else {
                    crate::ui::toast::show(&format!("Couldn't eject {vol} (in use?)"), "exclamationmark.triangle.fill");
                }
            });
        });
    }

    fn files_relayout(&self) {
        if let Some(Some(v)) = self.files_surface(|b| b.view.clone()) {
            v.layout_panes();
            v.setNeedsDisplay(true);
        }
    }

    fn files_toggle_preview(&self) {
        let Some(Some(view)) = self.files_surface(|b| b.view.clone()) else { return };
        let on = !view.preview_shown();
        view.set_preview_shown(on);
        self.files_remember("filesPreview", if on { "1" } else { "0" });
        self.files_surface(|_| ());
    }

    fn files_text_zoom(&self, sign: i32) {
        let Some(Some(view)) = self.files_surface(|b| b.view.clone()) else { return };
        let list = view.list_view();
        let z = (((list.zoom() + 0.1 * sign as f64) * 100.0).round() / 100.0).clamp(0.6, 3.0);
        list.set_zoom(z);
        self.files_remember("textZoom.files", &z.to_string());
        self.files_surface(|_| ());
    }

    fn files_trash(&self) {
        use crate::engines::file_ops::FileOps;
        let paths = self.files_selected();
        if paths.is_empty() {
            return;
        }
        let o = FileOps::trash(&paths, FileOps::shared());
        self.files_op_done(format!("Moved {} item(s) to the Trash \u{b7} \u{2318}Z undoes", paths.len()), o.failed);
    }

    fn files_duplicate(&self) {
        use crate::engines::file_ops::FileOps;
        let paths = self.files_selected();
        if paths.is_empty() {
            return;
        }
        let o = FileOps::duplicate(&paths, FileOps::shared());
        let copies = o.paths();
        self.files_op_done(crate::views::files::duplicate_toast(&copies), o.failed);
        // Select the (first) new copy, as Finder does.
        if let Some(first) = copies.first() {
            self.files_surface(|b| b.select_path(first));
        }
    }

    fn files_compress(&self) {
        let paths = self.files_selected();
        if paths.is_empty() {
            return;
        }
        crate::ui::toast::show(&format!("Compressing {}\u{2026}", crate::app::hooks::copied_what(&paths)), "doc.zipper");
        std::thread::spawn(move || {
            let res = crate::views::files::compress_paths(&paths);
            Self::later(move |ui| match res {
                Ok(zip) => {
                    ui.files_surface(|b| {
                        b.reload();
                        b.select_path(&zip);
                    });
                    crate::ui::toast::show(&format!("Created {}", file_name(&zip)), "checkmark.circle.fill");
                }
                Err(msg) => crate::ui::toast::show(&msg, "exclamationmark.triangle.fill"),
            });
        });
    }

    fn files_copy(&self) {
        if crate::app::hooks::copy_files_toast(&self.files_selected()) {
            FILES_CUT.with(|c| c.set(false));
        }
    }

    fn files_cut(&self) {
        let paths = self.files_selected();
        if crate::app::hooks::copy_files_to_pasteboard(&paths) {
            FILES_CUT.with(|c| c.set(true));
            crate::ui::toast::show(&format!("Cut {}", crate::app::hooks::copied_what(&paths)), "scissors");
        }
    }

    fn files_paste(&self, force_move: bool) {
        let src = pasteboard_file_paths();
        let Some(Some(dir)) = self.files_surface(|b| b.ops_directory()) else { return };
        if src.is_empty() {
            return;
        }
        let mv = force_move || FILES_CUT.with(|c| c.replace(false));
        self.files_transfer(src, dir, mv, "here".into());
    }

    fn files_transfer(&self, src: Vec<String>, dest: String, mv: bool, where_: String) {
        use crate::engines::file_ops::FileOps;
        let bytes: u64 = src
            .iter()
            .filter_map(|p| std::fs::symlink_metadata(p).ok())
            .map(|m| if m.is_dir() { 1 << 30 } else { m.len() })
            .sum();
        // A same-volume move is a rename (instant); anything else that is big
        // gets a "working" toast so the wait is not a mystery.
        if !mv && bytes >= 64 << 20 {
            crate::ui::toast::show(&format!("Copying {} item(s)\u{2026}", src.len()), "doc.on.doc");
        }
        std::thread::spawn(move || {
            let o = FileOps::transfer(&src, &dest, mv, FileOps::shared());
            let verb = if mv { "Moved" } else { "Copied" };
            let what = format!("{verb} {} item(s) {where_}", src.len());
            Self::later(move |ui| ui.files_op_done(what, o.failed));
        });
    }

    /// Right-click on selected rows.
    fn files_row_menu(&self, x: f64, y: f64) {
        let Some((paths, ctx)) = self.files_surface(|b| {
            let ctx = b.perm_context(pasteboard_file_paths().len());
            (b.selected_paths(), ctx)
        }) else {
            return;
        };
        if paths.is_empty() {
            return;
        }
        let many = paths.len() > 1;
        let first = paths[0].clone();
        let call = |f: fn(&Self)| move || Self::with_host(|ui| f(ui));
        let quick_look = can_perform(Action::QuickLook, &ctx).then(|| {
            MenuEntry::item("Quick Look", || {
                Self::with_host(|ui| {
                    if let Some(mtm) = MainThreadMarker::new() {
                        ui.files_quick_look(mtm);
                    }
                })
            })
        });
        let mut clipboard: Vec<MenuEntry> = Vec::new();
        if can_perform(Action::Copy, &ctx) {
            clipboard.push(MenuEntry::item(
                &if many { format!("Copy {} Items", paths.len()) } else { "Copy".to_string() },
                call(Self::files_copy),
            ));
        }
        if can_perform(Action::Cut, &ctx) {
            clipboard.push(MenuEntry::item(
                &if many { format!("Cut {} Items", paths.len()) } else { "Cut".to_string() },
                call(Self::files_cut),
            ));
        }
        if can_perform(Action::Duplicate, &ctx) {
            clipboard.push(MenuEntry::item("Duplicate", call(Self::files_duplicate)));
            let title = if many {
                format!("Compress {} Items", paths.len())
            } else {
                format!("Compress \u{201c}{}\u{201d}", file_name(&first))
            };
            clipboard.push(MenuEntry::item(&title, call(Self::files_compress)));
        }
        // Pictures only (checked by their bytes): open / merge them in the
        // screenshot editor.
        let editor = self
            .files_screenshot()
            .and_then(|shot| crate::views::screenshot::editor_menu_entry(&shot, &paths));
        // …and read their text (OCR / QR) onto the clipboard, with the card.
        let copy_text = self
            .files_screenshot()
            .and_then(|shot| crate::views::screenshot::copy_text_menu_entry(&shot, &paths));
        let mut entries: Vec<MenuEntry> = Vec::new();
        if many {
            entries.extend(quick_look);
            entries.extend(editor);
            entries.extend(copy_text);
            entries.push(MenuEntry::Separator);
            {
                let joined = paths.join("\n");
                entries.push(MenuEntry::item("Copy Absolute Paths", move || {
                    let _ = crate::platform::set_text(&joined);
                }));
            }
            entries.extend(clipboard);
            entries.push(MenuEntry::item(&format!("Rename {} Items\u{2026}", paths.len()), call(Self::files_batch_rename)));
            entries.push(MenuEntry::Separator);
            {
                let ps = paths.clone();
                entries.push(MenuEntry::item("Reveal in Files", move || crate::app::hooks::reveal_in_files(&ps)));
            }
            entries.push(MenuEntry::Separator);
            entries.push(MenuEntry::item(&format!("Move {} Items to Trash", paths.len()), call(Self::files_trash)));
        } else {
            // One shared core for every file-menu surface; rows this menu
            // builds richer pass `None` and join in below.
            let actions = crate::views::files::FileMenuActions {
                open: Some(Box::new({
                    let p = first.clone();
                    move || {
                        Self::with_host(|ui| {
                            if std::path::Path::new(&p).is_dir() {
                                ui.files_surface(|b| b.cd(&p));
                            } else {
                                ui.files_open_file(&p);
                            }
                        })
                    }
                })),
                reveal: Some(Box::new({
                    let ps = vec![first.clone()];
                    move || crate::app::hooks::reveal_in_files(&ps)
                })),
                rename: Some(Box::new(|| Self::with_host(|ui| ui.files_begin_rename(None)))),
                compress: None, // the `Compress “{name}”` row below is the richer twin
                trash: Some(Box::new(call(Self::files_trash))),
                copy_file: None, // `Copy` below is the richer twin
                copy_path: Some(Box::new({
                    let p = first.clone();
                    move || {
                        let _ = crate::platform::set_text(&p);
                    }
                })),
            };
            entries = crate::views::files::file_context_menu(&first, actions);
            // Quick Look and the editor join the open block, before Reveal.
            for e in [quick_look, editor, copy_text].into_iter().flatten() {
                let at = entries
                    .iter()
                    .position(|e| matches!(e, MenuEntry::Item { title, .. } if title == "Reveal in Files"))
                    .unwrap_or(entries.len());
                entries.insert(at, e);
            }
            if !clipboard.is_empty() {
                entries.push(MenuEntry::Separator);
                entries.extend(clipboard);
            }
        }
        self.files_append_folder_items(&mut entries, &ctx, true);
        crate::ui::menu_kit::popup(entries, None, x, y);
    }

    /// New Folder / New File / Paste / Show Hidden Files, when allowed here.
    fn files_append_folder_items(&self, entries: &mut Vec<MenuEntry>, ctx: &PermContext, sep: bool) {
        let mut extra: Vec<MenuEntry> = Vec::new();
        if can_perform(Action::NewFolder, ctx) {
            extra.push(MenuEntry::item("New Folder", || Self::with_host(|ui| ui.files_new_item(true))));
            extra.push(MenuEntry::item("New File", || Self::with_host(|ui| ui.files_new_item(false))));
        }
        if can_perform(Action::Paste, ctx) {
            extra.push(MenuEntry::item("Paste", || Self::with_host(|ui| ui.files_paste(false))));
        }
        if can_perform(Action::ToggleHidden, ctx) {
            let shown = self.files_surface(|b| b.show_hidden).unwrap_or(false);
            extra.push(MenuEntry::item(if shown { "Hide Hidden Files" } else { "Show Hidden Files" }, || {
                Self::with_host(|ui| {
                    ui.files_surface(|b| b.toggle_hidden());
                })
            }));
        }
        if extra.is_empty() {
            return;
        }
        if sep {
            entries.push(MenuEntry::Separator);
        }
        entries.extend(extra);
    }

    /// Right-click on empty space in the list.
    fn files_blank_menu(&self, x: f64, y: f64) {
        let Some(ctx) = self.files_surface(|b| {
            b.list.marked.clear();
            b.perm_context(pasteboard_file_paths().len())
        }) else {
            return;
        };
        let mut entries = Vec::new();
        self.files_append_folder_items(&mut entries, &ctx, false);
        if !entries.is_empty() {
            crate::ui::menu_kit::popup(entries, None, x, y);
        }
    }
}
