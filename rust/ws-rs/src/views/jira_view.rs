//! The live Jira list view — a port of Swift's `ListSession` (the `[jira]`
//! list window, kitchen_sink.swift) + `PopupRowView.drawTableRow` /
//! `PopupTableHeaderView` / `PopupInspectorView` (PopupWindow.swift,
//! PopupInspector.swift).
//!
//! Data: every `*.json` under `[jira] sources` (the poller's output dir) is
//! a SYNCED tab of flat issue records; per-tab columns come from the
//! endpoint's `columns` in the jira config (else `[jira] columns`). The
//! sidebar adds MY WORK (`jira_poll.py --my-work`), PINNED (views /
//! favourite releases / labels) and BOARDS (catalog + pinned boards).

use crate::app::paths::JiraPaths;
use crate::ui::sidebar::{SidebarEvent, SidebarModel, SidebarPin, SidebarRow};
use crate::ui::theme::PopupTone;
use crate::app::config::ListColumn;
use crate::views::jira::{base_field_label, board_columns, parse_columns, style_rules, BoardColumn, CellStyle, StyleRules};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

pub type Record = BTreeMap<String, String>;

pub const ROW_H: f64 = 32.0;
pub const HEADER_H: f64 = 26.0;
pub const PADDING: f64 = 8.0;
/// `rowLeadInset` with checkboxes + stars: padding + 10 + 22 + 20.
pub const LEAD: f64 = PADDING + 10.0 + 22.0 + 20.0;

/// One SYNCED tab.
#[derive(Clone, Debug, PartialEq)]
pub struct JiraTab {
    pub path: String,
    pub file: String,
    pub title: String,
    pub columns: Vec<ListColumn>,
    pub badge: Option<(String, PopupTone)>,
}

/// Where the list is (`currentTab` vs `pinView`).
#[derive(Clone, Debug, PartialEq)]
pub enum JiraPlace {
    Tab(usize),
    Pin(String),
}

/// `ListSession.myWork`.
pub const MY_WORK: [(&str, &str, &str); 4] = [
    ("mine", "Assigned to me", "person"),
    ("reported", "Reported by me", "square.and.pencil"),
    ("today", "Updated today", "clock"),
    ("watching", "Watching", "eye"),
];
pub const MY_WORK_PREFIX: &str = "mywork:";
pub const BOARD_PREFIX: &str = "board:";
pub const VIEW_PREFIX: &str = "view:";
pub const LABEL_PREFIX: &str = "label:";
pub const BOARD_LIST_KEY: &str = "boards:all";

/// One sprint in the cached `boardCatalog.json` (`CatalogSprint`).
#[derive(Clone, Debug, PartialEq)]
pub struct BoardSprint {
    pub id: String,
    pub name: String,
    pub state: String,
}

/// `PopupTableColumn.frames(x0:usable:)` — percent widths, autos share the
/// leftover, scaled down when they sum past 100.
pub fn column_frames(cols: &[ListColumn], x0: f64, usable: f64) -> Vec<(f64, f64)> {
    if cols.is_empty() || usable <= 0.0 {
        return cols.iter().map(|_| (x0, 0.0)).collect();
    }
    let explicit: f64 = cols.iter().map(|c| c.width.max(0.0)).sum();
    let autos = cols.iter().filter(|c| c.width <= 0.0).count();
    let leftover = (100.0_f64 - explicit).max(0.0);
    let mut pcts: Vec<f64> = cols
        .iter()
        .map(|c| if c.width > 0.0 { c.width } else if autos > 0 { leftover / autos as f64 } else { 0.0 })
        .collect();
    for p in pcts.iter_mut() {
        if *p <= 0.0 {
            *p = 5.0;
        }
    }
    let total: f64 = pcts.iter().sum();
    let scale = if total > 100.0 { 100.0 / total } else { 1.0 };
    let mut x = x0;
    pcts.iter()
        .map(|p| {
            let w = usable * p * scale / 100.0;
            let f = (x, w);
            x += w;
            f
        })
        .collect()
}

/// `jiraCellStyle(_:_:)` over the ported [`StyleRules`].
pub fn cell_style(rules: &StyleRules, field: &str, text: &str) -> Option<CellStyle> {
    let f = field.to_lowercase();
    let v = text.to_lowercase();
    if rules.dim_fields.iter().any(|d| *d == f) {
        return Some(CellStyle { tone: Some("dim".into()), ..Default::default() });
    }
    let hit = |words: &[String]| words.iter().any(|w| !w.is_empty() && v.contains(w.as_str()));
    match f.as_str() {
        "status" | "statuscategory" => Some(
            rules
                .status
                .iter()
                .find(|r| hit(&r.words))
                .map(|r| r.style.clone())
                .unwrap_or_else(|| rules.status_fallback.clone()),
        ),
        "priority" => Some(
            rules
                .priority
                .iter()
                .find(|r| hit(&r.words))
                .map(|r| r.style.clone())
                .unwrap_or_else(|| rules.priority_fallback.clone()),
        ),
        "releasestatus" => rules
            .release_status
            .iter()
            .find(|r| !r.contains.is_empty() && v.contains(&r.contains))
            .map(|r| r.style.clone()),
        _ => None,
    }
}

pub fn tone_of(name: Option<&str>) -> PopupTone {
    match name.unwrap_or("") {
        "dim" => PopupTone::Dim,
        "accent" => PopupTone::Accent,
        "accent2" => PopupTone::Accent2,
        "success" => PopupTone::Success,
        "warning" => PopupTone::Warning,
        "danger" => PopupTone::Danger,
        "info" => PopupTone::Info,
        _ => PopupTone::Text,
    }
}

/// Flatten one JSON issue record into strings (arrays join with ", ").
pub fn record_of(v: &Value) -> Option<Record> {
    let o = v.as_object()?;
    let mut r = Record::new();
    for (k, val) in o {
        let s = match val {
            Value::String(s) => s.clone(),
            Value::Null => String::new(),
            Value::Array(a) => a
                .iter()
                .map(|x| x.as_str().map(str::to_string).unwrap_or_else(|| x.to_string()))
                .collect::<Vec<_>>()
                .join(", "),
            other => other.to_string(),
        };
        r.insert(k.clone(), s);
    }
    Some(r)
}

pub fn load_records(path: &str) -> Vec<Record> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };
    let arr = match &v {
        Value::Array(a) => a.clone(),
        Value::Object(o) => o.get("items").and_then(Value::as_array).cloned().unwrap_or_default(),
        _ => Vec::new(),
    };
    arr.iter().filter_map(record_of).collect()
}

/// `updated` → "2026-10-08 20:11" (Jira ISO stamps shown short).
pub fn display_cell(field: &str, value: &str) -> String {
    let flat = value.replace('\n', " ");
    if matches!(field, "updated" | "created" | "resolved") && flat.len() >= 16 && flat.as_bytes()[10] == b'T' {
        return format!("{} {}", &flat[..10], &flat[11..16]);
    }
    flat
}

/// `JiraPoll.windowSeconds` ("10m" → 600).
pub fn window_seconds(w: &str) -> Option<f64> {
    let unit = w.chars().last()?;
    let n: f64 = w[..w.len() - unit.len_utf8()].parse().ok()?;
    let mult = match unit {
        's' => 1.0,
        'm' => 60.0,
        'h' => 3600.0,
        'd' => 86400.0,
        'w' => 604800.0,
        _ => return None,
    };
    Some(n * mult)
}

/// `JiraPoll.age`.
pub fn age(secs: f64) -> String {
    let s = secs.max(0.0) as i64;
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else if s < 86400 {
        format!("{}h", s / 3600)
    } else {
        format!("{}d", s / 86400)
    }
}

/// "2026-10-10 23:35:09" (local) → epoch seconds.
pub fn parse_stamp(s: &str) -> Option<f64> {
    #[cfg(target_os = "macos")]
    {
        use objc2_foundation::{NSDateFormatter, NSLocale, NSString};
        if s.is_empty() {
            return None;
        }
        let f = NSDateFormatter::new();
        f.setLocale(Some(&NSLocale::localeWithLocaleIdentifier(&NSString::from_str("en_US_POSIX"))));
        f.setDateFormat(Some(&NSString::from_str("yyyy-MM-dd HH:mm:ss")));
        return f.dateFromString(&NSString::from_str(s)).map(|d| d.timeIntervalSince1970());
    }
    #[cfg(not(target_os = "macos"))]
    {
        parse_stamp_local(s)
    }
}

/// Non-macOS Unix [`parse_stamp`] — `time` parses the formatter's `dateFormat`
/// "yyyy-MM-dd HH:mm:ss" and libc resolves it in the system zone (`time`'s
/// sound local-offset API refuses to read the zone in a multi-threaded
/// process like this daemon).
#[cfg(all(unix, not(target_os = "macos")))]
fn parse_stamp_local(s: &str) -> Option<f64> {
    let pdt = parse_stamp_fields(s)?;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_sec = pdt.second() as i32;
    tm.tm_min = pdt.minute() as i32;
    tm.tm_hour = pdt.hour() as i32;
    tm.tm_mday = pdt.day() as i32;
    tm.tm_mon = u8::from(pdt.month()) as i32 - 1;
    tm.tm_year = pdt.year() - 1900;
    tm.tm_isdst = -1;
    let epoch = unsafe { libc::mktime(&mut tm) };
    if epoch == -1 {
        None
    } else {
        Some(epoch as f64)
    }
}

/// Windows [`parse_stamp`] — the same `time` field parse, resolved in the
/// system zone with `time`'s local-offset API (falling back to UTC if the zone
/// cannot be read).
#[cfg(windows)]
fn parse_stamp_local(s: &str) -> Option<f64> {
    let pdt = parse_stamp_fields(s)?;
    let offset = time::UtcOffset::current_local_offset().unwrap_or(time::UtcOffset::UTC);
    Some(pdt.assume_offset(offset).unix_timestamp() as f64)
}

/// Parse the `NSDateFormatter` format "yyyy-MM-dd HH:mm:ss" into its
/// wall-clock fields (the pure half of [`parse_stamp_local`]).
#[cfg(not(target_os = "macos"))]
fn parse_stamp_fields(s: &str) -> Option<time::PrimitiveDateTime> {
    let format = time::format_description::parse_borrowed::<3>(
        "[year]-[month]-[day] [hour]:[minute]:[second]",
    )
    .ok()?;
    time::PrimitiveDateTime::parse(s, &format).ok()
}

fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn read_json(path: &str) -> Value {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(Value::Null)
}

fn str_list(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

/// The list model.
#[derive(Clone, Debug, Default)]
pub struct JiraList {
    pub section: HashMap<String, String>,
    pub paths: Option<JiraPaths>,
    pub config: Value,
    pub tabs: Vec<JiraTab>,
    pub place: Option<JiraPlace>,
    pub all: Vec<Record>,
    pub columns: Vec<ListColumn>,
    pub query: String,
    pub sort: Option<(String, bool)>,
    /// Indices into `all`, filtered + sorted.
    pub rows: Vec<usize>,
    pub selection: usize,
    pub checked: BTreeSet<String>,
    pub favs: BTreeSet<String>,
    pub rules: Option<StyleRules>,
    pub catalog: Vec<(String, String, String)>,
    pub boards: Vec<(String, String)>,
    /// `pinnedBoardViews` (`board|sprint|mode` specs), the sidebar "Pinned" rows.
    pub pinned_views: Vec<String>,
    /// `boardSprint`: board id -> the picked sprint (`current`, `all` or an id).
    pub board_sprint: HashMap<String, String>,
    /// `boardMode`: board id -> `columns` | `table`.
    pub board_mode: HashMap<String, String>,
    /// The columns of the open board (computed from `all` + the board spec).
    pub board_cols: Vec<BoardColumn>,
    /// `true` once `board:list-star` / `board:pin` changed `boards` /
    /// `pinned_views` in memory: the 10 s `refresh()` must not clobber the
    /// user's fresh pins with the on-disk config (the port pins in memory;
    /// persisting through `jira_poll.py --pin-*` is not ported).
    pub pins_dirty: bool,
    pub status_line: Option<(String, PopupTone)>,
    pub note: Option<String>,
    pub inspector: bool,
    /// The shown list file + its mtime (the `reloadWatcher`).
    pub shown_path: String,
    pub shown_mtime: Option<std::time::SystemTime>,
}

impl JiraList {
    pub fn new(section: HashMap<String, String>, paths: Option<JiraPaths>) -> Self {
        let mut l = JiraList { section, paths, inspector: true, ..Default::default() };
        let words: serde_json::Map<String, Value> = l
            .section
            .iter()
            .filter(|(k, _)| k.ends_with("-words"))
            .map(|(k, v)| (k.clone(), Value::String(v.clone())))
            .collect();
        l.rules = Some(style_rules(&Value::Object(words)));
        l.sort = l.section.get("table-sort").and_then(|s| {
            let (f, d) = s.split_once(':').unwrap_or((s.as_str(), "asc"));
            (!f.trim().is_empty()).then(|| (f.trim().to_string(), d.trim().to_lowercase() != "desc"))
        });
        l.reload_meta();
        l
    }

    fn sources_dir(&self) -> String {
        let raw = self.section.get("sources").cloned().unwrap_or_default();
        let home = std::env::var("HOME").unwrap_or_default();
        match raw.strip_prefix('~') {
            Some(rest) => format!("{home}{rest}"),
            None if raw.is_empty() => self.paths.as_ref().map(|p| p.out_dir.clone()).unwrap_or_default(),
            None => raw,
        }
    }

    fn endpoints(&self) -> Vec<Value> {
        self.config.get("endpoints").and_then(Value::as_array).cloned().unwrap_or_default()
    }

    /// `JiraPoll.listTitle(file:)`.
    pub fn list_title(&self, file: &str) -> String {
        if let Some(p) = &self.paths {
            if p.tabs.get("liveSearch").map(String::as_str) == Some(file) {
                return "Search results".into();
            }
            if p.tabs.get("blacklistRelease").map(String::as_str) == Some(file) {
                return "Hidden releases".into();
            }
        }
        for ep in self.endpoints() {
            let Some(name) = ep.get("name").and_then(Value::as_str) else {
                continue;
            };
            let f = ep.get("file").and_then(Value::as_str).map(str::to_string).unwrap_or(format!("{name}.json"));
            if f != file {
                continue;
            }
            if let Some(t) = ep.get("title").and_then(Value::as_str).filter(|t| !t.is_empty()) {
                return t.to_string();
            }
            return match ep.get("type").and_then(Value::as_str) {
                Some("releases") => "Releases".into(),
                Some("favorites") => "Starred issues".into(),
                _ if name == "all" => "All projects".into(),
                _ => name.to_string(),
            };
        }
        file.trim_end_matches(".json").to_string()
    }

    /// `ListSession.tabColumns(cmd, path)` — the endpoint's spec, labeled.
    pub fn tab_columns(&self, file: &str) -> Vec<ListColumn> {
        let spec = self
            .endpoints()
            .iter()
            .find(|ep| {
                let name = ep.get("name").and_then(Value::as_str).unwrap_or("");
                ep.get("file").and_then(Value::as_str).map(str::to_string).unwrap_or(format!("{name}.json")) == file
            })
            .and_then(|ep| ep.get("columns").and_then(Value::as_str).map(str::to_string))
            .or_else(|| self.section.get("columns").cloned())
            .unwrap_or_default();
        let mut cols = parse_columns(&spec);
        for c in cols.iter_mut() {
            if c.title == c.field || c.title.is_empty() {
                c.title = base_field_label(&c.field).map(str::to_string).unwrap_or_else(|| c.field.clone());
            }
        }
        cols
    }

    /// Re-read the config / tabs / catalog / status (cheap; JSON files).
    pub fn reload_meta(&mut self) {
        if let Some(p) = &self.paths {
            self.config = read_json(&p.config_json);
        }
        self.favs = str_list(&self.config, "favorites").into_iter().collect();
        let dir = self.sources_dir();
        let mut files: Vec<String> = std::fs::read_dir(&dir)
            .map(|rd| {
                rd.flatten()
                    .filter_map(|e| {
                        let n = e.file_name().to_string_lossy().into_owned();
                        (n.ends_with(".json") || n.ends_with(".tsv")).then_some(n)
                    })
                    .collect()
            })
            .unwrap_or_default();
        files.sort();
        let status = self
            .paths
            .as_ref()
            .map(|p| read_json(&p.cache_file("status")))
            .unwrap_or(Value::Null);
        self.tabs = files
            .iter()
            .map(|f| JiraTab {
                path: format!("{dir}/{f}"),
                file: f.clone(),
                title: self.list_title(f),
                columns: self.tab_columns(f),
                badge: self.tab_badge(f, &status),
            })
            .collect();
        self.status_line = self.sync_summary(&status);
        self.catalog = Vec::new();
        if let Some(p) = &self.paths {
            let cat = read_json(&p.cache_file("boardCatalog"));
            for proj in cat.get("projects").and_then(Value::as_array).cloned().unwrap_or_default() {
                let pk = proj.get("key").and_then(Value::as_str).unwrap_or("").to_string();
                for b in proj.get("boards").and_then(Value::as_array).cloned().unwrap_or_default() {
                    let Some(id) = b.get("id").and_then(Value::as_str) else { continue };
                    if self.catalog.iter().any(|c| c.0 == id) {
                        continue;
                    }
                    let name = b.get("name").and_then(Value::as_str).unwrap_or(id).to_string();
                    self.catalog.push((id.to_string(), name, pk.clone()));
                }
            }
        }
        // `boards` / `pinned_views` are the pin lists. The port keeps pins in
        // memory (`pins_dirty`); once the user toggled one, a periodic
        // `refresh()` must not restore the on-disk config over it.
        if !self.pins_dirty {
            self.boards = str_list(&self.config, "pinnedBoards")
                .into_iter()
                .map(|id| {
                    let name = self.catalog.iter().find(|c| c.0 == id).map(|c| c.1.clone()).unwrap_or(format!("Board {id}"));
                    (id, name)
                })
                .collect();
            self.pinned_views = str_list(&self.config, "pinnedBoardViews");
        }
    }

    /// `JiraPoll.tabBadge` → `(text, tone)`; success badges stay quiet in
    /// the sidebar (Swift `quietOKTabBadges`).
    fn tab_tone(&self, file: &str, status: &Value) -> Option<(String, PopupTone)> {
        let eps = self.endpoints();
        let blacklist = self.paths.as_ref().and_then(|p| p.tabs.get("blacklistRelease")).map(String::as_str) == Some(file);
        let ep = eps
            .iter()
            .find(|e| {
                let name = e.get("name").and_then(Value::as_str).unwrap_or("");
                e.get("file").and_then(Value::as_str).map(str::to_string).unwrap_or(format!("{name}.json")) == file
            })
            .or_else(|| blacklist.then(|| eps.iter().find(|e| e.get("type").and_then(Value::as_str) == Some("releases"))).flatten())?;
        let name = ep.get("name").and_then(Value::as_str)?;
        let entry = status
            .get("endpoints")
            .and_then(Value::as_array)
            .and_then(|a| a.iter().find(|e| e.get("name").and_then(Value::as_str) == Some(name)).cloned())
            .unwrap_or(Value::Null);
        let window = ep.get("window").and_then(Value::as_str).unwrap_or("10m");
        let wsec = window_seconds(window).unwrap_or(600.0);
        let ok = entry.get("lastSuccess").and_then(Value::as_str).and_then(parse_stamp);
        let failed = entry.get("status").and_then(Value::as_str) == Some("error");
        let stale = ok.map_or(true, |t| now() - t > wsec + (wsec / 2.0).max(300.0));
        let tone = if !stale && !failed {
            PopupTone::Success
        } else if stale && failed {
            PopupTone::Danger
        } else {
            PopupTone::Warning
        };
        Some((ok.map_or("never".to_string(), |t| age(now() - t)), tone))
    }

    fn tab_badge(&self, file: &str, status: &Value) -> Option<(String, PopupTone)> {
        self.tab_tone(file, status).filter(|(_, t)| *t != PopupTone::Success)
    }

    /// `JiraPoll.syncSummary`.
    fn sync_summary(&self, status: &Value) -> Option<(String, PopupTone)> {
        let skip: Vec<&str> = self
            .paths
            .as_ref()
            .map(|p| ["liveSearch", "blacklistRelease"].iter().filter_map(|k| p.tabs.get(*k).map(String::as_str)).collect())
            .unwrap_or_default();
        let entries = status.get("endpoints").and_then(Value::as_array).cloned().unwrap_or_default();
        let mut newest: Option<f64> = None;
        let (mut behind, mut polled, mut failing) = (0, 0, false);
        for t in &self.tabs {
            if skip.contains(&t.file.as_str()) {
                continue;
            }
            let Some((_, tone)) = self.tab_tone(&t.file, status) else { continue };
            polled += 1;
            let name = self
                .endpoints()
                .iter()
                .find(|e| {
                    let n = e.get("name").and_then(Value::as_str).unwrap_or("");
                    e.get("file").and_then(Value::as_str).map(str::to_string).unwrap_or(format!("{n}.json")) == t.file
                })
                .and_then(|e| e.get("name").and_then(Value::as_str).map(str::to_string));
            if let Some(ts) = entries
                .iter()
                .find(|e| e.get("name").and_then(Value::as_str).map(str::to_string) == name)
                .and_then(|e| e.get("lastSuccess").and_then(Value::as_str))
                .and_then(parse_stamp)
            {
                newest = Some(newest.map_or(ts, |n: f64| n.max(ts)));
            }
            if tone != PopupTone::Success {
                behind += 1;
            }
            if tone == PopupTone::Danger {
                failing = true;
            }
        }
        if polled == 0 {
            return None;
        }
        let Some(n) = newest else {
            return Some(("Never synced".into(), PopupTone::Danger));
        };
        let ago = age(now() - n);
        if behind > 0 {
            return Some((
                format!("{behind} out of date · last sync {ago} ago"),
                if failing { PopupTone::Danger } else { PopupTone::Warning },
            ));
        }
        Some((format!("Synced {ago} ago"), PopupTone::Success))
    }

    /// The reload watcher: re-read the sidebar meta and, when the shown
    /// file changed on disk, its rows (keeping the selected key).
    pub fn refresh(&mut self) {
        self.reload_meta();
        if self.shown_path.is_empty() {
            return;
        }
        let m = std::fs::metadata(&self.shown_path).and_then(|m| m.modified()).ok();
        if m == self.shown_mtime {
            return;
        }
        self.shown_mtime = m;
        let keep = self.selected().and_then(|r| r.get("key")).cloned();
        self.all = load_records(&self.shown_path);
        self.refilter();
        if let Some(k) = keep {
            if let Some(i) = self.rows.iter().position(|&j| self.all[j].get("key") == Some(&k)) {
                self.selection = i;
            }
        }
    }

    /// Pick the startup tab (the last one shown, else the first project tab).
    pub fn start(&mut self, last: Option<&str>) {
        let i = last
            .and_then(|l| self.tabs.iter().position(|t| t.file == l))
            .unwrap_or(0);
        if !self.tabs.is_empty() {
            self.select_tab(i);
        }
    }

    pub fn select_tab(&mut self, i: usize) {
        let Some(t) = self.tabs.get(i).cloned() else { return };
        self.place = Some(JiraPlace::Tab(i));
        self.columns = t.columns.clone();
        self.all = load_records(&t.path);
        self.shown_path = t.path.clone();
        self.shown_mtime = std::fs::metadata(&t.path).and_then(|m| m.modified()).ok();
        self.selection = 0;
        self.note = None;
        self.refilter();
    }

    /// Show a pinned list (`showPinFile`).
    pub fn show_pin_file(&mut self, key: &str, path: &str, columns: Vec<ListColumn>) {
        self.place = Some(JiraPlace::Pin(key.to_string()));
        self.columns = columns;
        self.all = load_records(path);
        self.shown_path = path.to_string();
        self.shown_mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        self.selection = 0;
        self.refilter();
    }

    /// `boardListRows()` as records (the "All boards" pin).
    pub fn show_board_list(&mut self) {
        self.place = Some(JiraPlace::Pin(BOARD_LIST_KEY.into()));
        self.columns = parse_columns("name:Board:40:left:filter+sort, project:Project:20:left:filter+sort, boardId:ID:10:left:sort");
        let mut ids: Vec<String> = self.catalog.iter().map(|c| c.0.clone()).collect();
        for (id, _) in &self.boards {
            if !ids.contains(id) {
                ids.push(id.clone());
            }
        }
        self.all = ids
            .iter()
            .map(|id| {
                let (name, proj) = self
                    .catalog
                    .iter()
                    .find(|c| c.0 == *id)
                    .map(|c| (c.1.clone(), c.2.clone()))
                    .unwrap_or_else(|| (self.board_name(id), String::new()));
                let mut r = Record::new();
                r.insert("key".into(), format!("{BOARD_PREFIX}{id}"));
                r.insert("name".into(), name.clone());
                r.insert("title".into(), name);
                r.insert("project".into(), proj);
                r.insert("boardId".into(), id.clone());
                r
            })
            .collect();
        self.selection = 0;
        self.board_cols = Vec::new();
        self.refilter();
    }

    fn default_columns(&self) -> Vec<ListColumn> {
        let mut cols = parse_columns(self.section.get("columns").map(String::as_str).unwrap_or(""));
        for c in cols.iter_mut() {
            if c.title.is_empty() {
                c.title = c.field.clone();
            }
        }
        cols
    }

    /// A pinned board's cached issues (`showBoardPin`), else poll it.
    pub fn board_path(&self, id: &str) -> Option<String> {
        let p = self.paths.as_ref()?;
        let side = p.side_dirs.get("boards")?;
        let out = Path::new(&p.out_dir).parent()?.join(side).join(format!("board-{id}.json"));
        Some(out.to_string_lossy().into_owned())
    }

    pub fn open_board(&mut self, id: &str) -> bool {
        let Some(path) = self.board_path(id) else { return false };
        if !Path::new(&path).exists() {
            return false;
        }
        // `openBoard` pins a board that is not pinned yet (the sidebar's
        // "Boards" rows), then shows its cached issues.
        self.pin_board(id);
        let cols = self.default_columns();
        self.show_pin_file(&format!("{BOARD_PREFIX}{id}"), &path, cols);
        let default_sprint = self.default_sprint(id);
        self.board_sprint.entry(id.to_string()).or_insert(default_sprint);
        self.board_mode.entry(id.to_string()).or_insert_with(|| "columns".to_string());
        self.compute_board_columns();
        true
    }

    /// The board shown when the list place is a `board:<id>` pin.
    pub fn on_board(&self) -> Option<String> {
        match &self.place {
            Some(JiraPlace::Pin(k)) => k.strip_prefix(BOARD_PREFIX).map(str::to_string),
            _ => None,
        }
    }

    /// `true` while the "All boards" list is the shown pin.
    pub fn on_board_list(&self) -> bool {
        matches!(&self.place, Some(JiraPlace::Pin(k)) if k == BOARD_LIST_KEY)
    }

    /// `boardName(_:)` — the catalog / pinned name, else `Board <id>`.
    pub fn board_name(&self, id: &str) -> String {
        self.catalog
            .iter()
            .find(|c| c.0 == id)
            .map(|c| c.1.clone())
            .or_else(|| self.boards.iter().find(|(bid, _)| bid == id).map(|(_, n)| n.clone()))
            .unwrap_or_else(|| format!("Board {id}"))
    }

    /// The sprints of one board (from the cached `boardCatalog.json`).
    pub fn board_sprints(&self, id: &str) -> Vec<BoardSprint> {
        let Some(p) = &self.paths else { return Vec::new() };
        let cat = read_json(&p.cache_file("boardCatalog"));
        for proj in cat.get("projects").and_then(Value::as_array).cloned().unwrap_or_default() {
            for b in proj.get("boards").and_then(Value::as_array).cloned().unwrap_or_default() {
                if b.get("id").and_then(Value::as_str) != Some(id) {
                    continue;
                }
                return b
                    .get("sprints")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|s| {
                        let sid = s.get("id").and_then(Value::as_str)?;
                        Some(BoardSprint {
                            id: sid.to_string(),
                            name: s.get("name").and_then(Value::as_str).unwrap_or(sid).to_string(),
                            state: s.get("state").and_then(Value::as_str).unwrap_or("").to_string(),
                        })
                    })
                    .collect();
            }
        }
        Vec::new()
    }

    /// `defaultSprint(_:)` — `current` while any sprint is active, else `all`.
    pub fn default_sprint(&self, id: &str) -> String {
        if self.board_sprints(id).iter().any(|s| s.state == "active") {
            "current".to_string()
        } else {
            "all".to_string()
        }
    }

    /// The board's `columns` spec from the cached `boards.json`.
    fn board_column_spec(&self, id: &str) -> Vec<Value> {
        let Some(p) = &self.paths else { return Vec::new() };
        let boards = read_json(&p.cache_file("boards"));
        boards
            .get(id)
            .and_then(|b| b.get("columns"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    /// Recompute the open board's columns from its loaded rows (`renderBoardColumns`).
    pub fn compute_board_columns(&mut self) {
        let Some(id) = self.on_board() else {
            self.board_cols = Vec::new();
            return;
        };
        let rows: Vec<Value> = self
            .all
            .iter()
            .map(|r| {
                let f = |k: &str| r.get(k).cloned().unwrap_or_default();
                serde_json::json!({
                    "key": f("key"),
                    "title": f("title"),
                    "rowTitle": r.get("title").cloned().unwrap_or_default(),
                    "type": f("type"),
                    "priority": f("priority"),
                    "assignee": f("assignee"),
                    "status": f("status"),
                })
            })
            .collect();
        let spec = self.board_column_spec(&id);
        let words = self
            .rules
            .as_ref()
            .map(|r| serde_json::to_value(&r.words).unwrap_or_else(|_| serde_json::json!({})))
            .unwrap_or_else(|| serde_json::json!({}));
        let params = serde_json::json!({
            "rows": rows,
            "columns": spec,
            "categories": {},
            "words": words,
            "people": {},
            "categoryNames": ["To Do", "In Progress", "Done"],
        });
        // The Swift board called `jira.board_columns` through the Python
        // helper; use it when the helper is up (same code the Python tests
        // cover) and fall back to the identical pure port in unit tests.
        self.board_cols = crate::views::jira::helper_board_columns(&params).unwrap_or_else(|_| board_columns(&params));
    }

    /// `openView(_:)` — parse `board|sprint|mode` and open that board.
    pub fn open_view(&mut self, v: &str) -> bool {
        let parts: Vec<&str> = v.split('|').collect();
        if parts.len() != 3 {
            return false;
        }
        let (board, sprint, mode) = (parts[0], parts[1], parts[2]);
        self.board_sprint.insert(board.to_string(), sprint.to_string());
        self.board_mode.insert(board.to_string(), mode.to_string());
        self.open_board(board)
    }

    /// `pickSprint(_:)` — the open board's sprint picker.
    pub fn pick_sprint(&mut self, sprint: &str) -> bool {
        let Some(id) = self.on_board() else { return false };
        self.board_sprint.insert(id, sprint.to_string());
        self.compute_board_columns();
        true
    }

    /// `boardBar.onMode` — `columns` | `table` for the open board.
    pub fn set_mode(&mut self, mode: &str) -> bool {
        let Some(id) = self.on_board() else { return false };
        self.board_mode.insert(id, mode.to_string());
        true
    }

    /// `list-star` — pin/unpin a board (the sidebar "Boards" rows).
    pub fn toggle_pin_board(&mut self, id: &str) -> bool {
        if let Some(i) = self.boards.iter().position(|(bid, _)| bid == id) {
            self.boards.remove(i);
        } else {
            self.boards.push((id.to_string(), self.board_name(id)));
        }
        self.pins_dirty = true;
        true
    }

    /// `openBoard`'s first step: a board that is not pinned yet joins the
    /// sidebar "Boards" section (in-memory — `--pin-board` is not ported).
    pub fn pin_board(&mut self, id: &str) {
        if !self.boards.iter().any(|(bid, _)| bid == id) {
            self.boards.push((id.to_string(), self.board_name(id)));
            self.pins_dirty = true;
        }
    }

    /// `boardBar.onPin` — pin/unpin the current `board|sprint|mode` view.
    pub fn toggle_pin_view(&mut self, spec: &str) -> bool {
        if let Some(i) = self.pinned_views.iter().position(|v| v == spec) {
            self.pinned_views.remove(i);
        } else {
            self.pinned_views.push(spec.to_string());
        }
        self.pins_dirty = true;
        true
    }

    /// `currentViewSpec` — `board|sprint|mode` for the open board.
    pub fn current_view_spec(&self) -> Option<String> {
        let id = self.on_board()?;
        let sprint = self
            .board_sprint
            .get(&id)
            .cloned()
            .unwrap_or_else(|| self.default_sprint(&id));
        let mode = self
            .board_mode
            .get(&id)
            .cloned()
            .unwrap_or_else(|| "columns".to_string());
        Some(format!("{id}|{sprint}|{mode}"))
    }

    /// `sidebarSelection` — the pinned view key when the current view is pinned,
    /// else the shown pin key.
    fn sidebar_selected(&self) -> String {
        if let Some(spec) = self.current_view_spec() {
            if self.pinned_views.iter().any(|v| v == &spec) {
                return format!("{VIEW_PREFIX}{spec}");
            }
        }
        match &self.place {
            Some(JiraPlace::Pin(k)) => k.clone(),
            _ => String::new(),
        }
    }

    /// `testExtra` for the board list / board forms (`state.views.jira.board`).
    pub fn board_state(&self) -> Value {
        if self.on_board_list() {
            return serde_json::json!({
                "list": true,
                "rows": self.all.len(),
                "pinned": self.boards.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>(),
                "selected": self.sidebar_selected(),
            });
        }
        match self.on_board() {
            Some(id) => serde_json::json!({
                "id": id,
                "sprint": self.board_sprint.get(&id).cloned().unwrap_or_else(|| self.default_sprint(&id)),
                "mode": self.board_mode.get(&id).cloned().unwrap_or_else(|| "columns".to_string()),
                "columns": self.board_mode.get(&id).map(|m| m == "columns").unwrap_or(true),
                "bar": true,
                "sprintKeys": -1,
                "views": self.pinned_views,
                "selected": self.sidebar_selected(),
                "catalog": self.catalog.iter().map(|(id, _, _)| id.clone()).collect::<Vec<_>>(),
            }),
            None => serde_json::json!({}),
        }
    }

    /// The `views.jira` extras the daemon UI pushes each drain tick, so
    /// `state.views.jira.board` is read from this same model the view draws.
    pub fn view_state(&self) -> Value {
        let selected_tab = match &self.place {
            Some(JiraPlace::Tab(i)) => *i as i64,
            _ => -1,
        };
        serde_json::json!({
            "board": self.board_state(),
            "tabs": self.tabs.len(),
            "selectedTab": selected_tab,
            "rowCount": self.rows.len(),
            "selection": self.selection,
            "query": self.query,
        })
    }

    /// When a board is open in `columns` mode the table shows the board's
    /// columns and cards (Swift's board overlay), built from `board_cols` — the
    /// real model, not a separate one. `table` mode keeps the issue table.
    fn board_snapshot(&self) -> Option<JiraSnapshot> {
        let id = self.on_board()?;
        if self.board_mode.get(&id).map(String::as_str).unwrap_or("columns") != "columns" {
            return None;
        }
        if self.board_cols.is_empty() {
            return None;
        }
        let columns = parse_columns(
            "column:Column:18:left, key:Key:12:left, title:Title:44:left, status:Status:16:left, assignee:Assignee:10:left",
        );
        let mut rows: Vec<Record> = Vec::new();
        for col in &self.board_cols {
            let mut head = Record::new();
            head.insert("column".into(), col.name.clone());
            head.insert("title".into(), format!("{} ({})", col.name, col.cards.len() + col.more));
            rows.push(head);
            for card in &col.cards {
                let mut r = Record::new();
                r.insert("column".into(), col.name.clone());
                r.insert("key".into(), card.key.clone());
                r.insert("title".into(), card.title.clone());
                r.insert("status".into(), card.status.clone());
                r.insert("assignee".into(), card.assignee.clone());
                rows.push(r);
            }
            if col.more > 0 {
                let mut more = Record::new();
                more.insert("column".into(), col.name.clone());
                more.insert("title".into(), format!("… {} more", col.more));
                rows.push(more);
            }
        }
        let selection = self.selection.min(rows.len().saturating_sub(1));
        Some(JiraSnapshot {
            columns,
            rows,
            selection,
            checked: self.checked.clone(),
            favs: self.favs.clone(),
            sort: None,
            sidebar: self.sidebar_model(),
            inspector: None,
            show_inspector: false,
            query: self.query.clone(),
            empty: "No board columns.".into(),
            rules: self.rules.clone(),
        })
    }

    /// `showReleasePin` — `--release-view KEY` writes one file per release
    /// from the issue cache (local, no request); show the pinned release's.
    pub fn open_release(&mut self, key: &str) -> Result<(), String> {
        let out = crate::views::jira::run_jira_poll(&["--release-view", key], None)?;
        let dir = out.get("dir").and_then(Value::as_str).ok_or("release: no dir")?;
        let file = out
            .get("releases")
            .and_then(Value::as_array)
            .and_then(|a| a.iter().find(|r| r.get("key").and_then(Value::as_str) == Some(key)))
            .and_then(|r| r.get("file").and_then(Value::as_str))
            .ok_or_else(|| format!("release {key}: not in the synced releases"))?;
        let cols = self.default_columns();
        self.show_pin_file(key, &format!("{dir}/{file}"), cols);
        Ok(())
    }

    /// `showLabelPin` — `--label-view NAME` -> the issues carrying the label.
    pub fn open_label(&mut self, name: &str) -> Result<(), String> {
        let out = crate::views::jira::run_jira_poll(&["--label-view", name], None)?;
        let dir = out.get("dir").and_then(Value::as_str).ok_or("label: no dir")?;
        let file = out.get("file").and_then(Value::as_str).ok_or("label: no file")?;
        let cols = self.default_columns();
        self.show_pin_file(&format!("{LABEL_PREFIX}{name}"), &format!("{dir}/{file}"), cols);
        Ok(())
    }

    /// `showMyWorkPin` — `jira_poll.py --my-work` → `<dir>/<id>.json`.
    pub fn open_my_work(&mut self, id: &str) -> Result<(), String> {
        let out = crate::views::jira::run_jira_poll(&["--my-work"], None)?;
        let dir = out.get("dir").and_then(Value::as_str).ok_or("my work: no dir")?;
        let path = format!("{dir}/{id}.json");
        if !Path::new(&path).exists() {
            return Err(format!("my work: {id} not synced yet"));
        }
        let cols = self.default_columns();
        self.show_pin_file(&format!("{MY_WORK_PREFIX}{id}"), &path, cols);
        Ok(())
    }

    fn filter_fields(&self) -> Vec<String> {
        self.section
            .get("filter")
            .map(|f| f.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect())
            .unwrap_or_else(|| vec!["key".into(), "title".into()])
    }

    pub fn refilter(&mut self) {
        let fields = self.filter_fields();
        let words: Vec<String> = self.query.to_lowercase().split_whitespace().map(str::to_string).collect();
        let mut idx: Vec<usize> = (0..self.all.len())
            .filter(|&i| {
                if words.is_empty() {
                    return true;
                }
                let r = &self.all[i];
                let hay: String = fields
                    .iter()
                    .chain(["name".to_string()].iter())
                    .filter_map(|f| r.get(f))
                    .map(|s| s.to_lowercase())
                    .collect::<Vec<_>>()
                    .join(" ");
                words.iter().all(|w| hay.contains(w.as_str()))
            })
            .collect();
        if let Some((field, asc)) = &self.sort {
            if self.columns.iter().any(|c| &c.field == field) {
                idx.sort_by(|&a, &b| {
                    let x = self.all[a].get(field).map(String::as_str).unwrap_or("");
                    let y = self.all[b].get(field).map(String::as_str).unwrap_or("");
                    let o = crate::engines::list_filter::localized_standard_compare(x, y);
                    if *asc { o } else { o.reverse() }
                });
            }
        }
        self.rows = idx;
        if self.selection >= self.rows.len() {
            self.selection = self.rows.len().saturating_sub(1);
        }
    }

    /// Header click: sort by column `i` (toggle when already sorted).
    pub fn sort_by(&mut self, i: usize) {
        let Some(c) = self.columns.get(i) else { return };
        if !c.sortable {
            return;
        }
        let field = c.field.clone();
        self.sort = match &self.sort {
            Some((f, asc)) if *f == field => Some((field, !asc)),
            _ => Some((field, true)),
        };
        self.refilter();
    }

    pub fn selected(&self) -> Option<&Record> {
        self.rows.get(self.selection).and_then(|&i| self.all.get(i))
    }

    pub fn row(&self, i: usize) -> Option<&Record> {
        self.rows.get(i).and_then(|&j| self.all.get(j))
    }

    pub fn site(&self) -> String {
        self.config.get("site").and_then(Value::as_str).unwrap_or("").trim_end_matches('/').to_string()
    }

    /// The sidebar (`syncReleasePins` + the SYNCED tabs).
    pub fn sidebar_model(&self) -> SidebarModel {
        let mut pinned: Vec<SidebarPin> = Vec::new();
        let my_work = self
            .section
            .get("my-work")
            .map_or(true, |v| !matches!(v.to_lowercase().as_str(), "false" | "no" | "0" | "off"));
        if my_work {
            for (id, title, icon) in MY_WORK {
                pinned.push(SidebarPin {
                    key: format!("{MY_WORK_PREFIX}{id}"),
                    label: title.into(),
                    icon: icon.into(),
                    section: "My work".into(),
                    tip: "From the issues already synced (no request)".into(),
                    ..Default::default()
                });
            }
        }
        for v in &self.pinned_views {
            let board = v.split('|').next().unwrap_or("").to_string();
            let name = self.catalog.iter().find(|c| c.0 == board).map(|c| c.1.clone()).unwrap_or(format!("Board {board}"));
            let sprint = self.sprint_name(&board);
            let label = if sprint.is_empty() { name } else { format!("{name} · {sprint}") };
            pinned.push(SidebarPin {
                key: format!("{VIEW_PREFIX}{v}"),
                label,
                icon: if v.ends_with("|table") { "tablecells" } else { "rectangle.split.3x1.fill" }.into(),
                section: "Pinned".into(),
                meta: "Board".into(),
                ..Default::default()
            });
        }
        for r in str_list(&self.config, "favoriteReleases") {
            let label = match r.split_once('-') {
                Some((a, b)) => format!("{a} {b}"),
                None => r.clone(),
            };
            pinned.push(SidebarPin {
                key: r.clone(),
                label,
                icon: "shippingbox".into(),
                section: "Pinned".into(),
                meta: "Release".into(),
                tip: format!("Show every issue in {r} here"),
            });
        }
        for l in str_list(&self.config, "pinnedLabels") {
            pinned.push(SidebarPin {
                key: format!("{LABEL_PREFIX}{l}"),
                label: l.clone(),
                icon: "tag".into(),
                section: "Pinned".into(),
                meta: "Label".into(),
                tip: format!("Show every issue labelled {l} here"),
            });
        }
        if !self.catalog.is_empty() || !self.boards.is_empty() {
            pinned.push(SidebarPin {
                key: BOARD_LIST_KEY.into(),
                label: "All boards".into(),
                icon: "square.grid.2x2".into(),
                section: "Boards".into(),
                meta: if self.catalog.is_empty() { String::new() } else { self.catalog.len().to_string() },
                tip: "Every board in the projects in scope — ☆ one to pin it here".into(),
            });
        }
        for (id, name) in &self.boards {
            pinned.push(SidebarPin {
                key: format!("{BOARD_PREFIX}{id}"),
                label: name.clone(),
                icon: "rectangle.split.3x1".into(),
                section: "Boards".into(),
                ..Default::default()
            });
        }
        let rows = self
            .tabs
            .iter()
            .map(|t| {
                let name = t.file.to_lowercase();
                let icon = if name.contains("blacklist_release") {
                    "eye.slash"
                } else if name.starts_with("release") {
                    "shippingbox"
                } else if name.starts_with("favorites") {
                    "star"
                } else if name.starts_with("search") {
                    "magnifyingglass"
                } else {
                    "list.bullet.rectangle"
                };
                SidebarRow {
                    title: t.title.clone(),
                    icon: icon.into(),
                    badge: t.badge.clone(),
                    tip: t.path.clone(),
                    ..Default::default()
                }
            })
            .collect();
        let (selected, pinned_selected) = match &self.place {
            Some(JiraPlace::Tab(i)) => (Some(*i), None),
            Some(JiraPlace::Pin(_)) => (None, Some(self.sidebar_selected())),
            None => (None, None),
        };
        SidebarModel {
            section_title: "Synced".into(),
            rows,
            selected,
            pinned,
            pinned_selected,
            status: self.status_line.clone(),
            ..Default::default()
        }
    }

    fn sprint_name(&self, board: &str) -> String {
        let Some(p) = &self.paths else { return String::new() };
        let cat = read_json(&p.cache_file("boardCatalog"));
        for proj in cat.get("projects").and_then(Value::as_array).cloned().unwrap_or_default() {
            for b in proj.get("boards").and_then(Value::as_array).cloned().unwrap_or_default() {
                if b.get("id").and_then(Value::as_str) != Some(board) {
                    continue;
                }
                for s in b.get("sprints").and_then(Value::as_array).cloned().unwrap_or_default() {
                    if s.get("state").and_then(Value::as_str) == Some("active") {
                        return s.get("name").and_then(Value::as_str).unwrap_or("").to_string();
                    }
                }
            }
        }
        String::new()
    }

    /// The inspector content (`inspectorContent`): key, title, chips from
    /// `[jira] trailing`, body, fields from `detail` + project / updated.
    pub fn inspector_content(&self) -> Option<InspectorContent> {
        let r = self.selected()?;
        let key = r.get("key").cloned().unwrap_or_default();
        let title = r.get("title").or_else(|| r.get("name")).cloned().unwrap_or_default();
        let rules = self.rules.clone().unwrap_or_else(|| style_rules(&Value::Null));
        let trailing: Vec<String> = self
            .section
            .get("trailing")
            .map(|t| t.split(',').map(|x| x.trim().to_string()).collect())
            .unwrap_or_else(|| vec!["status".into()]);
        let chips = trailing
            .iter()
            .filter_map(|f| {
                let v = r.get(f).filter(|v| !v.is_empty())?;
                let tone = cell_style(&rules, f, v).map(|s| tone_of(s.tone.as_deref())).unwrap_or(PopupTone::Dim);
                Some((v.clone(), tone))
            })
            .collect();
        let mut fields: Vec<(String, String)> = Vec::new();
        let detail: Vec<String> = self
            .section
            .get("detail")
            .map(|t| t.split(',').map(|x| x.trim().to_string()).collect())
            .unwrap_or_default();
        for f in detail.iter().chain(["project".to_string(), "updated".to_string()].iter()) {
            if fields.iter().any(|(l, _)| l == f) {
                continue;
            }
            let v = r.get(f).cloned().unwrap_or_default();
            let label = base_field_label(f).map(str::to_string).unwrap_or_else(|| f.clone());
            fields.push((label, display_cell(f, &v)));
        }
        let body = r.get("description").cloned().unwrap_or_default();
        Some(InspectorContent { key, title, chips, fields, body })
    }
}

/// `PopupInspectorContent`.
#[derive(Clone, Debug, PartialEq)]
pub struct InspectorContent {
    pub key: String,
    pub title: String,
    pub chips: Vec<(String, PopupTone)>,
    pub fields: Vec<(String, String)>,
    pub body: String,
}

/// What the view reports.
#[derive(Clone, Debug, PartialEq)]
pub enum JiraEvent {
    Click { row: usize, double: bool },
    Check(usize),
    Star(usize),
    Sort(usize),
    Query(String),
    Enter,
    Move(i64),
    Escape,
    OpenDetail,
    RowMenu { row: usize, x: f64, y: f64 },
    /// Right-click on the column header (`PopupTableHeader.rightMouseDown`).
    HeaderMenu { x: f64, y: f64 },
    Sidebar(crate::ui::sidebar::SidebarEvent),
}

/// `fitTableColumns`' width solver: cap each column's needed width at
/// `max(160, 45%)`, shrink the widest ones together until the total fits
/// `usable`, then turn the widths into percentages (one decimal).
pub fn fit_column_pcts(need: &[f64], usable: f64) -> Vec<f64> {
    let usable = usable.max(1.0);
    let mut need: Vec<f64> = need.iter().map(|w| w.min(160f64.max(usable * 0.45))).collect();
    let mut over = need.iter().sum::<f64>() - usable;
    while over > 0.5 {
        let max_w = need.iter().cloned().fold(0.0, f64::max);
        let idx: Vec<usize> = (0..need.len()).filter(|&i| need[i] >= max_w - 0.5).collect();
        let next = need.iter().cloned().filter(|w| *w < max_w - 0.5).fold(40.0, f64::max);
        let step = (over / idx.len() as f64).min(max_w - next.max(40.0));
        if step <= 0.5 {
            break;
        }
        for &i in &idx {
            need[i] -= step;
        }
        over -= step * idx.len() as f64;
    }
    need.iter().map(|w| ((w / usable * 100.0) * 10.0).round() / 10.0).collect()
}

/// Everything the view draws.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JiraSnapshot {
    pub columns: Vec<ListColumn>,
    pub rows: Vec<Record>,
    pub selection: usize,
    pub checked: BTreeSet<String>,
    pub favs: BTreeSet<String>,
    pub sort: Option<(usize, bool)>,
    pub sidebar: SidebarModel,
    pub inspector: Option<InspectorContent>,
    pub show_inspector: bool,
    pub query: String,
    pub empty: String,
    pub rules: Option<StyleRules>,
}

impl JiraList {
    pub fn snapshot(&self) -> JiraSnapshot {
        if let Some(board) = self.board_snapshot() {
            return board;
        }
        let rows: Vec<Record> = self.rows.iter().filter_map(|&i| self.all.get(i).cloned()).collect();
        let sort = self
            .sort
            .as_ref()
            .and_then(|(f, asc)| self.columns.iter().position(|c| &c.field == f).map(|i| (i, *asc)));
        JiraSnapshot {
            columns: self.columns.clone(),
            rows,
            selection: self.selection,
            checked: self.checked.clone(),
            favs: self.favs.clone(),
            sort,
            sidebar: self.sidebar_model(),
            inspector: self.inspector_content(),
            show_inspector: self.inspector,
            query: self.query.clone(),
            empty: if self.tabs.is_empty() {
                "No synced Jira lists yet — run the Jira setup (Settings ▸ Jira).".into()
            } else if self.all.is_empty() {
                "Nothing in this list.".into()
            } else {
                "No matches.".into()
            },
            rules: self.rules.clone(),
        }
    }

    /// Apply a user event to the model — the pure half of the macOS
    /// `DaemonUi::jira_event` dispatch (`app/host.rs`). Every model mutation
    /// the AppKit half performs for an event is here; UI-only effects (opening
    /// URLs, context menus, focus, logging, relayout) stay with the frontend.
    pub fn handle(&mut self, ev: JiraEvent) {
        match ev {
            JiraEvent::Click { row, double } => {
                self.selection = row;
                if double {
                    self.handle(JiraEvent::OpenDetail);
                }
            }
            JiraEvent::Check(row) => {
                if let Some(k) = self.row(row).and_then(|r| r.get("key")).cloned() {
                    if !self.checked.remove(&k) {
                        self.checked.insert(k);
                    }
                }
            }
            JiraEvent::Star(row) => {
                if let Some(k) = self.row(row).and_then(|r| r.get("key")).cloned() {
                    if !self.favs.remove(&k) {
                        self.favs.insert(k);
                    }
                }
            }
            JiraEvent::Sort(i) => self.sort_by(i),
            // The Fit Columns menu is a UI action.
            JiraEvent::HeaderMenu { .. } => {}
            JiraEvent::Query(q) => {
                self.query = q;
                self.selection = 0;
                self.refilter();
            }
            JiraEvent::Move(d) => {
                let n = self.rows.len() as i64;
                if n > 0 {
                    self.selection = (self.selection as i64 + d).clamp(0, n - 1) as usize;
                }
            }
            JiraEvent::Escape => {
                self.query.clear();
                self.refilter();
            }
            // A board row opens its board; an issue row opens a URL (UI).
            JiraEvent::Enter | JiraEvent::OpenDetail => {
                let board = self.selected().and_then(|r| {
                    r.get("boardId")
                        .filter(|_| r.get("key").is_some_and(|k| k.starts_with(BOARD_PREFIX)))
                        .cloned()
                });
                if let Some(id) = board {
                    let _ = self.open_board(&id);
                }
            }
            // The context menu is a UI action; selecting the row is model state.
            JiraEvent::RowMenu { row, .. } => self.selection = row,
            JiraEvent::Sidebar(SidebarEvent::Select(i)) => self.select_tab(i),
            JiraEvent::Sidebar(SidebarEvent::Pinned(key)) => {
                if let Some(id) = key.strip_prefix(MY_WORK_PREFIX) {
                    let _ = self.open_my_work(id);
                } else if key == BOARD_LIST_KEY {
                    self.show_board_list();
                } else if let Some(id) = key.strip_prefix(BOARD_PREFIX) {
                    let _ = self.open_board(id);
                } else if let Some(spec) = key.strip_prefix(VIEW_PREFIX) {
                    let _ = self.open_view(spec);
                } else if let Some(name) = key.strip_prefix(LABEL_PREFIX) {
                    let _ = self.open_label(name);
                } else {
                    let _ = self.open_release(&key);
                }
            }
            JiraEvent::Sidebar(_) => {}
        }
    }
}

#[cfg(target_os = "macos")]
pub use appkit::{draw_wrapped, JiraView};

#[cfg(target_os = "macos")]
mod appkit {
    use super::*;
    use crate::ui::chrome::Rect;
    use crate::ui::sidebar::SidebarView;
    use crate::ui::style::{self, ButtonState, Weight};
    use crate::ui::theme::{PopupColors, Rgba};
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, ProtocolObject, Sel};
    use objc2::{define_class, msg_send, sel, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, Message};
    use objc2_app_kit::{
        NSStringDrawingDeprecated, NSBezierPath, NSControlTextEditingDelegate, NSEvent, NSFocusRingType, NSFont,
        NSGraphicsContext, NSScrollView, NSStringDrawingOptions, NSTextField, NSTextFieldDelegate,
        NSTextView, NSTrackingArea, NSTrackingAreaOptions, NSView,
    };
    use objc2_foundation::{NSNotification, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    fn as_any<T: Message + ?Sized>(obj: &T) -> &AnyObject {
        unsafe { &*(obj as *const T as *const AnyObject) }
    }

    fn ns(r: Rect) -> NSRect {
        style::ns(r)
    }

    fn tracking(view: &NSView) -> Retained<NSTrackingArea> {
        let opts = NSTrackingAreaOptions::MouseMoved
            | NSTrackingAreaOptions::MouseEnteredAndExited
            | NSTrackingAreaOptions::ActiveAlways
            | NSTrackingAreaOptions::InVisibleRect;
        let t = unsafe {
            NSTrackingArea::initWithRect_options_owner_userInfo(
                NSTrackingArea::alloc(),
                view.bounds(),
                opts,
                Some(as_any(view)),
                None,
            )
        };
        view.addTrackingArea(&t);
        t
    }

    /// Wrapped text in `r`; returns the drawn height.
    pub fn draw_wrapped(s: &str, r: Rect, font: &NSFont, color: Rgba) -> f64 {
        let a = style::attrs(font, color);
        let text = NSString::from_str(s);
        let opts = NSStringDrawingOptions::UsesLineFragmentOrigin | NSStringDrawingOptions::UsesFontLeading;
        let b = unsafe {
            text.boundingRectWithSize_options_attributes(NSSize::new(r.width, f64::MAX), opts, Some(&a))
        };
        let h = b.size.height.min(r.height).ceil();
        unsafe {
            text.drawWithRect_options_attributes(
                NSRect::new(NSPoint::new(r.x, r.y), NSSize::new(r.width, h)),
                opts | NSStringDrawingOptions::TruncatesLastVisibleLine,
                Some(&a),
            )
        };
        h
    }

    type Sink = Rc<dyn Fn(JiraEvent)>;

    // ------------------------------------------------------------- table

    pub struct TableIvars {
        snap: Rc<RefCell<JiraSnapshot>>,
        colors: Cell<PopupColors>,
        hover: Cell<Option<usize>>,
        sink: RefCell<Option<Sink>>,
        tracking: RefCell<Option<Retained<NSTrackingArea>>>,
    }

    define_class!(
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSJiraTableView"]
        #[ivars = TableIvars]
        pub struct JiraTableView;

        impl JiraTableView {
            #[unsafe(method(isFlipped))]
            fn is_flipped(&self) -> bool { true }

            #[unsafe(method(acceptsFirstResponder))]
            fn afr(&self) -> bool { true }

            #[unsafe(method(mouseDownCanMoveWindow))]
            fn mdcmw(&self) -> bool { false }

            #[unsafe(method(acceptsFirstMouse:))]
            fn afm(&self, _e: Option<&NSEvent>) -> bool { true }

            #[unsafe(method(updateTrackingAreas))]
            fn uta(&self) {
                let _: () = unsafe { msg_send![super(self), updateTrackingAreas] };
                if let Some(t) = self.ivars().tracking.borrow_mut().take() {
                    self.removeTrackingArea(&t);
                }
                *self.ivars().tracking.borrow_mut() = Some(tracking(self));
            }

            #[unsafe(method(mouseMoved:))]
            fn mouse_moved(&self, e: &NSEvent) {
                let p = self.convertPoint_fromView(e.locationInWindow(), None);
                let n = self.ivars().snap.borrow().rows.len();
                let i = (p.y / ROW_H).floor();
                let h = (i >= 0.0 && (i as usize) < n).then_some(i as usize);
                if h != self.ivars().hover.get() {
                    self.ivars().hover.set(h);
                    self.setNeedsDisplay(true);
                }
            }

            #[unsafe(method(mouseExited:))]
            fn mouse_exited(&self, _e: &NSEvent) {
                self.ivars().hover.set(None);
                self.setNeedsDisplay(true);
            }

            #[unsafe(method(mouseDown:))]
            fn mouse_down(&self, e: &NSEvent) {
                let p = self.convertPoint_fromView(e.locationInWindow(), None);
                let n = self.ivars().snap.borrow().rows.len();
                let i = (p.y / ROW_H).floor();
                if i < 0.0 || i as usize >= n {
                    return;
                }
                let row = i as usize;
                let band = Rect::new(0.0, row as f64 * ROW_H, self.bounds().size.width, ROW_H);
                let ev = if check_rect(band).inset_by(-4.0, -6.0).contains(p.x, p.y) {
                    JiraEvent::Check(row)
                } else if star_rect(band).inset_by(-3.0, -6.0).contains(p.x, p.y) {
                    JiraEvent::Star(row)
                } else {
                    JiraEvent::Click { row, double: e.clickCount() >= 2 }
                };
                self.emit(ev);
            }

            #[unsafe(method(rightMouseDown:))]
            fn right_mouse_down(&self, e: &NSEvent) {
                let p = self.convertPoint_fromView(e.locationInWindow(), None);
                let n = self.ivars().snap.borrow().rows.len();
                let i = (p.y / ROW_H).floor();
                if i < 0.0 || i as usize >= n {
                    return;
                }
                let s = NSEvent::mouseLocation();
                self.emit(JiraEvent::RowMenu { row: i as usize, x: s.x, y: s.y });
            }

            #[unsafe(method(drawRect:))]
            fn draw_rect(&self, dirty: NSRect) {
                self.draw(dirty);
            }
        }

        unsafe impl NSObjectProtocol for JiraTableView {}
    );

    fn check_rect(band: Rect) -> Rect {
        Rect::new(PADDING + 2.0, band.mid_y() - 6.5, 13.0, 13.0)
    }

    fn star_rect(band: Rect) -> Rect {
        Rect::new(PADDING + 2.0 + 22.0 - 1.0, band.mid_y() - 7.0, 14.0, 14.0)
    }

    impl JiraTableView {
        fn new(mtm: MainThreadMarker, snap: Rc<RefCell<JiraSnapshot>>, colors: PopupColors) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(TableIvars {
                snap,
                colors: Cell::new(colors),
                hover: Cell::new(None),
                sink: RefCell::new(None),
                tracking: RefCell::new(None),
            });
            unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] }
        }

        fn emit(&self, ev: JiraEvent) {
            let s = self.ivars().sink.borrow().clone();
            if let Some(s) = s {
                s(ev);
            }
        }

        /// The visible rows' rects (for the vim search highlight overlay).
        pub fn vim_shown_rects(&self) -> Vec<(usize, Rect)> {
            let n = self.ivars().snap.borrow().rows.len();
            let vis = self.visibleRect();
            if n == 0 || vis.size.height <= 0.0 {
                return Vec::new();
            }
            let first = (vis.origin.y / ROW_H).floor().max(0.0) as usize;
            let last = (((vis.origin.y + vis.size.height) / ROW_H).floor() as usize).min(n - 1);
            if first > last {
                return Vec::new();
            }
            let w = self.bounds().size.width;
            (first..=last).map(|i| (i, Rect::new(0.0, i as f64 * ROW_H, w, ROW_H))).collect()
        }

        fn draw(&self, dirty: NSRect) {
            let c = self.ivars().colors.get();
            let snap = self.ivars().snap.borrow();
            let w = self.bounds().size.width;
            if snap.rows.is_empty() {
                let f = style::system_font(12.5, Weight::Regular);
                style::draw_text_centered(&snap.empty, Rect::new(0.0, 24.0, w, 24.0), &f, c.dim);
                return;
            }
            let first = (dirty.origin.y / ROW_H).floor().max(0.0) as usize;
            let last = (((dirty.origin.y + dirty.size.height) / ROW_H).ceil() as usize).min(snap.rows.len());
            let frames = column_frames(&snap.columns, LEAD, w - LEAD - (PADDING + 10.0));
            let font = style::system_font(12.0, Weight::Regular);
            let bold = style::system_font(12.0, Weight::Bold);
            let rules = snap.rules.clone().unwrap_or_else(|| style_rules(&Value::Null));
            let hover = self.ivars().hover.get();
            for i in first..last {
                let r = &snap.rows[i];
                let band = Rect::new(0.0, i as f64 * ROW_H, w, ROW_H);
                let pill = Rect::new(4.0, band.y + 1.0, w - 8.0 - 6.0, band.height - 2.0);
                if i == snap.selection {
                    style::fill_rounded(pill, 5.0, c.highlight);
                    NSGraphicsContext::saveGraphicsState_class();
                    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(ns(pill), 5.0, 5.0).addClip();
                    style::fill_rect(Rect::new(pill.x, pill.y, 3.0, pill.height), c.accent_on());
                    NSGraphicsContext::restoreGraphicsState_class();
                } else if hover == Some(i) {
                    style::fill_rounded(pill, 5.0, style::button_fill(ButtonState::Hover, &c));
                }
                let key = r.get("key").cloned().unwrap_or_default();
                draw_check(check_rect(band), snap.checked.contains(&key), &c);
                draw_star(star_rect(band), snap.favs.contains(&key), &c);
                let cells: Vec<(String, Option<CellStyle>)> = snap
                    .columns
                    .iter()
                    .map(|col| {
                        let flat = display_cell(&col.field, r.get(&col.field).map(String::as_str).unwrap_or(""));
                        let st = if flat.is_empty() { None } else { cell_style(&rules, &col.field, &flat) };
                        (flat, st)
                    })
                    .collect();
                let quiet = cells.iter().any(|(_, s)| s.as_ref().and_then(|s| s.quiets_row) == Some(true));
                let alpha = if quiet { 0.6 } else { 1.0 };
                for (k, col) in snap.columns.iter().enumerate() {
                    let Some(&(fx, fw)) = frames.get(k) else { continue };
                    if fw <= 8.0 {
                        continue;
                    }
                    let (flat, st) = &cells[k];
                    if flat.is_empty() {
                        continue;
                    }
                    let hue = st.as_ref().map(|s| c.tone(tone_of(s.tone.as_deref())));
                    let tinted = st.as_ref().is_some_and(|s| s.tinted == Some(true) || s.tone.as_deref() == Some("dim"));
                    let color = if tinted { hue.unwrap_or(c.text) } else { c.text };
                    let f = if st.as_ref().and_then(|s| s.bold) == Some(true) { &bold } else { &font };
                    let mut cell = Rect::new(fx + 3.0, band.y, fw - 8.0, band.height);
                    if let (Some(s), Some(h)) = (st, hue) {
                        if let Some(mark) = &s.mark {
                            let side = 9.0;
                            draw_mark(
                                Rect::new(cell.x, (band.mid_y() - side / 2.0).round(), side, side),
                                mark,
                                h.with_alpha(h.a * alpha),
                            );
                            cell.x += side + 7.0;
                            cell.width = (cell.width - side - 7.0).max(0.0);
                        }
                    }
                    let col_color = color.with_alpha(color.a * alpha);
                    if col.align == "right" {
                        style::draw_text_right(flat, cell, f, col_color);
                    } else if col.align == "center" {
                        style::draw_text_centered(flat, cell, f, col_color);
                    } else {
                        style::draw_text_left(flat, cell, f, col_color);
                    }
                }
                style::hline(LEAD - 6.0, w - PADDING - 10.0, band.max_y() - 1.0, c.hairline());
            }
        }
    }

    fn draw_check(r: Rect, on: bool, c: &PopupColors) {
        if on {
            style::fill_rounded(r, 3.0, c.accent_on());
            let p = NSBezierPath::bezierPath();
            p.setLineWidth(1.6);
            p.moveToPoint(NSPoint::new(r.x + 3.0, r.mid_y() + 0.5));
            p.lineToPoint(NSPoint::new(r.mid_x() - 0.5, r.max_y() - 3.0));
            p.lineToPoint(NSPoint::new(r.max_x() - 2.5, r.y + 3.0));
            c.on_accent().to_nscolor().setStroke();
            p.stroke();
        } else {
            style::fill_rounded(r, 3.0, c.highlight.with_alpha(0.5));
            style::stroke_rounded(r, 3.0, 1.0, c.dim.with_alpha(0.8));
        }
    }

    fn draw_star(r: Rect, on: bool, c: &PopupColors) {
        let p = NSBezierPath::bezierPath();
        let (cx, cy) = (r.mid_x(), r.mid_y() + 0.5);
        let outer = r.width / 2.0;
        let inner = outer * 0.45;
        for k in 0..10 {
            // Flipped view: start at -90° (up) means +y is down, so negate.
            let a = (-90.0 + k as f64 * 36.0_f64).to_radians();
            let rad = if k % 2 == 0 { outer } else { inner };
            let pt = NSPoint::new(cx + a.cos() * rad, cy + a.sin() * rad);
            if k == 0 { p.moveToPoint(pt) } else { p.lineToPoint(pt) }
        }
        p.closePath();
        p.setLineJoinStyle(objc2_app_kit::NSLineJoinStyle::Round);
        if on {
            c.tone(PopupTone::Warning).to_nscolor().setFill();
            p.fill();
        } else {
            c.dim.with_alpha(0.55).to_nscolor().setStroke();
            p.setLineWidth(1.1);
            p.stroke();
        }
    }

    /// `drawStageMark`.
    fn draw_mark(r: Rect, mark: &str, color: Rgba) {
        let lw = 1.5;
        let bx = r.inset_by(lw / 2.0, lw / 2.0);
        match mark {
            "filled" => style::fill_rounded(r, 2.0, color),
            "half" => {
                NSGraphicsContext::saveGraphicsState_class();
                NSBezierPath::clipRect(ns(Rect::new(r.x, r.y, r.width / 2.0, r.height)));
                style::fill_rounded(r, 2.0, color);
                NSGraphicsContext::restoreGraphicsState_class();
                let p = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(ns(bx), 2.0, 2.0);
                p.setLineWidth(lw);
                color.to_nscolor().setStroke();
                p.stroke();
            }
            _ => {
                let p = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(ns(bx), 2.0, 2.0);
                p.setLineWidth(lw);
                color.to_nscolor().setStroke();
                p.stroke();
            }
        }
    }

    // ------------------------------------------------------------ header

    pub struct HeaderIvars {
        snap: Rc<RefCell<JiraSnapshot>>,
        colors: Cell<PopupColors>,
        sink: RefCell<Option<Sink>>,
    }

    define_class!(
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSJiraHeaderView"]
        #[ivars = HeaderIvars]
        pub struct JiraHeaderView;

        impl JiraHeaderView {
            #[unsafe(method(isFlipped))]
            fn is_flipped(&self) -> bool { true }

            #[unsafe(method(mouseDownCanMoveWindow))]
            fn mdcmw(&self) -> bool { false }

            #[unsafe(method(mouseDown:))]
            fn mouse_down(&self, e: &NSEvent) {
                let p = self.convertPoint_fromView(e.locationInWindow(), None);
                let w = self.bounds().size.width;
                let cols = self.ivars().snap.borrow().columns.clone();
                let frames = column_frames(&cols, LEAD, w - LEAD - (PADDING + 10.0));
                if let Some(i) = frames.iter().position(|(x, fw)| p.x >= *x && p.x < x + fw) {
                    let s = self.ivars().sink.borrow().clone();
                    if let Some(s) = s {
                        s(JiraEvent::Sort(i));
                    }
                }
            }

            #[unsafe(method(rightMouseDown:))]
            fn right_mouse_down(&self, _e: &NSEvent) {
                let s = self.ivars().sink.borrow().clone();
                if let Some(s) = s {
                    let m = NSEvent::mouseLocation();
                    s(JiraEvent::HeaderMenu { x: m.x, y: m.y });
                }
            }

            #[unsafe(method(drawRect:))]
            fn draw_rect(&self, _dirty: NSRect) {
                self.draw();
            }
        }

        unsafe impl NSObjectProtocol for JiraHeaderView {}
    );

    impl JiraHeaderView {
        fn new(mtm: MainThreadMarker, snap: Rc<RefCell<JiraSnapshot>>, colors: PopupColors) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(HeaderIvars { snap, colors: Cell::new(colors), sink: RefCell::new(None) });
            unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] }
        }

        fn draw(&self) {
            let c = self.ivars().colors.get();
            let b = self.bounds();
            let (w, h) = (b.size.width, b.size.height);
            style::fill_rect(Rect::new(0.0, 0.0, w, h), c.mantle());
            let snap = self.ivars().snap.borrow();
            let frames = column_frames(&snap.columns, LEAD, w - LEAD - (PADDING + 10.0));
            let font = style::system_font(12.0 * 0.86, Weight::Bold);
            // The "fit columns" glyph in the lead gutter.
            style::draw_symbol("arrow.left.and.right", Rect::new(LEAD / 2.0 + 6.0, 0.0, 16.0, h), c.dim.with_alpha(0.9), 9.0);
            for (i, col) in snap.columns.iter().enumerate() {
                let Some(&(fx, fw)) = frames.get(i) else { continue };
                let mut title = col.title.to_uppercase();
                let sorted = snap.sort.map(|(si, _)| si) == Some(i);
                if sorted {
                    title.push_str(if snap.sort.unwrap().1 { " ↑" } else { " ↓" });
                }
                let color = if sorted { c.accent_on() } else { c.dim };
                let filter = col.filterable && fw > 30.0;
                let fr = Rect::new(fx + fw - 16.0 - 5.0, h / 2.0 - 8.0, 16.0, 16.0);
                let tw = (fw - 8.0 - if filter { 18.0 } else { 0.0 }).max(0.0);
                let r = Rect::new(fx + 3.0, 0.0, tw, h);
                let spaced = kerned(&title);
                if col.align == "right" {
                    style::draw_text_right(&spaced, r, &font, color);
                } else {
                    style::draw_text_left(&spaced, r, &font, color);
                }
                if filter {
                    style::chevron(fr.inset_by(4.0, 4.0), c.dim.with_alpha(0.9));
                }
                if i + 1 < snap.columns.len() {
                    style::vline(fx + fw - 0.5, 6.0, h - 6.0, c.hairline());
                }
            }
            style::hline(0.0, w, h - 1.0, c.hairline());
        }
    }

    /// Approximate `.kern: 0.6` with hair spaces between letters.
    fn kerned(s: &str) -> String {
        let mut out = String::new();
        for (i, ch) in s.chars().enumerate() {
            if i > 0 && ch != ' ' {
                out.push('\u{200A}');
            }
            out.push(ch);
        }
        out
    }

    // ---------------------------------------------------------- inspector

    pub struct InspectorIvars {
        snap: Rc<RefCell<JiraSnapshot>>,
        colors: Cell<PopupColors>,
        hover: Cell<bool>,
        sink: RefCell<Option<Sink>>,
        tracking: RefCell<Option<Retained<NSTrackingArea>>>,
    }

    define_class!(
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSJiraInspectorView"]
        #[ivars = InspectorIvars]
        pub struct JiraInspectorView;

        impl JiraInspectorView {
            #[unsafe(method(isFlipped))]
            fn is_flipped(&self) -> bool { true }

            #[unsafe(method(mouseDownCanMoveWindow))]
            fn mdcmw(&self) -> bool { false }

            #[unsafe(method(updateTrackingAreas))]
            fn uta(&self) {
                let _: () = unsafe { msg_send![super(self), updateTrackingAreas] };
                if let Some(t) = self.ivars().tracking.borrow_mut().take() {
                    self.removeTrackingArea(&t);
                }
                *self.ivars().tracking.borrow_mut() = Some(tracking(self));
            }

            #[unsafe(method(mouseMoved:))]
            fn mouse_moved(&self, e: &NSEvent) {
                let p = self.convertPoint_fromView(e.locationInWindow(), None);
                let on = self.button_rect().contains(p.x, p.y);
                if on != self.ivars().hover.get() {
                    self.ivars().hover.set(on);
                    self.setNeedsDisplay(true);
                }
            }

            #[unsafe(method(mouseDown:))]
            fn mouse_down(&self, e: &NSEvent) {
                let p = self.convertPoint_fromView(e.locationInWindow(), None);
                if self.button_rect().contains(p.x, p.y) && self.ivars().snap.borrow().inspector.is_some() {
                    let s = self.ivars().sink.borrow().clone();
                    if let Some(s) = s {
                        s(JiraEvent::OpenDetail);
                    }
                }
            }

            #[unsafe(method(drawRect:))]
            fn draw_rect(&self, _dirty: NSRect) {
                self.draw();
            }
        }

        unsafe impl NSObjectProtocol for JiraInspectorView {}
    );

    impl JiraInspectorView {
        fn new(mtm: MainThreadMarker, snap: Rc<RefCell<JiraSnapshot>>, colors: PopupColors) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(InspectorIvars {
                snap,
                colors: Cell::new(colors),
                hover: Cell::new(false),
                sink: RefCell::new(None),
                tracking: RefCell::new(None),
            });
            unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] }
        }

        fn button_rect(&self) -> Rect {
            let b = self.bounds();
            let footer = 70.0;
            let bw = (b.size.width - 36.0).max(160.0);
            Rect::new((b.size.width - bw) / 2.0, b.size.height - footer + 8.0, bw, 32.0)
        }

        fn draw(&self) {
            let c = self.ivars().colors.get();
            let b = self.bounds();
            let (w, h) = (b.size.width, b.size.height);
            style::fill_rect(Rect::new(0.0, 0.0, w, h), c.mantle().with_alpha(0.7));
            style::fill_rect(Rect::new(0.0, 0.0, 1.0, h), c.hairline());
            let snap = self.ivars().snap.borrow();
            let Some(ic) = &snap.inspector else {
                let f = style::system_font(13.0, Weight::Regular);
                style::draw_text(&"Select an issue".to_string(), 15.0, 16.0, &f, c.dim);
                return;
            };
            let x = 15.0;
            let tw = w - 30.0;
            let mut y = 14.0;
            y += draw_wrapped(&ic.key, Rect::new(x, y, tw, 40.0), &style::system_font(12.5, Weight::Bold), c.accent) + 4.0;
            y += draw_wrapped(&ic.title, Rect::new(x, y, tw, 120.0), &style::system_font(16.0, Weight::Semibold), c.text) + 10.0;
            if !ic.chips.is_empty() {
                let f = style::system_font(12.0, Weight::Semibold);
                let mut cx = x;
                for (t, tone) in &ic.chips {
                    let col = c.tone(*tone);
                    let (cw, ch) = style::text_size(t, &f);
                    let r = Rect::new(cx, y, cw + 6.0, ch + 2.0);
                    style::fill_rect(r, col.with_alpha(0.18));
                    style::draw_text(t, cx + 3.0, y + 1.0, &f, col);
                    cx += r.width + 8.0;
                }
                y += 30.0;
            }
            if !ic.body.is_empty() {
                let body: String = if ic.body.chars().count() > 700 {
                    ic.body.chars().take(700).collect::<String>() + "…"
                } else {
                    ic.body.clone()
                };
                y += draw_wrapped(&body, Rect::new(x, y, tw, (h - 90.0 - y).max(0.0)), &style::system_font(13.0, Weight::Regular), c.text) + 14.0;
            }
            let lf = style::system_font(12.0, Weight::Regular);
            let vf = style::system_font(12.5, Weight::Regular);
            for (label, value) in &ic.fields {
                if value.is_empty() {
                    continue;
                }
                if y > h - 90.0 {
                    break;
                }
                style::draw_text(label, x, y, &lf, c.dim);
                let vh = draw_wrapped(value, Rect::new(x + 84.0, y - 0.5, tw - 84.0, 60.0), &vf, c.text);
                y += vh.max(16.0) + 5.0;
            }
            let br = self.button_rect();
            let fill = if self.ivars().hover.get() { c.accent_on().with_alpha(0.28) } else { c.accent_on().with_alpha(0.18) };
            style::fill_rounded(br, 8.0, fill);
            let bf = style::system_font(13.0, Weight::Semibold);
            let (_, bh) = style::text_size("Open full detail", &bf);
            style::draw_text("Open full detail", br.x + 14.0, br.mid_y() - bh / 2.0, &bf, c.accent);
            let kc = Rect::new(br.max_x() - 30.0, br.mid_y() - 9.0, 20.0, 18.0);
            style::fill_rounded(kc, 4.0, c.accent_on().with_alpha(0.25));
            style::draw_text_centered("⏎", kc, &style::system_font(10.0, Weight::Semibold), c.readable(c.accent, 4.0));
            let hf = style::system_font(11.0, Weight::Regular);
            style::draw_text_centered("⌘I hides this panel", Rect::new(8.0, h - 70.0 + 42.0, w - 16.0, 16.0), &hf, c.dim);
        }
    }

    // ---------------------------------------------------------- container

    pub struct JiraIvars {
        snap: Rc<RefCell<JiraSnapshot>>,
        colors: Cell<PopupColors>,
        sidebar: Retained<SidebarView>,
        search: Retained<NSTextField>,
        search_box: Retained<NSView>,
        header: Retained<JiraHeaderView>,
        scroll: Retained<NSScrollView>,
        table: Retained<JiraTableView>,
        inspector: Retained<JiraInspectorView>,
        sink: RefCell<Option<Sink>>,
        sidebar_w: Cell<f64>,
        inspector_w: Cell<f64>,
    }

    define_class!(
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSJiraView"]
        #[ivars = JiraIvars]
        pub struct JiraView;

        impl JiraView {

            #[unsafe(method(isFlipped))]
            fn is_flipped(&self) -> bool { true }

            #[unsafe(method(setFrameSize:))]
            fn set_frame_size(&self, size: NSSize) {
                let _: () = unsafe { msg_send![super(self), setFrameSize: size] };
                self.layout_panes();
            }

            #[unsafe(method(controlTextDidChange:))]
            fn control_text_did_change(&self, _n: &NSNotification) {
                let q = self.ivars().search.stringValue().to_string();
                self.emit(JiraEvent::Query(q));
            }

            #[unsafe(method(control:textView:doCommandBySelector:))]
            fn do_command(&self, _c: &AnyObject, _tv: &NSTextView, cmd: Sel) -> objc2::runtime::Bool {
                let ev = if cmd == sel!(insertNewline:) {
                    JiraEvent::Enter
                } else if cmd == sel!(moveDown:) {
                    JiraEvent::Move(1)
                } else if cmd == sel!(moveUp:) {
                    JiraEvent::Move(-1)
                } else if cmd == sel!(cancelOperation:) {
                    JiraEvent::Escape
                } else {
                    return objc2::runtime::Bool::NO;
                };
                self.emit(ev);
                objc2::runtime::Bool::YES
            }
        }

        unsafe impl NSObjectProtocol for JiraView {}
        unsafe impl NSControlTextEditingDelegate for JiraView {}
        unsafe impl NSTextFieldDelegate for JiraView {}
    );

    impl JiraView {
        /// `fitTableColumns(sample:)`: measure each column's header and up to
        /// `sample` rows' cells, and solve percentages for the header width.
        pub fn fit_columns(&self, cols: &[ListColumn], rows: &[Record], sample: usize) -> Vec<f64> {
            let font = style::system_font(12.0, Weight::Regular);
            let bold = style::system_font(12.0, Weight::Bold);
            let hfont = style::system_font(12.0 * 0.86, Weight::Bold);
            let need: Vec<f64> = cols
                .iter()
                .enumerate()
                .map(|(i, col)| {
                    let mut w = style::text_size(&kerned(&format!("{} ↑", col.title.to_uppercase())), &hfont).0 + 12.0;
                    if col.filterable {
                        w += 22.0;
                    }
                    for r in rows.iter().take(sample) {
                        let flat = display_cell(&col.field, r.get(&col.field).map(String::as_str).unwrap_or(""));
                        if flat.is_empty() {
                            continue;
                        }
                        let f = if i == 0 { &bold } else { &font };
                        w = w.max(style::text_size(&flat.replace('\n', " "), f).0 + 22.0);
                    }
                    w.ceil()
                })
                .collect();
            let width = self.ivars().header.bounds().size.width;
            fit_column_pcts(&need, width - LEAD - (PADDING + 10.0))
        }
    }

    impl JiraView {
        /// The rail (`sidebarJumpItems` / `do:jira-jump`).
        pub fn sidebar(&self) -> Retained<SidebarView> {
            self.ivars().sidebar.clone()
        }

        /// The issue list's document view plus each visible row's rect (the vim
        /// search highlight draws the overlay into it).
        pub fn vim_shown_rows(&self) -> (Retained<NSView>, Vec<(usize, Rect)>) {
            let t = self.ivars().table.clone();
            let rects = t.vim_shown_rects();
            (Retained::into_super(t), rects)
        }

        /// `JiraDashboard.navPanes`: sidebar, search, list, issue.
        pub fn nav_panes(&self) -> Vec<crate::app::live_panes::LivePane> {
            use crate::app::live_panes::LivePane;
            let iv = self.ivars();
            vec![
                LivePane::new("sidebar", &iv.sidebar),
                LivePane::new("search", &iv.search_box).focusing(&iv.search),
                LivePane::new("list", &iv.scroll).focusing(&iv.table),
                LivePane::new("issue", &iv.inspector),
            ]
        }
        pub fn new(mtm: MainThreadMarker, colors: PopupColors, sidebar_w: f64, inspector_w: f64) -> Retained<Self> {
            let snap = Rc::new(RefCell::new(JiraSnapshot::default()));
            let sidebar = SidebarView::new(mtm, colors);
            sidebar.set_collapse_key("jira");
            let (search_box, search) = crate::ui::field::search_box(mtm, &colors, 13.0, "search...", 6.0, 28.0);
            let header = JiraHeaderView::new(mtm, snap.clone(), colors);
            let scroll = NSScrollView::new(mtm);
            scroll.setHasVerticalScroller(true);
            scroll.setAutohidesScrollers(true);
            scroll.setDrawsBackground(false);
            let table = JiraTableView::new(mtm, snap.clone(), colors);
            scroll.setDocumentView(Some(&table));
            let inspector = JiraInspectorView::new(mtm, snap.clone(), colors);
            let this = Self::alloc(mtm).set_ivars(JiraIvars {
                snap,
                colors: Cell::new(colors),
                sidebar,
                search,
                search_box,
                header,
                scroll,
                table,
                inspector,
                sink: RefCell::new(None),
                sidebar_w: Cell::new(sidebar_w),
                inspector_w: Cell::new(inspector_w),
            });
            let v: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
            let iv = v.ivars();
            for sub in [&*iv.sidebar as &NSView, &iv.search_box, &iv.header, &iv.scroll, &iv.inspector] {
                v.addSubview(sub);
            }
            unsafe { iv.search.setDelegate(Some(ProtocolObject::from_ref(&*v))) };
            v
        }

        pub fn set_on_event(&self, cb: Box<dyn Fn(JiraEvent)>) {
            let rc: Sink = Rc::from(cb);
            *self.ivars().sink.borrow_mut() = Some(rc.clone());
            *self.ivars().table.ivars().sink.borrow_mut() = Some(rc.clone());
            *self.ivars().header.ivars().sink.borrow_mut() = Some(rc.clone());
            *self.ivars().inspector.ivars().sink.borrow_mut() = Some(rc.clone());
            self.ivars().sidebar.set_on_event(Box::new(move |e| rc(JiraEvent::Sidebar(e))));
        }

        fn emit(&self, ev: JiraEvent) {
            let s = self.ivars().sink.borrow().clone();
            if let Some(s) = s {
                s(ev);
            }
        }

        pub fn table_view(&self) -> Retained<NSView> {
            Retained::into_super(self.ivars().table.clone())
        }

        pub fn search_field(&self) -> Retained<NSTextField> {
            self.ivars().search.clone()
        }

        pub fn apply(&self, s: JiraSnapshot) {
            let iv = self.ivars();
            iv.sidebar.set_model(s.sidebar.clone());
            let editing = iv.search.currentEditor().is_some();
            if !editing && iv.search.stringValue().to_string() != s.query {
                iv.search.setStringValue(&NSString::from_str(&s.query));
            }
            let rows_changed = {
                let old = iv.snap.borrow();
                old.rows.len() != s.rows.len()
                    || old.rows.first().and_then(|r| r.get("key")) != s.rows.first().and_then(|r| r.get("key"))
                    || old.columns != s.columns
            };
            *iv.snap.borrow_mut() = s;
            self.layout_panes();
            if rows_changed {
                iv.scroll.contentView().scrollToPoint(NSPoint::new(0.0, 0.0));
                iv.scroll.reflectScrolledClipView(&iv.scroll.contentView());
            }
            self.scroll_selection_visible();
            for v in [&*iv.table as &NSView, &iv.header, &iv.inspector] {
                v.setNeedsDisplay(true);
            }
        }

        pub fn layout_panes(&self) {
            let iv = self.ivars();
            let b = self.bounds();
            let (w, h) = (b.size.width, b.size.height);
            let sw = iv.sidebar.width(iv.sidebar_w.get());
            let show_insp = iv.snap.borrow().show_inspector;
            let insp = if show_insp { iv.inspector_w.get().min(w * 0.4) } else { 0.0 };
            iv.sidebar.setFrame(ns(Rect::new(0.0, 0.0, sw, h)));
            let x0 = sw + 4.0;
            let mw = (w - insp - x0).max(100.0);
            iv.search_box.setFrame(ns(Rect::new(x0 + 22.0, 9.0, (mw * 0.9 - 22.0).max(80.0), 28.0)));
            iv.header.setFrame(ns(Rect::new(x0, 50.0, mw, HEADER_H)));
            let ty = 50.0 + HEADER_H + 2.0;
            iv.scroll.setFrame(ns(Rect::new(x0, ty, mw, (h - ty).max(0.0))));
            let n = iv.snap.borrow().rows.len();
            let doc_h = (n as f64 * ROW_H).max(h - ty);
            iv.table.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(mw, doc_h)));
            iv.inspector.setHidden(!show_insp);
            iv.inspector.setFrame(ns(Rect::new(w - insp, 0.0, insp, h)));
        }

        fn scroll_selection_visible(&self) {
            let iv = self.ivars();
            let sel = iv.snap.borrow().selection;
            let clip = iv.scroll.contentView();
            let vis = clip.bounds();
            if vis.size.height < ROW_H {
                return;
            }
            let (top, bot) = (sel as f64 * ROW_H, (sel + 1) as f64 * ROW_H);
            let mut t = vis.origin.y;
            if bot > vis.origin.y + vis.size.height {
                t = bot - vis.size.height;
            } else if top < vis.origin.y {
                t = top;
            }
            if t != vis.origin.y {
                clip.scrollToPoint(NSPoint::new(0.0, t));
                iv.scroll.reflectScrolledClipView(&clip);
            }
        }

        pub fn sidebar_collapsed_changed(&self) {
            self.layout_panes();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn fit_column_pcts_fits_and_shrinks_the_widest() {
        // Room to spare: plain percentages of the usable width.
        assert_eq!(fit_column_pcts(&[100.0, 50.0], 1000.0), vec![10.0, 5.0]);
        // Too wide: the widest columns shrink together until the total fits.
        let p = fit_column_pcts(&[400.0, 400.0, 100.0], 500.0);
        assert!((p.iter().sum::<f64>() - 100.0).abs() < 0.5, "{p:?}");
        assert_eq!(p[0], p[1]);
        assert_eq!(p[2], 20.0);
    }

    fn col(field: &str, w: f64) -> ListColumn {
        ListColumn { field: field.into(), title: field.into(), width: w, align: "left".into(), sortable: true, filterable: true }
    }

    #[test]
    fn frames_follow_percent_widths() {
        let f = column_frames(&[col("a", 25.0), col("b", 75.0)], 10.0, 400.0);
        assert_eq!(f, vec![(10.0, 100.0), (110.0, 300.0)]);
    }

    #[test]
    fn frames_scale_down_over_100() {
        let f = column_frames(&[col("a", 100.0), col("b", 100.0)], 0.0, 400.0);
        assert!((f[0].1 - 200.0).abs() < 1e-9 && (f[1].0 - 200.0).abs() < 1e-9);
    }

    #[test]
    fn autos_share_leftover() {
        let f = column_frames(&[col("a", 50.0), col("b", 0.0), col("c", 0.0)], 0.0, 100.0);
        assert_eq!(f[1].1, 25.0);
        assert_eq!(f[2].1, 25.0);
    }

    #[test]
    fn iso_updated_displays_short() {
        assert_eq!(display_cell("updated", "2026-10-08T20:14:20.595-0400"), "2026-10-08 20:14");
        assert_eq!(display_cell("title", "a\nb"), "a b");
    }

    #[test]
    fn status_styles_follow_word_rules() {
        let mut v = serde_json::Map::new();
        v.insert("status-done-words".into(), Value::String("done, closed".into()));
        v.insert("status-active-words".into(), Value::String("progress".into()));
        let r = style_rules(&Value::Object(v));
        assert_eq!(cell_style(&r, "status", "Done").unwrap().mark.as_deref(), Some("filled"));
        assert_eq!(cell_style(&r, "status", "In Progress").unwrap().mark.as_deref(), Some("half"));
        assert_eq!(cell_style(&r, "status", "To Do").unwrap().mark.as_deref(), Some("hollow"));
        assert!(cell_style(&r, "title", "x").is_none());
    }

    #[test]
    fn sort_toggles_and_filters() {
        let mut l = JiraList::default();
        l.columns = vec![col("key", 50.0), col("title", 50.0)];
        l.section.insert("filter".into(), "key,title".into());
        for (k, t) in [("A-2", "beta"), ("A-1", "alpha"), ("A-3", "gamma")] {
            let mut r = Record::new();
            r.insert("key".into(), k.into());
            r.insert("title".into(), t.into());
            l.all.push(r);
        }
        l.sort_by(0);
        assert_eq!(l.row(0).unwrap()["key"], "A-1");
        l.sort_by(0);
        assert_eq!(l.row(0).unwrap()["key"], "A-3");
        l.query = "alp".into();
        l.refilter();
        assert_eq!(l.rows.len(), 1);
    }

    #[test]
    fn handle_applies_user_events_to_the_model() {
        let mut l = JiraList::default();
        l.columns = vec![col("key", 50.0), col("title", 50.0)];
        l.section.insert("filter".into(), "key,title".into());
        for (k, t) in [("A-2", "beta"), ("A-1", "alpha")] {
            let mut r = Record::new();
            r.insert("key".into(), k.into());
            r.insert("title".into(), t.into());
            l.all.push(r);
        }
        l.refilter();
        let before = l.snapshot();

        l.handle(JiraEvent::Click { row: 1, double: false });
        assert_eq!(l.snapshot().selection, 1);
        assert_ne!(l.snapshot(), before, "Click moves the selection");

        l.handle(JiraEvent::Star(1));
        assert!(l.favs.contains("A-1"));

        l.handle(JiraEvent::Query("alp".into()));
        assert_eq!(l.snapshot().rows.len(), 1);

        let filtered = l.snapshot();
        l.handle(JiraEvent::Escape);
        assert!(l.query.is_empty());
        assert_ne!(l.snapshot(), filtered, "Escape clears the query");

        // Two rows again after Escape; a big step clamps to the last row.
        l.handle(JiraEvent::Move(5));
        assert_eq!(l.snapshot().selection, 1);

        let unsorted = l.snapshot();
        l.handle(JiraEvent::Sort(0));
        assert_ne!(l.snapshot(), unsorted, "Sort reorders the rows");

        l.handle(JiraEvent::RowMenu { row: 0, x: 0.0, y: 0.0 });
        assert_eq!(l.snapshot().selection, 0);
    }

    #[test]
    fn record_flattens_arrays() {
        let r = record_of(&serde_json::json!({"key": "K-1", "labels": ["a", "b"], "n": 3})).unwrap();
        assert_eq!(r["labels"], "a, b");
        assert_eq!(r["n"], "3");
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn parse_stamp_fields_matches_the_formatter_format() {
        let dt = parse_stamp_fields("2026-10-10 23:35:09").expect("valid stamp");
        assert_eq!((dt.year(), u8::from(dt.month()), dt.day()), (2026, 10, 10));
        assert_eq!((dt.hour(), dt.minute(), dt.second()), (23, 35, 9));
        assert!(parse_stamp_fields("").is_none());
        assert!(parse_stamp_fields("2026-10-10T23:35:09").is_none());
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn parse_stamp_local_parses_and_rejects() {
        assert!(parse_stamp_local("2026-10-10 23:35:09").is_some());
        assert!(parse_stamp_local("").is_none());
        assert!(parse_stamp_local("nope").is_none());
    }

    /// The board flow end to end against fixture files (never Jira): the
    /// catalog, the board columns spec, the board's issues and config.json.
    #[test]
    fn board_list_open_columns_and_pins_from_fixtures() {
        let base = std::env::temp_dir().join(format!("ws-jira-board-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let cache = base.join("cache");
        let boards_dir = base.join("jira_boards");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::create_dir_all(&boards_dir).unwrap();

        // board_catalog.json: one project -> board 42, one active sprint.
        std::fs::write(
            cache.join("board_catalog.json"),
            json!({
                "projects": [{"key": "DEMO", "boards": [{
                    "id": "42", "name": "Demo board", "type": "scrum",
                    "sprints": [{"id": "7", "name": "Sprint 7", "state": "active"}]
                }]}]
            })
            .to_string(),
        )
        .unwrap();
        // boards.json: board 42's column spec (statuses -> columns).
        std::fs::write(
            cache.join("boards.json"),
            json!({
                "42": {"name": "Demo board", "type": "scrum", "columns": [
                    {"name": "Ready", "statuses": ["To Do"]},
                    {"name": "Doing", "statuses": ["In Progress"]}
                ]}
            })
            .to_string(),
        )
        .unwrap();
        // board-42.json: the board's issues (the poller's output).
        std::fs::write(
            boards_dir.join("board-42.json"),
            json!([
                {"key": "DEMO-1", "title": "One", "status": "To Do", "type": "Bug",
                 "priority": "High", "assignee": ""},
                {"key": "DEMO-2", "title": "Two", "status": "In Progress", "type": "Task",
                 "priority": "Low", "assignee": ""}
            ])
            .to_string(),
        )
        .unwrap();
        let config_json = base.join("config.json");
        std::fs::write(&config_json, json!({"pinnedBoards": ["42"], "pinnedBoardViews": []}).to_string()).unwrap();

        let paths = JiraPaths {
            config_json: config_json.to_string_lossy().into_owned(),
            team_json: String::new(),
            legacy_config: String::new(),
            cache_dir: cache.to_string_lossy().into_owned(),
            out_dir: base.join("jira_json").to_string_lossy().into_owned(),
            cache: [
                ("boardCatalog".to_string(), "board_catalog.json".to_string()),
                ("boards".to_string(), "boards.json".to_string()),
            ]
            .into_iter()
            .collect(),
            tabs: BTreeMap::new(),
            side_dirs: [("boards".to_string(), "jira_boards".to_string())].into_iter().collect(),
        };
        let section: HashMap<String, String> = [(
            "columns".to_string(),
            "key:Key:20:left, title:Title:60:left, status:Status:20:left".to_string(),
        )]
        .into_iter()
        .collect();
        let mut l = JiraList::new(section, Some(paths));

        // The catalog populated from board_catalog.json.
        assert_eq!(l.catalog.len(), 1);
        assert_eq!(l.catalog[0], ("42".to_string(), "Demo board".to_string(), "DEMO".to_string()));
        assert_eq!(l.board_sprints("42").len(), 1);
        assert_eq!(l.default_sprint("42"), "current");

        // list -> the board list rows.
        l.show_board_list();
        assert!(l.on_board_list());
        let st = l.board_state();
        assert_eq!(st["list"], json!(true));
        assert_eq!(st["rows"], json!(1));
        assert_eq!(st["pinned"], json!(["42"]));
        assert_eq!(l.snapshot().rows.len(), 1);

        // open -> the board's columns + cards, sprint/mode pinned.
        assert!(l.open_board("42"));
        assert_eq!(l.on_board().as_deref(), Some("42"));
        assert_eq!(l.board_cols.len(), 2);
        assert_eq!(l.board_cols[0].name, "Ready");
        assert_eq!(l.board_cols[0].cards[0].key, "DEMO-1");
        let st = l.board_state();
        assert_eq!(st["id"], json!("42"));
        assert_eq!(st["sprint"], json!("current"));
        assert_eq!(st["mode"], json!("columns"));
        assert_eq!(st["columns"], json!(true));
        assert_eq!(st["bar"], json!(true));
        assert_eq!(st["sprintKeys"], json!(-1));
        assert_eq!(st["catalog"], json!(["42"]));

        // columns mode draws the board table (column + key fields).
        let snap = l.snapshot();
        assert!(snap.columns.iter().any(|c| c.field == "column"));
        assert!(snap.rows.iter().any(|r| r.get("key").map(String::as_str) == Some("DEMO-1")));

        // table mode draws the issue rows instead.
        assert!(l.set_mode("table"));
        assert_eq!(l.board_state()["mode"], json!("table"));
        assert!(!l.snapshot().columns.iter().any(|c| c.field == "column"));
        assert!(l.snapshot().rows.iter().any(|r| r.get("key").map(String::as_str) == Some("DEMO-1")));

        // sprint pick + pinned view (in-memory; survives a refresh).
        assert!(l.pick_sprint("7"));
        assert_eq!(l.board_state()["sprint"], json!("7"));
        assert!(l.toggle_pin_view("42|7|table"));
        assert!(l.pinned_views.contains(&"42|7|table".to_string()));
        l.refresh();
        assert!(l.pinned_views.contains(&"42|7|table".to_string()));

        // list-star unpins the board; pin re-adds it.
        assert!(l.toggle_pin_board("42"));
        assert!(l.boards.iter().all(|(b, _)| b != "42"));
        l.pin_board("42");
        assert!(l.boards.iter().any(|(b, _)| b == "42"));

        // open-view spec parses BOARD|SPRINT|MODE.
        assert!(l.open_view("42|current|columns"));
        assert_eq!(l.board_state()["sprint"], json!("current"));
        assert_eq!(l.board_state()["mode"], json!("columns"));

        let _ = std::fs::remove_dir_all(&base);
    }
}
