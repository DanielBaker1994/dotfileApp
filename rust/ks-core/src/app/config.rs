//! Config layer ported from `kitchen_sink.swift` (the `[app]` settings, the
//! per-`[section]` `CommandSpec`, and the commands.toml validation / write-back
//! path). Colors are plain `f64` RGBA so this module stays AppKit-free.
//!
//! `icon` records the raw `[section] icon` value: Swift's `makeCommand` runs it
//! through `resolveIconName`, which needs AppKit; resolution is a UI concern.

use crate::app::python_helper::PythonHelper;
use crate::engines::config_text::{self, ConfigDecodedLine};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

// ---------------------------------------------------------------------------
// Colors (`hexColor` / `parsePalette` from `kitchen_sink.swift`)
// ---------------------------------------------------------------------------

pub use crate::ui::theme::Rgba;

/// `hexColor(_:)` — see [`crate::ui::theme::HexColor::parse`].
pub fn hex_color(s: &str) -> Option<Rgba> {
    crate::ui::theme::HexColor::parse(s)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub accent2: Rgba,
    pub success: Rgba,
    pub warning: Rgba,
    pub danger: Rgba,
    pub info: Rgba,
}

impl Palette {
    pub fn from_slice(colors: &[Rgba]) -> Option<Palette> {
        if colors.len() != 5 {
            return None;
        }
        Some(Palette {
            accent2: colors[0],
            success: colors[1],
            warning: colors[2],
            danger: colors[3],
            info: colors[4],
        })
    }
}

/// `parsePalette(_:)` — exactly 5 comma-separated hex colors.
pub fn parse_palette(v: Option<&str>) -> Option<Palette> {
    let v = v.filter(|s| !s.is_empty())?;
    let colors: Vec<Rgba> = v
        .split(',')
        .filter_map(|p| hex_color(p.trim()))
        .collect();
    Palette::from_slice(&colors)
}

// ---------------------------------------------------------------------------
// HeaderStyle (raw values shared with the UI enum)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeaderStyle {
    Quiet,
    Flat,
    Edge,
    Stripe,
    Tinted,
    Glow,
    Aurora,
}

impl HeaderStyle {
    pub const ALL: [HeaderStyle; 7] = [
        HeaderStyle::Quiet,
        HeaderStyle::Flat,
        HeaderStyle::Edge,
        HeaderStyle::Stripe,
        HeaderStyle::Tinted,
        HeaderStyle::Glow,
        HeaderStyle::Aurora,
    ];

    pub fn raw(self) -> &'static str {
        match self {
            HeaderStyle::Quiet => "quiet",
            HeaderStyle::Flat => "flat",
            HeaderStyle::Edge => "edge",
            HeaderStyle::Stripe => "stripe",
            HeaderStyle::Tinted => "tinted",
            HeaderStyle::Glow => "glow",
            HeaderStyle::Aurora => "aurora",
        }
    }

    pub fn from_raw(s: &str) -> Option<HeaderStyle> {
        Some(match s.to_lowercase().as_str() {
            "quiet" => HeaderStyle::Quiet,
            "flat" => HeaderStyle::Flat,
            "edge" => HeaderStyle::Edge,
            "stripe" => HeaderStyle::Stripe,
            "tinted" => HeaderStyle::Tinted,
            "glow" => HeaderStyle::Glow,
            "aurora" => HeaderStyle::Aurora,
            _ => return None,
        })
    }
}

impl Default for HeaderStyle {
    fn default() -> Self {
        HeaderStyle::Flat
    }
}

// ---------------------------------------------------------------------------
// small value helpers
// ---------------------------------------------------------------------------

/// `csv(_:)` — comma split, trimmed, empties dropped.
pub fn csv(s: Option<&str>) -> Vec<String> {
    s.unwrap_or("")
        .split(',')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

/// `(s as NSString).expandingTildeInPath` (the `~` / `~/…` common cases).
pub use crate::app::pathfmt::expand_tilde;

/// `num(_:)` — `CGFloat(Double(s ?? "") ?? 0)`.
pub fn num(s: Option<&str>) -> f64 {
    s.and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0)
}

fn int_of(s: Option<&str>) -> Option<i64> {
    s.and_then(|v| v.parse::<i64>().ok())
}

fn tri_of(s: Option<&str>) -> Option<bool> {
    config_text::tri(s)
}

fn home_dir() -> String {
    std::env::var("HOME").unwrap_or_default()
}

/// The shared window's (minimum, default) content sizes for a screen whose
/// visible area is `screen` (w, h). The minimum never exceeds the screen; the
/// default is `shared-width`/`shared-height` when set, else
/// `shared_screen_fraction` of the screen, never below the minimum and never
/// past the screen.
pub fn shared_window_sizes(s: &AppSettings, screen: (f64, f64)) -> ((f64, f64), (f64, f64)) {
    let (sw, sh) = screen;
    let min = (s.shared_min_width.min(sw), s.shared_min_height.min(sh));
    let pick = |set: f64, side: f64, lo: f64| {
        let want = if set > 0.0 { set } else { (side * s.shared_screen_fraction).round() };
        want.max(lo).min(side)
    };
    (min, (pick(s.shared_width, sw, min.0), pick(s.shared_height, sh, min.1)))
}

// ---------------------------------------------------------------------------
// AppSettings
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct AppSettings {
    pub shared_window: bool,
    /// `[app] shared-width` / `shared-height`: the shared window's default
    /// content size in points. 0 (unset) = `shared_screen_fraction` of the
    /// screen's visible area.
    pub shared_width: f64,
    pub shared_height: f64,
    /// `[app] shared-min-width` / `shared-min-height`: the smallest the
    /// shared window may be (drag, resize keys, a re-show); the views lay out
    /// for this much room and are clipped below it.
    pub shared_min_width: f64,
    pub shared_min_height: f64,
    /// `[app] shared-screen-fraction`: the default size as a share of the
    /// screen's visible area when no `shared-width`/`shared-height` is set.
    pub shared_screen_fraction: f64,
    pub palette_first: Vec<String>,
    /// `[app] palette`: the Hyper+S palette lists exactly these command ids,
    /// in this order. Empty = every `in-palette` command.
    pub palette: Vec<String>,
    pub shell: String,
    pub terminal_font: String,
    pub jira_icon_name: String,
    pub confluence_icon_name: String,
    pub ai_icon_name: String,
    pub app_icon_name: String,
    pub files_icon_name: String,
    pub notes_socket_name: String,
    pub focus_file_name: String,
    pub focus_bridge_name: String,
    pub switcher_window_name: String,
    pub hide_on_focus_loss: bool,
    pub focus_loss_delay: f64,
    pub esc_close: i64,
    pub copy_toast: String,
    pub header_style: HeaderStyle,
    pub preview_border: Rgba,
    pub preview_border_width: f64,
    pub pane_focus_color: Rgba,
    /// No `pane-focus-color` set: the ring takes the current view's accent.
    pub pane_focus_themed: bool,
    pub pane_focus_width: f64,
    pub vim_keys: bool,
    pub vim_mode_badge: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        AppSettings {
            shared_window: true,
            shared_width: 0.0,
            shared_height: 0.0,
            shared_min_width: 900.0,
            shared_min_height: 560.0,
            shared_screen_fraction: 0.85,
            palette_first: vec![
                "paths".to_string(),
                "filefast".to_string(),
                "text-table".to_string(),
                "prettyprint".to_string(),
            ],
            palette: Vec::new(),
            shell: "/opt/homebrew/bin/bash".to_string(),
            terminal_font: "Hack Nerd Font".to_string(),
            jira_icon_name: "jira_icon.png".to_string(),
            confluence_icon_name: "confluence_icon.png".to_string(),
            ai_icon_name: String::new(),
            app_icon_name: "app_icon.png".to_string(),
            files_icon_name: String::new(),
            notes_socket_name: "ws-notes.sock".to_string(),
            focus_file_name: "kitchen-sink-focus".to_string(),
            focus_bridge_name: "ws-aerospace-focus".to_string(),
            switcher_window_name: "kitchen-sink".to_string(),
            hide_on_focus_loss: true,
            focus_loss_delay: 0.3,
            esc_close: 0,
            copy_toast: "Copied {} to clipboard".to_string(),
            header_style: HeaderStyle::Flat,
            preview_border: Rgba::new(0.753, 0.753, 0.753, 1.0),
            preview_border_width: 2.0,
            pane_focus_color: Rgba::new(0xC8 as f64 / 255.0, 0xCE as f64 / 255.0, 0xD8 as f64 / 255.0, 0.55),
            pane_focus_themed: true,
            pane_focus_width: 1.0,
            vim_keys: true,
            vim_mode_badge: true,
        }
    }
}

impl AppSettings {
    /// `settings.commandsConfPath` (app mode: the home; tests override via
    /// `WS_COMMANDS_CONF`).
    pub fn commands_conf_path(&self) -> PathBuf {
        commands_conf_path()
    }
}

fn get(vars: &HashMap<String, String>, k: &str) -> Option<String> {
    vars.get(k).map(|v| v.trim().to_string())
}

/// `parseAppConfig(_:)`.
pub fn parse_app_config(settings: &mut AppSettings, vars: &HashMap<String, String>) {
    let strv = |k: &str| get(vars, k);

    if let Some(v) = strv("shell") {
        if !v.is_empty() {
            settings.shell = v;
        }
    }
    if let Some(v) = strv("terminal-font") {
        if !v.is_empty() {
            settings.terminal_font = v;
        }
    }
    if let Some(v) = strv("jira-icon") {
        if !v.is_empty() {
            settings.jira_icon_name = v;
        }
    }
    if let Some(v) = strv("confluence-icon") {
        if !v.is_empty() {
            settings.confluence_icon_name = v;
        }
    }
    if let Some(v) = strv("ai-icon") {
        settings.ai_icon_name = v;
    }
    if let Some(v) = strv("app-icon") {
        if !v.is_empty() {
            settings.app_icon_name = v;
        }
    }
    if let Some(v) = strv("files-icon") {
        settings.files_icon_name = v;
    }
    if let Some(v) = strv("notes-socket") {
        if !v.is_empty() {
            settings.notes_socket_name = v;
        }
    }
    if let Some(v) = strv("focus-file") {
        if !v.is_empty() {
            settings.focus_file_name = v;
        }
    }
    if let Some(v) = strv("focus-bridge") {
        if !v.is_empty() {
            settings.focus_bridge_name = v;
        }
    }
    if let Some(v) = strv("switcher-name") {
        if !v.is_empty() {
            settings.switcher_window_name = v;
        }
    }
    if let Some(v) = strv("hide-on-focus-loss") {
        settings.hide_on_focus_loss = config_text::tri(Some(&v)) == Some(true);
    }
    if let Some(v) = strv("focus-loss-delay").and_then(|v| v.parse::<f64>().ok()) {
        if v >= 0.0 {
            settings.focus_loss_delay = v.min(5.0);
        }
    }
    settings.header_style = strv("header-style")
        .and_then(|v| HeaderStyle::from_raw(&v))
        .unwrap_or(HeaderStyle::Flat);
    if let Some(v) = tri_of(strv("shared-window").as_deref()) {
        settings.shared_window = v;
    }
    if let Some(v) = strv("shared-width").and_then(|v| v.parse::<f64>().ok()) {
        if v >= 400.0 {
            settings.shared_width = v;
        }
    }
    if let Some(v) = strv("shared-height").and_then(|v| v.parse::<f64>().ok()) {
        if v >= 300.0 {
            settings.shared_height = v;
        }
    }
    if let Some(v) = strv("shared-min-width").and_then(|v| v.parse::<f64>().ok()) {
        settings.shared_min_width = v.clamp(400.0, 4000.0);
    }
    if let Some(v) = strv("shared-min-height").and_then(|v| v.parse::<f64>().ok()) {
        settings.shared_min_height = v.clamp(300.0, 4000.0);
    }
    if let Some(v) = strv("shared-screen-fraction").and_then(|v| v.parse::<f64>().ok()) {
        settings.shared_screen_fraction = v.clamp(0.3, 1.0);
    }
    // A repeated id would list its command twice: keep the first.
    let id_list = |v: &str| -> Vec<String> {
        let mut ids: Vec<String> = Vec::new();
        for p in v.split(',').map(|p| p.trim().to_lowercase()).filter(|p| !p.is_empty()) {
            if !ids.contains(&p) {
                ids.push(p);
            }
        }
        ids
    };
    if let Some(v) = strv("palette-first") {
        settings.palette_first = id_list(&v);
    }
    if let Some(v) = strv("palette") {
        settings.palette = id_list(&v);
    }
    if let Some(v) = strv("esc-close").and_then(|v| v.parse::<i64>().ok()) {
        settings.esc_close = v.max(0);
    }
    if vars.contains_key("copy-toast") {
        settings.copy_toast = strv("copy-toast").unwrap_or_default();
    }
    settings.preview_border = strv("preview-border")
        .and_then(|v| hex_color(&v))
        .unwrap_or_else(|| Rgba::new(0.753, 0.753, 0.753, 1.0));
    settings.preview_border_width = strv("preview-border-width")
        .and_then(|v| v.parse::<f64>().ok())
        .map(|n| n.max(0.0).min(8.0))
        .unwrap_or(2.0);
    let focus_color = strv("pane-focus-color").and_then(|v| hex_color(&v));
    settings.pane_focus_themed = focus_color.is_none();
    settings.pane_focus_color = focus_color
        .unwrap_or_else(|| {
            Rgba::new(0xC8 as f64 / 255.0, 0xCE as f64 / 255.0, 0xD8 as f64 / 255.0, 0.55)
        });
    settings.pane_focus_width = strv("pane-focus-width")
        .and_then(|v| v.parse::<f64>().ok())
        .map(|n| n.max(0.0).min(4.0))
        .unwrap_or(1.0);
    settings.vim_keys = tri_of(strv("vim-keys").as_deref()).unwrap_or(true);
    settings.vim_mode_badge = tri_of(strv("vim-mode-badge").as_deref()).unwrap_or(true);
}

/// `configSectionEntries(configLines(content), "app")` → a vars map.
pub fn app_vars_from_text(text: &str) -> HashMap<String, String> {
    let mut vars = HashMap::new();
    for (_, key, value) in
        config_text::config_section_entries(&config_text::config_lines(text), "app")
    {
        if !key.is_empty() {
            vars.insert(key, value);
        }
    }
    vars
}

pub fn apply_app_config_from_settings_text(settings: &mut AppSettings, text: &str) {
    parse_app_config(settings, &app_vars_from_text(text));
}

/// `applyAppConfigFromDisk()`.
pub fn apply_app_config_from_disk(settings: &mut AppSettings) {
    if let Some(text) = read_config_text() {
        apply_app_config_from_settings_text(settings, &text);
    }
}

// ---------------------------------------------------------------------------
// CommandSpec
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandKind {
    Shell,
    List,
    Output,
    Files,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ListColumn {
    pub field: String,
    pub title: String,
    pub width: f64,
    /// A fixed pixel width (the column does not scale with the window).
    pub width_px: Option<f64>,
    pub align: String,
    pub sortable: bool,
    pub filterable: bool,
}

static COLUMN_MEMO: OnceLock<Mutex<HashMap<String, Vec<ListColumn>>>> = OnceLock::new();

impl ListColumn {
    /// `ListColumn.parse(_:)` — a `jira.columns_parse` helper round trip, memoised
    /// per spec (a failed call is not cached, matching `tri`'s retry).
    pub fn parse(spec: Option<&str>) -> Vec<ListColumn> {
        let key = spec.unwrap_or("").to_string();
        let memo = COLUMN_MEMO.get_or_init(|| Mutex::new(HashMap::new()));
        if let Some(hit) = memo.lock().unwrap().get(&key) {
            return hit.clone();
        }
        let mut cols = Vec::new();
        if let Ok(boxed) = PythonHelper::shared()
            .call_default("jira.columns_parse", json!({ "spec": key }))
        {
            if let Some(list) = boxed.get("columns").and_then(Value::as_array) {
                for o in list {
                    cols.push(ListColumn {
                        field: o.get("field").and_then(Value::as_str).unwrap_or("").to_string(),
                        title: o.get("title").and_then(Value::as_str).unwrap_or("").to_string(),
                        width: o.get("width").and_then(Value::as_f64).unwrap_or(0.0),
                        width_px: o.get("width_px").and_then(Value::as_f64),
                        align: o
                            .get("align")
                            .and_then(Value::as_str)
                            .unwrap_or("left")
                            .to_string(),
                        sortable: o.get("sortable").and_then(Value::as_bool).unwrap_or(false),
                        filterable: o.get("filterable").and_then(Value::as_bool).unwrap_or(false),
                    });
                }
            }
            memo.lock().unwrap().insert(key, cols.clone());
        }
        cols
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CommandSpec {
    pub name: String,
    pub kind: CommandKind,
    pub script: Option<String>,
    pub paths: Vec<String>,
    pub sources: Vec<String>,
    pub root: Option<String>,
    pub favorites: Vec<String>,
    pub recent: bool,
    pub start_recent: bool,
    pub recent_everywhere: bool,
    pub tint_alpha: Option<f64>,
    pub primary: Option<String>,
    pub content: Option<String>,
    pub detail: Option<String>,
    pub trailing: Option<String>,
    pub body: Option<String>,
    pub filter: Vec<String>,
    pub filters: Vec<String>,
    pub width: f64,
    pub max_rows: i64,
    pub page_size: i64,
    pub checkbox: Option<bool>,
    pub resize: bool,
    pub drag: bool,
    pub sticky: bool,
    pub panel: bool,
    pub label: Option<String>,
    pub aliases: Vec<String>,
    pub in_palette: bool,
    pub sort: Option<String>,
    pub sort_descending: Option<bool>,
    pub terminal_words: Option<Vec<String>>,
    pub height: f64,
    pub font: Option<String>,
    pub font_size: f64,
    pub header_color: Option<Rgba>,
    pub terminal: bool,
    pub inspector_width: f64,
    pub sidebar_width: f64,
    pub text_color: Option<Rgba>,
    pub palette: Option<Palette>,
    pub esc_close: Option<i64>,
    pub icon: Option<String>,
    pub save_dir: String,
    pub table: bool,
    pub columns: Vec<ListColumn>,
}

impl Default for CommandSpec {
    fn default() -> Self {
        CommandSpec {
            name: String::new(),
            kind: CommandKind::Shell,
            script: None,
            paths: Vec::new(),
            sources: Vec::new(),
            root: None,
            favorites: Vec::new(),
            recent: true,
            start_recent: true,
            recent_everywhere: true,
            tint_alpha: None,
            primary: None,
            content: None,
            detail: None,
            trailing: None,
            body: None,
            filter: Vec::new(),
            filters: Vec::new(),
            width: 0.0,
            max_rows: 0,
            page_size: 0,
            checkbox: None,
            resize: false,
            drag: true,
            sticky: true,
            panel: false,
            label: None,
            aliases: Vec::new(),
            in_palette: true,
            sort: None,
            sort_descending: None,
            terminal_words: None,
            height: 0.0,
            font: None,
            font_size: 0.0,
            header_color: None,
            terminal: false,
            inspector_width: 340.0,
            sidebar_width: 210.0,
            text_color: None,
            palette: None,
            esc_close: None,
            icon: None,
            save_dir: "/tmp/".to_string(),
            table: false,
            columns: Vec::new(),
        }
    }
}

impl CommandSpec {
    pub fn new(name: impl Into<String>, kind: CommandKind, script: Option<String>) -> Self {
        let name = name.into();
        CommandSpec {
            name,
            kind,
            script,
            ..Default::default()
        }
    }

}

/// `makeCommand(_:_:)`.
pub fn make_command(name: &str, vars: &HashMap<String, String>) -> CommandSpec {
    let kind = match vars.get("type").map(String::as_str) {
        Some("list") => CommandKind::List,
        Some("output") => CommandKind::Output,
        Some("files") => CommandKind::Files,
        _ => CommandKind::Shell,
    };
    let mut s = CommandSpec::new(name, kind, vars.get("script").cloned());
    if let Some(l) = vars.get("label").map(|v| v.trim().to_string()) {
        if !l.is_empty() {
            s.label = Some(l);
        }
    }
    s.in_palette = tri_of(vars.get("in-palette").map(String::as_str)).unwrap_or(true);
    s.aliases = csv(vars.get("aliases").map(String::as_str));
    s.paths = csv(vars.get("paths").or_else(|| vars.get("path")).map(String::as_str));
    s.root = vars.get("root").cloned();
    s.favorites = csv(vars.get("favorites").map(String::as_str));
    s.panel = tri_of(vars.get("panel").map(String::as_str)).unwrap_or(false);
    s.icon = vars.get("icon").cloned();
    if let Some(v) = vars.get("save-dir") {
        if !v.is_empty() {
            s.save_dir = v.clone();
        }
    }
    s.sources = csv(vars.get("sources").or_else(|| vars.get("source")).map(String::as_str));
    s.primary = vars.get("primary").cloned();
    s.content = vars.get("content").cloned();
    s.detail = vars.get("detail").cloned();
    s.trailing = vars.get("trailing").cloned();
    s.body = vars.get("body").cloned();
    s.filter = csv(vars.get("filter").map(String::as_str));
    s.filters = csv(vars.get("filters").map(String::as_str));
    s.max_rows = int_of(vars.get("max-rows").map(String::as_str)).unwrap_or(0);
    s.page_size = int_of(vars.get("page-size").map(String::as_str)).unwrap_or(0);
    s.checkbox = tri_of(vars.get("checkbox").map(String::as_str));
    s.table = tri_of(vars.get("table").map(String::as_str)).unwrap_or(false);
    s.columns = ListColumn::parse(vars.get("columns").map(String::as_str));
    s.width = num(vars.get("width").map(String::as_str));
    s.height = num(vars.get("height").map(String::as_str));
    s.resize = tri_of(vars.get("resize").map(String::as_str)).unwrap_or(false);
    s.drag = tri_of(vars.get("drag").map(String::as_str)).unwrap_or(true);
    s.sticky = tri_of(vars.get("sticky").map(String::as_str)).unwrap_or(true);
    s.esc_close = vars
        .get("esc-close")
        .or_else(|| vars.get("vim-esc-close"))
        .and_then(|v| v.parse::<i64>().ok());
    s.font = vars.get("font").cloned();
    s.font_size = num(vars.get("font-size").map(String::as_str));
    s.header_color = vars.get("header-color").and_then(|v| hex_color(v));
    {
        let t = num(vars.get("tint-alpha").map(String::as_str));
        if t > 0.0 {
            s.tint_alpha = Some(t.min(1.0));
        }
    }
    s.text_color = vars.get("text-color").and_then(|v| hex_color(v));
    s.palette = parse_palette(vars.get("palette").map(String::as_str));
    s.terminal = tri_of(vars.get("terminal").map(String::as_str)).unwrap_or(false);
    if vars.contains_key("sidebar-width") {
        s.sidebar_width = num(vars.get("sidebar-width").map(String::as_str));
    }
    if vars.contains_key("inspector-width") {
        s.inspector_width = num(vars.get("inspector-width").map(String::as_str));
    }
    s.sort = vars.get("sort").cloned();
    s.sort_descending = vars
        .get("sort-order")
        .map(|o| o.to_lowercase().starts_with("desc"));
    if vars.contains_key("terminal-words") {
        s.terminal_words = Some(csv(vars.get("terminal-words").map(String::as_str)));
    }
    s.recent = tri_of(vars.get("recent").map(String::as_str)).unwrap_or(true);
    s.start_recent = vars
        .get("start")
        .map(|v| v.to_lowercase())
        .unwrap_or_else(|| "recent".to_string())
        != "root";
    s.recent_everywhere = vars
        .get("recent-scope")
        .map(|v| v.to_lowercase())
        .unwrap_or_else(|| "everywhere".to_string())
        != "home";
    s
}

// ---------------------------------------------------------------------------
// validation
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigIssue {
    pub line: i64,
    pub message: String,
    pub fatal: bool,
}

pub const CONFIG_BOOL_KEYS: &[&str] = &[
    "enabled", "resize", "drag", "sticky", "voice", "terminal", "vim-mode",
    "recent", "checkbox", "hide-on-focus-loss", "float", "table", "shared-window",
    "in-palette", "panel", "vim-keys", "vim-mode-badge", "show-help", "show-side-panel-button",
    "show-size-badge", "magnifier", "square-magnifier", "copy-on-double-click", "save-path-fixed",
    "save-after-copy", "copy-path-after-save", "save-last-region", "reverse-arrow",
    "counter-outline", "insecure-pixelate", "syntax-highlight",
];

pub const CONFIG_COLOR_KEYS: &[&str] = &[
    "header-color",
    "background-color",
    "browser-background",
    "terminal-background",
    "text-color",
    "dim-color",
    "highlight-color",
    "accent-color",
    "terminal-foreground",
];

/// `configNumberKeys` — the numeric range validation table (new numeric keys
/// must be added here).
pub const CONFIG_NUMBER_KEYS: &[(&str, f64, f64)] = &[
    ("limit", 1.0, 25.0),
    ("side-panel-width", 180.0, 480.0),
    ("merge-gap", 0.0, 200.0),
    ("merge-gutter", 0.0, 200.0),
    ("merge-label-size", 8.0, 96.0),
    ("merge-max-scale", 1.0, 6.0),
    ("merge-border-width", 0.0, 20.0),
    ("draw-thickness", 0.0, 100.0),
    ("draw-font-size", 0.0, 100.0),
    ("width", 100.0, 8000.0),
    ("height", 60.0, 8000.0),
    ("shared-width", 400.0, 8000.0),
    ("shared-height", 300.0, 8000.0),
    ("shared-min-width", 400.0, 4000.0),
    ("shared-min-height", 300.0, 4000.0),
    ("shared-screen-fraction", 0.3, 1.0),
    ("margin-top", 0.0, 400.0),
    ("margin-top-builtin", 0.0, 400.0),
    ("margin-bottom", 0.0, 400.0),
    ("switcher-width", 240.0, 1200.0),
    ("preview-border-width", 0.0, 8.0),
    ("pane-focus-width", 0.0, 4.0),
    ("sidebar-width", 0.0, 600.0),
    ("sidebar-min-width", 0.0, 600.0),
    ("sidebar-max-width", 0.0, 600.0),
    ("inspector-width", 0.0, 800.0),
    ("inspector-min-width", 0.0, 800.0),
    ("inspector-max-width", 0.0, 800.0),
    ("font-size", 6.0, 96.0),
    ("terminal-font-size", 6.0, 96.0),
    ("max-rows", 0.0, 10_000.0),
    ("page-size", 0.0, 100_000.0),
    ("body-lines", 0.0, 100.0),
    ("search-width", 0.0, 1.0),
    ("recent-days", 1.0, 365.0),
    ("recent-limit", 20.0, 5000.0),
    ("tint-alpha", 0.0, 1.0),
    ("max-row-stretch", 0.0, 1000.0),
    ("row-height", 18.0, 80.0),
    ("vim-esc-close", 0.0, 20.0),
    ("esc-close", 0.0, 20.0),
    ("dashboard-refresh", 2.0, 3600.0),
    ("split", 0.2, 0.8),
    ("context-tokens", 512.0, 1_000_000.0),
    ("focus-loss-delay", 0.0, 5.0),
    ("contrast-opacity", 0.0, 255.0),
    ("jpeg-quality", 1.0, 100.0),
    ("undo-limit", 1.0, 1000.0),
    ("button-size", 0.0, 80.0),
    ("arrow-style", 0.0, 1.0),
    ("delay", 0.0, 60_000.0),
    ("context-lines", 0.0, 1000.0),
    ("tab-width", 1.0, 16.0),
    ("time-tolerance", 0.0, 86_400.0),
    ("max-lines", 1000.0, 5_000_000.0),
    ("max-bytes", 1_048_576.0, 2_147_483_648.0),
];

pub fn config_number_keys() -> &'static [(&'static str, f64, f64)] {
    CONFIG_NUMBER_KEYS
}

pub fn config_number_range(key: &str) -> Option<(f64, f64)> {
    CONFIG_NUMBER_KEYS
        .iter()
        .find(|(k, _, _)| *k == key)
        .map(|(_, lo, hi)| (*lo, *hi))
}

pub fn config_enum_keys(key: &str) -> Option<Vec<&'static str>> {
    Some(match key {
        "type" => vec!["shell", "list", "output", "files"],
        "start-drawer" => vec!["browser", "terminal", "none"],
        "sort" => vec!["name", "modified", "created", "size", "kind"],
        "sort-order" => vec!["asc", "desc", "ascending", "descending"],
        "header-style" => HeaderStyle::ALL.iter().map(|h| h.raw()).collect(),
        _ => return None,
    })
}

const SHOT_TOOLS: &[&str] = &[
    "pencil", "line", "arrow", "selection", "rectangle", "circle", "marker", "text", "counter",
    "pixelate", "invert", "size", "move", "undo", "redo", "copy", "save", "accept", "exit",
    "pin", "recent", "copy-text", "size-increase", "size-decrease",
];

fn rgb_hex_ok(s: &str) -> bool {
    let t = s.trim();
    let t = t.strip_prefix('#').unwrap_or(t);
    t.len() == 6 && t.bytes().all(|b| b.is_ascii_hexdigit())
}

fn theme_preset_parse_ok(name: &str, value: &str) -> bool {
    let parts: Vec<&str> = value.split(',').map(str::trim).collect();
    let n = parts.len();
    if name.is_empty() || (n != 7 && n != 8 && n != 13) {
        return false;
    }
    parts.iter().all(|p| hex_color(p).is_some())
}

fn clean_num(x: f64) -> String {
    if x == x.round() {
        format!("{}", x as i64)
    } else {
        format!("{x}")
    }
}

/// `configValueProblem(section:key:value:)`.
pub fn config_value_problem(section: &str, key: &str, value: &str) -> Option<String> {
    if value.is_empty() {
        return None;
    }
    if section == "theme" || (section == "app" && (key == "pane-focus-color" || key == "preview-border")) {
        return if hex_color(value).is_none() {
            Some(format!(
                "'{value}' is not a hex color (RRGGBB / AARRGGBB)"
            ))
        } else {
            None
        };
    }
    if section == "themes" {
        return if !theme_preset_parse_ok(key, value) {
            Some("expected 7, 8 or 13 hex colors: background, browser, terminal, header, text, dim, highlight[, accent[, accent2, success, warning, danger, info]]".to_string())
        } else {
            None
        };
    }
    if section == "app" && key.starts_with("quick-hotkey-") {
        return crate::platform::global_hotkey::parse_spec(value).err();
    }
    if key == "favorites" {
        for e in value
            .split(',')
            .map(str::trim)
            .filter(|e| !e.is_empty())
        {
            let expanded = expand_tilde(e);
            if !Path::new(&expanded).is_dir() {
                return Some(format!(
                    "'{e}' is not a folder (separate favorites with commas)"
                ));
            }
        }
        return None;
    }
    if section == "pane-shot" {
        match key {
            "background" => {
                return if !rgb_hex_ok(value) {
                    Some(format!("'{value}' is not a hex color (#RRGGBB)"))
                } else {
                    None
                };
            }
            "lines" => {
                return match value.parse::<i64>() {
                    Ok(n) if (0..=1000).contains(&n) => None,
                    _ => Some(format!("{value}: 0…1000 (herdr's cap)")),
                };
            }
            _ => {}
        }
    }
    if section == "screenshot" {
        match key {
            "return" => {
                return if ["copy", "save", "pin"].contains(&value.to_lowercase().as_str()) {
                    None
                } else {
                    Some(format!("'{value}' is not one of copy | pin | save"))
                };
            }
            "start-mode" => {
                return if ["screenshot", "text"].contains(&value.to_lowercase().as_str()) {
                    None
                } else {
                    Some(format!("'{value}' is not one of screenshot | text"))
                };
            }
            "save-format" => {
                return if ["png", "jpg", "jpeg"].contains(&value.to_lowercase().as_str()) {
                    None
                } else {
                    Some(format!("'{value}' is not one of jpg | png"))
                };
            }
            "button-size" => {
                if let Ok(n) = value.parse::<f64>() {
                    if n > 0.0 && n < 20.0 {
                        return Some(format!("{value}: 0 (automatic) or 20…80"));
                    }
                }
            }
            "ui-color" | "contrast-color" | "draw-color" => {
                if value.to_lowercase() == "theme" && key == "ui-color" {
                    return None;
                }
                return if !rgb_hex_ok(value) {
                    Some(format!("'{value}' is not a hex color (#RRGGBB)"))
                } else {
                    None
                };
            }
            "user-colors" => {
                let bad: Vec<String> = value
                    .split(',')
                    .map(str::trim)
                    .filter(|s| s.to_lowercase() != "picker" && !rgb_hex_ok(s))
                    .map(str::to_string)
                    .collect();
                return if bad.is_empty() {
                    None
                } else {
                    Some(format!("not hex colors: {}", bad.join(", ")))
                };
            }
            "buttons" => {
                let bad: Vec<String> = value
                    .split(',')
                    .map(|s| s.trim().to_lowercase())
                    .filter(|s| !SHOT_TOOLS.contains(&s.as_str()))
                    .collect();
                return if bad.is_empty() {
                    None
                } else {
                    Some(format!("unknown buttons: {}", bad.join(", ")))
                };
            }
            _ => {}
        }
    }
    if CONFIG_BOOL_KEYS.contains(&key) && config_text::tri(Some(value)).is_none() {
        return Some(format!("'{value}' is not true/false"));
    }
    if let Some((lo, hi)) = config_number_range(key) {
        let n = match value.parse::<f64>() {
            Ok(n) => n,
            Err(_) => return Some(format!("'{value}' is not a number")),
        };
        if !(lo..=hi).contains(&n) {
            return Some(format!(
                "{value} is outside {}…{}",
                clean_num(lo),
                clean_num(hi)
            ));
        }
    }
    // `[app] palette` is the Hyper+S command id list (unknown ids are skipped
    // when the palette opens); every other section's `palette` is colors.
    if key == "palette" && section != "app" && parse_palette(Some(value)).is_none() {
        return Some("expected 5 hex colors: accent2, success, warning, danger, info".to_string());
    }
    if CONFIG_COLOR_KEYS.contains(&key) && hex_color(value).is_none() {
        return Some(format!("'{value}' is not a hex color (RRGGBB / AARRGGBB)"));
    }
    if let Some(allowed) = config_enum_keys(key) {
        if !allowed.contains(&value.to_lowercase().as_str()) {
            let mut sorted = allowed.clone();
            sorted.sort_unstable();
            return Some(format!(
                "'{value}' is not one of {}",
                sorted.join(" | ")
            ));
        }
    }
    if key == "columns" {
        if value.trim().starts_with('[') {
            return columns_json_problem(value);
        }
        for part in value.split(',') {
            let seg: Vec<String> = part
                .split(':')
                .map(|p| p.trim().to_string())
                .collect();
            if seg.first().map(|s| s.is_empty()).unwrap_or(true) {
                return Some("an entry has no field name".to_string());
            }
            if seg.len() > 2 && !seg[2].is_empty() && seg[2].parse::<f64>().is_err() {
                return Some(format!(
                    "'{}' width '{}' is not a number (percent)",
                    seg[0], seg[2]
                ));
            }
            if seg.len() > 3
                && !seg[3].is_empty()
                && !["left", "right", "center"].contains(&seg[3].to_lowercase().as_str())
            {
                return Some(format!(
                    "'{}' align '{}' is not left | right | center",
                    seg[0], seg[3]
                ));
            }
            for f in seg
                .iter()
                .skip(4)
                .flat_map(|s| {
                    s.to_lowercase()
                        .split(|c| c == '+' || c == '/' || c == '|')
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
            {
                if f != "filter" && f != "sort" {
                    return Some(format!(
                        "'{}' flag '{}' is not filter | sort",
                        seg[0], f
                    ));
                }
            }
        }
        let total: f64 = ListColumn::parse(Some(value)).iter().map(|c| c.width).sum();
        if total > 100.5 {
            return Some(format!(
                "column widths add up to {}% (> 100 — they are scaled to fit)",
                total as i64
            ));
        }
    }
    None
}

fn first_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// `validateConfig(_:)`.
/// `config_value_problem` for a structured (JSON array) `columns` value.
fn columns_json_problem(value: &str) -> Option<String> {
    let Ok(serde_json::Value::Array(arr)) = serde_json::from_str::<serde_json::Value>(value) else {
        return Some("columns must be a JSON list of {field, width, align, filter, sort}".to_string());
    };
    if arr.is_empty() {
        return Some("at least one column is required".to_string());
    }
    for o in &arr {
        let field = o.get("field").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        if field.is_empty() {
            return Some("a column has no field name".to_string());
        }
        if let Some(w) = o.get("width") {
            match w.as_f64() {
                Some(n) if (0.0..=100.0).contains(&n) => {}
                _ => return Some(format!("'{field}' width must be 0-100")),
            }
        }
        if let Some(px) = o.get("width_px") {
            match px.as_f64() {
                Some(n) if n >= 0.0 => {}
                _ => return Some(format!("'{field}' width_px must be >= 0")),
            }
        }
        if let Some(a) = o.get("align").and_then(|v| v.as_str()) {
            if !["left", "right", "center"].contains(&a.to_lowercase().as_str()) {
                return Some(format!("'{field}' align '{a}' is not left | right | center"));
            }
        }
    }
    let total: f64 = parse_columns(value).iter().map(|c| c.width).sum();
    if total > 100.5 {
        return Some(format!(
            "column widths add up to {}% (> 100 — they are scaled to fit)",
            total as i64
        ));
    }
    None
}

pub fn validate_config(text: &str) -> Vec<ConfigIssue> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let lines = config_text::config_decoded_lines(text);
    // A helper failure yields no decoded lines (a successful decode returns one
    // record per source line, blanks and comments included). Do not report a
    // good file as "no `key = value` entries": `reload` rejects + parks on
    // fatal issues, so a flaky helper must not disable the user's config.
    if lines.is_empty() && !text.is_empty() {
        return Vec::new();
    }
    validate_config_lines(&lines)
}

/// The decoder is injected so validation is testable without the Python helper.
pub fn validate_config_lines(lines: &[ConfigDecodedLine]) -> Vec<ConfigIssue> {
    let mut issues: Vec<ConfigIssue> = Vec::new();
    let fail = |issues: &mut Vec<ConfigIssue>, n: i64, m: String| {
        issues.push(ConfigIssue {
            line: n,
            message: m,
            fatal: true,
        });
    };
    let warn = |issues: &mut Vec<ConfigIssue>, n: i64, m: String| {
        issues.push(ConfigIssue {
            line: n,
            message: m,
            fatal: false,
        });
    };

    let mut section: Option<String> = None;
    let mut seen_sections: Vec<String> = Vec::new();
    let mut seen_keys: Vec<String> = Vec::new();
    let mut entries = 0i64;
    let mut garbage = 0i64;

    for rec in lines {
        let n = rec.index + 1;
        let s = rec.trimmed.as_str();
        if s.is_empty() || s.starts_with('#') {
            continue;
        }
        if s.starts_with('[') {
            let name = rec.header.clone().unwrap_or_default();
            if name.is_empty() || name.contains('[') || name.contains(']') {
                fail(
                    &mut issues,
                    n,
                    format!(
                        "malformed section header '{}' (expected [name])",
                        first_chars(s, 40)
                    ),
                );
                continue;
            }
            if seen_sections.contains(&name) {
                warn(&mut issues, n, format!("duplicate section [{name}]"));
            }
            seen_sections.push(name.clone());
            section = Some(name);
            seen_keys.clear();
            continue;
        }
        let (key, val) = match (&rec.key, &rec.value) {
            (Some(k), Some(v)) => (k.clone(), v.clone()),
            _ => {
                garbage += 1;
                warn(
                    &mut issues,
                    n,
                    format!("ignored line (no '='): {}", first_chars(s, 40)),
                );
                continue;
            }
        };
        if key.is_empty() {
            garbage += 1;
            warn(&mut issues, n, "missing key before '='".to_string());
            continue;
        }
        entries += 1;
        let Some(sec) = section.clone() else { continue };
        if seen_keys.contains(&key) {
            warn(
                &mut issues,
                n,
                format!("[{sec}] duplicate key '{key}' (the last one wins)"),
            );
        }
        seen_keys.push(key.clone());
        if let Some(problem) = config_value_problem(&sec, &key, &val) {
            warn(&mut issues, n, format!("[{sec}] {key}: {problem}"));
        }
    }

    if entries == 0 {
        fail(&mut issues, 0, "no `key = value` entries".to_string());
    } else if garbage > entries.max(5) {
        fail(
            &mut issues,
            0,
            format!("{garbage} unparseable lines — the file looks corrupted"),
        );
    }
    issues
}

// ---------------------------------------------------------------------------
// commands.toml read / write
// ---------------------------------------------------------------------------

/// The config file path. `WS_COMMANDS_CONF` (set by `main.swift` for the python
/// side) wins; else `$WS_HOME`, else `~/.config/kitchen-sink`.
pub fn commands_conf_path() -> PathBuf {
    if let Ok(p) = std::env::var("WS_COMMANDS_CONF") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    let dir = std::env::var("WS_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("{}/.config/kitchen-sink", home_dir()));
    PathBuf::from(dir).join("commands.toml")
}

pub fn read_config_text_at(path: &Path) -> Option<String> {
    // Drop a UTF-8 BOM so the first section header still parses.
    std::fs::read_to_string(path).ok().map(|t| match t.strip_prefix('\u{feff}') {
        Some(rest) => rest.to_string(),
        None => t,
    })
}

/// The personal overrides file, kept outside the repo in a sibling folder
/// (`~/.config/kitchen-sink-local/commands.toml` — `~/.config/kitchen-sink`
/// is the repo itself; `WS_COMMANDS_OVERRIDE` moves it, an empty value turns
/// it off). `bin/check-overrides.sh` (install / setup) reports the same path. Any key it sets wins over the same `[section]` key in
/// the repo's commands.toml, which stays the full default/backup copy.
pub fn commands_override_path() -> Option<PathBuf> {
    match std::env::var("WS_COMMANDS_OVERRIDE") {
        Ok(p) if p.is_empty() => None,
        Ok(p) => Some(PathBuf::from(p)),
        Err(_) => Some(config_dir().join("commands.toml")),
    }
}

/// THE folder for personal config: the commands.toml override, the Files
/// pins, `jira/` (token + team) and `confluence/`. `$WS_CONFIG_DIR`, else
/// `pylib/paths.json` `configDir` (the one place to change it), else
/// `~/.config/kitchen-sink-local`.
pub fn config_dir() -> PathBuf {
    if let Ok(p) = std::env::var("WS_CONFIG_DIR") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    let home = home_dir();
    let assets = crate::app::paths::Paths::from_env().asset_dir().to_string();
    let from_json = std::fs::read_to_string(format!("{assets}/pylib/paths.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.get("configDir").and_then(|s| s.as_str()).map(str::to_string));
    let raw = from_json.unwrap_or_else(|| "~/.config/kitchen-sink-local".to_string());
    match raw.strip_prefix("~/") {
        Some(rest) => PathBuf::from(format!("{home}/{rest}")),
        None => PathBuf::from(raw),
    }
}

/// `[section] -> [(key, value)]` of a config text, in file order (decoded
/// values, so they can be fed straight back to `config_setting_text`).
fn override_entries(text: &str) -> Vec<(String, Vec<(String, Option<String>)>)> {
    let mut out: Vec<(String, Vec<(String, Option<String>)>)> = Vec::new();
    for line in config_text::config_decoded_lines(text) {
        if let Some(h) = line.header {
            if !out.iter().any(|(s, _)| *s == h) {
                out.push((h, Vec::new()));
            }
        } else if let (Some(k), Some(v)) = (line.key, line.value) {
            if let Some((_, kv)) = out.last_mut() {
                kv.retain(|(key, _)| *key != k);
                kv.push((k, Some(v)));
            }
        }
    }
    out
}

/// Lay the override file's keys over `base` (memoised on both texts: every
/// view re-reads the config). A broken or unreadable override is ignored.
pub fn apply_override(base: &str) -> String {
    use std::sync::{Mutex, OnceLock};
    static MEMO: OnceLock<Mutex<Option<(String, String, String)>>> = OnceLock::new();
    let Some(over) = commands_override_path().and_then(|p| read_config_text_at(&p)) else {
        return base.to_string();
    };
    if over.trim().is_empty() || validate_config(&over).iter().any(|i| i.fatal) {
        return base.to_string();
    }
    let memo = MEMO.get_or_init(|| Mutex::new(None));
    if let Some((b, o, merged)) = memo.lock().unwrap().as_ref() {
        if b == base && *o == over {
            return merged.clone();
        }
    }
    // Helper not up yet: use the repo text, and don't memoise.
    let Some(text) = merge_override(base, &over) else {
        return base.to_string();
    };
    *memo.lock().unwrap() = Some((base.to_string(), over, text.clone()));
    text
}

/// `base` with every `[section] key` from `over` set (added when missing).
/// `None` = the codec call failed.
pub fn merge_override(base: &str, over: &str) -> Option<String> {
    let mut text = base.to_string();
    for (section, kv) in override_entries(over) {
        if !kv.is_empty() {
            text = config_text::config_setting_text(&text, &section, &kv)?;
        }
    }
    Some(text)
}

pub fn read_config_text() -> Option<String> {
    read_config_text_at(&commands_conf_path()).map(|t| apply_override(&t))
}

/// `read_config_text` without the Python helper: the quick agent must not
/// start a worker pool, so the override is laid over the repo file by a
/// plain line scan ([`merge_override_pure`]) instead of the `config.*`
/// codec. A missing / empty override leaves the repo text untouched.
pub fn read_config_text_pure() -> Option<String> {
    let base = read_config_text_at(&commands_conf_path())?;
    let over = commands_override_path().and_then(|p| read_config_text_at(&p));
    Some(match over {
        Some(o) if !o.trim().is_empty() => merge_override_pure(&base, &o),
        _ => base,
    })
}

/// `(section, [(key, raw "key = value" line)])` of a config text, in file
/// order; a repeated key keeps its last line.
fn scan_entries(text: &str) -> Vec<(String, Vec<(String, String)>)> {
    let mut out: Vec<(String, Vec<(String, String)>)> = Vec::new();
    for line in text.strip_prefix('\u{feff}').unwrap_or(text).lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if let Some(name) = t.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
            let name = name.trim().to_string();
            if !out.iter().any(|(s, _)| *s == name) {
                out.push((name, Vec::new()));
            }
            continue;
        }
        let Some((k, _)) = t.split_once('=') else { continue };
        let key = k.trim().to_string();
        if key.is_empty() {
            continue;
        }
        // Keys before any header are not part of a section.
        let Some(last) = out.last_mut() else { continue };
        last.1.retain(|(existing, _)| *existing != key);
        last.1.push((key, t.to_string()));
    }
    out
}

/// Python-free [`merge_override`]: every `[section] key` of `over` replaces
/// the same key's line in `base` (in place), is added at the end of its
/// section, or brings a new section along.
pub fn merge_override_pure(base: &str, over: &str) -> String {
    let mut lines: Vec<String> =
        base.strip_prefix('\u{feff}').unwrap_or(base).lines().map(str::to_string).collect();
    let header_of = |l: &str| -> Option<String> {
        let t = l.trim();
        t.strip_prefix('[').and_then(|r| r.strip_suffix(']')).map(|n| n.trim().to_string())
    };
    for (section, entries) in scan_entries(over) {
        for (key, raw) in entries {
            // The section's line range [start, end) in the current text.
            let start = lines.iter().position(|l| header_of(l).as_deref() == Some(section.as_str()));
            let Some(start) = start else {
                if lines.last().is_some_and(|l| !l.trim().is_empty()) {
                    lines.push(String::new());
                }
                lines.push(format!("[{section}]"));
                lines.push(raw);
                continue;
            };
            let end = lines[start + 1..]
                .iter()
                .position(|l| header_of(l).is_some())
                .map_or(lines.len(), |i| start + 1 + i);
            let hit = (start + 1..end).find(|&i| {
                let t = lines[i].trim();
                !t.starts_with('#') && t.split_once('=').is_some_and(|(k, _)| k.trim() == key)
            });
            match hit {
                Some(i) => lines[i] = raw,
                None => {
                    // After the section's last non-blank line.
                    let at = (start + 1..end)
                        .rev()
                        .find(|&i| !lines[i].trim().is_empty())
                        .map_or(start + 1, |i| i + 1);
                    lines.insert(at, raw);
                }
            }
        }
    }
    let mut text = lines.join("\n");
    if base.ends_with('\n') {
        text.push('\n');
    }
    text
}

/// The launch-time check of the config paths (same report as
/// `bin/check-overrides.sh`): one log line per file, plus a warning when an
/// old `~/.config/jira` / `~/.config/confluence` folder exists (no longer read).
pub fn startup_config_report() -> (Vec<String>, Option<String>) {
    let dir = config_dir();
    let mark = |p: &Path| if p.exists() { "found" } else { "not created yet" };
    let mut lines = vec![
        format!("config: defaults/backup {}", tilde(&commands_conf_path())),
        format!("config: folder {} ({})", tilde(&dir), mark(&dir)),
    ];
    if let Some(o) = commands_override_path() {
        lines.push(format!("config: override {} ({})", tilde(&o), mark(&o)));
    }
    for rel in ["files-favorites.json", "jira/config.json", "jira/team.json", "confluence/config.json"] {
        let p = dir.join(rel);
        lines.push(format!("config: {} ({})", tilde(&p), mark(&p)));
    }
    let home = home_dir();
    let stale: Vec<String> = [".config/jira", ".config/confluence"]
        .iter()
        .map(|r| PathBuf::from(format!("{home}/{r}")))
        .filter(|p| p.exists())
        .map(|p| tilde(&p))
        .collect();
    let warn = (!stale.is_empty()).then(|| {
        format!("{} is no longer read — move its files into {}", stale.join(" and "), tilde(&dir))
    });
    if let Some(w) = &warn {
        lines.push(format!("config: WARNING {w}"));
    }
    (lines, warn)
}

/// `~`-spelled path for menu titles.
pub fn tilde(p: &Path) -> String {
    crate::app::pathfmt::tilde_path(&p.to_string_lossy(), &home_dir())
}

/// Create the override file with a commented stub if it is missing (so the
/// menu's "Open Overrides" always has something to open). Returns its path.
pub fn ensure_override_file() -> Option<PathBuf> {
    let p = commands_override_path()?;
    if !p.exists() {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let stub = format!(
            "# Personal overrides for kitchen-sink (machine-local, not in the repo).\n\
             # Any [section] key set here wins over the same key in\n\
             # {}, which stays the full default/backup.\n\
             # Example — the Files sidebar's pinned folders:\n\
             #\n# [files]\n# favorites = \"~/notes, ~/Desktop\"\n",
            tilde(&commands_conf_path())
        );
        let _ = std::fs::write(&p, stub);
    }
    Some(p)
}

pub fn write_config_text_at(path: &Path, text: &str) -> bool {
    if validate_config(text).iter().any(|i| i.fatal) {
        return false;
    }
    if let Some(cur) = read_config_text_at(path) {
        // Refuse to rewrite a broken file: the codec would reduce it to a
        // one-section stub, and backing the broken text up would clobber the
        // last-known-good `.bak`. The broken file stays put for inspection.
        if validate_config(&cur).iter().any(|i| i.fatal) {
            return false;
        }
        if cur != text {
            let backup = PathBuf::from(format!("{}.bak", path.display()));
            let _ = std::fs::write(&backup, &cur);
        }
    }
    std::fs::write(path, text).is_ok()
}

/// Park a config file whose text failed validation (or was not valid UTF-8)
/// beside itself as `<name>.broken-<epoch>`. The live file is left in place so
/// it can still be inspected/fixed; the caller keeps the last-known-good
/// settings in force. Returns the parked path.
pub fn park_broken_config(path: &Path, bytes: &[u8]) -> Option<PathBuf> {
    let epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let parked = PathBuf::from(format!("{}.broken-{epoch}", path.display()));
    std::fs::write(&parked, bytes).ok().map(|_| parked)
}

/// `saveConfigValues(section:_:)` against an explicit file (testable).
pub fn save_config_values_at(
    path: &Path,
    section: &str,
    kv: &[(String, Option<String>)],
) -> bool {
    let Some(content) = read_config_text_at(path) else {
        return false;
    };
    let Some(text) = config_text::config_setting_text(&content, section, kv) else {
        return false;
    };
    write_config_text_at(path, &text)
}

/// `saveConfigValues(section:_:)`. `false` = the write did not land (unreadable
/// config, codec timeout under helper load — the documented silent-no-op
/// footgun): callers that claim success to the user must check.
pub fn save_config_values(section: &str, kv: &[(String, Option<String>)]) -> bool {
    // A key the override file sets is saved there (else the change would be
    // shadowed by it); everything else goes to the repo's commands.toml.
    let over_path = commands_override_path();
    let overridden: Vec<String> = over_path
        .as_deref()
        .and_then(read_config_text_at)
        .map(|t| {
            override_entries(&t)
                .into_iter()
                .filter(|(s, _)| s == section)
                .flat_map(|(_, kv)| kv.into_iter().map(|(k, _)| k))
                .collect()
        })
        .unwrap_or_default();
    let (to_over, to_base): (Vec<_>, Vec<_>) =
        kv.iter().cloned().partition(|(k, _)| overridden.contains(k));
    let mut ok = true;
    if !to_over.is_empty() {
        ok &= save_config_values_at(over_path.as_deref().unwrap(), section, &to_over);
    }
    if !to_base.is_empty() {
        ok &= save_config_values_at(&commands_conf_path(), section, &to_base);
    }
    ok
}

pub fn save_config_value(section: &str, key: &str, value: &str) {
    save_config_values(section, &[(key.to_string(), Some(value.to_string()))]);
}

// ---------------------------------------------------------------------------
// SwitcherController.handleEscape (the palette Esc decision)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EscapeAction {
    /// Clear the query, refilter to "", reset the selection.
    ClearQuery,
    /// Hide the palette and hand focus back.
    HideRestore,
}

/// `SwitcherController.handleEscape`: a query first clears, then hides.
pub fn handle_escape(query_is_empty: bool) -> EscapeAction {
    if !query_is_empty {
        EscapeAction::ClearQuery
    } else {
        EscapeAction::HideRestore
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn pure_override_merge_replaces_adds_and_appends() {
        let base = "[app]\nshell = \"a\"\npalette = \"x\"\n\n[screenshot]\nhistory = 20\n";
        let over = "# mine\n[app]\npalette = \"paths, filefast\"\nextra = 1\n[new]\nk = v\n";
        let m = merge_override_pure(base, over);
        assert_eq!(
            m,
            "[app]\nshell = \"a\"\npalette = \"paths, filefast\"\nextra = 1\n\n[screenshot]\nhistory = 20\n\n[new]\nk = v\n"
        );
        // No override keys: the base text is unchanged.
        assert_eq!(merge_override_pure(base, "# nothing\n"), base);
        // The last duplicate in the override wins.
        let m2 = merge_override_pure("[a]\nk = 1\n", "[a]\nk = 2\nk = 3\n");
        assert_eq!(m2, "[a]\nk = 3\n");
    }
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn helper_ready() -> bool {
        let lib = format!("{}/../../pylib", env!("CARGO_MANIFEST_DIR"));
        if !crate::app::python_helper::lib_has_helper(&lib) {
            return false;
        }
        let h = crate::app::python_helper::PythonHelper::shared();
        h.configure(&lib);
        h.call_default("ping", json!({})).is_ok()
    }

    #[test]
    fn shared_window_sizes_screen_default_and_min() {
        let mut s = AppSettings::default();
        // Unset width/height: 85% of a laptop screen, min 900x560.
        assert_eq!(
            shared_window_sizes(&s, (1512.0, 944.0)),
            ((900.0, 560.0), (1285.0, 802.0))
        );
        // A small screen: the min fits the screen, the default never goes under it.
        assert_eq!(
            shared_window_sizes(&s, (1024.0, 743.0)),
            ((900.0, 560.0), (900.0, 632.0))
        );
        // Tiny screen: the min shrinks to the screen.
        assert_eq!(shared_window_sizes(&s, (800.0, 500.0)), ((800.0, 500.0), (800.0, 500.0)));
        // An explicit size wins, floored at the min and capped at the screen.
        s.shared_width = 700.0;
        s.shared_height = 5000.0;
        assert_eq!(
            shared_window_sizes(&s, (1512.0, 944.0)),
            ((900.0, 560.0), (900.0, 944.0))
        );
        let v = vars(&[
            ("shared-min-width", "100"),
            ("shared-min-height", "700"),
            ("shared-screen-fraction", "2"),
        ]);
        parse_app_config(&mut s, &v);
        assert_eq!((s.shared_min_width, s.shared_min_height), (400.0, 700.0));
        assert_eq!(s.shared_screen_fraction, 1.0);
    }

    #[test]
    fn defaults_match_swift() {
        let s = AppSettings::default();
        assert!(s.shared_window);
        assert_eq!(s.shared_width, 0.0);
        assert_eq!(s.shared_height, 0.0);
        assert_eq!((s.shared_min_width, s.shared_min_height), (900.0, 560.0));
        assert_eq!(s.shared_screen_fraction, 0.85);
        assert_eq!(s.shell, "/opt/homebrew/bin/bash");
        assert_eq!(s.terminal_font, "Hack Nerd Font");
        assert_eq!(s.jira_icon_name, "jira_icon.png");
        assert_eq!(s.confluence_icon_name, "confluence_icon.png");
        assert_eq!(s.ai_icon_name, "");
        assert_eq!(s.app_icon_name, "app_icon.png");
        assert_eq!(s.files_icon_name, "");
        assert_eq!(s.notes_socket_name, "ws-notes.sock");
        assert_eq!(s.focus_file_name, "kitchen-sink-focus");
        assert_eq!(s.focus_bridge_name, "ws-aerospace-focus");
        assert_eq!(s.switcher_window_name, "kitchen-sink");
        assert!(s.hide_on_focus_loss);
        assert_eq!(s.focus_loss_delay, 0.3);
        assert_eq!(s.esc_close, 0);
        assert_eq!(s.copy_toast, "Copied {} to clipboard");
        assert_eq!(s.header_style, HeaderStyle::Flat);
        assert_eq!(s.preview_border_width, 2.0);
        assert_eq!(s.pane_focus_width, 1.0);
        assert!(s.vim_keys && s.vim_mode_badge);
        assert_eq!(s.preview_border, Rgba::new(0.753, 0.753, 0.753, 1.0));
    }

    #[test]
    fn parse_app_block_fields_and_defaults() {
        let mut s = AppSettings::default();
        let v = vars(&[
            ("shell", "/bin/zsh"),
            ("terminal-font", "Fira Code"),
            ("ai-icon", "ai.png"),
            ("shared-width", "300"),     // below 400 -> ignored
            ("palette-first", "Paths, filefast, paths"), // repeat dropped
            ("palette", "Text-Table, paths, text-table"),
            ("esc-close", "2"),
            ("copy-toast", "Copied!"),
            ("header-style", "AURORA"),
            ("preview-border", "#112233"),
            ("preview-border-width", "99"), // clamped to 8
            ("pane-focus-color", "#445566"),
            ("pane-focus-width", "9"),      // clamped to 4
        ]);
        parse_app_config(&mut s, &v);

        assert_eq!(s.shell, "/bin/zsh");
        assert_eq!(s.terminal_font, "Fira Code");
        assert_eq!(s.ai_icon_name, "ai.png");
        assert_eq!(s.shared_width, 0.0);
        assert_eq!(s.palette_first, vec!["paths", "filefast"]);
        assert_eq!(s.palette, vec!["text-table", "paths"]);
        assert_eq!(s.esc_close, 2);
        assert_eq!(s.copy_toast, "Copied!");
        assert_eq!(s.header_style, HeaderStyle::Aurora);
        assert_eq!(s.preview_border, hex_color("#112233").unwrap());
        assert_eq!(s.preview_border_width, 8.0);
        assert_eq!(s.pane_focus_color, hex_color("#445566").unwrap());
        assert_eq!(s.pane_focus_width, 4.0);

        // Absent keys keep defaults.
        let mut s2 = AppSettings::default();
        parse_app_config(&mut s2, &vars(&[]));
        assert_eq!(s2.shell, "/opt/homebrew/bin/bash");
    }

    #[test]
    fn parse_app_booleans_when_helper_present() {
        if !helper_ready() {
            eprintln!("skipping: no python helper");
            return;
        }
        let mut s = AppSettings::default();
        parse_app_config(
            &mut s,
            &vars(&[
                ("hide-on-focus-loss", "false"),
                ("shared-window", "no"),
                ("vim-keys", "0"),
                ("vim-mode-badge", "false"),
            ]),
        );
        assert!(!s.hide_on_focus_loss);
        assert!(!s.shared_window);
        assert!(!s.vim_keys);
        assert!(!s.vim_mode_badge);
    }

    #[test]
    fn command_spec_field_parsing() {
        let v = vars(&[
            ("type", "list"),
            ("script", "echo hi"),
            ("name", "My List"),
            ("title", "Titled"),
            ("label", "  Labelled  "),
            ("paths", "/tmp/a, /tmp/b"),
            ("sources", "a.json, b.json"),
            ("max-rows", "50"),
            ("page-size", "25"),
            ("width", "500"),
            ("height", "200"),
            ("resize", "true"),
            ("drag", "false"),
            ("sticky", "false"),
            ("esc-close", "1"),
            ("start", "root"),
            ("recent-scope", "home"),
            ("sidebar-width", "300"),
            ("font-size", "14"),
            ("header-color", "#abcdef"),
        ]);
        let s = make_command("mylist", &v);
        assert_eq!(s.name, "mylist");
        assert_eq!(s.kind, CommandKind::List);
        assert_eq!(s.script.as_deref(), Some("echo hi"));
        assert_eq!(s.label.as_deref(), Some("Labelled"));
        assert_eq!(s.paths, vec!["/tmp/a", "/tmp/b"]);
        assert_eq!(s.sources, vec!["a.json", "b.json"]);
        assert_eq!(s.max_rows, 50);
        assert_eq!(s.page_size, 25);
        assert_eq!(s.width, 500.0);
        assert_eq!(s.height, 200.0);
        assert_eq!(s.esc_close, Some(1));
        assert!(!s.start_recent);
        assert!(!s.recent_everywhere);
        assert_eq!(s.sidebar_width, 300.0);
        assert_eq!(s.font_size, 14.0);
        assert_eq!(s.header_color, hex_color("#abcdef"));
        assert_eq!(s.save_dir, "/tmp/");

        // Defaults: no `type` -> shell; resize false, drag/sticky true.
        let d = make_command("plain", &vars(&[]));
        assert_eq!(d.kind, CommandKind::Shell);
        assert!(!d.resize && d.drag && d.sticky);
        assert!(d.in_palette);
        assert!(d.recent);
    }

    #[test]
    fn command_spec_booleans_when_helper_present() {
        if !helper_ready() {
            eprintln!("skipping: no python helper");
            return;
        }
        let v = vars(&[
            ("in-palette", "false"),
            ("drag", "no"),
            ("resize", "yes"),
            ("checkbox", "on"),
            ("esc-close", "3"),
            ("vim-esc-close", "5"),
        ]);
        let s = make_command("x", &v);
        assert!(!s.in_palette);
        assert!(!s.drag);
        assert!(s.resize);
        assert_eq!(s.checkbox, Some(true));
        assert_eq!(s.esc_close, Some(3)); // esc-close wins over vim-esc-close
    }

    #[test]
    fn number_key_bounds() {
        assert_eq!(config_number_range("limit"), Some((1.0, 25.0)));
        assert_eq!(config_number_range("max-bytes"), Some((1_048_576.0, 2_147_483_648.0)));
        assert_eq!(config_number_range("nope"), None);
        assert_eq!(config_number_keys().len(), CONFIG_NUMBER_KEYS.len());

        // in range
        assert_eq!(config_value_problem("app", "limit", "10"), None);
        // out of range
        assert_eq!(
            config_value_problem("app", "limit", "30").as_deref(),
            Some("30 is outside 1…25")
        );
        // non-number
        assert_eq!(
            config_value_problem("app", "width", "wide").as_deref(),
            Some("'wide' is not a number")
        );
        // large bound formatting
        assert_eq!(
            config_value_problem("files", "max-bytes", "1").as_deref(),
            Some("1 is outside 1048576…2147483648")
        );
    }

    #[test]
    fn value_problem_cases() {
        // hex color (app preview-border)
        assert_eq!(
            config_value_problem("app", "preview-border", "nope").as_deref(),
            Some("'nope' is not a hex color (RRGGBB / AARRGGBB)")
        );
        assert_eq!(config_value_problem("app", "preview-border", "#dde1e8"), None);
        // palette: colors in a view section, command ids in [app]
        assert_eq!(
            config_value_problem("notes", "palette", "#111111").as_deref(),
            Some("expected 5 hex colors: accent2, success, warning, danger, info")
        );
        assert_eq!(config_value_problem("app", "palette", "paths, filefast, text-table, prettyprint"), None);
        // enum
        assert_eq!(
            config_value_problem("app", "header-style", "bogus").as_deref(),
            Some("'bogus' is not one of aurora | edge | flat | glow | quiet | stripe | tinted")
        );
        // columns
        assert_eq!(
            config_value_problem("jira", "columns", "key:Key:10").as_deref(),
            None
        );
        assert_eq!(
            config_value_problem("jira", "columns", "key:Key:abc").as_deref(),
            Some("'key' width 'abc' is not a number (percent)")
        );
        assert_eq!(
            config_value_problem("jira", "columns", "key:Key:10:middle").as_deref(),
            Some("'key' align 'middle' is not left | right | center")
        );
        assert_eq!(
            config_value_problem("jira", "columns", "key:Key:10:left:bogus").as_deref(),
            Some("'key' flag 'bogus' is not filter | sort")
        );
        // screenshot
        assert_eq!(
            config_value_problem("screenshot", "return", "none").as_deref(),
            Some("'none' is not one of copy | pin | save")
        );
        assert_eq!(config_value_problem("screenshot", "ui-color", "theme"), None);
        assert_eq!(
            config_value_problem("screenshot", "buttons", "pencil, bogus").as_deref(),
            Some("unknown buttons: bogus")
        );
        // pane-shot
        assert_eq!(
            config_value_problem("pane-shot", "lines", "5000").as_deref(),
            Some("5000: 0…1000 (herdr's cap)")
        );
        assert_eq!(config_value_problem("pane-shot", "background", "#aabbcc"), None);
        // empty value is always fine
        assert_eq!(config_value_problem("app", "limit", ""), None);
    }

    #[test]
    fn bool_and_tri_value_problem_when_helper_present() {
        if !helper_ready() {
            eprintln!("skipping: no python helper");
            return;
        }
        assert_eq!(
            config_value_problem("app", "vim-keys", "maybe").as_deref(),
            Some("'maybe' is not true/false")
        );
        assert_eq!(config_value_problem("app", "vim-keys", "true"), None);
        assert_eq!(config_value_problem("prettyprint", "syntax-highlight", "true"), None);
        assert_eq!(
            config_value_problem("prettyprint", "syntax-highlight", "sometimes").as_deref(),
            Some("'sometimes' is not true/false")
        );
    }

    #[test]
    fn validate_config_lines_reports_issues() {
        let lines = vec![
            ConfigDecodedLine {
                index: 0,
                trimmed: "[app]".into(),
                header: Some("app".into()),
                key: None,
                value: None,
            },
            ConfigDecodedLine {
                index: 1,
                trimmed: "vim-keys = true".into(),
                header: None,
                key: Some("vim-keys".into()),
                value: Some("true".into()),
            },
            ConfigDecodedLine {
                index: 2,
                trimmed: "vim-keys = false".into(),
                header: None,
                key: Some("vim-keys".into()),
                value: Some("false".into()),
            },
            ConfigDecodedLine {
                index: 3,
                trimmed: "garbage line".into(),
                header: None,
                key: None,
                value: None,
            },
        ];
        let issues = validate_config_lines(&lines);
        // duplicate key warning on line 3
        assert!(issues
            .iter()
            .any(|i| i.line == 3 && !i.fatal && i.message.contains("duplicate key 'vim-keys'")));
        // no-equals warning on line 4
        assert!(issues
            .iter()
            .any(|i| i.line == 4 && !i.fatal && i.message.contains("ignored line")));
        assert!(!issues.iter().any(|i| i.fatal));
    }

    #[test]
    fn validate_config_empty_is_fatal() {
        let issues = validate_config_lines(&[]);
        assert!(issues.iter().any(|i| i.fatal && i.message == "no `key = value` entries"));
    }

    #[test]
    fn escape_decision() {
        assert_eq!(handle_escape(false), EscapeAction::ClearQuery);
        assert_eq!(handle_escape(true), EscapeAction::HideRestore);
    }

    #[test]
    fn write_refuses_to_stub_a_broken_config() {
        if !helper_ready() {
            eprintln!("skipping: no python helper");
            return;
        }
        let path =
            std::env::temp_dir().join(format!("ws-config-broken-{}.toml", std::process::id()));
        let bak = std::path::PathBuf::from(format!("{}.bak", path.display()));
        let _ = std::fs::remove_file(&bak);
        let good = "[app]\nvim-keys = true\n[notes]\nsidebar-width = 299\n";
        std::fs::write(&path, good).unwrap();

        // A normal write lands and leaves the previous file as the backup.
        assert!(write_config_text_at(&path, "[app]\nvim-keys = false\n"));
        assert!(!std::fs::read_to_string(&bak).unwrap().is_empty());
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), good);

        // Now the live file is broken (truncated): a setting write must refuse
        // instead of reducing it to a one-section stub and clobbering the
        // last-good .bak with the 0-byte file.
        std::fs::write(&path, "").unwrap();
        assert!(!write_config_text_at(
            &path,
            "\n[notes]\nsidebar-width = 333\n"
        ));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
        assert_eq!(
            std::fs::read_to_string(&bak).unwrap(),
            good,
            "last-good backup preserved"
        );

        // The reported path: save_config_values against the broken file.
        assert!(!save_config_values_at(
            &path,
            "notes",
            &[("sidebar-width".to_string(), Some("333".to_string()))]
        ));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
        assert_eq!(
            std::fs::read_to_string(&bak).unwrap(),
            good,
            "last-good backup preserved"
        );

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&bak);
    }

    #[test]
    fn override_keys_win_and_new_keys_are_added() {
        if !helper_ready() {
            eprintln!("skipping: no python helper");
            return;
        }
        let base = "[app]\nshell = \"/bin/zsh\"\n\n[files]\nfavorites = \"~/notes, /tmp\"\nsort = \"name\"\n";
        let over = "[files]\nfavorites = \"~/work, ~/Desktop\"\n\n[jira]\nenabled = true\n";
        let merged = merge_override(base, over).unwrap();
        let entries = override_entries(&merged);
        let get = |sec: &str, key: &str| {
            entries.iter().find(|(s, _)| s == sec).and_then(|(_, kv)| {
                kv.iter().find(|(k, _)| k == key).and_then(|(_, v)| v.clone())
            })
        };
        assert_eq!(get("files", "favorites").as_deref(), Some("~/work, ~/Desktop"));
        assert_eq!(get("files", "sort").as_deref(), Some("name"));
        assert_eq!(get("app", "shell").as_deref(), Some("/bin/zsh"));
        assert_eq!(get("jira", "enabled").as_deref(), Some("true"));
        assert!(!merged.contains("~/notes"), "got: {merged}");
    }

    #[test]
    fn save_and_remove_config_value_round_trip() {
        if !helper_ready() {
            eprintln!("skipping: no python helper");
            return;
        }
        let dir = std::env::temp_dir();
        let path = dir.join(format!("ws-config-test-{}.toml", std::process::id()));
        let original = "# top\n[app]\n  vim-keys = true  # keep me\nshell = \"/bin/zsh\"\n";
        std::fs::write(&path, original).unwrap();

        // change an existing value, preserving indent + trailing comment
        assert!(save_config_values_at(
            &path,
            "app",
            &[("vim-keys".to_string(), Some("false".to_string()))]
        ));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("  vim-keys = false  # keep me"), "got: {text}");
        assert!(text.contains("# top"));

        // add a new key after the section's last entry
        assert!(save_config_values_at(
            &path,
            "app",
            &[("focus-loss-delay".to_string(), Some("0.5".to_string()))]
        ));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("focus-loss-delay = 0.5"), "got: {text}");

        // remove a key
        assert!(save_config_values_at(&path, "app", &[("shell".to_string(), None)]));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("shell ="), "got: {text}");

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}.bak", path.display()));
    }
}

// ---- columns parsing (moved from views::jira so config validation needs no view) ----

/// `columns` -> the columns. Accepts the structured JSON array of
/// `{field,title,width,align,filter,sort}` objects and the legacy one-line
/// string `field:Title:width:align:flags, ...`. Title defaults to the field;
/// width 0 shares the leftover width; negative widths clamp to 0; flags set
/// the sortable / filterable bits.
pub fn parse_columns(spec: &str) -> Vec<ListColumn> {
    let text = spec.trim();
    if text.starts_with('[') {
        return match serde_json::from_str::<Value>(text) {
            Ok(Value::Array(a)) => a.iter().filter_map(column_from_obj).collect(),
            _ => Vec::new(),
        };
    }
    let mut cols = Vec::new();
    for part in spec.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let seg: Vec<&str> = part.split(':').map(str::trim).collect();
        let field = seg[0];
        if field.is_empty() {
            continue;
        }
        let title = if seg.len() > 1 && !seg[1].is_empty() {
            seg[1].to_string()
        } else {
            field.to_string()
        };
        let width = if seg.len() > 2 && !seg[2].is_empty() {
            seg[2].parse::<f64>().unwrap_or(0.0)
        } else {
            0.0
        };
        let mut align = if seg.len() > 3 && !seg[3].is_empty() {
            seg[3].to_lowercase()
        } else {
            "left".to_string()
        };
        if !matches!(align.as_str(), "left" | "right" | "center") {
            align = "left".to_string();
        }
        let mut flags: Vec<String> = Vec::new();
        for s in &seg[4.min(seg.len())..] {
            for f in s.split(|c| c == '+' || c == '|' || c == '/') {
                let f = f.trim().to_lowercase();
                if !f.is_empty() {
                    flags.push(f);
                }
            }
        }
        cols.push(ListColumn {
            field: field.to_string(),
            title,
            width: width.max(0.0),
            width_px: None,
            align,
            sortable: flags.iter().any(|f| f == "sort"),
            filterable: flags.iter().any(|f| f == "filter"),
        });
    }
    cols
}

/// One structured column object -> a `ListColumn` (`None` without a field).
pub fn column_from_obj(o: &Value) -> Option<ListColumn> {
    let field = o.get("field").and_then(Value::as_str).unwrap_or("").trim().to_string();
    if field.is_empty() {
        return None;
    }
    let title = o
        .get("title")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or(&field)
        .to_string();
    let width = o.get("width").and_then(Value::as_f64).unwrap_or(0.0).max(0.0);
    let raw_align = o.get("align").and_then(Value::as_str).unwrap_or("left").to_lowercase();
    let align = if matches!(raw_align.as_str(), "left" | "right" | "center") {
        raw_align
    } else {
        "left".to_string()
    };
    Some(ListColumn {
        field,
        title,
        width,
        width_px: o.get("width_px").and_then(Value::as_f64),
        align,
        sortable: o.get("sort").map(truthy).unwrap_or(false),
        filterable: o.get("filter").map(truthy).unwrap_or(false),
    })
}

pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|x| x != 0.0).unwrap_or(false),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}
