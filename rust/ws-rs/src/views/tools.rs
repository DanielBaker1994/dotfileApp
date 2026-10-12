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

/// The three panels `openTool` maps onto this module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolKind {
    PrettyPrint,
    Filefast,
    HealthChecks,
}

impl ToolKind {
    /// The `config.name` the panel registers under (the `tools` map key).
    pub fn name(self) -> &'static str {
        match self {
            ToolKind::PrettyPrint => "prettyprint",
            ToolKind::Filefast => "filefast",
            ToolKind::HealthChecks => "health-checks",
        }
    }

    pub fn from_name(name: &str) -> Option<ToolKind> {
        match name {
            "prettyprint" => Some(ToolKind::PrettyPrint),
            "filefast" => Some(ToolKind::Filefast),
            "health-checks" | "health" => Some(ToolKind::HealthChecks),
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
            },
            ToolKind::Filefast => ToolSpec {
                script: String::new(),
                save_dir: "/tmp/filefast".to_string(),
                width: 620.0,
                height: 180.0,
                font: String::new(),
            },
            ToolKind::HealthChecks => ToolSpec {
                script: String::new(),
                save_dir: String::new(),
                width: 800.0,
                height: 640.0,
                font: "SF Mono".to_string(),
            },
        }
    }
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
                let v = v.trim().trim_matches('"').trim().to_string();
                out.entry(k).or_insert(v);
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
        let _ = tri(vars.get("enabled"));
        spec
    }
}

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
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
    use objc2_foundation::NSString;
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    let _ = unsafe { pb.setString_forType(&NSString::from_str(text), NSPasteboardTypeString) };
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
    #[cfg(target_os = "macos")]
    window: Option<Retained<crate::ui::popup::PopupPanel>>,
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
    /// The last header click (`onHeaderButton`), drained by `poll`.
    click: std::rc::Rc<std::cell::Cell<i32>>,
    #[cfg(target_os = "macos")]
    chrome: Option<Retained<crate::ui::chrome::PopupChrome>>,
    #[cfg(target_os = "macos")]
    ff_hint: Option<Retained<objc2_app_kit::NSTextField>>,
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
            #[cfg(target_os = "macos")]
            window: None,
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
            click: std::rc::Rc::new(std::cell::Cell::new(0)),
            #[cfg(target_os = "macos")]
            chrome: None,
            #[cfg(target_os = "macos")]
            ff_hint: None,
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
    pub fn show(&mut self, mtm: objc2::MainThreadMarker) {
        #[cfg(target_os = "macos")]
        self.show_macos(mtm);
        #[cfg(not(target_os = "macos"))]
        let _ = mtm;
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
        st
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
        use std::time::{SystemTime, UNIX_EPOCH};
        let root = if self.spec.save_dir == "/tmp/" {
            "/tmp/filefast".to_string()
        } else {
            crate::app::config::expand_tilde(&self.spec.save_dir)
        };
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        // yyyy_MM_dd (UTC; the exact stamp is not part of the contract).
        let days = secs / 86_400;
        let (y, m, d) = civil_from_days(days as i64);
        format!("{root}/{y:04}_{m:02}_{d:02}")
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
            let mut cmd = Command::new("/bin/bash");
            cmd.arg("-c")
                .arg(&script)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
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
        }
    }

    /// A header click: ✕ hides, the icon pops the app menu, 10/11 are the
    /// panel's buttons (save / copy, copy output / copy config path).
    #[cfg(target_os = "macos")]
    fn header_click(&mut self, id: i32) {
        use crate::ui::chrome::{HEADER_CLOSE_ID, HEADER_ICON_ID};
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
            let h = if filefast { 46.0 } else { self.spec.height.max(220.0) };
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
                panel.setContentMinSize(NSSize::new(320.0, 46.0));
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
                name_box.setFrame(NSRect::new(NSPoint::new(10.0, 9.0), NSSize::new(fw - 10.0, 28.0)));
                content.addSubview(&name_box);
                self.name_field = Some(field);
                let px = fw + 8.0;
                let scroll = NSScrollView::new(mtm);
                scroll.setFrame(NSRect::new(NSPoint::new(px, 9.0), NSSize::new(w - px - 10.0, 28.0)));
                scroll.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
                scroll.setHasVerticalScroller(false);
                scroll.setDrawsBackground(false);
                scroll.setWantsLayer(true);
                if let Some(l) = scroll.layer() {
                    l.setBackgroundColor(Some(&crate::ui::style::input_fill(&colors).to_nscolor().CGColor()));
                    l.setCornerRadius(6.0);
                    l.setBorderWidth(1.0);
                    l.setBorderColor(Some(&crate::ui::style::input_stroke(&colors).to_nscolor().CGColor()));
                }
                let text = NSTextView::new(mtm);
                text.setRichText(false);
                text.setDrawsBackground(false);
                text.setFont(Some(&NSFont::systemFontOfSize(13.0)));
                // The pasted text stays invisible; the hint reports it.
                text.setTextColor(Some(&objc2_app_kit::NSColor::clearColor()));
                text.setInsertionPointColor(Some(&colors.dim.to_nscolor()));
                text.setTextContainerInset(NSSize::new(6.0, 5.0));
                text.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(w - px - 10.0, 28.0)));
                scroll.setDocumentView(Some(&text));
                let hint = NSTextField::labelWithString(&NSString::from_str("paste output"), mtm);
                hint.setFont(Some(&NSFont::systemFontOfSize(13.0)));
                hint.setTextColor(Some(&colors.dim.to_nscolor()));
                hint.setFrame(NSRect::new(NSPoint::new(10.0, 5.0), NSSize::new(w - px - 30.0, 18.0)));
                scroll.addSubview(&hint);
                content.addSubview(&scroll);
                self.paste = Some(text.clone());
                self.ff_hint = Some(hint.clone());
                // Status: a centered toast-like label over the row.
                let status = NSTextField::labelWithString(&NSString::from_str(""), mtm);
                status.setFont(Some(&NSFont::systemFontOfSize(11.0)));
                status.setTextColor(Some(&colors.dim.to_nscolor()));
                status.setFrame(NSRect::new(NSPoint::new(10.0, 0.0), NSSize::new(w - 20.0, 9.0)));
                content.addSubview(&status);
                self.ff_status_label = Some(status);
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
                        if !panel_ref.isKeyWindow() {
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
                if let Some(field) = &self.name_field {
                    let responder: &objc2_app_kit::NSResponder = &**field;
                    panel.makeFirstResponder(Some(responder));
                }
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
            ToolKind::Filefast => {
                // `FileFastPasteView.refreshIndicator`.
                if let (Some(paste), Some(hint), Some(c)) = (&self.paste, &self.ff_hint, self.colors) {
                    let s = paste.string().to_string();
                    let want = if s.is_empty() {
                        "paste output".to_string()
                    } else {
                        let lines = s.split('\n').count();
                        format!(
                            "************************  {lines} line{}, {} chars pasted",
                            if lines == 1 { "" } else { "s" },
                            s.chars().count()
                        )
                    };
                    if hint.stringValue().to_string() != want {
                        hint.setStringValue(&objc2_foundation::NSString::from_str(&want));
                        hint.setTextColor(Some(
                            &(if s.is_empty() { c.dim } else { c.tone(crate::ui::theme::PopupTone::Success) }).to_nscolor(),
                        ));
                    }
                }
                let done = self.ff_out.lock().unwrap().take();
                if let Some((ok, last)) = done {
                    if ok {
                        let path = last.trim().to_string();
                        if !path.is_empty() {
                            copy_text(&path);
                        }
                        let msg = if path.is_empty() {
                            "saved".to_string()
                        } else {
                            format!("Copied {path} to clipboard")
                        };
                        self.set_ff_status(Some((msg, false)));
                        self.ff_hide_at =
                            Some(std::time::Instant::now() + std::time::Duration::from_millis(1300));
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
        for kind in [ToolKind::PrettyPrint, ToolKind::Filefast, ToolKind::HealthChecks] {
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
}
