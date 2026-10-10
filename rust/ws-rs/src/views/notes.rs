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
pub const FILE_POPUP_TEXT_LIMIT: u64 = 512 * 1024;

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
            "<base href=\"file://{}/\">\n<style>\nbody {{ max-width: {}px !important; }}\n</style>",
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
            "<base href=\"file://{}/\">\n<style>\nbody {{ max-width: {}px !important; }}\n</style>",
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
}

/// `ProseView` search + zoom state, AppKit-free.
#[derive(Clone, Debug)]
pub struct ProseViewState {
    pub search_state: SearchState,
    pub zoom: f64,
    last_g: Option<f64>,
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
    use objc2::runtime::{AnyObject, NSObject};
    use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly, Message};
    use objc2_app_kit::{
        NSAutoresizingMaskOptions, NSBackingStoreType, NSButton, NSFont, NSNormalWindowLevel,
        NSScreen, NSScrollView, NSTextField, NSTextView, NSView, NSWindow, NSWindowButton,
        NSWindowCollectionBehavior, NSWindowStyleMask, NSWindowTitleVisibility,
    };
    use objc2_foundation::{
        ns_string, NSNumber, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
    };
    use objc2_web_kit::{WKWebView, WKWebViewConfiguration};
    use std::cell::RefCell;
    use std::collections::HashMap;

    pub struct NotesFlippedIvars;

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
        }

        unsafe impl NSObjectProtocol for NotesFlippedView {}
    );

    impl NotesFlippedView {
        fn new(mtm: MainThreadMarker) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(NotesFlippedIvars);
            unsafe {
                msg_send![
                    super(this),
                    initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0))
                ]
            }
        }
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
        window: Retained<NSWindow>,
        _web: Retained<WKWebView>,
    }

    thread_local! {
        static LIVE_WINDOWS: RefCell<Vec<ProseLive>> = const { RefCell::new(Vec::new()) };
        static PROSE_CHILDREN: RefCell<HashMap<String, std::process::Child>>
            = RefCell::new(HashMap::new());
    }

    fn make_web(mtm: MainThreadMarker) -> Retained<WKWebView> {
        let config = unsafe { WKWebViewConfiguration::new(mtm) };
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(320.0, 240.0));
        let web = unsafe {
            WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), frame, &config)
        };
        let no = NSNumber::numberWithBool(false);
        let _: () = unsafe { msg_send![&*web, setValue: &*no, forKey: ns_string!("drawsBackground")] };
        web.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
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

        let vf = NSScreen::mainScreen(mtm)
            .map(|s| s.visibleFrame())
            .unwrap_or(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1400.0, 900.0)));
        let w = (vf.size.width * 0.6).min(opts.width + 160.0).max(320.0);
        let h = (vf.size.height * 0.8).max(240.0);
        let frame = NSRect::new(
            NSPoint::new(
                vf.origin.x + (vf.size.width - w) / 2.0,
                vf.origin.y + (vf.size.height - h) / 2.0,
            ),
            NSSize::new(w, h),
        );
        let mask = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Resizable
            | NSWindowStyleMask::FullSizeContentView;
        let window: Retained<NSWindow> = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                frame,
                mask,
                NSBackingStoreType::Buffered,
                false,
            )
        };
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
        window.setCollectionBehavior(NSWindowCollectionBehavior::FullScreenAuxiliary);
        for b in [
            NSWindowButton::CloseButton,
            NSWindowButton::MiniaturizeButton,
            NSWindowButton::ZoomButton,
        ] {
            if let Some(btn) = window.standardWindowButton(b) {
                btn.setHidden(true);
            }
        }

        let web = make_web(mtm);
        web.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(w, h)));
        load_html(&web, &html);
        window.setContentView(Some(&web));
        window.makeKeyAndOrderFront(None);
        LIVE_WINDOWS.with(|v| {
            v.borrow_mut().push(ProseLive {
                path: opts.path.clone(),
                window,
                _web: web,
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
    }

    /// Build the sidebar + editor surface with its hidden drawers
    /// (see [`super::NotesSurface`]).
    fn build_surface(mtm: MainThreadMarker) -> Option<NotesSurface> {
        let cfg = NotesConfig::load();
        let colors = crate::ui::theme::PopupThemeDefaults::colors();
        let mut model = NotesView::new(cfg.clone());
        let raw = setting("notes", "paths").unwrap_or_default();
        let paths = existing_note_paths(&raw, &|p| Path::new(p).exists());
        for p in &paths {
            model.tabs.append(p);
        }
        model.refresh_sidebar(now_secs());

        let sidebar_w = model.sidebar.width.clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH);
        let root = NotesFlippedView::new(mtm);
        root.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(900.0, 600.0)));
        root.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );

        // --- sidebar ---------------------------------------------------------
        let sidebar = NSScrollView::new(mtm);
        sidebar.setFrame(NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(sidebar_w, 600.0),
        ));
        sidebar.setAutoresizingMask(NSAutoresizingMaskOptions::ViewHeightSizable);
        sidebar.setHasVerticalScroller(true);
        sidebar.setDrawsBackground(true);
        sidebar.setBackgroundColor(&colors.mantle().to_nscolor());
        let rows_view = NotesFlippedView::new(mtm);
        rows_view.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        let row_h = 44.0;
        let mut y = 6.0;
        for (tab, row) in model.tabs.tabs.iter().zip(model.sidebar.rows.iter()) {
            let title = NSTextField::labelWithString(&NSString::from_str(&row.title), mtm);
            title.setFont(Some(&NSFont::systemFontOfSize(12.5)));
            title.setTextColor(Some(&colors.text.to_nscolor()));
            title.setFrame(NSRect::new(
                NSPoint::new(10.0, y),
                NSSize::new((sidebar_w - 16.0).max(0.0), 16.0),
            ));
            title.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
            rows_view.addSubview(&title);

            let folder = folder_text(&tab.path);
            let sub = match &row.trailing {
                Some(age) => format!("{folder}  \u{00b7}  {age}"),
                None => folder,
            };
            let sub_label = NSTextField::labelWithString(&NSString::from_str(&sub), mtm);
            sub_label.setFont(Some(&NSFont::systemFontOfSize(10.5)));
            sub_label.setTextColor(Some(&colors.dim.to_nscolor()));
            sub_label.setFrame(NSRect::new(
                NSPoint::new(10.0, y + 18.0),
                NSSize::new((sidebar_w - 16.0).max(0.0), 14.0),
            ));
            sub_label.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
            rows_view.addSubview(&sub_label);
            y += row_h;
        }
        rows_view.setFrame(NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(sidebar_w, y.max(600.0)),
        ));
        sidebar.setDocumentView(Some(&rows_view));
        root.addSubview(&sidebar);

        // --- editor ----------------------------------------------------------
        let editor_w = (900.0 - sidebar_w - 1.0).max(0.0);
        let editor = NSScrollView::new(mtm);
        editor.setFrame(NSRect::new(
            NSPoint::new(sidebar_w + 1.0, 0.0),
            NSSize::new(editor_w, 600.0),
        ));
        editor.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        editor.setHasVerticalScroller(true);
        editor.setDrawsBackground(true);
        editor.setBackgroundColor(&colors.base().to_nscolor());
        let text = NSTextView::new(mtm);
        text.setEditable(true);
        text.setSelectable(true);
        text.setRichText(false);
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
        if let Some(container) = unsafe { text.textContainer() } {
            container.setWidthTracksTextView(true);
        }
        let body = model
            .tabs
            .selected_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default();
        text.setString(&NSString::from_str(&body));
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
            NSPoint::new(sidebar_w + 1.0, 0.0),
            NSSize::new(editor_w, 600.0),
        );
        let mut vim = None;
        if vim_enabled {
            let file = model.tabs.selected_path().map(str::to_string);
            let mut pane =
                crate::views::notes_vim::VimPane::new(vim_bin.clone(), vim_asset.clone(), file);
            if let Some(view) = pane.build(mtm, vim_frame) {
                view.setAutoresizingMask(
                    NSAutoresizingMaskOptions::ViewWidthSizable
                        | NSAutoresizingMaskOptions::ViewHeightSizable,
                );
                text.setHidden(true);
                root.addSubview(&view);
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
        let pop = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str("Prose"),
                Some(as_any(&*handler)),
                Some(objc2::sel!(popOut:)),
                mtm,
            )
        };
        pop.setBordered(false);
        pop.setEnabled(has_target);
        pop.setFont(Some(&NSFont::systemFontOfSize(12.0)));
        pop.setContentTintColor(Some(&colors.text.to_nscolor()));
        pop.sizeToFit();
        let pop_w = pop.frame().size.width.max(52.0);
        pop.setFrame(NSRect::new(
            NSPoint::new((900.0 - pop_w - 12.0).max(sidebar_w + 8.0), 8.0),
            NSSize::new(pop_w, 22.0),
        ));
        pop.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin | NSAutoresizingMaskOptions::ViewMaxYMargin,
        );
        root.addSubview(&pop);

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
            swiftterm_shim::set_font(&shim, "Menlo", 12.0);
            let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
            let args = vec!["-l".to_string()];
            let home = std::env::var("HOME").ok();
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
            Some(shown)
        }

        /// The exact `{"terminal", "browser", "drawerInset"}` state shape.
        pub fn drawer_state(&self) -> Value {
            self.drawers.test_state()
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

        /// Whether the embedded nvim pane is active for this surface.
        pub fn vim_active(&self) -> bool {
            self.vim.is_some()
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
            if self.vim.is_some() {
                self.vim_open(path);
                return;
            }
            if let Ok(text) = std::fs::read_to_string(path) {
                self.editor.setString(&NSString::from_str(&text));
            }
            self.focus_editor();
        }

        /// `vimFlush()` — `silent! wall` before the window hides.
        pub fn vim_flush(&self) {
            if let Some(pane) = &self.vim {
                pane.flush();
            }
        }

        /// `onVimExit` — rebuild the pane on the current note after `:q` / a
        /// crash; returns true when a relaunch happened.
        pub fn vim_poll_exit(&mut self, mtm: MainThreadMarker) -> bool {
            let (exited, file) = match self.vim.as_mut() {
                Some(pane) => (pane.poll_exit(), pane.file().map(str::to_string)),
                None => return false,
            };
            if !exited {
                return false;
            }
            if let Some(old) = self.vim.as_ref().and_then(|p| p.view()) {
                old.removeFromSuperview();
            }
            let mut pane = crate::views::notes_vim::VimPane::new(
                self.vim_bin.clone(),
                self.vim_asset.clone(),
                file.clone(),
            );
            match pane.build(mtm, self.vim_frame) {
                Some(view) => {
                    view.setAutoresizingMask(
                        NSAutoresizingMaskOptions::ViewWidthSizable
                            | NSAutoresizingMaskOptions::ViewHeightSizable,
                    );
                    self.root.addSubview(&view);
                    self.vim = Some(pane);
                    if let Some(f) = file {
                        if let Some(p) = self.vim.as_mut() {
                            p.open(&f);
                        }
                    }
                    true
                }
                None => {
                    self.vim = None;
                    false
                }
            }
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
                format!(
                    "Type 2+ characters to search {} open notes",
                    paths.len()
                )
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
        self.selection = 0;
        if hits.is_empty() && open_count > 0 {
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
// File popup model
// ---------------------------------------------------------------------------

pub const FILE_POPUP_IMAGE_EXTS: [&str; 9] = [
    "png", "jpg", "jpeg", "gif", "heic", "webp", "tif", "tiff", "bmp",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FilePopupRoute {
    Image,
    Text(String),
    System,
}

pub struct FilePopup;

impl FilePopup {
    pub fn is_image(path: &str) -> bool {
        let ext = Path::new(path)
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        FILE_POPUP_IMAGE_EXTS.contains(&ext.as_str())
    }

    /// `FilePopup.previewText` — up to `limit` bytes, NUL = binary, UTF-8 with
    /// incremental truncation for a cut multibyte tail.
    pub fn preview_text(path: &str, limit: u64) -> Option<String> {
        use std::io::Read;
        let meta = std::fs::metadata(path).ok()?;
        if meta.is_dir() {
            return None;
        }
        let f = std::fs::File::open(path).ok()?;
        let mut data = Vec::new();
        f.take(limit + 4).read_to_end(&mut data).ok()?;
        if data.contains(&0) {
            return None;
        }
        let cut = data.len() as u64 > limit;
        let mut body = if cut {
            data[..limit as usize].to_vec()
        } else {
            data
        };
        let mut text = String::from_utf8(body.clone()).ok();
        let mut trimmed = 0;
        while text.is_none() && trimmed < 3 && !body.is_empty() {
            body.pop();
            trimmed += 1;
            text = String::from_utf8(body.clone()).ok();
        }
        let text = text?;
        if cut {
            Some(format!(
                "{text}\n\n… (preview stops at {} KB)",
                limit / 1024
            ))
        } else {
            Some(text)
        }
    }

    /// `FilePopup.open` routing decision.
    pub fn route(path: &str) -> FilePopupRoute {
        if Self::is_image(path) && std::fs::metadata(path).map(|m| m.is_file()).unwrap_or(false) {
            FilePopupRoute::Image
        } else if !Self::is_image(path) {
            if let Some(text) = Self::preview_text(path, FILE_POPUP_TEXT_LIMIT) {
                return FilePopupRoute::Text(text);
            }
            FilePopupRoute::System
        } else {
            FilePopupRoute::System
        }
    }
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

pub struct NotesView {
    pub config: NotesConfig,
    pub tabs: NotesTabs,
    pub sidebar: NotesSidebar,
    pub prose: ProseViewState,
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
