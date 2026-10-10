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

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgba {
    pub r: f64,
    pub g: f64,
    pub b: f64,
    pub a: f64,
}

impl Rgba {
    pub const fn new(r: f64, g: f64, b: f64, a: f64) -> Self {
        Rgba { r, g, b, a }
    }

    pub fn from_u8(r: u8, g: u8, b: u8, a: u8) -> Self {
        Rgba::new(
            r as f64 / 255.0,
            g as f64 / 255.0,
            b as f64 / 255.0,
            a as f64 / 255.0,
        )
    }
}

/// `hexColor(_:)`: optional `0x` / `#`, exactly 6 or 8 hex digits, alpha
/// clamped to a minimum of 0.08 (8-digit only).
pub fn hex_color(s: &str) -> Option<Rgba> {
    if s.is_empty() {
        return None;
    }
    let hex = s.strip_prefix("0x").unwrap_or(s);
    let hex = hex.strip_prefix('#').unwrap_or(hex);
    if hex.len() != 6 && hex.len() != 8 {
        return None;
    }
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let v = u32::from_str_radix(hex, 16).ok()?;
    let has_alpha = hex.len() == 8;
    let a = if has_alpha {
        ((v >> 24) & 0xFF) as f64 / 255.0
    } else {
        1.0
    };
    Some(Rgba::new(
        ((v >> 16) & 0xFF) as f64 / 255.0,
        ((v >> 8) & 0xFF) as f64 / 255.0,
        (v & 0xFF) as f64 / 255.0,
        a.max(0.08),
    ))
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
pub fn expand_tilde(s: &str) -> String {
    if let Some(rest) = s.strip_prefix('~') {
        let home = std::env::var("HOME").unwrap_or_default();
        return format!("{home}{rest}");
    }
    s.to_string()
}

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

fn user_name() -> String {
    std::env::var("USER").unwrap_or_default()
}

fn default_font_install_casks() -> Vec<FontCask> {
    fn c(label: &str, cask: &str, kind: &str) -> FontCask {
        FontCask {
            label: label.to_string(),
            cask: cask.to_string(),
            kind: kind.to_string(),
        }
    }
    vec![
        c("JetBrains Mono Nerd Font", "font-jetbrains-mono-nerd-font", "nerd"),
        c("Fira Code Nerd Font", "font-fira-code-nerd-font", "nerd"),
        c("Iosevka Term Nerd Font", "font-iosevka-term-nerd-font", "nerd"),
        c("IBM Plex Mono", "font-ibm-plex-mono", "mono"),
        c("Cascadia Code", "font-cascadia-code", "mono"),
        c("Inter", "font-inter", "sans"),
        c("Source Serif 4", "font-source-serif-4", "serif"),
    ]
}

// ---------------------------------------------------------------------------
// AppSettings
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontCask {
    pub label: String,
    pub cask: String,
    pub kind: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AppSettings {
    pub shared_window: bool,
    pub preload: bool,
    pub shared_width: f64,
    pub shared_height: f64,
    pub margin_top: f64,
    pub margin_top_builtin: f64,
    pub margin_bottom: f64,
    pub switcher_width: f64,
    pub palette_first: Vec<String>,
    pub shell: String,
    pub shell_args: Vec<String>,
    pub terminal_font: String,
    pub terminal_font_size: f64,
    pub font_install_casks: Vec<FontCask>,
    pub aerospace_cli: Vec<String>,
    pub app_dirs: Vec<String>,
    pub jira_icon_name: String,
    pub confluence_icon_name: String,
    pub ai_icon_name: String,
    pub app_icon_name: String,
    pub notes_icon_name: String,
    pub files_icon_name: String,
    pub notes_socket_name: String,
    pub focus_file_name: String,
    pub focus_bridge_name: String,
    pub switcher_window_name: String,
    pub detail_window_name: String,
    pub aerospace_socket_path: String,
    pub crash_log_path: String,
    pub aero_debug_flag: String,
    pub aero_log: String,
    pub voice_locale: String,
    pub screenshot_apps: Vec<String>,
    pub hide_on_focus_loss: bool,
    pub focus_loss_delay: f64,
    pub esc_close: i64,
    pub copy_toast: String,
    pub terminal_app: String,
    pub header_style: HeaderStyle,
    pub preview_border: Rgba,
    pub preview_border_width: f64,
    pub pane_focus_color: Rgba,
    pub pane_focus_width: f64,
    pub vim_keys: bool,
    pub vim_mode_badge: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        AppSettings {
            shared_window: true,
            preload: true,
            shared_width: 1100.0,
            shared_height: 640.0,
            margin_top: 0.0,
            margin_top_builtin: 0.0,
            margin_bottom: 0.0,
            switcher_width: 760.0,
            palette_first: vec![
                "filefast".to_string(),
                "paths".to_string(),
                "prettyprint".to_string(),
            ],
            shell: "/opt/homebrew/bin/bash".to_string(),
            shell_args: vec!["--login".to_string(), "-i".to_string()],
            terminal_font: "Hack Nerd Font".to_string(),
            terminal_font_size: 13.0,
            font_install_casks: default_font_install_casks(),
            aerospace_cli: vec![
                "/opt/homebrew/bin/aerospace".to_string(),
                "/usr/local/bin/aerospace".to_string(),
                "aerospace".to_string(),
            ],
            app_dirs: vec![
                "/Applications".to_string(),
                "/Applications/Utilities".to_string(),
                "/System/Applications".to_string(),
                "/System/Applications/Utilities".to_string(),
                "/System/Library/CoreServices".to_string(),
                format!("{}/Applications", home_dir()),
            ],
            jira_icon_name: "jira_icon.png".to_string(),
            confluence_icon_name: "confluence_icon.png".to_string(),
            ai_icon_name: String::new(),
            app_icon_name: "app_icon.png".to_string(),
            notes_icon_name: "notes_icon.png".to_string(),
            files_icon_name: String::new(),
            notes_socket_name: "ws-notes.sock".to_string(),
            focus_file_name: "kitchen-sink-focus".to_string(),
            focus_bridge_name: "ws-aerospace-focus".to_string(),
            switcher_window_name: "kitchen-sink".to_string(),
            detail_window_name: "jira-detail".to_string(),
            aerospace_socket_path: format!("/tmp/bobko.aerospace-{}.sock", user_name()),
            crash_log_path: expand_tilde("~/.cache/ws-crash.log"),
            aero_debug_flag: expand_tilde("~/.cache/aero-debug"),
            aero_log: expand_tilde("~/.cache/ws-aero.log"),
            voice_locale: "en-US".to_string(),
            screenshot_apps: vec![
                "org.flameshot".to_string(),
                "pl.maketheweb.cleanshotx".to_string(),
                "cc.ffitch.shottr".to_string(),
                "com.skitch.skitch".to_string(),
            ],
            hide_on_focus_loss: true,
            focus_loss_delay: 0.3,
            esc_close: 0,
            copy_toast: "Copied {} to clipboard".to_string(),
            terminal_app: String::new(),
            header_style: HeaderStyle::Flat,
            preview_border: Rgba::new(0.867, 0.882, 0.91, 1.0),
            preview_border_width: 2.0,
            pane_focus_color: Rgba::new(0xC8 as f64 / 255.0, 0xCE as f64 / 255.0, 0xD8 as f64 / 255.0, 0.55),
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
    let list = |k: &str| -> Vec<String> {
        csv(vars.get(k).map(String::as_str))
            .into_iter()
            .map(|s| if s.starts_with('~') { expand_tilde(&s) } else { s })
            .collect()
    };

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
    if let Some(v) = strv("terminal-font-size").and_then(|v| v.parse::<f64>().ok()) {
        if v >= 6.0 {
            settings.terminal_font_size = v;
        }
    }
    {
        let casks: Vec<FontCask> = csv(vars.get("font-install-casks").map(String::as_str))
            .into_iter()
            .filter_map(|entry| {
                let parts: Vec<String> =
                    entry.split('|').map(|p| p.trim().to_string()).collect();
                if parts.len() >= 2 && !parts[0].is_empty() && !parts[1].is_empty() {
                    Some(FontCask {
                        label: parts[0].clone(),
                        cask: parts[1].clone(),
                        kind: if parts.len() > 2 {
                            parts[2].to_lowercase()
                        } else {
                            "mono".to_string()
                        },
                    })
                } else {
                    None
                }
            })
            .collect();
        if !casks.is_empty() {
            settings.font_install_casks = casks;
        }
    }
    {
        let sa: Vec<String> = vars
            .get("shell-args")
            .map(String::as_str)
            .unwrap_or("")
            .split(|c| c == ' ' || c == '\t')
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        if !sa.is_empty() {
            settings.shell_args = sa;
        }
    }
    {
        let cli = list("aerospace-cli");
        if !cli.is_empty() {
            settings.aerospace_cli = cli;
        }
    }
    {
        let dirs = list("app-dirs");
        if !dirs.is_empty() {
            settings.app_dirs = dirs;
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
    if let Some(v) = strv("notes-icon") {
        if !v.is_empty() {
            settings.notes_icon_name = v;
        }
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
    if let Some(v) = strv("detail-name") {
        if !v.is_empty() {
            settings.detail_window_name = v;
        }
    }
    if let Some(v) = strv("aerospace-socket") {
        if !v.is_empty() {
            settings.aerospace_socket_path = v.replace("$USER", &user_name());
        }
    }
    if let Some(v) = strv("crash-log") {
        if !v.is_empty() {
            settings.crash_log_path = expand_tilde(&v);
        }
    }
    if let Some(v) = strv("debug-flag") {
        if !v.is_empty() {
            settings.aero_debug_flag = expand_tilde(&v);
        }
    }
    if let Some(v) = strv("aero-log") {
        if !v.is_empty() {
            settings.aero_log = expand_tilde(&v);
        }
    }
    if let Some(v) = strv("voice-locale") {
        if !v.is_empty() {
            settings.voice_locale = v;
        }
    }
    if vars.contains_key("screenshot-apps") {
        settings.screenshot_apps = csv(vars.get("screenshot-apps").map(String::as_str));
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
    if let Some(v) = tri_of(strv("preload").as_deref()) {
        settings.preload = v;
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
    if let Some(v) = strv("margin-top").and_then(|v| v.parse::<f64>().ok()) {
        settings.margin_top = v.max(0.0);
    }
    if let Some(v) = strv("switcher-width").and_then(|v| v.parse::<f64>().ok()) {
        settings.switcher_width = v.max(240.0).min(1200.0);
    }
    if let Some(v) = strv("palette-first") {
        settings.palette_first = v
            .split(',')
            .map(|p| p.trim().to_lowercase())
            .filter(|p| !p.is_empty())
            .collect();
    }
    if let Some(v) = strv("margin-top-builtin").and_then(|v| v.parse::<f64>().ok()) {
        settings.margin_top_builtin = v.max(0.0);
    }
    if let Some(v) = strv("margin-bottom").and_then(|v| v.parse::<f64>().ok()) {
        settings.margin_bottom = v.max(0.0);
    }
    if let Some(v) = strv("esc-close").and_then(|v| v.parse::<i64>().ok()) {
        settings.esc_close = v.max(0);
    }
    if vars.contains_key("copy-toast") {
        settings.copy_toast = strv("copy-toast").unwrap_or_default();
    }
    if let Some(v) = strv("terminal-app") {
        settings.terminal_app = v;
    }
    settings.preview_border = strv("preview-border")
        .and_then(|v| hex_color(&v))
        .unwrap_or_else(|| Rgba::new(0.867, 0.882, 0.91, 1.0));
    settings.preview_border_width = strv("preview-border-width")
        .and_then(|v| v.parse::<f64>().ok())
        .map(|n| n.max(0.0).min(8.0))
        .unwrap_or(2.0);
    settings.pane_focus_color = strv("pane-focus-color")
        .and_then(|v| hex_color(&v))
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
    Note,
    List,
    Output,
    Files,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ListColumn {
    pub field: String,
    pub title: String,
    pub width: f64,
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
    pub window_name: String,
    pub chrome_title: String,
    pub script: Option<String>,
    pub paths: Vec<String>,
    pub sources: Vec<String>,
    pub root: Option<String>,
    pub favorites: Vec<String>,
    pub recent: bool,
    pub recent_days: i64,
    pub recent_limit: i64,
    pub recent_exclude: Vec<String>,
    pub start_recent: bool,
    pub recent_everywhere: bool,
    pub browser_background: Option<Rgba>,
    pub background_color: Option<Rgba>,
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
    pub content_cap: i64,
    pub body_lines: i64,
    pub page_size: i64,
    pub copy_fields: Vec<String>,
    pub checkbox: Option<bool>,
    pub resize: bool,
    pub drag: bool,
    pub sticky: bool,
    pub float: Option<bool>,
    pub panel: bool,
    pub label: Option<String>,
    pub aliases: Vec<String>,
    pub in_palette: bool,
    pub tabs_opaque: Option<bool>,
    pub sort: Option<String>,
    pub sort_descending: Option<bool>,
    pub search_limit: Option<i64>,
    pub search_exclude: Option<Vec<String>>,
    pub terminal_words: Option<Vec<String>>,
    pub search_width: f64,
    pub max_stretch: f64,
    pub table_row_height: f64,
    pub height: f64,
    pub max_height: f64,
    pub font: Option<String>,
    pub font_size: f64,
    pub header_color: Option<Rgba>,
    pub voice: bool,
    pub voice_live: bool,
    pub terminal: bool,
    pub terminal_height: f64,
    pub inspector_width: f64,
    pub sidebar_width: f64,
    pub prose_font: Option<String>,
    pub prose_font_size: f64,
    pub prose_width: f64,
    pub new_note_name: String,
    pub new_doc_name: String,
    pub new_doc_template: String,
    pub terminal_dir: Option<String>,
    pub terminal_background: Option<Rgba>,
    pub text_color: Option<Rgba>,
    pub dim_color: Option<Rgba>,
    pub highlight_color: Option<Rgba>,
    pub accent_color: Option<Rgba>,
    pub palette: Option<Palette>,
    pub terminal_foreground: Option<Rgba>,
    pub vim_mode: bool,
    pub vim_bin: String,
    pub vim_init: Option<String>,
    pub start_drawer: String,
    pub esc_close: Option<i64>,
    pub image_rows: i64,
    pub icon: Option<String>,
    pub save_dir: String,
    pub table: bool,
    pub columns: Vec<ListColumn>,
    pub table_sort: Option<String>,
}

impl Default for CommandSpec {
    fn default() -> Self {
        CommandSpec {
            name: String::new(),
            kind: CommandKind::Shell,
            window_name: String::new(),
            chrome_title: String::new(),
            script: None,
            paths: Vec::new(),
            sources: Vec::new(),
            root: None,
            favorites: Vec::new(),
            recent: true,
            recent_days: 7,
            recent_limit: 200,
            recent_exclude: Vec::new(),
            start_recent: true,
            recent_everywhere: true,
            browser_background: None,
            background_color: None,
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
            content_cap: 0,
            body_lines: 0,
            page_size: 0,
            copy_fields: Vec::new(),
            checkbox: None,
            resize: false,
            drag: true,
            sticky: true,
            float: None,
            panel: false,
            label: None,
            aliases: Vec::new(),
            in_palette: true,
            tabs_opaque: None,
            sort: None,
            sort_descending: None,
            search_limit: None,
            search_exclude: None,
            terminal_words: None,
            search_width: 0.0,
            max_stretch: 0.0,
            table_row_height: 0.0,
            height: 0.0,
            max_height: 0.0,
            font: None,
            font_size: 0.0,
            header_color: None,
            voice: false,
            voice_live: true,
            terminal: false,
            terminal_height: 240.0,
            inspector_width: 340.0,
            sidebar_width: 210.0,
            prose_font: None,
            prose_font_size: 0.0,
            prose_width: 0.0,
            new_note_name: "Untitled".to_string(),
            new_doc_name: "doc".to_string(),
            new_doc_template: "markdown_doc_catppuccin_latte".to_string(),
            terminal_dir: None,
            terminal_background: None,
            text_color: None,
            dim_color: None,
            highlight_color: None,
            accent_color: None,
            palette: None,
            terminal_foreground: None,
            vim_mode: false,
            vim_bin: "nvim".to_string(),
            vim_init: None,
            start_drawer: "none".to_string(),
            esc_close: None,
            image_rows: 10,
            icon: None,
            save_dir: "/tmp/".to_string(),
            table: false,
            columns: Vec::new(),
            table_sort: None,
        }
    }
}

impl CommandSpec {
    pub fn new(name: impl Into<String>, kind: CommandKind, script: Option<String>) -> Self {
        let name = name.into();
        CommandSpec {
            window_name: name.clone(),
            chrome_title: name.clone(),
            name,
            kind,
            script,
            ..Default::default()
        }
    }

    /// `notesFolder` — the first path expanded; its parent when not a directory.
    pub fn notes_folder(&self) -> Option<String> {
        self.paths.first().map(|p| {
            let e = expand_tilde(p);
            if Path::new(&e).is_dir() {
                e
            } else {
                Path::new(&e)
                    .parent()
                    .map(|x| x.to_string_lossy().into_owned())
                    .unwrap_or(e)
            }
        })
    }
}

/// `makeCommand(_:_:)`.
pub fn make_command(name: &str, vars: &HashMap<String, String>) -> CommandSpec {
    let kind = match vars.get("type").map(String::as_str) {
        Some("note") => CommandKind::Note,
        Some("list") => CommandKind::List,
        Some("output") => CommandKind::Output,
        Some("files") => CommandKind::Files,
        _ => CommandKind::Shell,
    };
    let mut s = CommandSpec::new(name, kind, vars.get("script").cloned());
    if let Some(v) = vars.get("name") {
        s.window_name = v.clone();
    }
    s.chrome_title = vars
        .get("title")
        .cloned()
        .unwrap_or_else(|| s.window_name.clone());
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
    s.content_cap = int_of(vars.get("content-cap").map(String::as_str)).unwrap_or(0);
    s.body_lines = int_of(vars.get("body-lines").map(String::as_str)).unwrap_or(0);
    s.page_size = int_of(vars.get("page-size").map(String::as_str)).unwrap_or(0);
    s.copy_fields = csv(vars.get("copy-fields").map(String::as_str));
    s.checkbox = tri_of(vars.get("checkbox").map(String::as_str));
    s.search_width = num(vars.get("search-width").map(String::as_str));
    s.max_stretch = num(vars.get("max-row-stretch").map(String::as_str));
    s.table_row_height = num(vars.get("row-height").map(String::as_str));
    s.table = tri_of(vars.get("table").map(String::as_str)).unwrap_or(false);
    s.columns = ListColumn::parse(vars.get("columns").map(String::as_str));
    s.table_sort = vars.get("table-sort").cloned();
    s.width = num(vars.get("width").map(String::as_str));
    s.height = num(vars.get("height").map(String::as_str));
    s.max_height = num(vars.get("max-height").map(String::as_str));
    s.resize = tri_of(vars.get("resize").map(String::as_str)).unwrap_or(false);
    s.drag = tri_of(vars.get("drag").map(String::as_str)).unwrap_or(true);
    s.sticky = tri_of(vars.get("sticky").map(String::as_str)).unwrap_or(true);
    s.float = tri_of(vars.get("float").map(String::as_str));
    s.esc_close = vars
        .get("esc-close")
        .or_else(|| vars.get("vim-esc-close"))
        .and_then(|v| v.parse::<i64>().ok());
    s.font = vars.get("font").cloned();
    s.font_size = num(vars.get("font-size").map(String::as_str));
    s.header_color = vars.get("header-color").and_then(|v| hex_color(v));
    s.browser_background = vars.get("browser-background").and_then(|v| hex_color(v));
    s.background_color = vars.get("background-color").and_then(|v| hex_color(v));
    {
        let t = num(vars.get("tint-alpha").map(String::as_str));
        if t > 0.0 {
            s.tint_alpha = Some(t.min(1.0));
        }
    }
    s.text_color = vars.get("text-color").and_then(|v| hex_color(v));
    s.dim_color = vars.get("dim-color").and_then(|v| hex_color(v));
    s.highlight_color = vars.get("highlight-color").and_then(|v| hex_color(v));
    s.accent_color = vars.get("accent-color").and_then(|v| hex_color(v));
    s.palette = parse_palette(vars.get("palette").map(String::as_str));
    s.tabs_opaque = tri_of(vars.get("tabs-opaque").map(String::as_str));
    s.voice = tri_of(vars.get("voice").map(String::as_str)).unwrap_or(false);
    s.voice_live = tri_of(vars.get("voice-live").map(String::as_str)).unwrap_or(true);
    s.terminal = tri_of(vars.get("terminal").map(String::as_str)).unwrap_or(false);
    {
        let t = num(vars.get("terminal-height").map(String::as_str));
        if t > 0.0 {
            s.terminal_height = t;
        }
    }
    if vars.contains_key("sidebar-width") {
        s.sidebar_width = num(vars.get("sidebar-width").map(String::as_str));
    }
    if vars.contains_key("inspector-width") {
        s.inspector_width = num(vars.get("inspector-width").map(String::as_str));
    }
    s.prose_font = vars
        .get("prose-font")
        .and_then(|v| if v.is_empty() { None } else { Some(v.clone()) });
    s.prose_font_size = num(vars.get("prose-font-size").map(String::as_str));
    s.prose_width = num(vars.get("prose-width").map(String::as_str));
    if let Some(n) = vars.get("new-note-name").map(|v| v.trim().to_string()) {
        if !n.is_empty() {
            s.new_note_name = n;
        }
    }
    if let Some(n) = vars.get("new-doc-name").map(|v| v.trim().to_string()) {
        if !n.is_empty() {
            s.new_doc_name = n;
        }
    }
    if let Some(n) = vars.get("new-doc-template").map(|v| v.trim().to_string()) {
        if !n.is_empty() {
            s.new_doc_template = n;
        }
    }
    s.terminal_dir = vars.get("terminal-dir").cloned();
    s.terminal_background = vars.get("terminal-background").and_then(|v| hex_color(v));
    s.terminal_foreground = vars.get("terminal-foreground").and_then(|v| hex_color(v));
    s.vim_mode = tri_of(vars.get("vim-mode").map(String::as_str)).unwrap_or(false);
    if let Some(v) = vars.get("vim-bin") {
        s.vim_bin = v.trim().to_string();
    }
    if let Some(v) = vars.get("vim-init") {
        if !v.is_empty() {
            s.vim_init = Some(v.clone());
        }
    }
    if let Some(v) = vars.get("start-drawer") {
        if !v.is_empty() {
            s.start_drawer = v.to_lowercase();
        }
    }
    s.image_rows = int_of(vars.get("image-rows").map(String::as_str)).unwrap_or(10);
    s.sort = vars.get("sort").cloned();
    s.sort_descending = vars
        .get("sort-order")
        .map(|o| o.to_lowercase().starts_with("desc"));
    s.search_limit = int_of(vars.get("search-limit").map(String::as_str));
    if vars.contains_key("search-exclude") {
        s.search_exclude = Some(csv(vars.get("search-exclude").map(String::as_str)));
    }
    if vars.contains_key("terminal-words") {
        s.terminal_words = Some(csv(vars.get("terminal-words").map(String::as_str)));
    }
    s.recent = tri_of(vars.get("recent").map(String::as_str)).unwrap_or(true);
    s.recent_days = int_of(vars.get("recent-days").map(String::as_str)).unwrap_or(7);
    s.recent_limit = int_of(vars.get("recent-limit").map(String::as_str)).unwrap_or(200);
    s.recent_exclude = csv(vars.get("recent-exclude").map(String::as_str));
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
    "enabled", "resize", "drag", "sticky", "voice", "voice-live", "terminal", "vim-mode",
    "recent", "checkbox", "hide-on-focus-loss", "float", "table", "shared-window", "preload",
    "in-palette", "panel", "vim-keys", "vim-mode-badge", "show-help", "show-side-panel-button",
    "show-size-badge", "magnifier", "square-magnifier", "copy-on-double-click", "save-path-fixed",
    "save-after-copy", "copy-path-after-save", "save-last-region", "reverse-arrow",
    "counter-outline", "insecure-pixelate",
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
    ("width", 100.0, 8000.0),
    ("height", 60.0, 8000.0),
    ("max-height", 60.0, 8000.0),
    ("shared-width", 400.0, 8000.0),
    ("shared-height", 300.0, 8000.0),
    ("margin-top", 0.0, 400.0),
    ("margin-top-builtin", 0.0, 400.0),
    ("margin-bottom", 0.0, 400.0),
    ("switcher-width", 240.0, 1200.0),
    ("preview-border-width", 0.0, 8.0),
    ("pane-focus-width", 0.0, 4.0),
    ("terminal-height", 40.0, 4000.0),
    ("sidebar-width", 0.0, 600.0),
    ("inspector-width", 0.0, 800.0),
    ("prose-font-size", 8.0, 48.0),
    ("prose-width", 300.0, 2000.0),
    ("font-size", 6.0, 96.0),
    ("terminal-font-size", 6.0, 96.0),
    ("max-rows", 0.0, 10_000.0),
    ("page-size", 0.0, 100_000.0),
    ("content-cap", 0.0, 100_000.0),
    ("body-lines", 0.0, 100.0),
    ("search-width", 0.0, 1.0),
    ("recent-days", 1.0, 365.0),
    ("recent-limit", 20.0, 5000.0),
    ("tint-alpha", 0.0, 1.0),
    ("max-row-stretch", 0.0, 1000.0),
    ("row-height", 18.0, 80.0),
    ("image-rows", 1.0, 200.0),
    ("vim-esc-close", 0.0, 20.0),
    ("esc-close", 0.0, 20.0),
    ("search-limit", 1.0, 1_000_000.0),
    ("dashboard-width", 600.0, 8000.0),
    ("dashboard-height", 400.0, 8000.0),
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
        "type" => vec!["shell", "note", "list", "output", "files"],
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
    if section == "compare" {
        match key {
            "content" => {
                return if ["auto", "always", "never"].contains(&value.to_lowercase().as_str()) {
                    None
                } else {
                    Some(format!("'{value}' is not one of auto | always | never"))
                };
            }
            "gutter-arrows" => {
                return if ["hover", "always", "off"].contains(&value.to_lowercase().as_str()) {
                    None
                } else {
                    Some(format!("'{value}' is not one of hover | always | off"))
                };
            }
            "recent" => {
                return match value.parse::<i64>() {
                    Ok(n) if (0..=500).contains(&n) => None,
                    _ => Some(format!("{value}: 0…500 (recent pairs kept)")),
                };
            }
            "ignore-leading-ws" | "ignore-trailing-ws" | "ignore-embedded-ws" | "ignore-case"
            | "ignore-line-endings" | "ignore-blank-lines" | "use-gitignore" => {
                return if config_text::tri(Some(value)).is_none() {
                    Some(format!("'{value}' is not true/false"))
                } else {
                    None
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
    if key == "palette" && parse_palette(Some(value)).is_none() {
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
pub fn validate_config(text: &str) -> Vec<ConfigIssue> {
    validate_config_lines(&config_text::config_decoded_lines(text))
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
    std::fs::read_to_string(path).ok()
}

pub fn read_config_text() -> Option<String> {
    read_config_text_at(&commands_conf_path())
}

pub fn write_config_text_at(path: &Path, text: &str) -> bool {
    if validate_config(text).iter().any(|i| i.fatal) {
        return false;
    }
    if let Some(cur) = read_config_text_at(path) {
        if cur != text {
            let backup = PathBuf::from(format!("{}.bak", path.display()));
            let _ = std::fs::write(&backup, &cur);
        }
    }
    std::fs::write(path, text).is_ok()
}

pub fn write_config_text(text: &str) -> bool {
    write_config_text_at(&commands_conf_path(), text)
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

pub fn save_config_values(section: &str, kv: &[(String, Option<String>)]) {
    let _ = save_config_values_at(&commands_conf_path(), section, kv);
}

pub fn save_config_value(section: &str, key: &str, value: &str) {
    save_config_values(section, &[(key.to_string(), Some(value.to_string()))]);
}

pub fn remove_config_value(section: &str, key: &str) {
    save_config_values(section, &[(key.to_string(), None)]);
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
    fn defaults_match_swift() {
        let s = AppSettings::default();
        assert!(s.shared_window && s.preload);
        assert_eq!(s.shared_width, 1100.0);
        assert_eq!(s.shared_height, 640.0);
        assert_eq!(s.switcher_width, 760.0);
        assert_eq!(s.shell, "/opt/homebrew/bin/bash");
        assert_eq!(s.shell_args, vec!["--login", "-i"]);
        assert_eq!(s.terminal_font, "Hack Nerd Font");
        assert_eq!(s.terminal_font_size, 13.0);
        assert_eq!(s.font_install_casks.len(), 7);
        assert_eq!(s.aerospace_cli.len(), 3);
        assert_eq!(s.app_dirs.len(), 6);
        assert_eq!(s.jira_icon_name, "jira_icon.png");
        assert_eq!(s.confluence_icon_name, "confluence_icon.png");
        assert_eq!(s.ai_icon_name, "");
        assert_eq!(s.app_icon_name, "app_icon.png");
        assert_eq!(s.notes_icon_name, "notes_icon.png");
        assert_eq!(s.files_icon_name, "");
        assert_eq!(s.notes_socket_name, "ws-notes.sock");
        assert_eq!(s.focus_file_name, "kitchen-sink-focus");
        assert_eq!(s.focus_bridge_name, "ws-aerospace-focus");
        assert_eq!(s.switcher_window_name, "kitchen-sink");
        assert_eq!(s.detail_window_name, "jira-detail");
        assert!(s.aerospace_socket_path.starts_with("/tmp/bobko.aerospace-"));
        assert_eq!(s.voice_locale, "en-US");
        assert!(s.hide_on_focus_loss);
        assert_eq!(s.focus_loss_delay, 0.3);
        assert_eq!(s.esc_close, 0);
        assert_eq!(s.copy_toast, "Copied {} to clipboard");
        assert_eq!(s.terminal_app, "");
        assert_eq!(s.header_style, HeaderStyle::Flat);
        assert_eq!(s.preview_border_width, 2.0);
        assert_eq!(s.pane_focus_width, 1.0);
        assert!(s.vim_keys && s.vim_mode_badge);
        assert_eq!(s.preview_border, Rgba::new(0.867, 0.882, 0.91, 1.0));
    }

    #[test]
    fn parse_app_block_fields_and_defaults() {
        let mut s = AppSettings::default();
        let v = vars(&[
            ("shell", "/bin/zsh"),
            ("terminal-font", "Fira Code"),
            ("terminal-font-size", "15"),
            ("shell-args", "--login  -i"),
            ("aerospace-cli", "/opt/homebrew/bin/aerospace, aerospace"),
            ("app-dirs", "/Applications, ~/Apps"),
            ("notes-icon", "n.png"),
            ("ai-icon", "ai.png"),
            ("switcher-width", "200"),   // clamped up to 240
            ("shared-width", "300"),     // below 400 -> ignored
            ("margin-top", "-5"),        // clamped to 0
            ("palette-first", "Paths, filefast"),
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
        assert_eq!(s.terminal_font_size, 15.0);
        assert_eq!(s.shell_args, vec!["--login", "-i"]);
        assert_eq!(s.aerospace_cli.len(), 2);
        assert_eq!(s.app_dirs[1], expand_tilde("~/Apps"));
        assert_eq!(s.notes_icon_name, "n.png");
        assert_eq!(s.ai_icon_name, "ai.png");
        assert_eq!(s.switcher_width, 240.0);
        assert_eq!(s.shared_width, 1100.0);
        assert_eq!(s.margin_top, 0.0);
        assert_eq!(s.palette_first, vec!["paths", "filefast"]);
        assert_eq!(s.esc_close, 2);
        assert_eq!(s.copy_toast, "Copied!");
        assert_eq!(s.header_style, HeaderStyle::Aurora);
        assert_eq!(s.preview_border, hex_color("#112233").unwrap());
        assert_eq!(s.preview_border_width, 8.0);
        assert_eq!(s.pane_focus_color, hex_color("#445566").unwrap());
        assert_eq!(s.pane_focus_width, 4.0);

        // `terminal-font-size` below 6 is ignored; absent keys keep defaults.
        let mut s2 = AppSettings::default();
        parse_app_config(&mut s2, &vars(&[("terminal-font-size", "3")]));
        assert_eq!(s2.terminal_font_size, 13.0);
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
                ("preload", "off"),
                ("vim-keys", "0"),
                ("vim-mode-badge", "false"),
            ]),
        );
        assert!(!s.hide_on_focus_loss);
        assert!(!s.shared_window);
        assert!(!s.preload);
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
            ("content-cap", "10"),
            ("width", "500"),
            ("height", "200"),
            ("resize", "true"),
            ("drag", "false"),
            ("sticky", "false"),
            ("esc-close", "1"),
            ("vim-bin", " nvim "),
            ("start-drawer", "Terminal"),
            ("image-rows", "42"),
            ("recent-days", "30"),
            ("recent-limit", "500"),
            ("start", "root"),
            ("recent-scope", "home"),
            ("terminal-height", "0"),
            ("sidebar-width", "300"),
            ("font-size", "14"),
            ("header-color", "#abcdef"),
            ("new-note-name", "  Untitled 2 "),
        ]);
        let s = make_command("mylist", &v);
        assert_eq!(s.name, "mylist");
        assert_eq!(s.kind, CommandKind::List);
        assert_eq!(s.script.as_deref(), Some("echo hi"));
        assert_eq!(s.window_name, "My List");
        assert_eq!(s.chrome_title, "Titled");
        assert_eq!(s.label.as_deref(), Some("Labelled"));
        assert_eq!(s.paths, vec!["/tmp/a", "/tmp/b"]);
        assert_eq!(s.sources, vec!["a.json", "b.json"]);
        assert_eq!(s.max_rows, 50);
        assert_eq!(s.page_size, 25);
        assert_eq!(s.content_cap, 10);
        assert_eq!(s.width, 500.0);
        assert_eq!(s.height, 200.0);
        assert_eq!(s.esc_close, Some(1));
        assert_eq!(s.vim_bin, "nvim"); // trimmed
        assert_eq!(s.start_drawer, "terminal"); // lowercased
        assert_eq!(s.image_rows, 42);
        assert_eq!(s.recent_days, 30);
        assert_eq!(s.recent_limit, 500);
        assert!(!s.start_recent);
        assert!(!s.recent_everywhere);
        assert_eq!(s.terminal_height, 240.0); // 0 ignored
        assert_eq!(s.sidebar_width, 300.0);
        assert_eq!(s.font_size, 14.0);
        assert_eq!(s.header_color, hex_color("#abcdef"));
        assert_eq!(s.new_note_name, "Untitled 2");
        assert_eq!(s.save_dir, "/tmp/");

        // Defaults: no `type` -> shell; resize false, drag/sticky true.
        let d = make_command("plain", &vars(&[]));
        assert_eq!(d.kind, CommandKind::Shell);
        assert!(!d.resize && d.drag && d.sticky);
        assert!(d.in_palette);
        assert!(d.recent && d.voice_live);
        assert_eq!(d.new_doc_name, "doc");
        assert_eq!(d.new_doc_template, "markdown_doc_catppuccin_latte");
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
            ("voice", "true"),
            ("voice-live", "false"),
            ("checkbox", "on"),
            ("float", "true"),
            ("esc-close", "3"),
            ("vim-esc-close", "5"),
        ]);
        let s = make_command("x", &v);
        assert!(!s.in_palette);
        assert!(!s.drag);
        assert!(s.resize);
        assert!(s.voice);
        assert!(!s.voice_live);
        assert_eq!(s.checkbox, Some(true));
        assert_eq!(s.float, Some(true));
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
        // palette
        assert_eq!(
            config_value_problem("app", "palette", "#111111").as_deref(),
            Some("expected 5 hex colors: accent2, success, warning, danger, info")
        );
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
        // compare
        assert_eq!(
            config_value_problem("compare", "content", "sometimes").as_deref(),
            Some("'sometimes' is not one of auto | always | never")
        );
        assert_eq!(config_value_problem("compare", "content", "always"), None);
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
        assert_eq!(config_value_problem("compare", "use-gitignore", "zzz").as_deref(), Some("'zzz' is not true/false"));
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
