//! Jira view family — mirrors `JiraDashboard.swift`, `JiraSearch.swift`,
//! `JiraBoard.swift` and `JiraTicket.swift`.
//!
//! Bounded first cut: the pure DATA + STATE models (the same logic that lives
//! in `pylib/jira_fields.py`, `jira_data.py`, `jira_search.py`,
//! `jira_dashboard.py`, `jira_boards.py` and `jira_pages.py`), the `--describe`
//! payload parser, the CLI/helper IPC seams, and the controller. The AppKit
//! NSView tree is built from the ported models by
//! the live surfaces in [`crate::views::jira_view`] (the old
//! standalone tree was retired with the shared window). 

use crate::app::config::ListColumn;
use crate::app::python_helper::PythonHelper;
use crate::app::registry::Registry;
use crate::ui::card::CardConfig;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use crate::ui::theme::{PopupColors, PopupTone, Rgba};

// ===========================================================================
// Small JSON helpers
// ===========================================================================

fn get_str<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

fn get_bool(v: &Value, k: &str) -> bool {
    v.get(k).map(truthy).unwrap_or(false)
}

use crate::app::config::truthy;

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn str_field(v: &Value, k: &str) -> String {
    get_str(v, k).to_string()
}

/// Python `_scalar_str`: strings pass, numbers stringify, everything else
/// serialises as JSON.
fn scalar_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        Value::Bool(b) => (if *b { "1" } else { "0" }).to_string(),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.to_string()
            } else if let Some(f) = n.as_f64() {
                format!("{f}")
            } else {
                n.to_string()
            }
        }
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

// ===========================================================================
// jira_fields.py — column codec + field labels
// ===========================================================================

/// The shipped `jira/defaults.json` — the SAME file `pylib/jira_search.py`,
/// `pylib/jira_fields.py` and `jira/jira_config.py` read (base_field_labels,
/// search_ui.kinds / skip_fields, board_list_columns, ...). Read once; no
/// table is hand-mirrored in Rust. `asset_dir` carries `jira/` in both the
/// repo build and the dist bundle (install.conf RESOURCE_LINK_DIRS); the
/// manifest dir covers `cargo test`.
///
/// FRESHNESS REGIME: the Python side (jsonmgr) re-reads on mtime change;
/// this OnceLock freezes the values at first use until the daemon restarts
/// (`./ws build` or `jira-poll restart`). Editing defaults.json and
/// expecting the running app to follow is NOT supported — restart. A
/// mid-session reload would change board lanes / counts under the user.
fn jira_defaults() -> &'static Value {
    static DEFAULTS: OnceLock<Value> = OnceLock::new();
    DEFAULTS.get_or_init(|| {
        let asset = crate::app::paths::Paths::from_env().asset_dir().to_string();
        let candidates = [
            format!("{asset}/jira/defaults.json"),
            format!("{}/../../jira/defaults.json", env!("CARGO_MANIFEST_DIR")),
        ];
        for path in candidates {
            if let Ok(text) = std::fs::read_to_string(&path) {
                if let Ok(v) = serde_json::from_str::<Value>(&text) {
                    return v;
                }
            }
        }
        Value::Null
    })
}

/// `base_field_labels` (field -> built-in display name), read once.
fn base_field_label_map() -> &'static HashMap<String, String> {
    static MAP: OnceLock<HashMap<String, String>> = OnceLock::new();
    MAP.get_or_init(|| {
        jira_defaults()
            .get("base_field_labels")
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default()
    })
}

pub fn base_field_label(field: &str) -> Option<&'static str> {
    base_field_label_map().get(field).map(String::as_str)
}

/// A path into `jira/defaults.json` (`None` when absent).
fn defaults_at(path: &[&str]) -> Option<&'static Value> {
    let mut cur = jira_defaults();
    for key in path {
        cur = cur.get(*key)?;
    }
    Some(cur)
}

/// A string array at `path` (empty when absent).
fn defaults_strs(path: &[&str]) -> Vec<String> {
    defaults_at(path)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default()
}

/// A string at `path` (`""` when absent).
fn defaults_str(path: &[&str]) -> String {
    defaults_at(path).and_then(Value::as_str).unwrap_or("").to_string()
}

/// A usize at `path` (0 when absent).
fn defaults_usize(path: &[&str]) -> usize {
    defaults_at(path).and_then(Value::as_u64).map(|n| n as usize).unwrap_or(0)
}

/// The "All boards" pin's columns (`board_list_columns` in `jira/defaults.json`).
pub fn board_list_columns() -> Value {
    jira_defaults()
        .get("board_list_columns")
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()))
}

/// The "Labels" browse list's columns (`label_list_columns` in the file).
pub fn label_list_columns() -> Value {
    jira_defaults()
        .get("label_list_columns")
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()))
}

/// The open board's columns-mode table (`board_table_columns` in the file).
pub fn board_table_columns() -> Value {
    jira_defaults()
        .get("board_table_columns")
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()))
}

pub use crate::app::config::{column_from_obj, parse_columns};

/// A `columns` config value: the structured array, or a legacy / JSON string.
pub fn parse_columns_value(v: &Value) -> Vec<ListColumn> {
    match v {
        Value::Array(a) => a.iter().filter_map(column_from_obj).collect(),
        Value::String(s) => parse_columns(s),
        _ => Vec::new(),
    }
}

/// The starter columns (`columns` in `jira/defaults.json`) as a JSON array.
pub fn default_columns() -> Value {
    jira_defaults()
        .get("columns")
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()))
}

/// The structured JSON text of the columns — the `--set-columns` transport and
/// the helper reply (config.json stores the array itself). Defaults are
/// omitted: width 0 = auto, align left, no flags; `titles=false` drops the
/// title (the field label supplies it).
pub fn serialize_columns(cols: &[ListColumn], titles: bool) -> String {
    let arr: Vec<Value> = cols
        .iter()
        .map(|c| {
            let mut o = Map::new();
            o.insert("field".into(), Value::String(c.field.clone()));
            if titles && !c.title.is_empty() && c.title != c.field {
                o.insert("title".into(), Value::String(c.title.clone()));
            }
            if c.width > 0.0 {
                let n = if c.width.fract() == 0.0 {
                    serde_json::Number::from(c.width as i64)
                } else {
                    serde_json::Number::from_f64(c.width).unwrap_or_else(|| serde_json::Number::from(0))
                };
                o.insert("width".into(), Value::Number(n));
            }
            if let Some(px) = c.width_px {
                let n = if px.fract() == 0.0 {
                    serde_json::Number::from(px as i64)
                } else {
                    serde_json::Number::from_f64(px).unwrap_or_else(|| serde_json::Number::from(0))
                };
                o.insert("width_px".into(), Value::Number(n));
            }
            if c.align != "left" {
                o.insert("align".into(), Value::String(c.align.clone()));
            }
            if c.filterable {
                o.insert("filter".into(), Value::Bool(true));
            }
            if c.sortable {
                o.insert("sort".into(), Value::Bool(true));
            }
            Value::Object(o)
        })
        .collect();
    serde_json::to_string(&Value::Array(arr)).unwrap_or_default()
}

// ===========================================================================
// jira_data.py — words, categories, workflow, style rules, comments
// ===========================================================================

/// `words_keys` (category -> the `[jira]` config key), read once.
fn word_keys() -> &'static Vec<(String, String)> {
    static KEYS: OnceLock<Vec<(String, String)>> = OnceLock::new();
    KEYS.get_or_init(|| {
        defaults_at(&["words_keys"])
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()))
                    .collect()
            })
            .unwrap_or_default()
    })
}

/// The default list for one `words` key (from `jira/defaults.json`).
fn word_default(key: &str) -> String {
    defaults_at(&["words"])
        .and_then(|w| w.get(key))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// `dim_fields`, read once.
fn dim_fields() -> &'static [String] {
    static FIELDS: OnceLock<Vec<String>> = OnceLock::new();
    FIELDS.get_or_init(|| defaults_strs(&["dim_fields"])).as_slice()
}

/// `[jira]` config values -> the seven lowercased word lists (an explicit
/// non-empty value replaces only that list; an empty value falls back).
pub fn words(values: &Value) -> BTreeMap<String, Vec<String>> {
    let mut out = BTreeMap::new();
    for (name, key) in word_keys() {
        let fallback = word_default(key);
        let raw = values
            .get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or(&fallback);
        let list = raw
            .split(',')
            .map(|p| p.trim().to_lowercase())
            .filter(|p| !p.is_empty())
            .collect();
        out.insert(name.clone(), list);
    }
    out
}

pub fn word_matches(lowered: &str, list: &[String]) -> bool {
    list.iter().any(|w| lowered.contains(w.as_str()))
}

/// 0 To Do / 1 In Progress / 2 Done. The directory's statusCategories win;
/// the word lists classify anything it does not know.
pub fn category(
    status: &str,
    categories: &Map<String, Value>,
    w: &BTreeMap<String, Vec<String>>,
) -> i32 {
    if let Some(cat) = categories.get(status).and_then(Value::as_str) {
        match cat {
            "new" => return 0,
            "indeterminate" => return 1,
            "done" => return 2,
            _ => {}
        }
    }
    let lowered = status.to_lowercase();
    let empty: Vec<String> = Vec::new();
    let done = w.get("done").unwrap_or(&empty);
    let cancelled = w.get("cancelled").unwrap_or(&empty);
    if word_matches(&lowered, done) || word_matches(&lowered, cancelled) {
        return 2;
    }
    if word_matches(&lowered, w.get("new").unwrap_or(&empty)) {
        return 0;
    }
    1
}

#[derive(Clone, Debug, PartialEq, Default, Serialize)]
pub struct CellStyle {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mark: Option<String>,
    #[serde(rename = "quietsRow", skip_serializing_if = "Option::is_none")]
    pub quiets_row: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tinted: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bold: Option<bool>,
}

impl CellStyle {
    fn tone(tone: &str) -> Self {
        CellStyle {
            tone: Some(tone.to_string()),
            ..Default::default()
        }
    }
    fn tone_mark(tone: &str, mark: &str) -> Self {
        CellStyle {
            tone: Some(tone.to_string()),
            mark: Some(mark.to_string()),
            ..Default::default()
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StatusRule {
    pub words: Vec<String>,
    pub style: CellStyle,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ReleaseRule {
    pub contains: String,
    pub style: CellStyle,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StyleRules {
    pub words: BTreeMap<String, Vec<String>>,
    #[serde(rename = "dimFields")]
    pub dim_fields: Vec<String>,
    pub status: Vec<StatusRule>,
    #[serde(rename = "statusFallback")]
    pub status_fallback: CellStyle,
    pub priority: Vec<StatusRule>,
    #[serde(rename = "priorityFallback")]
    pub priority_fallback: CellStyle,
    #[serde(rename = "releaseStatus")]
    pub release_status: Vec<ReleaseRule>,
}

/// The evaluator table the app caches: word lists + ordered rules (first
/// match wins).
pub fn style_rules(values: &Value) -> StyleRules {
    let w = words(values);
    let list = |name: &str| w.get(name).cloned().unwrap_or_default();
    StyleRules {
        words: w.clone(),
        dim_fields: dim_fields().to_vec(),
        status: vec![
            StatusRule {
                words: list("cancelled"),
                style: CellStyle {
                    tone: Some("dim".into()),
                    mark: Some("hollow".into()),
                    quiets_row: Some(true),
                    ..Default::default()
                },
            },
            StatusRule {
                words: list("done"),
                style: CellStyle {
                    tone: Some("success".into()),
                    mark: Some("filled".into()),
                    quiets_row: Some(true),
                    ..Default::default()
                },
            },
            StatusRule {
                words: list("blocked"),
                style: CellStyle::tone_mark("danger", "filled"),
            },
            StatusRule {
                words: list("active"),
                style: CellStyle::tone_mark("info", "half"),
            },
            StatusRule {
                words: list("waiting"),
                style: CellStyle::tone_mark("warning", "hollow"),
            },
        ],
        status_fallback: CellStyle::tone_mark("dim", "hollow"),
        priority: vec![StatusRule {
            words: list("urgent"),
            style: CellStyle {
                tone: Some("danger".into()),
                tinted: Some(true),
                bold: Some(true),
                ..Default::default()
            },
        }],
        priority_fallback: CellStyle::tone("dim"),
        release_status: vec![
            ReleaseRule {
                contains: "unreleased".into(),
                style: CellStyle::tone_mark("warning", "hollow"),
            },
            ReleaseRule {
                contains: "released".into(),
                style: CellStyle::tone_mark("success", "filled"),
            },
        ],
    }
}

// ------------------------------------------------------- ticket comments

// ===========================================================================
// jira_search.py — filter menu model + criteria assembly
// ===========================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueKind {
    Users,
    List,
    Date,
    Text,
}

impl Default for ValueKind {
    fn default() -> Self {
        ValueKind::Text
    }
}

impl ValueKind {
    pub fn from_str(s: &str) -> ValueKind {
        match s {
            "users" => ValueKind::Users,
            "list" => ValueKind::List,
            "date" => ValueKind::Date,
            _ => ValueKind::Text,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FilterKind {
    pub key: String,
    pub title: String,
    pub kind: ValueKind,
}

fn label_for(catalog: &[Value], field: &str, fallback: &str) -> String {
    for c in catalog {
        if c.get("field").and_then(Value::as_str) == Some(field) {
            if let Some(label) = c.get("label").and_then(Value::as_str) {
                if !label.is_empty() {
                    return label.to_string();
                }
            }
        }
    }
    fallback.to_string()
}

/// The base kinds plus the describe catalog with its skip list — both read
/// from `jira/defaults.json` (`search_ui.kinds` / `search_ui.skip_fields`).
pub fn filter_kinds(catalog: &[Value]) -> Vec<FilterKind> {
    let ui = jira_defaults().get("search_ui");
    let skip: Vec<&str> = ui
        .and_then(|u| u.get("skip_fields"))
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let mut kinds = Vec::new();
    let base = ui.and_then(|u| u.get("kinds")).and_then(Value::as_array);
    for row in base.into_iter().flatten() {
        let key = get_str(row, "key");
        if key.is_empty() {
            continue;
        }
        let mut title = match row.get("labelField").and_then(Value::as_str) {
            Some(field) => label_for(catalog, field, get_str(row, "title")),
            None => get_str(row, "title").to_string(),
        };
        title.push_str(get_str(row, "suffix"));
        kinds.push(FilterKind {
            key: key.to_string(),
            title,
            kind: ValueKind::from_str(get_str(row, "kind")),
        });
    }
    for c in catalog {
        let Some(field) = c.get("field").and_then(Value::as_str) else {
            continue;
        };
        if skip.contains(&field) {
            continue;
        }
        let label = c
            .get("label")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or(field);
        kinds.push(FilterKind {
            key: format!("field:{field}"),
            title: format!("{label} contains"),
            kind: ValueKind::Text,
        });
    }
    kinds
}

/// UI state -> the criteria JSON `jira_poll.py --live-search` consumes.
pub fn criteria(params: &Value) -> Value {
    let mut c = Map::new();
    let text = get_str(params, "text").trim();
    if !text.is_empty() {
        c.insert("text".into(), Value::String(text.to_string()));
    }
    let projects: Vec<String> = params
        .get("projects")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default();
    if !projects.is_empty() && !get_bool(params, "projectsAll") {
        c.insert("projects".into(), json!(projects));
    }
    let mut fields = Map::new();
    if let Some(rows) = params.get("rows").and_then(Value::as_array) {
        for r in rows {
            let Some(key) = r.get("key").and_then(Value::as_str) else {
                continue;
            };
            if key.is_empty() {
                continue;
            }
            if let Some(selected) = r.get("selected").and_then(Value::as_array) {
                if !selected.is_empty() {
                    c.insert(
                        key.to_string(),
                        json!(selected.iter().filter_map(Value::as_str).collect::<Vec<_>>()),
                    );
                    continue;
                }
            }
            if let Some(val) = r.get("value") {
                let s = val.as_str().unwrap_or("").to_string();
                c.insert(key.to_string(), Value::String(s));
                continue;
            }
            let text_v = get_str(r, "text").trim();
            if !text_v.is_empty() {
                if let Some(field) = key.strip_prefix("field:") {
                    fields.insert(field.to_string(), Value::String(text_v.to_string()));
                }
            }
        }
    }
    if !fields.is_empty() {
        c.insert("fields".into(), Value::Object(fields));
    }
    let mut m: i64 = 0;
    match params.get("maxResults") {
        Some(Value::Number(n)) => m = n.as_i64().unwrap_or(0),
        Some(Value::String(s)) if !s.is_empty() && s == s.trim() => {
            m = s.parse::<i64>().unwrap_or(0)
        }
        _ => {}
    }
    if m > 0 {
        c.insert("maxResults".into(), json!(m));
    }
    Value::Object(c)
}

/// One live-search filter row (`JiraSearchPanel.Row`).
#[derive(Clone, Debug, PartialEq)]
pub struct SearchRow {
    pub key: String,
    pub kind: ValueKind,
    pub selected: Vec<String>,
    /// `Some` = a choice row (even `Some("")` counts as present).
    pub value: Option<String>,
    pub text: String,
}

impl Default for SearchRow {
    fn default() -> Self {
        SearchRow {
            key: String::new(),
            kind: ValueKind::Text,
            selected: Vec::new(),
            value: None,
            text: String::new(),
        }
    }
}

impl SearchRow {
    pub fn new(key: impl Into<String>, kind: ValueKind) -> Self {
        SearchRow {
            key: key.into(),
            kind,
            ..Default::default()
        }
    }
    fn to_json(&self) -> Value {
        let mut o = Map::new();
        o.insert("key".into(), json!(self.key));
        o.insert("selected".into(), json!(self.selected));
        if let Some(v) = &self.value {
            o.insert("value".into(), json!(v));
        }
        o.insert("text".into(), json!(self.text));
        Value::Object(o)
    }
    fn from_json(v: &Value) -> Self {
        SearchRow {
            key: str_field(v, "key"),
            kind: ValueKind::Text,
            selected: strings(v.get("selected").unwrap_or(&Value::Null)),
            value: v.get("value").map(|x| x.as_str().unwrap_or("").to_string()),
            text: str_field(v, "text"),
        }
    }
}

/// The UI-side live-search model: rows, picks + the last-criteria state.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct JiraSearchPanel {
    pub text: String,
    pub projects: Vec<String>,
    pub projects_all: bool,
    pub rows: Vec<SearchRow>,
    pub max_results: String,
    pub catalog: Vec<Value>,
    pub kinds: Vec<FilterKind>,
}

impl JiraSearchPanel {

    pub fn params(&self) -> Value {
        json!({
            "text": self.text,
            "projects": self.projects,
            "projectsAll": self.projects_all,
            "maxResults": self.max_results,
            "rows": self.rows.iter().map(SearchRow::to_json).collect::<Vec<_>>(),
        })
    }

    /// The UserDefaults-shaped state the app persists between opens.
    pub fn state(&self) -> Value {
        json!({
            "text": self.text,
            "projects": self.projects,
            "projectsAll": self.projects_all,
            "maxResults": self.max_results,
            "rows": self.rows.iter().map(SearchRow::to_json).collect::<Vec<_>>(),
        })
    }

    pub fn from_state(v: &Value) -> Self {
        let rows = v
            .get("rows")
            .and_then(Value::as_array)
            .map(|a| a.iter().map(SearchRow::from_json).collect())
            .unwrap_or_default();
        JiraSearchPanel {
            text: str_field(v, "text"),
            projects: strings(v.get("projects").unwrap_or(&Value::Null)),
            projects_all: get_bool(v, "projectsAll"),
            max_results: match v.get("maxResults") {
                Some(Value::String(s)) => s.clone(),
                Some(other) => scalar_str(other),
                None => String::new(),
            },
            rows,
            catalog: Vec::new(),
            kinds: Vec::new(),
        }
    }
}

// ===========================================================================
// jira_dashboard.py — the poll-job editor helpers
// ===========================================================================

/// `args` text: comma-separated key=value pairs (first `=` splits).
pub fn parse_args(s: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for part in s.split(',') {
        let kv: Vec<&str> = part.splitn(2, '=').map(str::trim).collect();
        if kv.len() == 2 && !kv[0].is_empty() {
            out.insert(kv[0].to_string(), kv[1].to_string());
        }
    }
    out
}

// ===========================================================================
// jira_boards.py — column/card mapping + the board page template
// ===========================================================================

/// `boards.done_limit` — a fully-done column clamps to this many cards.
pub fn done_limit() -> usize {
    defaults_usize(&["boards", "done_limit"])
}
/// `boards.other_column` — where a board's unmapped statuses land.
pub fn other_column() -> String {
    defaults_str(&["boards", "other_column"])
}
/// `boards.hot_words` — priorities drawn "hot" on the board page.
pub fn hot_words() -> Vec<String> {
    defaults_strs(&["boards", "hot_words"])
}
/// `boards.color_keys` — the board page's `@key` colors.
pub fn color_keys() -> Vec<String> {
    defaults_strs(&["boards", "color_keys"])
}
/// `category_names` — To Do / In Progress / Done.
pub fn category_names() -> &'static [String] {
    static NAMES: OnceLock<Vec<String>> = OnceLock::new();
    NAMES.get_or_init(|| defaults_strs(&["category_names"])).as_slice()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BoardCard {
    pub key: String,
    pub title: String,
    #[serde(rename = "type")]
    pub type_: String,
    pub priority: String,
    pub assignee: String,
    pub status: String,
    pub cat: i32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BoardColumn {
    pub name: String,
    pub cards: Vec<BoardCard>,
    pub more: usize,
}

/// `words` arrives as an object of name -> array (already resolved).
fn words_value(w: &Value) -> BTreeMap<String, Vec<String>> {
    let mut out = BTreeMap::new();
    if let Some(obj) = w.as_object() {
        for (k, v) in obj {
            out.insert(k.clone(), strings(v));
        }
    }
    out
}

/// Rows -> the board's columns. With a board spec statuses map to their
/// column and the rest land in "Not on the board"; without one the three
/// status categories are the columns. A fully-done column clamps to
/// `done_limit()` cards.
pub fn board_columns(params: &Value) -> Vec<BoardColumn> {
    let rows = params
        .get("rows")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let spec: Vec<&Value> = params
        .get("columns")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter(|c| c.is_object()).collect())
        .unwrap_or_default();
    let cats = params
        .get("categories")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let w = words_value(params.get("words").unwrap_or(&Value::Null));
    let people = params
        .get("people")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let names: Vec<String> = {
        let mut names: Vec<String> = params
            .get("categoryNames")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .filter(|v: &Vec<String>| !v.is_empty())
            .unwrap_or_else(|| category_names().iter().map(|s| s.to_string()).collect());
        if names.len() < 3 {
            // `category` answers 0..2: a short name list would panic below
            // (parity with pylib/jira_boards.py, which pads the same way).
            names.extend(
                category_names()[names.len()..3]
                    .iter()
                    .map(|s| s.to_string()),
            );
        }
        names
    };

    struct Col {
        name: String,
        statuses: Vec<String>,
        cards: Vec<BoardCard>,
    }
    let mut cols: Vec<Col> = if !spec.is_empty() {
        spec.iter()
            .map(|c| Col {
                name: str_field(c, "name"),
                statuses: strings(c.get("statuses").unwrap_or(&Value::Null)),
                cards: Vec::new(),
            })
            .collect()
    } else {
        names
            .iter()
            .map(|n| Col {
                name: n.clone(),
                statuses: Vec::new(),
                cards: Vec::new(),
            })
            .collect()
    };

    let mut other: Vec<BoardCard> = Vec::new();
    for r in &rows {
        let st = str_field(r, "status");
        let assignee = str_field(r, "assignee");
        let display = if assignee.is_empty() {
            String::new()
        } else {
            people
                .get(&assignee)
                .and_then(Value::as_str)
                .unwrap_or(&assignee)
                .to_string()
        };
        let title = {
            let t = str_field(r, "title");
            if t.is_empty() {
                str_field(r, "rowTitle")
            } else {
                t
            }
        };
        let card = BoardCard {
            key: str_field(r, "key"),
            title,
            type_: str_field(r, "type"),
            priority: str_field(r, "priority"),
            assignee: display,
            status: st.clone(),
            cat: category(&st, &cats, &w),
        };
        if !spec.is_empty() {
            let mut placed = false;
            for col in cols.iter_mut() {
                if col.statuses.contains(&st) {
                    col.cards.push(card.clone());
                    placed = true;
                    break;
                }
            }
            if !placed {
                other.push(card);
            }
        } else {
            let idx = card.cat.clamp(0, 2) as usize;
            cols[idx].cards.push(card);
        }
    }

    let mut out: Vec<BoardColumn> = cols
        .into_iter()
        .map(|c| {
            let done = !c.cards.is_empty() && c.cards.iter().all(|k| k.cat == 2);
            let cards = if done {
                c.cards[..done_limit().min(c.cards.len())].to_vec()
            } else {
                c.cards.clone()
            };
            let more = c.cards.len() - cards.len();
            BoardColumn {
                name: c.name,
                cards,
                more,
            }
        })
        .collect();
    if !other.is_empty() {
        out.push(BoardColumn {
            name: other_column(),
            cards: other,
            more: 0,
        });
    }
    out
}

const BOARD_PAGE_TEMPLATE: &str = r##"<!doctype html><html><head><meta charset="utf-8"><style>
:root { --bg: @bg; --col: @col; --card: @card;
  --card-hover: @cardHover; --line: @line; --text: @text;
  --dim: @dim; --accent: @accent; --todo: @dim;
  --prog: @accent; --done: @done; --hot: @hot; }
* { box-sizing: border-box; }
html, body { margin: 0; height: 100%; background: transparent; color: var(--text);
  font: 12.5px/1.4 -apple-system, BlinkMacSystemFont, "SF Pro Text", sans-serif; }
#board { display: flex; gap: 10px; padding: 12px 14px 14px; height: 100%; overflow-x: auto; }
.col { flex: 1 0 210px; max-width: 360px; background: var(--col); border-radius: 10px; display: flex;
  flex-direction: column; min-height: 0; }
.col h4 { margin: 0; padding: 10px 12px 8px; font-weight: 600; font-size: 10.5px; line-height: 1.2;
  letter-spacing: .08em; text-transform: uppercase; color: var(--dim); display: flex; gap: 8px; }
.col h4 .n { margin-left: auto; font-variant-numeric: tabular-nums; }
.cards { padding: 0 8px 8px; display: flex; flex-direction: column; gap: 7px; overflow-y: auto; min-height: 0; }
.card { background: var(--card); border: 1px solid var(--line); border-radius: 8px; padding: 8px 10px;
  display: grid; gap: 6px; cursor: default; }
.card:hover { background: var(--card-hover); }
.card .t { color: var(--text); overflow-wrap: anywhere; }
.card .f { display: flex; align-items: center; gap: 8px; color: var(--dim); font-size: 11.5px; }
.key { font: 11.5px ui-monospace, "SF Mono", Menlo, monospace; color: var(--accent); }
.dot { width: 8px; height: 8px; border-radius: 2px; flex: none; border: 1.5px solid var(--todo); }
.c1 .dot { border-color: var(--prog); background: linear-gradient(90deg, var(--prog) 50%, transparent 50%); }
.c2 .dot { border-color: var(--done); background: var(--done); }
.c2 .t { color: var(--dim); }
.hot { color: var(--hot); }
.av { margin-left: auto; width: 20px; height: 20px; border-radius: 50%; background: var(--line);
  color: var(--text); font-size: 9.5px; font-weight: 600; display: grid; place-items: center; flex: none; }
.more, .empty { color: var(--dim); font-size: 11.5px; padding: 4px 4px 2px; }
.none { color: var(--dim); padding: 40px; text-align: center; width: 100%; }
</style></head><body><div id="board"></div><script>
function esc(s) { return String(s).replace(/[&<>"]/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;'}[c])); }
function initials(n) { const p = n.trim().split(/\s+/); return ((p[0]||'')[0]||'') + (p.length > 1 ? p[p.length-1][0] : ''); }
function render(cols) {
  const b = document.getElementById('board');
  if (!cols.length || cols.every(c => !c.cards.length)) { b.innerHTML = '<div class="none">No issues here</div>'; return; }
  b.innerHTML = cols.map(c => `<section class="col"><h4>${esc(c.name)}<span class="n">${c.cards.length + c.more}</span></h4>
    <div class="cards">${c.cards.map(k => `<div class="card c${k.cat}" data-key="${esc(k.key)}" title="${esc(k.status)}">
      <div class="f"><span class="dot"></span><span class="key">${esc(k.key)}</span><span>${esc(k.type)}</span></div>
      <div class="t">${esc(k.title)}</div>
      <div class="f"><span class="${/@hotPattern/i.test(k.priority) ? 'hot' : ''}">${esc(k.priority)}</span>
        ${k.assignee ? `<span class="av" title="${esc(k.assignee)}">${esc(initials(k.assignee).toUpperCase())}</span>` : ''}</div>
    </div>`).join('')}${c.more ? `<div class="more">+ ${c.more} more — Table shows them all</div>` : ''}
    ${!c.cards.length && !c.more ? '<div class="empty">Nothing here</div>' : ''}</div></section>`).join('');
}
document.addEventListener('click', e => {
  const c = e.target.closest('.card');
  if (c) window.webkit.messageHandlers.board.postMessage('open:' + c.dataset.key);
});
window.webkit.messageHandlers.board.postMessage('ready');
</script></body></html>
"##;

/// The board page HTML with `@key` substitution (longest-first, so `@cardHover`
/// is never corrupted by `@card`).
pub fn board_page(colors: &HashMap<String, String>) -> String {
    let mut subst: Vec<(String, String)> = color_keys()
        .iter()
        .map(|k| (k.clone(), colors.get(k.as_str()).cloned().unwrap_or_default()))
        .collect();
    subst.push(("hotPattern".to_string(), hot_words().join("|")));
    subst.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
    let mut html = BOARD_PAGE_TEMPLATE.to_string();
    for (k, v) in &subst {
        html = html.replace(&format!("@{k}"), v);
    }
    html
}

// ===========================================================================
// jira_pages.py — ticket status lifecycle + page model
// ===========================================================================

// ===========================================================================
// JiraDashboardWindow / JiraColumnEditor model
// ===========================================================================

#[derive(Clone, Debug, PartialEq)]
pub struct DescribeRequest {
    pub purpose: String,
    pub curl: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DescribeColumn {
    pub field: String,
    pub title: String,
    pub width: f64,
    pub width_px: Option<f64>,
    pub align: String,
    pub sortable: bool,
    pub filterable: bool,
    pub label: String,
    pub api_fields: Vec<String>,
}

impl DescribeColumn {
    fn from_value(v: &Value) -> Self {
        DescribeColumn {
            field: str_field(v, "field"),
            title: str_field(v, "title"),
            width: v.get("width").and_then(Value::as_f64).unwrap_or(0.0),
            width_px: v.get("width_px").and_then(Value::as_f64),
            align: {
                let a = str_field(v, "align");
                if a.is_empty() {
                    "left".to_string()
                } else {
                    a
                }
            },
            sortable: get_bool(v, "sortable"),
            filterable: get_bool(v, "filterable"),
            label: str_field(v, "label"),
            api_fields: strings(v.get("apiFields").unwrap_or(&Value::Null)),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct DescribeJob {
    pub name: String,
    pub type_: String,
    pub window: String,
    pub enabled: bool,
    pub file: String,
    pub path: String,
    pub max_results: i64,
    pub max_total: i64,
    pub projects: Value,
    pub extra_jql: String,
    pub job: String,
    pub args: Value,
    pub next_window: String,
    pub jql: String,
    pub requests: Vec<DescribeRequest>,
    pub notes: Vec<String>,
    pub columns_spec: String,
    pub columns: Vec<DescribeColumn>,
    pub api_fields: Vec<String>,
    pub status: String,
    pub last_run: String,
    pub last_success: String,
    pub last_window: String,
    pub next_run: String,
    pub items: Option<i64>,
    pub last_error: String,
    pub last_curl: String,
    pub scoped_projects: Vec<String>,
    pub shared_sync: bool,
}

impl DescribeJob {
    fn from_value(v: &Value) -> Self {
        DescribeJob {
            name: str_field(v, "name"),
            type_: str_field(v, "type"),
            window: str_field(v, "window"),
            enabled: get_bool(v, "enabled"),
            file: str_field(v, "file"),
            path: str_field(v, "path"),
            max_results: v.get("maxResults").and_then(Value::as_i64).unwrap_or(0),
            max_total: v.get("maxTotal").and_then(Value::as_i64).unwrap_or(0),
            projects: v.get("projects").cloned().unwrap_or(Value::Null),
            extra_jql: str_field(v, "extraJql"),
            job: str_field(v, "job"),
            args: v.get("args").cloned().unwrap_or_else(|| json!({})),
            next_window: str_field(v, "nextWindow"),
            jql: str_field(v, "jql"),
            requests: v
                .get("requests")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .map(|r| DescribeRequest {
                            purpose: str_field(r, "purpose"),
                            curl: str_field(r, "curl"),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            notes: strings(v.get("notes").unwrap_or(&Value::Null)),
            columns_spec: str_field(v, "columnsSpec"),
            columns: v
                .get("columns")
                .and_then(Value::as_array)
                .map(|a| a.iter().map(DescribeColumn::from_value).collect())
                .unwrap_or_default(),
            api_fields: strings(v.get("apiFields").unwrap_or(&Value::Null)),
            status: str_field(v, "status"),
            last_run: str_field(v, "lastRun"),
            last_success: str_field(v, "lastSuccess"),
            last_window: str_field(v, "lastWindow"),
            next_run: str_field(v, "nextRun"),
            items: v.get("items").and_then(Value::as_i64),
            last_error: str_field(v, "lastError"),
            last_curl: str_field(v, "lastCurl"),
            scoped_projects: strings(v.get("scopedProjects").unwrap_or(&Value::Null)),
            shared_sync: get_bool(v, "sharedSync"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct LiveSearch {
    pub file: String,
    pub columns_spec: String,
    pub columns: Vec<DescribeColumn>,
    pub max_results: i64,
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct DirectoryInfo {
    pub path: String,
    pub fetched_at: String,
    pub for_projects: Vec<String>,
    pub warnings: Vec<String>,
    pub counts: BTreeMap<String, i64>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct CatalogField {
    pub field: String,
    pub titles: Vec<String>,
    pub used_by: Vec<String>,
    pub api_fields: Vec<String>,
    pub label: String,
    pub default_label: String,
    pub renamed: bool,
    pub custom: bool,
    pub base: bool,
    pub seen_in: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Describe {
    pub enabled: bool,
    pub background_poll: bool,
    pub site: String,
    pub auth: String,
    pub has_token: bool,
    pub config_path: String,
    pub team_path: String,
    pub team_exists: bool,
    pub tick: String,
    pub poll_margin_minutes: i64,
    pub fetch_comments: bool,
    pub status: String,
    pub last_run: String,
    pub last_error: String,
    pub lock_held: bool,
    pub scope: Vec<String>,
    pub rebuild_on_next_poll: bool,
    pub project_keys: Vec<String>,
    pub columns_template: String,
    pub search_defaults: Value,
    pub live_search: LiveSearch,
    pub directory: DirectoryInfo,
    pub team: Value,
    pub team_own: Value,
    pub team_jobs: Vec<String>,
    pub catalog: Vec<CatalogField>,
    pub available_fields: Vec<String>,
    pub endpoints: Vec<DescribeJob>,
    pub login_curl: String,
    pub setup_state: String,
    /// `statusPath` — the JSON cache `jira_status.py` writes (the window's
    /// status file row).
    pub status_path: String,
    /// `pollLog` / `debugLog` — the two poll logs the window lists.
    pub poll_log: String,
    pub debug_log: String,
    /// `problems` — the config/scope warnings the payload carries.
    pub problems: Vec<String>,
}

impl Describe {
    /// Parse the `jira_poll.py --describe` JSON payload.
    pub fn from_value(v: &Value) -> Self {
        let live = v.get("liveSearch").cloned().unwrap_or(Value::Null);
        let dir = v.get("directory").cloned().unwrap_or(Value::Null);
        Describe {
            enabled: get_bool(v, "enabled"),
            background_poll: get_bool(v, "backgroundPoll"),
            site: str_field(v, "site"),
            auth: str_field(v, "auth"),
            has_token: get_bool(v, "hasToken"),
            config_path: str_field(v, "configPath"),
            team_path: str_field(v, "teamPath"),
            team_exists: get_bool(v, "teamExists"),
            tick: str_field(v, "tick"),
            poll_margin_minutes: v.get("pollMarginMinutes").and_then(Value::as_i64).unwrap_or(5),
            fetch_comments: get_bool(v, "fetchComments"),
            status: str_field(v, "status"),
            last_run: str_field(v, "lastRun"),
            last_error: str_field(v, "lastError"),
            lock_held: get_bool(v.get("lock").unwrap_or(&Value::Null), "held"),
            scope: strings(v.get("scope").unwrap_or(&Value::Null)),
            rebuild_on_next_poll: get_bool(v, "rebuildOnNextPoll"),
            project_keys: strings(v.get("projectKeys").unwrap_or(&Value::Null)),
            columns_template: str_field(v, "columnsTemplate"),
            search_defaults: v.get("searchDefaults").cloned().unwrap_or_else(|| json!({})),
            live_search: LiveSearch {
                file: str_field(&live, "file"),
                columns_spec: str_field(&live, "columnsSpec"),
                columns: live
                    .get("columns")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().map(DescribeColumn::from_value).collect())
                    .unwrap_or_default(),
                max_results: live.get("maxResults").and_then(Value::as_i64).unwrap_or(0),
                path: str_field(&live, "path"),
            },
            directory: DirectoryInfo {
                path: str_field(&dir, "path"),
                fetched_at: str_field(&dir, "fetchedAt"),
                for_projects: strings(dir.get("forProjects").unwrap_or(&Value::Null)),
                warnings: strings(dir.get("warnings").unwrap_or(&Value::Null)),
                counts: dir
                    .get("counts")
                    .and_then(Value::as_object)
                    .map(|o| o.iter().map(|(k, x)| (k.clone(), x.as_i64().unwrap_or(0))).collect())
                    .unwrap_or_default(),
            },
            team: v.get("team").cloned().unwrap_or_else(|| json!({})),
            team_own: v.get("teamOwn").cloned().unwrap_or_else(|| json!({})),
            team_jobs: strings(v.get("teamJobs").unwrap_or(&Value::Null)),
            catalog: v
                .get("catalog")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .map(|c| CatalogField {
                            field: str_field(c, "field"),
                            titles: strings(c.get("titles").unwrap_or(&Value::Null)),
                            used_by: strings(c.get("usedBy").unwrap_or(&Value::Null)),
                            api_fields: strings(c.get("apiFields").unwrap_or(&Value::Null)),
                            label: str_field(c, "label"),
                            default_label: str_field(c, "defaultLabel"),
                            renamed: get_bool(c, "renamed"),
                            custom: get_bool(c, "custom"),
                            base: get_bool(c, "base"),
                            seen_in: strings(c.get("seenIn").unwrap_or(&Value::Null)),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            available_fields: strings(v.get("availableFields").unwrap_or(&Value::Null)),
            endpoints: v
                .get("endpoints")
                .and_then(Value::as_array)
                .map(|a| a.iter().map(DescribeJob::from_value).collect())
                .unwrap_or_default(),
            login_curl: str_field(v, "loginCurl"),
            setup_state: str_field(v.get("setup").unwrap_or(&Value::Null), "state"),
            status_path: str_field(v, "statusPath"),
            poll_log: str_field(v, "pollLog"),
            debug_log: str_field(v, "debugLog"),
            problems: strings(v.get("problems").unwrap_or(&Value::Null)),
        }
    }

    pub fn job(&self, name: &str) -> Option<&DescribeJob> {
        self.endpoints.iter().find(|j| j.name == name)
    }
}

// ===========================================================================
// JiraBoardBar model + the board state
// ===========================================================================

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SprintChoice {
    pub id: String,
    pub title: String,
    pub header: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct JiraBoardBarModel {
    pub board: String,
    pub choices: Vec<SprintChoice>,
    pub picked: String,
    pub mode: String,
    pub pinned: bool,
    pub summary: String,
}

impl Default for JiraBoardBarModel {
    fn default() -> Self {
        JiraBoardBarModel {
            board: String::new(),
            choices: Vec::new(),
            picked: String::new(),
            mode: "columns".to_string(),
            pinned: false,
            summary: String::new(),
        }
    }
}

pub const BOARD_BAR_HEIGHT: f64 = 40.0;

impl JiraBoardBarModel {

    pub fn show(
        &mut self,
        board: &str,
        choices: Vec<SprintChoice>,
        picked: &str,
        mode: &str,
        pinned: bool,
        summary: &str,
    ) {
        self.board = board.to_string();
        self.choices = choices;
        self.picked = picked.to_string();
        self.mode = mode.to_string();
        self.pinned = pinned;
        self.summary = summary.to_string();
    }

    /// The bar's ☆ button title: the same glyph the row lead-ins use, so the
    /// star language is one thing everywhere ("star = keep this handy").
    pub fn pin_title(&self) -> &'static str {
        if self.pinned {
            "★ Starred"
        } else {
            "☆ Star"
        }
    }
}

// ===========================================================================
// IPC seams
// ===========================================================================

fn helper_call(method: &str, params: Value, timeout_secs: u64) -> Result<Value, String> {
    PythonHelper::shared()
        .call(
            method,
            params,
            Duration::from_secs(timeout_secs),
            Duration::from_secs(5),
        )
        .map_err(|e| e.to_string())
}

pub fn helper_filter_kinds(catalog: &[Value]) -> Result<Vec<FilterKind>, String> {
    let out = helper_call("jira.filter_kinds", json!({"catalog": catalog}), 30)?;
    let kinds = out
        .get("kinds")
        .and_then(Value::as_array)
        .ok_or("jira.filter_kinds: no kinds")?;
    Ok(kinds
        .iter()
        .map(|k| FilterKind {
            key: str_field(k, "key"),
            title: str_field(k, "title"),
            kind: ValueKind::from_str(&str_field(k, "kind")),
        })
        .collect())
}

/// `JiraDirectory.fetchOptions` → `jira.options`: picker rows
/// `{id, title, detail, group, unused}` for a kind (`project`, `user`,
/// `status`, `label`, `version`, `value`).
pub fn helper_options(params: Value) -> Result<Vec<Value>, String> {
    let out = helper_call("jira.options", params, 60)?;
    Ok(out.get("options").and_then(Value::as_array).cloned().unwrap_or_default())
}

/// `JiraDirectory.load()` → `jira.directory` (the parsed directory.json).
pub fn helper_directory(path: &str) -> Result<Value, String> {
    helper_call("jira.directory", json!({"path": path}), 60)
}

pub fn helper_criteria(params: &Value) -> Result<Value, String> {
    helper_call("jira.criteria", params.clone(), 30)
}

pub fn jira_script() -> String {
    // repo checkout: rust/ws-rs/src/views/jira.rs -> ../../../jira/jira_poll.py
    let dir = env!("CARGO_MANIFEST_DIR");
    format!("{dir}/../../jira/jira_poll.py")
}

fn python3_candidates() -> Vec<String> {
    let mut c = Vec::new();
    if let Ok(p) = std::env::var("WS_PYTHON") {
        if !p.is_empty() {
            c.push(p);
        }
    }
    c.push("/opt/homebrew/bin/python3".into());
    c.push("/usr/local/bin/python3".into());
    c.push("python3".into());
    c
}

pub fn find_python3() -> Option<String> {
    // Cache the winner: probe-spawning (up to 4 version checks) on every
    // jira_poll.py call - and every 2.5 s of the force ticker - is pure waste.
    // Only a SUCCESS is stored (get_or_try_init is unstable), so a python
    // installed mid-session is picked up on the next call.
    static FOUND: OnceLock<String> = OnceLock::new();
    if let Some(p) = FOUND.get() {
        return Some(p.clone());
    }
    let found = python3_candidates()
        .into_iter()
        .find(|cand| {
            crate::app::process_run::run_process(
                cand,
                &[
                    "-c".to_string(),
                    "import sys; sys.exit(0 if sys.version_info >= (3, 11) else 1)".to_string(),
                ],
                None,
                None,
                false,
            )
            .map(|o| o.code == 0)
            .unwrap_or(false)
        })?;
    let _ = FOUND.set(found.clone());
    Some(found)
}

/// The readable reason for a non-zero `jira_poll.py` exit: the
/// `{ok:false, problems|error}` JSON it prints on stdout first, else the
/// last non-empty stderr line. An argv dump + full traceback only arrives
/// middle-truncated in a 1.4 s toast pill; callers add the item context and
/// log the long form.
fn poll_failure(code: i32, out: &str, err: &str) -> String {
    let json = serde_json::from_str::<serde_json::Value>(out.trim()).ok().and_then(|v| {
        v.get("problems")
            .and_then(serde_json::Value::as_array)
            .and_then(|a| a.first())
            .and_then(serde_json::Value::as_str)
            .or_else(|| v.get("error").and_then(serde_json::Value::as_str))
            .map(str::to_string)
    });
    let last = err.lines().rev().map(str::trim).find(|l| !l.is_empty());
    json.or_else(|| last.map(str::to_string))
        .unwrap_or_else(|| format!("jira_poll.py exited {code}"))
}

/// Run `jira_poll.py ARGS…` and parse one JSON object off stdout. Blocking —
/// call off the main thread. `--board-view` reads the whole issue cache and
/// network verbs wait out rate limits, so the deadline is generous; it
/// exists so a wedged child cannot hold a UI slot forever.
pub fn run_jira_poll(args: &[&str], stdin: Option<&[u8]>) -> Result<Value, String> {
    let python = find_python3().ok_or("jira: python 3.11+ not found")?;
    let mut argv = vec![jira_script()];
    argv.extend(args.iter().map(|s| s.to_string()));
    let out = crate::app::process_run::run_process_deadline(
        &python,
        &argv,
        stdin,
        None,
        false,
        Some(std::time::Duration::from_secs(600)),
    )
    .map_err(|e| format!("could not run python: {e}"))?;
    if out.code != 0 {
        return Err(poll_failure(out.code, &out.out, &out.err));
    }
    serde_json::from_str(out.out.trim())
        .map_err(|_| format!("unexpected output: {}", out.out.trim().chars().take(160).collect::<String>()))
}

// ===========================================================================
// View model — sections, tabs and rendition rows (AppKit-free)
// ===========================================================================

/// Register the Jira palette entry (only while enabled, mirroring `[jira-config]`).
pub fn register(registry: &mut Registry, enabled: bool) {
    if enabled {
        registry.add_palette(crate::app::registry::PaletteCommand::new(
            "jira-config",
            "Jira Config",
            "views",
        ));
    }
    // `do:jira-config:job:<name>` / `do:jira-config:state` — the Config
    // surface's per-job selection, verifiable headless.
    crate::views::jira_config::register(registry);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn register_wires_the_per_job_config_hooks() {
        // The `do:jira-config:*` hooks register whether or not the palette
        // entry does.
        let mut r = Registry::new();
        register(&mut r, false);
        assert_eq!(
            r.dispatch_test_do("jira-config:job:issues"),
            Some(json!({"job": "issues"}))
        );
        assert!(r.dispatch_test_do("jira-config:state").unwrap().get("job").is_some());
        assert!(r.palette_commands().iter().all(|c| c.id != "jira-config"));
        // Enabled: the palette row joins the same hooks.
        let mut r2 = Registry::new();
        register(&mut r2, true);
        assert!(r2.palette_commands().iter().any(|c| c.id == "jira-config"));
        assert!(r2.dispatch_test_do("jira-config:state").is_some());
    }

    fn col() -> ListColumn {
        ListColumn {
            field: "a".into(),
            title: "T".into(),
            width: 120.0,
            width_px: None,
            align: "left".into(),
            sortable: false,
            filterable: false,
        }
    }

    fn helper_ready() -> bool {
        let lib = format!("{}/../../pylib", env!("CARGO_MANIFEST_DIR"));
        if !crate::app::python_helper::lib_has_helper(&lib) {
            return false;
        }
        let h = PythonHelper::shared();
        h.configure(&lib);
        h.call_default("ping", json!({})).is_ok()
    }

    fn tmpdir(name: &str) -> String {
        let base = std::env::temp_dir().join(format!("ws-rs-jira-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        base.to_string_lossy().into_owned()
    }

    /// Write `body` and stamp the file with an explicit, distinct mtime so the
    /// in-process `COMMENTS_CACHE` (keyed on path+mtime) refreshes even on
    /// filesystems with 1-second mtime granularity (Docker overlayfs).
    fn write_cache(path: &str, body: &str, secs: u64) {
        std::fs::write(path, body).unwrap();
        let f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        f.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs))
            .unwrap();
    }

    // -------------------------------------------------------- jira_fields

    #[test]
    fn parse_columns_defaults() {
        let cols = parse_columns("key");
        assert_eq!(cols.len(), 1);
        let c = &cols[0];
        assert_eq!((c.field.as_str(), c.title.as_str(), c.width), ("key", "key", 0.0));
        assert_eq!(c.align, "left");
        assert!(!c.sortable && !c.filterable);
    }

    #[test]
    fn parse_columns_full_segments() {
        let cols = parse_columns(" status : Status : 120 : RIGHT : filter+sort , title");
        assert_eq!(cols[0].field, "status");
        assert_eq!(cols[0].title, "Status");
        assert_eq!(cols[0].width, 120.0);
        assert_eq!(cols[0].align, "right");
        assert!(cols[0].sortable && cols[0].filterable);
        assert_eq!(cols[1].field, "title");
    }

    #[test]
    fn parse_columns_width_and_align_fallbacks() {
        assert_eq!(parse_columns("a::12.5")[0].width, 12.5);
        assert_eq!(parse_columns("a::x")[0].width, 0.0);
        assert_eq!(parse_columns("a:: -3 ")[0].width, 0.0);
        assert_eq!(parse_columns("a:::middle")[0].align, "left");
        assert_eq!(parse_columns("a")[0].align, "left");
    }

    #[test]
    fn parse_columns_flag_separators() {
        assert!(parse_columns("a::::filter|sort")[0].sortable);
        assert!(parse_columns("a::::sort/filter")[0].filterable);
        assert!(parse_columns("a::::SORT")[0].sortable);
    }

    #[test]
    fn parse_columns_empty() {
        assert!(parse_columns("").is_empty());
        assert!(parse_columns("  , , ").is_empty());
        assert!(parse_columns(":title").is_empty());
    }

    #[test]
    fn serialize_columns_widths_and_flags() {
        let v = |s: &str| serde_json::from_str::<Value>(s).unwrap();
        assert_eq!(v(&serialize_columns(&[col()], true)), json!([{"field":"a","title":"T","width":120}]));
        let mut c = col();
        c.width = 120.5;
        assert_eq!(v(&serialize_columns(&[c.clone()], true)), json!([{"field":"a","title":"T","width":120.5}]));
        c.width = 0.0;
        assert_eq!(v(&serialize_columns(&[c], true)), json!([{"field":"a","title":"T"}]));
        assert_eq!(v(&serialize_columns(&[col()], false)), json!([{"field":"a","width":120}]));
        let mut c = col();
        c.sortable = true;
        c.filterable = true;
        assert_eq!(v(&serialize_columns(&[c], true)), json!([{"field":"a","title":"T","width":120,"filter":true,"sort":true}]));
        let mut c = col();
        c.sortable = true;
        assert_eq!(v(&serialize_columns(&[c], true)), json!([{"field":"a","title":"T","width":120,"sort":true}]));
    }

    #[test]
    fn parse_columns_structured_value() {
        let arr = json!([
            {"field": "key", "width": 8, "filter": true, "sort": true},
            {"field": "updated", "width": 10, "align": "right", "sort": true},
        ]);
        let cols = parse_columns_value(&arr);
        assert_eq!(cols.len(), 2);
        assert_eq!((cols[0].field.as_str(), cols[0].width, cols[0].filterable, cols[0].sortable), ("key", 8.0, true, true));
        assert_eq!((cols[1].field.as_str(), cols[1].width, cols[1].align.as_str()), ("updated", 10.0, "right"));
        // a JSON string parses the same way
        assert_eq!(parse_columns_value(&Value::String(arr.to_string())), cols);
    }

    #[test]
    fn serialize_columns_round_trip() {
        let spec = "key:Key:80:left:filter, status:Status:120:right:filter+sort, epic";
        assert_eq!(
            parse_columns(&serialize_columns(&parse_columns(spec), true)),
            parse_columns(spec)
        );
    }

    #[test]
    fn base_labels_spot_checks() {
        assert_eq!(base_field_label("key"), Some("Key"));
        assert_eq!(base_field_label("epic"), Some("Epic / parent"));
        assert_eq!(base_field_label("release"), Some("Fix versions"));
    }

    // -------------------------------------------------------- jira_data

    #[test]
    fn words_defaults_and_overrides() {
        let w = words(&json!({}));
        assert!(w["done"].contains(&"done".to_string()));
        assert!(w["cancelled"].contains(&"won't".to_string()));
        assert!(w["urgent"].contains(&"p0".to_string()));

        let w = words(&json!({"status-done-words": "shipped, nailed"}));
        assert_eq!(w["done"], vec!["shipped", "nailed"]);
        assert!(w["cancelled"].contains(&"cancel".to_string()));

        let w = words(&json!({"status-done-words": " Done , ,DONE,  "}));
        assert_eq!(w["done"], vec!["done", "done"]);

        let w = words(&json!({"status-done-words": ""}));
        assert!(w["done"].contains(&"resolved".to_string()));
    }

    #[test]
    fn category_directory_and_word_fallback() {
        let w = words(&json!({}));
        let cats: Map<String, Value> =
            serde_json::from_value(json!({"To Do": "new", "Review": "indeterminate", "Finished": "done"}))
                .unwrap();
        assert_eq!(category("To Do", &cats, &w), 0);
        assert_eq!(category("Review", &cats, &w), 1);
        assert_eq!(category("Finished", &cats, &w), 2);
        assert_eq!(category("Shipped", &Map::new(), &w), 2);
        assert_eq!(category("Won't Do", &Map::new(), &w), 2);
        assert_eq!(category("Backlog", &Map::new(), &w), 0);
        assert_eq!(category("In review", &Map::new(), &w), 1);

        let w2 = words(&json!({"status-done-words": "nailed"}));
        assert_eq!(category("nailed it", &Map::new(), &w2), 2);
        assert_eq!(category("shipped", &Map::new(), &w2), 1);
        assert_eq!(category("anything", &Map::new(), &BTreeMap::new()), 1);
    }

    #[test]
    fn style_rules_shape() {
        let r = style_rules(&json!({}));
        assert_eq!(r.status[0].words, r.words["cancelled"]);
        assert_eq!(r.status[0].style.tone.as_deref(), Some("dim"));
        assert_eq!(r.status[0].style.quiets_row, Some(true));
        assert_eq!(r.status[1].style.mark.as_deref(), Some("filled"));
        assert!(r.dim_fields.iter().any(|f| f == "key"));
        assert!(r.dim_fields.iter().any(|f| f == "release"));
        assert_eq!(r.priority[0].style.tone.as_deref(), Some("danger"));
        assert_eq!(r.priority[0].style.bold, Some(true));
        assert_eq!(r.priority_fallback.tone.as_deref(), Some("dim"));
        assert_eq!(r.release_status[0].contains, "unreleased");
        assert_eq!(r.release_status[0].style.tone.as_deref(), Some("warning"));
        assert_eq!(r.status_fallback.mark.as_deref(), Some("hollow"));

        let r = style_rules(&json!({"status-blocked-words": "stuck"}));
        assert_eq!(r.status[2].words, vec!["stuck"]);
    }

    // -------------------------------------------------------- jira_search

    #[test]
    fn filter_kinds_base_and_catalog() {
        let kinds = filter_kinds(&[]);
        let keys: Vec<&str> = kinds.iter().map(|k| k.key.as_str()).collect();
        assert_eq!(
            &keys[..5],
            &["assignee", "reporter", "status", "statusCategory", "issuetype"]
        );
        assert_eq!(
            kinds[8..11].iter().map(|k| k.kind).collect::<Vec<_>>(),
            vec![ValueKind::Date, ValueKind::Date, ValueKind::Date]
        );

        let catalog = vec![
            json!({"field": "assignee", "label": "Owner"}),
            json!({"field": "release", "label": "Fix Version"}),
            json!({"field": "updated", "label": "Changed"}),
            json!({"field": "title", "label": ""}),
        ];
        let kinds = filter_kinds(&catalog);
        let by_key: HashMap<&str, &str> =
            kinds.iter().map(|k| (k.key.as_str(), k.title.as_str())).collect();
        assert_eq!(by_key["assignee"], "Owner");
        assert_eq!(by_key["fixVersion"], "Fix Version");
        assert_eq!(by_key["updated"], "Changed within");
        assert_eq!(by_key["field:title"], "Summary contains");

        let catalog = vec![
            json!({"field": "customfield_100", "label": "Story Points"}),
            json!({"field": "key"}),
            json!({"field": "status"}),
            json!({"field": "description", "label": "Desc"}),
            json!({"field": "epic", "label": ""}),
        ];
        let kinds = filter_kinds(&catalog);
        assert!(kinds.iter().any(|k| k.key == "field:customfield_100"
            && k.title == "Story Points contains"
            && k.kind == ValueKind::Text));
        assert!(kinds
            .iter()
            .any(|k| k.key == "field:epic" && k.title == "epic contains"));
        assert!(!kinds.iter().any(|k| k.key == "field:key"));
        assert!(!kinds.iter().any(|k| k.key == "field:status"));
    }

    #[test]
    fn criteria_text_projects_and_rows() {
        assert_eq!(criteria(&json!({"text": "  retry bug  "})), json!({"text": "retry bug"}));
        assert_eq!(criteria(&json!({"text": "   "})), json!({}));
        assert_eq!(
            criteria(&json!({"projects": ["A"], "projectsAll": false})),
            json!({"projects": ["A"]})
        );
        assert_eq!(criteria(&json!({"projects": ["A"], "projectsAll": true})), json!({}));
        assert_eq!(criteria(&json!({"projects": [], "projectsAll": false})), json!({}));

        let crit = criteria(&json!({"rows": [
            {"key": "status", "selected": ["Done", "In Review"]},
            {"key": "assignee", "selected": []},
            {"key": "updated", "value": ""},
            {"key": "field:title", "text": "  retry  "},
            {"key": "field:epic", "text": "  "},
            {"key": "labels", "text": "not-a-field"},
        ]}));
        assert_eq!(
            crit,
            json!({"status": ["Done", "In Review"], "updated": "", "fields": {"title": "retry"}})
        );
        assert_eq!(
            criteria(&json!({"rows": [{"key": "created", "value": ""}]})),
            json!({"created": ""})
        );
    }

    #[test]
    fn criteria_max_results_strictness() {
        assert_eq!(criteria(&json!({"maxResults": "50"})), json!({"maxResults": 50}));
        assert_eq!(criteria(&json!({"maxResults": "0"})), json!({}));
        assert_eq!(criteria(&json!({"maxResults": ""})), json!({}));
        assert_eq!(criteria(&json!({"maxResults": " 3 "})), json!({}));
        assert_eq!(criteria(&json!({"maxResults": "x"})), json!({}));
    }

    #[test]
    fn criteria_full_example() {
        let crit = criteria(&json!({
            "text": "timeout",
            "projects": ["APP"],
            "projectsAll": false,
            "rows": [
                {"key": "assignee", "selected": ["currentUser()"]},
                {"key": "resolved", "value": "-2w"},
                {"key": "field:description", "text": "retry"},
            ],
            "maxResults": "25",
        }));
        assert_eq!(
            crit,
            json!({
                "text": "timeout",
                "projects": ["APP"],
                "assignee": ["currentUser()"],
                "resolved": "-2w",
                "fields": {"description": "retry"},
                "maxResults": 25,
            })
        );
    }

    // -------------------------------------------------------- jira_dashboard

    #[test]
    fn parse_args_trims_and_splits_first_equals() {
        let a = parse_args("a=1, b=2=x, bad, =x, c =");
        assert_eq!(a.get("a"), Some(&"1".to_string()));
        assert_eq!(a.get("b"), Some(&"2=x".to_string()));
        assert_eq!(a.get("c"), Some(&"".to_string()));
        assert!(!a.contains_key("bad"));
        assert!(parse_args("").is_empty());
        assert!(parse_args("  ,  ").is_empty());
    }

    fn draft_params(over: Value) -> Value {
        let mut p = json!({
            "kind": "job", "name": " team-bugs ", "type": "issues",
            "pageSize": "50", "maxTotal": "500", "window": " 30m ",
            "enabled": true, "projects": ["A"], "projectsAll": false,
            "queryIndex": 0, "jql": "", "jobTitle": "", "args": "",
            "columns": "key,title"
        });
        if let (Some(base), Some(add)) = (p.as_object_mut(), over.as_object()) {
            for (k, v) in add {
                base.insert(k.clone(), v.clone());
            }
        }
        p
    }

    // -------------------------------------------------------- jira_boards

    fn board_row(key: &str, status: &str) -> Value {
        json!({
            "key": key, "status": status, "title": format!("Title {key}"),
            "type": "Bug", "priority": "High", "assignee": "", "rowTitle": ""
        })
    }

    fn board_params(rows: Value, over: Value) -> Value {
        let mut p = json!({
            "rows": rows, "columns": [], "categories": {},
            "words": words(&json!({})), "people": {},
            "categoryNames": ["To Do", "In Progress", "Done"]
        });
        if let (Some(base), Some(add)) = (p.as_object_mut(), over.as_object()) {
            for (k, v) in add {
                base.insert(k.clone(), v.clone());
            }
        }
        p
    }

    #[test]
    fn board_columns_by_category() {
        let out = board_columns(&board_params(
            json!([board_row("A-1", "Backlog"), board_row("A-2", "In Review"), board_row("A-3", "Done")]),
            json!({}),
        ));
        assert_eq!(
            out.iter().map(|c| c.name.clone()).collect::<Vec<_>>(),
            vec!["To Do", "In Progress", "Done"]
        );
        assert_eq!(out[0].cards[0].key, "A-1");
        assert_eq!(out[1].cards[0].key, "A-2");
        assert_eq!(out[2].cards[0].key, "A-3");
        assert!(out.iter().all(|c| c.more == 0));
    }

    #[test]
    fn board_columns_directory_categories_win() {
        let out = board_columns(&board_params(
            json!([board_row("A-1", "Reviewing")]),
            json!({"categories": {"Reviewing": "indeterminate"}}),
        ));
        assert_eq!(out[1].cards[0].key, "A-1");
    }

    #[test]
    fn board_columns_spec_and_other() {
        let out = board_columns(&board_params(
            json!([board_row("A-1", "To Do"), board_row("A-2", "Doing"), board_row("A-3", "Odd")]),
            json!({"columns": [
                {"name": "Ready", "statuses": ["To Do"]},
                {"name": "Doing", "statuses": ["Doing"]},
            ]}),
        ));
        assert_eq!(
            out.iter().map(|c| c.name.clone()).collect::<Vec<_>>(),
            vec!["Ready".to_string(), "Doing".to_string(), other_column()]
        );
        assert_eq!(out[2].cards[0].key, "A-3");
    }

    #[test]
    fn board_columns_done_clamp() {
        let rows: Vec<Value> = (0..31).map(|i| board_row(&format!("D-{i:02}"), "Done")).collect();
        let out = board_columns(&board_params(json!(rows), json!({})));
        assert_eq!(out[2].cards.len(), done_limit());
        assert_eq!(out[2].more, 1);

        let out = board_columns(&board_params(
            json!([board_row("D-1", "Done"), board_row("D-2", "Backlog")]),
            json!({}),
        ));
        assert_eq!(out[0].cards.len(), 1);
    }

    #[test]
    fn board_columns_people_and_title_fallback() {
        let mut r = board_row("A-1", "Done");
        r["assignee"] = json!("ada");
        r["title"] = json!("");
        r["rowTitle"] = json!("Row Fallback");
        let out = board_columns(&board_params(json!([r]), json!({"people": {"ada": "Ada Lovelace"}})));
        let card = &out[2].cards[0];
        assert_eq!(card.assignee, "Ada Lovelace");
        assert_eq!(card.title, "Row Fallback");

        let out = board_columns(&board_params(json!([board_row("A-2", "Done")]), json!({})));
        assert_eq!(out[2].cards[0].assignee, "");
    }

    #[test]
    fn board_columns_empty_still_yields_columns() {
        let out = board_columns(&board_params(json!([]), json!({})));
        assert_eq!(out.len(), 3);
        assert!(out.iter().all(|c| c.cards.is_empty()));
    }

    #[test]
    fn board_page_colors_js_and_no_placeholder() {
        let mut colors = HashMap::new();
        colors.insert("bg".to_string(), "#111".to_string());
        colors.insert("col".to_string(), "rgba(1,1,1,0.045)".to_string());
        colors.insert("done".to_string(), "#0a0".to_string());
        let html = board_page(&colors);
        assert!(html.contains("--bg: #111;"));
        assert!(html.contains("--done: #0a0;"));
        assert!(html.contains("--col: rgba(1,1,1,0.045);"));
        assert!(html.contains("${esc(c.name)}"));
        assert!(html.contains("${k.cat}"));
        assert!(html.contains("render(cols)"));
        assert!(html.contains("postMessage('ready')"));

        let empty = board_page(&HashMap::new());
        let bytes = empty.as_bytes();
        for i in 0..bytes.len() {
            if bytes[i] == b'@' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_alphabetic() {
                panic!("unsubstituted placeholder at {i}");
            }
        }
    }

    // -------------------------------------------------------- jira_ticket

    // -------------------------------------------------------- describe + editor

    #[test]
    fn describe_parses_payload() {
        let v: Value = serde_json::from_str(r#"{
            "enabled": true, "backgroundPoll": false, "site": "https://x", "hasToken": true,
            "configPath": "/c.json", "teamPath": "/t.json", "teamExists": false,
            "tick": "60s", "pollMarginMinutes": 5, "fetchComments": true,
            "status": "ok", "lastRun": "raw", "lastError": "",
            "lock": {"held": true, "pid": "42"},
            "scope": ["A", "B"], "rebuildOnNextPoll": false,
            "projectKeys": ["A"], "columnsTemplate": "key,title",
            "searchDefaults": {"max_results_search": 500},
            "liveSearch": {
                "file": "search.json", "columnsSpec": "key,title",
                "columns": [{"field": "key", "title": "Key", "width": 80.0, "align": "left",
                             "sortable": true, "filterable": false, "label": "Key", "apiFields": []}],
                "maxResults": 100, "path": "/cache/search.json"
            },
            "directory": {"path": "/d.json", "fetchedAt": "now", "forProjects": ["A"],
                          "warnings": ["w"], "counts": {"projects": 2}},
            "team": {"project_keys": ["A"]}, "teamOwn": {}, "teamJobs": ["all"],
            "catalog": [{"field": "key", "titles": ["Key"], "usedBy": ["job:all"],
                         "apiFields": [], "label": "Key", "defaultLabel": "Key",
                         "renamed": false, "custom": false, "base": true, "seenIn": []}],
            "availableFields": ["key", "title"],
            "setup": {"state": "done"},
            "endpoints": [{
                "name": "all", "type": "issues", "window": "10m", "enabled": true,
                "file": "all.json", "path": "/cache/all.json", "maxResults": 500, "maxTotal": 0,
                "projects": "*", "extraJql": "", "job": "", "args": {},
                "nextWindow": "full", "jql": "ORDER BY created ASC",
                "requests": [{"purpose": "search", "curl": "curl x"}],
                "notes": ["shares ONE issue-cache sync per tick"],
                "columnsSpec": "key,title",
                "columns": [{"field": "key", "title": "Key"}],
                "apiFields": ["summary"], "status": "ok", "lastRun": "raw",
                "lastSuccess": "raw", "lastWindow": "full", "nextRun": "due",
                "items": 12, "lastError": "", "lastCurl": "", "scopedProjects": ["A"],
                "sharedSync": true
            }],
            "loginCurl": "curl -u x"
        }"#).unwrap();
        let d = Describe::from_value(&v);
        assert!(d.enabled && d.has_token && !d.background_poll);
        assert!(d.lock_held);
        assert_eq!(d.scope, vec!["A".to_string(), "B".to_string()]);
        assert_eq!(d.live_search.max_results, 100);
        assert_eq!(d.directory.counts["projects"], 2);
        assert_eq!(d.setup_state, "done");
        assert_eq!(d.endpoints.len(), 1);
        let job = d.job("all").unwrap();
        assert_eq!(job.name, "all");
        assert_eq!(job.max_results, 500);
        assert_eq!(job.items, Some(12));
        assert!(job.shared_sync);
        assert_eq!(d.catalog[0].base, true);
    }

    // -------------------------------------------------------- helper parity

    // --------------------------------------------------- view model (AppKit-free)

    #[test]
    fn poll_failure_prefers_problems_then_stderr() {
        // The `{ok:false, problems:[…]}` stdout the edit commands print is
        // the reason; no argv dumps, no tracebacks in the user-facing string.
        // The poller speaks "starred" directly (one vocabulary end-to-end —
        // Rust used to rewrite the phrase here and string-match it twice).
        let e = poll_failure(
            1,
            r#"{"ok": false, "problems": ["board 42 is not starred"]}"#,
            "",
        );
        assert_eq!(e, "board 42 is not starred");
        let e = poll_failure(2, "not json", "noise\njira-poll: boom\n");
        assert_eq!(e, "jira-poll: boom");
        let e = poll_failure(3, "", "");
        assert_eq!(e, "jira_poll.py exited 3");
    }

}
