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
use crate::views::jira::{
    base_field_label, board_columns, parse_columns_value, style_rules, BoardColumn, CellStyle,
    JiraBoardBarModel, SprintChoice, StyleRules,
};
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

/// The Jira view's top-level surface: the ticket list or the Config dashboard
/// — the `Tickets | Config` pill. Reported as `views.jira.tab`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum JiraSurface {
    #[default]
    Tickets,
    Config,
}

impl JiraSurface {
    pub fn raw(self) -> &'static str {
        match self {
            JiraSurface::Tickets => "tickets",
            JiraSurface::Config => "config",
        }
    }
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
    /// `start` / `end` / `complete` (ISO dates) for the bar's summary.
    pub start: String,
    pub end: String,
    pub complete: String,
}

/// `PopupTableColumn.frames(x0:usable:)` — percent widths, autos share the
/// leftover, scaled down when they sum past 100.
/// Compact cells (`[jira] compact-cells`, on by default): these fields draw
/// as a glyph / initials / short value in a fixed narrow column, so the
/// title keeps the room. `None` = a normal percentage column.
pub fn compact_width(field: &str) -> Option<f64> {
    match field {
        "status" | "assignee" | "reporter" | "creator" => Some(50.0),
        "priority" => Some(46.0),
        "updated" | "created" | "resolved" => Some(44.0),
        "project" => Some(70.0),
        "key" => Some(76.0),
        _ => None,
    }
}

/// The raw width of each column: fixed columns (an explicit `width_px`, or a
/// compact field) at their px, the rest sharing the leftover by percentage.
fn column_widths(cols: &[ListColumn], usable: f64, compact: bool) -> Vec<f64> {
    let fixed_of = |c: &ListColumn| c.width_px.or_else(|| if compact { compact_width(&c.field) } else { None });
    if !cols.iter().any(|c| fixed_of(c).is_some()) {
        return column_frames(cols, 0.0, usable).into_iter().map(|(_, w)| w).collect();
    }
    let fixed: f64 = cols.iter().filter_map(&fixed_of).sum();
    let mut flex: Vec<ListColumn> = cols.iter().filter(|c| fixed_of(c).is_none()).cloned().collect();
    // The percentages were set with the fixed columns in them: stretch the
    // rest so they fill the room those columns gave up.
    let sum: f64 = flex.iter().map(|c| c.width.max(0.0)).sum();
    if sum > 0.0 && flex.iter().all(|c| c.width > 0.0) {
        for c in flex.iter_mut() {
            c.width = c.width / sum * 100.0;
        }
    }
    let flex_frames = column_frames(&flex, 0.0, (usable - fixed).max(40.0));
    let mut fi = 0;
    cols.iter()
        .map(|c| match fixed_of(c) {
            Some(w) => w,
            None => {
                let w = flex_frames.get(fi).map_or(0.0, |f| f.1);
                fi += 1;
                w
            }
        })
        .collect()
}

fn lay_out(widths: &[f64], x0: f64) -> Vec<(f64, f64)> {
    let mut x = x0;
    widths
        .iter()
        .map(|w| {
            let f = (x, *w);
            x += *w;
            f
        })
        .collect()
}

/// [`column_frames`], with compact columns at their fixed width and the
/// rest sharing what is left by their percentages.
pub fn frames_for(cols: &[ListColumn], x0: f64, usable: f64, compact: bool) -> Vec<(f64, f64)> {
    if cols.is_empty() || usable <= 0.0 {
        return cols.iter().map(|_| (x0, 0.0)).collect();
    }
    lay_out(&column_widths(cols, usable, compact), x0)
}

/// [`frames_for`], but every column reserves at least `mins[i]` px (its full
/// header label) — the extra space is taken from the columns that have room,
/// so no header is clipped. `mins` shorter than `cols` leaves the tail alone.
pub fn frames_for_reserved(
    cols: &[ListColumn],
    x0: f64,
    usable: f64,
    compact: bool,
    mins: &[f64],
) -> Vec<(f64, f64)> {
    if cols.is_empty() || usable <= 0.0 {
        return cols.iter().map(|_| (x0, 0.0)).collect();
    }
    let mut w = column_widths(cols, usable, compact);
    let floor: Vec<f64> = (0..w.len()).map(|i| mins.get(i).copied().unwrap_or(0.0)).collect();
    for (wi, m) in w.iter_mut().zip(&floor) {
        *wi = wi.max(*m);
    }
    let total: f64 = w.iter().sum();
    if total > usable {
        let slack: f64 = w.iter().zip(&floor).map(|(wi, m)| (wi - m).max(0.0)).sum();
        if slack > 0.0 {
            let factor = ((slack - (total - usable)) / slack).max(0.0);
            for i in 0..w.len() {
                w[i] = floor[i] + (w[i] - floor[i]).max(0.0) * factor;
            }
        }
        // The floors alone exceed the width: scale everything down to fit.
        let total2: f64 = w.iter().sum();
        if total2 > usable {
            let s = usable / total2;
            for wi in w.iter_mut() {
                *wi *= s;
            }
        }
    }
    lay_out(&w, x0)
}

/// Two-letter initials for a person cell ("Ada Lovelace" / "ada.lovelace" → AL).
pub fn initials(name: &str) -> String {
    let base = name.split(" (").next().unwrap_or(name);
    let parts: Vec<&str> = base.split(|c: char| c.is_whitespace() || c == '.' || c == '_' || c == '-').filter(|p| !p.is_empty()).collect();
    let mut out: String = parts.iter().take(2).filter_map(|p| p.chars().next()).collect();
    if out.chars().count() < 2 {
        out = base.chars().filter(|c| c.is_alphanumeric()).take(2).collect();
    }
    out.to_uppercase()
}

/// Days since 1970-01-01 for a civil date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Epoch seconds of a Jira timestamp (`2026-10-08T20:14:20.595-0400`).
pub fn parse_jira_time(v: &str) -> Option<i64> {
    let b = v.as_bytes();
    if b.len() < 16 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' {
        return None;
    }
    let n = |a: usize, z: usize| v.get(a..z)?.parse::<i64>().ok();
    let (y, mo, d, h, mi) = (n(0, 4)?, n(5, 7)?, n(8, 10)?, n(11, 13)?, n(14, 16)?);
    let sec = if b.len() >= 19 && b[16] == b':' { n(17, 19).unwrap_or(0) } else { 0 };
    let mut t = days_from_civil(y, mo, d) * 86400 + h * 3600 + mi * 60 + sec;
    // A trailing ±HHMM / ±HH:MM offset; Z or none = UTC.
    let tail: String = v.chars().rev().take(6).collect::<Vec<_>>().into_iter().rev().collect();
    let tail = tail.replace(':', "");
    if let Some(pos) = tail.rfind(['+', '-']) {
        let off = &tail[pos..];
        if off.len() == 5 {
            if let (Ok(oh), Ok(om)) = (off[1..3].parse::<i64>(), off[3..5].parse::<i64>()) {
                let secs = oh * 3600 + om * 60;
                t -= if off.starts_with('-') { -secs } else { secs };
            }
        }
    }
    Some(t)
}

/// "5m" / "3h" / "2d" / "6w" / "4mo" / "1y" since `then` (compact Updated).
pub fn relative_age(then: i64, now: i64) -> String {
    let s = (now - then).max(0);
    match s {
        s if s < 3600 => format!("{}m", (s / 60).max(1)),
        s if s < 86400 => format!("{}h", s / 3600),
        s if s < 86400 * 14 => format!("{}d", s / 86400),
        s if s < 86400 * 60 => format!("{}w", s / (86400 * 7)),
        s if s < 86400 * 365 => format!("{}mo", s / (86400 * 30)),
        s => format!("{}y", s / (86400 * 365)),
    }
}

/// Priority → (glyph, tone): ⇈ ↑ = ↓ ⇊ by the usual Jira names.
pub fn priority_glyph(v: &str) -> (&'static str, PopupTone) {
    let l = v.to_lowercase();
    if l.contains("highest") || l.contains("blocker") || l.contains("critical") || l.contains("urgent") || l == "p0" {
        ("⇈", PopupTone::Danger)
    } else if l.contains("high") || l == "p1" {
        ("↑", PopupTone::Warning)
    } else if l.contains("lowest") || l.contains("trivial") || l == "p4" {
        ("⇊", PopupTone::Accent)
    } else if l.contains("low") || l.contains("minor") || l == "p3" {
        ("↓", PopupTone::Accent)
    } else {
        ("=", PopupTone::Dim)
    }
}

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

/// Python `os.path.expanduser` (leading `~` only).
fn expand_user(p: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    match p.strip_prefix('~') {
        Some(rest) => format!("{home}{rest}"),
        None => p.to_string(),
    }
}

/// `jira_poll.py release_view_file`: the tab file name for a release / label
/// view — alphanumerics, `._- ` kept, everything else `_`, trimmed, `.json`.
fn view_file_name(base: &str) -> String {
    let safe: String = base
        .chars()
        .map(|c| if c.is_alphanumeric() || matches!(c, '.' | '_' | '-' | ' ') { c } else { '_' })
        .collect();
    format!("{}.json", safe.trim())
}

/// One queued `jira_poll.py` write (`setFavorite`, `setPinnedLabels`, …).
/// On failure the host re-reads config.json (the revert) and toasts
/// `err_prefix: <reason>`.
#[derive(Clone, Debug, PartialEq)]
pub struct PollWrite {
    pub args: Vec<String>,
    pub stdin: Option<String>,
    pub err_prefix: String,
    /// The script beside `jira_poll.py` to run (`jira_config.py` for columns).
    pub script: String,
}

/// What a queued [`PollRun`] does with its JSON answer.
#[derive(Clone, Debug, PartialEq)]
pub enum PollRunKind {
    /// `--board-view ID`: land the board's freshly built tab file.
    BoardView(String),
    /// `--board-sprint-keys BOARD SPRINT`: narrow the open board to a sprint.
    SprintKeys(String, String),
    /// `--release-view KEY`.
    ReleaseView(String),
    /// `--label-view NAME`.
    LabelView(String),
    /// `--my-work` for one view id.
    MyWork(String),
}

/// One queued `jira_poll.py` verb whose ANSWER changes the model (a tab file
/// to show, sprint keys to filter by). These used to run synchronously on
/// the main thread from click handlers — a subprocess parsing the 100 MB
/// issue cache froze the UI for seconds per click. The host drains
/// `JiraList::pending_runs` off-main and routes the result back.
#[derive(Clone, Debug)]
pub struct PollRun {
    pub args: Vec<String>,
    pub kind: PollRunKind,
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
    /// `colFilters`: field -> the picked values (an `IN (…)` per column,
    /// ANDed across columns). [`EMPTY_VALUE`] stands for a blank cell.
    pub col_filters: BTreeMap<String, BTreeSet<String>>,
    /// Indices into `all`, filtered + sorted.
    pub rows: Vec<usize>,
    pub selection: usize,
    pub checked: BTreeSet<String>,
    pub favs: BTreeSet<String>,
    pub rules: Option<StyleRules>,
    pub catalog: Vec<(String, String, String)>,
    pub boards: Vec<(String, String)>,
    /// `pinnedBoardViews` (`board|sprint|mode` specs), the sidebar "Starred" rows.
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
    /// `jira_poll.py` config writes the model queued (stars, pins,
    /// blacklist); the host drains and runs them off the main thread.
    pub pending_writes: Vec<PollWrite>,
    /// `jira_poll.py` answer-runs the model queued (board tab builds,
    /// sprint key fetches, release/label/my-work views); the host drains
    /// and runs them off the main thread like `pending_writes`.
    pub pending_runs: Vec<PollRun>,
    /// Sprint filter for the open board: (board, sprint token, its issue
    /// keys). `None` shows every row — sprint `all`, a fetch in flight, or
    /// a fetch that failed (see `sprint_error`).
    pub sprint_filter: Option<(String, String, std::collections::HashSet<String>)>,
    /// The `(board, sprint)` fetch currently in flight (dedupe).
    pub sprint_pending: Option<(String, String)>,
    /// The last sprint-fetch failure, shown in the board summary so the
    /// lanes never lie about what they filtered.
    pub sprint_error: Option<String>,
    /// Sprint issue keys already fetched this session, per `(board, sprint)`.
    /// A re-show re-applies these instead of refetching (Mechanism 3 of
    /// `DEBUG-jira-sprint-switch-revert.md`: the `--board-view` completion
    /// used to un-filter every row until the next `--board-sprint-keys`
    /// answer landed). Bounded; a successful fetch replaces its entry.
    pub sprint_keys_memo: HashMap<(String, String), std::collections::HashSet<String>>,
    /// `backToBoardList`: the open board came from "All boards", so Esc
    /// returns there.
    pub back_to_board_list: bool,
    /// The last synced tab shown, where Esc out of a pinned list lands.
    pub last_tab: usize,
    /// Synthetic rows (group headers, "load N more…"): a `rows` index at or
    /// past `all.len()` points here (`all.len() + i`).
    pub synth: Vec<Record>,
    /// `groupBy` for lists (persisted per list) and `boardGroupBy` for a board.
    pub group_by: Option<String>,
    pub board_group_by: Option<String>,
    /// Collapsed group names (`collapsedGroups`).
    pub collapsed: BTreeSet<String>,
    /// The sidebar "All boards" disclosure triangle: hide the starred boards
    /// under it (persisted per view).
    pub boards_collapsed: bool,
    /// `[jira] page-size` and how many extra pages are shown (`visibleOffset`).
    pub page_size: usize,
    pub visible_offset: usize,
    /// The Jira directory (`directory.json`), read once per mtime.
    pub directory: Value,
    pub directory_mtime: Option<std::time::SystemTime>,
    /// `board_catalog.json` / `boards.json` read once per mtime: the board
    /// bar / sprint picker / sidebar re-read them per keystroke otherwise
    /// (one `apply()` triggered ≥4 catalog parses).
    pub cat_cache: std::cell::RefCell<(Option<std::time::SystemTime>, Value)>,
    pub boards_cache: std::cell::RefCell<(Option<std::time::SystemTime>, Value)>,
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
        l.page_size = l.section.get("page-size").and_then(|v| v.trim().parse().ok()).unwrap_or(0);
        l.group_by = crate::platform::get_string("listGroupBy.jira").filter(|g| !g.is_empty());
        l.boards_collapsed = crate::platform::get_string("jira.boardsCollapsed").as_deref() == Some("1");
        l.reload_meta();
        l
    }

    fn sources_dir(&self) -> String {
        let raw = self.section.get("sources").cloned().unwrap_or_default();
        let home = std::env::var("HOME").unwrap_or_default();
        match raw.strip_prefix('~') {
            Some(rest) => format!("{home}{rest}"),
            None if raw.is_empty() => self.poller_out_dir(),
            None => raw,
        }
    }

    /// The directory the POLLER publishes tabs to: config.json's `outDir`
    /// (which the poller always honors) else the paths.json default. The
    /// app used to read tabs from the default even when config.json pointed
    /// elsewhere — a custom outDir left the window showing empty lists while
    /// the poller happily published.
    fn poller_out_dir(&self) -> String {
        let custom = self
            .config
            .get("outDir")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(expand_user);
        custom.unwrap_or_else(|| self.paths.as_ref().map(|p| p.out_dir.clone()).unwrap_or_default())
    }

    /// A folder beside the poller's outDir (its files are not tabs) — the
    /// mirror of the poller's `side_dir(outDir, name)`.
    fn side_dir_path(&self, side: &str) -> Option<String> {
        let out = self.poller_out_dir();
        let out = out.trim_end_matches('/');
        Some(Path::new(out).parent()?.join(side).to_string_lossy().into_owned())
    }

    /// The board's / release's / label's side dir by `paths.json` key.
    fn side_dir_by_key(&self, key: &str) -> Option<String> {
        let side = self.paths.as_ref()?.side_dirs.get(key)?;
        self.side_dir_path(side)
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
                _ if name == "all" => "All issues".into(),
                _ => name.to_string(),
            };
        }
        file.trim_end_matches(".json").to_string()
    }

    /// `ListSession.tabColumns(cmd, path)` — the endpoint's columns, labeled.
    pub fn tab_columns(&self, file: &str) -> Vec<ListColumn> {
        let spec = self
            .endpoints()
            .iter()
            .find(|ep| {
                let name = ep.get("name").and_then(Value::as_str).unwrap_or("");
                ep.get("file").and_then(Value::as_str).map(str::to_string).unwrap_or(format!("{name}.json")) == file
            })
            .and_then(|ep| ep.get("columns").cloned())
            .or_else(|| self.section.get("columns").map(|s| Value::String(s.clone())))
            .unwrap_or_else(crate::views::jira::default_columns);
        let mut cols = parse_columns_value(&spec);
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
            let cat = self.cached_json(&self.cat_cache, &p.cache_file("boardCatalog"));
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
            if let Some(i) = self.rows.iter().position(|&j| self.rec(j).and_then(|r| r.get("key")) == Some(&k)) {
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
        self.last_tab = i;
        self.back_to_board_list = false;
        self.col_filters.clear();
        self.checked.clear(); // batch ticks belong to the shown list only
        self.sprint_filter = None;
        self.sprint_pending = None;
        self.sprint_error = None;
        self.visible_offset = 0;
        self.columns = t.columns.clone();
        self.all = load_records(&t.path);
        self.shown_path = t.path.clone();
        self.shown_mtime = std::fs::metadata(&t.path).and_then(|m| m.modified()).ok();
        self.selection = 0;
        self.note = None;
        // Remember where the user was so a rebuild restores this tab
        // (`start()`'s documented "the last one shown"; it used to fall
        // back to whatever sorts first — typically "Hidden releases").
        crate::platform::set_string("jira.lastTab", &t.file);
        self.refilter();
    }

    /// Show a pinned list (`showPinFile`).
    pub fn show_pin_file(&mut self, key: &str, path: &str, columns: Vec<ListColumn>) {
        self.place = Some(JiraPlace::Pin(key.to_string()));
        self.col_filters.clear();
        self.checked.clear();
        self.sprint_filter = None;
        self.sprint_pending = None;
        self.sprint_error = None;
        self.visible_offset = 0;
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
        self.checked.clear();
        self.col_filters.clear();
        self.sprint_filter = None;
        self.sprint_pending = None;
        self.sprint_error = None;
        self.columns = parse_columns_value(&crate::views::jira::board_list_columns());
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
        let spec = self
            .section
            .get("columns")
            .map(|s| Value::String(s.clone()))
            .unwrap_or_else(crate::views::jira::default_columns);
        let mut cols = parse_columns_value(&spec);
        for c in cols.iter_mut() {
            if c.title.is_empty() || c.title == c.field {
                c.title = base_field_label(&c.field).map(str::to_string).unwrap_or_else(|| c.field.clone());
            }
        }
        cols
    }

    /// A pinned board's cached issues (`showBoardPin`), else poll it.
    pub fn board_path(&self, id: &str) -> Option<String> {
        let dir = self.side_dir_by_key("boards")?;
        Some(Path::new(&dir).join(format!("board-{id}.json")).to_string_lossy().into_owned())
    }

    pub fn open_board(&mut self, id: &str) -> bool {
        let Some(cached) = self.board_path(id) else {
            self.board_error(id, "board path unavailable");
            return false;
        };
        if !Path::new(&cached).exists() {
            // No tab file yet: build it ASYNC like the sibling pins
            // (`--release-view` / `--label-view` / `--my-work`) — this used
            // to run `jira_poll.py --board-view` synchronously on the main
            // thread, which parses the whole issue cache: seconds of frozen
            // UI per click, forever if the child wedged. The board shows
            // empty for a moment; `finish_board_view` lands the rows.
            self.pending_runs.push(PollRun {
                args: vec!["--board-view".into(), id.to_string()],
                kind: PollRunKind::BoardView(id.to_string()),
            });
            self.status_line = Some((format!("Board {id}: building its tab…"), PopupTone::Info));
        }
        // `openBoard` pins a board that is not pinned yet (the sidebar's
        // "Boards" rows), then shows its cached issues.
        let from_list = self.on_board_list();
        self.pin_board(id);
        let cols = self.default_columns();
        self.show_pin_file(&format!("{BOARD_PREFIX}{id}"), &cached, cols);
        // The board's sprint: this session's pick, else the persisted pick
        // (Mechanism 2: a restart used to snap every board back to
        // `current`), else the default. Land it in the map so every later
        // read (`current_view_spec`, `board_bar`) agrees with what opened.
        let sp = self.sprint_token(id);
        self.board_sprint.insert(id.to_string(), sp.clone());
        self.board_mode.entry(id.to_string()).or_insert_with(|| "columns".to_string());
        self.compute_board_columns();
        self.request_sprint(id, &sp);
        self.back_to_board_list = from_list;
        true
    }

    /// The `--board-view` answer for a board opened without a tab file.
    pub fn finish_board_view(&mut self, board: &str, res: Result<Value, String>) {
        match res {
            Ok(v) => {
                if self.on_board().as_deref() != Some(board) {
                    return; // the user moved on; the watcher shows rows anyway
                }
                let path = match (v.get("dir").and_then(Value::as_str), v.get("file").and_then(Value::as_str)) {
                    (Some(d), Some(f)) => format!("{d}/{f}"),
                    _ => return,
                };
                if !Path::new(&path).exists() {
                    self.board_error(board, "board not synced yet");
                    return;
                }
                if self.shown_path == path {
                    self.reload_shown(); // rows written to where the empty view points
                } else if self.board_path(board).as_deref() == Some(path.as_str()) {
                    // Mechanism 3: the built rows landed in the board's OWN
                    // tab file — the file the open view already stands for
                    // (it was missing when the board opened). Reload in
                    // place: `show_pin_file` resets `sprint_filter`, which
                    // used to flash every issue unfiltered until the
                    // re-issued `--board-sprint-keys` answer arrived.
                    self.shown_path = path.clone();
                    self.reload_shown();
                    let sp = self.sprint_token(board);
                    self.reapply_sprint(board, &sp);
                } else {
                    let cols = self.default_columns();
                    self.show_pin_file(&format!("{BOARD_PREFIX}{board}"), &path, cols);
                    let sp = self.sprint_token(board);
                    self.reapply_sprint(board, &sp);
                }
                self.compute_board_columns();
            }
            Err(err) if err.contains("is not starred") => {
                // A board clicked from "All boards" that has no sync job
                // yet: `--board-view` can only shape an already-starred
                // board, so star it (queues `--pin-board add`, which fetches
                // the board's filter off-main; a failed write toasts "Stars
                // not saved: <reason>") and keep it open empty. The reload
                // watcher lands the rows once the poller publishes
                // board-<id>.json — no more dead end.
                self.pin_board(board);
                let text = format!("Board {board}: starred — syncing; rows arrive on the next poll");
                self.status_line = Some((text.clone(), PopupTone::Info));
                #[cfg(target_os = "macos")]
                crate::ui::toast::show(&text, "arrow.clockwise.circle");
            }
            Err(err) => self.board_error(board, &err),
        }
    }

    /// The board's sprint token: this session's pick, else the persisted
    /// pick (`jira.boardSprint.<id>` — Mechanism 2: restarts used to snap
    /// every board back to the current/latest sprint), else the default.
    fn sprint_token(&self, board: &str) -> String {
        if let Some(sp) = self.board_sprint.get(board) {
            return sp.clone();
        }
        crate::platform::get_string(&format!("jira.boardSprint.{board}"))
            .filter(|sp| !sp.is_empty() && self.sprint_token_usable(board, sp))
            .unwrap_or_else(|| self.default_sprint(board))
    }

    /// A persisted sprint id the catalog no longer lists (the board
    /// re-planned) must not resurrect a dead filter; `all` / `current`
    /// always qualify, as does anything while the catalog is still empty
    /// (the board is not synced yet).
    fn sprint_token_usable(&self, board: &str, sp: &str) -> bool {
        if sp == "all" || sp == "current" {
            return true;
        }
        let sprints = self.board_sprints(board);
        sprints.is_empty() || sprints.iter().any(|s| s.id == sp)
    }

    /// Remember this board's sprint for the session AND across restarts
    /// (Mechanism 2; mirrors `jira.boardsCollapsed`).
    fn remember_sprint(&mut self, board: &str, sprint: &str) {
        self.board_sprint.insert(board.to_string(), sprint.to_string());
        crate::platform::set_string(&format!("jira.boardSprint.{board}"), sprint);
    }

    /// Ask `--board-sprint-keys` for one sprint's issue keys (off-main via
    /// `pending_runs`). `all` clears the filter; "current" maps to Jira's
    /// `openSprints()`. The lanes and the table filter to the keys when they
    /// arrive (`apply_sprint_keys`); until then the board shows everything
    /// with a "loading" note so the view never silently lies.
    pub fn request_sprint(&mut self, board: &str, sprint: &str) {
        if sprint == "all" {
            self.sprint_filter = None;
            self.sprint_pending = None;
            self.sprint_error = None;
            return;
        }
        if self
            .sprint_pending
            .as_ref()
            .is_some_and(|(b, s)| b == board && s == sprint)
        {
            return; // one request per (board, sprint) in flight
        }
        self.sprint_error = None;
        // Mechanism 3: keys fetched once this session filter the lanes
        // immediately, so the confirming refetch never shows every issue
        // in between (the old un-filtered flash).
        if let Some(keys) = self
            .sprint_keys_memo
            .get(&(board.to_string(), sprint.to_string()))
            .cloned()
        {
            self.sprint_filter = Some((board.to_string(), sprint.to_string(), keys));
            self.refilter();
            self.compute_board_columns();
        }
        self.sprint_pending = Some((board.to_string(), sprint.to_string()));
        self.pending_runs.push(PollRun {
            args: vec!["--board-sprint-keys".into(), board.to_string(), sprint.to_string()],
            kind: PollRunKind::SprintKeys(board.to_string(), sprint.to_string()),
        });
    }

    /// Re-assert the sprint filter after a board re-show: a live filter
    /// needs nothing, the memo is re-applied synchronously instead of
    /// refetching (Mechanism 3), and only an unfetched sprint queues
    /// `--board-sprint-keys`.
    pub fn reapply_sprint(&mut self, board: &str, sprint: &str) {
        if sprint == "all" {
            return; // unfiltered is what `all` means; nothing was cleared
        }
        if self
            .sprint_filter
            .as_ref()
            .is_some_and(|(b, s, _)| b == board && s == sprint)
        {
            return; // `reload_shown` kept the filter live
        }
        if let Some(keys) = self
            .sprint_keys_memo
            .get(&(board.to_string(), sprint.to_string()))
            .cloned()
        {
            self.sprint_error = None;
            self.sprint_filter = Some((board.to_string(), sprint.to_string(), keys));
            self.refilter();
            self.compute_board_columns();
            return;
        }
        self.request_sprint(board, sprint);
    }

    /// Route a `--board-sprint-keys` answer back to the model. A stale answer
    /// (the user re-picked meanwhile) must not touch the view, but its keys
    /// are still facts — memoize first, or a board re-show between pick and
    /// answer (which resets `sprint_pending`) would throw the fetch away AND
    /// force every later pick to refetch.
    pub fn apply_sprint_keys(&mut self, board: &str, sprint: &str, res: Result<Vec<String>, String>) {
        if let Ok(keys) = &res {
            // Mechanism 3: remember the set so a re-show re-applies it
            // instead of refetching. Bounded crudely — a session browses a
            // handful of boards' sprints; the odd evicted set costs one
            // refetch, never a wrong answer.
            if self.sprint_keys_memo.len() >= 128 {
                self.sprint_keys_memo.clear();
            }
            self.sprint_keys_memo
                .insert((board.to_string(), sprint.to_string()), keys.iter().cloned().collect());
        }
        let current = self
            .sprint_pending
            .as_ref()
            .is_some_and(|(b, s)| b.as_str() == board && s.as_str() == sprint);
        if !current {
            return; // stale answer (the user re-picked meanwhile)
        }
        self.sprint_pending = None;
        let on_this = self.on_board().as_deref() == Some(board)
            && self.board_sprint.get(board).map(String::as_str) == Some(sprint);
        match res {
            Ok(keys) if on_this => {
                self.sprint_filter = Some((board.to_string(), sprint.to_string(), keys.into_iter().collect()));
                self.sprint_error = None;
                self.refilter();
                self.compute_board_columns();
            }
            Ok(_) => {}
            Err(e) => {
                if on_this {
                    // Never hide the failure: the summary says the lanes are
                    // unfiltered so the user does not "see the sprint".
                    self.sprint_error = Some(e);
                    self.sprint_filter = None;
                    self.refilter();
                    self.compute_board_columns();
                }
            }
        }
    }

    /// Surface a failure reason to the user: the sidebar's status line (which
    /// persists where the 1.4 s toast pill fades and truncates mid-string) plus
    /// a macOS toast, and a pointer to the Jira Config tab (its FILES section
    /// lists the live poll / debug / status log paths for this install).
    fn surface_error(&mut self, text: &str) {
        self.status_line = Some((format!("{text} · detail: Jira ▸ Config"), PopupTone::Warning));
        #[cfg(target_os = "macos")]
        crate::ui::toast::show(text, "exclamationmark.triangle");
    }

    /// Surface an on-demand board open failure instead of the old silent
    /// `false`: the Jira sidebar's status line (always), plus a toast on macOS.
    fn board_error(&mut self, id: &str, msg: &str) {
        self.surface_error(&format!("Board {id}: {msg}"));
    }

    /// One Esc in the list (`onEscape`): undo the narrowing (query AND
    /// column filters — clearing only the query left a list that looked
    /// "stuck" filtered with no visible way back), else go back from a
    /// board to "All boards", else leave a pinned list for the last tab.
    /// `false` when there was nothing to undo (the host backs the slot out).
    pub fn escape_step(&mut self) -> bool {
        if !self.query.is_empty() || !self.col_filters.is_empty() {
            self.query.clear();
            self.col_filters.clear();
            self.selection = 0;
            self.refilter();
            return true;
        }
        if self.back_to_board_list && self.on_board().is_some() {
            self.back_to_board_list = false;
            self.show_board_list();
            return true;
        }
        if matches!(self.place, Some(JiraPlace::Pin(_))) && !self.tabs.is_empty() {
            self.select_tab(self.last_tab.min(self.tabs.len() - 1));
            return true;
        }
        false
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

    /// `true` while a board is open in `columns` mode (the lane overlay).
    fn on_board_columns(&self) -> bool {
        self.on_board().is_some_and(|id| {
            self.board_mode.get(&id).map(String::as_str).unwrap_or("columns") == "columns"
        })
    }

    /// The fields the currently shown table displays (the board overlay's own
    /// columns in columns mode, else the list's).
    fn displayed_fields(&self) -> Vec<String> {
        if self.on_board_columns() {
            return parse_columns_value(&crate::views::jira::board_table_columns())
                .iter()
                .map(|c| c.field.clone())
                .collect();
        }
        self.columns.iter().map(|c| c.field.clone()).collect()
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

    /// A cache JSON file read once per mtime (see `cat_cache` / `boards_cache`).
    fn cached_json(&self, cell: &std::cell::RefCell<(Option<std::time::SystemTime>, Value)>, path: &str) -> Value {
        let m = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        let mut c = cell.borrow_mut();
        if c.0 != m {
            c.0 = m;
            c.1 = read_json(path);
        }
        c.1.clone()
    }

    /// The sprints of one board (from the cached `board_catalog.json`).
    pub fn board_sprints(&self, id: &str) -> Vec<BoardSprint> {
        let Some(p) = &self.paths else { return Vec::new() };
        let cat = self.cached_json(&self.cat_cache, &p.cache_file("boardCatalog"));
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
                            start: s.get("startDate").and_then(Value::as_str).unwrap_or("").to_string(),
                            end: s.get("endDate").and_then(Value::as_str).unwrap_or("").to_string(),
                            complete: s.get("completeDate").and_then(Value::as_str).unwrap_or("").to_string(),
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
        let boards = self.cached_json(&self.boards_cache, &p.cache_file("boards"));
        boards
            .get(id)
            .and_then(|b| b.get("columns"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    /// The `jira.board_columns` params for the open board, from its shown rows
    /// (so the search narrows the lanes, as `renderBoardColumns` did in Swift).
    fn board_columns_params(&self) -> Option<Value> {
        let id = self.on_board()?;
        let rows: Vec<Value> = self
            .rows
            .iter()
            .filter_map(|&i| self.all.get(i))
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
        // The directory's authoritative status -> category map ({name:
        // "new"/"indeterminate"/"done"}) beats word guessing for a status
        // like "Verified" or "Reopened" — it was hardcoded `{}` and every
        // card fell back to the word lists.
        let cats = self
            .directory
            .get("statusCategories")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        Some(serde_json::json!({
            "rows": rows,
            "columns": spec,
            "categories": cats,
            "words": words,
            "people": {},
            "categoryNames": ["To Do", "In Progress", "Done"],
        }))
    }

    /// Recompute the open board's columns from its loaded rows (`renderBoardColumns`).
    ///
    /// The pure port runs directly (parity-tested against `jira.board_columns`):
    /// the old helper IPC hop could block a board open or sprint switch for
    /// up to 30 s under helper-pool pressure, and bought nothing the local
    /// port does not. `helper_board_columns` stays for the parity test.
    pub fn compute_board_columns(&mut self) {
        self.refresh_board_columns();
    }

    /// The board columns from the in-process port (no IPC) — the per-keystroke
    /// search path and the recompute path.
    fn refresh_board_columns(&mut self) {
        self.refresh_directory(); // fresh statusCategories for the lanes
        let Some(params) = self.board_columns_params() else {
            self.board_cols = Vec::new();
            return;
        };
        self.board_cols = board_columns(&params);
    }

    /// `openView(_:)` — parse `board|sprint|mode` and open that board.
    pub fn open_view(&mut self, v: &str) -> bool {
        let parts: Vec<&str> = v.split('|').collect();
        if parts.len() != 3 {
            return false;
        }
        let (board, sprint, mode) = (parts[0], parts[1], parts[2]);
        self.remember_sprint(board, sprint);
        self.board_mode.insert(board.to_string(), mode.to_string());
        self.open_board(board)
    }

    /// `pickSprint(_:)` — the open board's sprint picker. Narrowing needs
    /// the sprint's issue keys (`--board-sprint-keys`, a network search):
    /// fetched off-main, the lanes filter when they land. Until then the
    /// board shows everything with a "loading" note — the choice used to be
    /// cosmetic and the view *said* it was the sprint.
    pub fn pick_sprint(&mut self, sprint: &str) -> bool {
        let Some(id) = self.on_board() else { return false };
        let old_spec = self.current_view_spec();
        // Mechanism 2: the pick must outlive restarts / window rebuilds —
        // it used to be memory-only, so every board snapped back to
        // `current` (the latest sprint), reading as "it only shows the
        // latest".
        self.remember_sprint(&id, sprint);
        // Mechanism 1: a "Starred" sidebar row opens the sprint FROZEN in
        // its `board|sprint|mode` spec, so clicking that row back used to
        // overwrite this pick with the pin's sprint. Re-point the row
        // instead — it now follows the pick.
        if let (Some(old), Some(new)) = (old_spec, self.current_view_spec()) {
            self.repoint_pinned_view(&old, &new);
        }
        self.sprint_filter = None;
        self.sprint_error = None;
        self.refilter();
        self.compute_board_columns();
        self.request_sprint(&id, sprint);
        true
    }

    /// `boardBar.onMode` — `columns` | `table` for the open board.
    pub fn set_mode(&mut self, mode: &str) -> bool {
        let Some(id) = self.on_board() else { return false };
        let old_spec = self.current_view_spec();
        self.board_mode.insert(id, mode.to_string());
        // The spec carries the mode too — a pinned row follows the switch
        // (Mechanism 1, same frozen-spec overwrite as the sprint).
        if let (Some(old), Some(new)) = (old_spec, self.current_view_spec()) {
            self.repoint_pinned_view(&old, &new);
        }
        true
    }

    /// `list-star` — star/unstar a board (it appears in the sidebar "Boards").
    pub fn toggle_pin_board(&mut self, id: &str) -> bool {
        if let Some(i) = self.boards.iter().position(|(bid, _)| bid == id) {
            self.boards.remove(i);
        } else {
            self.boards.push((id.to_string(), self.board_name(id)));
        }
        let on = self.boards.iter().any(|(bid, _)| bid == id);
        self.queue_write(&["--pin-board", if on { "add" } else { "remove" }, id], None, "Stars not saved");
        self.pins_dirty = true;
        if on {
            self.boards_collapsed = false; // a fresh star re-opens the tree
        }
        true
    }

    /// `openBoard`'s first step: a board that is not starred yet joins the
    /// sidebar "Boards" section (in-memory — `--pin-board` is not ported).
    pub fn pin_board(&mut self, id: &str) {
        if !self.boards.iter().any(|(bid, _)| bid == id) {
            self.boards.push((id.to_string(), self.board_name(id)));
            self.queue_write(&["--pin-board", "add", id], None, "Stars not saved");
            self.pins_dirty = true;
            self.boards_collapsed = false;
        }
    }

    /// The sidebar "All boards" disclosure triangle: hide / show the starred
    /// boards under it. Persisted per app (like the rail state).
    pub fn toggle_boards_collapsed(&mut self) {
        self.boards_collapsed = !self.boards_collapsed;
        crate::platform::set_string("jira.boardsCollapsed", if self.boards_collapsed { "1" } else { "0" });
    }

    /// `boardBar.onPin` — pin/unpin the current `board|sprint|mode` view.
    pub fn toggle_pin_view(&mut self, spec: &str) -> bool {
        if let Some(i) = self.pinned_views.iter().position(|v| v == spec) {
            self.pinned_views.remove(i);
        } else {
            self.pinned_views.push(spec.to_string());
        }
        let on = self.pinned_views.iter().any(|v| v == spec);
        self.queue_write(&["--pin-view", if on { "add" } else { "remove" }, spec], None, "Stars not saved");
        self.pins_dirty = true;
        true
    }

    /// Mechanism 1 (`DEBUG-jira-sprint-switch-revert.md`): a "Starred" row
    /// used to open the sprint/mode FROZEN in its spec, so a fresh pick was
    /// overwritten the moment the user clicked the row. When a pick changes
    /// the open board's spec, re-point the pinned row to it (keeping its
    /// slot); if the new spec is already pinned, the row folds into it.
    fn repoint_pinned_view(&mut self, old: &str, new: &str) {
        if old == new {
            return;
        }
        let Some(i) = self.pinned_views.iter().position(|v| v == old) else {
            return; // the current view is not a pinned row — nothing reverts
        };
        if self.pinned_views.iter().any(|v| v == new) {
            self.pinned_views.remove(i);
        } else {
            self.pinned_views[i] = new.to_string();
            self.queue_write(&["--pin-view", "add", new], None, "Stars not saved");
        }
        self.queue_write(&["--pin-view", "remove", old], None, "Stars not saved");
        self.pins_dirty = true;
    }

    fn queue_write(&mut self, args: &[&str], stdin: Option<String>, err_prefix: &str) {
        self.pending_writes.push(PollWrite {
            args: args.iter().map(|a| a.to_string()).collect(),
            stdin,
            err_prefix: err_prefix.into(),
            script: "jira_poll.py".into(),
        });
    }

    /// Edit one string list in the in-memory config.json (the optimistic half
    /// of a pin write; a failed write re-reads the file).
    fn config_list_edit(&mut self, key: &str, items: &[String], on: bool) {
        let mut cur = str_list(&self.config, key);
        if on {
            let mut new: Vec<String> = items.iter().filter(|k| !cur.contains(k)).cloned().collect();
            new.append(&mut cur);
            cur = new;
        } else {
            cur.retain(|k| !items.contains(k));
        }
        if !self.config.is_object() {
            self.config = Value::Object(Default::default());
        }
        self.config[key] = Value::Array(cur.into_iter().map(Value::String).collect());
    }

    /// `setFavorite` — star / unstar issues (`--favorite`); the shown rows go
    /// on stdin for keys the issue cache does not hold.
    pub fn set_favorite(&mut self, keys: &[String], on: bool) {
        let keys: Vec<String> = keys.iter().filter(|k| !k.is_empty()).cloned().collect();
        if keys.is_empty() {
            return;
        }
        for k in &keys {
            if on {
                self.favs.insert(k.clone());
            } else {
                self.favs.remove(k);
            }
        }
        self.config_list_edit("favorites", &keys, on);
        let rows: Vec<Value> = keys
            .iter()
            .filter_map(|k| self.all.iter().find(|r| r.get("key") == Some(k)))
            .map(|r| {
                Value::Object(
                    r.iter()
                        .filter(|(k, _)| !k.starts_with("__"))
                        .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                        .collect(),
                )
            })
            .collect();
        let mut args = vec!["--favorite", if on { "add" } else { "remove" }];
        args.extend(keys.iter().map(String::as_str));
        let stdin = on.then(|| Value::Array(rows).to_string());
        self.queue_write(&args, stdin, "Stars not saved");
    }

    /// The row's ☆ = star: a board row stars/unstars the board (the sidebar's
    /// "Boards" rows — favourites hold issue keys only), a release row stars
    /// the release, an issue row the issue.
    pub fn toggle_star(&mut self, row: usize) {
        let Some(r) = self.row(row).cloned() else { return };
        let Some(k) = r.get("key").filter(|k| !k.is_empty()).cloned() else { return };
        if let Some(id) = k.strip_prefix(BOARD_PREFIX) {
            self.toggle_pin_board(id);
            return;
        }
        if is_release_row(&r) {
            let on = !self.favorite_releases().contains(&k);
            self.set_release_favorite(&[k], on);
        } else {
            let on = !self.favs.contains(&k);
            self.set_favorite(&[k], on);
        }
    }

    /// `setReleaseFavoriteKeys` — the sidebar's starred releases.
    pub fn set_release_favorite(&mut self, keys: &[String], on: bool) {
        if keys.is_empty() {
            return;
        }
        self.config_list_edit("favoriteReleases", keys, on);
        let mut args = vec!["--favorite-release", if on { "add" } else { "remove" }];
        args.extend(keys.iter().map(String::as_str));
        self.queue_write(&args, None, "Stars not saved");
    }

    /// `setBlacklisted` — hide / restore releases (`--blacklist-release`).
    pub fn set_blacklisted(&mut self, keys: &[String], on: bool) {
        if keys.is_empty() {
            return;
        }
        self.config_list_edit("releaseBlacklist", keys, on);
        let mut args = vec!["--blacklist-release", if on { "add" } else { "remove" }];
        args.extend(keys.iter().map(String::as_str));
        self.queue_write(&args, None, "Not saved");
    }

    /// `setPinnedLabels` — the sidebar's pinned labels (`--pin-label`).
    pub fn set_pinned_labels(&mut self, names: &[String], on: bool) {
        if names.is_empty() {
            return;
        }
        let mut cur = str_list(&self.config, "pinnedLabels");
        if on {
            cur.extend(names.iter().filter(|n| !cur.contains(n)).cloned().collect::<Vec<_>>());
        } else {
            cur.retain(|n| !names.contains(n));
        }
        if !self.config.is_object() {
            self.config = Value::Object(Default::default());
        }
        self.config["pinnedLabels"] = Value::Array(cur.into_iter().map(Value::String).collect());
        let mut args = vec!["--pin-label", if on { "add" } else { "remove" }];
        args.extend(names.iter().map(String::as_str));
        self.queue_write(&args, None, "Stars not saved");
    }

    pub fn pinned_labels(&self) -> Vec<String> {
        str_list(&self.config, "pinnedLabels")
    }

    pub fn favorite_releases(&self) -> Vec<String> {
        str_list(&self.config, "favoriteReleases")
    }

    /// Is the shown list the hidden-releases (blacklist) tab?
    pub fn on_blacklist_tab(&self) -> bool {
        let bl = self.paths.as_ref().and_then(|p| p.tabs.get("blacklistRelease")).cloned();
        match (&self.place, bl) {
            (Some(JiraPlace::Tab(i)), Some(f)) => self.tabs.get(*i).is_some_and(|t| t.file == f),
            _ => false,
        }
    }

    /// `currentViewSpec` — `board|sprint|mode` for the open board.
    pub fn current_view_spec(&self) -> Option<String> {
        let id = self.on_board()?;
        let sprint = self.sprint_token(&id);
        let mode = self
            .board_mode
            .get(&id)
            .cloned()
            .unwrap_or_else(|| "columns".to_string());
        Some(format!("{id}|{sprint}|{mode}"))
    }

    /// `sprintTitle(_:_:)` — the sprint's display name for the bar.
    pub fn sprint_title(&self, board: &str, sp: &str) -> String {
        match sp {
            "all" => "All issues".to_string(),
            "current" => self
                .board_sprints(board)
                .into_iter()
                .find(|s| s.state == "active")
                .map(|s| format!("{} (current)", s.name))
                .unwrap_or_else(|| "Current sprint".to_string()),
            _ => self
                .board_sprints(board)
                .into_iter()
                .find(|s| s.id == sp)
                .map(|s| s.name)
                .unwrap_or_else(|| format!("Sprint {sp}")),
        }
    }

    /// `updateBoardChrome`'s sprint choices: the active sprint as `current`,
    /// future sprints as `(next)`, a `Past sprints` section, then `All issues`.
    pub fn board_sprint_choices(&self, id: &str) -> Vec<SprintChoice> {
        let mut choices: Vec<SprintChoice> = Vec::new();
        let sprints = self.board_sprints(id);
        if sprints.is_empty() {
            return choices;
        }
        if sprints.iter().any(|s| s.state == "active") {
            choices.push(SprintChoice {
                id: "current".into(),
                title: self.sprint_title(id, "current"),
                header: false,
            });
        }
        for s in sprints.iter().filter(|s| s.state == "future") {
            choices.push(SprintChoice {
                id: s.id.clone(),
                title: format!("{} (next)", s.name),
                header: false,
            });
        }
        let past: Vec<&BoardSprint> = sprints.iter().filter(|s| s.state == "closed").collect();
        if !past.is_empty() {
            choices.push(SprintChoice { id: String::new(), title: String::new(), header: true });
            choices.push(SprintChoice { id: String::new(), title: "Past sprints".into(), header: true });
            for s in past {
                choices.push(SprintChoice { id: s.id.clone(), title: s.name.clone(), header: false });
            }
        }
        choices.push(SprintChoice { id: String::new(), title: String::new(), header: true });
        choices.push(SprintChoice { id: "all".into(), title: "All issues".into(), header: false });
        choices
    }

    /// `sprintSummary(_:_:count:)` — `N issues · Oct 1 – Oct 14` (or
    /// `ended Oct 14` for a closed sprint). The first ten ISO chars are the
    /// date; a bad value is dropped.
    pub fn board_summary(&self, board: &str, sp: &str) -> String {
        let count = self
            .rows
            .iter()
            .filter(|&&i| self.rec(i).is_some_and(|r| !is_synthetic(r)))
            .count();
        let mut parts = vec![format!("{count} issue{}", if count == 1 { "" } else { "s" })];
        let s = if sp == "current" {
            self.board_sprints(board).into_iter().find(|s| s.state == "active")
        } else {
            self.board_sprints(board).into_iter().find(|s| s.id == sp)
        };
        if let Some(s) = s {
            if s.state == "closed" {
                if let Some(e) = sprint_day(&s.complete).or_else(|| sprint_day(&s.end)) {
                    parts.push(format!("ended {e}"));
                }
            } else if let Some(a) = sprint_day(&s.start) {
                let b = sprint_day(&s.end).unwrap_or_else(|| "?".to_string());
                parts.push(format!("{a} – {b}"));
            }
        }
        // The sprint filter's live state — the bar never says "Current
        // sprint" over unfiltered rows without saying so. A confirming
        // refetch in flight while the memo already filters says nothing:
        // the rows shown ARE the sprint's (as last fetched).
        if sp != "all" {
            let filtered = self
                .sprint_filter
                .as_ref()
                .is_some_and(|(b, s, _)| b == board && s == sp);
            if self.sprint_pending.as_ref().is_some_and(|(b, _)| b == board) && !filtered {
                parts.push("sprint rows loading…".into());
            } else if let Some(e) = &self.sprint_error {
                parts.push(format!("sprint filter failed — showing all: {e}"));
            }
        }
        parts.join(" · ")
    }

    /// `updateBoardChrome` — the board bar model when a board is open, else
    /// `None` (the "All boards" list has no bar).
    pub fn board_bar(&self) -> Option<JiraBoardBarModel> {
        let id = self.on_board()?;
        let sp = self.sprint_token(&id);
        let mode = self
            .board_mode
            .get(&id)
            .cloned()
            .unwrap_or_else(|| "columns".to_string());
        let pinned = self
            .current_view_spec()
            .is_some_and(|v| self.pinned_views.iter().any(|p| p == &v));
        let mut bar = JiraBoardBarModel::default();
        bar.show(
            &self.board_name(&id),
            self.board_sprint_choices(&id),
            &sp,
            &mode,
            pinned,
            &self.board_summary(&id, &sp),
        );
        Some(bar)
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
            Some(id) => {
                let keys: Vec<String> = match &self.sprint_filter {
                    Some((b, _, set)) if b == &id => {
                        let mut v: Vec<String> = set.iter().cloned().collect();
                        v.sort();
                        v
                    }
                    _ => Vec::new(),
                };
                serde_json::json!({
                    "id": id,
                    "sprint": self.sprint_token(&id),
                    "mode": self.board_mode.get(&id).cloned().unwrap_or_else(|| "columns".to_string()),
                    "columns": self.board_mode.get(&id).map(|m| m == "columns").unwrap_or(true),
                    "bar": true,
                    // The sprint picker's real filter state (the old `-1`
                    // "never fetched" sentinel was the cosmetic-picker bug).
                    "sprintKeys": keys,
                    "sprintPending": self.sprint_pending.as_ref().is_some_and(|(b, _)| b == &id),
                    "sprintError": self.sprint_error.clone(),
                    "views": self.pinned_views,
                    "selected": self.sidebar_selected(),
                    "catalog": self.catalog.iter().map(|(id, _, _)| id.clone()).collect::<Vec<_>>(),
                })
            }
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
            "columns": self.columns.iter().map(|c| match c.width_px {
                Some(px) => format!("{}:{}px", c.field, px),
                None => format!("{}:{}", c.field, c.width),
            }).collect::<Vec<_>>(),
            "groupBy": self.effective_group_by(),
            "filters": self.col_filters.iter().map(|(k, v)| (k.clone(), v.iter().cloned().collect::<Vec<_>>())).collect::<BTreeMap<_, _>>(),
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
        let columns = parse_columns_value(&crate::views::jira::board_table_columns());
        let filtering = !self.col_filters.is_empty();
        let mut rows: Vec<Record> = Vec::new();
        for col in &self.board_cols {
            let mut cards: Vec<Record> = Vec::new();
            for card in &col.cards {
                let mut r = Record::new();
                r.insert("column".into(), col.name.clone());
                r.insert("key".into(), card.key.clone());
                r.insert("title".into(), card.title.clone());
                r.insert("status".into(), card.status.clone());
                r.insert("assignee".into(), card.assignee.clone());
                if self.row_matches_filters(&r) {
                    cards.push(r);
                }
            }
            let mut head = Record::new();
            head.insert("column".into(), col.name.clone());
            let shown = cards.len() + if filtering { 0 } else { col.more };
            head.insert("title".into(), format!("{} ({})", col.name, shown));
            rows.push(head);
            rows.extend(cards);
            if col.more > 0 && !filtering {
                let mut more = Record::new();
                more.insert("column".into(), col.name.clone());
                more.insert("title".into(), format!("… {} more", col.more));
                rows.push(more);
            }
        }
        let filter_active: BTreeSet<usize> = columns
            .iter()
            .enumerate()
            .filter(|(_, c)| self.col_filters.contains_key(&c.field))
            .map(|(i, _)| i)
            .collect();
        let selection = self.selection.min(rows.len().saturating_sub(1));
        Some(JiraSnapshot {
            columns,
            rows,
            selection,
            checked: self.checked.clone(),
            favs: self.favs.clone(),
            pinned_boards: self.boards.iter().map(|(id, _)| id.clone()).collect(),
            sort: None,
            filter_active,
            compact: false,
            reserve_label: false,
            board_bar: None,
            board_columns: None,
            sidebar: self.sidebar_model(),
            inspector: None,
            show_inspector: false,
            query: self.query.clone(),
            empty: "No board columns.".into(),
            rules: self.rules.clone(),
            place: self.place_key(),
        })
    }

    /// `showReleasePin` — `--release-view KEY` writes one file per release
    /// from the issue cache (local, no request); show the pinned release's.
    /// Err strings carry no "release" prefix: the callers add the item
    /// context. A present file opens instantly; a missing one opens the
    /// (empty) view with a "building" note while the verb runs off-main
    /// (`finish_open_pin`) — this used to freeze the click on a subprocess
    /// parsing the whole issue cache.
    pub fn open_release(&mut self, key: &str) -> Result<(), String> {
        self.open_pin_view(
            "releases",
            view_file_name(key),
            key,
            &format!("Release {key}"),
            vec!["--release-view".into(), key.to_string()],
            PollRunKind::ReleaseView(key.to_string()),
        );
        Ok(())
    }

    /// `showLabelPin` — `--label-view NAME` -> the issues carrying the label.
    pub fn open_label(&mut self, name: &str) -> Result<(), String> {
        self.open_pin_view(
            "labels",
            view_file_name(&format!("label-{name}")),
            &format!("{LABEL_PREFIX}{name}"),
            &format!("Label {name}"),
            vec!["--label-view".into(), name.to_string()],
            PollRunKind::LabelView(name.to_string()),
        );
        Ok(())
    }

    /// `showMyWorkPin` — `jira_poll.py --my-work` → `<dir>/<id>.json`.
    pub fn open_my_work(&mut self, id: &str) -> Result<(), String> {
        self.open_pin_view(
            "myWork",
            format!("{id}.json"),
            &format!("{MY_WORK_PREFIX}{id}"),
            &format!("My work {id}"),
            vec!["--my-work".into()],
            PollRunKind::MyWork(id.to_string()),
        );
        Ok(())
    }

    /// Shared pin-view open: instant show when the file exists; otherwise
    /// show the computed candidate (empty) + status and queue the builder.
    fn open_pin_view(&mut self, side_key: &str, file: String, place_key: &str, label: &str, args: Vec<String>, kind: PollRunKind) {
        match self.side_dir_by_key(side_key) {
            Some(dir) => {
                let p = Path::new(&dir).join(&file).to_string_lossy().into_owned();
                let exists = Path::new(&p).exists();
                let cols = self.default_columns();
                self.show_pin_file(place_key, &p, cols);
                if !exists {
                    self.status_line = Some((format!("{label}: building its view…"), PopupTone::Info));
                    self.pending_runs.push(PollRun { args, kind });
                }
            }
            None => {
                self.status_line = Some((format!("{label}: view directory unavailable"), PopupTone::Warning));
            }
        }
    }

    /// A `--release-view` / `--label-view` / `--my-work` answer: show the
    /// file it wrote, but only while that pin is still the shown one (a
    /// stale answer must never hijack a list the user moved on to).
    pub fn finish_open_pin(&mut self, kind: &PollRunKind, res: Result<Value, String>) {
        let (place_key, label) = match kind {
            PollRunKind::ReleaseView(key) => (key.clone(), format!("Release {key}")),
            PollRunKind::LabelView(name) => (format!("{LABEL_PREFIX}{name}"), format!("Label {name}")),
            PollRunKind::MyWork(id) => (format!("{MY_WORK_PREFIX}{id}"), format!("My work {id}")),
            _ => return,
        };
        let v = match res {
            Ok(v) => v,
            Err(e) => {
                self.surface_error(&format!("{label}: {e}"));
                return;
            }
        };
        let path = match kind {
            PollRunKind::ReleaseView(key) => {
                let Some(dir) = v.get("dir").and_then(Value::as_str) else {
                    self.surface_error(&format!("{label}: no dir in output"));
                    return;
                };
                match v
                    .get("releases")
                    .and_then(Value::as_array)
                    .and_then(|a| a.iter().find(|r| r.get("key").and_then(Value::as_str) == Some(key.as_str())))
                    .and_then(|r| r.get("file").and_then(Value::as_str))
                {
                    Some(f) => format!("{dir}/{f}"),
                    None => {
                        self.surface_error(&format!("{label}: not in the synced releases"));
                        return;
                    }
                }
            }
            PollRunKind::LabelView(_) => {
                let (Some(dir), Some(f)) = (v.get("dir").and_then(Value::as_str), v.get("file").and_then(Value::as_str)) else {
                    self.surface_error(&format!("{label}: no file in output"));
                    return;
                };
                format!("{dir}/{f}")
            }
            PollRunKind::MyWork(id) => {
                let Some(dir) = v.get("dir").and_then(Value::as_str) else {
                    self.surface_error(&format!("{label}: no dir in output"));
                    return;
                };
                format!("{dir}/{id}.json")
            }
            _ => return,
        };
        if !Path::new(&path).exists() {
            self.surface_error(&format!("{label}: not synced yet"));
            return;
        }
        let on_place = matches!(&self.place, Some(JiraPlace::Pin(k)) if k == &place_key);
        if !on_place {
            return; // the user moved on; the tab is built for the next click
        }
        if self.shown_path == path {
            self.reload_shown();
        } else {
            let cols = self.default_columns();
            self.show_pin_file(&place_key, &path, cols);
        }
    }

    /// Re-read the shown file after its rows changed on disk (keeps the
    /// selection by key like `refresh`).
    fn reload_shown(&mut self) {
        let keep = self.selected().and_then(|r| r.get("key")).cloned();
        self.all = load_records(&self.shown_path);
        self.shown_mtime = std::fs::metadata(&self.shown_path).and_then(|m| m.modified()).ok();
        self.refilter();
        if let Some(k) = keep {
            if let Some(i) = self
                .rows
                .iter()
                .position(|&j| self.rec(j).and_then(|r| r.get("key")) == Some(&k))
            {
                self.selection = i;
            }
        }
    }

    fn filter_fields(&self) -> Vec<String> {
        self.section
            .get("filter")
            .map(|f| f.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect())
            .unwrap_or_else(|| vec!["key".into(), "title".into()])
    }

    /// `cellValues`: the filter values of one cell — multi-valued fields split
    /// on commas, a blank cell is [`EMPTY_VALUE`].
    pub fn cell_values(field: &str, r: &Record) -> Vec<String> {
        let v = r.get(field).map(|s| s.trim()).unwrap_or("");
        if v.is_empty() {
            return vec![EMPTY_VALUE.into()];
        }
        if !MULTI_VALUED.contains(&field) {
            return vec![v.to_string()];
        }
        let parts: Vec<String> = v.split(',').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect();
        if parts.is_empty() {
            vec![EMPTY_VALUE.into()]
        } else {
            parts
        }
    }

    /// `true` when `r` satisfies every active column filter (ANDed).
    pub fn row_matches_filters(&self, r: &Record) -> bool {
        self.col_filters
            .iter()
            .all(|(f, picked)| Self::cell_values(f, r).iter().any(|v| picked.contains(v)))
    }

    /// `filterOptions`: every value of `field` in the shown list with its row
    /// count, most rows first (ties by name), the blank value last.
    pub fn filter_options(&self, field: &str) -> Vec<FilterOption> {
        // The board overlay's `column` values are its lanes, not issue fields.
        if field == "column" && self.on_board_columns() {
            return self
                .board_cols
                .iter()
                .map(|c| FilterOption { count: c.cards.len(), id: c.name.clone(), title: c.name.clone() })
                .collect();
        }
        let people = if PERSON_FIELDS.contains(&field) { self.people_by_value() } else { HashMap::new() };
        let mut counts: HashMap<String, usize> = HashMap::new();
        let mut order: Vec<String> = Vec::new();
        for r in &self.all {
            for v in Self::cell_values(field, r) {
                let n = counts.entry(v.clone()).or_insert(0);
                if *n == 0 {
                    order.push(v);
                }
                *n += 1;
            }
        }
        order.sort_by(|a, b| {
            let (ea, eb) = (a == EMPTY_VALUE, b == EMPTY_VALUE);
            ea.cmp(&eb)
                .then(counts[b].cmp(&counts[a]))
                .then_with(|| crate::engines::list_filter::localized_standard_compare(a, b))
        });
        order
            .into_iter()
            .map(|v| {
                let title = if v != EMPTY_VALUE {
                    people.get(&v).map(|p| p.0.clone()).unwrap_or_else(|| v.clone())
                } else if field == "assignee" {
                    "(unassigned)".into()
                } else {
                    "(empty)".into()
                };
                FilterOption { count: counts[&v], id: v, title }
            })
            .collect()
    }

    /// Tick / untick one value of `field`'s filter.
    pub fn toggle_filter(&mut self, field: &str, value: &str) {
        let set = self.col_filters.entry(field.to_string()).or_default();
        if !set.remove(value) {
            set.insert(value.to_string());
        }
        if set.is_empty() {
            self.col_filters.remove(field);
        }
        self.selection = 0;
        self.visible_offset = 0;
        self.refilter();
    }

    pub fn clear_filter(&mut self, field: &str) {
        if self.col_filters.remove(field).is_some() {
            self.selection = 0;
            self.refilter();
        }
    }

    pub fn set_column_widths(&mut self, pcts: &[f64]) {
        for (c, p) in self.columns.iter_mut().zip(pcts) {
            c.width = *p;
        }
    }

    pub fn move_column(&mut self, from: usize, to: usize) {
        if from < self.columns.len() && to < self.columns.len() && from != to {
            let c = self.columns.remove(from);
            self.columns.insert(to, c);
        }
    }

    /// `JiraPoll.owner(ofTab:)`: the config.json job whose columns the shown
    /// list uses — `("live", "search")` or `("endpoint", NAME)`.
    pub fn columns_owner(&self) -> Option<(String, String)> {
        let file = Path::new(&self.shown_path).file_name()?.to_string_lossy().into_owned();
        if let Some(p) = &self.paths {
            if p.tabs.get("liveSearch") == Some(&file) {
                return Some(("live".into(), "search".into()));
            }
        }
        let eps = self.endpoints();
        let borrowed = match &self.place {
            Some(JiraPlace::Pin(k)) => {
                k.starts_with(LABEL_PREFIX) || k.starts_with(MY_WORK_PREFIX) || str_list(&self.config, "favoriteReleases").contains(k)
                    || (!k.starts_with(BOARD_PREFIX) && !k.starts_with(VIEW_PREFIX) && k != BOARD_LIST_KEY)
            }
            _ => false,
        };
        if borrowed {
            // Release / label / my-work views borrow the plain issues job.
            let plain: Vec<&Value> = eps
                .iter()
                .filter(|e| {
                    e.get("type").and_then(Value::as_str).unwrap_or("issues") == "issues"
                        && e.get("jql").and_then(Value::as_str).unwrap_or("").is_empty()
                })
                .collect();
            let e = plain.iter().find(|e| e.get("name").and_then(Value::as_str) == Some("all")).or(plain.first())?;
            return Some(("endpoint".into(), e.get("name")?.as_str()?.to_string()));
        }
        eps.iter().find_map(|e| {
            let name = e.get("name").and_then(Value::as_str)?;
            if e.get("type").and_then(Value::as_str) == Some("directory") {
                return None;
            }
            let f = e.get("file").and_then(Value::as_str).map(str::to_string).unwrap_or(format!("{name}.json"));
            (f == file).then(|| ("endpoint".to_string(), name.to_string()))
        })
    }

    /// `columnsResized` (final): queue the column spec save. Jira-owned
    /// lists go through `jira_config.py --set-columns`; `None` means the
    /// `[jira] columns` key in commands.toml owns them (spec returned).
    pub fn save_columns(&mut self) -> Option<String> {
        if self.on_board_list() || self.board_snapshot().is_some() {
            return None;
        }
        let spec = crate::views::jira::serialize_columns(&self.columns, false);
        match self.columns_owner() {
            Some((kind, name)) => {
                self.pending_writes.push(PollWrite {
                    args: vec!["--set-columns".into(), kind, name, spec],
                    stdin: None,
                    err_prefix: "Columns not saved".into(),
                    script: "jira_config.py".into(),
                });
                None
            }
            None => Some(spec),
        }
    }

    /// Set the sort explicitly (the filter picker's Sort ↑ / Sort ↓).
    pub fn set_sort(&mut self, field: &str, asc: bool) {
        self.sort = Some((field.to_string(), asc));
        self.refilter();
    }

    pub fn refilter(&mut self) {
        // A filter on a column the shown list does not have would hide rows
        // with no visible way to clear it (Swift drops those too).
        let fields_shown = self.displayed_fields();
        self.col_filters.retain(|f, v| !v.is_empty() && fields_shown.iter().any(|c| c == f));
        let fields = self.filter_fields();
        let words: Vec<String> = self.query.to_lowercase().split_whitespace().map(str::to_string).collect();
        let mut idx: Vec<usize> = (0..self.all.len())
            .filter(|&i| {
                let r = &self.all[i];
                let picked_ok = self.row_matches_filters(r);
                if !picked_ok {
                    return false;
                }
                if words.is_empty() {
                    return true;
                }
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
        // Sprint pick on a board: narrow to the fetched key set (the board
        // rows carry no sprint field, so keys from `--board-sprint-keys`
        // are the filter). Pending / failed fetch → no narrowing; the
        // summary says so rather than lying about what is shown.
        {
            let on_board = self.on_board();
            if let (Some((b, _, keys)), Some(cur)) = (&self.sprint_filter, on_board) {
                if b == &cur {
                    idx.retain(|&i| {
                        self.all.get(i).and_then(|r| r.get("key")).map_or(true, |k| keys.contains(k))
                    });
                }
            }
        }
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
        self.synth.clear();
        if let Some(g) = self.effective_group_by() {
            self.refresh_directory();
            idx = self.group_rows(&idx, &g);
        } else if self.page_size > 0 && idx.len() > self.page_size && self.query.is_empty() {
            let take = (self.page_size + self.visible_offset).min(idx.len());
            if take < idx.len() {
                let mut more = Record::new();
                more.insert("__loadmore".into(), format!("load {} more…", idx.len() - take));
                idx.truncate(take);
                idx.push(self.all.len());
                self.synth.push(more);
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

    /// A `rows` entry: an issue in `all`, or a synthetic row past it.
    pub fn rec(&self, j: usize) -> Option<&Record> {
        if j < self.all.len() {
            self.all.get(j)
        } else {
            self.synth.get(j - self.all.len())
        }
    }

    /// The vim snapshot's row texts — one per visible row, index-aligned with
    /// `rows` (the cursor must line up). Goes through `rec`: `rows` carries
    /// synthetic indices (group headers, "load N more…") past `all.len()`,
    /// and indexing `all` with them panics the daemon (the vim keypress path
    /// hit exactly that before this accessor existed).
    pub fn vim_row_texts(&self) -> Vec<String> {
        self.rows
            .iter()
            .map(|&i| {
                let Some(r) = self.rec(i) else { return String::new() };
                match (r.get("__loadmore"), r.get("__group")) {
                    (Some(m), _) => m.clone(),
                    (None, Some(g)) => {
                        let n = r.get("__count").cloned().unwrap_or_default();
                        format!("{g} {n}")
                    }
                    _ => {
                        let key = r.get("key").cloned().unwrap_or_default();
                        let sum = r.get("summary").cloned().unwrap_or_default();
                        format!("{key} {sum}")
                    }
                }
            })
            .collect()
    }

    pub fn selected(&self) -> Option<&Record> {
        self.rows.get(self.selection).and_then(|&i| self.rec(i))
    }

    pub fn row(&self, i: usize) -> Option<&Record> {
        self.rows.get(i).and_then(|&j| self.rec(j))
    }

    /// `[jira] compact-cells` (default on).
    pub fn compact_cells(&self) -> bool {
        self.section
            .get("compact-cells")
            .map_or(true, |v| !matches!(v.trim().to_lowercase().as_str(), "false" | "no" | "0" | "off"))
    }

    /// `[jira] reserve-label-width` (default on): every column reserves at
    /// least its full header label's width, so no header is ever clipped —
    /// even in compact mode.
    pub fn reserve_label_width(&self) -> bool {
        self.section
            .get("reserve-label-width")
            .map_or(true, |v| !matches!(v.trim().to_lowercase().as_str(), "false" | "no" | "0" | "off"))
    }

    /// The real (non-synthetic) shown rows.
    pub fn shown_records(&self) -> Vec<Record> {
        self.rows.iter().filter(|&&i| i < self.all.len()).filter_map(|&i| self.all.get(i).cloned()).collect()
    }

    /// The snapshot's place id (the view's scroll-keep heuristic).
    fn place_key(&self) -> String {
        match &self.place {
            Some(JiraPlace::Tab(i)) => format!("tab:{i}"),
            Some(JiraPlace::Pin(k)) => format!("pin:{k}"),
            None => String::new(),
        }
    }

    /// The directory cache, re-read when its file changed.
    pub fn refresh_directory(&mut self) {
        let Some(p) = &self.paths else { return };
        let mut path = p.cache_file("directory");
        if !Path::new(&path).exists() && Path::new(&format!("{path}.json")).exists() {
            path = format!("{path}.json");
        }
        let m = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        if m != self.directory_mtime {
            self.directory_mtime = m;
            self.directory = read_json(&path);
        }
    }

    /// `peopleByValue`: username / name / id -> (display title, handle).
    pub fn people_by_value(&self) -> HashMap<String, (String, String)> {
        let mut out: HashMap<String, (String, String)> = HashMap::new();
        for u in self.directory.get("users").and_then(Value::as_array).cloned().unwrap_or_default() {
            let g = |k: &str| u.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            let (username, name, id, email) = (g("username"), g("name"), g("id"), g("email"));
            for k in [&username, &name, &id] {
                if k.is_empty() || out.contains_key(k) {
                    continue;
                }
                let handle: Vec<String> =
                    [&username, &email].iter().filter(|h| !h.is_empty() && **h != &name && **h != k).map(|h| h.to_string()).collect();
                let title = if k == &name { name.clone() } else if name.is_empty() { k.clone() } else { format!("{name} ({k})") };
                out.insert(k.clone(), (title, handle.join(" · ")));
            }
        }
        out
    }

    /// Status -> 0 To Do / 1 In Progress / 2 Done (directory first, then words).
    pub fn status_category(&self, status: &str) -> usize {
        let cats = self.directory.get("statusCategories").and_then(Value::as_object).cloned().unwrap_or_default();
        let words = self.rules.as_ref().map(|r| r.words.clone()).unwrap_or_default();
        crate::views::jira::category(status, &cats, &words).clamp(0, 2) as usize
    }

    /// `effectiveGroupBy`.
    pub fn effective_group_by(&self) -> Option<String> {
        if self.on_board_list() {
            return None;
        }
        if self.on_board().is_some() {
            return self.board_group_by.clone();
        }
        self.group_by.clone().filter(|g| g != "boardColumn")
    }

    /// `groupChoices`: (title, field).
    pub fn group_choices(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = [
            ("Status category", "statusCategory"),
            ("Status", "status"),
            ("Assignee", "assignee"),
            ("Priority", "priority"),
            ("Release", "release"),
            ("Labels", "labels"),
            ("Epic / parent", "epic"),
            ("Components", "components"),
            ("Project", "project"),
        ]
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
        const SKIP: &[&str] = &["key", "title", "updated", "created", "description", "releaseLabel", "releaseDate"];
        for c in &self.columns {
            if c.filterable && !SKIP.contains(&c.field.as_str()) && !out.iter().any(|(_, f)| f == &c.field) {
                out.push((c.title.clone(), c.field.clone()));
            }
        }
        if self.on_board().is_some() {
            out.insert(0, ("Board column".into(), "boardColumn".into()));
        }
        out
    }

    /// `setGroupBy` — lists remember it (`listGroupBy.jira`), boards don't.
    pub fn set_group_by(&mut self, g: Option<String>) {
        self.collapsed.clear();
        if self.on_board().is_some() {
            self.board_group_by = g;
        } else {
            crate::platform::set_string("listGroupBy.jira", g.as_deref().unwrap_or(""));
            self.group_by = g;
        }
        self.selection = 0;
        self.refilter();
    }

    pub fn toggle_group(&mut self, name: &str) {
        if !self.collapsed.remove(name) {
            self.collapsed.insert(name.to_string());
        }
        self.refilter();
    }

    pub fn toggle_all_groups(&mut self, collapse: bool) {
        self.collapsed = if collapse {
            self.synth.iter().filter_map(|r| r.get("__group").cloned()).collect()
        } else {
            BTreeSet::new()
        };
        self.refilter();
    }

    /// Activate a synthetic row (header click / Return): toggle the group or
    /// show the next page. `false` for a real issue row.
    pub fn activate_synthetic(&mut self, row: usize) -> bool {
        let Some(r) = self.row(row).cloned() else { return false };
        if let Some(g) = r.get("__group") {
            self.toggle_group(g);
            return true;
        }
        if r.contains_key("__loadmore") {
            self.visible_offset += self.page_size;
            self.refilter();
            return true;
        }
        false
    }

    /// `groupedRows`: headers + member rows for the filtered/sorted `order`.
    fn group_rows(&mut self, order: &[usize], g: &str) -> Vec<usize> {
        const NONE: &str = "(none)";
        let mut fixed: Vec<String> = Vec::new();
        let people = if PERSON_FIELDS.contains(&g) { self.people_by_value() } else { HashMap::new() };
        let mut board_col: HashMap<String, String> = HashMap::new();
        match g {
            "statusCategory" => fixed = crate::views::jira::category_names().iter().map(|s| s.to_string()).collect(),
            "boardColumn" => {
                if let Some(id) = self.on_board() {
                    for c in self.board_column_spec(&id) {
                        let name = c.get("name").and_then(Value::as_str).unwrap_or("").to_string();
                        for st in c.get("statuses").and_then(Value::as_array).cloned().unwrap_or_default() {
                            if let Some(st) = st.as_str() {
                                board_col.entry(st.to_string()).or_insert_with(|| name.clone());
                            }
                        }
                        fixed.push(name);
                    }
                }
            }
            _ => {}
        }
        let key_of = |l: &JiraList, r: &Record| -> Vec<String> {
            match g {
                "statusCategory" => {
                    let st = r.get("status").map(String::as_str).unwrap_or("");
                    if st.is_empty() {
                        vec![NONE.into()]
                    } else {
                        vec![crate::views::jira::category_names()[l.status_category(st)].to_string()]
                    }
                }
                "boardColumn" => {
                    let st = r.get("status").map(String::as_str).unwrap_or("");
                    vec![board_col.get(st).cloned().unwrap_or_else(|| "Not on the board".into())]
                }
                _ => Self::cell_values(g, r)
                    .into_iter()
                    .map(|v| {
                        if v == EMPTY_VALUE {
                            if g == "assignee" { "Unassigned".into() } else { NONE.into() }
                        } else {
                            people.get(&v).map(|p| p.0.clone()).unwrap_or(v)
                        }
                    })
                    .collect(),
            }
        };
        let mut groups: HashMap<String, Vec<usize>> = HashMap::new();
        let mut seen: Vec<String> = Vec::new();
        for &i in order {
            for k in key_of(self, &self.all[i]) {
                let e = groups.entry(k.clone()).or_default();
                if e.is_empty() {
                    seen.push(k);
                }
                e.push(i);
            }
        }
        let tail = [NONE, "Unassigned", "Not on the board"];
        let mut rest: Vec<String> = seen.into_iter().filter(|k| !fixed.contains(k)).collect();
        rest.sort_by(|a, b| {
            let (ta, tb) = (tail.contains(&a.as_str()), tail.contains(&b.as_str()));
            ta.cmp(&tb)
                .then(groups[b].len().cmp(&groups[a].len()))
                .then_with(|| crate::engines::list_filter::localized_standard_compare(a, b))
        });
        let mut out = Vec::new();
        let names: Vec<String> = fixed.into_iter().filter(|f| groups.contains_key(f)).chain(rest).collect();
        for name in names {
            let idx = groups.remove(&name).unwrap_or_default();
            let shut = self.collapsed.contains(&name);
            let mut h = Record::new();
            h.insert("__group".into(), name.clone());
            h.insert("__count".into(), idx.len().to_string());
            if shut {
                h.insert("__collapsed".into(), "1".into());
            }
            out.push(self.all.len() + self.synth.len());
            self.synth.push(h);
            if !shut {
                out.extend(idx);
            }
        }
        out
    }

    /// `cellValues`: the filter values of one cell — multi-valued fields split

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
            let mut parts = v.split('|');
            let board = parts.next().unwrap_or("").to_string();
            let name = self.catalog.iter().find(|c| c.0 == board).map(|c| c.1.clone()).unwrap_or(format!("Board {board}"));
            // The ROW's own spec decides the sprint in the title — naming
            // the active sprint for every row made all pins look like
            // "current" and hid which sprint a click re-applies
            // (Mechanism 1 of `DEBUG-jira-sprint-switch-revert.md`).
            let sp = parts.next().unwrap_or("current");
            let sprint = match sp {
                "all" => "All issues".to_string(),
                "current" => self.sprint_name(&board),
                _ => self.sprint_title(&board, sp),
            };
            let label = if sprint.is_empty() { name } else { format!("{name} · {sprint}") };
            pinned.push(SidebarPin {
                key: format!("{VIEW_PREFIX}{v}"),
                label,
                icon: if v.ends_with("|table") { "tablecells" } else { "rectangle.split.3x1.fill" }.into(),
                section: "Starred".into(),
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
                section: "Starred".into(),
                meta: "Release".into(),
                tip: format!("Show every issue in {r} here"),
                ..Default::default()
            });
        }
        for l in str_list(&self.config, "pinnedLabels") {
            pinned.push(SidebarPin {
                key: format!("{LABEL_PREFIX}{l}"),
                label: l.clone(),
                icon: "tag".into(),
                section: "Starred".into(),
                meta: "Label".into(),
                tip: format!("Show every issue labelled {l} here"),
                ..Default::default()
            });
        }
        if !self.catalog.is_empty() || !self.boards.is_empty() {
            pinned.push(SidebarPin {
                key: BOARD_LIST_KEY.into(),
                label: "All boards".into(),
                icon: "square.grid.2x2".into(),
                section: "Boards".into(),
                meta: if self.catalog.is_empty() { String::new() } else { self.catalog.len().to_string() },
                tip: "Every board in the projects in scope — ☆ one to keep it here".into(),
                collapsible: !self.boards.is_empty(),
                expanded: !self.boards_collapsed,
                ..Default::default()
            });
        }
        // Starred boards are children of "All boards" (they came out of that
        // catalog); the disclosure triangle hides them.
        if !self.boards_collapsed {
            for (id, name) in &self.boards {
                pinned.push(SidebarPin {
                    key: format!("{BOARD_PREFIX}{id}"),
                    label: name.clone(),
                    icon: "rectangle.split.3x1".into(),
                    section: "Boards".into(),
                    depth: 1,
                    ..Default::default()
                });
            }
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
        self.board_sprints(board)
            .into_iter()
            .find(|s| s.state == "active")
            .map(|s| s.name)
            .unwrap_or_default()
    }

    /// The inspector content (`inspectorContent`): key, title, chips from
    /// `[jira] trailing`, body, fields from `detail` + project / updated.
    pub fn inspector_content(&self) -> Option<InspectorContent> {
        let r = self.selected().filter(|r| !is_synthetic(r))?;
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

/// `sprintSummary`'s `day(_:)`: the first ten chars `yyyy-MM-dd` -> `MMM d`
/// (US month names). A short / non-date value is `None`.
pub fn sprint_day(v: &str) -> Option<String> {
    let d = v.get(..10)?;
    let mut parts = d.split('-');
    let (Some(y), Some(m), Some(day)) = (parts.next(), parts.next(), parts.next()) else {
        return None;
    };
    if y.len() != 4 {
        return None;
    }
    let month: usize = m.parse().ok()?;
    let day: u32 = day.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    Some(format!("{} {}", MONTHS[month - 1], day))
}

/// A group header or the "load N more…" row (not an issue).
pub fn is_synthetic(r: &Record) -> bool {
    r.contains_key("__group") || r.contains_key("__loadmore")
}

/// `jiraIsReleaseRow`: a releases-tab row (not an issue).
pub fn is_release_row(r: &Record) -> bool {
    if r.contains_key("versionId") {
        return true;
    }
    let rel = r.get("release").map(String::as_str).unwrap_or("");
    let proj = r.get("project").map(String::as_str).unwrap_or("");
    let title = r.get("title").or_else(|| r.get("name")).map(String::as_str).unwrap_or("");
    !rel.is_empty() && !proj.is_empty() && r.get("key").map(String::as_str) == Some(&format!("{proj}-{rel}")) && title == rel
}

/// `jiraBrowseURL`: the issue page, or for a release its version page (or a
/// fixVersion JQL search when the version id is unknown).
pub fn browse_url(r: &Record, site: &str) -> Option<String> {
    let key = r.get("key").filter(|k| !k.is_empty())?;
    if site.is_empty() {
        return None;
    }
    if !is_release_row(r) {
        return Some(format!("{site}/browse/{key}"));
    }
    let proj = r.get("project").cloned().unwrap_or_default();
    let name = r.get("release").or_else(|| r.get("title")).cloned().unwrap_or_default();
    let id = r.get("versionId").cloned().unwrap_or_default();
    if !id.is_empty() {
        let tab = if site.contains(".atlassian.net") { "/tab/release-report-all-issues" } else { "" };
        return Some(format!("{site}/projects/{proj}/versions/{id}{tab}"));
    }
    let jql = format!("project = \"{proj}\" AND fixVersion = \"{name}\"");
    let enc: String = jql
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect();
    Some(format!("{site}/issues/?jql={enc}"))
}

/// `issueLabels`: the distinct labels of `rows`, first-seen order.
pub fn issue_labels<'a>(rows: impl IntoIterator<Item = &'a Record>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for r in rows {
        for l in r.get("labels").map(String::as_str).unwrap_or("").split(',').map(str::trim) {
            if !l.is_empty() && !out.iter().any(|o| o == l) {
                out.push(l.to_string());
            }
        }
    }
    out
}

/// `ListSession.emptyValue`: the filter value of a blank cell.
pub const EMPTY_VALUE: &str = "\u{0}none";

/// `ListSession.personFields`.
pub const PERSON_FIELDS: &[&str] = &["assignee", "reporter", "creator"];

/// `ListSession.multiValued`: fields whose cell is a comma list.
pub const MULTI_VALUED: &[&str] = &["labels", "release", "releaseLabel", "releaseDate", "components", "fixVersions"];

/// One row of a column's filter picker (`JiraMultiPicker.Option`).
#[derive(Clone, Debug, PartialEq)]
pub struct FilterOption {
    pub id: String,
    pub title: String,
    pub count: usize,
}

/// What the view reports.
#[derive(Clone, Debug, PartialEq)]
pub enum JiraEvent {
    Click { row: usize, double: bool },
    Check(usize),
    Star(usize),
    Sort(usize),
    /// Click on a filterable column's chevron (`onTableFilter`); `x`/`y` is
    /// the screen point under the chevron to drop the picker from.
    Filter { col: usize, x: f64, y: f64 },
    Query(String),
    Enter,
    Move(i64),
    Escape,
    OpenDetail,
    RowMenu { row: usize, x: f64, y: f64 },
    /// Right-click on the column header (`PopupTableHeader.rightMouseDown`).
    HeaderMenu { x: f64, y: f64 },
    /// A divider drag (`onResize`): new percentage widths; `done` on mouse-up.
    ColumnsResized { pcts: Vec<f64>, done: bool },
    /// A header drag (`onReorder`): move column `from` to `to`.
    ColumnsReordered { from: usize, to: usize },
    /// The lead gutter's ↔ glyph (`onFit`).
    FitColumns,
    /// The board bar's sprint picker (`boardBar.onSprint`).
    BoardSprint(String),
    /// The board bar's Columns / Table switch (`boardBar.onMode`).
    BoardMode(String),
    /// The board bar's Pin button (`boardBar.onPin`).
    BoardPin,
    /// The Tickets / Config pill (`tabPicked:`) — the selected surface.
    Tab(JiraSurface),
    /// A Config-tab control press (`JiraConfigAction`).
    Config(crate::views::jira_config::JiraConfigAction),
    /// A kanban card was clicked (`open:KEY` from the board page).
    OpenKey(String),
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
    /// Pinned board ids (`boards`): the "All boards" ☆ shows these, not
    /// favourites (a board key could never resolve as a favourite).
    pub pinned_boards: BTreeSet<String>,
    pub sort: Option<(usize, bool)>,
    /// Columns with a filter set (`tableFilterActive`): their chevron tints.
    pub filter_active: BTreeSet<usize>,
    /// Compact cells (status dot, priority glyph, initials, short dates).
    pub compact: bool,
    /// Every column reserves its full header label's width (`[jira]
    /// reserve-label-width`).
    pub reserve_label: bool,
    /// The board bar (`updateBoardChrome`) when a board is open, else `None`.
    pub board_bar: Option<JiraBoardBarModel>,
    /// The kanban columns when a board is open in `columns` mode — the web
    /// overlay's data. `None` shows the normal table (or the board's table
    /// mode).
    pub board_columns: Option<Vec<BoardColumn>>,
    pub sidebar: SidebarModel,
    pub inspector: Option<InspectorContent>,
    pub show_inspector: bool,
    pub query: String,
    pub empty: String,
    pub rules: Option<StyleRules>,
    /// The shown place ("tab:<i>" / "pin:<key>" / ""): the view snaps the
    /// scroll to the top only when this (or the columns) changed — new
    /// rows arriving in the SAME list must not throw away where the user
    /// scrolled (the poller sorts newest-first, so the first key changed
    /// every tick).
    pub place: String,
}

impl JiraList {
    pub fn snapshot(&self) -> JiraSnapshot {
        let bar = self.board_bar();
        let columns = if self.on_board_columns() && !self.board_cols.is_empty() {
            Some(self.board_cols.clone())
        } else {
            None
        };
        if let Some(mut board) = self.board_snapshot() {
            board.board_bar = bar;
            board.board_columns = columns;
            return board;
        }
        let rows: Vec<Record> = self.rows.iter().filter_map(|&i| self.rec(i).cloned()).collect();
        let sort = self
            .sort
            .as_ref()
            .and_then(|(f, asc)| self.columns.iter().position(|c| &c.field == f).map(|i| (i, *asc)));
        JiraSnapshot {
            columns: self.columns.clone(),
            rows,
            selection: self.selection,
            checked: self.checked.clone(),
            favs: self.favs.iter().cloned().chain(self.favorite_releases()).collect(),
            pinned_boards: self.boards.iter().map(|(id, _)| id.clone()).collect(),
            sort,
            filter_active: self
                .columns
                .iter()
                .enumerate()
                .filter(|(_, c)| self.col_filters.contains_key(&c.field))
                .map(|(i, _)| i)
                .collect(),
            compact: self.compact_cells(),
            reserve_label: self.reserve_label_width(),
            board_bar: bar,
            board_columns: columns,
            sidebar: self.sidebar_model(),
            inspector: self.inspector_content(),
            show_inspector: self.inspector,
            query: self.query.clone(),
            empty: if self.tabs.is_empty() {
                "No synced Jira lists yet — run the Jira setup (Tickets ▸ Config, or the palette's \"Jira Config\").".into()
            } else if self.all.is_empty() {
                "Nothing in this list.".into()
            } else {
                "No matches.".into()
            },
            rules: self.rules.clone(),
            place: self.place_key(),
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
            JiraEvent::Star(row) => self.toggle_star(row),
            JiraEvent::Sort(i) => self.sort_by(i),
            // The Fit Columns menu and the filter picker are UI actions.
            JiraEvent::HeaderMenu { .. }
            | JiraEvent::Filter { .. }
            | JiraEvent::FitColumns
            // The Tickets / Config pill and the Config controls are UI-only.
            | JiraEvent::Tab(_)
            | JiraEvent::Config(_) => {}
            JiraEvent::ColumnsResized { pcts, .. } => self.set_column_widths(&pcts),
            JiraEvent::ColumnsReordered { from, to } => self.move_column(from, to),
            JiraEvent::Query(q) => {
                self.query = q;
                self.selection = 0;
                self.visible_offset = 0;
                self.refilter();
                // The kanban's lanes follow the search (`renderBoardColumns`
                // ran on every query change in Swift).
                if self.on_board().is_some() {
                    self.refresh_board_columns();
                }
            }
            JiraEvent::Move(d) => {
                let n = self.rows.len() as i64;
                if n > 0 {
                    self.selection = (self.selection as i64 + d).clamp(0, n - 1) as usize;
                }
            }
            JiraEvent::Escape => {
                self.escape_step();
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
            JiraEvent::BoardSprint(s) => {
                self.pick_sprint(&s);
            }
            JiraEvent::BoardMode(m) => {
                self.set_mode(&m);
            }
            JiraEvent::BoardPin => {
                if let Some(spec) = self.current_view_spec() {
                    self.toggle_pin_view(&spec);
                }
            }
            // Opening a card is a UI action (browser URL).
            JiraEvent::OpenKey(_) => {}
            JiraEvent::Sidebar(SidebarEvent::Select(i)) => self.select_tab(i),
            JiraEvent::Sidebar(SidebarEvent::Pinned(key)) => {
                // A pin that cannot open says why (status line + toast) —
                // none of these may fail silently.
                if let Some(id) = key.strip_prefix(MY_WORK_PREFIX) {
                    if let Err(e) = self.open_my_work(id) {
                        self.surface_error(&format!("My work {id}: {e}"));
                    }
                } else if key == BOARD_LIST_KEY {
                    self.show_board_list();
                } else if let Some(id) = key.strip_prefix(BOARD_PREFIX) {
                    // `open_board` surfaces its own reason / pins + opens a
                    // never-pinned board.
                    let _ = self.open_board(id);
                } else if let Some(spec) = key.strip_prefix(VIEW_PREFIX) {
                    if !self.open_view(spec) {
                        self.surface_error(&format!("Board view {spec}: not synced yet"));
                    }
                } else if let Some(name) = key.strip_prefix(LABEL_PREFIX) {
                    if let Err(e) = self.open_label(name) {
                        self.surface_error(&format!("Label {name}: {e}"));
                    }
                } else if let Err(e) = self.open_release(&key) {
                    self.surface_error(&format!("Release {key}: {e}"));
                }
            }
            JiraEvent::Sidebar(SidebarEvent::TogglePin(key)) => {
                if key == BOARD_LIST_KEY {
                    self.toggle_boards_collapsed();
                }
            }
            JiraEvent::Sidebar(_) => {}
        }
    }
}

#[cfg(target_os = "macos")]
pub use appkit::{draw_wrapped, set_zoom, zoom, JiraView};

#[cfg(target_os = "macos")]
mod appkit {
    use super::*;
    use crate::ui::chrome::Rect;
    use crate::ui::pane_divider::{DividerRange, PaneDividerView};
    use crate::ui::pill_selector::TabPillView;
    use crate::ui::sidebar::SidebarView;
    use crate::ui::style::{self, ButtonState, Weight};
    use crate::ui::theme::{PopupColors, Rgba};
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, ProtocolObject, Sel};
    use objc2::{define_class, msg_send, sel, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, Message};
    use objc2_app_kit::{
        NSStringDrawingDeprecated, NSBezierPath, NSButton, NSButtonType, NSControlSize, NSControlTextEditingDelegate,
        NSEvent, NSFont, NSGraphicsContext, NSLineBreakMode, NSMenuItem, NSPopUpButton,
        NSScrollView, NSStringDrawingOptions,
        NSTextField, NSTextFieldDelegate, NSTextView, NSTrackingArea, NSTrackingAreaOptions, NSView,
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

    thread_local! {
        /// The list's text zoom (Cmd +/−, `textZoom.jira`).
        static ZOOM: Cell<f64> = Cell::new(
            crate::platform::get_string("textZoom.jira").and_then(|v| v.parse().ok()).unwrap_or(1.0)
        );
    }

    pub fn zoom() -> f64 {
        ZOOM.with(|z| z.get())
    }

    pub fn set_zoom(z: f64) {
        ZOOM.with(|c| c.set(z));
    }

    fn row_h() -> f64 {
        (ROW_H * zoom()).round()
    }

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
                let i = (p.y / row_h()).floor();
                let h = (i >= 0.0 && (i as usize) < n).then_some(i as usize);
                if h != self.ivars().hover.get() {
                    self.ivars().hover.set(h);
                    self.setNeedsDisplay(true);
                }
                self.update_cell_tip(h, p.x);
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
                let i = (p.y / row_h()).floor();
                if i < 0.0 || i as usize >= n {
                    return;
                }
                let row = i as usize;
                let band = Rect::new(0.0, row as f64 * row_h(), self.bounds().size.width, row_h());
                let synthetic = self.ivars().snap.borrow().rows.get(row).is_some_and(is_synthetic);
                let ev = if synthetic {
                    JiraEvent::Click { row, double: false }
                } else if check_rect(band).inset_by(-4.0, -6.0).contains(p.x, p.y) {
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
                let i = (p.y / row_h()).floor();
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

        /// A compact cell under the pointer shows its full value as the tooltip.
        fn update_cell_tip(&self, row: Option<usize>, x: f64) {
            let tip = (|| {
                let snap = self.ivars().snap.borrow();
                if !snap.compact {
                    return None;
                }
                let r = snap.rows.get(row?)?;
                let w = self.bounds().size.width;
                let frames = list_frames(&snap.columns, LEAD, w - LEAD - (PADDING + 10.0), true, snap.reserve_label);
                let k = frames.iter().position(|(fx, fw)| x >= *fx && x < fx + fw)?;
                let col = snap.columns.get(k)?;
                compact_width(&col.field)?;
                let v = r.get(&col.field).map(|v| display_cell(&col.field, v)).filter(|v| !v.trim().is_empty())?;
                Some(format!("{}: {v}", col.title))
            })();
            let cur = self.toolTip().map(|t| t.to_string());
            if cur != tip {
                self.setToolTip(tip.as_deref().map(NSString::from_str).as_deref());
            }
        }

        /// The visible rows' rects (for the vim search highlight overlay).
        pub fn vim_shown_rects(&self) -> Vec<(usize, Rect)> {
            let n = self.ivars().snap.borrow().rows.len();
            let vis = self.visibleRect();
            if n == 0 || vis.size.height <= 0.0 {
                return Vec::new();
            }
            let first = (vis.origin.y / row_h()).floor().max(0.0) as usize;
            let last = (((vis.origin.y + vis.size.height) / row_h()).floor() as usize).min(n - 1);
            if first > last {
                return Vec::new();
            }
            let w = self.bounds().size.width;
            (first..=last).map(|i| (i, Rect::new(0.0, i as f64 * row_h(), w, row_h()))).collect()
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
            let first = (dirty.origin.y / row_h()).floor().max(0.0) as usize;
            let last = (((dirty.origin.y + dirty.size.height) / row_h()).ceil() as usize).min(snap.rows.len());
            let frames = list_frames(&snap.columns, LEAD, w - LEAD - (PADDING + 10.0), snap.compact, snap.reserve_label);
            let font = style::system_font(12.0 * zoom(), Weight::Regular);
            let bold = style::system_font(12.0 * zoom(), Weight::Bold);
            let rules = snap.rules.clone().unwrap_or_else(|| style_rules(&Value::Null));
            let hover = self.ivars().hover.get();
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
            for i in first..last {
                let r = &snap.rows[i];
                let band = Rect::new(0.0, i as f64 * row_h(), w, row_h());
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
                if let Some(g) = r.get("__group") {
                    // `groupedRows` header: ▸/▾, the name, its row count.
                    let shut = r.contains_key("__collapsed");
                    let tri = Rect::new(PADDING + 4.0, band.mid_y() - 5.0, 10.0, 10.0);
                    style::draw_text_centered(if shut { "▸" } else { "▾" }, tri.inset_by(-3.0, -3.0), &bold, c.dim);
                    let name = if g.is_empty() { "(none)" } else { g.as_str() };
                    let (nw, nh) = style::text_size(name, &bold);
                    style::draw_text(name, PADDING + 22.0, band.mid_y() - nh / 2.0, &bold, c.text);
                    let n = r.get("__count").cloned().unwrap_or_default();
                    style::draw_text(&n, PADDING + 22.0 + nw + 8.0, band.mid_y() - nh / 2.0, &font, c.dim);
                    style::hline(PADDING, w - PADDING - 6.0, band.y + band.height - 0.5, c.hairline());
                    continue;
                }
                if let Some(t) = r.get("__loadmore") {
                    let (_, th) = style::text_size(t, &font);
                    style::draw_text(t, LEAD + 3.0, band.mid_y() - th / 2.0, &font, c.accent);
                    continue;
                }
                let key = r.get("key").cloned().unwrap_or_default();
                draw_check(check_rect(band), snap.checked.contains(&key), &c);
                // A board row's ☆ is its sidebar pin, not a favourite.
                let star_on = key
                    .strip_prefix(BOARD_PREFIX)
                    .map(|id| snap.pinned_boards.contains(id))
                    .unwrap_or_else(|| snap.favs.contains(&key));
                draw_star(star_rect(band), star_on, &c);
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
                    if snap.compact && compact_width(&col.field).is_some() {
                        let raw = r.get(&col.field).map(String::as_str).unwrap_or("");
                        draw_compact_cell(&col.field, raw, st.as_ref(), Rect::new(fx, band.y, fw, band.height), &c, alpha, &font, now);
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

    /// One compact cell: a status mark, a priority glyph, initials in a
    /// disc, a short age, or the plain (short) value.
    #[allow(clippy::too_many_arguments)]
    fn draw_compact_cell(field: &str, raw: &str, st: Option<&CellStyle>, cell: Rect, c: &PopupColors, alpha: f64, font: &Retained<NSFont>, now: i64) {
        let raw = raw.trim();
        if raw.is_empty() {
            return;
        }
        let z = zoom();
        match field {
            "status" => {
                let hue = st.map(|s| c.tone(tone_of(s.tone.as_deref()))).unwrap_or(c.dim);
                let mark = st.and_then(|s| s.mark.clone()).unwrap_or_else(|| "filled".into());
                let side = (11.0 * z).round();
                let r = Rect::new((cell.x + 8.0).round(), (cell.mid_y() - side / 2.0).round(), side, side);
                draw_mark(r, &mark, hue.with_alpha(hue.a * alpha));
            }
            "priority" => {
                let (g, tone) = priority_glyph(raw);
                let f = style::system_font(13.0 * z, Weight::Bold);
                style::draw_text_left(g, Rect::new(cell.x + 8.0, cell.y, cell.width - 8.0, cell.height), &f, c.tone(tone).with_alpha(alpha));
            }
            "assignee" | "reporter" | "creator" => {
                let d = (22.0 * z).round();
                let r = Rect::new((cell.x + 6.0).round(), (cell.mid_y() - d / 2.0).round(), d, d);
                // A stable colour per person from the palette.
                let h = raw.bytes().fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
                let tones = [PopupTone::Accent, PopupTone::Accent2, PopupTone::Success, PopupTone::Warning, PopupTone::Info];
                let fill = c.tone(tones[(h as usize) % tones.len()]);
                style::fill_rounded(r, d / 2.0, fill.with_alpha(0.85 * alpha));
                let f = style::system_font(9.5 * z, Weight::Bold);
                style::draw_text_centered(&initials(raw), r, &f, c.base().with_alpha(alpha));
            }
            "updated" | "created" | "resolved" => {
                let t = parse_jira_time(raw).map(|t| relative_age(t, now)).unwrap_or_else(|| display_cell(field, raw));
                style::draw_text_right(&t, Rect::new(cell.x, cell.y, cell.width - 6.0, cell.height), font, c.dim.with_alpha(alpha));
            }
            _ => {
                style::draw_text_left(&display_cell(field, raw), Rect::new(cell.x + 3.0, cell.y, cell.width - 6.0, cell.height), font, c.dim.with_alpha(alpha));
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
        /// Divider drag: (divider, start x, start percentages).
        drag: RefCell<Option<(usize, f64, Vec<f64>)>>,
        /// A column press that may become a reorder; the live reorder.
        press_col: Cell<Option<usize>>,
        reorder: RefCell<Option<(usize, f64)>>,
        down_x: Cell<f64>,
        moved: Cell<bool>,
    }

    fn divider_rect((fx, fw): (f64, f64), h: f64) -> Rect {
        Rect::new(fx + fw - 4.0, 0.0, 8.0, h)
    }

    fn filter_rect(fx: f64, fw: f64, h: f64) -> Rect {
        Rect::new(fx + fw - 16.0 - 5.0, h / 2.0 - 8.0, 16.0, 16.0)
    }

    fn fit_rect(h: f64) -> Rect {
        Rect::new(LEAD / 2.0 + 6.0 - 1.0, h / 2.0 - 9.0, 18.0, 18.0)
    }

    /// `dividerPercents`: move one divider, both neighbours keep ≥ 3 %.
    fn divider_percents(d: &(usize, f64, Vec<f64>), x: f64, usable: f64) -> Vec<f64> {
        let (i, start_x, start) = d;
        let mut p = start.clone();
        if *i + 1 >= p.len() {
            return p;
        }
        let min = 3.0;
        let delta = ((x - start_x) / usable.max(1.0) * 100.0).max(min - p[*i]).min(p[*i + 1] - min);
        p[*i] += delta;
        p[*i + 1] -= delta;
        p.iter().map(|v| (v * 10.0).round() / 10.0).collect()
    }

    /// `reorderTarget`: the slot the dragged column lands in.
    fn reorder_target(frames: &[(f64, f64)], from: usize, x: f64) -> usize {
        let mut slot = frames.iter().filter(|(fx, fw)| fx + fw / 2.0 < x).count();
        if slot > from {
            slot -= 1;
        }
        slot.min(frames.len().saturating_sub(1))
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
                // `PopupTableHeaderView.mouseDown`: a divider starts a resize,
                // a column press may become a reorder drag; sort / filter /
                // fit wait for mouse-up.
                let p = self.convertPoint_fromView(e.locationInWindow(), None);
                let iv = self.ivars();
                iv.down_x.set(p.x);
                iv.moved.set(false);
                *iv.reorder.borrow_mut() = None;
                iv.press_col.set(None);
                let (frames, usable) = self.frames();
                let n = frames.len();
                if n > 1 {
                    if let Some(i) = (0..n - 1).find(|&i| divider_rect(frames[i], self.bounds().size.height).contains(p.x, p.y)) {
                        *iv.drag.borrow_mut() = Some((i, p.x, frames.iter().map(|(_, w)| w / usable * 100.0).collect()));
                        return;
                    }
                }
                *iv.drag.borrow_mut() = None;
                if n > 1 && self.filter_hit(p.x, p.y).is_none() {
                    iv.press_col.set(frames.iter().position(|(x, w)| p.x >= *x && p.x < x + w));
                }
            }

            #[unsafe(method(mouseDragged:))]
            fn mouse_dragged(&self, e: &NSEvent) {
                let p = self.convertPoint_fromView(e.locationInWindow(), None);
                let iv = self.ivars();
                if let (Some(from), true) = (iv.press_col.get(), iv.drag.borrow().is_none()) {
                    if iv.reorder.borrow().is_none() && (p.x - iv.down_x.get()).abs() < 6.0 {
                        return;
                    }
                    *iv.reorder.borrow_mut() = Some((from, p.x));
                    objc2_app_kit::NSCursor::closedHandCursor().set();
                    self.setNeedsDisplay(true);
                    return;
                }
                let d = iv.drag.borrow().clone();
                let Some(d) = d else { return };
                iv.moved.set(true);
                let (_, usable) = self.frames();
                self.emit(JiraEvent::ColumnsResized { pcts: divider_percents(&d, p.x, usable), done: false });
            }

            #[unsafe(method(mouseUp:))]
            fn mouse_up(&self, e: &NSEvent) {
                let p = self.convertPoint_fromView(e.locationInWindow(), None);
                let iv = self.ivars();
                iv.press_col.set(None);
                let r = iv.reorder.borrow_mut().take();
                if let Some((from, _)) = r {
                    objc2_app_kit::NSCursor::arrowCursor().set();
                    self.setNeedsDisplay(true);
                    let to = reorder_target(&self.frames().0, from, p.x);
                    if to != from {
                        self.emit(JiraEvent::ColumnsReordered { from, to });
                    }
                    if let Some(w) = self.window() {
                        w.invalidateCursorRectsForView(self);
                    }
                    return;
                }
                let d = iv.drag.borrow_mut().take();
                if let Some(d) = d {
                    // A release away from the press commits even when no
                    // drag event arrived in between.
                    if iv.moved.get() || (p.x - iv.down_x.get()).abs() >= 2.0 {
                        let (_, usable) = self.frames();
                        self.emit(JiraEvent::ColumnsResized { pcts: divider_percents(&d, p.x, usable), done: true });
                        if let Some(w) = self.window() {
                            w.invalidateCursorRectsForView(self);
                        }
                    }
                    return;
                }
                if (p.x - iv.down_x.get()).abs() >= 5.0 {
                    return;
                }
                if fit_rect(self.bounds().size.height).inset_by(-3.0, -3.0).contains(p.x, p.y) {
                    self.emit(JiraEvent::FitColumns);
                    return;
                }
                if let Some(i) = self.filter_hit(p.x, p.y) {
                    let (frames, _) = self.frames();
                    let (fx, fw) = frames[i];
                    let h = self.bounds().size.height;
                    let at = self.convertPoint_toView(NSPoint::new(fx + fw - 21.0, h), None);
                    let screen = self.window().map(|w| w.convertPointToScreen(at)).unwrap_or(at);
                    self.emit(JiraEvent::Filter { col: i, x: screen.x, y: screen.y });
                    return;
                }
                let (frames, _) = self.frames();
                if let Some(i) = frames.iter().position(|(x, w)| p.x >= *x && p.x < x + w) {
                    self.emit(JiraEvent::Sort(i));
                }
            }

            #[unsafe(method(resetCursorRects))]
            fn reset_cursor_rects(&self) {
                let (frames, _) = self.frames();
                let h = self.bounds().size.height;
                let cols = self.ivars().snap.borrow().columns.clone();
                for (i, &(fx, fw)) in frames.iter().enumerate() {
                    if cols.get(i).is_some_and(|c| c.filterable) && fw > 30.0 {
                        self.addCursorRect_cursor(ns(filter_rect(fx, fw, h)), &objc2_app_kit::NSCursor::pointingHandCursor());
                    }
                }
                self.addCursorRect_cursor(ns(fit_rect(h)), &objc2_app_kit::NSCursor::pointingHandCursor());
                if frames.len() > 1 {
                    for &f in &frames[..frames.len() - 1] {
                        self.addCursorRect_cursor(ns(divider_rect(f, h)), &objc2_app_kit::NSCursor::resizeLeftRightCursor());
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
            let this = Self::alloc(mtm).set_ivars(HeaderIvars {
                snap,
                colors: Cell::new(colors),
                sink: RefCell::new(None),
                drag: RefCell::new(None),
                press_col: Cell::new(None),
                reorder: RefCell::new(None),
                down_x: Cell::new(0.0),
                moved: Cell::new(false),
            });
            unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] }
        }

        /// The column frames and the usable width they share.
        fn frames(&self) -> (Vec<(f64, f64)>, f64) {
            let w = self.bounds().size.width;
            let usable = w - LEAD - (PADDING + 10.0);
            let snap = self.ivars().snap.borrow();
            (list_frames(&snap.columns, LEAD, usable, snap.compact, snap.reserve_label), usable)
        }

        /// The filterable column whose chevron (padded) holds the point.
        fn filter_hit(&self, x: f64, y: f64) -> Option<usize> {
            let (frames, _) = self.frames();
            let h = self.bounds().size.height;
            let cols = self.ivars().snap.borrow().columns.clone();
            frames.iter().enumerate().position(|(i, &(fx, fw))| {
                cols.get(i).is_some_and(|c| c.filterable)
                    && fw > 30.0
                    && (filter_rect(fx, fw, h).inset_by(-5.0, -4.0).contains(x, y))
            })
        }

        fn emit(&self, ev: JiraEvent) {
            let s = self.ivars().sink.borrow().clone();
            if let Some(s) = s {
                s(ev);
            }
        }

        fn draw(&self) {
            let c = self.ivars().colors.get();
            let b = self.bounds();
            let (w, h) = (b.size.width, b.size.height);
            style::fill_rect(Rect::new(0.0, 0.0, w, h), c.mantle());
            let snap = self.ivars().snap.borrow();
            let frames = list_frames(&snap.columns, LEAD, w - LEAD - (PADDING + 10.0), snap.compact, snap.reserve_label);
            let font = style::system_font(12.0 * 0.86 * zoom(), Weight::Bold);
            // The "fit columns" glyph in the lead gutter.
            style::draw_symbol("arrow.left.and.right", Rect::new(LEAD / 2.0 + 6.0, 0.0, 16.0, h), c.dim.with_alpha(0.9), 9.0);
            for (i, col) in snap.columns.iter().enumerate() {
                let Some(&(fx, fw)) = frames.get(i) else { continue };
                let mut title = col.title.to_uppercase();
                if snap.compact && !snap.reserve_label && compact_width(&col.field).is_some() {
                    // A narrow column keeps a short label; the tooltip names it.
                    title = match col.field.as_str() {
                        "status" => "ST".into(),
                        "assignee" => "AS".into(),
                        "reporter" => "RE".into(),
                        "creator" => "CR".into(),
                        "priority" => "PR".into(),
                        "project" => "PROJ".into(),
                        "key" => "KEY".into(),
                        "updated" => "UPD".into(),
                        "created" => "NEW".into(),
                        _ => title.chars().take(3).collect(),
                    };
                }
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
                    if snap.filter_active.contains(&i) {
                        style::fill_rounded(fr, 4.0, c.accent_on().with_alpha(0.22));
                        style::chevron(fr.inset_by(4.0, 4.0), c.accent_on());
                    } else {
                        style::chevron(fr.inset_by(4.0, 4.0), c.dim.with_alpha(0.9));
                    }
                }
                if i + 1 < snap.columns.len() {
                    style::vline(fx + fw - 0.5, 6.0, h - 6.0, c.hairline());
                }
            }
            // A live reorder: tint the dragged column, mark the drop slot.
            if let Some((from, x)) = *self.ivars().reorder.borrow() {
                if let Some(&(fx, fw)) = frames.get(from) {
                    style::fill_rect(Rect::new(fx, 0.0, fw, h), c.accent_on().with_alpha(0.15));
                }
                let to = reorder_target(&frames, from, x);
                if let Some(&(tx, tw)) = frames.get(to) {
                    let edge = if to > from { tx + tw } else { tx };
                    style::fill_rect(Rect::new(edge - 1.0, 4.0, 2.0, h - 8.0), c.accent_on());
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

    /// The px each column needs so its full header label fits: the uppercase
    /// label + the sort arrow + the filter chevron (`[jira] reserve-label-width`).
    fn header_min_widths(cols: &[ListColumn]) -> Vec<f64> {
        let hfont = style::system_font(12.0 * 0.86 * zoom(), Weight::Bold);
        cols.iter()
            .map(|c| {
                let mut w = style::text_size(&kerned(&format!("{} ↑", c.title.to_uppercase())), &hfont).0 + 12.0;
                if c.filterable {
                    w += 22.0;
                }
                w.ceil()
            })
            .collect()
    }

    /// The column frames; with `reserve` each column keeps at least its full
    /// header label's width (so no header is clipped).
    fn list_frames(cols: &[ListColumn], x0: f64, usable: f64, compact: bool, reserve: bool) -> Vec<(f64, f64)> {
        if reserve {
            frames_for_reserved(cols, x0, usable, compact, &header_min_widths(cols))
        } else {
            frames_for(cols, x0, usable, compact)
        }
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

    // -------------------------------------------------------- board bar

    /// `JiraBoardBar` — the open board's top bar: title, sprint popup,
    /// Columns / Table switch, issue summary, Pin.
    pub struct BoardBarIvars {
        title: Retained<NSTextField>,
        sprint: Retained<NSPopUpButton>,
        mode: Retained<TabPillView>,
        summary: Retained<NSTextField>,
        pin: Retained<NSButton>,
        /// The sprint menu structure (one entry per choice; the popup's
        /// `indexOfSelectedItem` indexes this).
        choices: RefCell<Vec<SprintChoice>>,
        colors: Cell<PopupColors>,
        sink: RefCell<Option<Sink>>,
    }

    define_class!(
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSJiraBoardBar"]
        #[ivars = BoardBarIvars]
        pub struct JiraBoardBarView;

        impl JiraBoardBarView {
            #[unsafe(method(isFlipped))]
            fn is_flipped(&self) -> bool { true }

            #[unsafe(method(sprintPicked:))]
            fn sprint_picked(&self, _sender: &AnyObject) {
                let i = self.ivars().sprint.indexOfSelectedItem();
                let choice = self.ivars().choices.borrow().get(i.max(0) as usize).cloned();
                if let Some(c) = choice {
                    if !c.header && !c.id.is_empty() {
                        self.emit(JiraEvent::BoardSprint(c.id));
                    }
                }
            }

            #[unsafe(method(pinClicked:))]
            fn pin_clicked(&self, _sender: &AnyObject) {
                self.emit(JiraEvent::BoardPin);
            }

            #[unsafe(method(drawRect:))]
            fn draw_rect(&self, _r: NSRect) {
                let iv = self.ivars();
                let b = self.bounds();
                style::fill_rect(
                    Rect::new(0.0, b.size.height - 1.0, b.size.width, 1.0),
                    iv.colors.get().hairline(),
                );
            }
        }

        unsafe impl NSObjectProtocol for JiraBoardBarView {}
    );

    impl JiraBoardBarView {
        fn emit(&self, ev: JiraEvent) {
            if let Some(s) = self.ivars().sink.borrow().as_ref() {
                s(ev);
            }
        }

        pub fn set_on_event(&self, cb: Box<dyn Fn(JiraEvent)>) {
            *self.ivars().sink.borrow_mut() = Some(Rc::from(cb));
        }

        pub fn new(mtm: MainThreadMarker, colors: PopupColors) -> Retained<Self> {
            let title = NSTextField::labelWithString(&NSString::from_str(""), mtm);
            title.setFont(Some(&style::system_font(14.0, Weight::Semibold)));
            title.setTextColor(Some(&colors.text.to_nscolor()));
            title.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
            title.setDrawsBackground(false);
            title.setBezeled(false);
            title.setEditable(false);
            title.setSelectable(false);

            let sprint = NSPopUpButton::new(mtm);
            sprint.setControlSize(NSControlSize::Small);
            sprint.setHidden(true);

            let mode = TabPillView::new(mtm, colors, &["Columns", "Table"]);

            let summary = NSTextField::labelWithString(&NSString::from_str(""), mtm);
            summary.setFont(Some(&style::system_font(11.5, Weight::Regular)));
            summary.setTextColor(Some(&colors.dim.to_nscolor()));
            summary.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
            summary.setDrawsBackground(false);
            summary.setBezeled(false);
            summary.setEditable(false);
            summary.setSelectable(false);

            let pin = unsafe {
                NSButton::buttonWithTitle_target_action(
                    &NSString::from_str("☆ Star"),
                    None,
                    None,
                    mtm,
                )
            };
            pin.setButtonType(NSButtonType::MomentaryPushIn);
            pin.setControlSize(NSControlSize::Small);
            pin.setFont(Some(&style::system_font(11.0, Weight::Regular)));

            let this = Self::alloc(mtm).set_ivars(BoardBarIvars {
                title,
                sprint,
                mode,
                summary,
                pin,
                choices: RefCell::new(Vec::new()),
                colors: Cell::new(colors),
                sink: RefCell::new(None),
            });
            let v: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
            let iv = v.ivars();
            unsafe {
                iv.sprint.setTarget(Some(as_any(&*v)));
                iv.sprint.setAction(Some(sel!(sprintPicked:)));
                iv.pin.setTarget(Some(as_any(&*v)));
                iv.pin.setAction(Some(sel!(pinClicked:)));
            }
            let weak_mode = objc2::rc::Weak::from_retained(&v);
            iv.mode.set_on_pick(Box::new(move |i| {
                if let Some(bar) = weak_mode.load() {
                    bar.emit(JiraEvent::BoardMode(if i == 1 { "table".to_string() } else { "columns".to_string() }));
                }
            }));
            for sub in [&*iv.title as &NSView, &iv.sprint, &iv.mode, &iv.summary, &iv.pin] {
                v.addSubview(sub);
            }
            v
        }

        pub fn set_model(&self, m: &JiraBoardBarModel, colors: PopupColors) {
            let iv = self.ivars();
            iv.colors.set(colors);
            iv.title.setStringValue(&NSString::from_str(&m.board));
            iv.title.setTextColor(Some(&colors.text.to_nscolor()));
            iv.summary.setStringValue(&NSString::from_str(&m.summary));
            iv.summary.setTextColor(Some(&colors.dim.to_nscolor()));
            iv.pin.setTitle(&NSString::from_str(m.pin_title()));
            iv.pin.setToolTip(Some(&NSString::from_str(if m.pinned {
                "Unstar this view (it leaves the sidebar)"
            } else {
                "Star this exact view (board + sprint) to keep a row in the sidebar"
            })));
            // Rebuild the sprint menu: one entry per choice, headers become a
            // separator (or a disabled item when they carry a title).
            iv.sprint.removeAllItems();
            iv.sprint.setHidden(m.choices.is_empty());
            let mtm = MainThreadMarker::new();
            for c in &m.choices {
                if c.header {
                    let Some(mtm) = mtm else { continue };
                    let Some(menu) = iv.sprint.menu() else { continue };
                    let item = if c.title.is_empty() {
                        NSMenuItem::separatorItem(mtm)
                    } else {
                        let it = unsafe {
                            NSMenuItem::initWithTitle_action_keyEquivalent(
                                NSMenuItem::alloc(mtm),
                                &NSString::from_str(&c.title),
                                None,
                                &NSString::from_str(""),
                            )
                        };
                        it.setEnabled(false);
                        it
                    };
                    menu.addItem(&item);
                } else {
                    iv.sprint.addItemWithTitle(&NSString::from_str(&c.title));
                }
            }
            iv.sprint.setAutoenablesItems(false);
            if let Some(i) = m.choices.iter().position(|c| !c.header && c.id == m.picked) {
                iv.sprint.selectItemAtIndex(i as isize);
            }
            iv.mode.set_selected(if m.mode == "table" { 1 } else { 0 });
            *iv.choices.borrow_mut() = m.choices.clone();
            self.layout_controls();
            self.setNeedsDisplay(true);
        }

        pub fn layout_controls(&self) {
            let iv = self.ivars();
            let b = self.bounds();
            let (w, h) = (b.size.width, b.size.height);
            // Natural widths from AppKit's own metrics (accurate for the font).
            iv.title.sizeToFit();
            iv.pin.sizeToFit();
            // The ☆ leads, exactly like a row's star sits left of its title;
            // then the board name, sprint, mode and summary trail it.
            let title_w = iv.title.frame().size.width.min((w * 0.4).max(60.0));
            let pin_w = iv.pin.frame().size.width.max(56.0);
            let mut x = 16.0;
            iv.pin.setFrame(ns(Rect::new(x, (h - 22.0) / 2.0, pin_w, 22.0)));
            x += pin_w + 8.0;
            let th = iv.title.frame().size.height;
            iv.title
                .setFrame(NSRect::new(NSPoint::new(x, (h - th) / 2.0), NSSize::new(title_w, th)));
            x += title_w + 10.0;
            if !iv.sprint.isHidden() {
                let pw = 200.0_f64.min((w * 0.29).max(120.0));
                iv.sprint.setFrame(ns(Rect::new(x, (h - 22.0) / 2.0, pw, 22.0)));
                x += pw + 10.0;
            }
            let mode_w = iv.mode.desired_width().max(1.0);
            iv.mode.setFrame(ns(Rect::new(x, (h - 26.0) / 2.0, mode_w, 26.0)));
            x += mode_w + 10.0;
            let sum_x = x;
            let sum_w = (w - 14.0 - sum_x).max(0.0);
            let (_, sh) = style::text_size("Ag", &style::system_font(11.5, Weight::Regular));
            iv.summary.setFrame(ns(Rect::new(sum_x, (h - sh) / 2.0, sum_w, sh)));
        }
    }

    /// `JiraBoardColumnsView.pageColors(_:)` — the `@key` values the board page
    /// substitutes (hex for solid colors, `rgba()` for the translucent fills).
    pub fn board_page_colors(c: &PopupColors) -> HashMap<String, String> {
        fn css(x: Rgba) -> String {
            format!("#{}", crate::ui::theme::HexColor::format_rgb(&x))
        }
        fn rgba(x: Rgba, a: f64) -> String {
            let c = x.opaque();
            format!(
                "rgba({},{},{},{:.3})",
                (c.r * 255.0).round() as i64,
                (c.g * 255.0).round() as i64,
                (c.b * 255.0).round() as i64,
                a
            )
        }
        let mut m: HashMap<String, String> = HashMap::new();
        m.insert("bg".into(), css(c.background));
        m.insert("text".into(), css(c.text));
        m.insert("dim".into(), css(c.dim));
        m.insert("accent".into(), css(c.accent));
        m.insert("done".into(), css(c.palette.success));
        m.insert("hot".into(), css(c.palette.danger));
        m.insert("col".into(), rgba(c.text, 0.045));
        m.insert("card".into(), rgba(c.text, 0.07));
        m.insert("cardHover".into(), rgba(c.text, 0.11));
        m.insert("line".into(), rgba(c.text, 0.10));
        m
    }

    // ---------------------------------------------------------- container

    pub struct JiraIvars {
        snap: Rc<RefCell<JiraSnapshot>>,
        colors: Cell<PopupColors>,
        sidebar: Retained<SidebarView>,
        search: Retained<NSTextField>,
        search_box: Retained<NSView>,
        /// The `Tickets | Config` pill on the search row.
        tabs: Retained<TabPillView>,
        /// The selected surface (`views.jira.tab`).
        tab: Cell<JiraSurface>,
        /// The Config surface, painted over the list area when selected.
        config: Retained<crate::views::jira_config::JiraConfigBodyView>,
        header: Retained<JiraHeaderView>,
        scroll: Retained<NSScrollView>,
        table: Retained<JiraTableView>,
        inspector: Retained<JiraInspectorView>,
        /// The event sink. Shared (`Rc`) so the board web view's message
        /// handler can emit through it too.
        sink: Rc<RefCell<Option<Sink>>>,
        sidebar_w: Cell<f64>,
        inspector_w: Cell<f64>,
        /// The draggable divider on the sidebar's right edge.
        divider: Retained<PaneDividerView>,
        /// The Cmd+F live-search bar, attached above the list when open.
        search_bar: RefCell<Option<Retained<crate::views::jira_search_bar::JiraSearchBar>>>,
        /// The open board's top bar (`JiraBoardBar`).
        bar: Retained<JiraBoardBarView>,
        /// The board kanban `WKWebView` (created lazily; `JiraBoardColumnsView`).
        board_web: RefCell<Option<crate::ui::web_pane::WebPane>>,
        /// `true` once the board page posted `ready` (its `render` can run).
        board_loaded: Cell<bool>,
        /// The last `render(...)` awaiting the page's `ready`.
        board_pending: RefCell<Option<String>>,
        /// The board page's theme key — a change reloads the HTML.
        board_theme: RefCell<String>,
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
            let font = style::system_font(12.0 * zoom(), Weight::Regular);
            let bold = style::system_font(12.0 * zoom(), Weight::Bold);
            let hfont = style::system_font(12.0 * 0.86 * zoom(), Weight::Bold);
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
        /// Attach (`Some`) or detach (`None`) the live-search bar.
        pub fn set_search_bar(&self, bar: Option<Retained<crate::views::jira_search_bar::JiraSearchBar>>) {
            if let Some(old) = self.ivars().search_bar.borrow_mut().take() {
                old.removeFromSuperview();
            }
            if let Some(b) = &bar {
                self.addSubview(b);
            }
            *self.ivars().search_bar.borrow_mut() = bar;
            self.layout_panes();
        }

        pub fn search_bar(&self) -> Option<Retained<crate::views::jira_search_bar::JiraSearchBar>> {
            self.ivars().search_bar.borrow().clone()
        }

        /// The header view and column `col`'s chevron rect in it (the filter
        /// picker's anchor).
        pub fn filter_anchor(&self, col: usize) -> Option<(Retained<NSView>, Rect)> {
            let h = self.ivars().header.clone();
            let (frames, _) = h.frames();
            let (fx, fw) = *frames.get(col)?;
            let r = filter_rect(fx, fw, h.bounds().size.height);
            Some((Retained::into_super(h), r))
        }

        /// The rail (`sidebarJumpItems` / `do:jira-jump`).
        pub fn sidebar(&self) -> Retained<SidebarView> {
            self.ivars().sidebar.clone()
        }

        /// The draggable divider on the sidebar's right edge.
        pub fn divider(&self) -> Retained<PaneDividerView> {
            self.ivars().divider.clone()
        }

        /// The expanded sidebar width (`[jira] sidebar-width`).
        pub fn sidebar_width(&self) -> f64 {
            self.ivars().sidebar_w.get()
        }

        /// Apply a divider drag / `do:sidebar-width:jira`: clamp and re-lay-out.
        pub fn set_sidebar_width(&self, width: f64) {
            let w = self.ivars().divider.clamp(width);
            self.ivars().sidebar_w.set(w);
            self.layout_panes();
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
        pub fn new(
            mtm: MainThreadMarker,
            colors: PopupColors,
            sidebar_w: f64,
            sidebar_range: DividerRange,
            inspector_w: f64,
        ) -> Retained<Self> {
            let snap = Rc::new(RefCell::new(JiraSnapshot::default()));
            let sidebar = SidebarView::new(mtm, colors);
            sidebar.set_collapse_key("jira");
            let (search_box, search) = crate::ui::field::search_box(mtm, &colors, 13.0, "search...", 6.0, 28.0);
            // The Tickets / Config pill: hand-drawn capsule chips on theme
            // tokens, the same look as the Confluence mode chips. Labels come
            // from `[jira-config]` (rule 3).
            let cfg = crate::views::jira_config::JiraConfigSettings::load();
            let tabs =
                TabPillView::new(mtm, colors, &[cfg.tickets_label.as_str(), cfg.config_label.as_str()]);
            let config = crate::views::jira_config::JiraConfigBodyView::new(mtm, colors);
            config.setHidden(true);
            let header = JiraHeaderView::new(mtm, snap.clone(), colors);
            let scroll = NSScrollView::new(mtm);
            scroll.setHasVerticalScroller(true);
            scroll.setAutohidesScrollers(true);
            scroll.setDrawsBackground(false);
            let table = JiraTableView::new(mtm, snap.clone(), colors);
            scroll.setDocumentView(Some(&table));
            let inspector = JiraInspectorView::new(mtm, snap.clone(), colors);
            let bar = JiraBoardBarView::new(mtm, colors);
            bar.setHidden(true);
            let divider = PaneDividerView::new(mtm, sidebar_range);
            let this = Self::alloc(mtm).set_ivars(JiraIvars {
                snap,
                colors: Cell::new(colors),
                sidebar,
                search,
                search_box,
                tabs,
                tab: Cell::new(JiraSurface::Tickets),
                config,
                header,
                scroll,
                table,
                inspector,
                sink: Rc::new(RefCell::new(None)),
                sidebar_w: Cell::new(sidebar_w),
                inspector_w: Cell::new(inspector_w),
                divider: divider.clone(),
                search_bar: RefCell::new(None),
                bar,
                board_web: RefCell::new(None),
                board_loaded: Cell::new(false),
                board_pending: RefCell::new(None),
                board_theme: RefCell::new(String::new()),
            });
            let v: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
            let iv = v.ivars();
            for sub in [
                &*iv.sidebar as &NSView,
                &iv.search_box,
                &iv.config,
                &iv.header,
                &iv.scroll,
                &iv.inspector,
                &*iv.bar as &NSView,
            ] {
                v.addSubview(sub);
            }
            // The pill on top: its click target must never fall under the
            // sidebar card it shares the column with.
            v.addSubview(&*iv.tabs as &NSView);
            let weak_pill = objc2::rc::Weak::from_retained(&v);
            iv.tabs.set_on_pick(Box::new(move |i| {
                if let Some(view) = weak_pill.load() {
                    let tab = if i == 1 { JiraSurface::Config } else { JiraSurface::Tickets };
                    if view.tab() != tab {
                        view.set_tab(tab);
                        view.emit(JiraEvent::Tab(tab));
                    }
                }
            }));
            // The board kanban `WKWebView` (`JiraBoardColumnsView`): created
            // once, hidden until a board is open in `columns` mode. It posts
            // `ready` / `open:KEY` back through `board_message`.
            let weak = objc2::rc::Weak::from_retained(&v);
            let web = crate::ui::web_pane::WebPane::new(
                mtm,
                "board",
                Box::new(move |msg| {
                    if let Some(ui) = weak.load() {
                        ui.board_message(msg);
                    }
                }),
            );
            let web_view: Retained<NSView> = Retained::into_super(web.web.clone());
            web_view.setHidden(true);
            v.addSubview(&web_view);
            *iv.board_web.borrow_mut() = Some(web);
            // The sidebar divider rides above every pane so it always takes
            // the drag (and never a row click) inside its few-point strip.
            v.addSubview(&divider);
            unsafe { iv.search.setDelegate(Some(ProtocolObject::from_ref(&*v))) };
            v
        }

        pub fn set_on_event(&self, cb: Box<dyn Fn(JiraEvent)>) {
            let rc: Sink = Rc::from(cb);
            *self.ivars().sink.borrow_mut() = Some(rc.clone());
            *self.ivars().table.ivars().sink.borrow_mut() = Some(rc.clone());
            *self.ivars().header.ivars().sink.borrow_mut() = Some(rc.clone());
            *self.ivars().inspector.ivars().sink.borrow_mut() = Some(rc.clone());
            let bar_rc = rc.clone();
            self.ivars().bar.set_on_event(Box::new(move |e| bar_rc(e)));
            let cfg_rc = rc.clone();
            self.ivars()
                .config
                .set_on_action(Box::new(move |a| cfg_rc(JiraEvent::Config(a))));
            self.ivars().sidebar.set_on_event(Box::new(move |e| rc(JiraEvent::Sidebar(e))));
        }

        /// Switch the `Tickets | Config` surface (also the target of
        /// `do:jira-dashboard`'s host-side `set_tab`).
        pub fn set_tab(&self, tab: JiraSurface) {
            self.ivars().tab.set(tab);
            self.ivars()
                .tabs
                .set_selected(if tab == JiraSurface::Config { 1 } else { 0 });
            self.ivars().config.set_active(tab == JiraSurface::Config);
            self.update_board_chrome();
            self.layout_panes();
        }

        /// The selected surface (`views.jira.tab`).
        pub fn tab(&self) -> JiraSurface {
            self.ivars().tab.get()
        }

        /// Hand the host's `jira_poll.py --describe` result to the Config
        /// surface (repainted immediately).
        pub fn set_config_result(&self, res: Result<crate::views::jira::Describe, String>) {
            self.ivars().config.set_result(res);
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

        /// `updateBoardChrome` — show / refresh the board bar and the kanban
        /// web overlay from the snapshot.
        fn update_board_chrome(&self) {
            let iv = self.ivars();
            let colors = iv.colors.get();
            let (bar, columns) = {
                let s = iv.snap.borrow();
                (s.board_bar.clone(), s.board_columns.clone())
            };
            match bar {
                Some(b) => {
                    iv.bar.set_model(&b, colors);
                    iv.bar.setHidden(false);
                }
                None => iv.bar.setHidden(true),
            }
            // The Config tab owns the list area, so the kanban stays down even
            // when the open board is in columns mode.
            match columns {
                Some(cols) if !cols.is_empty() && iv.tab.get() == JiraSurface::Tickets => {
                    self.show_board_columns(&cols)
                }
                _ => self.hide_board_columns(),
            }
        }

        /// Load the board page if the theme changed, then push the columns
        /// (`JiraBoardColumnsView.show`).
        fn show_board_columns(&self, cols: &[BoardColumn]) {
            let iv = self.ivars();
            let colors = iv.colors.get();
            if let Some(web) = iv.board_web.borrow().as_ref() {
                web.web.setHidden(false);
            }
            let theme_key = format!(
                "{:?}|{:?}|{:?}|{:?}|{:?}",
                colors.background, colors.text, colors.dim, colors.accent, colors.highlight
            );
            let json = serde_json::to_string(cols).unwrap_or_else(|_| "[]".into());
            let render = format!("render({json})");
            let reload = *iv.board_theme.borrow() != theme_key;
            if reload {
                *iv.board_theme.borrow_mut() = theme_key;
                iv.board_loaded.set(false);
                let html = crate::views::jira::board_page(&board_page_colors(&colors));
                if let Some(web) = iv.board_web.borrow().as_ref() {
                    web.load_html(&html);
                }
                *iv.board_pending.borrow_mut() = Some(render);
            } else if iv.board_loaded.get() {
                if let Some(web) = iv.board_web.borrow().as_ref() {
                    web.eval(&render);
                }
            } else {
                *iv.board_pending.borrow_mut() = Some(render);
            }
        }

        fn hide_board_columns(&self) {
            let iv = self.ivars();
            if let Some(web) = iv.board_web.borrow().as_ref() {
                web.web.setHidden(true);
            }
        }

        /// The board page's `WKScriptMessageHandler`: `ready` flushes the
        /// pending `render`, `open:KEY` opens that issue.
        fn board_message(&self, msg: String) {
            if let Some(key) = msg.strip_prefix("open:") {
                self.emit(JiraEvent::OpenKey(key.to_string()));
            } else if msg == "ready" {
                let iv = self.ivars();
                iv.board_loaded.set(true);
                let pending = iv.board_pending.borrow_mut().take();
                if let Some(js) = pending {
                    if let Some(web) = iv.board_web.borrow().as_ref() {
                        web.eval(&js);
                    }
                }
            }
        }

        pub fn apply(&self, s: JiraSnapshot) {
            let iv = self.ivars();
            iv.sidebar.set_model(s.sidebar.clone());
            let editing = iv.search.currentEditor().is_some();
            if !editing && iv.search.stringValue().to_string() != s.query {
                iv.search.setStringValue(&NSString::from_str(&s.query));
            }
            let (cols_changed, place_changed) = {
                let old = iv.snap.borrow();
                (old.columns != s.columns, old.place != s.place)
            };
            *iv.snap.borrow_mut() = s;
            self.update_board_chrome();
            self.layout_panes();
            // Snap to the top only when the SHOWN LIST changed. The old
            // "first row key differs" heuristic fired on every poll tick
            // (newest-first publishing), throwing away wherever the user
            // had scrolled — within one list `scroll_selection_visible`
            // below protects the selection without moving the viewport.
            if cols_changed || place_changed {
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
            let (show_inspector, board_bar, board_cols) = {
                let s = iv.snap.borrow();
                (s.show_inspector, s.board_bar.clone(), s.board_columns.clone())
            };
            let board = board_bar.is_some();
            // The kanban covers the list area (Swift's list overlay): the table
            // header, rows and inspector are hidden while it is up.
            let kanban = board && board_cols.is_some();
            // The Config surface likewise owns the list area.
            let config = iv.tab.get() == JiraSurface::Config;
            let show_insp = show_inspector && !kanban && !config;
            let insp = if show_insp { iv.inspector_w.get().min(w * 0.4) } else { 0.0 };
            // The `Tickets | Config` pill heads the sidebar column (like the
            // Confluence scope chips): the surface switch belongs to the nav,
            // not the list's search strip. Fixed band at the top; the sidebar
            // card starts below it. In the collapsed rail there is no room —
            // the palette's Config command still reaches the dashboard, and
            // clicking a ticket from the rail flips back automatically.
            // The pill hugs its buttons (track sized to the chips, like the
            // Confluence segment group) — only a narrow sidebar squeezes it,
            // and then the chips share what's left equally.
            let pill_w = iv.tabs.desired_width().min((sw - 24.0).max(80.0)).max(1.0);
            iv.tabs.setFrame(ns(Rect::new(12.0, 9.0, pill_w, 26.0)));
            iv.tabs.setHidden(iv.sidebar.collapsed());
            iv.sidebar.setFrame(ns(Rect::new(0.0, 44.0, sw, (h - 44.0).max(60.0))));
            iv.divider.place(ns(Rect::new(sw - 4.0, 0.0, 8.0, h)));
            iv.divider.set_width(sw);
            iv.divider.setHidden(iv.sidebar.collapsed());
            let x0 = sw + 4.0;
            let mw = (w - insp - x0).max(100.0);
            // The live-search bar (Cmd+F) sits on top and pushes the list down.
            let bar_h = match iv.search_bar.borrow().as_ref() {
                Some(bar) => {
                    let bh = bar.wanted_height(h);
                    bar.setFrame(ns(Rect::new(x0, 0.0, mw, bh)));
                    bh
                }
                None => 0.0,
            };
            // The board bar (`JiraBoardBar`) sits above the list.
            let board_bar_h = if board { crate::views::jira::BOARD_BAR_HEIGHT } else { 0.0 };
            iv.bar.setFrame(ns(Rect::new(x0, bar_h, mw, board_bar_h)));
            iv.bar.layout_controls();
            let top = bar_h + board_bar_h;
            // The pill now lives in the sidebar column (see above); the
            // search strip owns its whole row.
            let search_w = (mw - 22.0 - 10.0 - 8.0).max(80.0);
            iv.search_box.setFrame(ns(Rect::new(x0 + 22.0, top + 9.0, search_w, 28.0)));
            iv.header.setFrame(ns(Rect::new(x0, top + 50.0, mw, HEADER_H)));
            let ty = top + 50.0 + HEADER_H + 2.0;
            iv.scroll.setFrame(ns(Rect::new(x0, ty, mw, (h - ty).max(0.0))));
            let n = iv.snap.borrow().rows.len();
            let doc_h = (n as f64 * row_h()).max(h - ty);
            iv.table.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(mw, doc_h)));
            // The Config surface replaces the list (header / rows / inspector).
            iv.header.setHidden(kanban || config);
            iv.scroll.setHidden(kanban || config);
            iv.inspector.setHidden(!show_insp);
            iv.inspector.setFrame(ns(Rect::new(w - insp, 0.0, insp, h)));
            iv.config.setHidden(!config);
            if config {
                let list_top = top + 50.0;
                iv.config.setFrame(ns(Rect::new(x0, list_top, mw, (h - list_top).max(0.0))));
            }
            // The kanban fills the list area (below the search field).
            if let Some(web) = iv.board_web.borrow().as_ref() {
                let list_top = top + 50.0;
                web.web.setFrame(NSRect::new(
                    NSPoint::new(x0, list_top),
                    NSSize::new((w - x0).max(1.0), (h - list_top).max(0.0)),
                ));
            }
        }

        fn scroll_selection_visible(&self) {
            let iv = self.ivars();
            let sel = iv.snap.borrow().selection;
            let clip = iv.scroll.contentView();
            let vis = clip.bounds();
            if vis.size.height < row_h() {
                return;
            }
            let (top, bot) = (sel as f64 * row_h(), (sel + 1) as f64 * row_h());
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
        ListColumn { field: field.into(), title: field.into(), width: w, width_px: None, align: "left".into(), sortable: true, filterable: true }
    }

    #[test]
    fn compact_frames_fix_narrow_columns_and_fill_the_rest() {
        let cols = vec![col("key", 8.0), col("title", 33.0), col("status", 10.0), col("priority", 8.0), col("release", 11.0)];
        let f = frames_for(&cols, 0.0, 600.0, true);
        assert_eq!(f[0].1, 76.0);
        assert_eq!(f[2].1, 50.0);
        assert_eq!(f[3].1, 46.0);
        let total: f64 = f.iter().map(|x| x.1).sum();
        assert!((total - 600.0).abs() < 0.01, "{total}");
        assert!((f[1].1 / f[4].1 - 33.0 / 11.0).abs() < 1e-9);
    }

    #[test]
    fn compact_helpers() {
        assert_eq!(initials("Ada Lovelace"), "AL");
        assert_eq!(initials("grace.hopper"), "GH");
        assert_eq!(initials("Ada Lovelace (alove)"), "AL");
        assert_eq!(initials("zed"), "ZE");
        let t = parse_jira_time("2026-10-08T20:14:20.595-0400").unwrap();
        assert_eq!(t, parse_jira_time("2026-10-09T00:14:20Z").unwrap());
        assert_eq!(relative_age(t, t + 3 * 86400), "3d");
        assert_eq!(relative_age(t, t + 90), "1m");
        assert_eq!(relative_age(t, t + 5 * 3600), "5h");
        assert_eq!(priority_glyph("Highest").0, "⇈");
        assert_eq!(priority_glyph("High").0, "↑");
        assert_eq!(priority_glyph("Medium").0, "=");
        assert_eq!(priority_glyph("Lowest").0, "⇊");
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
    fn reserved_frames_keep_the_label_floor() {
        // key is 5% of 400 = 20px, but its header label needs 60px.
        let cols = [col("key", 5.0), col("title", 95.0)];
        let plain = frames_for(&cols, 0.0, 400.0, false);
        assert!((plain[0].1 - 20.0).abs() < 0.5, "{plain:?}");
        let reserved = frames_for_reserved(&cols, 0.0, 400.0, false, &[60.0, 0.0]);
        assert!((reserved[0].1 - 60.0).abs() < 0.5, "{reserved:?}");
        assert!((reserved[0].1 + reserved[1].1 - 400.0).abs() < 1.0, "{reserved:?}");
        // a floor larger than the whole width never overflows it
        let tight = frames_for_reserved(&cols, 0.0, 400.0, false, &[500.0, 0.0]);
        assert!(tight[0].1 <= 401.0, "{tight:?}");
    }

    #[test]
    fn fixed_px_columns_do_not_scale() {
        let mut updated = col("updated", 7.0);
        updated.width_px = Some(111.0);
        let cols = [col("title", 100.0), updated];
        let wide = frames_for(&cols, 0.0, 1000.0, false);
        let narrow = frames_for(&cols, 0.0, 500.0, false);
        assert_eq!(wide[1].1, 111.0, "{wide:?}");
        assert_eq!(narrow[1].1, 111.0, "{narrow:?}");   // fixed: same px at any width
        assert!((wide[0].1 - 889.0).abs() < 0.5, "{wide:?}");
        assert!((narrow[0].1 - 389.0).abs() < 0.5, "{narrow:?}");
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

    fn filter_list() -> JiraList {
        let mut l = JiraList::default();
        l.columns = vec![col("key", 20.0), col("assignee", 20.0), col("status", 20.0), col("labels", 20.0)];
        for (k, a, s, lb) in [
            ("A-1", "ann", "To Do", "ui, bug"),
            ("A-2", "bob", "Done", "bug"),
            ("A-3", "ann", "In Progress", ""),
            ("A-4", "", "To Do", "ui"),
            ("A-5", "cy", "Done", ""),
        ] {
            let mut r = Record::new();
            r.insert("key".into(), k.into());
            r.insert("assignee".into(), a.into());
            r.insert("status".into(), s.into());
            r.insert("labels".into(), lb.into());
            l.all.push(r);
        }
        l.refilter();
        l
    }

    fn shown_keys(l: &JiraList) -> Vec<String> {
        (0..l.rows.len()).map(|i| l.row(i).unwrap()["key"].clone()).collect()
    }

    #[test]
    fn filter_options_count_rows_blank_last() {
        let l = filter_list();
        let a = l.filter_options("assignee");
        let got: Vec<(&str, usize)> = a.iter().map(|o| (o.title.as_str(), o.count)).collect();
        assert_eq!(got, vec![("ann", 2), ("bob", 1), ("cy", 1), ("(unassigned)", 1)]);
        assert_eq!(a[3].id, EMPTY_VALUE);
        // Multi-valued cells count once per value.
        let lb: Vec<(String, usize)> = l.filter_options("labels").into_iter().map(|o| (o.title, o.count)).collect();
        assert_eq!(lb, vec![("bug".into(), 2), ("ui".into(), 2), ("(empty)".into(), 2)]);
    }

    #[test]
    fn column_filters_are_in_per_column_and_across_columns() {
        let mut l = filter_list();
        l.toggle_filter("assignee", "ann");
        l.toggle_filter("assignee", EMPTY_VALUE);
        // assignee IN (ann, unassigned)
        assert_eq!(shown_keys(&l), vec!["A-1", "A-3", "A-4"]);
        l.toggle_filter("status", "To Do");
        // … AND status IN (To Do)
        assert_eq!(shown_keys(&l), vec!["A-1", "A-4"]);
        assert_eq!(l.snapshot().filter_active, BTreeSet::from([1, 2]));
        // A multi-valued cell matches when any of its values is picked.
        l.clear_filter("status");
        l.clear_filter("assignee");
        l.toggle_filter("labels", "ui");
        assert_eq!(shown_keys(&l), vec!["A-1", "A-4"]);
        // Unticking the last value drops the filter.
        l.toggle_filter("labels", "ui");
        assert!(l.col_filters.is_empty());
        assert_eq!(l.rows.len(), 5);
    }

    #[test]
    fn column_filter_drops_when_the_column_goes() {
        let mut l = filter_list();
        l.toggle_filter("status", "Done");
        assert_eq!(l.rows.len(), 2);
        l.columns.retain(|c| c.field != "status");
        l.refilter();
        assert!(l.col_filters.is_empty());
        assert_eq!(l.rows.len(), 5);
    }

    #[test]
    fn group_by_builds_headers_and_collapses() {
        let mut l = filter_list();
        l.group_by = Some("assignee".into());
        l.refilter();
        let heads: Vec<(String, String)> = (0..l.rows.len())
            .filter_map(|i| l.row(i).unwrap().get("__group").map(|g| (g.clone(), l.row(i).unwrap()["__count"].clone())))
            .collect();
        // Most rows first; "Unassigned" sorts last.
        assert_eq!(heads, vec![("ann".into(), "2".into()), ("bob".into(), "1".into()), ("cy".into(), "1".into()), ("Unassigned".into(), "1".into())]);
        assert_eq!(l.rows.len(), 4 + 5);
        // Clicking a header collapses it.
        assert!(l.activate_synthetic(0));
        assert_eq!(l.rows.len(), 4 + 3);
        assert!(l.row(0).unwrap().contains_key("__collapsed"));
        l.toggle_all_groups(true);
        assert_eq!(l.rows.len(), 4);
        l.toggle_all_groups(false);
        assert_eq!(l.rows.len(), 9);
        // A real row is not synthetic.
        assert!(!l.activate_synthetic(1));
        assert_eq!(l.shown_records().len(), 5);
    }

    #[test]
    fn status_category_grouping_uses_fixed_order() {
        let mut l = filter_list();
        l.directory = json!({"statusCategories": {"To Do": "new", "In Progress": "indeterminate", "Done": "done"}});
        l.group_by = Some("statusCategory".into());
        l.refilter();
        let heads: Vec<String> = (0..l.rows.len()).filter_map(|i| l.row(i).unwrap().get("__group").cloned()).collect();
        assert_eq!(heads, vec!["To Do", "In Progress", "Done"]);
    }

    #[test]
    fn page_size_adds_a_load_more_row() {
        let mut l = filter_list();
        l.page_size = 2;
        l.refilter();
        assert_eq!(l.rows.len(), 3);
        assert_eq!(l.row(2).unwrap()["__loadmore"], "load 3 more…");
        assert!(l.activate_synthetic(2));
        assert_eq!(l.rows.len(), 5);
        assert_eq!(l.row(4).unwrap()["__loadmore"], "load 1 more…");
        assert!(l.activate_synthetic(4));
        assert_eq!(l.rows.len(), 5);
        assert!(l.rows.iter().all(|&i| i < l.all.len()));
        // A query shows every match.
        l.visible_offset = 0;
        l.handle(JiraEvent::Query("A-".into()));
        assert_eq!(l.rows.len(), 5);
    }

    #[test]
    fn vim_row_texts_cover_synthetic_rows() {
        // The daemon used to index `all` with the synthetic row indices here
        // (a paged or grouped list + any vim keypress = index-out-of-bounds).
        let mut l = filter_list();
        l.page_size = 2;
        l.refilter();
        assert!(l.rows.iter().any(|&i| i >= l.all.len())); // synth present
        let t = l.vim_row_texts();
        assert_eq!(t.len(), l.rows.len());
        assert_eq!(t[2], "load 3 more…");
        l.group_by = Some("assignee".into());
        l.refilter();
        let t = l.vim_row_texts();
        assert_eq!(t.len(), l.rows.len());
        // A group header reads "name count", never an "A-n" issue row.
        assert!(t.iter().any(|x| x == "ann 2"));
    }

    #[test]
    fn escape_steps_back_out() {
        let mut l = filter_list();
        l.tabs = vec![JiraTab { path: "/x/a.json".into(), file: "a.json".into(), title: "A".into(), columns: l.columns.clone(), badge: None }];
        l.place = Some(JiraPlace::Pin("label:x".into()));
        l.query = "zzz".into();
        assert!(l.escape_step());
        assert!(l.query.is_empty());
        assert!(l.escape_step());
        assert_eq!(l.place, Some(JiraPlace::Tab(0)));
        assert!(!l.escape_step());
    }

    #[test]
    fn stars_and_pins_queue_poller_writes() {
        let mut l = filter_list();
        l.toggle_star(0);
        assert!(l.favs.contains("A-1"));
        let w = l.pending_writes.pop().unwrap();
        assert_eq!(w.args[..3], ["--favorite".to_string(), "add".into(), "A-1".into()]);
        assert!(w.stdin.as_deref().unwrap().contains("\"key\":\"A-1\""));
        l.toggle_star(0);
        assert!(!l.favs.contains("A-1"));
        assert_eq!(l.pending_writes.pop().unwrap().args[1], "remove");
        l.set_pinned_labels(&["ui".into()], true);
        assert_eq!(l.pinned_labels(), vec!["ui"]);
        assert_eq!(l.pending_writes.pop().unwrap().args, vec!["--pin-label", "add", "ui"]);
        l.toggle_pin_board("9");
        assert_eq!(l.pending_writes.pop().unwrap().args, vec!["--pin-board", "add", "9"]);
    }

    #[test]
    fn board_list_star_pins_not_favourites() {
        let mut l = JiraList::default();
        l.catalog = vec![("42".into(), "Demo board".into(), "DEMO".into())];
        l.show_board_list();
        // The ☆ on a board row pins the board (sidebar "Boards"); it never
        // writes the `board:` key into the favourites list.
        l.toggle_star(0);
        assert!(l.boards.iter().any(|(b, _)| b == "42"), "the ☆ stars the board");
        assert!(l.favs.is_empty(), "no bogus favourite");
        assert_eq!(l.pending_writes.last().unwrap().args, vec!["--pin-board".to_string(), "add".into(), "42".into()]);
        assert!(l.snapshot().pinned_boards.contains("42"), "the row ☆ reflects the star");
        l.toggle_star(0);
        assert!(l.boards.iter().all(|(b, _)| b != "42"), "the ☆ unstars");
        assert_eq!(l.pending_writes.last().unwrap().args, vec!["--pin-board".to_string(), "remove".into(), "42".into()]);
    }

    #[test]
    fn starred_boards_are_children_of_all_boards() {
        let mut l = JiraList::default();
        l.catalog = vec![("42".into(), "Demo board".into(), "D".into()), ("7".into(), "Other".into(), "O".into())];
        l.toggle_pin_board("42");
        l.toggle_pin_board("7");
        let pins = l.sidebar_model().pinned;
        let all = pins.iter().find(|p| p.key == BOARD_LIST_KEY).unwrap();
        assert!(all.collapsible && all.expanded, "the parent has an open disclosure");
        let kids: Vec<&str> = pins.iter().filter(|p| p.depth > 0).map(|p| p.key.as_str()).collect();
        assert_eq!(kids, vec![format!("{BOARD_PREFIX}42"), format!("{BOARD_PREFIX}7")]);
        // Collapsing hides the children; the parent stays selectable.
        l.boards_collapsed = true;
        let pins = l.sidebar_model().pinned;
        let all = pins.iter().find(|p| p.key == BOARD_LIST_KEY).unwrap();
        assert!(!all.expanded && pins.iter().all(|p| p.depth == 0), "children hidden");
        // Starring a board while collapsed re-opens the tree so the new star
        // is not hidden away.
        l.toggle_pin_board("42"); // unstar
        l.toggle_pin_board("42"); // re-star
        let pins = l.sidebar_model().pinned;
        assert!(pins.iter().any(|p| p.depth > 0), "a fresh star re-expands");
    }

    #[test]
    fn release_rows_and_urls() {
        let mut r = Record::new();
        r.insert("key".into(), "P-1.2".into());
        r.insert("project".into(), "P".into());
        r.insert("release".into(), "1.2".into());
        r.insert("title".into(), "1.2".into());
        assert!(is_release_row(&r));
        assert_eq!(browse_url(&r, "https://j").unwrap(), "https://j/issues/?jql=project%20%3D%20%22P%22%20AND%20fixVersion%20%3D%20%221.2%22");
        r.insert("versionId".into(), "77".into());
        assert_eq!(browse_url(&r, "https://x.atlassian.net").unwrap(), "https://x.atlassian.net/projects/P/versions/77/tab/release-report-all-issues");
        let mut i = Record::new();
        i.insert("key".into(), "P-9".into());
        assert!(!is_release_row(&i));
        assert_eq!(browse_url(&i, "https://j").unwrap(), "https://j/browse/P-9");
    }

    #[test]
    fn reorder_and_column_save_spec() {
        let mut l = filter_list();
        l.move_column(0, 2);
        let f: Vec<&str> = l.columns.iter().map(|c| c.field.as_str()).collect();
        assert_eq!(f, vec!["assignee", "status", "key", "labels"]);
        l.set_column_widths(&[10.0, 20.0, 30.0, 40.0]);
        // No config.json job owns it: commands.toml's spec comes back.
        let spec = l.save_columns().unwrap();
        assert!(spec.starts_with("[{\"field\":\"assignee\""), "{spec}");
        assert!(spec.contains("\"width\":10"), "{spec}");
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
        // Sprint picks persist across runs (Mechanism 2); start from a clean
        // slate so the "default = current" assertions below hold.
        crate::platform::set_string("jira.boardSprint.42", "");
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
        let mut l = JiraList::new(section.clone(), Some(paths.clone()));

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
        // The sprint picker is REAL now: opening queues a
        // `--board-sprint-keys` fetch (the old `-1` sentinel hid a
        // cosmetic control); no keys landed yet → empty list + pending.
        assert_eq!(st["sprintKeys"], json!([]));
        assert_eq!(st["sprintPending"], json!(true));
        assert!(l
            .pending_runs
            .iter()
            .any(|r| r.kind == PollRunKind::SprintKeys("42".into(), "current".into())));
        assert_eq!(st["catalog"], json!(["42"]));

        // Keys arrive → the lanes AND the row list narrow to them.
        let other = l.all.iter().find(|r| r.get("key").map(String::as_str) == Some("DEMO-1")).cloned();
        assert!(other.is_some(), "fixture row DEMO-1");
        l.apply_sprint_keys("42", "current", Ok(vec!["DEMO-1".into()]));
        assert_eq!(l.board_state()["sprintKeys"], json!(["DEMO-1"]));
        assert!(l
            .rows
            .iter()
            .all(|&i| l.rec(i).and_then(|r| r.get("key").cloned()) == Some("DEMO-1".into())));
        // A failed fetch says so and shows everything (never a fake filter).
        l.pick_sprint("all");
        assert!(l.sprint_filter.is_none());
        l.pick_sprint("current");
        l.apply_sprint_keys("42", "current", Err("401 nope".into()));
        assert!(l.sprint_filter.is_none());
        assert_eq!(l.sprint_error.as_deref(), Some("401 nope"));
        assert!(l.board_summary("42", "current").contains("sprint filter failed"));
        l.pick_sprint("all");
        assert!(!l.board_summary("42", "all").contains("sprint"));

        // columns mode draws the board table (column + key fields).
        let snap = l.snapshot();
        assert!(snap.columns.iter().any(|c| c.field == "column"));
        assert!(snap.rows.iter().any(|r| r.get("key").map(String::as_str) == Some("DEMO-1")));
        // The bar and the kanban overlay data come from the same snapshot.
        assert_eq!(snap.board_bar.as_ref().map(|b| b.board.as_str()), Some("Demo board"));
        assert_eq!(snap.board_bar.as_ref().map(|b| b.mode.as_str()), Some("columns"));
        assert_eq!(snap.board_columns.as_ref().map(Vec::len), Some(2));
        assert_eq!(snap.board_bar.as_ref().map(|b| b.summary.as_str()), Some("2 issues"));

        // Column/status filters apply to the board overlay's cards (lanes stay).
        l.toggle_filter("column", "Doing");
        assert!(l.col_filters.contains_key("column"), "board-column filter kept");
        let snap = l.snapshot();
        assert!(snap.rows.iter().any(|r| r.get("key").map(String::as_str) == Some("DEMO-2")));
        assert!(!snap.rows.iter().any(|r| r.get("key").map(String::as_str) == Some("DEMO-1")));
        assert!(snap.rows.iter().any(|r| r.get("column").map(String::as_str) == Some("Ready")), "lane header stays");
        l.clear_filter("column");

        l.toggle_filter("status", "To Do");
        let snap = l.snapshot();
        assert!(snap.rows.iter().any(|r| r.get("key").map(String::as_str) == Some("DEMO-1")));
        assert!(!snap.rows.iter().any(|r| r.get("key").map(String::as_str) == Some("DEMO-2")));
        l.clear_filter("status");
        assert!(l.col_filters.is_empty());

        // The search box narrows the kanban lanes (`renderBoardColumns`).
        l.handle(JiraEvent::Query("DEMO-1".into()));
        let cards: Vec<&str> = l
            .board_cols
            .iter()
            .flat_map(|c| c.cards.iter())
            .map(|k| k.key.as_str())
            .filter(|k| !k.is_empty())
            .collect();
        assert_eq!(cards, vec!["DEMO-1"]);
        l.handle(JiraEvent::Query(String::new()));
        assert!(l.board_cols.iter().flat_map(|c| c.cards.iter()).any(|k| k.key == "DEMO-2"));

        // table mode draws the issue rows instead.
        assert!(l.set_mode("table"));
        assert_eq!(l.board_state()["mode"], json!("table"));
        assert!(!l.snapshot().columns.iter().any(|c| c.field == "column"));
        assert!(l.snapshot().rows.iter().any(|r| r.get("key").map(String::as_str) == Some("DEMO-1")));
        assert!(l.snapshot().board_bar.is_some(), "the bar stays in table mode");
        assert!(l.snapshot().board_columns.is_none(), "no kanban in table mode");

        // sprint pick + pinned view (in-memory; survives a refresh).
        assert!(l.pick_sprint("7"));
        assert_eq!(l.board_state()["sprint"], json!("7"));
        assert!(l.toggle_pin_view("42|7|table"));
        assert!(l.pinned_views.contains(&"42|7|table".to_string()));
        l.refresh();
        assert!(l.pinned_views.contains(&"42|7|table".to_string()));

        // Mechanism 1: a pick on a pinned board RE-POINTS the "Starred"
        // row instead of leaving the old spec to overwrite the pick the
        // next time the row is clicked (`DEBUG-jira-sprint-switch-revert.md`).
        assert!(l.pick_sprint("current"));
        assert!(l.pinned_views.contains(&"42|current|table".to_string()));
        assert!(!l.pinned_views.iter().any(|v| v.starts_with("42|7|")));
        let w = |a: [&str; 3]| l.pending_writes.iter().any(|x| x.args == a);
        assert!(w(["--pin-view", "add", "42|current|table"]), "re-point writes the new spec");
        assert!(w(["--pin-view", "remove", "42|7|table"]), "…and removes the old one");
        // Clicking the row back lands on the pick — no revert to `current`.
        assert!(l.open_view("42|current|table"));
        assert_eq!(l.board_state()["sprint"], json!("current"));

        // Mechanism 2: the pick persists — a rebuilt list (daemon restart /
        // window re-open) opens on the picked sprint, not back on the
        // latest, and a persisted id the catalog no longer lists is dropped.
        assert!(l.pick_sprint("7"));
        assert_eq!(crate::platform::get_string("jira.boardSprint.42").as_deref(), Some("7"));
        let mut l2 = JiraList::new(section.clone(), Some(paths.clone()));
        assert!(l2.open_board("42"));
        assert_eq!(l2.board_state()["sprint"], json!("7"));
        crate::platform::set_string("jira.boardSprint.42", "999");
        let mut l3 = JiraList::new(section, Some(paths));
        assert!(l3.open_board("42"));
        assert_eq!(l3.board_state()["sprint"], json!("current"));
        crate::platform::set_string("jira.boardSprint.42", "");

        // Mechanism 3: fetched keys are memoized — a re-show re-applies the
        // filter synchronously instead of refetching (no unfiltered flash,
        // no round trip after every re-show).
        l.apply_sprint_keys("42", "7", Ok(vec!["DEMO-1".into()]));
        assert!(l.sprint_keys_memo.contains_key(&("42".to_string(), "7".to_string())));
        let runs_before = l.pending_runs.len();
        l.sprint_filter = None; // what a `show_pin_file` re-show leaves behind
        l.reapply_sprint("42", "7");
        assert!(l.sprint_filter.is_some(), "re-show re-applies the memoized keys");
        assert_eq!(l.pending_runs.len(), runs_before, "a memo hit must not refetch");
        // …and the memo lands SYNCHRONOUSLY through the plain request path
        // too (the live probe caught rows reloading unfiltered because the
        // filter was set without a re-refilter — the flash we are fixing).
        let board_file = boards_dir.join("board-42.json");
        l.show_pin_file(
            &format!("{BOARD_PREFIX}42"),
            &board_file.to_string_lossy(),
            l.default_columns(),
        );
        assert!(l.sprint_filter.is_none(), "show_pin_file resets the filter");
        l.request_sprint("42", "7");
        assert!(l.sprint_filter.is_some(), "the memo filters immediately — no unfiltered window");
        assert_eq!(l.rows.len(), 1, "rows narrow without waiting for the answer");

        // A STALE answer (its `sprint_pending` was reset by a re-show before
        // it landed) must not touch the view — but it is still a fact, so it
        // lands in the memo and the next pick applies it synchronously
        // instead of refetching (the old code dropped it on the floor).
        l.sprint_filter = None;
        l.sprint_pending = None; // what `show_pin_file` leaves behind
        let runs_before = l.pending_runs.len();
        l.apply_sprint_keys("42", "7", Ok(vec!["DEMO-1".into(), "DEMO-2".into()]));
        assert!(l.sprint_filter.is_none(), "a stale answer must not re-filter the view");
        assert_eq!(l.pending_runs.len(), runs_before, "a stale answer must not queue a refetch");
        l.request_sprint("42", "7");
        assert!(
            l.sprint_filter
                .as_ref()
                .is_some_and(|(_, _, keys)| keys.len() == 2),
            "the stale answer was memoized — the next pick applies it synchronously"
        );

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

    #[test]
    fn sprint_day_formats_iso_and_rejects_junk() {
        assert_eq!(sprint_day("2026-10-01T00:00:00.000Z"), Some("Oct 1".into()));
        assert_eq!(sprint_day("2026-10-14"), Some("Oct 14".into()));
        assert_eq!(sprint_day("2026-1-2"), None);
        assert_eq!(sprint_day("2026-13-01"), None);
        assert_eq!(sprint_day("nope"), None);
    }

    #[test]
    fn board_bar_choices_and_summary() {
        let base = std::env::temp_dir().join(format!("ws-jira-bar-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let cache = base.join("cache");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(
            cache.join("board_catalog.json"),
            json!({"projects": [{"key": "DEMO", "boards": [{
                "id": "42", "name": "Demo board", "type": "scrum",
                "sprints": [
                    {"id": "7", "name": "Sprint 7", "state": "active",
                     "startDate": "2026-10-01", "endDate": "2026-10-14"},
                    {"id": "8", "name": "Sprint 8", "state": "future"},
                    {"id": "6", "name": "Sprint 6", "state": "closed", "completeDate": "2026-09-30"}
                ]
            }]}]})
            .to_string(),
        )
        .unwrap();
        std::fs::write(cache.join("boards.json"), json!({}).to_string()).unwrap();
        std::fs::write(
            base.join("config.json"),
            json!({"pinnedBoards": [], "pinnedBoardViews": []}).to_string(),
        )
        .unwrap();
        let paths = JiraPaths {
            config_json: base.join("config.json").to_string_lossy().into_owned(),
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
            side_dirs: BTreeMap::new(),
        };
        let l = JiraList::new(HashMap::new(), Some(paths));

        // The catalog sprints carry their start / end / complete dates.
        let sprints = l.board_sprints("42");
        assert_eq!(sprints[0].start, "2026-10-01");
        assert_eq!(sprints[2].complete, "2026-09-30");

        // current, next, [sep, Past sprints, closed], [sep, All issues].
        let choices = l.board_sprint_choices("42");
        let ids: Vec<&str> =
            choices.iter().filter(|c| !c.header).map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["current", "8", "6", "all"]);
        assert_eq!(choices[0].title, "Sprint 7 (current)");
        assert_eq!(choices[1].title, "Sprint 8 (next)");
        assert_eq!(l.sprint_title("42", "8"), "Sprint 8");
        assert_eq!(l.sprint_title("42", "all"), "All issues");

        // The summary counts shown issues and dates the sprint.
        assert_eq!(l.board_summary("42", "current"), "0 issues · Oct 1 – Oct 14");
        assert_eq!(l.board_summary("42", "6"), "0 issues · ended Sep 30");

        let _ = std::fs::remove_dir_all(&base);
    }
}
