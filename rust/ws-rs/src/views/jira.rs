//! Jira view family — mirrors `JiraDashboard.swift`, `JiraSearch.swift`,
//! `JiraBoard.swift` and `JiraTicket.swift`.
//!
//! Bounded first cut: the pure DATA + STATE models (the same logic that lives
//! in `pylib/jira_fields.py`, `jira_data.py`, `jira_search.py`,
//! `jira_dashboard.py`, `jira_boards.py` and `jira_pages.py`), the `--describe`
//! payload parser, the CLI/helper IPC seams, and the controller. The AppKit
//! NSView tree is built from the ported models by
//! [`JiraViewController::build`] / [`build_content`] (macOS only).

use crate::app::config::ListColumn;
use crate::app::python_helper::PythonHelper;
use crate::app::registry::{RectI, Registry, SlotMember, SlotView};
use crate::ui::card::CardConfig;
use crate::ui::popup::PopupConfig;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use crate::ui::theme::{PopupColors, PopupTone, Rgba};
#[cfg(target_os = "macos")]
use objc2::rc::Retained;
#[cfg(target_os = "macos")]
use objc2_app_kit::NSView;

// ===========================================================================
// Small JSON helpers
// ===========================================================================

fn get_str<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

fn get_bool(v: &Value, k: &str) -> bool {
    v.get(k).map(truthy).unwrap_or(false)
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|x| x != 0.0).unwrap_or(false),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

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

const BASE_FIELD_LABELS: &[(&str, &str)] = &[
    ("key", "Key"),
    ("title", "Title"),
    ("status", "Status"),
    ("assignee", "Assignee"),
    ("reporter", "Reporter"),
    ("priority", "Priority"),
    ("labels", "Labels"),
    ("description", "Description"),
    ("project", "Project"),
    ("updated", "Updated"),
    ("release", "Fix versions"),
    ("releaseLabel", "Release"),
    ("releaseDate", "Release date"),
    ("releaseStatus", "Released"),
    ("comments", "Comments"),
    ("components", "Components"),
    ("epic", "Epic / parent"),
];

pub fn base_field_labels() -> &'static [(&'static str, &'static str)] {
    BASE_FIELD_LABELS
}

pub fn base_field_label(field: &str) -> Option<&'static str> {
    BASE_FIELD_LABELS
        .iter()
        .find(|(k, _)| *k == field)
        .map(|(_, v)| *v)
}

/// `field:Title:width:align:flags, ...` -> columns. Title defaults to the
/// field; width 0 shares the leftover width; negative widths clamp to 0;
/// flags (`filter` / `sort`, joined by `+` or split by `:`/`|`/`/`) set the
/// sortable / filterable bits.
pub fn parse_columns(spec: &str) -> Vec<ListColumn> {
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
            align,
            sortable: flags.iter().any(|f| f == "sort"),
            filterable: flags.iter().any(|f| f == "filter"),
        });
    }
    cols
}

/// The inverse; integral widths stay integers, fractional keep one decimal;
/// flags come back as `filter+sort` (filter first).
pub fn serialize_columns(cols: &[ListColumn], titles: bool) -> String {
    let mut out = Vec::new();
    for c in cols {
        let width = c.width;
        let w = if width.fract() == 0.0 {
            format!("{}", width as i64)
        } else {
            format!("{width:.1}")
        };
        let mut flags: Vec<&str> = Vec::new();
        if c.filterable {
            flags.push("filter");
        }
        if c.sortable {
            flags.push("sort");
        }
        let flags = flags.join("+");
        let title = if titles { c.title.clone() } else { String::new() };
        let seg = format!("{}:{}:{}:{}", c.field, title, w, c.align);
        out.push(if flags.is_empty() {
            seg
        } else {
            format!("{seg}:{flags}")
        });
    }
    out.join(", ")
}

/// Python `re.sub(r"[\s-]+", "_", k.strip()).lower()`.
pub fn norm_key(k: &str) -> String {
    let mut out = String::new();
    let mut prev_us = false;
    for ch in k.trim().chars() {
        if ch.is_whitespace() || ch == '-' {
            if !prev_us {
                out.push('_');
                prev_us = true;
            }
        } else {
            out.push(ch.to_ascii_lowercase());
            prev_us = false;
        }
    }
    out
}

/// The base labels + team.json custom_fields aliases + field_labels renames
/// (field_labels wins; section keys matched loosely).
pub fn merged_labels(team: Option<&Value>) -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, String> = BASE_FIELD_LABELS
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let Some(team) = team.and_then(Value::as_object) else {
        return out;
    };
    for (k, v) in team {
        if norm_key(k) != "custom_fields" {
            continue;
        }
        let Some(cf) = v.as_object() else { continue };
        for (alias, spec) in cf {
            let mut label: Option<String> = None;
            if let Some(s) = spec.as_object() {
                for (k2, v2) in s {
                    if norm_key(k2) == "label" {
                        label = v2.as_str().map(str::to_string);
                    }
                }
            }
            out.insert(
                alias.clone(),
                label.filter(|s| !s.is_empty()).unwrap_or_else(|| alias.clone()),
            );
        }
    }
    for (k, v) in team {
        if norm_key(k) != "field_labels" {
            continue;
        }
        let Some(fl) = v.as_object() else { continue };
        // the old Swift cast rejected the whole dict on any non-string value
        if !fl.values().all(Value::is_string) {
            continue;
        }
        for (f, label) in fl {
            let s = label.as_str().unwrap_or("").trim();
            if !s.is_empty() {
                out.insert(f.clone(), s.to_string());
            }
        }
    }
    out
}

// ===========================================================================
// jira_data.py — words, categories, workflow, style rules, comments
// ===========================================================================

const WORD_KEYS: &[(&str, &str)] = &[
    ("cancelled", "status-cancelled-words"),
    ("done", "status-done-words"),
    ("blocked", "status-blocked-words"),
    ("active", "status-active-words"),
    ("waiting", "status-waiting-words"),
    ("new", "status-new-words"),
    ("urgent", "priority-urgent-words"),
];

const WORD_DEFAULTS: &[(&str, &str)] = &[
    ("status-cancelled-words", "cancel, won't, wont, reject, duplicate"),
    (
        "status-done-words",
        "done, closed, resolved, released, complete, fixed, shipped",
    ),
    ("status-blocked-words", "block, fail, impediment"),
    (
        "status-active-words",
        "progress, review, test, qa, develop, doing, verif",
    ),
    ("status-waiting-words", "hold, wait, pending, paused"),
    (
        "status-new-words",
        "backlog, open, to do, todo, new, selected, triage, funnel",
    ),
    ("priority-urgent-words", "highest, blocker, critical, urgent, p0, p1"),
];

const DIM_FIELDS: &[&str] = &[
    "key",
    "updated",
    "created",
    "duedate",
    "releasedate",
    "project",
    "releaselabel",
    "release",
];

fn word_default(key: &str) -> &'static str {
    WORD_DEFAULTS
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| *v)
        .unwrap_or("")
}

/// `[jira]` config values -> the seven lowercased word lists (an explicit
/// non-empty value replaces only that list; an empty value falls back).
pub fn words(values: &Value) -> BTreeMap<String, Vec<String>> {
    let mut out = BTreeMap::new();
    for (name, key) in WORD_KEYS {
        let raw = values
            .get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| word_default(key));
        let list = raw
            .split(',')
            .map(|p| p.trim().to_lowercase())
            .filter(|p| !p.is_empty())
            .collect();
        out.insert((*name).to_string(), list);
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

/// `[jira] workflow` CSV -> the explicit step list, else None.
pub fn workflow_steps(value: Option<&str>) -> Option<Vec<String>> {
    let value = value?;
    if value.is_empty() {
        return None;
    }
    let steps: Vec<String> = value
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if steps.is_empty() {
        None
    } else {
        Some(steps)
    }
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
        dim_fields: DIM_FIELDS.iter().map(|s| s.to_string()).collect(),
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Comment {
    pub author: String,
    pub body: String,
    pub created: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CommentsResult {
    pub stamp: f64,
    pub comments: Vec<Comment>,
}

struct CommentsCache {
    path: Option<String>,
    stamp: Option<f64>,
    index: HashMap<String, Vec<Comment>>,
}

static COMMENTS_CACHE: OnceLock<Mutex<CommentsCache>> = OnceLock::new();

fn comments_cache() -> &'static Mutex<CommentsCache> {
    COMMENTS_CACHE.get_or_init(|| {
        Mutex::new(CommentsCache {
            path: None,
            stamp: None,
            index: HashMap::new(),
        })
    })
}

fn mtime_seconds(path: &str) -> Option<f64> {
    let md = std::fs::metadata(path).ok()?;
    let t = md.modified().ok()?;
    t.duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs_f64())
}

/// The issue's comment rows, or None when it has none (the whole list must
/// be objects).
fn comment_rows(issue: &Value) -> Option<Vec<Comment>> {
    let obj = issue.as_object()?;
    let arr = obj.get("comments")?.as_array()?;
    if arr.is_empty() {
        return None;
    }
    let mut rows = Vec::new();
    for c in arr {
        let c = c.as_object()?;
        rows.push(Comment {
            author: get_str(&Value::Object(c.clone()), "author").to_string(),
            body: get_str(&Value::Object(c.clone()), "body").to_string(),
            created: get_str(&Value::Object(c.clone()), "created").to_string(),
        });
    }
    Some(rows)
}

/// One issue's comments from the poller's issue cache, parsed once per
/// (path, mtime) and cached in-process.
pub fn comments(path: &str, key: &str) -> CommentsResult {
    let Some(stamp) = mtime_seconds(path) else {
        return CommentsResult {
            stamp: 0.0,
            comments: Vec::new(),
        };
    };
    let mut cache = comments_cache().lock().unwrap();
    if cache.path.as_deref() != Some(path) || cache.stamp != Some(stamp) {
        let mut index: HashMap<String, Vec<Comment>> = HashMap::new();
        if let Ok(text) = std::fs::read_to_string(path) {
            if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(&text) {
                for (k, issue) in map {
                    if let Some(rows) = comment_rows(&issue) {
                        index.insert(k, rows);
                    }
                }
            }
        }
        cache.path = Some(path.to_string());
        cache.stamp = Some(stamp);
        cache.index = index;
    }
    CommentsResult {
        stamp,
        comments: cache.index.get(key).cloned().unwrap_or_default(),
    }
}

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
    pub fn as_str(self) -> &'static str {
        match self {
            ValueKind::Users => "users",
            ValueKind::List => "list",
            ValueKind::Date => "date",
            ValueKind::Text => "text",
        }
    }
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

// jira/defaults.json `search_ui.kinds` (key, title, labelField, kind, suffix)
const SEARCH_KINDS: &[(&str, &str, Option<&str>, &str, &str)] = &[
    ("assignee", "Assignee", Some("assignee"), "users", ""),
    ("reporter", "Reporter", Some("reporter"), "users", ""),
    ("status", "Status", Some("status"), "list", ""),
    ("statusCategory", "Status category", None, "list", ""),
    ("issuetype", "Issue type", None, "list", ""),
    ("priority", "Priority", Some("priority"), "list", ""),
    ("fixVersion", "Release", Some("release"), "list", ""),
    ("labels", "Labels", Some("labels"), "list", ""),
    ("updated", "Updated", Some("updated"), "date", " within"),
    ("created", "Created", None, "date", " within"),
    ("resolved", "Resolved", None, "date", " within"),
    ("field:title", "Summary", Some("title"), "text", " contains"),
    ("field:description", "Description", Some("description"), "text", " contains"),
];

// jira/defaults.json `search_ui.skip_fields`
const SKIP_CATALOG_FIELDS: &[&str] = &[
    "key",
    "title",
    "status",
    "assignee",
    "release",
    "releaseLabel",
    "releaseDate",
    "releaseStatus",
    "priority",
    "labels",
    "description",
    "reporter",
    "project",
    "updated",
    "comments",
];

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

/// The base kinds plus the describe catalog with its skip list.
pub fn filter_kinds(catalog: &[Value]) -> Vec<FilterKind> {
    let mut kinds = Vec::new();
    for (key, base_title, label_field, kind, suffix) in SEARCH_KINDS {
        let mut title = if let Some(field) = label_field {
            label_for(catalog, field, base_title)
        } else {
            (*base_title).to_string()
        };
        title.push_str(suffix);
        kinds.push(FilterKind {
            key: (*key).to_string(),
            title,
            kind: ValueKind::from_str(kind),
        });
    }
    for c in catalog {
        let Some(field) = c.get("field").and_then(Value::as_str) else {
            continue;
        };
        if SKIP_CATALOG_FIELDS.contains(&field) {
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
    pub fn new(catalog: Vec<Value>) -> Self {
        let kinds = filter_kinds(&catalog);
        JiraSearchPanel {
            catalog,
            kinds,
            ..Default::default()
        }
    }

    pub fn params(&self) -> Value {
        json!({
            "text": self.text,
            "projects": self.projects,
            "projectsAll": self.projects_all,
            "maxResults": self.max_results,
            "rows": self.rows.iter().map(SearchRow::to_json).collect::<Vec<_>>(),
        })
    }

    pub fn criteria(&self) -> Value {
        criteria(&self.params())
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

/// Load the last criteria (`UserDefaults` equivalent: a JSON file).
pub fn load_last_criteria(path: &str) -> Option<JiraSearchPanel> {
    let text = std::fs::read_to_string(path).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    Some(JiraSearchPanel::from_state(&v))
}

pub fn save_last_criteria(path: &str, panel: &JiraSearchPanel) -> std::io::Result<()> {
    let data = serde_json::to_vec(&panel.state()).unwrap_or_default();
    std::fs::write(path, data)
}

// ===========================================================================
// jira_dashboard.py — the poll-job editor helpers
// ===========================================================================

/// Lowercase, runs of non-alphanumerics -> single underscores; a leading
/// non-letter gets an `f_` prefix.
pub fn snake_case(s: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut cur = String::new();
    for ch in s.to_lowercase().chars() {
        if ch.is_alphanumeric() {
            cur.push(ch);
        } else if !cur.is_empty() {
            parts.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        parts.push(cur);
    }
    let out = parts.join("_");
    if !out.is_empty() && !out.chars().next().unwrap().is_alphabetic() {
        format!("f_{out}")
    } else {
        out
    }
}

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

#[derive(Clone, Debug, PartialEq)]
pub struct LimitResult {
    pub ok: bool,
    pub value: i64,
    pub message: Option<String>,
}

/// Swift `Int()` semantics: empty means 0 (default), otherwise a whole
/// number >= 0.
pub fn check_limit(s: &str, what: &str) -> LimitResult {
    let s = s.trim();
    if s.is_empty() {
        return LimitResult {
            ok: true,
            value: 0,
            message: None,
        };
    }
    match s.parse::<i64>() {
        Ok(n) if n >= 0 => LimitResult {
            ok: true,
            value: n,
            message: None,
        },
        _ => LimitResult {
            ok: false,
            value: 0,
            message: Some(format!("✗ {what} must be a whole number (empty = default)")),
        },
    }
}

/// The poll-job draft assembly.
pub fn draft(params: &Value) -> Value {
    let ps = check_limit(get_str(params, "pageSize"), "Page size");
    if !ps.ok {
        return json!({"ok": false, "message": ps.message.unwrap_or_default()});
    }
    let mt = check_limit(get_str(params, "maxTotal"), "Max issues");
    if !mt.ok {
        return json!({"ok": false, "message": mt.message.unwrap_or_default()});
    }
    let typ = get_str(params, "type");
    let mut o = json!({
        "name": get_str(params, "name").trim(),
        "type": typ,
        "maxResults": ps.value,
        "maxTotal": mt.value,
        "window": get_str(params, "window").trim(),
        "enabled": get_bool(params, "enabled"),
    });
    if typ != "directory" && params.get("columns").is_some() {
        o["columns"] = params.get("columns").cloned().unwrap_or(Value::Null);
    }
    let picked: Vec<String> = params
        .get("projects")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default();
    o["projects"] = if get_bool(params, "projectsAll") || picked.is_empty() {
        json!("*")
    } else {
        json!(picked)
    };
    let q = if typ == "issues" {
        params.get("queryIndex").and_then(Value::as_i64).unwrap_or(0)
    } else {
        0
    };
    o["jql"] = if q == 1 {
        json!(get_str(params, "jql").trim())
    } else {
        json!("")
    };
    let title = get_str(params, "jobTitle");
    o["job"] = if q >= 2 {
        json!(title.strip_prefix("team.json: ").unwrap_or(""))
    } else {
        json!("")
    };
    o["args"] = if q >= 2 {
        json!(parse_args(get_str(params, "args")))
    } else {
        json!({})
    };
    json!({"ok": true, "draft": o})
}

/// The live-search persist payload.
pub fn live_persist(params: &Value) -> Value {
    let m = check_limit(get_str(params, "maxResults"), "Max results");
    if !m.ok {
        return json!({"ok": false, "message": m.message.unwrap_or_default()});
    }
    let columns = match params.get("columns") {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    };
    json!({"ok": true, "draft": {"columns": columns, "maxResults": m.value}})
}

fn find_customfield(raw: &str) -> Option<String> {
    let needle = "customfield_";
    let bytes = raw.as_bytes();
    let mut start = 0;
    while let Some(pos) = raw[start..].find(needle) {
        let i = start + pos;
        let after = i + needle.len();
        let mut j = after;
        while j < bytes.len() && bytes[j].is_ascii_digit() {
            j += 1;
        }
        if j > after {
            return Some(raw[i..j].to_string());
        }
        start = after;
    }
    None
}

/// The custom-field sheet: id from the typed text or a case-insensitive name
/// match, label/alias defaults, entry shape, label clearing.
pub fn custom_field_entry(params: &Value) -> Value {
    let customs: Vec<&Value> = params
        .get("customs")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter(|c| c.is_object()).collect())
        .unwrap_or_default();
    let raw = get_str(params, "raw");
    let alias = get_str(params, "alias").trim();
    let label = get_str(params, "label").trim();
    let desc = get_str(params, "description").trim();
    let mut fid = find_customfield(raw).unwrap_or_default();
    if fid.is_empty() {
        for c in &customs {
            if let Some(name) = c.get("name").and_then(Value::as_str) {
                if name.to_lowercase() == raw.to_lowercase() {
                    fid = scalar_str(c.get("id").unwrap_or(&Value::Null));
                    break;
                }
            }
        }
    }
    if fid.is_empty() {
        return json!({
            "ok": false,
            "message": "✗ pick a Jira custom field (or type its customfield_NNNNN id)",
        });
    }
    let jira_name = customs
        .iter()
        .find(|c| {
            scalar_str(c.get("id").unwrap_or(&Value::Null)) == fid
                && c.get("name").map(Value::is_string).unwrap_or(false)
        })
        .and_then(|c| c.get("name").and_then(Value::as_str))
        .unwrap_or("");
    let lbl = if !label.is_empty() { label } else { jira_name };
    let a = if !alias.is_empty() {
        alias.to_string()
    } else {
        snake_case(if !lbl.is_empty() { lbl } else { &fid })
    };
    let mut entry = json!({
        "field_id": fid,
        "label": if !lbl.is_empty() { lbl } else { a.as_str() },
    });
    if !desc.is_empty() {
        entry["description"] = json!(desc);
    }
    let mut d = params
        .get("currentCustomFields")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    d.insert(a.clone(), entry);
    let mut fl = params
        .get("currentLabels")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let clear = fl.remove(&a).is_some();
    let followup = if clear {
        json!({"key": "field_labels", "value": fl, "done": format!("label of {a}")})
    } else {
        Value::Null
    };
    json!({
        "ok": true,
        "alias": a,
        "save": {"key": "custom_fields",
                 "value": d,
                 "done": format!("{} {a}", if get_bool(params, "isNew") { "added" } else { "updated" })},
        "followup": followup,
    })
}

pub fn field_label_save(params: &Value) -> Value {
    let f = get_str(params, "field");
    let v = get_str(params, "value").trim();
    let default = get_str(params, "default");
    let mut d = params
        .get("current")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let done;
    if v.is_empty() || v == default {
        d.remove(f);
        done = format!("{f} back to “{default}”");
    } else {
        d.insert(f.to_string(), Value::String(v.to_string()));
        done = format!("{f} → “{v}”");
    }
    json!({"ok": true, "value": d, "done": done})
}

pub fn key_value_save(params: &Value) -> Value {
    let name = get_str(params, "name").trim();
    let val = get_str(params, "value").trim();
    if name.is_empty() || val.is_empty() {
        return json!({"ok": false, "beep": true});
    }
    let mut nd = params
        .get("current")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    nd.insert(name.to_string(), Value::String(val.to_string()));
    json!({
        "ok": true,
        "value": nd,
        "done": format!("{} {name}", if get_bool(params, "existing") { "updated" } else { "added" }),
    })
}

pub fn default_save(params: &Value) -> Value {
    let key = get_str(params, "key");
    let v = get_str(params, "value").trim();
    let n = v.parse::<i64>().ok().filter(|n| *n >= 0);
    let Some(n) = n else {
        return json!({"ok": false, "message": format!("✗ {key} must be a whole number")});
    };
    let mut d = params
        .get("current")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    d.insert(key.to_string(), json!(n));
    json!({"ok": true, "value": d, "done": format!("{key} = {n}")})
}

/// The poll-status line: stage/reason while waiting, else the message.
pub fn progress_text(p: Option<&Value>, now: Option<f64>) -> String {
    let empty = json!({});
    let p = p.unwrap_or(&empty);
    let stage = if p.get("stage").map(Value::is_string).unwrap_or(false) {
        get_str(p, "stage")
    } else {
        ""
    };
    let now = now.unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0)
    });
    if let Some(wu) = p.get("waitingUntil").and_then(Value::as_f64) {
        if wu > now {
            let left = (wu - now) as i64;
            let reason = if p.get("reason").map(Value::is_string).unwrap_or(false) {
                get_str(p, "reason")
            } else {
                "waiting"
            };
            let wait = if left < 60 {
                format!("{left}s")
            } else {
                format!("{}m {}s", left / 60, left % 60)
            };
            let prefix = if stage.is_empty() {
                String::new()
            } else {
                format!("{stage}: ")
            };
            return format!("{prefix}{reason} — resuming in {wait}");
        }
    }
    if p.get("message").map(Value::is_string).unwrap_or(false) {
        get_str(p, "message").to_string()
    } else {
        String::new()
    }
}

/// The dashboard status line, its tone and tooltip, the problems list and the
/// enable button's face.
pub fn header_state(params: &Value) -> Value {
    let enabled = get_bool(params, "enabled");
    let background = get_bool(params, "background");
    let failed = get_bool(params, "failed");
    let lock_held = get_bool(params, "lockHeld");
    let last_run = get_str(params, "lastRun");
    let last_run_short = {
        let s = get_str(params, "lastRunShort");
        if s.is_empty() {
            "never"
        } else {
            s
        }
    };

    let mut line = if enabled {
        "● Polling on".to_string()
    } else if background {
        "◐ Polling in the background (Jira window off)".to_string()
    } else {
        "○ Polling off".to_string()
    };
    if !get_bool(params, "setupDone") {
        line += if get_bool(params, "scopeEmpty") {
            " — enter the projects in scope (Setup)"
        } else {
            " — setup not finished (Setup)"
        };
    } else if lock_held {
        let p = progress_text(params.get("progress"), None);
        line += " — ";
        line += if p.is_empty() { "polling now…" } else { &p };
    } else if !last_run.is_empty() {
        line += &format!(
            " — {} {last_run_short}",
            if failed { "last poll failed" } else { "last checked" }
        );
    }

    let eps = params.get("epsCount").and_then(Value::as_i64).unwrap_or(0);
    let mut tip = vec![
        format!(
            "{eps} poll job{} in {}",
            if eps == 1 { "" } else { "s" },
            get_str(params, "configPath")
        ),
        format!(
            "launchd tick: {} — each job runs when its own interval is due",
            if get_str(params, "tick").is_empty() {
                "60s"
            } else {
                get_str(params, "tick")
            }
        ),
    ];
    if !last_run.is_empty() {
        tip.push(format!("last run {last_run} {}", get_str(params, "status")));
    }
    if lock_held {
        let pid = if get_str(params, "lockPid").is_empty() {
            "?"
        } else {
            get_str(params, "lockPid")
        };
        let since = if get_str(params, "lockSinceShort").is_empty() {
            "never"
        } else {
            get_str(params, "lockSinceShort")
        };
        tip.push(format!("polling now: pid {pid} since {since}"));
    }

    let mut problems: Vec<String> = params
        .get("problems")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default();
    if !get_str(params, "lastError").is_empty() {
        problems.push(format!("last error: {}", get_str(params, "lastError")));
    }
    if !get_str(params, "enableError").is_empty() {
        problems.push(format!("enable failed: {}", get_str(params, "enableError")));
    }

    json!({
        "line": line,
        "tone": if !enabled { "dim" } else if failed { "warn" } else { "text" },
        "tip": tip,
        "problems": problems,
        "enableTitle": if enabled { "Disable Jira" } else { "Enable Jira" },
        "enablePrimary": !enabled,
    })
}

// ===========================================================================
// jira_boards.py — column/card mapping + the board page template
// ===========================================================================

pub const DONE_LIMIT: usize = 30;
pub const OTHER_COLUMN: &str = "Not on the board";
pub const HOT_WORDS: &[&str] = &["highest", "high", "critical", "blocker"];
pub const COLOR_KEYS: &[&str] = &[
    "bg", "col", "card", "cardHover", "line", "text", "dim", "accent", "done", "hot",
];
pub const CATEGORY_NAMES: &[&str] = &["To Do", "In Progress", "Done"];

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
/// `DONE_LIMIT` cards.
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
    let names: Vec<String> = params
        .get("categoryNames")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .filter(|v: &Vec<String>| !v.is_empty())
        .unwrap_or_else(|| CATEGORY_NAMES.iter().map(|s| s.to_string()).collect());

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
                c.cards[..DONE_LIMIT.min(c.cards.len())].to_vec()
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
            name: OTHER_COLUMN.to_string(),
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
    let mut subst: Vec<(String, String)> = COLOR_KEYS
        .iter()
        .map(|k| (k.to_string(), colors.get(*k).cloned().unwrap_or_default()))
        .collect();
    subst.push(("hotPattern".to_string(), HOT_WORDS.join("|")));
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepState {
    Pending,
    Current,
    Done,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LifecycleStep {
    pub name: String,
    pub state: StepState,
    /// The `· actual status` sub when the category name differs.
    pub sub: Option<String>,
}

/// `JiraTicketPage.category` — the directory map wins, then the word lists.
pub fn ticket_category(
    status: &str,
    categories: &Map<String, Value>,
    w: &BTreeMap<String, Vec<String>>,
) -> i32 {
    category(status, categories, w)
}

/// The ticket stepper: an explicit `[jira] workflow` when the status is one of
/// its steps, else the three status categories.
pub fn lifecycle_steps(
    status: &str,
    categories: &Map<String, Value>,
    w: &BTreeMap<String, Vec<String>>,
    workflow: Option<&str>,
) -> Vec<LifecycleStep> {
    let steps = workflow_steps(workflow);
    if let Some(steps) = steps {
        if let Some(cur) = steps.iter().position(|s| s == status) {
            return steps
                .iter()
                .enumerate()
                .map(|(i, s)| LifecycleStep {
                    name: s.clone(),
                    state: if i < cur {
                        StepState::Done
                    } else if i == cur {
                        StepState::Current
                    } else {
                        StepState::Pending
                    },
                    sub: None,
                })
                .collect();
        }
    }
    if status.is_empty() {
        return Vec::new();
    }
    let cur = category(status, categories, w);
    CATEGORY_NAMES
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let state = if (i as i32) < cur {
                StepState::Done
            } else if i as i32 == cur {
                StepState::Current
            } else {
                StepState::Pending
            };
            let sub = if i as i32 == cur && status.to_lowercase() != name.to_lowercase() {
                Some(status.to_string())
            } else {
                None
            };
            LifecycleStep {
                name: (*name).to_string(),
                state,
                sub,
            }
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TicketTab {
    Details,
    Comments,
    AllFields,
}

/// The ticket page model: key/title/status, the stepper and the comments tab.
#[derive(Clone, Debug, PartialEq)]
pub struct TicketPageModel {
    pub key: String,
    pub title: String,
    pub status: String,
    pub category: i32,
    pub tabs: Vec<TicketTab>,
    pub comments: Vec<Comment>,
    pub comments_loaded: bool,
    pub workflow: Option<Vec<String>>,
}

impl TicketPageModel {
    pub fn from_fields(
        fields: &Value,
        categories: &Map<String, Value>,
        w: &BTreeMap<String, Vec<String>>,
        workflow: Option<&str>,
    ) -> Self {
        let status = str_field(fields, "status");
        let title = {
            let t = str_field(fields, "title");
            if t.is_empty() {
                str_field(fields, "summary")
            } else {
                t
            }
        };
        TicketPageModel {
            key: str_field(fields, "key"),
            title,
            category: ticket_category(&status, categories, w),
            status,
            tabs: vec![TicketTab::Details, TicketTab::Comments, TicketTab::AllFields],
            comments: Vec::new(),
            comments_loaded: false,
            workflow: workflow_steps(workflow),
        }
    }

    pub fn steps(
        &self,
        categories: &Map<String, Value>,
        w: &BTreeMap<String, Vec<String>>,
    ) -> Vec<LifecycleStep> {
        let wf = self.workflow.as_ref().map(|s| s.join(", "));
        lifecycle_steps(&self.status, categories, w, wf.as_deref())
    }

    pub fn set_comments(&mut self, comments: Vec<Comment>) {
        self.comments = comments;
        self.comments_loaded = true;
    }

    pub fn comments_tab_label(&self) -> String {
        if self.comments_loaded {
            format!("Comments {}", self.comments.len())
        } else {
            "Comments".to_string()
        }
    }

    pub fn test_state(&self) -> Value {
        json!({
            "key": self.key,
            "title": self.title,
            "status": self.status,
            "category": self.category,
            "comments": self.comments.len(),
            "commentsLoaded": self.comments_loaded,
            "commentsLabel": self.comments_tab_label(),
        })
    }
}

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
    pub fn to_list_column(&self) -> ListColumn {
        ListColumn {
            field: self.field.clone(),
            title: self.title.clone(),
            width: self.width,
            align: self.align.clone(),
            sortable: self.sortable,
            filterable: self.filterable,
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
        }
    }

    pub fn job(&self, name: &str) -> Option<&DescribeJob> {
        self.endpoints.iter().find(|j| j.name == name)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColumnEditError {
    EmptyField,
    Duplicate,
}

/// `JiraColumnEditor` model: the column rows + the field meta the sheets read.
#[derive(Clone, Debug, Default)]
pub struct ColumnEditor {
    pub cols: Vec<ListColumn>,
    pub meta: HashMap<String, Value>,
    pub catalog_fields: Vec<String>,
    pub copy_sources: Vec<(String, String)>,
}

impl ColumnEditor {
    pub fn new(cols: Vec<ListColumn>) -> Self {
        ColumnEditor {
            cols,
            ..Default::default()
        }
    }

    pub fn set_catalog(&mut self, fields: Vec<String>, sources: Vec<(String, String)>) {
        self.catalog_fields = fields;
        self.copy_sources = sources;
    }

    /// `fieldName(_:)` — a team.json rename, else the base label, else the
    /// raw field.
    pub fn field_name(&self, field: &str) -> String {
        if let Some(label) = self
            .meta
            .get(field)
            .and_then(|m| m.get("label"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            return label.to_string();
        }
        base_field_label(field).unwrap_or(field).to_string()
    }

    /// `apiText(_:)` — "" when the API field is the field itself.
    pub fn api_text(&self, field: &str) -> String {
        let api = match self.meta.get(field).and_then(|m| m.get("apiFields")) {
            Some(v) => {
                let ids = strings(v);
                if ids.is_empty() {
                    field.to_string()
                } else {
                    ids.iter()
                        .map(|s| if s.is_empty() { "(no API field)".to_string() } else { s.clone() })
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            }
            None => field.to_string(),
        };
        if api == field {
            String::new()
        } else {
            api
        }
    }

    fn sanitize_field(s: &str) -> String {
        s.trim()
            .chars()
            .filter(|c| *c != ':' && *c != ',' && *c != ' ')
            .collect()
    }

    /// Add or replace a column (`editSheet` validation: non-empty unique field,
    /// width clamped to 0..100, title always cleared).
    pub fn upsert(&mut self, index: Option<usize>, mut col: ListColumn) -> Result<usize, ColumnEditError> {
        col.field = Self::sanitize_field(&col.field);
        if col.field.is_empty() {
            return Err(ColumnEditError::EmptyField);
        }
        if self
            .cols
            .iter()
            .enumerate()
            .any(|(i, c)| Some(i) != index && c.field == col.field)
        {
            return Err(ColumnEditError::Duplicate);
        }
        col.title = String::new();
        col.width = col.width.clamp(0.0, 100.0);
        match index {
            Some(i) if i < self.cols.len() => {
                self.cols[i] = col;
                Ok(i)
            }
            _ => {
                self.cols.push(col);
                Ok(self.cols.len() - 1)
            }
        }
    }

    pub fn remove(&mut self, index: usize) -> bool {
        if index >= self.cols.len() || self.cols.len() <= 1 {
            return false;
        }
        self.cols.remove(index);
        true
    }

    pub fn move_by(&mut self, index: usize, delta: isize) -> Option<usize> {
        let to = index as isize + delta;
        if to < 0 || to as usize >= self.cols.len() {
            return None;
        }
        self.cols.swap(index, to as usize);
        Some(to as usize)
    }

    pub fn replace_from_source(&mut self, i: usize) -> bool {
        if let Some((_, spec)) = self.copy_sources.get(i) {
            self.cols = parse_columns(spec);
            true
        } else {
            false
        }
    }
}

/// `JiraDashboardWindow` skeleton: the describe payload + the column editor.
pub struct JiraDashboardWindow {
    pub config: CardConfig,
    pub describe: Option<Describe>,
    pub selected_job: usize,
    pub editor: ColumnEditor,
    pub status_line: String,
}

impl Default for JiraDashboardWindow {
    fn default() -> Self {
        Self::new()
    }
}

impl JiraDashboardWindow {
    pub fn new() -> Self {
        JiraDashboardWindow {
            config: CardConfig::default(),
            describe: None,
            selected_job: 0,
            editor: ColumnEditor::default(),
            status_line: String::new(),
        }
    }

    /// `--describe` (spawns `jira_poll.py --describe`; no network).
    pub fn load_describe(&mut self) -> Result<(), String> {
        let v = run_jira_poll(&["--describe"], None)?;
        self.describe = Some(Describe::from_value(&v));
        Ok(())
    }

    pub fn jobs(&self) -> &[DescribeJob] {
        self.describe
            .as_ref()
            .map(|d| d.endpoints.as_slice())
            .unwrap_or(&[])
    }

    pub fn select_job(&mut self, i: usize) -> bool {
        if i < self.jobs().len() {
            self.selected_job = i;
            true
        } else {
            false
        }
    }

    /// The selected job's columns as `ListColumn`s (the editor's starting set).
    pub fn columns_for_selected(&self) -> Vec<ListColumn> {
        self.jobs()
            .get(self.selected_job)
            .map(|j| j.columns.iter().map(DescribeColumn::to_list_column).collect())
            .unwrap_or_default()
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
    pub fn new() -> Self {
        Self::default()
    }

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

    pub fn pin_title(&self) -> &'static str {
        if self.pinned {
            "Pinned ✓"
        } else {
            "Pin to Sidebar"
        }
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct JiraBoardState {
    pub bar: JiraBoardBarModel,
    pub columns: Vec<BoardColumn>,
    pub pinned: bool,
    pub mode: String,
    pub sprint: String,
    pub sprint_keys: Vec<String>,
}

impl JiraBoardState {
    pub fn test_state(&self) -> Value {
        json!({
            "board": self.bar.board,
            "picked": self.bar.picked,
            "mode": if self.mode.is_empty() { "columns" } else { self.mode.as_str() },
            "pinned": self.pinned,
            "columns": self.columns,
            "sprintKeys": self.sprint_keys,
        })
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

/// `jira.board_columns` through the helper (same result as [`board_columns`]).
pub fn helper_board_columns(params: &Value) -> Result<Vec<BoardColumn>, String> {
    let out = helper_call("jira.board_columns", params.clone(), 30)?;
    let cols = out
        .get("columns")
        .and_then(Value::as_array)
        .ok_or("jira.board_columns: no columns")?;
    Ok(cols
        .iter()
        .map(|c| BoardColumn {
            name: str_field(c, "name"),
            more: c.get("more").and_then(Value::as_u64).unwrap_or(0) as usize,
            cards: c
                .get("cards")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .map(|k| BoardCard {
                            key: str_field(k, "key"),
                            title: str_field(k, "title"),
                            type_: str_field(k, "type"),
                            priority: str_field(k, "priority"),
                            assignee: str_field(k, "assignee"),
                            status: str_field(k, "status"),
                            cat: k.get("cat").and_then(Value::as_i64).unwrap_or(1) as i32,
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
        .collect())
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

pub fn helper_criteria(params: &Value) -> Result<Value, String> {
    helper_call("jira.criteria", params.clone(), 30)
}

pub fn helper_parse_columns(spec: &str) -> Result<Vec<ListColumn>, String> {
    let out = helper_call("jira.columns_parse", json!({"spec": spec}), 30)?;
    let cols = out
        .get("columns")
        .and_then(Value::as_array)
        .ok_or("jira.columns_parse: no columns")?;
    Ok(cols
        .iter()
        .map(|c| ListColumn {
            field: str_field(c, "field"),
            title: str_field(c, "title"),
            width: c.get("width").and_then(Value::as_f64).unwrap_or(0.0),
            align: {
                let a = str_field(c, "align");
                if a.is_empty() {
                    "left".to_string()
                } else {
                    a
                }
            },
            sortable: get_bool(c, "sortable"),
            filterable: get_bool(c, "filterable"),
        })
        .collect())
}

fn list_column_json(c: &ListColumn) -> Value {
    json!({
        "field": c.field,
        "title": c.title,
        "width": c.width,
        "align": c.align,
        "sortable": c.sortable,
        "filterable": c.filterable,
    })
}

pub fn helper_serialize_columns(cols: &[ListColumn], titles: bool) -> Result<String, String> {
    let params = json!({
        "columns": cols.iter().map(list_column_json).collect::<Vec<_>>(),
        "titles": titles,
    });
    let out = helper_call("jira.columns_serialize", params, 30)?;
    Ok(str_field(&out, "spec"))
}

fn jira_script() -> String {
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

fn find_python3() -> Option<String> {
    for cand in python3_candidates() {
        let ok = crate::app::process_run::run_process(
            &cand,
            &[
                "-c".to_string(),
                "import sys; sys.exit(0 if sys.version_info >= (3, 11) else 1)".to_string(),
            ],
            None,
            None,
            false,
        )
        .map(|o| o.code == 0)
        .unwrap_or(false);
        if ok {
            return Some(cand);
        }
    }
    None
}

/// Run `jira_poll.py ARGS…` and parse one JSON object off stdout.
pub fn run_jira_poll(args: &[&str], stdin: Option<&[u8]>) -> Result<Value, String> {
    let python = find_python3().ok_or("jira: python 3.11+ not found")?;
    let mut argv = vec![jira_script()];
    argv.extend(args.iter().map(|s| s.to_string()));
    let out = crate::app::process_run::run_process(&python, &argv, stdin, None, false)
        .map_err(|e| format!("jira_poll.py {:?}: {e}", args))?;
    if out.code != 0 {
        return Err(format!(
            "jira_poll.py {:?} exited {}: {}",
            args,
            out.code,
            out.err.trim()
        ));
    }
    serde_json::from_str(out.out.trim())
        .map_err(|e| format!("jira_poll.py {:?}: bad JSON: {e}", args))
}

// ===========================================================================
// View model — sections, tabs and rendition rows (AppKit-free)
// ===========================================================================

// Layout constants shared by the AppKit tree and its pure layout helpers.
pub const JIRA_HEADER_HEIGHT: f64 = 44.0;
pub const JIRA_DETAIL_WIDTH: f64 = 320.0;
pub const JIRA_LIST_MIN_WIDTH: f64 = 260.0;
pub const JIRA_DEFAULT_WIDTH: f64 = 920.0;
pub const JIRA_DEFAULT_HEIGHT: f64 = 620.0;
pub const JIRA_ROW_PAD: f64 = 8.0;
pub const JIRA_ROW_LEFT: f64 = 12.0;
pub const JIRA_ROW_HEIGHT: f64 = 26.0;
pub const JIRA_ROW_TWO_LINE_HEIGHT: f64 = 44.0;
pub const JIRA_HEADING_HEIGHT: f64 = 24.0;

/// The dashboard's top-level section shown in the header tabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JiraSection {
    Search,
    Board,
    Ticket,
}

impl Default for JiraSection {
    fn default() -> Self {
        JiraSection::Search
    }
}

impl JiraSection {
    /// Every section, in header order.
    pub fn all() -> [JiraSection; 3] {
        [JiraSection::Search, JiraSection::Board, JiraSection::Ticket]
    }

    pub fn title(self) -> &'static str {
        match self {
            JiraSection::Search => "Search",
            JiraSection::Board => "Board",
            JiraSection::Ticket => "Ticket",
        }
    }

    /// The `NSButton` tag routed to the target/action handler.
    pub fn tag(self) -> i64 {
        match self {
            JiraSection::Search => 0,
            JiraSection::Board => 1,
            JiraSection::Ticket => 2,
        }
    }

    pub fn from_tag(tag: i64) -> Option<JiraSection> {
        match tag {
            0 => Some(JiraSection::Search),
            1 => Some(JiraSection::Board),
            2 => Some(JiraSection::Ticket),
            _ => None,
        }
    }
}

/// One header tab derived from the model state.
#[derive(Clone, Debug, PartialEq)]
pub struct JiraTabInfo {
    pub section: JiraSection,
    pub title: String,
    pub count: usize,
    pub enabled: bool,
    pub active: bool,
}

/// The tab's visible caption (`"Board 12"`).
pub fn tab_label(tab: &JiraTabInfo) -> String {
    if tab.count > 0 {
        format!("{} {}", tab.title, tab.count)
    } else {
        tab.title.clone()
    }
}

/// One rendered line in the list / detail panes.
#[derive(Clone, Debug, PartialEq)]
pub struct JiraRowLine {
    /// Primary text.
    pub text: String,
    /// Secondary text; empty for single-line rows.
    pub detail: String,
    /// Tone name resolved through [`tone_color`]:
    /// `text` / `dim` / `accent` / `success` / `warning` / `hot`.
    pub tone: &'static str,
    /// Group headings render bold and are not selectable.
    pub heading: bool,
    /// Stable id (board card / ticket key) when the row addresses an item.
    pub id: String,
}

impl JiraRowLine {
    fn text(text: impl Into<String>, tone: &'static str) -> Self {
        JiraRowLine {
            text: text.into(),
            detail: String::new(),
            tone,
            heading: false,
            id: String::new(),
        }
    }

    fn field(text: impl Into<String>, detail: impl Into<String>, tone: &'static str) -> Self {
        JiraRowLine {
            text: text.into(),
            detail: detail.into(),
            tone,
            heading: false,
            id: String::new(),
        }
    }

    fn heading(text: impl Into<String>) -> Self {
        JiraRowLine {
            text: text.into(),
            detail: String::new(),
            tone: "dim",
            heading: true,
            id: String::new(),
        }
    }
}

/// The height of one rendered row.
pub fn row_height(line: &JiraRowLine) -> f64 {
    if line.heading {
        JIRA_HEADING_HEIGHT
    } else if line.detail.is_empty() {
        JIRA_ROW_HEIGHT
    } else {
        JIRA_ROW_TWO_LINE_HEIGHT
    }
}

/// The document height for a stack of rows (bottom padding included).
pub fn rows_total_height(lines: &[JiraRowLine]) -> f64 {
    lines.iter().map(row_height).sum::<f64>() + JIRA_ROW_PAD
}

/// `true` when a priority matches one of the board's hot words.
pub fn priority_is_hot(priority: &str) -> bool {
    let p = priority.to_lowercase();
    HOT_WORDS.iter().any(|w| p.contains(w))
}

fn category_name(cat: i32) -> String {
    CATEGORY_NAMES
        .get(cat.clamp(0, 2) as usize)
        .map(|s| (*s).to_string())
        .unwrap_or_default()
}

fn field_row(field: &str, value: &str, tone: &'static str) -> JiraRowLine {
    JiraRowLine::field(
        base_field_label(field).unwrap_or(field).to_string(),
        value.to_string(),
        tone,
    )
}

fn filter_title(kinds: &[FilterKind], key: &str) -> String {
    kinds
        .iter()
        .find(|k| k.key == key)
        .map(|k| k.title.clone())
        .unwrap_or_else(|| key.strip_prefix("field:").unwrap_or(key).to_string())
}

fn placeholder(text: &str) -> JiraRowLine {
    JiraRowLine::text(text, "dim")
}

/// The active section for the current model state: a selected ticket wins,
/// else a populated board, else search.
pub fn active_section(has_ticket: bool, board_cards: usize) -> JiraSection {
    if has_ticket {
        JiraSection::Ticket
    } else if board_cards > 0 {
        JiraSection::Board
    } else {
        JiraSection::Search
    }
}

/// The header tabs derived from the model state.
pub fn section_tabs(has_ticket: bool, board_cards: usize, search_count: usize) -> Vec<JiraTabInfo> {
    let active = active_section(has_ticket, board_cards);
    vec![
        JiraTabInfo {
            section: JiraSection::Search,
            title: JiraSection::Search.title().to_string(),
            count: search_count,
            enabled: true,
            active: active == JiraSection::Search,
        },
        JiraTabInfo {
            section: JiraSection::Board,
            title: JiraSection::Board.title().to_string(),
            count: board_cards,
            enabled: true,
            active: active == JiraSection::Board,
        },
        JiraTabInfo {
            section: JiraSection::Ticket,
            title: JiraSection::Ticket.title().to_string(),
            count: usize::from(has_ticket),
            enabled: has_ticket,
            active: active == JiraSection::Ticket,
        },
    ]
}

/// Search-section rows: the free-text query then one row per set filter.
pub fn search_lines(panel: &JiraSearchPanel) -> Vec<JiraRowLine> {
    let mut out = Vec::new();
    let text = panel.text.trim();
    if !text.is_empty() {
        out.push(JiraRowLine::field("Text", text, "text"));
    }
    for row in &panel.rows {
        let value = if !row.selected.is_empty() {
            row.selected.join(", ")
        } else if let Some(v) = &row.value {
            v.clone()
        } else {
            row.text.clone()
        };
        let tone = if value.trim().is_empty() { "dim" } else { "text" };
        out.push(JiraRowLine::field(filter_title(&panel.kinds, &row.key), value, tone));
    }
    if out.is_empty() {
        out.push(placeholder("No search filters"));
    }
    out
}

/// Board-section rows: one heading per column, then its cards.
pub fn board_lines(board: &JiraBoardState) -> Vec<JiraRowLine> {
    let mut out = Vec::new();
    if board.columns.is_empty() {
        out.push(placeholder("No board columns"));
        return out;
    }
    for col in &board.columns {
        out.push(JiraRowLine::heading(format!(
            "{} ({})",
            col.name,
            col.cards.len() + col.more
        )));
        for card in &col.cards {
            let detail = [card.status.clone(), card.assignee.clone()]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" · ");
            let tone = if priority_is_hot(&card.priority) {
                "hot"
            } else if card.cat == 2 {
                "dim"
            } else {
                "text"
            };
            let mut line = JiraRowLine::field(format!("{}  {}", card.key, card.title), detail, tone);
            line.id = card.key.clone();
            out.push(line);
        }
        if col.more > 0 {
            out.push(placeholder(&format!("… {} more", col.more)));
        }
    }
    out
}

/// Ticket-section rows: the identity fields then the comments.
pub fn ticket_lines(ticket: &TicketPageModel) -> Vec<JiraRowLine> {
    let mut out = vec![
        field_row("key", &ticket.key, "accent"),
        field_row("title", &ticket.title, "text"),
        field_row(
            "status",
            &ticket.status,
            if ticket.category == 2 { "dim" } else { "text" },
        ),
        JiraRowLine::field(
            base_field_label("statusCategory").unwrap_or("Category"),
            category_name(ticket.category),
            "dim",
        ),
    ];
    out.push(JiraRowLine::heading(ticket.comments_tab_label()));
    if !ticket.comments_loaded {
        out.push(placeholder("Comments not loaded"));
    } else if ticket.comments.is_empty() {
        out.push(placeholder("No comments"));
    } else {
        for comment in &ticket.comments {
            out.push(JiraRowLine::field(
                comment.author.clone(),
                comment.body.clone(),
                "text",
            ));
        }
    }
    out
}

/// The ticket's workflow / comments breakdown for the detail pane.
pub fn ticket_detail_lines(ticket: &TicketPageModel) -> Vec<JiraRowLine> {
    let mut out = vec![JiraRowLine::heading(
        base_field_label("status").unwrap_or("Status"),
    )];
    let steps = ticket_step_lines(ticket);
    if steps.is_empty() {
        out.push(placeholder("No workflow"));
    } else {
        for step in steps {
            out.push(JiraRowLine::text(step, "text"));
        }
    }
    out.push(JiraRowLine::heading(ticket.comments_tab_label()));
    if !ticket.comments_loaded {
        out.push(placeholder("Comments not loaded"));
    } else if ticket.comments.is_empty() {
        out.push(placeholder("No comments"));
    } else {
        for comment in &ticket.comments {
            out.push(JiraRowLine::field(
                comment.author.clone(),
                comment.body.clone(),
                "text",
            ));
        }
    }
    out
}

/// The workflow stepper as text: the explicit `[jira] workflow` when present,
/// else the three status categories with the current one marked.
pub fn ticket_step_lines(ticket: &TicketPageModel) -> Vec<String> {
    if let Some(wf) = &ticket.workflow {
        if !wf.is_empty() {
            return wf.clone();
        }
    }
    if ticket.status.is_empty() {
        return Vec::new();
    }
    CATEGORY_NAMES
        .iter()
        .enumerate()
        .map(|(i, name)| {
            if i as i32 == ticket.category {
                format!("● {name}")
            } else {
                format!("○ {name}")
            }
        })
        .collect()
}

/// Resolve a row's tone name to a theme colour.
pub fn tone_color(colors: &PopupColors, tone: &str) -> Rgba {
    match tone {
        "dim" => colors.dim,
        "accent" => colors.accent_on(),
        "success" => colors.tone(PopupTone::Success),
        "warning" => colors.tone(PopupTone::Warning),
        "hot" | "danger" => colors.tone(PopupTone::Danger),
        _ => colors.text,
    }
}

/// The immutable snapshot the AppKit builder renders from.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct JiraTreeModel {
    pub tabs: Vec<JiraTabInfo>,
    pub active: JiraSection,
    pub search: Vec<JiraRowLine>,
    pub board: Vec<JiraRowLine>,
    pub ticket: Vec<JiraRowLine>,
    pub detail: Vec<JiraRowLine>,
}

impl JiraTreeModel {
    /// An empty dashboard (used by the embeddable `build_content` entry).
    pub fn empty() -> Self {
        JiraTreeModel {
            tabs: section_tabs(false, 0, 0),
            active: JiraSection::Search,
            search: vec![placeholder("No search filters")],
            board: vec![placeholder("No board columns")],
            ticket: Vec::new(),
            detail: vec![placeholder("No ticket selected")],
        }
    }

    /// Derive the snapshot from the controller's model state.
    pub fn from_parts(
        ticket: Option<&TicketPageModel>,
        board: &JiraBoardState,
        panel: &JiraSearchPanel,
    ) -> Self {
        let board_cards: usize = board.columns.iter().map(|c| c.cards.len() + c.more).sum();
        let search_count = panel.rows.len() + usize::from(!panel.text.trim().is_empty());
        let has_ticket = ticket.is_some();
        JiraTreeModel {
            tabs: section_tabs(has_ticket, board_cards, search_count),
            active: active_section(has_ticket, board_cards),
            search: search_lines(panel),
            board: board_lines(board),
            ticket: ticket.map(ticket_lines).unwrap_or_default(),
            detail: ticket
                .map(ticket_detail_lines)
                .unwrap_or_else(|| vec![placeholder("No ticket selected")]),
        }
    }

    /// The rows for one section.
    pub fn lines_for(&self, section: JiraSection) -> &[JiraRowLine] {
        match section {
            JiraSection::Search => &self.search,
            JiraSection::Board => &self.board,
            JiraSection::Ticket => &self.ticket,
        }
    }
}

// ===========================================================================
// View/controller skeleton
// ===========================================================================

pub struct JiraViewState {
    shown: bool,
    key: bool,
    frame: Option<RectI>,
    dashboard: JiraDashboardWindow,
    board: JiraBoardState,
    ticket: Option<TicketPageModel>,
    panel: JiraSearchPanel,
}

/// The `.jira` shared-window slot: one real daemon-backed view whose AppKit
/// NSView tree is built from the ported models by [`Self::build`].
pub struct JiraViewController {
    pub config: PopupConfig,
    state: RefCell<JiraViewState>,
    #[cfg(target_os = "macos")]
    content: RefCell<Option<Retained<NSView>>>,
}

impl Default for JiraViewController {
    fn default() -> Self {
        Self::new()
    }
}

impl JiraViewController {
    pub fn new() -> Self {
        JiraViewController {
            config: PopupConfig::new("jira"),
            state: RefCell::new(JiraViewState {
                shown: false,
                key: false,
                frame: None,
                dashboard: JiraDashboardWindow::new(),
                board: JiraBoardState::default(),
                ticket: None,
                panel: JiraSearchPanel::default(),
            }),
            #[cfg(target_os = "macos")]
            content: RefCell::new(None),
        }
    }

    pub fn set_key(&self, key: bool) {
        self.state.borrow_mut().key = key;
    }

    pub fn with_state<R>(&self, f: impl FnOnce(&JiraViewState) -> R) -> R {
        f(&self.state.borrow())
    }

    /// The immutable snapshot the AppKit builder renders from.
    pub fn tree_model(&self) -> JiraTreeModel {
        let s = self.state.borrow();
        JiraTreeModel::from_parts(s.ticket.as_ref(), &s.board, &s.panel)
    }

    /// Build the AppKit view tree from the current model state.
    ///
    /// No-ops off the main thread (the test threads) and builds at most once
    /// per controller. Rendering reads only the ported model state — it never
    /// spawns `jira_poll.py` or fetches anything.
    pub fn build(&self) {
        #[cfg(target_os = "macos")]
        {
            if self.content.borrow().is_some() {
                return;
            }
            let Some(mtm) = objc2::MainThreadMarker::new() else {
                return;
            };
            let model = self.tree_model();
            *self.content.borrow_mut() = Some(macos::build_tree(mtm, model));
        }
    }

    /// The built content view, when [`Self::build`] has run on the main thread.
    #[cfg(target_os = "macos")]
    pub fn content_view(&self) -> Option<Retained<NSView>> {
        self.content.borrow().clone()
    }

    /// `--live-search` through the CLI (criteria on stdin).
    pub fn run_live_search(&self, criteria: &Value) -> Result<Value, String> {
        let data = serde_json::to_vec(criteria).unwrap_or_default();
        run_jira_poll(&["--live-search"], Some(&data))
    }
}

impl SlotMember for JiraViewController {
    fn view(&self) -> SlotView {
        SlotView::Jira
    }
    fn shown(&self) -> bool {
        self.state.borrow().shown
    }
    fn is_key(&self) -> bool {
        self.state.borrow().key
    }
    fn frame(&self) -> Option<RectI> {
        self.state.borrow().frame
    }
    fn slot_show(&self, frame: Option<RectI>) {
        let mut s = self.state.borrow_mut();
        s.shown = true;
        if let Some(f) = frame {
            s.frame = Some(f);
        }
    }
    fn slot_park(&self, _stop_voice: bool) {
        let mut s = self.state.borrow_mut();
        s.shown = false;
        s.key = false;
    }
    fn test_state(&self) -> Value {
        let s = self.state.borrow();
        json!({
            "shown": s.shown,
            "key": s.key,
            "frame": s.frame,
            "board": s.board.test_state(),
            "ticket": s.ticket.as_ref().map(TicketPageModel::test_state),
            "searchRows": s.panel.rows.len(),
            "jobs": s.dashboard.jobs().len(),
        })
    }
}

/// Register the Jira palette entry (only while enabled, mirroring `[jira-config]`).
pub fn register(registry: &mut Registry, enabled: bool) {
    if enabled {
        registry.add_palette(crate::app::registry::PaletteCommand::new(
            "jira-config",
            "Jira Config Window",
            "views",
        ));
    }
}

/// The embeddable content view for the shared host window.
#[cfg(target_os = "macos")]
pub fn build_content(
    mtm: objc2::MainThreadMarker,
) -> Option<objc2::rc::Retained<objc2_app_kit::NSView>> {
    Some(macos::build_tree(mtm, JiraTreeModel::empty()))
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use crate::ui::theme::PopupThemeDefaults;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObject};
    use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly, Message};
    use objc2_app_kit::{
        NSAutoresizingMaskOptions, NSButton, NSButtonType, NSFont, NSScrollView, NSTextField, NSView,
    };
    use objc2_foundation::{NSInteger, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};
    use std::cell::{Cell, RefCell};

    fn as_any<T: Message + ?Sized>(obj: &T) -> &AnyObject {
        unsafe { &*(obj as *const T as *const AnyObject) }
    }

    /// A themed single-line label.
    fn label(
        mtm: MainThreadMarker,
        s: &str,
        size: f64,
        bold: bool,
        color: Rgba,
    ) -> Retained<NSTextField> {
        let l = NSTextField::labelWithString(&NSString::from_str(s), mtm);
        let font = if bold {
            NSFont::boldSystemFontOfSize(size)
        } else {
            NSFont::systemFontOfSize(size)
        };
        l.setFont(Some(&font));
        l.setTextColor(Some(&color.to_nscolor()));
        l
    }

    pub struct JiraFlippedViewIvars;

    define_class!(
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSJiraFlippedView"]
        #[ivars = JiraFlippedViewIvars]
        pub struct JiraFlippedView;

        impl JiraFlippedView {
            #[unsafe(method(isFlipped))]
            fn is_flipped(&self) -> bool {
                true
            }
        }

        unsafe impl NSObjectProtocol for JiraFlippedView {}
    );

    impl JiraFlippedView {
        pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(JiraFlippedViewIvars);
            unsafe {
                msg_send![
                    super(this),
                    initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0))
                ]
            }
        }
    }

    pub struct JiraHandlerIvars {
        pub scroll: Retained<NSScrollView>,
        pub rows_view: Retained<NSView>,
        pub detail_view: Retained<NSView>,
        pub colors: PopupColors,
        pub tabs: RefCell<Vec<JiraTabInfo>>,
        pub buttons: RefCell<Vec<Retained<NSButton>>>,
        pub active: Cell<JiraSection>,
        pub search: Vec<JiraRowLine>,
        pub board: Vec<JiraRowLine>,
        pub ticket: Vec<JiraRowLine>,
        pub detail: Vec<JiraRowLine>,
    }

    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSJiraHandler"]
        #[ivars = JiraHandlerIvars]
        pub struct JiraHandler;

        impl JiraHandler {
            #[unsafe(method(selectSection:))]
            fn select_section(&self, sender: Option<&AnyObject>) {
                let tag: NSInteger = sender
                    .map(|s| unsafe { msg_send![as_any(s), tag] })
                    .unwrap_or(0);
                let Some(section) = JiraSection::from_tag(tag as i64) else {
                    return;
                };
                let enabled = self
                    .ivars()
                    .tabs
                    .borrow()
                    .iter()
                    .any(|t| t.section == section && t.enabled);
                if !enabled {
                    return;
                }
                self.ivars().active.set(section);
                self.sync_tabs();
                self.rebuild();
            }
        }

        unsafe impl NSObjectProtocol for JiraHandler {}
    );

    impl JiraHandler {
        fn new(
            mtm: MainThreadMarker,
            model: JiraTreeModel,
            scroll: Retained<NSScrollView>,
            rows_view: Retained<NSView>,
            detail_view: Retained<NSView>,
        ) -> Retained<Self> {
            let active = model.active;
            let this = Self::alloc(mtm).set_ivars(JiraHandlerIvars {
                scroll,
                rows_view,
                detail_view,
                colors: PopupThemeDefaults::colors(),
                tabs: RefCell::new(model.tabs),
                buttons: RefCell::new(Vec::new()),
                active: Cell::new(active),
                search: model.search,
                board: model.board,
                ticket: model.ticket,
                detail: model.detail,
            });
            unsafe { msg_send![super(this), init] }
        }

        pub fn set_buttons(&self, buttons: Vec<Retained<NSButton>>) {
            *self.ivars().buttons.borrow_mut() = buttons;
            self.sync_tabs();
        }

        /// Tint the tab captions for the active section.
        pub fn sync_tabs(&self) {
            let active = self.ivars().active.get();
            let colors = self.ivars().colors;
            let buttons = self.ivars().buttons.borrow();
            for (i, button) in buttons.iter().enumerate() {
                let section = match i {
                    0 => JiraSection::Search,
                    1 => JiraSection::Board,
                    _ => JiraSection::Ticket,
                };
                let color = if section == active {
                    colors.accent_on()
                } else {
                    colors.dim
                };
                button.setContentTintColor(Some(&color.to_nscolor()));
            }
        }

        fn lines(&self, section: JiraSection) -> Vec<JiraRowLine> {
            match section {
                JiraSection::Search => self.ivars().search.clone(),
                JiraSection::Board => self.ivars().board.clone(),
                JiraSection::Ticket => self.ivars().ticket.clone(),
            }
        }

        /// Replace the list rows for the active section.
        pub fn rebuild(&self) {
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };
            let rows_view = &self.ivars().rows_view;
            for sub in rows_view.subviews().iter() {
                sub.removeFromSuperview();
            }
            let width = self
                .ivars()
                .scroll
                .contentSize()
                .width
                .max(JIRA_LIST_MIN_WIDTH);
            let colors = self.ivars().colors;
            let lines = self.lines(self.ivars().active.get());
            let mut y = JIRA_ROW_PAD;
            for line in &lines {
                let color = tone_color(&colors, line.tone);
                let text = label(
                    mtm,
                    &line.text,
                    if line.heading { 11.0 } else { 12.5 },
                    line.heading,
                    color,
                );
                text.setFrame(NSRect::new(
                    NSPoint::new(JIRA_ROW_LEFT, y + 2.0),
                    NSSize::new((width - JIRA_ROW_LEFT * 2.0).max(1.0), 18.0),
                ));
                rows_view.addSubview(&text);
                if !line.detail.is_empty() {
                    let detail = label(mtm, &line.detail, 11.0, false, colors.dim);
                    detail.setFrame(NSRect::new(
                        NSPoint::new(JIRA_ROW_LEFT + 12.0, y + 20.0),
                        NSSize::new((width - JIRA_ROW_LEFT * 2.0 - 12.0).max(1.0), 16.0),
                    ));
                    rows_view.addSubview(&detail);
                }
                y += row_height(line);
            }
            let total = rows_total_height(&lines);
            rows_view.setFrame(NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(width, total.max(1.0)),
            ));
            self.rebuild_detail(mtm);
        }

        fn rebuild_detail(&self, mtm: MainThreadMarker) {
            let detail_view = &self.ivars().detail_view;
            for sub in detail_view.subviews().iter() {
                sub.removeFromSuperview();
            }
            let width = detail_view.bounds().size.width.max(JIRA_DETAIL_WIDTH);
            let colors = self.ivars().colors;
            let mut y = JIRA_ROW_PAD;
            for line in &self.ivars().detail {
                let color = tone_color(&colors, line.tone);
                let text = label(
                    mtm,
                    &line.text,
                    if line.heading { 11.0 } else { 12.0 },
                    line.heading,
                    color,
                );
                text.setFrame(NSRect::new(
                    NSPoint::new(JIRA_ROW_LEFT, y + 2.0),
                    NSSize::new((width - JIRA_ROW_LEFT * 2.0).max(1.0), 18.0),
                ));
                detail_view.addSubview(&text);
                if !line.detail.is_empty() {
                    let detail = label(mtm, &line.detail, 11.0, false, colors.dim);
                    detail.setFrame(NSRect::new(
                        NSPoint::new(JIRA_ROW_LEFT + 12.0, y + 20.0),
                        NSSize::new((width - JIRA_ROW_LEFT * 2.0 - 12.0).max(1.0), 16.0),
                    ));
                    detail_view.addSubview(&detail);
                }
                y += row_height(line);
            }
        }
    }

    thread_local! {
        static LIVE_HANDLERS: RefCell<Vec<Retained<JiraHandler>>>
            = const { RefCell::new(Vec::new()) };
    }

    /// Build the full dashboard tree and keep the target/action handler alive.
    pub fn build_tree(mtm: MainThreadMarker, model: JiraTreeModel) -> Retained<NSView> {
        let colors = PopupThemeDefaults::colors();
        let root = JiraFlippedView::new(mtm);
        root.setFrame(NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(JIRA_DEFAULT_WIDTH, JIRA_DEFAULT_HEIGHT),
        ));
        root.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );

        // Header bar, pinned to the visual top of the flipped root.
        let header = JiraFlippedView::new(mtm);
        header.setFrame(NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(JIRA_DEFAULT_WIDTH, JIRA_HEADER_HEIGHT),
        ));
        header.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewMaxYMargin,
        );
        root.addSubview(&header);

        let title = label(mtm, "Jira", 15.0, true, colors.text);
        title.setFrame(NSRect::new(
            NSPoint::new(JIRA_ROW_LEFT, 12.0),
            NSSize::new(56.0, 20.0),
        ));
        header.addSubview(&title);

        // Main split: flexible list on the left, fixed-width detail on the right.
        let content_w = JIRA_DEFAULT_WIDTH;
        let content_h = JIRA_DEFAULT_HEIGHT - JIRA_HEADER_HEIGHT;
        let list_w = (content_w - JIRA_DETAIL_WIDTH).max(JIRA_LIST_MIN_WIDTH);

        let scroll = NSScrollView::new(mtm);
        scroll.setFrame(NSRect::new(
            NSPoint::new(0.0, JIRA_HEADER_HEIGHT),
            NSSize::new(list_w, content_h),
        ));
        scroll.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        scroll.setHasVerticalScroller(true);
        scroll.setDrawsBackground(true);
        scroll.setBackgroundColor(&colors.mantle().to_nscolor());
        let rows_view = JiraFlippedView::new(mtm);
        rows_view.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        rows_view.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(list_w, 1.0)));
        scroll.setDocumentView(Some(&rows_view));
        root.addSubview(&scroll);

        let detail_view = JiraFlippedView::new(mtm);
        detail_view.setFrame(NSRect::new(
            NSPoint::new(content_w - JIRA_DETAIL_WIDTH, JIRA_HEADER_HEIGHT),
            NSSize::new(JIRA_DETAIL_WIDTH, content_h),
        ));
        detail_view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        root.addSubview(&detail_view);

        let tabs = model.tabs.clone();
        let handler = JiraHandler::new(
            mtm,
            model,
            scroll.clone(),
            rows_view.into_super(),
            detail_view.into_super(),
        );

        // Header tabs, now that the handler exists to serve as their target.
        let mut buttons: Vec<Retained<NSButton>> = Vec::new();
        let mut x = 72.0;
        for tab in &tabs {
            let button = unsafe {
                NSButton::buttonWithTitle_target_action(
                    &NSString::from_str(&tab_label(tab)),
                    Some(as_any(&*handler)),
                    Some(objc2::sel!(selectSection:)),
                    mtm,
                )
            };
            button.setTag(tab.section.tag() as NSInteger);
            button.setButtonType(NSButtonType::MomentaryPushIn);
            button.setBordered(false);
            button.setFont(Some(&NSFont::systemFontOfSize(12.5)));
            button.setEnabled(tab.enabled);
            button.sizeToFit();
            let w = button.frame().size.width.max(1.0);
            button.setFrame(NSRect::new(
                NSPoint::new(x, (JIRA_HEADER_HEIGHT - 20.0) / 2.0),
                NSSize::new(w, 20.0),
            ));
            header.addSubview(&button);
            x += w + 10.0;
            buttons.push(button);
        }
        handler.set_buttons(buttons);

        LIVE_HANDLERS.with(|v| v.borrow_mut().push(handler.clone()));
        handler.rebuild();

        root.into_super()
    }
}

// ===========================================================================
// Tests — mirroring Tests/test_jira_fields|data|search|dashboard|boards.py
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn col() -> ListColumn {
        ListColumn {
            field: "a".into(),
            title: "T".into(),
            width: 120.0,
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
        assert_eq!(serialize_columns(&[col()], true), "a:T:120:left");
        let mut c = col();
        c.width = 120.5;
        assert_eq!(serialize_columns(&[c.clone()], true), "a:T:120.5:left");
        c.width = 0.0;
        assert_eq!(serialize_columns(&[c], true), "a:T:0:left");
        assert_eq!(serialize_columns(&[col()], false), "a::120:left");
        let mut c = col();
        c.sortable = true;
        c.filterable = true;
        assert_eq!(serialize_columns(&[c], true), "a:T:120:left:filter+sort");
        let mut c = col();
        c.sortable = true;
        assert_eq!(serialize_columns(&[c], true), "a:T:120:left:sort");
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

    #[test]
    fn merged_labels_custom_fields_and_renames() {
        assert_eq!(merged_labels(None).get("key").cloned(), Some("Key".to_string()));
        let team = json!({
            "custom_fields": {
                "story_points": {"field_id": "customfield_1", "label": "Points"},
                "empty": {"label": ""},
                "bare": {"field_id": "customfield_2"},
            }
        });
        let out = merged_labels(Some(&team));
        assert_eq!(out.get("story_points"), Some(&"Points".to_string()));
        assert_eq!(out.get("empty"), Some(&"empty".to_string()));
        assert_eq!(out.get("bare"), Some(&"bare".to_string()));

        let team = json!({
            "Custom Fields": {"a": {"label": "A"}},
            "custom-fields": {"b": {"label": "B"}},
            "field_labels": {"alias": "  Custom  ", "key": "Ticket"},
        });
        let out = merged_labels(Some(&team));
        assert_eq!(out.get("a"), Some(&"A".to_string()));
        assert_eq!(out.get("b"), Some(&"B".to_string()));
        assert_eq!(out.get("alias"), Some(&"Custom".to_string()));
        assert_eq!(out.get("key"), Some(&"Ticket".to_string()));

        let team = json!({"field_labels": {"key": "Ticket", "bad": 7}});
        assert_eq!(merged_labels(Some(&team)).get("key"), Some(&"Key".to_string()));
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
    fn workflow_steps_split() {
        assert_eq!(workflow_steps(None), None);
        assert_eq!(workflow_steps(Some("")), None);
        assert_eq!(workflow_steps(Some("  , , ")), None);
        assert_eq!(
            workflow_steps(Some("To Do, In Progress , Done")),
            Some(vec!["To Do".to_string(), "In Progress".to_string(), "Done".to_string()])
        );
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

    #[test]
    fn comments_cache_and_parsing() {
        let dir = tmpdir("comments");
        let path = format!("{dir}/issues.json");
        std::fs::write(&path, r#"{"A-1":{"comments":[{"author":"ada","body":"hi","created":"2026-01-01T00:00:00Z"},{"author":"bob","body":"yo","created":"2026-01-02T00:00:00Z"}]},"A-2":{"comments":[{"author":"x","body":"y","created":"z"}]}}"#).unwrap();
        let out = comments(&path, "A-1");
        assert!(out.stamp > 0.0);
        assert_eq!(out.comments[0].author, "ada");
        assert_eq!(out.comments[1].author, "bob");
        assert_eq!(comments(&path, "A-2").comments[0].author, "x");

        std::fs::write(&path, r#"{"A-1":{"comments":[]},"A-2":{},"A-3":"not a dict"}"#).unwrap();
        assert!(comments(&path, "A-1").comments.is_empty());
        assert!(comments(&path, "A-2").comments.is_empty());
        assert!(comments(&path, "nope").comments.is_empty());

        std::fs::write(&path, r#"{"A-1":{"comments":[{"author":7,"body":null,"created":["x"]}]}}"#).unwrap();
        let c = &comments(&path, "A-1").comments[0];
        assert_eq!((c.author.as_str(), c.body.as_str(), c.created.as_str()), ("", "", ""));

        std::fs::write(&path, r#"{"A-1":{"comments":["nope"]}}"#).unwrap();
        assert!(comments(&path, "A-1").comments.is_empty());

        std::fs::write(&path, "{nope").unwrap();
        assert!(comments(&path, "A-1").comments.is_empty());

        let missing = format!("{dir}/none.json");
        let out = comments(&missing, "A-1");
        assert_eq!(out.stamp, 0.0);
        assert!(out.comments.is_empty());
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

    #[test]
    fn search_panel_state_round_trip() {
        let mut panel = JiraSearchPanel::new(vec![json!({"field": "customfield_1", "label": "Points"})]);
        panel.text = "retry".into();
        panel.projects = vec!["A".into()];
        let mut row = SearchRow::new("status", ValueKind::List);
        row.selected = vec!["Done".into()];
        panel.rows.push(row);
        let mut choice = SearchRow::new("updated", ValueKind::Date);
        choice.value = Some(String::new());
        panel.rows.push(choice);

        let dir = tmpdir("search");
        let path = format!("{dir}/last.json");
        save_last_criteria(&path, &panel).unwrap();
        let loaded = load_last_criteria(&path).unwrap();
        assert_eq!(loaded.text, "retry");
        assert_eq!(loaded.projects, vec!["A".to_string()]);
        assert_eq!(loaded.rows.len(), 2);
        assert_eq!(loaded.rows[0].selected, vec!["Done".to_string()]);
        assert_eq!(loaded.rows[1].value, Some(String::new()));
        assert_eq!(loaded.criteria(), panel.criteria());
    }

    // -------------------------------------------------------- jira_dashboard

    #[test]
    fn snake_case_cases() {
        assert_eq!(snake_case("Story Points"), "story_points");
        assert_eq!(snake_case("  Type-of  thing__x "), "type_of_thing_x");
        assert_eq!(snake_case("1st Place"), "f_1st_place");
        assert_eq!(snake_case("already_ok"), "already_ok");
        assert_eq!(snake_case(""), "");
    }

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

    #[test]
    fn check_limit_semantics() {
        assert_eq!(check_limit("", "Page size"), LimitResult { ok: true, value: 0, message: None });
        assert_eq!(check_limit("   ", "Page size"), LimitResult { ok: true, value: 0, message: None });
        assert_eq!(check_limit(" 25 ", "Page size").value, 25);
        assert_eq!(check_limit("+3", "Page size").value, 3);
        assert_eq!(check_limit("0", "Page size").value, 0);
        for bad in ["x", "3.5", "-1", "1 000"] {
            let out = check_limit(bad, "Max issues");
            assert!(!out.ok, "{bad}");
            assert_eq!(
                out.message.as_deref(),
                Some("✗ Max issues must be a whole number (empty = default)")
            );
        }
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

    #[test]
    fn draft_basic_and_projects() {
        let out = draft(&draft_params(json!({})));
        assert_eq!(out["ok"], true);
        assert_eq!(out["draft"]["name"], "team-bugs");
        assert_eq!(out["draft"]["maxResults"], 50);
        assert_eq!(out["draft"]["maxTotal"], 500);
        assert_eq!(out["draft"]["window"], "30m");
        assert_eq!(out["draft"]["projects"], json!(["A"]));
        assert_eq!(out["draft"]["jql"], "");
        assert_eq!(out["draft"]["args"], json!({}));

        assert_eq!(draft(&draft_params(json!({"projectsAll": true})))["draft"]["projects"], "*");
        assert_eq!(draft(&draft_params(json!({"projects": []})))["draft"]["projects"], "*");
    }

    #[test]
    fn draft_directory_and_jql_and_team_job() {
        let out = draft(&draft_params(json!({"type": "directory"})));
        assert!(out["draft"].get("columns").is_none());

        let out = draft(&draft_params(json!({"queryIndex": 1, "jql": " x "})));
        assert_eq!(out["draft"]["jql"], "x");
        let out = draft(&draft_params(json!({"queryIndex": 1, "jql": " x ", "type": "directory"})));
        assert_eq!(out["draft"]["jql"], "");

        let out = draft(&draft_params(
            json!({"queryIndex": 2, "jobTitle": "team.json: night-audit", "args": " days=30,label=ops"}),
        ));
        assert_eq!(out["draft"]["job"], "night-audit");
        assert_eq!(out["draft"]["args"], json!({"days": "30", "label": "ops"}));
        let out = draft(&draft_params(json!({"queryIndex": 2, "jobTitle": "ab"})));
        assert_eq!(out["draft"]["job"], "");
    }

    #[test]
    fn draft_limit_failure_short_circuits() {
        let out = draft(&draft_params(json!({"pageSize": "nope"})));
        assert_eq!(out["ok"], false);
        assert!(out["message"].as_str().unwrap().contains("Page size"));
        assert!(out.get("draft").is_none());
    }

    #[test]
    fn live_persist_parses_limit() {
        assert_eq!(
            live_persist(&json!({"maxResults": "25", "columns": "key,status"})),
            json!({"ok": true, "draft": {"columns": "key,status", "maxResults": 25}})
        );
        assert_eq!(live_persist(&json!({"maxResults": "-2"}))["ok"], false);
    }

    #[test]
    fn custom_field_entry_cases() {
        let base = json!({
            "raw": "Story Points — customfield_10016", "alias": "", "isNew": true, "label": "",
            "description": "", "customs": [{"id": "customfield_10016", "name": "Story Points"}],
            "currentCustomFields": {}, "currentLabels": {}
        });
        let out = custom_field_entry(&base);
        assert_eq!(out["alias"], "story_points");
        assert_eq!(
            out["save"]["value"]["story_points"],
            json!({"field_id": "customfield_10016", "label": "Story Points"})
        );
        assert_eq!(out["save"]["done"], "added story_points");
        assert_eq!(out["followup"], Value::Null);

        let mut m = base.clone();
        m["raw"] = json!("story points");
        assert_eq!(
            custom_field_entry(&m)["save"]["value"]["story_points"]["field_id"],
            "customfield_10016"
        );

        let mut m = base.clone();
        m["raw"] = json!("nope");
        m["customs"] = json!([]);
        let out = custom_field_entry(&m);
        assert_eq!(out["ok"], false);
        assert!(out["message"].as_str().unwrap().contains("customfield_NNNNN"));

        let mut m = base.clone();
        m["raw"] = json!("customfield_9");
        m["customs"] = json!([]);
        m["alias"] = json!("");
        m["label"] = json!("My Custom Label");
        assert_eq!(custom_field_entry(&m)["alias"], "my_custom_label");
        assert_eq!(
            custom_field_entry(&m)["save"]["value"]["my_custom_label"]["label"],
            "My Custom Label"
        );

        let mut m = base.clone();
        m["raw"] = json!("customfield_9");
        m["customs"] = json!([]);
        m["alias"] = json!("given");
        m["label"] = json!("");
        assert_eq!(
            custom_field_entry(&m)["save"]["value"]["given"],
            json!({"field_id": "customfield_9", "label": "given"})
        );

        let mut m = base.clone();
        m["alias"] = json!("sp");
        m["isNew"] = json!(false);
        m["description"] = json!(" points ");
        assert_eq!(custom_field_entry(&m)["save"]["value"]["sp"]["description"], "points");
        assert_eq!(custom_field_entry(&m)["save"]["done"], "updated sp");

        let mut m = base.clone();
        m["alias"] = json!("sp");
        m["isNew"] = json!(false);
        m["currentLabels"] = json!({"sp": "x", "k": "y"});
        let out = custom_field_entry(&m);
        assert_eq!(out["followup"]["value"], json!({"k": "y"}));
        assert_eq!(out["followup"]["done"], "label of sp");
    }

    #[test]
    fn field_label_and_default_and_key_value() {
        assert_eq!(
            field_label_save(&json!({"field": "title", "value": " Heading ", "default": "Title", "current": {"k": "v"}})),
            json!({"ok": true, "value": {"k": "v", "title": "Heading"}, "done": "title → “Heading”"})
        );
        for value in ["", "   ", "Title"] {
            let out = field_label_save(&json!({"field": "title", "value": value, "default": "Title", "current": {"title": "Old"}}));
            assert_eq!(out["value"], json!({}), "{value}");
            assert_eq!(out["done"], "title back to “Title”");
        }
        assert_eq!(
            key_value_save(&json!({"name": " my_bugs ", "value": " p in ({projects}) ", "existing": false, "current": {}})),
            json!({"ok": true, "value": {"my_bugs": "p in ({projects})"}, "done": "added my_bugs"})
        );
        assert_eq!(
            key_value_save(&json!({"name": "my_bugs", "value": "x", "existing": true, "current": {}}))["done"],
            "updated my_bugs"
        );
        assert_eq!(key_value_save(&json!({"name": " ", "value": "x"})), json!({"ok": false, "beep": true}));
        assert_eq!(key_value_save(&json!({"name": "x", "value": " "})), json!({"ok": false, "beep": true}));

        assert_eq!(
            default_save(&json!({"key": "maxResults", "value": " 25 ", "current": {"k": 1}})),
            json!({"ok": true, "value": {"k": 1, "maxResults": 25}, "done": "maxResults = 25"})
        );
        for bad in ["x", "-1", "3.5", ""] {
            let out = default_save(&json!({"key": "pageSize", "value": bad}));
            assert_eq!(out["ok"], false, "{bad}");
            assert_eq!(out["message"], "✗ pageSize must be a whole number");
        }
    }

    #[test]
    fn progress_text_and_header_state() {
        let now = 1_000_000.0;
        assert_eq!(
            progress_text(
                Some(&json!({"stage": "sync", "waitingUntil": now + 90.0, "reason": "rate limit"})),
                Some(now)
            ),
            "sync: rate limit — resuming in 1m 30s"
        );
        assert_eq!(
            progress_text(Some(&json!({"waitingUntil": now + 45.0, "reason": "cooling"})), Some(now)),
            "cooling — resuming in 45s"
        );
        assert_eq!(
            progress_text(
                Some(&json!({"stage": "sync", "waitingUntil": now - 5.0, "message": "downloading"})),
                Some(now)
            ),
            "downloading"
        );
        assert_eq!(progress_text(Some(&json!({"message": "downloading"})), Some(now)), "downloading");
        assert_eq!(progress_text(Some(&json!({})), Some(now)), "");
        assert_eq!(progress_text(None, Some(now)), "");

        fn params(over: Value) -> Value {
            let mut p = json!({
                "enabled": true, "background": false, "setupDone": true, "scopeEmpty": false,
                "lockHeld": false, "failed": false, "lastRun": "", "lastRunShort": "never",
                "progress": {}, "epsCount": 3, "configPath": "/c.json", "tick": "60s",
                "status": "ok", "lockPid": "42", "lockSinceShort": "10:00",
                "problems": [], "lastError": "", "enableError": ""
            });
            if let (Some(base), Some(add)) = (p.as_object_mut(), over.as_object()) {
                for (k, v) in add {
                    base.insert(k.clone(), v.clone());
                }
            }
            p
        }
        let out = header_state(&params(json!({"enabled": false})));
        assert_eq!(out["line"], "○ Polling off");
        assert_eq!(out["tone"], "dim");
        assert_eq!(out["enableTitle"], "Enable Jira");
        assert_eq!(out["enablePrimary"], true);
        assert_eq!(
            header_state(&params(json!({"enabled": false, "background": true})))["line"],
            "◐ Polling in the background (Jira window off)"
        );
        assert_eq!(
            header_state(&params(json!({"setupDone": false, "scopeEmpty": true})))["line"],
            "● Polling on — enter the projects in scope (Setup)"
        );
        assert_eq!(
            header_state(&params(json!({"setupDone": false, "scopeEmpty": false})))["line"],
            "● Polling on — setup not finished (Setup)"
        );
        assert_eq!(
            header_state(&params(json!({"lockHeld": true, "progress": {"message": "downloading"}})))["line"],
            "● Polling on — downloading"
        );
        assert_eq!(
            header_state(&params(json!({"lockHeld": true})))["line"],
            "● Polling on — polling now…"
        );
        assert_eq!(
            header_state(&params(json!({"lastRun": "2026-01-01T10:00:00Z", "lastRunShort": "10:00", "failed": true})))["line"],
            "● Polling on — last poll failed 10:00"
        );
        assert_eq!(
            header_state(&params(json!({"lastRun": "x", "lastRunShort": "10:00"})))["line"],
            "● Polling on — last checked 10:00"
        );
        assert_eq!(
            header_state(&params(json!({"lastRun": "x", "lastRunShort": "10:00", "failed": true})))["tone"],
            "warn"
        );
        let out = header_state(&params(json!({"lastRun": "raw", "lockHeld": true, "problems": ["p1"], "lastError": "e", "enableError": "nope"})));
        assert_eq!(
            out["tip"],
            json!([
                "3 poll jobs in /c.json",
                "launchd tick: 60s — each job runs when its own interval is due",
                "last run raw ok",
                "polling now: pid 42 since 10:00"
            ])
        );
        assert_eq!(out["problems"], json!(["p1", "last error: e", "enable failed: nope"]));
        assert_eq!(header_state(&params(json!({"epsCount": 1})))["tip"][0], "1 poll job in /c.json");
        assert_eq!(header_state(&params(json!({"enabled": true})))["enableTitle"], "Disable Jira");
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
            vec!["Ready", "Doing", OTHER_COLUMN]
        );
        assert_eq!(out[2].cards[0].key, "A-3");
    }

    #[test]
    fn board_columns_done_clamp() {
        let rows: Vec<Value> = (0..31).map(|i| board_row(&format!("D-{i:02}"), "Done")).collect();
        let out = board_columns(&board_params(json!(rows), json!({})));
        assert_eq!(out[2].cards.len(), DONE_LIMIT);
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

    #[test]
    fn ticket_lifecycle_categories() {
        let w = words(&json!({}));
        let cats: Map<String, Value> =
            serde_json::from_value(json!({"Reviewing": "indeterminate"})).unwrap();
        let steps = lifecycle_steps("Reviewing", &cats, &w, None);
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[0].name, "To Do");
        assert_eq!(steps[0].state, StepState::Done);
        assert_eq!(steps[1].state, StepState::Current);
        assert_eq!(steps[1].sub.as_deref(), Some("Reviewing"));
        assert_eq!(steps[2].state, StepState::Pending);

        assert!(lifecycle_steps("", &Map::new(), &w, None).is_empty());
    }

    #[test]
    fn ticket_lifecycle_workflow_wins() {
        let w = words(&json!({}));
        let steps = lifecycle_steps("In Progress", &Map::new(), &w, Some("To Do, In Progress , Done"));
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[0].state, StepState::Done);
        assert_eq!(steps[1].state, StepState::Current);
        assert_eq!(steps[2].state, StepState::Pending);
        assert!(steps[1].sub.is_none());
    }

    #[test]
    fn ticket_page_model_comments_label() {
        let w = words(&json!({}));
        let fields = json!({"key": "A-1", "title": "Retry bug", "status": "Done"});
        let mut page = TicketPageModel::from_fields(&fields, &Map::new(), &w, None);
        assert_eq!(page.key, "A-1");
        assert_eq!(page.category, 2);
        assert_eq!(
            page.tabs,
            vec![TicketTab::Details, TicketTab::Comments, TicketTab::AllFields]
        );
        assert_eq!(page.comments_tab_label(), "Comments");
        page.set_comments(vec![Comment {
            author: "ada".into(),
            body: "hi".into(),
            created: "c".into(),
        }]);
        assert_eq!(page.comments_tab_label(), "Comments 1");
        assert_eq!(page.test_state()["commentsLoaded"], true);
    }

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
        assert_eq!(job.columns[0].to_list_column().field, "key");
        assert_eq!(d.catalog[0].base, true);
    }

    #[test]
    fn column_editor_validation_and_moves() {
        let mut ed = ColumnEditor::new(parse_columns("key, title, status"));
        let mut c = col();
        c.field = "assignee".into();
        assert_eq!(ed.upsert(None, c.clone()).unwrap(), 3);

        let mut dup = col();
        dup.field = "assignee".into();
        assert_eq!(ed.upsert(None, dup), Err(ColumnEditError::Duplicate));

        let mut bad = col();
        bad.field = " : , ".into();
        assert_eq!(ed.upsert(None, bad), Err(ColumnEditError::EmptyField));

        // sanitize: `a:b` -> `ab`
        let mut odd = col();
        odd.field = "a:b".into();
        odd.width = 200.0;
        let i = ed.upsert(None, odd).unwrap();
        assert_eq!(ed.cols[i].field, "ab");
        assert_eq!(ed.cols[i].width, 100.0);
        assert_eq!(ed.cols[i].title, "");

        assert_eq!(ed.move_by(i, -1), Some(i - 1));
        assert_eq!(ed.move_by(0, -1), None);
        assert!(ed.remove(i - 1));
        assert!(!ed.remove(100));
    }

    #[test]
    fn column_editor_field_names_and_api_text() {
        let mut ed = ColumnEditor::new(vec![]);
        ed.meta.insert(
            "customfield_1".into(),
            json!({"label": "Points", "apiFields": ["customfield_1"]}),
        );
        ed.meta.insert(
            "title".into(),
            json!({"apiFields": ["summary"]}),
        );
        assert_eq!(ed.field_name("customfield_1"), "Points");
        assert_eq!(ed.field_name("key"), "Key");
        assert_eq!(ed.field_name("zzz"), "zzz");
        assert_eq!(ed.api_text("customfield_1"), "");
        assert_eq!(ed.api_text("title"), "summary");
        assert_eq!(ed.api_text("zzz"), "");
    }

    // -------------------------------------------------------- helper parity

    #[test]
    fn helper_parity_when_present() {
        if !helper_ready() {
            eprintln!("skipping: no python 3.11+ or pylib/helper not found");
            return;
        }

        let spec = "key:Key:80:left:filter, status:Status:120:right:filter+sort, epic";
        assert_eq!(helper_parse_columns(spec).unwrap(), parse_columns(spec));
        assert_eq!(
            helper_serialize_columns(&parse_columns(spec), true).unwrap(),
            serialize_columns(&parse_columns(spec), true)
        );

        let kinds = filter_kinds(&[json!({"field": "customfield_100", "label": "Story Points"})]);
        assert_eq!(helper_filter_kinds(&[json!({"field": "customfield_100", "label": "Story Points"})]).unwrap(), kinds);

        let params = json!({
            "text": "timeout",
            "projects": ["APP"],
            "rows": [
                {"key": "assignee", "selected": ["currentUser()"]},
                {"key": "field:description", "text": "retry"},
            ],
            "maxResults": "25",
        });
        assert_eq!(helper_criteria(&params).unwrap(), criteria(&params));

        let bp = json!({
            "rows": [board_row("A-1", "Done"), board_row("A-2", "Backlog")],
            "columns": [],
            "categories": {},
            "words": words(&json!({})),
            "people": {},
            "categoryNames": ["To Do", "In Progress", "Done"],
        });
        assert_eq!(helper_board_columns(&bp).unwrap(), board_columns(&bp));
    }

    // --------------------------------------------------- view model (AppKit-free)

    fn ticket_model() -> TicketPageModel {
        TicketPageModel {
            key: "PROJ-1".into(),
            title: "Do the thing".into(),
            status: "In Progress".into(),
            category: 1,
            tabs: vec![TicketTab::Details, TicketTab::Comments, TicketTab::AllFields],
            comments: vec![Comment {
                author: "alice".into(),
                body: "hi".into(),
                created: "2026-01-01".into(),
            }],
            comments_loaded: true,
            workflow: None,
        }
    }

    fn board_state() -> JiraBoardState {
        JiraBoardState {
            columns: vec![BoardColumn {
                name: "To Do".into(),
                cards: vec![BoardCard {
                    key: "A-1".into(),
                    title: "Fix".into(),
                    type_: "Bug".into(),
                    priority: "Highest".into(),
                    assignee: "me".into(),
                    status: "Backlog".into(),
                    cat: 0,
                }],
                more: 2,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn section_tag_roundtrip() {
        for section in JiraSection::all() {
            assert_eq!(JiraSection::from_tag(section.tag()), Some(section));
        }
        assert_eq!(JiraSection::from_tag(9), None);
        assert_eq!(JiraSection::default(), JiraSection::Search);
    }

    #[test]
    fn section_tabs_track_model_state() {
        let empty = section_tabs(false, 0, 0);
        assert_eq!(empty.len(), 3);
        assert!(empty[0].active, "empty dashboard opens on search");
        assert!(empty[0].enabled && empty[1].enabled);
        assert!(!empty[2].enabled, "no ticket disables the ticket tab");

        let with_ticket = section_tabs(true, 5, 2);
        assert!(with_ticket[2].active, "a selected ticket wins");
        assert!(with_ticket[2].enabled);
        assert_eq!(with_ticket[1].count, 5);
        assert_eq!(with_ticket[0].count, 2);

        let with_board = section_tabs(false, 5, 0);
        assert!(with_board[1].active);
        assert!(!with_board[2].active);
    }

    #[test]
    fn tab_label_appends_nonzero_count() {
        let tab = |count| JiraTabInfo {
            section: JiraSection::Board,
            title: "Board".into(),
            count,
            enabled: true,
            active: false,
        };
        assert_eq!(tab_label(&tab(3)), "Board 3");
        assert_eq!(tab_label(&tab(0)), "Board");
    }

    #[test]
    fn row_height_and_total() {
        assert_eq!(row_height(&JiraRowLine::heading("H")), JIRA_HEADING_HEIGHT);
        assert_eq!(row_height(&JiraRowLine::text("t", "text")), JIRA_ROW_HEIGHT);
        assert_eq!(
            row_height(&JiraRowLine::field("t", "detail", "text")),
            JIRA_ROW_TWO_LINE_HEIGHT
        );
        assert_eq!(rows_total_height(&[]), JIRA_ROW_PAD);
        assert_eq!(
            rows_total_height(&[JiraRowLine::text("a", "text"), JiraRowLine::text("b", "text")]),
            JIRA_ROW_PAD + JIRA_ROW_HEIGHT * 2.0
        );
    }

    #[test]
    fn priority_hot_words() {
        assert!(priority_is_hot("Highest"));
        assert!(priority_is_hot("BLOCKER"));
        assert!(!priority_is_hot("low"));
    }

    #[test]
    fn search_lines_render_query_and_rows() {
        let mut panel = JiraSearchPanel::new(Vec::new());
        panel.text = "timeout".into();
        panel.rows.push(SearchRow {
            key: "assignee".into(),
            selected: vec!["me".into()],
            ..Default::default()
        });
        panel.rows.push(SearchRow {
            key: "field:title".into(),
            text: "retry".into(),
            ..Default::default()
        });
        let lines = search_lines(&panel);
        assert_eq!(lines[0].text, "Text");
        assert_eq!(lines[0].detail, "timeout");
        assert_eq!(lines[1].text, "Assignee");
        assert_eq!(lines[1].detail, "me");
        // `field:title` resolves to the Summary kind label from `filter_kinds`.
        assert!(lines[2].text.contains("Summary"));
        assert_eq!(lines[2].detail, "retry");
        assert_eq!(lines[2].tone, "text");

        assert_eq!(search_lines(&JiraSearchPanel::default())[0].text, "No search filters");
    }

    #[test]
    fn board_lines_group_columns_and_cards() {
        let lines = board_lines(&board_state());
        assert_eq!(lines[0].text, "To Do (3)");
        assert!(lines[0].heading);
        assert_eq!(lines[1].text, "A-1  Fix");
        assert_eq!(lines[1].detail, "Backlog · me");
        assert_eq!(lines[1].tone, "hot", "highest priority is hot");
        assert_eq!(lines[1].id, "A-1");
        assert!(lines[2].text.contains("2 more"));

        let empty = board_lines(&JiraBoardState::default());
        assert_eq!(empty[0].text, "No board columns");
    }

    #[test]
    fn ticket_lines_and_steps() {
        let t = ticket_model();
        let lines = ticket_lines(&t);
        assert_eq!(lines[0].text, "Key");
        assert_eq!(lines[0].detail, "PROJ-1");
        assert_eq!(lines[2].text, "Status");
        assert_eq!(lines[2].detail, "In Progress");
        assert_eq!(lines[3].detail, "In Progress", "category name");
        assert!(lines.iter().any(|l| l.heading && l.text == "Comments 1"));
        assert!(lines.iter().any(|l| l.text == "alice" && l.detail == "hi"));

        assert_eq!(
            ticket_step_lines(&t),
            vec!["○ To Do".to_string(), "● In Progress".to_string(), "○ Done".to_string()]
        );
    }

    #[test]
    fn workflow_steps_override_category_marks() {
        let mut t = ticket_model();
        t.workflow = Some(vec!["Todo".into(), "Doing".into(), "Done".into()]);
        t.status = "Doing".into();
        assert_eq!(
            ticket_step_lines(&t),
            vec!["Todo".to_string(), "Doing".to_string(), "Done".to_string()]
        );
    }

    #[test]
    fn tree_model_from_parts() {
        let empty = JiraTreeModel::from_parts(None, &JiraBoardState::default(), &JiraSearchPanel::default());
        assert_eq!(empty.active, JiraSection::Search);
        assert_eq!(empty.detail[0].text, "No ticket selected");
        assert_eq!(empty.ticket.len(), 0);

        let t = ticket_model();
        let with_ticket = JiraTreeModel::from_parts(Some(&t), &board_state(), &JiraSearchPanel::default());
        assert_eq!(with_ticket.active, JiraSection::Ticket);
        assert_eq!(with_ticket.lines_for(JiraSection::Ticket)[0].detail, "PROJ-1");
        assert!(with_ticket.lines_for(JiraSection::Board)[0].heading);
        assert_eq!(with_ticket.detail[0].text, "Status");
    }

    #[test]
    fn tone_color_resolves_names() {
        let colors = PopupColors::default();
        assert_eq!(tone_color(&colors, "dim"), colors.dim);
        assert_eq!(tone_color(&colors, "accent"), colors.accent_on());
        assert_eq!(tone_color(&colors, "success"), colors.tone(PopupTone::Success));
        assert_eq!(tone_color(&colors, "hot"), colors.tone(PopupTone::Danger));
        assert_eq!(tone_color(&colors, "unknown"), colors.text);
    }
}
