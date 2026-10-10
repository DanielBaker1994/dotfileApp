//! Port of `FilePopup.swift` + `PopupFileBrowser` (in `PopupWindow.swift`) —
//! the file browser model: rows/selection/marked set, the filter model,
//! Places/Recent/Arrived lists, rename/duplicate/trash and drop/open actions.
//!
//! `FileListPane` lives in `PopupWindow.swift`; it is modelled locally as
//! [`FileList`] (no AppKit). Deep drawing and the Quick Look panel are
//! `todo!()`; the model is complete and tested.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::engines::file_ops::{FileOps, Outcome, UndoStack};
use crate::engines::list_filter::localized_standard_compare;
use crate::ui::chrome::Rect;

#[cfg(target_os = "macos")]
use objc2::rc::Retained;
#[cfg(target_os = "macos")]
use objc2_app_kit::NSView;

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: i64,
    pub created: f64,
    pub modified: f64,
    pub trailing_text: String,
}

impl Entry {
    pub fn new(name: impl Into<String>, path: impl Into<String>, is_dir: bool, size: i64) -> Self {
        Entry {
            name: name.into(),
            path: path.into(),
            is_dir,
            size,
            created: 0.0,
            modified: 0.0,
            trailing_text: String::new(),
        }
    }

    pub fn is_parent(&self) -> bool {
        self.name == ".."
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Modified,
    Created,
    Size,
    Kind,
}

impl SortKey {
    pub const ALL: [SortKey; 5] = [
        SortKey::Name,
        SortKey::Modified,
        SortKey::Created,
        SortKey::Size,
        SortKey::Kind,
    ];

    pub fn raw(self) -> &'static str {
        match self {
            SortKey::Name => "name",
            SortKey::Modified => "modified",
            SortKey::Created => "created",
            SortKey::Size => "size",
            SortKey::Kind => "kind",
        }
    }

    pub fn from_raw(s: &str) -> SortKey {
        match s.to_lowercase().as_str() {
            "modified" => SortKey::Modified,
            "created" => SortKey::Created,
            "size" => SortKey::Size,
            "kind" => SortKey::Kind,
            _ => SortKey::Name,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SortKey::Name => "Name",
            SortKey::Modified => "Date Modified",
            SortKey::Created => "Date Created",
            SortKey::Size => "Size",
            SortKey::Kind => "Kind",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            SortKey::Name => "Name",
            SortKey::Modified => "Modified",
            SortKey::Created => "Created",
            SortKey::Size => "Size",
            SortKey::Kind => "Kind",
        }
    }

    pub fn natural_descending(self) -> bool {
        matches!(self, SortKey::Modified | SortKey::Created | SortKey::Size)
    }
}

/// `PopupFileBrowser.Entry` decoration: size / date trailing text.
pub fn decorate(e: &mut Entry, key: SortKey) {
    let size = if e.is_dir { String::new() } else { human_size(e.size) };
    let with_size = |date: String| {
        if size.is_empty() {
            date
        } else {
            format!("{date}  ·  {size}")
        }
    };
    e.trailing_text = match key {
        SortKey::Modified => {
            if e.is_parent() {
                String::new()
            } else {
                with_size(short_date(e.modified, 0.0))
            }
        }
        SortKey::Created => {
            if e.is_parent() {
                String::new()
            } else {
                with_size(short_date(e.created, 0.0))
            }
        }
        _ => size,
    };
}

pub fn sort_entries(list: &[Entry], key: SortKey, desc: bool) -> Vec<Entry> {
    let mut parent: Vec<Entry> = list.iter().filter(|e| e.is_parent()).cloned().collect();
    let mut rest: Vec<Entry> = list.iter().filter(|e| !e.is_parent()).cloned().collect();
    let ext = |e: &Entry| {
        Path::new(&e.name)
            .extension()
            .map(|x| x.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    };
    rest.sort_by(|a, b| {
        if a.is_dir != b.is_dir {
            return if a.is_dir {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            };
        }
        let ord = match key {
            SortKey::Name => localized_standard_compare(&a.name, &b.name),
            SortKey::Modified => cmp_f64(a.modified, b.modified),
            SortKey::Created => cmp_f64(a.created, b.created),
            SortKey::Size => a.size.cmp(&b.size),
            SortKey::Kind => ext(a).cmp(&ext(b)),
        };
        if ord == std::cmp::Ordering::Equal {
            return localized_standard_compare(&a.name, &b.name);
        }
        if desc {
            ord.reverse()
        } else {
            ord
        }
    });
    parent.append(&mut rest);
    parent
}

fn cmp_f64(a: f64, b: f64) -> std::cmp::Ordering {
    a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal)
}

pub fn human_size(bytes: i64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut i = 0usize;
    while v >= 1024.0 && i < units.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{bytes} B")
    } else {
        format!("{:.1} {}", v, units[i])
    }
}

/// `PopupFileBrowser.ago(_:)` for Recent rows.
pub fn ago(secs: f64) -> String {
    let s = secs.max(0.0) as i64;
    if s < 60 {
        "just now".to_string()
    } else if s < 3600 {
        format!("{}m ago", s / 60)
    } else if s < 86400 {
        format!("{}h ago", s / 3600)
    } else {
        format!("{}d ago", s / 86400)
    }
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// `shortDate(_:)`: `"MMM d HH:mm"` this year, else `"MMM d yyyy"`.
pub fn short_date(epoch: f64, now: f64) -> String {
    if epoch <= 0.0 {
        return String::new();
    }
    let (y, m, d, hh, mm) = civil_from_epoch(epoch);
    let now_year = if now > 0.0 { civil_from_epoch(now).0 } else { y };
    if y == now_year {
        format!("{} {} {:02}:{:02}", MONTHS[(m - 1) as usize], d, hh, mm)
    } else {
        format!("{} {} {}", MONTHS[(m - 1) as usize], d, y)
    }
}

fn civil_from_epoch(epoch: f64) -> (i64, i64, i64, i64, i64) {
    let secs = epoch.floor() as i64;
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (year, m, d, rem / 3600, (rem % 3600) / 60)
}

// ------------------------------------------------------------ query + filter

#[derive(Clone, Debug, PartialEq)]
pub enum QueryMode {
    All,
    Terminal(String),
    Local(String),
    Dir { dir: String, pat: String },
    Recursive { base: String, glob: String },
}

fn expand_tilde(s: &str) -> String {
    if s == "~" {
        return std::env::var("HOME").unwrap_or_default();
    }
    if let Some(rest) = s.strip_prefix("~/") {
        return format!("{}/{}", std::env::var("HOME").unwrap_or_default(), rest);
    }
    s.to_string()
}

/// Lexical path cleanup (Swift `NSString.standardizingPath` without symlinks).
pub fn standardize(p: &str) -> String {
    let abs = expand_tilde(p);
    let abs = if abs.starts_with('/') {
        abs
    } else {
        format!(
            "{}/{}",
            std::env::current_dir()
                .map(|c| c.to_string_lossy().into_owned())
                .unwrap_or_default(),
            abs
        )
    };
    let mut out: Vec<&str> = Vec::new();
    for comp in abs.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            c => out.push(c),
        }
    }
    format!("/{}", out.join("/"))
}

pub fn has_glob(s: &str) -> bool {
    s.contains('*') || s.contains('?') || s.contains('[')
}

/// A case-insensitive glob (`*`, `?`, `[…]`) full match.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    glob_here(&p, &t)
}

fn glob_here(p: &[char], t: &[char]) -> bool {
    if p.is_empty() {
        return t.is_empty();
    }
    match p[0] {
        '*' => {
            for i in 0..=t.len() {
                if glob_here(&p[1..], &t[i..]) {
                    return true;
                }
            }
            false
        }
        '?' => !t.is_empty() && glob_here(&p[1..], &t[1..]),
        '[' => {
            let Some(close) = p.iter().position(|c| *c == ']') else {
                return !t.is_empty() && p[0] == t[0] && glob_here(&p[1..], &t[1..]);
            };
            if t.is_empty() {
                return false;
            }
            let set = &p[1..close];
            let negate = set.first() == Some(&'!') || set.first() == Some(&'^');
            let body = if negate { &set[1..] } else { set };
            let hit = set_contains(body, t[0]);
            if hit == negate {
                return false;
            }
            glob_here(&p[close + 1..], &t[1..])
        }
        c => !t.is_empty() && c == t[0] && glob_here(&p[1..], &t[1..]),
    }
}

fn set_contains(set: &[char], c: char) -> bool {
    let mut i = 0;
    while i < set.len() {
        if i + 2 < set.len() && set[i + 1] == '-' {
            if set[i] <= c && c <= set[i + 2] {
                return true;
            }
            i += 3;
            continue;
        }
        if set[i] == c {
            return true;
        }
        i += 1;
    }
    false
}

/// `PopupFileBrowser.nameFilter(_:)`.
pub fn name_matches(pattern: &str, name: &str) -> bool {
    if name == ".." {
        return false;
    }
    if pattern.is_empty() {
        return true;
    }
    if has_glob(pattern) {
        glob_match(pattern, name)
    } else {
        name.to_lowercase().contains(&pattern.to_lowercase())
    }
}

/// `parseQuery(_:)` — with an explicit `cwd` / terminal words so it is testable.
pub fn parse_query(raw: &str, cwd: &str, terminal_words: &[String]) -> QueryMode {
    let q = raw.trim();
    if q.is_empty() {
        return QueryMode::All;
    }
    let mut words = q.splitn(2, ' ');
    let first = words.next().unwrap_or("").to_lowercase();
    let rest = words.next();
    if terminal_words.iter().any(|w| w.to_lowercase() == first) {
        if let Some(rest) = rest {
            let target = resolve_path(rest.trim(), cwd);
            let p = Path::new(&target);
            if p.exists() {
                let dir = if p.is_dir() {
                    target
                } else {
                    Path::new(&target)
                        .parent()
                        .map(|x| x.to_string_lossy().into_owned())
                        .unwrap_or(target)
                };
                return QueryMode::Terminal(dir);
            }
        } else {
            return QueryMode::Terminal(cwd.to_string());
        }
    }
    let path_like = q.starts_with('/') || q.starts_with('~') || q.contains('/');
    if !path_like {
        return if q.contains("**") {
            QueryMode::Recursive {
                base: cwd.to_string(),
                glob: q.to_string(),
            }
        } else {
            QueryMode::Local(q.to_string())
        };
    }
    let slash = q.rfind('/');
    let (head, tail) = match slash {
        Some(i) => (&q[..=i], &q[i + 1..]),
        None => ("", q),
    };
    if has_glob(head) {
        let mut base = if head.starts_with('/') {
            "/".to_string()
        } else if head.starts_with('~') {
            std::env::var("HOME").unwrap_or_default()
        } else {
            cwd.to_string()
        };
        let mut rest: Vec<String> = Vec::new();
        let mut literal = true;
        for comp in head.split('/') {
            if literal && comp == "~" && base == std::env::var("HOME").unwrap_or_default() {
                continue;
            }
            if literal && !has_glob(comp) && !comp.is_empty() {
                base = standardize(&format!("{}/{}", base, comp));
            } else if !comp.is_empty() {
                literal = false;
                rest.push(comp.to_string());
            }
        }
        rest.push(if tail.is_empty() {
            "*".to_string()
        } else {
            tail.to_string()
        });
        let glob = rest.join("/");
        return QueryMode::Recursive { base, glob };
    }
    let dir = if head.is_empty() {
        cwd.to_string()
    } else {
        resolve_path(head.trim_end_matches('/'), cwd)
    };
    if tail.contains("**") {
        QueryMode::Recursive {
            base: dir,
            glob: tail.to_string(),
        }
    } else {
        QueryMode::Dir {
            dir,
            pat: tail.to_string(),
        }
    }
}

pub fn resolve_path(s: &str, cwd: &str) -> String {
    let p = expand_tilde(s);
    if p.starts_with('/') {
        standardize(&p)
    } else {
        standardize(&format!("{}/{}", cwd, p))
    }
}

// -------------------------------------------------------------------- FileList

/// `FileListPane`'s selection model (no drawing).
#[derive(Clone, Debug, Default)]
pub struct FileList {
    pub rows: Vec<Entry>,
    pub selection: usize,
    pub marked: BTreeSet<usize>,
    pub anchor: usize,
}

impl FileList {
    pub fn new(rows: Vec<Entry>) -> Self {
        FileList {
            rows,
            selection: 0,
            marked: BTreeSet::new(),
            anchor: 0,
        }
    }

    pub fn set_rows(&mut self, rows: Vec<Entry>) {
        self.rows = rows;
        self.marked.clear();
        if self.selection >= self.rows.len() {
            self.selection = self.rows.len().saturating_sub(1);
        }
    }

    /// `selectedRows`.
    pub fn selected_rows(&self) -> Vec<usize> {
        if !self.marked.is_empty() {
            self.marked.iter().copied().collect()
        } else if self.selection < self.rows.len() {
            vec![self.selection]
        } else {
            Vec::new()
        }
    }

    pub fn select_all(&mut self) {
        let all: Vec<usize> = (0..self.rows.len()).filter(|&i| !self.rows[i].is_parent()).collect();
        if all.is_empty() {
            return;
        }
        self.marked = all.iter().copied().collect();
        if !self.marked.contains(&self.selection) {
            self.selection = all[0];
        }
        self.anchor = all[0];
    }

    pub fn extend_selection(&mut self, to: usize) {
        if to >= self.rows.len() {
            return;
        }
        if self.marked.is_empty() {
            self.anchor = self.selection;
        }
        let a = self.anchor.min(self.rows.len() - 1);
        let (lo, hi) = (a.min(to), a.max(to));
        self.marked = (lo..=hi).filter(|&i| !self.rows[i].is_parent()).collect();
        self.selection = to;
    }

    pub fn move_selection(&mut self, delta: i64) {
        if self.rows.is_empty() {
            return;
        }
        self.marked.clear();
        let last = self.rows.len() as i64 - 1;
        self.selection = (self.selection as i64 + delta).clamp(0, last) as usize;
        self.anchor = self.selection;
    }

    /// `mouseDown` Cmd-click.
    pub fn command_click(&mut self, idx: usize) -> usize {
        if idx >= self.rows.len() || self.rows[idx].is_parent() {
            return self.selection;
        }
        let mut m: BTreeSet<usize> =
            if self.marked.is_empty() && self.selection < self.rows.len() && !self.rows[self.selection].is_parent()
            {
                BTreeSet::from([self.selection])
            } else {
                self.marked.clone()
            };
        if m.contains(&idx) {
            m.remove(&idx);
        } else {
            m.insert(idx);
        }
        if m.contains(&idx) {
            self.selection = idx;
        } else if let Some(first) = m.iter().next() {
            self.selection = *first;
        }
        self.marked = if m.len() > 1 { m } else { BTreeSet::new() };
        self.anchor = self.selection;
        self.selection
    }

    pub fn row_rect(&self, i: usize, row_h: f64) -> (f64, f64, f64, f64) {
        (0.0, i as f64 * row_h, 0.0, row_h)
    }
}

// ---------------------------------------------------------------- places/lists

#[derive(Clone, Debug, PartialEq)]
pub struct PlaceEntry {
    pub title: String,
    pub symbol: String,
    pub path: String,
}

/// `PopupFileBrowser.places` — the standard folder shortcuts that exist.
pub fn places(home: &str) -> Vec<PlaceEntry> {
    let cands = [
        ("Home", "house", home.to_string()),
        ("Desktop", "menubar.dock.rectangle", format!("{home}/Desktop")),
        ("Documents", "doc", format!("{home}/Documents")),
        ("Downloads", "arrow.down.circle", format!("{home}/Downloads")),
    ];
    cands
        .iter()
        .filter(|(_, _, p)| Path::new(p).exists())
        .map(|(title, symbol, path)| PlaceEntry {
            title: (*title).to_string(),
            symbol: (*symbol).to_string(),
            path: path.clone(),
        })
        .collect()
}

/// The `virtualLists` metadata for Recent / Arrived (the providers are the
/// `RecentFiles` engine's `entries(false)` / `entries(true)`).
pub fn default_virtual_lists() -> Vec<(String, String)> {
    vec![
        ("Recent".to_string(), "clock".to_string()),
        ("Arrived".to_string(), "arrow.down.circle".to_string()),
    ]
}

// -------------------------------------------------------------------- actions

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Trash,
    Duplicate,
    Copy,
    Cut,
    Paste,
    NewFolder,
    NewFile,
    QuickLook,
    Enclosing,
    ToggleHidden,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PermContext {
    pub in_recent: bool,
    pub recursive: bool,
    pub has_ops_dir: bool,
    pub clipboard_files: usize,
    pub has_selection: bool,
}

/// `PopupFileBrowser.canPerform(_:)`.
pub fn can_perform(a: Action, ctx: &PermContext) -> bool {
    match a {
        Action::NewFolder | Action::NewFile => ctx.has_ops_dir,
        Action::Paste => ctx.has_ops_dir && ctx.clipboard_files > 0,
        Action::ToggleHidden => !ctx.in_recent,
        Action::Enclosing => ctx.in_recent || ctx.recursive,
        Action::Trash | Action::Duplicate | Action::Copy | Action::Cut | Action::QuickLook => {
            ctx.has_selection
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum OpenOutcome {
    Cd(String),
    Open(String),
    None,
}

/// `openIndex(_:)`.
pub fn open_index(rows: &[Entry], i: usize) -> OpenOutcome {
    match rows.get(i) {
        None => OpenOutcome::None,
        Some(e) if e.is_dir => OpenOutcome::Cd(e.path.clone()),
        Some(e) => OpenOutcome::Open(e.path.clone()),
    }
}

// --------------------------------------------------------------- file ops glue

/// `commitRename`'s validation + move (mirrors `PathsWindow.commitRename`).
pub fn rename_into(old_path: &str, new_name: &str) -> Result<Option<String>, String> {
    let old = Path::new(old_path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let new = new_name.trim();
    if new.is_empty() || new == old {
        return Ok(None);
    }
    if new.contains('/') || new == "." || new == ".." {
        return Err(format!("can't rename: “{new}” is not a valid name"));
    }
    let dir = Path::new(old_path)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let dst = format!("{dir}/{new}");
    if new.to_lowercase() != old.to_lowercase() && Path::new(&dst).exists() {
        return Err(format!("can't rename: “{new}” already exists"));
    }
    std::fs::rename(old_path, &dst).map_err(|e| format!("rename failed: {e}"))?;
    FileOps::record_rename(old_path, &dst, FileOps::shared());
    Ok(Some(dst))
}

pub fn duplicate_paths(paths: &[String], undo: &UndoStack) -> Outcome {
    FileOps::duplicate(paths, undo)
}

pub fn trash_paths(paths: &[String], undo: &UndoStack) -> Outcome {
    FileOps::trash(paths, undo)
}

pub fn transfer_paths(paths: &[String], into: &str, move_: bool, undo: &UndoStack) -> Outcome {
    FileOps::transfer(paths, into, move_, undo)
}

// ------------------------------------------------------------------ appkit layout

// `PopupFileBrowser` surface geometry (flipped, top-left origin).
pub const BROWSER_DEFAULT_WIDTH: f64 = 760.0;
pub const BROWSER_DEFAULT_HEIGHT: f64 = 460.0;
pub const BROWSER_ROW_HEIGHT: f64 = 22.0;
pub const BROWSER_TOOLBAR_HEIGHT: f64 = 30.0;
pub const BROWSER_PREVIEW_WIDTH: f64 = 220.0;
pub const BROWSER_MARGIN: f64 = 8.0;

/// The three regions of the file browser surface, for a given content size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BrowserLayout {
    pub toolbar: Rect,
    pub list: Rect,
    pub preview: Rect,
    /// The rows view height (at least the visible list height).
    pub rows_height: f64,
}

/// `PopupFileBrowser` layout — toolbar across the top, the file list on the
/// left and the preview/info pane on the right.
pub fn browser_layout(width: f64, height: f64, row_count: usize) -> BrowserLayout {
    let inner = (width - BROWSER_MARGIN * 2.0).max(0.0);
    let toolbar = Rect::new(BROWSER_MARGIN, 0.0, inner, BROWSER_TOOLBAR_HEIGHT);
    let top = BROWSER_TOOLBAR_HEIGHT + BROWSER_MARGIN;
    let preview_w = BROWSER_PREVIEW_WIDTH.min(inner * 0.5);
    let list_w = (inner - preview_w - BROWSER_MARGIN).max(0.0);
    let list_h = (height - top - BROWSER_MARGIN).max(0.0);
    let list = Rect::new(BROWSER_MARGIN, top, list_w, list_h);
    let preview = Rect::new(list.max_x() + BROWSER_MARGIN, top, preview_w, list_h);
    let rows_height = (row_count as f64 * BROWSER_ROW_HEIGHT).max(list_h);
    BrowserLayout { toolbar, list, preview, rows_height }
}

/// An "icon-ish" glyph for a row (dir / file / `..`), no SF Symbols in this cut.
pub fn row_glyph(e: &Entry) -> &'static str {
    if e.is_parent() {
        "↑"
    } else if e.is_dir {
        "▸"
    } else {
        "·"
    }
}

/// The preview pane's attribute text for an entry (kind, size, dates).
pub fn entry_detail(e: &Entry, now: f64) -> String {
    let mut lines = vec![if e.is_dir { "Folder" } else { "File" }.to_string()];
    if !e.is_dir {
        lines.push(format!("Size  {}", human_size(e.size)));
    }
    if e.modified > 0.0 {
        lines.push(format!("Modified  {}", short_date(e.modified, now)));
    }
    if e.created > 0.0 {
        lines.push(format!("Created  {}", short_date(e.created, now)));
    }
    lines.join("\n")
}

// ---------------------------------------------------------------- file browser

#[derive(Clone, Debug, PartialEq)]
struct Place {
    virtual_index: Option<usize>,
    dir: String,
}

/// The `PopupFileBrowser` browsing model (drawing / Quick Look / preview are
/// out of scope for this cut).
#[derive(Clone)]
pub struct FileBrowser {
    pub cwd: String,
    pub query: String,
    pub show_hidden: bool,
    pub mode: QueryMode,
    pub all: Vec<Entry>,
    pub list: FileList,
    pub virtual_index: Option<usize>,
    pub terminal_words: Vec<String>,
    pub sort_key: SortKey,
    pub sort_descending: bool,
    back: Vec<Place>,
    forward: Vec<Place>,
    travelling: bool,
    /// The built AppKit view tree (`build_macos`).
    #[cfg(target_os = "macos")]
    pub root: Option<Retained<NSView>>,
}

impl FileBrowser {
    pub fn new(start_dir: &str) -> Self {
        FileBrowser {
            cwd: standardize(start_dir),
            query: String::new(),
            show_hidden: false,
            mode: QueryMode::All,
            all: Vec::new(),
            list: FileList::default(),
            virtual_index: None,
            terminal_words: vec!["term".into(), "terminal".into(), "shell".into()],
            sort_key: SortKey::Name,
            sort_descending: false,
            back: Vec::new(),
            forward: Vec::new(),
            travelling: false,
            #[cfg(target_os = "macos")]
            root: None,
        }
    }

    pub fn in_recent(&self) -> bool {
        self.virtual_index.is_some()
    }

    pub fn where_text(&self) -> String {
        let home = std::env::var("HOME").unwrap_or_default();
        if self.cwd == home {
            return "~".to_string();
        }
        if !home.is_empty() {
            if let Some(rest) = self.cwd.strip_prefix(&format!("{home}/")) {
                return format!("~/{rest}");
            }
        }
        self.cwd.clone()
    }

    /// Feed the directory listing / Recent provider (the app supplies it).
    pub fn set_all(&mut self, mut entries: Vec<Entry>) {
        let key = self.sort_key;
        for e in entries.iter_mut() {
            decorate(e, key);
        }
        self.all = sort_entries(&entries, key, self.sort_descending);
        self.refilter();
    }

    pub fn list_dir(&self, dir: &str, hidden: bool) -> Vec<Entry> {
        let mut out = Vec::new();
        if let Some(parent) = Path::new(dir).parent() {
            if parent != Path::new(dir) {
                out.push(Entry::new("..", parent.to_string_lossy(), true, 0));
            }
        }
        let Ok(rd) = std::fs::read_dir(dir) else {
            return sort_entries(&out, self.sort_key, self.sort_descending);
        };
        for ent in rd.flatten() {
            let name = ent.file_name().to_string_lossy().into_owned();
            if !hidden && !self.show_hidden && name.starts_with('.') {
                continue;
            }
            let path = ent.path();
            let Ok(md) = ent.metadata() else { continue };
            let mut e = Entry::new(&name, path.to_string_lossy(), md.is_dir(), md.len() as i64);
            if let Ok(t) = md.modified() {
                if let Ok(d) = t.duration_since(std::time::UNIX_EPOCH) {
                    e.modified = d.as_secs_f64();
                }
            }
            if let Ok(t) = md.created() {
                if let Ok(d) = t.duration_since(std::time::UNIX_EPOCH) {
                    e.created = d.as_secs_f64();
                }
            }
            decorate(&mut e, self.sort_key);
            out.push(e);
        }
        sort_entries(&out, self.sort_key, self.sort_descending)
    }

    /// `reload()` for the plain (non-Recent) case.
    pub fn reload(&mut self) {
        if self.in_recent() {
            self.refilter();
            return;
        }
        let dir = self.cwd.clone();
        let entries = self.list_dir(&dir, self.show_hidden);
        self.set_all(entries);
    }

    /// `refilter()` for All / Local / Terminal / Dir (the live app's search
    /// scheduling is out of scope).
    pub fn refilter(&mut self) {
        self.mode = parse_query(&self.query, &self.cwd, &self.terminal_words);
        let rows: Vec<Entry> = match &self.mode {
            QueryMode::All | QueryMode::Terminal(_) => self.all.clone(),
            QueryMode::Local(pat) => {
                let mut source = self.all.clone();
                if pat.starts_with('.') {
                    source = self.list_dir(&self.cwd.clone(), true);
                }
                source
                    .into_iter()
                    .filter(|e| name_matches(pat, &e.name))
                    .collect()
            }
            QueryMode::Dir { dir, pat } => {
                let hidden = pat.starts_with('.');
                self.list_dir(dir, hidden)
                    .into_iter()
                    .filter(|e| name_matches(pat, &e.name))
                    .collect()
            }
            QueryMode::Recursive { .. } => Vec::new(),
        };
        self.list.set_rows(rows);
    }

    pub fn visible_rows(&self) -> &[Entry] {
        &self.list.rows
    }

    pub fn selection(&self) -> usize {
        self.list.selection
    }

    pub fn selected_rows(&self) -> Vec<usize> {
        self.list.selected_rows()
    }

    /// `selectedPaths()` — non-`..` rows only.
    pub fn selected_paths(&self) -> Vec<String> {
        self.list
            .selected_rows()
            .into_iter()
            .filter(|&i| !self.list.rows[i].is_parent())
            .map(|i| self.list.rows[i].path.clone())
            .collect()
    }

    /// `opsDirectory`.
    pub fn ops_directory(&self) -> Option<String> {
        if self.in_recent() {
            return None;
        }
        match &self.mode {
            QueryMode::All | QueryMode::Local(_) | QueryMode::Terminal(_) => Some(self.cwd.clone()),
            QueryMode::Dir { dir, .. } => Some(dir.clone()),
            QueryMode::Recursive { .. } => None,
        }
    }

    pub fn perm_context(&self, clipboard_files: usize) -> PermContext {
        PermContext {
            in_recent: self.in_recent(),
            recursive: matches!(self.mode, QueryMode::Recursive { .. }),
            has_ops_dir: self.ops_directory().is_some(),
            clipboard_files,
            has_selection: !self.selected_paths().is_empty(),
        }
    }

    fn place(&self) -> Place {
        Place {
            virtual_index: self.virtual_index,
            dir: self.cwd.clone(),
        }
    }

    fn remember(&mut self) {
        if self.travelling {
            return;
        }
        let p = self.place();
        if self.back.last() != Some(&p) {
            self.back.push(p);
        }
        if self.back.len() > 50 {
            self.back.remove(0);
        }
        self.forward.clear();
    }

    pub fn cd(&mut self, dir: &str) {
        let dest = standardize(dir);
        if self.in_recent() || dest != self.cwd {
            self.remember();
        }
        self.virtual_index = None;
        self.cwd = dest;
        self.query.clear();
        self.list.selection = 0;
        self.reload();
    }

    pub fn cd_parent(&mut self) {
        if self.in_recent() {
            let cwd = self.cwd.clone();
            self.cd(&cwd);
            return;
        }
        if let Some(parent) = Path::new(&self.cwd).parent() {
            let parent = parent.to_string_lossy().into_owned();
            if parent != self.cwd {
                self.cd(&parent);
            }
        }
    }

    pub fn go_back(&mut self) {
        let Some(to) = self.back.pop() else { return };
        self.forward.push(self.place());
        self.travelling = true;
        if let Some(v) = to.virtual_index {
            self.show_virtual(v);
        } else {
            self.cd(&to.dir);
        }
        self.travelling = false;
    }

    pub fn go_forward(&mut self) {
        let Some(to) = self.forward.pop() else { return };
        self.back.push(self.place());
        self.travelling = true;
        if let Some(v) = to.virtual_index {
            self.show_virtual(v);
        } else {
            self.cd(&to.dir);
        }
        self.travelling = false;
    }

    pub fn show_virtual(&mut self, i: usize) {
        self.virtual_index = Some(i);
        self.query.clear();
        self.list.selection = 0;
        self.reload();
    }

    pub fn toggle_hidden(&mut self) {
        if self.in_recent() {
            return;
        }
        self.show_hidden = !self.show_hidden;
        self.reload();
    }

    pub fn set_query(&mut self, q: &str) {
        self.query = q.to_string();
        self.refilter();
    }

    /// Build the AppKit browser: the toolbar, the scrollable rows list and the
    /// preview/info pane. No-op off macOS.
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
        let where_text = self.where_text();
        let selected = self.list.rows.get(self.list.selection).cloned();
        let view = macos::build_view(mtm, &where_text, self.visible_rows(), selected.as_ref(), &colors);
        self.root = Some(view);
    }

    /// The built AppKit root view, once [`Self::build`] has run.
    #[cfg(target_os = "macos")]
    pub fn content_view(&self) -> Option<Retained<NSView>> {
        self.root.clone()
    }

    pub fn test_state(&self) -> Value {
        json!({
            "cwd": self.cwd,
            "where": self.where_text(),
            "query": self.query,
            "selection": self.list.selection,
            "marked": self.list.marked.iter().copied().collect::<Vec<_>>(),
            "rows": self.list.rows.iter().map(|e| json!({
                "name": e.name, "path": e.path, "isDir": e.is_dir
            })).collect::<Vec<_>>(),
        })
    }
}

impl Default for FileBrowser {
    fn default() -> Self {
        FileBrowser::new(&std::env::var("HOME").unwrap_or_else(|_| "/".to_string()))
    }
}

impl std::fmt::Debug for FileBrowser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileBrowser")
            .field("cwd", &self.cwd)
            .field("query", &self.query)
            .field("show_hidden", &self.show_hidden)
            .field("mode", &self.mode)
            .field("all", &self.all)
            .field("list", &self.list)
            .field("virtual_index", &self.virtual_index)
            .field("sort_key", &self.sort_key)
            .field("sort_descending", &self.sort_descending)
            .finish()
    }
}

/// Build a default browser view rooted at `$HOME` (the real `reload()`
/// listing), for the host to drop into a window.
#[cfg(target_os = "macos")]
pub fn build_content(mtm: objc2::MainThreadMarker) -> Option<Retained<NSView>> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
    let mut browser = FileBrowser::new(&home);
    browser.reload();
    browser.build(mtm);
    browser.content_view()
}

/// Convenience: a `PathBuf` display helper the ops glue can use.
pub fn display_path(p: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    if p == home {
        return "~".to_string();
    }
    if !home.is_empty() {
        if let Some(rest) = p.strip_prefix(&format!("{home}/")) {
            return format!("~/{rest}");
        }
    }
    p.to_string()
}

#[allow(dead_code)]
fn as_pathbuf(p: &str) -> PathBuf {
    PathBuf::from(p)
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

    fn wrapping_label(
        mtm: MainThreadMarker,
        s: &str,
        size: f64,
        color: Rgba,
    ) -> Retained<NSTextField> {
        let l = NSTextField::wrappingLabelWithString(&NSString::from_str(s), mtm);
        l.setFont(Some(&NSFont::systemFontOfSize(size)));
        l.setTextColor(Some(&color.to_nscolor()));
        l.setSelectable(true);
        l
    }

    pub struct FlippedViewIvars;

    define_class!(
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSFilesFlippedView"]
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

    fn set_background(view: &NSView, color: Rgba) {
        view.setWantsLayer(true);
        if let Some(layer) = view.layer() {
            layer.setBackgroundColor(Some(&color.to_nscolor().CGColor()));
        }
    }

    /// The browser view tree: toolbar, scrollable row list, preview pane.
    pub fn build_view(
        mtm: MainThreadMarker,
        where_text: &str,
        rows: &[Entry],
        selected: Option<&Entry>,
        colors: &PopupColors,
    ) -> Retained<NSView> {
        let w = BROWSER_DEFAULT_WIDTH;
        let h = BROWSER_DEFAULT_HEIGHT;
        let layout = browser_layout(w, h, rows.len());

        let root = FlippedView::new(mtm);
        root.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(w, h)));
        root.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        set_background(&root, colors.mantle());

        // Toolbar: the current path (`where_text()`).
        let toolbar = label(mtm, where_text, 13.0, colors.text);
        toolbar.setFrame(nsrect(layout.toolbar));
        toolbar.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        root.addSubview(&toolbar);

        // Main list: one flipped row view per visible entry.
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
        for (i, e) in rows.iter().enumerate() {
            let row = FlippedView::new(mtm);
            row.setFrame(NSRect::new(
                NSPoint::new(0.0, i as f64 * BROWSER_ROW_HEIGHT),
                NSSize::new(row_w, BROWSER_ROW_HEIGHT),
            ));
            row.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);

            let glyph = label(mtm, row_glyph(e), 12.0, colors.accent_on());
            glyph.setFrame(NSRect::new(NSPoint::new(4.0, 2.0), NSSize::new(18.0, 18.0)));
            row.addSubview(&glyph);

            let name = label(mtm, &e.name, 12.0, colors.text);
            name.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
            name.setFrame(NSRect::new(
                NSPoint::new(24.0, 2.0),
                NSSize::new((row_w - 150.0).max(1.0), 18.0),
            ));
            row.addSubview(&name);

            let trailing = label(mtm, &e.trailing_text, 11.0, colors.dim);
            trailing.setAlignment(NSTextAlignment::Right);
            trailing.setLineBreakMode(NSLineBreakMode::ByTruncatingHead);
            trailing.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
            trailing.setFrame(NSRect::new(
                NSPoint::new((row_w - 136.0).max(1.0), 2.0),
                NSSize::new(128.0, 18.0),
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

        // Preview / info pane (name, path, attributes; no file reads).
        let preview = FlippedView::new(mtm);
        preview.setFrame(nsrect(layout.preview));
        preview.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        set_background(&preview, colors.crust());
        if let Some(layer) = preview.layer() {
            layer.setCornerRadius(6.0);
        }

        let (title, path_line, detail) = match selected {
            Some(e) => (
                e.name.clone(),
                display_path(&e.path),
                entry_detail(e, 0.0),
            ),
            None => (
                "No selection".to_string(),
                String::new(),
                String::new(),
            ),
        };
        let pw = layout.preview.width.max(1.0);

        let title_l = NSTextField::labelWithString(&NSString::from_str(&title), mtm);
        title_l.setFont(Some(&NSFont::boldSystemFontOfSize(13.0)));
        title_l.setTextColor(Some(&colors.text.to_nscolor()));
        title_l.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
        title_l.setFrame(NSRect::new(
            NSPoint::new(10.0, 8.0),
            NSSize::new((pw - 20.0).max(1.0), 18.0),
        ));
        preview.addSubview(&title_l);

        let path_l = wrapping_label(mtm, &path_line, 11.0, colors.dim);
        path_l.setFrame(NSRect::new(
            NSPoint::new(10.0, 28.0),
            NSSize::new((pw - 20.0).max(1.0), 44.0),
        ));
        preview.addSubview(&path_l);

        let detail_l = wrapping_label(mtm, &detail, 11.0, colors.dim);
        detail_l.setFrame(NSRect::new(
            NSPoint::new(10.0, 80.0),
            NSSize::new((pw - 20.0).max(1.0), 96.0),
        ));
        preview.addSubview(&detail_l);

        root.addSubview(&preview);

        root.into_super()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows() -> Vec<Entry> {
        vec![
            Entry::new("..", "/tmp", true, 0),
            Entry::new("A", "/tmp/A", false, 10),
            Entry::new("b", "/tmp/b", false, 20),
            Entry::new("c", "/tmp/c", false, 30),
        ]
    }

    #[test]
    fn mark_and_select() {
        let mut l = FileList::new(rows());
        assert_eq!(l.selected_rows(), vec![0]);
        l.selection = 1;
        assert_eq!(l.selected_rows(), vec![1]);

        l.select_all();
        assert_eq!(l.marked.iter().copied().collect::<Vec<_>>(), vec![1, 2, 3], ".. is excluded");
        assert_eq!(l.selection, 1);
        assert_eq!(l.anchor, 1);

        l.extend_selection(2);
        assert_eq!(l.marked.iter().copied().collect::<Vec<_>>(), vec![1, 2]);
        l.extend_selection(0);
        assert_eq!(l.marked.iter().copied().collect::<Vec<_>>(), vec![1], "range folds in .., excluded");
        assert_eq!(l.selection, 0);

        l.move_selection(1);
        assert!(l.marked.is_empty(), "move clears marked");
        assert_eq!(l.selection, 1);
        assert_eq!(l.anchor, 1);
        l.move_selection(100);
        assert_eq!(l.selection, 3, "clamped");
    }

    #[test]
    fn command_click_toggles_and_collapses() {
        let mut l = FileList::new(rows());
        l.selection = 1;
        assert_eq!(l.command_click(2), 2);
        assert_eq!(l.marked.iter().copied().collect::<Vec<_>>(), vec![1, 2]);
        // click 2 again -> only 1 remains, collapses to a single selection
        assert_eq!(l.command_click(2), 1);
        assert!(l.marked.is_empty());
        assert_eq!(l.selection, 1);
        // clicking ".." is a no-op
        assert_eq!(l.command_click(0), 1);
    }

    #[test]
    fn human_size_units() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1024), "1.0 KB");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(1024 * 1024), "1.0 MB");
    }

    #[test]
    fn sort_dirs_first_and_natural() {
        let mut list = rows();
        list.push(Entry::new("Dir", "/tmp/Dir", true, 0));
        let sorted = sort_entries(&list, SortKey::Name, false);
        assert_eq!(sorted[0].name, "..");
        assert_eq!(sorted[1].name, "Dir", "dirs before files");
        assert_eq!(
            sorted[2..].iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            vec!["A", "b", "c"]
        );

        let desc = sort_entries(&list, SortKey::Size, true);
        let sizes: Vec<i64> = desc
            .iter()
            .filter(|e| !e.is_parent() && !e.is_dir)
            .map(|e| e.size)
            .collect();
        assert_eq!(sizes, vec![30, 20, 10]);
    }

    #[test]
    fn name_filter_substring_and_glob() {
        assert!(name_matches("", "a.txt"));
        assert!(!name_matches("", ".."));
        assert!(name_matches("txt", "a.txt"));
        assert!(name_matches("TXT", "a.txt"));
        assert!(!name_matches("md", "a.txt"));
        assert!(name_matches("*.txt", "a.txt"));
        assert!(!name_matches("*.md", "a.txt"));
        assert!(name_matches("?.txt", "a.txt"));
        assert!(name_matches("[ab].txt", "a.txt"));
        assert!(!name_matches("*.txt", ".."), ".. never matches");
        assert!(glob_match("a*c", "abc"));
        assert!(glob_match("*", "anything"));
        assert!(!glob_match("a", "ab"));
    }

    #[test]
    fn parse_query_modes() {
        let cwd = "/work";
        let tw = vec!["term".to_string()];
        assert_eq!(parse_query("", cwd, &tw), QueryMode::All);
        assert_eq!(parse_query("foo", cwd, &tw), QueryMode::Local("foo".into()));
        assert_eq!(
            parse_query("**", cwd, &tw),
            QueryMode::Recursive {
                base: "/work".into(),
                glob: "**".into()
            }
        );
        assert_eq!(
            parse_query("src/foo", cwd, &tw),
            QueryMode::Dir {
                dir: "/work/src".into(),
                pat: "foo".into()
            }
        );
        assert_eq!(parse_query("term", cwd, &tw), QueryMode::Terminal("/work".into()));
        let rec = parse_query("src/**/*.rs", cwd, &tw);
        match rec {
            QueryMode::Recursive { base, glob } => {
                assert_eq!(base, "/work/src");
                assert_eq!(glob, "**/*.rs");
            }
            other => panic!("expected recursive, got {other:?}"),
        }
    }

    #[test]
    fn standardize_and_resolve() {
        assert_eq!(standardize("/a/b/../c"), "/a/c");
        assert_eq!(standardize("/a/./b"), "/a/b");
        assert_eq!(resolve_path("src/x", "/work"), "/work/src/x");
        assert_eq!(resolve_path("/abs/./x", "/work"), "/abs/x");
    }

    #[test]
    fn rename_validation_and_move() {
        let root = std::env::temp_dir().join(format!("ws-rs-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let old = root.join("a.txt");
        std::fs::write(&old, "x").unwrap();
        let old = old.to_string_lossy().into_owned();

        assert_eq!(rename_into(&old, "a.txt").unwrap(), None);
        assert_eq!(rename_into(&old, "").unwrap(), None);
        assert!(rename_into(&old, "x/y").is_err());
        assert!(rename_into(&old, "..").is_err());

        std::fs::write(root.join("b.txt"), "x").unwrap();
        assert!(rename_into(&old, "b.txt").is_err());

        let dst = rename_into(&old, "new.txt").unwrap().unwrap();
        assert!(Path::new(&dst).exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn can_perform_matrix() {
        let none = PermContext {
            in_recent: false,
            recursive: false,
            has_ops_dir: false,
            clipboard_files: 0,
            has_selection: false,
        };
        assert!(!can_perform(Action::NewFolder, &none));
        assert!(!can_perform(Action::Paste, &none));
        assert!(can_perform(Action::ToggleHidden, &none));
        assert!(!can_perform(Action::Trash, &none));

        let all = PermContext {
            in_recent: false,
            recursive: false,
            has_ops_dir: true,
            clipboard_files: 2,
            has_selection: true,
        };
        for a in [
            Action::NewFolder,
            Action::NewFile,
            Action::Paste,
            Action::Trash,
            Action::Duplicate,
            Action::Copy,
            Action::Cut,
            Action::QuickLook,
        ] {
            assert!(can_perform(a, &all), "{a:?}");
        }
        assert!(!can_perform(Action::Enclosing, &all), "not recent, not recursive");
        assert!(can_perform(
            Action::Enclosing,
            &PermContext {
                recursive: true,
                ..all
            }
        ));
        assert!(!can_perform(
            Action::ToggleHidden,
            &PermContext {
                in_recent: true,
                ..all
            }
        ));
    }

    #[test]
    fn open_index_dir_vs_file() {
        let r = rows();
        assert_eq!(open_index(&r, 0), OpenOutcome::Cd("/tmp".into()));
        assert_eq!(open_index(&r, 1), OpenOutcome::Open("/tmp/A".into()));
        assert_eq!(open_index(&r, 99), OpenOutcome::None);
    }

    #[test]
    fn browser_history_and_filter() {
        let mut b = FileBrowser::new("/tmp");
        b.set_all(vec![
            Entry::new("a.txt", "/tmp/a.txt", false, 1),
            Entry::new("b.md", "/tmp/b.md", false, 2),
        ]);
        assert_eq!(b.visible_rows().len(), 2);
        b.set_query("a");
        assert_eq!(b.visible_rows().len(), 1);
        assert_eq!(b.visible_rows()[0].name, "a.txt");
        assert_eq!(b.ops_directory().as_deref(), Some("/tmp"));

        b.set_query("");
        b.list.selection = 1;
        assert_eq!(b.selected_paths(), vec!["/tmp/b.md".to_string()]);
    }

    #[test]
    fn places_filters_missing() {
        let root = std::env::temp_dir().join(format!("ws-rs-places-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Desktop")).unwrap();
        let home = root.to_string_lossy().into_owned();
        let p = places(&home);
        let titles: Vec<&str> = p.iter().map(|e| e.title.as_str()).collect();
        assert!(titles.contains(&"Home"));
        assert!(titles.contains(&"Desktop"));
        assert!(!titles.contains(&"Documents"), "missing folder is dropped");
        let _ = std::fs::remove_dir_all(&root);

        assert_eq!(default_virtual_lists()[0].0, "Recent");
        assert_eq!(default_virtual_lists()[1].0, "Arrived");
    }

    #[test]
    fn short_date_and_ago() {
        assert_eq!(short_date(0.0, 0.0), "");
        // 2021-01-02 03:04 UTC
        let s = short_date(1609556640.0, 1609556640.0);
        assert_eq!(s, "Jan 2 03:04");
        assert_eq!(ago(0.0), "just now");
        assert_eq!(ago(120.0), "2m ago");
        assert_eq!(ago(7200.0), "2h ago");
        assert_eq!(ago(172800.0), "2d ago");
    }

    #[test]
    fn decorate_size_and_date() {
        let mut e = Entry::new("a.txt", "/a.txt", false, 2048);
        decorate(&mut e, SortKey::Name);
        assert_eq!(e.trailing_text, "2.0 KB");
        let mut d = Entry::new("dir", "/dir", true, 0);
        decorate(&mut d, SortKey::Name);
        assert_eq!(d.trailing_text, "", "dirs show no size");
    }

    #[test]
    fn browser_layout_regions() {
        let l = browser_layout(760.0, 460.0, 3);
        assert_eq!(l.toolbar.y, 0.0);
        assert_eq!(l.toolbar.height, BROWSER_TOOLBAR_HEIGHT);
        assert!(l.list.x < l.preview.x);
        assert!(l.list.max_x() <= l.preview.x + 0.001, "list and preview do not overlap");
        assert!(l.rows_height >= 3.0 * BROWSER_ROW_HEIGHT);
        assert_eq!(l.rows_height, l.list.height, "short listing fills the visible list");

        // A long listing grows the rows view beyond the visible list.
        let tall = browser_layout(760.0, 460.0, 100);
        assert!(tall.rows_height >= tall.list.height);
        assert_eq!(tall.rows_height, 100.0 * BROWSER_ROW_HEIGHT);

        // Degenerate sizes never produce a negative rect.
        let tiny = browser_layout(10.0, 10.0, 0);
        assert!(tiny.list.width >= 0.0 && tiny.list.height >= 0.0);
        assert!(tiny.preview.width >= 0.0);
    }

    #[test]
    fn row_glyph_kinds() {
        assert_eq!(row_glyph(&Entry::new("..", "/", true, 0)), "↑");
        assert_eq!(row_glyph(&Entry::new("dir", "/dir", true, 0)), "▸");
        assert_eq!(row_glyph(&Entry::new("f", "/f", false, 0)), "·");
    }

    #[test]
    fn entry_detail_lines() {
        let mut e = Entry::new("a.txt", "/a.txt", false, 2048);
        e.modified = 1609556640.0;
        let d = entry_detail(&e, 0.0);
        assert!(d.starts_with("File"));
        assert!(d.contains("2.0 KB"));
        assert!(d.contains("Modified"));

        let dir = Entry::new("d", "/d", true, 0);
        let dd = entry_detail(&dir, 0.0);
        assert!(dd.starts_with("Folder"));
        assert!(!dd.contains("Size"), "folders show no size");
    }
}
