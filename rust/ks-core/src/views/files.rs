//! Port of `FilePopup.swift` + `PopupFileBrowser` (in `PopupWindow.swift`) —
//! the file browser model: rows/selection/marked set, the filter model,
//! Places/Recent/Arrived lists, rename/duplicate/trash and drop/open actions.
//!
//! `FileListPane` lives in `PopupWindow.swift`; it is modelled locally as
//! [`FileList`] (no AppKit). The AppKit surface is real: [`FileBrowser::build`]
//! builds the toolbar, the scrollable row list (with selected/marked row
//! highlighting) and the preview/info pane, and [`QuickLook`] ports the
//! `QLPreviewPanelDataSource`/`QLPreviewPanelDelegate` pair
//! ([`FileBrowser::toggle_quick_look`], [`FileBrowser::refresh_quick_look`]).
//! Drag-and-drop lives in the view (`files_view`: drag out and drop in); the
//! per-row `NSWorkspace` file icons are not modelled here.

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::{json, Value};

use crate::engines::file_ops::{FileOps, Outcome, UndoStack};
use crate::engines::list_filter::localized_standard_compare;
use crate::ui::chrome::Rect;

#[cfg(target_os = "macos")]
use objc2::rc::Retained;
#[cfg(target_os = "macos")]
use objc2::runtime::{AnyObject, NSObject, ProtocolObject};
#[cfg(target_os = "macos")]
use objc2::{define_class, msg_send, DefinedClass, MainThreadOnly};
#[cfg(target_os = "macos")]
use objc2_app_kit::{NSEvent, NSEventType, NSView, NSWindowDelegate};
#[cfg(target_os = "macos")]
use objc2_foundation::{NSObjectProtocol, NSString, NSURL};
#[cfg(target_os = "macos")]
use objc2_quick_look_ui::{
    QLPreviewItem, QLPreviewPanel, QLPreviewPanelDataSource, QLPreviewPanelDelegate,
};

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: i64,
    pub created: f64,
    pub modified: f64,
    pub trailing_text: String,
    /// Nesting below the listed folder (Opt+→ expands a folder in place).
    pub depth: u16,
    /// A folder whose children are shown under it.
    pub expanded: bool,
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
            depth: 0,
            expanded: false,
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

/// A local date/time, Finder style: "19 Nov 2026 at 14:05".
pub fn long_date(epoch: f64) -> String {
    #[cfg(unix)]
    {
        let t = epoch as libc::time_t;
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
            return String::new();
        }
        let mut buf = [0u8; 64];
        let fmt = b"%e %b %Y at %H:%M\0";
        let n = unsafe {
            libc::strftime(buf.as_mut_ptr() as *mut libc::c_char, buf.len(), fmt.as_ptr() as *const libc::c_char, &tm)
        };
        String::from_utf8_lossy(&buf[..n]).trim().to_string()
    }
    #[cfg(not(unix))]
    {
        format!("{epoch:.0}")
    }
}

/// `drwxr-xr-x`-style permissions from a mode.
pub fn mode_string(mode: u32, is_dir: bool, is_link: bool) -> String {
    let mut s = String::with_capacity(10);
    s.push(if is_link { 'l' } else if is_dir { 'd' } else { '-' });
    for shift in [6u32, 3, 0] {
        let b = (mode >> shift) & 7;
        s.push(if b & 4 != 0 { 'r' } else { '-' });
        s.push(if b & 2 != 0 { 'w' } else { '-' });
        s.push(if b & 1 != 0 { 'x' } else { '-' });
    }
    s
}

/// The size of everything below `dir` (bytes, files counted), stopping
/// after `budget` so a huge tree can't stall Get Info; `complete` is false
/// when it stopped early.
pub fn tree_size(dir: &Path, budget: std::time::Duration) -> (u64, u64, bool) {
    let start = std::time::Instant::now();
    let (mut bytes, mut files) = (0u64, 0u64);
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        if start.elapsed() > budget {
            return (bytes, files, false);
        }
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let Ok(m) = e.path().symlink_metadata() else { continue };
            if m.is_dir() {
                stack.push(e.path());
            } else {
                bytes += m.len();
                files += 1;
            }
        }
    }
    (bytes, files, true)
}

/// Finder's Get Info (Cmd+I) as label/value rows for one or more paths.
pub fn file_info_rows(paths: &[String], budget: std::time::Duration) -> (String, Vec<(String, String)>) {
    #[cfg(unix)]
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let mut rows: Vec<(String, String)> = Vec::new();
    if paths.len() > 1 {
        let (mut bytes, mut files, mut complete) = (0u64, 0u64, true);
        for p in paths {
            let path = Path::new(p);
            match path.symlink_metadata() {
                Ok(m) if m.is_dir() => {
                    let (b, f, c) = tree_size(path, budget);
                    bytes += b;
                    files += f;
                    complete &= c;
                }
                Ok(m) => {
                    bytes += m.len();
                    files += 1;
                }
                Err(_) => {}
            }
        }
        let more = if complete { "" } else { "+ (still counting when it stopped)" };
        rows.push(("Size".into(), format!("{}{more} ({files} files)", human_size(bytes as i64))));
        let dir = Path::new(&paths[0]).parent().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
        rows.push(("Where".into(), display_path(&dir)));
        return (format!("{} items", paths.len()), rows);
    }
    let Some(p) = paths.first() else { return (String::new(), rows) };
    let path = Path::new(p);
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.clone());
    let Ok(lm) = path.symlink_metadata() else {
        rows.push(("Error".into(), "can't read it".into()));
        return (name, rows);
    };
    let is_link = lm.file_type().is_symlink();
    let m = std::fs::metadata(path).unwrap_or(lm.clone());
    let kind = if is_link {
        "Alias (symbolic link)".to_string()
    } else if m.is_dir() {
        if p.ends_with(".app") { "Application".into() } else { "Folder".into() }
    } else {
        match path.extension().map(|e| e.to_string_lossy().to_lowercase()) {
            Some(e) if !e.is_empty() => format!("{} file", e.to_uppercase()),
            _ => "Document".into(),
        }
    };
    rows.push(("Kind".into(), kind));
    if m.is_dir() {
        let (b, f, complete) = tree_size(path, budget);
        let more = if complete { "" } else { "+ (still counting when it stopped)" };
        rows.push(("Size".into(), format!("{}{more} for {f} files", human_size(b as i64))));
        let n = std::fs::read_dir(path).map(|r| r.count()).unwrap_or(0);
        rows.push(("Contains".into(), format!("{n} item{}", if n == 1 { "" } else { "s" })));
    } else {
        rows.push(("Size".into(), format!("{} ({} bytes)", human_size(m.len() as i64), m.len())));
    }
    let dir = path.parent().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
    rows.push(("Where".into(), display_path(&dir)));
    if is_link {
        if let Ok(t) = std::fs::read_link(path) {
            rows.push(("Original".into(), t.to_string_lossy().into_owned()));
        }
    }
    let secs = |t: std::io::Result<std::time::SystemTime>| {
        t.ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs_f64())
    };
    if let Some(c) = secs(m.created()) {
        rows.push(("Created".into(), long_date(c)));
    }
    if let Some(c) = secs(m.modified()) {
        rows.push(("Modified".into(), long_date(c)));
    }
    #[cfg(unix)]
    {
        let owner = unsafe {
            let pw = libc::getpwuid(m.uid());
            if pw.is_null() {
                m.uid().to_string()
            } else {
                std::ffi::CStr::from_ptr((*pw).pw_name).to_string_lossy().into_owned()
            }
        };
        rows.push((
            "Permissions".into(),
            format!("{} {owner}", mode_string(m.permissions().mode(), m.is_dir(), is_link)),
        ));
    }
    (name, rows)
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

/// `shortDate(_:)`: `"MMM d HH:mm"` this year, else `"MMM d yyyy"`, in the
/// Mac's local time zone (like `DateFormatter`; it used to print UTC).
pub fn short_date(epoch: f64, now: f64) -> String {
    if epoch <= 0.0 {
        return String::new();
    }
    let local = |t: f64| {
        let d = crate::engines::screenshot_annotations::ShotDate::from_unix_local(t.floor() as i64);
        (d.year, d.month as i64, d.day as i64, d.hour as i64, d.minute as i64)
    };
    let now_year = if now > 0.0 { Some(local(now).0) } else { None };
    short_date_fields(local(epoch), now_year)
}

/// The `short_date` text for calendar fields `(y, m, d, hh, mm)`.
fn short_date_fields((y, m, d, hh, mm): (i64, i64, i64, i64, i64), now_year: Option<i64>) -> String {
    if y == now_year.unwrap_or(y) {
        format!("{} {} {:02}:{:02}", MONTHS[(m - 1) as usize], d, hh, mm)
    } else {
        format!("{} {} {}", MONTHS[(m - 1) as usize], d, y)
    }
}

#[cfg(test)]
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

/// A recursive-search glob over a path *relative to the search base*
/// (Swift `pathGlobRegex(_:)`): `**` crosses folders (`**/` may match none),
/// `*` / `?` stay inside one name, and a glob with no `/` matches the file
/// name at any depth. Case-insensitive, like the name filter.
pub fn path_glob_match(pattern: &str, rel: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = rel.to_lowercase().chars().collect();
    if pattern.contains('/') {
        return path_glob_here(&p, &t);
    }
    // No `/`: Swift prefixes `^(?:.*/)?`, i.e. try every path suffix.
    let mut start = 0;
    loop {
        if path_glob_here(&p, &t[start..]) {
            return true;
        }
        match t[start..].iter().position(|c| *c == '/') {
            Some(i) => start += i + 1,
            None => return false,
        }
    }
}

fn path_glob_here(p: &[char], t: &[char]) -> bool {
    if p.is_empty() {
        return t.is_empty();
    }
    if p.len() >= 2 && p[0] == '*' && p[1] == '*' {
        if p.len() >= 3 && p[2] == '/' {
            // `**/` = zero or more whole folders.
            for i in 0..=t.len() {
                if (i == 0 || t[i - 1] == '/') && path_glob_here(&p[3..], &t[i..]) {
                    return true;
                }
            }
            return false;
        }
        // A bare `**` = anything at all, `/` included.
        return (0..=t.len()).any(|i| path_glob_here(&p[2..], &t[i..]));
    }
    match p[0] {
        '*' => {
            let mut i = 0;
            loop {
                if path_glob_here(&p[1..], &t[i..]) {
                    return true;
                }
                if i >= t.len() || t[i] == '/' {
                    return false;
                }
                i += 1;
            }
        }
        '?' => !t.is_empty() && t[0] != '/' && path_glob_here(&p[1..], &t[1..]),
        '[' => {
            let Some(close) = p.iter().position(|c| *c == ']') else {
                return !t.is_empty() && p[0] == t[0] && path_glob_here(&p[1..], &t[1..]);
            };
            if t.is_empty() || t[0] == '/' {
                return false;
            }
            let set = &p[1..close];
            let negate = set.first() == Some(&'!') || set.first() == Some(&'^');
            let body = if negate { &set[1..] } else { set };
            if set_contains(body, t[0]) == negate {
                return false;
            }
            path_glob_here(&p[close + 1..], &t[1..])
        }
        c => !t.is_empty() && c == t[0] && path_glob_here(&p[1..], &t[1..]),
    }
}

/// `startSearch`'s glob fix-up: a last component with no wildcard becomes a
/// substring match (`**/name` → `**/*name*`), like the local filter.
pub fn widen_recursive_glob(glob: &str) -> String {
    let last = glob.rsplit('/').next().unwrap_or("");
    if last.is_empty() || has_glob(last) {
        glob.to_string()
    } else {
        format!("{}*{last}*", &glob[..glob.len() - last.len()])
    }
}

/// `browserSearchLimit` (the Swift default).
pub const RECURSIVE_SEARCH_LIMIT: usize = 2000;
/// Wall-clock budget for one recursive search: it runs on the main thread per
/// keystroke, and the Files root can be the whole home folder.
pub const RECURSIVE_SEARCH_BUDGET: std::time::Duration = std::time::Duration::from_millis(400);
/// `browserSearchExcludes` — folder names never searched into.
pub const RECURSIVE_SEARCH_EXCLUDES: [&str; 3] = ["Library", "node_modules", ".Trash"];

/// The recursive listing (`scheduleSearch` / `walk`): files below `base`
/// whose path relative to it matches `glob`, at most `limit` of them.
/// Directories are not rows, hidden names are skipped unless `hidden`, and the
/// shared search excludes are never descended into.
pub fn recursive_search(base: &str, glob: &str, hidden: bool, limit: usize) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    let root = Path::new(base);
    let deadline = std::time::Instant::now() + RECURSIVE_SEARCH_BUDGET;
    walk_tree(root, root, glob, hidden, limit, deadline, &mut out);
    out
}

fn walk_tree(
    dir: &Path,
    base: &Path,
    glob: &str,
    hidden: bool,
    limit: usize,
    deadline: std::time::Instant,
    out: &mut Vec<Entry>,
) {
    if out.len() >= limit || std::time::Instant::now() >= deadline {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<std::fs::DirEntry> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for ent in entries {
        let name = ent.file_name().to_string_lossy().into_owned();
        if !hidden && name.starts_with('.') {
            continue;
        }
        let Ok(ft) = ent.file_type() else { continue };
        let path = ent.path();
        if ft.is_dir() {
            if RECURSIVE_SEARCH_EXCLUDES.contains(&name.as_str()) {
                continue;
            }
            walk_tree(&path, base, glob, hidden, limit, deadline, out);
            if out.len() >= limit {
                return;
            }
            continue;
        }
        let rel = match path.strip_prefix(base) {
            Ok(r) => r.to_string_lossy().into_owned(),
            Err(_) => name.clone(),
        };
        if !path_glob_match(glob, &rel) {
            continue;
        }
        let md = ent.metadata().ok();
        let mut e = Entry::new(
            &name,
            path.to_string_lossy(),
            false,
            md.as_ref().map_or(0, |m| m.len() as i64),
        );
        if let Some(md) = &md {
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
        }
        // The folder the match sits in, so rows far below `base` stay readable.
        if let Some((parent, _)) = rel.rsplit_once('/') {
            e.trailing_text = parent.to_string();
        }
        out.push(e);
        if out.len() >= limit {
            return;
        }
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

    #[cfg(test)]
    fn clone_for_test(&self) -> FileList {
        FileList { rows: self.rows.clone(), selection: self.selection, marked: self.marked.clone(), anchor: self.anchor }
    }

    /// Cmd+Opt+A (Finder's Deselect All): drop the multi-selection; the
    /// keyboard cursor row stays (the list always has one).
    pub fn deselect_all(&mut self) {
        self.marked.clear();
        self.anchor = self.selection;
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

/// Finder's "Rename N Items…": replace text, add text before / after, or
/// number the items from a base name. Extensions are kept.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BatchRename {
    pub find: String,
    pub replace: String,
    pub prefix: String,
    pub suffix: String,
    /// Non-empty: name the items "<base> 1", "<base> 2", … (in the given order).
    pub base: String,
}

fn split_name(name: &str, is_dir: bool) -> (String, String) {
    if is_dir {
        return (name.to_string(), String::new());
    }
    match name.rfind('.') {
        Some(i) if i > 0 => (name[..i].to_string(), name[i..].to_string()),
        _ => (name.to_string(), String::new()),
    }
}

/// The `(from, to)` moves a batch rename makes (unchanged names skipped), or
/// why it is refused (an empty / slashed name, two items landing on one
/// name, or a name that is already taken by something not being renamed).
pub fn batch_rename_plan(
    paths: &[String],
    o: &BatchRename,
    is_dir: &dyn Fn(&str) -> bool,
    exists: &dyn Fn(&str) -> bool,
) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    let mut taken: std::collections::HashSet<String> = std::collections::HashSet::new();
    let sources: std::collections::HashSet<&String> = paths.iter().collect();
    for (n, p) in paths.iter().enumerate() {
        let path = Path::new(p);
        let name = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let dir = path.parent().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
        let (mut stem, ext) = split_name(&name, is_dir(p));
        if !o.base.trim().is_empty() {
            stem = format!("{} {}", o.base.trim(), n + 1);
        } else if !o.find.is_empty() {
            stem = stem.replace(&o.find, &o.replace);
        }
        stem = format!("{}{}{}", o.prefix, stem, o.suffix);
        let new = format!("{stem}{ext}");
        if stem.trim().is_empty() {
            return Err(format!("\u{201c}{name}\u{201d} would get an empty name"));
        }
        if new.contains('/') || new.contains(':') {
            return Err(format!("names can't contain / or : (\u{201c}{new}\u{201d})"));
        }
        let to = format!("{dir}/{new}");
        if !taken.insert(to.clone()) {
            return Err(format!("two items would both be named \u{201c}{new}\u{201d}"));
        }
        if to == *p {
            continue;
        }
        if exists(&to) && !sources.contains(&to) {
            return Err(format!("\u{201c}{new}\u{201d} already exists"));
        }
        out.push((p.clone(), to));
    }
    // A rename onto another source's current name only works if that source
    // moves first; refuse cycles/chains rather than order them.
    for (_, to) in &out {
        if out.iter().any(|(from, _)| from == to) {
            let n = Path::new(to).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            return Err(format!("\u{201c}{n}\u{201d} is one of the items being renamed — rename in two steps"));
        }
    }
    Ok(out)
}

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
    // Our own rename is not reported by the Recent/Arrived FSEvents stream
    // (created with `IgnoreSelf`), so notify the engine directly; otherwise it
    // keeps the old path and `recent_entries` drops the row (files.rs:1283).
    with_recent(|r| r.own_change(Some(old_path), &dst));
    Ok(Some(dst))
}

/// Finder's "Compress": the archive path for `paths` — `<name>.zip` for one
/// item, `Archive.zip` for several — beside the first item, with ` 2`, ` 3`…
/// added while the name is taken. `None` for an empty selection.
pub fn compress_target(paths: &[String], exists: &dyn Fn(&str) -> bool) -> Option<String> {
    let first = Path::new(paths.first()?);
    let dir = first.parent()?.to_string_lossy().into_owned();
    let base = if paths.len() == 1 {
        first.file_name()?.to_string_lossy().into_owned()
    } else {
        "Archive".to_string()
    };
    let mut n = 1;
    loop {
        let name = if n == 1 { format!("{base}.zip") } else { format!("{base} {n}.zip") };
        let p = format!("{}/{name}", dir.trim_end_matches('/'));
        if !exists(&p) {
            return Some(p);
        }
        n += 1;
    }
}

/// Zip `paths` into [`compress_target`] like Finder: on macOS one item
/// through `ditto -c -k --sequesterRsrc --keepParent`, several through
/// `zip -r -y` run in their folder (relative names, symlinks kept). `ditto`
/// does not exist off macOS, so single-item zips take the same relative-
/// name `zip` route there. Blocking — call it off the main thread.
/// `Ok(archive path)` or `Err(message)`.
pub fn compress_paths(paths: &[String]) -> Result<String, String> {
    let dst = compress_target(paths, &|p| Path::new(p).exists()).ok_or("nothing selected")?;
    let out = if paths.len() == 1 && cfg!(target_os = "macos") {
        std::process::Command::new("/usr/bin/ditto")
            .args(["-c", "-k", "--sequesterRsrc", "--keepParent", &paths[0], &dst])
            .output()
    } else {
        let dir = Path::new(&paths[0]).parent().map(|d| d.to_path_buf()).unwrap_or_default();
        let names: Vec<String> = paths
            .iter()
            .map(|p| match Path::new(p).strip_prefix(&dir) {
                Ok(rel) => rel.to_string_lossy().into_owned(),
                Err(_) => p.clone(),
            })
            .collect();
        std::process::Command::new("/usr/bin/zip")
            .current_dir(&dir)
            .args(["-q", "-r", "-y", &dst])
            .args(&names)
            .output()
    }
    .map_err(|e| format!("couldn't compress: {e}"))?;
    if !out.status.success() {
        let _ = std::fs::remove_file(&dst);
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(format!("couldn't compress: {}", if err.is_empty() { "the zip tool failed".into() } else { err }));
    }
    Ok(dst)
}

/// Cmd+D's toast for the copies it made ("" when it made none).
pub fn duplicate_toast(copies: &[String]) -> String {
    match copies {
        [] => String::new(),
        [one] => format!(
            "Duplicated as {} \u{b7} \u{2318}Z undoes",
            Path::new(one).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
        ),
        many => format!("Duplicated {} items \u{b7} \u{2318}Z undoes", many.len()),
    }
}

// ---------------------------------------------------------- shared context menu

/// One row action of [`file_context_menu`]. Surfaces wire these to their own
/// backends — an inline editor, a prompt sheet, the Files browser's ops — so
/// a surface without a `DaemonUi` (a standalone popup) passes its own.
pub type FileMenuAction = Box<dyn FnOnce()>;

/// The injectable actions behind [`file_context_menu`]; `None` drops that
/// row (a surface without the backend stays quiet, and the empty blocks —
/// with their separators — collapse).
#[derive(Default)]
pub struct FileMenuActions {
    pub open: Option<FileMenuAction>,
    pub reveal: Option<FileMenuAction>,
    pub rename: Option<FileMenuAction>,
    pub compress: Option<FileMenuAction>,
    pub trash: Option<FileMenuAction>,
    pub copy_file: Option<FileMenuAction>,
    pub copy_path: Option<FileMenuAction>,
}

/// THE rule-2 file context menu — one builder for the Files browser row menu,
/// the terminal's selected-path menu and the "Find Recent Files" popup, so a
/// right-click on a file reads the same everywhere. Canonical order:
/// `Open / Reveal in Files / ── / Rename… / Compress / Move to Trash / ── /
/// Copy File / Copy Absolute Path / Copy Contents`. "Copy
/// Contents" (the file's text, like `cat | pbcopy`) rides on Copy Absolute
/// Path and is skipped for a directory. Surfaces splice their
/// extra rows (Quick Look, the Copy/Cut/Duplicate block) around the
/// core; richer surface variants of a core row keep their own label and
/// action and pass `None` here.
pub fn file_context_menu(path: &str, a: FileMenuActions) -> Vec<crate::ui::menu_kit::MenuEntry> {
    use crate::ui::menu_kit::MenuEntry;
    let mut blocks: Vec<Vec<MenuEntry>> = vec![Vec::new(), Vec::new(), Vec::new()];
    if let Some(f) = a.open {
        blocks[0].push(MenuEntry::item("Open", f));
    }
    if let Some(f) = a.reveal {
        blocks[0].push(MenuEntry::item("Reveal in Files", f));
    }
    if let Some(f) = a.rename {
        blocks[1].push(MenuEntry::item("Rename\u{2026}", f));
    }
    if let Some(f) = a.compress {
        blocks[1].push(MenuEntry::item("Compress", f));
    }
    if let Some(f) = a.trash {
        blocks[1].push(MenuEntry::item("Move to Trash", f));
    }
    if let Some(f) = a.copy_file {
        blocks[2].push(MenuEntry::item("Copy File", f));
    }
    if let Some(f) = a.copy_path {
        blocks[2].push(MenuEntry::item("Copy Absolute Path", f));
        blocks[2].extend(copy_contents_entry(path));
    }
    let mut e: Vec<MenuEntry> = Vec::new();
    for block in blocks {
        if block.is_empty() {
            continue;
        }
        if !e.is_empty() {
            e.push(MenuEntry::Separator);
        }
        e.extend(block);
    }
    e
}

/// Copy Contents refuses files larger than this: the read runs on the main
/// thread, and the pasteboard silently drops huge strings (a 286 MB log
/// toasted "Copied" and left the clipboard empty).
pub const COPY_CONTENTS_MAX_BYTES: u64 = 32 * 1024 * 1024;

/// The file's text as UTF-8, or why it can't be copied as text.
pub fn file_text_contents(path: &str) -> Result<String, String> {
    let len = std::fs::metadata(path).map_err(|e| format!("Can't read {path}: {e}"))?.len();
    if len > COPY_CONTENTS_MAX_BYTES {
        return Err(format!("Too large to copy as text ({})", human_size(len as i64)));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("Can't read {path}: {e}"))?;
    String::from_utf8(bytes).map_err(|_| "Not a text file".to_string())
}

/// "Copy Contents": the file's text on the clipboard (`cat file | pbcopy`).
/// `None` for a directory or a missing path, so menus simply skip the row.
pub fn copy_contents_entry(path: &str) -> Option<crate::ui::menu_kit::MenuEntry> {
    if !Path::new(path).is_file() {
        return None;
    }
    let p = path.to_string();
    Some(crate::ui::menu_kit::MenuEntry::item("Copy Contents", move || {
        match file_text_contents(&p) {
            Ok(text) => {
                let home = std::env::var("HOME").unwrap_or_default();
                let name = crate::app::pathfmt::tilde_path(&p, &home);
                if crate::platform::set_text(&text) {
                    crate::ui::toast::show(&format!("Copied contents of {name}"), "doc.on.clipboard");
                } else {
                    crate::ui::toast::show(&format!("Couldn't copy the contents of {name}"), "exclamationmark.triangle.fill");
                }
            }
            Err(e) => crate::ui::toast::show(&e, "exclamationmark.triangle.fill"),
        }
    }))
}

// ------------------------------------------------------------------ appkit layout

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

// ---------------------------------------------------------------- quick look

/// The Quick Look preview-panel bridge shared by the file browser and the
/// paths shelf — the Rust port of Swift's `QLPreviewPanelDataSource` /
/// `QLPreviewPanelDelegate` pair (`PathsWindow`/`PopupFileBrowser`
/// `toggleQuickLook()`).
///
/// `paths` mirrors Swift's `quickLookPaths`: the items last handed to the
/// shared panel. Every panel access goes through a [`MainThreadMarker`], so the
/// panel is never created or mutated off the main thread, and an empty
/// selection returns early (Swift's `guard !paths.isEmpty else { return }`) —
/// toggling with nothing selected is safe.
#[derive(Clone)]
pub struct QuickLook {
    /// Swift `quickLookPaths` (empty when the panel is down).
    pub paths: Vec<String>,
    /// Whether [`Self::toggle`] last left the panel up.
    up: bool,
    #[cfg(target_os = "macos")]
    source: Option<Retained<QuickLookSource>>,
}

impl Default for QuickLook {
    fn default() -> Self {
        QuickLook {
            paths: Vec::new(),
            up: false,
            #[cfg(target_os = "macos")]
            source: None,
        }
    }
}

impl QuickLook {
    pub fn new() -> Self {
        Self::default()
    }

    /// The `quickLook` state field: the paths while the panel is up, else `[]`
    /// (`quickLookUp ? quickLookPaths : []`).
    pub fn state(&self) -> Vec<String> {
        #[cfg(target_os = "macos")]
        if let Some(mtm) = objc2::MainThreadMarker::new() {
            if !self.is_up(mtm) {
                return Vec::new();
            }
        }
        if self.up {
            self.paths.clone()
        } else {
            Vec::new()
        }
    }

    /// Swift `quickLookUp`: the shared panel exists, is visible, and we are its
    /// data source. `false` off macOS.
    #[cfg(target_os = "macos")]
    pub fn is_up(&self, mtm: objc2::MainThreadMarker) -> bool {
        if !unsafe { QLPreviewPanel::sharedPreviewPanelExists(mtm) } {
            return false;
        }
        let Some(panel) = (unsafe { QLPreviewPanel::sharedPreviewPanel(mtm) }) else {
            return false;
        };
        if !panel.isVisible() {
            return false;
        }
        let Some(source) = &self.source else { return false };
        let Some(data_source) = (unsafe { panel.dataSource() }) else {
            return false;
        };
        let ds: &AnyObject = AsRef::<AnyObject>::as_ref(&*data_source);
        std::ptr::eq(ds, any_object(&**source))
    }

    /// Swift `toggleQuickLook()`: order the panel out if it is ours and up,
    /// otherwise open it over `selected` (no-op when `selected` is empty).
    #[cfg(target_os = "macos")]
    pub fn toggle(&mut self, mtm: objc2::MainThreadMarker, selected: &[String]) {
        let Some(panel) = (unsafe { QLPreviewPanel::sharedPreviewPanel(mtm) }) else {
            return;
        };
        if self.is_up(mtm) {
            panel.orderOut(None);
            self.clear();
            return;
        }
        if selected.is_empty() {
            self.clear();
            return;
        }
        self.paths = selected.to_vec();
        let source = match &self.source {
            Some(s) => s.clone(),
            None => {
                let s = QuickLookSource::new(mtm);
                self.source = Some(s.clone());
                s
            }
        };
        source.set_paths(&self.paths);
        let delegate = any_object(&*source);
        unsafe {
            panel.setDataSource(Some(ProtocolObject::from_ref(&*source)));
            panel.setDelegate(Some(delegate));
            panel.reloadData();
            panel.setCurrentPreviewItemIndex(0);
            panel.makeKeyAndOrderFront(None);
        }
        // The silver preview border, like every other preview window.
        if let Some(content) = panel.contentView() {
            crate::ui::preview_ring::attach(&content, 0.0, 16.0);
        }
        self.up = true;
    }

    /// Swift `refreshQuickLook()`: no-op unless the panel is up.
    #[cfg(target_os = "macos")]
    pub fn refresh(&mut self, mtm: objc2::MainThreadMarker, selected: &[String]) {
        if !self.is_up(mtm) {
            return;
        }
        self.paths = selected.to_vec();
        if let Some(source) = &self.source {
            source.set_paths(&self.paths);
        }
        if let Some(panel) = unsafe { QLPreviewPanel::sharedPreviewPanel(mtm) } {
            unsafe { panel.reloadData() };
        }
    }

    /// Order the shared panel out (if we own it) and clear the preview paths.
    #[cfg(target_os = "macos")]
    pub fn dismiss(&mut self, mtm: objc2::MainThreadMarker) {
        if unsafe { QLPreviewPanel::sharedPreviewPanelExists(mtm) } {
            if let Some(panel) = unsafe { QLPreviewPanel::sharedPreviewPanel(mtm) } {
                panel.orderOut(None);
            }
        }
        self.clear();
    }

    #[cfg(target_os = "macos")]
    fn clear(&mut self) {
        self.paths.clear();
        self.up = false;
    }
}

/// Order out the shared Quick Look panel if it is on screen (whoever opened
/// it). Returns whether it was up. The host calls this on Esc / Space: a panel
/// opened from a menu may never become key, so its own `handleEvent` never
/// sees those keys.
#[cfg(target_os = "macos")]
pub fn close_shared_quick_look(mtm: objc2::MainThreadMarker) -> bool {
    if !unsafe { QLPreviewPanel::sharedPreviewPanelExists(mtm) } {
        return false;
    }
    match unsafe { QLPreviewPanel::sharedPreviewPanel(mtm) } {
        Some(panel) if panel.isVisible() => {
            panel.orderOut(None);
            true
        }
        _ => false,
    }
}

/// Cmd+Opt+Y: put the (just opened) Quick Look panel into full screen.
#[cfg(target_os = "macos")]
pub fn quick_look_full_screen(mtm: objc2::MainThreadMarker) {
    if !unsafe { QLPreviewPanel::sharedPreviewPanelExists(mtm) } {
        return;
    }
    if let Some(panel) = unsafe { QLPreviewPanel::sharedPreviewPanel(mtm) } {
        if panel.isVisible() {
            let nil: Option<&AnyObject> = None;
            let _: objc2::runtime::Bool = unsafe { msg_send![&*panel, enterFullScreenMode: nil, withOptions: nil] };
        }
    }
}

/// Cast any Objective-C object to `&AnyObject` (the identity cast used for the
/// panel's unretained `delegate`).
#[cfg(target_os = "macos")]
fn any_object<T: objc2::Message>(obj: &T) -> &AnyObject {
    unsafe { &*(obj as *const T as *const AnyObject) }
}

/// The panel's data source *and* delegate: it hands `NSURL` preview items to
/// `QLPreviewPanel` and closes the panel on Space/Esc
/// (`previewPanel(_:handle:)`). One instance is kept alive by [`QuickLook`] for
/// as long as the shared panel might call back into it.
#[cfg(target_os = "macos")]
pub struct QuickLookSourceIvars {
    paths: std::cell::RefCell<Vec<String>>,
}

#[cfg(target_os = "macos")]
define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "WSQuickLookSource"]
    #[ivars = QuickLookSourceIvars]
    pub struct QuickLookSource;

    impl QuickLookSource {
        /// `numberOfPreviewItems(in:)`.
        #[unsafe(method(numberOfPreviewItemsInPreviewPanel:))]
        fn number_of_items(&self, _panel: Option<&QLPreviewPanel>) -> isize {
            self.ivars().paths.borrow().len() as isize
        }

        /// `previewPanel(_:previewItemAt:)` — an `NSURL` is itself a
        /// `QLPreviewItem`.
        #[unsafe(method_id(previewPanel:previewItemAtIndex:))]
        fn item_at(
            &self,
            _panel: Option<&QLPreviewPanel>,
            index: isize,
        ) -> Option<Retained<ProtocolObject<dyn QLPreviewItem>>> {
            let paths = self.ivars().paths.borrow();
            let path = if index >= 0 { paths.get(index as usize) } else { None };
            path.map(|path| {
                let url = NSURL::fileURLWithPath(&NSString::from_str(path));
                ProtocolObject::from_retained(url)
            })
        }

        /// `previewPanel(_:handle:)`: Space/Esc close; the panel keeps its own
        /// arrow-key navigation.
        #[unsafe(method(previewPanel:handleEvent:))]
        fn handle_event(&self, panel: Option<&QLPreviewPanel>, event: Option<&NSEvent>) -> bool {
            let close = event
                .map(|e| e.r#type() == NSEventType::KeyDown && matches!(e.keyCode(), 49 | 53))
                .unwrap_or(false);
            if close {
                if let Some(panel) = panel {
                    panel.orderOut(None);
                }
            }
            close
        }
    }

    unsafe impl NSObjectProtocol for QuickLookSource {}
    unsafe impl NSWindowDelegate for QuickLookSource {}
    unsafe impl QLPreviewPanelDataSource for QuickLookSource {}
    unsafe impl QLPreviewPanelDelegate for QuickLookSource {}
);

#[cfg(target_os = "macos")]
impl QuickLookSource {
    fn new(mtm: objc2::MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(QuickLookSourceIvars {
            paths: std::cell::RefCell::new(Vec::new()),
        });
        unsafe { msg_send![super(this), init] }
    }

    fn set_paths(&self, paths: &[String]) {
        *self.ivars().paths.borrow_mut() = paths.to_vec();
    }
}

// ---------------------------------------------------------------- file browser

#[derive(Clone, Debug, PartialEq)]
struct Place {
    virtual_index: Option<usize>,
    dir: String,
}

/// One Files tab (Cmd+T): everything that differs per tab. The live tab's
/// state is in the browser's own fields; the others wait here.
#[derive(Clone, Debug, PartialEq)]
struct TabState {
    place: Place,
    query: String,
    selection: usize,
    expanded: BTreeSet<String>,
    back: Vec<Place>,
    forward: Vec<Place>,
}

/// The `PopupFileBrowser` browsing model (drawing / Quick Look / preview are
/// out of scope for this cut).
#[derive(Clone)]
pub struct FileBrowser {
    pub cwd: String,
    pub query: String,
    pub show_hidden: bool,
    /// Folders expanded in place (Opt+→), by path; cleared on navigation.
    pub expanded: BTreeSet<String>,
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
    /// Every tab (Cmd+T); `tabs[tab]` is stale while live — the browser's
    /// fields hold that one.
    tabs: Vec<TabState>,
    pub tab: usize,
    /// The Quick Look preview-panel bridge (Swift `QLPreviewPanelDataSource`).
    pub quick_look: QuickLook,
    /// `InlineRename` target: (old path, in-progress text).
    pub rename: Option<(String, String)>,
    pub rename_editor: Option<String>,
    /// The built AppKit view tree (`build_macos`).
    #[cfg(target_os = "macos")]
    pub root: Option<Retained<NSView>>,
    /// The live view (`FilesView`) the model pushes snapshots into.
    #[cfg(target_os = "macos")]
    pub view: Option<Retained<crate::views::files_view::FilesView>>,
    /// `[files] favorites` (static) + the pinned list (`files-favorites.json`).
    pub static_favorites: Vec<String>,
    pub pinned_favorites: Vec<String>,
    pub favorites_file: String,
    /// `[files] recent-scope` != home (`recentEverywhere`).
    pub recent_everywhere: bool,
    /// A transient status message (`setStatus`) shown until the next change.
    pub status_note: Option<String>,
}

impl FileBrowser {
    pub fn new(start_dir: &str) -> Self {
        FileBrowser {
            cwd: standardize(start_dir),
            query: String::new(),
            show_hidden: false,
            expanded: BTreeSet::new(),
            mode: QueryMode::All,
            all: Vec::new(),
            list: FileList::default(),
            virtual_index: None,
            terminal_words: vec!["term".into(), "terminal".into(), "shell".into()],
            sort_key: SortKey::Name,
            sort_descending: false,
            back: Vec::new(),
            forward: Vec::new(),
            tabs: Vec::new(),
            tab: 0,
            travelling: false,
            quick_look: QuickLook::new(),
            rename: None,
            rename_editor: None,
            #[cfg(target_os = "macos")]
            root: None,
            #[cfg(target_os = "macos")]
            view: None,
            static_favorites: Vec::new(),
            pinned_favorites: Vec::new(),
            favorites_file: favorites_file_path(),
            recent_everywhere: true,
            status_note: None,
        }
    }

    /// Apply the `[files]` section (Swift `applyBrowserSettings` +
    /// `fileBrowserFavorites` + `loadFavorites` + the sort settings).
    pub fn configure(&mut self, section: &std::collections::HashMap<String, String>) {
        if let Some(s) = section.get("sort") {
            self.sort_key = SortKey::from_raw(s);
        }
        self.sort_descending = match section.get("sort-order").map(|s| s.to_lowercase()) {
            Some(o) if o == "desc" => true,
            Some(o) if o == "asc" => false,
            _ => self.sort_key.natural_descending(),
        };
        self.static_favorites = section
            .get("favorites")
            .map(|f| f.split(',').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect())
            .unwrap_or_default();
        self.recent_everywhere = section
            .get("recent-scope")
            .map_or(true, |s| s.to_lowercase() != "home");
        if let Ok(text) = std::fs::read_to_string(&self.favorites_file) {
            if let Ok(v) = serde_json::from_str::<Vec<String>>(&text) {
                self.pinned_favorites = v;
            }
        }
    }

    /// `mergedFavorites()` — existing folders, static first, de-duplicated.
    pub fn merged_favorites(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for raw in self.static_favorites.iter().chain(self.pinned_favorites.iter()) {
            let p = standardize(&expand_tilde(raw));
            if Path::new(&p).is_dir() && !out.contains(&p) {
                out.push(p);
            }
        }
        out
    }

    fn save_favorites(&self) {
        if let Some(dir) = Path::new(&self.favorites_file).parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string(&self.pinned_favorites) {
            let _ = std::fs::write(&self.favorites_file, text);
        }
    }

    /// `toggleStar()` — pin / unpin the current folder.
    pub fn toggle_pin(&mut self) {
        if self.in_recent() {
            return;
        }
        let cwd = self.cwd.clone();
        self.toggle_pin_path(&cwd);
    }

    /// Finder's Add to Sidebar (Cmd+Ctrl+T): pin the selected folder, else
    /// the current one.
    pub fn pin_selection_or_cwd(&mut self) {
        let sel = self.list.rows.get(self.list.selection).filter(|e| e.is_dir && !e.is_parent()).map(|e| e.path.clone());
        match sel {
            Some(p) => self.toggle_pin_path(&p),
            None => self.toggle_pin(),
        }
    }

    fn toggle_pin_path(&mut self, cwd: &str) {
        let cwd = cwd.to_string();
        if self.merged_favorites().contains(&cwd) && self.pinned_favorites.contains(&cwd) {
            self.pinned_favorites.retain(|p| p != &cwd);
        } else if !self.merged_favorites().contains(&cwd) {
            self.pinned_favorites.push(cwd.clone());
        }
        self.save_favorites();
        let on = self.merged_favorites().contains(&cwd);
        self.status_note = Some(if on {
            format!("★ pinned {}", display_path(&cwd))
        } else {
            format!("unpinned {}", display_path(&cwd))
        });
    }

    /// Persist the pinned list after an external edit (sidebar "Unpin").
    pub fn toggle_pin_save(&mut self) {
        self.save_favorites();
    }

    /// `pickSortKey` / `toggleSortOrder` → `applySort()`.
    pub fn set_sort(&mut self, key: SortKey) {
        if key != self.sort_key {
            self.sort_descending = key.natural_descending();
        }
        self.sort_key = key;
        self.resort();
    }

    pub fn toggle_order(&mut self) {
        self.sort_descending = !self.sort_descending;
        self.resort();
    }

    fn resort(&mut self) {
        let key = self.sort_key;
        let mut all = std::mem::take(&mut self.all);
        if !self.in_recent() {
            for e in all.iter_mut() {
                decorate(e, key);
            }
            all = sort_entries(&all, key, self.sort_descending);
        }
        self.all = all;
        self.refilter();
    }

    /// `recentEntries()` — the virtual list rows with Swift's trailing text
    /// ("~/dir · ↓ source · 12 KB · 5m ago").
    pub fn recent_entries(&self, arrived: bool) -> Vec<Entry> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        recent_snapshot(arrived)
            .into_iter()
            .filter_map(|it| {
                let md = std::fs::metadata(&it.path).ok()?;
                let mut e = Entry::new(basename(&it.path), it.path.clone(), md.is_dir(), md.len() as i64);
                let mut dir = display_path(
                    &Path::new(&it.path).parent().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default(),
                );
                if let Some(rest) = dir.strip_prefix("/private/tmp") {
                    dir = format!("/tmp{rest}");
                }
                let mut parts = vec![dir];
                if let Some(src) = &it.source {
                    parts.push(format!("↓ {src}"));
                }
                if !e.is_dir {
                    parts.push(human_size(e.size));
                }
                parts.push(ago(now - it.at));
                e.trailing_text = parts.join(" · ");
                Some(e)
            })
            .collect()
    }

    /// `updateStatus()`.
    pub fn status_text(&self) -> String {
        if let Some(n) = &self.status_note {
            return n.clone();
        }
        let items = self.list.rows.iter().filter(|e| !e.is_parent()).count();
        let marked = self.list.marked.len();
        let count = if marked > 1 {
            // Finder's status bar: "3 of 12 selected".
            format!("{marked} of {items} selected")
        } else {
            format!("{items} item{}", if items == 1 { "" } else { "s" })
        };
        match &self.mode {
            QueryMode::All if self.in_recent() => {
                let status = match self.virtual_index {
                    Some(1) => "downloads, AirDrop, Messages / Mail saves — wherever they were saved".to_string(),
                    _ => format!(
                        "newest first · created or changed anywhere {}",
                        if self.recent_everywhere { "on this Mac (outside system folders)" } else { "in ~ or /tmp" }
                    ),
                };
                format!("{count} · {status}")
            }
            QueryMode::All => format!("{count} · sorted by {}", self.sort_key.label().to_lowercase()),
            QueryMode::Terminal(dir) => format!("↵ open a terminal in {}", display_path(dir)),
            QueryMode::Local(_) => {
                if items == 0 {
                    "no matches — try a path (~/…) or **/name to search subfolders".to_string()
                } else {
                    format!("{count} · ↵ open")
                }
            }
            QueryMode::Dir { dir, pat } => {
                if !Path::new(dir).is_dir() {
                    return format!("no such folder: {}", display_path(dir));
                }
                let exact = Path::new(dir).join(pat);
                if !pat.is_empty() && exact.exists() {
                    if exact.is_dir() {
                        format!("↵ cd {} · ⇥ list it", display_path(&exact.to_string_lossy()))
                    } else {
                        format!("↵ open {}", display_path(&exact.to_string_lossy()))
                    }
                } else {
                    format!("{count} in {} · ⇥ complete · ↵ open", display_path(dir))
                }
            }
            QueryMode::Recursive { base, .. } => format!(
                "{items} match{} below {}",
                if items == 1 { "" } else { "es" },
                display_path(base)
            ),
        }
    }

    /// `previewSelection()` — synchronous (text capped at 256 KB).
    pub fn preview(&self) -> (String, crate::views::files_view::Preview) {
        use crate::views::files_view::Preview;
        let Some(e) = self.list.rows.get(self.list.selection) else {
            return (String::new(), Preview::Hint(String::new()));
        };
        let key = format!("{}|{}|{}", e.path, e.modified, e.size);
        if e.is_dir {
            let rows: Vec<Entry> = self
                .list_dir(&e.path, false)
                .into_iter()
                .filter(|r| !r.is_parent())
                .collect();
            return (key, Preview::Folder(rows));
        }
        let ext = Path::new(&e.path)
            .extension()
            .map(|x| x.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        const IMAGES: [&str; 9] = ["png", "jpg", "jpeg", "gif", "heic", "webp", "tif", "tiff", "pdf"];
        if IMAGES.contains(&ext.as_str()) {
            return (key, Preview::Image(e.path.clone()));
        }
        use std::io::Read;
        let mut buf = Vec::new();
        if let Ok(f) = std::fs::File::open(&e.path) {
            let _ = f.take(262_144).read_to_end(&mut buf);
        }
        if !buf.is_empty() {
            let text = match String::from_utf8(buf) {
                Ok(t) => Some(t),
                Err(err) => {
                    // A cut multi-byte char at the cap is still text.
                    let valid = err.utf8_error().valid_up_to();
                    let bytes = err.into_bytes();
                    (valid + 4 >= bytes.len() && valid > 0)
                        .then(|| String::from_utf8_lossy(&bytes[..valid]).into_owned())
                }
            };
            if let Some(t) = text {
                if !t.contains('\0') {
                    return (key, Preview::Text(t));
                }
            }
        }
        (key, Preview::Hint("no preview".into()))
    }

    /// The sidebar model (`syncSidebar`): Pinned favorites + the Places
    /// rows (Recent, Arrived, Home, Desktop, Documents, Downloads).
    pub fn sidebar_model(&self) -> crate::ui::sidebar::SidebarModel {
        use crate::ui::sidebar::{SidebarModel, SidebarPin, SidebarRow};
        let home = std::env::var("HOME").unwrap_or_default();
        let mut rows: Vec<SidebarRow> = default_virtual_lists()
            .into_iter()
            .map(|(t, sym)| SidebarRow::new(&t, &sym))
            .collect();
        let places = places(&home);
        rows.extend(places.iter().map(|p| SidebarRow::new(&p.title, &p.symbol)));
        let selected = match self.virtual_index {
            Some(v) => Some(v),
            None => places.iter().position(|p| p.path == self.cwd).map(|k| 2 + k),
        };
        let pinned = self
            .merged_favorites()
            .into_iter()
            .map(|p| SidebarPin {
                label: display_path(&p),
                tip: format!("Open {} in the file browser", display_path(&p)),
                key: p,
                icon: "folder".into(),
                section: "Pinned".into(),
                meta: String::new(),
                ..Default::default()
            })
            .collect();
        SidebarModel {
            section_title: "Places".into(),
            rows,
            selected,
            pinned,
            pinned_selected: (!self.in_recent()).then(|| self.cwd.clone()),
            ..Default::default()
        }
    }

    /// The full view snapshot.
    pub fn snapshot(&self) -> crate::views::files_view::FilesSnapshot {
        let (preview_key, preview) = self.preview();
        let in_recent = self.in_recent();
        crate::views::files_view::FilesSnapshot {
            rows: self.list.rows.clone(),
            selection: self.list.selection,
            marked: self.list.marked.clone(),
            sidebar: self.sidebar_model(),
            pinned_on: !in_recent && self.merged_favorites().contains(&self.cwd),
            can_pin: !in_recent,
            sort_short: self.sort_key.short().to_string(),
            descending: self.sort_descending,
            status: self.status_text(),
            placeholder: match self.virtual_index {
                Some(v) => format!(
                    "{} · type to filter",
                    default_virtual_lists().get(v).map(|x| x.0.clone()).unwrap_or_default()
                ),
                None => "filter…".into(),
            },
            query: self.query.clone(),
            field_text: if in_recent { String::new() } else { self.cwd.clone() },
            preview,
            preview_key,
            tabs: self.tab_titles(),
            tab: self.tab,
        }
    }

    /// Push the current state into the live view (no-op before `build`).
    pub fn sync_view(&self) {
        #[cfg(target_os = "macos")]
        if let Some(v) = &self.view {
            v.apply(self.snapshot());
        }
    }

    /// The pure event contract (X7): apply a click / keystroke a frontend
    /// reports to the model. It mirrors the model-side effects of the AppKit
    /// dispatcher (`host::files_event`, which is left untouched): selection /
    /// marked, query, history and sidebar navigation, sort order and pinning.
    /// Platform actions (opening a file with the OS, Quick Look, popups, focus,
    /// the Recent click-rename window) are the frontend's job, so they are not
    /// modelled here — a directory descent is, because [`Self::cd`] is model.
    pub fn handle(&mut self, ev: crate::views::files_view::FilesEvent) {
        use crate::ui::sidebar::SidebarEvent;
        use crate::views::files_view::{FilesEvent as E, Preview};
        match ev {
            E::Click { row, cmd, shift, double } => {
                self.status_note = None;
                if shift {
                    self.list.extend_selection(row);
                } else if cmd {
                    self.list.command_click(row);
                } else {
                    self.list.marked.clear();
                    self.list.selection = row;
                }
                // The model half of `files_accept_selected` for a plain
                // double-click: a directory descends; a file is the OS's job.
                if double && !cmd && !shift {
                    let rows = self.visible_rows().to_vec();
                    if let OpenOutcome::Cd(dir) = open_index(&rows, self.selection()) {
                        self.cd(&dir);
                    }
                }
            }
            E::RowMenu { row, .. } => {
                if !self.list.marked.contains(&row) {
                    self.list.marked.clear();
                    self.list.selection = row;
                }
            }
            E::BlankMenu { .. } => self.list.marked.clear(),
            // The transfer itself is the frontend's (`FileOps`).
            E::Drop { .. } => {}
            E::PreviewOpen(i) => {
                let target = match self.preview().1 {
                    Preview::Folder(rows) => rows.get(i).cloned(),
                    _ => None,
                };
                if let Some(e) = target {
                    if e.is_dir {
                        self.cd(&e.path);
                    }
                }
            }
            E::Parent => self.cd_parent(),
            E::TogglePin => self.toggle_pin(),
            E::ToggleOrder => self.toggle_order(),
            // The sort popup (and its `set_sort` callback) is the frontend's.
            E::SortMenu { .. } => {}
            E::Query(q) => {
                self.status_note = None;
                self.list.selection = 0;
                self.set_query(&q);
            }
            E::Enter => {
                // The query is a literal path: descend into an existing
                // directory (opening a file is the frontend's job).
                let q = self.query.trim().to_string();
                if (q.contains('/') || q.starts_with('~')) && !has_glob(&q) {
                    let p = resolve_path(&q, &self.cwd);
                    if Path::new(&p).is_dir() {
                        self.cd(&p);
                        return;
                    }
                }
                let rows = self.visible_rows().to_vec();
                if let OpenOutcome::Cd(dir) = open_index(&rows, self.selection()) {
                    self.cd(&dir);
                }
            }
            E::Move(d) => self.list.move_selection(d),
            E::DragEnded => self.reload(),
            E::Tab(i) => {
                self.select_tab(i);
            }
            E::Escape => {
                if !self.query.is_empty() {
                    self.set_query("");
                }
            }
            E::Sidebar(SidebarEvent::Select(i)) => {
                self.status_note = None;
                let home = std::env::var("HOME").unwrap_or_default();
                let places = places(&home);
                if i < 2 {
                    self.show_virtual(i);
                } else if let Some(p) = places.get(i - 2) {
                    self.cd(&p.path);
                }
            }
            E::Sidebar(SidebarEvent::Pinned(path)) => {
                if Path::new(&path).is_dir() {
                    self.cd(&path);
                }
            }
            E::Sidebar(_) => {}
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
        if let Some(v) = self.virtual_index {
            self.all = self.recent_entries(v == 1);
            self.refilter();
            return;
        }
        let dir = self.cwd.clone();
        let entries = self.list_dir(&dir, self.show_hidden);
        self.set_all(entries);
    }

    /// The current folder's modification time (`None` on Recent / Arrived).
    /// It moves when anything adds, removes or renames an entry in it.
    pub fn dir_stamp(&self) -> Option<std::time::SystemTime> {
        if self.virtual_index.is_some() {
            return None;
        }
        std::fs::metadata(&self.cwd).and_then(|m| m.modified()).ok()
    }

    /// Re-list the folder after an outside change, keeping the selected and
    /// marked rows by path (the list used to stay stale until you left it).
    pub fn refresh_from_disk(&mut self) {
        let sel = self.list.rows.get(self.list.selection).map(|e| e.path.clone());
        let marked: Vec<String> =
            self.list.marked.iter().filter_map(|&i| self.list.rows.get(i).map(|e| e.path.clone())).collect();
        self.reload();
        if let Some(i) = sel.and_then(|p| self.list.rows.iter().position(|e| e.path == p)) {
            self.list.selection = i;
        }
        self.list.marked =
            self.list.rows.iter().enumerate().filter(|(_, e)| marked.contains(&e.path)).map(|(i, _)| i).collect();
    }

    /// `refilter()` for All / Local / Terminal / Dir / Recursive (the live
    /// app's search scheduling is out of scope — the walk runs inline).
    pub fn refilter(&mut self) {
        self.mode = parse_query(&self.query, &self.cwd, &self.terminal_words);
        let rows: Vec<Entry> = match &self.mode {
            QueryMode::All if !self.in_recent() => self.with_expanded(self.all.clone()),
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
            QueryMode::Recursive { base, glob } => {
                let glob = widen_recursive_glob(glob);
                let hidden = self.show_hidden || glob.starts_with('.') || glob.contains("/.");
                let rows = recursive_search(base, &glob, hidden, RECURSIVE_SEARCH_LIMIT);
                sort_entries(&rows, self.sort_key, self.sort_descending)
            }
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

    /// Swift `select(path:)`: move the selection to `path` when it is listed.
    pub fn select_path(&mut self, path: &str) -> bool {
        match self.list.rows.iter().position(|e| e.path == path) {
            Some(i) => {
                self.list.marked.clear();
                self.list.selection = i;
                true
            }
            None => false,
        }
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

    fn tab_state(&self) -> TabState {
        TabState {
            place: self.place(),
            query: self.query.clone(),
            selection: self.list.selection,
            expanded: self.expanded.clone(),
            back: self.back.clone(),
            forward: self.forward.clone(),
        }
    }

    fn load_tab(&mut self, t: TabState) {
        self.travelling = true;
        match t.place.virtual_index {
            Some(v) => self.show_virtual(v),
            None => self.cd(&t.place.dir),
        }
        self.travelling = false;
        self.back = t.back;
        self.forward = t.forward;
        self.expanded = t.expanded;
        if !t.query.is_empty() {
            self.set_query(&t.query);
        } else {
            self.refilter();
        }
        self.list.selection = t.selection.min(self.list.rows.len().saturating_sub(1));
        self.list.anchor = self.list.selection;
    }

    /// How many tabs (1 when tabs were never used).
    pub fn tab_count(&self) -> usize {
        self.tabs.len().max(1)
    }

    /// Tab titles (folder names), for the tab bar.
    pub fn tab_titles(&self) -> Vec<String> {
        if self.tabs.len() < 2 {
            return Vec::new();
        }
        let title = |p: &Place| match p.virtual_index {
            Some(v) => default_virtual_lists().get(v).map(|x| x.0.clone()).unwrap_or_default(),
            None => Path::new(&p.dir)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.dir.clone()),
        };
        self.tabs
            .iter()
            .enumerate()
            .map(|(i, t)| if i == self.tab { title(&self.place()) } else { title(&t.place) })
            .collect()
    }

    /// Cmd+T: a new tab on the current folder, made current.
    pub fn new_tab(&mut self) {
        let cur = self.tab_state();
        if self.tabs.is_empty() {
            self.tabs.push(cur.clone());
            self.tab = 0;
        } else {
            self.tabs[self.tab] = cur.clone();
        }
        let fresh = TabState { back: Vec::new(), forward: Vec::new(), query: String::new(), ..cur };
        self.tabs.insert(self.tab + 1, fresh);
        self.tab += 1;
        self.back.clear();
        self.forward.clear();
        if !self.query.is_empty() {
            self.set_query("");
        }
    }

    /// Switch to tab `i`.
    pub fn select_tab(&mut self, i: usize) -> bool {
        if self.tabs.len() < 2 || i >= self.tabs.len() || i == self.tab {
            return false;
        }
        self.tabs[self.tab] = self.tab_state();
        self.tab = i;
        let t = self.tabs[i].clone();
        self.load_tab(t);
        true
    }

    /// Cmd+Shift+] / Cmd+Shift+[: the next / previous tab, wrapping.
    pub fn cycle_tab(&mut self, d: i64) -> bool {
        let n = self.tabs.len();
        if n < 2 {
            return false;
        }
        let i = (self.tab as i64 + d).rem_euclid(n as i64) as usize;
        self.select_tab(i)
    }

    /// Cmd+W with several tabs: close this one (false = the last tab, so
    /// Cmd+W keeps hiding the window).
    pub fn close_tab(&mut self) -> bool {
        if self.tabs.len() < 2 {
            return false;
        }
        self.tabs.remove(self.tab);
        if self.tab >= self.tabs.len() {
            self.tab = self.tabs.len() - 1;
        }
        let t = self.tabs[self.tab].clone();
        self.load_tab(t);
        if self.tabs.len() == 1 {
            self.tabs.clear();
            self.tab = 0;
        }
        true
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
        self.expanded.clear();
        self.list.selection = 0;
        self.reload();
    }

    /// The listing with every expanded folder's children inserted under it
    /// (recursively, one level of indent each).
    fn with_expanded(&self, rows: Vec<Entry>) -> Vec<Entry> {
        if self.expanded.is_empty() {
            return rows;
        }
        let mut out = Vec::with_capacity(rows.len());
        self.push_tree(rows, 0, &mut out);
        out
    }

    fn push_tree(&self, rows: Vec<Entry>, depth: u16, out: &mut Vec<Entry>) {
        for mut e in rows {
            e.depth = depth;
            let open = e.is_dir && !e.is_parent() && self.expanded.contains(&e.path) && depth < 12;
            e.expanded = open;
            let path = e.path.clone();
            out.push(e);
            if open {
                let kids: Vec<Entry> =
                    self.list_dir(&path, self.show_hidden).into_iter().filter(|k| !k.is_parent()).collect();
                self.push_tree(kids, depth + 1, out);
            }
        }
    }

    /// Opt+→: show the selected folder's contents under it (Finder list view).
    pub fn expand_selected(&mut self) -> bool {
        let Some(e) = self.list.rows.get(self.list.selection).cloned() else { return false };
        if !e.is_dir || e.is_parent() || self.expanded.contains(&e.path) {
            return false;
        }
        self.expanded.insert(e.path.clone());
        self.relist_keeping(&e.path);
        true
    }

    /// Opt+←: collapse the selected folder; on a child row, collapse its
    /// parent and select it.
    pub fn collapse_selected(&mut self) -> bool {
        let Some(e) = self.list.rows.get(self.list.selection).cloned() else { return false };
        let target = if e.expanded {
            e.path.clone()
        } else if e.depth > 0 {
            Path::new(&e.path).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
        } else {
            return false;
        };
        let before = self.expanded.len();
        self.expanded.retain(|p| p != &target && !p.starts_with(&format!("{target}/")));
        if self.expanded.len() == before {
            return false;
        }
        self.relist_keeping(&target);
        true
    }

    fn relist_keeping(&mut self, path: &str) {
        self.refilter();
        if let Some(i) = self.list.rows.iter().position(|r| r.path == path) {
            self.list.selection = i;
            self.list.anchor = i;
        }
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

    /// Swift `PopupFileBrowser.toggleQuickLook()`.
    #[cfg(target_os = "macos")]
    pub fn toggle_quick_look(&mut self, mtm: objc2::MainThreadMarker) {
        let paths = self.selected_paths();
        self.quick_look.toggle(mtm, &paths);
    }

    /// Swift `PopupFileBrowser.refreshQuickLook()` (called on selection change).
    #[cfg(target_os = "macos")]
    pub fn refresh_quick_look(&mut self, mtm: objc2::MainThreadMarker) {
        let paths = self.selected_paths();
        self.quick_look.refresh(mtm, &paths);
    }

    /// Swift `PopupFileBrowser.beginRename(_:)`: start an inline rename on the
    /// given row (or the selection). `..` is never renameable.
    pub fn begin_rename(&mut self, index: Option<usize>) {
        let i = index.unwrap_or(self.list.selection);
        if i >= self.list.rows.len() || self.list.rows[i].is_parent() {
            return;
        }
        self.commit_rename();
        let path = self.list.rows[i].path.clone();
        self.list.selection = i;
        let name = basename(&path);
        self.rename = Some((path, name.clone()));
        self.rename_editor = Some(name);
    }

    /// Swift `PopupFileBrowser.cancelRename()`.
    pub fn cancel_rename(&mut self) {
        self.rename = None;
        self.rename_editor = None;
    }

    /// Swift `PopupFileBrowser.commitRename()`: move the file and reload, or
    /// return the status message on rejection/failure.
    pub fn commit_rename(&mut self) -> Option<String> {
        let (path, text) = self.rename.clone()?;
        self.rename = None;
        self.rename_editor = None;
        let old = basename(&path);
        let new = text.trim().to_string();
        if new.is_empty() || new == old {
            return None;
        }
        match rename_into(&path, &new) {
            Err(msg) => Some(msg),
            Ok(None) => None,
            Ok(Some(dst)) => {
                self.reload();
                if let Some(i) = self.list.rows.iter().position(|e| e.path == dst) {
                    self.list.selection = i;
                }
                Some(format!("renamed “{old}” to “{new}”"))
            }
        }
    }

    /// Build the AppKit browser: the toolbar, the scrollable rows list and the
    /// preview/info pane. No-op off macOS.
    #[cfg(target_os = "macos")]
    pub fn build(&mut self, mtm: objc2::MainThreadMarker) {
        self.build_macos(mtm);
    }

    #[cfg(target_os = "macos")]
    fn build_macos(&mut self, mtm: objc2::MainThreadMarker) {
        if self.root.is_some() {
            return;
        }
        let colors = crate::ui::theme::section_colors("files");
        let view = crate::views::files_view::FilesView::new(mtm, colors);
        view.setAutoresizingMask(
            objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable
                | objc2_app_kit::NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        view.apply(self.snapshot());
        self.root = Some(Retained::into_super(Retained::into_super(view.clone())));
        self.view = Some(view);
    }

    /// The built AppKit root view, once [`Self::build`] has run.
    #[cfg(target_os = "macos")]
    pub fn content_view(&self) -> Option<Retained<NSView>> {
        self.root.clone()
    }

    pub fn test_state(&self) -> Value {
        #[allow(unused_mut)]
        let mut v = json!({
            "cwd": self.cwd,
            "where": self.where_text(),
            "query": self.query,
            "selection": self.list.selection,
            "marked": self.list.marked.iter().copied().collect::<Vec<_>>(),
            "quickLook": self.quick_look.state(),
            "sortKey": self.sort_key.raw(),
            "tabs": self.tab_titles(),
            "tab": self.tab,
            "pinned": self.pinned_favorites,
            "status": self.status_text(),
            "rows": self.list.rows.iter().map(|e| json!({
                "name": e.name, "path": e.path, "isDir": e.is_dir
            })).collect::<Vec<_>>(),
        });
        // What the filter / path field really shows (the view can lag the model).
        #[cfg(target_os = "macos")]
        if let Some(view) = &self.view {
            v["fieldText"] = json!(view.search_text());
            v["previewShown"] = json!(view.preview_shown());
        }
        v
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

// ------------------------------------------------------------ recent files

#[cfg(target_os = "macos")]
thread_local! {
    /// `RecentFiles.shared` — leaked once (its FSEvents context points at it).
    static RECENT: std::cell::Cell<*mut crate::engines::recent_files::RecentFiles> =
        const { std::cell::Cell::new(std::ptr::null_mut()) };
}

/// `configureRecentFiles()` — start the shared Recent / Arrived engine
/// (main thread, once) from the `[files]` section.
pub fn start_recent(section: &std::collections::HashMap<String, String>) {
    #[cfg(target_os = "macos")]
    RECENT.with(|r| {
        if !r.get().is_null() {
            return;
        }
        let home = std::env::var("HOME").unwrap_or_default();
        let enabled = section.get("recent").map_or(true, |v| !matches!(v.to_lowercase().as_str(), "false" | "no" | "0" | "off"));
        if !enabled {
            return;
        }
        let mut engine = Box::new(crate::engines::recent_files::RecentFiles::new(
            &home,
            &format!("{home}/.cache/kitchen-sink/recent.json"),
        ));
        let days = section.get("recent-days").and_then(|v| v.parse().ok()).unwrap_or(7);
        let limit = section.get("recent-limit").and_then(|v| v.parse().ok()).unwrap_or(200);
        let excludes: Vec<String> = section
            .get("recent-exclude")
            .map(|v| v.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect())
            .unwrap_or_default();
        let everywhere = section.get("recent-scope").map_or(true, |s| s.to_lowercase() != "home");
        engine.configure(true, days, limit, &excludes, everywhere);
        engine.start();
        r.set(Box::into_raw(engine));
    });
    #[cfg(not(target_os = "macos"))]
    let _ = section;
}

/// Run `f` on the shared Recent engine (no-op before `start_recent`).
pub fn with_recent(f: impl FnOnce(&mut crate::engines::recent_files::RecentFiles)) {
    #[cfg(target_os = "macos")]
    RECENT.with(|r| {
        let p = r.get();
        if !p.is_null() {
            // SAFETY: leaked box, main thread only.
            f(unsafe { &mut *p });
        }
    });
    #[cfg(not(target_os = "macos"))]
    let _ = f;
}

/// `RecentFiles.shared.entries(arrivedOnly:)` (empty before `start_recent`).
pub fn recent_snapshot(arrived: bool) -> Vec<crate::engines::recent_files::Entry> {
    #[cfg(target_os = "macos")]
    {
        return RECENT.with(|r| {
            let p = r.get();
            if p.is_null() {
                Vec::new()
            } else {
                // SAFETY: leaked box, main thread only.
                unsafe { (*p).entries(arrived) }
            }
        });
    }
    #[allow(unreachable_code)]
    {
        let _ = arrived;
        Vec::new()
    }
}

/// The last path component (Swift `(path as NSString).lastPathComponent`).
fn basename(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
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

#[cfg(test)]
mod tests {
    #[test]
    fn compress_names_like_finder() {
        let none = |_: &str| false;
        assert_eq!(compress_target(&["/d/notes.md".into()], &none).as_deref(), Some("/d/notes.md.zip"));
        assert_eq!(compress_target(&["/d/proj".into()], &none).as_deref(), Some("/d/proj.zip"));
        let two = vec!["/d/a.txt".to_string(), "/d/b".to_string()];
        assert_eq!(compress_target(&two, &none).as_deref(), Some("/d/Archive.zip"));
        let taken = |p: &str| p == "/d/Archive.zip" || p == "/d/Archive 2.zip";
        assert_eq!(compress_target(&two, &taken).as_deref(), Some("/d/Archive 3.zip"));
        assert_eq!(compress_target(&[], &none), None);
    }

    #[test]
    fn compress_paths_zips_a_folder_and_a_selection() {
        let dir = std::env::temp_dir().join(format!("ks-zip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("proj/sub")).unwrap();
        std::fs::write(dir.join("proj/sub/x.txt"), "x").unwrap();
        std::fs::write(dir.join("a.txt"), "a").unwrap();
        let d = dir.to_string_lossy().into_owned();
        let one = compress_paths(&[format!("{d}/proj")]).unwrap();
        assert_eq!(one, format!("{d}/proj.zip"));
        assert!(std::fs::metadata(&one).unwrap().len() > 0);
        let many = compress_paths(&[format!("{d}/a.txt"), format!("{d}/proj")]).unwrap();
        assert_eq!(many, format!("{d}/Archive.zip"));
        let list = std::process::Command::new("/usr/bin/unzip").args(["-Z1", &many]).output().unwrap();
        let names = String::from_utf8_lossy(&list.stdout);
        assert!(names.lines().any(|l| l == "a.txt"), "{names}");
        assert!(names.lines().any(|l| l == "proj/sub/x.txt"), "{names}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Row titles of a built menu (`──` for separators) — the drift test for
    /// [`file_context_menu`]'s canonical order.
    fn menu_titles(entries: Vec<crate::ui::menu_kit::MenuEntry>) -> Vec<String> {
        entries
            .into_iter()
            .map(|e| match e {
                crate::ui::menu_kit::MenuEntry::Item { title, .. } => title,
                crate::ui::menu_kit::MenuEntry::Separator => "\u{2500}\u{2500}".to_string(),
                _ => String::new(),
            })
            .collect()
    }

    /// Every core action present — the full canonical menu.
    fn all_menu_actions() -> FileMenuActions {
        FileMenuActions {
            open: Some(Box::new(|| {})),
            reveal: Some(Box::new(|| {})),
            rename: Some(Box::new(|| {})),
            compress: Some(Box::new(|| {})),
            trash: Some(Box::new(|| {})),
            copy_file: Some(Box::new(|| {})),
            copy_path: Some(Box::new(|| {})),
        }
    }

    #[test]
    fn refresh_from_disk_lists_outside_changes_and_keeps_the_selection() {
        let dir = std::env::temp_dir().join(format!("ks-refresh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("b.txt"), "b").unwrap();
        std::fs::write(dir.join("d.txt"), "d").unwrap();
        let mut b = FileBrowser::new(&dir.to_string_lossy());
        b.sort_key = SortKey::Name;
        b.reload();
        let at = |b: &FileBrowser, n: &str| b.list.rows.iter().position(|e| e.name == n).unwrap();
        b.list.selection = at(&b, "d.txt");
        assert!(b.dir_stamp().is_some());
        // Something else drops a file in: a plain reload keeps the stale index,
        // the refresh shows the file and keeps d.txt selected.
        std::fs::write(dir.join("a.txt"), "a").unwrap();
        b.refresh_from_disk();
        assert!(b.list.rows.iter().any(|e| e.name == "a.txt"));
        assert_eq!(b.list.rows[b.list.selection].name, "d.txt");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_text_contents_reads_text_and_refuses_binary() {
        let dir = std::env::temp_dir().join(format!("ks-contents-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let md = dir.join("n.md");
        std::fs::write(&md, "# Title\n\n| a | b |\n").unwrap();
        assert_eq!(file_text_contents(&md.to_string_lossy()).unwrap(), "# Title\n\n| a | b |\n");
        let bin = dir.join("b.bin");
        std::fs::write(&bin, [0xff, 0xfe, 0x00, 0x80]).unwrap();
        assert_eq!(file_text_contents(&bin.to_string_lossy()).unwrap_err(), "Not a text file");
        let big = dir.join("big.log");
        let f = std::fs::File::create(&big).unwrap();
        f.set_len(COPY_CONTENTS_MAX_BYTES + 1).unwrap();
        assert!(file_text_contents(&big.to_string_lossy()).unwrap_err().starts_with("Too large to copy as text ("));
        assert!(copy_contents_entry(&dir.to_string_lossy()).is_none());
        assert!(copy_contents_entry(&md.to_string_lossy()).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_context_menu_core_order_for_a_file() {
        let dir = std::env::temp_dir().join(format!("ks-menu-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        std::fs::write(&file, "a").unwrap();
        let got = menu_titles(file_context_menu(
            &file.to_string_lossy(),
            all_menu_actions(),
        ));
        assert_eq!(
            got,
            vec![
                "Open",
                "Reveal in Files",
                "\u{2500}\u{2500}",
                "Rename\u{2026}",
                "Compress",
                "Move to Trash",
                "\u{2500}\u{2500}",
                "Copy File",
                "Copy Absolute Path",
                "Copy Contents",
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_context_menu_dir_drops_copy_contents_only() {
        let dir = std::env::temp_dir().join(format!("ks-menudir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let got = menu_titles(file_context_menu(
            &dir.to_string_lossy(),
            all_menu_actions(),
        ));
        // Only "Copy Contents" is dropped; the rest of the core — rename /
        // compress / trash included — is a directory's too (Finder parity).
        assert_eq!(
            got,
            vec![
                "Open",
                "Reveal in Files",
                "\u{2500}\u{2500}",
                "Rename\u{2026}",
                "Compress",
                "Move to Trash",
                "\u{2500}\u{2500}",
                "Copy File",
                "Copy Absolute Path",
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_context_menu_collapses_missing_rows_and_separators() {
        // A surface with no DaemonUi can pass only what it backs; empty
        // blocks and their separators vanish.
        let mut a = FileMenuActions::default();
        a.copy_path = Some(Box::new(|| {}));
        assert_eq!(
            menu_titles(file_context_menu("/nonexistent.txt", a)),
            vec!["Copy Absolute Path"]
        );
        let mut a = FileMenuActions::default();
        a.open = Some(Box::new(|| {}));
        a.copy_path = Some(Box::new(|| {}));
        assert_eq!(
            menu_titles(file_context_menu("/nonexistent.txt", a)),
            vec!["Open", "\u{2500}\u{2500}", "Copy Absolute Path"]
        );
        assert!(file_context_menu("/nonexistent.txt", FileMenuActions::default()).is_empty());
    }

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
    fn get_info_rows_for_a_file_and_a_folder() {
        let dir = std::env::temp_dir().join(format!("ks-info-{}", std::process::id()));
        let _ = std::fs::create_dir_all(dir.join("sub"));
        std::fs::write(dir.join("a.txt"), "hello").unwrap();
        std::fs::write(dir.join("sub/b.md"), "123").unwrap();
        let budget = std::time::Duration::from_secs(2);
        let (name, rows) = file_info_rows(&[dir.join("a.txt").to_string_lossy().into_owned()], budget);
        let get = |rows: &[(String, String)], k: &str| rows.iter().find(|r| r.0 == k).map(|r| r.1.clone()).unwrap_or_default();
        assert_eq!(name, "a.txt");
        assert_eq!(get(&rows, "Kind"), "TXT file");
        assert!(get(&rows, "Size").contains("5 bytes"));
        assert!(get(&rows, "Permissions").starts_with("-rw"));
        assert!(!get(&rows, "Modified").is_empty());
        let (_, rows) = file_info_rows(&[dir.to_string_lossy().into_owned()], budget);
        assert_eq!(get(&rows, "Kind"), "Folder");
        assert!(get(&rows, "Size").contains("for 2 files"), "{rows:?}");
        assert_eq!(get(&rows, "Contains"), "2 items");
        let (title, rows) = file_info_rows(
            &[dir.join("a.txt").to_string_lossy().into_owned(), dir.join("sub").to_string_lossy().into_owned()],
            budget,
        );
        assert_eq!(title, "2 items");
        assert!(get(&rows, "Size").contains("(2 files)"));
        assert_eq!(mode_string(0o755, true, false), "drwxr-xr-x");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn opt_right_expands_a_folder_in_place_and_opt_left_collapses() {
        let root = std::env::temp_dir().join(format!("ks-expand-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("a/inner")).unwrap();
        std::fs::write(root.join("a/one.txt"), "1").unwrap();
        std::fs::write(root.join("b.txt"), "b").unwrap();
        let r = root.to_string_lossy().into_owned();
        let mut b = FileBrowser::new(&r);
        b.cd(&r);
        let names = |b: &FileBrowser| b.list.rows.iter().filter(|e| !e.is_parent()).map(|e| (e.name.clone(), e.depth)).collect::<Vec<_>>();
        let a = b.list.rows.iter().position(|e| e.name == "a").unwrap();
        b.list.selection = a;
        assert!(b.expand_selected());
        assert_eq!(names(&b), vec![("a".into(), 0), ("inner".into(), 1), ("one.txt".into(), 1), ("b.txt".into(), 0)]);
        assert_eq!(b.list.rows[b.list.selection].name, "a", "the folder stays selected");
        // Expand the nested one, then collapse from a child row.
        b.list.selection = b.list.rows.iter().position(|e| e.name == "inner").unwrap();
        assert!(b.expand_selected());
        b.list.selection = b.list.rows.iter().position(|e| e.name == "one.txt").unwrap();
        assert!(b.collapse_selected(), "a child collapses its parent");
        assert_eq!(names(&b), vec![("a".into(), 0), ("b.txt".into(), 0)]);
        assert_eq!(b.list.rows[b.list.selection].name, "a");
        assert!(b.expanded.is_empty(), "nested expansions go with it");
        assert!(!b.collapse_selected());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn tabs_keep_their_own_folder_and_close_back_to_one() {
        let root = std::env::temp_dir().join(format!("ks-tabs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("x")).unwrap();
        std::fs::create_dir_all(root.join("y")).unwrap();
        let r = root.to_string_lossy().into_owned();
        let mut b = FileBrowser::new(&r);
        b.cd(&r);
        assert_eq!(b.tab_count(), 1);
        assert!(b.tab_titles().is_empty(), "no tab bar with one tab");
        assert!(!b.close_tab(), "Cmd+W on the only tab hides the window instead");
        b.new_tab();
        b.cd(&format!("{r}/x"));
        b.new_tab();
        b.cd(&format!("{r}/y"));
        assert_eq!(b.tab_titles(), vec![root.file_name().unwrap().to_string_lossy().into_owned(), "x".into(), "y".into()]);
        assert!(b.cycle_tab(1), "wraps to the first");
        assert_eq!(b.cwd, r);
        assert!(b.cycle_tab(-1));
        assert_eq!(b.cwd, format!("{r}/y"));
        b.go_back();
        assert_eq!(b.cwd, format!("{r}/x"), "each tab has its own history (y's tab began on x)");
        assert!(b.select_tab(1));
        assert_eq!(b.cwd, format!("{r}/x"));
        assert!(b.close_tab());
        assert_eq!(b.tab_count(), 2);
        assert!(b.close_tab());
        assert_eq!(b.tab_count(), 1);
        assert!(b.tab_titles().is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn batch_rename_replace_add_and_number() {
        let ps = vec!["/d/IMG_1.jpg".to_string(), "/d/IMG_2.jpg".to_string(), "/d/folder".to_string()];
        let dir = |p: &str| p.ends_with("folder");
        let none = |_: &str| false;
        let o = BatchRename { find: "IMG_".into(), replace: "Trip ".into(), ..Default::default() };
        let plan = batch_rename_plan(&ps[..2], &o, &dir, &none).unwrap();
        assert_eq!(plan, vec![("/d/IMG_1.jpg".into(), "/d/Trip 1.jpg".into()), ("/d/IMG_2.jpg".into(), "/d/Trip 2.jpg".into())]);
        let o = BatchRename { prefix: "old-".into(), suffix: "-v2".into(), ..Default::default() };
        let plan = batch_rename_plan(&ps, &o, &dir, &none).unwrap();
        assert_eq!(plan[0].1, "/d/old-IMG_1-v2.jpg", "suffix goes before the extension");
        assert_eq!(plan[2].1, "/d/old-folder-v2", "folders have no extension");
        let o = BatchRename { base: "Photo".into(), ..Default::default() };
        let plan = batch_rename_plan(&ps[..2], &o, &dir, &none).unwrap();
        assert_eq!(plan[1].1, "/d/Photo 2.jpg");
        // Refusals.
        let o = BatchRename { find: "IMG_1".into(), replace: "IMG_2".into(), ..Default::default() };
        assert!(batch_rename_plan(&ps[..2], &o, &dir, &none).unwrap_err().contains("both"));
        let taken = |p: &str| p == "/d/Trip 1.jpg";
        let o = BatchRename { find: "IMG_".into(), replace: "Trip ".into(), ..Default::default() };
        assert!(batch_rename_plan(&ps[..2], &o, &dir, &taken).unwrap_err().contains("already exists"));
        let o = BatchRename { find: "IMG_1".into(), replace: "".into(), ..Default::default() };
        assert!(batch_rename_plan(&ps[..1], &o, &dir, &none).unwrap_err().contains("empty"));
        let o = BatchRename { find: "x".into(), replace: "y".into(), ..Default::default() };
        assert!(batch_rename_plan(&ps, &o, &dir, &none).unwrap().is_empty(), "no change = no moves");
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
        let mut d = l.clone_for_test();
        d.deselect_all();
        assert!(d.marked.is_empty(), "Cmd+Opt+A drops the multi-selection");
        assert_eq!(d.selected_rows(), vec![1], "the cursor row stays");

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
    fn recursive_glob_matches_across_folders() {
        assert!(path_glob_match("**deep**", "notes/deep.txt"));
        assert!(path_glob_match("**deep**", "deep.txt"));
        assert!(path_glob_match("**deep**", "a/b/deep.txt"));
        assert!(!path_glob_match("**deep**", "notes/shallow.txt"));
        // `**/` matches no folder at all, `*` stays inside one name.
        assert!(path_glob_match("**/*.rs", "foo.rs"));
        assert!(path_glob_match("**/*.rs", "src/ui/foo.rs"));
        assert!(!path_glob_match("sub/*.rs", "sub/ui/foo.rs"), "`*` never crosses a folder");
        assert!(path_glob_match("sub/*.rs", "sub/foo.rs"));
        // No `/`: the name matches at any depth (Swift's `^(?:.*/)?`).
        assert!(path_glob_match("deep.txt", "a/b/deep.txt"));
        assert!(path_glob_match("*deep*", "a/b/deep.txt"));
        assert!(!path_glob_match("*deep*", "a/b/shallow.txt"));
        assert!(path_glob_match("src/**", "src/a/b.txt"));
        assert!(!path_glob_match("src/**", "other/a.txt"));
        // A last component with no wildcard becomes a substring match.
        assert_eq!(widen_recursive_glob("**deep**"), "**deep**");
        assert_eq!(widen_recursive_glob("**/deep.txt"), "**/*deep.txt*");
    }

    #[test]
    fn browser_recursive_filter_lists_matches() {
        let root = std::env::temp_dir().join(format!("ws-rs-fbrec-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("files/notes")).unwrap();
        std::fs::create_dir_all(root.join("files/docs")).unwrap();
        std::fs::write(root.join("files/notes/deep.txt"), "far below").unwrap();
        std::fs::write(root.join("files/docs/shallow.txt"), "near").unwrap();

        let mut b = FileBrowser::new(&root.to_string_lossy());
        b.reload();
        b.set_query("**deep**");
        let names: Vec<&str> = b.visible_rows().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["deep.txt"], "the `**` filter lists matches below the root");
        assert_eq!(
            b.visible_rows()[0].path,
            root.join("files/notes/deep.txt").to_string_lossy()
        );
        assert_eq!(b.visible_rows()[0].trailing_text, "files/notes");

        // A non-matching recursive query lists nothing; clearing restores all.
        b.set_query("**missing**");
        assert!(b.visible_rows().is_empty());
        b.set_query("");
        assert!(b.visible_rows().iter().any(|e| e.name == "files"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn standardize_and_resolve() {
        assert_eq!(standardize("/a/b/../c"), "/a/c");
        assert_eq!(standardize("/a/./b"), "/a/b");
        assert_eq!(resolve_path("src/x", "/work"), "/work/src/x");
        assert_eq!(resolve_path("/abs/./x", "/work"), "/abs/x");
    }

    #[test]
    fn duplicate_toast_names_the_copy() {
        assert_eq!(duplicate_toast(&[]), "");
        assert_eq!(duplicate_toast(&["/d/a copy.txt".into()]), "Duplicated as a copy.txt \u{b7} \u{2318}Z undoes");
        assert_eq!(duplicate_toast(&["/d/a copy".into(), "/d/b copy".into()]), "Duplicated 2 items \u{b7} \u{2318}Z undoes");
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
        let s = short_date_fields(civil_from_epoch(1609556640.0), Some(2021));
        assert_eq!(s, "Jan 2 03:04");
        assert_eq!(short_date_fields(civil_from_epoch(1609556640.0), Some(2022)), "Jan 2 2021");
        assert_eq!(ago(0.0), "just now");
        assert_eq!(ago(120.0), "2m ago");
        assert_eq!(ago(7200.0), "2h ago");
        assert_eq!(ago(172800.0), "2d ago");
    }

    #[test]
    fn short_date_is_local_time_not_utc() {
        // The wall clock `localtime_r` gives for the instant, whatever the zone.
        let t = 1609556640.0;
        let d = crate::engines::screenshot_annotations::ShotDate::from_unix_local(t as i64);
        assert_eq!(short_date(t, t), format!("{} {} {:02}:{:02}", MONTHS[d.month as usize - 1], d.day, d.hour, d.minute));
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

    #[test]
    fn quick_look_state_starts_empty() {
        let ql = QuickLook::new();
        assert!(ql.paths.is_empty());
        assert!(ql.state().is_empty(), "down panel exposes no quickLook paths");
        let b = FileBrowser::default();
        assert_eq!(b.test_state()["quickLook"], json!([]));
    }

    #[test]
    fn file_browser_rename_model() {
        let root = std::env::temp_dir().join(format!("ws-rs-fbrename-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let old = root.join("a.txt");
        std::fs::write(&old, "x").unwrap();
        let old = old.to_string_lossy().into_owned();

        let mut b = FileBrowser::new(&root.to_string_lossy());
        b.reload();
        // Row 0 is `..`, so select the file explicitly.
        let idx = b.list.rows.iter().position(|e| e.name == "a.txt").unwrap();
        b.list.selection = idx;

        b.begin_rename(None);
        assert!(b.rename.is_some());
        assert_eq!(b.rename_editor.as_deref(), Some("a.txt"));

        // Reject an invalid name; nothing moves.
        b.rename = Some((old.clone(), "bad/name".to_string()));
        let msg = b.commit_rename().unwrap();
        assert!(msg.contains("not a valid name"));
        assert!(Path::new(&old).exists());

        // A real rename moves the file and re-selects it.
        b.begin_rename(Some(idx));
        b.rename = Some((old.clone(), "renamed.txt".to_string()));
        let msg = b.commit_rename().unwrap();
        assert_eq!(msg, "renamed “a.txt” to “renamed.txt”");
        assert!(root.join("renamed.txt").exists());
        assert_eq!(b.list.rows[b.list.selection].name, "renamed.txt");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn file_browser_rename_ignores_parent_row() {
        let root = std::env::temp_dir().join(format!("ws-rs-fbparent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let mut b = FileBrowser::new(&root.to_string_lossy());
        b.reload();
        let parent = b.list.rows.iter().position(|e| e.is_parent()).unwrap();
        b.begin_rename(Some(parent));
        assert!(b.rename.is_none(), "`..` is never renameable");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Renaming on the Recent/Arrived page must re-key the engine so the row
    /// survives: our own move is not reported by the FSEvents stream.
    #[cfg(target_os = "macos")]
    #[test]
    fn file_browser_rename_recent_scope_rekeys() {
        use crate::engines::recent_files::{RecentFiles, FSEVENT_ITEM_CREATED, FSEVENT_ITEM_IS_FILE};

        let home = std::env::temp_dir().join(format!("ws-rs-fbrecent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let home = home.to_string_lossy().into_owned();
        let old = format!("{home}/a.txt");
        std::fs::write(&old, "x").unwrap();

        // Install a real Recent engine into the module's RECENT thread-local
        // (the one `with_recent` / `recent_snapshot` use). Leaked on this
        // thread; no other test in this module touches RECENT.
        let mut engine = Box::new(RecentFiles::new(&home, &format!("{home}/store/recent.json")));
        engine.configure(true, 7, 200, &[], false);
        engine.handle(
            &[old.clone()],
            &[FSEVENT_ITEM_CREATED | FSEVENT_ITEM_IS_FILE],
            &[Some(1)],
        );
        assert!(engine.entries(false).iter().any(|e| e.path == old), "engine seeded with the old path");
        RECENT.with(|r| r.set(Box::into_raw(engine)));

        // The browser on its default Recent/Arrived page.
        let mut b = FileBrowser::new(&home);
        b.show_virtual(0);
        let idx = b.list.rows.iter().position(|e| e.path == old).unwrap();
        b.list.selection = idx;

        b.begin_rename(Some(idx));
        b.rename = Some((old.clone(), "b.txt".to_string()));
        let dst = format!("{home}/b.txt");
        let msg = b.commit_rename().unwrap();
        assert_eq!(msg, "renamed “a.txt” to “b.txt”");
        assert!(Path::new(&dst).exists(), "the file moved on disk");
        let row = b.list.rows.iter().find(|e| e.path == dst);
        assert!(row.is_some(), "the renamed row stays on the Recent page");
        assert_eq!(row.unwrap().name, "b.txt");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn files_handle_contract() {
        use crate::views::files_view::FilesEvent;
        let mut b = FileBrowser::new("/tmp");
        b.set_all(vec![
            Entry::new("..", "/", true, 0),
            Entry::new("a.txt", "/tmp/a.txt", false, 1),
            Entry::new("b.md", "/tmp/b.md", false, 2),
        ]);

        // A query narrows the rows and resets the selection.
        let before = b.snapshot();
        b.handle(FilesEvent::Query("b".into()));
        let after = b.snapshot();
        assert_ne!(after, before);
        assert_eq!(after.query, "b");
        assert_eq!(after.rows.len(), 1);
        assert_eq!(after.rows[0].name, "b.md");

        // Move walks the visible rows.
        b.handle(FilesEvent::Query(String::new()));
        b.handle(FilesEvent::Move(2));
        assert_eq!(b.snapshot().selection, 2);

        // Cmd-click extends the marked set from the current selection.
        b.handle(FilesEvent::Click { row: 1, cmd: true, shift: false, double: false });
        assert_eq!(
            b.snapshot().marked.iter().copied().collect::<Vec<_>>(),
            vec![1, 2]
        );

        // Escape clears a non-empty query.
        b.handle(FilesEvent::Query("z".into()));
        assert_eq!(b.snapshot().query, "z");
        b.handle(FilesEvent::Escape);
        assert!(b.snapshot().query.is_empty());
    }

    #[test]
    fn files_handle_pin_and_sidebar() {
        use crate::ui::sidebar::SidebarEvent;
        use crate::views::files_view::FilesEvent;
        let fav = std::env::temp_dir().join(format!("ws-rs-filesfav-{}", std::process::id()));
        let _ = std::fs::remove_file(&fav);
        let mut b = FileBrowser::new("/tmp");
        b.favorites_file = fav.to_string_lossy().into_owned();
        b.set_all(Vec::new());
        assert!(!b.snapshot().pinned_on);
        b.handle(FilesEvent::TogglePin);
        assert!(b.snapshot().pinned_on, "pinning /tmp flips pinned_on");
        b.handle(FilesEvent::TogglePin);
        assert!(!b.snapshot().pinned_on);
        let _ = std::fs::remove_file(&fav);

        // Selecting the first virtual list switches into Recent.
        b.handle(FilesEvent::Sidebar(SidebarEvent::Select(0)));
        assert!(b.in_recent());
    }
}

/// Where the sidebar's pinned folders are kept: beside the personal
/// commands.toml override (`<configDir>/files-favorites.json`), so pins
/// live with the other machine-local settings, not in the repo or a cache.
/// The old `~/.cache/kitchen-sink/files-favorites.json` is copied over once.
fn favorites_file_path() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let legacy = format!("{home}/.cache/kitchen-sink/files-favorites.json");
    let Some(dir) = crate::app::config::commands_override_path()
        .and_then(|p| p.parent().map(Path::to_path_buf))
    else {
        return legacy;
    };
    let path = dir.join("files-favorites.json");
    if !path.exists() && Path::new(&legacy).exists() {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::copy(&legacy, &path);
    }
    path.to_string_lossy().into_owned()
}
