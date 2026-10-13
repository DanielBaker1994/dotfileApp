//! Notes family — mirrors the model layer of `NotesProse.swift`,
//! `NoteFindWindow.swift`, `FilePopup.swift` and `InlineRename.swift`, plus the
//! notes-specific parts of `PopupWindow.swift` (new-note/template logic, the
//! tab model, the left NOTES sidebar and the prose reading flow).
//!
//! The AppKit surfaces are mostly real: the floating `ProseWindow` panel
//! (`WKWebView` over the `ProseRender` HTML), the `prose` child-process launch,
//! and the embeddable notes surface ([`build_content`] / [`NotesSurface`]:
//! sidebar + `NSTextView` editor plus hidden terminal/browser drawers). The
//! `FilePopupPanel` / inline `NSTextField` surfaces
//! are not built here. Helper calls go through the
//! ported `PythonHelper` to the frozen `pylib/` (`prose.*`, `doc_templates.*`,
//! `config.*`), exactly like `AIFormat.swift` does.

use crate::app::registry::{PaletteCommand, RectI, Registry, SlotMember, SlotView};
use crate::ui::popup::{KEY_ESC, KEY_F, KEY_N, KEY_SLASH};
use crate::ui::theme::{PopupColors, PopupPalette, PopupTone, Rgba};
use crate::views::notes_tree::{NotesTree, TreeRow};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

// ---------------------------------------------------------------------------
// Shared constants + tiny formatters
// ---------------------------------------------------------------------------

pub const DEFAULT_NEW_NOTE_NAME: &str = "Untitled";
pub const DEFAULT_NEW_DOC_NAME: &str = "doc";
pub const DEFAULT_NEW_DOC_TEMPLATE: &str = "markdown_doc_catppuccin_latte";
pub const NOTES_LABEL: &str = "NOTES";
pub const SIDEBAR_MIN_WIDTH: f64 = 140.0;
pub const SIDEBAR_MAX_WIDTH: f64 = 480.0;
pub const SIDEBAR_DEFAULT_WIDTH: f64 = 210.0;
pub const RAIL_WIDTH: f64 = 50.0;
pub const SIDEBAR_ROW_HEIGHT: f64 = 34.0;
pub const NOTE_FIND_DEFAULT_WIDTH: f64 = 640.0;
pub const NOTE_FIND_DEFAULT_ROWS: usize = 12;
pub const NOTE_FIND_DEFAULT_GREP_MAX: usize = 200;
pub const PROSE_MIN_ZOOM: f64 = 0.5;
pub const PROSE_MAX_ZOOM: f64 = 4.0;

/// Height of the notes terminal drawer (bottom).
pub const TERMINAL_DRAWER_HEIGHT: f64 = 200.0;
/// Width of the notes file-browser drawer (side).
pub const BROWSER_DRAWER_WIDTH: f64 = 260.0;

/// Pure bookkeeping for the notes view's two drawers (terminal at the bottom,
/// browser at the side). AppKit-free so it is unit-testable; [`NotesSurface`]
/// holds one and mirrors it onto the real drawer views.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DrawerState {
    terminal: bool,
    browser: bool,
}

impl DrawerState {
    /// Flip the named drawer (`"terminal"` / `"browser"`), returning the new
    /// shown flag. `None` for an unknown name.
    pub fn toggle(&mut self, which: &str) -> Option<bool> {
        match which {
            "terminal" => {
                self.terminal = !self.terminal;
                Some(self.terminal)
            }
            "browser" => {
                self.browser = !self.browser;
                Some(self.browser)
            }
            _ => None,
        }
    }

    pub fn terminal_shown(&self) -> bool {
        self.terminal
    }

    pub fn browser_shown(&self) -> bool {
        self.browser
    }

    /// The visible drawer extent in points: 0 when none shown, otherwise the
    /// max of the shown drawers' dimensions (terminal height / browser width).
    pub fn inset(&self) -> i64 {
        let mut inset = 0.0f64;
        if self.terminal {
            inset = inset.max(TERMINAL_DRAWER_HEIGHT);
        }
        if self.browser {
            inset = inset.max(BROWSER_DRAWER_WIDTH);
        }
        inset as i64
    }

    /// The exact socket `state.views.notes.drawer*` shape.
    pub fn test_state(&self) -> Value {
        json!({
            "terminal": self.terminal,
            "browser": self.browser,
            "drawerInset": self.inset(),
        })
    }
}

/// Basenames of the regular files directly inside `dir`, sorted (the contents
/// of the notes browser drawer). A missing/unreadable directory yields `[]`.
pub fn directory_names(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    names
}

/// `PathsWindow.age`.
pub fn age_text(secs: f64) -> String {
    if secs < 60.0 {
        "now".to_string()
    } else if secs < 3600.0 {
        format!("{}m", (secs / 60.0) as i64)
    } else if secs < 86400.0 {
        format!("{}h", (secs / 3600.0) as i64)
    } else {
        format!("{}d", (secs / 86400.0) as i64)
    }
}

/// `PathsWindow.folder` — the ~ / /private/tmp abbreviation and the 34-char
/// middle-ellipsis rule.
pub fn folder_text(path: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut d = Path::new(path)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    if d == home {
        return "~".to_string();
    }
    if !home.is_empty() && d.starts_with(&format!("{home}/")) {
        d = format!("~{}", &d[home.len()..]);
    }
    if d.starts_with("/private/tmp") {
        d = d[8..].to_string();
    }
    if d.chars().count() <= 34 {
        return d;
    }
    let comps: Vec<&str> = d.split('/').collect();
    let tail: Vec<&str> = comps.iter().rev().take(2).rev().cloned().collect();
    format!("…/{}", tail.join("/"))
}

/// `InlineRename` / `NSHomeDirectory` replacement for the find window rel path.
pub fn tilde_path(path: &str, home: &str) -> String {
    if !home.is_empty() && path.starts_with(&format!("{home}/")) {
        format!("~{}", &path[home.len()..])
    } else {
        path.to_string()
    }
}

/// The `[notes] paths` open-note list (`csv(_:)`) with `~` expansion and the
/// existence filter `notePaths(_:)` applies before opening tabs.
pub fn existing_note_paths(raw: &str, exists: &dyn Fn(&str) -> bool) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(expand_tilde)
        .filter(|p| exists(p))
        .collect()
}

/// `newNote(template:)` base-name rule: whitespace runs collapse to `-`.
pub fn slugify(name: &str) -> String {
    name.split_whitespace().collect::<Vec<_>>().join("-")
}

fn helper_call(method: &str, params: Value, timeout_secs: u64) -> Option<Value> {
    crate::app::python_helper::PythonHelper::shared()
        .call(
            method,
            params,
            Duration::from_secs(timeout_secs),
            Duration::from_secs(5),
        )
        .ok()
}

fn read_commands_text() -> Option<String> {
    let path = std::env::var("WS_COMMANDS_CONF")
        .ok()
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".config/kitchen-sink/commands.toml"))
        })?;
    std::fs::read_to_string(path).ok()
}

/// `configSectionValue(section, key)`: the first entry of that section, read
/// through the ONE codec's `config.section_entries`.
pub fn section_value(section: &str, key: &str) -> Option<String> {
    let text = read_commands_text()?;
    let params = json!({"text": text, "section": section});
    helper_call("config.section_entries", params, 10)?
        .get("entries")?
        .as_array()?
        .iter()
        .find_map(|e| {
            if e.get("key").and_then(Value::as_str) == Some(key) {
                e.get("value").and_then(Value::as_str).map(str::to_string)
            } else {
                None
            }
        })
}

fn setting(section: &str, key: &str) -> Option<String> {
    section_value(section, key)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn number(section: &str, key: &str, fallback: f64) -> f64 {
    setting(section, key)
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(fallback)
}

// ---------------------------------------------------------------------------
// [notes] / [notes-find] config
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct NotesConfig {
    pub new_note_name: String,
    pub new_doc_name: String,
    pub new_doc_template: String,
    pub sidebar_width: f64,
    pub prose_font: String,
    pub prose_font_size: f64,
    pub prose_width: f64,
    pub doc_templates: Option<String>,
    pub pdf_css: Option<String>,
    pub pdf_filter: Option<String>,
    pub pdf_highlight: Option<String>,
    pub pdf_engine: Option<String>,
    pub pdf_path: Option<String>,
    pub copy_buttons: bool,
}

impl Default for NotesConfig {
    fn default() -> Self {
        NotesConfig {
            new_note_name: DEFAULT_NEW_NOTE_NAME.to_string(),
            new_doc_name: DEFAULT_NEW_DOC_NAME.to_string(),
            new_doc_template: DEFAULT_NEW_DOC_TEMPLATE.to_string(),
            sidebar_width: SIDEBAR_DEFAULT_WIDTH,
            prose_font: String::new(),
            prose_font_size: 19.0,
            prose_width: 900.0,
            doc_templates: None,
            pdf_css: None,
            pdf_filter: None,
            pdf_highlight: None,
            pdf_engine: None,
            pdf_path: None,
            copy_buttons: true,
        }
    }
}

impl NotesConfig {
    /// `parseAppConfig` for the `[notes]` section; falls back to the Swift
    /// defaults when the helper is unavailable.
    pub fn load() -> NotesConfig {
        let mut c = NotesConfig::default();
        if let Some(v) = setting("notes", "new-note-name") {
            c.new_note_name = v;
        }
        if let Some(v) = setting("notes", "new-doc-name") {
            c.new_doc_name = v;
        }
        if let Some(v) = setting("notes", "new-doc-template") {
            c.new_doc_template = v;
        }
        c.sidebar_width = number("notes", "sidebar-width", c.sidebar_width);
        if let Some(v) = setting("notes", "font") {
            c.prose_font = v;
        }
        c.prose_font_size = number("notes", "font-size", c.prose_font_size);
        c.prose_width = number("notes", "prose-width", c.prose_width);
        c.doc_templates = setting("notes", "doc-templates");
        c.pdf_css = setting("notes", "pdf-css");
        c.pdf_filter = setting("notes", "pdf-filter");
        c.pdf_highlight = setting("notes", "pdf-highlight");
        c.pdf_engine = setting("notes", "pdf-engine-bin");
        c.pdf_path = setting("notes", "pdf-path");
        c.copy_buttons = crate::engines::config_text::tri(setting("notes", "copy-buttons").as_deref())
            .unwrap_or(true);
        c
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct NoteFindConfig {
    pub width: f64,
    pub rows: usize,
    pub grep_max: usize,
    pub rg_bin: Option<String>,
}

impl Default for NoteFindConfig {
    fn default() -> Self {
        NoteFindConfig {
            width: NOTE_FIND_DEFAULT_WIDTH,
            rows: NOTE_FIND_DEFAULT_ROWS,
            grep_max: NOTE_FIND_DEFAULT_GREP_MAX,
            rg_bin: None,
        }
    }
}

impl NoteFindConfig {
    pub fn load() -> NoteFindConfig {
        let mut c = NoteFindConfig::default();
        c.width = number("notes-find", "width", c.width).max(360.0);
        c.rows = number("notes-find", "rows", c.rows as f64).max(5.0) as usize;
        c.grep_max = number("notes-find", "grep-max", c.grep_max as f64).max(1.0) as usize;
        c.rg_bin = setting("notes-find", "rg-bin");
        c
    }
}

// ---------------------------------------------------------------------------
// New note / template snippets
// ---------------------------------------------------------------------------

/// `snippetText`: the vim snippet `body` with its placeholders stripped, always
/// ending in a newline. `None` = no such snippet.
pub fn snippet_text(all: &Value, name: &str) -> Option<String> {
    let snip = all.get(name)?;
    let mut text = match snip.get("body") {
        Some(Value::Array(parts)) => parts
            .iter()
            .map(|p| p.as_str().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n"),
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    };
    text = strip_placeholders(&text);
    if text.ends_with('\n') {
        Some(text)
    } else {
        Some(format!("{text}\n"))
    }
}

/// The three regex passes of `snippetText`, hand-rolled (no regex crate):
/// `${N:x}` → `x`, `${N}` removed, `$N` removed unless backslash-escaped,
/// then `\$` → `$`.
fn strip_placeholders(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    // Pass 1: ${N:default} -> default
    let mut pass1 = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' && i + 2 < chars.len() && chars[i + 1] == '{' {
            let mut j = i + 2;
            while j < chars.len() && chars[j].is_ascii_digit() {
                j += 1;
            }
            if j > i + 2 && j < chars.len() && chars[j] == ':' {
                if let Some(close) = chars[j + 1..].iter().position(|&c| c == '}') {
                    let end = j + 1 + close;
                    for c in &chars[j + 1..end] {
                        pass1.push(*c);
                    }
                    i = end + 1;
                    continue;
                }
            }
        }
        pass1.push(chars[i]);
        i += 1;
    }
    // Pass 2: remove ${N} (unconditional) and $N unless preceded by a `\`.
    let p2: Vec<char> = pass1.chars().collect();
    let mut pass2 = String::new();
    let mut i = 0;
    while i < p2.len() {
        if p2[i] == '$' {
            let mut j = i + 1;
            let braced = j < p2.len() && p2[j] == '{';
            if braced {
                j += 1;
            }
            let digits_start = j;
            while j < p2.len() && p2[j].is_ascii_digit() {
                j += 1;
            }
            if j > digits_start && ((braced && j < p2.len() && p2[j] == '}')
                || (!braced))
            {
                let end = if braced { j + 1 } else { j };
                let escaped = i > 0 && p2[i - 1] == '\\';
                if braced || !escaped {
                    i = end;
                    continue;
                }
            }
        }
        pass2.push(p2[i]);
        i += 1;
    }
    pass2.replace("\\$", "$")
}

/// A planned new note: `newNote(template:)` before the file write.
#[derive(Clone, Debug, PartialEq)]
pub struct NewNotePlan {
    pub path: String,
    pub body: String,
    pub snippet_missing: bool,
}

impl NewNotePlan {
    pub fn write(&self) -> std::io::Result<()> {
        std::fs::write(&self.path, self.body.as_bytes())
    }
}

/// Build the plan (`dir/base-N.md`, body from `new-doc-template`). `exists`
/// and `open` mirror `FileManager.fileExists` + the open `paths` array.
pub fn plan_new_note(
    dir: &str,
    is_doc: bool,
    all_snippets: &Value,
    cfg: &NotesConfig,
    open: &[String],
) -> NewNotePlan {
    let base = if is_doc {
        cfg.new_doc_name.clone()
    } else {
        cfg.new_note_name.clone()
    };
    let path = new_note_path(dir, &base, open, &|p| Path::new(p).exists());
    let snippet = snippet_text(all_snippets, &cfg.new_doc_template);
    NewNotePlan {
        path,
        body: snippet.clone().unwrap_or_default(),
        snippet_missing: snippet.is_none(),
    }
}

/// `repeat { path = dir/base-N.md; n += 1 } while fileExists || paths.contains`.
pub fn new_note_path(
    dir: &str,
    base: &str,
    open: &[String],
    exists: &dyn Fn(&str) -> bool,
) -> String {
    let base = slugify(base);
    let mut n = 1;
    loop {
        let path = format!("{}/{}-{}.md", dir.trim_end_matches('/'), base, n);
        n += 1;
        if !exists(&path) && !open.iter().any(|p| p == &path) {
            return path;
        }
    }
}

// ---------------------------------------------------------------------------
// Tabs + dismissed notes
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoteTab {
    pub path: String,
    pub title: String,
}

/// The notes `PopupTabsBar` model (`paths` / `titles` / `selectedTab`) plus the
/// `DismissedNotes` set.
#[derive(Clone, Debug, Default)]
pub struct NotesTabs {
    pub tabs: Vec<NoteTab>,
    pub selected: usize,
    dismissed: HashSet<String>,
}

impl NotesTabs {
    pub fn new() -> Self {
        NotesTabs::default()
    }

    pub fn len(&self) -> usize {
        self.tabs.len()
    }
    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    pub fn paths(&self) -> Vec<String> {
        self.tabs.iter().map(|t| t.path.clone()).collect()
    }

    pub fn titles(&self) -> Vec<String> {
        self.tabs.iter().map(|t| t.title.clone()).collect()
    }

    pub fn selected_path(&self) -> Option<&str> {
        self.tabs.get(self.selected).map(|t| t.path.as_str())
    }

    /// `appendTab`: title = last path component, selected = last, undismissed.
    pub fn append(&mut self, path: &str) -> usize {
        let title = Path::new(path)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string());
        self.tabs.push(NoteTab {
            path: path.to_string(),
            title,
        });
        self.dismissed.remove(path);
        self.selected = self.tabs.len() - 1;
        self.selected
    }

    /// `openExternal`: focus an already-open path, else append.
    pub fn open_external(&mut self, path: &str) -> bool {
        if let Some(i) = self.tabs.iter().position(|t| t.path == path) {
            self.selected = i;
            true
        } else {
            self.append(path);
            false
        }
    }

    /// Close tab by index; returns its path (the caller decides the fallback).
    pub fn close(&mut self, index: usize) -> Option<String> {
        if index >= self.tabs.len() {
            return None;
        }
        let removed = self.tabs.remove(index);
        if self.selected > index {
            self.selected -= 1;
        }
        if self.selected >= self.tabs.len() && !self.tabs.is_empty() {
            self.selected = self.tabs.len() - 1;
        }
        Some(removed.path)
    }

    pub fn dismiss(&mut self, path: &str) {
        self.dismissed.insert(path.to_string());
    }
    pub fn undismiss(&mut self, path: &str) {
        self.dismissed.remove(path);
    }
    pub fn is_dismissed(&self, path: &str) -> bool {
        self.dismissed.contains(path)
    }
    /// The dismissed set, sorted (what [`save_dismissed`] persists).
    pub fn dismissed_paths(&self) -> Vec<String> {
        let mut v: Vec<String> = self.dismissed.iter().cloned().collect();
        v.sort();
        v
    }
    pub fn set_dismissed(&mut self, paths: impl IntoIterator<Item = String>) {
        self.dismissed = paths.into_iter().collect();
    }
}

const DISMISSED_KEY: &str = "notes.dismissed";

/// The persisted dismissed-notes set (Swift `DismissedNotes`): closing a tab
/// must survive a restart, since the tabs are rebuilt from `[notes] paths`.
pub fn load_dismissed() -> Vec<String> {
    crate::platform::get_string(DISMISSED_KEY)
        .map(|s| s.lines().filter(|l| !l.is_empty()).map(str::to_string).collect())
        .unwrap_or_default()
}

pub fn save_dismissed(paths: &[String]) {
    crate::platform::set_string(DISMISSED_KEY, &paths.join("\n"));
}

/// `lastWrite` from the file's modification time (unix seconds).
pub fn last_write(path: &str) -> Option<f64> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs_f64())
}

// ---------------------------------------------------------------------------
// Sidebar model (left NOTES sidebar)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SidebarRow {
    pub title: String,
    pub icon: Option<String>,
    pub trailing: Option<String>,
    pub pinned: bool,
    pub section: String,
}

/// `PopupTabsBar.vertical` state for notes: the NOTES section header, the
/// width/rail, and the rows' icon · name · last-write.
#[derive(Clone, Debug, PartialEq)]
pub struct NotesSidebar {
    pub label: String,
    pub width: f64,
    pub collapsed: bool,
    pub rows: Vec<SidebarRow>,
}

impl Default for NotesSidebar {
    fn default() -> Self {
        NotesSidebar {
            label: NOTES_LABEL.to_string(),
            width: SIDEBAR_DEFAULT_WIDTH,
            collapsed: false,
            rows: Vec::new(),
        }
    }
}

impl NotesSidebar {
    pub fn new(width: f64) -> Self {
        NotesSidebar {
            width: width.clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH),
            ..Default::default()
        }
    }

    /// Effective width, matching `PopupTabsBar.width(expanded:)`.
    pub fn visible_width(&self) -> f64 {
        if self.collapsed {
            RAIL_WIDTH
        } else {
            self.width
        }
    }

    pub fn set_width(&mut self, w: f64) {
        self.width = w.clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH);
    }

    /// Rebuild the tab rows for the notes document list.
    pub fn rebuild(&mut self, tabs: &NotesTabs, now: f64, times: &dyn Fn(&str) -> Option<f64>) {
        self.rows = tabs
            .tabs
            .iter()
            .map(|t| SidebarRow {
                title: t.title.clone(),
                icon: Some("doc.text".to_string()),
                trailing: times(&t.path).map(|m| age_text(now - m)),
                pinned: false,
                section: self.label.clone(),
            })
            .collect();
    }

    pub fn row_index_for_path(&self, tabs: &NotesTabs, path: &str) -> Option<usize> {
        tabs.tabs.iter().position(|t| t.path == path)
    }
}

// ---------------------------------------------------------------------------
// Prose reading view + PDF export
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProseSource {
    pub markdown: String,
    pub path: String,
}

impl ProseSource {
    pub fn new(markdown: impl Into<String>, path: impl Into<String>) -> Self {
        ProseSource {
            markdown: markdown.into(),
            path: path.into(),
        }
    }
}

/// The `[notes]` PDF knobs collected into the helper's `config` object.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProsePdfConfig {
    pub pandoc: String,
    pub engine: String,
    pub css: Option<String>,
    pub filter: Option<String>,
    pub highlight: Option<String>,
    pub out_dir: Option<String>,
    pub theme_css: Option<String>,
}

impl ProsePdfConfig {
    pub fn to_json(&self) -> Value {
        let mut o = serde_json::Map::new();
        o.insert("pandoc".into(), json!(self.pandoc));
        if !self.engine.is_empty() {
            o.insert("engine".into(), json!(self.engine));
        }
        if let Some(v) = &self.css {
            o.insert("css".into(), json!(v));
        }
        if let Some(v) = &self.filter {
            o.insert("filter".into(), json!(v));
        }
        if let Some(v) = &self.highlight {
            o.insert("highlight".into(), json!(v));
        }
        if let Some(v) = &self.out_dir {
            o.insert("outDir".into(), json!(v));
        }
        if let Some(v) = &self.theme_css {
            o.insert("themeCSS".into(), json!(v));
        }
        Value::Object(o)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PdfExport {
    pub ok: bool,
    pub out: Option<String>,
    pub error: Option<String>,
}

pub struct ProseRender;

impl ProseRender {
    pub fn esc(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    }

    fn hex_rgb(c: Rgba) -> String {
        let c = c.opaque();
        format!(
            "#{:02X}{:02X}{:02X}",
            (c.r * 255.0).round() as i64,
            (c.g * 255.0).round() as i64,
            (c.b * 255.0).round() as i64
        )
    }

    fn rgba_str(x: Rgba, a: f64) -> String {
        let c = x.opaque();
        format!(
            "rgba({},{},{}, {:.3})",
            (c.r * 255.0).round() as i64,
            (c.g * 255.0).round() as i64,
            (c.b * 255.0).round() as i64,
            a
        )
    }

    /// `highlightTheme(_:)`.
    pub fn highlight_theme(c: &PopupColors) -> &'static str {
        if c.is_light() {
            "tango"
        } else {
            "breezedark"
        }
    }

    /// `themeCSS(_:)` — the `<style>:root { --p-… }</style>` block.
    pub fn theme_css(c: &PopupColors) -> String {
        let bg = Self::hex_rgb(c.base());
        let text = Self::hex_rgb(c.text);
        let dim = Self::hex_rgb(c.dim);
        let accent = Self::hex_rgb(c.accent_on());
        let accent2 = Self::hex_rgb(c.tone(PopupTone::Accent2));
        let well = Self::hex_rgb(c.mantle());
        let s0 = Self::hex_rgb(c.surface0());
        let s1 = Self::hex_rgb(c.surface1());
        let rule = Self::hex_rgb(c.dim.opaque().blended(0.6, c.base()));
        let vars: [(&str, String); 19] = [
            ("bg", bg),
            ("text", text),
            ("dim", dim),
            ("accent", accent),
            ("accent2", accent2),
            ("well", well),
            ("s0", s0),
            ("s1", s1),
            ("rule", rule),
            ("info", Self::hex_rgb(c.tone(PopupTone::Info))),
            ("info-tint", Self::rgba_str(c.tone(PopupTone::Info), 0.12)),
            ("success", Self::hex_rgb(c.tone(PopupTone::Success))),
            ("success-tint", Self::rgba_str(c.tone(PopupTone::Success), 0.12)),
            ("accent2-tint", Self::rgba_str(c.tone(PopupTone::Accent2), 0.12)),
            ("warning", Self::hex_rgb(c.tone(PopupTone::Warning))),
            ("warning-tint", Self::rgba_str(c.tone(PopupTone::Warning), 0.12)),
            ("danger", Self::hex_rgb(c.tone(PopupTone::Danger))),
            ("danger-tint", Self::rgba_str(c.tone(PopupTone::Danger), 0.12)),
            ("selection", Self::rgba_str(c.accent_on(), 0.28)),
        ];
        let root: String = vars
            .iter()
            .map(|(k, v)| format!("--p-{k}: {v};"))
            .collect::<Vec<_>>()
            .join(" ");
        format!("<style>\n:root {{ {root} }}\n</style>")
    }

    /// `copyButtonCSS` fallback (when no sibling `copy_button.css` exists).
    pub fn copy_button_css(c: &PopupColors) -> String {
        format!(
            ".codeblock {{ display: flex; align-items: stretch; }}\n\
             .codeblock pre {{ flex: 1 1 auto; min-width: 0; }}\n\
             .copy-btn {{ flex: 0 0 auto; align-self: stretch; width: 34px; margin: .8em 8px .8em 0;\n\
             \x20 display: flex; align-items: center; justify-content: center; padding: 0;\n\
             \x20 border: none; border-radius: 8px; background: var(--p-s0, {s0}); color: var(--p-dim, {dim});\n\
             \x20 cursor: pointer; transition: background .15s, color .15s; }}\n\
             .copy-btn:hover {{ background: var(--p-s1, {s1}); color: var(--p-text, {text}); }}\n\
             .copy-btn:active {{ background: var(--p-s1, {s1}); }}\n\
             .copy-btn.copied {{ background: var(--p-success-tint, {stint}); color: var(--p-success, {suc}); }}\n\
             .copy-btn:focus {{ outline: none; }}\n\
             .copy-btn svg {{ width: 16px; height: 16px; }}",
            s0 = Self::hex_rgb(c.surface0()),
            s1 = Self::hex_rgb(c.surface1()),
            dim = Self::hex_rgb(c.dim),
            text = Self::hex_rgb(c.text),
            stint = Self::rgba_str(c.tone(PopupTone::Success), 0.20),
            suc = Self::hex_rgb(c.tone(PopupTone::Success)),
        )
    }

    /// `copyButtonStyles(_:cssPath:)` — prefer a `copy_button.css` beside the
    /// `[notes] pdf-css`, else the builtin block.
    pub fn copy_button_styles(c: &PopupColors, css_path: Option<&str>) -> String {
        if let Some(p) = css_path.filter(|p| !p.is_empty()) {
            let expanded = expand_tilde(p);
            if let Some(parent) = Path::new(&expanded).parent() {
                let file = parent.join("copy_button.css");
                if let Ok(text) = std::fs::read_to_string(&file) {
                    return text.replace("<style>", "").replace("</style>", "");
                }
            }
        }
        Self::copy_button_css(c)
    }

    /// `ProseRender.basic` — the pandoc-less markdown fallback.
    pub fn basic(md: &str) -> String {
        let mut out: Vec<String> = Vec::new();
        let mut para: Vec<String> = Vec::new();
        let mut list: Option<&str> = None;
        let mut fence: Option<Vec<String>> = None;
        let mut fence_lang = String::new();

        fn pre(lang: &str, lines: &[String]) -> String {
            let cls = if lang.is_empty() {
                String::new()
            } else {
                format!(" class=\"{}\"", ProseRender::esc(lang))
            };
            format!("<pre{cls}><code>{}</code></pre>", ProseRender::esc(&lines.join("\n")))
        }

        macro_rules! flush_para {
            () => {
                if !para.is_empty() {
                    out.push(format!(
                        "<p>{}</p>",
                        para.iter().map(|s| ProseRender::inline(s)).collect::<Vec<_>>().join(" ")
                    ));
                    para.clear();
                }
            };
        }
        macro_rules! close_list {
            () => {
                if let Some(l) = list.take() {
                    out.push(format!("</{l}>"));
                }
            };
        }

        for line in md.split('\n') {
            let t = line.trim();
            if let Some(f) = fence.as_mut() {
                if t.starts_with("```") {
                    out.push(pre(&fence_lang, f));
                    fence = None;
                } else {
                    f.push(line.to_string());
                }
                continue;
            }
            if t.starts_with("```") {
                flush_para!();
                close_list!();
                fence_lang = t[3..].trim().split(' ').next().unwrap_or("").to_string();
                fence = Some(Vec::new());
                continue;
            }
            if t.is_empty() {
                flush_para!();
                close_list!();
                continue;
            }
            if let Some((level, rest)) = heading(t) {
                flush_para!();
                close_list!();
                out.push(format!("<h{level}>{}</h{level}>", ProseRender::inline(rest)));
                continue;
            }
            if t == "---" || t == "***" {
                flush_para!();
                close_list!();
                out.push("<hr>".to_string());
                continue;
            }
            if let Some(rest) = t.strip_prefix("> ") {
                flush_para!();
                close_list!();
                out.push(format!("<blockquote>{}</blockquote>", ProseRender::inline(rest)));
                continue;
            }
            if let Some((kind, item)) = list_item(t) {
                flush_para!();
                if list != Some(kind) {
                    close_list!();
                    out.push(format!("<{kind}>"));
                    list = Some(kind);
                }
                let (box_html, body) = task_prefix(item);
                if box_html.is_empty() {
                    out.push(format!("<li>{}</li>", ProseRender::inline(body)));
                } else {
                    out.push(format!(
                        "<li class=\"task\">{}{}</li>",
                        box_html,
                        ProseRender::inline(body)
                    ));
                }
                continue;
            }
            close_list!();
            para.push(t.to_string());
        }
        if let Some(f) = fence {
            out.push(pre(&fence_lang, &f));
        }
        flush_para!();
        close_list!();
        out.join("\n")
    }

    /// `ProseRender.inline` — the inline markdown passes.
    pub fn inline(raw: &str) -> String {
        let mut s = Self::esc(raw);
        s = replace_code(&s);
        s = replace_images(&s);
        s = replace_links(&s);
        s = replace_bold(&s);
        s = replace_italic(&s);
        s = replace_del(&s);
        s
    }

    /// `ProseRender.page`: `prose.screen_html`, falling back to `basic`.
    pub fn page(
        src: &ProseSource,
        colors: &PopupColors,
        font: &str,
        size: f64,
        width: f64,
        cfg: &NotesConfig,
    ) -> String {
        let highlight = cfg
            .pdf_highlight
            .clone()
            .unwrap_or_else(|| Self::highlight_theme(colors).to_string());
        let pc = ProsePdfConfig {
            pandoc: crate::engines::ai_format::ai_setting("pandoc-bin", "/opt/homebrew/bin/pandoc"),
            engine: String::new(),
            css: cfg.pdf_css.clone(),
            filter: cfg.pdf_filter.clone(),
            highlight: Some(highlight),
            out_dir: None,
            theme_css: Some(Self::theme_css(colors)),
        };
        let helper_cfg = pc.to_json();
        let html = helper_call(
            "prose.screen_html",
            json!({"note": src.path, "config": helper_cfg}),
            120,
        )
        .and_then(|v| v.get("html").and_then(Value::as_str).map(str::to_string));

        let mut html = match html {
            Some(h) if !h.is_empty() => h,
            _ => {
                let css = helper_call("prose.css_content", json!({"config": helper_cfg}), 30)
                    .and_then(|v| v.get("css").and_then(Value::as_str).map(str::to_string))
                    .unwrap_or_default();
                format!(
                    "<!doctype html><html><head><meta charset=\"utf-8\">\n{css}\n</head><body>{}</body></html>",
                    Self::basic(&src.markdown)
                )
            }
        };

        let dir = Path::new(&src.path)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let mut override_html = format!(
            "<base href=\"file://{}/\">\n<style>\nbody {{ max-width: {}px !important; margin: 0 auto !important; padding: 28px 40px 56px !important; box-sizing: border-box !important; }}\n</style>",
            dir,
            width as i64
        );
        if cfg.copy_buttons {
            override_html.push_str(&format!(
                "\n<style id=\"ws-copy\">\n{}</style>",
                Self::copy_button_styles(colors, pc.css.as_deref())
            ));
        }
        let _ = (font, size);
        if let Some(pos) = find_ci(&html, "</head>") {
            html.insert_str(pos, &override_html);
        }
        html
    }

    /// `prose.pdf_export` through the helper.
    pub fn export_pdf(note: &str, cfg: &NotesConfig, colors: &PopupColors) -> PdfExport {
        let highlight = cfg
            .pdf_highlight
            .clone()
            .unwrap_or_else(|| Self::highlight_theme(colors).to_string());
        let pc = ProsePdfConfig {
            pandoc: crate::engines::ai_format::ai_setting("pandoc-bin", "/opt/homebrew/bin/pandoc"),
            engine: cfg.pdf_engine.clone().unwrap_or_default(),
            css: cfg.pdf_css.clone(),
            filter: cfg.pdf_filter.clone(),
            highlight: Some(highlight),
            out_dir: cfg.pdf_path.clone(),
            theme_css: Some(Self::theme_css(colors)),
        };
        let Some(v) = helper_call(
            "prose.pdf_export",
            json!({"note": note, "config": pc.to_json()}),
            300,
        ) else {
            return PdfExport {
                ok: false,
                out: None,
                error: Some("pdf export failed (python helper)".to_string()),
            };
        };
        if let Some(out) = v.get("out").and_then(Value::as_str) {
            PdfExport {
                ok: true,
                out: Some(out.to_string()),
                error: None,
            }
        } else {
            PdfExport {
                ok: false,
                out: None,
                error: v
                    .get("error")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| Some("pdf export failed".to_string())),
            }
        }
    }

    /// The `<base …>` + body-max-width + copy-button override spliced before
    /// `</head>`; exposed for tests.
    pub fn head_override(src: &ProseSource, width: f64, copy: Option<String>) -> String {
        let dir = Path::new(&src.path)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let mut s = format!(
            "<base href=\"file://{}/\">\n<style>\nbody {{ max-width: {}px !important; margin: 0 auto !important; padding: 28px 40px 56px !important; box-sizing: border-box !important; }}\n</style>",
            dir,
            width as i64
        );
        if let Some(css) = copy {
            s.push_str(&format!("\n<style id=\"ws-copy\">\n{css}</style>"));
        }
        s
    }

    /// `ProseView.bodyInner`.
    pub fn body_inner(html: &str) -> Option<String> {
        let b = find_ci(html, "<body")?;
        let gt_rel = html[b..].find('>')?;
        let start = b + gt_rel + 1;
        let e = find_ci(html, "</body>")?;
        if start > e {
            return None;
        }
        Some(html[start..e].to_string())
    }

    /// `ProseView.patchJS`.
    pub fn patch_js(body: &str) -> String {
        let lit = serde_json::to_string(body).unwrap_or_else(|_| "\"\"".to_string());
        format!(
            "var __y=window.scrollY;document.body.innerHTML={lit};window.scrollTo(0,__y);\
             window.__wsAddCopyButtons&&window.__wsAddCopyButtons();\
             window.__wsBuildOutline&&window.__wsBuildOutline();\
             window.__wsFindRefresh&&window.__wsFindRefresh();"
        )
    }
}

fn find_ci(hay: &str, needle: &str) -> Option<usize> {
    hay.to_lowercase().find(&needle.to_lowercase())
}

fn expand_tilde(p: &str) -> String {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{home}/{rest}");
        }
    }
    p.to_string()
}

fn heading(t: &str) -> Option<(usize, &str)> {
    let hashes = t.chars().take_while(|&c| c == '#').count();
    if (1..=6).contains(&hashes) && t.as_bytes().get(hashes) == Some(&b' ') {
        Some((hashes, &t[hashes + 1..]))
    } else {
        None
    }
}

fn list_item(t: &str) -> Option<(&'static str, &str)> {
    if let Some(rest) = t
        .strip_prefix("- ")
        .or_else(|| t.strip_prefix("* "))
        .or_else(|| t.strip_prefix("+ "))
    {
        return Some(("ul", rest));
    }
    let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 {
        let rest = &t[digits..];
        if rest.starts_with(". ") || rest.starts_with(") ") {
            return Some(("ol", &rest[2..]));
        }
    }
    None
}

fn task_prefix(item: &str) -> (String, &str) {
    let lower = item.to_lowercase();
    if item.starts_with("[ ] ") {
        ("<input type=\"checkbox\" disabled> ".to_string(), &item[4..])
    } else if lower.starts_with("[x] ") {
        (
            "<input type=\"checkbox\" disabled checked> ".to_string(),
            &item[4..],
        )
    } else {
        (String::new(), item)
    }
}

fn replace_code(s: &str) -> String {
    let b: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == '`' {
            let mut j = i + 1;
            while j < b.len() && b[j] != '`' {
                j += 1;
            }
            if j < b.len() && j > i + 1 {
                out.push_str("<code>");
                for c in &b[i + 1..j] {
                    out.push(*c);
                }
                out.push_str("</code>");
                i = j + 1;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// Shared `[alt](src)` scan. `image` = the leading `!`.
fn replace_links_impl(s: &str, image: bool) -> String {
    let b: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        let start = if image {
            if b[i] == '!' && i + 1 < b.len() && b[i + 1] == '[' {
                i + 1
            } else {
                out.push(b[i]);
                i += 1;
                continue;
            }
        } else if b[i] == '[' {
            i
        } else {
            out.push(b[i]);
            i += 1;
            continue;
        };
        // alt / text: no ']'
        let mut j = start + 1;
        while j < b.len() && b[j] != ']' {
            j += 1;
        }
        if j + 1 < b.len() && b[j] == ']' && b[j + 1] == '(' {
            let mut k = j + 2;
            while k < b.len() && b[k] != ')' && !b[k].is_whitespace() {
                k += 1;
            }
            if k < b.len() && b[k] == ')' {
                let content: String = b[start + 1..j].iter().collect();
                let target: String = b[j + 2..k].iter().collect();
                if image {
                    out.push_str(&format!("<img alt=\"{content}\" src=\"{target}\">"));
                } else {
                    out.push_str(&format!("<a href=\"{target}\">{content}</a>"));
                }
                i = k + 1;
                continue;
            }
        }
        // no match: emit the opener and resume right after it.
        if image {
            out.push('!');
        }
        out.push('[');
        i = start + 1;
    }
    out
}

fn replace_images(s: &str) -> String {
    replace_links_impl(s, true)
}
fn replace_links(s: &str) -> String {
    replace_links_impl(s, false)
}

fn replace_bold(s: &str) -> String {
    let b: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        if i + 1 < b.len() && b[i] == '*' && b[i + 1] == '*' {
            if let Some(close) = find_double(&b, i + 2) {
                let content: String = b[i + 2..close].iter().collect();
                if !content.is_empty() && !content.contains('*') {
                    out.push_str(&format!("<strong>{content}</strong>"));
                    i = close + 2;
                    continue;
                }
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

fn find_double(b: &[char], from: usize) -> Option<usize> {
    let mut i = from;
    while i + 1 < b.len() {
        if b[i] == '*' && b[i + 1] == '*' {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn replace_italic(s: &str) -> String {
    let b: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == '*' {
            let prev = if i > 0 { Some(b[i - 1]) } else { None };
            let prev_ok = prev.map(|c| c != '*' && !c.is_alphanumeric()).unwrap_or(true);
            let next_star = i + 1 < b.len() && b[i + 1] != '*';
            if prev_ok && next_star {
                let mut j = i + 1;
                while j < b.len() && b[j] != '*' {
                    j += 1;
                }
                if j < b.len() {
                    let content: String = b[i + 1..j].iter().collect();
                    let first = content.chars().next();
                    let last_ok = !b.get(j + 1).map(|&c| c == '*').unwrap_or(false);
                    if first.map(|c| !c.is_whitespace()).unwrap_or(false) && last_ok {
                        out.push_str(&format!("<em>{content}</em>"));
                        i = j + 1;
                        continue;
                    }
                }
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

fn replace_del(s: &str) -> String {
    let b: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        if i + 1 < b.len() && b[i] == '~' && b[i + 1] == '~' {
            let mut j = i + 2;
            while j + 1 < b.len() && !(b[j] == '~' && b[j + 1] == '~') {
                j += 1;
            }
            if j + 1 < b.len() {
                let content: String = b[i + 2..j].iter().collect();
                if !content.is_empty() && !content.contains('~') {
                    out.push_str(&format!("<del>{content}</del>"));
                    i = j + 2;
                    continue;
                }
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

// ---------------------------------------------------------------------------
// Prose view state (key handling + zoom)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchState {
    Idle,
    Typing,
    Active,
}

impl SearchState {
    pub fn as_str(self) -> &'static str {
        match self {
            SearchState::Idle => "idle",
            SearchState::Typing => "typing",
            SearchState::Active => "active",
        }
    }
    pub fn from_str(s: &str) -> SearchState {
        match s {
            "typing" => SearchState::Typing,
            "active" => SearchState::Active,
            _ => SearchState::Idle,
        }
    }
}

/// The modifier set intersected with the four the Swift code cares about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Mods {
    pub command: bool,
    pub control: bool,
    pub option: bool,
    pub shift: bool,
}

impl Mods {
    pub const NONE: Mods = Mods {
        command: false,
        control: false,
        option: false,
        shift: false,
    };
    pub const CMD: Mods = Mods {
        command: true,
        control: false,
        option: false,
        shift: false,
    };
    pub const CTRL: Mods = Mods {
        command: false,
        control: true,
        option: false,
        shift: false,
    };
    pub const SHIFT: Mods = Mods {
        command: false,
        control: false,
        option: false,
        shift: true,
    };
    pub fn any(self) -> bool {
        self.command || self.control || self.option || self.shift
    }
    pub fn is_empty(self) -> bool {
        !self.any()
    }
}

/// What `searchKey` wants the web view to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProseSearchKey {
    CloseFind,
    OpenFind,
    ScrollHalf { down: bool },
    GoTop,
    GoBottom,
    FindStep { back: bool },
    /// Handled but still forwarded to `super.sendEvent` (search is typing).
    PassThrough,
}

impl ProseSearchKey {
    /// `searchKey` → `true` unless `PassThrough`.
    pub fn consumed(self) -> bool {
        self != ProseSearchKey::PassThrough
    }

    /// The page script that performs this key (`None` for `PassThrough`).
    /// Shared by the in-place view and the pop-out window.
    pub fn js(self) -> Option<String> {
        Some(match self {
            ProseSearchKey::CloseFind => "window.__wsFindEsc&&window.__wsFindEsc()".to_string(),
            ProseSearchKey::OpenFind => "window.__wsFindOpen&&window.__wsFindOpen()".to_string(),
            ProseSearchKey::ScrollHalf { down } => format!(
                "window.scrollBy({{top:{}window.innerHeight/2}})",
                if down { "" } else { "-" }
            ),
            ProseSearchKey::GoTop => "window.scrollTo(0, 0)".to_string(),
            ProseSearchKey::GoBottom => {
                "window.scrollTo(0, document.documentElement.scrollHeight)".to_string()
            }
            ProseSearchKey::FindStep { back } => {
                format!("window.__wsFindStep&&window.__wsFindStep({back})")
            }
            ProseSearchKey::PassThrough => return None,
        })
    }
}

/// `ProseView` search + zoom state, AppKit-free.
#[derive(Clone, Debug)]
pub struct ProseViewState {
    pub search_state: SearchState,
    pub zoom: f64,
    last_g: Option<f64>,
}

/// Pixels one `j` / `k` scrolls in a reading page.
pub const PROSE_LINE_STEP: i64 = 60;

/// A vim-style scroll the reading page should perform. Shared by the in-place
/// reading view and the pop-out window so both take the same motions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProseScroll {
    /// `j` / `k`: scroll by this many pixels (positive = down).
    By(i64),
    /// `Ctrl+D` / `Ctrl+U`: half a window.
    HalfPage { down: bool },
    /// `gg`.
    Top,
    /// `G`.
    Bottom,
    /// The first `g` of a possible `gg`: swallowed, nothing to scroll yet.
    Pending,
}

impl ProseScroll {
    /// The JavaScript that performs the scroll (`None` for [`Self::Pending`]).
    pub fn js(self) -> Option<String> {
        match self {
            ProseScroll::By(dy) => Some(format!("window.scrollBy(0, {dy})")),
            ProseScroll::HalfPage { down } => Some(format!(
                "window.scrollBy({{top:{}window.innerHeight/2}})",
                if down { "" } else { "-" }
            )),
            ProseScroll::Top => Some("window.scrollTo(0, 0)".to_string()),
            ProseScroll::Bottom => {
                Some("window.scrollTo(0, document.documentElement.scrollHeight)".to_string())
            }
            ProseScroll::Pending => None,
        }
    }
}

impl Default for ProseViewState {
    fn default() -> Self {
        ProseViewState {
            search_state: SearchState::Idle,
            zoom: 1.0,
            last_g: None,
        }
    }
}

impl ProseViewState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_search_state(&mut self, s: &str) {
        self.search_state = SearchState::from_str(s);
    }

    /// `searchKey(code:mods:)`; `None` = not handled. `now` drives the 0.6 s
    /// double-`g` window.
    pub fn search_key(&mut self, code: u16, m: Mods, now: f64) -> Option<ProseSearchKey> {
        if code == KEY_ESC && m.is_empty() && self.search_state != SearchState::Idle {
            return Some(ProseSearchKey::CloseFind);
        }
        if code == KEY_F && (m == Mods::CMD || m == Mods::CTRL) {
            return Some(ProseSearchKey::OpenFind);
        }
        if self.search_state == SearchState::Typing {
            return Some(ProseSearchKey::PassThrough);
        }
        // Ctrl+D / Ctrl+U half-page scroll.
        if (code == 2 || code == 32) && m == Mods::CTRL {
            return Some(ProseSearchKey::ScrollHalf { down: code == 2 });
        }
        // g / G top/bottom with the 0.6 s double-tap.
        if code == 5 && (m.is_empty() || m == Mods::SHIFT) {
            if m == Mods::SHIFT {
                self.last_g = None;
                return Some(ProseSearchKey::GoBottom);
            }
            if let Some(t) = self.last_g {
                if now - t < 0.6 {
                    self.last_g = None;
                    return Some(ProseSearchKey::GoTop);
                }
            }
            self.last_g = Some(now);
            return Some(ProseSearchKey::PassThrough);
        }
        if code == KEY_SLASH && m.is_empty() {
            return Some(ProseSearchKey::OpenFind);
        }
        if code == KEY_N
            && (m.is_empty() || m == Mods::SHIFT)
            && self.search_state == SearchState::Active
        {
            return Some(ProseSearchKey::FindStep {
                back: m == Mods::SHIFT,
            });
        }
        None
    }

    /// The vim motions a reading page takes with no find bar involved: `j` /
    /// `k`, `gg` / `G`, `Ctrl+D` / `Ctrl+U`. `None` = not a scroll key. The
    /// pop-out window (no find bar, no host router) is driven by this alone;
    /// `gg` uses the same 0.6 s double-tap window as the in-place view.
    pub fn scroll_key(&mut self, code: u16, m: Mods, now: f64) -> Option<ProseScroll> {
        use crate::ui::popup::{KEY_J, KEY_K};
        if m.is_empty() && (code == KEY_J || code == KEY_K) {
            self.last_g = None;
            return Some(ProseScroll::By(if code == KEY_J { PROSE_LINE_STEP } else { -PROSE_LINE_STEP }));
        }
        // Only the scroll arms of `search_key`; its find / `/` / `n` keys do
        // not exist in a page without the find bar.
        match self.search_key(code, m, now)? {
            ProseSearchKey::ScrollHalf { down } => Some(ProseScroll::HalfPage { down }),
            ProseSearchKey::GoTop => Some(ProseScroll::Top),
            ProseSearchKey::GoBottom => Some(ProseScroll::Bottom),
            ProseSearchKey::PassThrough if code == 5 => Some(ProseScroll::Pending),
            _ => None,
        }
    }

    pub fn apply_zoom(&mut self, z: f64) -> f64 {
        self.zoom = z.clamp(PROSE_MIN_ZOOM, PROSE_MAX_ZOOM);
        self.zoom
    }
    pub fn zoom_by(&mut self, factor: f64) -> f64 {
        self.apply_zoom(self.zoom * factor)
    }
    pub fn reset_zoom(&mut self) -> f64 {
        self.apply_zoom(1.0)
    }
}

/// The Swift `prose` subprocess command line. `ProseProcess.run` parses it.
#[derive(Clone, Debug, PartialEq)]
pub struct ProseLaunchOptions {
    pub colors: PopupColors,
    pub font: String,
    pub size: f64,
    pub width: f64,
    pub path: String,
}

impl ProseLaunchOptions {
    pub fn parse(args: &[String]) -> ProseLaunchOptions {
        let mut colors = PopupColors::default();
        let mut font = String::new();
        let mut size = 19.0;
        let mut width = 900.0;
        let mut path = String::new();
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "--colors" if i + 1 < args.len() => {
                    let cs: Vec<Rgba> = args[i + 1]
                        .split(',')
                        .filter_map(parse_hex_color)
                        .collect();
                    if cs.len() == 11 {
                        if let Some(pal) = PopupPalette::from_slice(&cs[6..]) {
                            colors = PopupColors {
                                background: cs[0],
                                border: cs[1],
                                text: cs[2],
                                dim: cs[3],
                                highlight: cs[4],
                                accent: cs[5],
                                palette: pal,
                            };
                        }
                    }
                    i += 1;
                }
                "--font" if i + 1 < args.len() => {
                    font = args[i + 1].clone();
                    i += 1;
                }
                "--size" if i + 1 < args.len() => {
                    size = args[i + 1].parse().unwrap_or(19.0);
                    i += 1;
                }
                "--width" if i + 1 < args.len() => {
                    width = args[i + 1].parse().unwrap_or(900.0);
                    i += 1;
                }
                other => path = other.to_string(),
            }
            i += 1;
        }
        ProseLaunchOptions {
            colors,
            font,
            size,
            width,
            path,
        }
    }

    pub fn color_hex(c: Rgba) -> String {
        // `ProseProcess.hex` — always 8 digits `RRGGBBAA` (matches
        // `parse_hex_color`, unlike `HexColor::format` which drops a == 255).
        let b = |x: f64| (x * 255.0).round().clamp(0.0, 255.0) as i64;
        format!("{:02X}{:02X}{:02X}{:02X}", b(c.r), b(c.g), b(c.b), b(c.a))
    }

    /// `ProseProcess.launch`'s options for a bare path: the theme colors plus
    /// the `[notes]` prose font/size/width.
    pub fn for_path(path: &str) -> ProseLaunchOptions {
        let cfg = NotesConfig::load();
        ProseLaunchOptions {
            colors: crate::ui::theme::PopupThemeDefaults::colors(),
            font: cfg.prose_font,
            size: cfg.prose_font_size,
            width: cfg.prose_width,
            path: path.to_string(),
        }
    }

    /// The 11 `--colors` hex codes: the six base colors then the palette, in
    /// `ProseProcess.launch`'s `[background, border, text, dim, highlight,
    /// accent] + palette.all` order.
    pub fn colors_csv(&self) -> String {
        let mut out: Vec<String> = [
            self.colors.background,
            self.colors.border,
            self.colors.text,
            self.colors.dim,
            self.colors.highlight,
            self.colors.accent,
        ]
        .iter()
        .map(|c| Self::color_hex(*c))
        .collect();
        out.extend(self.colors.palette.all().iter().map(|c| Self::color_hex(*c)));
        out.join(",")
    }

    /// The `prose` subprocess argv after the executable name (round-trips
    /// through [`ProseLaunchOptions::parse`]).
    pub fn to_args(&self) -> Vec<String> {
        vec![
            "--colors".to_string(),
            self.colors_csv(),
            "--font".to_string(),
            self.font.clone(),
            "--size".to_string(),
            format!("{}", self.size),
            "--width".to_string(),
            format!("{}", self.width),
            self.path.clone(),
        ]
    }
}

fn parse_hex_color(h: &str) -> Option<Rgba> {
    if h.len() != 8 {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    Some(Rgba::new(
        ((v >> 24) & 255) as f64 / 255.0,
        ((v >> 16) & 255) as f64 / 255.0,
        ((v >> 8) & 255) as f64 / 255.0,
        (v & 255) as f64 / 255.0,
    ))
}

/// Standalone `ProseWindow` skeleton (a floating WKWebView panel).
pub struct ProseWindow;

impl ProseWindow {
    /// `ProseWindow.show` — builds/orders the floating panel.
    pub fn show(opts: &ProseLaunchOptions) {
        #[cfg(target_os = "macos")]
        macos::show(opts);
        #[cfg(not(target_os = "macos"))]
        {
            let _ = opts;
        }
    }
}

/// `ProseProcess.launch` — spawn `kitchen-sink prose …` (or fall back to the
/// in-process [`ProseWindow`]).
pub struct ProseProcess;

impl ProseProcess {
    /// `ProseProcess.run` — the `kitchen-sink prose` CLI entry; prints usage and
    /// exits 2 when the file is missing.
    pub fn run(opts: &ProseLaunchOptions) -> Result<(), String> {
        if !Path::new(&opts.path).exists() {
            return Err(
                "usage: kitchen-sink prose [--colors …] [--font F] [--size N] [--width N] FILE.md"
                    .to_string(),
            );
        }
        ProseWindow::show(opts);
        Ok(())
    }

    /// Spawn `<current_exe> prose …` detached; reuse a live child per path and
    /// fall back to the in-process window when the spawn fails.
    pub fn launch(path: &str) {
        #[cfg(target_os = "macos")]
        macos::launch(path);
        #[cfg(not(target_os = "macos"))]
        {
            let _ = path;
        }
    }
}

/// `popOutProse`'s guard: the selected note path, when one is selected and the
/// file still exists. `None` is a safe no-op — nothing is launched.
pub fn prose_launch_target(
    selected: Option<&str>,
    exists: impl Fn(&str) -> bool,
) -> Option<String> {
    let p = selected.filter(|p| !p.is_empty())?;
    if exists(p) {
        Some(p.to_string())
    } else {
        None
    }
}

/// The height (pt) of the draggable title strip at a prose pop-out's top. The
/// page covers the transparent titlebar, so this band carries the drag and the
/// double-click-to-zoom gesture (Swift `ProseWindow.sendEvent`'s
/// `frame.height - 28`).
pub const PROSE_TITLE_STRIP: f64 = 28.0;

/// Whether a point (AppKit window coordinates, y up) lies in a prose pop-out's
/// title strip.
pub fn prose_in_title_strip(location_y: f64, frame_height: f64) -> bool {
    location_y > frame_height - PROSE_TITLE_STRIP
}

/// `ProseWindow.sendEvent`'s double-click arm, as pure frame math: with no
/// saved frame, remember `current` and fill `visible`; with one, restore it and
/// clear the save. Returns the frame to apply and the new saved frame.
pub fn prose_zoom_toggle(
    current: crate::platform::screens::Rect,
    saved: Option<crate::platform::screens::Rect>,
    visible: crate::platform::screens::Rect,
) -> (
    crate::platform::screens::Rect,
    Option<crate::platform::screens::Rect>,
) {
    match saved {
        Some(restored) => (restored, None),
        None => (visible, Some(current)),
    }
}

fn now_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// The stateful notes surface the shared host embeds: the sidebar + editor tree
/// plus two toggleable drawers (terminal, browser) and editor focus.
#[cfg(target_os = "macos")]
pub use macos::NotesSurface;

/// The embeddable content view for the shared host window.
///
/// A flipped root with the NOTES sidebar (one row per open tab: title, folder
/// and last-write) on the left and an `NSTextView` editor for the selected note
/// on the right. The tab list comes from `[notes] paths`; a file that no longer
/// exists is skipped and the editor stays empty. `None` off the main thread.
///
/// Delegates to [`NotesSurface::build`]; a caller that keeps only the returned
/// view gets a surface whose drawers are never toggled.
#[cfg(target_os = "macos")]
pub fn build_content(
    mtm: objc2::MainThreadMarker,
) -> Option<objc2::rc::Retained<objc2_app_kit::NSView>> {
    NotesSurface::build(mtm).map(|surface| surface.content_view())
}

/// The AppKit surfaces of the notes family: the floating [`ProseWindow`]'s
/// `WKWebView` panel, the `prose` child-process launch, and the embeddable
/// [`build_content`] tree.
#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObject, ProtocolObject};
    use objc2::{define_class, msg_send, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, Message};
    use objc2_app_kit::{
        NSAccessibility, NSAutoresizingMaskOptions, NSBackingStoreType, NSDraggingInfo, NSFont,
        NSImage, NSImageScaling, NSImageView, NSNormalWindowLevel, NSPasteboard, NSScrollView,
        NSTextAttachment, NSTextField, NSTextView, NSView, NSWindow, NSWindowButton,
        NSWindowCollectionBehavior, NSWindowStyleMask, NSWindowTitleVisibility,
    };
    use objc2_foundation::{
        ns_string, NSAttributedString, NSMutableAttributedString, NSNumber, NSObjectProtocol,
        NSPoint, NSRange, NSRect, NSSize, NSString,
    };
    use objc2_web_kit::{WKWebView, WKWebViewConfiguration};
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;
    use std::rc::Rc;

    pub struct NotesFlippedIvars {
        /// The rail column: the tree card over the Open sidebar. Re-split on
        /// every resize so the tree stays only as tall as its rows.
        rail: RefCell<Option<(Retained<crate::ui::tree_view::TreeView>, Retained<crate::ui::sidebar::SidebarView>)>>,
    }

    define_class!(
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSNotesFlippedView"]
        #[ivars = NotesFlippedIvars]
        pub struct NotesFlippedView;

        impl NotesFlippedView {
            #[unsafe(method(isFlipped))]
            fn is_flipped(&self) -> bool {
                true
            }

            #[unsafe(method(setFrameSize:))]
            fn set_frame_size(&self, size: NSSize) {
                let _: () = unsafe { msg_send![super(self), setFrameSize: size] };
                self.layout_rail();
            }
        }

        unsafe impl NSObjectProtocol for NotesFlippedView {}
    );

    impl NotesFlippedView {
        fn set_rail(&self, tree: &Retained<crate::ui::tree_view::TreeView>, sidebar: &Retained<crate::ui::sidebar::SidebarView>) {
            *self.ivars().rail.borrow_mut() = Some((tree.clone(), sidebar.clone()));
        }

        /// Stack tree over sidebar inside the current height.
        fn layout_rail(&self) {
            let guard = self.ivars().rail.borrow();
            let Some((tree, sidebar)) = guard.as_ref() else { return };
            let h = self.frame().size.height;
            let w = sidebar.frame().size.width;
            let show = !tree.isHidden();
            let (tree_h, open_h) = crate::views::notes_tree::rail_split(h, show, tree.wanted_height());
            tree.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(w, tree_h)));
            sidebar.setFrame(NSRect::new(NSPoint::new(0.0, tree_h), NSSize::new(w, open_h)));
        }

        fn new(mtm: MainThreadMarker) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(NotesFlippedIvars { rail: RefCell::new(None) });
            unsafe {
                msg_send![
                    super(this),
                    initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0))
                ]
            }
        }
    }

    pub struct NotesTextViewIvars {
        /// Right-click → the editor menu, with the clicked character index.
        on_menu: RefCell<Option<Rc<dyn Fn(usize)>>>,
        /// Cmd+V / an image drop → save next to the note and insert
        /// (`PopupTextView.onPasteImage`; wired by the host).
        on_paste_image: RefCell<Option<Rc<dyn Fn(Retained<NSImage>)>>>,
        /// A click on an image attachment → the FilePopup
        /// (`PopupTextView.onOpenImage`).
        on_open_image: RefCell<Option<Rc<dyn Fn(String)>>>,
    }

    define_class!(
        /// `PopupTextView`: the plain notes editor. Right-click shows the
        /// notes menu (`menu(for:)`), Cmd+V / drops with an image go through
        /// `onPasteImage`, a click on an inline image opens the FilePopup.
        #[unsafe(super(NSTextView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSNotesTextView"]
        #[ivars = NotesTextViewIvars]
        pub struct NotesTextView;

        impl NotesTextView {
            #[unsafe(method(rightMouseDown:))]
            fn right_mouse_down(&self, e: &objc2_app_kit::NSEvent) {
                let cb = self.ivars().on_menu.borrow().clone();
                let Some(cb) = cb else {
                    let _: () = unsafe { msg_send![super(self), rightMouseDown: e] };
                    return;
                };
                let p = self.convertPoint_fromView(e.locationInWindow(), None);
                cb(self.characterIndexForInsertionAtPoint(p));
            }

            /// `PopupTextView.paste(_:)`: an image on the clipboard wins.
            #[unsafe(method(paste:))]
            fn paste(&self, sender: Option<&AnyObject>) {
                let pb = NSPasteboard::generalPasteboard();
                if let Some(img) = crate::views::notes_vim::pasteboard_image(&pb) {
                    if img.size().width > 1.0 {
                        let cb = self.ivars().on_paste_image.borrow().clone();
                        if let Some(cb) = cb {
                            cb(img);
                            return;
                        }
                    }
                }
                let _: () = unsafe { msg_send![super(self), paste: sender] };
            }

            /// `PopupTextView.mouseDown`: a single click on an image
            /// attachment opens the FilePopup; anything else is normal.
            #[unsafe(method(mouseDown:))]
            fn mouse_down(&self, e: &objc2_app_kit::NSEvent) {
                if e.clickCount() == 1 {
                    let p = self.convertPoint_fromView(e.locationInWindow(), None);
                    if let Some(rel) = self.attachment_rel_at_point(p) {
                        let cb = self.ivars().on_open_image.borrow().clone();
                        if let Some(cb) = cb {
                            cb(rel);
                            return;
                        }
                    }
                }
                let _: () = unsafe { msg_send![super(self), mouseDown: e] };
            }

            /// `PopupTextView.draggingEntered`: an image drop is a copy.
            #[unsafe(method(draggingEntered:))]
            fn dragging_entered(
                &self,
                sender: &ProtocolObject<dyn objc2_app_kit::NSDraggingInfo>,
            ) -> objc2_app_kit::NSDragOperation {
                let pb = sender.draggingPasteboard();
                if crate::views::notes_vim::pasteboard_image(&pb).is_some() {
                    return objc2_app_kit::NSDragOperation::Copy;
                }
                unsafe { msg_send![super(self), draggingEntered: sender] }
            }

            /// `PopupTextView.performDragOperation`: save + insert the image.
            #[unsafe(method(performDragOperation:))]
            fn perform_drag_operation(
                &self,
                sender: &ProtocolObject<dyn objc2_app_kit::NSDraggingInfo>,
            ) -> objc2::runtime::Bool {
                let pb = sender.draggingPasteboard();
                if let Some(img) = crate::views::notes_vim::pasteboard_image(&pb) {
                    let cb = self.ivars().on_paste_image.borrow().clone();
                    if let Some(cb) = cb {
                        cb(img);
                        return true.into();
                    }
                }
                unsafe { msg_send![super(self), performDragOperation: sender] }
            }
        }

        unsafe impl NSObjectProtocol for NotesTextView {}
    );

    impl NotesTextView {
        fn new(mtm: MainThreadMarker) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(NotesTextViewIvars {
                on_menu: RefCell::new(None),
                on_paste_image: RefCell::new(None),
                on_open_image: RefCell::new(None),
            });
            let view: Retained<Self> = unsafe {
                msg_send![super(this), initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0))]
            };
            // `PopupTextView` accepted image drops (`draggingEntered:` /
            // `performDragOperation:`); NSTextView's defaults cover text.
            let types = unsafe {
                [
                    objc2_app_kit::NSPasteboardTypePNG,
                    objc2_app_kit::NSPasteboardTypeTIFF,
                    objc2_app_kit::NSPasteboardTypeFileURL,
                ]
            };
            let arr = objc2_foundation::NSArray::from_slice(&types);
            view.registerForDraggedTypes(&arr);
            view
        }

        /// The markdown-relative target of the attachment covering `idx`.
        pub fn attachment_rel_at(&self, idx: usize) -> Option<String> {
            let Some(storage) = (unsafe { self.textStorage() }) else {
                return None;
            };
            if idx >= storage.length() {
                return None;
            }
            let attrs =
                unsafe { storage.attributesAtIndex_effectiveRange(idx, std::ptr::null_mut()) };
            let name = unsafe { objc2_app_kit::NSAttachmentAttributeName };
            let obj = attrs.objectForKey(name)?;
            let rel = obj
                .downcast_ref::<NoteAttachment>()
                .map(|a| a.ivars().rel.borrow().clone());
            rel
        }

        /// `PopupTextView.imagePath(at:)`: the glyph under `point` when it is
        /// an image attachment.
        fn attachment_rel_at_point(&self, point: NSPoint) -> Option<String> {
            let Some(lm) = (unsafe { self.layoutManager() }) else {
                return None;
            };
            let Some(tc) = (unsafe { self.textContainer() }) else {
                return None;
            };
            let o = self.textContainerOrigin();
            let p = NSPoint::new(point.x - o.x, point.y - o.y);
            let g = unsafe {
                lm.glyphIndexForPoint_inTextContainer_fractionOfDistanceThroughGlyph(
                    p,
                    &tc,
                    std::ptr::null_mut(),
                )
            };
            let rect = lm.boundingRectForGlyphRange_inTextContainer(NSRange::new(g, 1), &tc);
            if !(p.x >= rect.origin.x
                && p.x <= rect.origin.x + rect.size.width
                && p.y >= rect.origin.y
                && p.y <= rect.origin.y + rect.size.height)
            {
                return None;
            }
            let idx = lm.characterIndexForGlyphAtIndex(g);
            self.attachment_rel_at(idx)
        }
    }

    pub struct NoteAttachmentIvars {
        /// The markdown link this attachment serializes back to
        /// (`![](rel)`; Swift's `attachmentPaths` dictionary).
        rel: RefCell<String>,
    }

    define_class!(
        /// `NSTextAttachment` that remembers its markdown target.
        #[unsafe(super(NSTextAttachment))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSNoteAttachment"]
        #[ivars = NoteAttachmentIvars]
        pub struct NoteAttachment;

        impl NoteAttachment {}

        unsafe impl NSObjectProtocol for NoteAttachment {}
    );

    impl NoteAttachment {
        fn create(mtm: MainThreadMarker, img: &NSImage, rel: &str, max_w: f64) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(NoteAttachmentIvars {
                rel: RefCell::new(rel.to_string()),
            });
            let att: Retained<Self> = unsafe { msg_send![super(this), init] };
            att.setImage(Some(img));
            let mut size = img.size();
            if max_w > 0.0 && size.width > max_w {
                size.height = size.height * max_w / size.width;
                size.width = max_w;
            }
            att.setBounds(NSRect::new(NSPoint::new(0.0, 0.0), size));
            att
        }
    }

    /// The `![alt](target)` spans of `s` (the scan `setEditorMarkdown` does
    /// with an `NSRegularExpression` in Swift): byte range + target.
    fn image_links(s: &str) -> Vec<(std::ops::Range<usize>, String)> {
        let b: Vec<char> = s.chars().collect();
        let mut out = Vec::new();
        let mut i = 0;
        while i < b.len() {
            if b[i] == '!' && i + 1 < b.len() && b[i + 1] == '[' {
                let mut j = i + 2;
                while j < b.len() && b[j] != ']' {
                    j += 1;
                }
                if j + 1 < b.len() && b[j] == ']' && b[j + 1] == '(' {
                    let mut k = j + 2;
                    while k < b.len() && b[k] != ')' {
                        k += 1;
                    }
                    if k < b.len() {
                        let inner: String = b[j + 2..k].iter().collect();
                        let target = inner.trim();
                        let target = target
                            .split_once(" \"")
                            .map(|(t, _)| t)
                            .unwrap_or(target)
                            .trim();
                        let target = target.trim_start_matches('<').trim_end_matches('>');
                        if !target.is_empty() {
                            let start: usize = b[..i].iter().map(|c| c.len_utf8()).sum();
                            let end: usize = b[..=k].iter().map(|c| c.len_utf8()).sum();
                            out.push((start..end, target.to_string()));
                        }
                        i = k + 1;
                        continue;
                    }
                }
            }
            i += 1;
        }
        out
    }

    /// `editorMarkdown`: attachment runs serialize back to `![](rel)\n`.
    fn editor_markdown(editor: &NSTextView) -> String {
        let Some(storage) = (unsafe { editor.textStorage() }) else {
            return editor.string().to_string();
        };
        let len = storage.length();
        let mut out = String::new();
        let mut i = 0usize;
        while i < len {
            let mut eff = NSRange::new(0, 0);
            let attrs = unsafe { storage.attributesAtIndex_effectiveRange(i, &mut eff) };
            let run = if eff.length == 0 { 1 } else { eff.length };
            let rel = attrs
                .objectForKey(unsafe { objc2_app_kit::NSAttachmentAttributeName })
                .and_then(|o| {
                    o.downcast_ref::<NoteAttachment>()
                        .map(|a| a.ivars().rel.borrow().clone())
                });
            match rel {
                Some(rel) => out.push_str(&format!("![]({rel})\n")),
                None => {
                    let seg = storage.attributedSubstringFromRange(NSRange::new(i, run));
                    out.push_str(&seg.string().to_string());
                }
            }
            i += run;
        }
        out
    }

    /// The plain-text attributes (`restyleEditor`): the editor font + text
    /// colour over every run.
    fn restyle_editor(editor: &NSTextView, colors: PopupColors) {
        let Some(storage) = (unsafe { editor.textStorage() }) else {
            return;
        };
        let font = editor.font().unwrap_or_else(|| {
            crate::ui::style::system_font(13.0, crate::ui::style::Weight::Regular)
        });
        let attrs = crate::ui::style::attrs(&font, colors.text);
        let range = NSRange::new(0, storage.length());
        unsafe {
            storage.addAttributes_range(&attrs, range);
            editor.setTypingAttributes(&attrs);
        }
    }

    /// `setEditorMarkdown(_:baseDir:)`: load `s` with image attachments —
    /// every readable `![alt](target)` becomes an inline image followed by a
    /// newline; unreadable ones stay literal markdown.
    fn set_editor_markdown(
        editor: &NSTextView,
        scroll: &NSScrollView,
        colors: PopupColors,
        s: &str,
        base_dir: &str,
    ) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        let Some(storage) = (unsafe { editor.textStorage() }) else {
            return;
        };
        let font = editor.font().unwrap_or_else(|| {
            crate::ui::style::system_font(13.0, crate::ui::style::Weight::Regular)
        });
        let attrs = crate::ui::style::attrs(&font, colors.text);
        let max_w = scroll.bounds().size.width;
        let home = std::env::var("HOME").unwrap_or_default();
        let out = NSMutableAttributedString::initWithString(
            NSMutableAttributedString::alloc(),
            &NSString::from_str(""),
        );
        let mut last = 0usize;
        for (range, target) in image_links(s) {
            if range.start > last {
                let a = unsafe {
                    NSAttributedString::initWithString_attributes(
                        NSAttributedString::alloc(),
                        &NSString::from_str(&s[last..range.start]),
                        Some(&attrs),
                    )
                };
                out.appendAttributedString(&a);
            }
            let path = if target.starts_with('/') {
                target.clone()
            } else if let Some(rest) = target.strip_prefix("~/") {
                format!("{home}/{rest}")
            } else {
                format!("{base_dir}/{}", target.trim_start_matches("./"))
            };
            let img = NSImage::initWithContentsOfFile(
                NSImage::alloc(),
                &NSString::from_str(&path),
            );
            if let Some(img) = img {
                if img.size().width > 0.0 && img.size().height > 0.0 {
                    let att = NoteAttachment::create(mtm, &img, &target, max_w);
                    let piece = NSMutableAttributedString::initWithString(
                        NSMutableAttributedString::alloc(),
                        &NSString::from_str("\u{FFFC}\n"),
                    );
                    unsafe {
                        piece.addAttribute_value_range(
                            objc2_app_kit::NSAttachmentAttributeName,
                            as_any(&*att),
                            NSRange::new(0, 1),
                        )
                    };
                    out.appendAttributedString(&piece);
                    last = range.end;
                    continue;
                }
            }
            let a = unsafe {
                NSAttributedString::initWithString_attributes(
                    NSAttributedString::alloc(),
                    &NSString::from_str(&s[range.clone()]),
                    Some(&attrs),
                )
            };
            out.appendAttributedString(&a);
            last = range.end;
        }
        if last < s.len() {
            let a = unsafe {
                NSAttributedString::initWithString_attributes(
                    NSAttributedString::alloc(),
                    &NSString::from_str(&s[last..]),
                    Some(&attrs),
                )
            };
            out.appendAttributedString(&a);
        }
        storage.setAttributedString(&out);
        unsafe { editor.setTypingAttributes(&attrs) };
        editor.setNeedsDisplay(true);
    }

    /// `PopupWindow.cellSize(vimFont)`: the vim pane's cell (W advance ×
    /// ascent+descent+leading), used for `g:ws_cell_h` and the image rows.
    fn cell_size_for(name: &str, size: f64) -> (f64, f64) {
        let Some(font) = NSFont::fontWithName_size(&NSString::from_str(name), size) else {
            return (8.0, 16.0);
        };
        let h = (font.ascender() - font.descender() + font.leading()).ceil();
        let w = crate::ui::style::text_size("W", &font).0.ceil();
        (w.max(1.0), h.max(1.0))
    }

    // -- vim pane inline images (`VimImageOverlay`) -------------------------

    /// One `{"path","row","rows"}` entry of the nvim sidecar.
    #[derive(Clone, Debug, PartialEq)]
    pub struct VimImageItem {
        pub path: String,
        pub row: i64,
        pub rows: i64,
    }

    pub struct VimImageOverlayIvars {
        cell: Cell<(f64, f64)>,
        text_rows: Cell<i64>,
        items: RefCell<Vec<VimImageItem>>,
        views: RefCell<Vec<Retained<NSView>>>,
        hits: RefCell<Vec<(NSRect, String)>>,
        cache: RefCell<HashMap<String, (f64, Retained<NSImage>)>>,
        on_open: RefCell<Option<Rc<dyn Fn(String)>>>,
        /// The sidecar mtime of the last reload.
        mtime: Cell<Option<std::time::SystemTime>>,
    }

    define_class!(
        /// `VimImageOverlay`: draws the images nvim's `init.lua` reserved
        /// virtual lines for, over the SwiftTerm pane; click opens the
        /// FilePopup, right-click the image menu. Empty areas are
        /// click-through (`hitTest:` returns nil).
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSVimImageOverlay"]
        #[ivars = VimImageOverlayIvars]
        pub struct VimImageOverlay;

        impl VimImageOverlay {
            #[unsafe(method(isFlipped))]
            fn is_flipped(&self) -> bool {
                true
            }

            #[unsafe(method(hitTest:))]
            fn hit_test(&self, point: NSPoint) -> *mut NSView {
                let p = if let Some(sv) = unsafe { self.superview() } {
                    self.convertPoint_fromView(point, Some(&sv))
                } else {
                    point
                };
                let hit = self
                    .ivars()
                    .hits
                    .borrow()
                    .iter()
                    .any(|(r, _)| rect_contains(*r, p));
                if hit {
                    self as *const Self as *mut NSView
                } else {
                    std::ptr::null_mut()
                }
            }

            #[unsafe(method(mouseDown:))]
            fn mouse_down(&self, e: &objc2_app_kit::NSEvent) {
                if let Some(path) = self.path_at(e) {
                    let cb = self.ivars().on_open.borrow().clone();
                    if let Some(cb) = cb {
                        cb(path);
                    }
                }
            }

            #[unsafe(method(rightMouseDown:))]
            fn right_mouse_down(&self, e: &objc2_app_kit::NSEvent) {
                if let Some(path) = self.path_at(e) {
                    vim_image_menu(&path, self);
                }
            }

            /// `VimImageOverlay.setFrameSize`: re-fit the images to the pane.
            #[unsafe(method(setFrameSize:))]
            fn set_frame_size(&self, size: NSSize) {
                let _: () = unsafe { msg_send![super(self), setFrameSize: size] };
                self.relayout();
            }
        }

        unsafe impl NSObjectProtocol for VimImageOverlay {}
    );

    impl VimImageOverlay {
        pub fn create(mtm: MainThreadMarker, frame: NSRect, cell: (f64, f64)) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(VimImageOverlayIvars {
                cell: Cell::new(cell),
                text_rows: Cell::new(0),
                items: RefCell::new(Vec::new()),
                views: RefCell::new(Vec::new()),
                hits: RefCell::new(Vec::new()),
                cache: RefCell::new(HashMap::new()),
                on_open: RefCell::new(None),
                mtime: Cell::new(None),
            });
            let view: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
            view.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable
                    | NSAutoresizingMaskOptions::ViewHeightSizable,
            );
            view.setWantsLayer(true);
            if let Some(l) = view.layer() {
                l.setMasksToBounds(true);
            }
            view
        }

        pub fn set_on_open(&self, cb: Rc<dyn Fn(String)>) {
            *self.ivars().on_open.borrow_mut() = Some(cb);
        }

        pub fn set_cell(&self, cell: (f64, f64)) {
            if self.ivars().cell.get() != cell {
                self.ivars().cell.set(cell);
                self.relayout();
            }
        }

        /// `reloadVimImages`: replace the drawn set (nvim sidecar JSON).
        pub fn set_state(&self, text_rows: i64, items: Vec<VimImageItem>) {
            let changed = self.ivars().text_rows.get() != text_rows
                || *self.ivars().items.borrow() != items;
            if !changed {
                return;
            }
            self.ivars().text_rows.set(text_rows);
            *self.ivars().items.borrow_mut() = items;
            self.relayout();
        }

        fn path_at(&self, e: &objc2_app_kit::NSEvent) -> Option<String> {
            let p = self.convertPoint_fromView(e.locationInWindow(), None);
            self.ivars()
                .hits
                .borrow()
                .iter()
                .find(|(r, _)| rect_contains(*r, p))
                .map(|(_, path)| path.clone())
        }

        fn image(&self, path: &str) -> Option<Retained<NSImage>> {
            let mtime = std::fs::metadata(path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0);
            if let Some((m, img)) = self.ivars().cache.borrow().get(path) {
                if *m == mtime {
                    return Some(img.clone());
                }
            }
            let img = NSImage::initWithContentsOfFile(NSImage::alloc(), &NSString::from_str(path))?;
            self.ivars()
                .cache
                .borrow_mut()
                .insert(path.to_string(), (mtime, img.clone()));
            Some(img)
        }

        fn relayout(&self) {
            let old: Vec<Retained<NSView>> = self.ivars().views.borrow_mut().drain(..).collect();
            for v in old {
                v.removeFromSuperview();
            }
            self.ivars().hits.borrow_mut().clear();
            let Some(mtm) = MainThreadMarker::new() else { return };
            let (_, ch) = self.ivars().cell.get();
            let limit = self.ivars().text_rows.get() as f64 * ch;
            let bounds_w = self.bounds().size.width;
            let mut views = Vec::new();
            let mut hits = Vec::new();
            for it in self.ivars().items.borrow().iter() {
                let Some(img) = self.image(&it.path) else { continue };
                let is = img.size();
                if is.width <= 0.0 || is.height <= 0.0 {
                    continue;
                }
                let top = it.row as f64 * ch + 2.0;
                if top >= limit {
                    continue;
                }
                let box_h = it.rows as f64 * ch - 4.0;
                let box_w = (bounds_w - 16.0).max(40.0);
                let scale = 1.0f64.min(box_h / is.height).min(box_w / is.width);
                let size = NSSize::new((is.width * scale).floor(), (is.height * scale).floor());
                let iv = NSImageView::initWithFrame(
                    NSImageView::alloc(mtm),
                    NSRect::new(NSPoint::new(2.0, top), size),
                );
                iv.setImage(Some(&img));
                iv.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
                iv.setWantsLayer(true);
                if let Some(l) = iv.layer() {
                    l.setCornerRadius(6.0);
                    l.setMasksToBounds(true);
                }
                if top + size.height > limit {
                    let visible = limit - top;
                    if visible <= 4.0 {
                        continue;
                    }
                    let clip = NSView::initWithFrame(
                        NSView::alloc(mtm),
                        NSRect::new(NSPoint::new(2.0, top), NSSize::new(size.width, visible)),
                    );
                    clip.setWantsLayer(true);
                    if let Some(l) = clip.layer() {
                        l.setMasksToBounds(true);
                    }
                    iv.setFrameOrigin(NSPoint::new(0.0, 0.0));
                    clip.addSubview(&iv);
                    self.addSubview(&clip);
                    hits.push((clip.frame(), it.path.clone()));
                    views.push(clip);
                    continue;
                }
                self.addSubview(&iv);
                hits.push((iv.frame(), it.path.clone()));
                views.push(iv.into_super().into_super());
            }
            *self.ivars().views.borrow_mut() = views;
            *self.ivars().hits.borrow_mut() = hits;
        }
    }

    fn rect_contains(r: NSRect, p: NSPoint) -> bool {
        p.x >= r.origin.x
            && p.x <= r.origin.x + r.size.width
            && p.y >= r.origin.y
            && p.y <= r.origin.y + r.size.height
    }

    /// `PopupWindow.imageMenu(_:)` — the vim image's right-click menu.
    fn vim_image_menu(path: &str, view: &NSView) {
        use crate::ui::menu_kit::{popup_context, MenuEntry};
        let home = std::env::var("HOME").unwrap_or_default();
        let tilde = path.strip_prefix(&home).map(|r| format!("~{r}")).unwrap_or_else(|| path.to_string());
        let p1 = path.to_string();
        let p2 = path.to_string();
        let p3 = path.to_string();
        let p4 = path.to_string();
        let t = tilde.clone();
        popup_context(
            vec![
                MenuEntry::item("Open Full Size", move || {
                    crate::views::file_popup::show(&p1, None);
                }),
                MenuEntry::Separator,
                MenuEntry::item("Copy Image Path", move || {
                    copy_text_to_clipboard(&p2);
                    crate::ui::toast::show(&format!("Copied {t}"), "doc.on.clipboard");
                }),
                MenuEntry::item("Copy Image", move || {
                    copy_image_file_to_pasteboard(&p3);
                }),
                MenuEntry::item("Reveal in Files", move || {
                    crate::app::host::reveal_in_files(&[p4.clone()]);
                }),
            ],
            view,
        );
    }

    /// `Copy Image` from the vim image menu: PNG + TIFF onto the pasteboard.
    fn copy_image_file_to_pasteboard(path: &str) {
        use objc2::runtime::ProtocolObject;
        use objc2_app_kit::{
            NSBitmapImageFileType, NSBitmapImageRep, NSBitmapImageRepPropertyKey, NSPasteboard,
            NSPasteboardItem, NSPasteboardTypePNG, NSPasteboardTypeTIFF, NSPasteboardWriting,
        };
        use objc2_foundation::{NSArray, NSDictionary};
        let Some(img) =
            NSImage::initWithContentsOfFile(NSImage::alloc(), &NSString::from_str(path))
        else {
            return;
        };
        let Some(tiff) = img.TIFFRepresentation() else { return };
        let item = NSPasteboardItem::new();
        item.setData_forType(&tiff, unsafe { NSPasteboardTypeTIFF });
        if let Some(rep) = NSBitmapImageRep::initWithData(NSBitmapImageRep::alloc(), &tiff) {
            let props: Retained<NSDictionary<NSBitmapImageRepPropertyKey, AnyObject>> =
                NSDictionary::new();
            if let Some(png) =
                unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &props) }
            {
                item.setData_forType(&png, unsafe { NSPasteboardTypePNG });
            }
        }
        let writable: Retained<ProtocolObject<dyn NSPasteboardWriting>> =
            ProtocolObject::from_retained(item);
        let objects = NSArray::from_retained_slice(&[writable]);
        let pb = NSPasteboard::generalPasteboard();
        pb.clearContents();
        let _ = pb.writeObjects(&objects);
        crate::ui::toast::show("Copied image", "photo");
    }

    fn copy_text_to_clipboard(text: &str) {
        let _ = crate::platform::set_text(text);
    }

    /// The "Prose" pop-out control's target. It holds `popOutProse`'s source
    /// path — the note selected when the content view was built — and launches
    /// it via the same `ProseProcess::launch` the prose switch's pop-out uses.
    pub struct NotesProseHandlerIvars {
        pub path: Option<String>,
    }

    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSNotesProseHandler"]
        #[ivars = NotesProseHandlerIvars]
        pub struct NotesProseHandler;

        impl NotesProseHandler {
            #[unsafe(method(popOut:))]
            fn pop_out(&self, _sender: Option<&AnyObject>) {
                if let Some(path) = self.ivars().path.as_deref() {
                    launch(path);
                }
            }
        }

        unsafe impl NSObjectProtocol for NotesProseHandler {}
    );

    fn as_any<T: Message + ?Sized>(obj: &T) -> &AnyObject {
        unsafe { &*(obj as *const T as *const AnyObject) }
    }

    thread_local! {
        /// Keeps the pop-out button's target alive for the main thread's life.
        static PROSE_HANDLERS: RefCell<Vec<Retained<NotesProseHandler>>>
            = const { RefCell::new(Vec::new()) };
    }

    /// A live floating prose window, kept for the life of the main thread so
    /// AppKit never releases it out from under us.
    struct ProseLive {
        path: String,
        window: Retained<ProseWindow>,
        _web: Retained<WKWebView>,
        /// The vim-motion key monitor (`j k gg G Ctrl+D Ctrl+U`).
        _keys: crate::ui::popup::MonitorHandle,
    }

    thread_local! {
        static LIVE_WINDOWS: RefCell<Vec<ProseLive>> = const { RefCell::new(Vec::new()) };
        static PROSE_CHILDREN: RefCell<HashMap<String, std::process::Child>>
            = RefCell::new(HashMap::new());
    }

    pub struct ProseTitleStripIvars {}

    define_class!(
        /// The transparent drag strip across a prose pop-out's top: the page's
        /// `WKWebView` covers the hidden titlebar and swallows mouse drags, so
        /// this thin band (the Swift "thin draggable title strip") is what moves
        /// the window. A double-click here is caught by [`ProseWindow`]'s
        /// `sendEvent:`.
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSProseTitleStrip"]
        #[ivars = ProseTitleStripIvars]
        pub struct ProseTitleStrip;

        impl ProseTitleStrip {
            #[unsafe(method(mouseDownCanMoveWindow))]
            fn mouse_down_can_move_window(&self) -> bool {
                true
            }

            #[unsafe(method(acceptsFirstMouse:))]
            fn accepts_first_mouse(&self, _e: Option<&objc2_app_kit::NSEvent>) -> bool {
                true
            }
        }

        unsafe impl NSObjectProtocol for ProseTitleStrip {}
    );

    impl ProseTitleStrip {
        fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(ProseTitleStripIvars {});
            unsafe { msg_send![super(this), initWithFrame: frame] }
        }
    }

    pub struct ProseWindowIvars {
        /// The frame saved on the first double-click of the title strip, put
        /// back on the second (Swift `ProseWindow.restoreFrame`).
        restore_frame: Cell<Option<NSRect>>,
    }

    define_class!(
        /// `NotesProse.swift`'s `ProseWindow`: a plain titled document window
        /// whose hidden titlebar is covered by the page, so the
        /// double-click-to-zoom titlebar gesture is caught in `sendEvent:`.
        #[unsafe(super(NSWindow))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSProseWindow"]
        #[ivars = ProseWindowIvars]
        pub struct ProseWindow;

        impl ProseWindow {
            #[unsafe(method(canBecomeKeyWindow))]
            fn can_become_key(&self) -> bool {
                true
            }

            #[unsafe(method(canBecomeMainWindow))]
            fn can_become_main(&self) -> bool {
                true
            }

            /// `ProseWindow.sendEvent`: a double-click in the top strip toggles
            /// fill/restore (the stock titlebar gesture never reaches the
            /// hidden titlebar — the page is on top of it).
            #[unsafe(method(sendEvent:))]
            fn send_event(&self, event: &objc2_app_kit::NSEvent) {
                if event.r#type() == objc2_app_kit::NSEventType::LeftMouseDown
                    && event.clickCount() == 2
                    && prose_in_title_strip(event.locationInWindow().y, self.frame().size.height)
                {
                    self.toggle_zoom();
                    return;
                }
                unsafe { msg_send![super(self), sendEvent: event] }
            }
        }

        unsafe impl NSObjectProtocol for ProseWindow {}
    );

    impl ProseWindow {
        fn new(mtm: MainThreadMarker, frame: NSRect, mask: NSWindowStyleMask) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(ProseWindowIvars {
                restore_frame: Cell::new(None),
            });
            unsafe {
                msg_send![
                    super(this),
                    initWithContentRect: frame,
                    styleMask: mask,
                    backing: NSBackingStoreType::Buffered,
                    defer: false
                ]
            }
        }

        /// Fill the window to its screen, or put back the frame saved by the
        /// previous double-click (Swift's `restoreFrame` flip).
        fn toggle_zoom(&self) {
            let current = nsrect_to_rect(self.frame());
            let visible = self
                .screen()
                .map(|s| nsrect_to_rect(s.visibleFrame()))
                .or_else(|| crate::platform::main_screen().map(|s| s.visible));
            let Some(visible) = visible else { return };
            let saved = self.ivars().restore_frame.get().map(nsrect_to_rect);
            let (frame, next) = prose_zoom_toggle(current, saved, visible);
            self.ivars().restore_frame.set(next.map(rect_to_nsrect));
            self.setFrame_display_animate(rect_to_nsrect(frame), true, true);
        }
    }

    fn nsrect_to_rect(r: NSRect) -> crate::platform::screens::Rect {
        crate::platform::screens::Rect::new(r.origin.x, r.origin.y, r.size.width, r.size.height)
    }

    fn rect_to_nsrect(r: crate::platform::screens::Rect) -> NSRect {
        NSRect::new(NSPoint::new(r.x, r.y), NSSize::new(r.w, r.h))
    }

    fn make_web(
        mtm: MainThreadMarker,
        on_search: Box<dyn Fn(crate::views::notes_prose::ProseMsg)>,
    ) -> Retained<crate::views::notes_prose::ProseWebView> {
        let config = unsafe { WKWebViewConfiguration::new(mtm) };
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(320.0, 240.0));
        // The content controller retains the handler for the web view's life.
        let _ = crate::views::notes_prose::add_find(mtm, &config, on_search);
        let web = crate::views::notes_prose::ProseWebView::new(mtm, frame, &config);
        let no = NSNumber::numberWithBool(false);
        let _: () = unsafe { msg_send![&*web, setValue: &*no, forKey: ns_string!("drawsBackground")] };
        web.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        // Trackpad pinch-to-zoom, same as the embedded reading view.
        unsafe { web.setAllowsMagnification(true) };
        web
    }

    fn load_html(web: &WKWebView, html: &str) {
        unsafe { web.loadHTMLString_baseURL(&NSString::from_str(html), None) };
    }

    /// `ProseWindow.show` — reuse/open a floating `WKWebView` panel.
    pub fn show(opts: &ProseLaunchOptions) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        // `open.first(where: { $0.path == path })` — raise the existing window.
        let existing = LIVE_WINDOWS.with(|v| {
            v.borrow()
                .iter()
                .find(|l| l.path == opts.path)
                .map(|l| l.window.clone())
        });
        if let Some(win) = existing {
            win.orderFrontRegardless();
            win.makeKeyAndOrderFront(None);
            return;
        }

        let markdown = std::fs::read_to_string(&opts.path).unwrap_or_default();
        let cfg = NotesConfig::load();
        let src = ProseSource::new(markdown, opts.path.clone());
        let html = ProseRender::page(&src, &opts.colors, &opts.font, opts.size, opts.width, &cfg);

        let vf = crate::platform::main_screen()
            .map(|s| s.visible)
            .unwrap_or(crate::platform::screens::Rect::new(0.0, 0.0, 1400.0, 900.0));
        let w = (vf.w * 0.6).min(opts.width + 160.0).max(320.0);
        let h = (vf.h * 0.8).max(240.0);
        let frame = NSRect::new(
            NSPoint::new(vf.x + (vf.w - w) / 2.0, vf.y + (vf.h - h) / 2.0),
            NSSize::new(w, h),
        );
        let mask = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Resizable
            | NSWindowStyleMask::FullSizeContentView;
        let window = ProseWindow::new(mtm, frame, mask);
        let title = Path::new(&opts.path)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        window.setTitle(&NSString::from_str(&title));
        window.setTitlebarAppearsTransparent(true);
        window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
        window.setMovableByWindowBackground(true);
        window.setLevel(NSNormalWindowLevel);
        unsafe { window.setReleasedWhenClosed(false) };
        window.setBackgroundColor(Some(&opts.colors.base().to_nscolor()));
        window.setMinSize(NSSize::new(320.0, 240.0));
        window.setCollectionBehavior(NSWindowCollectionBehavior::FullScreenAuxiliary);
        // A dialog-like accessibility subrole: a tiling WM that floats the
        // windows it is told are dialogs (AeroSpace) leaves the pop-out alone,
        // so it keeps its own frame instead of being tiled onto a workspace.
        unsafe {
            window.setAccessibilitySubrole(Some(objc2_app_kit::NSAccessibilityDialogSubrole));
        }
        for b in [
            NSWindowButton::CloseButton,
            NSWindowButton::MiniaturizeButton,
            NSWindowButton::ZoomButton,
        ] {
            if let Some(btn) = window.standardWindowButton(b) {
                btn.setHidden(true);
            }
        }

        // The page fills a container; a thin strip on top is the drag handle
        // the page would otherwise cover (the hidden titlebar). Letting the
        // `WKWebView` be the content view directly leaves no background to
        // drag, so the window could not be moved by its top.
        let content = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(w, h)),
        );
        content.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        let nav = Rc::new(RefCell::new(ProseViewState::new()));
        let nav_msg = nav.clone();
        let web = make_web(
            mtm,
            Box::new(move |m| {
                if let crate::views::notes_prose::ProseMsg::Search(st) = m {
                    nav_msg.borrow_mut().set_search_state(&st);
                }
            }),
        );
        {
            let (path, colors) = (opts.path.clone(), opts.colors);
            web.set_on_menu(Rc::new(move |v: &NSView| {
                crate::views::notes_prose::show_context_menu(&path, colors, v);
            }));
        }
        web.setFrame(content.bounds());
        load_html(&web, &html);
        content.addSubview(&web);
        let strip = ProseTitleStrip::new(
            mtm,
            NSRect::new(
                NSPoint::new(0.0, h - PROSE_TITLE_STRIP),
                NSSize::new(w, PROSE_TITLE_STRIP),
            ),
        );
        strip.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        content.addSubview(&strip);
        window.setContentView(Some(&content));
        window.makeKeyAndOrderFront(None);
        window.makeFirstResponder(Some(&web));
        // The same vim motions and `/` find as the in-place reading view.
        let (key_web, key_win) = (web.clone(), window.clone());
        let keys = crate::ui::popup::install_local_monitor(mtm, move |event| {
            if !key_win.isKeyWindow() {
                return false;
            }
            let k = crate::ui::popup::key_input_from_event(event);
            let m = Mods { command: k.cmd, control: k.ctrl, option: k.opt, shift: k.shift };
            let mut st = nav.borrow_mut();
            let eval = |js: &str| unsafe {
                key_web.evaluateJavaScript_completionHandler(&NSString::from_str(js), None)
            };
            if let Some(sk) = st.search_key(k.key_code, m, event.timestamp()) {
                if let Some(js) = sk.js() {
                    eval(&js);
                }
                // The first `g` is swallowed unless it is being typed in the bar.
                return sk.consumed() || (k.key_code == 5 && st.search_state != SearchState::Typing);
            }
            if m.is_empty() && st.search_state != SearchState::Typing {
                if let Some(scroll) = st.scroll_key(k.key_code, m, event.timestamp()) {
                    if let Some(js) = scroll.js() {
                        eval(&js);
                    }
                    return true;
                }
            }
            false
        });
        LIVE_WINDOWS.with(|v| {
            v.borrow_mut().push(ProseLive {
                path: opts.path.clone(),
                window,
                _web: Retained::into_super(web),
                _keys: keys,
            })
        });
    }

    /// `ProseProcess.launch` — spawn `<current_exe> prose …` (stdin/out/err
    /// null, `PYTHONDONTWRITEBYTECODE=1`), tracking the child per path; a spawn
    /// failure falls back to the in-process [`ProseWindow::show`].
    pub fn launch(path: &str) {
        let opts = ProseLaunchOptions::for_path(path);
        if child_running(path) {
            return;
        }
        let exe = match std::env::current_exe() {
            Ok(e) => e,
            Err(_) => {
                show(&opts);
                return;
            }
        };
        let mut cmd = std::process::Command::new(exe);
        cmd.arg("prose");
        cmd.args(opts.to_args());
        cmd.env("PYTHONDONTWRITEBYTECODE", "1");
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());
        match cmd.spawn() {
            Ok(child) => {
                PROSE_CHILDREN.with(|m| {
                    m.borrow_mut().insert(path.to_string(), child);
                });
            }
            Err(_) => show(&opts),
        }
    }

    /// Whether the tracked `prose` child for `path` is still alive (reaping it
    /// once it has exited).
    fn child_running(path: &str) -> bool {
        PROSE_CHILDREN.with(|m| {
            let mut m = m.borrow_mut();
            let mut finished = false;
            let running = match m.get_mut(path) {
                Some(child) => match child.try_wait() {
                    Ok(None) => true,
                    _ => {
                        finished = true;
                        false
                    }
                },
                None => false,
            };
            if finished {
                m.remove(path);
            }
            running
        })
    }

    /// The stateful notes surface the shared host embeds: the sidebar + editor
    /// tree plus two initially hidden drawers (terminal, browser). See
    /// [`super::NotesSurface`].
    /// The drawer shell: `$SHELL -l` in `$HOME` (Swift `config.shell` /
    /// `shellArgs` / `terminalDir` defaults).
    fn drawer_shell() -> (String, Vec<String>, Option<String>) {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
        (shell, vec!["-l".to_string()], std::env::var("HOME").ok())
    }

    pub struct NotesSurface {
        root: Retained<NSView>,
        editor: Retained<NSTextView>,
        terminal: Retained<NSView>,
        browser: Retained<NSScrollView>,
        drawers: DrawerState,
        /// The embedded nvim pane when `[notes] vim-mode = true`.
        vim: Option<crate::views::notes_vim::VimPane>,
        /// Relaunch inputs kept for the `:q` rebuild.
        vim_bin: String,
        vim_asset: String,
        vim_frame: NSRect,
        /// The live model behind the tree (tabs + sidebar cursor).
        model: NotesView,
        /// The vertical sidebar (`PopupTabsBar` NOTES rail): the open tabs.
        sidebar: Retained<crate::ui::sidebar::SidebarView>,
        /// The pinned-folder tree stacked above the sidebar in the rail.
        tree: Retained<crate::ui::tree_view::TreeView>,
        tree_model: crate::views::notes_tree::NotesTree,
        /// The bottom-right "Prose | nvim" capsule (`ProseModeSwitch`).
        prose_switch: Retained<crate::ui::prose_switch::ProseSwitch>,
        /// The editor scroll view (moved when the rail collapses).
        editor_scroll: Retained<NSScrollView>,
        /// The expanded sidebar width, restored when the rail expands.
        rail_expanded_width: f64,
        /// Whether the native `NSTextView` find bar is up (`Cmd+F`).
        find_shown: bool,
        /// The in-place reading view (`ProseView`), built on first use.
        prose: Option<crate::views::notes_prose::ProsePane>,
        prose_shown: bool,
        prose_sink: Option<std::rc::Rc<dyn Fn(crate::views::notes_prose::ProseMsg)>>,
        colors: PopupColors,
        /// The inline-image overlay above the vim pane (`VimImageOverlay`).
        vim_images: Option<Retained<VimImageOverlay>>,
        /// The vim pane's cell size (`PopupWindow.cellSize(vimFont)`).
        vim_cell: Cell<(f64, f64)>,
        /// The `g:ws_cell_h` (rounded cell height) last pushed to the embedded
        /// nvim; the real SwiftTerm value is pushed exactly once per change.
        vim_cell_h: Cell<i64>,
        /// Whether the configured font was re-asserted once after the pane's
        /// grid came up (guards against a lost pre-window font assignment).
        vim_font_reapplied: Cell<bool>,
        /// The point size SwiftTerm reports it is rendering the pane with (for
        /// the socket `state`; diagnostics).
        vim_font_size: Cell<f64>,
        /// Throttle for the nvim image-sidecar poll.
        vim_images_at: Cell<f64>,
    }

    /// The NOTES sidebar rows (`PopupTabsBar` titles + `vIcon` + `vMeta`).
    fn notes_sidebar_model(model: &NotesView, has_tree: bool) -> crate::ui::sidebar::SidebarModel {
        use crate::ui::sidebar::{icon_for_name, SidebarModel, SidebarRow};
        let rows = model
            .tabs
            .tabs
            .iter()
            .map(|t| SidebarRow {
                title: t.title.clone(),
                icon: icon_for_name(&t.title).to_string(),
                meta: short_mtime(&t.path),
                tip: folder_text(&t.path),
                ..Default::default()
            })
            .collect();
        SidebarModel {
            section_title: if has_tree { "Open" } else { "Notes" }.into(),
            rows,
            selected: (!model.tabs.tabs.is_empty()).then_some(model.tabs.selected),
            add_button: true,
            closable: true,
            join_top: has_tree,
            ..Default::default()
        }
    }

    /// `vMeta` — "HH:mm" today, else "MMM d".
    fn short_mtime(path: &str) -> String {
        use objc2_foundation::{NSCalendar, NSDate, NSDateFormatter};
        let Ok(t) = std::fs::metadata(path).and_then(|m| m.modified()) else { return String::new() };
        let secs = t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
        let d = NSDate::dateWithTimeIntervalSince1970(secs);
        let f = NSDateFormatter::new();
        let today = NSCalendar::currentCalendar().isDateInToday(&d);
        f.setDateFormat(Some(&NSString::from_str(if today { "HH:mm" } else { "MMM d" })));
        f.stringFromDate(&d).to_string()
    }

    /// Build the sidebar + editor surface with its hidden drawers
    /// (see [`super::NotesSurface`]).
    fn build_surface(mtm: MainThreadMarker) -> Option<NotesSurface> {
        let cfg = NotesConfig::load();
        let colors = crate::ui::theme::section_colors("notes");
        let mut model = NotesView::new(cfg.clone());
        let raw = setting("notes", "paths").unwrap_or_default();
        // Directories in `[notes] paths` become pinned tree roots; files stay tabs.
        let (roots, paths) = crate::views::notes_tree::split_roots(&raw, &|p| Path::new(p).is_dir(), &|p| Path::new(p).exists());
        model.tabs.set_dismissed(load_dismissed());
        // A tab the user closed stays closed across restarts, unless that would
        // leave the window with no note at all.
        let live: Vec<&String> = paths.iter().filter(|p| !model.tabs.is_dismissed(p)).collect();
        let keep: Vec<&String> = if live.is_empty() { paths.iter().collect() } else { live };
        for p in keep {
            model.tabs.append(p);
        }
        let show_hidden = matches!(setting("notes", "tree-show-hidden").as_deref(), Some("true") | Some("yes") | Some("on") | Some("1"));
        let mut expanded = crate::views::notes_tree::load_expanded();
        if expanded.is_empty() {
            expanded.extend(roots.iter().take(1).cloned());
        }
        let mut tree_model = crate::views::notes_tree::NotesTree::new(roots, expanded, show_hidden);
        if let Some(p) = model.tabs.selected_path() {
            tree_model.reveal(p);
        }
        let has_tree = !tree_model.is_empty();
        model.refresh_sidebar(now_secs());

        let sidebar_w = model.sidebar.width.clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH);
        let root = NotesFlippedView::new(mtm);
        root.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(900.0, 600.0)));
        root.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );

        // --- sidebar (`PopupTabsBar` vertical, NOTES) -------------------------
        let sidebar = crate::ui::sidebar::SidebarView::new(mtm, colors);
        sidebar.set_collapse_key("notes");
        model.sidebar.collapsed = sidebar.collapsed();
        let sidebar_w = model.sidebar.visible_width();
        let tree_wanted = if crate::views::notes_tree::load_tree_collapsed() { crate::ui::tree_view::TreeGeometry::collapsed_height() } else { crate::ui::tree_view::TreeGeometry::height_for(tree_model.rows().len()) };
        let (tree_h, open_h) = crate::views::notes_tree::rail_split(600.0, has_tree && !model.sidebar.collapsed, tree_wanted);
        sidebar.setFrame(NSRect::new(NSPoint::new(0.0, tree_h), NSSize::new(sidebar_w, open_h)));
        // Tree and open list share the column: the open list's top margin
        // flexes with the tree's height so they stay edge to edge on resize.
        sidebar.setAutoresizingMask(NSAutoresizingMaskOptions::empty());
        sidebar.set_model(notes_sidebar_model(&model, has_tree));
        root.addSubview(&sidebar);

        // --- pinned-folder tree (above the open-tabs sidebar) ------------------
        let tree = crate::ui::tree_view::TreeView::new(mtm, colors);
        tree.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(sidebar_w, tree_h)));
        tree.setAutoresizingMask(NSAutoresizingMaskOptions::empty());
        tree.set_indent(number("notes", "tree-indent", 14.0));
        tree.setHidden(tree_h <= 0.0);
        let tree_rows = tree_model.rows();
        tree.set_rows(tree_rows.clone(), tree_model.selected.clone());
        if let Some(i) = tree_model.selected.as_deref().and_then(|p| crate::views::notes_tree::NotesTree::row_of(&tree_rows, p)) {
            tree.reveal_row(i);
        }
        root.addSubview(&tree);
        root.set_rail(&tree, &sidebar);

        // --- editor ----------------------------------------------------------
        let editor_w = (900.0 - sidebar_w - 4.0).max(0.0);
        let editor = NSScrollView::new(mtm);
        editor.setFrame(NSRect::new(
            NSPoint::new(sidebar_w + 4.0, 0.0),
            NSSize::new(editor_w, 600.0),
        ));
        editor.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        editor.setHasVerticalScroller(true);
        editor.setDrawsBackground(true);
        editor.setBackgroundColor(&colors.base().to_nscolor());
        let text: Retained<NSTextView> = Retained::into_super(NotesTextView::new(mtm));
        text.setEditable(true);
        text.setSelectable(true);
        text.setRichText(true);
        text.setDrawsBackground(true);
        text.setBackgroundColor(&colors.base().to_nscolor());
        text.setTextColor(Some(&colors.text.to_nscolor()));
        let size = cfg.prose_font_size.clamp(9.0, 40.0);
        let font = if cfg.prose_font.is_empty() {
            None
        } else {
            NSFont::fontWithName_size(&NSString::from_str(&cfg.prose_font), size)
        };
        text.setFont(Some(&font.unwrap_or_else(|| NSFont::systemFontOfSize(size))));
        text.setTextContainerInset(NSSize::new(14.0, 14.0));
        text.setVerticallyResizable(true);
        text.setHorizontallyResizable(false);
        // Cmd+F: the native `NSTextView` find bar (`findAction` below).
        text.setUsesFindBar(true);
        text.setIncrementalSearchingEnabled(true);
        if let Some(container) = unsafe { text.textContainer() } {
            container.setWidthTracksTextView(true);
        }
        let selected = model.tabs.selected_path().map(str::to_string);
        let body = selected
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default();
        let base_dir = selected
            .as_deref()
            .and_then(|p| Path::new(p).parent())
            .map(|d| d.to_string_lossy().into_owned())
            .unwrap_or_default();
        set_editor_markdown(&text, &editor, colors, &body, &base_dir);
        text.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(editor_w, 600.0)));
        editor.setDocumentView(Some(&text));
        root.addSubview(&editor);

        // --- vim pane (`[notes] vim-mode = true`) ----------------------------
        // The Swift notes window embeds nvim in a terminal view over the
        // editor (`installVimPane` / `buildVimPane`); the editor stays hidden
        // while the pane is active.
        let vim_enabled = matches!(
            setting("notes", "vim-mode").as_deref(),
            Some("true") | Some("yes") | Some("on") | Some("1")
        );
        let vim_bin = setting("notes", "vim-bin").unwrap_or_else(|| "nvim".to_string());
        let vim_paths = crate::app::paths::Paths::from_env();
        let vim_asset = vim_paths.asset_dir().to_string();
        let vim_frame = NSRect::new(
            NSPoint::new(sidebar_w + 4.0 + 8.0, 4.0),
            NSSize::new(editor_w - 16.0, 600.0 - 8.0),
        );
        let mut vim = None;
        let mut vim_images: Option<Retained<VimImageOverlay>> = None;
        let mut vim_cell = (8.0, 16.0);
        // `g:ws_cell_h` value the editor was launched with (the
        // `cell_size_for` estimate); the first `refresh_vim_cell` corrects it to
        // SwiftTerm's real cell height and pushes the new value once.
        let mut vim_cell_h = 0i64;
        if vim_enabled {
            let file = model.tabs.selected_path().map(str::to_string);
            let mut pane =
                crate::views::notes_vim::VimPane::new(vim_bin.clone(), vim_asset.clone(), file);
            // `PopupWindow.vimFont`: [notes] font (or "<font> Mono") at font-size.
            let size = setting("notes", "font-size").and_then(|v| v.parse::<f64>().ok()).unwrap_or(14.0);
            let font_name = setting("notes", "font").unwrap_or_default();
            let mono = [font_name.clone(), format!("{font_name} Mono")]
                .into_iter()
                .find(|n| !n.is_empty() && NSFont::fontWithName_size(&NSString::from_str(n), size).is_some_and(|f| f.isFixedPitch()))
                .or_else(|| setting("app", "terminal-font"))
                .unwrap_or_else(|| "Menlo".to_string());
            pane.set_font(&mono, size);
            // `refreshVimImageRows`: the cell height the sidecar rows use.
            vim_cell = cell_size_for(&mono, size);
            let mut launch = pane.launch().clone();
            launch.cell_h = vim_cell.1.round() as i64;
            vim_cell_h = launch.cell_h;
            if let Some(rows) = setting("notes", "image-rows").and_then(|v| v.parse::<i64>().ok()) {
                launch.image_rows = rows.max(1);
            }
            pane.set_launch(launch);
            if let Some(view) = pane.build(mtm, vim_frame) {
                if let Ok(shim) = view.clone().downcast::<swiftterm_shim::WSShim>() {
                    let pal: Vec<Retained<objc2_app_kit::NSColor>> =
                        crate::ui::theme::ansi_palette(&colors).into_iter().map(|c| c.to_nscolor()).collect();
                    swiftterm_shim::set_colors(
                        &shim,
                        &objc2_app_kit::NSColor::clearColor(),
                        &colors.text.to_nscolor(),
                        &pal,
                    );
                    // SwiftTerm lays the grid out with the font's real metrics,
                    // which can differ from `cell_size_for`'s estimate; use the
                    // actual cell size for the image overlay (and let
                    // `refresh_vim_cell` push the matching `g:ws_cell_h`).
                    if let Some(real) = swiftterm_shim::cell_size(&shim) {
                        vim_cell = real;
                    }
                }
                view.setAutoresizingMask(
                    NSAutoresizingMaskOptions::ViewWidthSizable
                        | NSAutoresizingMaskOptions::ViewHeightSizable,
                );
                text.setHidden(true);
                root.addSubview(&view);
                // The inline images nvim's `init.lua` reserved rows for.
                let overlay = VimImageOverlay::create(mtm, vim_frame, vim_cell);
                overlay.set_on_open(Rc::new(|path: String| {
                    crate::views::file_popup::show(&path, None);
                }));
                root.addSubview_positioned_relativeTo(
                    &overlay,
                    objc2_app_kit::NSWindowOrderingMode::Above,
                    Some(&view),
                );
                vim_images = Some(overlay);
                vim = Some(pane);
            }
        }

        // --- pop-out control (`popOutProse`) ---------------------------------
        // The prose switch's "pop out" path: launch the selected note in the
        // floating prose window. The selection is captured when this surface is
        // built; a missing selection/file is a safe no-op.
        let selected = model.tabs.selected_path().map(|s| s.to_string());
        let target = prose_launch_target(selected.as_deref(), |p| Path::new(p).exists());
        let has_target = target.is_some();
        let handler: Retained<NotesProseHandler> = unsafe {
            let h = NotesProseHandler::alloc(mtm).set_ivars(NotesProseHandlerIvars { path: target });
            msg_send![super(h), init]
        };
        PROSE_HANDLERS.with(|v| v.borrow_mut().push(handler.clone()));
        let _ = has_target;
        let edit_label = if vim.is_some() { "nvim" } else { "Edit" };
        let prose_switch = crate::ui::prose_switch::ProseSwitch::new(mtm, colors, edit_label);
        prose_switch.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        prose_switch.place(editor.frame());
        root.addSubview(&prose_switch);

        // --- terminal drawer (bottom, hidden by default) ---------------------
        let terminal_rect = NSRect::new(
            NSPoint::new(sidebar_w + 1.0, 600.0 - TERMINAL_DRAWER_HEIGHT),
            NSSize::new(editor_w, TERMINAL_DRAWER_HEIGHT),
        );
        let terminal = swiftterm_shim::create_terminal(mtm, terminal_rect);
        terminal.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        if let Ok(shim) = terminal.clone().downcast::<swiftterm_shim::WSShim>() {
            let tf = setting("app", "terminal-font").unwrap_or_else(|| "Menlo".to_string());
            let ts = setting("app", "terminal-font-size").and_then(|v| v.parse::<f64>().ok()).unwrap_or(12.0);
            swiftterm_shim::set_font(&shim, &tf, ts.min(18.0));
            let pal: Vec<Retained<objc2_app_kit::NSColor>> =
                crate::ui::theme::ansi_palette(&colors).into_iter().map(|c| c.to_nscolor()).collect();
            swiftterm_shim::set_colors(&shim, &colors.background.with_alpha(1.0).to_nscolor(), &colors.text.to_nscolor(), &pal);
            let (shell, args, home) = drawer_shell();
            swiftterm_shim::start_process(&shim, &shell, &args, home.as_deref());
        }
        terminal.setHidden(true);
        root.addSubview(&terminal);

        // --- browser drawer (side, hidden by default) ------------------------
        let browser = NSScrollView::new(mtm);
        browser.setFrame(NSRect::new(
            NSPoint::new((900.0 - BROWSER_DRAWER_WIDTH).max(0.0), 0.0),
            NSSize::new(BROWSER_DRAWER_WIDTH, 600.0),
        ));
        browser.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewHeightSizable
                | NSAutoresizingMaskOptions::ViewMinXMargin,
        );
        browser.setHasVerticalScroller(true);
        browser.setDrawsBackground(true);
        browser.setBackgroundColor(&colors.mantle().to_nscolor());
        let list = NotesFlippedView::new(mtm);
        list.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        let names = model
            .tabs
            .selected_path()
            .and_then(|p| Path::new(p).parent())
            .map(directory_names)
            .unwrap_or_default();
        let mut list_y = 6.0;
        for name in &names {
            let label = NSTextField::labelWithString(&NSString::from_str(name), mtm);
            label.setFont(Some(&NSFont::systemFontOfSize(11.5)));
            label.setTextColor(Some(&colors.text.to_nscolor()));
            label.setFrame(NSRect::new(
                NSPoint::new(10.0, list_y),
                NSSize::new((BROWSER_DRAWER_WIDTH - 16.0).max(0.0), 16.0),
            ));
            label.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
            list.addSubview(&label);
            list_y += 20.0;
        }
        list.setFrame(NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(BROWSER_DRAWER_WIDTH, list_y.max(600.0)),
        ));
        browser.setDocumentView(Some(&list));
        browser.setHidden(true);
        root.addSubview(&browser);

        Some(NotesSurface {
            root: root.into_super(),
            editor: text.clone(),
            terminal,
            browser,
            drawers: DrawerState::default(),
            vim,
            vim_bin,
            vim_asset,
            vim_frame,
            model,
            sidebar,
            tree,
            tree_model,
            prose_switch,
            editor_scroll: editor,
            rail_expanded_width: sidebar_w,
            find_shown: false,
            prose: None,
            prose_shown: false,
            prose_sink: None,
            colors,
            vim_images,
            vim_cell: Cell::new(vim_cell),
            vim_cell_h: Cell::new(vim_cell_h),
            vim_font_reapplied: Cell::new(false),
            vim_font_size: Cell::new(0.0),
            vim_images_at: Cell::new(0.0),
        })
    }

    impl NotesSurface {
        /// Build the sidebar + editor surface with its hidden drawers.
        pub fn build(mtm: MainThreadMarker) -> Option<NotesSurface> {
            build_surface(mtm)
        }

        /// The root view to embed in the shared host window.
        pub fn content_view(&self) -> Retained<NSView> {
            self.root.clone()
        }

        /// The note editor, so the host can `makeFirstResponder` it.
        pub fn editor_view(&self) -> Option<Retained<NSTextView>> {
            Some(self.editor.clone())
        }

        /// Flip the named drawer (`"terminal"` / `"browser"`), returning the new
        /// shown flag; `None` for an unknown name. Toggling one drawer never
        /// un-toggles the other.
        pub fn toggle_drawer(&mut self, which: &str) -> Option<bool> {
            let shown = self.drawers.toggle(which)?;
            match which {
                "terminal" => self.terminal.setHidden(!shown),
                "browser" => self.browser.setHidden(!shown),
                _ => {}
            }
            self.apply_sidebar_width();
            Some(shown)
        }

        /// The exact `{"terminal", "browser", "drawerInset"}` state shape.
        pub fn drawer_state(&self) -> Value {
            self.drawers.test_state()
        }

        // -- list keys / selection (`PopupWindow.moveSelection` etc.) --------

        /// The selected sidebar row (`tabs.selected`).
        pub fn selected_index(&self) -> usize {
            self.model.tabs.selected
        }

        pub fn row_count(&self) -> usize {
            self.model.tabs.tabs.len()
        }

        pub fn selected_path(&self) -> Option<String> {
            self.model.tabs.selected_path().map(str::to_string)
        }

        /// Whether the sidebar rail is collapsed (`PopupTabsBar.collapsed`).
        pub fn rail_collapsed(&self) -> bool {
            self.model.sidebar.collapsed
        }

        /// Whether the native find bar is up.
        pub fn find_shown(&self) -> bool {
            self.find_shown
        }

        /// The `views.notes` selection extras the socket `state` reports.
        pub fn selection_state(&self) -> Value {
            json!({
                "selection": self.model.tabs.selected,
                "selectedTab": self.model.tabs.selected,
                "rowCount": self.model.tabs.tabs.len(),
                "sidebarCursor": self.model.tabs.selected,
                "findBar": self.find_shown,
                "collapsed": self.model.sidebar.collapsed,
                "tabs": self.model.tabs.titles(),
                "pinnedRoots": self.tree_model.roots,
                "treeSelection": self.tree_model.selected,
                "treeExpanded": self.tree_model.expanded,
                "treeRows": self.tree.row_count(),
                "treeCollapsed": self.tree.collapsed(),
                "vimCellHeight": self.vim_cell.get().1,
                "vimFontSize": self.vim_font_size.get(),
            })
        }

        /// `moveSelection(_:)`: clamp into the tab list.
        pub fn list_move(&mut self, delta: i64) {
            if self.model.tabs.tabs.is_empty() {
                return;
            }
            let n = self.model.tabs.tabs.len() as i64;
            let cur = self.model.tabs.selected as i64;
            let next = (cur + delta).clamp(0, n - 1) as usize;
            self.select_row(next);
        }

        /// Tab / Shift+Tab: wrap around the tab list.
        pub fn list_move_wrap(&mut self, delta: i64) {
            if self.model.tabs.tabs.is_empty() {
                return;
            }
            let n = self.model.tabs.tabs.len() as i64;
            let cur = self.model.tabs.selected as i64;
            let next = ((cur + delta).rem_euclid(n)) as usize;
            self.select_row(next);
        }

        /// Home / End / PageUp / PageDown (`listKey`'s page branch).
        pub fn list_page(&mut self, page: crate::ui::popup::PageMove) {
            use crate::ui::popup::PageMove;
            let n = self.model.tabs.tabs.len();
            if n == 0 {
                return;
            }
            let cur = self.model.tabs.selected;
            let next = match page {
                PageMove::Home => 0,
                PageMove::End => n - 1,
                PageMove::Up => cur.saturating_sub(10),
                PageMove::Down => (cur + 10).min(n - 1),
            };
            self.select_row(next);
        }

        /// `acceptSelection` — open the selected note.
        pub fn list_accept(&mut self) {
            if let Some(path) = self.selected_path() {
                self.open_note(&path);
            }
        }

        /// `toggleRowSelection` — the notes sidebar has no marked set.
        pub fn list_toggle_selection(&mut self) {}

        /// Move the cursor to row `i`: repaint the sidebar, open the note.
        fn select_row(&mut self, i: usize) {
            if i >= self.model.tabs.tabs.len() {
                return;
            }
            let changed = self.model.tabs.selected != i;
            self.model.tabs.selected = i;
            self.repaint_rows();
            if changed {
                if let Some(path) = self.selected_path() {
                    self.open_note(&path);
                }
            }
        }

        /// Push the tab rows / selection into the sidebar view.
        fn repaint_rows(&mut self) {
            self.sidebar.set_model(notes_sidebar_model(&self.model, !self.tree_model.is_empty()));
        }

        /// The tree view (the host routes its clicks).
        pub fn tree_view(&self) -> Retained<crate::ui::tree_view::TreeView> {
            self.tree.clone()
        }

        /// Push the model's rows + selection into the tree view.
        fn repaint_tree(&mut self, scroll_to_selection: bool) {
            let rows = self.tree_model.rows();
            self.tree.set_rows(rows.clone(), self.tree_model.selected.clone());
            if scroll_to_selection {
                if let Some(i) = self.tree_model.selected.as_deref().and_then(|p| crate::views::notes_tree::NotesTree::row_of(&rows, p)) {
                    self.tree.reveal_row(i);
                }
            }
            // The tree is only as tall as its rows: re-split the rail.
            self.apply_sidebar_width();
        }

        /// The open file changed: expand its ancestors and select its row. A
        /// file outside every pinned root clears the selection (it still shows
        /// in the Open list) instead of faking a position.
        fn tree_reveal(&mut self, path: &str) {
            if self.tree_model.is_empty() {
                return;
            }
            let before = self.tree_model.expanded.len();
            self.tree_model.reveal(path);
            if self.tree_model.expanded.len() != before {
                crate::views::notes_tree::save_expanded(&self.tree_model.expanded);
            }
            self.repaint_tree(true);
        }

        /// A left click in the tree: folders toggle, files open (adding a tab).
        pub fn tree_click(&mut self, path: &str, is_dir: bool) {
            if is_dir {
                self.tree_model.toggle(path);
                crate::views::notes_tree::save_expanded(&self.tree_model.expanded);
                self.repaint_tree(false);
            } else {
                self.open_external_path(path);
            }
        }

        /// Collapse / expand every folder under `path` from the context menu.
        pub fn tree_set_expanded(&mut self, path: &str, open: bool) {
            if open {
                self.tree_model.expanded.insert(path.to_string());
            } else {
                self.tree_model.expanded.remove(path);
            }
            crate::views::notes_tree::save_expanded(&self.tree_model.expanded);
            self.repaint_tree(false);
        }

        /// The PINNED header: collapse / expand the whole tree card.
        pub fn tree_toggle_section(&mut self) {
            self.tree.set_collapsed(!self.tree.collapsed());
            self.apply_sidebar_width();
        }

        /// A file or folder under a pinned root was trashed: close the tabs it
        /// held and drop it from the tree.
        pub fn tree_removed(&mut self, path: &str) {
            let prefix = format!("{path}/");
            let doomed: Vec<usize> = self
                .model
                .tabs
                .paths()
                .iter()
                .enumerate()
                .filter(|(_, p)| p.as_str() == path || p.starts_with(&prefix))
                .map(|(i, _)| i)
                .collect();
            for i in doomed.into_iter().rev() {
                self.close_tab(i);
            }
            if self.tree_model.selected.as_deref().is_some_and(|p| p == path || p.starts_with(&prefix)) {
                self.tree_model.selected = None;
            }
            self.tree_model.expanded.retain(|p| p != path && !p.starts_with(&prefix));
            self.repaint_tree(false);
        }

        /// Re-read the expanded folders (after a create / rename / trash).
        pub fn tree_refresh(&mut self) {
            self.repaint_tree(false);
        }

        /// A new note goes into the selected folder (or the open file's
        /// folder); used by the tree's "New Note Here".
        pub fn tree_new_note_in(&mut self, dir: &str) {
            let home = std::env::var("HOME").unwrap_or_default();
            let assets = crate::app::paths::Paths::from_env().asset_dir().to_string();
            let snippets: Value = std::fs::read_to_string(format!("{assets}/vim/snippets/markdown.json"))
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
                .unwrap_or(Value::Null);
            let _ = home;
            let open = self.model.tabs.paths();
            let plan = plan_new_note(dir, false, &snippets, &self.model.config, &open);
            if plan.write().is_err() {
                return;
            }
            let path = plan.path.clone();
            self.tree_model.expanded.insert(dir.to_string());
            self.open_external_path(&path);
        }

        /// The sidebar view (the host routes its clicks).
        pub fn sidebar_view(&self) -> Retained<crate::ui::sidebar::SidebarView> {
            self.sidebar.clone()
        }

        pub fn prose_switch(&self) -> Retained<crate::ui::prose_switch::ProseSwitch> {
            self.prose_switch.clone()
        }

        pub fn tab_path(&self, i: usize) -> Option<String> {
            self.model.tabs.tabs.get(i).map(|t| t.path.clone())
        }

        /// The sidebar "+" (`onNewNote`): `dir/<new-note-name>-N.md` beside
        /// the current note (else `~/notes`), opened as a new tab.
        pub fn new_note(&mut self) {
            self.new_note_from(false);
        }

        /// `newNote(template:)` — Cmd+N (`new-note-name`) / Ctrl+N
        /// (`new-doc-name`); both start from the `new-doc-template` snippet in
        /// `vim/snippets/markdown.json`, as the Swift code does.
        pub fn new_note_from(&mut self, template: bool) {
            let assets = crate::app::paths::Paths::from_env().asset_dir().to_string();
            let snippets: Value = std::fs::read_to_string(format!("{assets}/vim/snippets/markdown.json"))
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
                .unwrap_or(Value::Null);
            let home = std::env::var("HOME").unwrap_or_default();
            let dir = self
                .selected_path()
                .and_then(|p| Path::new(&p).parent().map(|d| d.to_string_lossy().into_owned()))
                .unwrap_or_else(|| format!("{home}/notes"));
            let _ = std::fs::create_dir_all(&dir);
            let open = self.model.tabs.paths();
            let plan = plan_new_note(&dir, template, &snippets, &self.model.config, &open);
            if plan.write().is_err() {
                return;
            }
            // `append` already selects the new tab, so `select_row` would see
            // no change and never load it: the editor (and prose page) kept the
            // previous note and typing / paste went into (and saved) that file.
            self.model.tabs.append(&plan.path);
            save_dismissed(&self.model.tabs.dismissed_paths());
            self.repaint_rows();
            self.open_note(&plan.path);
        }

        /// The sidebar ✕ on tab `i` (`onCloseTab`).
        pub fn close_tab(&mut self, i: usize) {
            let Some(closed) = self.model.tabs.close(i) else {
                return;
            };
            self.model.tabs.dismiss(&closed);
            save_dismissed(&self.model.tabs.dismissed_paths());
            let n = self.model.tabs.tabs.len();
            if n > 0 {
                let sel = self.model.tabs.selected.min(n - 1);
                self.model.tabs.selected = sel;
                if let Some(p) = self.selected_path() {
                    self.open_note(&p);
                }
            }
            self.repaint_rows();
        }

        /// Select tab `i` from a sidebar click.
        pub fn click_row(&mut self, i: usize) {
            self.select_row(i);
            self.focus_editor();
        }

        /// `popOutProse()` — the selected note in a floating prose window.
        pub fn pop_out_prose(&self) {
            if let Some(p) = prose_launch_target(self.selected_path().as_deref(), |p| Path::new(p).exists()) {
                launch(&p);
            }
        }

        // -- the in-place reading view (`setProse` / `ProseView`) -----------

        /// Whether the reading view covers the editor (`proseShown`).
        pub fn prose_shown(&self) -> bool {
            self.prose_shown
        }

        /// `proseProvider != nil` — a selected note that exists on disk.
        pub fn has_prose_source(&self) -> bool {
            self.selected_path().is_some_and(|p| Path::new(&p).exists())
        }

        /// The host's handler for page messages (outline jump, find state,
        /// image double-click). Also receives the top line on close.
        /// The plain editor's right-click (`PopupTextView.menu(for:)`):
        /// `cb` gets the clicked character index.
        pub fn set_editor_menu(&self, cb: std::rc::Rc<dyn Fn(usize)>) {
            if let Some(tv) = (&*self.editor as &AnyObject).downcast_ref::<NotesTextView>() {
                *tv.ivars().on_menu.borrow_mut() = Some(cb);
            }
        }

        pub fn set_prose_sink(&mut self, cb: std::rc::Rc<dyn Fn(crate::views::notes_prose::ProseMsg)>) {
            if let Some(p) = &self.prose {
                let cb = cb.clone();
                p.set_on_message(Box::new(move |m| cb(m)));
            }
            self.prose_sink = Some(cb);
        }

        /// `PopupTextView.onPasteImage` — Cmd+V / a drop with an image; the
        /// host saves it next to the note and calls
        /// [`Self::insert_image_attachment`].
        pub fn set_editor_paste_image(&self, cb: Rc<dyn Fn(Retained<NSImage>)>) {
            if let Some(tv) = (&*self.editor as &AnyObject).downcast_ref::<NotesTextView>() {
                *tv.ivars().on_paste_image.borrow_mut() = Some(cb);
            }
        }

        /// `PopupTextView.onOpenImage` — a click on an inline image.
        pub fn set_editor_open_image(&self, cb: Rc<dyn Fn(String)>) {
            if let Some(tv) = (&*self.editor as &AnyObject).downcast_ref::<NotesTextView>() {
                *tv.ivars().on_open_image.borrow_mut() = Some(cb);
            }
        }

        /// The vim overlay's image click (`VimImageOverlay.onOpen`).
        pub fn set_vim_image_open(&self, cb: Rc<dyn Fn(String)>) {
            if let Some(ov) = &self.vim_images {
                ov.set_on_open(cb);
            }
        }

        /// The image target under the clicked character index: an attachment,
        /// else the `![alt](target)` link text (the `menu(for:)` path).
        pub fn image_path_at(&self, idx: usize) -> Option<String> {
            let note = self.selected_path().unwrap_or_default();
            let home = std::env::var("HOME").unwrap_or_default();
            if let Some(tv) = (&*self.editor as &AnyObject).downcast_ref::<NotesTextView>() {
                // `characterIndexForInsertionAtPoint` can land just after the
                // attachment; try the character before too.
                if let Some(rel) = tv
                    .attachment_rel_at(idx)
                    .or_else(|| idx.checked_sub(1).and_then(|i| tv.attachment_rel_at(i)))
                {
                    return Some(resolve_note_link(&rel, &note, &home));
                }
            }
            let text = self.editor.string().to_string();
            image_link_at(&text, idx).map(|t| resolve_note_link(&t, &note, &home))
        }

        /// Resolve an image target (`assets/img-x.png`, `~/x.png`, `/x.png`)
        /// against the selected note's folder.
        pub fn resolve_image_path(&self, target: &str) -> String {
            let note = self.selected_path().unwrap_or_default();
            let home = std::env::var("HOME").unwrap_or_default();
            resolve_note_link(target, &note, &home)
        }

        /// `insertImageAttachment(rel:image:)`: the attachment + newline at
        /// the caret, caret after it.
        pub fn insert_image_attachment(&mut self, rel: &str, image: &NSImage) {
            let Some(mtm) = MainThreadMarker::new() else { return };
            let Some(storage) = (unsafe { self.editor.textStorage() }) else {
                return;
            };
            let att =
                NoteAttachment::create(mtm, image, rel, self.editor_scroll.bounds().size.width);
            let piece = NSMutableAttributedString::initWithString(
                NSMutableAttributedString::alloc(),
                &NSString::from_str("\u{FFFC}\n"),
            );
            unsafe {
                piece.addAttribute_value_range(
                    objc2_app_kit::NSAttachmentAttributeName,
                    as_any(&*att),
                    NSRange::new(0, 1),
                )
            };
            let sel = self.editor.selectedRange();
            storage.replaceCharactersInRange_withAttributedString(sel, &piece);
            let caret = NSRange::new(sel.location + 2, 0);
            self.editor.setSelectedRange(caret);
            self.editor.scrollRangeToVisible(caret);
            self.editor.setNeedsDisplay(true);
        }

        /// The editor's markdown (`editorMarkdown`): attachments → `![](rel)`.
        pub fn editor_markdown(&self) -> String {
            editor_markdown(&self.editor)
        }

        /// Load `text` into the plain editor with inline images
        /// (`setEditorMarkdown`), based at `path`'s folder.
        fn set_note_text(&mut self, text: &str, path: &str) {
            let base_dir = Path::new(path)
                .parent()
                .map(|d| d.to_string_lossy().into_owned())
                .unwrap_or_default();
            set_editor_markdown(&self.editor, &self.editor_scroll, self.colors, text, &base_dir);
        }

        /// The per-drain nvim sidecar poll (`reloadVimImages`, throttled).
        pub fn tick_vim_images(&mut self) {
            if self.vim_images.is_none() {
                return;
            }
            let now = now_secs();
            if now - self.vim_images_at.get() < 0.12 {
                return;
            }
            self.vim_images_at.set(now);
            let Some(overlay) = self.vim_images.clone() else { return };
            let Some(pane) = self.vim.as_ref() else { return };
            let path = pane.launch().image_file.clone();
            let Ok(meta) = std::fs::metadata(&path) else { return };
            // The sidecar is written by `init.lua`, so its presence means the
            // editor is up and `_G.ws_images_refresh` exists — safe to push a
            // corrected `g:ws_cell_h` (SwiftTerm's real cell height, which can
            // differ from the launch-time estimate) and re-place the rows.
            self.refresh_vim_cell();
            let Ok(mtime) = meta.modified() else { return };
            if overlay.ivars().mtime.get() == Some(mtime) {
                return;
            }
            overlay.ivars().mtime.set(Some(mtime));
            let Ok(raw) = std::fs::read_to_string(&path) else { return };
            let Ok(v) = serde_json::from_str::<Value>(&raw) else { return };
            let lines = v.get("lines").and_then(Value::as_i64).unwrap_or(0);
            let items: Vec<VimImageItem> = v
                .get("images")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|d| {
                            Some(VimImageItem {
                                path: d.get("path")?.as_str()?.to_string(),
                                row: d.get("row")?.as_i64()?,
                                rows: d.get("rows")?.as_i64()?,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            overlay.set_state((lines - 1).max(0), items);
        }

        /// `refreshVimImageRows` + `cellSize`: read SwiftTerm's *actual* cell
        /// size from the pane's shim, hand it to the image overlay, and push the
        /// matching `g:ws_cell_h` to the editor (re-placing the reserved rows so
        /// the sidecar's `rows` match the real cell height). A no-op until the
        /// shim reports a size; only pushes when the rounded height changes.
        fn refresh_vim_cell(&self) {
            let Some(pane) = self.vim.as_ref() else { return };
            let Some(view) = pane.view() else { return };
            let Ok(shim) = view.downcast::<swiftterm_shim::WSShim>() else { return };
            // A font assigned before the terminal is in a window can fail to
            // stick in SwiftTerm; re-assert the configured font once the grid is
            // live, then trust what SwiftTerm reports.
            let actual = swiftterm_shim::font_point_size(&shim);
            if !self.vim_font_reapplied.get() {
                if let Some((name, size)) = pane.font() {
                    if actual > 0.0 && (actual - size).abs() > 0.5 {
                        swiftterm_shim::set_font(&shim, &name, size);
                        self.vim_font_reapplied.set(true);
                        return; // re-read the cell on the next tick
                    }
                }
                self.vim_font_reapplied.set(true);
            }
            self.vim_font_size.set(swiftterm_shim::font_point_size(&shim));
            let Some(cell) = swiftterm_shim::cell_size(&shim) else { return };
            self.vim_cell.set(cell);
            if let Some(ov) = &self.vim_images {
                ov.set_cell(cell);
            }
            let h = cell.1.round() as i64;
            if h > 0 && h != self.vim_cell_h.get() {
                self.vim_cell_h.set(h);
                pane.command(&format!("let g:ws_cell_h={h} | lua _G.ws_images_refresh()"));
            }
        }

        /// `toggleProseFromPrefix()` (Ctrl+B T): only from the vim pane or the
        /// reading view; `false` lets the prefix fall back to the terminal.
        pub fn toggle_prose_from_prefix(&mut self) -> bool {
            if !self.has_prose_source() || !(self.prose_shown || self.vim.is_some()) {
                return false;
            }
            let on = !self.prose_shown;
            self.set_prose(on);
            true
        }

        /// `setProse(_:)`.
        pub fn set_prose(&mut self, on: bool) {
            self.set_prose_to(on, None);
        }

        /// `setProse(_:)` with an explicit source `path` (opening a note while
        /// the reading view is up); `None` falls back to the selected tab.
        fn set_prose_to(&mut self, on: bool, path: Option<&str>) {
            self.prose_switch.set_prose(on);
            if !on {
                if !self.prose_shown {
                    return;
                }
                self.prose_shown = false;
                if let (Some(pv), true) = (&self.prose, self.vim.is_some()) {
                    if let Some(sink) = self.prose_sink.clone() {
                        pv.top_source_line(move |n| {
                            if let Some(n) = n {
                                sink(crate::views::notes_prose::ProseMsg::Jump(n));
                            }
                        });
                    }
                }
                if let Some(pv) = &self.prose {
                    pv.view().setHidden(true);
                }
                self.focus_editor();
                return;
            }
            let Some(mtm) = MainThreadMarker::new() else { return };
            if let Some(vim) = &self.vim {
                vim.command("silent! update");
            } else {
                self.commit();
            }
            let target = path.map(str::to_string).or_else(|| self.selected_path());
            let Some(path) = target.filter(|p| Path::new(p).exists()) else {
                // Nothing to render: leave the switch showing what is actually
                // up (`prose_shown`) instead of a stale value, so a later click
                // is not swallowed by the control's cached mode.
                self.prose_switch.set_prose(self.prose_shown);
                return;
            };
            let Ok(markdown) = std::fs::read_to_string(&path) else {
                self.prose_switch.set_prose(self.prose_shown);
                return;
            };
            if self.prose.is_none() {
                let pane = crate::views::notes_prose::ProsePane::new(mtm, self.editor_scroll.frame());
                if let Some(cb) = self.prose_sink.clone() {
                    pane.set_on_message(Box::new(move |m| cb(m)));
                }
                self.root.addSubview_positioned_relativeTo(
                    pane.view(),
                    objc2_app_kit::NSWindowOrderingMode::Below,
                    Some(&self.prose_switch),
                );
                self.prose = Some(pane);
            }
            self.prose_shown = true;
            let sync = self
                .vim
                .as_ref()
                .and_then(|v| v.eval("line('w0')"))
                .and_then(|l| l.trim().parse::<i64>().ok());
            let cfg = self.model.config.clone();
            let colors = self.colors;
            let frame = self.editor_scroll.frame();
            let pv = self.prose.as_mut().unwrap();
            pv.set_frame(frame);
            pv.view().setHidden(false);
            pv.sync_line = sync;
            let (font, size, width) = (cfg.prose_font.clone(), cfg.prose_font_size, cfg.prose_width);
            pv.show(ProseSource::new(markdown, path), colors, &font, size, width, &cfg);
            if let Some(w) = self.root.window() {
                w.makeFirstResponder(Some(pv.view()));
            }
        }

        /// Re-render the reading view after the note changed (doc style).
        pub fn refresh_prose(&mut self) {
            if self.prose_shown {
                // Keep `prose_shown` set: a re-render must not flip the flag
                // off first, or a failed render would leave the view up while
                // the capsule (and later clicks) think it is hidden.
                self.set_prose(true);
            }
        }

        /// Re-render the reading view for a specific opened `path` (used by
        /// `open_note`, where the path may not be the selected tab yet).
        fn refresh_prose_to(&mut self, path: &str) {
            if self.prose_shown {
                self.set_prose_to(true, Some(path));
            }
        }

        /// The open reading view, for key routing.
        pub fn prose_pane(&mut self) -> Option<&mut crate::views::notes_prose::ProsePane> {
            if self.prose_shown {
                self.prose.as_mut()
            } else {
                None
            }
        }

        /// `wsJump` / the close sync: put the vim pane's top line at `n`.
        pub fn prose_jump(&self, n: i64) {
            if let Some(vim) = &self.vim {
                vim.command(&format!("call cursor({n}, 1) | normal! zt"));
            }
        }

        /// `applyDocTemplateChange`: replace the first `remove` lines with
        /// `insert` — through nvim (`nvim_buf_set_lines` + `update`) in the vim
        /// pane, else in the editor (then saved).
        pub fn apply_doc_template_change(&mut self, remove: usize, insert: &[String]) {
            use crate::engines::nvim_rpc::{vim_dq, vim_string};
            if let Some(vim) = &self.vim {
                let lua = [
                    "(function(a)",
                    "local n = tonumber(a:match('^(%d+)\\n'))",
                    "local rest = a:gsub('^%d+\\n', '')",
                    "local new = rest == '' and {} or vim.split(rest, '\\n', {plain = true})",
                    "vim.api.nvim_buf_set_lines(0, 0, n, false, new)",
                    "vim.cmd('silent! update') return 1 end)(_A)",
                ]
                .join(" ");
                let arg = format!("{remove}\n{}", insert.join("\n"));
                vim.eval(&format!("luaeval({}, {})", vim_string(&lua), vim_dq(&arg)));
                return;
            }
            let text = self.editor_markdown();
            let mut end = 0usize;
            for _ in 0..remove {
                if end >= text.len() {
                    break;
                }
                end = text[end..].find('\n').map(|i| end + i + 1).unwrap_or(text.len());
            }
            let new_head = if insert.is_empty() { String::new() } else { insert.join("\n") + "\n" };
            let out = format!("{new_head}{}", &text[end..]);
            if let Some(path) = self.selected_path() {
                self.set_note_text(&out, &path);
            }
            self.commit();
        }

        /// `stepFontSizes(_:)` (Cmd+± in the notes window): editor and drawer
        /// terminal font sizes ±1, saved as `[notes] font-size` /
        /// `[app] terminal-font-size`.
        pub fn step_font(&mut self, delta: i32) {
            let d = delta as f64;
            let editor = setting("notes", "font-size").and_then(|v| v.parse::<f64>().ok()).unwrap_or(14.0);
            let next = (editor + d).clamp(8.0, 40.0);
            if let Some((name, _)) = self.vim.as_ref().and_then(|v| v.font()) {
                if let Some(vim) = self.vim.as_mut() {
                    vim.set_font(&name, next);
                }
                // `refreshVimImageRows`: SwiftTerm recomputes its grid from the
                // new font; re-sync the overlay + `g:ws_cell_h` to the real cell.
                self.refresh_vim_cell();
            }
            if let Some(f) = self.editor.font() {
                let nf = NSFont::fontWithName_size(&f.fontName(), next);
                self.editor.setFont(nf.as_deref());
                restyle_editor(&self.editor, self.colors);
            }
            crate::app::config::save_config_value("notes", "font-size", &format!("{}", next as i64));
            let term = setting("app", "terminal-font-size").and_then(|v| v.parse::<f64>().ok()).unwrap_or(12.0);
            let tnext = (term + d).clamp(8.0, 40.0);
            if let Ok(shim) = self.terminal.clone().downcast::<swiftterm_shim::WSShim>() {
                let tf = setting("app", "terminal-font").unwrap_or_else(|| "Menlo".to_string());
                swiftterm_shim::set_font(&shim, &tf, tnext);
            }
            crate::app::config::save_config_value("app", "terminal-font-size", &format!("{}", tnext as i64));
        }

        /// Give the reading view the keyboard (search keys land in the page).
        pub fn focus_prose(&self) {
            if let (true, Some(pv), Some(w)) = (self.prose_shown, &self.prose, self.root.window()) {
                w.makeFirstResponder(Some(pv.view()));
            }
        }

        /// Re-color in place after a theme change (`setThemeColor` /
        /// `setTextColors`) — the nvim pane and shell keep running.
        pub fn retheme(&mut self) {
            let colors = crate::ui::theme::section_colors("notes");
            self.colors = colors;
            self.sidebar.set_colors(colors);
            self.tree.set_colors(colors);
            self.prose_switch.set_colors(colors);
            self.editor_scroll.setBackgroundColor(&colors.base().to_nscolor());
            self.editor.setBackgroundColor(&colors.base().to_nscolor());
            self.editor.setTextColor(Some(&colors.text.to_nscolor()));
            restyle_editor(&self.editor, colors);
            let pal: Vec<Retained<objc2_app_kit::NSColor>> =
                crate::ui::theme::ansi_palette(&colors).into_iter().map(|c| c.to_nscolor()).collect();
            if let Some(view) = self.vim.as_ref().and_then(|p| p.view()) {
                if let Ok(shim) = view.downcast::<swiftterm_shim::WSShim>() {
                    swiftterm_shim::set_colors(&shim, &objc2_app_kit::NSColor::clearColor(), &colors.text.to_nscolor(), &pal);
                }
            }
            if let Ok(shim) = self.terminal.clone().downcast::<swiftterm_shim::WSShim>() {
                let sec = |k: &str| setting("notes", k).and_then(|v| crate::ui::theme::HexColor::parse(v.trim()));
                let bg = sec("terminal-background").unwrap_or(colors.background).with_alpha(1.0);
                let fg = sec("terminal-foreground").unwrap_or(colors.text);
                swiftterm_shim::set_colors(&shim, &bg.to_nscolor(), &fg.to_nscolor(), &pal);
            }
            self.root.setNeedsDisplay(true);
            self.refresh_prose();
        }

        /// The note text the doc-style menu reads (`proseProvider().markdown`).
        pub fn prose_markdown(&self) -> Option<String> {
            self.selected_path().and_then(|p| std::fs::read_to_string(p).ok())
        }

        /// `PopupTabsBar.toggleRail` — collapse / expand the notes sidebar.
        pub fn toggle_rail(&mut self) {
            self.model.sidebar.collapsed = !self.model.sidebar.collapsed;
            self.sidebar.set_collapsed(self.model.sidebar.collapsed);
            self.apply_sidebar_width();
        }

        /// The sidebar view flipped its own rail (toggle button).
        pub fn rail_changed(&mut self, collapsed: bool) {
            self.model.sidebar.collapsed = collapsed;
            self.apply_sidebar_width();
        }

        /// Re-apply the sidebar / editor / drawer geometry after a rail flip.
        fn apply_sidebar_width(&mut self) {
            let full = self.root.frame().size;
            let w = self.model.sidebar.visible_width();
            let show_tree = !self.tree_model.is_empty() && !self.model.sidebar.collapsed;
            let wanted = self.tree.wanted_height();
            let (tree_h, open_h) = crate::views::notes_tree::rail_split(full.height, show_tree, wanted);
            self.tree.setHidden(!show_tree);
            // The Open card only joins the tree card above while the tree shows.
            let mut sm = self.sidebar.model();
            if sm.join_top != show_tree {
                sm.join_top = show_tree;
                self.sidebar.set_model(sm);
            }
            self.tree.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(w, tree_h)));
            self.sidebar
                .setFrame(NSRect::new(NSPoint::new(0.0, tree_h), NSSize::new(w, open_h)));
            let editor_w = (full.width - w - 4.0).max(0.0);
            // `drawerInset`: an open terminal drawer takes the editor's bottom.
            let term_h = self.terminal.frame().size.height.max(TERMINAL_DRAWER_HEIGHT);
            let inset = if self.drawers.terminal_shown() { term_h } else { 0.0 };
            let editor_h = (full.height - inset).max(0.0);
            self.editor_scroll.setFrame(NSRect::new(
                NSPoint::new(w + 4.0, 0.0),
                NSSize::new(editor_w, editor_h),
            ));
            self.prose_switch.place(self.editor_scroll.frame());
            if let Some(p) = &self.prose {
                p.set_frame(self.editor_scroll.frame());
            }
            self.terminal.setFrame(NSRect::new(
                NSPoint::new(w + 4.0, full.height - term_h),
                NSSize::new(editor_w, term_h),
            ));
            if let Some(view) = self.vim.as_ref().and_then(|p| p.view()) {
                view.setFrame(NSRect::new(
                    NSPoint::new(w + 4.0 + 8.0, 4.0),
                    NSSize::new((editor_w - 16.0).max(0.0), (editor_h - 8.0).max(0.0)),
                ));
            }
            if let Some(ov) = &self.vim_images {
                ov.setFrame(NSRect::new(
                    NSPoint::new(w + 4.0 + 8.0, 4.0),
                    NSSize::new((editor_w - 16.0).max(0.0), (editor_h - 8.0).max(0.0)),
                ));
            }
            // Drawers above the editor (a relaunched nvim pane is re-added).
            if self.drawers.terminal_shown() {
                self.root.addSubview_positioned_relativeTo(&self.terminal, objc2_app_kit::NSWindowOrderingMode::Above, None);
            }
            if self.drawers.browser_shown() {
                self.root.addSubview_positioned_relativeTo(&self.browser, objc2_app_kit::NSWindowOrderingMode::Above, None);
            }
        }

        /// Rebuild the sidebar rows from the model (new tabs etc.).
        pub fn rebuild_sidebar_rows(&mut self) {
            self.repaint_rows();
        }

        /// `commitSave` — flush the vim pane, else write the editor text.
        pub fn commit(&mut self) -> bool {
            if self.vim.is_some() {
                self.vim_flush();
                return true;
            }
            let Some(path) = self.selected_path() else {
                return false;
            };
            let text = self.editor_markdown();
            std::fs::write(&path, text).is_ok()
        }

        /// `onOpenExternalPath` — open an absolute path as a note.
        pub fn open_external_path(&mut self, path: &str) {
            if !std::path::Path::new(path).exists() {
                return;
            }
            match self.model.tabs.tabs.iter().position(|t| t.path == path) {
                Some(i) => {
                    self.model.tabs.selected = i;
                }
                None => {
                    self.model.tabs.append(path);
                    save_dismissed(&self.model.tabs.dismissed_paths());
                    self.model.refresh_sidebar(now_secs());
                    self.rebuild_sidebar_rows();
                }
            }
            self.repaint_rows();
            self.open_note(path);
        }

        // -- find bar (Cmd+F / Enter / Esc) -----------------------------------

        /// `toggleFindBar`: show or hide the native `NSTextView` find bar.
        pub fn toggle_find(&mut self) {
            if self.find_shown {
                self.close_find();
            } else {
                self.find_command(objc2_app_kit::NSFindPanelAction::ShowFindPanel);
                self.find_shown = true;
            }
        }

        /// `findStep(_:)` — Return / Shift+Return while the editor is focused.
        pub fn find_step(&mut self, dir: i64) {
            let action = if dir >= 0 {
                objc2_app_kit::NSFindPanelAction::Next
            } else {
                objc2_app_kit::NSFindPanelAction::Previous
            };
            self.find_command(action);
        }

        /// `closeFindBar` — the Esc branch while the bar is up.
        pub fn close_find(&mut self) {
            if !self.find_shown {
                return;
            }
            self.find_shown = false;
            if let Some(window) = self.editor.window() {
                if let Some(fr) = window.firstResponder() {
                    let sel = objc2::sel!(cancelOperation:);
                    let any: &objc2::runtime::AnyObject =
                        unsafe { &*(objc2::rc::Retained::as_ptr(&fr) as *const objc2::runtime::AnyObject) };
                    let responds: bool = unsafe { objc2::msg_send![any, respondsToSelector: sel] };
                    if responds {
                        let _: () = unsafe { objc2::msg_send![any, cancelOperation: std::ptr::null::<objc2::runtime::AnyObject>()] };
                    }
                }
            }
            self.focus_editor();
        }

        /// Run one `NSFindPanelAction` against the editor (the action rides on
        /// a throwaway menu item's tag, the AppKit convention).
        fn find_command(&self, action: objc2_app_kit::NSFindPanelAction) {
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };
            let item = objc2_app_kit::NSMenuItem::new(mtm);
            item.setTag(action.0 as isize);
            unsafe { self.editor.performFindPanelAction(Some(&item)) };
        }

        /// Make the editor the first responder of its window (a no-op until the
        /// surface is attached to one).
        pub fn focus_editor(&self) {
            if let Some(pane) = &self.vim {
                pane.focus();
                return;
            }
            if let Some(window) = self.editor.window() {
                window.makeFirstResponder(Some(&self.editor));
            }
        }

        /// Whether the drawer terminal holds the keyboard (`focusedTerm()`).
        pub fn terminal_has_focus(&self) -> bool {
            if self.terminal.isHidden() {
                return false;
            }
            let Some(w) = self.terminal.window() else { return false };
            let Some(fr) = w.firstResponder() else { return false };
            let fr_ptr = objc2::rc::Retained::as_ptr(&fr) as *const objc2::runtime::AnyObject;
            let term_ptr = objc2::rc::Retained::as_ptr(&self.terminal) as *const objc2::runtime::AnyObject;
            if fr_ptr == term_ptr {
                return true;
            }
            let fr_any: &objc2::runtime::AnyObject = unsafe { &*fr_ptr };
            match fr_any.downcast_ref::<NSView>() {
                Some(v) => v.isDescendantOf(&self.terminal),
                None => false,
            }
        }

        /// `PopupWindow.navPanes` for the notes member: sidebar, editor
        /// (nvim / prose / text), browser drawer, terminal drawer.
        pub fn nav_panes(&self) -> Vec<crate::app::live_panes::LivePane> {
            use crate::app::live_panes::LivePane;
            let mut out = Vec::new();
            if !self.tree_model.is_empty() {
                out.push(LivePane::new("tree", &self.tree));
            }
            out.push(LivePane::new("sidebar", &self.sidebar));
            if self.prose_shown {
                if let Some(p) = &self.prose {
                    out.push(LivePane::new("editor", &p.web));
                }
            } else if let Some(v) = self.vim.as_ref().and_then(|v| v.view()) {
                let mut e = LivePane::new("editor", &v);
                e.vim_splits = true;
                out.push(e);
            } else {
                out.push(LivePane::new("editor", &self.editor_scroll).focusing(&self.editor));
            }
            if self.drawers.browser_shown() {
                out.push(LivePane::new("browser", &self.browser));
            }
            if self.drawers.terminal_shown() {
                out.push(LivePane::new("terminal", &self.terminal));
            }
            out
        }

        /// `vimSplitMove`: `wincmd` inside nvim when a split lies that way.
        pub fn vim_split_move(&self, dir: crate::panes::pane_geometry::PaneDir) -> bool {
            let Some(v) = self.vim.as_ref() else { return false };
            if !v.has_focus() {
                return false;
            }
            let k = dir.raw_value();
            match v.eval(&format!("winnr('{k}') != winnr()")) {
                Some(r) if r.trim() == "1" => {
                    v.command(&format!("wincmd {k}"));
                    true
                }
                _ => false,
            }
        }

        /// `openNotePaths` — every note open in the sidebar, in tab order.
        pub fn open_paths(&self) -> Vec<String> {
            self.model.tabs.paths()
        }

        /// `vimCommand(_:)` on the notes nvim pane.
        pub fn vim_command(&self, ex: &str) {
            if let Some(v) = &self.vim {
                v.command(ex);
            }
        }

        /// Whether the embedded nvim pane is active for this surface.
        pub fn vim_active(&self) -> bool {
            self.vim.is_some()
        }

        /// Whether the embedded nvim pane holds the keyboard (the `:q`
        /// relaunch's `TerminalAutoRestart` reclaim check).
        pub fn vim_has_focus(&self) -> bool {
            self.vim.as_ref().map(|p| p.has_focus()).unwrap_or(false)
        }

        /// The embedded nvim pane, when active.
        pub fn vim_pane(&self) -> Option<&crate::views::notes_vim::VimPane> {
            self.vim.as_ref()
        }

        /// `vimOpen(_:)` on the shared process (tab switch / `open:` message).
        pub fn vim_open(&mut self, path: &str) {
            if let Some(pane) = self.vim.as_mut() {
                pane.open(path);
                self.focus_editor();
            }
        }

        /// `openNoteFile(_:)`'s surface subset: the vim pane follows the path,
        /// the plain editor loads the file (used when vim-mode is off).
        pub fn open_note(&mut self, path: &str) {
            self.tree_reveal(path);
            // An open tab: keep the rail's selection in step.
            if let Some(i) = self.model.tabs.paths().iter().position(|p| p == path) {
                if self.model.tabs.selected != i {
                    self.model.tabs.selected = i;
                    self.repaint_rows();
                }
            }
            if self.vim.is_some() {
                self.vim_open(path);
                self.refresh_prose_to(path);
                return;
            }
            if let Ok(text) = std::fs::read_to_string(path) {
                self.set_note_text(&text, path);
            }
            self.focus_editor();
            self.refresh_prose_to(path);
        }

        /// `vimFlush()` — `silent! wall` before the window hides.
        pub fn vim_flush(&self) {
            if let Some(pane) = &self.vim {
                pane.flush();
            }
        }

        /// The drawer terminal's `TerminalAutoRestart`: `exit` in the shell
        /// starts a new one in the same view. Returns true when it restarted.
        pub fn terminal_poll_restart(&self) -> bool {
            let Ok(shim) = self.terminal.clone().downcast::<swiftterm_shim::WSShim>() else {
                return false;
            };
            let (shell, args, home) = drawer_shell();
            swiftterm_shim::restart_if_exited(&shim, &shell, &args, home.as_deref())
        }

        /// `onVimExit` — restart the editor on the current note after `:q` / a
        /// crash (same terminal view, `startVimIfNeeded`); returns true when a
        /// relaunch happened.
        pub fn vim_poll_exit(&mut self, _mtm: MainThreadMarker) -> bool {
            let Some(pane) = self.vim.as_mut() else {
                return false;
            };
            if !pane.poll_exit() {
                return false;
            }
            pane.restart()
        }
    }
}

// ---------------------------------------------------------------------------
// Find / grep window model
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteFindMode {
    Files,
    Grep,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NoteHit {
    pub path: String,
    pub rel: String,
    pub mtime: f64,
    pub tab: usize,
}

pub struct NoteFinder;

impl NoteFinder {
    /// `NoteFinder.score` — greedy subsequence with a filename bonus.
    pub fn score(hit: &NoteHit, q: &str) -> Option<i64> {
        if q.is_empty() {
            return Some(0);
        }
        let rel: Vec<char> = hit.rel.to_lowercase().chars().collect();
        let dir_len = Path::new(&hit.rel)
            .parent()
            .map(|p| p.to_string_lossy().len())
            .unwrap_or(0);
        let name_start = dir_len + if dir_len == 0 { 0 } else { 1 };

        let mut i = 0usize;
        let mut first: i64 = -1;
        let mut last: i64 = -1;
        for ch in q.chars() {
            let mut found = None;
            for j in i..rel.len() {
                if rel[j] == ch {
                    found = Some(j);
                    break;
                }
            }
            let j = found?;
            if first < 0 {
                first = j as i64;
            }
            last = j as i64;
            i = j + 1;
        }
        let span = last - first;
        let in_name = first >= name_start as i64;
        let base = if in_name { 0 } else { 1000 };
        let anchor = if in_name { name_start as i64 } else { 0 };
        Some(base + span * 4 + (first - anchor).max(0))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct GrepHit {
    pub path: String,
    pub line: i64,
    pub text: String,
}

/// `rgPath()` — the configured binary else the Homebrew / system locations.
pub fn rg_path(configured: Option<&str>) -> Option<String> {
    let mut candidates: Vec<String> = Vec::new();
    if let Some(c) = configured.filter(|c| !c.is_empty()) {
        candidates.push(expand_tilde(c));
    }
    candidates.extend([
        "/opt/homebrew/bin/rg".to_string(),
        "/usr/local/bin/rg".to_string(),
        "/usr/bin/rg".to_string(),
    ]);
    candidates.into_iter().find(|p| {
        std::fs::metadata(p)
            .map(|m| m.is_file())
            .unwrap_or(false)
    })
}

/// The ripgrep argv of `runGrep`.
pub fn rg_args(query: &str, paths: &[String]) -> Vec<String> {
    let sep = "\u{1f}";
    let mut a: Vec<String> = [
        "--line-number",
        "--no-heading",
        "--with-filename",
        "--color",
        "never",
        "--smart-case",
        "--max-columns",
        "300",
        "--max-columns-preview",
        "--field-match-separator",
        sep,
        "-e",
        query,
        "--",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    a.extend(paths.iter().cloned());
    a
}

/// Parse `rg --field-match-separator \x1f` output, capped like `runGrep`.
pub fn parse_rgrep(out: &str, cap: usize) -> Vec<GrepHit> {
    let sep = '\u{1f}';
    let mut hits = Vec::new();
    for line in out.split('\n').filter(|l| !l.is_empty()) {
        let parts: Vec<&str> = line.splitn(3, sep).collect();
        if parts.len() != 3 {
            continue;
        }
        let Ok(n) = parts[1].parse::<i64>() else {
            continue;
        };
        hits.push(GrepHit {
            path: parts[0].to_string(),
            line: n,
            text: parts[2].trim().to_string(),
        });
        if hits.len() >= cap {
            break;
        }
    }
    hits
}

/// `runGrep`'s final sort: open order, then line number.
pub fn sort_grep(hits: &mut [GrepHit], paths: &[String]) {
    let order = |p: &str| paths.iter().position(|x| x == p).unwrap_or(0);
    hits.sort_by(|a, b| (order(&a.path), a.line).cmp(&(order(&b.path), b.line)));
}

/// The `NoteFindWindow` model (both modes) without the PopupWindow/list UI.
#[derive(Clone, Debug)]
pub struct NoteFindModel {
    pub mode: NoteFindMode,
    pub query: String,
    pub all: Vec<NoteHit>,
    pub shown: Vec<NoteHit>,
    pub lines: Vec<i64>,
    /// The matched line's text per grep row (the row title).
    pub grep_texts: Vec<String>,
    pub selection: usize,
    pub grep_note: String,
    pub rows: usize,
    pub grep_max: usize,
}

impl NoteFindModel {
    pub fn new(mode: NoteFindMode, rows: usize, grep_max: usize) -> Self {
        NoteFindModel {
            mode,
            query: String::new(),
            all: Vec::new(),
            shown: Vec::new(),
            lines: Vec::new(),
            grep_texts: Vec::new(),
            selection: 0,
            grep_note: String::new(),
            rows: rows.max(5),
            grep_max: grep_max.max(1),
        }
    }

    pub fn set_open_paths(&mut self, paths: &[String]) {
        let home = std::env::var("HOME").unwrap_or_default();
        self.all = paths
            .iter()
            .enumerate()
            .map(|(i, p)| NoteHit {
                path: p.clone(),
                rel: tilde_path(p, &home),
                mtime: last_write(p).unwrap_or(0.0),
                tab: i,
            })
            .collect();
        self.reload();
    }

    pub fn set_query(&mut self, q: &str) {
        self.query = q.to_string();
        match self.mode {
            NoteFindMode::Files => self.reload(),
            NoteFindMode::Grep => self.grep_note_placeholder(),
        }
    }

    /// `reload` (`mode == .files`).
    pub fn reload(&mut self) {
        let q: String = self.query.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
        let mut scored: Vec<(usize, i64)> = self
            .all
            .iter()
            .enumerate()
            .filter_map(|(i, h)| NoteFinder::score(h, &q).map(|s| (i, s)))
            .collect();
        scored.sort_by(|a, b| {
            a.1.cmp(&b.1)
                .then_with(|| self.all[a.0].tab.cmp(&self.all[b.0].tab))
        });
        self.shown = scored
            .into_iter()
            .take(self.rows)
            .map(|(i, _)| self.all[i].clone())
            .collect();
        self.lines = vec![0; self.shown.len()];
        self.selection = 0;
    }

    /// The message `runGrep` shows while there is not enough to search.
    pub fn grep_note_placeholder(&mut self) {
        let paths: Vec<&NoteHit> = self
            .all
            .iter()
            .filter(|h| Path::new(&h.path).exists())
            .collect();
        let q = self.query.trim();
        if q.chars().count() < 2 || paths.is_empty() {
            self.grep_note = if paths.is_empty() {
                "No notes are open".to_string()
            } else {
                String::new()
            };
            self.show_grep(&[], paths.len());
        }
    }

    /// Apply a completed grep run.
    pub fn show_grep(&mut self, hits: &[GrepHit], open_count: usize) {
        self.shown = hits
            .iter()
            .map(|h| NoteHit {
                path: h.path.clone(),
                rel: Path::new(&h.path)
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default(),
                mtime: 0.0,
                tab: 0,
            })
            .collect();
        self.lines = hits.iter().map(|h| h.line).collect();
        self.grep_texts = hits.iter().map(|h| h.text.clone()).collect();
        self.selection = 0;
        if hits.is_empty() && open_count > 0 && self.query.trim().chars().count() >= 2 {
            self.grep_note = format!("No match for “{}”", self.query.trim());
        }
    }

    pub fn grep_hits_for(&self, hits: &[GrepHit], paths: &[String]) -> Vec<GrepHit> {
        let order = |p: &str| paths.iter().position(|x| x == p).unwrap_or(0);
        let mut out = hits.to_vec();
        out.sort_by(|a, b| (order(&a.path), a.line).cmp(&(order(&b.path), b.line)));
        out.truncate(self.grep_max);
        out
    }

    pub fn empty_message(&self) -> String {
        if self.mode == NoteFindMode::Grep {
            return self.grep_note.clone();
        }
        if self.all.is_empty() {
            "No notes are open".to_string()
        } else {
            format!("No file matches “{}”", self.query)
        }
    }

    pub fn move_selection(&mut self, d: i64) {
        if self.shown.is_empty() {
            return;
        }
        let n = self.shown.len() as i64;
        self.selection = ((self.selection as i64 + d).rem_euclid(n)) as usize;
    }

    /// `open(_:)` — the path plus the grep line (nil in files mode).
    pub fn open(&self, i: usize) -> Option<(String, Option<i64>)> {
        let h = self.shown.get(i)?;
        let line = if self.mode == NoteFindMode::Grep {
            self.lines.get(i).copied()
        } else {
            None
        };
        Some((h.path.clone(), line))
    }
}

// ---------------------------------------------------------------------------
// Inline rename model
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenameResult {
    pub from: String,
    pub to: String,
}

/// `InlineRename`: the field's initial selection + commit/cancel state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlineRenameModel {
    pub name: String,
    pub selected_len: usize,
    pub is_dir: bool,
    pub path: Option<String>,
    pub active: bool,
}

impl InlineRenameModel {
    /// `InlineRename.begin`: select the stem (or the whole name for a dir).
    pub fn begin(name: &str, is_dir: bool) -> Self {
        let stem_len = if is_dir {
            name.chars().count()
        } else {
            Path::new(name)
                .file_stem()
                .map(|s| s.to_string_lossy().chars().count())
                .unwrap_or_else(|| name.chars().count())
        };
        InlineRenameModel {
            name: name.to_string(),
            selected_len: stem_len.max(if is_dir { 0 } else { name.chars().count().min(0) }),
            is_dir,
            path: None,
            active: true,
        }
    }

    pub fn set_path(&mut self, p: &str) {
        self.path = Some(p.to_string());
    }

    /// `InlineRename.end` — `Some((path, text))`, or `None` if not begun.
    pub fn end(&mut self, text: &str) -> Option<(String, String)> {
        let path = self.path.take()?;
        self.active = false;
        Some((path, text.to_string()))
    }

    pub fn cancel(&mut self) {
        self.path = None;
        self.active = false;
    }
}

// ---------------------------------------------------------------------------
// File popup (model shared with `views::file_popup`, the live window)
// ---------------------------------------------------------------------------

// Re-exported for the tests and the historical `notes::FilePopup` path.
#[allow(unused_imports)]
pub use crate::views::file_popup::{
    FilePopup, FilePopupRoute, FILE_POPUP_IMAGE_EXTS, FILE_POPUP_TEXT_LIMIT,
};

/// The target of the markdown image link `![alt](target)` that covers the
/// UTF-16 offset `idx` (an `NSTextView` character index), if any. A
/// `"title"` after the target and `<…>` brackets are dropped.
pub fn image_link_at(text: &str, idx: usize) -> Option<String> {
    let mut start16 = 0usize;
    for line in text.split_inclusive('\n') {
        let len16 = line.encode_utf16().count();
        if idx < start16 + len16 || start16 + len16 == text.encode_utf16().count() {
            let col = idx.saturating_sub(start16);
            let mut search = 0;
            while let Some(rel) = line[search..].find("![") {
                let open = search + rel;
                let Some(mid) = line[open..].find("](").map(|m| open + m) else { break };
                let Some(close) = line[mid..].find(')').map(|c| mid + c) else { break };
                let (a, b) = (line[..open].encode_utf16().count(), line[..=close].encode_utf16().count());
                if col >= a && col < b {
                    let inner = line[mid + 2..close].trim();
                    let target = inner.split_once(" \"").map(|(t, _)| t).unwrap_or(inner).trim();
                    let target = target.trim_start_matches('<').trim_end_matches('>');
                    return (!target.is_empty()).then(|| target.to_string());
                }
                search = close + 1;
            }
            return None;
        }
        start16 += len16;
    }
    None
}

/// Resolve an image link target against the note's folder (`imageBaseDir`):
/// absolute and `~` paths stay, relative ones join the note's directory.
pub fn resolve_note_link(target: &str, note_path: &str, home: &str) -> String {
    if target.starts_with('/') {
        return target.to_string();
    }
    if let Some(rest) = target.strip_prefix("~/") {
        return format!("{home}/{rest}");
    }
    let dir = Path::new(note_path).parent().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
    format!("{dir}/{}", target.trim_start_matches("./"))
}

// ---------------------------------------------------------------------------
// Doc templates (helper façade, mirrors the Swift docTemplateMenu flow)
// ---------------------------------------------------------------------------

/// `doc_templates.menu` via the helper.
pub fn doc_template_menu(
    text: &str,
    configured: &str,
    css: &str,
) -> Option<(Option<String>, Vec<String>)> {
    let v = helper_call(
        "doc_templates.menu",
        json!({"text": text, "configured": configured, "css": css}),
        3,
    )?;
    let current = v.get("current").and_then(Value::as_str).map(str::to_string);
    let names = v
        .get("names")?
        .as_array()?
        .iter()
        .filter_map(|n| n.as_str().map(str::to_string))
        .collect();
    Some((current, names))
}

/// `[notes] doc-templates` list, comma-split like `DocTemplates.names`.
pub fn doc_template_names(configured: Option<&str>) -> Vec<String> {
    let configured = configured.unwrap_or("");
    let names: Vec<String> = configured
        .split(',')
        .map(|w| w.trim().to_string())
        .filter(|w| !w.is_empty())
        .collect();
    if !names.is_empty() {
        return names;
    }
    vec![
        "paper".into(),
        "terminal".into(),
        "executive".into(),
    ]
}

// ---------------------------------------------------------------------------
// Notes view (SlotMember) + palette registration
// ---------------------------------------------------------------------------

/// Everything the notes view draws, pushed by the model: the pinned-folder
/// tree (from `notes_tree.rs`), the open-note tabs, and the editor's file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NotesSnapshot {
    /// The flattened tree rows (`NotesTree::rows()`): roots + expanded children.
    pub tree: Vec<TreeRow>,
    /// The tree path highlighted as "the open file" (or a selected folder).
    pub tree_selected: Option<String>,
    /// The open note tabs, in tab order.
    pub tabs: Vec<NoteTab>,
    /// The selected tab index.
    pub selected: usize,
    /// The file the editor is showing (`None` when no tab is open).
    pub path: Option<String>,
    /// The editor's current text (unsaved edits live here until `Save`).
    pub text: String,
}

/// Clicks / typing the frontend reports back to [`NotesView`].
#[derive(Clone, Debug, PartialEq)]
pub enum NotesEvent {
    /// A click in the pinned-folder tree: folders toggle, files open a tab.
    TreeClick { path: String, is_dir: bool },
    /// Select an open tab (`select_row`).
    SelectTab(usize),
    /// Close an open tab by index (`close_tab`).
    CloseTab(usize),
    /// The editor text changed.
    Edit(String),
    /// Write the current text back to the selected file (`commitSave`).
    Save,
}

pub struct NotesView {
    pub config: NotesConfig,
    pub tabs: NotesTabs,
    pub sidebar: NotesSidebar,
    pub prose: ProseViewState,
    /// The pinned-folder tree (the rail's top card); empty until roots are set.
    pub tree: NotesTree,
    /// The file the editor is showing (mirrors the selected tab).
    pub path: Option<String>,
    /// The editor's text (unsaved edits until `Save`).
    pub text: String,
    shown: bool,
    key: bool,
}

impl NotesView {
    pub fn new(config: NotesConfig) -> Self {
        let sidebar = NotesSidebar::new(config.sidebar_width);
        NotesView {
            config,
            tabs: NotesTabs::new(),
            sidebar,
            prose: ProseViewState::new(),
            tree: NotesTree::default(),
            path: None,
            text: String::new(),
            shown: false,
            key: false,
        }
    }

    pub fn refresh_sidebar(&mut self, now: f64) {
        self.sidebar
            .rebuild(&self.tabs, now, &|p| last_write(p));
    }

    pub fn set_key(&mut self, key: bool) {
        self.key = key;
    }

    /// The full view state for a frontend to draw.
    pub fn snapshot(&self) -> NotesSnapshot {
        NotesSnapshot {
            tree: self.tree.rows(),
            tree_selected: self.tree.selected.clone(),
            tabs: self.tabs.tabs.clone(),
            selected: self.tabs.selected,
            path: self.path.clone(),
            text: self.text.clone(),
        }
    }

    /// Apply one user action, calling the same model operations the AppKit half
    /// calls (`tree_click` / `select_row` / `close_tab` / `commitSave`).
    pub fn handle(&mut self, ev: NotesEvent) {
        match ev {
            NotesEvent::TreeClick { path, is_dir } => {
                if is_dir {
                    self.tree.toggle(&path);
                } else {
                    self.open_path(&path);
                }
            }
            NotesEvent::SelectTab(i) => {
                if i < self.tabs.tabs.len() {
                    self.tabs.selected = i;
                    self.load_selected();
                }
            }
            NotesEvent::CloseTab(i) => {
                if self.tabs.close(i).is_some() {
                    let n = self.tabs.tabs.len();
                    if n == 0 {
                        self.path = None;
                        self.text.clear();
                    } else {
                        self.tabs.selected = self.tabs.selected.min(n - 1);
                        self.load_selected();
                    }
                }
            }
            NotesEvent::Edit(text) => self.text = text,
            NotesEvent::Save => {
                if let Some(p) = &self.path {
                    if std::fs::write(p, &self.text).is_ok() {
                        self.refresh_sidebar(now_secs());
                    }
                }
            }
        }
    }

    /// Open a file from the tree (or an existing tab): reveal its ancestors,
    /// select it and load its text (`openExternalPath` + `openNote`).
    fn open_path(&mut self, path: &str) {
        if !Path::new(path).exists() {
            return;
        }
        match self.tabs.tabs.iter().position(|t| t.path == path) {
            Some(i) => self.tabs.selected = i,
            None => {
                self.tabs.append(path);
            }
        }
        self.tree.reveal(path);
        self.refresh_sidebar(now_secs());
        self.load_selected();
    }

    /// Point the editor at the selected tab and read its file (`set_note_text`).
    fn load_selected(&mut self) {
        self.path = self.tabs.selected_path().map(str::to_string);
        self.text = self
            .path
            .as_deref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default();
    }
}

impl Default for NotesView {
    fn default() -> Self {
        NotesView::new(NotesConfig::default())
    }
}

impl SlotMember for NotesView {
    fn view(&self) -> SlotView {
        SlotView::Notes
    }
    fn shown(&self) -> bool {
        self.shown
    }
    fn is_key(&self) -> bool {
        self.key
    }
    fn slot_show(&self, _frame: Option<RectI>) {
        // The host maps this to the shared window; the pure model only tracks
        // the flag via `set_key` + the returned state.
    }
    fn slot_park(&self, _stop_voice: bool) {}

    fn test_state(&self) -> Value {
        json!({
            "view": "notes",
            "tabs": self.tabs.titles(),
            "paths": self.tabs.paths(),
            "selected": self.tabs.selected,
            "sidebar": {
                "label": self.sidebar.label,
                "width": self.sidebar.visible_width(),
                "collapsed": self.sidebar.collapsed,
                "rows": self.sidebar.rows.iter().map(|r| json!({
                    "title": r.title,
                    "icon": r.icon,
                    "trailing": r.trailing,
                })).collect::<Vec<_>>(),
            },
            "prose": { "zoom": self.prose.zoom, "search": self.prose.search_state.as_str() },
        })
    }
}

/// Register the notes palette entries + a `do:notes:*` test table.
pub fn register(reg: &mut Registry) {
    reg.add_palette(PaletteCommand::new("notes", "Notes", "views"));
    reg.add_palette(PaletteCommand::new("notes-find", "Find Notes", "views"));
    reg.add_palette(PaletteCommand::new("notes-grep", "Search Notes", "views"));
    reg.register_test_do(|action| match action {
        "notes:state" => Some(json!({"available": true})),
        _ => None,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_link_at_finds_the_link_under_the_cursor() {
        let t = "intro\nsee ![shot](img/a.png \"t\") and ![](<b c.png>)\nend";
        let at = |needle: &str| t.encode_utf16().count() - t[t.find(needle).unwrap()..].encode_utf16().count();
        assert_eq!(image_link_at(t, at("shot")), Some("img/a.png".into()));
        assert_eq!(image_link_at(t, at("b c")), Some("b c.png".into()));
        assert_eq!(image_link_at(t, at("and")), None);
        assert_eq!(image_link_at(t, at("intro")), None);
        assert_eq!(resolve_note_link("img/a.png", "/n/notes.md", "/h"), "/n/img/a.png");
        assert_eq!(resolve_note_link("~/x.png", "/n/notes.md", "/h"), "/h/x.png");
        assert_eq!(resolve_note_link("/abs.png", "/n/notes.md", "/h"), "/abs.png");
    }
    use serde_json::json;

    fn helper_ready() -> bool {
        let lib = format!("{}/../../pylib", env!("CARGO_MANIFEST_DIR"));
        if !crate::app::python_helper::lib_has_helper(&lib) {
            return false;
        }
        let h = crate::app::python_helper::PythonHelper::shared();
        h.configure(&lib);
        h.call_default("ping", json!({})).is_ok()
    }

    const SNIPPETS: &str = r##"{
        "doc_x": {"body": ["<div class=\"doc paper\" data-foot=\"${1:Confidential}\"></div>", "", "# ${2:Title}", "", "${0}"]},
        "doc_y": {"body": "plain \\$5 body"}
    }"##;

    #[test]
    fn slugify_collapses_whitespace() {
        assert_eq!(slugify("My New  Note"), "My-New-Note");
        assert_eq!(slugify("  a\tb "), "a-b");
        assert_eq!(slugify("plain"), "plain");
        assert_eq!(slugify("   "), "");
    }

    #[test]
    fn snippet_strips_placeholders_and_ends_newline() {
        let all: Value = serde_json::from_str(SNIPPETS).unwrap();
        let body = snippet_text(&all, "doc_x").unwrap();
        assert_eq!(body, "<div class=\"doc paper\" data-foot=\"Confidential\"></div>\n\n# Title\n\n");
        assert!(body.ends_with('\n'));
        assert!(snippet_text(&all, "nope").is_none());
    }

    #[test]
    fn snippet_keeps_escaped_dollar() {
        let all: Value = serde_json::from_str(SNIPPETS).unwrap();
        // body "plain \$5 body": `\$` is not stripped (lookbehind), then unescaped.
        assert_eq!(snippet_text(&all, "doc_y").unwrap(), "plain $5 body\n");
    }

    #[test]
    fn snippet_missing_body_is_empty() {
        let all: Value = json!({"x": {"prefix": "x"}});
        assert_eq!(snippet_text(&all, "x").unwrap(), "\n");
    }

    #[test]
    fn new_note_path_counts_and_skips_open() {
        let existing = |p: &str| p.ends_with("-1.md") || p.ends_with("-2.md");
        let open = vec!["/n/Untitled-3.md".to_string()];
        let path = new_note_path("/n", "Untitled", &open, &existing);
        assert_eq!(path, "/n/Untitled-4.md");
    }

    #[test]
    fn new_note_path_slugifies_base() {
        let path = new_note_path("/n", "My Doc", &[], &|_| false);
        assert_eq!(path, "/n/My-Doc-1.md");
    }

    #[test]
    fn plan_new_note_uses_doc_template() {
        let all: Value = serde_json::from_str(SNIPPETS).unwrap();
        let mut cfg = NotesConfig::default();
        cfg.new_doc_template = "doc_x".to_string();
        let plan = plan_new_note("/n", true, &all, &cfg, &[]);
        assert_eq!(plan.path, "/n/doc-1.md");
        assert!(plan.body.contains("data-foot=\"Confidential\""));
        assert!(!plan.snippet_missing);

        cfg.new_doc_template = "missing".to_string();
        let plan = plan_new_note("/n", false, &all, &cfg, &[]);
        assert_eq!(plan.path, "/n/Untitled-1.md");
        assert!(plan.snippet_missing);
        assert!(plan.body.is_empty());
    }

    #[test]
    fn tabs_append_open_external_close_and_dismiss() {
        let mut t = NotesTabs::new();
        t.append("/n/A.md");
        t.append("/n/B.md");
        assert_eq!(t.titles(), vec!["A.md", "B.md"]);
        assert_eq!(t.selected, 1);
        assert_eq!(t.selected_path(), Some("/n/B.md"));

        assert!(t.open_external("/n/A.md"));
        assert_eq!(t.selected, 0);
        assert_eq!(t.len(), 2);
        assert!(!t.open_external("/n/C.md"));
        assert_eq!(t.len(), 3);
        assert_eq!(t.selected_path(), Some("/n/C.md"));

        t.dismiss("/n/C.md");
        assert!(t.is_dismissed("/n/C.md"));
        t.append("/n/D.md");
        assert!(!t.is_dismissed("/n/D.md"));
        assert!(t.is_dismissed("/n/C.md"));

        let removed = t.close(0).unwrap();
        assert_eq!(removed, "/n/A.md");
        assert_eq!(t.titles(), vec!["B.md", "C.md", "D.md"]);
    }

    #[test]
    fn dismissed_notes_survive_close() {
        let mut t = NotesTabs::new();
        t.append("/n/dead.md");
        t.dismiss("/n/dead.md");
        t.close(0);
        assert!(t.is_dismissed("/n/dead.md"));
    }

    #[test]
    fn sidebar_rebuilds_with_last_write() {
        let mut tabs = NotesTabs::new();
        tabs.append("/n/A.md");
        tabs.append("/n/B.md");
        let times = |p: &str| if p.ends_with("A.md") { Some(1000.0) } else { None };
        let mut side = NotesSidebar::new(200.0);
        side.rebuild(&tabs, 1000.0 + 300.0, &times);
        assert_eq!(side.rows.len(), 2);
        assert_eq!(side.rows[0].title, "A.md");
        assert_eq!(side.rows[0].trailing.as_deref(), Some("5m"));
        assert_eq!(side.rows[1].trailing, None);
        assert_eq!(side.visible_width(), 200.0);
        side.collapsed = true;
        assert_eq!(side.visible_width(), RAIL_WIDTH);
    }

    #[test]
    fn sidebar_width_clamps() {
        let side = NotesSidebar::new(10.0);
        assert_eq!(side.width, SIDEBAR_MIN_WIDTH);
        let side = NotesSidebar::new(9999.0);
        assert_eq!(side.width, SIDEBAR_MAX_WIDTH);
    }

    #[test]
    fn age_text_buckets() {
        assert_eq!(age_text(0.0), "now");
        assert_eq!(age_text(59.0), "now");
        assert_eq!(age_text(600.0), "10m");
        assert_eq!(age_text(7200.0), "2h");
        assert_eq!(age_text(200000.0), "2d");
    }

    // ---- prose ----------------------------------------------------------

    #[test]
    fn inline_markdown_passes() {
        assert_eq!(ProseRender::inline("a `b` c"), "a <code>b</code> c");
        assert_eq!(ProseRender::inline("**bold**"), "<strong>bold</strong>");
        assert_eq!(ProseRender::inline("*it*"), "<em>it</em>");
        assert_eq!(ProseRender::inline("~~x~~"), "<del>x</del>");
        assert_eq!(
            ProseRender::inline("[t](https://a.b)"),
            "<a href=\"https://a.b\">t</a>"
        );
        assert_eq!(
            ProseRender::inline("![alt](p.png)"),
            "<img alt=\"alt\" src=\"p.png\">"
        );
    }

    #[test]
    fn basic_markdown_structures() {
        let html = ProseRender::basic("# H\n\np1\np2\n\n- a\n- b\n");
        assert!(html.contains("<h1>H</h1>"));
        assert!(html.contains("<p>p1 p2</p>"));
        assert!(html.contains("<ul>"));
        assert!(html.contains("<li>a</li>"));
        assert!(html.contains("</ul>"));
    }

    #[test]
    fn basic_task_list_and_fence() {
        let html = ProseRender::basic("- [ ] todo\n- [x] done\n\n```rust\nlet x = 1;\n```\n");
        assert!(html.contains("class=\"task\""));
        assert!(html.contains("checked"));
        assert!(html.contains("<pre class=\"rust\"><code>let x = 1;</code></pre>"));
    }

    #[test]
    fn body_inner_and_patch_js() {
        let html = "<html><head></head><body><p>hi</p></body></html>";
        assert_eq!(ProseRender::body_inner(html).as_deref(), Some("<p>hi</p>"));
        let js = ProseRender::patch_js("<p>hi</p>");
        assert!(js.contains("document.body.innerHTML=\"<p>hi</p>\""));
        assert!(js.contains("__wsFindRefresh"));
        assert!(ProseRender::body_inner("no body").is_none());
    }

    #[test]
    fn head_override_has_base_and_width() {
        let src = ProseSource::new("# T", "/n/notes/a.md");
        let o = ProseRender::head_override(&src, 820.0, Some("CSS".to_string()));
        assert!(o.contains("file:///n/notes/"));
        assert!(o.contains("max-width: 820px"));
        assert!(o.contains("id=\"ws-copy\""));
    }

    #[test]
    fn theme_css_has_palette_vars() {
        let css = ProseRender::theme_css(&PopupColors::default());
        assert!(css.contains("--p-bg: #"));
        assert!(css.contains("--p-selection: rgba("));
    }

    #[test]
    fn search_key_routing() {
        let mut v = ProseViewState::new();
        // idle: Esc not handled, Cmd+F opens.
        assert_eq!(v.search_key(KEY_ESC, Mods::NONE, 0.0), None);
        assert_eq!(
            v.search_key(KEY_F, Mods::CMD, 0.0),
            Some(ProseSearchKey::OpenFind)
        );
        // while typing, everything but Esc/Cmd+F passes through.
        v.set_search_state("typing");
        assert_eq!(
            v.search_key(KEY_SLASH, Mods::NONE, 0.0),
            Some(ProseSearchKey::PassThrough)
        );
        assert_eq!(
            v.search_key(KEY_ESC, Mods::NONE, 0.0),
            Some(ProseSearchKey::CloseFind)
        );
        v.set_search_state("active");
        assert_eq!(
            v.search_key(45, Mods::NONE, 0.0),
            Some(ProseSearchKey::FindStep { back: false })
        );
        assert_eq!(
            v.search_key(45, Mods::SHIFT, 0.0),
            Some(ProseSearchKey::FindStep { back: true })
        );
        assert_eq!(
            v.search_key(44, Mods::NONE, 0.0),
            Some(ProseSearchKey::OpenFind)
        );
        assert_eq!(
            v.search_key(5, Mods::SHIFT, 0.0),
            Some(ProseSearchKey::GoBottom)
        );
        assert_eq!(
            v.search_key(2, Mods::CTRL, 0.0),
            Some(ProseSearchKey::ScrollHalf { down: true })
        );
    }

    #[test]
    fn scroll_key_covers_every_vim_motion() {
        let mut v = ProseViewState::new();
        assert_eq!(v.scroll_key(crate::ui::popup::KEY_J, Mods::NONE, 0.0), Some(ProseScroll::By(PROSE_LINE_STEP)));
        assert_eq!(v.scroll_key(crate::ui::popup::KEY_K, Mods::NONE, 0.0), Some(ProseScroll::By(-PROSE_LINE_STEP)));
        assert_eq!(v.scroll_key(2, Mods::CTRL, 0.0), Some(ProseScroll::HalfPage { down: true }));
        assert_eq!(v.scroll_key(32, Mods::CTRL, 0.0), Some(ProseScroll::HalfPage { down: false }));
        assert_eq!(v.scroll_key(5, Mods::SHIFT, 0.0), Some(ProseScroll::Bottom));
        // gg: the first g is swallowed, the second within 0.6 s goes to the top.
        assert_eq!(v.scroll_key(5, Mods::NONE, 10.0), Some(ProseScroll::Pending));
        assert_eq!(v.scroll_key(5, Mods::NONE, 10.4), Some(ProseScroll::Top));
        // ...but a slow second g starts over.
        assert_eq!(v.scroll_key(5, Mods::NONE, 20.0), Some(ProseScroll::Pending));
        assert_eq!(v.scroll_key(5, Mods::NONE, 21.0), Some(ProseScroll::Pending));
    }

    #[test]
    fn scroll_key_ignores_find_and_modified_keys() {
        let mut v = ProseViewState::new();
        // No find bar in the pop-out: `/`, Cmd+F, `n`, Esc and Cmd/Ctrl+j are not scrolls.
        assert_eq!(v.scroll_key(KEY_SLASH, Mods::NONE, 0.0), None);
        assert_eq!(v.scroll_key(KEY_F, Mods::CMD, 0.0), None);
        assert_eq!(v.scroll_key(KEY_N, Mods::NONE, 0.0), None);
        assert_eq!(v.scroll_key(KEY_ESC, Mods::NONE, 0.0), None);
        assert_eq!(v.scroll_key(crate::ui::popup::KEY_J, Mods::CMD, 0.0), None);
        assert_eq!(v.scroll_key(crate::ui::popup::KEY_J, Mods::CTRL, 0.0), None);
    }

    #[test]
    fn prose_scroll_js() {
        assert_eq!(ProseScroll::By(60).js().as_deref(), Some("window.scrollBy(0, 60)"));
        assert_eq!(ProseScroll::By(-60).js().as_deref(), Some("window.scrollBy(0, -60)"));
        assert_eq!(ProseScroll::Top.js().as_deref(), Some("window.scrollTo(0, 0)"));
        assert!(ProseScroll::Bottom.js().unwrap().contains("scrollHeight"));
        assert!(ProseScroll::HalfPage { down: false }.js().unwrap().contains("-window.innerHeight/2"));
        assert_eq!(ProseScroll::Pending.js(), None);
    }

    #[test]
    fn search_key_double_g() {
        let mut v = ProseViewState::new();
        v.set_search_state("active");
        assert_eq!(
            v.search_key(5, Mods::NONE, 10.0),
            Some(ProseSearchKey::PassThrough)
        );
        assert_eq!(v.search_key(5, Mods::NONE, 10.5), Some(ProseSearchKey::GoTop));
        assert_eq!(
            v.search_key(5, Mods::NONE, 12.0),
            Some(ProseSearchKey::PassThrough)
        );
    }

    #[test]
    fn zoom_clamps() {
        let mut v = ProseViewState::new();
        assert_eq!(v.apply_zoom(10.0), PROSE_MAX_ZOOM);
        assert_eq!(v.apply_zoom(0.1), PROSE_MIN_ZOOM);
        v.reset_zoom();
        assert_eq!(v.zoom_by(1.1), 1.1);
    }

    #[test]
    fn prose_launch_parses_colors() {
        let hex: Vec<String> = (1..=11)
            .map(|i| format!("{:08X}", i * 0x01010101u32))
            .collect();
        let args: Vec<String> = vec![
            "--colors".into(),
            hex.join(","),
            "--font".into(),
            "Menlo".into(),
            "--size".into(),
            "21".into(),
            "--width".into(),
            "800".into(),
            "/n/a.md".into(),
        ];
        let o = ProseLaunchOptions::parse(&args);
        assert_eq!(o.font, "Menlo");
        assert_eq!(o.size, 21.0);
        assert_eq!(o.width, 800.0);
        assert_eq!(o.path, "/n/a.md");
        assert_eq!(o.colors.background, parse_hex_color(&hex[0]).unwrap());
        assert_ne!(o.colors.palette, PopupPalette::default());
    }

    #[test]
    fn prose_launch_bad_color_count_keeps_default() {
        let o = ProseLaunchOptions::parse(&["--colors".into(), "ZZ".into(), "/n/a".into()]);
        assert_eq!(o.colors, PopupColors::default());
        assert_eq!(o.path, "/n/a");
    }

    #[test]
    fn prose_launch_args_round_trip() {
        let opts = ProseLaunchOptions {
            colors: PopupColors::default(),
            font: "Menlo".into(),
            size: 21.0,
            width: 800.0,
            path: "/n/a.md".into(),
        };
        let csv = opts.colors_csv();
        assert_eq!(csv.split(',').count(), 11, "6 base + 5 palette");
        assert_eq!(ProseLaunchOptions::color_hex(Rgba::new(1.0, 0.0, 0.0, 1.0)), "FF0000FF");

        let args = opts.to_args();
        assert_eq!(args[0], "--colors");
        assert_eq!(args[1], csv);
        assert_eq!(&args[2..4], &["--font", "Menlo"]);
        assert_eq!(args[args.len() - 1], "/n/a.md");

        let parsed = ProseLaunchOptions::parse(&args);
        assert_eq!(parsed.path, "/n/a.md");
        assert_eq!(parsed.font, "Menlo");
        assert_eq!(parsed.size, 21.0);
        assert_eq!(parsed.width, 800.0);
        assert_eq!(parsed.colors, PopupColors::default());
    }

    #[test]
    fn existing_note_paths_trims_expands_and_filters() {
        let home = std::env::var("HOME").unwrap_or_default();
        let raw = "~/a.md, /tmp/b.md ,, /missing.md";
        let got = existing_note_paths(raw, &|p| p.ends_with("a.md") || p == "/tmp/b.md");
        assert_eq!(got, vec![format!("{home}/a.md"), "/tmp/b.md".to_string()]);
        assert!(existing_note_paths("  ,  ", &|_| true).is_empty());
    }

    // ---- find / grep ----------------------------------------------------

    #[test]
    fn prose_launch_target_guards_selection_and_existence() {
        // No selection, or an empty path: safe no-op.
        assert_eq!(prose_launch_target(None, |_| true), None);
        assert_eq!(prose_launch_target(Some(""), |_| true), None);
        // Selected path that no longer exists: safe no-op.
        assert_eq!(prose_launch_target(Some("/n/gone.md"), |_| false), None);
        // Selected, existing path: returned for launch.
        assert_eq!(
            prose_launch_target(Some("/n/note.md"), |p| p == "/n/note.md"),
            Some("/n/note.md".to_string())
        );
    }

    #[test]
    fn prose_title_strip_hit_test() {
        // The top 28 pt of the window (y up) is the drag/zoom strip.
        assert!(prose_in_title_strip(600.0 - 1.0, 600.0));
        assert!(prose_in_title_strip(600.0 - PROSE_TITLE_STRIP + 0.5, 600.0));
        // Just below it, and the very bottom, are the page.
        assert!(!prose_in_title_strip(600.0 - PROSE_TITLE_STRIP, 600.0));
        assert!(!prose_in_title_strip(0.0, 600.0));
    }

    #[test]
    fn prose_zoom_toggle_fills_then_restores() {
        use crate::platform::screens::Rect;
        let cur = Rect::new(100.0, 100.0, 400.0, 300.0);
        let visible = Rect::new(0.0, 0.0, 1440.0, 900.0);
        // First double-click: remember the frame and fill the screen.
        let (frame, saved) = prose_zoom_toggle(cur, None, visible);
        assert_eq!(frame, visible);
        assert_eq!(saved, Some(cur));
        // Second double-click: put the saved frame back and clear the save.
        let (frame, saved) = prose_zoom_toggle(visible, saved, visible);
        assert_eq!(frame, cur);
        assert_eq!(saved, None);
    }

    #[test]
    fn finder_score_prefers_name_and_span() {
        let name_hit = NoteHit {
            path: "/n/a/Hello.md".into(),
            rel: "a/Hello.md".into(),
            mtime: 0.0,
            tab: 0,
        };
        let dir_hit = NoteHit {
            path: "/n/Hello/a.md".into(),
            rel: "Hello/a.md".into(),
            mtime: 0.0,
            tab: 1,
        };
        let in_name = NoteFinder::score(&name_hit, "hello").unwrap();
        let in_dir = NoteFinder::score(&dir_hit, "hello").unwrap();
        assert!(in_name < in_dir, "{in_name} < {in_dir}");
        assert_eq!(NoteFinder::score(&name_hit, ""), Some(0));
        assert_eq!(NoteFinder::score(&name_hit, "zzz"), None);
    }

    #[test]
    fn parse_rgrep_and_sort() {
        let sep = '\u{1f}';
        let out = format!("/n/b.md{sep}10{sep}foo bar\n/n/a.md{sep}2{sep}  foo  \n/badline");
        let hits = parse_rgrep(&out, 200);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].text, "foo bar");
        assert_eq!(hits[1].text, "foo");
        assert_eq!(hits[1].line, 2);

        let paths = vec!["/n/a.md".to_string(), "/n/b.md".to_string()];
        let mut h = hits;
        sort_grep(&mut h, &paths);
        assert_eq!(h[0].path, "/n/a.md");
        assert_eq!(h[1].path, "/n/b.md");
    }

    #[test]
    fn parse_rgrep_respects_cap() {
        let sep = '\u{1f}';
        let out: String = (0..10)
            .map(|i| format!("/n/a.md{sep}{i}{sep}x\n"))
            .collect();
        assert_eq!(parse_rgrep(&out, 3).len(), 3);
    }

    #[test]
    fn rg_args_shape() {
        let a = rg_args("q", &["/n/a.md".into()]);
        assert_eq!(a[0], "--line-number");
        assert!(a.contains(&"\u{1f}".to_string()));
        assert!(a.contains(&"-e".to_string()));
        assert_eq!(a[a.len() - 1], "/n/a.md");
        assert_eq!(a[a.len() - 2], "--");
    }

    #[test]
    fn find_model_files_mode() {
        let mut m = NoteFindModel::new(NoteFindMode::Files, 12, 200);
        m.all = vec![
            NoteHit { path: "/n/Beta.md".into(), rel: "~/Beta.md".into(), mtime: 0.0, tab: 0 },
            NoteHit { path: "/n/Alpha.md".into(), rel: "~/Alpha.md".into(), mtime: 0.0, tab: 1 },
        ];
        m.set_query("alpha");
        assert_eq!(m.shown.len(), 1);
        assert_eq!(m.open(0).unwrap().0, "/n/Alpha.md");
        assert_eq!(m.open(0).unwrap().1, None);
        assert_eq!(m.empty_message(), "No file matches “alpha”");
    }

    #[test]
    fn find_model_grep_placeholder_and_show() {
        let mut m = NoteFindModel::new(NoteFindMode::Grep, 12, 200);
        m.grep_note_placeholder();
        assert_eq!(m.grep_note, "No notes are open");
        m.query = "ab".into();
        m.grep_note_placeholder();
        assert_eq!(m.grep_note, "No notes are open");
        m.show_grep(
            &[GrepHit { path: "/n/a.md".into(), line: 4, text: "ab".into() }],
            1,
        );
        assert_eq!(m.shown.len(), 1);
        assert_eq!(m.open(0).unwrap().1, Some(4));
    }

    #[test]
    fn move_selection_wraps() {
        let mut m = NoteFindModel::new(NoteFindMode::Files, 12, 200);
        m.shown = vec![
            NoteHit { path: "/a".into(), rel: "a".into(), mtime: 0.0, tab: 0 },
            NoteHit { path: "/b".into(), rel: "b".into(), mtime: 0.0, tab: 1 },
        ];
        m.move_selection(1);
        assert_eq!(m.selection, 1);
        m.move_selection(1);
        assert_eq!(m.selection, 0);
        m.move_selection(-1);
        assert_eq!(m.selection, 1);
    }

    // ---- inline rename --------------------------------------------------

    #[test]
    fn inline_rename_selects_stem() {
        let r = InlineRenameModel::begin("note.md", false);
        assert_eq!(r.selected_len, 4);
        let d = InlineRenameModel::begin("folder", true);
        assert_eq!(d.selected_len, 6);
    }

    #[test]
    fn inline_rename_commit_and_cancel() {
        let mut r = InlineRenameModel::begin("note.md", false);
        r.set_path("/n/note.md");
        assert_eq!(r.end("renamed.md").unwrap(), ("/n/note.md".into(), "renamed.md".into()));
        assert!(!r.active);
        assert!(r.end("x").is_none());

        let mut r = InlineRenameModel::begin("note.md", false);
        r.set_path("/n/note.md");
        r.cancel();
        assert!(!r.active);
        assert!(r.path.is_none());
    }

    #[test]
    fn inline_rename_commit_requires_path() {
        let mut r = InlineRenameModel::begin("note.md", false);
        assert!(r.end("x").is_none());
    }

    // ---- file popup -----------------------------------------------------

    #[test]
    fn file_popup_image_detection() {
        assert!(FilePopup::is_image("/a/b.PNG"));
        assert!(FilePopup::is_image("x.jpeg"));
        assert!(!FilePopup::is_image("x.md"));
        assert!(!FilePopup::is_image("noext"));
    }

    #[test]
    fn preview_text_reads_and_marks_cut() {
        let dir = std::env::temp_dir().join(format!("ws-notes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let small = dir.join("small.txt");
        std::fs::write(&small, "hello").unwrap();
        assert_eq!(
            FilePopup::preview_text(small.to_str().unwrap(), 1024),
            Some("hello".to_string())
        );

        let cut = dir.join("cut.txt");
        std::fs::write(&cut, "abcdefghij").unwrap();
        let p = FilePopup::preview_text(cut.to_str().unwrap(), 5).unwrap();
        assert!(p.starts_with("abcde"));
        assert!(p.contains("preview stops at 0 KB"));

        let bin = dir.join("bin.dat");
        std::fs::write(&bin, [0u8, 1, 2, 3]).unwrap();
        assert_eq!(FilePopup::preview_text(bin.to_str().unwrap(), 1024), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_popup_routes() {
        let dir = std::env::temp_dir().join(format!("ws-notes-route-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let txt = dir.join("a.md");
        std::fs::write(&txt, "# hi").unwrap();
        assert!(matches!(
            FilePopup::route(txt.to_str().unwrap()),
            FilePopupRoute::Text(_)
        ));
        let png = dir.join("a.png");
        std::fs::write(&png, b"not really").unwrap();
        assert_eq!(FilePopup::route(png.to_str().unwrap()), FilePopupRoute::Image);
        let missing = dir.join("none.md");
        assert_eq!(FilePopup::route(missing.to_str().unwrap()), FilePopupRoute::System);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- doc templates --------------------------------------------------

    #[test]
    fn doc_template_names_defaults_and_config() {
        assert_eq!(doc_template_names(Some("a, b,, ")), vec!["a", "b"]);
        assert!(!doc_template_names(None).is_empty());
    }

    // ---- drawers --------------------------------------------------------

    #[test]
    fn drawer_state_toggles_independently() {
        let mut d = DrawerState::default();
        assert_eq!(d.inset(), 0);
        assert_eq!(d.toggle("terminal"), Some(true));
        assert!(d.terminal_shown());
        assert!(!d.browser_shown());
        assert_eq!(d.toggle("browser"), Some(true));
        assert!(d.terminal_shown());
        assert!(d.browser_shown());
        assert_eq!(d.toggle("terminal"), Some(false));
        assert!(!d.terminal_shown());
        assert!(d.browser_shown());
        assert_eq!(d.toggle("nope"), None);
        assert_eq!(d.toggle("Terminal"), None);
    }

    #[test]
    fn drawer_state_inset_is_max_of_shown() {
        let mut d = DrawerState::default();
        assert_eq!(d.inset(), 0);
        d.toggle("terminal");
        assert_eq!(d.inset(), TERMINAL_DRAWER_HEIGHT as i64);
        d.toggle("browser");
        assert_eq!(d.inset(), BROWSER_DRAWER_WIDTH as i64);
        d.toggle("terminal");
        assert_eq!(d.inset(), BROWSER_DRAWER_WIDTH as i64);
        d.toggle("browser");
        assert_eq!(d.inset(), 0);
    }

    #[test]
    fn drawer_state_json_shape() {
        let mut d = DrawerState::default();
        assert_eq!(
            d.test_state(),
            json!({"terminal": false, "browser": false, "drawerInset": 0})
        );
        d.toggle("terminal");
        assert_eq!(
            d.test_state(),
            json!({"terminal": true, "browser": false, "drawerInset": 200})
        );
    }

    #[test]
    fn directory_names_lists_sorted_files_only() {
        let dir = std::env::temp_dir().join(format!("ws-notes-dir-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("b.md"), "b").unwrap();
        std::fs::write(dir.join("a.md"), "a").unwrap();
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        assert_eq!(directory_names(&dir), vec!["a.md", "b.md"]);
        assert!(directory_names(&dir.join("missing")).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- view / registry ------------------------------------------------

    #[test]
    fn notes_view_state_shape() {
        let mut v = NotesView::default();
        v.tabs.append("/n/A.md");
        v.refresh_sidebar(1000.0);
        let s = v.test_state();
        assert_eq!(s["view"], "notes");
        assert_eq!(s["tabs"], json!(["A.md"]));
        assert_eq!(s["selected"], 0);
        assert_eq!(s["sidebar"]["label"], NOTES_LABEL);
        assert_eq!(s["sidebar"]["rows"][0]["title"], "A.md");
        assert_eq!(s["prose"]["zoom"], 1.0);
    }

    #[test]
    fn notes_snapshot_handle_contract() {
        let dir = std::env::temp_dir().join(format!("ws-notes-snap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("A.md"), "alpha").unwrap();
        let root = dir.to_string_lossy().into_owned();
        let file = format!("{root}/A.md");

        let mut v = NotesView::new(NotesConfig::default());
        v.tree = NotesTree::new(vec![root.clone()], std::collections::BTreeSet::new(), false);

        // A tree click expands the folder: the snapshot's rows change.
        let before = v.snapshot();
        v.handle(NotesEvent::TreeClick { path: root.clone(), is_dir: true });
        assert_ne!(before, v.snapshot());
        assert!(v.snapshot().tree[0].expanded);

        // A file click opens a tab and loads its text; Edit + Save writes it.
        v.handle(NotesEvent::TreeClick { path: file.clone(), is_dir: false });
        assert_eq!(v.snapshot().path.as_deref(), Some(file.as_str()));
        assert_eq!(v.snapshot().text, "alpha");
        v.handle(NotesEvent::Edit("beta".into()));
        v.handle(NotesEvent::Save);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "beta");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn register_adds_palette_and_test_do() {
        let mut r = Registry::new();
        register(&mut r);
        let ids: Vec<&str> = r.palette_commands().iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["notes", "notes-find", "notes-grep"]);
        assert_eq!(r.dispatch_test_do("notes:state"), Some(json!({"available": true})));
    }

    // ---- helper-backed (skipped without python) -------------------------

    #[test]
    fn prose_screen_html_real_helper() {
        if !helper_ready() {
            eprintln!("skipping: no python 3.11+ or pylib/helper not found");
            return;
        }
        let dir = std::env::temp_dir().join(format!("ws-notes-prose-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let note = dir.join("t.md");
        std::fs::write(&note, "# Hi\n\n- [ ] todo\n- [x] done\n").unwrap();
        let src = ProseSource::new("# Hi\n", note.to_str().unwrap());
        let html = ProseRender::page(&src, &PopupColors::default(), "", 19.0, 900.0, &NotesConfig::default());
        // The helper path may be unavailable (pandoc missing); the basic
        // fallback still yields a head + body.
        assert!(html.contains("<body"));
        assert!(html.contains("</body>"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn doc_template_menu_real_helper() {
        if !helper_ready() {
            eprintln!("skipping: no python 3.11+ or pylib/helper not found");
            return;
        }
        let menu = doc_template_menu("# T", "a, b", "");
        assert_eq!(menu, Some((None, vec!["a".to_string(), "b".to_string()])));
    }
}
