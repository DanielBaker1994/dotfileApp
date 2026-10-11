//! Port of `PathsWindow.swift` — the `/paths` recent-file shelf window.
//!
//! The shelf model — rows, filtering, the trailing `folder · age · why` text,
//! the Return action (`file | path | open`), rename and the `do:paths:*` hooks —
//! is complete and tested. The AppKit surface is real too:
//! [`PathsWindow::build`] builds the query field, the scrollable row list and
//! the empty-state label, [`PathsWindow::show_window`] places the standalone
//! tool panel, and the Quick Look panel is ported via
//! [`crate::views::files::QuickLook`] ([`PathsWindow::toggle_quick_look`],
//! [`PathsWindow::key`]). Not yet drawn: the per-row `NSWorkspace` file icons.

use serde_json::{json, Value};

use crate::engines::file_ops::FileOps;
use crate::engines::path_shelf::{Item, Why};
use crate::ui::chrome::Rect;
use crate::ui::popup::KeyInput;
use crate::views::files::QuickLook;

#[cfg(target_os = "macos")]
use objc2::rc::Retained;
#[cfg(target_os = "macos")]
use objc2_app_kit::{NSTextField, NSView};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReturnAction {
    File,
    Path,
    Open,
}

impl ReturnAction {
    pub fn from_str(s: &str) -> ReturnAction {
        match s {
            "path" => ReturnAction::Path,
            "open" => ReturnAction::Open,
            _ => ReturnAction::File,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ReturnAction::File => "file",
            ReturnAction::Path => "path",
            ReturnAction::Open => "open",
        }
    }
}

/// A decoded `PathsWindow.key(_:_:)` intent. The pure [`key_action`] maps a
/// key event to one of these; [`PathsWindow::key`] applies it (the Quick Look
/// arm needs the main thread).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathsKey {
    /// Esc / Cmd+W.
    Hide,
    /// Arrow / Ctrl+N / Ctrl+P: `(delta, extend)`.
    Move(i64, bool),
    /// Return / keypad Enter.
    RunDefault,
    /// Cmd+Shift+C.
    CopyFiles,
    /// Cmd+C (with no text selected).
    CopyPaths,
    /// Cmd+Y / Space.
    QuickLook,
    /// Cmd+O.
    Open,
    /// Cmd+R.
    BeginRename,
    /// Cmd+Shift+R.
    Reveal,
    /// Cmd+Delete.
    Forget,
}

/// `PathsWindow.key(_:_:)`'s switch, pure and testable. `query_empty` and
/// `list_focused` stand in for Swift's `query.isEmpty || firstResponder === list`
/// Space guard.
pub fn key_action(
    key: KeyInput,
    text_selected: bool,
    query_empty: bool,
    list_focused: bool,
) -> Option<PathsKey> {
    let (cmd, ctrl, shift) = (key.cmd, key.ctrl, key.shift);
    match key.key_code {
        53 => Some(PathsKey::Hide),
        13 if cmd => Some(PathsKey::Hide),
        125 => Some(PathsKey::Move(1, shift)),
        45 if ctrl => Some(PathsKey::Move(1, shift)),
        126 => Some(PathsKey::Move(-1, shift)),
        35 if ctrl => Some(PathsKey::Move(-1, shift)),
        36 | 76 => Some(PathsKey::RunDefault),
        8 if cmd && shift => Some(PathsKey::CopyFiles),
        8 if cmd && !text_selected => Some(PathsKey::CopyPaths),
        16 if cmd => Some(PathsKey::QuickLook),
        49 if !cmd && !ctrl && (query_empty || list_focused) => Some(PathsKey::QuickLook),
        31 if cmd => Some(PathsKey::Open),
        15 if cmd && !shift => Some(PathsKey::BeginRename),
        15 if cmd && shift => Some(PathsKey::Reveal),
        51 if cmd => Some(PathsKey::Forget),
        _ => None,
    }
}

/// `PathsWindow.folder(_:)`.
pub fn folder(path: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut d = parent_of(path);
    if !home.is_empty() && d == home {
        return "~".to_string();
    }
    if !home.is_empty() {
        if let Some(rest) = d.strip_prefix(&format!("{home}/")) {
            d = format!("~/{rest}");
        }
    }
    if let Some(rest) = d.strip_prefix("/private/tmp") {
        d = format!("/tmp{rest}");
    }
    if d.chars().count() <= 34 {
        return d;
    }
    let comps: Vec<&str> = d.split('/').filter(|s| !s.is_empty()).collect();
    let tail = &comps[comps.len().saturating_sub(2)..];
    format!("…/{}", tail.join("/"))
}

/// `PathsWindow.age(_:)`.
pub fn age(secs: f64) -> String {
    let s = secs.max(0.0);
    if s < 60.0 {
        "now".to_string()
    } else if s < 3600.0 {
        format!("{}m", (s / 60.0) as i64)
    } else if s < 86400.0 {
        format!("{}h", (s / 3600.0) as i64)
    } else {
        format!("{}d", (s / 86400.0) as i64)
    }
}

fn parent_of(path: &str) -> String {
    std::path::Path::new(path)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn basename(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The trailing text `reload()` builds: `folder · age · why.label`.
pub fn trailing(item: &Item, now: f64) -> String {
    format!(
        "{} · {} · {}",
        folder(&item.path),
        age(now - item.at),
        item.why.label()
    )
}

/// `reload()`'s filter: every lowercased query word must be a substring.
pub fn filter(items: &[Item], query: &str) -> Vec<Item> {
    let words: Vec<String> = query.to_lowercase().split(' ').map(str::to_string).collect();
    items
        .iter()
        .filter(|i| {
            let p = i.path.to_lowercase();
            words.iter().all(|w| p.contains(w))
        })
        .cloned()
        .collect()
}

/// One displayed row.
#[derive(Clone, Debug, PartialEq)]
pub struct PathRow {
    pub path: String,
    pub why: Why,
    pub name: String,
    pub trailing: String,
}

/// `shelf.rekey` in-process: a path inside a renamed file/folder is re-keyed.
pub fn rekey(p: &str, old: &str, new: &str) -> Option<String> {
    if p == old {
        return Some(new.to_string());
    }
    if let Some(rest) = p.strip_prefix(&format!("{old}/")) {
        return Some(format!("{new}/{rest}"));
    }
    None
}

/// `commitRename`'s validation + `FileManager.moveItem`.
pub fn rename_path(old_path: &str, new_name: &str) -> Result<Option<String>, String> {
    let old = basename(old_path);
    let new = new_name.trim();
    if new.is_empty() || new == old {
        return Ok(None);
    }
    if new.contains('/') || new == "." || new == ".." {
        return Err(format!("can't rename: “{new}” is not a valid name"));
    }
    let dst = format!("{}/{}", parent_of(old_path), new);
    if new.to_lowercase() != old.to_lowercase() && std::path::Path::new(&dst).exists() {
        return Err(format!("can't rename: “{new}” already exists"));
    }
    std::fs::rename(old_path, &dst).map_err(|e| format!("rename failed: {e}"))?;
    FileOps::record_rename(old_path, &dst, FileOps::shared());
    Ok(Some(dst))
}

// ------------------------------------------------------------------ appkit layout

// `PathsWindow` surface geometry (flipped, top-left origin).
pub const PATHS_DEFAULT_WIDTH: f64 = 680.0;
pub const PATHS_DEFAULT_HEIGHT: f64 = 300.0;
pub const PATH_ROW_HEIGHT: f64 = 22.0;
pub const PATH_SEARCH_HEIGHT: f64 = 30.0;
pub const PATH_MARGIN: f64 = 6.0;

/// The search field, the list region and the empty-state label of the shelf.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PathsLayout {
    pub search: Rect,
    pub list: Rect,
    pub empty: Rect,
    /// The rows view height (at least the visible list height).
    pub rows_height: f64,
}

pub fn paths_layout(width: f64, height: f64, row_count: usize) -> PathsLayout {
    let inner = (width - PATH_MARGIN * 2.0).max(0.0);
    let search = Rect::new(PATH_MARGIN, PATH_MARGIN, inner, PATH_SEARCH_HEIGHT);
    let top = search.max_y() + 8.0;
    let list = Rect::new(
        PATH_MARGIN,
        top,
        inner,
        (height - top - 10.0).max(0.0),
    );
    let empty = Rect::new(16.0, top + 3.0, (width - 32.0).max(0.0), 16.0);
    let rows_height = (row_count as f64 * PATH_ROW_HEIGHT).max(list.height);
    PathsLayout { search, list, empty, rows_height }
}

/// Where [`PathsWindow::show_window`] drops the tool panel: horizontally
/// center-right of the screen's visible frame, vertically centered, clamped so
/// the panel always stays fully on-screen. Pure so it is testable off AppKit.
pub fn place_origin(
    visible_x: f64,
    visible_y: f64,
    visible_w: f64,
    visible_h: f64,
    width: f64,
    height: f64,
) -> (f64, f64) {
    let x = visible_x + (visible_w - width) * 0.62;
    let y = visible_y + (visible_h - height) * 0.5;
    let max_x = (visible_x + visible_w - width).max(visible_x);
    let max_y = (visible_y + visible_h - height).max(visible_y);
    (x.clamp(visible_x, max_x), y.clamp(visible_y, max_y))
}

/// `PathsWindow` — the shelf window model.
pub struct PathsWindow {
    pub return_action: ReturnAction,
    pub items: Vec<Item>,
    pub rows: Vec<PathRow>,
    pub selection: usize,
    pub query: String,
    pub shown: bool,
    pub key: bool,
    pub level: i64,
    pub frame: [i64; 4],
    /// `InlineRename` target: (old path, in-progress text).
    pub rename: Option<(String, String)>,
    pub rename_editor: Option<String>,
    /// The Quick Look preview-panel bridge (Swift `QLPreviewPanelDataSource`).
    pub quick_look: QuickLook,
    /// The built AppKit view tree (`build_macos`).
    #[cfg(target_os = "macos")]
    pub root: Option<Retained<NSView>>,
    /// The standalone tool panel once [`Self::show_window`] has run.
    #[cfg(target_os = "macos")]
    pub window: Option<Retained<crate::ui::popup::PopupPanel>>,
    /// The query field, kept so `show_window` can make it first responder.
    #[cfg(target_os = "macos")]
    pub search: Option<Retained<NSTextField>>,
}

impl PathsWindow {
    pub fn new(return_action: ReturnAction) -> Self {
        PathsWindow {
            return_action,
            items: Vec::new(),
            rows: Vec::new(),
            selection: 0,
            query: String::new(),
            shown: false,
            key: false,
            level: 0,
            frame: [0, 0, 680, 0],
            rename: None,
            rename_editor: None,
            quick_look: QuickLook::new(),
            #[cfg(target_os = "macos")]
            root: None,
            #[cfg(target_os = "macos")]
            window: None,
            #[cfg(target_os = "macos")]
            search: None,
        }
    }

    pub fn from_action(return_action: &str) -> Self {
        PathsWindow::new(ReturnAction::from_str(return_action))
    }

    /// Feed the current shelf snapshot (the app calls this from the shelf's
    /// `onChanged` / NotificationCenter observer).
    pub fn set_items(&mut self, items: Vec<Item>) {
        self.items = items;
        self.reload(false);
    }

    pub fn reload(&mut self, keep_selection: bool) {
        let selected = self.rows.get(self.selection).map(|r| r.path.clone());
        let now = now_secs();
        let filtered = filter(&self.items, &self.query);
        self.rows = filtered
            .iter()
            .map(|i| PathRow {
                path: i.path.clone(),
                why: i.why,
                name: basename(&i.path),
                trailing: trailing(i, now),
            })
            .collect();
        if keep_selection {
            if let Some(s) = selected {
                self.selection = self.rows.iter().position(|r| r.path == s).unwrap_or(0);
            }
        } else {
            self.selection = 0;
        }
    }

    pub fn set_query(&mut self, q: &str) {
        self.query = q.trim().to_string();
        if self.shown {
            self.reload(false);
        }
    }

    pub fn set_key(&mut self, key: bool) {
        self.key = key;
    }

    pub fn show(&mut self) {
        self.shown = true;
        self.query = self.query.trim().to_string();
        self.reload(false);
    }

    pub fn hide(&mut self) {
        self.shown = false;
        self.key = false;
        #[cfg(target_os = "macos")]
        if let Some(mtm) = objc2::MainThreadMarker::new() {
            self.quick_look.dismiss(mtm);
        }
    }

    pub fn is_empty_text(&self) -> String {
        if self.query.is_empty() {
            "Nothing yet — files you save, download or copy show up here".to_string()
        } else {
            format!("No recent file matches “{}”", self.query)
        }
    }

    pub fn selected_rows(&self) -> Vec<usize> {
        if self.rows.is_empty() {
            Vec::new()
        } else {
            vec![self.selection.min(self.rows.len() - 1)]
        }
    }

    pub fn selected_paths(&self) -> Vec<String> {
        self.selected_rows().into_iter().map(|i| self.rows[i].path.clone()).collect()
    }

    pub fn move_by(&mut self, d: i64) {
        if self.rows.is_empty() {
            return;
        }
        let last = self.rows.len() as i64 - 1;
        self.selection = (self.selection as i64 + d).clamp(0, last) as usize;
    }

    /// Swift `PathsWindow.toggleQuickLook()`.
    pub fn toggle_quick_look(&mut self, mtm: objc2::MainThreadMarker) {
        let paths = self.selected_paths();
        self.quick_look.toggle(mtm, &paths);
    }

    /// Swift `PathsWindow.refreshQuickLook()` (called when the selection moves).
    pub fn refresh_quick_look(&mut self, mtm: objc2::MainThreadMarker) {
        let paths = self.selected_paths();
        self.quick_look.refresh(mtm, &paths);
    }

    /// Swift `PathsWindow.key(_:_:)`: decode the event with [`key_action`] and
    /// apply it. Returns whether the key was consumed. Shift-arrow multi-select
    /// is not modelled (the shelf is single-selection here), so an extending
    /// move behaves like a plain move.
    pub fn key(
        &mut self,
        mtm: objc2::MainThreadMarker,
        key: KeyInput,
        text_selected: bool,
        query_empty: bool,
        list_focused: bool,
    ) -> bool {
        let Some(action) = key_action(key, text_selected, query_empty, list_focused) else {
            return false;
        };
        match action {
            PathsKey::Hide => self.hide(),
            PathsKey::Move(d, _extend) => {
                self.move_by(d);
                self.refresh_quick_look(mtm);
            }
            PathsKey::RunDefault => {
                self.run_default();
            }
            PathsKey::CopyFiles => {
                self.copy_files();
            }
            PathsKey::CopyPaths => {
                self.copy_paths();
            }
            PathsKey::QuickLook => self.toggle_quick_look(mtm),
            PathsKey::Open => {
                self.open_paths();
            }
            PathsKey::BeginRename => {
                let text = self
                    .rows
                    .get(self.selection)
                    .map(|r| basename(&r.path))
                    .unwrap_or_default();
                self.begin_rename(&text);
            }
            PathsKey::Reveal => {
                self.reveal_paths();
            }
            PathsKey::Forget => self.forget(),
        }
        true
    }

    /// `runDefault()`.
    pub fn run_default(&mut self) -> Vec<String> {
        match self.return_action {
            ReturnAction::Path => {
                self.copy_paths();
                Vec::new()
            }
            ReturnAction::Open => {
                let paths = self.selected_paths();
                paths.clone()
            }
            ReturnAction::File => {
                self.copy_files();
                Vec::new()
            }
        }
    }

    /// `copyFiles()` — the paths handed to the pasteboard (`writeFiles`).
    pub fn copy_files(&self) -> Vec<String> {
        self.selected_paths()
    }

    /// `copyPaths()` — the newline-joined path text the pasteboard receives.
    pub fn copy_paths_text(&self) -> String {
        self.selected_paths().join("\n")
    }

    pub fn copy_paths(&self) -> String {
        self.copy_paths_text()
    }

    /// `open(_:)` — the paths to open through `FilePopup`.
    pub fn open_paths(&mut self) -> Vec<String> {
        self.selected_paths()
    }

    pub fn reveal_paths(&self) -> Vec<String> {
        self.selected_paths()
    }

    /// `forget()` — drop the selected rows from the shelf.
    pub fn forget(&mut self) {
        let paths = self.selected_paths();
        if paths.is_empty() {
            return;
        }
        self.items.retain(|i| !paths.contains(&i.path));
        self.reload(true);
    }

    pub fn begin_rename(&mut self, text: &str) {
        if let Some(r) = self.rows.get(self.selection) {
            self.rename = Some((r.path.clone(), text.to_string()));
            self.rename_editor = Some(text.to_string());
        }
    }

    pub fn cancel_rename(&mut self) {
        self.rename = None;
        self.rename_editor = None;
    }

    /// `commitRename()` — returns the status message.
    pub fn commit_rename(&mut self) -> Option<String> {
        let (path, text) = self.rename.clone()?;
        self.rename = None;
        self.rename_editor = None;
        let old = basename(&path);
        let new = text.trim().to_string();
        match rename_path(&path, &new) {
            Ok(None) => None,
            Ok(Some(dst)) => {
                for i in self.items.iter_mut() {
                    if let Some(p) = rekey(&i.path, &path, &dst) {
                        i.path = p;
                    }
                }
                self.reload(true);
                if let Some(i) = self.rows.iter().position(|r| r.path == dst) {
                    self.selection = i;
                }
                Some(format!("renamed “{old}” to “{new}”"))
            }
            Err(msg) => Some(msg),
        }
    }

    #[allow(non_snake_case)]
    pub fn test_select(&mut self, i: usize) {
        if i < self.rows.len() {
            self.selection = i;
        }
    }

    #[allow(non_snake_case)]
    pub fn test_return(&mut self) -> Vec<String> {
        self.run_default()
    }

    /// The window fields reported to the socket. Once [`Self::show_window`] has
    /// built the panel we read them straight from it — Swift's `testState`
    /// reads `window.isShown` / `isKeyWindow` / `level` / `frame` — so
    /// `tools.paths.key`/`level`/`frame` track the live panel. Before the panel
    /// exists we fall back to the model's placeholder values.
    fn window_state(&self) -> (bool, bool, i64, [i64; 4]) {
        #[cfg(target_os = "macos")]
        if let Some(panel) = &self.window {
            let f = panel.frame();
            return (
                panel.isVisible(),
                panel.isKeyWindow(),
                panel.level() as i64,
                [
                    f.origin.x.round() as i64,
                    f.origin.y.round() as i64,
                    f.size.width.round() as i64,
                    f.size.height.round() as i64,
                ],
            );
        }
        (self.shown, self.key, self.level, self.frame)
    }

    pub fn test_state(&self) -> Value {
        let (shown, key, level, frame) = self.window_state();
        json!({
            "shown": shown,
            "key": key,
            "level": level,
            "selection": self.selection,
            "query": self.query,
            "quickLook": self.quick_look.state(),
            "rows": self.rows.iter().map(|r| json!({
                "path": r.path, "why": r.why.raw(), "name": r.name, "trailing": r.trailing
            })).collect::<Vec<_>>(),
            "frame": frame,
        })
    }

    /// `do:paths:show|hide|return|select:N`.
    pub fn handle_do(&mut self, action: &str) -> Option<Value> {
        let rest = action.strip_prefix("paths:")?;
        match rest {
            "show" => {
                self.show();
                Some(self.test_state())
            }
            "hide" => {
                self.hide();
                Some(self.test_state())
            }
            "return" => {
                self.run_default();
                Some(self.test_state())
            }
            sel => {
                if let Some(n) = sel.strip_prefix("select:") {
                    if let Ok(i) = n.parse::<usize>() {
                        self.test_select(i);
                        return Some(self.test_state());
                    }
                }
                None
            }
        }
    }

    /// Build the window + list. No-op off macOS.
    pub fn build(&mut self, mtm: objc2::MainThreadMarker) {
        #[cfg(target_os = "macos")]
        self.build_macos(mtm);
        #[cfg(not(target_os = "macos"))]
        let _ = mtm;
    }

    #[cfg(target_os = "macos")]
    fn build_macos(&mut self, mtm: objc2::MainThreadMarker) {
        if self.root.is_some() {
            return;
        }
        let colors = crate::ui::theme::PopupThemeDefaults::colors();
        let rows = self.rows.clone();
        let query = self.query.clone();
        let empty_text = self.is_empty_text();
        let (view, search) = macos::build_view(mtm, &query, &rows, &empty_text, &colors);
        self.root = Some(view);
        self.search = Some(search);
    }

    /// Build (once) the standalone tool panel and order it front, focusing the
    /// query field. Mirrors `PathsWindow.show()` (`showPersistent` + `place`).
    /// No-op off macOS.
    pub fn show_window(&mut self, mtm: objc2::MainThreadMarker) {
        #[cfg(target_os = "macos")]
        self.show_window_macos(mtm);
        #[cfg(not(target_os = "macos"))]
        let _ = mtm;
    }

    #[cfg(target_os = "macos")]
    fn show_window_macos(&mut self, mtm: objc2::MainThreadMarker) {
        use crate::ui::popup::{CardFrame, PopupConfig, PopupPanel};
        use objc2_app_kit::{NSResponder, NSScreen};
        use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

        if self.root.is_none() {
            self.build_macos(mtm);
        }

        if self.window.is_none() {
            let cfg = PopupConfig {
                name: "paths".to_string(),
                frame: CardFrame {
                    x: 0.0,
                    y: 0.0,
                    width: PATHS_DEFAULT_WIDTH,
                    height: PATHS_DEFAULT_HEIGHT,
                },
                tool_panel: true,
                floating: true,
                ..PopupConfig::default()
            };
            let panel = PopupPanel::create(mtm, &cfg);
            if let Some(root) = &self.root {
                panel.setContentView(Some(root));
            }
            panel.setTitle(&NSString::from_str("paths"));
            self.window = Some(panel);
        }

        if let Some(panel) = &self.window {
            let vf = NSScreen::mainScreen(mtm)
                .map(|s| s.visibleFrame())
                .unwrap_or(NSRect::new(
                    NSPoint::new(0.0, 0.0),
                    NSSize::new(1440.0, 900.0),
                ));
            let (x, y) = place_origin(
                vf.origin.x,
                vf.origin.y,
                vf.size.width,
                vf.size.height,
                PATHS_DEFAULT_WIDTH,
                PATHS_DEFAULT_HEIGHT,
            );
            panel.setFrameOrigin(NSPoint::new(x, y));
            panel.makeKeyAndOrderFront(None);
            if let Some(search) = &self.search {
                let responder: &NSResponder = &**search;
                panel.makeFirstResponder(Some(responder));
            }
        }

        self.show();
    }

    /// Order the panel out (if built) and clear the model's shown flag.
    pub fn hide_window(&mut self) {
        #[cfg(target_os = "macos")]
        if let Some(panel) = &self.window {
            panel.orderOut(None);
        }
        self.hide();
    }

    /// Whether the built panel is currently visible. `false` off macOS / unbuilt.
    pub fn is_window_visible(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.window
                .as_ref()
                .map(|w| w.isVisible())
                .unwrap_or(false)
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }

    /// The panel's `windowNumber()` once built, else `0`.
    pub fn window_number(&self) -> i64 {
        #[cfg(target_os = "macos")]
        {
            self.window
                .as_ref()
                .map(|w| w.windowNumber() as i64)
                .unwrap_or(0)
        }
        #[cfg(not(target_os = "macos"))]
        {
            0
        }
    }

    /// The built AppKit root view, once [`Self::build`] has run.
    #[cfg(target_os = "macos")]
    pub fn content_view(&self) -> Option<Retained<NSView>> {
        self.root.clone()
    }
}

impl std::fmt::Debug for PathsWindow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PathsWindow")
            .field("return_action", &self.return_action)
            .field("items", &self.items.len())
            .field("rows", &self.rows)
            .field("selection", &self.selection)
            .field("query", &self.query)
            .field("shown", &self.shown)
            .field("key", &self.key)
            .field("level", &self.level)
            .field("frame", &self.frame)
            .finish()
    }
}

/// Build a default shelf view (empty state until the host feeds the shelf).
#[cfg(target_os = "macos")]
pub fn build_content(mtm: objc2::MainThreadMarker) -> Option<Retained<NSView>> {
    let mut window = PathsWindow::new(ReturnAction::File);
    window.build(mtm);
    window.content_view()
}

fn now_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use crate::ui::theme::{PopupColors, Rgba};
    use objc2::rc::Retained;
    use objc2::{define_class, msg_send, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSAutoresizingMaskOptions, NSFont, NSLineBreakMode, NSScrollView, NSTextAlignment,
        NSTextField, NSView,
    };
    use objc2_foundation::{NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};

    fn nsrect(r: Rect) -> NSRect {
        NSRect::new(NSPoint::new(r.x, r.y), NSSize::new(r.width, r.height))
    }

    fn label(mtm: MainThreadMarker, s: &str, size: f64, color: Rgba) -> Retained<NSTextField> {
        let l = NSTextField::labelWithString(&NSString::from_str(s), mtm);
        l.setFont(Some(&NSFont::systemFontOfSize(size)));
        l.setTextColor(Some(&color.to_nscolor()));
        l
    }

    pub struct FlippedViewIvars;

    define_class!(
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSPathsFlippedView"]
        #[ivars = FlippedViewIvars]
        pub struct FlippedView;

        impl FlippedView {
            #[unsafe(method(isFlipped))]
            fn is_flipped(&self) -> bool {
                true
            }
        }

        unsafe impl NSObjectProtocol for FlippedView {}
    );

    impl FlippedView {
        pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(FlippedViewIvars);
            unsafe {
                msg_send![
                    super(this),
                    initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0))
                ]
            }
        }
    }

    /// The shelf view tree: query field, scrollable row list, empty-state label.
    /// Returns the root view and the query field (so the caller can focus it).
    pub fn build_view(
        mtm: MainThreadMarker,
        query: &str,
        rows: &[PathRow],
        empty_text: &str,
        colors: &PopupColors,
    ) -> (Retained<NSView>, Retained<NSTextField>) {
        let w = PATHS_DEFAULT_WIDTH;
        let h = PATHS_DEFAULT_HEIGHT;
        let layout = paths_layout(w, h, rows.len());

        let root = FlippedView::new(mtm);
        root.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(w, h)));
        root.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        root.setWantsLayer(true);
        if let Some(layer) = root.layer() {
            layer.setBackgroundColor(Some(&colors.mantle().to_nscolor().CGColor()));
        }

        // Search / query field.
        let search = NSTextField::textFieldWithString(&NSString::from_str(query), mtm);
        search.setPlaceholderString(Some(&NSString::from_str(
            "recent files — type to filter · ↩ copy file",
        )));
        search.setFont(Some(&NSFont::systemFontOfSize(13.0)));
        search.setBezeled(true);
        search.setEditable(true);
        search.setSelectable(true);
        search.setFrame(nsrect(layout.search));
        search.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        root.addSubview(&search);

        // Row list.
        let scroll = NSScrollView::new(mtm);
        scroll.setFrame(nsrect(layout.list));
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setDrawsBackground(false);
        scroll.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        let rows_view = FlippedView::new(mtm);
        rows_view.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        let row_w = layout.list.width.max(1.0);
        for (i, r) in rows.iter().enumerate() {
            let row = FlippedView::new(mtm);
            row.setFrame(NSRect::new(
                NSPoint::new(0.0, i as f64 * PATH_ROW_HEIGHT),
                NSSize::new(row_w, PATH_ROW_HEIGHT),
            ));
            row.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);

            let name = label(mtm, &r.name, 12.0, colors.text);
            name.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
            name.setFrame(NSRect::new(
                NSPoint::new(6.0, 2.0),
                NSSize::new((row_w * 0.5).max(1.0), 18.0),
            ));
            row.addSubview(&name);

            let trailing = label(mtm, &r.trailing, 10.0, colors.dim);
            trailing.setAlignment(NSTextAlignment::Right);
            trailing.setLineBreakMode(NSLineBreakMode::ByTruncatingHead);
            trailing.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
            trailing.setFrame(NSRect::new(
                NSPoint::new((row_w * 0.5).max(1.0), 2.0),
                NSSize::new((row_w * 0.5 - 6.0).max(1.0), 18.0),
            ));
            row.addSubview(&trailing);

            rows_view.addSubview(&row);
        }
        rows_view.setFrame(NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(row_w, layout.rows_height),
        ));
        scroll.setDocumentView(Some(&rows_view));
        root.addSubview(&scroll);

        // Empty-state label.
        let empty = label(mtm, empty_text, 12.0, colors.dim);
        empty.setFrame(nsrect(layout.empty));
        empty.setHidden(!rows.is_empty());
        root.addSubview(&empty);

        (root.into_super(), search)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(path: &str, at: f64, why: Why) -> Item {
        Item {
            path: path.to_string(),
            at,
            why,
        }
    }

    #[test]
    fn age_buckets() {
        assert_eq!(age(0.0), "now");
        assert_eq!(age(59.0), "now");
        assert_eq!(age(60.0), "1m");
        assert_eq!(age(3599.0), "59m");
        assert_eq!(age(3600.0), "1h");
        assert_eq!(age(86399.0), "23h");
        assert_eq!(age(86400.0), "1d");
        assert_eq!(age(-5.0), "now");
    }

    #[test]
    fn folder_abbreviates_and_shortens() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(folder(&format!("{home}/x.txt")), "~");
        assert_eq!(folder(&format!("{home}/a/b.txt")), "~/a");
        assert_eq!(folder("/private/tmp/foo.txt"), "/tmp");
        let deep = format!("{home}/a/very/long/path/segment/here/and/there/f.txt");
        assert!(folder(&deep).starts_with("…/"));
        assert!(!folder(&deep).contains("/very/"));
    }

    #[test]
    fn filter_all_words() {
        let items = vec![
            item("/tmp/report.pdf", 1.0, Why::Downloaded),
            item("/tmp/notes.txt", 2.0, Why::Modified),
        ];
        assert_eq!(filter(&items, "").len(), 2);
        assert_eq!(filter(&items, "report").len(), 1);
        assert_eq!(filter(&items, "REPORT").len(), 1, "case-insensitive");
        assert_eq!(filter(&items, "tmp notes").len(), 1);
        assert_eq!(filter(&items, "tmp missing").len(), 0);
    }

    #[test]
    fn window_rows_and_state() {
        let mut w = PathsWindow::new(ReturnAction::File);
        w.set_items(vec![
            item("/tmp/a.txt", 100.0, Why::Clipboard),
            item("/tmp/b.txt", 200.0, Why::Filefast),
        ]);
        assert_eq!(w.rows.len(), 2);
        assert_eq!(w.selection, 0);
        assert_eq!(w.rows[0].name, "a.txt");
        assert!(w.rows[0].trailing.contains("copied"));
        w.show();
        w.set_query("b");
        assert_eq!(w.rows.len(), 1);
        assert_eq!(w.selection, 0);
        w.hide();
        let st = w.test_state();
        assert_eq!(st["shown"], false);
        assert_eq!(st["query"], "b");
        assert_eq!(st["rows"][0]["path"], "/tmp/b.txt");
        assert_eq!(st["rows"][0]["why"], "filefast");

        w.show();

        w.set_query("");
        w.move_by(1);
        assert_eq!(w.selected_paths(), vec!["/tmp/b.txt".to_string()]);
        w.move_by(5);
        assert_eq!(w.selection, 1, "clamped");
        w.move_by(-5);
        assert_eq!(w.selection, 0);
    }

    #[test]
    fn return_action_mapping() {
        let mut w = PathsWindow::new(ReturnAction::Path);
        w.set_items(vec![
            item("/tmp/a.txt", 1.0, Why::Clipboard),
            item("/tmp/b.txt", 2.0, Why::Clipboard),
        ]);
        assert_eq!(w.copy_paths(), "/tmp/a.txt");

        let opened = w.open_paths();
        assert_eq!(opened, vec!["/tmp/a.txt".to_string()], "open returns the path");
        assert_eq!(ReturnAction::from_str("open").as_str(), "open");
        assert_eq!(ReturnAction::from_str("weird").as_str(), "file");
    }

    #[test]
    fn do_hooks() {
        let mut w = PathsWindow::from_action("file");
        w.set_items(vec![item("/tmp/a.txt", 1.0, Why::Clipboard)]);
        let st = w.handle_do("paths:show").unwrap();
        assert_eq!(st["shown"], true);
        w.handle_do("paths:select:0");
        assert_eq!(w.selection, 0);
        let st = w.handle_do("paths:hide").unwrap();
        assert_eq!(st["shown"], false);
        assert!(w.handle_do("nope").is_none());
        assert!(w.handle_do("paths:select:x").is_none());
    }

    #[test]
    fn rename_validation_and_move() {
        let root = std::env::temp_dir().join(format!("ws-rs-paths-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let old = root.join("a.txt");
        std::fs::write(&old, "x").unwrap();
        let old = old.to_string_lossy().into_owned();

        assert_eq!(rename_path(&old, "a.txt").unwrap(), None, "unchanged is a no-op");
        assert_eq!(rename_path(&old, "").unwrap(), None, "empty is a no-op");
        assert!(rename_path(&old, "bad/name").is_err());
        assert!(rename_path(&old, "..").is_err());
        assert!(rename_path(&old, ".").is_err());

        std::fs::write(root.join("b.txt"), "x").unwrap();
        assert!(rename_path(&old, "b.txt").is_err(), "already exists");

        let dst = rename_path(&old, "renamed.txt").unwrap().unwrap();
        assert_eq!(dst, root.join("renamed.txt").to_string_lossy());
        assert!(std::path::Path::new(&dst).exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn paths_layout_regions() {
        let l = paths_layout(680.0, 300.0, 4);
        assert_eq!(l.search.y, PATH_MARGIN);
        assert_eq!(l.search.height, PATH_SEARCH_HEIGHT);
        assert!(l.list.y > l.search.y, "list sits below the search field");
        assert!(l.rows_height >= 4.0 * PATH_ROW_HEIGHT);
        assert_eq!(l.rows_height, l.list.height, "short listing fills the visible list");
        assert!(l.empty.x >= 0.0);

        let tall = paths_layout(680.0, 300.0, 100);
        assert!(tall.rows_height >= tall.list.height);
        let tiny = paths_layout(10.0, 10.0, 0);
        assert!(tiny.list.width >= 0.0 && tiny.list.height >= 0.0);
    }

    #[test]
    fn place_origin_centers_right_and_clamps() {
        // 1920x1080 visible frame: center-right of the screen.
        let (x, y) = place_origin(0.0, 0.0, 1920.0, 1080.0, 680.0, 300.0);
        assert!(x > (1920.0 - 680.0) * 0.5, "right of dead center");
        assert!(x + 680.0 <= 1920.0, "stays on-screen (x)");
        assert_eq!(y, (1080.0 - 300.0) * 0.5, "vertically centered");
        assert!(y + 300.0 <= 1080.0, "stays on-screen (y)");

        // A panel wider/taller than the visible frame clamps to its origin.
        let (x, y) = place_origin(10.0, 20.0, 400.0, 200.0, 680.0, 300.0);
        assert_eq!(x, 10.0);
        assert_eq!(y, 20.0);
    }

    #[test]
    fn unbuilt_window_reports_hidden() {
        // No AppKit object is constructed here: a fresh model has no panel.
        let w = PathsWindow::new(ReturnAction::File);
        assert!(!w.is_window_visible());
        assert_eq!(w.window_number(), 0);
    }

    #[test]
    fn hide_window_clears_shown() {
        let mut w = PathsWindow::new(ReturnAction::File);
        w.show();
        assert!(w.shown);
        w.hide_window();
        assert!(!w.shown);
        assert!(!w.is_window_visible());
    }

    #[test]
    fn rekey_rules() {
        assert_eq!(rekey("/a/b.txt", "/a/b.txt", "/a/c.txt").as_deref(), Some("/a/c.txt"));
        assert_eq!(rekey("/a/b/x.txt", "/a/b", "/a/z").as_deref(), Some("/a/z/x.txt"));
        assert!(rekey("/other/x.txt", "/a/b", "/a/z").is_none());
        assert!(rekey("/a/bsuffix", "/a/b", "/a/z").is_none());
    }

    #[test]
    fn rename_rekeys_items() {
        let root = std::env::temp_dir().join(format!("ws-rs-pathsw-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let old = root.join("a.txt");
        std::fs::write(&old, "x").unwrap();
        let old = old.to_string_lossy().into_owned();

        let mut w = PathsWindow::new(ReturnAction::File);
        w.set_items(vec![item(&old, 1.0, Why::Clipboard)]);
        w.begin_rename("b.txt");
        let msg = w.commit_rename().unwrap();
        assert_eq!(msg, "renamed “a.txt” to “b.txt”");
        assert_eq!(w.rows.len(), 1);
        assert!(w.rows[0].path.ends_with("/b.txt"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn quick_look_state_starts_empty() {
        let w = PathsWindow::new(ReturnAction::File);
        assert!(w.quick_look.state().is_empty());
        let st = w.test_state();
        assert_eq!(st["quickLook"], serde_json::json!([]));
    }

    #[test]
    fn key_action_maps_the_paths_switch() {
        let mut k = KeyInput::new(53);
        assert_eq!(key_action(k, false, true, false), Some(PathsKey::Hide));

        // Cmd+W hides; plain W does not.
        k = KeyInput::new(13);
        k.cmd = true;
        assert_eq!(key_action(k, false, true, false), Some(PathsKey::Hide));
        assert_eq!(key_action(KeyInput::new(13), false, true, false), None);

        // Arrows / emacs Ctrl+N,P.
        k = KeyInput::new(125);
        assert_eq!(key_action(k, false, true, false), Some(PathsKey::Move(1, false)));
        k.shift = true;
        assert_eq!(key_action(k, false, true, false), Some(PathsKey::Move(1, true)));
        k = KeyInput::new(45);
        k.ctrl = true;
        assert_eq!(key_action(k, false, true, false), Some(PathsKey::Move(1, false)));
        k = KeyInput::new(35);
        k.ctrl = true;
        assert_eq!(key_action(k, false, true, false), Some(PathsKey::Move(-1, false)));

        // Return and keypad Enter run the default action.
        assert_eq!(key_action(KeyInput::new(36), false, true, false), Some(PathsKey::RunDefault));
        assert_eq!(key_action(KeyInput::new(76), false, true, false), Some(PathsKey::RunDefault));

        // Copy: Cmd+Shift+C = files, Cmd+C = paths (unless text is selected).
        k = KeyInput::new(8);
        k.cmd = true;
        k.shift = true;
        assert_eq!(key_action(k, false, true, false), Some(PathsKey::CopyFiles));
        k = KeyInput::new(8);
        k.cmd = true;
        assert_eq!(key_action(k, false, true, false), Some(PathsKey::CopyPaths));
        assert_eq!(key_action(k, true, true, false), None, "text selection wins");

        // Quick Look: Cmd+Y always; Space only with an empty query or list focus.
        k = KeyInput::new(16);
        k.cmd = true;
        assert_eq!(key_action(k, false, true, false), Some(PathsKey::QuickLook));
        assert_eq!(key_action(KeyInput::new(49), false, true, false), Some(PathsKey::QuickLook));
        assert_eq!(key_action(KeyInput::new(49), false, false, true), Some(PathsKey::QuickLook));
        assert_eq!(key_action(KeyInput::new(49), false, false, false), None);
        k = KeyInput::new(49);
        k.cmd = true;
        assert_eq!(key_action(k, false, true, false), None, "Cmd+Space is Spotlight");

        // Cmd+O open, Cmd+R rename, Cmd+Shift+R reveal, Cmd+Delete forget.
        k = KeyInput::new(31);
        k.cmd = true;
        assert_eq!(key_action(k, false, true, false), Some(PathsKey::Open));
        k = KeyInput::new(15);
        k.cmd = true;
        assert_eq!(key_action(k, false, true, false), Some(PathsKey::BeginRename));
        k.shift = true;
        assert_eq!(key_action(k, false, true, false), Some(PathsKey::Reveal));
        k = KeyInput::new(51);
        k.cmd = true;
        assert_eq!(key_action(k, false, true, false), Some(PathsKey::Forget));
    }
}
