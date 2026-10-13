//! The tool panels `SwitcherController.openTool` owns beyond paths/terminal —
//! `openPrettyPrintWindow` (prettyprint), `openFileFastWindow` (filefast) and
//! the `health-checks` output panel (`kitchen_sink.swift`).
//!
//! Window mechanics mirror `raiseToolPanel` / `reclaimToolKey`: a
//! non-activating `PopupPanel` at the floating level that never activates the
//! app, ordered front with `orderFrontRegardless` + `makeKey`. The bodies are
//! ported per kind: the prettyprint editor (debounced auto-format through
//! `jq`/`xmllint`, the two header buttons), the filefast paste view
//! (`FF_DIR`/`FF_NAME` script run on Return) and the streaming health-checks
//! output.
//!
//! `text-table` (no Swift original) is the "Text to Table" panel: the input
//! (pre-filled from the clipboard, e.g. Hyper+X's Copy Text) goes through the
//! standalone `pylib/text_table` CLI over temp files, the preview shows the
//! result, and Copy puts it on the clipboard (Outlook = HTML + plain twin).
//!
//! Filefast's Swift `PathShelf.shared.add([path], why: .filefast)` has no
//! process-wide store in the Rust port (the shelf is owned by the paths
//! window); it is intentionally a no-op here.

use serde_json::{json, Value};

use crate::app::config::AppSettings;

/// The envelopes below hold AppKit handles only on macOS; the panel model is
/// pure so the host can hold it on any platform.
#[cfg(target_os = "macos")]
use objc2::rc::Retained;

/// Filefast's background-run latch (ok, last stdout line).
type FfOut = std::sync::Arc<std::sync::Mutex<Option<(bool, String)>>>;

/// The panels `openTool` maps onto this module.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToolKind {
    #[default]
    PrettyPrint,
    Filefast,
    HealthChecks,
    TextTable,
}

impl ToolKind {
    /// The `config.name` the panel registers under (the `tools` map key).
    pub fn name(self) -> &'static str {
        match self {
            ToolKind::PrettyPrint => "prettyprint",
            ToolKind::Filefast => "filefast",
            ToolKind::HealthChecks => "health-checks",
            ToolKind::TextTable => "text-table",
        }
    }

    pub fn from_name(name: &str) -> Option<ToolKind> {
        match name {
            "prettyprint" => Some(ToolKind::PrettyPrint),
            "filefast" => Some(ToolKind::Filefast),
            "health-checks" | "health" => Some(ToolKind::HealthChecks),
            "text-table" => Some(ToolKind::TextTable),
            _ => None,
        }
    }

    /// Swift's `isToolPanel` names this module owns.
    pub fn is_panel(name: &str) -> bool {
        ToolKind::from_name(name).is_some()
    }
}

/// The resolved `[section]` values the panels need.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolSpec {
    pub script: String,
    pub save_dir: String,
    pub width: f64,
    pub height: f64,
    pub font: String,
    /// Text-table: close the panel (and return focus) after a copy.
    pub close_on_copy: bool,
}

impl ToolSpec {
    fn defaults(kind: ToolKind) -> ToolSpec {
        match kind {
            ToolKind::PrettyPrint => ToolSpec {
                script: String::new(),
                save_dir: "/tmp/".to_string(),
                width: 1000.0,
                height: 600.0,
                font: String::new(),
                close_on_copy: false,
            },
            ToolKind::Filefast => ToolSpec {
                script: String::new(),
                save_dir: "/tmp/filefast".to_string(),
                width: 620.0,
                height: 180.0,
                font: String::new(),
                close_on_copy: false,
            },
            ToolKind::HealthChecks => ToolSpec {
                script: String::new(),
                save_dir: String::new(),
                width: 800.0,
                height: 640.0,
                font: "SF Mono".to_string(),
                close_on_copy: false,
            },
            ToolKind::TextTable => ToolSpec {
                script: String::new(),
                save_dir: "/tmp/text-table".to_string(),
                width: 900.0,
                height: 600.0,
                font: String::new(),
                close_on_copy: true,
            },
        }
    }
}

/// A TOML basic string's value: one pair of surrounding quotes dropped and
/// `\"` / `\\` / `\n` / `\t` unescaped (the filefast `script` is full of
/// `\"`). Bare values pass through trimmed.
fn unquote_basic(v: &str) -> String {
    let Some(inner) = v.strip_prefix('"').and_then(|r| r.strip_suffix('"')) else {
        return v.trim_matches('"').trim().to_string();
    };
    let mut out = String::with_capacity(inner.len());
    let mut it = inner.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(o) => out.push(o),
            None => out.push('\\'),
        }
    }
    out.trim().to_string()
}

/// Resolve a `[section]`'s raw `key = value` pairs (a tiny pure parser; the
/// full codec is the Python `config_text` one and stays off this path).
pub fn section_vars(text: &str, section: &str) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    let mut current = String::new();
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if let Some(rest) = t.strip_prefix('[') {
            current = rest.trim_end_matches(']').trim().to_string();
            continue;
        }
        if current == section {
            if let Some((k, v)) = t.split_once('=') {
                let k = k.trim().to_string();
                out.entry(k).or_insert(unquote_basic(v.trim()));
            }
        }
    }
    out
}

fn tri(v: Option<&String>) -> Option<bool> {
    match v.map(String::as_str) {
        Some("true") | Some("yes") | Some("on") | Some("1") => Some(true),
        Some("false") | Some("no") | Some("off") | Some("0") => Some(false),
        _ => None,
    }
}

impl ToolSpec {
    pub fn load(kind: ToolKind) -> ToolSpec {
        let section = kind.name();
        let mut spec = ToolSpec::defaults(kind);
        let Some(text) = crate::app::config::read_config_text() else {
            return spec;
        };
        let vars = section_vars(&text, section);
        if let Some(v) = vars.get("script") {
            spec.script = v.clone();
        }
        if let Some(v) = vars.get("save-dir") {
            spec.save_dir = v.clone();
        }
        if let Some(w) = vars.get("width").and_then(|s| s.parse::<f64>().ok()) {
            if w > 0.0 {
                spec.width = w;
            }
        }
        if let Some(h) = vars.get("height").and_then(|s| s.parse::<f64>().ok()) {
            if h > 0.0 {
                spec.height = h;
            }
        }
        if let Some(f) = vars.get("font") {
            if !f.is_empty() {
                spec.font = f.clone();
            }
        }
        if let Some(on) = tri(vars.get("close-on-copy")) {
            spec.close_on_copy = on;
        }
        let _ = tri(vars.get("enabled"));
        spec
    }
}

// -- text-table engine (`python3 -m text_table`, file in / file out) ---------

/// The engine format the panel runs: `outlook` yields both outputs the panel
/// offers — the styled HTML table (`output`) and its Markdown twin
/// (`fallback`) — in one pass.
pub const TABLE_ENGINE_FORMAT: &str = "outlook";

/// Where a text-table copy goes (the panel's only two actions).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableTarget {
    /// A real table for Outlook / Teams / Word: HTML + RTF + Markdown text.
    Outlook,
    /// The Markdown table saved as a `.md` file; the clipboard carries the
    /// file (Finder / chat / mail paste it) and the Markdown text (editors).
    MarkdownFile,
}

/// `<save-dir>/table-<stamp>.md` (the text-table Markdown file).
pub fn table_md_path(dir: &str, secs: i64) -> String {
    let abs = crate::app::config::expand_tilde(dir);
    let base = abs.trim_end_matches('/');
    format!("{}/table-{}.md", if base.is_empty() { "/tmp" } else { base }, pretty_stamp(secs))
}

/// One transform: the output, its plain twin and the `--meta` summary.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TableResult {
    pub format: String,
    /// The requested format's text (HTML source for a rich format).
    pub output: String,
    /// The plain-text twin (`--fallback-out`; the output itself for a plain
    /// format) — the clipboard's text flavor and the panel's preview.
    pub fallback: String,
    pub mime: String,
    pub ok: bool,
    pub columns: i64,
    pub rows: i64,
    pub delimiter: String,
    pub warnings: Vec<String>,
}

impl TableResult {
    pub fn is_html(&self) -> bool {
        self.mime == "text/html"
    }

    /// The status line: `4 columns × 12 rows · delimiter tab · 1 short row padded`.
    pub fn summary(&self) -> String {
        if !self.ok {
            return "no table found — paste or type rows (first row = header)".to_string();
        }
        let mut s = format!(
            "{} column{} × {} row{} · delimiter {}",
            self.columns,
            if self.columns == 1 { "" } else { "s" },
            self.rows,
            if self.rows == 1 { "" } else { "s" },
            self.delimiter
        );
        for w in &self.warnings {
            s.push_str(" · ");
            s.push_str(w);
        }
        s
    }

    /// Put the result on the clipboard: a rich format as HTML (+ RTF) + its
    /// plain twin, a plain one as text.
    pub fn copy(&self) -> bool {
        if self.is_html() {
            crate::platform::set_html(&self.output, &self.fallback)
        } else {
            crate::platform::set_text(&self.output)
        }
    }

    /// The Markdown table: the plain twin of a rich run, else the output.
    pub fn markdown(&self) -> &str {
        if self.is_html() { &self.fallback } else { &self.output }
    }
}

/// The `pylib` the engine runs from (the helper's, else the assets').
pub fn text_table_lib() -> String {
    let lib = crate::app::python_helper::PythonHelper::shared().lib_dir();
    if !lib.is_empty() {
        return lib;
    }
    format!("{}/pylib", crate::app::paths::Paths::from_env().asset_dir())
}

/// Run the engine over `input` as `format`: write the input to a temp file,
/// `python3 -m text_table IN OUT --format F --fallback-out FB --meta META`,
/// read the files back. Blocking — call off the main thread.
pub fn run_text_table(input: &str, format: &str) -> Result<TableResult, String> {
    use std::process::{Command, Stdio};
    let lib = text_table_lib();
    let py = crate::app::python_helper::python_bin(&lib)
        .ok_or_else(|| "python 3.11+ not found (set WS_PYTHON)".to_string())?;
    let dir = std::env::temp_dir().join(format!("kitchen-sink-text-table-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let (inp, out, fb, meta) = (dir.join("in.txt"), dir.join("out"), dir.join("fallback.txt"), dir.join("meta.json"));
    std::fs::write(&inp, input).map_err(|e| e.to_string())?;
    let run = Command::new(&py)
        .args(["-B", "-m", "text_table"])
        .arg(&inp)
        .arg(&out)
        .args(["--format", format, "--fallback-out"])
        .arg(&fb)
        .arg("--meta")
        .arg(&meta)
        .current_dir(&lib)
        .env("PYTHONPATH", &lib)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("cannot run {py}: {e}"))?;
    // 0 = a table, 1 = no table (files still written), 2 = usage.
    if !matches!(run.status.code(), Some(0) | Some(1)) {
        let err = String::from_utf8_lossy(&run.stderr).trim().to_string();
        return Err(if err.is_empty() { format!("text_table failed ({})", run.status) } else { err });
    }
    let read = |p: &std::path::Path| std::fs::read_to_string(p).unwrap_or_default();
    let m: Value = serde_json::from_str(&read(&meta)).map_err(|e| format!("bad meta: {e}"))?;
    Ok(TableResult {
        format: m["format"].as_str().unwrap_or(format).to_string(),
        output: read(&out),
        fallback: read(&fb),
        mime: m["mime"].as_str().unwrap_or("text/plain").to_string(),
        ok: m["ok"].as_bool().unwrap_or(false),
        columns: m["columns"].as_i64().unwrap_or(0),
        rows: m["rows"].as_i64().unwrap_or(0),
        delimiter: m["delimiterLabel"].as_str().unwrap_or("none").to_string(),
        warnings: m["warnings"]
            .as_array()
            .map(|a| a.iter().filter_map(|w| w.as_str().map(str::to_string)).collect())
            .unwrap_or_default(),
    })
}

/// The text-table run latch (`None` = idle).
type TtOut = std::sync::Arc<std::sync::Mutex<Option<Result<TableResult, String>>>>;

// -- prettyprint save (`savePrettyPrint` + `prettyFormat` helpers) ----------

/// `prettyFormat`'s extension pick: first non-space char `{`/`[` = json,
/// `<` = xml, anything else (or empty) = txt.
pub fn pretty_ext(contents: &str) -> &'static str {
    match contents.trim_start().chars().next() {
        Some('{') | Some('[') => "json",
        Some('<') => "xml",
        _ => "txt",
    }
}

/// The `prettySaveStamp` formatter: `yyyyMMdd-HHmmss` (UTC; the exact stamp is
/// not part of the contract).
pub fn pretty_stamp(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}{m:02}{d:02}-{:02}{:02}{:02}",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// `savePrettyPrint`'s path: `<save-dir>/prettyprint-<stamp>.<ext>`.
pub fn pretty_save_path(dir: &str, contents: &str, secs: i64) -> String {
    let abs = crate::app::config::expand_tilde(dir);
    let base = abs.trim_end_matches('/');
    let name = format!("prettyprint-{}.{}", pretty_stamp(secs), pretty_ext(contents));
    if base.is_empty() {
        format!("/{name}")
    } else {
        format!("{base}/{name}")
    }
}

/// Write `contents` to the dated path; the handler's side of `savePrettyPrint`.
pub fn save_prettyprint_at(contents: &str, dir: &str, secs: i64) -> Result<String, String> {
    let path = pretty_save_path(dir, contents, secs);
    std::fs::write(&path, contents).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Unix seconds, the clock the panel stamps with.
pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(target_os = "macos")]
pub fn copy_text(text: &str) {
    let _ = crate::platform::set_text(text);
}

/// `PopupWindow.setStatus(_:isError:)` — the status line's text + tone.
#[cfg(target_os = "macos")]
pub fn set_status_label(label: &objc2_app_kit::NSTextField, msg: &str, is_error: bool) {
    use crate::ui::theme::{PopupThemeDefaults, PopupTone};
    use objc2_foundation::NSString;
    label.setStringValue(&NSString::from_str(msg));
    let colors = PopupThemeDefaults::colors();
    let tone = if is_error { PopupTone::Danger } else { PopupTone::Dim };
    label.setTextColor(Some(&colors.tone(tone).to_nscolor()));
}

/// The prettyprint header's two buttons (`w.onHeaderButton` id 10/11).
#[cfg(target_os = "macos")]
mod pp_handler {
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObject};
    use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{NSTextField, NSTextView};
    use objc2_foundation::NSObjectProtocol;

    pub fn as_any<T: objc2::Message + ?Sized>(obj: &T) -> &AnyObject {
        unsafe { &*(obj as *const T as *const AnyObject) }
    }

    pub struct PrettyPrintHandlerIvars {
        pub editor: Retained<NSTextView>,
        pub status: Retained<NSTextField>,
        pub save_dir: String,
    }

    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSPrettyPrintHandler"]
        #[ivars = PrettyPrintHandlerIvars]
        pub struct PrettyPrintHandler;

        impl PrettyPrintHandler {
            /// id 11 — `save file`.
            #[unsafe(method(saveClicked:))]
            fn save_clicked(&self, _sender: Option<&AnyObject>) {
                let contents = self.ivars().editor.string().to_string();
                if contents.is_empty() {
                    return;
                }
                match super::save_prettyprint_at(
                    &contents,
                    &self.ivars().save_dir,
                    super::now_secs(),
                ) {
                    Ok(path) => {
                        super::copy_text(&path);
                        super::set_status_label(&self.ivars().status, &format!("saved: {path}"), false);
                    }
                    Err(e) => super::set_status_label(
                        &self.ivars().status,
                        &format!("save failed: {e}"),
                        true,
                    ),
                }
            }

            /// id 10 — `copy contents`.
            #[unsafe(method(copyClicked:))]
            fn copy_clicked(&self, _sender: Option<&AnyObject>) {
                let contents = self.ivars().editor.string().to_string();
                if contents.is_empty() {
                    return;
                }
                super::copy_text(&contents);
                super::set_status_label(&self.ivars().status, "copied contents", false);
            }
        }

        unsafe impl NSObjectProtocol for PrettyPrintHandler {}
    );

    impl PrettyPrintHandler {
        pub fn new(
            mtm: MainThreadMarker,
            editor: Retained<NSTextView>,
            status: Retained<NSTextField>,
            save_dir: String,
        ) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(PrettyPrintHandlerIvars {
                editor,
                status,
                save_dir,
            });
            unsafe { msg_send![super(this), init] }
        }
    }
}

/// Run the filefast `cmd.script` with `FF_DIR`/`FF_NAME` and the paste contents
/// on stdin (the `save` closure of `openFileFastWindow`); the outcome lands in
/// the latch and [`ToolPanel::poll`] copies the path / dresses the status.
#[cfg(target_os = "macos")]
fn spawn_filefast(
    script: String,
    dir: String,
    name: String,
    contents: String,
    out: FfOut,
    pending: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> bool {
    if script.is_empty() {
        return false;
    }
    if pending.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return false;
    }
    std::thread::spawn(move || {
        let env: Vec<(String, String)> = std::env::vars()
            .chain([
                ("FF_DIR".to_string(), dir),
                ("FF_NAME".to_string(), name),
            ])
            .collect();
        let result = crate::app::process_run::run_process(
            "/bin/bash",
            &["-c".to_string(), script],
            Some(contents.as_bytes()),
            Some(&env),
            true,
        );
        let (ok, last) = match result {
            Ok(o) => (
                o.code == 0,
                o.out.trim().lines().last().unwrap_or("").to_string(),
            ),
            Err(e) => (false, e.to_string()),
        };
        *out.lock().unwrap() = Some((ok, last));
        pending.store(false, std::sync::atomic::Ordering::Relaxed);
    });
    true
}

/// The date-stamped save directory under `save_dir` (local time).
fn filefast_dir_for(save_dir: &str) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let root = if save_dir == "/tmp/" {
        "/tmp/filefast".to_string()
    } else {
        crate::app::config::expand_tilde(&save_dir)
    };
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // yyyy_MM_dd in local time (Swift's DateFormatter).
    let (y, m, d) = {
        let t = secs as libc::time_t;
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        unsafe { libc::localtime_r(&t, &mut tm) };
        (tm.tm_year as i64 + 1900, tm.tm_mon as u32 + 1, tm.tm_mday as u32)
    };
    format!("{root}/{y:04}_{m:02}_{d:02}")
}

/// The Create File panel's height and its input row's y (the pasted-count
/// line sits under the row, the status line at the foot).
#[cfg(target_os = "macos")]
const FF_PANEL_H: f64 = 82.0;
#[cfg(target_os = "macos")]
const FF_ROW_Y: f64 = 40.0;

/// `"3 lines, 120 chars pasted"` (empty for no text).
pub fn pasted_count(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }
    let lines = s.split('\n').count();
    format!("{lines} line{}, {} chars pasted", if lines == 1 { "" } else { "s" }, s.chars().count())
}

/// Everything a tool panel draws, pulled by the owner each frame (the pure
/// half of `test_state` plus the panel's text and status line).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolsSnapshot {
    pub kind: ToolKind,
    pub shown: bool,
    /// The prettyprint editor / filefast paste box / health-checks output.
    pub text: String,
    /// The prettyprint or filefast status line `(message, is_error)`.
    pub status: Option<(String, bool)>,
    /// Health-checks: a run is in flight.
    pub running: bool,
    pub frame: [f64; 4],
    /// Text-table: the preview (the Markdown twin).
    pub preview: String,
}

/// User actions reported by the owner (header buttons, the filefast key
/// monitor, the prettyprint debounce, the health run).
#[derive(Clone, Debug, PartialEq)]
pub enum ToolsEvent {
    /// Order the panel out (`header_click`'s ✕, the filefast Esc, auto-hide).
    Close,
    /// Prettyprint: the editor text settled — capture it (the debounce start).
    Edit(String),
    /// Prettyprint: run the debounce's `format_prettyprint` now.
    Format,
    /// Prettyprint: the header's "save file" button (id 10).
    Save,
    /// Health-checks: (re)start the streaming run.
    HealthRun,
    /// Filefast: the Return / Cmd+Return save.
    FilefastSave,
    /// Text-table: copy the current result to one of the two targets.
    TableCopy(TableTarget),
}

/// One live tool panel (model + window handle; the window only exists on
/// macOS and only after the first [`ToolPanel::show`]).
pub struct ToolPanel {
    kind: ToolKind,
    spec: ToolSpec,
    shown: bool,
    frame: [f64; 4],
    /// Health-checks: the shared stdout buffer + "reader done" latch.
    health_out: std::sync::Arc<std::sync::Mutex<String>>,
    health_done: std::sync::Arc<std::sync::atomic::AtomicBool>,
    health_started: bool,
    /// Prettyprint: the formatting result latch (`None` = idle).
    fmt_out: std::sync::Arc<std::sync::Mutex<Option<Result<Option<String>, String>>>>,
    fmt_pending: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Prettyprint: the debounce (last editor text + when it settled).
    pp_last_seen: String,
    pp_last_change: Option<std::time::Instant>,
    pp_status: Option<(String, bool)>,
    /// Filefast: the script result latch + run latch + auto-hide deadline.
    ff_out: FfOut,
    ff_pending: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ff_status: Option<(String, bool)>,
    ff_hide_at: Option<std::time::Instant>,
    /// Text-table: the run latch + run-in-flight flag, the debounce (last
    /// input + when it settled), the (input, format) of the last run started,
    /// its result, the status line, the clipboard text last pre-filled, and
    /// the close-after-copy deadline.
    tt_out: TtOut,
    tt_pending: std::sync::Arc<std::sync::atomic::AtomicBool>,
    tt_last_seen: String,
    tt_last_change: Option<std::time::Instant>,
    tt_ran: Option<(String, String)>,
    tt_result: Option<TableResult>,
    tt_status: Option<(String, bool)>,
    tt_clip_loaded: String,
    tt_hide_at: Option<std::time::Instant>,
    /// The app to hand focus back to when the panel closes itself (the app
    /// that was frontmost when Hyper+S opened the palette).
    return_pid: Option<i32>,
    #[cfg(target_os = "macos")]
    window: Option<Retained<crate::ui::popup::PopupPanel>>,
    /// Text-table: the read-only result preview under the input.
    #[cfg(target_os = "macos")]
    preview: Option<Retained<objc2_app_kit::NSTextView>>,
    #[cfg(target_os = "macos")]
    editor: Option<Retained<objc2_app_kit::NSTextView>>,
    #[cfg(target_os = "macos")]
    paste: Option<Retained<objc2_app_kit::NSTextView>>,
    #[cfg(target_os = "macos")]
    name_field: Option<Retained<objc2_app_kit::NSTextField>>,
    #[cfg(target_os = "macos")]
    pp_status_label: Option<Retained<objc2_app_kit::NSTextField>>,
    #[cfg(target_os = "macos")]
    ff_status_label: Option<Retained<objc2_app_kit::NSTextField>>,
    #[cfg(target_os = "macos")]
    pp_handler: Option<Retained<pp_handler::PrettyPrintHandler>>,
    /// Filefast's Return/Cmd+Return save guard.
    #[cfg(target_os = "macos")]
    key_monitor: Option<crate::ui::popup::MonitorHandle>,
    click_monitor: Option<crate::ui::popup::MonitorHandle>,
    #[cfg(target_os = "macos")]
    ff_cell: Option<objc2::rc::Retained<objc2_app_kit::NSView>>,
    #[cfg(target_os = "macos")]
    ff_copy_btn: Option<objc2::rc::Retained<objc2_app_kit::NSView>>,
    /// The last header click (`onHeaderButton`), drained by `poll`.
    click: std::rc::Rc<std::cell::Cell<i32>>,
    #[cfg(target_os = "macos")]
    chrome: Option<Retained<crate::ui::chrome::PopupChrome>>,
    #[cfg(target_os = "macos")]
    ff_hint: Option<Retained<objc2_app_kit::NSTextField>>,
    /// Filefast: the "N lines, M chars pasted" line under the paste box.
    #[cfg(target_os = "macos")]
    ff_count: Option<Retained<objc2_app_kit::NSTextField>>,
    #[cfg(target_os = "macos")]
    colors: Option<crate::ui::theme::PopupColors>,
    /// The health text last rendered (ANSI → attributed).
    health_shown: String,
}

impl ToolPanel {
    pub fn new(kind: ToolKind, settings: &AppSettings) -> ToolPanel {
        let mut spec = ToolSpec::load(kind);
        if spec.font.is_empty() {
            spec.font = settings.terminal_font.clone();
        }
        ToolPanel {
            kind,
            spec,
            shown: false,
            frame: [0.0, 0.0, 0.0, 0.0],
            health_out: std::sync::Arc::new(std::sync::Mutex::new(String::new())),
            health_done: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            health_started: false,
            fmt_out: std::sync::Arc::new(std::sync::Mutex::new(None)),
            fmt_pending: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            pp_last_seen: String::new(),
            pp_last_change: None,
            pp_status: None,
            ff_out: std::sync::Arc::new(std::sync::Mutex::new(None)),
            ff_pending: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            ff_status: None,
            ff_hide_at: None,
            tt_out: std::sync::Arc::new(std::sync::Mutex::new(None)),
            tt_pending: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            tt_last_seen: String::new(),
            tt_last_change: None,
            tt_ran: None,
            tt_result: None,
            tt_status: None,
            tt_clip_loaded: String::new(),
            tt_hide_at: None,
            return_pid: None,
            #[cfg(target_os = "macos")]
            window: None,
            #[cfg(target_os = "macos")]
            preview: None,
            #[cfg(target_os = "macos")]
            editor: None,
            #[cfg(target_os = "macos")]
            paste: None,
            #[cfg(target_os = "macos")]
            name_field: None,
            #[cfg(target_os = "macos")]
            pp_status_label: None,
            #[cfg(target_os = "macos")]
            ff_status_label: None,
            #[cfg(target_os = "macos")]
            pp_handler: None,
            #[cfg(target_os = "macos")]
            key_monitor: None,
            click_monitor: None,
            #[cfg(target_os = "macos")]
            ff_cell: None,
            #[cfg(target_os = "macos")]
            ff_copy_btn: None,
            click: std::rc::Rc::new(std::cell::Cell::new(0)),
            #[cfg(target_os = "macos")]
            chrome: None,
            #[cfg(target_os = "macos")]
            ff_hint: None,
            #[cfg(target_os = "macos")]
            ff_count: None,
            #[cfg(target_os = "macos")]
            colors: None,
            health_shown: String::new(),
        }
    }

    pub fn kind(&self) -> ToolKind {
        self.kind
    }

    pub fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    pub fn is_shown(&self) -> bool {
        self.shown
    }

    pub fn has_key(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.window.as_ref().map(|w| w.isKeyWindow()).unwrap_or(false)
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }

    pub fn window_number(&self) -> i64 {
        #[cfg(target_os = "macos")]
        {
            self.window
                .as_ref()
                .map(|w| w.windowNumber() as i64)
                .unwrap_or(0)
        }
        #[cfg(not(target_os = "macos"))]
        {
            0
        }
    }

    pub fn window_level(&self) -> i64 {
        #[cfg(target_os = "macos")]
        {
            self.window.as_ref().map(|w| w.level() as i64).unwrap_or(0)
        }
        #[cfg(not(target_os = "macos"))]
        {
            0
        }
    }

    pub fn frame(&self) -> [f64; 4] {
        self.frame
    }

    /// Build (once) the panel and order it front with the keyboard, without
    /// activating the app (`raiseToolPanel` + `reclaimToolKey`).
    #[cfg(target_os = "macos")]
    pub fn show(&mut self, mtm: objc2::MainThreadMarker) {
        self.show_macos(mtm);
        // A fresh raise cancels any pending filefast auto-hide.
        self.ff_hide_at = None;
        self.shown = true;
    }

    /// Order the panel out (`unregisterSubWindow`'s window half).
    pub fn hide(&mut self) {
        #[cfg(target_os = "macos")]
        if let Some(w) = &self.window {
            w.orderOut(None);
        }
        self.shown = false;
    }

    /// The main-thread tick: apply background results (the prettyprint
    /// debounce + formatting, the health stream, the filefast outcome). Cheap;
    /// safe to call every drain.
    pub fn poll(&mut self) {
        #[cfg(target_os = "macos")]
        self.poll_macos();
        if self.kind == ToolKind::TextTable {
            self.poll_table();
        }
    }

    /// The `state.tools.<name>` document (the panel's `testState` + wid/level).
    pub fn test_state(&self) -> Value {
        let mut st = json!({
            "shown": self.shown,
            "key": self.has_key(),
            "wid": self.window_number(),
            "level": self.window_level(),
            "frame": [
                self.frame[0] as i64,
                self.frame[1] as i64,
                self.frame[2] as i64,
                self.frame[3] as i64,
            ],
        });
        if self.kind == ToolKind::HealthChecks {
            if let Some(o) = st.as_object_mut() {
                o.insert(
                    "running".into(),
                    json!(!self.health_done.load(std::sync::atomic::Ordering::Relaxed)),
                );
            }
        }
        if self.kind == ToolKind::TextTable {
            if let Some(o) = st.as_object_mut() {
                let r = self.tt_result.clone().unwrap_or_default();
                o.insert("ok".into(), json!(r.ok));
                o.insert("columns".into(), json!(r.columns));
                o.insert("rows".into(), json!(r.rows));
                o.insert("delimiter".into(), json!(r.delimiter));
                o.insert("running".into(), json!(self.tt_pending.load(std::sync::atomic::Ordering::Relaxed)));
            }
        }
        st
    }

    // -- text-table ----------------------------------------------------------

    /// Hand focus back to `pid` when the panel closes itself (`None` = stay).
    pub fn set_return_pid(&mut self, pid: Option<i32>) {
        self.return_pid = pid;
    }

    /// The input text: the AppKit editor, else the model's mirror.
    fn table_input(&self) -> String {
        #[cfg(target_os = "macos")]
        if let Some(editor) = &self.editor {
            return editor.string().to_string();
        }
        self.tt_last_seen.clone()
    }

    fn spawn_table_run(&mut self, input: String, format: String) {
        if self.tt_pending.swap(true, std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        self.tt_ran = Some((input.clone(), format.clone()));
        let out = self.tt_out.clone();
        let pending = self.tt_pending.clone();
        std::thread::spawn(move || {
            let r = run_text_table(&input, &format);
            *out.lock().unwrap() = Some(r);
            pending.store(false, std::sync::atomic::Ordering::Relaxed);
        });
    }

    /// The debounce + run loop (0.25 s after the input settles, at once after
    /// opening), applying finished runs and the close-after-copy.
    fn poll_table(&mut self) {
        let input = self.table_input();
        if input != self.tt_last_seen {
            self.tt_last_seen = input.clone();
            self.tt_last_change = Some(std::time::Instant::now());
        }
        let want = (input.clone(), TABLE_ENGINE_FORMAT.to_string());
        let settled = self
            .tt_last_change
            .map(|t| t.elapsed() >= std::time::Duration::from_millis(250))
            .unwrap_or(true);
        if settled
            && self.tt_ran.as_ref() != Some(&want)
            && !self.tt_pending.load(std::sync::atomic::Ordering::Relaxed)
        {
            if input.trim().is_empty() {
                self.tt_ran = Some(want);
                self.tt_result = None;
                self.apply_table_preview();
                self.set_tt_status(Some((
                    "paste or type rows — the first row is the header".to_string(),
                    false,
                )));
            } else {
                self.spawn_table_run(want.0, want.1);
            }
        }
        let done = self.tt_out.lock().unwrap().take();
        match done {
            Some(Ok(r)) => {
                self.set_tt_status(Some((r.summary(), !r.ok)));
                self.tt_result = Some(r);
                self.apply_table_preview();
            }
            Some(Err(e)) => self.set_tt_status(Some((e, true))),
            None => {}
        }
        if let Some(at) = self.tt_hide_at {
            if std::time::Instant::now() >= at {
                self.tt_hide_at = None;
                self.hand_back();
            }
        }
    }

    /// Copy the result for the current input to `target` (running the
    /// engine inline when the last run is stale) and, with `close-on-copy`,
    /// close shortly after so the status is readable.
    pub fn copy_table(&mut self, target: TableTarget) -> bool {
        let want = (self.table_input(), TABLE_ENGINE_FORMAT.to_string());
        let fresh = self.tt_ran.as_ref() == Some(&want)
            && !self.tt_pending.load(std::sync::atomic::Ordering::Relaxed)
            && self.tt_result.is_some();
        if !fresh && !want.0.trim().is_empty() {
            match run_text_table(&want.0, &want.1) {
                Ok(r) => {
                    self.tt_ran = Some(want.clone());
                    self.tt_result = Some(r);
                    self.apply_table_preview();
                }
                Err(e) => {
                    self.set_tt_status(Some((e, true)));
                    return false;
                }
            }
        }
        let Some(r) = self.tt_result.clone().filter(|r| r.ok) else {
            crate::platform::bell();
            self.set_tt_status(Some(("nothing to copy — no table found".to_string(), true)));
            return false;
        };
        let msg = match target {
            TableTarget::Outlook => {
                if !r.copy() {
                    self.set_tt_status(Some(("clipboard write failed".to_string(), true)));
                    return false;
                }
                format!("Copied table for Outlook ({} × {}) — paste with ⌘V", r.columns, r.rows)
            }
            TableTarget::MarkdownFile => {
                let path = table_md_path(&self.spec.save_dir, now_secs());
                let written = std::path::Path::new(&path)
                    .parent()
                    .map_or(Ok(()), std::fs::create_dir_all)
                    .and_then(|_| std::fs::write(&path, r.markdown()));
                if let Err(e) = written {
                    self.set_tt_status(Some((format!("could not write {path}: {e}"), true)));
                    return false;
                }
                if !crate::platform::set_file_and_text(&path, r.markdown()) {
                    self.set_tt_status(Some(("clipboard write failed".to_string(), true)));
                    return false;
                }
                format!("Saved {path} — file + Markdown copied, paste with ⌘V")
            }
        };
        self.set_tt_status(Some((msg, false)));
        if self.spec.close_on_copy {
            self.tt_hide_at = Some(std::time::Instant::now() + std::time::Duration::from_millis(700));
        }
        true
    }

    /// The panel closed itself: order out and give focus back to the app the
    /// user came from (else the shared window would be raised in its place).
    fn hand_back(&mut self) {
        self.hide();
        if let Some(pid) = self.return_pid.take() {
            let _ = crate::platform::activate_pid(pid);
        }
    }

    fn set_tt_status(&mut self, status: Option<(String, bool)>) {
        #[cfg(target_os = "macos")]
        if let Some(label) = &self.pp_status_label {
            match &status {
                Some((msg, is_err)) => set_status_label(label, msg, *is_err),
                None => label.setStringValue(&objc2_foundation::NSString::from_str("")),
            }
        }
        self.tt_status = status;
    }

    /// Show the result's plain twin in the preview.
    fn apply_table_preview(&mut self) {
        #[cfg(target_os = "macos")]
        if let Some(preview) = &self.preview {
            let text = self.tt_result.as_ref().map(|r| r.fallback.as_str()).unwrap_or("");
            if preview.string().to_string() != text {
                preview.setString(&objc2_foundation::NSString::from_str(text));
            }
        }
    }

    /// The header buttons: the panel's two copy targets.
    #[cfg(target_os = "macos")]
    fn table_buttons(&self) -> Vec<(String, i32)> {
        vec![("Copy table for Outlook".into(), 10), ("Copy Markdown file".into(), 11)]
    }

    /// `openFileFastWindow`'s save: run `cmd.script` with the paste contents
    /// on stdin and `FF_DIR`/`FF_NAME` in the env (the completion lands in
    /// [`Self::poll`]).
    pub fn save_filefast(&mut self) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.save_filefast_macos()
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }

    #[cfg(target_os = "macos")]
    fn save_filefast_macos(&mut self) -> bool {
        let Some(name_field) = self.name_field.clone() else {
            return false;
        };
        let Some(paste) = self.paste.clone() else {
            return false;
        };
        let name = name_field.stringValue().to_string().trim().to_string();
        let contents = paste.string().to_string();
        if name.is_empty() || name.contains('/') {
            crate::platform::bell();
            if let Some(w) = &self.window {
                let responder: &objc2_app_kit::NSResponder = &**name_field;
                w.makeFirstResponder(Some(responder));
            }
            return false;
        }
        if contents.is_empty() {
            crate::platform::bell();
            if let Some(w) = &self.window {
                let responder: &objc2_app_kit::NSResponder = &**paste;
                w.makeFirstResponder(Some(responder));
            }
            return false;
        }
        spawn_filefast(
            self.spec.script.clone(),
            self.filefast_dir(),
            name,
            contents,
            self.ff_out.clone(),
            self.ff_pending.clone(),
        )
    }

    /// The `openFileFastWindow` date-stamped root directory.
    pub fn filefast_dir(&self) -> String {
        filefast_dir_for(&self.spec.save_dir)
    }

    /// `prettyFormat` — jq for JSON, `xmllint --format -` for XML.
    pub fn format_text(raw: &str) -> Result<Option<String>, String> {
        let t = raw.trim();
        let Some(first) = t.chars().next() else {
            return Ok(None);
        };
        let (exe, args): (&str, Vec<String>) = if first == '{' || first == '[' {
            let exe = ["/opt/homebrew/bin/jq", "/usr/local/bin/jq", "jq"]
                .iter()
                .find(|p| !p.contains('/') || std::path::Path::new(p).exists())
                .copied()
                .unwrap_or("jq");
            (exe, vec![".".to_string()])
        } else if first == '<' {
            let exe = ["/usr/bin/xmllint", "/opt/homebrew/bin/xmllint"]
                .iter()
                .find(|p| std::path::Path::new(p).exists())
                .copied()
                .unwrap_or("/usr/bin/xmllint");
            (exe, vec!["--format".to_string(), "-".to_string()])
        } else {
            return Ok(None);
        };
        match crate::app::process_run::run_process(exe, &args, Some(raw.as_bytes()), None, false) {
            Ok(o) => {
                let err = o.err.trim();
                if !err.is_empty() {
                    return Err(err.to_string());
                }
                if o.out.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(o.out))
                }
            }
            Err(e) => Err(e.to_string()),
        }
    }

    /// Trigger the debounced auto-format of the prettyprint editor.
    pub fn format_prettyprint(&mut self) {
        if self.kind != ToolKind::PrettyPrint {
            return;
        }
        #[cfg(target_os = "macos")]
        {
            let Some(editor) = &self.editor else { return };
            let raw = editor.string().to_string();
            if raw.trim().is_empty() {
                return;
            }
            let out = self.fmt_out.clone();
            let pending = self.fmt_pending.clone();
            if pending.swap(true, std::sync::atomic::Ordering::Relaxed) {
                return;
            }
            std::thread::spawn(move || {
                let result = ToolPanel::format_text(&raw);
                *out.lock().unwrap() = Some(result);
                pending.store(false, std::sync::atomic::Ordering::Relaxed);
            });
        }
    }

    /// Start the health-checks script streaming into the buffer (only once
    /// while a run is live; a fresh `show` after completion re-runs it). An
    /// empty script still shows the panel and reports `(exit 0)`.
    pub fn run_health(&mut self) {
        if self.kind != ToolKind::HealthChecks {
            return;
        }
        if self.health_started && !self.health_done.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        self.health_out.lock().unwrap().clear();
        self.health_done
            .store(false, std::sync::atomic::Ordering::Relaxed);
        self.health_started = true;
        let script = crate::app::config::expand_tilde(&self.spec.script);
        let out = self.health_out.clone();
        let done = self.health_done.clone();
        std::thread::spawn(move || {
            use std::io::Read;
            use std::process::{Command, Stdio};
            if script.trim().is_empty() {
                out.lock().unwrap().push_str("(exit 0)\n");
                done.store(true, std::sync::atomic::Ordering::Relaxed);
                return;
            }
            #[cfg(unix)]
            let mut cmd = {
                let mut cmd = Command::new("/bin/bash");
                cmd.arg("-c")
                    .arg(&script)
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped());
                cmd
            };
            #[cfg(windows)]
            let mut cmd = {
                let mut cmd = Command::new("cmd");
                cmd.arg("/C")
                    .arg(&script)
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped());
                cmd
            };
            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    out.lock().unwrap().push_str(&format!("cannot run {script}: {e}\n"));
                    done.store(true, std::sync::atomic::Ordering::Relaxed);
                    return;
                }
            };
            let stdout = child.stdout.take();
            let stderr = child.stderr.take();
            let out_o = out.clone();
            let read_out = std::thread::spawn(move || {
                if let Some(mut o) = stdout {
                    let mut chunk = [0u8; 4096];
                    while let Ok(n) = o.read(&mut chunk) {
                        if n == 0 {
                            break;
                        }
                        out_o.lock().unwrap().push_str(&String::from_utf8_lossy(&chunk[..n]));
                    }
                }
            });
            let out_e = out.clone();
            let read_err = std::thread::spawn(move || {
                if let Some(mut e) = stderr {
                    let mut chunk = [0u8; 4096];
                    while let Ok(n) = e.read(&mut chunk) {
                        if n == 0 {
                            break;
                        }
                        out_e.lock().unwrap().push_str(&String::from_utf8_lossy(&chunk[..n]));
                    }
                }
            });
            let code = child.wait().map(|s| s.code().unwrap_or(-1)).unwrap_or(-1);
            let _ = read_out.join();
            let _ = read_err.join();
            out.lock().unwrap().push_str(&format!("\n(exit {code})\n"));
            done.store(true, std::sync::atomic::Ordering::Relaxed);
        });
    }

    /// The health buffer / the prettyprint editor text / the filefast paste
    /// text (parity probes).
    pub fn buffer_text(&self) -> String {
        match self.kind {
            ToolKind::PrettyPrint => {
                #[cfg(target_os = "macos")]
                if let Some(editor) = &self.editor {
                    return editor.string().to_string();
                }
                self.pp_last_seen.clone()
            }
            ToolKind::Filefast => {
                #[cfg(target_os = "macos")]
                if let Some(paste) = &self.paste {
                    return paste.string().to_string();
                }
                String::new()
            }
            ToolKind::HealthChecks => self.health_out.lock().unwrap().clone(),
            ToolKind::TextTable => self.table_input(),
        }
    }

    /// A header click: ✕ hides, the icon pops the app menu, 10/11 are the
    /// panel's buttons (save / copy, copy output / copy config path).
    #[cfg(target_os = "macos")]
    fn header_click(&mut self, id: i32) {
        use crate::ui::chrome::{HEADER_CLOSE_ID, HEADER_ICON_ID};
        if self.kind == ToolKind::TextTable {
            match id {
                HEADER_CLOSE_ID => self.hand_back(),
                10 => {
                    self.copy_table(TableTarget::Outlook);
                }
                11 => {
                    self.copy_table(TableTarget::MarkdownFile);
                }
                HEADER_ICON_ID => {
                    if let Some(ch) = &self.chrome {
                        let r = ch.icon_button_rect();
                        crate::app::menu::pop_status_menu(ch, r.x, r.max_y() + 4.0);
                    }
                }
                _ => {}
            }
            if (id == 10 || id == 11) && self.chrome.is_some() {
                if let Some(ch) = &self.chrome {
                    ch.flash(id);
                }
            }
            return;
        }
        match id {
            HEADER_CLOSE_ID => self.hide(),
            HEADER_ICON_ID => {
                if let Some(ch) = &self.chrome {
                    let r = ch.icon_button_rect();
                    crate::app::menu::pop_status_menu(ch, r.x, r.max_y() + 4.0);
                }
            }
            10 | 11 => {
                match (self.kind, id) {
                    (ToolKind::PrettyPrint, 10) => {
                        if let Some(h) = &self.pp_handler {
                            let _: () = unsafe { objc2::msg_send![&**h, saveClicked: std::ptr::null::<objc2::runtime::AnyObject>()] };
                        }
                    }
                    (ToolKind::PrettyPrint, _) => {
                        if let Some(h) = &self.pp_handler {
                            let _: () = unsafe { objc2::msg_send![&**h, copyClicked: std::ptr::null::<objc2::runtime::AnyObject>()] };
                        }
                    }
                    (ToolKind::HealthChecks, 10) => copy_text(&self.health_out.lock().unwrap()),
                    (ToolKind::HealthChecks, _) => {
                        copy_text(&crate::app::paths::Paths::from_env().commands_conf_path());
                    }
                    _ => {}
                }
                if let Some(ch) = &self.chrome {
                    ch.flash(id);
                }
            }
            _ => {}
        }
    }

    #[cfg(target_os = "macos")]
    fn set_pp_status(&mut self, status: Option<(String, bool)>) {
        use objc2_foundation::NSString;
        self.pp_status = status.clone();
        if let Some(label) = &self.pp_status_label {
            match &status {
                Some((msg, is_err)) => set_status_label(label, msg, *is_err),
                None => label.setStringValue(&NSString::from_str("")),
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn set_ff_status(&mut self, status: Option<(String, bool)>) {
        use objc2_foundation::NSString;
        self.ff_status = status.clone();
        if let Some(label) = &self.ff_status_label {
            match &status {
                Some((msg, is_err)) => set_status_label(label, msg, *is_err),
                None => label.setStringValue(&NSString::from_str("")),
            }
        }
    }
}

impl ToolPanel {
    /// The panel's drawable state (the pure half of `test_state`).
    pub fn snapshot(&self) -> ToolsSnapshot {
        let (status, running) = match self.kind {
            ToolKind::PrettyPrint => (self.pp_status.clone(), false),
            ToolKind::Filefast => (self.ff_status.clone(), false),
            ToolKind::HealthChecks => (
                None,
                !self.health_done.load(std::sync::atomic::Ordering::Relaxed),
            ),
            ToolKind::TextTable => (
                self.tt_status.clone(),
                self.tt_pending.load(std::sync::atomic::Ordering::Relaxed),
            ),
        };
        ToolsSnapshot {
            kind: self.kind,
            shown: self.shown,
            text: self.buffer_text(),
            status,
            running,
            frame: self.frame,
            preview: self.tt_result.as_ref().map(|r| r.fallback.clone()).unwrap_or_default(),
        }
    }

    /// Apply a user action — the same model calls the AppKit half makes inline.
    pub fn handle(&mut self, ev: ToolsEvent) {
        match ev {
            ToolsEvent::Close => self.hide(),
            // `onEditorTextChange`: capture the edit and restart the debounce
            // (`poll_macos`'s first half; the format lands on a later tick).
            ToolsEvent::Edit(text) => {
                if self.kind == ToolKind::PrettyPrint && text != self.pp_last_seen {
                    self.pp_last_seen = text;
                    self.pp_last_change = Some(std::time::Instant::now());
                } else if self.kind == ToolKind::TextTable && text != self.tt_last_seen {
                    // The frontend owns the buffer; `poll_table` picks it up.
                    self.tt_last_seen = text;
                    self.tt_last_change = Some(std::time::Instant::now());
                }
            }
            ToolsEvent::TableCopy(target) => {
                self.copy_table(target);
            }
            ToolsEvent::Format => self.format_prettyprint(),
            // `pp_handler::save_clicked`, but the contents come from the model
            // (the AppKit half reads its own editor).
            ToolsEvent::Save => {
                let contents = self.buffer_text();
                if contents.is_empty() {
                    return;
                }
                self.pp_status =
                    Some(match save_prettyprint_at(&contents, &self.spec.save_dir, now_secs()) {
                        Ok(path) => (format!("saved: {path}"), false),
                        Err(e) => (format!("save failed: {e}"), true),
                    });
            }
            ToolsEvent::HealthRun => self.run_health(),
            ToolsEvent::FilefastSave => {
                self.save_filefast();
            }
        }
    }
}

/// `yyyy-mm-dd` from days since the epoch (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

impl ToolPanel {
    #[cfg(target_os = "macos")]
    fn show_macos(&mut self, mtm: objc2::MainThreadMarker) {
        use crate::ui::popup::{CardFrame, PopupConfig, PopupPanel};
        use objc2::MainThreadOnly;
        use objc2_app_kit::{
            NSAutoresizingMaskOptions, NSButton, NSFont, NSScreen, NSScrollView, NSTextField,
            NSTextView, NSView,
        };
        use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

        if self.window.is_none() {
            let colors = crate::ui::theme::section_colors(self.kind.name());
            let filefast = self.kind == ToolKind::Filefast;
            let w = self.spec.width.max(420.0);
            // Filefast: the input row, the pasted-count line under the
            // paste box, and the status line at the foot.
            let h = if filefast { FF_PANEL_H } else { self.spec.height.max(220.0) };
            let cfg = PopupConfig {
                name: self.kind.name().to_string(),
                frame: CardFrame { x: 0.0, y: 0.0, width: w, height: h },
                tool_panel: true,
                floating: true,
                enable_resize: !filefast,
                ..PopupConfig::default()
            };
            let panel = PopupPanel::create(mtm, &cfg);
            panel.setTitle(&NSString::from_str(self.kind.name()));
            if filefast {
                panel.setContentMinSize(NSSize::new(320.0, FF_PANEL_H));
            }
            let assets = crate::app::paths::Paths::from_env().asset_dir().to_string();
            let icon = match self.kind {
                ToolKind::HealthChecks => Some(crate::ui::style::glyph_icon(
                    "heart.fill",
                    "♥",
                    crate::ui::theme::Rgba::new(1.0, 0.23, 0.19, 0.9),
                    22.0,
                )),
                _ => crate::ui::style::image_file(&format!("{assets}/notes_icon.png")),
            };
            let (meta, buttons): (&str, Vec<(String, i32)>) = match self.kind {
                ToolKind::PrettyPrint => (
                    "paste JSON or XML below — auto-formats",
                    vec![("save file".into(), 10), ("copy contents".into(), 11)],
                ),
                ToolKind::HealthChecks => ("", vec![("copy output".into(), 10), ("copy config path".into(), 11)]),
                ToolKind::Filefast => ("", Vec::new()),
                ToolKind::TextTable => (
                    "rows from OCR or paste · first row = header · ⌘↩ Outlook · ⌘⇧↩ Markdown file",
                    self.table_buttons(),
                ),
            };
            let frame = crate::ui::panel_frame::build(
                mtm,
                w,
                h,
                crate::ui::panel_frame::PanelSpec {
                    colors,
                    header_color: Some(crate::ui::theme::section_header_color(self.kind.name())),
                    title: None,
                    meta,
                    buttons,
                    icon,
                    tint_alpha: 0.78,
                },
            );
            let content = frame.content.clone();
            if filefast {
                // No header: the card is one row (name box + paste box).
                frame.chrome.setHidden(true);
                content.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(w, h)));
            }
            {
                let click = self.click.clone();
                frame.chrome.set_on_click(Box::new(move |id| click.set(id)));
            }
            self.chrome = Some(frame.chrome.clone());
            let ch = content.frame().size.height;

            if filefast {
                let fw = (w * 0.42).round();
                let (name_box, field) = crate::ui::field::search_box(mtm, &colors, 13.0, "file name", 6.0, 28.0);
                name_box.setFrame(NSRect::new(NSPoint::new(10.0, FF_ROW_Y), NSSize::new(fw - 10.0, 28.0)));
                content.addSubview(&name_box);
                self.name_field = Some(field);
                let px = fw + 8.0;
                let btn_w = 28.0;
                let pw = w - px - 10.0 - btn_w - 6.0;
                // The border/fill live on a plain container (as `search_box`
                // does); layer props set on the NSScrollView itself never drew.
                let cell = NSView::initWithFrame(
                    NSView::alloc(mtm),
                    NSRect::new(NSPoint::new(px, FF_ROW_Y), NSSize::new(pw, 28.0)),
                );
                cell.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
                cell.setWantsLayer(true);
                if let Some(l) = cell.layer() {
                    l.setBackgroundColor(Some(&crate::ui::style::input_fill(&colors).to_nscolor().CGColor()));
                    l.setCornerRadius(6.0);
                    l.setBorderWidth(1.0);
                    l.setBorderColor(Some(&crate::ui::style::input_stroke(&colors).to_nscolor().CGColor()));
                }
                // Under the transparent scroll (a sibling, not a text-view
                // subview, which never drew): clicks reach the text view.
                let hint = NSTextField::labelWithString(&NSString::from_str("paste output"), mtm);
                hint.setFont(Some(&NSFont::systemFontOfSize(13.0)));
                hint.setTextColor(Some(&colors.dim.to_nscolor()));
                hint.setFrame(NSRect::new(NSPoint::new(6.0, 5.0), NSSize::new(pw - 20.0, 18.0)));
                cell.addSubview(&hint);
                let scroll = NSScrollView::new(mtm);
                scroll.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(pw, 28.0)));
                scroll.setAutoresizingMask(
                    NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
                );
                scroll.setHasVerticalScroller(false);
                scroll.setDrawsBackground(false);
                cell.addSubview(&scroll);
                let text = NSTextView::new(mtm);
                text.setRichText(false);
                text.setDrawsBackground(false);
                text.setFont(Some(&NSFont::systemFontOfSize(13.0)));
                // The pasted text stays invisible; the hint reports it.
                text.setTextColor(Some(&objc2_app_kit::NSColor::clearColor()));
                text.setInsertionPointColor(Some(&colors.dim.to_nscolor()));
                text.setTextContainerInset(NSSize::new(6.0, 6.0));
                text.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(pw, 28.0)));
                // An empty text view otherwise shrinks to zero height and
                // a click on the cell hits nothing (never focusing it).
                text.setMinSize(NSSize::new(0.0, 28.0));
                text.setMaxSize(NSSize::new(f64::MAX, f64::MAX));
                text.setVerticallyResizable(true);
                text.setHorizontallyResizable(false);
                text.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
                scroll.setDocumentView(Some(&text));
                content.addSubview(&cell);
                self.ff_cell = Some(cell.clone());
                // Tiny "copy file path" button right of the paste box.
                let btn = NSView::initWithFrame(
                    NSView::alloc(mtm),
                    NSRect::new(NSPoint::new(px + pw + 6.0, FF_ROW_Y), NSSize::new(btn_w, 28.0)),
                );
                btn.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
                btn.setWantsLayer(true);
                if let Some(l) = btn.layer() {
                    l.setBackgroundColor(Some(&crate::ui::style::input_fill(&colors).to_nscolor().CGColor()));
                    l.setCornerRadius(6.0);
                    l.setBorderWidth(1.0);
                    l.setBorderColor(Some(&crate::ui::style::input_stroke(&colors).to_nscolor().CGColor()));
                }
                let glyph = crate::ui::style::glyph_icon("doc.on.doc", "⧉", colors.dim, 22.0);
                let iv = objc2_app_kit::NSImageView::imageViewWithImage(&glyph, mtm);
                iv.setFrame(NSRect::new(NSPoint::new(3.0, 3.0), NSSize::new(22.0, 22.0)));
                btn.addSubview(&iv);
                btn.setToolTip(Some(&NSString::from_str("Save the pasted text to the file and copy its path (same as Return)")));
                content.addSubview(&btn);
                self.ff_copy_btn = Some(btn);
                self.paste = Some(text.clone());
                self.ff_hint = Some(hint.clone());
                // The pasted count, right under the paste box.
                let count = NSTextField::labelWithString(&NSString::from_str(""), mtm);
                count.setFont(Some(&NSFont::systemFontOfSize(11.0)));
                count.setTextColor(Some(&colors.dim.to_nscolor()));
                count.setFrame(NSRect::new(NSPoint::new(px + 6.0, FF_ROW_Y - 17.0), NSSize::new(pw - 6.0, 15.0)));
                count.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
                content.addSubview(&count);
                self.ff_count = Some(count);
                // Status: what the save did (written path / error).
                let status = NSTextField::labelWithString(&NSString::from_str(""), mtm);
                status.setFont(Some(&NSFont::systemFontOfSize(11.0)));
                status.setTextColor(Some(&colors.dim.to_nscolor()));
                status.setFrame(NSRect::new(NSPoint::new(10.0, 4.0), NSSize::new(w - 20.0, 15.0)));
                status.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
                content.addSubview(&status);
                self.ff_status_label = Some(status);
            } else if self.kind == ToolKind::TextTable {
                // Input (top, editable) over the preview (bottom, read-only,
                // the plain twin of the result), status line at the foot.
                let status_h = 18.0;
                let area = (ch - status_h).max(0.0);
                let gap = 1.0;
                let preview_h = (area * 0.55).round();
                let input_h = (area - preview_h - gap).max(0.0);
                let font = if self.spec.font.is_empty() {
                    NSFont::monospacedSystemFontOfSize_weight(13.0, unsafe { objc2_app_kit::NSFontWeightRegular })
                } else {
                    NSFont::fontWithName_size(&NSString::from_str(&self.spec.font), 13.0).unwrap_or_else(|| {
                        NSFont::monospacedSystemFontOfSize_weight(13.0, unsafe { objc2_app_kit::NSFontWeightRegular })
                    })
                };
                let area_view = |y: f64, h: f64, editable: bool, wrap: bool, mask: NSAutoresizingMaskOptions| {
                    let scroll = NSScrollView::new(mtm);
                    scroll.setFrame(NSRect::new(NSPoint::new(0.0, y), NSSize::new(w, h)));
                    scroll.setAutoresizingMask(mask);
                    scroll.setHasVerticalScroller(true);
                    scroll.setHasHorizontalScroller(!wrap);
                    scroll.setAutohidesScrollers(true);
                    scroll.setDrawsBackground(false);
                    let text = NSTextView::new(mtm);
                    text.setEditable(editable);
                    text.setSelectable(true);
                    text.setRichText(false);
                    text.setDrawsBackground(false);
                    text.setTextColor(Some(&(if editable { colors.text } else { colors.dim }).to_nscolor()));
                    text.setInsertionPointColor(Some(&colors.accent_on().to_nscolor()));
                    text.setFont(Some(&font));
                    text.setTextContainerInset(NSSize::new(14.0, 10.0));
                    text.setVerticallyResizable(true);
                    text.setHorizontallyResizable(!wrap);
                    text.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
                    if let Some(tc) = unsafe { text.textContainer() } {
                        tc.setWidthTracksTextView(wrap);
                        if !wrap {
                            // A table preview must not wrap its rows.
                            tc.setContainerSize(NSSize::new(1.0e7, 1.0e7));
                        }
                    }
                    unsafe {
                        text.setAutomaticQuoteSubstitutionEnabled(false);
                        text.setAutomaticDashSubstitutionEnabled(false);
                        text.setAutomaticTextReplacementEnabled(false);
                    }
                    text.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(w, h)));
                    scroll.setDocumentView(Some(&text));
                    content.addSubview(&scroll);
                    text
                };
                let flex_h = NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable;
                let preview = area_view(
                    status_h,
                    preview_h,
                    false,
                    false,
                    flex_h | NSAutoresizingMaskOptions::ViewMaxYMargin,
                );
                let divider = NSView::new(mtm);
                divider.setFrame(NSRect::new(NSPoint::new(0.0, status_h + preview_h), NSSize::new(w, gap)));
                divider.setAutoresizingMask(
                    NSAutoresizingMaskOptions::ViewWidthSizable
                        | NSAutoresizingMaskOptions::ViewMinYMargin
                        | NSAutoresizingMaskOptions::ViewMaxYMargin,
                );
                divider.setWantsLayer(true);
                if let Some(l) = divider.layer() {
                    l.setBackgroundColor(Some(&crate::ui::style::input_stroke(&colors).to_nscolor().CGColor()));
                }
                content.addSubview(&divider);
                let input = area_view(
                    status_h + preview_h + gap,
                    input_h,
                    true,
                    false,
                    flex_h | NSAutoresizingMaskOptions::ViewMinYMargin,
                );
                self.editor = Some(input);
                self.preview = Some(preview);
                let status = NSTextField::labelWithString(&NSString::from_str(""), mtm);
                status.setFont(Some(&NSFont::systemFontOfSize(11.0)));
                status.setTextColor(Some(&colors.dim.to_nscolor()));
                status.setFrame(NSRect::new(NSPoint::new(14.0, 1.0), NSSize::new(w - 28.0, 16.0)));
                status.setAutoresizingMask(
                    NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMaxYMargin,
                );
                content.addSubview(&status);
                self.pp_status_label = Some(status);

                // Esc closes (rule 6), Cmd+Return copies the Outlook table,
                // Cmd+Shift+Return the Markdown file; everything else (edit
                // shortcuts) passes through.
                let panel_ref = panel.clone();
                let click = self.click.clone();
                let monitor = crate::ui::popup::install_local_monitor(mtm, move |event| {
                    if !panel_ref.isKeyWindow() {
                        return false;
                    }
                    let key = crate::ui::popup::key_input_from_event(event);
                    if key.key_code == crate::ui::popup::KEY_ESC && !key.cmd && !key.ctrl {
                        click.set(crate::ui::chrome::HEADER_CLOSE_ID);
                        return true;
                    }
                    if key.cmd && (key.key_code == crate::ui::popup::KEY_RETURN || key.key_code == 76) {
                        click.set(if key.shift { 11 } else { 10 });
                        return true;
                    }
                    false
                });
                self.key_monitor = Some(monitor);
            } else {
                let status_h = if self.kind == ToolKind::PrettyPrint { 18.0 } else { 0.0 };
                let scroll = NSScrollView::new(mtm);
                scroll.setFrame(NSRect::new(NSPoint::new(0.0, status_h), NSSize::new(w, (ch - status_h).max(0.0))));
                scroll.setAutoresizingMask(
                    NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
                );
                scroll.setHasVerticalScroller(true);
                scroll.setAutohidesScrollers(true);
                scroll.setDrawsBackground(false);
                let text = NSTextView::new(mtm);
                text.setEditable(self.kind == ToolKind::PrettyPrint);
                text.setSelectable(true);
                text.setRichText(self.kind == ToolKind::HealthChecks);
                text.setDrawsBackground(false);
                text.setTextColor(Some(&colors.text.to_nscolor()));
                text.setInsertionPointColor(Some(&colors.accent_on().to_nscolor()));
                let size = 13.0;
                let font = if self.spec.font.is_empty() {
                    NSFont::monospacedSystemFontOfSize_weight(size, unsafe { objc2_app_kit::NSFontWeightRegular })
                } else {
                    NSFont::fontWithName_size(&NSString::from_str(&self.spec.font), size).unwrap_or_else(|| {
                        NSFont::monospacedSystemFontOfSize_weight(size, unsafe { objc2_app_kit::NSFontWeightRegular })
                    })
                };
                text.setFont(Some(&font));
                text.setTextContainerInset(NSSize::new(14.0, 14.0));
                text.setVerticallyResizable(true);
                text.setHorizontallyResizable(false);
                text.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
                if let Some(tc) = unsafe { text.textContainer() } {
                    tc.setWidthTracksTextView(true);
                }
                unsafe {
                    text.setAutomaticQuoteSubstitutionEnabled(false);
                    text.setAutomaticDashSubstitutionEnabled(false);
                }
                text.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(w, (ch - status_h).max(0.0))));
                scroll.setDocumentView(Some(&text));
                content.addSubview(&scroll);
                self.editor = Some(text.clone());
                if self.kind == ToolKind::PrettyPrint {
                    let status = NSTextField::labelWithString(&NSString::from_str(""), mtm);
                    status.setFont(Some(&NSFont::systemFontOfSize(11.0)));
                    status.setTextColor(Some(&colors.dim.to_nscolor()));
                    status.setFrame(NSRect::new(NSPoint::new(14.0, 1.0), NSSize::new(w - 28.0, 16.0)));
                    status.setAutoresizingMask(
                        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMaxYMargin,
                    );
                    content.addSubview(&status);
                    self.pp_status_label = Some(status.clone());
                    let handler = pp_handler::PrettyPrintHandler::new(
                        mtm,
                        text.clone(),
                        status,
                        self.spec.save_dir.clone(),
                    );
                    self.pp_handler = Some(handler);
                }
            }
            self.colors = Some(colors);

            if filefast {
                if let Some(field) = self.name_field.clone() {
                    let paste = self.paste.clone().unwrap();
                    let panel_ref = panel.clone();
                    let script = self.spec.script.clone();
                    let dir = self.filefast_dir();
                    let out = self.ff_out.clone();
                    let pending = self.ff_pending.clone();
                    let monitor = crate::ui::popup::install_local_monitor(mtm, move |event| {
                        // This panel's events only (not `isKeyWindow`: opened from
                        // the palette it may not be key yet, and Tab then died).
                        let ours = event.window(mtm).is_some_and(|w| std::ptr::eq(&*w as *const _ as *const u8, &*panel_ref as *const _ as *const u8));
                        if !ours {
                            return false;
                        }
                        let key = crate::ui::popup::key_input_from_event(event);
                        if key.key_code == 48 && !key.ctrl {
                            // Tab flips between the name and the paste box.
                            let in_paste = panel_ref
                                .firstResponder()
                                .is_some_and(|r| std::ptr::eq(&*r as *const _ as *const u8, &*paste as *const _ as *const u8));
                            if in_paste {
                                let r: &objc2_app_kit::NSResponder = &field;
                                panel_ref.makeFirstResponder(Some(r));
                            } else {
                                let r: &objc2_app_kit::NSResponder = &paste;
                                panel_ref.makeFirstResponder(Some(r));
                            }
                            return true;
                        }
                        if key.key_code == crate::ui::popup::KEY_ESC {
                            panel_ref.orderOut(None);
                            return true;
                        }
                        if key.key_code == crate::ui::popup::KEY_RETURN && !key.shift {
                            let name = field.stringValue().to_string().trim().to_string();
                            let contents = paste.string().to_string();
                            if name.is_empty() || name.contains('/') || contents.is_empty() {
                                crate::platform::bell();
                                return true;
                            }
                            let _ = spawn_filefast(
                                script.clone(),
                                dir.clone(),
                                name,
                                contents,
                                out.clone(),
                                pending.clone(),
                            );
                            return true;
                        }
                        false
                    });
                    self.key_monitor = Some(monitor);
                }
                if let (Some(cell), Some(field_paste)) = (self.ff_cell.clone(), self.paste.clone()) {
                    let panel_ref = panel.clone();
                    let copy_btn = self.ff_copy_btn.clone();
                    let name_field = self.name_field.clone();
                    let status = self.ff_status_label.clone();
                    let save_dir = self.spec.save_dir.clone();
                    let ff_script = self.spec.script.clone();
                    let ff_out = self.ff_out.clone();
                    let ff_pending = self.ff_pending.clone();
                    let monitor = crate::ui::popup::install_local_monitor_mask(
                        mtm,
                        objc2_app_kit::NSEventMask::LeftMouseDown,
                        move |event| {
                            let ours = event.window(mtm).is_some_and(|w| std::ptr::eq(&*w as *const _ as *const u8, &*panel_ref as *const _ as *const u8));
                            if !ours {
                                return false;
                            }
                            if let (Some(btn), Some(nf)) = (&copy_btn, &name_field) {
                                let bp = btn.convertPoint_fromView(event.locationInWindow(), None);
                                let bb = btn.bounds();
                                if bp.x >= 0.0 && bp.y >= 0.0 && bp.x <= bb.size.width && bp.y <= bb.size.height {
                                    let name = nf.stringValue().to_string().trim().to_string();
                                    let contents = field_paste.string().to_string();
                                    if name.is_empty() || name.contains('/') || contents.is_empty() {
                                        crate::platform::bell();
                                        if let Some(l) = &status {
                                            set_status_label(l, "Enter a name and paste text first", true);
                                        }
                                    } else if !spawn_filefast(
                                        ff_script.clone(),
                                        filefast_dir_for(&save_dir),
                                        name,
                                        contents,
                                        ff_out.clone(),
                                        ff_pending.clone(),
                                    ) {
                                        // Nothing written, so nothing copied.
                                        crate::platform::bell();
                                        if let Some(l) = &status {
                                            set_status_label(l, "Could not create file", true);
                                        }
                                    }
                                    // Same write path as Return: the script
                                    // writes the file (overwriting the same
                                    // path, so a second click never makes a
                                    // duplicate); `poll` copies the path only
                                    // on success and shows the error otherwise.
                                    return true;
                                }
                            }
                            let p = cell.convertPoint_fromView(event.locationInWindow(), None);
                            let b = cell.bounds();
                            if p.x >= 0.0 && p.y >= 0.0 && p.x <= b.size.width && p.y <= b.size.height {
                                let r: &objc2_app_kit::NSResponder = &field_paste;
                                panel_ref.makeKeyWindow();
                                panel_ref.makeFirstResponder(Some(r));
                                return true;
                            }
                            false
                        },
                    );
                    self.click_monitor = Some(monitor);
                }
            }

            panel.setContentView(Some(&frame.root));
            self.window = Some(panel);
        }
        if let Some(panel) = &self.window {
            let vf = NSScreen::mainScreen(mtm)
                .map(|s| s.visibleFrame())
                .unwrap_or(NSRect::new(
                    NSPoint::new(0.0, 0.0),
                    NSSize::new(1440.0, 900.0),
                ));
            let f = panel.frame();
            let x = vf.origin.x + (vf.size.width - f.size.width) / 2.0;
            let y = vf.origin.y + (vf.size.height - f.size.height) / 2.0 + vf.size.height * 0.06;
            panel.setFrameOrigin(NSPoint::new(x, y));
            panel.orderFrontRegardless();
            panel.makeKeyWindow();
            if self.kind == ToolKind::Filefast {
                // A fresh open starts clean: no leftover name, pasted text,
                // status or result from the previous Create File.
                if let Some(field) = &self.name_field {
                    field.setStringValue(&NSString::from_str(""));
                }
                if let Some(paste) = &self.paste {
                    paste.setString(&NSString::from_str(""));
                }
                *self.ff_out.lock().unwrap() = None;
                self.ff_hide_at = None;
                self.ff_status = None;
                if let Some(label) = &self.ff_status_label {
                    label.setStringValue(&NSString::from_str(""));
                }
                if let Some(field) = &self.name_field {
                    let responder: &objc2_app_kit::NSResponder = &**field;
                    panel.makeFirstResponder(Some(responder));
                }
            }
            if self.kind == ToolKind::TextTable {
                if let Some(editor) = &self.editor {
                    // Fresh clipboard text (e.g. Hyper+X Copy Text since the
                    // last open) replaces the input; an unchanged clipboard
                    // keeps the user's edits.
                    if let Some(clip) = crate::platform::get_text() {
                        if !clip.trim().is_empty() && clip != self.tt_clip_loaded {
                            editor.setString(&NSString::from_str(&clip));
                            self.tt_clip_loaded = clip;
                        }
                    }
                    let responder: &objc2_app_kit::NSResponder = &**editor;
                    panel.makeFirstResponder(Some(responder));
                }
                self.tt_hide_at = None;
            }
            let pf = panel.frame();
            self.frame = [pf.origin.x, pf.origin.y, pf.size.width, pf.size.height];
        }
        if self.kind == ToolKind::HealthChecks {
            self.run_health();
        }
    }

    #[cfg(target_os = "macos")]
    fn poll_macos(&mut self) {
        let click = self.click.replace(0);
        if click != 0 {
            self.header_click(click);
        }
        match self.kind {
            ToolKind::HealthChecks => {
                // Streaming: render the buffer's ANSI colors as attributes.
                let text = self.health_out.lock().unwrap().clone();
                let text = if text.is_empty() && !self.health_done.load(std::sync::atomic::Ordering::Relaxed) {
                    "running health-checks…".to_string()
                } else {
                    text
                };
                if text != self.health_shown {
                    self.health_shown = text.clone();
                    if let (Some(editor), Some(c)) = (&self.editor, self.colors) {
                        let font = editor.font().unwrap_or_else(|| objc2_app_kit::NSFont::systemFontOfSize(13.0));
                        let attr = crate::ui::ansi_text::attributed(&text, &font, &c);
                        if let Some(ts) = unsafe { editor.textStorage() } {
                            ts.setAttributedString(&attr);
                        }
                        { use objc2_app_kit::NSStandardKeyBindingResponding; unsafe { editor.scrollToEndOfDocument(None) }; }
                    }
                }
            }
            ToolKind::PrettyPrint => {
                // Debounce: capture the edit, then format 0.35 s after it
                // settles (`onEditorTextChange` + the work item).
                let editor = self.editor.clone();
                if let Some(editor) = &editor {
                    let raw = editor.string().to_string();
                    if raw != self.pp_last_seen {
                        self.pp_last_seen = raw.clone();
                        self.pp_last_change = Some(std::time::Instant::now());
                    }
                    let settled = self
                        .pp_last_change
                        .map(|t| t.elapsed() >= std::time::Duration::from_millis(350))
                        .unwrap_or(false);
                    if settled
                        && !self.fmt_pending.load(std::sync::atomic::Ordering::Relaxed)
                        && !raw.trim().is_empty()
                    {
                        self.pp_last_change = None;
                        self.format_prettyprint();
                    }
                }
                // Apply a completed format.
                let done = self.fmt_out.lock().unwrap().take();
                match done {
                    Some(Ok(Some(formatted))) => {
                        if let Some(editor) = &self.editor {
                            if editor.string().to_string() != formatted {
                                editor.setString(&objc2_foundation::NSString::from_str(&formatted));
                            }
                            // Track what the editor actually holds so the
                            // debounce never sees this write as a fresh edit.
                            self.pp_last_seen = editor.string().to_string();
                        } else {
                            self.pp_last_seen = formatted;
                        }
                        self.set_pp_status(None);
                    }
                    Some(Ok(None)) => self.set_pp_status(None),
                    Some(Err(e)) => self.set_pp_status(Some((e, true))),
                    None => {}
                }
            }
            // `poll_table` (every platform) does the work.
            ToolKind::TextTable => {}
            ToolKind::Filefast => {
                // `FileFastPasteView.refreshIndicator`.
                if let (Some(paste), Some(hint), Some(count), Some(c)) =
                    (&self.paste, &self.ff_hint, &self.ff_count, self.colors)
                {
                    let s = paste.string().to_string();
                    // The pasted text is invisible, so the caret trails it (off
                    // the bottom / out of the box on a big paste): park the
                    // view at the top and show the caret only while empty.
                    if let Some(sv) = paste.enclosingScrollView() {
                        let o = sv.contentView().bounds().origin;
                        if o.x != 0.0 || o.y != 0.0 {
                            sv.contentView().scrollToPoint(objc2_foundation::NSPoint::new(0.0, 0.0));
                            sv.reflectScrolledClipView(&sv.contentView());
                        }
                    }
                    paste.setInsertionPointColor(Some(&(if s.is_empty() { c.dim } else { crate::ui::theme::Rgba::new(0.0, 0.0, 0.0, 0.0) }).to_nscolor()));
                    let want = if s.is_empty() { "paste output".to_string() } else { "************************".to_string() };
                    let counted = pasted_count(&s);
                    if count.stringValue().to_string() != counted {
                        count.setStringValue(&objc2_foundation::NSString::from_str(&counted));
                    }
                    if hint.stringValue().to_string() != want {
                        hint.setStringValue(&objc2_foundation::NSString::from_str(&want));
                        hint.setTextColor(Some(
                            &(if s.is_empty() { c.dim } else { c.tone(crate::ui::theme::PopupTone::Success) }).to_nscolor(),
                        ));
                    }
                }
                let done = self.ff_out.lock().unwrap().take();
                if let Some((ok, last)) = done {
                    // Confirm from the file on disk, not the script's word.
                    let path = last.trim().to_string();
                    let on_disk = if ok && !path.is_empty() { std::fs::read_to_string(&path).ok() } else { None };
                    if let Some(written) = on_disk {
                        copy_text(&path);
                        let msg = format!("Saved {} → {path} · path copied", pasted_count(&written).trim_end_matches(" pasted"));
                        self.set_ff_status(Some((msg, false)));
                        self.ff_hide_at =
                            Some(std::time::Instant::now() + std::time::Duration::from_millis(2000));
                    } else if ok {
                        crate::platform::bell();
                        let msg = if path.is_empty() { "file was not created".to_string() } else { format!("file was not created: {path}") };
                        self.set_ff_status(Some((msg, true)));
                    } else {
                        crate::platform::bell();
                        self.set_ff_status(Some((last, true)));
                    }
                }
                if let Some(at) = self.ff_hide_at {
                    if std::time::Instant::now() >= at {
                        self.ff_hide_at = None;
                        self.hide();
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_names_round_trip() {
        for kind in [ToolKind::PrettyPrint, ToolKind::Filefast, ToolKind::HealthChecks, ToolKind::TextTable] {
            assert_eq!(ToolKind::from_name(kind.name()), Some(kind));
        }
        assert!(ToolKind::is_panel("prettyprint"));
        assert!(ToolKind::is_panel("health"));
        assert!(!ToolKind::is_panel("paths"));
    }

    #[test]
    fn section_vars_reads_the_section() {
        let text = "[app]\nshared-window = true\n\n[prettyprint]\nsave-dir = \"/tmp/\"\nwidth = 1000\n";
        let vars = section_vars(text, "prettyprint");
        assert_eq!(vars.get("save-dir").map(String::as_str), Some("/tmp/"));
        assert_eq!(vars.get("width").map(String::as_str), Some("1000"));
        assert!(section_vars(text, "filefast").is_empty());
    }

    /// The Create File script (shared by Return and the "Copy file path"
    /// button) writes the pasted text to the path it prints, and a repeat run
    /// reuses that path rather than making a second file.
    #[test]
    fn filefast_script_writes_file_at_printed_path_idempotently() {
        let toml = include_str!("../../../../commands.toml");
        let script = section_vars(toml, "filefast")["script"].replace("| pbcopy", "> /dev/null");
        let dir = std::env::temp_dir().join(format!("ks-ff-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let run = |body: &str| {
            use std::io::Write;
            let mut child = std::process::Command::new("/bin/bash")
                .args(["-c", &script])
                .env("FF_DIR", dir.to_str().unwrap())
                .env("FF_NAME", "note.txt")
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            child.stdin.take().unwrap().write_all(body.as_bytes()).unwrap();
            let o = child.wait_with_output().unwrap();
            assert!(o.status.success());
            String::from_utf8(o.stdout).unwrap()
        };
        let path = run("hello");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");
        assert_eq!(run("hello2"), path);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello2");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn section_vars_unescapes_the_filefast_script() {
        let text = "[filefast]\nscript = \"mkdir -p \\\"$FF_DIR\\\" && printf %s \\\"$FF_DIR/$FF_NAME\\\"\"\n";
        let v = section_vars(text, "filefast");
        assert_eq!(v["script"], "mkdir -p \"$FF_DIR\" && printf %s \"$FF_DIR/$FF_NAME\"");
    }

    #[test]
    fn tool_spec_defaults_match_swift() {
        assert_eq!(ToolSpec::defaults(ToolKind::PrettyPrint).width, 1000.0);
        assert_eq!(ToolSpec::defaults(ToolKind::HealthChecks).height, 640.0);
        assert_eq!(ToolSpec::defaults(ToolKind::Filefast).width, 620.0);
    }

    #[test]
    fn filefast_dir_is_date_stamped_under_the_root() {
        let mut p = ToolPanel::new(ToolKind::Filefast, &AppSettings::default());
        p.spec.save_dir = "/tmp/".to_string();
        let dir = p.filefast_dir();
        assert!(dir.starts_with("/tmp/filefast/"), "{dir}");
        assert_eq!(dir.len(), "/tmp/filefast/2026_01_01".len());
    }

    #[test]
    fn format_text_handles_json_xml_and_other() {
        assert_eq!(ToolPanel::format_text("plain").unwrap(), None);
        assert_eq!(ToolPanel::format_text("").unwrap(), None);
        if std::path::Path::new("/opt/homebrew/bin/jq").exists()
            || std::path::Path::new("/usr/local/bin/jq").exists()
            || which("jq")
        {
            let out = ToolPanel::format_text("{\"a\":1}").unwrap().unwrap();
            assert!(out.contains("\"a\": 1"), "{out}");
        }
    }

    fn which(bin: &str) -> bool {
        std::env::var("PATH")
            .unwrap_or_default()
            .split(':')
            .any(|p| std::path::Path::new(p).join(bin).exists())
    }

    #[test]
    fn civil_from_days_epoch_and_known_date() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
    }

    #[test]
    fn pretty_ext_picks_by_first_non_space_char() {
        assert_eq!(pretty_ext("{\"a\":1}"), "json");
        assert_eq!(pretty_ext("[1,2]"), "json");
        assert_eq!(pretty_ext("<a/>"), "xml");
        assert_eq!(pretty_ext("  \n<a/>"), "xml");
        assert_eq!(pretty_ext("hello"), "txt");
        assert_eq!(pretty_ext("   "), "txt");
    }

    #[test]
    fn pretty_stamp_is_yyyy_mmdd_hhmmss() {
        assert_eq!(pretty_stamp(0), "19700101-000000");
        assert_eq!(pretty_stamp(1_704_067_200), "20240101-000000");
        assert_eq!(pretty_stamp(1_704_067_200 + 3661), "20240101-010101");
    }

    #[test]
    fn pretty_save_path_uses_dir_and_ext() {
        assert_eq!(
            pretty_save_path("/tmp/", "{\"a\":1}", 1_704_067_200),
            "/tmp/prettyprint-20240101-000000.json"
        );
        assert_eq!(
            pretty_save_path("/tmp", "<a/>", 0),
            "/tmp/prettyprint-19700101-000000.xml"
        );
    }

    #[test]
    fn save_prettyprint_at_writes_the_file() {
        let dir = std::env::temp_dir().join(format!("ws-pretty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = save_prettyprint_at("{\"a\":1}", dir.to_str().unwrap(), 0).unwrap();
        assert!(path.ends_with("prettyprint-19700101-000000.json"), "{path}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"a\":1}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn snapshot_changes_on_edit_and_close() {
        let mut p = ToolPanel::new(ToolKind::PrettyPrint, &AppSettings::default());
        let before = p.snapshot();
        p.handle(ToolsEvent::Edit("{\"a\":1}".into()));
        let after = p.snapshot();
        assert_ne!(before, after, "edit must change the snapshot");
        assert_eq!(after.text, "{\"a\":1}");
        assert_eq!(after.kind, ToolKind::PrettyPrint);
        assert!(!after.shown);

        p.shown = true;
        let shown = p.snapshot();
        p.handle(ToolsEvent::Close);
        assert_ne!(shown, p.snapshot(), "close must change the snapshot");
        assert!(!p.snapshot().shown);
    }

    #[test]
    fn text_table_markdown_path_and_twin() {
        assert_eq!(table_md_path("/tmp/text-table/", 0), "/tmp/text-table/table-19700101-000000.md");
        let rich = TableResult { mime: "text/html".into(), output: "<table/>".into(), fallback: "| a |".into(), ..TableResult::default() };
        assert_eq!(rich.markdown(), "| a |");
        let plain = TableResult { mime: "text/plain".into(), output: "| b |".into(), ..TableResult::default() };
        assert_eq!(plain.markdown(), "| b |");
    }

    #[test]
    fn text_table_summary_line() {
        let r = TableResult {
            ok: true,
            columns: 3,
            rows: 1,
            delimiter: "tab".into(),
            warnings: vec!["1 short row padded".into()],
            ..TableResult::default()
        };
        assert_eq!(r.summary(), "3 columns × 1 row · delimiter tab · 1 short row padded");
        assert!(TableResult::default().summary().starts_with("no table found"));
    }

    /// The real engine over temp files (skips without a 3.11+ python).
    #[test]
    fn run_text_table_end_to_end() {
        let lib = format!("{}/../../pylib", env!("CARGO_MANIFEST_DIR"));
        crate::app::python_helper::PythonHelper::shared().configure(&lib);
        if crate::app::python_helper::python_bin(&lib).is_none() {
            return;
        }
        let r = run_text_table("Name\tPoints\nA\t5\n", "outlook").unwrap();
        assert!(r.ok && r.is_html(), "{r:?}");
        assert!(r.output.contains("<table"), "{}", r.output);
        assert!(r.fallback.starts_with("| Name | Points |"), "{}", r.fallback);
        assert_eq!((r.columns, r.rows, r.delimiter.as_str()), (2, 1, "tab"));
        let md = run_text_table("a,b\n1,2\n", "markdown").unwrap();
        assert_eq!(md.output, md.fallback);
        assert!(!run_text_table("\n", "markdown").unwrap().ok);
    }

    #[test]
    fn health_snapshot_reports_running() {
        let p = ToolPanel::new(ToolKind::HealthChecks, &AppSettings::default());
        let snap = p.snapshot();
        assert_eq!(snap.kind, ToolKind::HealthChecks);
        assert!(snap.running, "a fresh health panel has not finished a run");
        assert!(snap.text.is_empty());
    }
}
