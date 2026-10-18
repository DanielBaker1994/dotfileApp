//! The Jira **Config** surface: the poll-jobs dashboard the Swift
//! `JiraDashboardWindow` showed, now the `Config` half of the Jira view's
//! `Tickets | Config` pill (`views/jira_view.rs`) — no separate window.
//!
//! The pure, config-driven model lives here ([`JiraConfigSettings`],
//! [`job_rows`], [`next_poll`], [`status_line`], [`file_paths`]); the AppKit
//! half ([`appkit::JiraConfigBodyView`]) paints the status line, every poll
//! job with its interval + last run, the config / status / log paths and the
//! Enable / Disable, Force Poll, Stop Poll, Refresh controls, in place
//! of the ticket list — plus the embedded **Connection / Setup** form
//! ([`crate::ui::jira_setup::JiraSetupFormView`], in place of the old separate
//! `JiraSetupWindow`).
//!
//! Data comes from `jira_poll.py --describe` (no network, no writes) but the
//! fetch is owned by the host (`app/host.rs`), which hands the parse back
//! through `JiraView::set_config_result`; the body only renders and reports
//! control presses as [`JiraConfigAction`]s. All user-facing strings and the
//! auto-refresh interval come from `commands.toml [jira-config]` (rule 3).

use std::collections::HashMap;
use std::sync::Mutex;

use serde_json::{json, Value};

use crate::app::registry::Registry;
use crate::views::jira::{Describe, DescribeJob};

// ===========================================================================
// Pure config + view model (AppKit-free, unit-tested)
// ===========================================================================

/// A Config-tab control press, reported to the host as `JiraEvent::Config`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JiraConfigAction {
    /// Enable / Disable Jira (`jira-poll-on` / `jira-poll-off`).
    Enable,
    /// Poll every enabled job now (`jira_poll.py --force`).
    Force,
    /// Stop the running poll (`jira_poll.py --cancel`).
    Stop,
    /// The embedded Connection / Setup form saved & enabled the connection:
    /// turn polling on (`setJiraEnabled(true)`).
    SetupSaved,
    /// Re-read `jira_poll.py --describe`.
    Refresh,
}

/// `[jira-config]` — the surface's user-facing strings and the auto-refresh
/// interval, read back from `commands.toml` (rule 3).
#[derive(Clone, Debug, PartialEq)]
pub struct JiraConfigSettings {
    /// The pill's two segment labels.
    pub tickets_label: String,
    pub config_label: String,
    /// Auto-refresh period in seconds while the Config tab is selected.
    pub refresh: f64,
    pub enable_label: String,
    pub disable_label: String,
    pub force_label: String,
    pub stop_label: String,
    pub refresh_label: String,
    /// The Config body's section headings + loading / empty lines.
    pub jobs_title: String,
    /// The jobs table's column headers (`[jira-config] job-col-*`).
    pub job_col_name_label: String,
    pub job_col_frequency_label: String,
    pub job_col_last_run_label: String,
    pub job_col_status_label: String,
    pub files_title: String,
    pub loading: String,
    pub loading_jobs: String,
    pub no_jobs: String,
    pub loading_paths: String,
    /// The per-job pane (`[jira-config] job-*`): the right-hand detail's
    /// labels, buttons and result lines.
    pub job_detail_title: String,
    pub job_select_hint: String,
    pub job_schedule_label: String,
    pub job_schedule_placeholder: String,
    pub job_enabled_label: String,
    pub job_jql_label: String,
    pub job_jql_placeholder: String,
    pub job_args_label: String,
    pub job_args_placeholder: String,
    pub job_columns_label: String,
    pub job_columns_placeholder: String,
    pub job_force_label: String,
    pub job_save_label: String,
    pub job_columns_button_label: String,
    pub job_delete_label: String,
    pub job_saved: String,
    pub job_deleted: String,
    pub job_polled: String,
    pub job_running: String,
    pub job_failed: String,
}

impl Default for JiraConfigSettings {
    fn default() -> Self {
        JiraConfigSettings {
            tickets_label: "Tickets".to_string(),
            config_label: "Config".to_string(),
            refresh: 5.0,
            enable_label: "Enable Jira".to_string(),
            disable_label: "Disable Jira".to_string(),
            force_label: "Force Poll".to_string(),
            stop_label: "Stop Poll".to_string(),
            refresh_label: "Refresh".to_string(),
            jobs_title: "Poll jobs".to_string(),
            job_col_name_label: "Job name".to_string(),
            job_col_frequency_label: "Frequency".to_string(),
            job_col_last_run_label: "Last run".to_string(),
            job_col_status_label: "Last status".to_string(),
            files_title: "Files".to_string(),
            loading: "Loading…".to_string(),
            loading_jobs: "Loading poll jobs…".to_string(),
            no_jobs: "No poll jobs configured — set the connection above.".to_string(),
            loading_paths: "Loading paths…".to_string(),
            job_detail_title: "Job".to_string(),
            job_select_hint: "Pick a job above to edit it.".to_string(),
            job_schedule_label: "Schedule".to_string(),
            job_schedule_placeholder: "e.g. 10m, 1h, 1d".to_string(),
            job_enabled_label: "Enabled".to_string(),
            job_jql_label: "JQL".to_string(),
            job_jql_placeholder: "extra JQL (blank = shared sync)".to_string(),
            job_args_label: "Args".to_string(),
            job_args_placeholder: "key=value, key=value".to_string(),
            job_columns_label: "Columns".to_string(),
            job_columns_placeholder: "field:Title:width, … (or JSON)".to_string(),
            job_force_label: "Poll Now".to_string(),
            job_save_label: "Save".to_string(),
            job_columns_button_label: "Set Columns".to_string(),
            job_delete_label: "Delete".to_string(),
            job_saved: "✓ saved {name}".to_string(),
            job_deleted: "✓ deleted {name}".to_string(),
            job_polled: "✓ polled {name}".to_string(),
            job_running: "Running {name}…".to_string(),
            job_failed: "✗ {error}".to_string(),
        }
    }
}

impl JiraConfigSettings {
    /// `JiraConfigSettings.load()` — reads `[jira-config]`; defaults on failure.
    pub fn load() -> JiraConfigSettings {
        match crate::app::config::read_config_text() {
            Some(text) => JiraConfigSettings::from_text(&text),
            None => JiraConfigSettings::default(),
        }
    }

    pub fn from_text(text: &str) -> JiraConfigSettings {
        JiraConfigSettings::from_section(&parse_section(text, "jira-config"))
    }

    /// The pure key parsing (`load()` without the file read).
    pub fn from_section(e: &HashMap<String, String>) -> JiraConfigSettings {
        let d = JiraConfigSettings::default();
        let num = |k: &str, dv: f64| {
            e.get(k)
                .and_then(|v| v.trim().parse::<f64>().ok())
                .filter(|n| n.is_finite() && *n > 0.0)
                .unwrap_or(dv)
        };
        let s = |k: &str, dv: &str| match e.get(k) {
            Some(v) if !v.trim().is_empty() => v.trim().to_string(),
            _ => dv.to_string(),
        };
        JiraConfigSettings {
            tickets_label: s("tickets-label", &d.tickets_label),
            config_label: s("config-label", &d.config_label),
            refresh: num("dashboard-refresh", d.refresh),
            enable_label: s("enable-label", &d.enable_label),
            disable_label: s("disable-label", &d.disable_label),
            force_label: s("force-poll-label", &d.force_label),
            stop_label: s("stop-poll-label", &d.stop_label),
            refresh_label: s("refresh-button-label", &d.refresh_label),
            jobs_title: s("poll-jobs-title", &d.jobs_title),
            job_col_name_label: s("job-col-name-label", &d.job_col_name_label),
            job_col_frequency_label: s("job-col-frequency-label", &d.job_col_frequency_label),
            job_col_last_run_label: s("job-col-last-run-label", &d.job_col_last_run_label),
            job_col_status_label: s("job-col-status-label", &d.job_col_status_label),
            files_title: s("files-title", &d.files_title),
            loading: s("loading", &d.loading),
            loading_jobs: s("loading-jobs", &d.loading_jobs),
            no_jobs: s("no-jobs", &d.no_jobs),
            loading_paths: s("loading-paths", &d.loading_paths),
            job_detail_title: s("job-detail-title", &d.job_detail_title),
            job_select_hint: s("job-select-hint", &d.job_select_hint),
            job_schedule_label: s("job-schedule-label", &d.job_schedule_label),
            job_schedule_placeholder: s("job-schedule-placeholder", &d.job_schedule_placeholder),
            job_enabled_label: s("job-enabled-label", &d.job_enabled_label),
            job_jql_label: s("job-jql-label", &d.job_jql_label),
            job_jql_placeholder: s("job-jql-placeholder", &d.job_jql_placeholder),
            job_args_label: s("job-args-label", &d.job_args_label),
            job_args_placeholder: s("job-args-placeholder", &d.job_args_placeholder),
            job_columns_label: s("job-columns-label", &d.job_columns_label),
            job_columns_placeholder: s("job-columns-placeholder", &d.job_columns_placeholder),
            job_force_label: s("job-force-label", &d.job_force_label),
            job_save_label: s("job-save-label", &d.job_save_label),
            job_columns_button_label: s("job-columns-button-label", &d.job_columns_button_label),
            job_delete_label: s("job-delete-label", &d.job_delete_label),
            job_saved: s("job-saved", &d.job_saved),
            job_deleted: s("job-deleted", &d.job_deleted),
            job_polled: s("job-polled", &d.job_polled),
            job_running: s("job-running", &d.job_running),
            job_failed: s("job-failed", &d.job_failed),
        }
    }
}

/// A minimal pure `[section]` reader (mirrors the app's config-codec shape
/// for a handful of keys; no helper round-trip).
pub fn parse_section(text: &str, section: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut cur = String::new();
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if t.starts_with('[') && t.ends_with(']') {
            cur = t[1..t.len() - 1].trim().to_string();
            continue;
        }
        if cur != section {
            continue;
        }
        if let Some((k, v)) = t.split_once('=') {
            let v = v.split(" #").next().unwrap_or(v);
            out.insert(k.trim().to_string(), v.trim().trim_matches('"').to_string());
        }
    }
    out
}

/// One poll job as the surface shows it (name + interval + last run + next).
#[derive(Clone, Debug, PartialEq)]
pub struct JobRow {
    pub name: String,
    pub interval: String,
    pub last_run: String,
    pub next_run: String,
    pub status: String,
    pub enabled: bool,
}

/// The `Describe.endpoints` as display rows (the required core of the surface).
pub fn job_rows(d: &Describe) -> Vec<JobRow> {
    d.endpoints.iter().map(job_row).collect()
}

fn job_row(j: &DescribeJob) -> JobRow {
    let blank = |s: &str, dv: &str| {
        if s.trim().is_empty() {
            dv.to_string()
        } else {
            s.trim().to_string()
        }
    };
    JobRow {
        name: j.name.clone(),
        interval: blank(&j.window, "?"),
        last_run: blank(&j.last_run, "never"),
        next_run: blank(&j.next_run, "due"),
        status: blank(&j.status, "never run"),
        enabled: j.enabled,
    }
}

/// The row's four table cells, in column order: job name, frequency, last
/// run (relative to `now`, e.g. "6m ago"; the raw value passes through for
/// non-timestamps like `never`), last status ("ok (off)" when disabled).
/// The jobs list draws these as evenly spaced columns under a header row
/// (`[jira-config] job-col-*-label`).
pub fn job_row_cells(r: &JobRow, now: i64) -> [String; 4] {
    let last = fmt_poll_time(&r.last_run, now).unwrap_or_else(|| r.last_run.clone());
    let status = if r.enabled {
        r.status.clone()
    } else {
        format!("{} (off)", r.status)
    };
    [r.name.clone(), format!("every {}", r.interval), last, status]
}

/// A row's status tone (the `●` bullet + the status word): the same
/// traffic-light language as the sidebar's sync dot — green ok, red failed,
/// amber running/cancelled, dim off/never-run. Names per `color(_:)`.
pub fn job_row_tone(status: &str, enabled: bool) -> &'static str {
    if !enabled {
        return "dim";
    }
    let s = status.trim().to_ascii_lowercase();
    if s.contains("error") || s.contains("fail") {
        "err"
    } else if s.contains("running") || s.contains("cancel") || s.contains("stale") || s.contains("locked") {
        "warn"
    } else if s.starts_with("ok") {
        "ok"
    } else {
        "dim"
    }
}

/// The soonest next run across the enabled jobs (the poll payload already
/// formats each `nextRun`; `"due"` wins).
pub fn next_poll(d: &Describe) -> String {
    let mut best: Option<&str> = None;
    for j in d.endpoints.iter().filter(|j| j.enabled) {
        let n = j.next_run.trim();
        if n.is_empty() || n == "due" {
            return "due".to_string();
        }
        best = Some(match best {
            None => n,
            Some(b) => {
                if n < b {
                    n
                } else {
                    b
                }
            }
        });
    }
    best.map(str::to_string).unwrap_or_else(|| "—".to_string())
}

/// Seconds since the epoch, for the relative-time formatters below.
pub fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Parse `jira_status.TIME_FMT` ("2026-10-16 09:41:45", wall-clock local) to
/// epoch seconds. `None` on anything else (empty, `never`, `due`, `—`).
fn parse_poll_ts(ts: &str) -> Option<i64> {
    let (date, time) = ts.trim().split_once(' ')?;
    let d: Vec<i32> = date.split('-').filter_map(|p| p.parse().ok()).collect();
    let t: Vec<i32> = time.split(':').filter_map(|p| p.parse().ok()).collect();
    if d.len() != 3 || t.len() != 3 {
        return None;
    }
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = d[0] - 1900;
    tm.tm_mon = d[1] - 1;
    tm.tm_mday = d[2];
    tm.tm_hour = t[0];
    tm.tm_min = t[1];
    tm.tm_sec = t[2];
    tm.tm_isdst = -1;
    let secs = unsafe { libc::mktime(&mut tm) };
    if secs == -1 {
        None
    } else {
        Some(secs as i64)
    }
}

/// A span of seconds as a compact unit: `41s` / `9m` / `5h` / `2d`.
fn human_span(secs: i64) -> String {
    let s = secs.abs();
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

/// A poll timestamp as relative text against `now`: past → "3m ago",
/// future → "in 9m"; `None` when it is not a timestamp (the caller keeps
/// the raw value, e.g. `due`).
pub fn fmt_poll_time(ts: &str, now: i64) -> Option<String> {
    let at = parse_poll_ts(ts)?;
    let d = at - now;
    Some(if d <= 0 {
        format!("{} ago", human_span(d))
    } else {
        format!("in {}", human_span(d))
    })
}

/// The status line + a tone key (`ok` / `dim` / `warn` / `err`): polling
/// on/off with relative times ("last run 41s ago · next in 9m"), and the
/// same traffic light as the job rows.
pub fn status_line(
    d: &Describe,
    enabled: bool,
    poll_active: bool,
    lock_held: bool,
    now: i64,
) -> (String, &'static str) {
    if !enabled {
        return if poll_active {
            ("◐ Polling in the background (Jira window off)".to_string(), "dim")
        } else {
            ("○ Polling off".to_string(), "dim")
        };
    }
    let last_raw = d.last_run.trim();
    let last = if last_raw.is_empty() {
        "never".to_string()
    } else {
        fmt_poll_time(last_raw, now).unwrap_or_else(|| last_raw.to_string())
    };
    let next_raw = next_poll(d);
    // `due` / `—` are not timestamps; they pass through.
    let next = fmt_poll_time(&next_raw, now).unwrap_or(next_raw);
    let head = if lock_held {
        "● Polling now…"
    } else {
        "● Polling on"
    };
    // Traffic light like the job rows: amber while a poll runs, red when an
    // enabled job's last run failed, green when the sync is healthy.
    let tone = if lock_held {
        "warn"
    } else if d
        .endpoints
        .iter()
        .any(|j| job_row_tone(&j.status, j.enabled) == "err")
    {
        "err"
    } else {
        "ok"
    };
    (format!("{head} — last run {last} · next {next}"), tone)
}

/// The config / status / cache file paths the surface lists (existing files
/// first is the caller's concern; empty paths are dropped here).
pub fn file_paths(d: &Describe) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = vec![
        ("Jira config".to_string(), d.config_path.clone()),
        ("Team schema".to_string(), d.team_path.clone()),
        ("Poll status".to_string(), d.status_path.clone()),
        ("Directory cache".to_string(), d.directory.path.clone()),
        ("Poll log".to_string(), d.poll_log.clone()),
        ("Debug log".to_string(), d.debug_log.clone()),
    ];
    out.retain(|(_, p)| !p.trim().is_empty());
    out
}

// ===========================================================================
// Per-job detail model (the right-hand pane) + headless hooks
// ===========================================================================

/// The editable detail of one poll job — the right-hand pane's field values.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JobDetail {
    pub name: String,
    pub window: String,
    pub enabled: bool,
    pub jql: String,
    pub args: String,
    pub columns: String,
}

/// A [`DescribeJob`] as the detail pane's starting values.
pub fn job_detail(j: &DescribeJob) -> JobDetail {
    JobDetail {
        name: j.name.clone(),
        window: j.window.clone(),
        enabled: j.enabled,
        // The editable "extra JQL" only: a plain (shared-sync) job has an
        // empty `extra_jql` while `j.jql` is the *generated* sync query (with
        // ORDER BY). Falling back to `j.jql` would send that generated query
        // through `--upsert-endpoint` and silently turn the plain job into a
        // standalone-JQL job.
        jql: j.extra_jql.clone(),
        args: args_text(&j.args),
        columns: j.columns_spec.clone(),
    }
}

/// The selected job's detail from a describe payload (`None` = no such job).
pub fn detail_for(d: &Describe, name: &str) -> Option<JobDetail> {
    d.endpoints.iter().find(|j| j.name == name).map(job_detail)
}

/// An endpoint `args` object -> the `key=value, key=value` text the pane edits.
pub fn args_text(args: &Value) -> String {
    args.as_object()
        .map(|o| {
            o.iter()
                .map(|(k, v)| format!("{k}={}", scalar_text(v)))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}

/// A JSON scalar as the plain text the args field shows.
fn scalar_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

/// The `--upsert-endpoint` payload for a detail: its JQL / args. The schedule
/// and enabled flag are the dedicated `--set-window` / `--set-enabled` verbs;
/// columns are written by `--set-columns` (upsert keeps the endpoint's
/// existing keys it does not receive).
pub fn upsert_payload(d: &JobDetail) -> Value {
    json!({
        "name": d.name,
        "jql": d.jql.trim(),
        "args": crate::views::jira::parse_args(&d.args),
    })
}

/// The Config surface's selected poll job, shared by the AppKit body and the
/// headless `do:jira-config:*` hooks (the socket thread reaches it without a
/// view).
static CONFIG_UI: Mutex<Option<String>> = Mutex::new(None);

/// The job the Config surface has selected (`None` = none yet).
pub fn selected_job() -> Option<String> {
    CONFIG_UI.lock().unwrap().clone()
}

/// Select a job (the list click / `do:jira-config:job:<name>`).
pub fn select_job(name: &str) {
    *CONFIG_UI.lock().unwrap() = Some(name.to_string());
}

/// Forgets the selection (after the selected job was deleted).
pub fn clear_job() {
    *CONFIG_UI.lock().unwrap() = None;
}

/// The pure half of the `do:jira-config:*` hooks (`None` = not ours).
pub fn config_do(action: &str) -> Option<Value> {
    let rest = action.strip_prefix("jira-config:")?;
    if rest == "state" {
        return Some(json!({ "job": selected_job() }));
    }
    if let Some(name) = rest.strip_prefix("job:") {
        let name = name.trim();
        if name.is_empty() {
            return Some(json!({ "error": "jira-config:job:<name>" }));
        }
        select_job(name);
        return Some(json!({ "job": name }));
    }
    None
}

/// Register the per-job Config hooks into the shared [`Registry`]
/// (`do:jira-config:job:<name>` / `do:jira-config:state`).
pub fn register(registry: &mut Registry) {
    registry.register_test_do(|action| config_do(action));
}

// ===========================================================================
// AppKit body (drawn into the Jira view's list area)
// ===========================================================================

#[cfg(target_os = "macos")]
pub use appkit::JiraConfigBodyView;

#[cfg(target_os = "macos")]
mod appkit {
    use super::*;
    use crate::ui::chrome;
    use crate::ui::jira_setup::{JiraSetupFormView, SetupOutcome};
    use crate::ui::outline_button::{OutlineButton, Tone};
    use crate::ui::style::{self, Weight};
    use crate::ui::theme::{PopupColors, PopupTone, Rgba};
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSAutoresizingMaskOptions, NSEvent, NSFont, NSLineBreakMode, NSScrollView, NSTextAlignment,
        NSTextField, NSTrackingArea, NSView,
    };
    use objc2_foundation::{
        NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSTimer,
    };
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    const PAD: f64 = 16.0;
    const ROW_H: f64 = 22.0;
    /// The jobs table's header band (column names above the evenly spaced
    /// columns); rows are laid out below it.
    const HEADER_H: f64 = 18.0;
    const TITLE_H: f64 = 20.0;
    const LINE_H: f64 = 16.0;
    /// The per-job detail pane's row grid (the pane spans the document
    /// width now; the job list sits above it, not beside it).
    const D_ROW: f64 = 30.0;
    const D_LABEL_W: f64 = 78.0;
    const D_FIELD_H: f64 = 24.0;
    const D_BUTTON_H: f64 = 26.0;
    const D_RESULT_H: f64 = 30.0;
    /// The fixed detail-pane height (heading + 5 rows + buttons + result) the
    /// two-column region reserves.
    const DETAIL_H: f64 =
        26.0 + 5.0 * D_ROW + D_BUTTON_H + 8.0 + D_RESULT_H;

    /// The event sink: one [`JiraConfigAction`] per control press.
    pub type ActionSink = Rc<dyn Fn(JiraConfigAction)>;

    /// One background writer's result: `Ok((message, clear_selection))` or the
    /// error line to show in red.
    type WriteResult = Result<(String, bool), String>;

    fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
        NSRect::new(NSPoint::new(x, y), NSSize::new(w.max(1.0), h.max(1.0)))
    }

    use crate::ui::view_base::{self, as_any};

    /// `{name}` → value for a `[jira-config]` format string.
    fn fill(template: &str, name: &str) -> String {
        template.replace("{name}", name)
    }

    /// `{error}` → value for the `[jira-config] job-failed` template.
    fn fill_error(template: &str, err: &str) -> String {
        template.replace("{error}", err)
    }

    /// `--upsert-endpoint` / `--set-columns` / `--delete-endpoint` print JSON
    /// `{ok, problems}` on failure; anything else falls back to stderr.
    fn writer_error(out: &str, err: &str, code: i32) -> String {
        if let Ok(v) = serde_json::from_str::<Value>(out.trim()) {
            if v.get("ok").and_then(Value::as_bool) == Some(false) {
                let probs: Vec<String> = v
                    .get("problems")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
                    .unwrap_or_default();
                if !probs.is_empty() {
                    return probs.join("; ");
                }
            }
        }
        crate::app::jira_poll::error_line(err, &format!("exit {code}"))
    }

    thread_local! {
        /// The live body, so a background writer can hop back on main (the
        /// objc2 handle is not `Send`; only the main thread touches it).
        static LIVE_BODY: RefCell<Option<objc2::rc::Weak<JiraConfigBodyView>>> =
            const { RefCell::new(None) };
    }

    /// Run `job` off the main thread, then `then` on main with the live body
    /// (skipped if it went away meanwhile) — the setup form's pattern.
    fn background<R: Send + 'static>(
        job: impl FnOnce() -> R + Send + 'static,
        then: impl FnOnce(&JiraConfigBodyView, R) + Send + 'static,
    ) {
        std::thread::spawn(move || {
            let r = job();
            crate::platform::run_on_main(move || {
                if let Some(b) = LIVE_BODY.with(|l| l.borrow().as_ref().and_then(|w| w.load())) {
                    then(&b, r);
                }
            });
        });
    }

    /// `WSJiraConfigFlippedView` — a top-left-origin document view (paths.rs).
    pub struct FlippedIvars;

    define_class!(
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSJiraConfigFlippedView"]
        #[ivars = FlippedIvars]
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
        fn new(mtm: MainThreadMarker) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(FlippedIvars);
            unsafe {
                objc2::msg_send![super(this), initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1.0, 1.0))]
            }
        }
    }

    /// One row of the jobs table: bullet + the four cells from
    /// [`job_row_cells`] (name / frequency / last run / status) drawn in
    /// evenly spaced columns under the header, in the traffic-light language
    /// of the sidebar's sync dot.
    #[derive(Clone)]
    pub struct JobRowItem {
        pub cells: [String; 4],
        pub tone: Rgba,
    }

    pub struct JobListIvars {
        rows: RefCell<Vec<JobRowItem>>,
        headers: RefCell<[String; 4]>,
        selected: Cell<Option<usize>>,
        hover: Cell<Option<usize>>,
        pressed: Cell<Option<usize>>,
        colors: Cell<PopupColors>,
        on_select: RefCell<Option<Rc<dyn Fn(usize)>>>,
        tracking: RefCell<Option<Retained<NSTrackingArea>>>,
    }

    // The themed jobs list — one exactly-one-of row per poll job. Replaces
    // the per-row `NSButton` `PushOnPushOff` bezels (system chrome the
    // `[theme]` cannot reach; see `pill_selector.rs` for the same ban).
    define_class!(
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSJobRowList"]
        #[ivars = JobListIvars]
        pub struct JobRowListView;

        impl JobRowListView {
            #[unsafe(method(isFlipped))]
            fn is_flipped(&self) -> bool {
                true
            }

            #[unsafe(method(drawRect:))]
            fn draw_rect(&self, _r: NSRect) {
                let iv = self.ivars();
                let c = iv.colors.get();
                let b = self.bounds();
                let rows = iv.rows.borrow();
                let f = style::system_font(11.5, Weight::Regular);
                // The table grid: after the bullet, four evenly spaced
                // columns spanning the row pill; header and rows share it.
                let grid = chrome::Rect::new(0.0, 0.0, b.size.width, ROW_H).inset_by(4.0, 1.0);
                let col_x = grid.x + 6.0 + 12.0 + 6.0;
                let col_w = ((grid.max_x() - 6.0 - col_x) / 4.0).max(40.0);
                // Header row: the column names (job-col-*), dim + a hairline.
                let headers = iv.headers.borrow();
                let hf = style::system_font(10.0, Weight::Semibold);
                for (i, h) in headers.iter().enumerate() {
                    if h.is_empty() {
                        continue;
                    }
                    style::draw_text_left(
                        &h.to_uppercase(),
                        chrome::Rect::new(col_x + i as f64 * col_w, 0.0, col_w, HEADER_H),
                        &hf,
                        c.dim,
                    );
                }
                style::fill_rect(
                    chrome::Rect::new(col_x, HEADER_H - 1.0, (grid.max_x() - 6.0 - col_x).max(0.0), 1.0),
                    c.dim.with_alpha(0.35),
                );
                for (idx, row) in rows.iter().enumerate() {
                    let band = chrome::Rect::new(
                        0.0,
                        HEADER_H + idx as f64 * (ROW_H + 2.0),
                        b.size.width,
                        ROW_H,
                    );
                    let pill = band.inset_by(4.0, 1.0);
                    let sel = iv.selected.get() == Some(idx);
                    let hov = iv.pressed.get() == Some(idx) || (!sel && iv.hover.get() == Some(idx));
                    if sel {
                        style::fill_rounded(pill, 4.0, c.highlight);
                        style::fill_rect(
                            chrome::Rect::new(pill.x, pill.y + 3.0, 3.0, pill.height - 6.0),
                            c.accent_on(),
                        );
                    } else if hov {
                        style::fill_rounded(
                            pill,
                            4.0,
                            style::button_fill(style::ButtonState::Hover, &c),
                        );
                    }
                    style::draw_text_left(
                        "\u{25cf}",
                        chrome::Rect::new(pill.x + 6.0, band.y, 12.0, band.height),
                        &f,
                        row.tone,
                    );
                    for (i, cell) in row.cells.iter().enumerate() {
                        let color = if i == 3 { row.tone } else { c.text };
                        style::draw_text_left(
                            cell,
                            chrome::Rect::new(
                                col_x + i as f64 * col_w,
                                band.y,
                                col_w,
                                band.height,
                            ),
                            &f,
                            color,
                        );
                    }
                }
            }

            #[unsafe(method(mouseDown:))]
            fn mouse_down(&self, e: &NSEvent) {
                let p = view_base::mouse_point(self, e);
                self.ivars().pressed.set(self.row_at(p));
                self.setNeedsDisplay(true);
            }

            #[unsafe(method(mouseUp:))]
            fn mouse_up(&self, e: &NSEvent) {
                let p = view_base::mouse_point(self, e);
                let was = self.ivars().pressed.replace(None);
                self.setNeedsDisplay(true);
                if was.is_some() && was == self.row_at(p) {
                    if let Some(cb) = self.ivars().on_select.borrow().as_ref() {
                        cb(was.unwrap());
                    }
                }
            }

            #[unsafe(method(mouseEntered:))]
            fn mouse_entered(&self, e: &NSEvent) {
                self.track(e);
            }

            #[unsafe(method(mouseMoved:))]
            fn mouse_moved(&self, e: &NSEvent) {
                self.track(e);
            }

            #[unsafe(method(mouseExited:))]
            fn mouse_exited(&self, _e: &NSEvent) {
                if self.ivars().hover.get().is_some() {
                    self.ivars().hover.set(None);
                    self.setNeedsDisplay(true);
                }
            }

            #[unsafe(method(setFrameSize:))]
            fn set_frame_size(&self, size: NSSize) {
                let _: () = unsafe { msg_send![super(self), setFrameSize: size] };
                self.reset_tracking();
            }
        }

        unsafe impl NSObjectProtocol for JobRowListView {}
    );

    impl JobRowListView {
        fn row_at(&self, p: NSPoint) -> Option<usize> {
            let iv = self.ivars();
            let rows = iv.rows.borrow();
            if rows.is_empty() || p.y < HEADER_H {
                return None;
            }
            let i = ((p.y - HEADER_H) / (ROW_H + 2.0)).floor() as usize;
            (i < rows.len()).then_some(i)
        }

        fn track(&self, e: &NSEvent) {
            let p = view_base::mouse_point(self, e);
            let on = self.row_at(p);
            if self.ivars().hover.get() != on {
                self.ivars().hover.set(on);
                self.setNeedsDisplay(true);
            }
        }

        fn reset_tracking(&self) {
            view_base::reset_tracking(self, &self.ivars().tracking, view_base::hover_opts());
        }

        pub fn new(mtm: MainThreadMarker, colors: PopupColors) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(JobListIvars {
                rows: RefCell::new(Vec::new()),
                headers: RefCell::new(Default::default()),
                selected: Cell::new(None),
                hover: Cell::new(None),
                pressed: Cell::new(None),
                colors: Cell::new(colors),
                on_select: RefCell::new(None),
                tracking: RefCell::new(None),
            });
            let v: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
            v.reset_tracking();
            v
        }

        /// The list's natural height for `n` rows (header band + row pitch).
        pub fn height_for(n: usize) -> f64 {
            HEADER_H + n as f64 * (ROW_H + 2.0)
        }

        pub fn set_colors(&self, c: PopupColors) {
            self.ivars().colors.set(c);
            self.setNeedsDisplay(true);
        }

        pub fn set_on_select(&self, cb: impl Fn(usize) + 'static) {
            *self.ivars().on_select.borrow_mut() = Some(Rc::new(cb));
        }

        /// The four column titles drawn above the evenly spaced columns.
        pub fn set_headers(&self, headers: [String; 4]) {
            *self.ivars().headers.borrow_mut() = headers;
            self.setNeedsDisplay(true);
        }

        pub fn set_data(&self, rows: Vec<JobRowItem>, selected: Option<usize>) {
            let iv = self.ivars();
            *iv.rows.borrow_mut() = rows;
            iv.selected.set(selected);
            iv.hover.set(None);
            iv.pressed.set(None);
            self.setNeedsDisplay(true);
        }
    }

    pub struct BodyIvars {
        scroll: Retained<NSScrollView>,
        doc: Retained<FlippedView>,
        describe: RefCell<Option<Describe>>,
        load_error: RefCell<String>,
        settings: JiraConfigSettings,
        colors: Cell<PopupColors>,
        /// The auto-refresh timer (`[jira-config] dashboard-refresh`), live for
        /// the life of the view; it only fires while the tab is selected.
        timer: RefCell<Option<Retained<NSTimer>>>,
        sink: RefCell<Option<ActionSink>>,
        /// True while the Config tab is the selected surface.
        active: Cell<bool>,
        /// The embedded Connection / Setup form (persistent across `render`,
        /// so typed values and focus survive the auto-refresh).
        form: Retained<JiraSetupFormView>,
        /// `true` once the form's `--check` prefill has run.
        prefilled: Cell<bool>,
        /// The right-hand per-job detail pane (persistent across `render`, so
        /// typed values and focus survive the auto-refresh).
        detail: Retained<FlippedView>,
        d_heading: Retained<NSTextField>,
        d_sched_lbl: Retained<NSTextField>,
        d_jql_lbl: Retained<NSTextField>,
        d_args_lbl: Retained<NSTextField>,
        d_cols_lbl: Retained<NSTextField>,
        d_window: Retained<NSTextField>,
        d_enabled: Retained<OutlineButton>,
        /// The themed jobs list (persistent across `render`; the row NSButtons
        /// it replaces rebuilt their bezels — and the system-blue selection —
        /// on every auto-refresh tick).
        job_list: Retained<JobRowListView>,
        d_jql: Retained<NSTextField>,
        d_args: Retained<NSTextField>,
        d_columns: Retained<NSTextField>,
        d_result: Retained<NSTextField>,
        /// Save / Set Columns / Poll Now / Delete.
        d_buttons: [Retained<crate::ui::outline_button::OutlineButton>; 4],
        /// The job the detail pane currently shows (its fields hold its values;
        /// an empty name = nothing selected).
        d_shown_for: RefCell<String>,
        /// The values `populate_detail` last wrote into the four detail
        /// fields: if the fields no longer match them, the user is mid-edit
        /// and a refresh must NOT clobber the form (the old guard only
        /// covered a field that still held a field editor — click anything
        /// else and the edits vanished under the ≤5 s timer).
        d_last_detail: RefCell<Option<(String, String, String, String)>>,
        /// The job names behind the left list's rows, by button tag.
        d_names: RefCell<Vec<String>>,
        /// `true` while a background writer is running (its buttons are off).
        d_busy: Cell<bool>,
    }

    define_class!(
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSJiraConfigBody"]
        #[ivars = BodyIvars]
        pub struct JiraConfigBodyView;

        impl JiraConfigBodyView {
            #[unsafe(method(isFlipped))]
            fn is_flipped(&self) -> bool {
                true
            }

            /// Re-wrap the document whenever the Jira view resizes the surface.
            #[unsafe(method(setFrameSize:))]
            fn set_frame_size(&self, size: NSSize) {
                let _: () = unsafe { msg_send![super(self), setFrameSize: size] };
                self.render();
            }

            #[unsafe(method(enableClicked:))]
            fn enable_clicked(&self, _s: Option<&AnyObject>) {
                self.emit(JiraConfigAction::Enable);
            }

            #[unsafe(method(forceClicked:))]
            fn force_clicked(&self, _s: Option<&AnyObject>) {
                self.emit(JiraConfigAction::Force);
            }

            #[unsafe(method(stopClicked:))]
            fn stop_clicked(&self, _s: Option<&AnyObject>) {
                self.emit(JiraConfigAction::Stop);
            }

            #[unsafe(method(refreshClicked:))]
            fn refresh_clicked(&self, _s: Option<&AnyObject>) {
                self.emit(JiraConfigAction::Refresh);
            }

            /// Save: `--upsert-endpoint` (schedule / enabled / JQL / args).
            #[unsafe(method(saveJobClicked:))]
            fn save_job_clicked(&self, _s: Option<&AnyObject>) {
                self.write_save();
            }

            /// Set Columns: `--set-columns endpoint NAME SPEC`.
            #[unsafe(method(columnsJobClicked:))]
            fn columns_job_clicked(&self, _s: Option<&AnyObject>) {
                self.write_columns();
            }

            /// Poll Now: `jira_poll.py --job NAME` (that job only).
            #[unsafe(method(forceJobClicked:))]
            fn force_job_clicked(&self, _s: Option<&AnyObject>) {
                self.write_force();
            }

            /// Delete: `--delete-endpoint NAME`.
            #[unsafe(method(deleteJobClicked:))]
            fn delete_job_clicked(&self, _s: Option<&AnyObject>) {
                self.write_delete();
            }
        }

        unsafe impl NSObjectProtocol for JiraConfigBodyView {}
    );

    impl JiraConfigBodyView {
        pub fn new(mtm: MainThreadMarker, colors: PopupColors) -> Retained<Self> {
            let settings = JiraConfigSettings::load();
            let text_color = colors.text.to_nscolor();
            let dim = colors.dim.to_nscolor();

            let scroll = crate::ui::style::plain_scroll(mtm);
            scroll.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable
                    | NSAutoresizingMaskOptions::ViewHeightSizable,
            );
            let doc = FlippedView::new(mtm);
            doc.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
            scroll.setDocumentView(Some(&doc));
            // The embedded Connection / Setup form lives in the document view
            // for the body's whole life: `render()` must never rebuild it, so
            // typed values and focus survive the auto-refresh.
            let form = JiraSetupFormView::new(mtm, colors);

            // The persistent per-job detail pane (right column). Kept across
            // renders for the same reason as the form.
            let detail = FlippedView::new(mtm);
            let heading = NSTextField::labelWithString(&NSString::from_str(""), mtm);
            heading.setFont(Some(&NSFont::boldSystemFontOfSize(14.0)));
            heading.setTextColor(Some(&text_color));
            let label = |text: &str| {
                let l = NSTextField::labelWithString(&NSString::from_str(text), mtm);
                l.setAlignment(NSTextAlignment::Right);
                l.setFont(Some(&NSFont::systemFontOfSize(11.0)));
                l.setTextColor(Some(&dim));
                l
            };
            let sched_lbl = label(&settings.job_schedule_label);
            let jql_lbl = label(&settings.job_jql_label);
            let args_lbl = label(&settings.job_args_label);
            let cols_lbl = label(&settings.job_columns_label);
            let field = |placeholder: &str| {
                let f = NSTextField::new(mtm);
                f.setFont(Some(&NSFont::systemFontOfSize(12.0)));
                f.setTextColor(Some(&text_color));
                f.setPlaceholderString(Some(&NSString::from_str(placeholder)));
                f.setUsesSingleLineMode(true);
                if let Some(cell) = f.cell() {
                    cell.setWraps(false);
                    cell.setScrollable(true);
                }
                f
            };
            let window = field(&settings.job_schedule_placeholder);
            let jql = field(&settings.job_jql_placeholder);
            let args = field(&settings.job_args_placeholder);
            let columns = field(&settings.job_columns_placeholder);
            // Themed single-chip toggle (was a native `Switch` checkbox).
            let enabled = OutlineButton::new(mtm, colors, &settings.job_enabled_label);
            enabled.set_tone(Tone::Neutral);
            {
                let w = objc2::rc::Weak::from_retained(&enabled);
                enabled.set_on_click(move || {
                    if let Some(b) = w.load() {
                        let on = !b.is_on();
                        b.set_on(on);
                    }
                });
            }
            let job_list = JobRowListView::new(mtm, colors);
            let result = NSTextField::wrappingLabelWithString(&NSString::from_str(""), mtm);
            result.setFont(Some(&NSFont::systemFontOfSize(11.0)));
            result.setTextColor(Some(&dim));
            let button = |title: &str| {
                // Themed capsule (was a system `Push` bezel that ignored
                // `[theme]`).
                let b = crate::ui::outline_button::OutlineButton::new(mtm, colors, title);
                b.set_tone(crate::ui::outline_button::Tone::Neutral);
                b
            };
            let d_buttons = [
                button(&settings.job_save_label),
                button(&settings.job_columns_button_label),
                button(&settings.job_force_label),
                button(&settings.job_delete_label),
            ];

            let this = Self::alloc(mtm).set_ivars(BodyIvars {
                scroll: scroll.clone(),
                doc: doc.clone(),
                describe: RefCell::new(None),
                load_error: RefCell::new(String::new()),
                settings,
                colors: Cell::new(colors),
                timer: RefCell::new(None),
                sink: RefCell::new(None),
                active: Cell::new(false),
                form: form.clone(),
                prefilled: Cell::new(false),
                detail: detail.clone(),
                d_heading: heading,
                d_sched_lbl: sched_lbl,
                d_jql_lbl: jql_lbl,
                d_args_lbl: args_lbl,
                d_cols_lbl: cols_lbl,
                d_window: window,
                d_enabled: enabled,
                job_list: job_list.clone(),
                d_jql: jql,
                d_args: args,
                d_columns: columns,
                d_result: result,
                d_buttons,
                d_shown_for: RefCell::new(String::new()),
                d_last_detail: RefCell::new(None),
                d_names: RefCell::new(Vec::new()),
                d_busy: Cell::new(false),
            });
            let v: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
            v.addSubview(&scroll);
            doc.addSubview(&form);
            doc.addSubview(&detail);
            {
                let i = v.ivars();
                let subs: [&NSView; 11] = [
                    &i.d_heading, &i.d_sched_lbl, &i.d_jql_lbl, &i.d_args_lbl, &i.d_cols_lbl,
                    &i.d_window, &i.d_enabled, &i.d_jql, &i.d_args, &i.d_columns, &i.d_result,
                ];
                for w in subs {
                    i.detail.addSubview(w);
                }
                for b in &i.d_buttons {
                    i.detail.addSubview(b);
                }
                let target: &AnyObject = as_any(&*v);
                for (b, s) in [
                    (&i.d_buttons[0], sel!(saveJobClicked:)),
                    (&i.d_buttons[1], sel!(columnsJobClicked:)),
                    (&i.d_buttons[2], sel!(forceJobClicked:)),
                    (&i.d_buttons[3], sel!(deleteJobClicked:)),
                ] {
                    b.set_target_action(target, s);
                }
            }
            let weak_jobs = objc2::rc::Weak::from_retained(&v);
            v.ivars().job_list.set_on_select(move |idx| {
                if let Some(b) = weak_jobs.load() {
                    b.select_row(idx);
                }
            });
            LIVE_BODY.with(|l| *l.borrow_mut() = Some(objc2::rc::Weak::from_retained(&v)));
            // The timer holds only a Weak: the view owns the timer, so a strong
            // capture would keep it alive forever.
            let weak = objc2::rc::Weak::from_retained(&v);
            let secs = v.ivars().settings.refresh.max(2.0);
            let block = block2::RcBlock::new(move |_t: std::ptr::NonNull<NSTimer>| {
                let Some(body) = weak.load() else {
                    return;
                };
                if body.ivars().active.get() {
                    body.emit(JiraConfigAction::Refresh);
                }
            });
            let timer =
                unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(secs, true, &block) };
            *v.ivars().timer.borrow_mut() = Some(timer);
            v
        }

        pub fn set_on_action(&self, cb: Box<dyn Fn(JiraConfigAction)>) {
            let rc: ActionSink = Rc::from(cb);
            *self.ivars().sink.borrow_mut() = Some(rc.clone());
            // The embedded form reports a successful Save & Enable.
            self.ivars().form.set_on_event(Box::new(move |event| match event {
                SetupOutcome::Saved => rc(JiraConfigAction::SetupSaved),
            }));
        }

        /// `true` while the Config tab is selected: the auto-refresh only runs
        /// then. The first activation also runs the form's `--check` prefill.
        pub fn set_active(&self, on: bool) {
            self.ivars().active.set(on);
            if on && !self.ivars().prefilled.replace(true) {
                self.ivars().form.prefill();
            }
        }

        /// The host's `jira_poll.py --describe` result (replaces the status
        /// line and the job rows; an error keeps the last good payload).
        pub fn set_result(&self, res: Result<Describe, String>) {
            match res {
                Ok(d) => {
                    *self.ivars().describe.borrow_mut() = Some(d);
                    *self.ivars().load_error.borrow_mut() = String::new();
                }
                Err(e) => *self.ivars().load_error.borrow_mut() = format!("✗ {e}"),
            }
            self.render();
        }

        fn emit(&self, a: JiraConfigAction) {
            let s = self.ivars().sink.borrow().clone();
            if let Some(s) = s {
                s(a);
            }
        }

        // -- rendering -----------------------------------------------------

        /// A jobs-list row pick: select its job and repopulate the detail
        /// pane (`JobRowListView`'s callback; was `jobClicked:` + sender tags
        /// on per-row NSButtons).
        fn select_row(&self, idx: usize) {
            let name = self.ivars().d_names.borrow().get(idx).cloned();
            if let Some(name) = name {
                select_job(&name);
                self.ivars().d_shown_for.borrow_mut().clear();
                self.render();
            }
        }

        fn color(&self, tone: &str) -> Rgba {
            let c = self.ivars().colors.get();
            match tone {
                "dim" => c.dim,
                "warn" => c.tone(PopupTone::Warning),
                "err" => c.tone(PopupTone::Danger),
                "ok" => c.tone(PopupTone::Success),
                _ => c.text,
            }
        }

        fn clear(&self) {
            // Everything except the embedded form and the per-job detail pane
            // (they keep their typed values and focus across re-renders).
            let doc = &self.ivars().doc;
            let keep = [
                as_any(&*self.ivars().form) as *const AnyObject,
                as_any(&*self.ivars().detail) as *const AnyObject,
                as_any(&*self.ivars().job_list) as *const AnyObject,
            ];
            for v in doc.subviews().iter() {
                let p = as_any(&*v) as *const AnyObject;
                if keep.iter().any(|k| std::ptr::eq(p, *k)) {
                    continue;
                }
                v.removeFromSuperview();
            }
        }

        fn add(&self, v: &NSView) {
            self.ivars().doc.addSubview(v);
        }

        fn label(
            &self,
            mtm: MainThreadMarker,
            text: &str,
            size: f64,
            bold: bool,
            color: Rgba,
        ) -> Retained<NSTextField> {
            let l = NSTextField::labelWithString(&NSString::from_str(text), mtm);
            let f = if bold {
                NSFont::boldSystemFontOfSize(size)
            } else {
                NSFont::systemFontOfSize(size)
            };
            l.setFont(Some(&f));
            l.setTextColor(Some(&color.to_nscolor()));
            l
        }

        fn title_at_x(&self, mtm: MainThreadMarker, text: &str, x: f64, y: f64, w: f64) -> f64 {
            let l = self.label(mtm, &text.to_uppercase(), 10.0, true, self.ivars().colors.get().dim);
            l.setFrame(rect(x, y, w, LINE_H));
            self.add(&l);
            y + TITLE_H
        }

        fn title_at(&self, mtm: MainThreadMarker, text: &str, y: f64, w: f64) -> f64 {
            self.title_at_x(mtm, text, PAD, y, w)
        }

        /// Rebuild the document view from the current state.
        fn render(&self) {
            let Some(mtm) = MainThreadMarker::new() else { return };
            self.clear();
            let colors = self.ivars().colors.get();
            let doc_w = (self.ivars().scroll.bounds().size.width).max(320.0);
            let mut y = PAD;

            // Status line.
            let err = self.ivars().load_error.borrow().clone();
            let enabled = crate::app::jira_poll::enabled_in_config();
            let poll_active = crate::app::jira_poll::poll_active_in_config();
            let describe = self.ivars().describe.borrow().clone();
            let (status, tone) = if !err.is_empty() {
                (err, "err")
            } else {
                match &describe {
                    Some(d) => status_line(d, enabled, poll_active, d.lock_held, now_epoch()),
                    None => (self.ivars().settings.loading.clone(), "dim"),
                }
            };
            let sl = self.label(mtm, &status, 13.0, true, self.color(tone));
            sl.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
            sl.setFrame(rect(PAD, y, doc_w - PAD * 2.0, LINE_H + 4.0));
            self.add(&sl);
            y += 28.0;

            // Problems (from the describe payload + last error).
            let mut problems: Vec<String> = describe.as_ref().map(|d| d.problems.clone()).unwrap_or_default();
            if let Some(d) = &describe {
                if !d.last_error.trim().is_empty() && !problems.iter().any(|p| p.contains(&d.last_error)) {
                    problems.push(format!("last error: {}", d.last_error));
                }
            }
            if !problems.is_empty() {
                let text = problems.iter().map(|p| format!("⚠ {p}")).collect::<Vec<_>>().join("\n");
                let l = NSTextField::wrappingLabelWithString(&NSString::from_str(&text), mtm);
                l.setFont(Some(&NSFont::systemFontOfSize(11.0)));
                l.setTextColor(Some(&colors.tone(PopupTone::Danger).to_nscolor()));
                let h = (problems.len() as f64 * 15.0).max(16.0);
                l.setFrame(rect(PAD, y, doc_w - PAD * 2.0, h));
                self.add(&l);
                y += h + 8.0;
            }

            // Controls.
            y = self.render_controls(mtm, y);

            // The two-column job area: left = every endpoint (name, interval,
            // last run, status), right = the selected job's detail pane.
            let s = self.ivars().settings.clone();
            y = self.render_jobs(mtm, &describe, y, doc_w);

            // Files.
            y = self.title_at(mtm, &s.files_title, y + 10.0, doc_w);
            let paths = describe.as_ref().map(file_paths).unwrap_or_default();
            if paths.is_empty() {
                let l = self.label(mtm, &s.loading_paths, 11.5, false, colors.dim);
                l.setFrame(rect(PAD + 4.0, y, doc_w - PAD * 2.0, LINE_H));
                self.add(&l);
                y += LINE_H + 4.0;
            } else {
                for (name, path) in &paths {
                    let l = NSTextField::labelWithString(&NSString::from_str(&format!("{name}  {path}")), mtm);
                    l.setFont(Some(&NSFont::monospacedSystemFontOfSize_weight(11.0, 0.0)));
                    l.setTextColor(Some(&colors.dim.to_nscolor()));
                    l.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
                    l.setFrame(rect(PAD + 4.0, y, doc_w - PAD * 2.0, LINE_H));
                    self.add(&l);
                    y += LINE_H + 2.0;
                }
            }

            // Connection / Setup — the embedded form (the old separate
            // `JiraSetupWindow`, now a section of this surface). Its fields are
            // persistent, so it survives every re-render with values intact.
            let form = self.ivars().form.clone();
            let title = form.section_title();
            y = self.title_at(mtm, &title, y + 12.0, doc_w);
            let form_h = form.height();
            let fw = (doc_w - PAD * 2.0).max(1.0);
            let cur = form.frame();
            if cur.origin.y != y || cur.size.width != fw || cur.size.height != form_h {
                form.setFrame(rect(PAD, y, fw, form_h));
            }
            y += form_h + PAD;

            let h = (y + PAD).max(self.ivars().scroll.bounds().size.height);
            self.ivars().doc.setFrame(rect(0.0, 0.0, doc_w, h));
        }

        /// The two-column job area: the left list of every endpoint, the right
        /// per-job detail pane. Returns the next `y`.
        fn render_jobs(
            &self,
            mtm: MainThreadMarker,
            describe: &Option<Describe>,
            y: f64,
            doc_w: f64,
        ) -> f64 {
            let colors = self.ivars().colors.get();
            let s = self.ivars().settings.clone();
            let rows = describe.as_ref().map(job_rows).unwrap_or_default();
            let names: Vec<String> = describe
                .as_ref()
                .map(|d| d.endpoints.iter().map(|j| j.name.clone()).collect())
                .unwrap_or_default();
            *self.ivars().d_names.borrow_mut() = names;

            let list_w = (doc_w - PAD * 2.0).max(200.0);
            let top = y + 6.0;
            // The poll-jobs list spans the whole document width (it used to
            // share a fixed 320 pt column with the detail pane, which chopped
            // the job line). The detail pane sits below it, also full width;
            // the enclosing scroll view handles a long list.
            let mut ly = self.title_at_x(mtm, &s.jobs_title, PAD, top, list_w);
            let list_top = ly;
            if rows.is_empty() {
                ly += ROW_H + 2.0;
            } else {
                ly += JobRowListView::height_for(rows.len());
            }
            ly += 10.0;

            // The selected job's detail pane (its widgets are persistent;
            // only their values are refreshed here). Populated before the
            // rows so the list highlights the resolved selection.
            let pane_y = self.title_at_x(mtm, &s.job_detail_title, PAD, ly, list_w);
            let detail = self.ivars().detail.clone();
            let cur = detail.frame();
            if cur.origin.x != PAD
                || cur.origin.y != pane_y
                || cur.size.width != list_w
                || cur.size.height != DETAIL_H
            {
                detail.setFrame(rect(PAD, pane_y, list_w, DETAIL_H));
            }
            self.layout_detail(list_w);
            self.populate_detail(describe);
            let selected = selected_job();

            if rows.is_empty() {
                let text = if describe.is_none() {
                    s.loading_jobs.as_str()
                } else {
                    s.no_jobs.as_str()
                };
                let l = self.label(mtm, text, 12.0, false, colors.dim);
                l.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
                l.setFrame(rect(PAD, list_top, list_w, LINE_H));
                self.add(&l);
            } else {
                // One themed row table instead of a pile of system-bezel
                // NSButtons (exactly-one-of selection, hover + press): four
                // evenly spaced columns under a header of column names.
                let now = now_epoch();
                let items: Vec<JobRowItem> = rows
                    .iter()
                    .map(|r| JobRowItem {
                        cells: job_row_cells(r, now),
                        tone: self.color(job_row_tone(&r.status, r.enabled)),
                    })
                    .collect();
                let sel_idx =
                    rows.iter().position(|r| selected.as_deref() == Some(r.name.as_str()));
                let list = self.ivars().job_list.clone();
                list.set_colors(colors);
                list.set_headers([
                    s.job_col_name_label.clone(),
                    s.job_col_frequency_label.clone(),
                    s.job_col_last_run_label.clone(),
                    s.job_col_status_label.clone(),
                ]);
                list.set_data(items, sel_idx);
                list.setFrame(rect(PAD, list_top, list_w, JobRowListView::height_for(rows.len())));
                self.add(&list);
            }

            pane_y + DETAIL_H + 10.0
        }

        /// Rebuild the detail pane's widget frames for the given width.
        fn layout_detail(&self, w: f64) {
            let i = self.ivars();
            let fx = D_LABEL_W + 6.0;
            let fw = (w - fx - 8.0).max(60.0);
            i.d_heading.setFrame(rect(0.0, 0.0, w, 20.0));
            let mut y = 26.0;
            for (lbl, fld) in [
                (&i.d_sched_lbl, &i.d_window),
                (&i.d_jql_lbl, &i.d_jql),
                (&i.d_args_lbl, &i.d_args),
                (&i.d_cols_lbl, &i.d_columns),
            ] {
                lbl.setFrame(rect(0.0, y + 4.0, D_LABEL_W, 16.0));
                fld.setFrame(rect(fx, y, fw, D_FIELD_H));
                y += D_ROW;
            }
            let ew = crate::ui::outline_button::OutlineButton::width_for(&i.settings.job_enabled_label);
            i.d_enabled.setFrame(rect(fx, y + 1.0, ew, 22.0));
            y += D_ROW;
            let bw = ((w - 6.0) / 4.0 - 6.0).max(60.0);
            for (n, b) in i.d_buttons.iter().enumerate() {
                b.setFrame(rect(n as f64 * (bw + 6.0), y, bw, D_BUTTON_H));
            }
            i.d_result.setFrame(rect(0.0, y + D_BUTTON_H + 6.0, w, D_RESULT_H - 8.0));
        }

        /// Fill the detail pane from the describe payload (the selected job, or
        /// the first one; never clobbers a value the user is typing).
        fn populate_detail(&self, describe: &Option<Describe>) {
            let i = self.ivars();
            let s = &i.settings;
            let selected = selected_job();
            let job = describe.as_ref().and_then(|d| {
                selected
                    .as_deref()
                    .and_then(|n| d.job(n))
                    .or_else(|| d.endpoints.first())
            });
            let Some(job) = job else {
                i.d_shown_for.borrow_mut().clear();
                *i.d_last_detail.borrow_mut() = None;
                i.d_heading.setStringValue(&NSString::from_str(&s.job_detail_title));
                self.set_detail_result(&s.job_select_hint, None);
                i.d_enabled.set_on(false);
                for f in [&i.d_window, &i.d_jql, &i.d_args, &i.d_columns] {
                    f.setStringValue(&NSString::from_str(""));
                    f.setEnabled(false);
                }
                for b in &i.d_buttons {
                    b.set_enabled(false);
                }
                return;
            };
            let name = job.name.clone();
            if selected.as_deref() != Some(name.as_str()) {
                select_job(&name);
            }
            let editing = [&i.d_window, &i.d_jql, &i.d_args, &i.d_columns]
                .iter()
                .any(|f| f.currentEditor().is_some());
            // Same job and the fields hold un-saved user edits → leave the
            // form alone (the value that differs from what we last wrote
            // is the user's, not the disk's).
            let dirty = i.d_last_detail.borrow().as_ref().is_some_and(|(w, j, a, c)| {
                i.d_window.stringValue().to_string() != *w
                    || i.d_jql.stringValue().to_string() != *j
                    || i.d_args.stringValue().to_string() != *a
                    || i.d_columns.stringValue().to_string() != *c
            });
            if *i.d_shown_for.borrow() == name && (editing || dirty) {
                return;
            }
            let detail = job_detail(job);
            i.d_heading.setStringValue(&NSString::from_str(&name));
            i.d_window.setStringValue(&NSString::from_str(&detail.window));
            i.d_enabled.set_on(detail.enabled);
            i.d_jql.setStringValue(&NSString::from_str(&detail.jql));
            i.d_args.setStringValue(&NSString::from_str(&detail.args));
            i.d_columns.setStringValue(&NSString::from_str(&detail.columns));
            *i.d_last_detail.borrow_mut() = Some((
                detail.window.clone(),
                detail.jql.clone(),
                detail.args.clone(),
                detail.columns.clone(),
            ));
            for f in [&i.d_window, &i.d_jql, &i.d_args, &i.d_columns] {
                f.setEnabled(true);
            }
            if !i.d_busy.get() {
                for b in &i.d_buttons {
                    b.set_enabled(true);
                }
            }
            *i.d_shown_for.borrow_mut() = name;
        }

        /// The detail pane's result line (`ok` = green / red / dim).
        fn set_detail_result(&self, text: &str, ok: Option<bool>) {
            let i = self.ivars();
            let c = match ok {
                Some(true) => i.colors.get().tone(PopupTone::Success),
                Some(false) => i.colors.get().tone(PopupTone::Danger),
                None => i.colors.get().dim,
            };
            i.d_result.setStringValue(&NSString::from_str(text));
            i.d_result.setTextColor(Some(&c.to_nscolor()));
        }

        fn set_detail_busy(&self, on: bool) {
            self.ivars().d_busy.set(on);
            for b in &self.ivars().d_buttons {
                b.set_enabled(!on);
            }
        }

        /// The job the detail pane currently shows (empty = none selected).
        fn selected_name(&self) -> String {
            let shown = self.ivars().d_shown_for.borrow().clone();
            if !shown.is_empty() {
                shown
            } else {
                selected_job().unwrap_or_default()
            }
        }

        /// A background writer finished: show its message, refresh, and forget a
        /// deleted job's selection.
        fn finish_write(&self, result: WriteResult) {
            self.set_detail_busy(false);
            match result {
                Ok((msg, clear)) => {
                    self.set_detail_result(&msg, Some(true));
                    // The form's content is now what the disk holds (we
                    // just wrote it): drop the dirty guard so the refresh
                    // can re-populate normally.
                    *self.ivars().d_last_detail.borrow_mut() = None;
                    if clear {
                        clear_job();
                        self.ivars().d_shown_for.borrow_mut().clear();
                    }
                    self.emit(JiraConfigAction::Refresh);
                }
                Err(e) => {
                    let msg = fill_error(&self.ivars().settings.job_failed, &e);
                    self.set_detail_result(&msg, Some(false));
                }
            }
        }

        /// Save: the schedule (`--set-window`), enabled (`--set-enabled`) and
        /// JQL / args (`--upsert-endpoint`). Columns are Set Columns.
        fn write_save(&self) {
            let name = self.selected_name();
            if name.is_empty() {
                return;
            }
            let detail = JobDetail {
                name: name.clone(),
                window: self.ivars().d_window.stringValue().to_string(),
                enabled: self.ivars().d_enabled.is_on(),
                jql: self.ivars().d_jql.stringValue().to_string(),
                args: self.ivars().d_args.stringValue().to_string(),
                columns: self.ivars().d_columns.stringValue().to_string(),
            };
            let window = detail.window.trim().to_string();
            let enabled = detail.enabled;
            let payload = upsert_payload(&detail).to_string();
            let saved = fill(&self.ivars().settings.job_saved, &name);
            self.set_detail_busy(true);
            let name_arg = name.clone();
            background(
                move || {
                    // A blank schedule field means "leave the schedule as it
                    // is": `--set-window NAME ""` would fail (parse_window)
                    // and abort the whole Save before enabled / JQL apply.
                    if !window.is_empty() {
                        let (code, out, err) = crate::app::jira_poll::run_stdin(
                            "jira_config.py",
                            &["--set-window", name_arg.as_str(), window.as_str()],
                            None,
                        );
                        if code != 0 {
                            return Err(writer_error(&out, &err, code));
                        }
                    }
                    let (code, out, err) = crate::app::jira_poll::run_stdin(
                        "jira_config.py",
                        &[
                            "--set-enabled",
                            name_arg.as_str(),
                            if enabled { "true" } else { "false" },
                        ],
                        None,
                    );
                    if code != 0 {
                        return Err(writer_error(&out, &err, code));
                    }
                    let (code, out, err) = crate::app::jira_poll::run_stdin(
                        "jira_config.py",
                        &["--upsert-endpoint"],
                        Some(&payload),
                    );
                    if code == 0 {
                        Ok((saved, false))
                    } else {
                        Err(writer_error(&out, &err, code))
                    }
                },
                |b, r| b.finish_write(r),
            );
        }

        /// Set Columns: `jira_config.py --set-columns endpoint NAME SPEC`.
        fn write_columns(&self) {
            let name = self.selected_name();
            if name.is_empty() {
                return;
            }
            let spec = self.ivars().d_columns.stringValue().to_string();
            let saved = fill(&self.ivars().settings.job_saved, &name);
            self.set_detail_busy(true);
            let name_arg = name.clone();
            background(
                move || {
                    let (code, out, err) = crate::app::jira_poll::run_stdin(
                        "jira_config.py",
                        &["--set-columns", "endpoint", name_arg.as_str(), spec.as_str()],
                        None,
                    );
                    if code == 0 {
                        Ok((saved, false))
                    } else {
                        Err(writer_error(&out, &err, code))
                    }
                },
                |b, r| b.finish_write(r),
            );
        }

        /// Poll Now: `jira_poll.py --job NAME` (that one job only).
        fn write_force(&self) {
            let name = self.selected_name();
            if name.is_empty() {
                return;
            }
            let polled = fill(&self.ivars().settings.job_polled, &name);
            let running = fill(&self.ivars().settings.job_running, &name);
            self.set_detail_busy(true);
            self.set_detail_result(&running, None);
            background(
                move || {
                    let (code, out, err) =
                        crate::app::jira_poll::run("jira_poll.py", &["--job", name.as_str(), "--quiet"]);
                    if code == 0 {
                        Ok((polled, false))
                    } else {
                        Err(writer_error(&out, &err, code))
                    }
                },
                |b, r| b.finish_write(r),
            );
        }

        /// Delete: `jira_config.py --delete-endpoint NAME`.
        fn write_delete(&self) {
            let name = self.selected_name();
            if name.is_empty() {
                return;
            }
            let deleted = fill(&self.ivars().settings.job_deleted, &name);
            self.set_detail_busy(true);
            let name_arg = name.clone();
            background(
                move || {
                    let (code, out, err) = crate::app::jira_poll::run_stdin(
                        "jira_config.py",
                        &["--delete-endpoint", name_arg.as_str()],
                        None,
                    );
                    if code == 0 {
                        Ok((deleted, true))
                    } else {
                        Err(writer_error(&out, &err, code))
                    }
                },
                |b, r| b.finish_write(r),
            );
        }

        fn render_controls(&self, mtm: MainThreadMarker, y: f64) -> f64 {
            let s = &self.ivars().settings;
            let colors = self.ivars().colors.get();
            let enabled = crate::app::jira_poll::enabled_in_config();
            let lock_held = self
                .ivars()
                .describe
                .borrow()
                .as_ref()
                .map(|d| d.lock_held)
                .unwrap_or(false);
            let enable = if enabled { &s.disable_label } else { &s.enable_label };
            let specs: [(&str, objc2::runtime::Sel, JiraConfigAction); 4] = [
                (enable.as_str(), sel!(enableClicked:), JiraConfigAction::Enable),
                (s.force_label.as_str(), sel!(forceClicked:), JiraConfigAction::Force),
                (s.stop_label.as_str(), sel!(stopClicked:), JiraConfigAction::Stop),
                (s.refresh_label.as_str(), sel!(refreshClicked:), JiraConfigAction::Refresh),
            ];
            let target: &AnyObject = as_any(self);
            let mut x = PAD;
            for (title, action, kind) in specs {
                // Themed capsules (were system `Push` bezels the `[theme]`
                // cannot reach), now sized by the label's real metrics.
                let b = crate::ui::outline_button::OutlineButton::new(mtm, colors, title);
                b.set_tone(crate::ui::outline_button::Tone::Neutral);
                let w = crate::ui::outline_button::OutlineButton::width_for(title).max(84.0);
                b.setFrame(rect(x, y, w, 28.0));
                match kind {
                    JiraConfigAction::Stop => {
                        b.setToolTip(Some(&NSString::from_str("Stop the running poll (jira_poll.py --cancel)")));
                        b.set_enabled(lock_held);
                    }
                    JiraConfigAction::Force => {
                        // Clickable while a poll holds the lock meant a
                        // guaranteed skipped run ("✗ exit 3"); the honest
                        // control is a greyed button while it runs.
                        b.setToolTip(Some(&NSString::from_str(
                            "Poll every enabled job now (jira_poll.py --force)",
                        )));
                        b.set_enabled(!lock_held);
                    }
                    _ => {}
                }
                b.set_target_action(target, action);
                self.add(&b);
                x += w + 8.0;
            }
            y + 38.0
        }
    }

    impl std::fmt::Debug for JiraConfigBodyView {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("JiraConfigBodyView").finish()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn describe(v: serde_json::Value) -> Describe {
        Describe::from_value(&v)
    }

    #[test]
    fn settings_read_from_section_with_defaults() {
        let d = JiraConfigSettings::default();
        assert_eq!(JiraConfigSettings::from_section(&HashMap::new()), d);

        let mut m = HashMap::new();
        m.insert("tickets-label".to_string(), "Issues".to_string());
        m.insert("config-label".to_string(), "Jobs".to_string());
        m.insert("dashboard-refresh".to_string(), "9".to_string());
        m.insert("force-poll-label".to_string(), "Poll Now".to_string());
        let s = JiraConfigSettings::from_section(&m);
        assert_eq!(s.tickets_label, "Issues");
        assert_eq!(s.config_label, "Jobs");
        assert_eq!(s.refresh, 9.0);
        assert_eq!(s.force_label, "Poll Now");
        // Untouched keys keep their defaults.
        assert_eq!(s.enable_label, d.enable_label);
        assert_eq!(s.stop_label, d.stop_label);
    }

    #[test]
    fn from_text_reads_only_the_jira_config_section() {
        let text = "\
[app]
tickets-label = \"Nope\"

[jira-config]
enabled = true
in-palette = true
tickets-label = \"Tickets\"
config-label = \"Config\"
enable-label = \"Turn On\"

[jira]
enabled = true
";
        let s = JiraConfigSettings::from_text(text);
        assert_eq!(s.tickets_label, "Tickets");
        assert_eq!(s.config_label, "Config");
        assert_eq!(s.enable_label, "Turn On");
        assert_eq!(s.disable_label, "Disable Jira");
    }

    #[test]
    fn job_rows_report_interval_and_last_run() {
        let d = describe(json!({
            "endpoints": [
                {"name": "issues", "window": "5m", "enabled": true, "status": "ok",
                 "lastRun": "2026-10-14 12:00:00", "nextRun": "2026-10-14 12:05:00"},
                {"name": "dir", "window": "6h", "enabled": false, "status": "never run"},
            ]
        }));
        let rows = job_rows(&d);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "issues");
        assert_eq!(rows[0].interval, "5m");
        assert_eq!(rows[0].last_run, "2026-10-14 12:00:00");
        assert!(rows[0].enabled);
        assert_eq!(rows[1].interval, "6h");
        assert_eq!(rows[1].last_run, "never");
        assert_eq!(rows[1].next_run, "due");
        assert!(!rows[1].enabled);
    }

    #[test]
    fn next_poll_prefers_due_else_soonest_enabled() {
        let d = describe(json!({
            "endpoints": [
                {"name": "a", "enabled": true, "nextRun": "2026-10-14 13:00:00"},
                {"name": "b", "enabled": true, "nextRun": "2026-10-14 12:30:00"},
                {"name": "c", "enabled": false, "nextRun": "due"},
            ]
        }));
        assert_eq!(next_poll(&d), "2026-10-14 12:30:00");

        let due = describe(json!({"endpoints": [{"name": "a", "enabled": true, "nextRun": "due"}]}));
        assert_eq!(next_poll(&due), "due");

        let none = describe(json!({"endpoints": []}));
        assert_eq!(next_poll(&none), "—");
    }

    #[test]
    fn job_row_cells_are_four_even_columns() {
        let d = describe(json!({
            "endpoints": [
                {"name": "all", "window": "10m", "enabled": true, "status": "ok",
                 "lastRun": "2026-10-14 12:00:00"},
                {"name": "dir", "window": "1w", "enabled": false, "status": "ok"},
            ]
        }));
        let now = parse_poll_ts("2026-10-14 12:06:00").unwrap();
        let rows = job_rows(&d);
        assert_eq!(
            job_row_cells(&rows[0], now),
            ["all", "every 10m", "6m ago", "ok"],
            "name / frequency / last run / status"
        );
        let off = job_row_cells(&rows[1], now);
        assert_eq!(off[1], "every 1w");
        assert_eq!(off[2], "never", "a non-timestamp last run passes through");
        assert_eq!(off[3], "ok (off)", "a disabled job keeps the off marker");
    }

    #[test]
    fn job_row_tone_is_a_traffic_light() {
        assert_eq!(job_row_tone("ok", true), "ok");
        assert_eq!(job_row_tone("error", true), "err");
        assert_eq!(job_row_tone("Error: 401 Unauthorized", true), "err");
        assert_eq!(job_row_tone("running", true), "warn");
        assert_eq!(job_row_tone("cancelled", true), "warn");
        assert_eq!(job_row_tone("never run", true), "dim");
        assert_eq!(job_row_tone("", true), "dim");
        assert_eq!(job_row_tone("ok", false), "dim", "a disabled job stays quiet");
    }

    #[test]
    fn status_line_reports_on_off_and_last_run() {
        let d = describe(json!({
            "lastRun": "2026-10-14 12:00:00",
            "endpoints": [{"name": "a", "enabled": true, "nextRun": "2026-10-14 12:05:00"}]
        }));
        let now = parse_poll_ts("2026-10-14 12:02:00").unwrap();
        let (line, tone) = status_line(&d, true, false, false, now);
        assert!(line.starts_with("● Polling on"));
        assert!(line.contains("last run 2m ago"), "{line}");
        assert!(line.contains("next in 3m"), "{line}");
        assert_eq!(tone, "ok");

        let (off, _) = status_line(&d, false, false, false, now);
        assert_eq!(off, "○ Polling off");
        let (bg, _) = status_line(&d, false, true, false, now);
        assert!(bg.starts_with("◐ Polling in the background"));

        let (now_line, now_tone) = status_line(&d, true, false, true, now);
        assert!(now_line.starts_with("● Polling now…"));
        assert_eq!(now_tone, "warn", "a poll in flight shows amber");

        let bad = describe(json!({
            "lastRun": "2026-10-14 12:00:00",
            "endpoints": [{"name": "a", "enabled": true, "status": "error: 401", "nextRun": "due"}]
        }));
        let (_, err_tone) = status_line(&bad, true, false, false, now);
        assert_eq!(err_tone, "err", "one failed job reddens the header");
    }

    #[test]
    fn fmt_poll_time_is_relative_and_passes_through_non_timestamps() {
        let at = parse_poll_ts("2026-10-14 12:00:00").unwrap();
        assert_eq!(fmt_poll_time("2026-10-14 12:00:00", at + 41).unwrap(), "41s ago");
        assert_eq!(fmt_poll_time("2026-10-14 12:00:00", at + 600).unwrap(), "10m ago");
        assert_eq!(fmt_poll_time("2026-10-14 12:00:00", at + 5 * 3600 + 60).unwrap(), "5h ago");
        assert_eq!(fmt_poll_time("2026-10-14 12:00:00", at + 2 * 86400).unwrap(), "2d ago");
        assert_eq!(fmt_poll_time("2026-10-14 12:00:00", at - 540).unwrap(), "in 9m");
        assert_eq!(fmt_poll_time("2026-10-14 12:00:00", at).unwrap(), "0s ago");
        assert_eq!(fmt_poll_time("due", at), None);
        assert_eq!(fmt_poll_time("never", at), None);
        assert_eq!(fmt_poll_time("", at), None);
    }

    #[test]
    fn file_paths_drop_empty_entries() {
        let d = describe(json!({
            "configPath": "/c/config.json",
            "teamPath": "/c/team.json",
            "statusPath": "/c/status.json",
            "pollLog": "/c/poll.log",
            "directory": {"path": "/c/directory.json"}
        }));
        let files = file_paths(&d);
        let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            ["Jira config", "Team schema", "Poll status", "Directory cache", "Poll log"]
        );
        assert!(files.iter().all(|(_, p)| !p.is_empty()));
    }

    #[test]
    fn job_pane_strings_read_from_section() {
        let d = JiraConfigSettings::default();
        let mut m = HashMap::new();
        m.insert("job-detail-title".to_string(), "Endpoint".to_string());
        m.insert("job-force-label".to_string(), "Run".to_string());
        m.insert("job-saved".to_string(), "done {name}".to_string());
        m.insert("job-col-frequency-label".to_string(), "Cadence".to_string());
        let s = JiraConfigSettings::from_section(&m);
        assert_eq!(s.job_detail_title, "Endpoint");
        assert_eq!(s.job_force_label, "Run");
        assert_eq!(s.job_saved, "done {name}");
        assert_eq!(s.job_col_frequency_label, "Cadence");
        // Untouched keys keep their defaults.
        assert_eq!(s.job_delete_label, d.job_delete_label);
        assert_eq!(s.job_polled, d.job_polled);
        assert_eq!(s.job_col_name_label, d.job_col_name_label);
        assert_eq!(s.job_col_status_label, d.job_col_status_label);
    }

    #[test]
    fn detail_and_upsert_payload_carry_the_editable_fields() {
        let d = describe(json!({
            "endpoints": [
                {
                    "name": "issues", "window": "10m", "enabled": true,
                    "jql": "", "extraJql": "project = KAN",
                    "args": {"a": "1", "b": 2},
                    "columnsSpec": "[{\"field\":\"key\"}]"
                },
                {
                    // A plain (shared-sync) job: `jql` is the *generated* sync
                    // query (with ORDER BY), `extraJql` is empty.
                    "name": "plain", "window": "30m", "enabled": true,
                    "jql": "project in (\"KAN\") ORDER BY key ASC",
                    "extraJql": ""
                }
            ]
        }));
        let detail = detail_for(&d, "issues").unwrap();
        assert_eq!(detail.name, "issues");
        assert_eq!(detail.window, "10m");
        assert!(detail.enabled);
        assert_eq!(detail.jql, "project = KAN");
        assert_eq!(detail.args, "a=1, b=2");
        assert_eq!(detail.columns, "[{\"field\":\"key\"}]");
        // No such job -> None (the pane stays empty).
        assert!(detail_for(&d, "nope").is_none());

        let payload = upsert_payload(&detail);
        assert_eq!(payload["name"], json!("issues"));
        assert_eq!(payload["jql"], json!("project = KAN"));
        assert_eq!(payload["args"], json!({"a": "1", "b": "2"}));
        // The schedule / enabled / columns are the other verbs' concern.
        assert!(payload.get("window").is_none());
        assert!(payload.get("enabled").is_none());
        assert!(payload.get("columns").is_none());

        // A plain job's detail must NOT fall back to the generated `jql`:
        // saving it would convert the shared-sync job into a standalone-JQL
        // job (and nest a second ORDER BY on the next sync).
        let plain = detail_for(&d, "plain").unwrap();
        assert_eq!(plain.jql, "");
        assert_eq!(upsert_payload(&plain)["jql"], json!(""));
    }

    #[test]
    fn args_text_renders_scalars() {
        assert_eq!(args_text(&json!({})), "");
        assert_eq!(args_text(&json!({"x": "y"})), "x=y");
        assert_eq!(args_text(&json!({"n": 3, "b": true})), "b=true, n=3");
    }

    #[test]
    fn config_do_selects_a_job_and_reports_state() {
        assert_eq!(config_do("jira-config:job:issues"), Some(json!({"job": "issues"})));
        assert_eq!(selected_job().as_deref(), Some("issues"));
        // `state` is an object carrying the current selection.
        let st = config_do("jira-config:state").unwrap();
        assert!(st.get("job").is_some());
        // An empty name and an unknown sub-verb are refused / not ours.
        assert_eq!(config_do("jira-config:job:"), Some(json!({"error": "jira-config:job:<name>"})));
        assert!(config_do("jira-config:nope").is_none());
        assert!(config_do("something-else").is_none());
    }
}
