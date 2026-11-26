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
    /// Start an inline rename of the selected row.
    fn files_start_rename(&self);
    /// Open a file (not a folder) the way this window opens files.
    fn files_open_file(&self, path: &str);
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
        self.files_start_rename();
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
        let mut entries: Vec<MenuEntry> = Vec::new();
        if many {
            entries.extend(quick_look);
            entries.extend(editor);
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
                rename: Some(Box::new(call(Self::files_start_rename))),
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
            for e in [quick_look, editor].into_iter().flatten() {
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
